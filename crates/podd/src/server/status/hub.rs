//! The status hub: one daemon-wide sequence and broadcast for every
//! `status.subscribe` event (status changes, gates, group checks,
//! deliveries, lifecycle, mods, orchestration, sessions), the engine clock
//! and the state dir that hook tokens and persisted status live in.
//!
//! The hub is process-wide (one daemon per process). A publish holds the
//! sequence lock while it sends, so events go out in sequence order; that
//! lock is the last one in the lock order (see the parent module).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use pod_status::Millis;
use tokio::sync::broadcast;

use crate::protocol::*;

/// `status.subscribe` buffer; a slower subscriber gets `status.lagged`.
const EVENT_CAPACITY: usize = 4096;

pub(super) struct Hub {
    /// Unix ms at `epoch`; engine time is `base_ms + epoch.elapsed()` (monotonic).
    pub(super) base_ms: u64,
    epoch: Instant,
    /// Last sequence number; held while sending so events go out in order.
    last_seq: Mutex<u64>,
    tx: broadcast::Sender<HubEvent>,
    pub(super) state_dir: OnceLock<PathBuf>,
}

/// What the hub broadcasts: status changes and (v5) gate changes, in one
/// sequence.
#[derive(Debug, Clone)]
pub(in crate::server) enum HubEvent {
    Status(StatusChangedEvent),
    Gate(GateChangedEvent),
    /// v6: background `group.check` progress.
    GroupCheck(GroupCheckEvent),
    /// A review delivery was recorded or changed state (`review.delivery`).
    Delivery(ReviewDeliveryEvent),
    /// v9 lifecycle events (only for `lifecycle` subscribers).
    Terminal(TerminalChangedEvent),
    Task(TaskChangedEvent),
    TaskRemoving(TaskRemovingEvent),
    /// v10 `task.pr.changed` (lifecycle subscribers only).
    TaskPr(TaskPrChangedEvent),
    /// v11 extensions (docs/16): `pod.notify` and mod state changes.
    ModNotify(ModNotifyEvent),
    ModChanged(ModChangedEvent),
    /// v16 orchestration (docs/19 §7): unfiltered subscribers only.
    OrchChanged(OrchChangedEvent),
    OrchMail(OrchMailEvent),
    /// Session discovery (docs/18), lifecycle subscribers only.
    Sessions(SessionsChangedEvent),
}

pub(super) fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        base_ms: crate::now_ms(),
        epoch: Instant::now(),
        last_seq: Mutex::new(0),
        tx: broadcast::channel(EVENT_CAPACITY).0,
        state_dir: OnceLock::new(),
    })
}

/// Engine clock: monotonic within this daemon, close to unix ms (so `since`
/// can be shown as a wall-clock time).
pub fn now() -> Millis {
    let h = hub();
    h.base_ms + h.epoch.elapsed().as_millis() as u64
}

impl Hub {
    pub(super) fn publish(
        &self,
        id: &str,
        status: StatusInfo,
        from: Option<String>,
        removed: bool,
    ) -> u64 {
        let mut seq = self.last_seq.lock().unwrap();
        *seq += 1;
        // No receivers is fine.
        let _ = self.tx.send(HubEvent::Status(StatusChangedEvent {
            seq: *seq,
            id: id.to_string(),
            status,
            from,
            removed,
        }));
        *seq
    }
}

/// Announce a created or resolved gate on every `status.subscribe` stream.
pub(in crate::server) fn publish_gate(gate: GateInfo) -> u64 {
    let h = hub();
    let mut seq = h.last_seq.lock().unwrap();
    *seq += 1;
    let _ =
        h.tx.send(HubEvent::Gate(GateChangedEvent { seq: *seq, gate }));
    *seq
}

/// Announce `group.check` job progress on every unfiltered `status.subscribe` stream.
pub(in crate::server) fn publish_group_check(
    group: &str,
    job: CheckJob,
    member: Option<MemberCheck>,
) -> u64 {
    let h = hub();
    let mut seq = h.last_seq.lock().unwrap();
    *seq += 1;
    let _ = h.tx.send(HubEvent::GroupCheck(GroupCheckEvent {
        seq: *seq,
        group: group.to_string(),
        job,
        member,
    }));
    *seq
}

