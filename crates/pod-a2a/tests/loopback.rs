//! Loopback tests: the real client and transport against a scripted HTTP
//! server on 127.0.0.1 (test-only). They check what goes on the wire
//! (method, path, headers, JSON-RPC body, credential placement) and that
//! redirects, auth challenges, oversized or missing answers are reported,
//! never followed or retried.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pod_a2a::model::{
    ListTasksRequest, Message, SendMessageConfiguration, SendMessageResponse, TaskState,
};
use pod_a2a::target::TargetPolicy;
use pod_a2a::{
    A2aClient, A2aError, BearerToken, CardSource, ClientConfig, Delivery, RpcErrorKind, discover,
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
struct Captured {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

enum Reply {
    Http {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
    },
    /// Read the request, then close without answering.
    Close,
    /// Read the request, then never answer.
    Hang,
}

fn json_reply(status: u16, v: Value) -> Reply {
    Reply::Http {
        status,
        headers: vec![("Content-Type", "application/json".into())],
        body: serde_json::to_vec(&v).unwrap(),
    }
}

struct Server {
    port: u16,
    seen: Arc<Mutex<Vec<Captured>>>,
}

impl Server {
    fn requests(&self) -> Vec<Captured> {
        self.seen.lock().unwrap().clone()
    }
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

fn read_request(s: &mut TcpStream) -> Option<Captured> {
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
    let method = first.next()?.to_string();
    let path = first.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        let n = s.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some(Captured {
        method,
        path,
        headers,
        body,
    })
}

/// Serves until the test process ends; `handler` gets the port and request.
fn serve(handler: impl Fn(u16, &Captured) -> Reply + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let handler = Arc::new(handler);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let seen = seen2.clone();
            let handler = handler.clone();
            thread::spawn(move || {
                let Some(req) = read_request(&mut s) else {
                    return;
                };
                seen.lock().unwrap().push(req.clone());
                match handler(port, &req) {
                    Reply::Http {
                        status,
                        headers,
                        body,
                    } => {
                        let mut out = format!(
                            "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                            body.len()
                        );
                        for (k, v) in headers {
                            out.push_str(&format!("{k}: {v}\r\n"));
                        }
                        out.push_str("\r\n");
                        let _ = s.write_all(out.as_bytes());
                        let _ = s.write_all(&body);
                    }
                    Reply::Close => {}
                    Reply::Hang => thread::sleep(Duration::from_secs(30)),
                }
            });
        }
    });
    Server { port, seen }
}

fn card(port: u16, extra: Value) -> Value {
    let mut c = json!({
        "name": "loopback agent",
        "description": "test",
        "version": "1",
        "supportedInterfaces": [{
            "url": format!("http://127.0.0.1:{port}/rpc"),
            "protocolBinding": "JSONRPC",
            "protocolVersion": "1.0"
        }],
        "capabilities": {},
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
        "skills": []
    });
    if let (Value::Object(c), Value::Object(e)) = (&mut c, extra) {
        c.extend(e);
    }
    c
}

fn completed(id: &Value) -> Reply {
    json_reply(
        200,
        json!({"jsonrpc": "2.0", "id": id, "result": {"task": {
        "id": "t1", "contextId": "c1",
        "status": {"state": "TASK_STATE_COMPLETED"},
        "artifacts": [{"artifactId": "a1", "parts": [{"text": "ok"}]}]}}}),
    )
}

/// A GetTask/CancelTask result: the Task itself (no oneof wrapper).
fn bare_task(id: &Value, task_id: &str) -> Reply {
    json_reply(
        200,
        json!({"jsonrpc": "2.0", "id": id, "result": {
        "id": task_id, "status": {"state": "TASK_STATE_WORKING"}}}),
    )
}

fn cfg() -> ClientConfig {
    ClientConfig {
        policy: TargetPolicy {
            allow_http_loopback: true,
        },
        connect_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(5),
        ..ClientConfig::default()
    }
}

