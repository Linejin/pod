//! The data source: the only module that talks to podd.
//!
//! Two kinds of connections, both on plain threads with podd's blocking client:
//! - one **control** connection ([`PoddSource`]): polls `terminal.list` (and
//!   `task.list` when available), runs commands (run / kill / rm / rename /
//!   input / resize), and owns reconnection — including restarting podd the
//!   same way the `pod` CLI does (`Client::connect_or_start`).
//! - one **attach** connection per terminal panel ([`crate::attach`]).
//!
//! Everything the UI sees comes out as [`SourceEvent`]s carrying view models
//! ([`crate::vm`]). Daemon features that are still being built (tasks, live
//! status, rename) are detected per connection ([`Capabilities`]); when the
//! daemon answers "method not found" the UI simply hides that part.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use futures::channel::{mpsc, oneshot};
use podd::client::{Client, ClientError};
use podd::protocol::{self as p, Method, codes};
use serde_json::{Value, json};

use crate::vm::{Capabilities, ConnState, RepoVm, StatusVm, TaskVm, TerminalVm};

/// How often the terminal list is refreshed while connected (no lifecycle events).
const POLL: Duration = Duration::from_millis(500);
/// How often tasks and repos are refreshed (each task costs several git runs, docs/08 §7).
const TASK_POLL: Duration = Duration::from_millis(3000);
/// With v9 lifecycle events the lists are re-read on events; these slow
/// polls stay as a safety net (docs/03 §4.3).
const LIFECYCLE_TERMINAL_POLL: Duration = Duration::from_secs(15);
const LIFECYCLE_TASK_POLL: Duration = Duration::from_secs(45);

/// The daemon speaks protocol v9 (lifecycle events), set by the control connection.
static LIFECYCLE_CAPABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The status stream is subscribed with `lifecycle: true` right now.
static LIFECYCLE_LIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Lifecycle events replace fast list polling.
pub fn lifecycle_active() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    LIFECYCLE_CAPABLE.load(Relaxed) && LIFECYCLE_LIVE.load(Relaxed)
}
/// Delay between reconnect attempts.
const RETRY: Duration = Duration::from_millis(1000);

#[derive(Clone, Debug, PartialEq)]
pub enum SourceEvent {
    Conn(ConnState),
    Caps(Capabilities),
    /// Full terminal list (sent when it changed).
    Terminals(Vec<TerminalVm>),
    /// Full task list (only when `caps.tasks`).
    Tasks(Vec<TaskVm>),
    /// Registered repositories (only when `caps.tasks`).
    Repos(Vec<RepoVm>),
    /// Live status change from `status.subscribe` (`None` = terminal removed).
    Status {
        id: String,
        status: Option<StatusVm>,
    },
    /// Pending gates in a (re)subscribe snapshot: replaces the known pending set.
    Gates(Vec<p::GateInfo>),
    /// `gate.changed`: a gate was created or resolved (full record; boxed,
    /// it is the largest payload).
    Gate(Box<p::GateInfo>),
    /// `group.check`: background check job progress (v6).
    GroupCheck(Box<p::GroupCheckEvent>),
    /// `review.delivery`: a review delivery was recorded or changed state.
    /// v8 adds the submit check (a second event once it is final).
    Delivery(Box<p::DeliveryInfo>, Option<p::SubmitCheck>),
    /// v9 `task.changed` `touched`/`git`: the task's files changed (Review re-reads its diff).
    TaskFilesChanged(String),
    /// v9 `task.removing`: the daemon is about to delete this worktree.
    TaskRemoving {
        id: String,
        worktree: String,
    },
    /// v11 `mod.notify`: an extension's notification (docs/16).
    ModNotice(Box<p::Notice>),
    /// v11 `mod.changed`: an extension was enabled, disabled, stopped or reloaded.
    ModChanged(Box<p::ModInfo>),
    /// `task.changed created` with an open hint (docs/19, `auto_open.rs`).
    TaskCreated {
        id: String,
        terminal: Option<String>,
        hint: p::OpenHint,
    },
    /// v16 `orch.changed`: a run, item or attempt changed (`orch_ui`).
    OrchChanged(Box<p::OrchChangedEvent>),
    /// v16 `orch.mail`: a message was recorded (subject only).
    OrchMail(Box<p::OrchMailEvent>),
    /// The status stream (re)subscribed: orchestration events may have been
    /// missed, so the board re-reads its runs.
    OrchResync,
}

/// Set once the connected daemon has sent a `review.delivery` event (it then
/// announces every delivery change, so panels need not poll); cleared on
/// every resubscribe.
pub static DELIVERY_EVENTS: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// True when podd announces delivery changes on the status stream.
pub fn delivery_events() -> bool {
    DELIVERY_EVENTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// A failed request: the JSON-RPC error code when the daemon answered one.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceError {
    pub code: Option<i64>,
    pub message: String,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl SourceError {
    fn msg(m: impl Into<String>) -> Self {
        Self {
            code: None,
            message: m.into(),
        }
    }
    fn of(e: &ClientError) -> Self {
        match e {
            ClientError::Rpc(r) => Self {
                code: Some(r.code),
                message: r.message.clone(),
            },
            other => Self::msg(other.to_string()),
        }
    }
}

/// `task.create` input from the New Task dialog.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskSpec {
    pub repo: String,
    pub name: String,
    pub base: Option<String>,
    pub agent: p::AgentKind,
    pub prompt: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
}

/// What to start with `terminal.run`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunSpec {
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    pub name: Option<String>,
    pub cols: Option<u16>,
    pub rows: Option<u16>,
    /// Add the terminal to this task (cwd defaults to its worktree).
    pub task: Option<String>,
}

type Reply<T> = oneshot::Sender<Result<T, SourceError>>;
pub type Answer<T> = oneshot::Receiver<Result<T, SourceError>>;

enum Cmd {
    Run(RunSpec, Reply<TerminalVm>),
    Kill(String, Reply<()>),
    Rm(String, Reply<()>),
    Rename(String, String, Reply<()>),
    RepoAdd(String, Option<String>, Reply<RepoVm>),
    TaskCreate(TaskSpec, Reply<TaskVm>),
    /// task, force, delete_branch -> removed terminal ids
    TaskRemove(String, bool, bool, Reply<Vec<String>>),
    Ack(String),
    Resize(String, u16, u16),
    /// Poll tasks and repos on the next round (e.g. after `group.create`).
    RefreshTasks,
    /// Re-read the terminal list now (a lifecycle event or `status.lagged`).
    RefreshTerminals,
}

