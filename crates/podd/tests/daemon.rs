//! End-to-end: the real `podd` and `pod-holder` binaries on a test-only pipe
//! and state directory, driven through the blocking client.

mod common;

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use podd::client::{AttachEvent, Client, ClientError};
use podd::protocol::*;
use podd::vt::Vt;

#[test]
fn echo_exit_send_wait_kill_rm() {
    let daemon = Daemon::start("e2e");
    let mut c = daemon.client();

    // Short-lived command: run, wait for exit, read the output.
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec!["cmd".into(), "/c".into(), "echo hello".into()],
            cwd: cwd(),
            name: Some("echo".into()),
            ..Default::default()
        })
        .expect("run cmd");
    assert_eq!(t.id, "t1");
    let w = c
        .call::<TerminalWait>(&WaitParams {
            target: TerminalRef::name("echo"),
            until: WaitUntil::Exit,
            timeout_ms: Some(15_000),
        })
        .unwrap();
    assert_eq!(w.outcome, WaitOutcome::Matched, "{w:?}");
    assert_eq!(w.terminal.exit_code, Some(0));
    let lines = read(&mut c, "t1", ReadMode::History);
    println!("cmd output: {lines:?} (conpty {:?})", w.terminal.conpty);
    assert!(
        lines.iter().any(|l| l.trim() == "hello"),
        "no 'hello' line in {lines:?}"
    );

    // Long-lived shell.
    let ps = powershell(&mut c, "ps");
    assert_eq!(ps.state, TerminalState::Running);
    assert_eq!(
        rpc_code(c.call::<TerminalRun>(&RunParams {
            argv: vec!["cmd".into()],
            name: Some("ps".into()),
            cwd: cwd(),
            ..Default::default()
        })),
        codes::NAME_TAKEN
    );

    // Environment: our variables in, launcher markers out.
    send(
        &mut c,
        "ps",
        "echo \"id=$env:POD_TERMINAL_ID sock=$env:POD_SOCKET nc=[$env:NO_COLOR] cc=[$env:CLAUDECODE] ide=[$env:ORCA_TERMINAL_HANDLE] term=$env:TERM\"",
    );
    wait_output(
        &mut c,
        "ps",
        &format!(
            "id={} sock={} nc=[] cc=[] ide=[] term=xterm-256color",
            ps.id, daemon.pipe
        ),
    );

    // The daemon's folder is first on PATH when `pod.exe` is next to it, so
    // agents in Pod terminals reach the matching `pod` (docs/19 §17).
    let bin = std::path::Path::new(env!("CARGO_BIN_EXE_podd"))
        .parent()
        .unwrap();
    send(&mut c, "ps", "echo \"path0=[$($env:PATH.Split(';')[0])]\"");
    let first = if bin.join("pod.exe").is_file() {
        bin.display().to_string()
    } else {
        String::new()
    };
    if first.is_empty() {
        println!("pod.exe not built next to podd: PATH check skipped");
    } else {
        wait_output(&mut c, "ps", &format!("path0=[{first}]"));
    }

    // Echo filter: the wait returns only once the *output* line exists, so a
    // read right after it must already show it.
    send(&mut c, "ps", "echo pod-ok");
    wait_output(&mut c, "ps", "pod-ok");
    let lines = read(&mut c, "ps", ReadMode::Screen);
    println!("powershell screen: {lines:#?}");
    assert!(
        lines.iter().any(|l| l.trim() == "pod-ok"),
        "wait matched the echo, not the output: {lines:?}"
    );

    // Named keys: type, then Ctrl+C cancels the line; Enter on a fresh prompt.
    c.call::<TerminalKeys>(&KeysParams {
        target: TerminalRef::name("ps"),
        keys: vec!["x".into(), "ctrl+c".into()],
    })
    .expect("keys");
    assert_eq!(
        rpc_code(c.call::<TerminalKeys>(&KeysParams {
            target: TerminalRef::name("ps"),
            keys: vec!["hyper+z".into()]
        })),
        codes::INVALID_PARAMS
    );

    // Timeouts are an outcome, not an error.
    let w = c
        .call::<TerminalWait>(&WaitParams {
            target: TerminalRef::name("ps"),
            until: WaitUntil::OutputContains("never-printed-xyz".into()),
            timeout_ms: Some(300),
        })
        .unwrap();
    assert_eq!(w.outcome, WaitOutcome::TimedOut);

    // rm refuses running terminals; kill then rm ends the holder.
    assert_eq!(
        rpc_code(c.call::<TerminalRm>(&RmParams {
            target: TerminalRef::name("ps"),
            force: false
        })),
        codes::TERMINAL_RUNNING
    );
    let k = c
        .call::<TerminalKill>(&TerminalRef::name("ps"))
        .expect("kill");
    assert_eq!(k.state, TerminalState::Exited);
    assert!(k.killed);
    assert_eq!(
        rpc_code(c.call::<TerminalSend>(&SendParams {
            target: TerminalRef::id(&ps.id),
            text: "x".into(),
            enter: false,
            confirm: false,
        })),
        codes::TERMINAL_EXITED
    );
    let holder = k.holder_pid.unwrap();
    c.call::<TerminalRm>(&RmParams {
        target: TerminalRef::id(&ps.id),
        force: false,
    })
    .expect("rm");
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_alive(holder) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        !process_alive(holder),
        "holder {holder} still alive after rm"
    );
    let list = c.call::<TerminalList>(&Empty {}).unwrap();
    assert_eq!(
        list.terminals
            .iter()
            .map(|t| t.id.as_str())
            .collect::<Vec<_>>(),
        ["t1"]
    );
    assert_eq!(
        rpc_code(c.call::<TerminalRead>(&ReadParams {
            target: TerminalRef::id("t99"),
            mode: ReadMode::Screen,
            lines: None,
            raw: false,
        })),
        codes::TERMINAL_NOT_FOUND
    );
}

