//! Core-daemon gaps (docs/03 §7.2): terminal parents for plain `pod run`,
//! holder protocol negotiation (incompatible and v1 holders), the bounded
//! attach queue (`terminal.lagged` + re-attach) and de-duplication of query
//! replies from attach clients. Real `podd`/`pod-holder`/`pod`; fake holders
//! are pipes served by this test.

mod common;

use std::io::Write as _;

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use common::*;
use pod_test_support::tools;
use podd::client::{AttachEvent, Client};
use podd::holder_proto::{ExitInfo, Frame, HolderRecord, read_frame};
use podd::protocol::*;

// ───────────────────────────── helpers ─────────────────────────────

/// [`common::until`] at this file's 50 ms poll interval.
fn until<T>(what: &str, secs: u64, f: impl FnMut() -> Option<T>) -> T {
    until_every(what, secs, Duration::from_millis(50), f)
}

fn list(c: &mut Client) -> Vec<TerminalInfo> {
    c.call::<TerminalList>(&Empty {}).expect("list").terminals
}

fn info(c: &mut Client, id: &str) -> Option<TerminalInfo> {
    list(c).into_iter().find(|t| t.id == id)
}

fn run(c: &mut Client, argv: &[&str], parent: Option<&str>) -> TerminalInfo {
    c.call::<TerminalRun>(&RunParams {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        cwd: cwd(),
        parent_terminal: parent.map(Into::into),
        ..Default::default()
    })
    .expect("run")
}

// ───────────────────────────── 1. parents ─────────────────────────────

/// `pod run` inside a pod terminal records that terminal as the parent; it
/// is kept by the holder, so it survives a daemon restart. Unknown parents
/// are ignored. The real holder (protocol 2) gets the daemon's version.
#[test]
fn pod_run_inside_a_terminal_records_its_parent() {
    let pod = fixture_bin("pod.exe");
    let d = Daemon::start("parent");
    let mut c = d.client();
    let sh = powershell(&mut c, "sh");
    assert_eq!(sh.parent_terminal, None);
    send(
        &mut c,
        "sh",
        &format!(
            "& '{}' run --name kid -- cmd /c \"echo kid-out\"; 'ran=' + $LASTEXITCODE",
            pod.display()
        ),
    );
    wait_output(&mut c, "sh", "ran=0");
    let kid = until("kid listed", 10, || {
        list(&mut c)
            .into_iter()
            .find(|t| t.name.as_deref() == Some("kid"))
    });
    assert_eq!(kid.parent_terminal.as_deref(), Some(sh.id.as_str()));
    assert_eq!(kid.parent_task, None);

    // Through the API: a known parent is kept, an unknown one is dropped.
    let a = run(&mut c, &["cmd", "/c", "echo a"], Some(&sh.id));
    assert_eq!(a.parent_terminal.as_deref(), Some(sh.id.as_str()));
    let b = run(&mut c, &["cmd", "/c", "echo b"], Some("t999"));
    assert_eq!(b.parent_terminal, None);

    // The version exchange reached the holder (its log).
    let holder_log = d.state_dir.join(format!("{}.log", sh.id));
    until("holder logs the daemon's protocol", 10, || {
        std::fs::read_to_string(&holder_log).ok().filter(|l| {
            l.contains(&format!(
                "daemon speaks holder protocol {}",
                podd::holder_proto::HOLDER_PROTOCOL
            ))
        })
    });

    // Restart: the parent comes back from the holder's hello.
    let (pipe, dir) = d.stop(false);
    let d = Daemon::start_on(pipe, dir);
    let mut c = d.client();
    let kid2 = info(&mut c, &kid.id).expect("kid after restart");
    assert_eq!(kid2.parent_terminal.as_deref(), Some(sh.id.as_str()));
    let json = serde_json::to_value(&kid2).unwrap();
    assert_eq!(json["parent_terminal"], sh.id.as_str());
    assert!(
        json.get("parent_task").is_none(),
        "absent optional fields are omitted"
    );
}