/// A request for the request connection: method, params, reply.
type RpcCmd = (&'static str, Value, Reply<Value>);

/// Bytes for a terminal's input, on the input connection ([`input_loop`]).
enum InputCmd {
    /// What the user typed or pasted.
    Keys(String, Vec<u8>),
    /// The view's answer to a query in the output (OSC 10/11/4, CSI 14 t),
    /// stamped when the view produced it.
    Reply(String, Vec<u8>, Instant),
}

/// Cheap to clone; all clones share the control thread (and the request and input threads).
#[derive(Clone)]
pub struct PoddSource {
    pipe: String,
    tx: std_mpsc::Sender<Cmd>,
    rpc: std_mpsc::Sender<RpcCmd>,
    input: std_mpsc::Sender<InputCmd>,
}

impl PoddSource {
    /// Read-only, isolated UI volume fixture: never connects to or starts podd.
    pub fn fixture() -> (Self, mpsc::UnboundedReceiver<SourceEvent>) {
        let (tx, _) = std_mpsc::channel();
        // Editor QA scenes give task-000 a real (fixture) worktree; only then
        // does the fixture answer `task.diff` itself (from that worktree's
        // git). Everything else stays unanswered, as before.
        let fixture_worktree = crate::fixture_scenes::worktree();
        let (rpc, rpc_rx) = std_mpsc::channel::<RpcCmd>();
        let wt = fixture_worktree.clone();
        std::thread::Builder::new()
            .name("fixture-rpc".into())
            .spawn(move || {
                while let Ok((method, params, reply)) = rpc_rx.recv() {
                    // PR scenes (docs/15): `task.pr.*` from the fake forge.
                    if let Some(a) = crate::orch_ui::fixture::answer(method, &params) {
                        let _ = reply.send(a);
                    } else if let Some(a) = crate::pr_panel::fixture_answer(method, &params)
                        .or_else(|| crate::layouts::kanban::fixture_answer(method, &params))
                    {
                        let _ = reply.send(a);
                    } else if let ("task.diff", Some(wt)) = (method, wt.as_ref()) {
                        let _ = reply.send(fixture_task_diff(std::path::Path::new(wt)));
                    } else if method == "review.comments.list" {
                        let _ = reply.send(Ok(json!({"comments": []})));
                    } else if method == "review.deliveries" {
                        let _ = reply.send(Ok(json!({"deliveries": []})));
                    }
                }
            })
            .expect("spawn fixture rpc thread");
        let (input, _) = std_mpsc::channel();
        let (etx, erx) = mpsc::unbounded();
        let mut tasks = Vec::new();
        let mut terminals = Vec::new();
        for n in 0..100 {
            let ids: Vec<String> = (0..3).map(|k| format!("fixture-{}", n * 3 + k)).collect();
            let mut task=TaskVm::from_json(&json!({"id":format!("sample/task-{n:03}"),"name":format!("Task {n:03} - 한글 작업과 레이아웃 🔎"),"agent":"powershell.exe -NoProfile -File C:/fixtures/sample/agent.ps1","repo":"sample","terminals":ids,"branch":format!("pod/task-{n:03}")})).expect("fixture task");
            if n == 0 {
                task.worktree = fixture_worktree.clone();
            }
            for (k, id) in ids.iter().enumerate() {
                let state = ["working", "blocked", "done", "idle", "unknown", "error"][(n + k) % 6];
                terminals.push(TerminalVm {
                    id: id.clone(),
                    name: Some(format!("Worker {}", n * 3 + k)),
                    program: "powershell".into(),
                    argv: vec!["powershell.exe".into(), "-NoProfile".into(), "-File".into(), "C:/fixtures/sample/agent.ps1".into()],
                    cwd: "C:/fixtures/sample".into(),
                    live: crate::vm::Liveness::Running,
                    attached: 0,
                    created_ms: 0,
                    task_id: Some(task.id.clone()),
                    // A mix of agents (docs/10 §D5.11): Claude, Codex, then a plain shell per task.
                    status: StatusVm::from_json(&json!({"state":state,"kind":"permission","source":if state == "unknown" {"heuristic"} else {"osc"},"confidence":if state == "unknown" {0.4} else {1.0},"since":0,
                        "agent":([Some("claude"),Some("codex"),None][k % 3])})),
                    parent_terminal: None,
                    parent_task: None,
                    unreachable: None,
                });
            }
            tasks.push(task);
        }
        let _ = etx.unbounded_send(SourceEvent::Conn(ConnState::Connected {
            pid: 0,
            version: "UI fixture (read only)".into(),
        }));
        let _ = etx.unbounded_send(SourceEvent::Caps(Capabilities {
            tasks: true,
            status: true,
            review: true,
            orch: true,
            ..Default::default()
        }));
        let _ = etx.unbounded_send(SourceEvent::Repos(vec![RepoVm {
            name: "sample".into(),
            path: "C:/fixtures/sample".into(),
            default_base: Some("main".into()),
        }]));
        let _ = etx.unbounded_send(SourceEvent::Tasks(tasks));
        let _ = etx.unbounded_send(SourceEvent::Terminals(terminals));
        if let Some(g) = crate::orch_ui::fixture::gates() {
            let _ = etx.unbounded_send(SourceEvent::Gates(g));
        }
        let _ = etx.unbounded_send(SourceEvent::OrchResync);
        (
            Self {
                pipe: "ui-fixture-no-daemon".into(),
                tx,
                rpc,
                input,
            },
            erx,
        )
    }

    /// Start the control thread. Events arrive on the returned receiver.
    pub fn start(pipe: String) -> (Self, mpsc::UnboundedReceiver<SourceEvent>) {
        let (tx, rx) = std_mpsc::channel();
        let (etx, erx) = mpsc::unbounded();
        let thread_pipe = pipe.clone();
        let status_events = etx.clone();
        let status_cmds = tx.clone();
        std::thread::Builder::new()
            .name("podd-control".into())
            .spawn(move || Control::new(thread_pipe, rx, etx).run())
            .expect("spawn control thread");
        let status_pipe = pipe.clone();
        std::thread::Builder::new()
            .name("podd-status".into())
            .spawn(move || status_stream(status_pipe, status_events, status_cmds))
            .expect("spawn status thread");
        let (rpc, rpc_rx) = std_mpsc::channel();
        let rpc_pipe = pipe.clone();
        std::thread::Builder::new()
            .name("podd-request".into())
            .spawn(move || request_loop(rpc_pipe, rpc_rx))
            .expect("spawn request thread");
        let (input, input_rx) = std_mpsc::channel();
        let input_pipe = pipe.clone();
        std::thread::Builder::new()
            .name("podd-input".into())
            .spawn(move || input_loop(input_pipe, input_rx))
            .expect("spawn input thread");
        (
            Self {
                pipe,
                tx,
                rpc,
                input,
            },
            erx,
        )
    }

    /// Native-QA tests: a source served by [`TestBackend`] on the test thread
    /// (no podd, no pipe; terminal panels attach to a pipe that never exists).
    #[cfg(test)]
    pub(crate) fn scripted() -> (Self, mpsc::UnboundedReceiver<SourceEvent>, TestBackend) {
        let (tx, cmds) = std_mpsc::channel();
        let (rpc, rpcs) = std_mpsc::channel();
        let (input, inputs) = std_mpsc::channel();
        let (events, erx) = mpsc::unbounded();
        let pipe = format!(r"\\.\pipe\pod-native-qa-absent-{}", std::process::id());
        let backend = TestBackend {
            cmds,
            rpcs,
            inputs,
            events,
            typed: Vec::new(),
            acks: Vec::new(),
            calls: Vec::new(),
            repos: Vec::new(),
            tasks: Vec::new(),
            terminals: Vec::new(),
            worktrees: HashMap::new(),
            rpc: None,
            next: 1,
        };
        (
            Self {
                pipe,
                tx,
                rpc,
                input,
            },
            erx,
            backend,
        )
    }

    pub fn pipe(&self) -> &str {
        &self.pipe
    }

    fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Cmd) -> Answer<T> {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(make(tx));
        rx
    }

    pub fn run(&self, spec: RunSpec) -> Answer<TerminalVm> {
        self.ask(|r| Cmd::Run(spec, r))
    }

    pub fn kill(&self, id: &str) -> Answer<()> {
        self.ask(|r| Cmd::Kill(id.into(), r))
    }

    /// Remove an exited terminal (ends its holder).
    pub fn remove(&self, id: &str) -> Answer<()> {
        self.ask(|r| Cmd::Rm(id.into(), r))
    }

    /// Daemon-side rename; only meaningful when `caps.rename`.
    pub fn rename(&self, id: &str, name: &str) -> Answer<()> {
        self.ask(|r| Cmd::Rename(id.into(), name.into(), r))
    }

    pub fn add_repo(&self, path: &str, name: Option<String>) -> Answer<RepoVm> {
        self.ask(|r| Cmd::RepoAdd(path.into(), name, r))
    }

    pub fn create_task(&self, spec: TaskSpec) -> Answer<TaskVm> {
        self.ask(|r| Cmd::TaskCreate(spec, r))
    }

    /// Remove a task (kills its terminals, removes the worktree). Without
    /// `force` podd refuses dirty / unpushed work with `TASK_DIRTY`.
    pub fn remove_task(&self, task: &str, force: bool, delete_branch: bool) -> Answer<Vec<String>> {
        self.ask(|r| Cmd::TaskRemove(task.into(), force, delete_branch, r))
    }

    /// Raw input bytes (fire and forget; dropped while disconnected). Sent on
    /// the input connection, never queued behind polls or task requests.
    pub fn input(&self, id: &str, bytes: Vec<u8>) {
        let _ = self.input.send(InputCmd::Keys(id.into(), bytes));
    }

    /// A query reply produced by a terminal view (see [`ReplyFilter`]).
    pub fn reply(&self, id: &str, bytes: Vec<u8>) {
        let _ = self
            .input
            .send(InputCmd::Reply(id.into(), bytes, Instant::now()));
    }

    /// `terminal.ack`: the user looked at the terminal (`done` -> `idle`).
    pub fn ack(&self, id: &str) {
        let _ = self.tx.send(Cmd::Ack(id.into()));
    }

    /// Re-read tasks and repos right away (they are otherwise polled every 3 s).
    /// v9 `task.touch`: files of `task` were saved (fresh git facts, Review refresh).
    /// Fire and forget; an older daemon just answers "unknown method".
    pub fn touch_task(&self, task: &str, paths: Vec<String>) {
        // The answer is not needed: dropping the receiver leaves the request running.
        drop(self.request(p::TaskTouch::NAME, json!({"task": task, "paths": paths})));
    }

    pub fn refresh_tasks(&self) {
        let _ = self.tx.send(Cmd::RefreshTasks);
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        let _ = self.tx.send(Cmd::Resize(id.into(), cols, rows));
    }

    /// One request on the **request connection** (its own pipe connection and
    /// thread), used by the review panel: `task.diff` and `review.*`. Kept off
    /// the control connection so a slow `task.list` round (git per task) does
    /// not delay comments and sends, and the reverse (docs/09 §5-4).
    pub fn request(&self, method: &'static str, params: Value) -> Answer<Value> {
        let (tx, rx) = oneshot::channel();
        let _ = self.rpc.send((method, params, tx));
        rx
    }

    /// Typed [`request`](Self::request) for a podd [`Method`].
    pub fn call<M: Method>(
        &self,
        params: &M::Params,
    ) -> impl std::future::Future<Output = Result<M::Result, SourceError>> + use<M>
    where
        M::Result: 'static,
    {
        let rx = self.request(M::NAME, serde_json::to_value(params).unwrap_or(Value::Null));
        async move {
            let v = rx
                .await
                .map_err(|_| SourceError::msg("podd client stopped"))??;
            serde_json::from_value::<M::Result>(v)
                .map_err(|e| SourceError::msg(format!("bad {} result: {e}", M::NAME)))
        }
    }
}

