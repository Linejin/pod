//! The A2A section in the real workspace on GPUI's headless platform, talking
//! through the real `pod-a2a` client to a peer on 127.0.0.1:
//!
//! - [`Peer`]: a scripted A2A 1.0 JSON-RPC peer (std only, one exchange per
//!   connection) that records every request as it arrived;
//! - `POD_A2A_UI_FIXTURE=<base url>`: the same UI flow against an external
//!   standard peer (the official a2a-python SDK echo fixture of the client
//!   report). Without the variable that test says so and does nothing.
//!
//! Proven here: nothing is fetched before a peer is chosen; nothing is sent
//! before the review; what arrives is exactly what was reviewed; task/context
//! ids, remote states and results come from the peer's SendMessage/GetTask
//! answers; an unknown outcome keeps the draft and is not resent; refresh
//! stops without a viewer; one project's peers and tasks never show in
//! another's graph; the graph and agent tree keep working.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use core::prelude::v1::test;
use gpui_kit::*;
use serde_json::{Value, json};

use crate::native_qa::Harness;
use crate::source::TestBackend;
use crate::vm::{RepoVm, TaskVm};

// ---------------------------------------------------------------- the peer

/// One request as the peer received it.
#[derive(Clone, Debug)]
struct Rec {
    line: String,
    rpc: Option<String>,
    a2a_version: Option<String>,
    authorization: bool,
    body: Value,
}

#[derive(Default)]
struct PeerState {
    log: Vec<Rec>,
    /// task id → (state, context, GetTask answers so far, text)
    tasks: HashMap<String, (String, String, u32, String)>,
    /// message ids whose first send was dropped
    dropped: Vec<String>,
    next: u32,
}