// ───────────────────── 2. holder protocol negotiation ─────────────────────

/// A pipe served by the test that speaks the holder protocol: sends `hello`
/// (raw JSON, so any version or shape) and optionally a snapshot, then
/// records every frame the daemon sends. Answers `Kill` with an exit.
fn fake_holder(
    pipe: String,
    hello: serde_json::Value,
    snapshot: Option<Vec<u8>>,
) -> Arc<Mutex<Vec<Frame>>> {
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ServerOptions;
    let got = Arc::new(Mutex::new(Vec::new()));
    let frames = got.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let mut server = ServerOptions::new()
                .first_pipe_instance(true)
                .create(&pipe)
                .expect("fake holder pipe");
            ready_tx.send(()).unwrap();
            loop {
                if server.connect().await.is_err() {
                    return;
                }
                let next = ServerOptions::new().create(&pipe).expect("next instance");
                let conn = std::mem::replace(&mut server, next);
                let (mut rd, mut wr) = tokio::io::split(conn);
                let _ = wr
                    .write_all(&Frame::ForeignHello(hello.clone()).encode())
                    .await;
                if let Some(s) = &snapshot {
                    let f = Frame::Snapshot {
                        seq: s.len() as u64,
                        data: s.clone(),
                    };
                    let _ = wr.write_all(&f.encode()).await;
                }
                while let Ok(Some(f)) = read_frame(&mut rd).await {
                    if f == Frame::Kill {
                        let seq = snapshot.as_ref().map_or(0, |s| s.len() as u64);
                        let e = Frame::Exited(ExitInfo {
                            seq,
                            exit_code: Some(1),
                            killed: true,
                            exited_ms: 1,
                        });
                        let _ = wr.write_all(&e.encode()).await;
                    }
                    frames.lock().unwrap().push(f);
                }
            }
        });
    });
    ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    got
}

fn write_record(dir: &std::path::Path, id: &str, pipe: &str, holder_pid: u32) {
    let rec = HolderRecord {
        id: id.into(),
        name: None,
        pipe: pipe.into(),
        holder_pid,
        child_pid: 0,
        created_ms: 1,
    };
    std::fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec(&rec).unwrap(),
    )
    .unwrap();
}