#[test]
fn terminal_survives_daemon_restart() {
    let daemon = Daemon::start("restart");
    let mut c = daemon.client();
    let ps = powershell(&mut c, "keep");
    send(&mut c, "keep", "echo before-restart");
    wait_output(&mut c, "keep", "before-restart");
    drop(c);

    // Stop the daemon without killing terminals; the holder keeps the shell.
    let old_pid = daemon.child.id();
    let (pipe, state_dir) = daemon.stop(false);
    assert!(Client::connect(&pipe).is_err(), "daemon still listening");
    assert!(
        process_alive(ps.holder_pid.unwrap()),
        "holder died with the daemon"
    );
    assert!(process_alive(ps.pid.unwrap()), "shell died with the daemon");

    let daemon = Daemon::start_on(pipe, state_dir);
    assert_ne!(daemon.child.id(), old_pid);
    let mut c = daemon.client();
    let list = c.call::<TerminalList>(&Empty {}).unwrap().terminals;
    assert_eq!(list.len(), 1, "{list:?}");
    let t = &list[0];
    assert_eq!(
        (t.id.as_str(), t.name.as_deref(), t.state),
        ("t1", Some("keep"), TerminalState::Running)
    );
    assert_eq!((t.pid, t.holder_pid), (ps.pid, ps.holder_pid));

    let lines = read(&mut c, "keep", ReadMode::History);
    println!("after restart: {lines:#?}");
    assert!(
        lines.iter().any(|l| l.trim() == "before-restart"),
        "{lines:?}"
    );

    // Still interactive, and new ids continue after the re-discovered ones.
    send(&mut c, "keep", "echo after-restart");
    wait_output(&mut c, "keep", "after-restart");
    let t2 = c
        .call::<TerminalRun>(&RunParams {
            argv: vec!["cmd".into(), "/c".into(), "exit 3".into()],
            cwd: cwd(),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(t2.id, "t2");
    let w = c
        .call::<TerminalWait>(&WaitParams {
            target: TerminalRef::id("t2"),
            until: WaitUntil::Exit,
            timeout_ms: Some(15_000),
        })
        .unwrap();
    assert_eq!(w.terminal.exit_code, Some(3));
}

#[test]
fn attach_snapshot_then_live_bytes_without_gaps_and_resize() {
    let daemon = Daemon::start("attach");
    let mut c = daemon.client();
    // The default prompt contains the fixture path. At exactly the resized
    // width, PowerShell/PSReadLine can redraw part of that prompt on the
    // preceding output row. Keep this transport test's prompt independent of
    // the temporary path; the boundary's raw writes are replayed below.
    c.call::<TerminalRun>(&RunParams {
        argv: vec![
            "powershell".into(),
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NoExit".into(),
            "-Command".into(),
            "function prompt { 'PS> ' }".into(),
        ],
        cwd: cwd(),
        name: Some("att".into()),
        ..Default::default()
    })
    .expect("run powershell with a fixed prompt");
    send(&mut c, "att", "1..40 | % { \"pre $_\" }");
    wait_output(&mut c, "att", "pre 40");

    let (res, snapshot, mut stream) = Client::connect(&daemon.pipe)
        .unwrap()
        .attach(&AttachParams {
            target: TerminalRef::name("att"),
            scrollback: Some(10_000),
            ..Default::default()
        })
        .expect("attach");
    assert_eq!(res.terminal.attached, 1);
    let mut local = Vt::from_snapshot(res.terminal.cols, res.terminal.rows, res.seq, &snapshot);
    assert!(
        local
            .read_lines(ReadMode::History)
            .0
            .iter()
            .any(|l| l == "pre 40"),
        "snapshot lacks earlier output"
    );

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_event() {
            if tx.send(ev).is_err() {
                break;
            }
        }
    });

    send(&mut c, "att", "1..300 | % { \"live $_\" }");
    wait_output(&mut c, "att", "live 300");
    c.call::<TerminalResize>(&ResizeParams {
        target: TerminalRef::name("att"),
        cols: 90,
        rows: 25,
    })
    .expect("resize");
    send(
        &mut c,
        "att",
        "echo \"after=$([Console]::WindowWidth)x$([Console]::WindowHeight)\"",
    );
    wait_output(&mut c, "att", "after=90x25");

    // Apply every event in order; offsets must be contiguous.
    let mut expect = res.seq;
    let mut resized = None;
    let mut chunks = 0;
    let diagnostic_path = std::env::var_os("POD_ATTACH_DIAGNOSTICS");
    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rx.recv_timeout(Duration::from_millis(700)) {
            Ok(AttachEvent::Output { seq, data }) => {
                if diagnostic_path.is_some() {
                    events.push(serde_json::json!({"seq": seq, "data": data}));
                }
                assert_eq!(seq, expect, "gap or duplicate in the attach stream");
                expect += data.len() as u64;
                local.feed(&data);
                chunks += 1;
            }
            Ok(AttachEvent::Resized { seq, cols, rows }) => {
                if diagnostic_path.is_some() {
                    events.push(serde_json::json!({"seq": seq, "cols": cols, "rows": rows,
                        "history_before_resize": local.read_lines(ReadMode::History).0}));
                }
                assert_eq!(seq, expect, "resize not in stream order");
                local.resize(cols, rows);
                resized = Some((cols, rows));
            }
            Ok(AttachEvent::Exited(e)) => panic!("unexpected exit {e:?}"),
            Ok(AttachEvent::Lagged(e)) => panic!("unexpected lag {e:?}"),
            Err(_)
                if local
                    .read_lines(ReadMode::History)
                    .0
                    .iter()
                    .any(|l| l == "after=90x25") =>
            {
                break;
            }
            Err(_) => assert!(Instant::now() < deadline, "attach stream stalled"),
        }
    }
    assert_eq!(resized, Some((90, 25)));
    let history = local.read_lines(ReadMode::History).0;
    if let Some(path) = diagnostic_path {
        let diagnostic = serde_json::json!({
            "initial_cols": res.terminal.cols, "initial_rows": res.terminal.rows,
            "initial_seq": res.seq, "snapshot": snapshot, "events": events,
            "local_history": history, "local_screen": local.read_lines(ReadMode::Screen).0,
            "daemon_history": read(&mut c, "att", ReadMode::History),
            "daemon_screen": read(&mut c, "att", ReadMode::Screen),
        });
        std::fs::write(path, serde_json::to_vec_pretty(&diagnostic).unwrap()).unwrap();
    }
    let live: Vec<&String> = history.iter().filter(|l| l.starts_with("live ")).collect();
    assert_eq!(live.len(), 300, "every live line exactly once");
    for (i, line) in live.iter().enumerate() {
        assert_eq!(**line, format!("live {}", i + 1), "live line at index {i}");
    }
    // The client's own VT ends up with the same screen as the daemon's.
    assert_eq!(
        local.read_lines(ReadMode::Screen).0,
        read(&mut c, "att", ReadMode::Screen)
    );
    println!("attach: {chunks} output chunks, final offset {expect}");

    let info = c.call::<TerminalKill>(&TerminalRef::name("att")).unwrap();
    assert!(info.killed);
    // ConPTY may flush a last chunk (mode resets) before the exit notification.
    loop {
        match rx
            .recv_timeout(Duration::from_secs(5))
            .expect("exit notification")
        {
            AttachEvent::Output { seq, data } => {
                assert_eq!(seq, expect);
                expect += data.len() as u64;
            }
            AttachEvent::Exited(e) => {
                assert!(e.killed, "{e:?}");
                assert_eq!(e.seq, expect, "exit offset matches the bytes seen");
                break;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn kill_ends_the_whole_process_tree() {
    let daemon = Daemon::start("tree");
    let mut c = daemon.client();
    // A grandchild that is NOT attached to the pseudo console (own hidden
    // console), so only the Job Object can reach it.
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec![
                "powershell".into(),
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-Command".into(),
                "$p = Start-Process ping -ArgumentList '-n','600','127.0.0.1' -WindowStyle Hidden -PassThru; \
                 Write-Output ('grandchild-pid=' + $p.Id); Start-Sleep 600"
                    .into(),
            ],
            cwd: cwd(),
            name: Some("tree".into()),
            ..Default::default()
        })
        .unwrap();
    wait_output(&mut c, "tree", "grandchild-pid=");
    let line = read(&mut c, "tree", ReadMode::History)
        .into_iter()
        .find(|l| l.starts_with("grandchild-pid="))
        .unwrap();
    let pid: u32 = line
        .trim_start_matches("grandchild-pid=")
        .trim()
        .parse()
        .unwrap();
    assert!(process_alive(pid), "grandchild {pid} not running");
    assert!(process_alive(t.pid.unwrap()));

    let k = c.call::<TerminalKill>(&TerminalRef::name("tree")).unwrap();
    assert_eq!(k.state, TerminalState::Exited);
    let deadline = Instant::now() + Duration::from_secs(5);
    while (process_alive(pid) || process_alive(t.pid.unwrap())) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!process_alive(t.pid.unwrap()), "root survived kill");
    assert!(
        !process_alive(pid),
        "grandchild {pid} survived kill (Job Object)"
    );
}

