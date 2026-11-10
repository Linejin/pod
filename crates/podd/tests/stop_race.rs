//! The window right after `server.stop` (docs/03 §server.stop, docs/15 §9):
//! a client that connects or sends a request while the daemon is shutting
//! down gets a clear `SERVER_STOPPING` refusal or waits for the next daemon —
//! never an unserved pipe instance ("daemon closed the connection").
//! `pod server stop` returns only after the daemon process has exited, and
//! the next command auto-starts a fresh daemon right away.
//!
//! Auto-started daemons write their log and state under `%LOCALAPPDATA%`;
//! this test binary points `LOCALAPPDATA` (and the other roots) at a temp dir.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use common::*;
use podd::client::{Client, ClientError, ProcessWatch};
use podd::protocol::*;

/// A sandbox for daemons auto-started from this process (never the user's
/// real `%LOCALAPPDATA%\pod`, worktrees or mods). Set once, before any test
/// body runs, and the tests are serialized.
fn sandbox() -> std::sync::MutexGuard<'static, PathBuf> {
    static DIR: OnceLock<Mutex<PathBuf>> = OnceLock::new();
    let m = DIR.get_or_init(|| {
        let dir = std::env::temp_dir().join(unique("stop-race-home"));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: done once, before any test in this binary spawns a thread
        // that reads the environment (every test takes this lock first).
        unsafe {
            std::env::set_var("LOCALAPPDATA", &dir);
            std::env::set_var("POD_WORKTREES_ROOT", dir.join("worktrees"));
            std::env::set_var("POD_MODS_DIR", dir.join("extensions"));
            std::env::set_var("POD_HOLDER", env!("CARGO_BIN_EXE_pod-holder"));
        }
        Mutex::new(dir)
    });
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A `terminal.wait` that never matches, on its own connection: keeps a
/// request in flight while the daemon shuts down (it allows them a grace).
fn wait_forever(
    pipe: &str,
    target: &str,
) -> std::thread::JoinHandle<Result<WaitResult, ClientError>> {
    let mut c = Client::connect(pipe).expect("connect");
    let target = target.to_string();
    std::thread::spawn(move || {
        c.call::<TerminalWait>(&WaitParams {
            target: TerminalRef::parse(&target),
            until: WaitUntil::OutputContains("never-printed-0xC0FFEE".into()),
            timeout_ms: Some(60_000),
        })
    })
}

#[test]
fn requests_and_connects_during_shutdown_never_reach_a_half_dead_daemon() {
    let _home = sandbox();
    let mut daemon = Daemon::start("stop-inflight");
    let pipe = daemon.pipe.clone();
    let mut c = daemon.client();
    let ps = powershell(&mut c, "keep");
    let old_pid = daemon.child.id();
    let watch = ProcessWatch::open(old_pid).expect("daemon process");

    // Connected before the stop, used after it.
    let mut early = daemon.client();
    let waiter = wait_forever(&pipe, "keep");
    // Let the wait reach the daemon.
    std::thread::sleep(Duration::from_millis(300));

    let r = c.call::<ServerStop>(&StopParams { kill: false }).unwrap();
    assert!(r.stopping);
    drop(c);

    // 1. A request on a connection made before the stop is refused clearly
    //    (nothing done), not served by a daemon that is going away.
    let st = early.call::<ServerStatus>(&Empty {});
    assert_eq!(
        rpc_code(st),
        codes::SERVER_STOPPING,
        "a request after server.stop must get SERVER_STOPPING"
    );
    let e = early.call::<TerminalList>(&Empty {}).unwrap_err();
    assert!(e.is_stopping(), "{e}");
    assert!(e.to_string().contains("stopping"), "{e}");

    // 2. A new connection does not land on an instance nobody serves: it
    //    waits while the old daemon still exists, then finds no daemon.
    let t0 = Instant::now();
    match Client::connect(&pipe) {
        Err(ClientError::NotRunning) => {}
        Ok(mut c) => panic!(
            "connected to the stopping daemon after {:?}: {:?}",
            t0.elapsed(),
            c.call::<ServerStatus>(&Empty {})
        ),
        Err(e) => panic!("unexpected connect error: {e}"),
    }
    assert!(
        watch.wait(Duration::from_secs(2)),
        "the pipe went away before the daemon process exited"
    );
    // The in-flight wait was cut off at the grace and saw the connection close.
    let w = waiter.join().unwrap();
    assert!(
        matches!(&w, Err(e) if !matches!(e, ClientError::Rpc(_))),
        "in-flight wait: {w:?}"
    );
    let _ = daemon.child.wait();

    // 3. Terminals stay alive in their holders (default stop) and the next
    //    daemon on the same pipe and state re-attaches them.
    assert!(process_alive(ps.holder_pid.unwrap()), "holder died");
    let pipe = std::mem::take(&mut daemon.pipe);
    let state_dir = std::mem::take(&mut daemon.state_dir);
    std::mem::forget(daemon);
    let daemon = Daemon::start_on(pipe, state_dir);
    let mut c = daemon.client();
    let list = c.call::<TerminalList>(&Empty {}).unwrap().terminals;
    assert_eq!(list.len(), 1, "{list:?}");
    assert_eq!(list[0].state, TerminalState::Running);
    assert_eq!(list[0].pid, ps.pid);
}

