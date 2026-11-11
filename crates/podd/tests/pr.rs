//! Task pull requests end to end (protocol v10, docs/15) with the **fake
//! forge** (`POD_FORGE=fake:<file>`): the real podd and git, a repository
//! whose `origin` names a GitHub repository that is never contacted, and the
//! fake's state file edited from the test to move checks and reviews.
//! No GitHub write (nor read) happens here.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use pod_forge::fake::FakeForge;
use pod_test_support::git::{git_ok, git_output};
use pod_test_support::wait;
use podd::client::{Client, ClientError, SubscribeEvent};
use podd::protocol::*;

struct TempRepo {
    root: PathBuf,
    path: PathBuf,
}

impl TempRepo {
    fn new(remote: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(unique("pr-repo"));
        let path = root.join("pod repo");
        std::fs::create_dir_all(&path).unwrap();
        git_ok(&path, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git_ok(&path, &["config", k, v]);
        }
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        git_ok(&path, &["add", "-A"]);
        git_ok(&path, &["commit", "-q", "-m", "init"]);
        if let Some(url) = remote {
            // Configuration only: nothing is fetched or pushed.
            git_ok(&path, &["remote", "add", "origin", url]);
        }
        Self { root, path }
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn task(c: &mut Client, repo: &TempRepo, name: &str) -> TaskInfo {
    c.call::<RepoAdd>(&RepoAddParams {
        path: repo.path.display().to_string(),
        name: Some("demo".into()),
    })
    .ok();
    c.call::<TaskCreate>(&TaskCreateParams {
        repo: "demo".into(),
        name: name.into(),
        agent: Some(AgentSpec::Argv {
            argv: vec![
                "cmd".into(),
                "/c".into(),
                "ping -n 600 127.0.0.1 >nul".into(),
            ],
        }),
        ..Default::default()
    })
    .expect("task.create")
}

fn commit_in(wt: &str, file: &str, text: &str) {
    let wt = Path::new(wt);
    std::fs::write(wt.join(file), text).unwrap();
    git_ok(wt, &["add", "-A"]);
    git_ok(wt, &["commit", "-q", "-m", &format!("add {file}")]);
}

fn status(c: &mut Client, task: &str) -> TaskPrInfo {
    c.call::<TaskPrStatusMethod>(&TaskPrStatusParams {
        task: task.into(),
        cached: false,
    })
    .expect("task.pr.status")
}

fn rpc_err<T: std::fmt::Debug>(r: Result<T, ClientError>) -> RpcError {
    match r {
        Err(ClientError::Rpc(e)) => e,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

/// Upper bound for one `task.pr.changed` event. The slow timer is 1 s here,
/// but a loaded CI runner needed more than 10 s once; waits end at the event.
const EVENT_WAIT: Duration = Duration::from_secs(30);

/// `task.pr.changed` events of a lifecycle subscription.
fn pr_events(d: &Daemon) -> mpsc::Receiver<TaskPrChangedEvent> {
    let (_, mut stream) = Client::connect(&d.pipe)
        .unwrap()
        .subscribe_status(&StatusSubscribeParams {
            lifecycle: true,
            ..Default::default()
        })
        .expect("subscribe");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if let SubscribeEvent::TaskPr(e) = ev
                && tx.send(*e).is_err()
            {
                return;
            }
        }
    });
    rx
}

fn next_pr_event(
    rx: &mpsc::Receiver<TaskPrChangedEvent>,
    within: Duration,
    what: &str,
    ok: impl Fn(&TaskPrChangedEvent) -> bool,
) -> TaskPrChangedEvent {
    let deadline = Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(e) if ok(&e) => return e,
            Ok(_) => continue,
            Err(_) => panic!("no task.pr.changed event for {what} within {within:?}"),
        }
    }
}