/// Card at the well-known path, `rpc` for POST /rpc.
fn agent(card_extra: Value, rpc: impl Fn(&Captured) -> Reply + Send + Sync + 'static) -> Server {
    serve(move |port, req| {
        if req.method == "GET" && req.path == "/.well-known/agent-card.json" {
            json_reply(200, card(port, card_extra.clone()))
        } else if req.method == "POST" && req.path == "/rpc" {
            rpc(req)
        } else {
            json_reply(404, json!({}))
        }
    })
}

fn client(srv: &Server, token: Option<&str>) -> A2aClient {
    let c = cfg();
    let d = discover(&CardSource::Peer(srv.base()), &c).unwrap();
    let iface = d.select(None, c.policy).unwrap();
    A2aClient::new(iface, token.map(|t| BearerToken::new(t).unwrap()), c).unwrap()
}

#[test]
fn send_message_wire_format_and_task_result() {
    let srv = agent(json!({}), |req| completed(&req.json()["id"]));
    let c = client(&srv, None);
    let mut msg = Message::user_text("m-1", "hello");
    msg.context_id = "c1".into();
    let r = c
        .send_message(
            msg,
            Some(SendMessageConfiguration {
                return_immediately: true,
                ..Default::default()
            }),
        )
        .unwrap();
    let SendMessageResponse::Task(t) = r else {
        panic!()
    };
    assert_eq!(t.status.state, TaskState::Completed);

    let reqs = srv.requests();
    assert_eq!(reqs.len(), 2, "card + one SendMessage, nothing else");
    let (get, post) = (&reqs[0], &reqs[1]);
    assert_eq!(
        (get.method.as_str(), get.path.as_str()),
        ("GET", "/.well-known/agent-card.json")
    );
    assert_eq!((post.method.as_str(), post.path.as_str()), ("POST", "/rpc"));
    assert_eq!(post.header("A2A-Version"), Some("1.0"));
    assert_eq!(post.header("Content-Type"), Some("application/json"));
    assert_eq!(post.header("Accept"), Some("application/json"));
    assert_eq!(
        post.header("Host"),
        Some(format!("127.0.0.1:{}", srv.port).as_str())
    );
    assert!(post.header("Authorization").is_none());
    assert!(post.header("Cookie").is_none());
    let body = post.json();
    assert_eq!(body["jsonrpc"], "2.0");
    assert_eq!(body["method"], "SendMessage");
    assert!(body["id"].as_str().unwrap().starts_with("pod-"));
    assert_eq!(
        body["params"],
        json!({"message": {"messageId": "m-1", "contextId": "c1", "role": "ROLE_USER",
                           "parts": [{"text": "hello"}]},
               "configuration": {"returnImmediately": true}})
    );
}

#[test]
fn bearer_token_goes_to_the_rpc_endpoint_only_never_the_card() {
    let srv = agent(
        json!({"securitySchemes": {"bearer": {"httpAuthSecurityScheme": {"scheme": "Bearer"}}},
               "securityRequirements": [{"schemes": {"bearer": {"list": []}}}]}),
        |req| bare_task(&req.json()["id"], "t1"),
    );
    let c = client(&srv, Some("s3cr3t-token"));
    c.get_task("t1", None).unwrap_or_else(|e| panic!("{e}"));
    let reqs = srv.requests();
    assert!(
        reqs[0].header("Authorization").is_none(),
        "card fetch carries no credential"
    );
    assert_eq!(reqs[1].header("Authorization"), Some("Bearer s3cr3t-token"));
    assert!(!format!("{:?}", BearerToken::new("s3cr3t-token").unwrap()).contains("s3cr3t"));
}

