//! A minimal in-process podd for native-QA tests (`cfg(test)` only).

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;

use futures::channel::mpsc;
use podd::protocol::codes;
use serde_json::{Value, json};

use super::control::Cmd;
use super::fixture::fixture_task_diff;
use super::input::InputCmd;
use super::rpc::RpcCmd;
use super::{PoddSource, SourceError, SourceEvent};
use crate::vm::{Capabilities, ConnState, RepoVm, StatusVm, TaskVm, TerminalVm};

impl PoddSource {
    /// Native-QA tests: a source served by [`TestBackend`] on the test thread
    /// (no podd, no pipe; terminal panels attach to a pipe that never exists).
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
}

/// A minimal in-process podd for native-QA tests. It answers queued commands
/// only when the test calls [`TestBackend::serve`], on the test thread, so the
/// GPUI test scheduler stays deterministic. Nothing is spawned or contacted.
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
