//! P4/P5 backend end to end (docs/11): review comments -> agent, fan-out
//! groups, gates. Real `podd`, `pod-holder` and `pod` binaries; the agents
//! are `shell` (PowerShell) and the `fake-agent` example, which reports OSC
//! 7501 states and records the prompts it receives. No real Claude/Codex.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use common::*;
use podd::client::{Client, StatusStreamEvent, SubscribeEvent};
use podd::protocol::*;

// ───────────────────────────── helpers ─────────────────────────────

fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

/// The same single cargo invocation as tests/status.rs (same feature
/// resolution, so nothing is re-linked under running tests).
fn workspace_bin(rel: &str) -> PathBuf {
    static BUILT: Mutex<bool> = Mutex::new(false);
    let mut built = BUILT.lock().unwrap();
    let dir = profile_dir();
    if !*built {
        let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.args([
            "build",
            "-p",
            "podd",
            "--example",
            "fake-agent",
            "-p",
            "pod-hook",
            "-p",
            "pod-cli",
            // Target flags apply to every -p: name the binaries too, or
            // only the example is built (stale/missing pod.exe, pod-hook.exe).
            "--bin",
            "pod",
            "--bin",
            "pod-hook",
        ]);
        if dir.file_name().and_then(|n| n.to_str()) == Some("release") {
            cmd.arg("--release");
        }
        assert!(cmd.status().expect("cargo build").success());
        *built = true;
    }
    let p = dir.join(rel);
    assert!(p.exists(), "{} missing", p.display());
    p
}

fn fake_agent() -> String {
    workspace_bin("examples/fake-agent.exe")
        .display()
        .to_string()
}

fn pod_exe() -> PathBuf {
    workspace_bin("pod.exe")
}

struct TempRepo {
    root: PathBuf,
    path: PathBuf,
}

impl TempRepo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("orch-repo"));
        let path = root.join("pod repo");
        std::fs::create_dir_all(path.join("src")).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git(&path, &["config", k, v]);
        }
        std::fs::write(
            path.join("src").join("lib.rs"),
            "fn one() {}\nfn two() {\n    let x = `tick`;\n}\nfn five() {}\n",
        )
        .unwrap();
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "init"]);
        Self { root, path }
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn add_repo(c: &mut Client, repo: &TempRepo) {
    c.call::<RepoAdd>(&RepoAddParams {
        path: repo.path.display().to_string(),
        name: Some("demo".into()),
    })
    .expect("repo.add");
}

fn create_task(c: &mut Client, name: &str, argv: Vec<String>) -> TaskInfo {
    c.call::<TaskCreate>(&TaskCreateParams {
        repo: "demo".into(),
        name: name.into(),
        agent: Some(AgentSpec::Argv { argv }),
        ..Default::default()
    })
    .expect("task.create")
}

fn wait_state(c: &mut Client, id: &str, state: &str) -> StatusInfo {
    let r = c
        .call::<StatusWait>(&StatusWaitParams {
            target: TerminalRef::parse(id),
            status: Some(state.into()),
            turn_end: false,
            timeout_ms: Some(30_000),
        })
        .expect("status.wait");
    assert_eq!(
        r.outcome,
        WaitOutcome::Matched,
        "{id} never became {state}: {:?}",
        r.status
    );
    r.status
}

fn until<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn comments(c: &mut Client, task: &str) -> Vec<ReviewComment> {
    c.call::<ReviewCommentsList>(&ReviewListParams {
        task: Some(task.into()),
        state: None,
    })
    .expect("review.comments.list")
    .comments
}

fn delivery(c: &mut Client, id: &str) -> DeliveryInfo {
    c.call::<ReviewDeliveries>(&ReviewDeliveriesParams::default())
        .unwrap()
        .deliveries
        .into_iter()
        .find(|d| d.id == id)
        .expect("delivery listed")
}