struct Peer {
    port: u16,
    state: Arc<Mutex<PeerState>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Peer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(PeerState::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (state.clone(), stop.clone());
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if st.load(Ordering::SeqCst) {
                    break;
                }
                if let Ok(c) = conn {
                    serve(c, port, &s);
                }
            }
        });
        Self {
            port,
            state,
            stop,
            thread: Some(thread),
        }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn log(&self) -> Vec<Rec> {
        self.state.lock().unwrap().log.clone()
    }

    fn rpcs(&self, method: &str) -> Vec<Rec> {
        self.log()
            .into_iter()
            .filter(|r| r.rpc.as_deref() == Some(method))
            .collect()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn read_request(c: &mut TcpStream) -> Option<(String, HashMap<String, String>, Vec<u8>)> {
    c.set_read_timeout(Some(Duration::from_secs(10))).ok()?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = c.read(&mut chunk).ok()?;
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
    let line = lines.next()?.to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < len {
        let n = c.read(&mut chunk).ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((line, headers, body))
}

fn respond(c: &mut TcpStream, body: &Value) {
    let b = body.to_string();
    let _ = write!(
        c,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
        b.len()
    );
    let _ = c.flush();
}

fn card(port: u16) -> Value {
    json!({
        "name": "Scripted peer",
        "description": "Loopback A2A 1.0 peer of the pod-app tests",
        "version": "1.0.0",
        "supportedInterfaces": [{
            "url": format!("http://127.0.0.1:{port}/rpc"),
            "protocolBinding": "JSONRPC",
            "protocolVersion": "1.0"
        }],
        "capabilities": {"streaming": false, "pushNotifications": false},
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain", "application/json"],
        "skills": [
            {"id": "echo", "name": "Echo", "description": "Echoes text", "tags": ["echo"],
             "outputModes": ["text/plain"]}
        ]
    })
}

fn task_json(id: &str, ctx: &str, state: &str, text: &str, done: bool) -> Value {
    let mut t = json!({
        "id": id,
        "contextId": ctx,
        "status": {
            "state": state,
            "message": {"messageId": format!("{id}-status"), "role": "ROLE_AGENT",
                        "parts": [{"text": format!("status of {text}")}]},
            "timestamp": "2026-11-24T00:00:00Z"
        }
    });
    if done {
        t["artifacts"] = json!([{
            "artifactId": format!("{id}-a"),
            "name": "echo",
            "parts": [
                {"text": format!("echo: {text}")},
                {"data": {"length": text.len()}},
                {"url": "https://files.invalid/report.pdf", "filename": "report.pdf"}
            ]
        }]);
    }
    t
}

fn serve(mut c: TcpStream, port: u16, state: &Arc<Mutex<PeerState>>) {
    let Some((line, headers, body)) = read_request(&mut c) else {
        return;
    };
    let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let rpc = json
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut s = state.lock().unwrap();
    s.log.push(Rec {
        line: line.clone(),
        rpc: rpc.clone(),
        a2a_version: headers.get("a2a-version").cloned(),
        authorization: headers.contains_key("authorization"),
        body: json.clone(),
    });
    if line.starts_with("GET /.well-known/agent-card.json") {
        drop(s);
        return respond(&mut c, &card(port));
    }
    let id = json.get("id").cloned().unwrap_or(Value::Null);
    let params = json.get("params").cloned().unwrap_or(Value::Null);
    let result = match rpc.as_deref() {
        Some("SendMessage") => {
            let m = &params["message"];
            let text = m["parts"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let mid = m["messageId"].as_str().unwrap_or_default().to_string();
            let word = text.split(' ').next().unwrap_or_default().to_string();
            if word == "drop" && !s.dropped.contains(&mid) {
                // Read, then hang up without an answer: outcome unknown.
                s.dropped.push(mid);
                return;
            }
            if word == "message" {
                json!({"message": {"messageId": "agent-reply", "contextId": "ctx-direct",
                    "role": "ROLE_AGENT", "parts": [{"text": format!("echo-message: {text}")}]}})
            } else if let Some(tid) = m["taskId"].as_str().filter(|t| !t.is_empty()) {
                let ctx = s.tasks.get(tid).map(|t| t.1.clone()).unwrap_or_default();
                s.tasks.insert(
                    tid.into(),
                    ("TASK_STATE_COMPLETED".into(), ctx.clone(), 0, text.clone()),
                );
                json!({"task": task_json(tid, &ctx, "TASK_STATE_COMPLETED", &text, true)})
            } else {
                s.next += 1;
                let (tid, ctx) = (format!("task-{}", s.next), format!("ctx-{}", s.next));
                let state = match word.as_str() {
                    "slow" => "TASK_STATE_WORKING",
                    "input" => "TASK_STATE_INPUT_REQUIRED",
                    _ => "TASK_STATE_COMPLETED",
                };
                s.tasks
                    .insert(tid.clone(), (state.into(), ctx.clone(), 0, text.clone()));
                json!({"task": task_json(&tid, &ctx, state, &text, state == "TASK_STATE_COMPLETED")})
            }
        }
        Some("GetTask") => {
            let tid = params["id"].as_str().unwrap_or_default().to_string();
            match s.tasks.get_mut(&tid) {
                Some(t) => {
                    t.2 += 1;
                    // A slow task finishes on its second GetTask.
                    if t.0 == "TASK_STATE_WORKING" && t.2 >= 2 {
                        t.0 = "TASK_STATE_COMPLETED".into();
                    }
                    let done = t.0 == "TASK_STATE_COMPLETED";
                    json!(task_json(&tid, &t.1, &t.0, &t.3, done))
                }
                None => {
                    drop(s);
                    return respond(
                        &mut c,
                        &json!({"jsonrpc": "2.0", "id": id,
                                "error": {"code": -32001, "message": "task not found"}}),
                    );
                }
            }
        }
        _ => {
            drop(s);
            return respond(
                &mut c,
                &json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32601, "message": "method not found"}}),
            );
        }
    };
    drop(s);
    respond(
        &mut c,
        &json!({"jsonrpc": "2.0", "id": id, "result": result}),
    );
}

// ---------------------------------------------------------------- the app

fn repo(name: &str) -> RepoVm {
    RepoVm {
        name: name.into(),
        path: format!("C:/fixtures/{name}"),
        default_base: Some("main".into()),
    }
}