#[test]
fn get_list_cancel_send_tenant_and_exact_methods() {
    let srv = agent(json!({}), |req| {
        let b = req.json();
        let id = &b["id"];
        match b["method"].as_str().unwrap() {
            "GetTask" | "CancelTask" => json_reply(
                200,
                json!({"jsonrpc": "2.0", "id": id, "result": {
                "id": b["params"]["id"], "status": {"state": if b["method"] == "CancelTask" {
                    "TASK_STATE_CANCELED" } else { "TASK_STATE_WORKING" }}}}),
            ),
            "ListTasks" => json_reply(
                200,
                json!({"jsonrpc": "2.0", "id": id, "result": {
                "tasks": [{"id": "t1", "status": {"state": "TASK_STATE_WORKING"}}],
                "nextPageToken": "", "pageSize": 50, "totalSize": 1}}),
            ),
            _ => json_reply(
                200,
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "Method not found"}}),
            ),
        }
    });
    let c = client(&srv, None);
    assert_eq!(
        c.get_task("t9", Some(2)).unwrap().status.state,
        TaskState::Working
    );
    let l = c
        .list_tasks(ListTasksRequest {
            status: Some(TaskState::Working),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(l.tasks.len(), 1);
    assert_eq!(
        c.cancel_task("t9").unwrap().status.state,
        TaskState::Canceled
    );
    let bodies: Vec<Value> = srv.requests()[1..].iter().map(Captured::json).collect();
    assert_eq!(bodies[0]["method"], "GetTask");
    assert_eq!(bodies[0]["params"], json!({"id": "t9", "historyLength": 2}));
    assert_eq!(bodies[1]["method"], "ListTasks");
    assert_eq!(bodies[1]["params"], json!({"status": "TASK_STATE_WORKING"}));
    assert_eq!(bodies[2]["method"], "CancelTask");
    assert_eq!(bodies[2]["params"], json!({"id": "t9"}));
}

#[test]
fn a_task_id_mismatch_is_an_invalid_agent_response() {
    let srv = agent(json!({}), |req| bare_task(&req.json()["id"], "t1"));
    let e = client(&srv, None).get_task("other", None).unwrap_err();
    assert!(
        matches!(e, A2aError::Protocol(ref s) if s.contains("returned task")),
        "{e:?}"
    );
}

#[test]
fn redirects_are_reported_not_followed() {
    let target = serve(|_, _| json_reply(200, json!({})));
    let location = format!("{}/elsewhere", target.base());
    let srv = agent(json!({}), move |_| Reply::Http {
        status: 307,
        headers: vec![("Location", location.clone())],
        body: Vec::new(),
    });
    let e = client(&srv, Some("tok")).get_task("t1", None).unwrap_err();
    assert!(matches!(e, A2aError::Redirect { status: 307, .. }), "{e:?}");
    assert_eq!(srv.requests().len(), 2);
    assert!(
        target.requests().is_empty(),
        "the redirect target was never contacted"
    );
}

#[test]
fn http_401_is_auth_required_with_the_scheme_name_only() {
    let srv = agent(json!({}), |_| Reply::Http {
        status: 401,
        headers: vec![(
            "WWW-Authenticate",
            "Bearer realm=\"x\", error=\"invalid_token\"".into(),
        )],
        body: b"nope".to_vec(),
    });
    let e = client(&srv, None).get_task("t1", None).unwrap_err();
    assert_eq!(
        e,
        A2aError::AuthRequired {
            status: 401,
            challenges: vec!["Bearer".into()]
        }
    );
    assert!(!e.outcome_unknown());
}

#[test]
fn jsonrpc_error_with_http_error_status_is_still_the_peer_answer() {
    let srv = agent(json!({}), |req| {
        json_reply(
            400,
            json!({"jsonrpc": "2.0", "id": req.json()["id"],
            "error": {"code": -32009, "message": "Version not supported"}}),
        )
    });
    let A2aError::Rpc(e) = client(&srv, None).get_task("t1", None).unwrap_err() else {
        panic!()
    };
    assert_eq!(e.kind, RpcErrorKind::VersionNotSupported);
}

#[test]
fn oversized_responses_are_cut_off() {
    let srv = agent(json!({}), |_| Reply::Http {
        status: 200,
        headers: vec![("Content-Type", "application/json".into())],
        body: vec![b' '; 64 * 1024],
    });
    let mut c = cfg();
    c.max_response_bytes = 1024;
    let d = discover(&CardSource::Peer(srv.base()), &c).unwrap();
    let client = A2aClient::new(d.select(None, c.policy).unwrap(), None, c).unwrap();
    let e = client
        .send_message(Message::user_text("m", "x"), None)
        .unwrap_err();
    assert_eq!(e, A2aError::TooLarge { limit: 1024 });
    assert!(e.outcome_unknown());
}

#[test]
fn a_dropped_connection_after_sending_is_unknown_and_not_retried() {
    let srv = agent(json!({}), |_| Reply::Close);
    let e = client(&srv, None)
        .send_message(Message::user_text("m-9", "x"), None)
        .unwrap_err();
    assert!(
        matches!(
            e,
            A2aError::Transport {
                delivery: Delivery::Unknown,
                ..
            }
        ),
        "{e:?}"
    );
    assert!(e.outcome_unknown());
    let posts = srv.requests().iter().filter(|r| r.method == "POST").count();
    assert_eq!(posts, 1, "sent exactly once");
}

#[test]
fn a_silent_peer_times_out_as_unknown_and_is_not_retried() {
    let srv = agent(json!({}), |_| Reply::Hang);
    let mut c = cfg();
    c.request_timeout = Duration::from_millis(700);
    let d = discover(&CardSource::Peer(srv.base()), &c).unwrap();
    let client = A2aClient::new(d.select(None, c.policy).unwrap(), None, c).unwrap();
    let e = client
        .send_message(Message::user_text("m", "x"), None)
        .unwrap_err();
    assert!(
        matches!(
            e,
            A2aError::Timeout {
                delivery: Delivery::Unknown,
                ..
            }
        ),
        "{e:?}"
    );
    assert!(e.outcome_unknown());
    assert_eq!(
        srv.requests().iter().filter(|r| r.method == "POST").count(),
        1
    );
}

#[test]
fn nothing_listening_is_not_sent() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let e = discover(
        &CardSource::Peer(format!("http://127.0.0.1:{port}")),
        &cfg(),
    )
    .unwrap_err();
    assert!(
        matches!(
            e,
            A2aError::Transport {
                delivery: Delivery::NotSent,
                ..
            }
        ),
        "{e:?}"
    );
    assert!(!e.outcome_unknown());
}