/// `task.diff` for the editor QA fixture: the worktree against HEAD (tracked
/// changes + untracked files), shaped like podd's answer.
fn fixture_task_diff(wt: &std::path::Path) -> Result<Value, SourceError> {
    let git = |args: &[&str]| -> Result<String, SourceError> {
        let mut c = std::process::Command::new("git");
        c.arg("-C").arg(wt).args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        let out = c.output().map_err(|e| SourceError::msg(e.to_string()))?;
        if !out.status.success() {
            return Err(SourceError::msg(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let head = git(&["rev-parse", "HEAD"])?.trim().to_string();
    let mut counts: HashMap<String, (u64, u64)> = HashMap::new();
    for line in git(&["diff", "--numstat", "HEAD"])?.lines() {
        let mut it = line.splitn(3, '\t');
        if let (Some(a), Some(d), Some(path)) = (it.next(), it.next(), it.next()) {
            counts.insert(
                path.to_string(),
                (a.parse().unwrap_or(0), d.parse().unwrap_or(0)),
            );
        }
    }
    let mut files = Vec::new();
    let (mut added, mut deleted) = (0, 0);
    for entry in git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?
        .split('\0')
        .filter(|e| e.len() > 3)
    {
        let (xy, path) = entry.split_at(3);
        let status = match xy.trim() {
            "??" => "?",
            x if x.contains('D') => "D",
            x if x.contains('A') => "A",
            _ => "M",
        };
        let (a, d) = if status == "?" {
            (
                std::fs::read_to_string(wt.join(path))
                    .map(|t| t.lines().count() as u64)
                    .unwrap_or(0),
                0,
            )
        } else {
            counts.get(path).copied().unwrap_or((0, 0))
        };
        added += a;
        deleted += d;
        files.push(json!({"path": path, "status": status, "added": a, "deleted": d}));
    }
    Ok(
        json!({"task": "sample/task-000", "base": "HEAD", "merge_base": head, "files": files, "added": added, "deleted": deleted}),
    )
}

/// `true` when the error means the connection is gone (vs. an RPC error).
fn is_transport(e: &ClientError) -> bool {
    // A daemon refusing requests because it is stopping is as good as gone:
    // drop the connection and reconnect (to the next daemon) instead of
    // showing the refusal as an ordinary error on a half-dead connection.
    matches!(
        e,
        ClientError::Io(_) | ClientError::Protocol(_) | ClientError::NotRunning
    ) || e.is_stopping()
}

fn is_method_not_found(e: &ClientError) -> bool {
    matches!(e, ClientError::Rpc(r) if r.code == codes::METHOD_NOT_FOUND)
}

/// Does the daemon know `method`? Probed with empty params: anything other
/// than "method not found" (typically "invalid params") means yes.
fn probe(client: &mut Client, method: &str) -> Result<bool, ClientError> {
    match client.call_raw(method, json!({})) {
        Ok(_) => Ok(true),
        Err(e) if is_method_not_found(&e) => Ok(false),
        Err(e) if is_transport(&e) => Err(e),
        Err(_) => Ok(true),
    }
}

struct Control {
    pipe: String,
    rx: std_mpsc::Receiver<Cmd>,
    events: mpsc::UnboundedSender<SourceEvent>,
    client: Option<Client>,
    caps: Capabilities,
    last_terminals: Option<Vec<TerminalVm>>,
    last_tasks: Option<Vec<TaskVm>>,
    last_repos: Option<Vec<RepoVm>>,
    /// When tasks were last polled (`None` = poll on the next round).
    task_polled: Option<Instant>,
    /// When terminals were last listed (`None` = list on the next round).
    terminals_polled: Option<Instant>,
}

impl Control {
    fn new(
        pipe: String,
        rx: std_mpsc::Receiver<Cmd>,
        events: mpsc::UnboundedSender<SourceEvent>,
    ) -> Self {
        Self {
            pipe,
            rx,
            events,
            client: None,
            caps: Capabilities::default(),
            last_terminals: None,
            last_tasks: None,
            last_repos: None,
            task_polled: None,
            terminals_polled: None,
        }
    }

    fn emit(&self, e: SourceEvent) -> bool {
        self.events.unbounded_send(e).is_ok()
    }

    fn run(mut self) {
        self.emit(SourceEvent::Conn(ConnState::Connecting));
        let mut attempt = 0u32;
        loop {
            if self.client.is_none() {
                match self.connect() {
                    Ok(()) => attempt = 0,
                    Err(e) => {
                        attempt += 1;
                        if !self.emit(SourceEvent::Conn(ConnState::Reconnecting {
                            attempt,
                            error: e.to_string(),
                        })) {
                            return;
                        }
                        if !self.serve_until(Instant::now() + RETRY) {
                            return;
                        }
                        continue;
                    }
                }
            }
            if let Err(e) = self.poll() {
                self.lost(e);
                continue;
            }
            // With lifecycle events this only wakes for commands and the safety-net polls.
            if !self.serve_until(Instant::now() + POLL) {
                return;
            }
        }
    }

    fn connect(&mut self) -> Result<(), ClientError> {
        // Same as the `pod` CLI: start podd (detached) when its pipe is absent.
        let mut c = Client::connect_or_start(&self.pipe, None)?;
        let status = c.call::<p::ServerStatus>(&p::Empty {})?;
        let caps = Capabilities {
            tasks: probe(&mut c, "task.list")?,
            rename: probe(&mut c, "terminal.rename")?,
            // Protocol v4 = live agent status (docs/07 §8). An older daemon
            // simply has no status UI.
            status: status.protocol >= 4,
            // Protocol v5 = review comments and delivery (docs/11).
            review: status.protocol >= 5,
            // Protocol v6 = async group.check + gate expiry (P5 UI).
            orch: status.protocol >= 6,
        };
        self.client = Some(c);
        self.caps = caps;
        self.last_terminals = None;
        self.last_tasks = None;
        self.last_repos = None;
        self.task_polled = None;
        self.terminals_polled = None;
        LIFECYCLE_CAPABLE.store(status.protocol >= 9, std::sync::atomic::Ordering::Relaxed);
        self.emit(SourceEvent::Caps(caps));
        self.emit(SourceEvent::Conn(ConnState::Connected {
            pid: status.pid,
            version: status.version,
        }));
        Ok(())
    }

    fn lost(&mut self, e: ClientError) {
        self.client = None;
        self.emit(SourceEvent::Conn(ConnState::Reconnecting {
            attempt: 1,
            error: e.to_string(),
        }));
    }

    fn poll(&mut self) -> Result<(), ClientError> {
        let Some(c) = self.client.as_mut() else {
            return Ok(());
        };
        let lifecycle = lifecycle_active();
        let (term_every, task_every) = if lifecycle {
            (LIFECYCLE_TERMINAL_POLL, LIFECYCLE_TASK_POLL)
        } else {
            (POLL, TASK_POLL)
        };
        if self
            .terminals_polled
            .is_none_or(|t| t.elapsed() >= term_every.saturating_sub(Duration::from_millis(50)))
        {
            self.terminals_polled = Some(Instant::now());
            let list = c.call_raw(p::TerminalList::NAME, json!({}))?;
            let terminals: Vec<TerminalVm> = list
                .get("terminals")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(TerminalVm::from_json).collect())
                .unwrap_or_default();
            if self.last_terminals.as_ref() != Some(&terminals) {
                self.emit(SourceEvent::Terminals(terminals.clone()));
                self.last_terminals = Some(terminals);
            }
        }
        if self.caps.tasks && self.task_polled.is_none_or(|t| t.elapsed() >= task_every) {
            self.task_polled = Some(Instant::now());
            let c = self.client.as_mut().expect("connected");
            match c.call::<p::RepoList>(&p::Empty {}) {
                Ok(r) => {
                    let repos: Vec<RepoVm> = r
                        .repos
                        .into_iter()
                        .map(|r| RepoVm {
                            name: r.name,
                            path: r.path,
                            default_base: r.default_base,
                        })
                        .collect();
                    if self.last_repos.as_ref() != Some(&repos) {
                        self.emit(SourceEvent::Repos(repos.clone()));
                        self.last_repos = Some(repos);
                    }
                }
                Err(e) if is_transport(&e) => return Err(e),
                Err(_) => {}
            }
            let c = self.client.as_mut().expect("connected");
            match c.call_raw("task.list", json!({})) {
                Ok(v) => {
                    let tasks: Vec<TaskVm> = v
                        .get("tasks")
                        .or(Some(&v))
                        .and_then(Value::as_array)
                        .map(|a| a.iter().filter_map(TaskVm::from_json).collect())
                        .unwrap_or_default();
                    if self.last_tasks.as_ref() != Some(&tasks) {
                        self.emit(SourceEvent::Tasks(tasks.clone()));
                        self.last_tasks = Some(tasks);
                    }
                }
                Err(e) if is_transport(&e) => return Err(e),
                Err(_) => {}
            }
        }
        Ok(())
    }

    /// Run commands until `deadline`. Returns false when the app is gone.
    fn serve_until(&mut self, deadline: Instant) -> bool {
        loop {
            let wait = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(wait) {
                Ok(Cmd::RefreshTasks) => {
                    self.task_polled = None;
                    return true;
                }
                Ok(Cmd::RefreshTerminals) => {
                    self.terminals_polled = None;
                    return true;
                }
                Ok(cmd) => {
                    if let Err(e) = self.handle(cmd) {
                        self.lost(e);
                        return true;
                    }
                }
                Err(std_mpsc::RecvTimeoutError::Timeout) => return true,
                Err(std_mpsc::RecvTimeoutError::Disconnected) => return false,
            }
        }
    }

    /// Run one command. Err only for transport failures.
    fn handle(&mut self, cmd: Cmd) -> Result<(), ClientError> {
        fn reply<T>(r: Reply<T>, v: Result<T, ClientError>) -> Result<(), ClientError> {
            match v {
                Ok(x) => {
                    let _ = r.send(Ok(x));
                    Ok(())
                }
                Err(e) if is_transport(&e) => {
                    let _ = r.send(Err(SourceError::msg(format!("podd connection lost: {e}"))));
                    Err(e)
                }
                Err(e) => {
                    let _ = r.send(Err(SourceError::of(&e)));
                    Ok(())
                }
            }
        }
        let Some(c) = self.client.as_mut() else {
            // Disconnected: fail requests right away, drop input.
            let e = || SourceError::msg("not connected to podd");
            match cmd {
                Cmd::Run(_, r) => drop(r.send(Err(e()))),
                Cmd::Kill(_, r) | Cmd::Rm(_, r) | Cmd::Rename(_, _, r) => drop(r.send(Err(e()))),
                Cmd::RepoAdd(_, _, r) => drop(r.send(Err(e()))),
                Cmd::TaskCreate(_, r) => drop(r.send(Err(e()))),
                Cmd::TaskRemove(_, _, _, r) => drop(r.send(Err(e()))),
                Cmd::Ack(..) | Cmd::Resize(..) | Cmd::RefreshTasks | Cmd::RefreshTerminals => {}
            }
            return Ok(());
        };
        let target = |id: &str| p::TerminalRef::id(id);
        match cmd {
            Cmd::Run(s, r) => {
                let params = p::RunParams {
                    argv: s.argv,
                    cwd: s.cwd,
                    name: s.name,
                    cols: s.cols,
                    rows: s.rows,
                    task: s.task,
                    ..Default::default()
                };
                let v = c
                    .call_raw(
                        p::TerminalRun::NAME,
                        serde_json::to_value(&params).unwrap_or_default(),
                    )
                    .and_then(|v| {
                        TerminalVm::from_json(&v)
                            .ok_or_else(|| ClientError::Protocol("bad terminal.run result".into()))
                    });
                let ok = v.is_ok();
                reply(r, v)?;
                if ok {
                    self.poll()?;
                }
            }
            Cmd::Kill(id, r) => {
                let v = c.call::<p::TerminalKill>(&target(&id)).map(|_| ());
                reply(r, v)?;
                self.poll()?;
            }
            Cmd::Rm(id, r) => {
                let v = c
                    .call::<p::TerminalRm>(&p::RmParams {
                        target: target(&id),
                        force: false,
                    })
                    .map(|_| ());
                reply(r, v)?;
                self.poll()?;
            }
            Cmd::Rename(id, name, r) => {
                let v = c
                    .call_raw("terminal.rename", json!({"id": id, "name": name}))
                    .map(|_| ());
                reply(r, v)?;
                self.poll()?;
            }
            Cmd::RepoAdd(path, name, r) => {
                let v = c
                    .call::<p::RepoAdd>(&p::RepoAddParams { path, name })
                    .map(|r| RepoVm {
                        name: r.name,
                        path: r.path,
                        default_base: r.default_base,
                    });
                let ok = v.is_ok();
                reply(r, v)?;
                if ok {
                    self.task_polled = None;
                    self.poll()?;
                }
            }
            Cmd::TaskCreate(s, r) => {
                let params = p::TaskCreateParams {
                    repo: s.repo,
                    name: s.name,
                    base: s.base,
                    agent: Some(p::AgentSpec::Kind(s.agent)),
                    prompt: s.prompt,
                    cols: s.cols,
                    rows: s.rows,
                    ..Default::default()
                };
                let v = c
                    .call_raw(
                        p::TaskCreate::NAME,
                        serde_json::to_value(&params).unwrap_or_default(),
                    )
                    .and_then(|v| {
                        TaskVm::from_json(&v)
                            .ok_or_else(|| ClientError::Protocol("bad task.create result".into()))
                    });
                let ok = v.is_ok();
                reply(r, v)?;
                if ok {
                    self.task_polled = None;
                    self.poll()?;
                }
            }
            Cmd::TaskRemove(task, force, delete_branch, r) => {
                let v = c
                    .call::<p::TaskRemove>(&p::TaskRemoveParams {
                        task,
                        force,
                        delete_branch,
                    })
                    .map(|r| r.terminals);
                reply(r, v)?;
                self.task_polled = None;
                self.poll()?;
            }
            Cmd::Ack(id) => {
                if let Err(e) = c.call_raw("terminal.ack", json!({ "id": id }))
                    && is_transport(&e)
                {
                    return Err(e);
                }
            }
            Cmd::Resize(id, cols, rows) => match c.call::<p::TerminalResize>(&p::ResizeParams {
                target: target(&id),
                cols,
                rows,
            }) {
                Err(e) if is_transport(&e) => return Err(e),
                _ => {}
            },
            Cmd::RefreshTasks => self.task_polled = None,
            Cmd::RefreshTerminals => self.terminals_polled = None,
        }
        Ok(())
    }
}

/// The request connection: serves [`PoddSource::request`] one at a time.
/// Connects lazily and never starts podd (the control connection does); a
/// transport error drops the connection and the next request reconnects.
fn request_loop(pipe: String, rx: std_mpsc::Receiver<RpcCmd>) {
    let mut client: Option<Client> = None;
    while let Ok((method, params, reply)) = rx.recv() {
        if client.is_none() {
            client = Client::connect(&pipe).ok();
        }
        let Some(c) = client.as_mut() else {
            let _ = reply.send(Err(SourceError::msg("not connected to podd")));
            continue;
        };
        match c.call_raw(method, params) {
            Ok(v) => drop(reply.send(Ok(v))),
            Err(e) => {
                if is_transport(&e) {
                    client = None;
                }
                let _ = reply.send(Err(SourceError::of(&e)));
            }
        }
    }
}

/// How long a query reply may wait before it is dropped. A program reads the
/// reply only while it waits for it; after that (Codex on Windows reads
/// console key events) the bytes arrive as typed text (E2E B4).
pub const REPLY_MAX_AGE: Duration = Duration::from_millis(500);

/// Decides which view replies go to the program (E2E B4).
///
/// - **Stale**: older than [`REPLY_MAX_AGE`] when the input thread gets to it
///   (it was stuck behind a reconnect): dropped instead of typed into the agent.
/// - **Duplicate**: every panel showing the terminal answers the same query
///   with the same bytes; the program waits for one answer, so the same reply
///   for the same terminal within `REPLY_MAX_AGE` of the last one is dropped.
///   (podd v7 drops duplicates of the holder's DA/DSR/OSC 7501 answers, not of
///   color replies, which the holder does not produce.)
#[derive(Default)]
pub struct ReplyFilter {
    last: HashMap<String, (Vec<u8>, Instant)>,
}

impl ReplyFilter {
    pub fn admit(&mut self, id: &str, bytes: &[u8], born: Instant, now: Instant) -> bool {
        if now.saturating_duration_since(born) > REPLY_MAX_AGE {
            return false;
        }
        if let Some((prev, at)) = self.last.get(id)
            && prev == bytes
            && born.saturating_duration_since(*at) <= REPLY_MAX_AGE
        {
            return false;
        }
        self.last
            .retain(|_, (_, at)| now.saturating_duration_since(*at) <= REPLY_MAX_AGE);
        self.last.insert(id.to_string(), (bytes.to_vec(), born));
        true
    }
}

/// The input connection: `terminal.input` for typed keys and view replies,
/// in order, on a connection of its own. On the control connection a reply
/// waited behind `task.list` (git status of every worktree) right after an
/// app-created task started, reached Codex after its color query timed out
/// and showed up as `]10;rgb:…` in its composer (E2E B4). Connects lazily,
/// never starts podd; input is dropped while podd is unreachable.
fn input_loop(pipe: String, rx: std_mpsc::Receiver<InputCmd>) {
    let mut client: Option<Client> = None;
    let mut filter = ReplyFilter::default();
    let debug = std::env::var_os("POD_APP_DEBUG").is_some();
    while let Ok(first) = rx.recv() {
        let (id, bytes) = match first {
            InputCmd::Keys(id, mut bytes) => {
                // Typing bursts and pastes: merge what is already queued for the same terminal.
                let mut next = None;
                while let Ok(n) = rx.try_recv() {
                    match n {
                        InputCmd::Keys(i, b) if i == id => bytes.extend_from_slice(&b),
                        other => {
                            next = Some(other);
                            break;
                        }
                    }
                }
                send_input(&pipe, &mut client, &id, &bytes);
                match next {
                    None => continue,
                    Some(InputCmd::Keys(i, b)) => (i, b),
                    Some(InputCmd::Reply(i, b, born)) => {
                        match admit_reply(&mut filter, &i, b, born, debug) {
                            Some(b) => (i, b),
                            None => continue,
                        }
                    }
                }
            }
            InputCmd::Reply(i, b, born) => match admit_reply(&mut filter, &i, b, born, debug) {
                Some(b) => (i, b),
                None => continue,
            },
        };
        send_input(&pipe, &mut client, &id, &bytes);
    }
}

fn admit_reply(
    filter: &mut ReplyFilter,
    id: &str,
    bytes: Vec<u8>,
    born: Instant,
    debug: bool,
) -> Option<Vec<u8>> {
    let now = Instant::now();
    let ok = filter.admit(id, &bytes, born, now);
    if debug {
        let verdict = if ok { "sent" } else { "dropped" };
        println!(
            "[pod-app] reply {verdict} for {id} after {} ms: {}",
            now.saturating_duration_since(born).as_millis(),
            bytes.escape_ascii()
        );
    }
    ok.then_some(bytes)
}

fn send_input(pipe: &str, client: &mut Option<Client>, id: &str, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    if client.is_none() {
        *client = Client::connect(pipe).ok();
    }
    let Some(c) = client.as_mut() else { return };
    let params = p::InputParams {
        target: p::TerminalRef::id(id),
        data: p::b64_encode(bytes),
    };
    if let Err(e) = c.call::<p::TerminalInput>(&params)
        && is_transport(&e)
    {
        *client = None;
    }
}

/// `status.subscribe` stream on its own connection (docs/07 §8.3): emits a
/// [`SourceEvent::Status`] per change. Reconnects forever; never starts podd
/// (the control connection does). Missed events (`status.lagged`, a daemon
/// restart) are covered by the control connection's `terminal.list` polling.
///
/// v9: subscribed with `lifecycle: true`; `terminal.changed`, `task.changed`
/// and `status.lagged` make the control connection re-read its lists now
/// (it then polls only slowly), `task.removing` goes to the workspace.
fn status_stream(
    pipe: String,
    events: mpsc::UnboundedSender<SourceEvent>,
    cmds: std_mpsc::Sender<Cmd>,
) {
    use std::io::{BufRead, BufReader, Write};
    loop {
        if events.is_closed() {
            return;
        }
        let Ok(file) = podd::transport::open_pipe(&pipe) else {
            std::thread::sleep(RETRY);
            continue;
        };
        let Ok(mut writer) = file.try_clone() else {
            continue;
        };
        let mut reader = BufReader::new(file);
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": p::StatusSubscribe::NAME, "params": {"lifecycle": true}});
        if writeln!(writer, "{req}")
            .and_then(|_| writer.flush())
            .is_err()
        {
            std::thread::sleep(RETRY);
            continue;
        }
        let mut line = String::new();
        let mut first = true;
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(n) if n > 0 => {}
                _ => break,
            }
            if first {
                first = false;
                // Reply: snapshot of every terminal's status. An error reply
                // (old daemon without status) ends this attempt quietly.
                let Ok(resp) = serde_json::from_str::<p::Response>(&line) else {
                    break;
                };
                let Some(result) = resp.result else { break };
                DELIVERY_EVENTS.store(false, std::sync::atomic::Ordering::Relaxed);
                LIFECYCLE_LIVE.store(true, std::sync::atomic::Ordering::Relaxed);
                // Events may have been missed while unsubscribed: re-read now.
                let _ = cmds.send(Cmd::RefreshTerminals);
                let _ = cmds.send(Cmd::RefreshTasks);
                if let Some(gates) = result
                    .get("gates")
                    .and_then(|g| serde_json::from_value::<Vec<p::GateInfo>>(g.clone()).ok())
                {
                    let _ = events.unbounded_send(SourceEvent::Gates(gates));
                }
                let _ = events.unbounded_send(SourceEvent::OrchResync);
                if let Some(list) = result.get("terminals").and_then(Value::as_array) {
                    for e in list {
                        if let (Some(id), Some(s)) = (
                            e.get("id").and_then(Value::as_str),
                            e.get("status").and_then(StatusVm::from_json),
                        ) {
                            let _ = events.unbounded_send(SourceEvent::Status {
                                id: id.into(),
                                status: Some(s),
                            });
                        }
                    }
                }
                continue;
            }
            let Ok(n) = serde_json::from_str::<p::Notification>(&line) else {
                continue;
            };
            if let Some(ev) = crate::auto_open::created_event(&n)
                && events.unbounded_send(ev).is_err()
            {
                return;
            }
            if let Some(ev) = orch_event(&n) {
                if events.unbounded_send(ev).is_err() {
                    return;
                }
                continue;
            }
            match lifecycle_action(&n) {
                Lifecycle::None => {}
                Lifecycle::Refresh {
                    terminals,
                    tasks,
                    files,
                } => {
                    if terminals {
                        let _ = cmds.send(Cmd::RefreshTerminals);
                    }
                    if tasks {
                        let _ = cmds.send(Cmd::RefreshTasks);
                    }
                    if let Some(id) = files
                        && events
                            .unbounded_send(SourceEvent::TaskFilesChanged(id))
                            .is_err()
                    {
                        return;
                    }
                    continue;
                }
                Lifecycle::Removing(id, worktree) => {
                    if events
                        .unbounded_send(SourceEvent::TaskRemoving { id, worktree })
                        .is_err()
                    {
                        return;
                    }
                    continue;
                }
            }
            if n.method == p::NOTIFY_STATUS_CHANGED {
                let id = n
                    .params
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let removed = n
                    .params
                    .get("removed")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let status = if removed {
                    None
                } else {
                    n.params.get("status").and_then(StatusVm::from_json)
                };
                if events
                    .unbounded_send(SourceEvent::Status { id, status })
                    .is_err()
                {
                    return;
                }
            }
        }
        LIFECYCLE_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        std::thread::sleep(RETRY);
    }
}

/// What a v9 lifecycle notification (or `status.lagged`) asks of the app.
#[derive(Debug, PartialEq, Eq)]
enum Lifecycle {
    None,
    /// Re-read lists; `files`: that task's files changed (diff views refresh).
    Refresh {
        terminals: bool,
        tasks: bool,
        files: Option<String>,
    },
    Removing(String, String),
}

fn lifecycle_action(n: &p::Notification) -> Lifecycle {
    let text = |k: &str| {
        n.params
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match n.method.as_str() {
        p::NOTIFY_STATUS_LAGGED => Lifecycle::Refresh {
            terminals: true,
            tasks: true,
            files: None,
        },
        p::NOTIFY_TERMINAL_CHANGED => Lifecycle::Refresh {
            terminals: true,
            tasks: false,
            files: None,
        },
        p::NOTIFY_TASK_CHANGED => {
            let change = text("change");
            let files = matches!(change.as_str(), "touched" | "git").then(|| text("id"));
            // `touched` alone changes nothing a list shows.
            Lifecycle::Refresh {
                terminals: change == "terminals" || change == "removed",
                tasks: change != "touched",
                files,
            }
        }
        p::NOTIFY_TASK_REMOVING => Lifecycle::Removing(text("id"), text("worktree")),
        // v10: the task's pull request changed (TaskInfo.pr; PR sections re-read).
        p::NOTIFY_TASK_PR_CHANGED => Lifecycle::Refresh {
            terminals: false,
            tasks: true,
            files: None,
        },
        _ => Lifecycle::None,
    }
}

/// Orchestration notifications on the status stream (gates, group checks,
/// review deliveries) as [`SourceEvent`]s; `None` for anything else.
fn orch_event(n: &p::Notification) -> Option<SourceEvent> {
    match n.method.as_str() {
        p::NOTIFY_GATE_CHANGED => {
            let e: p::GateChangedEvent = serde_json::from_value(n.params.clone()).ok()?;
            Some(SourceEvent::Gate(Box::new(e.gate)))
        }
        p::NOTIFY_GROUP_CHECK => {
            let e: p::GroupCheckEvent = serde_json::from_value(n.params.clone()).ok()?;
            Some(SourceEvent::GroupCheck(Box::new(e)))
        }
        p::NOTIFY_REVIEW_DELIVERY => {
            let e: p::ReviewDeliveryEvent = serde_json::from_value(n.params.clone()).ok()?;
            DELIVERY_EVENTS.store(true, std::sync::atomic::Ordering::Relaxed);
            Some(SourceEvent::Delivery(Box::new(e.delivery), e.submit))
        }
        p::NOTIFY_MOD_NOTIFY => {
            let e: p::ModNotifyEvent = serde_json::from_value(n.params.clone()).ok()?;
            Some(SourceEvent::ModNotice(Box::new(e.notice)))
        }
        p::NOTIFY_MOD_CHANGED => {
            let e: p::ModChangedEvent = serde_json::from_value(n.params.clone()).ok()?;
            Some(SourceEvent::ModChanged(Box::new(e.info)))
        }
        p::NOTIFY_ORCH_CHANGED => Some(SourceEvent::OrchChanged(Box::new(
            serde_json::from_value(n.params.clone()).ok()?,
        ))),
        p::NOTIFY_ORCH_MAIL => Some(SourceEvent::OrchMail(Box::new(
            serde_json::from_value(n.params.clone()).ok()?,
        ))),
        _ => None,
    }
}

/// Program presets of the new-terminal dialog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Program {
    Shell,
    Claude,
    Codex,
    Custom,
}

impl Program {
    pub const ALL: [Program; 4] = [
        Program::Shell,
        Program::Claude,
        Program::Codex,
        Program::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Program::Shell => crate::strings::SHELL,
            Program::Claude => "Claude",
            Program::Codex => "Codex",
            Program::Custom => crate::strings::CUSTOM,
        }
    }

    /// argv for the preset; `custom` is a command line (double quotes group).
    pub fn argv(self, custom: &str) -> Vec<String> {
        match self {
            Program::Shell => crate::layouts::shell_integration::integrate(vec![
                "powershell.exe".into(),
                "-NoLogo".into(),
            ]),
            Program::Claude => vec!["claude".into()],
            Program::Codex => vec!["codex".into()],
            Program::Custom => {
                crate::layouts::shell_integration::integrate(split_command_line(custom))
            }
        }
    }
}

