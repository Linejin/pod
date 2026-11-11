//! The data source: the only module that talks to podd.
//!
//! Four kinds of connections, all on plain threads with podd's blocking client:
//! - one **control** connection ([`control`]): polls `terminal.list` (and
//!   `task.list` when available), runs commands (run / kill / rm / rename /
//!   resize), and owns reconnection — including restarting podd the same way
//!   the `pod` CLI does (`Client::connect_or_start`).
//! - the **status stream** ([`events`]): `status.subscribe` with lifecycle
//!   and orchestration notifications.
//! - the **request** ([`rpc`]) and **input** ([`input`]) connections:
//!   feature requests and terminal input, each off the control connection.
//! - one **attach** connection per terminal panel ([`crate::attach`]).
//!
//! Everything the UI sees comes out as [`SourceEvent`]s carrying view models
//! ([`crate::vm`], decoded in [`mapping`]). Daemon features that are still
//! being built (tasks, live status, rename) are detected per connection
//! ([`Capabilities`]); when the daemon answers "method not found" the UI
//! simply hides that part. [`PoddSource::fixture`] ([`fixture`]) and the
//! `cfg(test)` `TestBackend` serve the same interface without podd.

mod control;
mod events;
mod fixture;
mod input;
mod mapping;
mod rpc;
#[cfg(test)]
mod test_backend;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod wire_tests;

use std::sync::mpsc as std_mpsc;
use std::time::Instant;

use futures::channel::{mpsc, oneshot};
use podd::client::ClientError;
use podd::protocol::{self as p, Method};
use serde_json::{Value, json};

use crate::vm::{Capabilities, ConnState, RepoVm, StatusVm, TaskVm, TerminalVm};
use control::{Cmd, Control};
use input::InputCmd;
use rpc::RpcCmd;

pub use events::delivery_events;
// Kept at its old path: `panel.rs` docs link `crate::source::ReplyFilter`.
#[allow(unused_imports)]
pub use input::ReplyFilter;
#[cfg(test)]
pub(crate) use test_backend::TestBackend;

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

/// Cheap to clone; all clones share the control thread (and the request and input threads).
#[derive(Clone)]
pub struct PoddSource {
    pipe: String,
    tx: std_mpsc::Sender<Cmd>,
    rpc: std_mpsc::Sender<RpcCmd>,
    input: std_mpsc::Sender<InputCmd>,
}

impl PoddSource {
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
            .spawn(move || events::status_stream(status_pipe, status_events, status_cmds))
            .expect("spawn status thread");
        let (rpc, rpc_rx) = std_mpsc::channel();
        let rpc_pipe = pipe.clone();
        std::thread::Builder::new()
            .name("podd-request".into())
            .spawn(move || rpc::request_loop(rpc_pipe, rpc_rx))
            .expect("spawn request thread");
        let (input, input_rx) = std_mpsc::channel();
        let input_pipe = pipe.clone();
        std::thread::Builder::new()
            .name("podd-input".into())
            .spawn(move || input::input_loop(input_pipe, input_rx))
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
