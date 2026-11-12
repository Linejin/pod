//! docs/17 R4 at the daemon (protocol v20): remote framing for a connection
//! whose `connection.hello` chose 20 (chunked snapshot, output pieces of at
//! most 32 KiB, 2 MiB attach queue), the unchanged v19 shapes for a client
//! that chose 19 or sent no hello, and `terminal.input_once` (written at
//! most once, `check` fences). Input effects are counted by what the shell
//! actually ran (lines appended to a file), not by accepted calls.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::*;
use podd::client::Client;
use podd::protocol::*;
use podd::transport::{LineReader, LineWriter};
use serde_json::{Value, json};

fn hello(c: &mut Client, max: u32) -> HelloResult {
    c.call::<ConnectionHello>(&HelloParams {
        min_protocol: HELLO_PROTOCOL,
        max_protocol: max,
        require: vec![],
        client: Some("remote_attach test".into()),
        ..Default::default()
    })
    .expect("hello")
}

/// `cmd` with a `>` prompt.
fn shell(c: &mut Client, name: &str) -> TerminalInfo {
    c.call::<TerminalRun>(&RunParams {
        argv: vec![
            "cmd".into(),
            "/d".into(),
            "/q".into(),
            "/k".into(),
            "prompt $G".into(),
        ],
        cwd: cwd(),
        name: Some(name.into()),
        cols: Some(120),
        rows: Some(30),
        ..Default::default()
    })
    .expect("run cmd")
}

/// Print `n` lines of about 100 bytes and wait for the last one.
fn print_lines(c: &mut Client, t: &str, n: u32, tag: &str) {
    send(
        c,
        t,
        &format!(
            "for /L %i in (1,1,{n}) do @echo {tag}-%i-{}",
            "x".repeat(80)
        ),
    );
    wait_output(c, t, &format!("{tag}-{n}-"));
}

/// A raw connection: (reader, writer) after an optional hello.
fn raw(d: &Daemon, hello_max: Option<u32>) -> (LineReader, LineWriter) {
    let mut c = d.client();
    if let Some(m) = hello_max {
        hello(&mut c, m);
    }
    c.into_transport().into_parts()
}

fn send_raw(w: &mut LineWriter, id: u64, method: &str, params: Value) {
    let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    w.write_line(&req.to_string()).unwrap();
}

fn next(r: &mut LineReader) -> Value {
    serde_json::from_str(&r.read_line().unwrap().expect("a line")).unwrap()
}

fn data_of(n: &Value) -> Vec<u8> {
    b64_decode(n["params"]["data"].as_str().unwrap()).unwrap()
}

