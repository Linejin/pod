//! Session discovery through the daemon (docs/18): opt-in, list/scan,
//! register (with repository registration), resume with a fake agent
//! executable, hide, preview and off settings, `sessions.changed`.
//! Synthetic agent stores only; process scanning is off (harness).

mod common;

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use pod_test_support::git::git_ok;
use podd::client::SubscribeEvent;
use podd::protocol::*;

const LIVE: &str = "44444444-2222-4333-8444-555555555555";
const DEMO: &str = "11111111-2222-4333-8444-555555555555";

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

/// Fixture stores plus one fresh Claude session whose cwd is `cwd`.
fn stores(root: &Path, cwd: &Path) {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("pod-discover")
        .join("tests")
        .join("fixtures");
    copy_dir(&fixtures, root);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let ts = rfc3339(now);
    let dir = root.join("claude").join("projects").join("live");
    std::fs::create_dir_all(&dir).unwrap();
    let line = format!(
        r#"{{"type":"user","sessionId":"{LIVE}","cwd":{},"gitBranch":"main","timestamp":"{ts}","message":{{"role":"user","content":"Synthetic prompt for the live session"}}}}"#,
        json_str(&cwd.display().to_string())
    );
    std::fs::write(dir.join(format!("{LIVE}.jsonl")), format!("{line}\n")).unwrap();
}