fn seed(b: &mut TestBackend) {
    b.repos.extend([repo("alpha"), repo("beta")]);
    let mut k: TaskVm = TestBackend::task("alpha/lead", "리드", "alpha", &["c1"]);
    k.agent = Some("claude".into());
    b.tasks.push(k);
    let mut t = TestBackend::terminal("c1", "리더", Some("alpha/lead"), "working");
    t.argv = vec!["claude".into()];
    t.program = "claude".into();
    b.terminals.push(t);
}

fn peers_file(alpha_base: &str, beta_base: &str) -> String {
    json!({
        "version": 1,
        "projects": {
            "alpha": {"repo_path": "C:/fixtures/alpha", "peers": [
                {"id": "echo", "label": "Echo peer", "peer": alpha_base, "allow_http_loopback": true}
            ]},
            "beta": {"peers": [
                {"id": "other", "label": "Beta peer", "peer": beta_base, "allow_http_loopback": true}
            ]}
        }
    })
    .to_string()
}

fn center(b: Bounds<Pixels>) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width / 2.,
        b.origin.y + b.size.height / 2.,
    )
}

fn click(h: &mut Harness, mark: &str) {
    let b = h.bounds(mark).unwrap_or_else(|| panic!("{mark} is drawn"));
    h.cx.simulate_click(center(b), Modifiers::none());
    h.settle();
}

fn section(h: &mut Harness) -> Entity<super::A2aSection> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.a2a_section(cx)))
}

fn with<R>(h: &mut Harness, f: impl FnOnce(&super::A2aSection, &App) -> R) -> R {
    let s = section(h);
    h.cx.update(|_, cx| f(s.read(cx), cx))
}

fn draft(h: &mut Harness, repo: &str) -> String {
    with(h, |s, cx| {
        s.draft(repo)
            .map(|d| d.read(cx).value().to_string())
            .unwrap_or_default()
    })
}

fn open_graph(h: &mut Harness, repo: &str) {
    let ws = h.ws.clone();
    let repo = repo.to_string();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_project_graph(&repo, window, cx)));
    h.settle();
}

/// A test-owned config dir (layout.json, settings, a2a-peers.json) for the
/// life of the guard: these tests open the graph in the right dock, and a
/// layout saved into the process-wide test dir would reach other tests.
struct OwnConfig {
    dir: tempfile::TempDir,
}

impl OwnConfig {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("config dir");
        crate::layout::set_test_config_dir(Some(dir.path().to_path_buf()));
        Self { dir }
    }
}

impl Drop for OwnConfig {
    fn drop(&mut self) {
        crate::layout::set_test_config_dir(None);
    }
}

/// A harness in its own config dir whose A2A section reads `file`. The
/// window is tall so the whole section (card, review, task list) is on
/// screen without scrolling: clicks land where the user would click.
/// Bind as `let (_cfg, mut h)`: the harness drops before its config dir.
fn harness(name: &'static str, file: &str) -> (OwnConfig, Harness) {
    let cfg = OwnConfig::new();
    std::fs::write(super::config::path_in(cfg.dir.path()), file).unwrap();
    let mut h = Harness::sized(name, 1.0, size(px(1440.), px(4000.)), seed);
    let s = section(&mut h);
    let path = cfg.dir.path().to_path_buf();
    h.cx.update(|_, cx| s.update(cx, |s, _| s.config_dir = path));
    (cfg, h)
}

fn type_draft(h: &mut Harness, text: &str) {
    click(h, "a2a-draft");
    h.type_text(text);
}

fn advance(h: &mut Harness, secs: u64) {
    h.cx.executor().advance_clock(Duration::from_secs(secs));
    h.settle();
}

fn tasks_drawn(h: &mut Harness, ids: &[&str]) -> Vec<String> {
    ids.iter()
        .filter(|id| h.bounds(&format!("a2a-task:{id}")).is_some())
        .map(|s| s.to_string())
        .collect()
}