/// v20: the hello result says it, the snapshot comes in ordered chunks of at
/// most 32 KiB right after the reply, and output pieces stay within 32 KiB
/// with contiguous offsets.
#[test]
fn v20_hello_gets_chunked_snapshots_and_small_output_pieces() {
    let d = Daemon::start("r4-v20");
    let mut c = d.client();
    let t = shell(&mut c, "big");
    print_lines(&mut c, &t.id, 1500, "pre");

    let mut h = d.client();
    let hr = hello(&mut h, CHUNKED_ATTACH_PROTOCOL);
    assert_eq!(hr.protocol, CHUNKED_ATTACH_PROTOCOL);
    assert_eq!(hr.capabilities, [capability::ATTACH_CHUNKS]);
    assert_eq!(hr.limits.max_output_chunk_bytes, REMOTE_OUTPUT_CHUNK as u64);
    assert_eq!(hr.limits.attach_queue_bytes, REMOTE_ATTACH_QUEUE as u64);

    let (mut r, mut w) = h.into_transport().into_parts();
    send_raw(&mut w, 7, "terminal.attach", json!({"id": t.id}));
    let reply = next(&mut r);
    let res = &reply["result"];
    assert_eq!(res["snapshot"], "", "{reply}");
    let total = res["snapshot_chunks"].as_u64().unwrap();
    let bytes = res["snapshot_bytes"].as_u64().unwrap();
    assert!(total >= 2, "1000 scrollback lines of ~100 bytes: {total}");
    assert_eq!(total, bytes.div_ceil(REMOTE_OUTPUT_CHUNK as u64));
    assert!(res.get("scrollback_omitted").is_none(), "{reply}");
    let seq = res["seq"].as_u64().unwrap();
    let mut snapshot = Vec::new();
    for i in 0..total {
        let n = next(&mut r);
        assert_eq!(n["method"], NOTIFY_SNAPSHOT, "chunk {i}: {n}");
        assert_eq!(n["params"]["index"], i);
        assert_eq!(n["params"]["total"], total);
        assert_eq!(n["params"]["seq"], seq);
        let piece = data_of(&n);
        assert!(piece.len() <= REMOTE_OUTPUT_CHUNK);
        snapshot.extend(piece);
    }
    assert_eq!(snapshot.len() as u64, bytes);
    let text = String::from_utf8_lossy(&snapshot);
    assert!(text.contains("pre-1500-"), "the snapshot has the last line");

    // Live output after the snapshot: contiguous, at most 32 KiB per piece.
    print_lines(&mut c, &t.id, 3000, "live");
    let mut at = seq;
    let mut live = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    while !String::from_utf8_lossy(&live).contains("live-3000-") {
        assert!(Instant::now() < deadline, "live output did not arrive");
        let n = next(&mut r);
        if n["method"] != NOTIFY_OUTPUT {
            continue;
        }
        assert_eq!(n["params"]["seq"], at, "contiguous offsets");
        let piece = data_of(&n);
        assert!(piece.len() <= REMOTE_OUTPUT_CHUNK, "{}", piece.len());
        at += piece.len() as u64;
        live.extend(piece);
    }

    // The typed client joins the chunks the same way.
    let mut h2 = d.client();
    hello(&mut h2, PROTOCOL_VERSION);
    let (res, snap, _stream) = h2
        .attach(&AttachParams {
            target: TerminalRef::id(&t.id),
            ..Default::default()
        })
        .unwrap();
    assert!(res.snapshot_chunks.unwrap() >= 2);
    assert!(String::from_utf8_lossy(&snap).contains("live-3000-"));
}

/// Older clients keep the v19 shapes: a hello that chose 19 and a local
/// connection without hello get the inline snapshot with the three v19
/// fields only, no `terminal.snapshot`, the v19 hello values, and no
/// `attach.chunks`.
#[test]
fn a_v19_hello_and_local_clients_keep_the_v19_shapes() {
    let d = Daemon::start("r4-v19");
    let mut c = d.client();
    let t = shell(&mut c, "old");
    print_lines(&mut c, &t.id, 1500, "pre");

    let mut h = d.client();
    let hr = hello(&mut h, 19);
    assert_eq!(hr.protocol, 19);
    assert!(hr.capabilities.is_empty(), "{:?}", hr.capabilities);
    assert_eq!(hr.limits.max_output_chunk_bytes, 64 * 1024);
    assert_eq!(hr.limits.attach_queue_bytes, 16 * 1024 * 1024);
    drop(h);

    for (label, max) in [("hello 19", Some(19)), ("no hello", None)] {
        let (mut r, mut w) = raw(&d, max);
        send_raw(&mut w, 1, "terminal.attach", json!({"id": t.id}));
        let reply = next(&mut r);
        let res = reply["result"].as_object().unwrap();
        let mut keys: Vec<&str> = res.keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(keys, ["seq", "snapshot", "terminal"], "{label}");
        let snap = b64_decode(res["snapshot"].as_str().unwrap()).unwrap();
        assert!(
            snap.len() > REMOTE_OUTPUT_CHUNK,
            "{label}: inline, not chunked"
        );
        send(&mut c, &t.id, &format!("echo {}", label.replace(' ', "-")));
        let n = next(&mut r);
        assert_eq!(n["method"], NOTIFY_OUTPUT, "{label}: {n}");
    }
}

