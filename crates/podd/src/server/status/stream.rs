//! `status.wait` and the `status.subscribe` stream over the hub: a snapshot
//! of the wanted terminals and pending gates first, then the hub's events
//! filtered per connection (terminal ids, lifecycle opt-in), events the
//! snapshot already shows skipped, `status.lagged` when the subscriber
//! falls behind.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::io::BufReader;
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::broadcast;

use super::hub::{HubEvent, hub, subscribe};
use crate::protocol::*;
use crate::server::terminal::Terminal;
use crate::server::{
    Daemon, discard_input, err_response, notification, ok_response, params, write_json,
};

/// Connected `status.subscribe` streams that asked for lifecycle events.
static LIFECYCLE_SUBSCRIBERS: AtomicUsize = AtomicUsize::new(0);

pub(in crate::server) fn lifecycle_subscribers() -> usize {
    LIFECYCLE_SUBSCRIBERS.load(AtomicOrdering::Relaxed)
}

struct LifecycleGuard;
impl LifecycleGuard {
    fn new() -> Self {
        LIFECYCLE_SUBSCRIBERS.fetch_add(1, AtomicOrdering::Relaxed);
        LifecycleGuard
    }
}
impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        LIFECYCLE_SUBSCRIBERS.fetch_sub(1, AtomicOrdering::Relaxed);
    }
}

const STATE_NAMES: &[&str] = &[
    "unknown", "idle", "working", "blocked", "done", "error", "exited",
];

enum Want {
    Any,
    State(String),
    AnyChange,
}

pub(super) async fn status_wait(
    daemon: &Daemon,
    p: StatusWaitParams,
) -> Result<StatusWaitResult, RpcError> {
    let want = match p.status.as_deref() {
        None => Want::Any,
        Some("any_change" | "any-change") => Want::AnyChange,
        Some(s) if STATE_NAMES.contains(&s) => Want::State(s.to_string()),
        Some(s) => {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("unknown status {s:?} (expected one of {STATE_NAMES:?} or any_change)"),
            ));
        }
    };
    if matches!(want, Want::Any) && !p.turn_end {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            "status or turn_end is required",
        ));
    }
    let t = daemon.resolve(&p.target)?;
    // Subscribe before the first check so no change is missed.
    let mut rx = subscribe();
    let start = Instant::now();
    let initial = t.status.info();
    let deadline = p
        .timeout_ms
        .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
    let outcome = loop {
        let cur = t.status.info();
        let state_ok = match &want {
            Want::Any => true,
            Want::State(s) => cur.state == *s,
            Want::AnyChange => cur.state_label() != initial.state_label(),
        };
        let turn_ok = !p.turn_end || (cur.turn > initial.turn && cur.state != "working");
        let running = t.is_running();
        if state_ok && turn_ok && (running || matches!(&want, Want::State(s) if s == "exited")) {
            break WaitOutcome::Matched;
        }
        if !running {
            break WaitOutcome::Exited;
        }
        // Wait for an event of this terminal.
        let next = async {
            loop {
                match rx.recv().await {
                    Ok(HubEvent::Status(ev)) if ev.id == t.id => return,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => return,
                    Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
                }
            }
        };
        match deadline {
            Some(d) => {
                if tokio::time::timeout_at(d, next).await.is_err() {
                    break WaitOutcome::TimedOut;
                }
            }
            None => next.await,
        }
    };
    Ok(StatusWaitResult {
        outcome,
        status: t.status.info(),
        terminal: t.info(),
        waited_ms: start.elapsed().as_millis() as u64,
    })
}

/// `status.subscribe`: reply with every terminal's current status, then
/// stream `status.changed` until the client disconnects.
pub(in crate::server) async fn stream_subscribe(
    daemon: Arc<Daemon>,
    id: Value,
    p: Value,
    mut reader: BufReader<tokio::io::ReadHalf<NamedPipeServer>>,
    mut write: tokio::io::WriteHalf<NamedPipeServer>,
) -> anyhow::Result<()> {
    let p: StatusSubscribeParams = match params(p) {
        Ok(p) => p,
        Err(e) => return write_json(&mut write, &err_response(id, e)).await,
    };
    let wanted = |tid: &str| p.ids.is_empty() || p.ids.iter().any(|x| x == tid);
    let mut rx = subscribe();
    let _lifecycle = p.lifecycle.then(LifecycleGuard::new);
    let terms: Vec<Arc<Terminal>> = daemon.terminals.lock().unwrap().clone();
    let entries: Vec<StatusEntry> = terms
        .iter()
        .filter(|t| wanted(&t.id))
        .map(|t| t.status.entry())
        .collect();
    // Events already reflected in the snapshot are skipped per terminal.
    let mut seen: HashMap<String, u64> = entries.iter().map(|e| (e.id.clone(), e.seq)).collect();
    let gate_wanted = |g: &GateInfo| p.ids.is_empty() || g.terminal.as_deref().is_some_and(wanted);
    let gates: Vec<GateInfo> = daemon
        .orch
        .lock()
        .unwrap()
        .gates
        .iter()
        .filter(|g| g.state == GateState::Pending && gate_wanted(g))
        .cloned()
        .collect();
    let result = StatusSubscribeResult {
        epoch: hub().base_ms,
        seq: entries.iter().map(|e| e.seq).max().unwrap_or(0),
        terminals: entries,
        gates,
    };
    write_json(&mut write, &ok_response(id, serde_json::to_value(result)?)).await?;

    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(HubEvent::Gate(ev)) => {
                    if !gate_wanted(&ev.gate) {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_GATE_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::GroupCheck(ev)) => {
                    // Not about a terminal: only for unfiltered subscribers.
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_GROUP_CHECK, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Delivery(ev)) => {
                    if !wanted(&ev.delivery.terminal) {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_REVIEW_DELIVERY, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Terminal(ev)) => {
                    if !p.lifecycle || !wanted(&ev.id) {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_TERMINAL_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Task(ev)) => {
                    if !p.lifecycle || !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_TASK_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::TaskRemoving(ev)) => {
                    if !p.lifecycle || !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_TASK_REMOVING, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::TaskPr(ev)) => {
                    if !p.lifecycle || !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_TASK_PR_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::ModNotify(ev)) => {
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_MOD_NOTIFY, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Sessions(ev)) => {
                    if !p.lifecycle || !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_SESSIONS_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::ModChanged(ev)) => {
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_MOD_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::OrchChanged(ev)) => {
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_ORCH_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::OrchMail(ev)) => {
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_ORCH_MAIL, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Status(ev)) => {
                    if !wanted(&ev.id) || ev.seq <= seen.get(&ev.id).copied().unwrap_or(0) {
                        continue;
                    }
                    seen.insert(ev.id.clone(), ev.seq);
                    if write_json(&mut write, &notification(NOTIFY_STATUS_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    let n = notification(NOTIFY_STATUS_LAGGED, StatusLaggedEvent { missed });
                    if write_json(&mut write, &n).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Anything the client sends is ignored; EOF ends the subscription.
            r = discard_input(&mut reader) => {
                if !matches!(r, Ok(n) if n > 0) {
                    break;
                }
            }
        }
    }
    Ok(())
}
