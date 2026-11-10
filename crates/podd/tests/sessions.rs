//! Worktree sessions (docs/19 §10 S1–S3) with the real `podd`/`pod-holder`,
//! git, and fake agents only (a fake `claude.cmd` on the task's PATH, the
//! `fake-agent` example for hook payloads). Never a real Claude or Codex.
//!
//! - S1: auto names skip retired names, `--branch`, qualified bases (tag vs
//!   branch, remote vs local), a multi-line brief with `" %` through a `.cmd`
//!   agent (argv first line and typed first line), environment variables.
//! - S2: `.pod/config.toml` commands do not run before approval, a changed
//!   file asks again, a failing setup does not fail the task, `wait` mode
//!   types the brief only after setup, tabs, personal overrides.
//! - S3: archive script failure changes nothing, locked / nested worktrees
//!   are refused, the branch is deleted only when merged, a hook payload
//!   records the session id and `task.resume` runs `claude --resume <id>`.

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use std::time::{Duration, Instant};

use common::*;
use podd::client::Client;
use podd::protocol::*;

struct TempRepo {
    root: PathBuf,
    path: PathBuf,
}

impl TempRepo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("sess-repo"));
        let path = root.join("my repo");
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git(&path, &["config", k, v]);
        }
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "init"]);
        Self { root, path }
    }

    fn commit_config(&self, toml: &str) {
        std::fs::create_dir_all(self.path.join(".pod")).unwrap();
        std::fs::write(self.path.join(".pod").join("config.toml"), toml).unwrap();
        git(&self.path, &["add", "-A"]);
        git(&self.path, &["commit", "-q", "-m", "config"]);
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
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn add_repo(c: &mut Client, repo: &TempRepo, name: &str) {
    c.call::<RepoAdd>(&RepoAddParams {
        path: repo.path.display().to_string(),
        name: Some(name.into()),
    })
    .expect("repo.add");
}

fn show(c: &mut Client, task: &str) -> TaskInfo {
    c.call::<TaskShow>(&TaskRef { task: task.into() })
        .expect("task.show")
}

fn wait_task(c: &mut Client, task: &str, what: &str, ok: impl Fn(&TaskInfo) -> bool) -> TaskInfo {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let t = show(c, task);
        if ok(&t) {
            return t;
        }
        assert!(Instant::now() < deadline, "{what}: {t:#?}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn shell() -> AgentSpec {
    AgentSpec::Kind(AgentKind::Shell)
}

fn screen(c: &mut Client, t: &str) -> String {
    read(c, t, ReadMode::History).join("\n")
}

/// A directory with a fake `claude.cmd`: prints its arguments, the brief
/// file, then echoes typed lines (and whether setup already ran).
fn fake_claude_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(unique("fake-claude"));
    std::fs::create_dir_all(&dir).unwrap();
    let script = "@echo off\r\n\
        echo FAKE-ARGS=[%*]\r\n\
        if defined POD_BRIEF type \"%POD_BRIEF%\"\r\n\
        echo BRIEF-END\r\n\
        :loop\r\n\
        set LINE=\r\n\
        set /p LINE=\r\n\
        if exist setup-done.txt (echo TYPED-AFTER-SETUP=[%LINE%]) else (echo TYPED-BEFORE-SETUP=[%LINE%])\r\n\
        goto loop\r\n";
    std::fs::write(dir.join("claude.cmd"), script).unwrap();
    std::fs::write(dir.join("fake-typed.cmd"), script).unwrap();
    dir
}

/// PATH with the fake agent directory first.
fn fake_env(dir: &Path) -> BTreeMap<String, String> {
    let path = format!(
        "{};{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    BTreeMap::from([("PATH".to_string(), path)])
}

const BRIEF: &str = "Fix the \"login\" bug.\r\nIt fails 100% of the time & | < > ^\nLine three: %PATH% stays literal.\n";

// ───────────────────────────── S1 ─────────────────────────────

#[test]
fn s1_names_branches_bases_and_briefs() {
    let repo = TempRepo::new();
    let d = Daemon::start("sess-s1");
    let mut c = d.client();
    add_repo(&mut c, &repo, "app");

    // Auto name; a removed task's name is retired and never suggested again.
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .expect("auto-named task");
    assert!(
        !t.name.is_empty() && t.branch == format!("pod/{}", t.name),
        "{t:?}"
    );
    let first = t.name.clone();
    c.call::<TaskRemove>(&TaskRemoveParams {
        task: t.id.clone(),
        force: true,
        delete_branch: true,
    })
    .unwrap();
    let retired = std::fs::read_to_string(d.state_dir.join("tasks").join("retired.json")).unwrap();
    assert!(retired.contains(&format!("\"{first}\"")), "{retired}");
    for _ in 0..40 {
        let s = c
            .call::<TaskSuggestName>(&SuggestNameParams { repo: "app".into() })
            .unwrap();
        assert_ne!(s.name, first, "retired name suggested again");
    }

    // Qualified bases: a tag must not shadow the branch of the same name;
    // with a slash the remote-tracking branch wins over a local look-alike.
    let init = git(&repo.path, &["rev-parse", "HEAD"]).trim().to_string();
    git(&repo.path, &["tag", "amb", &init]);
    git(&repo.path, &["checkout", "-q", "-b", "amb"]);
    std::fs::write(repo.path.join("b.txt"), "b\n").unwrap();
    git(&repo.path, &["add", "-A"]);
    git(&repo.path, &["commit", "-q", "-m", "on branch amb"]);
    let amb = git(&repo.path, &["rev-parse", "refs/heads/amb"])
        .trim()
        .to_string();
    git(&repo.path, &["checkout", "-q", "main"]);
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "q1".into(),
            base: Some("amb".into()),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        (t.base.as_str(), t.base_commit.as_str()),
        ("refs/heads/amb", amb.as_str())
    );
    git(
        &repo.path,
        &["update-ref", "refs/remotes/origin/feat", &init],
    );
    git(&repo.path, &["branch", "origin/feat", &amb]);
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "q2".into(),
            base: Some("origin/feat".into()),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        (t.base.as_str(), t.base_commit.as_str()),
        ("refs/remotes/origin/feat", init.as_str())
    );
    // An unambiguous name stays as given.
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "q3".into(),
            base: Some("main".into()),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(t.base, "main");

    // --branch: a new name, and an existing branch checked out as is.
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "br1".into(),
            branch: Some("feature/new-one".into()),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        (t.branch.as_str(), t.branch_existed),
        ("feature/new-one", false)
    );
    git(&repo.path, &["branch", "feature/old", &amb]);
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "br2".into(),
            branch: Some("feature/old".into()),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    assert!(t.branch_existed);
    assert_eq!(
        t.git.as_ref().and_then(|g| g.head.clone()).as_deref(),
        Some(amb.as_str())
    );
    // Existing branches are never deleted by Pod.
    let r = c
        .call::<TaskRemove>(&TaskRemoveParams {
            task: "app/br2".into(),
            force: true,
            delete_branch: true,
        })
        .unwrap();
    assert!(
        !r.branch_deleted && r.branch_kept.as_deref().unwrap_or("").contains("existed"),
        "{r:?}"
    );
    assert!(git(&repo.path, &["branch", "--list", "feature/old"]).contains("feature/old"));

    // A brief with quotes, % and line breaks through a .cmd agent (claude.cmd
    // shim on PATH): the agent gets the safe first line as its argument and
    // reads the brief file named by POD_BRIEF verbatim.
    let fake = fake_claude_dir();
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "brief-argv".into(),
            agent: Some(AgentSpec::Kind(AgentKind::Claude)),
            brief: Some(BRIEF.into()),
            env: fake_env(&fake),
            ..Default::default()
        })
        .expect("a multi-line brief is accepted for a .cmd agent");
    let b = t.brief.clone().expect("brief info");
    assert_eq!(
        (b.delivery, b.state),
        (BriefDelivery::Argv, BriefState::Sent)
    );
    assert!(!b.first_line.contains(['"', '%', '\r', '\n']));
    let term = t.terminals[0].id.clone();
    wait_output(&mut c, &term, "BRIEF-END");
    let out = screen(&mut c, &term);
    assert!(
        out.contains(&format!("FAKE-ARGS=[\"{}\"]", b.first_line)),
        "{out}"
    );
    for line in [
        "Fix the \"login\" bug.",
        "It fails 100% of the time & | < > ^",
        "Line three: %PATH% stays literal.",
    ] {
        assert!(out.contains(line), "{line:?} missing in {out}");
    }
    assert_eq!(
        std::fs::read_to_string(&b.path)
            .unwrap()
            .matches(BRIEF)
            .count(),
        1,
        "brief file keeps the text verbatim"
    );
    // The same text as a raw prompt is still refused for a .cmd agent.
    let e = rpc_code(c.call::<TaskCreate>(&TaskCreateParams {
        repo: "app".into(),
        name: "prompt-cmd".into(),
        agent: Some(AgentSpec::Kind(AgentKind::Claude)),
        prompt: Some(BRIEF.into()),
        env: fake_env(&fake),
        ..Default::default()
    }));
    assert_eq!(e, codes::INVALID_PARAMS);

    // An argv .cmd agent: the first line is typed, then Enter.
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "brief-typed".into(),
            agent: Some(AgentSpec::Argv {
                argv: vec![fake.join("fake-typed.cmd").display().to_string()],
            }),
            brief: Some(BRIEF.into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(t.brief.as_ref().unwrap().delivery, BriefDelivery::Typed);
    let term = t.terminals[0].id.clone();
    wait_output(
        &mut c,
        &term,
        &format!(
            "TYPED-BEFORE-SETUP=[{}]",
            t.brief.as_ref().unwrap().first_line
        ),
    );
    wait_task(&mut c, &t.id, "brief sent", |t| {
        t.brief
            .as_ref()
            .is_some_and(|b| b.state == BriefState::Sent)
    });

    // Environment: POD_TASK_ID, POD_BRIEF, and task env in every task terminal.
    let mut env = BTreeMap::new();
    env.insert("POD_TEST_EXTRA".to_string(), "extra-ok".to_string());
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "env".into(),
            agent: Some(shell()),
            brief: Some("shell brief".into()),
            env,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(t.brief.as_ref().unwrap().delivery, BriefDelivery::Env);
    let term = t.terminals[0].id.clone();
    send(
        &mut c,
        &term,
        "Write-Output \"ID=$env:POD_TASK_ID X=$env:POD_TEST_EXTRA\"; Get-Content $env:POD_BRIEF | Select-String 'shell brief'",
    );
    wait_output(&mut c, &term, "ID=app/env X=extra-ok");
    wait_output(&mut c, &term, "shell brief");
    // A worker-style open hint by default when created from a Pod terminal
    // is covered in the lifecycle test; an explicit hint is accepted here.
    c.call::<TaskCreate>(&TaskCreateParams {
        repo: "app".into(),
        name: "hint".into(),
        agent: Some(shell()),
        open_hint: Some(OpenHint {
            focus: false,
            reason: OpenReason::Worker,
        }),
        ..Default::default()
    })
    .unwrap();
    let _ = std::fs::remove_dir_all(&fake);
}