/// A holder with an unsupported protocol is listed as `incompatible holder`
/// (exited, not driven, explained on screen) instead of breaking the daemon;
/// `rm --force` terminates it. A protocol-1 holder still works and is not
/// sent the v2 `DaemonHello` frame it would reject.
#[test]
fn holder_versions_are_negotiated() {
    let name = unique("holders");
    let dir = std::env::temp_dir().join(&name);
    std::fs::create_dir_all(&dir).unwrap();

    // Stand-in "holder process" for the incompatible one: rm --force must end it.
    let mut stand_in = Command::new("ping")
        .args(["-n", "120", "127.0.0.1"])
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let new_pipe = format!(r"\\.\pipe\{name}-t7");
    fake_holder(
        new_pipe.clone(),
        serde_json::json!({"version": 99, "id": "t7", "name": "future", "argv": ["claude"],
            "cwd": "C:\\", "holder_pid": stand_in.id(), "cols": 80, "rows": 24, "frames": "v99"}),
        None,
    );
    write_record(&dir, "t7", &new_pipe, stand_in.id());

    let old_pipe = format!(r"\\.\pipe\{name}-t8");
    let old_frames = fake_holder(
        old_pipe.clone(),
        serde_json::json!({"version": 1, "id": "t8", "name": "old", "argv": ["cmd"],
            "cwd": "C:\\", "created_ms": 1, "holder_pid": 1, "child_pid": 2, "cols": 80, "rows": 24}),
        Some(b"old v1 screen".to_vec()),
    );
    write_record(&dir, "t8", &old_pipe, 1);

    let d = Daemon::start_on(format!(r"\\.\pipe\{name}"), dir.clone());
    let mut c = d.client();

    let t7 = info(&mut c, "t7").expect("incompatible terminal listed");
    assert_eq!(t7.state, TerminalState::Exited);
    let why = t7.incompatible_holder.clone().expect("incompatible_holder");
    assert!(why.contains("holder protocol 99"), "{why}");
    assert_eq!(
        (t7.name.as_deref(), t7.argv.as_slice()),
        (Some("future"), &["claude".to_string()][..])
    );
    // Never swept by the exited-terminal retention (its process still runs).
    let all = c.call::<TerminalList>(&Empty {}).unwrap();
    assert!(all.retention_ms.is_some());
    assert!(
        all.retention.iter().all(|r| r.id != "t7"),
        "{:?}",
        all.retention
    );
    let screen = read(&mut c, "t7", ReadMode::Screen).join("\n");
    assert!(screen.contains("[pod: incompatible holder"), "{screen}");
    // Not driven: input is refused like for any exited terminal.
    let input = c.call::<TerminalInput>(&InputParams {
        target: TerminalRef::id("t7"),
        data: b64_encode(b"x"),
    });
    assert_eq!(rpc_code(input), codes::TERMINAL_EXITED);

    // The v1 holder is a normal running terminal.
    let t8 = info(&mut c, "t8").expect("v1 terminal listed");
    assert_eq!(
        (t8.state, t8.incompatible_holder.as_deref()),
        (TerminalState::Running, None)
    );
    assert!(
        read(&mut c, "t8", ReadMode::Screen)
            .join("\n")
            .contains("old v1 screen")
    );
    c.call::<TerminalInput>(&InputParams {
        target: TerminalRef::id("t8"),
        data: b64_encode(b"hi"),
    })
    .unwrap();
    let frames = until("input reaches the v1 holder", 10, || {
        let f = old_frames.lock().unwrap().clone();
        f.contains(&Frame::Input(b"hi".to_vec())).then_some(f)
    });
    assert!(
        !frames.iter().any(|f| matches!(f, Frame::DaemonHello(_))),
        "a v1 holder must not get DaemonHello: {frames:?}"
    );

    // New ids continue after the rediscovered ones.
    let next = run(&mut c, &["cmd", "/c", "echo n"], None);
    assert_eq!(next.id, "t9");

    // rm without force explains; with force the holder process is ended.
    let rm = c.call::<TerminalRm>(&RmParams {
        target: TerminalRef::id("t7"),
        force: false,
    });
    assert_eq!(rpc_code(rm), codes::TERMINAL_RUNNING);
    assert!(stand_in.try_wait().unwrap().is_none());
    c.call::<TerminalRm>(&RmParams {
        target: TerminalRef::id("t7"),
        force: true,
    })
    .expect("rm --force");
    until("the incompatible holder is terminated", 10, || {
        stand_in.try_wait().unwrap()
    });
    assert!(info(&mut c, "t7").is_none());
    assert!(!dir.join("t7.json").exists());

    let _ = stand_in.kill();
}

// ───────────────────────── 3. bounded attach queue ─────────────────────────

