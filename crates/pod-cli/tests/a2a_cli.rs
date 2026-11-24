//! `pod a2a` end to end against a scripted A2A peer on 127.0.0.1 (test
//! only): exit codes, JSON output, refusals that happen before any request,
//! no retry after an unknown outcome, and peer text printed as data.
//! No daemon is started: `pod a2a` never connects to podd.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::thread;

use serde_json::{Value, json};

const POD: &str = env!("CARGO_BIN_EXE_pod");

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    path: String,
    auth: Option<String>,
}

enum Reply {
    Json(Value),
    Close,
}

struct Peer {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Peer {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    fn posts(&self) -> usize {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.method == "POST")
            .count()
    }
}

fn read_request(s: &mut TcpStream) -> Option<(Seen, Value)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, path) = (first.next()?.to_string(), first.next()?.to_string());
    let mut len = 0;
    let mut auth = None;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().unwrap_or(0);
            }
            if k.eq_ignore_ascii_case("authorization") {
                auth = Some(v.trim().to_string());
            }
        }
    }
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let v = serde_json::from_slice(&body).unwrap_or(Value::Null);
    Some((Seen { method, path, auth }, v))
}

/// An A2A peer: the card at the well-known path, `rpc` for POST /rpc.
fn peer(card_extra: Value, rpc: impl Fn(&Value) -> Reply + Send + Sync + 'static) -> Peer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let rpc = Arc::new(rpc);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let Some((req, body)) = read_request(&mut s) else {
                continue;
            };
            seen2.lock().unwrap().push(req.clone());
            let reply = if req.method == "GET" && req.path == "/.well-known/agent-card.json" {
                let mut card = json!({
                    "name": "cli test agent", "description": "d", "version": "1",
                    "supportedInterfaces": [{"url": format!("http://127.0.0.1:{port}/rpc"),
                        "protocolBinding": "JSONRPC", "protocolVersion": "1.0"}],
                    "capabilities": {}, "defaultInputModes": ["text/plain"],
                    "defaultOutputModes": ["text/plain"], "skills": []
                });
                if let (Value::Object(c), Value::Object(e)) = (&mut card, card_extra.clone()) {
                    c.extend(e);
                }
                Reply::Json(card)
            } else {
                rpc(&body)
            };
            if let Reply::Json(v) = reply {
                let body = serde_json::to_vec(&v).unwrap();
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = s.write_all(head.as_bytes());
                let _ = s.write_all(&body);
            }
        }
    });
    Peer { port, seen }
}

fn pod(args: &[&str]) -> Output {
    Command::new(POD)
        .args(args)
        // Never reach a real daemon or the user's pipe.
        .env("POD_SOCKET", r"\\.\pipe\pod-a2a-cli-test-unused")
        .env_remove("POD_ATTEMPT_ID")
        .env_remove("POD_ATTEMPT_TOKEN")
        .output()
        .unwrap()
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).to_string()
}

fn task_reply(body: &Value, state: &str, artifact_text: &str) -> Reply {
    Reply::Json(
        json!({"jsonrpc": "2.0", "id": body["id"], "result": {"task": {
        "id": "task-1", "contextId": "ctx-1", "status": {"state": state},
        "artifacts": [{"artifactId": "a1", "name": "out", "parts": [{"text": artifact_text}]}]}}}),
    )
}