fn rfc3339(secs: u64) -> String {
    let days = secs / 86_400;
    let rem = secs % 86_400;
    // civil_from_days (H. Hinnant).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[test]
fn discovery_end_to_end() {
    let scratch = std::env::temp_dir().join(unique("sessions"));
    // An unregistered repository; the live session ran in a sub-folder.
    let repo = scratch.join("live repo");
    let work = repo.join("sub dir");
    std::fs::create_dir_all(&work).unwrap();
    git_ok(&repo, &["init", "-q", "-b", "main"]);
    let store = scratch.join("stores");
    stores(&store, &work);
    // Fake agent: prints its arguments, never a real Claude/Codex.
    let fake = scratch.join("fake-claude.cmd");
    std::fs::write(&fake, "@echo FAKE-RESUME %*\r\n").unwrap();
    let claude_dir = store.join("claude");
    let codex_dir = store.join("codex");
    let d = Daemon::start_env(
        "sessions",
        &[
            ("POD_DISCOVER_CLAUDE_DIR", claude_dir.to_str().unwrap()),
            ("POD_DISCOVER_CODEX_DIR", codex_dir.to_str().unwrap()),
            ("POD_SESSIONS_CLAUDE_EXE", fake.to_str().unwrap()),
        ],
    );
    let mut c = d.client();

    // Off by default: nothing listed, actions refused.
    let l = c
        .call::<SessionsListMethod>(&SessionsListParams::default())
        .unwrap();
    assert!(!l.enabled && l.sessions.is_empty());
    assert_eq!(
        rpc_code(c.call::<SessionsScan>(&SessionsScanParams::default())),
        sessions_codes::DISCOVERY_OFF
    );
    assert_eq!(
        rpc_code(c.call::<SessionsResume>(&SessionsResumeParams {
            session: LIVE.into(),
            name: None
        })),
        sessions_codes::DISCOVERY_OFF
    );
    assert!(
        !d.state_dir.join("sessions").join("cache.json").exists(),
        "nothing read while off"
    );

    // Events for lifecycle subscribers.
    let (_, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams {
            lifecycle: true,
            ..Default::default()
        })
        .unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if let SubscribeEvent::SessionsChanged(e) = ev
                && tx.send(*e).is_err()
            {
                break;
            }
        }
    });

    // Turn on and scan.
    let l = c
        .call::<SessionsScan>(&SessionsScanParams {
            enable: true,
            include_hidden: false,
        })
        .unwrap();
    assert!(l.enabled && l.preview);
    assert!(l.scanned_ms.is_some());
    let keys: Vec<&str> = l.sessions.iter().map(|s| s.key.as_str()).collect();
    assert_eq!(l.sessions.len(), 6, "{keys:?}");
    assert_eq!(l.sessions[0].id, LIVE, "newest first");
    let live = &l.sessions[0];
    assert_eq!(live.state, SessionState::Resumable);
    assert!(live.matched.is_empty(), "repository not registered yet");
    assert_eq!(
        live.preview.as_deref(),
        Some("Synthetic prompt for the live session")
    );
    assert_eq!(
        live.resume_argv,
        [fake.display().to_string(), "--resume".into(), LIVE.into()]
    );
    let demo = l.sessions.iter().find(|s| s.id == DEMO).unwrap();
    assert_eq!(
        demo.state,
        SessionState::Stale,
        "C:\\src\\demo does not exist"
    );
    assert!(!demo.cwd_exists);
    let deadline = Instant::now() + Duration::from_secs(20);
    let ev = loop {
        let ev = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("sessions.changed");
        if ev.enabled && ev.count == 6 {
            break ev;
        }
    };
    assert_eq!(ev.running, 0);

    // Register: unknown repository -> refused without add_repo, then registered.
    assert_eq!(
        rpc_code(c.call::<SessionsRegister>(&SessionsRegisterParams {
            session: LIVE.into(),
            ..Default::default()
        })),
        orch_codes::INVALID_STATE
    );
    let s = c
        .call::<SessionsRegister>(&SessionsRegisterParams {
            session: "4444".into(),
            task: None,
            add_repo: true,
        })
        .unwrap();
    let repos = c.call::<RepoList>(&Empty {}).unwrap().repos;
    assert_eq!(repos.len(), 1);
    assert!(
        PathBuf::from(&repos[0].path).ends_with("live repo"),
        "{}",
        repos[0].path
    );
    assert_eq!(s.matched.repo.as_deref(), Some(repos[0].name.as_str()));
    assert_eq!(
        s.link.as_ref().unwrap().repo.as_deref(),
        Some(repos[0].name.as_str())
    );

    // Resume: the agent's own command in the session's cwd (fake executable).
    let t = c
        .call::<SessionsResume>(&SessionsResumeParams {
            session: format!("claude:{LIVE}"),
            name: Some("resumed".into()),
        })
        .unwrap();
    assert_eq!(PathBuf::from(&t.cwd), work);
    wait_output(&mut c, &t.id, &format!("FAKE-RESUME --resume {LIVE}"));
    // A session whose folder is gone cannot be resumed.
    assert_eq!(
        rpc_code(c.call::<SessionsResume>(&SessionsResumeParams {
            session: DEMO.into(),
            name: None
        })),
        orch_codes::INVALID_STATE
    );
    // Ambiguous or unknown keys.
    assert_eq!(
        rpc_code(c.call::<SessionsHide>(&SessionsHideParams {
            session: "codex:nope".into(),
            hidden: None
        })),
        sessions_codes::SESSION_NOT_FOUND
    );

    // Hide / show.
    let h = c
        .call::<SessionsHide>(&SessionsHideParams {
            session: DEMO.into(),
            hidden: None,
        })
        .unwrap();
    assert!(h.hidden);
    let l = c
        .call::<SessionsListMethod>(&SessionsListParams::default())
        .unwrap();
    assert_eq!((l.sessions.len(), l.hidden), (5, 1));
    let l = c
        .call::<SessionsListMethod>(&SessionsListParams {
            include_hidden: true,
        })
        .unwrap();
    assert_eq!(l.sessions.len(), 6);

    // Previews off: gone from the list and from the stored cache.
    let cache = d.state_dir.join("sessions").join("cache.json");
    assert!(
        std::fs::read_to_string(&cache)
            .unwrap()
            .contains("Synthetic prompt")
    );
    let l = c
        .call::<SessionsSettings>(&SessionsSettingsParams {
            enabled: None,
            preview: Some(false),
        })
        .unwrap();
    assert!(!l.preview && l.sessions.iter().all(|s| s.preview.is_none()));
    assert!(
        !std::fs::read_to_string(&cache)
            .unwrap()
            .contains("Synthetic prompt")
    );
    let l = c
        .call::<SessionsScan>(&SessionsScanParams::default())
        .unwrap();
    assert!(
        l.sessions.iter().all(|s| s.preview.is_none()),
        "a rescan keeps previews off"
    );

    // The agents' files were only read.
    let after = std::fs::read_to_string(
        claude_dir
            .join("projects")
            .join("live")
            .join(format!("{LIVE}.jsonl")),
    )
    .unwrap();
    assert!(after.contains("Synthetic prompt for the live session") && after.lines().count() == 1);

    // A session that ran at the root of a Pod task worktree resumes the way
    // `task.resume` does: the task's agent terminal, session recorded on the
    // task (source "discovery"). Fake `claude.cmd` on the task's PATH.
    git_ok(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let fake_dir = scratch.join("fake-bin");
    std::fs::create_dir_all(&fake_dir).unwrap();
    std::fs::write(
        fake_dir.join("claude.cmd"),
        "@echo TASK-RESUME %*\r\n@pause >nul\r\n",
    )
    .unwrap();
    let path = format!(
        "{};{}",
        fake_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let repo_name = c.call::<RepoList>(&Empty {}).unwrap().repos[0].name.clone();
    let task = c
        .call::<TaskCreate>(&TaskCreateParams {
            repo: repo_name.clone(),
            name: "disc".into(),
            agent: Some(AgentSpec::Kind(AgentKind::Shell)),
            env: std::collections::BTreeMap::from([("PATH".to_string(), path)]),
            ..Default::default()
        })
        .unwrap();
    for t in &task.terminals {
        let _ = c.call::<TerminalKill>(&TerminalRef::id(&t.id));
    }
    const IN_TASK: &str = "77777777-2222-4333-8444-555555555555";
    let line = format!(
        r#"{{"type":"user","sessionId":"{IN_TASK}","cwd":{},"timestamp":"{}","message":{{"role":"user","content":"In the task worktree"}}}}"#,
        json_str(&task.worktree),
        rfc3339(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        )
    );
    std::fs::write(
        claude_dir
            .join("projects")
            .join("live")
            .join(format!("{IN_TASK}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
    let l = c
        .call::<SessionsScan>(&SessionsScanParams::default())
        .unwrap();
    let s = l.sessions.iter().find(|s| s.id == IN_TASK).expect("found");
    assert_eq!(s.matched.task.as_deref(), Some(task.id.as_str()));
    let t = c
        .call::<SessionsResume>(&SessionsResumeParams {
            session: IN_TASK.into(),
            name: None,
        })
        .unwrap();
    assert_eq!(
        t.argv,
        ["claude", "--resume", IN_TASK],
        "same command as task.resume"
    );
    wait_output(&mut c, &t.id, &format!("TASK-RESUME --resume {IN_TASK}"));
    let shown = c
        .call::<TaskShow>(&TaskRef {
            task: task.id.clone(),
        })
        .unwrap();
    assert_eq!(shown.agent_terminal.as_deref(), Some(t.id.as_str()));
    let rec = shown.agent_session.expect("session recorded on the task");
    assert_eq!(
        (rec.agent.as_str(), rec.id.as_str(), rec.source.as_str()),
        ("claude", IN_TASK, "discovery")
    );

    // Off: forgets what was read, keeps the user's choices.
    let l = c
        .call::<SessionsSettings>(&SessionsSettingsParams {
            enabled: Some(false),
            preview: None,
        })
        .unwrap();
    assert!(!l.enabled && l.sessions.is_empty());
    assert!(!cache.exists());
    let stored =
        std::fs::read_to_string(d.state_dir.join("sessions").join("sessions.json")).unwrap();
    assert!(
        stored.contains(&format!("claude:{DEMO}")),
        "hidden flag kept"
    );

    drop(c);
    drop(d);
    let _ = std::fs::remove_dir_all(&scratch);
}

fn rpc_error(e: podd::client::ClientError) -> RpcError {
    match e {
        podd::client::ClientError::Rpc(e) => e,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}

/// Opens `path` with no sharing (an injected disk failure: writes, renames
/// and deletes of it fail until the handle drops).
fn lock(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
        .unwrap()
}

/// security-04 / orch-05: when the disk cannot follow, "previews off" and
/// "off" are errors that name what is left, never a silent success; memory
/// is private at once, and the cleanup is retried on the next call and on
/// restart. Leftover temporary and corrupt cache files are deleted too.
#[test]
fn discovery_off_and_preview_off_report_disk_failures() {
    let scratch = std::env::temp_dir().join(unique("sessions-persist"));
    let work = scratch.join("work");
    std::fs::create_dir_all(&work).unwrap();
    let store = scratch.join("stores");
    stores(&store, &work);
    let (claude_dir, codex_dir) = (store.join("claude"), store.join("codex"));
    let env = [
        ("POD_DISCOVER_CLAUDE_DIR", claude_dir.to_str().unwrap()),
        ("POD_DISCOVER_CODEX_DIR", codex_dir.to_str().unwrap()),
    ];
    let d = Daemon::start_env("sessions-persist", &env);
    let mut c = d.client();
    let dir = d.state_dir.join("sessions");
    let cache = dir.join("cache.json");
    c.call::<SessionsScan>(&SessionsScanParams {
        enable: true,
        include_hidden: false,
    })
    .unwrap();
    assert!(
        std::fs::read_to_string(&cache)
            .unwrap()
            .contains("Synthetic prompt")
    );

    // Previews off while cache.json cannot be replaced.
    let held = lock(&cache);
    let e = rpc_error(
        c.call::<SessionsSettings>(&SessionsSettingsParams {
            enabled: None,
            preview: Some(false),
        })
        .unwrap_err(),
    );
    assert_eq!(e.code, sessions_codes::PERSIST_FAILED, "{e:?}");
    assert!(
        e.data.as_ref().unwrap()["paths"]
            .to_string()
            .contains("cache.json"),
        "{e:?}"
    );
    let l = c
        .call::<SessionsListMethod>(&SessionsListParams::default())
        .unwrap();
    assert!(
        !l.preview && !l.sessions.is_empty() && l.sessions.iter().all(|s| s.preview.is_none()),
        "memory is private at once"
    );
    assert!(
        !dir.join("cache.json.tmp").exists(),
        "a failed write leaves no temporary copy"
    );

    // Off while it is still locked; leftovers of older versions as well.
    std::fs::write(dir.join("cache.corrupt-1.json"), "Synthetic prompt").unwrap();
    let e = rpc_error(
        c.call::<SessionsSettings>(&SessionsSettingsParams {
            enabled: Some(false),
            preview: None,
        })
        .unwrap_err(),
    );
    assert_eq!(e.code, sessions_codes::PERSIST_FAILED, "{e:?}");
    assert!(e.message.contains("cache.json"), "{}", e.message);
    let l = c
        .call::<SessionsListMethod>(&SessionsListParams::default())
        .unwrap();
    assert!(!l.enabled && l.sessions.is_empty());
    assert!(cache.exists() && !dir.join("cache.corrupt-1.json").exists());

    // The disk is back: the next call finishes the cleanup.
    drop(held);
    c.call::<SessionsListMethod>(&SessionsListParams::default())
        .unwrap();
    assert!(!cache.exists(), "deleted on retry");

    // A settings write that fails is an error too; the retry saves it and a
    // restart sees it.
    let settings = dir.join("sessions.json");
    let held = lock(&settings);
    let e = rpc_error(
        c.call::<SessionsSettings>(&SessionsSettingsParams {
            enabled: None,
            preview: Some(true),
        })
        .unwrap_err(),
    );
    assert_eq!(e.code, sessions_codes::PERSIST_FAILED);
    assert!(e.message.contains("sessions.json"), "{}", e.message);
    drop(held);
    c.call::<SessionsListMethod>(&SessionsListParams::default())
        .unwrap();
    drop(c);
    let (pipe, state_dir) = d.stop(false);
    // A stray cache while off (an "off" whose cleanup never finished) goes at start.
    std::fs::write(&cache, "{}").unwrap();
    let d = Daemon::start_full(pipe, state_dir, &[], &env);
    let mut c = d.client();
    let l = c
        .call::<SessionsListMethod>(&SessionsListParams::default())
        .unwrap();
    assert!(!l.enabled && l.preview, "off and previews on, as saved");
    assert!(!cache.exists(), "no cache while off after a restart");
    drop(c);
    drop(d);
    let _ = std::fs::remove_dir_all(&scratch);
}