#[test]
fn review_then_send_reaches_the_peer_exactly_and_shows_the_remote_task() {
    let peer = Peer::start();
    let other = Peer::start();
    let (_cfg, mut h) = harness("a2a-review-send", &peers_file(&peer.base(), &other.base()));
    open_graph(&mut h, "alpha");
    assert!(h.bounds("a2a-section").is_none(), "hidden until asked for");
    click(&mut h, "a2a-toggle");
    assert!(h.bounds("a2a-section").is_some());
    assert!(h.bounds("a2a-peer:echo").is_some());
    assert!(
        h.bounds("a2a-peer:other").is_none(),
        "beta's peer is not offered"
    );
    assert!(
        peer.log().is_empty() && other.log().is_empty(),
        "nothing is contacted before a peer is chosen"
    );
    assert!(
        h.bounds("graph-node:c1").is_some(),
        "the graph is still there"
    );

    click(&mut h, "a2a-peer:echo");
    let log = peer.log();
    assert_eq!(log.len(), 1, "{log:?}");
    assert!(log[0].line.starts_with("GET /.well-known/agent-card.json"));
    assert!(!log[0].authorization);
    assert!(h.bounds("a2a-card").is_some());
    assert!(h.bounds("a2a-card-error").is_none());

    type_draft(&mut h, "hello peer");
    click(&mut h, "a2a-review-button");
    assert!(h.bounds("a2a-review").is_some(), "review shown");
    assert!(peer.rpcs("SendMessage").is_empty(), "review sends nothing");
    let reviewed = with(&mut h, |s, _| match &s.project("alpha").unwrap().submit {
        super::model::Submit::Reviewing(r) => (**r).clone(),
        other => panic!("not reviewing: {other:?}"),
    });
    assert_eq!(reviewed.text, "hello peer");
    assert_eq!(reviewed.interface_url, format!("{}/rpc", peer.base()));

    click(&mut h, "a2a-send");
    let sends = peer.rpcs("SendMessage");
    assert_eq!(sends.len(), 1);
    let sent = &sends[0];
    assert_eq!(sent.a2a_version.as_deref(), Some("1.0"));
    assert!(!sent.authorization, "no credential configured, none sent");
    let m = &sent.body["params"]["message"];
    assert_eq!(m["role"], "ROLE_USER");
    assert_eq!(m["messageId"], reviewed.message_id.as_str());
    assert_eq!(
        m["parts"],
        json!([{"text": "hello peer"}]),
        "exactly the reviewed text, one text part, nothing else"
    );
    assert!(m.get("taskId").is_none() && m.get("contextId").is_none());
    assert!(sent.body["params"].get("metadata").is_none());
    assert_eq!(
        sent.body["params"]["configuration"]["returnImmediately"],
        true
    );
    assert!(
        other.log().is_empty(),
        "the other project's peer is never contacted"
    );

    assert_eq!(tasks_drawn(&mut h, &["task-1"]), ["task-1"]);
    let t = with(&mut h, |s, _| s.project("alpha").unwrap().tasks[0].clone());
    assert_eq!(
        (t.task_id.as_str(), t.context_id.as_str()),
        ("task-1", "ctx-1")
    );
    assert_eq!(t.task.status.state, pod_a2a::model::TaskState::Completed);
    assert_eq!(
        draft(&mut h, "alpha"),
        "",
        "delivered: the draft is cleared"
    );
    assert!(
        h.bounds("a2a-task-0-art-0-part-0").is_some(),
        "the artifact's text part is shown"
    );
    assert!(
        h.bounds("a2a-task-0-art-0-part-2").is_some(),
        "its url part is shown as text"
    );

    // Input required → continue on the same task and context.
    type_draft(&mut h, "input more");
    click(&mut h, "a2a-review-button");
    click(&mut h, "a2a-send");
    assert!(h.bounds("a2a-task-continue:task-2").is_some());
    click(&mut h, "a2a-task-continue:task-2");
    assert!(h.bounds("a2a-continuing").is_some());
    type_draft(&mut h, "the answer");
    click(&mut h, "a2a-review-button");
    click(&mut h, "a2a-send");
    let last = peer.rpcs("SendMessage").pop().unwrap();
    let m = &last.body["params"]["message"];
    assert_eq!(
        (m["taskId"].clone(), m["contextId"].clone()),
        (json!("task-2"), json!("ctx-2"))
    );
    let t = with(&mut h, |s, _| s.project("alpha").unwrap().tasks[0].clone());
    assert_eq!(t.task_id, "task-2");
    assert_eq!(t.task.status.state, pod_a2a::model::TaskState::Completed);

    // A direct Message reply is listed apart, no task.
    type_draft(&mut h, "message hi");
    click(&mut h, "a2a-review-button");
    click(&mut h, "a2a-send");
    assert!(h.bounds("a2a-reply:0").is_some());

    // Pod orchestration is untouched: no A2A traffic went through podd.
    assert!(h.bounds("graph-node:c1").is_some());
}

