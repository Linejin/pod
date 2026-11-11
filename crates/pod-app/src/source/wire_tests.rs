//! J (wire) golden of the app's podd boundary (card A-P2,
//! docs/20-refactor/app.md §3): what the source sends to podd and how it
//! reads podd's answers, checked against `qa/refactor/wire.json`.
//!
//! The expected values were captured once from the source before the split
//! (84bdf9e) and are never regenerated from the code under test: a difference
//! is a wire change to look at, not to bless. The daemon is an in-memory
//! JSON-RPC peer behind [`Client::from_transport`]; nothing is spawned or
//! contacted. `Value` equality normalizes key order only: array order, null
//! vs. omitted, enum tags, defaults and error codes are compared as they are.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use podd::client::Client;
use podd::protocol::codes;
use podd::transport::Transport;

use super::control::{is_method_not_found, is_transport};
use super::events::{Lifecycle, lifecycle_action, orch_event};
use super::input::send_input;
use super::*;
use crate::vm::{AgentState, Capabilities, Liveness};

const WIRE: &str = include_str!("../../qa/refactor/wire.json");

/// A pipe path nothing listens on: a dropped input client never reconnects.
fn absent_pipe() -> String {
    format!(r"\\.\pipe\pod-wire-golden-absent-{}", std::process::id())
}

/// One scripted daemon connection: request lines in, answer lines out.
#[derive(Default)]
struct Peer {
    /// method -> `{"result": …}`, `{"error": {"code", "message"}}` or `"close"`.
    answers: serde_json::Map<String, Value>,
    sent: Vec<Value>,
    line: Vec<u8>,
    out: VecDeque<u8>,
}

#[derive(Clone, Default)]
struct FakeDaemon(Arc<Mutex<Peer>>);

impl FakeDaemon {
    fn new(defaults: &Value, answers: &Value) -> Self {
        let d = Self::default();
        {
            let mut p = d.0.lock().unwrap();
            for src in [defaults, answers] {
                if let Some(m) = src.as_object() {
                    p.answers.extend(m.clone());
                }
            }
        }
        d
    }

    fn client(&self) -> Client {
        Client::from_transport(
            "wire-golden",
            Transport::new(self.clone(), self.clone(), 1 << 20),
        )
    }

    fn sent(&self) -> Value {
        Value::Array(self.0.lock().unwrap().sent.clone())
    }
}

impl Write for FakeDaemon {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut p = self.0.lock().unwrap();
        for &b in buf {
            if b != b'\n' {
                p.line.push(b);
                continue;
            }
            let line = std::mem::take(&mut p.line);
            let req: Value = serde_json::from_slice(&line).expect("request line");
            let method = req["method"].as_str().unwrap_or_default().to_string();
            p.sent
                .push(json!({"method": method, "params": req["params"].clone()}));
            let answer = p.answers.get(&method).cloned().unwrap_or_else(|| {
                json!({"error": {"code": codes::METHOD_NOT_FOUND, "message": format!("{method}: not scripted")}})
            });
            if answer == "close" {
                // Never answered: the reader sees the connection end.
                continue;
            }
            let mut resp = json!({"jsonrpc": "2.0", "id": req["id"].clone()});
            for k in ["result", "error"] {
                if let Some(v) = answer.get(k) {
                    resp[k] = v.clone();
                }
            }
            let text = resp.to_string();
            p.out.extend(text.bytes());
            p.out.push_back(b'\n');
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Read for FakeDaemon {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut p = self.0.lock().unwrap();
        let n = buf.len().min(p.out.len());
        for (slot, b) in buf.iter_mut().zip(p.out.drain(..n)) {
            *slot = b;
        }
        Ok(n)
    }
}

fn to<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).expect("serializable wire type")
}

