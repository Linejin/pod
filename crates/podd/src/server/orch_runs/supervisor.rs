//! The supervisor (docs/19 §5.2): after a restart, open attempts whose
//! terminal did not survive are lost; then, on every hub event, wake-up or
//! tick, one pass marks workers ready, escalates a turn that ended without
//! `done` and a stall, loses exited or unreachable workers, releases
//! finished ones, settles waits, admits from the resource queue and types
//! wake-ups. Review deliveries and group checks are mailed to coordinators.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use pod_orch::engine::NewMail;
use pod_orch::model::{attempt_mailbox, run_mailbox};
use serde_json::json;
use tokio::sync::broadcast;

use crate::protocol::*;
use crate::server::{Daemon, log, status};

const TICK_MS: u64 = 500;

/// Called once by `serve` after terminals were re-attached.
pub(in crate::server) fn start(daemon: &Arc<Daemon>) {
    daemon.orch_reconcile_after_start();
    let daemon = daemon.clone();
    tokio::spawn(async move {
        let mut rx = status::subscribe();
        loop {
            daemon.orch_supervise().await;
            tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(ev) => daemon.orch_link(ev),
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
                    }
                },
                _ = daemon.runs.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(TICK_MS)) => {}
            }
        }
    });
}

impl Daemon {
    /// Links to the older orchestration features (docs/19 §9, §16.2): review
    /// comments delivered to a worker's task, and a group check over worker
    /// tasks, are mailed (`status`) to the run's coordinator so it sees them
    /// in `pod orch check`. Each delivery / check job is mailed once.
    fn orch_link(&self, ev: status::HubEvent) {
        use pod_orch::text as tx;
        let (key, tasks, text, payload) = match ev {
            status::HubEvent::Delivery(e) if e.delivery.state == DeliveryState::Delivered => {
                let dl = e.delivery;
                let text = MsgText::new(tx::REVIEW_DELIVERED)
                    .with("task", &dl.task)
                    .with("delivery", &dl.id)
                    .with("comments", dl.comment_ids.len());
                let payload = json!({"review": dl.id, "task": dl.task, "comments": dl.comment_ids});
                (format!("review:{}", dl.id), vec![dl.task], text, payload)
            }
            status::HubEvent::GroupCheck(e)
                if e.member.is_none() && e.job.state != CheckJobState::Running =>
            {
                let state = serde_json::to_value(e.job.state)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let mut text = MsgText::new(tx::GROUP_CHECKED)
                    .with("group", &e.group)
                    .with("state", &state);
                if let Some(all) = e.job.all_passed {
                    text = text.with("all_passed", all);
                }
                let payload = json!({"group": e.group, "job": e.job.id, "state": state, "all_passed": e.job.all_passed});
                (
                    format!("check:{}:{}", e.group, e.job.id),
                    e.job.tasks,
                    text,
                    payload,
                )
            }
            _ => return,
        };
        let workers: Vec<Attempt> = {
            let o = self.orch();
            let mut by_run: BTreeMap<String, Attempt> = BTreeMap::new();
            // The newest attempt of each run that worked in one of these tasks.
            for a in o
                .attempts()
                .filter(|a| a.task.as_ref().is_some_and(|t| tasks.contains(t)))
            {
                by_run.insert(a.run.clone(), a.clone());
            }
            by_run.into_values().collect()
        };
        if workers.is_empty() || !self.runs.linked.lock().unwrap().insert(key) {
            return;
        }
        let mut o = self.orch();
        for a in workers {
            let t = text.clone().with("attempt", &a.id).with("item", &a.item);
            let mut p = payload.clone();
            p["attempt"] = json!(a.id);
            let m = NewMail::pod("pod", run_mailbox(&a.run), MailType::Status, t).payload(p);
            if let Err(e) = self.post(&mut o, None, m) {
                log(&format!("orch: link mail for {}: {}", a.id, e.message));
            }
        }
    }