#[test]
fn without_a_remote_status_is_empty_and_create_refuses() {
    let repo = TempRepo::new(None);
    let fake = std::env::temp_dir()
        .join(unique("fake-forge"))
        .with_extension("json");
    let daemon = Daemon::start_env(
        "pr-noremote",
        &[("POD_FORGE", &format!("fake:{}", fake.display()))],
    );
    let mut c = daemon.client();
    let t = task(&mut c, &repo, "local");

    let s = status(&mut c, &t.id);
    assert!(s.status.remote.is_none() && s.status.pr.is_none(), "{s:?}");
    assert_eq!(
        s.status.error.as_ref().map(|e| e.kind),
        Some(ForgeErrorKind::NoRemote)
    );
    assert!(s.cached, "nothing was asked of GitHub");
    assert_eq!(
        (s.branch.as_str(), s.default_base.as_deref(), s.ahead),
        ("pod/local", Some("main"), 0)
    );

    // No commits yet: refused before any forge call.
    let e = rpc_err(c.call::<TaskPrCreate>(&TaskPrCreateParams {
        task: t.id.clone(),
        title: "제목".into(),
        ..Default::default()
    }));
    assert_eq!(e.code, orch_codes::INVALID_STATE, "{e:?}");
    assert!(e.message.contains("no commits ahead"), "{}", e.message);
    // With a commit: the remote is missing, nothing is created or pushed.
    commit_in(&t.worktree, "a.txt", "a\n");
    let e = rpc_err(c.call::<TaskPrCreate>(&TaskPrCreateParams {
        task: t.id.clone(),
        title: "제목".into(),
        ..Default::default()
    }));
    assert_eq!(e.code, pr_codes::FORGE_FAILED);
    assert_eq!(e.data.as_ref().unwrap()["kind"], "no_remote");
    assert!(
        e.message.contains("Pod does not create one"),
        "{}",
        e.message
    );
    let calls = FakeForge::load_file(&fake).calls;
    assert!(
        calls.iter().all(|c| c == "detect"),
        "only remote detection: {calls:?}"
    );
    assert!(
        git_output(&repo.path, &["remote"]).trim().is_empty(),
        "no remote was added"
    );
    // Empty title / unknown task.
    assert_eq!(
        rpc_err(c.call::<TaskPrCreate>(&TaskPrCreateParams {
            task: t.id.clone(),
            title: "  ".into(),
            ..Default::default()
        }))
        .code,
        codes::INVALID_PARAMS
    );
    assert_eq!(
        rpc_err(c.call::<TaskPrStatusMethod>(&TaskPrStatusParams {
            task: "nope".into(),
            cached: false
        }))
        .code,
        codes::TASK_NOT_FOUND
    );
    assert!(
        c.call::<TaskShow>(&TaskRef { task: t.id.clone() })
            .unwrap()
            .pr
            .is_none()
    );
    let _ = std::fs::remove_file(&fake);
}