/// docs/22 TM-14: four remote views that stop reading while their
/// terminals flood are each cut off at their queue limit (`terminal.lagged`
/// with `limit_bytes` = the limit and at most that queued) and closed; the
/// daemon buffers no more for them. The daemon limit is small here so the
/// fake agents' floods pass it quickly; a remote view gets
/// `min(daemon limit, 2 MiB)` (`attach_queue_for`, unit-tested in
/// `connection.rs`; the hello reports it).
#[test]
fn four_flooded_remote_attaches_stay_within_their_queue() {
    const LIMIT: usize = 32 * 1024;
    let fake = fixture_bin("examples/fake-agent.exe");
    let name = unique("r4-flood");
    let d = Daemon::start_with(
        format!(r"\\.\pipe\{name}"),
        std::env::temp_dir().join(&name),
        &["--attach-queue-limit", &LIMIT.to_string()],
    );
    let mut c = d.client();
    let go = std::env::temp_dir().join(unique("r4-flood-go"));
    let mut views = Vec::new();
    for i in 0..4 {
        let t = c
            .call::<TerminalRun>(&RunParams {
                argv: vec![
                    fake.display().to_string(),
                    "print:agent-ready".into(),
                    format!("wait-file:{}", go.display()),
                    "flood:6000".into(),
                    "print:flood-done".into(),
                    "hold".into(),
                ],
                cwd: cwd(),
                name: Some(format!("flood{i}")),
                ..Default::default()
            })
            .unwrap();
        wait_output(&mut c, &t.id, "agent-ready");
        let mut h = d.client();
        let hr = hello(&mut h, CHUNKED_ATTACH_PROTOCOL);
        assert_eq!(hr.limits.attach_queue_bytes, LIMIT as u64);
        let (mut r, mut w) = h.into_transport().into_parts();
        send_raw(&mut w, 1, "terminal.attach", json!({"id": t.id}));
        let reply = next(&mut r);
        for _ in 0..reply["result"]["snapshot_chunks"].as_u64().unwrap() {
            next(&mut r);
        }
        views.push((t.id, r, w));
    }
    // All four flood at once; nobody reads the views meanwhile.
    std::fs::write(&go, "go").unwrap();
    for (id, _, _) in &views {
        wait_output(&mut c, id, "flood-done");
    }
    let _ = std::fs::remove_file(&go);
    for (id, mut r, _w) in views {
        let mut lagged = None;
        while let Ok(Some(line)) = r.read_line() {
            let n: Value = serde_json::from_str(&line).unwrap();
            if n["method"] == NOTIFY_OUTPUT {
                assert!(data_of(&n).len() <= REMOTE_OUTPUT_CHUNK);
            }
            if n["method"] == NOTIFY_LAGGED {
                lagged = Some(n["params"].clone());
                break;
            }
        }
        let l = lagged.unwrap_or_else(|| panic!("{id}: no terminal.lagged"));
        assert_eq!(l["limit_bytes"], LIMIT as u64, "{id}");
        assert!(l["queued_bytes"].as_u64().unwrap() <= LIMIT as u64, "{l}");
        // The daemon closed the view after saying so.
        assert!(matches!(r.read_line(), Ok(None) | Err(_)), "{id}");
    }
    // None of them counts as attached any more.
    let list = c.call::<TerminalList>(&Empty {}).unwrap();
    assert!(list.terminals.iter().all(|t| t.attached == 0), "{list:?}");
}

fn once(stream: &str, seq: u64, text: &str) -> InputOnceParams {
    InputOnceParams {
        target: TerminalRef::id("t1"),
        data: b64_encode(text.as_bytes()),
        stream: stream.into(),
        seq,
        check: false,
    }
}

