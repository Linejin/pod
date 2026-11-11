//! Wake-ups (docs/19 §4.3, orch-02, orch-quality-04): one notification line
//! typed into an idle agent with unread mail; reserved before it is typed,
//! settled after the Enter check (delivered, unconfirmed or not typed).

use std::collections::BTreeMap;
use std::sync::Arc;

use pod_orch::model::{attempt_mailbox, run_mailbox};
use pod_orch::wake;

use crate::protocol::*;
use crate::server::{Daemon, log, status};

impl Daemon {
    /// orch-02: settle a typed notification. Confirmed is persisted as
    /// delivered; unconfirmed (or not checkable, orch-quality-04) or not
    /// typed (input refused, holder gone)
    /// wants a wake again: right away when nothing was typed, after a check
    /// or a restart when the line was typed but not taken
    /// ([`pod_orch::engine::MAX_UNCONFIRMED_WAKES`]).
    fn orch_wake_settled(&self, terminal: &str, mark: &[String], typed: WakeTyped) {
        let mut o = self.orch();
        let r = match typed {
            WakeTyped::Taken => o.wake_delivered(mark, crate::now_ms()),
            WakeTyped::Unconfirmed => {
                // Not typed again: the line may still sit in the input box
                // (a second one would be appended). A check or a restart
                // allows the next one.
                log(&format!(
                    "orch: wake-up in {terminal} was not confirmed as submitted; the line may sit in the input box (press Enter there, or run `pod orch check`)"
                ));
                self.runs.unconfirmed.note(mark, crate::now_ms());
                o.wake_failed(mark, true)
            }
            WakeTyped::NotTyped => o.wake_failed(mark, false),
        };
        drop(o);
        if let Err(e) = r {
            log(&format!("orch: wake-up bookkeeping: {}", e.message));
        }
        if let Some(run) = mark.first().and_then(|mb| self.orch().mailbox_run(mb)) {
            status::publish_orch_changed(&run, None, None);
        }
        self.runs.wake.notify_one();
    }

    /// Type one notification line into idle agents with unread mail.
    pub(super) fn orch_wake_ups(self: &Arc<Self>) {
        // terminal -> its mailboxes that want a wake-up
        let mut by_term: BTreeMap<String, Vec<String>> = BTreeMap::new();
        {
            let o = self.orch();
            let waiters = self.runs.waiters.lock().unwrap();
            let mut boxes: Vec<String> = o.runs().map(|r| run_mailbox(&r.rec.run.id)).collect();
            boxes.extend(o.attempts().map(|a| attempt_mailbox(&a.id)));
            for mb in boxes {
                if waiters.contains_key(&mb) || !o.wants_wake(&mb) {
                    continue;
                }
                if let Some(t) = o.mailbox_terminal(&mb) {
                    by_term.entry(t).or_default().push(mb);
                }
            }
        }
        for (tid, boxes) in by_term {
            let Some(t) = self.terminal_by_id(&tid) else {
                continue;
            };
            if !t.is_running() {
                continue;
            }
            let s = t.status.info();
            let quiet = crate::now_ms().saturating_sub(t.last_output_ms());
            if wake::may_wake(&s.state, t.human_draft(), quiet).is_err() {
                continue;
            }
            let (count, mark) = {
                let mut o = self.orch();
                // Every mailbox of the terminal counts; all are marked.
                let all = o.terminal_mailboxes(&tid);
                let n: usize = all.iter().map(|mb| o.unread_waking(mb)).sum();
                let mut mark = boxes.clone();
                for mb in all {
                    if !mark.contains(&mb)
                        && o.unread_waking(&mb) > 0
                        && o.notified(&mb).is_none()
                        && !o.wake_pending(&mb)
                    {
                        mark.push(mb);
                    }
                }
                // orch-02: scheduled only; marked delivered once the agent
                // took the line (below).
                if let Err(e) = o.wake_scheduled(&mark, crate::now_ms()) {
                    log(&format!("orch: {}", e.message));
                    continue;
                }
                (n, mark)
            };
            let line = wake::notification_line(count);
            log(&format!("orch: wake {tid} ({count} unread)"));
            let t2 = t.clone();
            let d = self.clone();
            self.runs.passes.wake_started();
            tokio::spawn(async move {
                let written = crate::now_ms();
                let chars = line.chars().count();
                // orch-quality-04: a refused line (input backlog, holder
                // gone) is not typed; no Enter, a wake again later.
                t2.remember_sent(&line);
                let body = t2.try_input(line.clone().into_bytes());
                let s = match body {
                    Ok(()) => crate::server::submit::press_enter(&t2, written, chars, true, |_| {})
                        .await
                        .map(|s| s.state),
                    Err(e) => {
                        log(&format!("orch: wake-up for {} not typed: {e:?}", t2.id));
                        None
                    }
                };
                d.orch_wake_settled(&t2.id, &mark, wake_outcome(body, s));
                d.runs.passes.wake_settled();
            });
        }
    }
}

/// orch-quality-04: how a typed wake-up ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WakeTyped {
    /// The agent started a turn for it.
    Taken,
    /// Typed and Enter pressed, but nothing showed the agent took it
    /// (including agents that report no status): not a delivery.
    Unconfirmed,
    /// The line was refused (input backlog, holder gone) or Enter could not
    /// be sent: nothing to take, a wake-up again.
    NotTyped,
}

/// The line's input result and the Enter check's state, as [`WakeTyped`].
/// A refused line never had its Enter pressed.
pub(super) fn wake_outcome(
    body: Result<(), crate::server::terminal::InputRefused>,
    enter: Option<SubmitState>,
) -> WakeTyped {
    match (body, enter) {
        (Err(_), _) | (Ok(()), None) => WakeTyped::NotTyped,
        (Ok(()), Some(SubmitState::Confirmed)) => WakeTyped::Taken,
        (
            Ok(()),
            Some(SubmitState::Unverifiable | SubmitState::Unconfirmed | SubmitState::Pending),
        ) => WakeTyped::Unconfirmed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// orch-quality-04: a refused line is not typed (its Enter is never
    /// pressed, so the Enter state is `None`), and only a confirmed Enter
    /// is a delivery; not checkable counts as unconfirmed.
    #[test]
    fn wake_outcome_needs_the_line_and_a_confirmed_enter() {
        use crate::server::terminal::InputRefused;
        use SubmitState::*;
        let backlog = Err(InputRefused::Backlog { in_flight: 9 });
        for enter in [None, Some(Confirmed), Some(Unverifiable)] {
            assert_eq!(wake_outcome(backlog, enter), WakeTyped::NotTyped);
            assert_eq!(
                wake_outcome(Err(InputRefused::Gone), enter),
                WakeTyped::NotTyped
            );
        }
        assert_eq!(wake_outcome(Ok(()), None), WakeTyped::NotTyped);
        assert_eq!(wake_outcome(Ok(()), Some(Confirmed)), WakeTyped::Taken);
        for s in [Unverifiable, Unconfirmed, Pending] {
            assert_eq!(wake_outcome(Ok(()), Some(s)), WakeTyped::Unconfirmed);
        }
    }
}