/// daemon-02: `server.stop kill:true` while a `terminal.run` is still
/// starting its holder. Whatever the run answers, neither the holder nor its
/// child may outlive the stop. The holder is slowed down with a debug-only
/// knob so the stop lands deterministically inside the start.
#[test]
fn stop_kill_also_ends_a_terminal_that_is_still_starting() {
    let _home = sandbox();
    let mut daemon = Daemon::start_env(
        "stop-kill-starting",
        &[("POD_HOLDER_TEST_START_DELAY_MS", "1500")],
    );
    let record = daemon.state_dir.join("t1.json");
    let mut runner = daemon.client();
    let run = std::thread::spawn(move || {
        runner.call::<TerminalRun>(&RunParams {
            argv: vec!["powershell".into(), "-NoLogo".into(), "-NoProfile".into()],
            cwd: cwd(),
            name: Some("late".into()),
            ..Default::default()
        })
    });
    // The run is in flight: its holder sleeps before serving its pipe.
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        !record.exists(),
        "the holder started too early for this test"
    );
    let r = daemon
        .client()
        .call::<ServerStop>(&StopParams { kill: true })
        .unwrap();
    assert!(r.stopping);
    daemon.wait_exit();
    let run = run.join().unwrap();

    // The holder removes its record when it is released; give it a moment.
    let deadline = Instant::now() + Duration::from_secs(10);
    while record.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let leaked: Option<(u32, u32)> = std::fs::read(&record)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .map(|v| {
            (
                v["holder_pid"].as_u64().unwrap_or(0) as u32,
                v["child_pid"].as_u64().unwrap_or(0) as u32,
            )
        });
    let (holder, child) = match (&run, leaked) {
        (_, Some(p)) => p,
        (Ok(info), None) => (info.holder_pid.unwrap_or(0), info.pid.unwrap_or(0)),
        (Err(_), None) => (0, 0),
    };
    let alive: Vec<u32> = [holder, child]
        .into_iter()
        .filter(|&p| p != 0 && process_alive(p))
        .collect();
    // Never leave a leaked holder behind, even when the assertion fails.
    for p in &alive {
        let _ = Command::new("taskkill")
            .args(["/F", "/T", "/PID", &p.to_string()])
            .output();
    }
    assert!(
        leaked.is_none() && alive.is_empty(),
        "stop --kill left the starting terminal alive (run: {run:?}, record: {leaked:?}, alive: {alive:?})"
    );
    if let Err(e) = &run {
        assert!(e.is_stopping(), "a run cut by the stop says so: {e}");
    }
}