    /// Open attempts whose terminal did not survive (host restart) are lost;
    /// their coordinators get an escalation.
    fn orch_reconcile_after_start(&self) {
        let open: Vec<Attempt> = self
            .orch()
            .attempts()
            .filter(|a| a.state.is_open())
            .cloned()
            .collect();
        for a in open {
            let term = a.terminal.as_deref().and_then(|t| self.terminal_by_id(t));
            if let Some(t) = &term {
                // The status engine starts a new turn count after a restart.
                let turn = t.status.info().turn;
                let _ = self.orch().update_attempt(&a.id, |x| x.seen_turn = turn);
            }
            let alive = term.is_some();
            if let Some(tid) = a.terminal.as_deref().filter(|_| !alive) {
                if self.holder_retrying(tid) {
                    // orch-quality-02: podd still retries its holder; the
                    // supervisor re-binds or loses it later.
                    self.runs
                        .rebind
                        .lock()
                        .unwrap()
                        .insert(tid.to_string(), false);
                    log(&format!(
                        "orch: attempt {} waits for the holder of {tid} (unreachable, retrying)",
                        a.id
                    ));
                } else {
                    self.orch_lose(&a, "host_restart", "host_restart");
                }
            } else if a.terminal.is_none() && !a.is_queued() {
                // Queued workers keep their place (docs/19 §5.1).
                self.orch_lose(&a, "host_restart", "host_restart_starting");
            }
        }
        // Released workers whose terminal is gone anyway, and notifications
        // that were never typed, are handled by the supervisor loop.
        self.orch_reconcile_gates();
    }

    /// orch-quality-02: `terminal` is a holder podd could not reach at start
    /// and still retries (`lifecycle.rs`).
    pub(super) fn holder_retrying(&self, terminal: &str) -> bool {
        self.unreachable
            .lock()
            .unwrap()
            .iter()
            .any(|u| u.id == terminal && !u.gave_up)
    }

    /// orch-quality-02: an open attempt's terminal is not attached. While
    /// its holder is retried the attempt stays open; otherwise it
    /// is lost: retries used up, the holder gone, or `pod kill` gave up
    /// on it (after a restart), or the terminal was removed.
    fn orch_terminal_missing(&self, a: &Attempt, tid: &str) {
        if self.holder_retrying(tid) {
            return;
        }
        let waited = {
            let mut rb = self.runs.rebind.lock().unwrap();
            match rb.get_mut(tid) {
                // Not seen in either list once: maybe between the two.
                Some(seen @ false) => {
                    *seen = true;
                    return;
                }
                Some(true) => rb.remove(tid).is_some(),
                None => false,
            }
        };
        if waited {
            self.orch_lose(a, "holder_unreachable", "host_restart");
        } else {
            self.orch_lose(a, "terminal_gone", "terminal_gone");
        }
    }

    /// `cause` is a [`pod_orch::text::LOST`] cause; `reason` the stop_reason.
    fn orch_lose(&self, a: &Attempt, reason: &str, cause: &str) {
        let text = MsgText::new(pod_orch::text::LOST)
            .with("attempt", &a.id)
            .with("item", &a.item)
            .with("cause", cause);
        let full = text.en();
        let msg = full.split_once(": ").map(|(_, m)| m).unwrap_or(&full);
        let mut o = self.orch();
        let now = crate::now_ms();
        if !o
            .end_attempt(
                &a.id,
                pod_orch::AttemptState::Lost,
                msg,
                None,
                self.orch_release_ms(),
                now,
            )
            .unwrap_or(false)
        {
            return;
        }
        log(&format!("orch: attempt {} lost ({reason})", a.id));
        let m = NewMail::pod(
            attempt_mailbox(&a.id),
            run_mailbox(&a.run),
            MailType::Escalation,
            text,
        )
        .payload(json!({"stop_reason": reason, "attempt": a.id, "item": a.item}));
        if let Err(e) = self.post(&mut o, None, m) {
            log(&format!("orch: escalation for {}: {}", a.id, e.message));
        }
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
    }

