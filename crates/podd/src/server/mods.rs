//! Extensions in the daemon (docs/16): one `pod-mods` thread, fed by the
//! status hub (event bridge) and the status sampler (detectors), acting
//! through [`PoddHost`]. `mod.*` RPCs manage it.
//!
//! Mods live in `POD_MODS_DIR` or `%APPDATA%\Pod\mods`; `POD_MODS=off`
//! turns the whole thing off. Logs go to `<state dir>\mods\<name>.log`.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;

use pod_mods::{
    GroupFacts, GroupMemberFacts, Host, ModConfig, ModHandle, Notice, Scope, StatusHint, TaskFacts,
    TerminalFacts,
};
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::status::{HubEvent, publish_mod};
use super::terminal::Terminal;
use super::{Daemon, log, params, to_value};
use crate::protocol::*;

/// `pod.task.runCheck` time limit.
const CHECK_TIMEOUT: Duration = Duration::from_secs(600);

static MODS: OnceLock<ModHandle> = OnceLock::new();

fn handle() -> Option<&'static ModHandle> {
    MODS.get()
}

/// Called once by `serve`.
pub(super) fn start(daemon: &Arc<Daemon>) {
    if std::env::var("POD_MODS").is_ok_and(|v| v.eq_ignore_ascii_case("off")) {
        log("extensions off (POD_MODS=off)");
        return;
    }
    let cfg = ModConfig {
        root: ModConfig::default_root(),
        log_dir: Some(daemon.config.state_dir.join("mods")),
        limits: Default::default(),
    };
    log(&format!("extensions from {}", cfg.root.display()));
    let host = Arc::new(PoddHost {
        daemon: Arc::downgrade(daemon),
        rt: tokio::runtime::Handle::current(),
    });
    let h = ModHandle::start(cfg, host);
    if MODS.set(h.clone()).is_err() {
        return;
    }
    // Event bridge: hub events → small JSON payloads for the mods.
    let daemon = Arc::downgrade(daemon);
    let mut rx = super::status::subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    let Some(d) = daemon.upgrade() else { return };
                    if let Some((name, scope, payload)) = bridge(&d, ev) {
                        h.post_event(name, scope, &payload);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

fn task_of(d: &Daemon, terminal: &str) -> Option<String> {
    d.tasks_of_terminal(terminal).into_iter().next()
}

/// A hub event as a mod event: name, scope, payload (docs/16 §5).
fn bridge(d: &Daemon, ev: HubEvent) -> Option<(&'static str, Scope, Value)> {
    let h = handle()?;
    let name = match &ev {
        HubEvent::Status(_) => "status.changed",
        HubEvent::Terminal(_) => "terminal.changed",
        HubEvent::Task(_) => "task.changed",
        HubEvent::Delivery(_) => "review.delivery",
        HubEvent::Gate(_) => "gate.changed",
        HubEvent::GroupCheck(_) => "group.check",
        HubEvent::OrchChanged(_) => "orch.changed",
        HubEvent::OrchMail(_) => "orch.mail",
        HubEvent::TaskRemoving(_)
        | HubEvent::TaskPr(_)
        | HubEvent::ModNotify(_)
        | HubEvent::ModChanged(_)
        | HubEvent::Sessions(_) => return None,
    };
    if !h.wants_event(name) {
        return None;
    }
    let text = |v: &Value| {
        serde_json::to_value(v)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
    };
    Some(match ev {
        HubEvent::Status(e) => {
            // Transitions only (not confidence / progress updates).
            if e.removed || e.from.is_none() {
                return None;
            }
            let task = task_of(d, &e.id);
            let p = json!({"terminal": e.id, "task": task, "state": e.status.state, "kind": e.status.kind, "from": e.from,
                "source": e.status.source, "badge": e.status.badge, "agent": e.status.agent});
            (
                name,
                Scope {
                    task,
                    terminal: Some(e.id),
                },
                p,
            )
        }
        HubEvent::Terminal(e) => {
            let task = task_of(d, &e.id);
            let change = serde_json::to_value(e.change).ok();
            let p =
                json!({"terminal": e.id, "task": task, "change": change.as_ref().and_then(text)});
            (
                name,
                Scope {
                    task,
                    terminal: Some(e.id),
                },
                p,
            )
        }
        HubEvent::Task(e) => {
            let change = serde_json::to_value(e.change).ok();
            let p = json!({"task": e.id, "change": change.as_ref().and_then(text), "state": e.status.as_ref().map(|s| s.state.clone())});
            (
                name,
                Scope {
                    task: Some(e.id),
                    terminal: None,
                },
                p,
            )
        }
        HubEvent::Delivery(e) => {
            let dl = e.delivery;
            let state = serde_json::to_value(dl.state).ok();
            let p = json!({"delivery": dl.id, "task": dl.task, "terminal": dl.terminal, "state": state.as_ref().and_then(text)});
            (
                name,
                Scope {
                    task: Some(dl.task),
                    terminal: Some(dl.terminal),
                },
                p,
            )
        }
        HubEvent::Gate(e) => {
            let g = e.gate;
            let state = serde_json::to_value(g.state).ok();
            let p = json!({"gate": g.id, "task": g.task, "terminal": g.terminal, "state": state.as_ref().and_then(text), "title": g.title});
            (
                name,
                Scope {
                    task: g.task,
                    terminal: g.terminal,
                },
                p,
            )
        }
        HubEvent::GroupCheck(e) => {
            let state = serde_json::to_value(e.job.state).ok();
            let p = json!({"group": e.group, "state": state.as_ref().and_then(text), "all_passed": e.job.all_passed});
            (name, Scope::default(), p)
        }
        // docs/19 §16.2: subjects only (never bodies); the scope is the
        // worker's task, so a mod may act on it (e.g. `runCheck` when done).
        HubEvent::OrchChanged(e) => {
            let (state, task) = d.orch_attempt_facts(e.attempt.as_deref());
            let p = json!({"run": e.run, "item": e.item, "attempt": e.attempt, "state": state, "task": task});
            (
                name,
                Scope {
                    task,
                    terminal: None,
                },
                p,
            )
        }
        HubEvent::OrchMail(e) => {
            let attempt = [e.to.as_str(), e.from.as_str()]
                .into_iter()
                .find_map(|a| a.strip_prefix("attempt:"))
                .map(str::to_string);
            let (_, task) = d.orch_attempt_facts(attempt.as_deref());
            let kind = serde_json::to_value(e.kind).ok();
            let p = json!({"run": e.run, "id": e.id, "from": e.from, "to": e.to, "type": kind.as_ref().and_then(text), "subject": e.subject,
                "text": e.text, "attempt": attempt, "task": task});
            (
                name,
                Scope {
                    task,
                    terminal: None,
                },
                p,
            )
        }
        _ => return None,
    })
}

/// Status sampler hook: a terminal's screen changed and/or its title. Cheap
/// when no detector matches (one read lock).
pub(super) fn sample(daemon: &Daemon, t: &Terminal, screen: Option<&str>, title: Option<String>) {
    let Some(h) = handle() else { return };
    if (screen.is_none() && title.is_none()) || !h.wants_terminal(&t.argv) {
        return;
    }
    let lines = screen.map(|s| s.lines().map(|l| l.trim_end().to_string()).collect());
    let task = task_of(daemon, &t.id);
    h.post_sample(&t.id, task.as_deref(), &t.argv, lines, title);
}

// ───────────────────────────── host ─────────────────────────────

struct PoddHost {
    daemon: Weak<Daemon>,
    rt: tokio::runtime::Handle,
}

fn hint_state(h: &StatusHint) -> pod_status::AgentState {
    use pod_status::{AgentState, BlockedKind};
    match h.state.as_str() {
        "idle" => AgentState::Idle,
        "working" => AgentState::Working,
        "done" => AgentState::Done,
        "error" => AgentState::Error,
        _ => AgentState::Blocked(match h.kind.as_deref() {
            Some("permission") => BlockedKind::Permission,
            Some("question") => BlockedKind::Question,
            Some("auth") => BlockedKind::Auth,
            _ => BlockedKind::Other,
        }),
    }
}

impl PoddHost {
    fn d(&self) -> Result<Arc<Daemon>, String> {
        self.daemon
            .upgrade()
            .ok_or_else(|| "daemon stopping".to_string())
    }

    fn terminal(&self, id: &str) -> Result<Arc<Terminal>, String> {
        let d = self.d()?;
        let t = d
            .terminals
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .cloned();
        t.ok_or_else(|| format!("no terminal {id}"))
    }
}

impl Host for PoddHost {
    fn terminals(&self) -> Vec<TerminalFacts> {
        let Ok(d) = self.d() else { return Vec::new() };
        let terms: Vec<Arc<Terminal>> = d.terminals.lock().unwrap().clone();
        terms
            .iter()
            .map(|t| {
                let s = t.status.info();
                TerminalFacts {
                    id: t.id.clone(),
                    name: t.name.clone(),
                    task: task_of(&d, &t.id),
                    state: s.state,
                    kind: s.kind,
                    agent: s.agent,
                    argv0: t
                        .argv
                        .first()
                        .map(|a| pod_mods::program_name(a))
                        .unwrap_or_default(),
                    running: t.is_running(),
                }
            })
            .collect()
    }

    fn tasks(&self) -> Vec<TaskFacts> {
        let Ok(d) = self.d() else { return Vec::new() };
        let recs: Vec<_> = d
            .registry
            .lock()
            .unwrap()
            .tasks
            .iter()
            .map(|t| {
                (
                    t.id.clone(),
                    t.name.clone(),
                    t.repo.clone(),
                    t.terminals.clone(),
                )
            })
            .collect();
        recs.into_iter()
            .map(|(id, name, repo, terminals)| TaskFacts {
                state: d.task_status_now(&id).map(|s| s.state),
                id,
                name,
                repo,
                terminals,
            })
            .collect()
    }

    fn groups(&self) -> Vec<GroupFacts> {
        let Ok(d) = self.d() else { return Vec::new() };
        let groups: Vec<(String, String, Vec<String>)> = d
            .orch
            .lock()
            .unwrap()
            .groups
            .iter()
            .map(|g| {
                (
                    g.id.clone(),
                    g.name.clone(),
                    g.members.iter().map(|m| m.task.clone()).collect(),
                )
            })
            .collect();
        groups
            .into_iter()
            .map(|(id, name, tasks)| GroupFacts {
                id,
                name,
                members: tasks
                    .into_iter()
                    .map(|task| GroupMemberFacts {
                        state: d.task_status_now(&task).map(|s| s.state),
                        task,
                    })
                    .collect(),
            })
            .collect()
    }

    fn notify(&self, notice: Notice) {
        publish_mod(HubEvent::ModNotify(ModNotifyEvent { seq: 0, notice }));
    }

    fn send_text(&self, terminal: &str, text: &str, enter: bool) -> Result<(), String> {
        let d = self.d()?;
        let target = TerminalRef {
            id: Some(terminal.to_string()),
            name: None,
        };
        d.running(&target).map_err(|e| e.message)?;
        let p = SendParams {
            target,
            text: text.to_string(),
            enter,
            confirm: enter,
        };
        self.rt.spawn(async move {
            if let Err(e) = d.send(p).await {
                log(&format!("mod send failed: {}", e.message));
            }
        });
        Ok(())
    }

    fn set_status(
        &self,
        owner: &str,
        terminal: &str,
        hint: Option<StatusHint>,
    ) -> Result<(), String> {
        let t = self.terminal(terminal)?;
        t.status
            .on_mod(owner, hint.as_ref().map(|h| (hint_state(h), h.msg.clone())));
        Ok(())
    }

    fn clear_status(&self, owner: &str) {
        let Ok(d) = self.d() else { return };
        let terms: Vec<Arc<Terminal>> = d.terminals.lock().unwrap().clone();
        for t in terms {
            t.status.clear_mod(owner);
        }
    }

    fn run_check(
        &self,
        owner: &str,
        task: &str,
        check: &str,
        argv: Vec<String>,
    ) -> Result<(), String> {
        let d = self.d()?;
        let rec = d.task_record(task).map_err(|e| e.message)?;
        let (owner, task, check) = (owner.to_string(), task.to_string(), check.to_string());
        self.rt.spawn_blocking(move || {
            let dir = std::path::PathBuf::from(&rec.worktree);
            let r = super::groups::run_check(&dir, &argv, CHECK_TIMEOUT, &AtomicBool::new(false));
            if let Some(h) = handle() {
                let p = json!({"task": task, "check": check, "passed": r.passed, "exit_code": r.exit_code, "timed_out": r.timed_out,
                    "error": r.error, "output_tail": r.output_tail, "duration_ms": r.duration_ms});
                h.post_to(&owner, "check.finished", Scope { task: Some(task.clone()), terminal: None }, &p);
            }
        });
        Ok(())
    }

    fn request_gate(
        &self,
        owner: &str,
        task: &str,
        title: &str,
        message: &str,
    ) -> Result<String, String> {
        let d = self.d()?;
        let g = d
            .gate_create(GateCreateParams {
                task: Some(task.into()),
                title: Some(format!("[{owner}] {title}")),
                message: message.into(),
                ..Default::default()
            })
            .map_err(|e| e.message)?;
        Ok(g.id)
    }

    fn mod_changed(&self, info: &ModInfo) {
        publish_mod(HubEvent::ModChanged(ModChangedEvent {
            seq: 0,
            info: info.clone(),
        }));
    }
}

// ───────────────────────────── RPC ─────────────────────────────

pub(super) fn handles(method: &str) -> bool {
    method.starts_with("mod.")
}

fn off() -> RpcError {
    RpcError::new(codes::INVALID_REQUEST, "extensions are off (POD_MODS=off)")
}

fn failed(e: String) -> RpcError {
    RpcError::new(codes::INVALID_PARAMS, e)
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce(&ModHandle) -> Result<T, String> + Send + 'static,
) -> Result<T, RpcError> {
    let h = handle().ok_or_else(off)?;
    tokio::task::spawn_blocking(move || f(h))
        .await
        .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, e.to_string()))?
        .map_err(failed)
}