/// Split on whitespace, honoring double quotes: `vim -c "set mouse=a"`.
pub fn split_command_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// A minimal in-process podd for native-QA tests. It answers queued commands
/// only when the test calls [`TestBackend::serve`], on the test thread, so the
/// GPUI test scheduler stays deterministic. Nothing is spawned or contacted.
#[cfg(test)]
pub(crate) struct TestBackend {
    cmds: std_mpsc::Receiver<Cmd>,
    rpcs: std_mpsc::Receiver<RpcCmd>,
    inputs: std_mpsc::Receiver<InputCmd>,
    pub events: mpsc::UnboundedSender<SourceEvent>,
    /// Bytes the user sent to each terminal, in order.
    pub typed: Vec<(String, Vec<u8>)>,
    /// `terminal.ack` (done confirmation) per terminal, in order.
    pub acks: Vec<String>,
    /// Every control command / request method seen, in order.
    pub calls: Vec<String>,
    pub repos: Vec<RepoVm>,
    pub tasks: Vec<TaskVm>,
    pub terminals: Vec<TerminalVm>,
    /// Task id -> a real git worktree whose `task.diff` this backend answers.
    pub worktrees: HashMap<String, std::path::PathBuf>,
    /// Extra request answers (e.g. `task.pr.*` in the PR tests); `None` falls through.
    #[allow(clippy::type_complexity)]
    pub rpc: Option<Box<dyn FnMut(&str, &Value) -> Option<Result<Value, SourceError>>>>,
    next: u32,
}