    fn orch_escalate(
        &self,
        a: &Attempt,
        reason: &str,
        subject: MsgText,
        mark: impl FnOnce(&mut Attempt),
    ) {
        let subject = subject.with("attempt", &a.id).with("item", &a.item);
        let mut o = self.orch();
        if o.update_attempt(&a.id, mark).is_err() {
            return;
        }
        log(&format!("orch: {} {reason}", a.id));
        let m = NewMail::pod(
            attempt_mailbox(&a.id),
            run_mailbox(&a.run),
            MailType::Escalation,
            subject,
        )
        .payload(json!({"stop_reason": reason, "attempt": a.id, "item": a.item}));
        if let Err(e) = self.post(&mut o, None, m) {
            log(&format!("orch: escalation for {}: {}", a.id, e.message));
        }
    }

    async fn orch_supervise(self: &Arc<Self>) {
        let now = crate::now_ms();
        let attempts: Vec<Attempt> = self.orch().attempts().cloned().collect();
        let stopping = self.runs.stopping.lock().unwrap().clone();
        for a in attempts
            .iter()
            .filter(|a| a.state.is_open() && !stopping.contains(&a.id))
        {
            let Some(tid) = a.terminal.as_deref() else {
                continue;
            };
            let Some(t) = self.terminal_by_id(tid) else {
                self.orch_terminal_missing(a, tid);
                continue;
            };
            if self.runs.rebind.lock().unwrap().remove(tid).is_some() {
                // orch-quality-02: reached after a restart; its status
                // engine counts turns from here.
                let turn = t.status.info().turn;
                let _ = self.orch().update_attempt(&a.id, |x| x.seen_turn = turn);
                log(&format!("orch: attempt {} re-bound to {tid}", a.id));
                status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
                continue;
            }
            if !t.is_running() {
                self.orch_lose(a, "agent_exited", "agent_exited");
                continue;
            }
            let s = t.status.info();
            let busy = matches!(s.state.as_str(), "working" | "blocked");
            if a.state == pod_orch::AttemptState::Starting
                && (s.state == "working" || s.turn > a.seen_turn)
            {
                let _ = self.orch().mark_ready(&a.id, now);
                status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
            }
            // A turn ended and the worker neither reported nor waits for an answer.
            if s.turn > a.seen_turn && !busy {
                let waiting = self
                    .runs
                    .waiters
                    .lock()
                    .unwrap()
                    .contains_key(&attempt_mailbox(&a.id));
                let open_q = self.orch().has_open_question(&a.id);
                let open_q = open_q || self.orch_waits_on_gate(&a.id);
                // Waiting for other items (`pod orch wait`, §18.2).
                let open_q = open_q || !a.waiting_on.is_empty();
                let turn = s.turn;
                if !waiting && !open_q && a.escalated_turn != Some(turn) {
                    self.orch_escalate(
                        a,
                        "turn_ended_without_done",
                        MsgText::new(pod_orch::text::TURN_ENDED),
                        move |x| {
                            x.seen_turn = turn;
                            x.escalated_turn = Some(turn);
                        },
                    );
                } else {
                    let _ = self
                        .orch()
                        .update_attempt(&a.id, move |x| x.seen_turn = turn);
                }
            }
            // Stalled: working without output for too long (never killed).
            if let Some(stall) = self.orch_config().stall_after {
                let last = t.last_output_ms();
                if s.state == "working"
                    && last > 0
                    && now.saturating_sub(last) >= stall.as_millis() as u64
                    && a.stalled_at_output_ms != Some(last)
                {
                    self.orch_escalate(
                        a,
                        "stalled",
                        MsgText::new(pod_orch::text::STALLED)
                            .with("secs", now.saturating_sub(last) / 1000),
                        move |x| x.stalled_at_output_ms = Some(last),
                    );
                }
            }
        }
        // Finished workers past their release time.
        let due = self.orch().due_releases(now);
        for id in due {
            if let Err(e) = self.release_worker(&id).await {
                log(&format!("orch: release {id}: {}", e.message));
            }
        }
        self.orch_settle_waits();
        // The resource queue: skipped while a start or another mutation runs
        // (that call admits, or the next tick does).
        if let Ok(_ops) = self.runs.ops.try_lock() {
            self.orch_admit(None).await;
        }
        self.orch_wake_ups();
        self.runs.passes.pass_done();
    }
}