#[test]
fn create_follow_checks_and_merge_through_the_fake_forge() {
    let repo = TempRepo::new(Some("https://github.com/pod-fixtures/demo.git"));
    let fake = std::env::temp_dir()
        .join(unique("fake-forge"))
        .with_extension("json");
    let forge = FakeForge::with_file(fake.clone());
    let daemon = Daemon::start_env(
        "pr-flow",
        &[
            ("POD_FORGE", &format!("fake:{}", fake.display())),
            ("POD_PR_POLL", "1s"),
        ],
    );
    let events = pr_events(&daemon);
    let mut c = daemon.client();
    let t = task(&mut c, &repo, "feat");
    commit_in(&t.worktree, "hello.txt", "안녕\n");
    std::fs::write(Path::new(&t.worktree).join("scratch.txt"), "x").unwrap();

    let s = status(&mut c, &t.id);
    let remote = s.status.remote.clone().expect("origin detected");
    assert_eq!(
        (remote.name.as_str(), remote.slug.full_name().as_str()),
        ("origin", "pod-fixtures/demo")
    );
    assert!(s.status.pr.is_none() && s.status.error.is_none(), "{s:?}");
    assert_eq!((s.ahead, s.unpushed, s.dirty), (1, 1, 1));

    // Create: push first, then the PR (title, body, base), pending checks.
    let i = c
        .call::<TaskPrCreate>(&TaskPrCreateParams {
            task: t.id.clone(),
            title: "인사말 추가".into(),
            body: "## 요약\n- hello.txt".into(),
            ..Default::default()
        })
        .expect("task.pr.create");
    let pr = i.status.pr.clone().unwrap();
    assert_eq!(
        (
            pr.number,
            pr.state,
            pr.draft,
            pr.head.as_str(),
            pr.base.as_str()
        ),
        (1, PrState::Open, false, "pod/feat", "main")
    );
    assert_eq!(pr.url, "https://github.com/pod-fixtures/demo/pull/1");
    assert_eq!(i.status.summary.verdict(), "pending");
    let st = forge.state();
    let push = st
        .calls
        .iter()
        .position(|c| c == "push origin pod/feat")
        .expect("pushed");
    let create = st
        .calls
        .iter()
        .position(|c| c.starts_with("create pod/feat -> main"))
        .expect("created");
    assert!(push < create, "push before create: {:?}", st.calls);
    assert_eq!(st.prs[0].body, "## 요약\n- hello.txt");
    let ev = next_pr_event(&events, EVENT_WAIT, "create", |e| {
        e.pr.as_ref().is_some_and(|p| p.number == 1)
    });
    assert_eq!(ev.id, t.id);
    let info = c
        .call::<TaskShow>(&TaskRef { task: t.id.clone() })
        .unwrap()
        .pr
        .expect("TaskInfo.pr");
    assert_eq!(
        (info.number, info.state, info.checks.pending),
        (1, PrState::Open, 3)
    );
    // Creating again returns the same open pull request.
    let again = c
        .call::<TaskPrCreate>(&TaskPrCreateParams {
            task: t.id.clone(),
            title: "x".into(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(again.status.pr.unwrap().number, 1);

    // CI fails: the slow timer (1 s here) picks it up and announces it.
    forge.edit(|s| s.pr_mut(1).unwrap().checks = pod_forge::fixtures::checks_failed());
    let ev = next_pr_event(&events, EVENT_WAIT, "failed checks", |e| {
        e.pr.as_ref().is_some_and(|p| p.checks.failed > 0)
    });
    assert_eq!(ev.pr.unwrap().checks.verdict(), "failed");
    let s = status(&mut c, &t.id);
    let failed: Vec<&PrCheck> = s
        .status
        .checks
        .iter()
        .filter(|c| c.bucket == CheckBucket::Fail)
        .collect();
    assert_eq!(failed.len(), 1);
    assert!(failed[0].link.as_deref().unwrap().starts_with("https://"));

    // Green and approved.
    forge.edit(|s| {
        let p = s.pr_mut(1).unwrap();
        p.checks = pod_forge::fixtures::checks_passed();
        p.view.review_decision = Some(ReviewDecision::Approved);
    });
    let ev = next_pr_event(&events, EVENT_WAIT, "approval", |e| {
        e.pr.as_ref()
            .is_some_and(|p| p.review_decision == Some(ReviewDecision::Approved))
    });
    assert_eq!(ev.pr.unwrap().checks.verdict(), "passed");

    // Merge (squash).
    let m = c
        .call::<TaskPrMerge>(&TaskPrMergeParams {
            task: t.id.clone(),
            method: MergeMethod::Squash,
        })
        .expect("task.pr.merge");
    assert_eq!(m.status.pr.as_ref().unwrap().state, PrState::Merged);
    assert_eq!(forge.state().prs[0].merged_with, Some(MergeMethod::Squash));
    next_pr_event(&events, EVENT_WAIT, "merged", |e| {
        e.pr.as_ref().is_some_and(|p| p.state == PrState::Merged)
    });
    assert_eq!(
        c.call::<TaskShow>(&TaskRef { task: t.id.clone() })
            .unwrap()
            .pr
            .unwrap()
            .state,
        PrState::Merged
    );
    let e = rpc_err(c.call::<TaskPrMerge>(&TaskPrMergeParams {
        task: t.id.clone(),
        method: MergeMethod::Merge,
    }));
    assert_eq!(e.code, orch_codes::INVALID_STATE, "{e:?}");
    assert!(e.message.contains("merged"), "{}", e.message);

    // Merged pull requests are no longer polled. (The 300 ms lets a poll
    // already in flight land before the count is taken.)
    std::thread::sleep(Duration::from_millis(300));
    let before = forge.state().calls.len();
    wait::stays(Duration::from_millis(2500), POLL, || {
        let mut calls = forge.state().calls;
        if calls.len() == before {
            Ok(())
        } else {
            Err(calls.split_off(before.min(calls.len())))
        }
    })
    .unwrap_or_else(|b| panic!("no polling after merge: {b}"));

    // The record survives a daemon restart.
    let (pipe, dir) = daemon.stop(false);
    let daemon = Daemon::start_full(
        pipe,
        dir,
        &[],
        &[("POD_FORGE", &format!("fake:{}", fake.display()))],
    );
    let mut c = daemon.client();
    let pr = c
        .call::<TaskShow>(&TaskRef { task: t.id.clone() })
        .unwrap()
        .pr
        .unwrap();
    assert_eq!((pr.number, pr.state), (1, PrState::Merged));
    // orch-04: the merged commit is kept with the record (branch deletion
    // compares it with the branch tip).
    assert_eq!(pr.head_oid.as_ref().map(String::len), Some(40), "{pr:?}");
    let cached = c
        .call::<TaskPrStatusMethod>(&TaskPrStatusParams {
            task: t.id.clone(),
            cached: true,
        })
        .unwrap();
    assert!(
        cached.cached
            && cached
                .status
                .pr
                .as_ref()
                .is_some_and(|p| p.state == PrState::Merged)
    );
    drop(c);
    drop(daemon);
    let _ = std::fs::remove_file(&fake);
}

#[test]
fn forge_failures_keep_their_kind_and_change_nothing() {
    let repo = TempRepo::new(Some("git@github.com:pod-fixtures/demo.git"));
    let fake = std::env::temp_dir()
        .join(unique("fake-forge"))
        .with_extension("json");
    let forge = FakeForge::with_file(fake.clone());
    forge.edit(|s| {
        s.fail.insert("push".into(), ForgeError::classify("git", Some(128), "fatal: could not read Username for 'https://github.com': terminal prompts disabled"));
    });
    let daemon = Daemon::start_env(
        "pr-errors",
        &[
            ("POD_FORGE", &format!("fake:{}", fake.display())),
            ("POD_PR_POLL", "off"),
        ],
    );
    let mut c = daemon.client();
    let t = task(&mut c, &repo, "err");
    commit_in(&t.worktree, "b.txt", "b\n");

    let e = rpc_err(c.call::<TaskPrCreate>(&TaskPrCreateParams {
        task: t.id.clone(),
        title: "t".into(),
        draft: true,
        ..Default::default()
    }));
    assert_eq!(
        (e.code, e.data.as_ref().unwrap()["kind"].as_str()),
        (pr_codes::FORGE_FAILED, Some("not_logged_in"))
    );
    assert!(e.message.contains("gh auth login"), "{}", e.message);
    assert!(
        forge.state().prs.is_empty(),
        "no pull request after a failed push"
    );

    // Push works now; a draft is created without pushing again (--no-push path).
    let pushes_before = forge
        .state()
        .calls
        .iter()
        .filter(|c| c.starts_with("push"))
        .count();
    forge.edit(|s| {
        s.fail.clear();
        s.pushes.push(pod_forge::fake::FakePush {
            remote: "origin".into(),
            branch: "pod/err".into(),
        });
    });
    let i = c
        .call::<TaskPrCreate>(&TaskPrCreateParams {
            task: t.id.clone(),
            title: "t".into(),
            draft: true,
            push: Some(false),
            ..Default::default()
        })
        .unwrap();
    assert!(i.status.pr.as_ref().unwrap().draft);
    assert_eq!(
        forge
            .state()
            .calls
            .iter()
            .filter(|c| c.starts_with("push"))
            .count(),
        pushes_before,
        "push skipped"
    );
    // A draft cannot be merged; the forge's message comes through.
    let e = rpc_err(c.call::<TaskPrMerge>(&TaskPrMergeParams {
        task: t.id.clone(),
        method: MergeMethod::Merge,
    }));
    assert_eq!(e.code, pr_codes::FORGE_FAILED);
    assert!(e.message.contains("draft"), "{}", e.message);

    // Rate limited while refreshing: the error is reported, the record kept.
    forge.edit(|s| {
        s.fail.insert(
            "view".into(),
            ForgeError::classify(
                "gh",
                Some(1),
                "HTTP 403: API rate limit exceeded for user ID 1.",
            ),
        );
    });
    let s = status(&mut c, &t.id);
    assert_eq!(
        s.status.error.as_ref().map(|e| e.kind),
        Some(ForgeErrorKind::RateLimited)
    );
    assert!(s.status.remote.is_some());
    assert_eq!(
        c.call::<TaskShow>(&TaskRef { task: t.id.clone() })
            .unwrap()
            .pr
            .map(|p| p.number),
        Some(1),
        "kept"
    );
    // ...and the answer still names the last known pull request (seen against a
    // real repository: a `gh` timeout made `pod pr status` say "pr none").
    assert_eq!(
        s.status.pr.as_ref().map(|p| p.number),
        Some(1),
        "last known pull request, not none"
    );
    assert!(s.cached, "marked as not freshly read");
    let stored = c
        .call::<TaskShow>(&TaskRef { task: t.id.clone() })
        .unwrap()
        .pr
        .unwrap()
        .checks;
    assert!(
        stored.total > 0,
        "the draft started with checks: {stored:?}"
    );
    assert_eq!(s.status.summary, stored);

    // The pull request is readable but its checks time out: the stored summary stays.
    forge.edit(|s| {
        s.fail.clear();
        s.fail.insert(
            "checks".into(),
            ForgeError::new(
                ForgeErrorKind::Timeout,
                "gh pr checks did not finish within 30 s",
            ),
        );
    });
    let s = status(&mut c, &t.id);
    assert_eq!(
        s.status.error.as_ref().map(|e| e.kind),
        Some(ForgeErrorKind::Timeout)
    );
    assert_eq!(s.status.pr.as_ref().map(|p| p.number), Some(1));
    assert_eq!(s.status.summary, stored, "checks summary not reset to zero");
    assert_eq!(
        c.call::<TaskShow>(&TaskRef { task: t.id.clone() })
            .unwrap()
            .pr
            .unwrap()
            .checks,
        stored
    );
    let _ = std::fs::remove_file(&fake);
}

/// `pod pr` against the fake forge: status JSON, open --print, merge needs --yes.
#[test]
fn pod_pr_cli() {
    let repo = TempRepo::new(Some("https://github.com/pod-fixtures/demo"));
    let fake = std::env::temp_dir()
        .join(unique("fake-forge"))
        .with_extension("json");
    let daemon = Daemon::start_env(
        "pr-cli",
        &[
            ("POD_FORGE", &format!("fake:{}", fake.display())),
            ("POD_PR_POLL", "off"),
        ],
    );
    let mut c = daemon.client();
    let t = task(&mut c, &repo, "cli");
    commit_in(&t.worktree, "c.txt", "c\n");
    let pod = |args: &[&str]| {
        let out = Command::new(pod_exe())
            .arg("--pipe")
            .arg(daemon.pipe.trim_start_matches(r"\\.\pipe\"))
            .args(args)
            .output()
            .expect("run pod");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let (code, out, err) = pod(&[
        "pr",
        "create",
        &t.id,
        "--title",
        "CLI 제목",
        "--body",
        "본문",
    ]);
    assert_eq!(code, Some(0), "{out}\n{err}");
    assert!(
        out.contains("#1 open") && out.contains("https://github.com/pod-fixtures/demo/pull/1"),
        "{out}"
    );
    let (code, out, _) = pod(&["pr", "status", &t.id, "--json"]);
    assert_eq!(code, Some(0));
    let v: serde_json::Value = serde_json::from_str(&out).expect("json");
    assert_eq!(
        (v["pr"]["number"].as_u64(), v["summary"]["pending"].as_u64()),
        (Some(1), Some(3))
    );
    let (code, out, _) = pod(&["pr", "open", &t.id, "--print"]);
    assert_eq!(
        (code, out.trim()),
        (Some(0), "https://github.com/pod-fixtures/demo/pull/1")
    );
    let (code, _, err) = pod(&["pr", "merge", &t.id, "--method", "squash"]);
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("--yes"), "{err}");
    // The preview shows the checks and warns (GitHub decides; a repository without
    // branch protection accepts the merge, as seen on a real private repository).
    assert!(
        err.contains("checks: 8 passed, 3 pending (pending)")
            && err.contains("warning: 3 checks still running"),
        "{err}"
    );
    assert_eq!(
        FakeForge::load_file(&fake).prs[0].view.state,
        PrState::Open,
        "nothing merged without --yes"
    );
    let (code, out, err) = pod(&["pr", "merge", &t.id, "--method", "rebase", "--yes"]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(
        err.contains("warning: 3 checks still running; merging anyway"),
        "{err}"
    );
    assert!(out.contains("#1 merged"), "{out}");
    assert!(
        !out.contains("mergeable"),
        "no merge state for a merged pull request: {out}"
    );
    let _ = std::fs::remove_file(&fake);
}
