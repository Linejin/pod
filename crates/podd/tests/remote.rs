//! docs/17 R1 (duplex transport) and R2 (`connection.hello`, v12).
//!
//! The client runs over the real named pipe, over in-memory pipes to a fake
//! server thread and over a child process's stdio. The handshake is checked
//! against a real isolated podd (v12) and fake v11 / future daemons.

mod common;

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use common::*;
use podd::client::{AttachEvent, Client, ClientError};
use podd::protocol::*;
use podd::transport::{LineReader, LineWriter, REMOTE_MAX_LINE, Transport};
use serde_json::{Value, json};

/// A client wired to a fake server thread through two in-memory pipes. The
/// closure gets each request and returns the raw bytes to send back (any
/// framing, possibly several lines or none); returning `None` closes.
fn fake_server(mut reply: impl FnMut(Request) -> Option<Vec<u8>> + Send + 'static) -> Client {
    let (c_read, s_write) = std::io::pipe().unwrap();
    let (s_read, c_write) = std::io::pipe().unwrap();
    std::thread::spawn(move || {
        let mut r = LineReader::new(s_read, REMOTE_MAX_LINE);
        let mut w = s_write;
        while let Ok(Some(line)) = r.read_line() {
            let req: Request = serde_json::from_str(&line).unwrap();
            match reply(req) {
                Some(bytes) => {
                    // Byte by byte with flushes: the client must reassemble.
                    for b in bytes {
                        if w.write_all(&[b]).is_err() {
                            return;
                        }
                        let _ = w.flush();
                    }
                }
                None => return,
            }
        }
    });
    Client::from_transport("fake", Transport::new(c_read, c_write, REMOTE_MAX_LINE))
}

fn response_line(id: &Option<Value>, result: Value) -> Vec<u8> {
    let mut s =
        serde_json::to_string(&json!({"jsonrpc": "2.0", "id": id, "result": result})).unwrap();
    s.push('\n');
    s.into_bytes()
}

fn status_value(protocol: u32) -> Value {
    json!({
        "version": "0.0.0-fake", "protocol": protocol, "pid": 1, "pipe": "fake",
        "started_ms": 0, "uptime_ms": 0, "terminals": 0, "running": 0, "state_dir": "x"
    })
}

// ---------------------------------------------------------------------------
// R1: transport
// ---------------------------------------------------------------------------

#[test]
fn in_memory_transport_reassembles_split_lines_with_utf8() {
    let mut c = fake_server(|req| {
        assert_eq!(req.method, "server.status");
        let mut v = status_value(12);
        v["pipe"] = json!("파이프 é");
        Some(response_line(&req.id, v))
    });
    for _ in 0..3 {
        let s = c.call::<ServerStatus>(&Empty {}).unwrap();
        assert_eq!(s.pipe, "파이프 é");
    }
}

