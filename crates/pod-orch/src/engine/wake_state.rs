//! Wake-up bookkeeping per mailbox (orch-02): a notification is scheduled,
//! then delivered (persisted with the run) or failed (memory only), and a
//! check clears it.

use super::{MAX_UNCONFIRMED_WAKES, Orch, Result};
use crate::model::{Ms, WakeState};

impl Orch {
    pub fn mailbox_run(&self, mailbox: &str) -> Option<String> {
        if let Some(r) = mailbox.strip_prefix("run:") {
            return self.runs.contains_key(r).then(|| r.to_string());
        }
        mailbox
            .strip_prefix("attempt:")
            .and_then(|a| self.attempt(a).ok())
            .map(|a| a.run.clone())
    }

    pub fn notified(&self, mailbox: &str) -> Option<Ms> {
        let run = self.mailbox_run(mailbox)?;
        self.runs.get(&run)?.rec.wakes.get(mailbox)?.notified_ms
    }

    fn wake_state(&self, mailbox: &str) -> Option<&WakeState> {
        let run = self.mailbox_run(mailbox)?;
        self.runs.get(&run)?.rec.wakes.get(mailbox)
    }

    /// A notification for this mailbox is being typed (not confirmed yet).
    pub fn wake_pending(&self, mailbox: &str) -> bool {
        self.wake_state(mailbox)
            .is_some_and(|w| w.pending_ms.is_some())
    }

    /// Notifications typed but not taken since the last check.
    pub fn wake_unconfirmed(&self, mailbox: &str) -> u32 {
        self.wake_state(mailbox).map_or(0, |w| w.unconfirmed)
    }

    /// The mailbox has waking mail, no notification delivered or on its
    /// way, and has not used up its unconfirmed tries.
    pub fn wants_wake(&self, mailbox: &str) -> bool {
        let w = self.wake_state(mailbox);
        w.is_none_or(|w| {
            w.notified_ms.is_none()
                && w.pending_ms.is_none()
                && w.unconfirmed < MAX_UNCONFIRMED_WAKES
        }) && self.unread_waking(mailbox) > 0
    }

    /// orch-02, step 1: a notification is about to be typed for these
    /// mailboxes. Nothing is persisted; [`Orch::wake_delivered`] or
    /// [`Orch::wake_failed`] settles it.
    pub fn wake_scheduled(&mut self, mailboxes: &[String], now: Ms) -> Result<()> {
        for mb in mailboxes {
            if let Some(run) = self.mailbox_run(mb) {
                self.run_mut(&run)?
                    .rec
                    .wakes
                    .entry(mb.clone())
                    .or_default()
                    .pending_ms = Some(now);
            }
        }
        Ok(())
    }

    /// orch-02, step 2: the agent took the notification (or nothing can
    /// confirm it). Persisted: no second notification until a check.
    pub fn wake_delivered(&mut self, mailboxes: &[String], now: Ms) -> Result<()> {
        for mb in mailboxes {
            if let Some(run) = self.mailbox_run(mb) {
                let w = self.run_mut(&run)?.rec.wakes.entry(mb.clone()).or_default();
                w.pending_ms = None;
                w.notified_ms = Some(now);
                self.save_run(&run)?;
            }
        }
        Ok(())
    }

    /// orch-02: the notification was not taken (`unconfirmed`: typed but
    /// no reaction; otherwise not typed at all). The mailbox wants a wake
    /// again; unconfirmed tries are limited ([`MAX_UNCONFIRMED_WAKES`]).
    pub fn wake_failed(&mut self, mailboxes: &[String], unconfirmed: bool) -> Result<()> {
        for mb in mailboxes {
            if let Some(run) = self.mailbox_run(mb)
                && let Some(w) = self.run_mut(&run)?.rec.wakes.get_mut(mb)
            {
                w.pending_ms = None;
                if unconfirmed {
                    w.unconfirmed += 1;
                }
            }
        }
        Ok(())
    }

    /// Typed and delivered in one step (tests and callers that cannot
    /// confirm).
    pub fn mark_notified(&mut self, mailboxes: &[String], now: Ms) -> Result<()> {
        self.wake_scheduled(mailboxes, now)?;
        self.wake_delivered(mailboxes, now)
    }

    pub(super) fn clear_notified(&mut self, mailbox: &str) -> Result<()> {
        if let Some(run) = self.mailbox_run(mailbox) {
            let st = self.run_mut(&run)?;
            if st.rec.wakes.remove(mailbox).is_some() {
                self.save_run(&run)?;
            }
        }
        Ok(())
    }
}
