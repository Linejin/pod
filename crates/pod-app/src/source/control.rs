//! The control connection: polls `terminal.list` (and `task.list` / `repo.list`
//! when available), runs commands (run / kill / rm / rename / repo / task /
//! ack / resize), probes capabilities and owns reconnection — including
//! restarting podd the same way the `pod` CLI does (`Client::connect_or_start`).

use std::sync::mpsc as std_mpsc;
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use podd::client::{Client, ClientError};
use podd::protocol::{self as p, Method, codes};
use serde_json::json;

use super::events::{LIFECYCLE_CAPABLE, lifecycle_active};
use super::{Reply, RunSpec, SourceError, SourceEvent, TaskSpec, mapping};
use crate::vm::{Capabilities, ConnState, RepoVm, TaskVm, TerminalVm};

/// How often the terminal list is refreshed while connected (no lifecycle events).
const POLL: Duration = Duration::from_millis(500);
/// How often tasks and repos are refreshed (each task costs several git runs, docs/08 §7).
const TASK_POLL: Duration = Duration::from_millis(3000);
/// With v9 lifecycle events the lists are re-read on events; these slow
/// polls stay as a safety net (docs/03 §4.3).
const LIFECYCLE_TERMINAL_POLL: Duration = Duration::from_secs(15);
const LIFECYCLE_TASK_POLL: Duration = Duration::from_secs(45);

/// Delay between reconnect attempts.
pub(super) const RETRY: Duration = Duration::from_millis(1000);

pub(super) enum Cmd {
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

/// `true` when the error means the connection is gone (vs. an RPC error).
pub(super) fn is_transport(e: &ClientError) -> bool {
    // A daemon refusing requests because it is stopping is as good as gone:
    // drop the connection and reconnect (to the next daemon) instead of
    // showing the refusal as an ordinary error on a half-dead connection.
    matches!(
        e,
        ClientError::Io(_) | ClientError::Protocol(_) | ClientError::NotRunning
    ) || e.is_stopping()
}

pub(super) fn is_method_not_found(e: &ClientError) -> bool {
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

pub(super) struct Control {
    pipe: String,
    rx: std_mpsc::Receiver<Cmd>,
    events: mpsc::UnboundedSender<SourceEvent>,
    pub(super) client: Option<Client>,
    pub(super) caps: Capabilities,
    last_terminals: Option<Vec<TerminalVm>>,
    last_tasks: Option<Vec<TaskVm>>,
    last_repos: Option<Vec<RepoVm>>,
    /// When tasks were last polled (`None` = poll on the next round).
    task_polled: Option<Instant>,
    /// When terminals were last listed (`None` = list on the next round).
    terminals_polled: Option<Instant>,
}

impl Control {
    pub(super) fn new(
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

    pub(super) fn run(mut self) {
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

    pub(super) fn poll(&mut self) -> Result<(), ClientError> {
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
            let terminals = mapping::terminal_list(&list);
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
                    let repos: Vec<RepoVm> = r.repos.into_iter().map(mapping::repo).collect();
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
                    let tasks = mapping::task_list(&v);
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
    pub(super) fn handle(&mut self, cmd: Cmd) -> Result<(), ClientError> {
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
                    .map(mapping::repo);
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