fn check(stream: &str, seq: u64) -> InputOnceParams {
    InputOnceParams {
        check: true,
        data: String::new(),
        ..once(stream, seq, "")
    }
}

/// Lines the shell appended to `file`, after it settled.
fn ran(file: &PathBuf) -> Vec<String> {
    std::thread::sleep(Duration::from_millis(1500));
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// `terminal.input_once`: a second delivery of the same `(stream, seq)` is
/// not written (also when two race on two connections); a check of an
/// applied seq says so, a check of a new seq fences it, so its late
/// delivery is never written. Counted by what the shell ran.
#[test]
fn input_once_runs_once_and_a_check_fences() {
    let d = Daemon::start("r4-once");
    let mut c = d.client();
    let t = shell(&mut c, "once");
    assert_eq!(t.id, "t1");
    let file = d.state_dir.join("ran.txt");
    let cmd = |tag: &str| format!("echo {tag}>>\"{}\"\r", file.display());

    let a = c
        .call::<TerminalInputOnce>(&once("s1", 1, &cmd("A")))
        .unwrap();
    assert_eq!(a.outcome, InputOnceOutcome::Written);
    assert!(a.bytes > 0);
    let again = c
        .call::<TerminalInputOnce>(&once("s1", 1, &cmd("A")))
        .unwrap();
    assert_eq!(again.outcome, InputOnceOutcome::Already);
    assert_eq!(again.bytes, a.bytes, "the first delivery's count");
    let asked = c.call::<TerminalInputOnce>(&check("s1", 1)).unwrap();
    assert_eq!(
        (asked.outcome, asked.bytes),
        (InputOnceOutcome::Already, a.bytes)
    );

    // A lost input that never arrived: the check fences it.
    let fenced = c.call::<TerminalInputOnce>(&check("s1", 2)).unwrap();
    assert_eq!(
        (fenced.outcome, fenced.bytes),
        (InputOnceOutcome::Fenced, 0)
    );
    let late = c
        .call::<TerminalInputOnce>(&once("s1", 2, &cmd("B")))
        .unwrap();
    assert_eq!(late.outcome, InputOnceOutcome::Fenced, "never written");
    // An older seq of the stream is not written either.
    let old = c
        .call::<TerminalInputOnce>(&once("s1", 1, &cmd("C")))
        .unwrap();
    assert_eq!(old.outcome, InputOnceOutcome::Already);

    // Two connections deliver the same new input at once: one writes.
    let pipe = d.pipe.clone();
    let p = once("s2", 1, &cmd("D"));
    let racers: Vec<_> = (0..2)
        .map(|_| {
            let (pipe, p) = (pipe.clone(), p.clone());
            std::thread::spawn(move || {
                Client::connect(&pipe)
                    .unwrap()
                    .call::<TerminalInputOnce>(&p)
                    .unwrap()
                    .outcome
            })
        })
        .collect();
    let mut outcomes: Vec<InputOnceOutcome> =
        racers.into_iter().map(|r| r.join().unwrap()).collect();
    outcomes.sort_by_key(|o| *o as u8);
    assert_eq!(
        outcomes,
        [InputOnceOutcome::Written, InputOnceOutcome::Already]
    );

    assert_eq!(ran(&file), ["A", "D"], "each input ran exactly once");

    // Bad stream names are refused before anything is written.
    for bad in ["", "a b", &"x".repeat(65)] {
        let e = c.call::<TerminalInputOnce>(&once(bad, 9, &cmd("E")));
        assert_eq!(rpc_code(e), codes::INVALID_PARAMS, "{bad:?}");
    }
    assert_eq!(ran(&file), ["A", "D"]);
    // An unknown terminal: the usual error.
    let mut p = once("s3", 1, "x");
    p.target = TerminalRef::id("t99");
    assert_eq!(
        rpc_code(c.call::<TerminalInputOnce>(&p)),
        codes::TERMINAL_NOT_FOUND
    );
}
