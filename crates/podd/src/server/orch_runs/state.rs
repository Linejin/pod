//! The shared state's guards and helpers: the engine and config locks,
//! the "blocked reader" and "being stopped" marks, the `request_id` cache
//! (`ops` serializes the mutating calls around it), supervisor passes and
//! unconfirmed wake-ups.

use std::collections::HashMap;

use pod_orch::engine::Orch;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::rpc::oe;
use super::{OrchConfig, OrchRuns};
use crate::protocol::*;
use crate::server::Daemon;

fn release_ms(c: &OrchConfig) -> Option<u64> {
    c.release_after.map(|d| d.as_millis() as u64)
}

/// orch-quality-06: progress of the supervisor, for `orch.queue` (tests
/// wait for passes instead of sleeping before "nothing happened").
#[derive(Default)]
pub struct Passes {
    done: std::sync::atomic::AtomicU64,
    wakes: std::sync::atomic::AtomicU64,
}

impl Passes {
    pub(super) fn pass_done(&self) {
        self.done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(super) fn wake_started(&self) {
        self.wakes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    pub(super) fn wake_settled(&self) {
        self.wakes.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// (passes finished, wake-ups in flight).
    pub fn get(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::SeqCst;
        (self.done.load(SeqCst), self.wakes.load(SeqCst))
    }
}

/// orch-02: since when each mailbox has a typed but unconfirmed wake-up.
/// The engine's [`Orch::wake_unconfirmed`] count decides whether one is
/// open (a check or a restart clears it); this only remembers the time.
#[derive(Default)]
pub struct Unconfirmed(std::sync::Mutex<HashMap<String, u64>>);

impl Unconfirmed {
    /// A typed wake-up for these mailboxes was not confirmed at `now`
    /// (the first time counts).
    pub fn note(&self, mailboxes: &[String], now: u64) {
        let mut m = self.0.lock().unwrap();
        for mb in mailboxes {
            m.entry(mb.clone()).or_insert(now);
        }
    }

    /// Since when `mailbox` has an open unconfirmed wake-up; forgets
    /// settled ones.
    pub fn since(&self, o: &Orch, mailbox: &str) -> Option<u64> {
        let mut m = self.0.lock().unwrap();
        if o.wake_unconfirmed(mailbox) == 0 {
            m.remove(mailbox);
            return None;
        }
        // Unknown time (cannot happen: noted when counted): say "now".
        Some(*m.entry(mailbox.to_string()).or_insert_with(crate::now_ms))
    }
}

/// Holds a mailbox's "someone is blocked reading it" mark.
pub(super) struct Waiting<'a> {
    runs: &'a OrchRuns,
    boxes: Vec<String>,
}

impl<'a> Waiting<'a> {
    pub(super) fn new(runs: &'a OrchRuns, boxes: Vec<String>) -> Self {
        let mut w = runs.waiters.lock().unwrap();
        for b in &boxes {
            *w.entry(b.clone()).or_default() += 1;
        }
        drop(w);
        Waiting { runs, boxes }
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut w = self.runs.waiters.lock().unwrap();
        for b in &self.boxes {
            if let Some(n) = w.get_mut(b) {
                *n -= 1;
                if *n == 0 {
                    w.remove(b);
                }
            }
        }
        drop(w);
        // A reader left: mail it did not take may need a notification now.
        self.runs.wake.notify_one();
    }
}

/// Marks an attempt as being stopped while it lives.
pub(super) struct Stopping<'a>(&'a OrchRuns, String);

impl<'a> Stopping<'a> {
    pub(super) fn new(runs: &'a OrchRuns, id: &str) -> Self {
        runs.stopping.lock().unwrap().insert(id.to_string());
        Stopping(runs, id.to_string())
    }
}

impl Drop for Stopping<'_> {
    fn drop(&mut self) {
        self.0.stopping.lock().unwrap().remove(&self.1);
    }
}

pub(super) fn cached<T: DeserializeOwned>(
    o: &Orch,
    req: Option<&str>,
    method: &str,
) -> Result<Option<T>, RpcError> {
    let Some(id) = req else { return Ok(None) };
    match o.request(id, method).map_err(oe)? {
        Some(v) => serde_json::from_value(v)
            .map(Some)
            .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("stored result: {e}"))),
        None => Ok(None),
    }
}

pub(super) fn remember<T: Serialize>(
    o: &mut Orch,
    req: Option<&str>,
    method: &str,
    result: &T,
) -> Result<(), RpcError> {
    if let Some(id) = req {
        let v = serde_json::to_value(result)
            .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, e.to_string()))?;
        o.remember(id, method, v, crate::now_ms()).map_err(oe)?;
    }
    Ok(())
}

impl Daemon {
    pub(super) fn orch(&self) -> std::sync::MutexGuard<'_, Orch> {
        self.runs.engine.lock().unwrap()
    }

    /// The timings and defaults in effect (a copy; never held across an await).
    pub(super) fn orch_config(&self) -> OrchConfig {
        self.runs
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(super) fn orch_release_ms(&self) -> Option<u64> {
        release_ms(&self.runs.config.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub(super) fn orch_config_set(&self, s: OrchSettings) -> Result<OrchSettings, RpcError> {
        let mut c = self.runs.config.lock().unwrap_or_else(|e| e.into_inner());
        let out = c
            .set(&self.runs.state_dir, &s)
            .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
        // The per-run worker cap belongs to `orch.limits`: keep the engine's.
        let mut o = self.orch();
        o.limits = pod_orch::engine::Limits {
            run_worker_cap: o.limits.run_worker_cap,
            ..c.limits
        };
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::super::records::run_info;
    use super::*;
    use pod_orch::engine::NewMail;

    /// orch-02 (app part): the time of an unconfirmed wake-up is reported
    /// while the engine counts it open, kept from the first miss, and gone
    /// after a check (which settles it).
    #[test]
    fn unconfirmed_wake_since_follows_the_engine() {
        let dir = std::env::temp_dir().join(format!(
            "podd-unconfirmed-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let mut o = Orch::load(&dir, Default::default(), 1);
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.send(
            None,
            NewMail::new("human", "run:r1", MailType::Status, "hi"),
            |_| false,
            2,
        )
        .unwrap();
        let rb = vec!["run:r1".to_string()];
        let u = Unconfirmed::default();
        assert_eq!(u.since(&o, "run:r1"), None);
        // Typed but not taken (what `orch_wake_settled` does).
        o.wake_scheduled(&rb, 3).unwrap();
        u.note(&rb, 4);
        o.wake_failed(&rb, true).unwrap();
        assert_eq!(u.since(&o, "run:r1"), Some(4));
        u.note(&rb, 9);
        assert_eq!(u.since(&o, "run:r1"), Some(4), "the first miss counts");
        let info = run_info(&o, "r1", &u).unwrap();
        assert_eq!(info.wake_unconfirmed_ms, Some(4));
        // A check settles it; a later miss starts a new time.
        o.check(&rb, true, 10).unwrap();
        assert_eq!(u.since(&o, "run:r1"), None);
        assert_eq!(run_info(&o, "r1", &u).unwrap().wake_unconfirmed_ms, None);
        u.note(&rb, 20);
        o.wake_failed(&rb, false).unwrap();
        assert_eq!(
            u.since(&o, "run:r1"),
            None,
            "not typed at all is not unconfirmed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
