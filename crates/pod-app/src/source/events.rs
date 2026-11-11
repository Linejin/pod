//! The `status.subscribe` stream (its own connection) and the notifications
//! it carries: live status, v9 lifecycle events, orchestration events.

use std::sync::atomic::AtomicBool;
use std::sync::mpsc as std_mpsc;

use futures::channel::mpsc;
use podd::protocol::{self as p, Method};
use serde_json::{Value, json};

use super::SourceEvent;
use super::control::{Cmd, RETRY};
use crate::vm::StatusVm;

/// The daemon speaks protocol v9 (lifecycle events), set by the control connection.
pub(super) static LIFECYCLE_CAPABLE: AtomicBool = AtomicBool::new(false);
/// The status stream is subscribed with `lifecycle: true` right now.
static LIFECYCLE_LIVE: AtomicBool = AtomicBool::new(false);

/// Lifecycle events replace fast list polling.
pub(super) fn lifecycle_active() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    LIFECYCLE_CAPABLE.load(Relaxed) && LIFECYCLE_LIVE.load(Relaxed)
}

/// Set once the connected daemon has sent a `review.delivery` event (it then
/// announces every delivery change, so panels need not poll); cleared on
/// every resubscribe.
pub static DELIVERY_EVENTS: AtomicBool = AtomicBool::new(false);

/// True when podd announces delivery changes on the status stream.
pub fn delivery_events() -> bool {
    DELIVERY_EVENTS.load(std::sync::atomic::Ordering::Relaxed)
}

/// `status.subscribe` stream on its own connection (docs/07 §8.3): emits a
/// [`SourceEvent::Status`] per change. Reconnects forever; never starts podd
/// (the control connection does). Missed events (`status.lagged`, a daemon
/// restart) are covered by the control connection's `terminal.list` polling.
///
/// v9: subscribed with `lifecycle: true`; `terminal.changed`, `task.changed`
/// and `status.lagged` make the control connection re-read its lists now
/// (it then polls only slowly), `task.removing` goes to the workspace.
pub(super) fn status_stream(
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
pub(super) enum Lifecycle {
    None,
    /// Re-read lists; `files`: that task's files changed (diff views refresh).
    Refresh {
        terminals: bool,
        tasks: bool,
        files: Option<String>,
    },
    Removing(String, String),
}

pub(super) fn lifecycle_action(n: &p::Notification) -> Lifecycle {
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
pub(super) fn orch_event(n: &p::Notification) -> Option<SourceEvent> {
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