fn text(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn num_u16(v: &Value) -> Option<u16> {
    v.as_u64().map(|n| n as u16)
}

fn err_json(e: &SourceError) -> Value {
    json!({"code": e.code, "message": e.message})
}

fn status_json(s: &StatusVm) -> Value {
    json!({
        "state": format!("{:?}", s.state),
        "final_state": s.final_state.map(|f| format!("{f:?}")),
        "since": s.since,
        // Display keeps the f32 as written (0.95, not 0.949999988…).
        "confidence": s.confidence.to_string(),
        "kind": s.kind,
        "source": s.source,
        "badge": s.badge,
        "running_children": s.running_children,
        "turn": s.turn,
        "agent": s.agent,
        "app": s.app,
    })
}

fn live_json(l: &Liveness) -> Value {
    match l {
        Liveness::Running => json!("running"),
        Liveness::Exited { code, killed } => json!({"exited": {"code": code, "killed": killed}}),
        Liveness::Lost => json!("lost"),
    }
}

fn terminal_json(t: &TerminalVm) -> Value {
    json!({
        "id": t.id,
        "name": t.name,
        "program": t.program,
        "argv": t.argv,
        "cwd": t.cwd,
        "live": live_json(&t.live),
        "attached": t.attached,
        "created_ms": t.created_ms,
        "task_id": t.task_id,
        "status": t.status.as_ref().map(status_json),
        "parent_terminal": t.parent_terminal,
        "parent_task": t.parent_task,
        "unreachable": t.unreachable,
    })
}

fn task_json(t: &TaskVm) -> Value {
    json!({
        "id": t.id,
        "title": t.title,
        "repo": t.repo,
        "branch": t.branch,
        "base": t.base,
        "agent": t.agent,
        "worktree": t.worktree,
        "worktree_exists": t.worktree_exists,
        "terminals": t.terminals,
        "git": t.git.as_ref().map(|g| json!({
            "ahead": g.ahead, "behind": g.behind, "dirty": g.dirty,
            "unpushed": g.unpushed, "last_commit": g.last_commit,
        })),
        "git_error": t.git_error,
        "review_open": t.review_open,
        "status": t.status.as_ref().map(status_json),
        "parent_task": t.parent_task,
        "parent_terminal": t.parent_terminal,
        "group": t.group,
        "prompt": t.prompt,
        "pr": t.pr.as_ref().map(to),
    })
}

fn repo_json(r: &RepoVm) -> Value {
    json!({"name": r.name, "path": r.path, "default_base": r.default_base})
}

fn event_json(e: &SourceEvent) -> Value {
    match e {
        SourceEvent::Conn(c) => json!({"conn": format!("{c:?}")}),
        SourceEvent::Caps(c) => json!({"caps": {
            "tasks": c.tasks, "status": c.status, "rename": c.rename,
            "review": c.review, "orch": c.orch,
        }}),
        SourceEvent::Terminals(v) => {
            json!({"terminals": v.iter().map(terminal_json).collect::<Vec<_>>()})
        }
        SourceEvent::Tasks(v) => json!({"tasks": v.iter().map(task_json).collect::<Vec<_>>()}),
        SourceEvent::Repos(v) => json!({"repos": v.iter().map(repo_json).collect::<Vec<_>>()}),
        SourceEvent::Status { id, status } => {
            json!({"status": {"id": id, "status": status.as_ref().map(status_json)}})
        }
        SourceEvent::Gates(g) => json!({"gates": to(g)}),
        SourceEvent::Gate(g) => json!({"gate": to(g)}),
        SourceEvent::GroupCheck(c) => json!({"group_check": to(c)}),
        SourceEvent::Delivery(d, s) => json!({"delivery": to(d), "submit": to(s)}),
        SourceEvent::TaskFilesChanged(id) => json!({"task_files_changed": id}),
        SourceEvent::TaskRemoving { id, worktree } => {
            json!({"task_removing": {"id": id, "worktree": worktree}})
        }
        SourceEvent::ModNotice(n) => json!({"mod_notice": to(n)}),
        SourceEvent::ModChanged(m) => json!({"mod_changed": to(m)}),
        SourceEvent::TaskCreated { id, terminal, hint } => {
            json!({"task_created": {"id": id, "terminal": terminal, "hint": to(hint)}})
        }
        SourceEvent::OrchChanged(c) => json!({"orch_changed": to(c)}),
        SourceEvent::OrchMail(m) => json!({"orch_mail": to(m)}),
        SourceEvent::OrchResync => json!("orch_resync"),
    }
}

fn drain(events: &mut mpsc::UnboundedReceiver<SourceEvent>) -> Value {
    let mut out = Vec::new();
    while let Ok(e) = events.try_recv() {
        out.push(event_json(&e));
    }
    Value::Array(out)
}

/// `podd` → view model, one decoder per `as`.
fn decode(case: &Value) -> Value {
    let wire = &case["wire"];
    match case["as"].as_str().expect("decode as") {
        "terminal" => TerminalVm::from_json(wire)
            .as_ref()
            .map(terminal_json)
            .into(),
        "task" => TaskVm::from_json(wire).as_ref().map(task_json).into(),
        "status" => StatusVm::from_json(wire).as_ref().map(status_json).into(),
        "agent_state" => wire
            .as_array()
            .expect("states")
            .iter()
            .map(|s| {
                json!(format!(
                    "{:?}",
                    AgentState::parse(s.as_str().unwrap_or_default())
                ))
            })
            .collect(),
        other => panic!("unknown decode kind {other}"),
    }
}

fn lifecycle_json(l: &Lifecycle) -> Value {
    match l {
        Lifecycle::None => json!("none"),
        Lifecycle::Refresh {
            terminals,
            tasks,
            files,
        } => json!({"refresh": {"terminals": terminals, "tasks": tasks, "files": files}}),
        Lifecycle::Removing(id, wt) => json!({"removing": [id, wt]}),
    }
}

/// One status-stream notification through each reader, in stream order.
fn notification(case: &Value) -> Value {
    let n = p::Notification {
        jsonrpc: "2.0".into(),
        method: case["method"].as_str().expect("method").into(),
        params: case["params"].clone(),
    };
    json!({
        "created": crate::auto_open::created_event(&n).as_ref().map(event_json),
        "orch": orch_event(&n).as_ref().map(event_json),
        "lifecycle": lifecycle_json(&lifecycle_action(&n)),
    })
}

/// A control-connection reply, read once the command was handled.
enum Pending {
    None,
    Terminal(Answer<TerminalVm>),
    Unit(Answer<()>),
    Repo(Answer<RepoVm>),
    Task(Answer<TaskVm>),
    Ids(Answer<Vec<String>>),
}

fn settle<T>(mut rx: Answer<T>, f: impl Fn(&T) -> Value) -> Value {
    match rx.try_recv() {
        Ok(Some(Ok(v))) => json!({"ok": f(&v)}),
        Ok(Some(Err(e))) => json!({"err": err_json(&e)}),
        Ok(None) => json!("pending"),
        Err(_) => json!("dropped"),
    }
}

impl Pending {
    fn settle(self) -> Value {
        match self {
            Pending::None => Value::Null,
            Pending::Terminal(rx) => settle(rx, terminal_json),
            Pending::Unit(rx) => settle(rx, |_| Value::Null),
            Pending::Repo(rx) => settle(rx, repo_json),
            Pending::Task(rx) => settle(rx, task_json),
            Pending::Ids(rx) => settle(rx, |ids| json!(ids)),
        }
    }
}

/// A control command from its wire.json form (`{"run": {…}}`, `{"ack": "t1"}` …).
fn command(v: &Value) -> Option<(Cmd, Pending)> {
    let (name, a) = v.as_object()?.iter().next()?;
    let s = |k: &str| text(&a[k]).unwrap_or_default();
    Some(match name.as_str() {
        "run" => {
            let (tx, rx) = oneshot::channel();
            let spec = RunSpec {
                argv: serde_json::from_value(a["argv"].clone()).unwrap_or_default(),
                cwd: text(&a["cwd"]),
                name: text(&a["name"]),
                cols: num_u16(&a["cols"]),
                rows: num_u16(&a["rows"]),
                task: text(&a["task"]),
            };
            (Cmd::Run(spec, tx), Pending::Terminal(rx))
        }
        "kill" => {
            let (tx, rx) = oneshot::channel();
            (Cmd::Kill(text(a)?, tx), Pending::Unit(rx))
        }
        "rm" => {
            let (tx, rx) = oneshot::channel();
            (Cmd::Rm(text(a)?, tx), Pending::Unit(rx))
        }
        "rename" => {
            let (tx, rx) = oneshot::channel();
            (Cmd::Rename(s("id"), s("name"), tx), Pending::Unit(rx))
        }
        "repo_add" => {
            let (tx, rx) = oneshot::channel();
            (
                Cmd::RepoAdd(s("path"), text(&a["name"]), tx),
                Pending::Repo(rx),
            )
        }
        "task_create" => {
            let (tx, rx) = oneshot::channel();
            let spec = TaskSpec {
                repo: s("repo"),
                name: s("name"),
                base: text(&a["base"]),
                agent: serde_json::from_value(a["agent"].clone()).expect("agent kind"),
                prompt: text(&a["prompt"]),
                cols: num_u16(&a["cols"]),
                rows: num_u16(&a["rows"]),
            };
            (Cmd::TaskCreate(spec, tx), Pending::Task(rx))
        }
        "task_remove" => {
            let (tx, rx) = oneshot::channel();
            let flag = |k: &str| a[k].as_bool().unwrap_or(false);
            (
                Cmd::TaskRemove(s("task"), flag("force"), flag("delete_branch"), tx),
                Pending::Ids(rx),
            )
        }
        "ack" => (Cmd::Ack(text(a)?), Pending::None),
        "resize" => (
            Cmd::Resize(s("id"), num_u16(&a["cols"])?, num_u16(&a["rows"])?),
            Pending::None,
        ),
        other => panic!("unknown command {other}"),
    })
}

/// The control connection against a scripted daemon: one command (or one
/// poll round), what it sent, its reply, the events and a lost connection.
fn control(case: &Value, defaults: &Value) -> Value {
    let daemon = FakeDaemon::new(defaults, &case["daemon"]);
    let (_cmds, rx) = std_mpsc::channel();
    let (etx, mut erx) = mpsc::unbounded();
    let mut c = Control::new(absent_pipe(), rx, etx);
    if case["connected"].as_bool() != Some(false) {
        c.client = Some(daemon.client());
    }
    c.caps = Capabilities {
        tasks: case["caps_tasks"].as_bool() != Some(false),
        status: true,
        rename: true,
        review: true,
        orch: true,
    };
    let (result, reply) = if case["cmd"] == "poll" {
        (c.poll(), Value::Null)
    } else {
        let (cmd, pending) = command(&case["cmd"]).expect("command");
        let r = c.handle(cmd);
        (r, pending.settle())
    };
    json!({
        "sent": daemon.sent(),
        "reply": reply,
        "lost": result.err().map(|e| e.to_string()),
        "events": drain(&mut erx),
    })
}

/// The input connection: `terminal.input` per send, in order.
fn input(case: &Value, defaults: &Value) -> Value {
    let daemon = FakeDaemon::new(defaults, &case["daemon"]);
    let mut client = Some(daemon.client());
    for s in case["sends"].as_array().expect("sends") {
        let bytes: Vec<u8> = match &s["bytes"] {
            Value::String(t) => t.as_bytes().to_vec(),
            b => serde_json::from_value(b.clone()).expect("byte array"),
        };
        send_input(
            &absent_pipe(),
            &mut client,
            s["id"].as_str().unwrap_or_default(),
            &bytes,
        );
    }
    json!({"sent": daemon.sent(), "client_kept": client.is_some()})
}

fn client_error(v: &Value) -> ClientError {
    let (kind, a) = v.as_object().and_then(|m| m.iter().next()).expect("error");
    match kind.as_str() {
        "rpc" => ClientError::Rpc(p::RpcError::new(
            a["code"].as_i64().expect("code"),
            a["message"].as_str().unwrap_or_default(),
        )),
        "protocol" => ClientError::Protocol(a.as_str().unwrap_or_default().into()),
        "io" => ClientError::Io(std::io::Error::other(a.as_str().unwrap_or_default())),
        "not_running" => ClientError::NotRunning,
        other => panic!("unknown client error {other}"),
    }
}

/// How a client error is classified and shown to a request's caller.
fn error(case: &Value) -> Value {
    let e = client_error(&case["error"]);
    json!({
        "transport": is_transport(&e),
        "method_not_found": is_method_not_found(&e),
        "source_error": err_json(&SourceError::of(&e)),
    })
}

/// A typed request ([`PoddSource::call`]) as a feature controller sends it,
/// answered on the request connection (scripted backend).
fn typed<M: Method>(source: &PoddSource, backend: &mut TestBackend, params: &Value) -> Value
where
    M::Result: 'static,
{
    let params: M::Params = serde_json::from_value(params.clone())
        .unwrap_or_else(|e| panic!("{} params: {e}", M::NAME));
    let answer = source.call::<M>(&params);
    backend.serve();
    match futures::executor::block_on(answer) {
        Ok(r) => json!({"ok": to(&r)}),
        Err(e) => json!({"err": err_json(&e)}),
    }
}

fn call(case: &Value) -> Value {
    let (source, _events, mut backend) = PoddSource::scripted();
    let sent = Rc::new(RefCell::new(Vec::new()));
    let log = sent.clone();
    let answer = case["answer"].clone();
    backend.rpc = Some(Box::new(move |method, params| {
        log.borrow_mut()
            .push(json!({"method": method, "params": params.clone()}));
        Some(match answer.get("error") {
            Some(e) => Err(SourceError {
                code: e["code"].as_i64(),
                message: e["message"].as_str().unwrap_or_default().into(),
            }),
            None => Ok(answer["result"].clone()),
        })
    }));
    let params = &case["params"];
    let reply = match case["method"].as_str().expect("method") {
        "task.touch" => {
            let paths = serde_json::from_value(params["paths"].clone()).expect("paths");
            source.touch_task(params["task"].as_str().unwrap_or_default(), paths);
            backend.serve();
            Value::Null
        }
        m if m == p::OrchRunList::NAME => typed::<p::OrchRunList>(&source, &mut backend, params),
        m if m == p::OrchMailList::NAME => typed::<p::OrchMailList>(&source, &mut backend, params),
        m if m == p::OrchConfigGet::NAME => {
            typed::<p::OrchConfigGet>(&source, &mut backend, params)
        }
        m if m == p::OrchConfigSet::NAME => {
            typed::<p::OrchConfigSet>(&source, &mut backend, params)
        }
        m if m == p::ModListMethod::NAME => {
            typed::<p::ModListMethod>(&source, &mut backend, params)
        }
        m if m == p::SessionsListMethod::NAME => {
            typed::<p::SessionsListMethod>(&source, &mut backend, params)
        }
        m if m == p::GateList::NAME => typed::<p::GateList>(&source, &mut backend, params),
        m if m == p::GateResolve::NAME => typed::<p::GateResolve>(&source, &mut backend, params),
        m if m == p::ReviewDeliveries::NAME => {
            typed::<p::ReviewDeliveries>(&source, &mut backend, params)
        }
        m if m == p::TaskDiff::NAME => typed::<p::TaskDiff>(&source, &mut backend, params),
        other => panic!("no typed call for {other}"),
    };
    let sent = sent.borrow().clone();
    json!({"sent": sent, "reply": reply})
}

const SECTIONS: [&str; 6] = [
    "decode",
    "notifications",
    "control",
    "input",
    "errors",
    "calls",
];

fn run_case(section: &str, case: &Value, defaults: &Value) -> Value {
    match section {
        "decode" => decode(case),
        "notifications" => notification(case),
        "control" => control(case, defaults),
        "input" => input(case, defaults),
        "errors" => error(case),
        "calls" => call(case),
        other => panic!("unknown section {other}"),
    }
}

/// fx-app regression: a typed result that does not decode fails that request
/// only. Its caller gets the message, the control and input connections stay
/// and the next request goes over the same connection.
#[test]
fn a_bad_result_fails_the_request_and_keeps_the_connection() {
    let daemon = FakeDaemon::new(
        &json!({}),
        &json!({
            "terminal.kill": {"result": {}},
            "terminal.resize": {"result": {}},
            "repo.list": {"result": {"repos": [{"name": "demo"}]}},
            "task.list": {"result": {"tasks": []}},
            "terminal.list": {"result": {"terminals": []}},
            "terminal.input": {"result": {}},
        }),
    );
    let (_cmds, rx) = std_mpsc::channel();
    let (etx, _erx) = mpsc::unbounded();
    let mut c = Control::new(absent_pipe(), rx, etx);
    c.client = Some(daemon.client());
    c.caps.tasks = true;

    // A kill polls after its reply: a bad repo.list there does not end the round.
    let (cmd, pending) = command(&json!({"kill": "t1"})).expect("kill");
    assert!(c.handle(cmd).is_ok(), "the connection is not lost");
    assert_eq!(
        pending.settle(),
        json!({"err": {"code": null, "message": "bad terminal.kill result: missing field `id`"}})
    );
    let (cmd, _) = command(&json!({"resize": {"id": "t1", "cols": 80, "rows": 24}})).unwrap();
    assert!(c.handle(cmd).is_ok());
    assert!(c.client.is_some());
    let methods: Vec<Value> = daemon
        .sent()
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["method"].clone())
        .collect();
    assert_eq!(
        methods,
        [
            "terminal.kill",
            "terminal.list",
            "repo.list",
            "task.list",
            "terminal.resize",
        ],
    );

    let mut input = Some(daemon.client());
    send_input(&absent_pipe(), &mut input, "t1", b"x");
    assert!(input.is_some(), "the input connection stays");
}

#[test]
fn wire_golden_matches_the_pre_split_source() {
    let doc: Value = serde_json::from_str(WIRE).expect("wire.json");
    assert_eq!(
        doc["protocol_version"],
        json!(p::PROTOCOL_VERSION),
        "the golden is for this protocol version"
    );
    let defaults = &doc["daemon_defaults"];
    let mut failures = Vec::new();
    let mut seen = 0;
    for section in SECTIONS {
        let cases = doc[section].as_array().expect(section);
        assert!(!cases.is_empty(), "{section}: no cases");
        for case in cases {
            seen += 1;
            let name = case["name"].as_str().unwrap_or("?");
            let actual = run_case(section, case, defaults);
            // `null` is an expectation too (a record the decoder drops).
            match case.get("expect") {
                Some(expected) if *expected == actual => {}
                expected => failures.push(format!(
                    "{section}/{name}:\n  expected {}\n  actual   {actual}",
                    expected.map_or("(missing)".into(), Value::to_string)
                )),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {seen} wire cases differ from the golden:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