/// Announce a recorded or changed review delivery (`review.delivery`) on
/// every `status.subscribe` stream that wants its terminal.
pub(in crate::server) fn publish_delivery(
    delivery: DeliveryInfo,
    submit: Option<SubmitCheck>,
) -> u64 {
    let h = hub();
    let mut seq = h.last_seq.lock().unwrap();
    *seq += 1;
    let _ = h.tx.send(HubEvent::Delivery(ReviewDeliveryEvent {
        seq: *seq,
        delivery,
        submit,
    }));
    *seq
}

/// Publish with the next sequence number (the closure gets it).
fn publish_with(make: impl FnOnce(u64) -> HubEvent) -> u64 {
    let h = hub();
    let mut seq = h.last_seq.lock().unwrap();
    *seq += 1;
    let _ = h.tx.send(make(*seq));
    *seq
}

/// v9 `terminal.changed`.
pub(in crate::server) fn publish_terminal(
    id: &str,
    change: TerminalChange,
    terminal: Option<TerminalInfo>,
) -> u64 {
    publish_with(|seq| {
        HubEvent::Terminal(TerminalChangedEvent {
            seq,
            id: id.to_string(),
            change,
            terminal: terminal.map(Box::new),
        })
    })
}

/// v9 `task.changed`; `seq` in `ev` is replaced.
pub(in crate::server) fn publish_task(mut ev: TaskChangedEvent) -> u64 {
    publish_with(|seq| {
        ev.seq = seq;
        HubEvent::Task(ev)
    })
}

/// A `task.changed` event of `change` with no payload (fill fields after).
pub(in crate::server) fn task_event(id: &str, change: TaskChange) -> TaskChangedEvent {
    TaskChangedEvent {
        seq: 0,
        id: id.to_string(),
        change,
        task: None,
        status: None,
        git: None,
        terminals: Vec::new(),
        paths: Vec::new(),
        open_hint: None,
    }
}

/// v9 `task.removing`.
pub(in crate::server) fn publish_task_removing(
    id: &str,
    worktree: &str,
    force: bool,
    grace_ms: u64,
) -> u64 {
    publish_with(|seq| {
        HubEvent::TaskRemoving(TaskRemovingEvent {
            seq,
            id: id.to_string(),
            worktree: worktree.to_string(),
            force,
            grace_ms,
        })
    })
}

/// v10 `task.pr.changed`.
pub(in crate::server) fn publish_task_pr(id: &str, pr: Option<TaskPr>) -> u64 {
    publish_with(|seq| {
        HubEvent::TaskPr(TaskPrChangedEvent {
            seq,
            id: id.to_string(),
            pr,
        })
    })
}

/// v13 `orch.changed`.
pub(in crate::server) fn publish_orch_changed(
    run: &str,
    item: Option<&str>,
    attempt: Option<&str>,
) -> u64 {
    publish_with(|seq| {
        HubEvent::OrchChanged(OrchChangedEvent {
            seq,
            run: run.to_string(),
            item: item.map(str::to_string),
            attempt: attempt.map(str::to_string),
        })
    })
}

/// v13 `orch.mail` (subject only).
pub(in crate::server) fn publish_orch_mail(m: &Mail) -> u64 {
    publish_with(|seq| {
        HubEvent::OrchMail(OrchMailEvent {
            seq,
            run: m.run.clone(),
            to: m.to.clone(),
            id: m.id.clone(),
            kind: m.kind,
            subject: m.subject.clone(),
            text: m.text.clone(),
            from: m.from.clone(),
        })
    })
}

/// v11 `mod.notify` / `mod.changed`; the `seq` in `ev` is replaced.
pub(in crate::server) fn publish_mod(ev: HubEvent) -> u64 {
    publish_with(|seq| match ev {
        HubEvent::ModNotify(mut e) => {
            e.seq = seq;
            HubEvent::ModNotify(e)
        }
        HubEvent::ModChanged(mut e) => {
            e.seq = seq;
            HubEvent::ModChanged(e)
        }
        other => other,
    })
}

pub(in crate::server) fn publish_sessions(mut ev: SessionsChangedEvent) -> u64 {
    publish_with(|seq| {
        ev.seq = seq;
        HubEvent::Sessions(ev)
    })
}

/// Receive every later hub event (status and gates).
pub(in crate::server) fn subscribe() -> broadcast::Receiver<HubEvent> {
    hub().tx.subscribe()
}

/// Remember the state dir (hook tokens, persisted status). Called by `serve`
/// before terminals are re-attached.
pub(in crate::server) fn set_state_dir(dir: &Path) {
    let _ = hub().state_dir.set(dir.to_path_buf());
}