#[test]
fn an_unknown_outcome_keeps_the_draft_and_resends_only_on_request() {
    let peer = Peer::start();
    let (_cfg, mut h) = harness("a2a-unknown", &peers_file(&peer.base(), &peer.base()));
    open_graph(&mut h, "alpha");
    click(&mut h, "a2a-toggle");
    click(&mut h, "a2a-peer:echo");
    type_draft(&mut h, "drop this");
    click(&mut h, "a2a-review-button");
    click(&mut h, "a2a-send");
    assert!(
        h.bounds("a2a-unknown").is_some(),
        "says the outcome is unknown"
    );
    assert_eq!(draft(&mut h, "alpha"), "drop this", "the draft is kept");
    assert_eq!(peer.rpcs("SendMessage").len(), 1);
    advance(&mut h, 120);
    assert_eq!(peer.rpcs("SendMessage").len(), 1, "never resent on its own");
    let first = peer.rpcs("SendMessage")[0].body["params"]["message"]["messageId"].clone();

    click(&mut h, "a2a-resend");
    assert!(
        h.bounds("a2a-review").is_some(),
        "the resend is reviewed first"
    );
    assert_eq!(peer.rpcs("SendMessage").len(), 1);
    click(&mut h, "a2a-send");
    let sends = peer.rpcs("SendMessage");
    assert_eq!(sends.len(), 2);
    assert_eq!(
        sends[1].body["params"]["message"]["messageId"], first,
        "the same message id, so the peer can spot a duplicate"
    );
    assert!(h.bounds("a2a-unknown").is_none());
    assert!(!tasks_drawn(&mut h, &["task-1"]).is_empty());
}

#[test]
fn refresh_runs_only_while_viewed_and_stops_at_a_terminal_state() {
    let peer = Peer::start();
    let (_cfg, mut h) = harness("a2a-refresh", &peers_file(&peer.base(), &peer.base()));
    open_graph(&mut h, "alpha");
    click(&mut h, "a2a-toggle");
    click(&mut h, "a2a-peer:echo");
    type_draft(&mut h, "slow work");
    click(&mut h, "a2a-review-button");
    click(&mut h, "a2a-send");
    let state = |h: &mut Harness| {
        with(h, |s, _| {
            s.project("alpha").unwrap().tasks[0]
                .task
                .status
                .state
                .clone()
        })
    };
    assert_eq!(state(&mut h), pod_a2a::model::TaskState::Working);
    assert!(peer.rpcs("GetTask").is_empty());
    advance(&mut h, 2);
    assert_eq!(peer.rpcs("GetTask").len(), 1);
    assert_eq!(state(&mut h), pod_a2a::model::TaskState::Working);

    // No viewer: hiding the section stops the refresh.
    click(&mut h, "a2a-toggle");
    assert!(h.bounds("a2a-section").is_none());
    advance(&mut h, 600);
    assert_eq!(peer.rpcs("GetTask").len(), 1, "no work without a viewer");
    assert!(!with(&mut h, |s, _| s.is_polling()));

    // Shown again: one refresh at once, then completion ends polling.
    click(&mut h, "a2a-toggle");
    assert_eq!(peer.rpcs("GetTask").len(), 2);
    assert_eq!(state(&mut h), pod_a2a::model::TaskState::Completed);
    advance(&mut h, 600);
    assert_eq!(peer.rpcs("GetTask").len(), 2, "terminal: no more requests");
    assert!(
        h.bounds("a2a-task-0-art-0-part-0").is_some(),
        "results shown"
    );
}