#[test]
fn eof_before_a_response_is_a_disconnect() {
    let mut c = fake_server(|_| None);
    match c.call::<ServerStatus>(&Empty {}) {
        Err(ClientError::Protocol(m)) => assert_eq!(m, "daemon closed the connection"),
        // The request write may already see the closed pipe.
        Err(ClientError::Io(_)) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn half_line_then_eof_is_reported_as_closed() {
    let (c_read, mut s_write) = std::io::pipe().unwrap();
    let (_s_read, c_write) = std::io::pipe().unwrap();
    s_write
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"res")
        .unwrap();
    drop(s_write);
    let mut c = Client::from_transport("fake", Transport::new(c_read, c_write, 1024));
    match c.call::<ServerStatus>(&Empty {}) {
        Err(ClientError::Protocol(m)) => assert_eq!(m, "daemon closed the connection"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn oversized_response_line_fails_without_buffering_it() {
    let (c_read, s_write) = std::io::pipe().unwrap();
    let (_s_read, c_write) = std::io::pipe().unwrap();
    // 1 MiB of a line, limit 64 KiB. The writer blocks once the pipe is full
    // and the client stopped reading, so write from a thread.
    std::thread::spawn(move || {
        let mut w = s_write;
        let _ = w.write_all(&vec![b'x'; 1024 * 1024]);
    });
    let mut c = Client::from_transport("fake", Transport::new(c_read, c_write, 64 * 1024));
    match c.call::<ServerStatus>(&Empty {}) {
        Err(ClientError::Protocol(m)) => assert!(m.contains("65536-byte limit"), "{m}"),
        other => panic!("{other:?}"),
    }
    // The connection stays failed.
    assert!(c.call::<ServerStatus>(&Empty {}).is_err());
}

#[test]
fn non_protocol_output_fails_closed_instead_of_being_skipped() {
    let mut c = fake_server(|req| {
        let mut out = b"Welcome to the server!\r\n".to_vec();
        out.extend(response_line(&req.id, status_value(12)));
        Some(out)
    });
    match c.call::<ServerStatus>(&Empty {}) {
        Err(ClientError::Protocol(m)) => {
            assert!(m.contains("non-protocol output"), "{m}");
            assert!(m.contains("Welcome"), "{m}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn attach_switches_an_in_memory_connection_to_a_stream() {
    let c = fake_server(|req| {
        assert_eq!(req.method, "terminal.attach");
        let info = TerminalInfo {
            id: "t1".into(),
            ..Default::default()
        };
        let mut out = response_line(
            &req.id,
            json!({"terminal": info, "seq": 3, "snapshot": b64_encode(b"abc")}),
        );
        for n in [
            json!({"jsonrpc":"2.0","method":"terminal.output","params":{"id":"t1","seq":3,"data":b64_encode("한".as_bytes())}}),
            json!({"jsonrpc":"2.0","method":"some.future.event","params":{}}),
            json!({"jsonrpc":"2.0","method":"terminal.resized","params":{"id":"t1","seq":6,"cols":100,"rows":30}}),
        ] {
            out.extend(serde_json::to_vec(&n).unwrap());
            out.push(b'\n');
        }
        Some(out)
    });
    let (res, snap, mut stream) = c
        .attach(&AttachParams {
            target: TerminalRef::id("t1"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(res.seq, 3);
    assert_eq!(snap, b"abc");
    assert_eq!(
        stream.next_event().unwrap(),
        Some(AttachEvent::Output {
            seq: 3,
            data: "한".as_bytes().to_vec()
        })
    );
    // The unknown notification is skipped (forward compatible).
    assert_eq!(
        stream.next_event().unwrap(),
        Some(AttachEvent::Resized {
            seq: 6,
            cols: 100,
            rows: 30
        })
    );
}

#[test]
fn child_stdio_transport() {
    // A scripted "daemon": a child process printing canned response lines.
    let dir = std::env::temp_dir().join(unique("child-stdio"));
    std::fs::create_dir_all(&dir).unwrap();
    let canned = dir.join("responses.ndjson");
    let mut text = String::new();
    for id in 1..=2 {
        text.push_str(
            &String::from_utf8(response_line(&Some(json!(id)), status_value(12))).unwrap(),
        );
    }
    text.push_str("{\"jsonrpc\":\"2.0\",\"id\":3,"); // cut by EOF
    std::fs::write(&canned, text).unwrap();
    let mut child = Command::new("cmd")
        .args(["/d", "/c", "type"])
        .arg(&canned)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let t = Transport::from_child(&mut child).unwrap();
    let mut c = Client::from_transport("stdio:type", t);
    assert_eq!(c.call::<ServerStatus>(&Empty {}).unwrap().protocol, 12); // canned replies
    assert_eq!(c.call::<ServerStatus>(&Empty {}).unwrap().protocol, 12); // canned replies
    match c.call::<ServerStatus>(&Empty {}) {
        Err(ClientError::Protocol(m)) => assert_eq!(m, "daemon closed the connection"),
        // Writing the request may fail first once the child has exited.
        Err(ClientError::Io(_)) => {}
        other => panic!("{other:?}"),
    }
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn named_pipe_regression_and_attach_stream_switch() {
    let d = Daemon::start("r1-pipe");
    let mut c = d.client();
    assert_eq!(c.pipe(), d.pipe);
    let s = c.call::<ServerStatus>(&Empty {}).unwrap();
    assert_eq!(s.protocol, PROTOCOL_VERSION);
    let t = c
        .call::<TerminalRun>(&RunParams {
            argv: vec![
                "cmd".into(),
                "/d".into(),
                "/q".into(),
                "/k".into(),
                "prompt $G".into(),
            ],
            cwd: cwd(),
            name: Some("r1".into()),
            ..Default::default()
        })
        .unwrap();
    let (res, _snap, mut stream) = d
        .client()
        .attach(&AttachParams {
            target: TerminalRef::id(&t.id),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(res.terminal.id, t.id);
    send(&mut c, &t.id, "echo r1-marker-한글");
    let mut seen = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        if let Some(AttachEvent::Output { data, .. }) = stream.next_event().unwrap() {
            seen.extend(data);
            if String::from_utf8_lossy(&seen).matches("r1-marker").count() >= 2 {
                break;
            }
        }
    }
    assert!(
        String::from_utf8_lossy(&seen).matches("r1-marker").count() >= 2,
        "attach stream did not deliver the echo"
    );
    drop(stream);
    // The control connection still works after the other one became a stream.
    c.call::<TerminalKill>(&TerminalRef::id(&t.id)).unwrap();
}

#[test]
fn named_pipe_transport_reads_and_writes_at_the_same_time() {
    // A synchronous pipe handle serializes I/O: a read blocked in one thread
    // used to stall the write in another forever (the bridge deadlock).
    let d = Daemon::start("r1-duplex");
    let (mut r, mut w) = d.client().into_transport().into_parts();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(r.read_line().map_err(|e| e.to_string()));
    });
    std::thread::sleep(Duration::from_millis(200)); // the read is now blocked
    let (wtx, wrx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = wtx.send(
            w.write_line(r#"{"jsonrpc":"2.0","id":7,"method":"server.status"}"#)
                .map_err(|e| e.to_string()),
        );
        std::thread::sleep(Duration::from_secs(5)); // keep the pipe open
    });
    wrx.recv_timeout(Duration::from_secs(10))
        .expect("write blocked behind the pending read")
        .unwrap();
    let line = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("no answer")
        .unwrap()
        .unwrap();
    let v: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["id"], 7);
    assert_eq!(v["result"]["protocol"], PROTOCOL_VERSION);
}

#[test]
fn daemon_refuses_an_oversized_request_line_and_keeps_serving() {
    let d = Daemon::start("r1-bigline");
    // Retry while every pipe instance is busy (between accepts).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut f = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&d.pipe)
        {
            Ok(f) => break f,
            Err(e) if e.raw_os_error() == Some(231) && std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("open {}: {e}", d.pipe),
        }
    };
    // 5 MiB without a newline (limit 4 MiB). The daemon closes this
    // connection; our writes then fail, which is fine.
    let chunk = vec![b' '; 64 * 1024];
    let mut closed = false;
    for _ in 0..80 {
        if f.write_all(&chunk).is_err() {
            closed = true;
            break;
        }
    }
    if !closed {
        // Before the limit was checked while reading, the daemon kept
        // buffering a line with no newline forever; now it hangs up.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = [0u8; 16];
            let _ = tx.send(std::io::Read::read(&mut f, &mut buf).unwrap_or(0));
        });
        let n = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the daemon kept reading an oversized line");
        assert_eq!(n, 0, "the daemon answered an oversized line");
    }
    // Other clients are unaffected.
    assert_eq!(
        d.client().call::<ServerStatus>(&Empty {}).unwrap().protocol,
        PROTOCOL_VERSION
    );
}

// ---------------------------------------------------------------------------
// R2: connection.hello
// ---------------------------------------------------------------------------

#[test]
fn hello_against_a_real_v12_daemon() {
    let name = unique("r2-hello");
    let state = std::env::temp_dir().join(&name);
    let pipe = format!(r"\\.\pipe\{name}");
    let d = Daemon::start_on(pipe.clone(), state.clone());
    let h = d.client().hello(&[]).unwrap();
    assert_eq!(h.protocol, PROTOCOL_VERSION);
    assert_eq!(
        (h.min_protocol, h.max_protocol),
        (PROTOCOL_MIN, PROTOCOL_VERSION)
    );
    assert_eq!(h.os, "windows");
    assert_eq!(h.machine_id.len(), 32);
    assert_eq!(h.boot_id.len(), 32);
    assert_ne!(h.machine_id, h.boot_id);
    assert_eq!(h.limits.max_line_bytes, 8 * 1024 * 1024);
    assert_eq!(h.limits.max_request_bytes, 4 * 1024 * 1024);
    // Same daemon, another connection: same ids.
    let again = d.client().hello(&[]).unwrap();
    assert_eq!(
        (again.machine_id.as_str(), again.boot_id.as_str()),
        (h.machine_id.as_str(), h.boot_id.as_str())
    );

    // Required capability that this podd does not have: refused, nothing done.
    let e = d.client().hello(&[capability::REMOTE_FILES]).unwrap_err();
    assert!(e.is_incompatible(), "{e:?}");

    // A future client that only speaks newer versions only: PROTOCOL_MISMATCH with the range.
    let mut c = d.client();
    let r = c.call::<ConnectionHello>(&HelloParams {
        min_protocol: PROTOCOL_VERSION + 1,
        max_protocol: PROTOCOL_VERSION + 2,
        ..Default::default()
    });
    match r {
        Err(ClientError::Rpc(e)) => {
            assert_eq!(e.code, remote_codes::PROTOCOL_MISMATCH);
            assert_eq!(e.data.as_ref().unwrap()["max_protocol"], PROTOCOL_VERSION);
        }
        other => panic!("{other:?}"),
    }
    // The connection is still usable after a refused hello.
    assert_eq!(
        c.call::<ServerStatus>(&Empty {}).unwrap().protocol,
        PROTOCOL_VERSION
    );

    // Restart on the same state dir: machine id persists, boot id changes.
    let (pipe, state) = d.stop(false);
    let d = Daemon::start_on(pipe, state);
    let h2 = d.client().hello(&[]).unwrap();
    assert_eq!(h2.machine_id, h.machine_id);
    assert_ne!(h2.boot_id, h.boot_id);
}

#[test]
fn hello_rejects_a_v11_daemon_without_sending_anything_else() {
    let methods = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let m = methods.clone();
    let mut c = fake_server(move |req| {
        m.lock().unwrap().push(req.method.clone());
        match req.method.as_str() {
            "server.status" => Some(response_line(&req.id, status_value(11))),
            _ => panic!("unexpected {}", req.method),
        }
    });
    let e = c.hello(&[]).unwrap_err();
    assert!(e.is_incompatible(), "{e:?}");
    assert!(e.to_string().contains("protocol 11"), "{e}");
    assert_eq!(*methods.lock().unwrap(), vec!["server.status".to_string()]);
}

#[test]
fn hello_rejects_a_future_daemon_without_a_common_version() {
    // Reports V+2, serves only V+1..=V+2: answers hello with PROTOCOL_MISMATCH.
    let mut c = fake_server(|req| match req.method.as_str() {
        "server.status" => Some(response_line(&req.id, status_value(PROTOCOL_VERSION + 2))),
        "connection.hello" => {
            let p: HelloParams = serde_json::from_value(req.params).unwrap();
            assert_eq!((p.min_protocol, p.max_protocol), (12, PROTOCOL_VERSION));
            let e = negotiate(&p, (PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 2), &[]).unwrap_err();
            let mut s =
                serde_json::to_string(&json!({"jsonrpc":"2.0","id":req.id,"error":e})).unwrap();
            s.push('\n');
            Some(s.into_bytes())
        }
        m => panic!("unexpected {m}"),
    });
    let e = c.hello(&[]).unwrap_err();
    assert!(e.is_incompatible(), "{e:?}");
}

#[test]
fn hello_accepts_a_future_daemon_that_still_speaks_12() {
    let mut c = fake_server(|req| match req.method.as_str() {
        "server.status" => Some(response_line(&req.id, status_value(PROTOCOL_VERSION + 3))),
        "connection.hello" => {
            let p: HelloParams = serde_json::from_value(req.params).unwrap();
            let v = negotiate(&p, (11, PROTOCOL_VERSION + 3), &["future.thing"]).unwrap();
            Some(response_line(
                &req.id,
                json!({
                    "protocol": v, "min_protocol": 11, "max_protocol": PROTOCOL_VERSION + 3,
                    "capabilities": ["future.thing"], "machine_id": "m", "boot_id": "b",
                    "version": "9.9.9", "os": "windows", "extra_future_field": true,
                    "limits": {"max_line_bytes": 1, "max_request_bytes": 1,
                               "max_output_chunk_bytes": 1, "attach_queue_bytes": 1}
                }),
            ))
        }
        m => panic!("unexpected {m}"),
    });
    let h = c.hello(&[]).unwrap();
    assert_eq!(h.protocol, PROTOCOL_VERSION);
}

#[test]
fn hello_rejects_a_daemon_that_picks_a_version_we_do_not_speak() {
    let mut c = fake_server(|req| match req.method.as_str() {
        "server.status" => Some(response_line(&req.id, status_value(PROTOCOL_VERSION + 1))),
        "connection.hello" => Some(response_line(
            &req.id,
            json!({
                "protocol": PROTOCOL_VERSION + 1, "min_protocol": PROTOCOL_VERSION + 1, "max_protocol": PROTOCOL_VERSION + 1,
                "capabilities": [], "machine_id": "m", "boot_id": "b",
                "version": "9", "os": "windows",
                "limits": {"max_line_bytes": 1, "max_request_bytes": 1,
                           "max_output_chunk_bytes": 1, "attach_queue_bytes": 1}
            }),
        )),
        m => panic!("unexpected {m}"),
    });
    assert!(c.hello(&[]).unwrap_err().is_incompatible());
}

/// `LineWriter` is what the bridge uses for stdout: one write per line.
#[test]
fn line_writer_roundtrip_over_a_pipe() {
    let (r, w) = std::io::pipe().unwrap();
    let mut w = LineWriter::new(w);
    w.write_line("{\"a\":1}").unwrap();
    drop(w);
    let mut r = LineReader::new(r, 16);
    assert_eq!(r.read_line().unwrap().as_deref(), Some("{\"a\":1}"));
}