#[cfg(test)]
impl TestBackend {
    pub fn terminal(id: &str, name: &str, task: Option<&str>, state: &str) -> TerminalVm {
        TerminalVm {
            id: id.into(),
            name: Some(name.into()),
            program: "powershell".into(),
            argv: vec!["powershell.exe".into(), "-NoProfile".into()],
            cwd: "C:/fixtures/sample".into(),
            live: crate::vm::Liveness::Running,
            attached: 0,
            created_ms: 0,
            task_id: task.map(str::to_owned),
            status: StatusVm::from_json(
                &json!({"state": state, "kind": "permission", "source": "osc", "confidence": 1.0, "since": 0}),
            ),
            parent_terminal: None,
            parent_task: None,
            unreachable: None,
        }
    }

    pub fn task(id: &str, title: &str, repo: &str, terminals: &[&str]) -> TaskVm {
        let mut t = TaskVm::from_json(&json!({"id": id, "name": title, "agent": "shell", "repo": repo, "terminals": terminals, "branch": format!("pod/{title}")})).expect("test task");
        t.title = title.into();
        t
    }

    /// Connected, every capability on, and the current repos/tasks/terminals.
    pub fn connect(&mut self) {
        let _ = self
            .events
            .unbounded_send(SourceEvent::Conn(ConnState::Connected {
                pid: 0,
                version: "native QA (scripted)".into(),
            }));
        let _ = self.events.unbounded_send(SourceEvent::Caps(Capabilities {
            tasks: true,
            status: true,
            review: true,
            orch: true,
            rename: true,
        }));
        self.publish();
    }