#[test]
fn another_project_sees_neither_the_peers_nor_the_tasks() {
    let peer = Peer::start();
    let other = Peer::start();
    let (_cfg, mut h) = harness("a2a-isolation", &peers_file(&peer.base(), &other.base()));
    open_graph(&mut h, "alpha");
    click(&mut h, "a2a-toggle");
    click(&mut h, "a2a-peer:echo");
    type_draft(&mut h, "alpha secret");
    click(&mut h, "a2a-review-button");
    click(&mut h, "a2a-send");
    assert_eq!(tasks_drawn(&mut h, &["task-1"]), ["task-1"]);
    type_draft(&mut h, "unsent alpha draft");

    open_graph(&mut h, "beta");
    assert!(
        h.bounds("a2a-section").is_some(),
        "the section follows the graph"
    );
    assert!(h.bounds("a2a-peer:echo").is_none());
    assert!(h.bounds("a2a-peer:other").is_some());
    assert!(
        tasks_drawn(&mut h, &["task-1"]).is_empty(),
        "alpha's task stays in alpha"
    );
    assert!(h.bounds("a2a-card").is_none(), "no peer chosen in beta");
    assert!(other.log().is_empty(), "nothing contacted for beta yet");
    let beta = with(&mut h, |s, _| {
        let p = s.project("beta").unwrap();
        (p.tasks.len(), p.selected.clone())
    });
    assert_eq!(beta, (0, None));

    open_graph(&mut h, "alpha");
    assert_eq!(tasks_drawn(&mut h, &["task-1"]), ["task-1"]);
    assert_eq!(draft(&mut h, "alpha"), "unsent alpha draft");
}

#[test]
fn no_configuration_and_unreachable_peers_explain_themselves() {
    let (_cfg, mut h) = harness(
        "a2a-offline",
        &peers_file("http://127.0.0.1:9", "http://127.0.0.1:9"),
    );
    // Not registered at the bound path: no peers for that project.
    open_graph(&mut h, "alpha");
    click(&mut h, "a2a-toggle");
    assert!(h.bounds("a2a-peer:echo").is_some());
    click(&mut h, "a2a-peer:echo");
    assert!(
        h.bounds("a2a-card-error").is_some(),
        "offline card kept as an error"
    );
    type_draft(&mut h, "x");
    click(&mut h, "a2a-review-button");
    assert!(h.bounds("a2a-review").is_none());
    assert!(
        h.bounds("a2a-hint").is_some(),
        "says why it cannot be reviewed"
    );

    let s = section(&mut h);
    h.cx.update(|_, cx| {
        s.update(cx, |s, _| {
            let _ = std::fs::remove_file(super::config::path_in(&s.config_dir));
        })
    });
    click(&mut h, "a2a-reload");
    assert!(h.bounds("a2a-no-peers").is_some());
    assert!(h.bounds("graph-node:c1").is_some());
}

#[test]
fn the_toggle_is_reached_and_operated_with_the_keyboard() {
    let (_cfg, mut h) = harness(
        "a2a-keyboard",
        &peers_file("http://127.0.0.1:9", "http://127.0.0.1:9"),
    );
    open_graph(&mut h, "alpha");
    let ws = h.ws.clone();
    let toggle = h.cx.update(|_, cx| {
        let p = ws.read(cx).project_graph_panel().unwrap();
        p.read(cx).a2a_toggle.clone()
    });
    assert!(
        !h.is_focused(&toggle),
        "the graph has focus, not the toggle"
    );
    // From the graph, Shift+Tab goes back to the header toggle.
    h.keys("shift-tab");
    assert!(h.is_focused(&toggle), "toggle focused: {}", h.focus_name());
    h.press("enter");
    assert!(
        h.bounds("a2a-section").is_some(),
        "Enter on the toggle shows it"
    );
    assert!(h.is_focused(&toggle), "focus stays on the toggle");
    h.press("space");
    assert!(h.bounds("a2a-section").is_none(), "Space hides it again");
}