#[test]
fn non_json_and_non_200_cards_are_refused() {
    let html = serve(|_, _| Reply::Http {
        status: 200,
        headers: vec![("Content-Type", "text/html".into())],
        body: b"<html>".to_vec(),
    });
    assert!(matches!(
        discover(&CardSource::Peer(html.base()), &cfg()),
        Err(A2aError::Card(_))
    ));
    let missing = serve(|_, _| json_reply(404, json!({})));
    let e = discover(&CardSource::Peer(missing.base()), &cfg()).unwrap_err();
    assert!(e.to_string().contains("HTTP 404"), "{e}");
    let redirect = serve(|port, _| Reply::Http {
        status: 302,
        headers: vec![("Location", format!("http://127.0.0.1:{port}/x"))],
        body: Vec::new(),
    });
    assert!(matches!(
        discover(&CardSource::Peer(redirect.base()), &cfg()),
        Err(A2aError::Redirect { status: 302, .. })
    ));
    assert_eq!(redirect.requests().len(), 1);
}

#[test]
fn explicit_card_url_is_fetched_as_given() {
    let srv = serve(|port, req| {
        if req.path == "/cards/a.json" {
            json_reply(200, card(port, json!({})))
        } else {
            json_reply(404, json!({}))
        }
    });
    let d = discover(
        &CardSource::CardUrl(format!("{}/cards/a.json", srv.base())),
        &cfg(),
    )
    .unwrap();
    assert_eq!(d.card.name, "loopback agent");
}

#[test]
fn https_to_a_plaintext_peer_fails_before_sending() {
    let srv = serve(|_, _| json_reply(200, json!({})));
    let c = ClientConfig {
        connect_timeout: Duration::from_secs(2),
        ..ClientConfig::default()
    };
    let e = discover(
        &CardSource::Peer(format!("https://127.0.0.1:{}", srv.port)),
        &c,
    )
    .unwrap_err();
    // The handshake fails or times out, or (with a broken SSL_CERT_FILE) no
    // trust roots load: in every case nothing was sent in the clear.
    assert!(
        matches!(
            e,
            A2aError::Transport {
                delivery: Delivery::NotSent,
                ..
            } | A2aError::Timeout {
                delivery: Delivery::NotSent,
                ..
            }
        ),
        "{e:?}"
    );
    assert!(
        srv.requests().is_empty(),
        "no plaintext HTTP request reached the server"
    );
}