#[test]
fn http_without_the_switch_is_refused_before_any_request() {
    let p = peer(json!({}), |b| task_reply(b, "TASK_STATE_COMPLETED", "x"));
    let out = pod(&["a2a", "card", "--peer", &p.url()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("--allow-http-loopback"),
        "{}",
        text(&out.stderr)
    );
    assert!(p.seen.lock().unwrap().is_empty());
    let out = pod(&[
        "a2a",
        "card",
        "--peer",
        "http://example.com/",
        "--allow-http-loopback",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("non-loopback"));
}

#[test]
fn send_json_reports_task_state_and_message_id() {
    let p = peer(json!({}), |b| {
        task_reply(b, "TASK_STATE_INPUT_REQUIRED", "need more")
    });
    let url = p.url();
    let out = pod(&[
        "--json",
        "a2a",
        "send",
        "--peer",
        &url,
        "--allow-http-loopback",
        "--text",
        "hello",
        "--message-id",
        "msg-cli-1",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["result"], "task");
    assert_eq!(v["messageId"], "msg-cli-1");
    assert_eq!(v["state"], "TASK_STATE_INPUT_REQUIRED");
    assert_eq!(v["stateClass"], "interrupted");
    assert_eq!(v["task"]["contextId"], "ctx-1");
    assert_eq!(v["interface"]["url"], format!("{url}/rpc"));
    // Text mode names the continuation command.
    let out = pod(&[
        "a2a",
        "send",
        "--peer",
        &url,
        "--allow-http-loopback",
        "--text",
        "hello",
    ]);
    let s = text(&out.stdout);
    assert!(s.contains("input-required"), "{s}");
    assert!(s.contains("--task-id task-1 --context-id ctx-1"), "{s}");
}

#[test]
fn an_unknown_outcome_exits_4_and_is_not_retried() {
    let p = peer(json!({}), |_| Reply::Close);
    let out = pod(&[
        "a2a",
        "send",
        "--peer",
        &p.url(),
        "--allow-http-loopback",
        "--text",
        "x",
        "--message-id",
        "msg-unknown-1",
    ]);
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    let err = text(&out.stderr);
    assert!(err.contains("NOT retried"), "{err}");
    assert!(err.contains("msg-unknown-1"), "{err}");
    assert_eq!(p.posts(), 1, "exactly one SendMessage");
}

#[test]
fn rpc_errors_exit_2_with_the_a2a_error_name() {
    let p = peer(json!({}), |b| {
        Reply::Json(json!({"jsonrpc": "2.0", "id": b["id"],
            "error": {"code": -32001, "message": "Task not found"}}))
    });
    let out = pod(&[
        "--json",
        "a2a",
        "task",
        "show",
        "--peer",
        &p.url(),
        "--allow-http-loopback",
        "nope",
    ]);
    assert_eq!(out.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["error"]["code"], "rpc-error");
    assert_eq!(v["error"]["rpc"]["kind"], "TaskNotFound");
    assert_eq!(v["error"]["outcomeUnknown"], false);
    assert!(text(&out.stderr).contains("TaskNotFoundError (-32001)"));
}

#[test]
fn a_token_is_not_sent_unless_the_card_declares_bearer() {
    let dir = std::env::temp_dir().join(format!("pod-a2a-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let token = dir.join("token.txt");
    std::fs::write(&token, "tok-123\n").unwrap();
    let token = token.to_str().unwrap().to_string();

    let plain = peer(json!({}), |b| task_reply(b, "TASK_STATE_COMPLETED", "x"));
    let out = pod(&[
        "a2a",
        "send",
        "--peer",
        &plain.url(),
        "--allow-http-loopback",
        "--text",
        "x",
        "--token-file",
        &token,
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("no HTTP Bearer"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(plain.posts(), 0);
    assert!(plain.seen.lock().unwrap().iter().all(|s| s.auth.is_none()));

    let bearer = peer(
        json!({"securitySchemes": {"b": {"httpAuthSecurityScheme": {"scheme": "bearer"}}}}),
        |b| task_reply(b, "TASK_STATE_COMPLETED", "x"),
    );
    let out = pod(&[
        "a2a",
        "send",
        "--peer",
        &bearer.url(),
        "--allow-http-loopback",
        "--text",
        "x",
        "--token-file",
        &token,
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let seen = bearer.seen.lock().unwrap().clone();
    assert_eq!(seen[0].auth, None, "the card fetch carries no token");
    assert_eq!(seen[1].auth.as_deref(), Some("Bearer tok-123"));
    assert!(!text(&out.stdout).contains("tok-123") && !text(&out.stderr).contains("tok-123"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn peer_text_is_printed_as_data() {
    let p = peer(json!({}), |b| {
        task_reply(
            b,
            "TASK_STATE_COMPLETED",
            "line1\n\u{1b}]0;pwned\u{7}\u{1b}[2Jline2",
        )
    });
    let out = pod(&[
        "a2a",
        "send",
        "--peer",
        &p.url(),
        "--allow-http-loopback",
        "--text",
        "x",
    ]);
    assert_eq!(out.status.code(), Some(0));
    let s = text(&out.stdout);
    assert!(!s.contains('\u{1b}') && !s.contains('\u{7}'), "{s:?}");
    assert!(s.contains("\\u{1b}]0;pwned"), "{s}");
}

#[test]
fn a2a_is_refused_with_machine() {
    let out = pod(&[
        "--machine",
        "box",
        "a2a",
        "card",
        "--peer",
        "https://a.example/",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out.stderr).contains("--machine box"));
}