/// The same flow against an external standard peer (the official a2a-python
/// SDK echo fixture): `POD_A2A_UI_FIXTURE=http://127.0.0.1:<port>`.
#[test]
fn official_sdk_fixture_flow_when_configured() {
    let Ok(base) = std::env::var("POD_A2A_UI_FIXTURE") else {
        println!("[a2a-ui] POD_A2A_UI_FIXTURE not set: official SDK fixture flow not run");
        return;
    };
    let (_cfg, mut h) = harness("a2a-official-sdk", &peers_file(&base, "http://127.0.0.1:9"));
    open_graph(&mut h, "alpha");
    click(&mut h, "a2a-toggle");
    click(&mut h, "a2a-peer:echo");
    assert!(h.bounds("a2a-card-error").is_none(), "card from the SDK");
    let send = |h: &mut Harness, text: &str| {
        type_draft(h, text);
        click(h, "a2a-review-button");
        click(h, "a2a-send");
        // returnImmediately: the SDK may answer while the task is still
        // submitted/working; the section's own GetTask refresh follows it.
        for _ in 0..10 {
            let active = with(h, |s, _| {
                s.project("alpha")
                    .unwrap()
                    .tasks
                    .first()
                    .is_some_and(|t| t.active())
            });
            if !active {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
            advance(h, 30);
        }
        let p = with(h, |s, _| s.project("alpha").unwrap().clone());
        println!(
            "[a2a-ui] sent {text:?}: submit={:?} tasks={:?} replies={}",
            std::mem::discriminant(&p.submit),
            p.tasks
                .iter()
                .map(|t| (
                    t.task_id.clone(),
                    t.context_id.clone(),
                    t.task.status.state.word().to_string()
                ))
                .collect::<Vec<_>>(),
            p.replies.len()
        );
        p
    };
    use pod_a2a::model::TaskState as S;
    let p = send(&mut h, "hello sdk");
    assert_eq!(p.tasks[0].task.status.state, S::Completed);
    assert!(!p.tasks[0].task_id.is_empty() && !p.tasks[0].context_id.is_empty());
    let p = send(&mut h, "input more");
    assert_eq!(p.tasks[0].task.status.state, S::InputRequired);
    let id = p.tasks[0].task_id.clone();
    click(&mut h, &format!("a2a-task-continue:{id}"));
    let p = send(&mut h, "the answer");
    assert_eq!(p.tasks[0].task_id, id);
    assert_eq!(p.tasks[0].task.status.state, S::Completed);
    let p = send(&mut h, "message hi");
    assert_eq!(p.replies.len(), 1);
    assert!(h.bounds("a2a-reply:0").is_some());
    let p = send(&mut h, "auth please");
    assert_eq!(p.tasks[0].task.status.state, S::AuthRequired);
    let p = send(&mut h, "files please");
    assert_eq!(p.tasks[0].task.status.state, S::Completed);
    let parts: Vec<_> = p.tasks[0]
        .task
        .artifacts
        .iter()
        .flatten()
        .flat_map(|a| a.parts.iter().map(super::model::part_view))
        .collect();
    println!("[a2a-ui] files artifact parts: {parts:?}");
    assert!(
        parts
            .iter()
            .any(|v| matches!(v, super::model::PartView::Url { .. }))
    );
    assert!(
        parts
            .iter()
            .any(|v| matches!(v, super::model::PartView::Raw { .. }))
    );
    // GetTask through the UI's refresh button.
    let (id, before) = (p.tasks[0].task_id.clone(), p.tasks[0].fetched);
    click(&mut h, &format!("a2a-task-refresh:{id}"));
    let p = with(&mut h, |s, _| s.project("alpha").unwrap().clone());
    assert_eq!(
        p.tasks[0].fetched,
        before + 1,
        "GetTask answered: {:?}",
        p.tasks[0].error
    );
    assert!(p.tasks[0].error.is_none());
    println!("[a2a-ui] official SDK fixture flow passed");
}