    pub fn publish(&mut self) {
        let _ = self
            .events
            .unbounded_send(SourceEvent::Repos(self.repos.clone()));
        let _ = self
            .events
            .unbounded_send(SourceEvent::Tasks(self.tasks.clone()));
        let _ = self
            .events
            .unbounded_send(SourceEvent::Terminals(self.terminals.clone()));
    }

    /// Live status change, as `status.subscribe` would report it.
    pub fn set_status(&mut self, id: &str, state: &str) {
        let status = StatusVm::from_json(
            &json!({"state": state, "kind": "permission", "source": "osc", "confidence": 1.0, "since": 0, "turn": 2}),
        );
        if let Some(t) = self.terminals.iter_mut().find(|t| t.id == id) {
            t.status = status.clone();
        }
        let _ = self.events.unbounded_send(SourceEvent::Status {
            id: id.into(),
            status,
        });
    }

    /// Answers everything queued so far; returns how many requests were served.
    pub fn serve(&mut self) -> usize {
        let mut n = 0;
        while let Ok(cmd) = self.cmds.try_recv() {
            n += 1;
            match cmd {
                Cmd::RepoAdd(path, name, r) => {
                    self.calls.push(format!("repo.add {path}"));
                    let name = name.unwrap_or_else(|| {
                        path.trim_end_matches(['/', '\\'])
                            .rsplit(['/', '\\'])
                            .next()
                            .unwrap_or("repo")
                            .to_string()
                    });
                    let repo = RepoVm {
                        name,
                        path,
                        default_base: Some("main".into()),
                    };
                    self.repos.retain(|x| x.name != repo.name);
                    self.repos.push(repo.clone());
                    let _ = r.send(Ok(repo));
                    self.publish();
                }
                Cmd::TaskCreate(spec, r) => {
                    self.calls.push(format!(
                        "task.create {}/{} {:?}",
                        spec.repo, spec.name, spec.agent
                    ));
                    let term = format!("t{}", self.next);
                    self.next += 1;
                    let id = format!("{}/{}", spec.repo, spec.name);
                    let mut task = Self::task(&id, &spec.name, &spec.repo, &[&term]);
                    task.worktree = Some(format!("C:/fixtures/worktrees/{}", spec.name));
                    self.terminals.push(Self::terminal(
                        &term,
                        &format!("{} shell", spec.name),
                        Some(&id),
                        "idle",
                    ));
                    self.tasks.push(task.clone());
                    let _ = r.send(Ok(task));
                    self.publish();
                }
                Cmd::Run(spec, r) => {
                    self.calls.push(format!("terminal.run {:?}", spec.argv));
                    let id = format!("t{}", self.next);
                    self.next += 1;
                    let t = Self::terminal(
                        &id,
                        spec.name.as_deref().unwrap_or("shell"),
                        spec.task.as_deref(),
                        "idle",
                    );
                    self.terminals.push(t.clone());
                    let _ = r.send(Ok(t));
                    self.publish();
                }
                Cmd::Kill(id, r) | Cmd::Rm(id, r) => {
                    self.calls.push(format!("terminal.kill/rm {id}"));
                    let _ = r.send(Ok(()));
                }
                Cmd::Rename(id, name, r) => {
                    self.calls.push(format!("terminal.rename {id} {name}"));
                    let _ = r.send(Ok(()));
                }
                Cmd::TaskRemove(task, _, _, r) => {
                    self.calls.push(format!("task.remove {task}"));
                    let _ = r.send(Ok(Vec::new()));
                }
                Cmd::Ack(id) => {
                    self.calls.push(format!("terminal.ack {id}"));
                    self.acks.push(id);
                }
                Cmd::Resize(..) | Cmd::RefreshTasks | Cmd::RefreshTerminals => n -= 1,
            }
        }
        while let Ok((method, params, reply)) = self.rpcs.try_recv() {
            n += 1;
            self.calls.push(format!("{method} {params}"));
            let task = params
                .get("task")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if let Some(a) = self.rpc.as_mut().and_then(|f| f(method, &params)) {
                let _ = reply.send(a);
                continue;
            }
            let answer = match (method, self.worktrees.get(task)) {
                ("task.diff", Some(wt)) => fixture_task_diff(wt),
                _ => Err(SourceError {
                    code: Some(codes::METHOD_NOT_FOUND),
                    message: format!("{method}: not in the native-QA backend"),
                }),
            };
            let _ = reply.send(answer);
        }
        while let Ok(input) = self.inputs.try_recv() {
            if let InputCmd::Keys(id, bytes) = input {
                self.typed.push((id, bytes));
            }
        }
        n
    }