pub(super) async fn dispatch(method: &str, p: Value) -> Result<Value, RpcError> {
    match method {
        ModListMethod::NAME => {
            let _: Empty = params(p)?;
            if handle().is_none() {
                return to_value(ModListResult::default());
            }
            to_value(ModListResult {
                enabled: true,
                list: blocking(|h| h.list()).await?,
            })
        }
        ModSetEnabled::NAME => {
            let p: ModEnableParams = params(p)?;
            to_value(blocking(move |h| h.set_enabled(&p.name, p.enabled)).await?)
        }
        ModGrant::NAME => {
            let p: ModGrantParams = params(p)?;
            to_value(blocking(move |h| h.grant(&p.name, p.permission, p.granted)).await?)
        }
        ModLog::NAME => {
            let p: ModLogParams = params(p)?;
            let name = p.name.clone();
            let lines = blocking(move |h| h.log(&p.name, p.tail.unwrap_or(200))).await?;
            to_value(ModLogResult { name, lines })
        }
        ModCommand::NAME => {
            let p: ModCommandParams = params(p)?;
            blocking(move |h| {
                h.command(
                    &p.name,
                    &p.command,
                    Scope {
                        task: p.task,
                        terminal: p.terminal,
                    },
                )
            })
            .await?;
            to_value(ModCommandResult {})
        }
        ModReload::NAME => {
            let p: ModReloadParams = params(p)?;
            to_value(ModListResult {
                enabled: true,
                list: blocking(move |h| h.reload(p.name.as_deref())).await?,
            })
        }
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method {other:?}"),
        )),
    }
}