/// A client that stops reading is dropped once its queue passes the limit:
/// it gets `terminal.lagged` (limit, offset) and the stream ends, the daemon
/// stops counting it as attached, and a new attach starts from a snapshot
/// with everything. Memory per client stays bounded.
///
/// Kept fast and load-proof: a small limit, the flood written by the fake
/// agent in one go (PowerShell's 20 000-iteration loop took ~25 s), and the
/// stream read on a thread with one overall deadline instead of a blocking
/// read inside a wall-clock loop.
#[test]
fn slow_attach_client_is_dropped_and_can_reattach() {
    const LIMIT: usize = 16 * 1024;
    const LINES: usize = 6_000; // ~300 KB, far above limit + pipe buffers
    let fake = fixture_bin("examples/fake-agent.exe");
    let name = unique("lag");
    let d = Daemon::start_with(
        format!(r"\\.\pipe\{name}"),
        std::env::temp_dir().join(&name),
        &["--attach-queue-limit", &LIMIT.to_string()],
    );
    let mut c = d.client();
    let go = std::env::temp_dir().join(unique("lag-go"));
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec![
                fake.display().to_string(),
                "print:agent-ready".into(),
                format!("wait-file:{}", go.display()),
                format!("flood:{LINES}"),
                "print:big-done".into(),
                "hold".into(),
            ],
            cwd: cwd(),
            name: Some("big".into()),
            ..Default::default()
        })
        .unwrap();
    wait_output(&mut c, "big", "agent-ready");

    let (res, _snap, mut stream) = Client::connect(&d.pipe)
        .unwrap()
        .attach(&AttachParams {
            target: TerminalRef::id(&t.id),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(info(&mut c, &t.id).unwrap().attached, 1);
    // The flood while the client reads nothing.
    std::fs::write(&go, "go").unwrap();
    wait_output(&mut c, "big", "big-done");

    // Now read what the client was sent, on a thread (a read blocks).
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let ev = stream.next_event();
            let end = matches!(ev, Ok(None) | Err(_));
            if tx.send(ev.map_err(|e| e.to_string())).is_err() || end {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    let next = || {
        rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("attach stream event before the deadline")
            .expect("attach stream read")
    };
    let mut expect = res.seq;
    let mut got = 0usize;
    let lag = loop {
        match next() {
            Some(AttachEvent::Output { seq, data }) => {
                assert_eq!(seq, expect, "delivered output stays contiguous");
                expect += data.len() as u64;
                got += data.len();
            }
            Some(AttachEvent::Lagged(l)) => break l,
            other => panic!("unexpected {other:?} after {got} bytes"),
        }
    };
    assert_eq!(lag.limit_bytes, LIMIT as u64);
    assert!(lag.queued_bytes <= LIMIT as u64);
    assert!(
        lag.seq >= expect,
        "lag offset {} before delivered end {expect}",
        lag.seq
    );
    assert!(
        got < LINES * 40,
        "the client did not get the whole flood ({got} bytes)"
    );
    assert!(next().is_none(), "the stream ends after lagged");
    until("detached", 10, || {
        (info(&mut c, &t.id).unwrap().attached == 0).then_some(())
    });

    // Re-attach from a snapshot: the end of the flood is there.
    let (_res, snap, _stream) = Client::connect(&d.pipe)
        .unwrap()
        .attach(&AttachParams {
            target: TerminalRef::id(&t.id),
            ..Default::default()
        })
        .unwrap();
    let text = String::from_utf8_lossy(&snap);
    assert!(text.contains(&format!("line {LINES} ")) && text.contains("big-done"));
    let _ = std::fs::remove_file(&go);
}

// ──────────────────── 4. query replies from attach clients ────────────────────

/// The holder answers DA1; an attached real terminal answers too. Its reply
/// (sent as attach input) is dropped once per attached client; a later
/// identical reply, keys and text pass through.
#[test]
fn attach_query_replies_are_not_duplicated() {
    let fake = fixture_bin("examples/fake-agent.exe");
    // A wide reply window: this test checks de-duplication and stray replies
    // without racing the 500 ms default (the window itself is unit-tested in
    // `query_dedup`).
    let name = unique("dedup");
    let d = Daemon::start_with(
        format!(r"\\.\pipe\{name}"),
        std::env::temp_dir().join(&name),
        &["--reply-window", "10000"],
    );
    let mut c = d.client();
    let root = std::env::temp_dir().join(unique("dedup-files"));
    std::fs::create_dir_all(&root).unwrap();
    let go = root.join("go.flag");
    let log = root.join("input.log");
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec![
                fake.display().to_string(),
                "print:agent-started".into(),
                format!("wait-file:{}", go.display()),
                "print:\x1b[c\x1b]11;?\x07".into(),
                format!("paste-loop-silent:{}", log.display()),
            ],
            cwd: cwd(),
            ..Default::default()
        })
        .unwrap();

    // The inbox ConPTY answers DA1 itself and drops OSC replies on its input
    // pipe, so the query never reaches attach clients and there is nothing to
    // de-duplicate. Only the bundled ConPTY (tools/fetch-conpty.ps1) exercises
    // this path.
    let conpty = until("terminal conpty kind", 10, || {
        info(&mut c, &t.id).and_then(|i| i.conpty)
    });
    if !conpty.starts_with("bundled") {
        eprintln!(
            "skipping attach_query_replies_are_not_duplicated: needs the bundled ConPTY              (terminal runs on {conpty}; run tools/fetch-conpty.ps1)"
        );
        let _ = c.call::<TerminalKill>(&TerminalRef::id(&t.id));
        return;
    }

    // Attach only after the start (ConPTY's own start-up output may contain
    // queries too; a real attached terminal would answer those as well),
    // reading on a thread, then let the program ask DA1 and the background color.
    wait_output(&mut c, &t.id, "agent-started");
    let (_res, _snap, mut stream) = Client::connect(&d.pipe)
        .unwrap()
        .attach(&AttachParams {
            target: TerminalRef::id(&t.id),
            ..Default::default()
        })
        .unwrap();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_event() {
            if let AttachEvent::Output { data, .. } = ev
                && tx.send(data).is_err()
            {
                return;
            }
        }
    });
    std::fs::File::create(&go).unwrap().flush().unwrap();
    let mut seen = Vec::new();
    until("query and prompt on the attach stream", 30, || {
        while let Ok(d) = rx.try_recv() {
            seen.extend(d);
        }
        let s = String::from_utf8_lossy(&seen);
        (s.contains("\x1b[c") && s.contains("ready for prompts")).then_some(())
    });
    let color_query = String::from_utf8_lossy(&seen).contains("\x1b]11;?");

    let input = |c: &mut Client, b: &[u8]| {
        c.call::<TerminalInput>(&InputParams {
            target: TerminalRef::id(&t.id),
            data: b64_encode(b),
        })
        .unwrap()
    };
    // The attached terminal's DA1 reply: the holder already answered (dropped).
    let r = input(&mut c, b"\x1b[?1;2;99c");
    assert_eq!(
        r.bytes,
        0,
        "the duplicate reply is not forwarded; stream: {:?}",
        String::from_utf8_lossy(&seen)
    );
    assert_eq!(
        r.dropped_replies,
        Some(DroppedReplies {
            duplicate: 1,
            late: 0
        })
    );
    // v9: the color reply is not answered by the holder; the first (only)
    // client's reply passes. A second DA1 reply has no outstanding query: a
    // stray reply that would become keystrokes, dropped (v9 late guard).
    let mut typed = b"a".to_vec();
    if color_query {
        let r = input(&mut c, b"\x1b]11;rgb:1111/2222/3333\x1b\\");
        assert_eq!(r.bytes, 25, "first color reply passes");
        assert_eq!(r.dropped_replies, None);
    } else {
        tools::missing("OSC 11 query on the attach stream (bundled ConPTY)");
    }
    let r = input(&mut c, b"a\x1b[?1;2;98cb\x1b]11;rgb:9/9/9\x07");
    assert_eq!(r.bytes, 2, "stray DA1 and color replies are dropped");
    assert_eq!(
        r.dropped_replies,
        Some(DroppedReplies {
            duplicate: 0,
            late: 2
        })
    );
    typed.push(b'b');
    input(&mut c, b"\r");
    let logged = until("submission logged", 20, || {
        std::fs::read_to_string(&log)
            .ok()
            .filter(|l| l.contains("<<<END"))
    });
    assert!(
        logged.contains(std::str::from_utf8(&typed).unwrap()),
        "{logged:?}"
    );
    assert!(
        !logged.contains("99c") && !logged.contains("98c") && !logged.contains("9/9/9"),
        "duplicate or stray reply reached the program: {logged:?}"
    );
    if color_query {
        assert!(logged.contains("rgb:1111/2222/3333"), "{logged:?}");
    }
    let _ = c.call::<TerminalKill>(&TerminalRef::id(&t.id));
    let _ = std::fs::remove_dir_all(&root);
}