#[test]
fn an_auto_starting_client_resends_to_the_next_daemon() {
    let _home = sandbox();
    // The daemon the client starts uses the default state dir of the pipe
    // (in the sandbox); share it so the next daemon finds the terminal.
    let pipe = format!(r"\\.\pipe\{}", unique("stop-resend"));
    let daemon = Daemon::start_on(pipe.clone(), podd::endpoint::default_state_dir(&pipe));
    let mut c = daemon.client();
    powershell(&mut c, "keep");
    let old_pid = daemon.child.id();

    // Like the CLI and the app: auto-start, connected before the stop.
    let mut auto = Client::connect_or_start(&pipe, Some(Path::new(env!("CARGO_BIN_EXE_podd"))))
        .expect("connect");
    let waiter = wait_forever(&pipe, "keep");
    std::thread::sleep(Duration::from_millis(300));
    c.call::<ServerStop>(&StopParams { kill: false }).unwrap();
    drop(c);

    // Refused by the stopping daemon, then resent to a fresh one.
    let st = auto
        .call::<ServerStatus>(&Empty {})
        .expect("request resent to the next daemon");
    assert_ne!(st.pid, old_pid, "answered by the stopping daemon");
    let _ = waiter.join();
    // The new daemon (default state dir in the sandbox) found the terminal.
    let list = auto.call::<TerminalList>(&Empty {}).unwrap().terminals;
    assert!(
        list.iter()
            .any(|t| t.name.as_deref() == Some("keep") && t.state == TerminalState::Running),
        "{list:?}"
    );
    let new = ProcessWatch::open(st.pid);
    auto.call::<ServerStop>(&StopParams { kill: true }).unwrap();
    drop(auto);
    if let Some(w) = new {
        assert!(w.wait(Duration::from_secs(20)), "new daemon did not exit");
    }
    // `daemon` is dropped here: its process is gone already.
    drop(daemon);
}

// ---------------------------------------------------------------------------
// `pod server stop` (CLI)
// ---------------------------------------------------------------------------

fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

/// `pod.exe`, built with the same cargo profile (see tests/detach.rs).
fn pod_exe() -> PathBuf {
    let dir = profile_dir();
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.args(["build", "-p", "pod-cli", "--bin", "pod"]);
    if dir.file_name().and_then(|n| n.to_str()) == Some("release") {
        cmd.arg("--release");
    }
    assert!(cmd.status().expect("cargo build").success());
    let p = dir.join("pod.exe");
    assert!(p.exists(), "{} missing", p.display());
    p
}

fn pod(exe: &Path, pipe: &str, args: &[&str]) -> Output {
    Command::new(exe)
        .arg("--pipe")
        .arg(pipe)
        .args(args)
        .env("POD_DAEMON", env!("CARGO_BIN_EXE_podd"))
        .stdin(Stdio::null())
        .output()
        .expect("run pod")
}

fn json_of(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "bad json ({e}): {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

#[test]
fn pod_server_stop_returns_after_exit_and_the_next_command_auto_starts() {
    let _home = sandbox();
    let exe = pod_exe();
    let pipe = format!(r"\\.\pipe\{}", unique("stop-cli"));

    for round in 0..3 {
        // Auto-start (round 0) or re-start right after the previous stop.
        let ls = pod(&exe, &pipe, &["ls"]);
        assert!(ls.status.success(), "round {round}: pod ls: {ls:?}");
        let st = json_of(&pod(&exe, &pipe, &["server", "status", "--json"]));
        let pid = st["pid"].as_u64().unwrap() as u32;
        let watch = ProcessWatch::open(pid);

        let stop = pod(&exe, &pipe, &["server", "stop", "--json"]);
        assert!(stop.status.success(), "round {round}: {stop:?}");
        let v = json_of(&stop);
        assert_eq!(v["stopped"], true, "{v}");
        assert_eq!(v["pid"], pid, "{v}");
        // Returned only after the process (pipe, state files) was gone.
        if let Some(w) = watch {
            assert!(
                w.wait(Duration::ZERO),
                "round {round}: podd still running after stop returned"
            );
        }
        let after = pod(&exe, &pipe, &["server", "status"]);
        assert_eq!(after.status.code(), Some(3), "round {round}: {after:?}");
    }

    // Human output says "stopped".
    assert!(pod(&exe, &pipe, &["ls"]).status.success());
    let stop = pod(&exe, &pipe, &["server", "stop", "--kill"]);
    let text = String::from_utf8_lossy(&stop.stdout);
    assert!(
        stop.status.success() && text.contains("podd stopped"),
        "{stop:?}"
    );
}