    /// Everything typed into `id`, as text.
    pub fn typed_text(&self, id: &str) -> String {
        String::from_utf8_lossy(
            &self
                .typed
                .iter()
                .filter(|(t, _)| t == id)
                .flat_map(|(_, b)| b.clone())
                .collect::<Vec<_>>(),
        )
        .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_events_map_to_refreshes_and_removal() {
        let n = |method: &str, params: Value| p::Notification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        };
        let refresh = |terminals, tasks, files: Option<&str>| Lifecycle::Refresh {
            terminals,
            tasks,
            files: files.map(str::to_owned),
        };
        assert_eq!(
            lifecycle_action(&n(
                p::NOTIFY_TERMINAL_CHANGED,
                json!({"seq": 1, "id": "t1", "change": "exited"})
            )),
            refresh(true, false, None)
        );
        assert_eq!(
            lifecycle_action(&n(p::NOTIFY_STATUS_LAGGED, json!({"missed": 9}))),
            refresh(true, true, None)
        );
        assert_eq!(
            lifecycle_action(&n(
                p::NOTIFY_TASK_CHANGED,
                json!({"seq": 2, "id": "r/a", "change": "touched", "paths": ["x"]})
            )),
            refresh(false, false, Some("r/a"))
        );
        assert_eq!(
            lifecycle_action(&n(
                p::NOTIFY_TASK_CHANGED,
                json!({"seq": 3, "id": "r/a", "change": "git"})
            )),
            refresh(false, true, Some("r/a"))
        );
        assert_eq!(
            lifecycle_action(&n(
                p::NOTIFY_TASK_CHANGED,
                json!({"seq": 4, "id": "r/a", "change": "terminals", "terminals": ["t2"]})
            )),
            refresh(true, true, None)
        );
        assert_eq!(
            lifecycle_action(&n(
                p::NOTIFY_TASK_CHANGED,
                json!({"seq": 5, "id": "r/a", "change": "status"})
            )),
            refresh(false, true, None)
        );
        assert_eq!(
            lifecycle_action(&n(
                p::NOTIFY_TASK_REMOVING,
                json!({"seq": 6, "id": "r/a", "worktree": "C:/w/a", "force": false, "grace_ms": 300})
            )),
            Lifecycle::Removing("r/a".into(), "C:/w/a".into())
        );
        assert_eq!(
            lifecycle_action(&n(p::NOTIFY_STATUS_CHANGED, json!({}))),
            Lifecycle::None
        );
    }