fn pod(pipe: &str, args: &[&str]) -> std::process::Output {
    Command::new(pod_exe())
        .arg("--pipe")
        .arg(pipe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run pod")
}

// ───────────────────────────── P4: review ─────────────────────────────

/// Comments survive a daemon restart; `review.send` is queued while the
/// agent works and delivered exactly once, intact, when it turns idle; it is
/// refused while an agent is blocked.
#[test]
fn review_comments_persist_queue_and_deliver_once() {
    let repo = TempRepo::new();
    let d = Daemon::start("review");
    let mut c = d.client();
    add_repo(&mut c, &repo);

    let go = repo.root.join("go.flag");
    let log = repo.root.join("received.log");
    let t = create_task(
        &mut c,
        "rev",
        vec![
            fake_agent(),
            "osc:state=working".into(),
            format!("wait-file:{}", go.display()),
            format!("paste-loop:{}", log.display()),
        ],
    );
    let term = t.terminals[0].id.clone();
    wait_state(&mut c, &term, "working");

    // The agent "changed" a line in the worktree after the base.
    let wt = PathBuf::from(&t.worktree);
    std::fs::write(
        wt.join("src").join("lib.rs"),
        "fn one() {}\nfn two() {\n    let y = `tick`;\n}\nfn five() {}\n",
    )
    .unwrap();

    // Add: a range on the new side, an old-side comment using the UI export
    // shape (`path`), and one with explicit code.
    let added: ReviewCommentsResult = serde_json::from_value(
        c.call_raw(
            ReviewCommentsAdd::NAME,
            serde_json::json!({
                "task": "rev",
                "comments": [
                    {"file": "src\\lib.rs", "side": "new", "line": 2, "end_line": 4,
                     "body": "Why rename x to y?\nKeep the old name."},
                    {"path": "src/lib.rs", "side": "old", "line": 3, "body": "This was fine — keep it"},
                    {"file": "README.md", "line": 1, "body": "Say more", "code": "hello\r\n"},
                ]
            }),
        )
        .expect("review.comments.add"),
    )
    .unwrap();
    let ids: Vec<String> = added.comments.iter().map(|c| c.id.clone()).collect();
    assert_eq!(ids, ["c1", "c2", "c3"]);
    assert_eq!(added.comments[0].file, "src/lib.rs");
    assert_eq!(
        added.comments[0].code.as_deref(),
        Some("fn two() {\n    let y = `tick`;\n}")
    );
    assert_eq!(added.comments[1].side, Side::Old);
    assert_eq!(
        added.comments[1].code.as_deref(),
        Some("    let x = `tick`;")
    );
    assert_eq!(added.comments[2].code.as_deref(), Some("hello"));
    // Bad input.
    let bad = c.call::<ReviewCommentsAdd>(&ReviewAddParams {
        task: "rev".into(),
        comments: vec![NewComment {
            file: "../escape.rs".into(),
            line: 1,
            body: "x".into(),
            ..Default::default()
        }],
    });
    assert_eq!(rpc_code(bad), codes::INVALID_PARAMS);

    // ── daemon restart: comments are persisted ──
    let (pipe, dir) = d.stop(false);
    let d = Daemon::start_on(pipe, dir);
    let mut c = d.client();
    let back = comments(&mut c, "demo/rev");
    assert_eq!(back, added.comments, "comments changed across a restart");
    let ti = c.call::<TaskShow>(&TaskRef { task: "rev".into() }).unwrap();
    assert_eq!(ti.review.as_ref().unwrap().open, 3);
    // The agent survived and still reports working (OSC replay).
    wait_state(&mut c, &term, "working");

    // Dry run builds the prompt without delivering.
    let preview = c
        .call::<ReviewSend>(&ReviewSendParams {
            task: "rev".into(),
            all_open: true,
            dry_run: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(preview.state, DeliveryState::Preview);

    // ── queued while working ──
    let instructions = "Fix these, then run the tests.\nDo not touch README.md beyond the comment.";
    let sent = c
        .call::<ReviewSend>(&ReviewSendParams {
            task: "demo/rev".into(),
            all_open: true,
            instructions: Some(instructions.into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(sent.state, DeliveryState::Queued, "{sent:?}");
    assert_eq!(sent.agent_state, "working");
    assert_eq!(sent.comment_ids, ["c3", "c1", "c2"]); // README.md < src/lib.rs, then by line
    assert!(
        comments(&mut c, "rev")
            .iter()
            .all(|x| x.state == CommentState::Queued)
    );
    let p = &sent.prompt;
    assert!(p.contains("Instructions from the reviewer:\nFix these, then run the tests.\n"));
    assert!(p.contains("\n2. src/lib.rs:2-4\n```\n   2 | fn two() {\n   3 |     let y = `tick`;\n   4 | }\n```\nReviewer: Why rename x to y?\nKeep the old name.\n"), "{p}");
    assert!(
        p.contains("\n3. src/lib.rs:3 (base version, before your changes)\n"),
        "{p}"
    );
    assert!(p.contains("Reviewer: This was fine — keep it\n"), "{p}");
    // orch-quality-06: no waiting here; a delivery while the agent works
    // would record `delivered_state: working` (checked below).
    assert!(!log.exists(), "delivered while the agent was working");
    assert_eq!(delivery(&mut c, &sent.id).state, DeliveryState::Queued);

    // ── the agent becomes idle: delivered exactly once ──
    std::fs::write(&go, "go").unwrap();
    let dl = until("delivery", 30, || {
        let x = delivery(&mut c, &sent.id);
        (x.state == DeliveryState::Delivered).then_some(x)
    });
    assert_eq!(
        dl.delivered_state.as_deref(),
        Some("idle"),
        "delivered while the agent was working"
    );
    assert_eq!(
        dl.app_bracketed_paste,
        Some(true),
        "DECSET 2004 seen by the VT mirror"
    );
    let text = until("the agent to record the prompt", 30, || {
        std::fs::read_to_string(&log)
            .ok()
            .filter(|s| s.contains("<<<END"))
    });
    // The agent finished its turn and the Enter check settled: nothing
    // types into it again (the delivery is no longer queued).
    wait_output(&mut c, &term, "done");
    until("the submit check to settle", 30, || {
        let r = c
            .call::<ReviewDeliveries>(&ReviewDeliveriesParams::default())
            .unwrap();
        r.submits
            .get(&sent.id)
            .filter(|s| s.state != SubmitState::Pending)
            .cloned()
    });
    assert_eq!(delivery(&mut c, &sent.id).state, DeliveryState::Delivered);
    let text_after = std::fs::read_to_string(&log).unwrap();
    assert_eq!(text, text_after, "something was delivered again");
    assert_eq!(text_after.matches("<<<END").count(), 1, "{text_after}");
    let (body, end) = text_after.split_once("\n<<<END").unwrap();
    assert_eq!(end, " pastes=1>>>\n", "not one bracketed paste");
    assert_eq!(
        body,
        sent.prompt.trim_end_matches('\n'),
        "prompt not intact"
    );
    assert!(
        comments(&mut c, "rev")
            .iter()
            .all(|x| x.state == CommentState::Sent
                && x.delivery.as_deref() == Some(sent.id.as_str()))
    );

    // ── idle agent: a new comment is delivered at once ──
    let more = c
        .call::<ReviewCommentsAdd>(&ReviewAddParams {
            task: "rev".into(),
            comments: vec![NewComment {
                file: "src/lib.rs".into(),
                line: 5,
                body: "One more thing".into(),
                ..Default::default()
            }],
        })
        .unwrap();
    wait_state(&mut c, &term, "idle");
    std::thread::sleep(Duration::from_millis(300));
    let now = c
        .call::<ReviewSend>(&ReviewSendParams {
            task: "rev".into(),
            comment_ids: vec![more.comments[0].id.clone()],
            ..Default::default()
        })
        .unwrap();
    assert_eq!(now.state, DeliveryState::Delivered, "{now:?}");
    until("second prompt", 30, || {
        let s = std::fs::read_to_string(&log).ok()?;
        (s.matches("<<<END").count() == 2).then_some(())
    });
    assert!(
        std::fs::read_to_string(&log)
            .unwrap()
            .contains("Reviewer: One more thing")
    );

    // Resolve, and resolved comments cannot be sent.
    let r = c
        .call::<ReviewCommentsResolve>(&ReviewResolveParams {
            ids: vec!["c1".into()],
            note: Some("done".into()),
            reopen: false,
        })
        .unwrap();
    assert_eq!(r.comments[0].state, CommentState::Resolved);
    let again = c.call::<ReviewSend>(&ReviewSendParams {
        task: "rev".into(),
        comment_ids: vec!["c1".into()],
        ..Default::default()
    });
    assert_eq!(rpc_code(again), orch_codes::INVALID_STATE);

    // ── blocked agent: refused ──
    let b = create_task(
        &mut c,
        "blk",
        vec![
            fake_agent(),
            "osc:state=blocked:kind=question:msg=UHJvY2VlZD8=".into(),
            "hold".into(),
        ],
    );
    wait_state(&mut c, &b.terminals[0].id, "blocked");
    c.call::<ReviewCommentsAdd>(&ReviewAddParams {
        task: "blk".into(),
        comments: vec![NewComment {
            file: "README.md".into(),
            line: 1,
            body: "x".into(),
            ..Default::default()
        }],
    })
    .unwrap();
    let refused = c.call::<ReviewSend>(&ReviewSendParams {
        task: "blk".into(),
        all_open: true,
        ..Default::default()
    });
    match refused {
        Err(podd::client::ClientError::Rpc(e)) => {
            assert_eq!(e.code, orch_codes::AGENT_BLOCKED);
            assert!(e.message.contains("blocked:question"), "{}", e.message);
        }
        other => panic!("expected AGENT_BLOCKED, got {other:?}"),
    }
    assert!(
        comments(&mut c, "blk")
            .iter()
            .all(|x| x.state == CommentState::Open)
    );

    // CLI: `pod review ls --json` sees the same comments.
    let out = pod(&d.pipe, &["review", "ls", "demo/rev", "--json"]);
    assert!(out.status.success());
    let listed: ReviewCommentsResult = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(listed.comments.len(), 4);
    let out = pod(&d.pipe, &["review", "send", "blk"]);
    assert_eq!(out.status.code(), Some(2), "blocked send must fail");
}

// ───────────────────────────── P5: groups ─────────────────────────────

/// Three shell agents from one prompt; compare shows per-member stats; a
/// configured check runs in each worktree (failures reported as failures);
/// pick refuses to drop unsaved work unless forced, then removes the others.
#[test]
fn group_fan_out_compare_check_pick() {
    let repo = TempRepo::new();
    let d = Daemon::start("group");
    let mut c = d.client();
    add_repo(&mut c, &repo);

    // Each member writes its task id; member 2 adds a line the check looks for.
    let prompt = "Set-Content -Path result.txt -Value $env:POD_TASK_ID; \
                  if ($env:POD_TASK_ID -like '*-2') { Add-Content result.txt 'extra' }; \
                  echo ('fan' + 'out-done')";
    let g = c
        .call::<GroupCreate>(&GroupCreateParams {
            repo: "demo".into(),
            name: "fx".into(),
            prompt: prompt.into(),
            agents: vec![AgentSpec::Kind(AgentKind::Shell); 3],
            check: Some(vec![
                "cmd".into(),
                "/c".into(),
                "findstr /c:extra result.txt".into(),
            ]),
            ..Default::default()
        })
        .expect("group.create");
    assert_eq!(g.id, "demo/fx");
    let names: Vec<&str> = g.members.iter().map(|m| m.task.as_str()).collect();
    assert_eq!(
        names,
        ["demo/fx-shell-1", "demo/fx-shell-2", "demo/fx-shell-3"]
    );
    let base = git(&repo.path, &["rev-parse", "HEAD"]).trim().to_string();
    assert_eq!(g.base_commit, base);
    for m in &g.members {
        let info = m.info.as_ref().unwrap();
        assert_eq!(info.base_commit, base);
        assert_eq!(info.base, "main");
        assert_eq!(info.prompt.as_deref(), Some(prompt));
        assert_eq!(info.group.as_deref(), Some("demo/fx"));
        wait_output(&mut c, &info.terminals[0].id, "fanout-done");
    }
    // Duplicate group.
    let dup = c.call::<GroupCreate>(&GroupCreateParams {
        repo: "demo".into(),
        name: "fx".into(),
        prompt: "x".into(),
        agents: vec![AgentSpec::Kind(AgentKind::Shell)],
        ..Default::default()
    });
    assert_eq!(rpc_code(dup), orch_codes::GROUP_EXISTS);

    let cmp = c
        .call::<GroupCompare>(&GroupRef { group: "fx".into() })
        .unwrap();
    assert_eq!(cmp.members.len(), 3);
    for (i, m) in cmp.members.iter().enumerate() {
        assert_eq!(m.files_changed, 1, "{m:?}");
        assert_eq!(m.files[0].path, "result.txt");
        assert_eq!(m.files[0].status, "?");
        assert_eq!(m.added, if i == 1 { 2 } else { 1 });
        assert_eq!(m.dirty, Some(1));
        assert!(m.status.is_some());
        assert!(m.check.is_none());
    }

    // The check passes only in member 2; the failures are reported.
    let chk = c
        .call::<GroupCheck>(&GroupCheckParams {
            group: "demo/fx".into(),
            timeout_ms: Some(60_000),
            wait: true,
            ..Default::default()
        })
        .unwrap();
    assert!(!chk.all_passed);
    let job = chk.job.as_ref().expect("v6 job");
    assert_eq!(job.state, CheckJobState::Finished);
    assert_eq!(job.all_passed, Some(false));
    assert_eq!(job.done.len(), 3);
    let passed: Vec<bool> = chk.results.iter().map(|r| r.result.passed).collect();
    assert_eq!(passed, [false, true, false]);
    assert_eq!(chk.results[0].result.exit_code, Some(1));
    assert!(chk.results[1].result.output_tail.contains("extra"));
    let cmp = c
        .call::<GroupCompare>(&GroupRef { group: "fx".into() })
        .unwrap();
    assert_eq!(cmp.members[1].check.as_ref().map(|r| r.passed), Some(true));
    assert_eq!(cmp.members[0].check.as_ref().map(|r| r.passed), Some(false));
    // CLI: exit code 1 for a failing check, JSON compare.
    let out = pod(&d.pipe, &["group", "check", "fx"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("CHECK FAILED"));
    let out = pod(&d.pipe, &["group", "compare", "fx", "--json"]);
    assert!(out.status.success());
    let cli_cmp: GroupCompareResult = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(cli_cmp.members.len(), 3);

    // ── v6: group.check is a background job: it returns at once, the same
    // connection keeps working, progress arrives on status.subscribe, and
    // group.check_cancel kills the commands. ──
    let (_, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    let (tx, events) = mpsc::channel::<GroupCheckEvent>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if let SubscribeEvent::GroupCheck(e) = ev
                && tx.send(*e).is_err()
            {
                break;
            }
        }
    });
    let slow = vec![
        "cmd".into(),
        "/c".into(),
        "ping -n 120 127.0.0.1 >nul".into(),
    ];
    let started = Instant::now();
    let r = c
        .call::<GroupCheck>(&GroupCheckParams {
            group: "fx".into(),
            check: Some(slow.clone()),
            timeout_ms: Some(300_000),
            ..Default::default()
        })
        .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "group.check blocked for {:?}",
        started.elapsed()
    );
    let job = r.job.clone().expect("job");
    assert_eq!(job.state, CheckJobState::Running);
    assert!(r.results.is_empty() && !r.all_passed);
    assert_eq!(job.tasks.len(), 3);
    // The same connection is served while the job runs.
    let shown = c
        .call::<GroupShow>(&GroupRef { group: "fx".into() })
        .unwrap();
    assert_eq!(
        shown.check_job.as_ref().map(|j| j.state),
        Some(CheckJobState::Running)
    );
    assert_eq!(shown.check.as_ref(), Some(&slow));
    let again = c.call::<GroupCheck>(&GroupCheckParams {
        group: "fx".into(),
        ..Default::default()
    });
    assert_eq!(
        rpc_code(again),
        orch_codes::INVALID_STATE,
        "one job per group"
    );
    let first = events
        .recv_timeout(Duration::from_secs(10))
        .expect("start event");
    assert_eq!(
        (first.job.id.as_str(), first.job.state),
        (job.id.as_str(), CheckJobState::Running)
    );
    // Cancel: replies once the job ended; every command was killed.
    let cancel_at = Instant::now();
    let r = c
        .call::<GroupCheckCancel>(&GroupRef { group: "fx".into() })
        .unwrap();
    assert!(cancel_at.elapsed() < Duration::from_secs(15));
    assert_eq!(
        r.job.as_ref().map(|j| j.state),
        Some(CheckJobState::Cancelled)
    );
    assert_eq!(r.results.len(), 3);
    assert!(r.results.iter().all(|m| !m.result.passed
        && m.result.error.as_deref() == Some("cancelled")
        && m.result.duration_ms < 60_000));
    assert!(!r.all_passed);
    let last = until("the cancelled event", 15, || {
        events
            .try_iter()
            .find(|e| e.job.state == CheckJobState::Cancelled)
    });
    assert_eq!(last.job.id, job.id);
    // CLI: --no-wait prints the job; --cancel ends it (exit 1: not passed).
    let out = pod(&d.pipe, &["group", "check", "fx", "--no-wait"]);
    assert!(out.status.success(), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("running on 3 member(s)"));
    let out = pod(&d.pipe, &["group", "check", "fx", "--cancel"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("Cancelled"));

    // Pick without force: the others hold unsaved work, so nothing is removed.
    let winner = "demo/fx-shell-2";
    let p = c
        .call::<GroupPick>(&GroupPickParams {
            group: "fx".into(),
            task: winner.into(),
            remove_others: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(p.picked, winner);
    assert!(p.removed.is_empty());
    assert_eq!(p.kept.len(), 2);
    assert!(p.kept.iter().all(|k| k.code == codes::TASK_DIRTY));
    let wts: Vec<PathBuf> = g
        .members
        .iter()
        .map(|m| PathBuf::from(&m.info.as_ref().unwrap().worktree))
        .collect();
    assert!(wts.iter().all(|w| w.join("result.txt").exists()));

    // Forced: the two losers go (terminals, worktrees, branches); the winner stays.
    let p = c
        .call::<GroupPick>(&GroupPickParams {
            group: "fx".into(),
            task: "fx-shell-2".into(),
            remove_others: true,
            force: true,
            delete_branch: true,
        })
        .unwrap();
    let removed: Vec<&str> = p.removed.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(removed, ["demo/fx-shell-1", "demo/fx-shell-3"]);
    assert!(p.kept.is_empty());
    assert!(!wts[0].exists() && !wts[2].exists());
    assert!(wts[1].join("result.txt").exists());
    let branches = git(&repo.path, &["branch", "--list", "pod/*"]);
    assert!(branches.contains("pod/fx-shell-2"));
    assert!(!branches.contains("pod/fx-shell-1") && !branches.contains("pod/fx-shell-3"));
    let shown = c
        .call::<GroupShow>(&GroupRef {
            group: "demo/fx".into(),
        })
        .unwrap();
    assert_eq!(shown.picked.as_deref(), Some(winner));
    let flags: Vec<(bool, bool)> = shown
        .members
        .iter()
        .map(|m| (m.removed, m.picked))
        .collect();
    assert_eq!(flags, [(true, false), (false, true), (true, false)]);
    let ls = c.call::<GroupList>(&GroupListParams::default()).unwrap();
    assert_eq!(ls.groups.len(), 1);
}

// ───────────────────────────── P5: gates ─────────────────────────────

/// v6: `gate.create {expires_in_s}` resolves as `expired` (event, wait,
/// CLI exit code 1), also when the expiry passed while the daemon was down.
#[test]
fn gates_expire() {
    let d = Daemon::start("gate-expiry");
    let mut c = d.client();
    let (_, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    let (tx, events) = mpsc::channel::<GateChangedEvent>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if let SubscribeEvent::Gate(g) = ev
                && tx.send(*g).is_err()
            {
                break;
            }
        }
    });
    let bad = c.call::<GateCreate>(&GateCreateParams {
        message: "x".into(),
        expires_in_s: Some(0),
        ..Default::default()
    });
    assert_eq!(rpc_code(bad), codes::INVALID_PARAMS);
    let g = c
        .call::<GateCreate>(&GateCreateParams {
            message: "Deploy?".into(),
            expires_in_s: Some(1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(g.state, GateState::Pending);
    assert_eq!(g.expires_ms, Some(g.created_ms + 1000));
    let never = c
        .call::<GateCreate>(&GateCreateParams {
            message: "No expiry".into(),
            ..Default::default()
        })
        .unwrap();
    // pod wait --gate returns (exit 1) when it expires.
    let out = pod(&d.pipe, &["wait", "--gate", &g.id, "--timeout", "30"]);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("expired"),
        "{out:?}"
    );
    let shown = c.call::<GateShow>(&GateRef { id: g.id.clone() }).unwrap();
    assert_eq!(shown.state, GateState::Expired);
    assert!(shown.resolved_ms.unwrap() >= g.created_ms + 1000);
    let ev = until("gate.changed expired", 10, || {
        events
            .try_iter()
            .find(|e| e.gate.id == g.id && e.gate.state == GateState::Expired)
    });
    assert!(ev.seq > 0);
    let late = c.call::<GateResolve>(&GateResolveParams {
        id: g.id.clone(),
        decision: GateDecision::Approve,
        note: None,
        choice: None,
    });
    assert_eq!(rpc_code(late), orch_codes::GATE_RESOLVED);
    assert_eq!(
        c.call::<GateShow>(&GateRef {
            id: never.id.clone()
        })
        .unwrap()
        .state,
        GateState::Pending
    );

    // Expiry while the daemon is down: resolved right after the restart.
    let g2 = c
        .call::<GateCreate>(&GateCreateParams {
            message: "Merge?".into(),
            expires_in_s: Some(2),
            ..Default::default()
        })
        .unwrap();
    drop(c);
    let (pipe, dir) = d.stop(false);
    std::thread::sleep(Duration::from_millis(2500));
    let d = Daemon::start_on(pipe, dir);
    let mut c = d.client();
    let s = until("g2 expired after restart", 10, || {
        let s = c.call::<GateShow>(&GateRef { id: g2.id.clone() }).ok()?;
        (s.state == GateState::Expired).then_some(s)
    });
    assert!(s.note.unwrap().contains("expired"));
    let pending = c
        .call::<GateList>(&GateListParams {
            state: Some(GateState::Pending),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(pending.gates.len(), 1, "the gate without expiry stays");
}

/// docs/11 §6-2: a terminal that never reports a turn (a shell, or an agent
/// seen only through heuristics) used to get its next queued prompt only
/// after 30 s. Now it goes once the output settled, still exactly once.
#[test]
fn review_queue_is_prompt_for_terminals_without_turns() {
    let repo = TempRepo::new();
    let d = Daemon::start("review-quiet");
    let mut c = d.client();
    add_repo(&mut c, &repo);
    let log = repo.root.join("received.log");
    let t = create_task(
        &mut c,
        "quiet",
        vec![fake_agent(), format!("paste-loop-silent:{}", log.display())],
    );
    let term = t.terminals[0].id.clone();
    wait_output(&mut c, &term, "ready for prompts");
    let s = c.call::<TerminalShow>(&TerminalRef::id(&term)).unwrap();
    let st = s.status.unwrap();
    assert!(
        !matches!(st.source.as_deref(), Some("osc" | "hook")),
        "must not report turns: {st:?}"
    );
    let added = c
        .call::<ReviewCommentsAdd>(&ReviewAddParams {
            task: "quiet".into(),
            comments: vec![
                NewComment {
                    file: "src/lib.rs".into(),
                    line: 1,
                    body: "First remark".into(),
                    ..Default::default()
                },
                NewComment {
                    file: "src/lib.rs".into(),
                    line: 2,
                    body: "Second remark".into(),
                    ..Default::default()
                },
            ],
        })
        .unwrap();
    let send = |c: &mut Client, id: &str| {
        c.call::<ReviewSend>(&ReviewSendParams {
            task: "quiet".into(),
            comment_ids: vec![id.into()],
            ..Default::default()
        })
        .unwrap()
    };
    // `review.delivery` events (additive in v6): every recorded or changed
    // delivery is announced on status.subscribe, so UIs need not poll.
    let (_, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    let (tx, delivery_events) = mpsc::channel::<ReviewDeliveryEvent>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if let SubscribeEvent::Delivery(e) = ev
                && tx.send(*e).is_err()
            {
                break;
            }
        }
    });
    // A subscriber filtered to another terminal does not get them.
    let (_, mut other) = d
        .client()
        .subscribe_status(&StatusSubscribeParams {
            ids: vec!["t999".into()],
            ..Default::default()
        })
        .unwrap();
    let (otx, other_events) = mpsc::channel::<ReviewDeliveryEvent>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = other.next_any() {
            if let SubscribeEvent::Delivery(e) = ev
                && otx.send(*e).is_err()
            {
                break;
            }
        }
    });
    let d1 = send(&mut c, &added.comments[0].id);
    assert_eq!(d1.state, DeliveryState::Delivered, "{d1:?}");
    let d2 = send(&mut c, &added.comments[1].id);
    assert_eq!(
        d2.state,
        DeliveryState::Queued,
        "the first prompt is still being typed"
    );
    let queued_at = Instant::now();
    until("the queued prompt", 20, || {
        (delivery(&mut c, &d2.id).state == DeliveryState::Delivered).then_some(())
    });
    let waited = queued_at.elapsed();
    assert!(
        waited < Duration::from_secs(10),
        "queued prompt waited {waited:?} (the old fallback was 30 s)"
    );
    // Events: d1 delivered, d2 queued, then d2 delivered from the queue, in
    // increasing `seq` and with the full record.
    let mut seen: Vec<(String, DeliveryState, u64)> = Vec::new();
    while seen.len() < 3 {
        let e = delivery_events
            .recv_timeout(Duration::from_secs(10))
            .unwrap_or_else(|_| panic!("review.delivery events so far: {seen:?}"));
        // v8: a later event per delivery carries the final submit check
        // (`unverifiable` here: this agent reports no status).
        if let Some(s) = e
            .submit
            .as_ref()
            .filter(|s| s.state != SubmitState::Pending)
        {
            assert_eq!(s.state, SubmitState::Unverifiable, "{e:?}");
            continue;
        }
        assert_eq!(e.delivery.task, t.id);
        assert_eq!(e.delivery.terminal, term);
        seen.push((e.delivery.id.clone(), e.delivery.state, e.seq));
    }
    let states: Vec<(&str, DeliveryState)> =
        seen.iter().map(|(id, s, _)| (id.as_str(), *s)).collect();
    assert_eq!(
        states,
        [
            (d1.id.as_str(), DeliveryState::Delivered),
            (d2.id.as_str(), DeliveryState::Queued),
            (d2.id.as_str(), DeliveryState::Delivered),
        ]
    );
    assert!(
        seen.windows(2).all(|w| w[0].2 < w[1].2),
        "seq grows: {seen:?}"
    );
    assert!(
        other_events
            .recv_timeout(Duration::from_millis(300))
            .is_err(),
        "a subscriber filtered to another terminal got a delivery event"
    );
    until("both prompts recorded", 20, || {
        let s = std::fs::read_to_string(&log).ok()?;
        (s.matches("<<<END").count() == 2).then_some(())
    });
    std::thread::sleep(Duration::from_millis(2000));
    let text = std::fs::read_to_string(&log).unwrap();
    assert_eq!(
        text.matches("<<<END").count(),
        2,
        "exactly once each: {text}"
    );
    assert_eq!(text.matches("Reviewer: First remark").count(), 1);
    assert_eq!(text.matches("Reviewer: Second remark").count(), 1);
    assert!(
        text.find("First remark").unwrap() < text.find("Second remark").unwrap(),
        "in order"
    );
}

/// An agent asks for approval from inside its terminal (`pod gate request`):
/// the gate shows up on `status.subscribe`, in `task.show`, and
/// `pod wait --gate` returns when the human approves.
#[test]
fn gate_requested_in_terminal_subscribe_and_wait() {
    let repo = TempRepo::new();
    let d = Daemon::start("gate");
    let mut c = d.client();
    add_repo(&mut c, &repo);
    let t = create_task(
        &mut c,
        "gt",
        vec!["powershell".into(), "-NoLogo".into(), "-NoProfile".into()],
    );
    let term = t.terminals[0].id.clone();

    // Subscriber collecting gate events.
    let (snap, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    assert!(snap.gates.is_empty());
    let (tx, rx) = mpsc::channel::<GateChangedEvent>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            match ev {
                SubscribeEvent::Gate(g) => {
                    if tx.send(*g).is_err() {
                        break;
                    }
                }
                SubscribeEvent::Status(StatusStreamEvent::Changed(_))
                | SubscribeEvent::Status(StatusStreamEvent::Lagged(_)) => {}
                _ => {}
            }
        }
    });

    // The agent requests a gate from inside its terminal.
    let pod_path = pod_exe().display().to_string();
    send(
        &mut c,
        &term,
        &format!(
            "& '{pod_path}' gate request 'Deploy to staging?' --title Deploy; echo ('gate' + '-asked')"
        ),
    );
    wait_output(&mut c, &term, "gate-asked");
    let ev = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("gate.changed");
    let g = ev.gate;
    assert_eq!(g.id, "g1");
    assert_eq!(g.state, GateState::Pending);
    assert_eq!(g.title, "Deploy");
    assert_eq!(g.message, "Deploy to staging?");
    assert_eq!(g.task.as_deref(), Some("demo/gt"));
    assert_eq!(g.terminal.as_deref(), Some(term.as_str()));
    let shown = c.call::<TaskShow>(&TaskRef { task: "gt".into() }).unwrap();
    assert_eq!(shown.gates.len(), 1);
    // A new subscriber sees it in the snapshot.
    let (snap2, _s2) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    assert_eq!(snap2.gates.len(), 1);

    // `pod wait --gate` blocks until the human approves.
    let mut waiter = Command::new(pod_exe())
        .args(["--pipe", &d.pipe, "wait", "--gate", "g1", "--timeout", "60"])
        .stdout(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        waiter.try_wait().unwrap().is_none(),
        "returned before approval"
    );
    let out = pod(&d.pipe, &["gate", "approve", "g1", "--note", "go ahead"]);
    assert!(out.status.success());
    let st = until("pod wait --gate to return", 15, || {
        waiter.try_wait().unwrap()
    });
    assert_eq!(st.code(), Some(0));
    let mut so = String::new();
    std::io::Read::read_to_string(waiter.stdout.as_mut().unwrap(), &mut so).unwrap();
    assert!(so.contains("g1 approved"), "{so}");
    let ev = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("resolution event");
    assert_eq!(ev.gate.state, GateState::Approved);
    assert_eq!(ev.gate.note.as_deref(), Some("go ahead"));
    assert!(ev.seq > 0);
    let twice = c.call::<GateResolve>(&GateResolveParams {
        id: "g1".into(),
        decision: GateDecision::Reject,
        note: None,
        choice: None,
    });
    assert_eq!(rpc_code(twice), orch_codes::GATE_RESOLVED);

    // `--wait` inside the terminal: a rejection gives exit code 1 to the agent.
    send(
        &mut c,
        &term,
        &format!(
            "& '{pod_path}' gate request 'Delete the cache?' --wait; echo \"gate-exit=$LASTEXITCODE\""
        ),
    );
    let ev = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("second gate");
    assert_eq!(ev.gate.id, "g2");
    c.call::<GateResolve>(&GateResolveParams {
        id: "g2".into(),
        decision: GateDecision::Reject,
        note: Some("no".into()),
        choice: None,
    })
    .unwrap();
    wait_output(&mut c, &term, "gate-exit=1");

    // Gate for a group-less task created through the API, listed by state.
    let pending = c
        .call::<GateList>(&GateListParams {
            state: Some(GateState::Pending),
            ..Default::default()
        })
        .unwrap();
    assert!(pending.gates.is_empty());
    let all = c
        .call::<GateList>(&GateListParams {
            task: Some("gt".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(all.gates.len(), 2);
    // Outside a Pod terminal without --task: a clear error.
    let out = Command::new(pod_exe())
        .args(["--pipe", &d.pipe, "gate", "request", "x"])
        .env_remove("POD_TERMINAL_ID")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

// ─────────────────────────── podd-gaps2 (v7) ───────────────────────────

/// docs/11 §6-3: a delivery queued for longer than the review queue timeout
/// expires: `failed` with an "expired" error, a `review.delivery` event, and
/// its comments are open again. Deliveries still inside the timeout stay.
#[test]
fn queued_review_expires_with_an_event() {
    let repo = TempRepo::new();
    let name = unique("review-expire");
    let d = Daemon::start_with(
        format!(r"\\.\pipe\{name}"),
        std::env::temp_dir().join(&name),
        &["--review-queue-timeout", "2s"],
    );
    let mut c = d.client();
    add_repo(&mut c, &repo);
    let t = create_task(
        &mut c,
        "busy",
        vec![fake_agent(), "osc:state=working".into(), "hold".into()],
    );
    let term = t.terminals[0].id.clone();
    wait_state(&mut c, &term, "working");
    let added = c
        .call::<ReviewCommentsAdd>(&ReviewAddParams {
            task: "busy".into(),
            comments: vec![NewComment {
                file: "README.md".into(),
                line: 1,
                body: "Never delivered".into(),
                ..Default::default()
            }],
        })
        .unwrap();
    let (_, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    let (tx, events) = mpsc::channel::<ReviewDeliveryEvent>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if let SubscribeEvent::Delivery(e) = ev
                && tx.send(*e).is_err()
            {
                break;
            }
        }
    });
    let sent = Instant::now();
    let q = c
        .call::<ReviewSend>(&ReviewSendParams {
            task: "busy".into(),
            all_open: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(q.state, DeliveryState::Queued);
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(
        delivery(&mut c, &q.id).state,
        DeliveryState::Queued,
        "not before the timeout"
    );

    let expired = loop {
        let e = events
            .recv_timeout(Duration::from_secs(15))
            .expect("expiry event");
        if e.delivery.id == q.id && e.delivery.state == DeliveryState::Failed {
            break e.delivery;
        }
    };
    let waited = sent.elapsed();
    assert!(
        waited >= Duration::from_secs(2) && waited < Duration::from_secs(10),
        "{waited:?}"
    );
    let err = expired.error.clone().unwrap_or_default();
    assert!(err.starts_with("expired: queued for"), "{err}");
    assert_eq!(delivery(&mut c, &q.id), expired);
    let cs = comments(&mut c, "busy");
    assert_eq!(cs[0].id, added.comments[0].id);
    assert_eq!(cs[0].state, CommentState::Open, "comments are open again");
}

/// `terminal.run` from a terminal of a task records the task as `parent_task`.
#[test]
fn run_from_a_task_terminal_records_parent_task() {
    let repo = TempRepo::new();
    let d = Daemon::start("parent-task");
    let mut c = d.client();
    add_repo(&mut c, &repo);
    let t = create_task(
        &mut c,
        "boss",
        vec!["powershell".into(), "-NoLogo".into(), "-NoProfile".into()],
    );
    let term = t.terminals[0].id.clone();
    let kid = c
        .call::<TerminalRun>(&RunParams {
            argv: vec!["cmd".into(), "/c".into(), "echo kid".into()],
            cwd: cwd(),
            parent_terminal: Some(term.clone()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(kid.parent_terminal.as_deref(), Some(term.as_str()));
    assert_eq!(kid.parent_task.as_deref(), Some(t.id.as_str()));
}

/// docs/12 B1: a review delivered to real Codex stayed in its input box
/// ("delivered", but no turn). The `burst-loop` agent reads console key
/// events like Codex on Windows, applies its paste-burst rules and lags
/// 0.3 ms per key behind its input, so an Enter pressed at a fixed 250 ms
/// after the paste became a newline. Now Enter waits for the paste to be
/// drawn, and the delivery records the agent's reaction (`submits`, event).
#[test]
fn review_delivery_submits_to_a_lagging_paste_burst_tui() {
    let repo = TempRepo::new();
    let d = Daemon::start("review-burst");
    let mut c = d.client();
    add_repo(&mut c, &repo);
    let log = repo.root.join("received.log");
    let t = create_task(
        &mut c,
        "burst",
        vec![fake_agent(), format!("burst-loop:{}:300", log.display())],
    );
    let term = t.terminals[0].id.clone();
    wait_output(&mut c, &term, "ready for prompts");
    let (_, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    let (tx, events) = mpsc::channel::<ReviewDeliveryEvent>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if let SubscribeEvent::Delivery(e) = ev
                && tx.send(*e).is_err()
            {
                break;
            }
        }
    });
    let body: String = (1..=25)
        .map(|i| format!("Point {i}: rename this and explain why — briefly."))
        .collect::<Vec<_>>()
        .join("\n");
    c.call::<ReviewCommentsAdd>(&ReviewAddParams {
        task: "burst".into(),
        comments: vec![NewComment {
            file: "src/lib.rs".into(),
            line: 2,
            end_line: Some(4),
            body,
            ..Default::default()
        }],
    })
    .unwrap();
    let sent = c
        .call::<ReviewSend>(&ReviewSendParams {
            task: "burst".into(),
            all_open: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(sent.state, DeliveryState::Delivered, "{sent:?}");
    assert!(sent.prompt.len() > 1_500, "a long prompt");
    // The confirmation event follows the delivery event.
    let confirmed = until("submit confirmation event", 20, || {
        let e = events.recv_timeout(Duration::from_millis(200)).ok()?;
        assert_eq!(e.delivery.id, sent.id);
        e.submit.filter(|s| s.state != SubmitState::Pending)
    });
    assert_eq!(confirmed.state, SubmitState::Confirmed, "{confirmed:?}");
    // One Enter normally; the retry may be needed under heavy CPU load.
    assert!(confirmed.enters <= 2, "{confirmed:?}");
    let listed = c
        .call::<ReviewDeliveries>(&ReviewDeliveriesParams::default())
        .unwrap();
    assert_eq!(listed.submits.get(&sent.id), Some(&confirmed));
    // Exactly one submission, the whole multi-line prompt.
    let got = until("the prompt recorded", 20, || {
        let s = std::fs::read_to_string(&log).ok()?;
        s.contains("<<<END").then_some(s)
    });
    assert_eq!(got.matches("<<<END").count(), 1, "{got}");
    let text = got.split("\n<<<END").next().unwrap();
    assert_eq!(text, sent.prompt.trim_end());
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(
        std::fs::read_to_string(&log)
            .unwrap()
            .matches("<<<END")
            .count(),
        1,
        "submitted once"
    );
}

/// docs/12 B2: Codex asked for permission (hook `PermissionRequest`), the
/// user approved with Enter, and the approved command ran for a long time
/// with no further hook: the status stayed `blocked:permission` and
/// `review.send` was refused as AGENT_BLOCKED. Now the answer key plus the
/// agent's output means `working` (source `heuristic`), so a review is
/// queued; this survives a daemon restart.
#[test]
fn approved_permission_is_working_and_reviews_queue() {
    let repo = TempRepo::new();
    let d = Daemon::start("perm-approved");
    let mut c = d.client();
    add_repo(&mut c, &repo);
    create_task(&mut c, "perm", vec![fake_agent(), "hold".into()]);
    let hook = workspace_bin("pod-hook.exe").display().to_string();
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec![
                fake_agent(),
                r#"hook:codex:UserPromptSubmit:{"hook_event_name":"UserPromptSubmit"}"#.into(),
                r#"hook:codex:PermissionRequest:{"hook_event_name":"PermissionRequest","tool_name":"shell"}"#.into(),
                "read".into(),
                "print:running the approved command".into(),
                "hold".into(),
            ],
            cwd: cwd(),
            env: [("POD_HOOK_EXE".to_string(), hook)].into(),
            ..Default::default()
        })
        .unwrap();
    let term = t.id.clone();
    let s = wait_state(&mut c, &term, "blocked");
    assert_eq!(
        (s.kind.as_deref(), s.source.as_deref()),
        (Some("permission"), Some("hook"))
    );
    c.call::<ReviewCommentsAdd>(&ReviewAddParams {
        task: "perm".into(),
        comments: vec![NewComment {
            file: "README.md".into(),
            line: 1,
            body: "Mention the approval flow".into(),
            ..Default::default()
        }],
    })
    .unwrap();
    let send = |c: &mut Client| {
        c.call::<ReviewSend>(&ReviewSendParams {
            task: "perm".into(),
            all_open: true,
            terminal: Some(term.clone()),
            ..Default::default()
        })
    };
    // Genuinely waiting for the user: refused.
    assert_eq!(rpc_code(send(&mut c)), orch_codes::AGENT_BLOCKED);
    // Arrow keys move in the prompt but do not answer it.
    c.call::<TerminalKeys>(&KeysParams {
        target: TerminalRef::id(&term),
        keys: vec!["down".into()],
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        c.call::<TerminalShow>(&TerminalRef::id(&term))
            .unwrap()
            .status
            .unwrap()
            .state,
        "blocked"
    );
    // Approve.
    let approved_at = Instant::now();
    c.call::<TerminalKeys>(&KeysParams {
        target: TerminalRef::id(&term),
        keys: vec!["enter".into()],
    })
    .unwrap();
    let s = wait_state(&mut c, &term, "working");
    assert!(approved_at.elapsed() < Duration::from_secs(5));
    assert_eq!(s.source.as_deref(), Some("heuristic"), "{s:?}");
    // The command keeps running (no PostToolUse): a review is queued now.
    let q = send(&mut c).expect("not refused any more");
    assert_eq!(q.state, DeliveryState::Queued, "{q:?}");
    assert_eq!(q.agent_state, "working");

    // Restart: the persisted hook is the PermissionRequest, but the answer
    // was recorded too.
    drop(c);
    let (pipe, dir) = d.stop(false);
    let d = Daemon::start_on(pipe, dir);
    let mut c = d.client();
    let s = c
        .call::<TerminalShow>(&TerminalRef::id(&term))
        .unwrap()
        .status
        .unwrap();
    assert_eq!(
        (s.state.as_str(), s.source.as_deref()),
        ("working", Some("heuristic")),
        "{s:?}"
    );
    c.call::<ReviewCancel>(&DeliveryRef { id: q.id }).unwrap();
}

fn spawn_pod(pipe: &str, args: &[&str], env: &[(&str, &str)]) -> std::process::Child {
    let mut cmd = Command::new(pod_exe());
    cmd.arg("--pipe").arg(pipe).args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pod")
}

fn finish(child: std::process::Child, secs: u64) -> (i32, String, String) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = rx
        .recv_timeout(Duration::from_secs(secs))
        .expect("pod wait did not finish")
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// docs/12 B6: `pod gate request --wait` (and `pod wait`) aborted with
/// "daemon closed the connection" when podd restarted, although the gate
/// was persisted. Now they reconnect and keep waiting on the same gate /
/// status; with no daemon coming back they fail clearly after the
/// reconnect window.
#[test]
fn waits_survive_a_daemon_restart() {
    let repo = TempRepo::new();
    let mut d = Daemon::start("wait-restart");
    let mut c = d.client();
    add_repo(&mut c, &repo);
    let go = repo.root.join("go.flag");
    let t = create_task(
        &mut c,
        "wr",
        vec![
            fake_agent(),
            "osc:state=working".into(),
            format!("wait-file:{}", go.display()),
            "osc:state=done".into(),
            "hold".into(),
        ],
    );
    let term = t.terminals[0].id.clone();
    wait_state(&mut c, &term, "working");
    let g = c
        .call::<GateCreate>(&GateCreateParams {
            task: Some("wr".into()),
            message: "Ship it?".into(),
            ..Default::default()
        })
        .unwrap();
    let pipe = d.pipe.clone();
    let gate_wait = spawn_pod(&pipe, &["wait", "--gate", &g.id, "--timeout", "120"], &[]);
    let status_wait = spawn_pod(
        &pipe,
        &["wait", &term, "--status", "done", "--timeout", "120"],
        &[],
    );
    std::thread::sleep(Duration::from_millis(1_000));

    // Crash podd (terminals stay in their holders), restart it 2 s later.
    drop(c);
    d.child.kill().unwrap();
    let _ = d.child.wait();
    let dir = d.state_dir.clone();
    std::mem::forget(d);
    std::thread::sleep(Duration::from_millis(2_000));
    let d = Daemon::start_on(pipe.clone(), dir);
    let mut c = d.client();
    std::thread::sleep(Duration::from_millis(1_500));
    c.call::<GateResolve>(&GateResolveParams {
        id: g.id.clone(),
        decision: GateDecision::Approve,
        note: None,
        choice: None,
    })
    .unwrap();
    std::fs::write(&go, "go").unwrap();
    let (code, out, err) = finish(gate_wait, 30);
    assert_eq!(code, 0, "gate wait: {out}\n{err}");
    assert!(out.contains(&format!("{} approved", g.id)), "{out}");
    assert!(err.contains("reconnected"), "{err}");
    let (code, out, err) = finish(status_wait, 30);
    assert_eq!(code, 0, "status wait: {out}\n{err}");
    assert!(out.contains("done"), "{out}");

    // No daemon comes back: a clear error after POD_RECONNECT_TIMEOUT.
    let g2 = c
        .call::<GateCreate>(&GateCreateParams {
            task: Some("wr".into()),
            message: "Again?".into(),
            ..Default::default()
        })
        .unwrap();
    let lonely = spawn_pod(
        &pipe,
        &["wait", "--gate", &g2.id, "--timeout", "120"],
        &[("POD_RECONNECT_TIMEOUT", "1.5")],
    );
    std::thread::sleep(Duration::from_millis(800));
    drop(c);
    let (pipe, dir) = d.stop(false);
    let started = Instant::now();
    let (code, out, err) = finish(lonely, 30);
    assert_ne!(code, 0, "{out}\n{err}");
    assert!(err.contains("did not come back"), "{err}");
    assert!(err.contains(&g2.id), "{err}");
    assert!(started.elapsed() < Duration::from_secs(10));
    // Clean up the terminals with a daemon that kills them.
    drop(Daemon::start_on(pipe, dir));
}