// ───────────────────────────── S2 ─────────────────────────────

#[test]
fn s2_config_approval_setup_and_wait_mode() {
    let repo = TempRepo::new();
    repo.commit_config(
        "[setup]\nscript = \"Set-Content ran.txt x\"\n\n[[tabs]]\ntitle = \"dev\"\ncommand = \"Write-Output TAB-OK\"\n",
    );
    let d = Daemon::start("sess-s2");
    let mut c = d.client();
    add_repo(&mut c, &repo, "app");
    let cfg = c
        .call::<RepoConfigMethod>(&RepoConfigParams {
            repo: "app".into(),
            task: None,
        })
        .unwrap();
    let hash = cfg.hash.clone().expect("commands hash");
    assert!(!cfg.trusted && cfg.commands.len() == 2, "{cfg:?}");

    // Not approved: nothing runs, the task is created, setup says untrusted.
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "u1".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    let s = t.setup.clone().expect("setup info");
    assert_eq!(
        (s.state, s.hash.as_deref()),
        (SetupState::Untrusted, Some(hash.as_str()))
    );
    assert_eq!(
        t.terminals.len(),
        1,
        "no setup or tab terminal before approval"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        !Path::new(&t.worktree).join("ran.txt").exists(),
        "setup ran without approval"
    );

    // Approve this exact content: setup runs, the tab opens.
    let a = c
        .call::<RepoSet>(&RepoSetParams {
            repo: "app".into(),
            trust: Some(TrustParams {
                hash: hash.clone(),
                approve: true,
            }),
            ..Default::default()
        })
        .unwrap();
    assert!(a.trusted);
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "a1".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(t.setup.as_ref().unwrap().state, SetupState::Running);
    let t = wait_task(&mut c, &t.id, "setup ok", |t| {
        t.setup.as_ref().is_some_and(|s| s.state == SetupState::Ok)
    });
    assert_eq!(t.setup.as_ref().unwrap().exit_code, Some(0));
    assert!(Path::new(&t.worktree).join("ran.txt").exists());
    let names: Vec<String> = t
        .terminals
        .iter()
        .filter_map(|x| x.info.as_ref()?.name.clone())
        .collect();
    assert!(names.contains(&"app/a1:dev".to_string()), "{names:?}");
    wait_output(&mut c, "app/a1:dev", "TAB-OK");

    // The file's commands change: asks again (old hash refused), not run.
    repo.commit_config("[setup]\nscript = \"Set-Content ran.txt changed\"\n");
    let cfg = c
        .call::<RepoConfigMethod>(&RepoConfigParams {
            repo: "app".into(),
            task: None,
        })
        .unwrap();
    assert!(!cfg.trusted && cfg.hash.as_deref() != Some(hash.as_str()));
    assert_eq!(
        rpc_code(c.call::<RepoSet>(&RepoSetParams {
            repo: "app".into(),
            trust: Some(TrustParams {
                hash: hash.clone(),
                approve: true
            }),
            ..Default::default()
        })),
        codes::INVALID_PARAMS
    );
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "c1".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(t.setup.as_ref().unwrap().state, SetupState::Untrusted);

    // Personal setup wins and needs no approval; a failing setup leaves the
    // task in place with `failed` and the exit code.
    c.call::<RepoSet>(&RepoSetParams {
        repo: "app".into(),
        personal: Some(PersonalConfig {
            setup: Some(SetupSection {
                script: Some("Write-Output SETUP-OUT; exit 3".into()),
                mode: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "f1".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .expect("a failing setup does not fail task creation");
    let t = wait_task(&mut c, &t.id, "setup failed", |t| {
        t.setup
            .as_ref()
            .is_some_and(|s| s.state == SetupState::Failed)
    });
    let s = t.setup.as_ref().unwrap();
    assert_eq!(
        (s.exit_code, s.source.as_deref()),
        (Some(3), Some("personal"))
    );
    assert!(
        t.terminals[0]
            .info
            .as_ref()
            .is_some_and(|i| i.state == TerminalState::Running),
        "agent still runs"
    );
    // setup: skip.
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "s1".into(),
            agent: Some(shell()),
            setup: Some(SetupPolicy::Skip),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(t.setup.as_ref().unwrap().state, SetupState::Skipped);

    // wait mode: the brief is typed only after setup finished.
    c.call::<RepoSet>(&RepoSetParams {
        repo: "app".into(),
        personal: Some(PersonalConfig {
            setup: Some(SetupSection {
                script: Some("Start-Sleep -Seconds 4; Set-Content setup-done.txt ok".into()),
                mode: Some(SetupMode::Wait),
            }),
            ..Default::default()
        }),
        ..Default::default()
    })
    .unwrap();
    let fake = fake_claude_dir();
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "w1".into(),
            agent: Some(AgentSpec::Argv {
                argv: vec![fake.join("fake-typed.cmd").display().to_string()],
            }),
            brief: Some(BRIEF.into()),
            ..Default::default()
        })
        .unwrap();
    let b = t.brief.clone().unwrap();
    assert_eq!(
        (b.delivery, b.state, b.after_setup),
        (BriefDelivery::Typed, BriefState::Pending, true)
    );
    let term = t.terminals[0].id.clone();
    wait_output(&mut c, &term, "BRIEF-END");
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        show(&mut c, &t.id).brief.unwrap().state,
        BriefState::Pending,
        "not typed during setup"
    );
    wait_output(
        &mut c,
        &term,
        &format!("TYPED-AFTER-SETUP=[{}]", b.first_line),
    );
    assert!(!screen(&mut c, &term).contains("TYPED-BEFORE-SETUP"));
    let t = wait_task(&mut c, &t.id, "brief sent", |t| {
        t.brief
            .as_ref()
            .is_some_and(|b| b.state == BriefState::Sent)
    });
    assert_eq!(t.setup.as_ref().unwrap().state, SetupState::Ok);
    let _ = std::fs::remove_dir_all(&fake);
}

// ───────────────────────────── S3 ─────────────────────────────

/// Kill a terminal and wait until its process exit is recorded.
/// `terminal.kill` itself waits at most 5 s; under load the holder can report
/// the exit later, and `task.resume` refuses a terminal that still runs.
fn kill_exited(c: &mut Client, t: &str) {
    c.call::<TerminalKill>(&TerminalRef::id(t)).unwrap();
    let w = c
        .call::<TerminalWait>(&WaitParams {
            target: TerminalRef::id(t),
            until: WaitUntil::Exit,
            timeout_ms: Some(60_000),
        })
        .expect("wait for exit");
    assert_eq!(
        w.outcome,
        WaitOutcome::Matched,
        "{t} did not exit after kill"
    );
}

#[test]
fn s3_archive_remove_guards_and_resume() {
    let repo = TempRepo::new();
    repo.commit_config("[archive]\nscript = \"Write-Output ARCHIVE-RAN; exit 4\"\n");
    let d = Daemon::start("sess-s3");
    let mut c = d.client();
    add_repo(&mut c, &repo, "app");

    // Archive script not approved / failing: nothing changes.
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "ar".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    let agent_term = t.terminals[0].id.clone();
    assert_eq!(
        rpc_code(c.call::<TaskArchive>(&TaskArchiveParams {
            task: t.id.clone(),
            ..Default::default()
        })),
        session_codes::ARCHIVE_FAILED,
        "not approved"
    );
    let hash = c
        .call::<RepoConfigMethod>(&RepoConfigParams {
            repo: "app".into(),
            task: None,
        })
        .unwrap()
        .hash
        .unwrap();
    c.call::<RepoSet>(&RepoSetParams {
        repo: "app".into(),
        trust: Some(TrustParams {
            hash,
            approve: true,
        }),
        ..Default::default()
    })
    .unwrap();
    let e = c.call::<TaskArchive>(&TaskArchiveParams {
        task: t.id.clone(),
        ..Default::default()
    });
    match e {
        Err(podd::client::ClientError::Rpc(e)) => {
            assert_eq!(e.code, session_codes::ARCHIVE_FAILED);
            let data = e.data.unwrap();
            assert_eq!(data["exit_code"], 4);
            assert!(
                data["output"].as_str().unwrap().contains("ARCHIVE-RAN"),
                "{data}"
            );
        }
        other => panic!("expected ARCHIVE_FAILED, got {other:?}"),
    }
    let after = show(&mut c, &t.id);
    assert_eq!(after.lifecycle, Lifecycle::Active);
    assert!(after.terminals.iter().any(|x| {
        x.id == agent_term
            && x.info
                .as_ref()
                .is_some_and(|i| i.state == TerminalState::Running)
    }));
    // Explicitly ignored: archived, terminals closed, worktree kept.
    let r = c
        .call::<TaskArchive>(&TaskArchiveParams {
            task: t.id.clone(),
            ignore_script_failure: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(r.terminals, vec![agent_term.clone()]);
    let after = show(&mut c, &t.id);
    assert_eq!(after.lifecycle, Lifecycle::Archived);
    assert!(after.terminals.is_empty() && after.worktree_exists);
    let cands = c
        .call::<TaskCleanupCandidates>(&CleanupParams::default())
        .unwrap();
    let cand = cands.candidates.iter().find(|x| x.task == t.id).unwrap();
    assert_eq!(cand.tier, CleanupTier::Ready, "{cand:?}");
    // Unarchive: active again with a fresh agent (no session recorded).
    let r = c
        .call::<TaskUnarchive>(&TaskUnarchiveParams {
            task: t.id.clone(),
            ..Default::default()
        })
        .unwrap();
    assert!(r.resumed_session.is_none() && r.terminal.is_some());
    assert_eq!(show(&mut c, &t.id).lifecycle, Lifecycle::Active);

    // Locked and nested worktrees are refused, even with force.
    let wt = PathBuf::from(&t.worktree);
    git(&repo.path, &["worktree", "lock", &t.worktree]);
    assert_eq!(
        rpc_code(c.call::<TaskRemove>(&TaskRemoveParams {
            task: t.id.clone(),
            force: true,
            delete_branch: false
        })),
        session_codes::WORKTREE_LOCKED
    );
    git(&repo.path, &["worktree", "unlock", &t.worktree]);
    let inner = wt.join("nested");
    git(
        &repo.path,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "nested-x",
            &inner.display().to_string(),
        ],
    );
    assert_eq!(
        rpc_code(c.call::<TaskRemove>(&TaskRemoveParams {
            task: t.id.clone(),
            force: true,
            delete_branch: false
        })),
        session_codes::WORKTREE_LOCKED
    );
    assert!(wt.is_dir() && inner.is_dir(), "nothing removed");
    git(
        &repo.path,
        &[
            "worktree",
            "remove",
            "--force",
            &inner.display().to_string(),
        ],
    );

    // The branch is deleted only with merge proof.
    send(
        &mut c,
        &r.terminal.as_ref().unwrap().id,
        "Set-Content f.txt x; git add f.txt; git commit -qm work; Write-Output COMMITTED",
    );
    wait_output(&mut c, &r.terminal.as_ref().unwrap().id, "COMMITTED");
    let rm = c
        .call::<TaskRemove>(&TaskRemoveParams {
            task: t.id.clone(),
            force: true,
            delete_branch: true,
        })
        .unwrap();
    assert!(rm.worktree_removed && !rm.branch_deleted, "{rm:?}");
    assert!(
        rm.branch_kept
            .as_deref()
            .unwrap_or("")
            .contains("not merged"),
        "{rm:?}"
    );
    assert!(git(&repo.path, &["branch", "--list", "pod/ar"]).contains("pod/ar"));
    let t2 = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "merged".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    let rm = c
        .call::<TaskRemove>(&TaskRemoveParams {
            task: t2.id.clone(),
            force: false,
            delete_branch: true,
        })
        .unwrap();
    assert!(
        rm.branch_deleted && rm.branch_kept.is_none(),
        "an unchanged branch is merged: {rm:?}"
    );

    // Session id from a (fake) hook payload, terminal killed, then resume
    // runs the agent's own command: `claude --resume <id>` (fake claude.cmd).
    let fake = fake_claude_dir();
    let mut env = fake_env(&fake);
    env.insert(
        "POD_HOOK_EXE".into(),
        fixture_bin("pod-hook.exe").display().to_string(),
    );
    // The fake agent waits for a line before its SessionStart hook: a hook
    // that fires before `task.create` registered the agent terminal is not
    // matched to the task (seen under load), so start it only afterwards.
    let agent = AgentSpec::Argv {
        argv: vec![
            fixture_bin("examples/fake-agent.exe").display().to_string(),
            "read".into(),
            r#"hook:claude:SessionStart:{"session_id":"sess-4b1d","hook_event_name":"SessionStart","source":"startup"}"#.into(),
            "hold".into(),
        ],
    };
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "res".into(),
            agent: Some(agent),
            env,
            ..Default::default()
        })
        .unwrap();
    send(&mut c, t.agent_terminal.as_deref().unwrap(), "go");
    let t = wait_task(&mut c, &t.id, "session recorded", |t| {
        t.agent_session.is_some()
    });
    let s = t.agent_session.clone().unwrap();
    assert_eq!(
        (s.agent.as_str(), s.id.as_str(), s.source.as_str()),
        ("claude", "sess-4b1d", "hook")
    );
    let old = t.agent_terminal.clone().unwrap();
    assert_eq!(
        rpc_code(c.call::<TaskResume>(&TaskResumeParams {
            task: t.id.clone(),
            ..Default::default()
        })),
        codes::TERMINAL_RUNNING,
        "no resume while the agent runs"
    );
    kill_exited(&mut c, &old);
    let r = c
        .call::<TaskResume>(&TaskResumeParams {
            task: t.id.clone(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(r.argv, ["claude", "--resume", "sess-4b1d"]);
    assert_eq!(r.resumed_session.as_deref(), Some("sess-4b1d"));
    let new = r.terminal.unwrap().id;
    wait_output(&mut c, &new, "FAKE-ARGS=[--resume sess-4b1d]");
    let after = show(&mut c, &t.id);
    assert_eq!(after.agent_terminal.as_deref(), Some(new.as_str()));
    assert_eq!(after.terminals[0].id, new);
    assert!(
        !after.terminals.iter().any(|x| x.id == old),
        "the dead agent terminal is gone"
    );
    // An explicit session (e.g. from discovery) for codex.
    kill_exited(&mut c, &new);
    let e = c.call::<TaskResume>(&TaskResumeParams {
        task: t.id.clone(),
        session: Some("bad id".into()),
        ..Default::default()
    });
    assert_eq!(rpc_code(e), codes::INVALID_PARAMS);
    let _ = std::fs::remove_dir_all(&fake);
}

/// docs/19 §14.3: an agent's SessionStart hook that arrives before
/// `task.create` has registered the agent terminal is kept and attached at
/// registration. The debug-only `POD_TEST_HOLD_TASK_REGISTER` makes
/// `task.create` wait until that hook is in, so the order is fixed (no
/// sleeps): the fake agent fires the hook as its first action.
#[test]
fn hook_before_task_registration_is_kept() {
    let repo = TempRepo::new();
    let d = Daemon::start_env("sess-early-hook", &[("POD_TEST_HOLD_TASK_REGISTER", "1")]);
    let mut c = d.client();
    add_repo(&mut c, &repo, "app");
    let mut env = BTreeMap::new();
    env.insert(
        "POD_HOOK_EXE".into(),
        fixture_bin("pod-hook.exe").display().to_string(),
    );
    let agent = AgentSpec::Argv {
        argv: vec![
            fixture_bin("examples/fake-agent.exe").display().to_string(),
            r#"hook:claude:SessionStart:{"session_id":"sess-early","hook_event_name":"SessionStart","source":"startup"}"#.into(),
            "hold".into(),
        ],
    };
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "early".into(),
            agent: Some(agent),
            env,
            ..Default::default()
        })
        .unwrap();
    let s = t
        .agent_session
        .clone()
        .expect("the early hook's session is on the created task");
    assert_eq!(
        (s.agent.as_str(), s.id.as_str(), s.source.as_str()),
        ("claude", "sess-early", "hook")
    );
    // Saved with the task (not only in the create result).
    let s = show(&mut c, &t.id).agent_session.unwrap();
    assert_eq!(s.id, "sess-early");
    drop(c);
    drop(d);
}

/// docs/19 §3.2 S8: `[worktree] sparse` in the base commit's config checks
/// out only those directories (plus root files and `.pod`, so setup still
/// reads the config) in new task worktrees; the main checkout stays full;
/// an invalid list is ignored (full checkout) instead of failing the task.
#[test]
fn s8_sparse_checkout_for_new_worktrees() {
    let repo = TempRepo::new();
    for f in [
        "crates/podd/src/lib.rs",
        "crates/pod-app/src/main.rs",
        "docs/a.md",
    ] {
        let p = repo.path.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x\n").unwrap();
    }
    repo.commit_config("[worktree]\nsparse = [\"crates/podd\", \"docs/\"]\n\n[setup]\nscript = \"Set-Content ran.txt x\"\n");
    let d = Daemon::start("sess-s8");
    let mut c = d.client();
    add_repo(&mut c, &repo, "app");
    let t = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "sp".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    let w = PathBuf::from(&t.worktree);
    assert!(w.join("crates/podd/src/lib.rs").exists());
    assert!(w.join("docs/a.md").exists());
    assert!(w.join("README.md").exists(), "root files are always there");
    assert!(
        w.join(".pod/config.toml").exists(),
        ".pod is always in the cone"
    );
    assert!(!w.join("crates/pod-app").exists(), "outside the sparse set");
    // The repository config was still read from the worktree (an unapproved setup).
    assert_eq!(
        t.setup.as_ref().map(|s| s.state),
        Some(SetupState::Untrusted),
        "{:?}",
        t.setup
    );
    let list = git(&w, &["sparse-checkout", "list"]);
    assert_eq!(
        list.lines().collect::<Vec<_>>(),
        vec![".pod", "crates/podd", "docs"],
        "{list}"
    );
    assert!(
        git(&w, &["status", "--porcelain"]).trim().is_empty(),
        "a clean worktree"
    );
    assert!(
        repo.path.join("crates/pod-app/src/main.rs").exists(),
        "the main checkout is untouched"
    );
    assert!(
        !repo.path.join(".git/info/sparse-checkout").exists(),
        "the main checkout is not sparse"
    );

    // An invalid list: a full checkout, the task is still created.
    repo.commit_config("[worktree]\nsparse = [\"../outside\"]\n");
    let t2 = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: "app".into(),
            name: "full".into(),
            agent: Some(shell()),
            ..Default::default()
        })
        .unwrap();
    assert!(
        Path::new(&t2.worktree)
            .join("crates/pod-app/src/main.rs")
            .exists()
    );
}