    #[test]
    fn color_reply_filter_drops_stale_and_duplicate_replies() {
        let reply = b"\x1b]11;rgb:1c1c/1d1d/2121\x1b\\".to_vec();
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut f = ReplyFilter::default();
        // Prompt reply goes out.
        assert!(f.admit("t2", &reply, t0, ms(5)));
        // A second panel on t2 answers the same query: dropped.
        assert!(!f.admit("t2", &reply, ms(3), ms(8)));
        // Same reply for another terminal: its own query.
        assert!(f.admit("t3", &reply, ms(3), ms(9)));
        // A later query of t2 (Codex re-queries on focus/theme change) is answered again.
        assert!(f.admit("t2", &reply, ms(2000), ms(2001)));
        // E2E B4: a reply stuck for longer than the program waits is dropped,
        // not typed into the agent's composer.
        assert!(!f.admit(
            "t4",
            &reply,
            ms(3000),
            ms(3000) + REPLY_MAX_AGE + Duration::from_millis(1)
        ));
        // Different bytes (OSC 10 vs 11) are separate answers.
        let fg = b"\x1b]10;rgb:d6d6/d8d8/dbdb\x1b\\".to_vec();
        assert!(f.admit("t5", &fg, ms(4000), ms(4001)));
        assert!(f.admit("t5", &reply, ms(4000), ms(4001)));
    }

    #[test]
    fn command_line_quotes() {
        assert_eq!(
            split_command_line(r#"vim -c "set mouse=a" f.txt"#),
            ["vim", "-c", "set mouse=a", "f.txt"]
        );
        assert_eq!(
            split_command_line("  cmd.exe   /c dir "),
            ["cmd.exe", "/c", "dir"]
        );
        assert!(split_command_line("   ").is_empty());
    }

    #[test]
    fn presets() {
        assert_eq!(Program::Shell.argv("")[0], "powershell.exe");
        assert_eq!(Program::Claude.argv("ignored"), ["claude"]);
        assert_eq!(
            Program::Custom.argv("git log -n 3"),
            ["git", "log", "-n", "3"]
        );
    }

    #[test]
    fn run_and_mail_stream_events() {
        let n = |method: &str, params: Value| p::Notification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        };
        match orch_event(&n(
            p::NOTIFY_ORCH_CHANGED,
            json!({"seq": 3, "run": "r1", "attempt": "a2"}),
        )) {
            Some(SourceEvent::OrchChanged(e)) => assert_eq!(
                (e.run.as_str(), e.attempt.as_deref(), e.item),
                ("r1", Some("a2"), None)
            ),
            other => panic!("{other:?}"),
        }
        match orch_event(&n(
            p::NOTIFY_ORCH_MAIL,
            json!({"seq": 4, "run": "r1", "to": "run:r1", "id": "m7", "type": "escalation", "subject": "stalled"}),
        )) {
            Some(SourceEvent::OrchMail(e)) => assert_eq!(
                (e.id.as_str(), e.kind, e.subject.as_str()),
                ("m7", p::MailType::Escalation, "stalled")
            ),
            other => panic!("{other:?}"),
        }
        assert!(
            orch_event(&n(p::NOTIFY_ORCH_MAIL, json!({"run": "r1"}))).is_none(),
            "malformed events are dropped"
        );
    }

    #[test]
    fn orchestration_stream_events() {
        let n = |method: &str, params: Value| p::Notification {
            jsonrpc: "2.0".into(),
            method: method.into(),
            params,
        };
        let gate = json!({"seq": 4, "gate": {"id": "g1", "title": "Deploy", "message": "ok?", "state": "pending", "created_ms": 1, "terminal": "t2"}});
        match orch_event(&n(p::NOTIFY_GATE_CHANGED, gate)) {
            Some(SourceEvent::Gate(g)) => {
                assert_eq!((g.id.as_str(), g.terminal.as_deref()), ("g1", Some("t2")))
            }
            other => panic!("{other:?}"),
        }
        let delivery = json!({"seq": 9, "delivery": {"id": "d1", "task": "demo/a", "terminal": "t1", "comment_ids": ["c1"],
            "state": "queued", "prompt": "p", "created_ms": 1, "agent_state": "working"}});
        DELIVERY_EVENTS.store(false, std::sync::atomic::Ordering::Relaxed);
        match orch_event(&n(p::NOTIFY_REVIEW_DELIVERY, delivery)) {
            Some(SourceEvent::Delivery(d, submit)) => {
                assert_eq!((d.state, submit), (p::DeliveryState::Queued, None))
            }
            other => panic!("{other:?}"),
        }
        assert!(
            delivery_events(),
            "a delivery event switches the review panel off polling"
        );
        let check = json!({"seq": 5, "group": "demo/fx", "job": {"id": "k1", "state": "running", "argv": ["cmd"], "tasks": ["demo/fx-shell-1"],
            "timeout_ms": 1000, "started_ms": 1}});
        assert!(
            matches!(orch_event(&n(p::NOTIFY_GROUP_CHECK, check)), Some(SourceEvent::GroupCheck(e)) if e.group == "demo/fx")
        );
        assert!(orch_event(&n(p::NOTIFY_STATUS_CHANGED, json!({}))).is_none());
        assert!(orch_event(&n(p::NOTIFY_GATE_CHANGED, json!({"bad": 1}))).is_none());
    }

    #[test]
    fn transport_vs_rpc_errors() {
        let rpc = ClientError::Rpc(p::RpcError::new(codes::METHOD_NOT_FOUND, "nope"));
        assert!(is_method_not_found(&rpc));
        assert!(!is_transport(&rpc));
        assert!(is_transport(&ClientError::Protocol("closed".into())));
        // A stopping daemon: reconnect (to the next one), not an ordinary error.
        assert!(is_transport(&ClientError::Rpc(p::RpcError::new(
            codes::SERVER_STOPPING,
            "podd is stopping"
        ))));
    }
}