#[test]
fn second_daemon_on_same_pipe_is_refused() {
    let daemon = Daemon::start("lock");
    let out = Command::new(env!("CARGO_BIN_EXE_podd"))
        .args(["--pipe", &daemon.pipe])
        .arg("--state-dir")
        .arg(&daemon.state_dir)
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn second podd");
    assert_eq!(
        out.status.code(),
        Some(3),
        "second daemon should fail with exit code 3"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("lock held"));
    let st = daemon.client().call::<ServerStatus>(&Empty {}).unwrap();
    assert_eq!(st.pipe, daemon.pipe);
}

/// Replay the resize boundary that made the default-prompt attach fixture
/// path-dependent. These cursor writes came from PowerShell/PSReadLine, not
/// from missing/reordered attach bytes. Both a default 90-cell path prompt
/// and an explicit 90-cell prompt produced them with bundled ConPTY.
#[test]
fn attach_fixture_long_prompt_redraw_overwrites_previous_output() {
    let replies = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = replies.clone();
    let mut vt = Vt::new(
        120,
        30,
        Some(std::sync::Arc::new(move |bytes| {
            captured.lock().unwrap().extend_from_slice(bytes);
        })),
    );
    let expected: Vec<String> = (1..=300).map(|i| format!("live {i}")).collect();
    for line in &expected {
        vt.feed(format!("{line}\r\n").as_bytes());
    }
    vt.feed(format!("{}> ", "p".repeat(88)).as_bytes());
    assert_eq!(vt.read_lines(ReadMode::Screen).0[28], "live 300");

    vt.resize(90, 25);
    assert_eq!(vt.read_lines(ReadMode::Screen).0[23], "live 300");
    let live = |vt: &Vt| {
        vt.read_lines(ReadMode::History)
            .0
            .into_iter()
            .filter(|line| line.starts_with("live "))
            .collect::<Vec<_>>()
    };
    assert_eq!(live(&vt), expected, "resize alone preserves every line");

    // The resized grid answers DSR at row 25, column 90 (pending wrap),
    // so this capture did not answer the query with the old dimensions.
    vt.feed(b"\x1b[6n");
    assert_eq!(*replies.lock().unwrap(), b"\x1b[25;90R");
    // Exact captured prefix after that query: redraw the first typed letter,
    // then move to row 24, column 89 to redraw the prompt's final two cells.
    vt.feed(
        concat!(
            "\x1b[25;90H\x1b[?25l\x1b[25;1H\x1b[93me\x1b[39;49m\x1b[0m",
            "\x1b[25;2H\x1b[?25h\x1b[?25l\x1b[25;1H\x1b[24;89H\x1b[91m",
        )
        .as_bytes(),
    );
    assert_eq!(
        live(&vt),
        expected,
        "cursor moves have not changed live output"
    );
    vt.feed(b">");
    let overwritten = live(&vt);
    assert_eq!(overwritten.len(), 300);
    assert_eq!(overwritten[..299], expected[..299]);
    assert_eq!(overwritten[299], format!("live 300{}>", " ".repeat(80)));
}

#[test]
fn raw_jsonrpc_errors() {
    let daemon = Daemon::start("rpc");
    let mut c = daemon.client();
    let e = c
        .call_raw("nope.method", serde_json::Value::Null)
        .unwrap_err();
    assert!(matches!(e, ClientError::Rpc(ref e) if e.code == codes::METHOD_NOT_FOUND));
    let e = c
        .call_raw("terminal.run", serde_json::json!({"argv": "not-a-list"}))
        .unwrap_err();
    assert!(matches!(e, ClientError::Rpc(ref e) if e.code == codes::INVALID_PARAMS));
    let e = c
        .call_raw(
            "terminal.run",
            serde_json::json!({"argv": ["cmd"], "name": "t5"}),
        )
        .unwrap_err();
    assert!(matches!(e, ClientError::Rpc(ref e) if e.code == codes::INVALID_PARAMS));
    let e = c
        .call_raw(
            "terminal.run",
            serde_json::json!({"argv": ["no-such-program-xyz"]}),
        )
        .unwrap_err();
    assert!(
        matches!(e, ClientError::Rpc(ref e) if e.code == codes::SPAWN_FAILED),
        "{e:?}"
    );
}
