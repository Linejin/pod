//! Sessions of paired devices, the clock they run on, and rate limits.
//!
//! Sessions live in memory only: a daemon restart ends all of them
//! (docs/17 §4.2-5). A session dies at 8 h absolute, after 15 min without a
//! user-initiated request (background polling does not extend it), or when
//! its device is revoked (the device's `auth_epoch` moved on). Session ids
//! are not secrets in M1: the channel that will carry them (M2 TLS tokens,
//! M4 Noise) authenticates the device first.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use super::registry::Registry;
use crate::protocol::*;

pub const SESSION_ABSOLUTE_MS: u64 = 8 * 3600 * 1000;
pub const SESSION_IDLE_MS: u64 = 15 * 60 * 1000;
/// Sessions per device at once (oldest dropped).
const MAX_SESSIONS_PER_DEVICE: usize = 4;

/// Unix ms. The daemon uses the system clock; tests drive a fake one.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        crate::now_ms()
    }
}

/// Test clock: starts where it is told, moves only when advanced.
#[cfg(test)]
pub struct FakeClock(std::sync::atomic::AtomicU64);

#[cfg(test)]
impl FakeClock {
    pub fn new(ms: u64) -> Arc<Self> {
        Arc::new(Self(std::sync::atomic::AtomicU64::new(ms)))
    }

    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

pub fn system_clock() -> Arc<dyn Clock> {
    Arc::new(SystemClock)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub id: String,
    pub device: String,
    pub epoch: u64,
    pub created_ms: u64,
    pub last_ms: u64,
}

#[derive(Debug, Default)]
pub struct Sessions {
    /// Distinguishes this daemon run (ids of a previous run never match).
    boot: u64,
    next: u64,
    map: HashMap<String, Session>,
}

fn auth(m: impl Into<String>) -> RpcError {
    RpcError::new(mobile_codes::MOBILE_AUTH, m)
}

impl Sessions {
    pub fn new(boot: u64) -> Self {
        Self {
            boot,
            next: 0,
            map: HashMap::new(),
        }
    }

    /// Open a session for an authenticated, active device.
    pub fn open(&mut self, reg: &Registry, device: &str, now: u64) -> Result<String, RpcError> {
        let d = reg
            .device(device)
            .filter(|d| d.active())
            .ok_or_else(|| auth(format!("device {device:?} is not paired or was revoked")))?;
        let mut mine: Vec<(u64, String)> = self
            .map
            .values()
            .filter(|s| s.device == device)
            .map(|s| (s.created_ms, s.id.clone()))
            .collect();
        mine.sort();
        while mine.len() >= MAX_SESSIONS_PER_DEVICE {
            let (_, id) = mine.remove(0);
            self.map.remove(&id);
        }
        self.next += 1;
        let id = format!("s{:x}-{}", self.boot, self.next);
        self.map.insert(
            id.clone(),
            Session {
                id: id.clone(),
                device: device.to_string(),
                epoch: d.auth_epoch,
                created_ms: now,
                last_ms: now,
            },
        );
        Ok(id)
    }

    /// The live session's device. `user` = a request the person made
    /// (extends the idle timer); background polls pass false.
    pub fn check(
        &mut self,
        reg: &Registry,
        id: &str,
        now: u64,
        user: bool,
    ) -> Result<String, RpcError> {
        let Some(s) = self.map.get_mut(id) else {
            return Err(auth(
                "no such session (expired, revoked or from before a restart)",
            ));
        };
        let reason = match reg.device(&s.device) {
            None => Some("device unknown"),
            Some(d) if !d.active() => Some("device revoked"),
            Some(d) if d.auth_epoch != s.epoch => Some("device re-keyed"),
            Some(_) if now >= s.created_ms + SESSION_ABSOLUTE_MS => Some("session expired (8 h)"),
            Some(_) if now >= s.last_ms + SESSION_IDLE_MS => Some("session idle for 15 min"),
            Some(_) => None,
        };
        if let Some(r) = reason {
            self.map.remove(id);
            return Err(auth(r));
        }
        if user {
            s.last_ms = now;
        }
        Ok(s.device.clone())
    }

    pub fn drop_device(&mut self, device: &str) {
        self.map.retain(|_, s| s.device != device);
    }

    pub fn count(&self, device: &str) -> usize {
        self.map.values().filter(|s| s.device == device).count()
    }
}

/// Request classes with separate budgets (docs/17 §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bucket {
    Read,
    Decide,
    Message,
}

impl Bucket {
    /// (requests, window ms)
    fn budget(self) -> (usize, u64) {
        match self {
            Self::Read => (60, 60_000),
            Self::Decide => (10, 60_000),
            Self::Message => (5, 60_000),
        }
    }
}

/// Sliding-window limits per device and bucket. Every attempt counts,
/// refused ones included (no free guessing).
#[derive(Debug, Default)]
pub struct Limits {
    hits: HashMap<(String, Bucket), VecDeque<u64>>,
}

impl Limits {
    pub fn hit(&mut self, device: &str, bucket: Bucket, now: u64) -> Result<(), RpcError> {
        let (max, window) = bucket.budget();
        let q = self.hits.entry((device.to_string(), bucket)).or_default();
        while q.front().is_some_and(|&t| t + window <= now) {
            q.pop_front();
        }
        if q.len() >= max {
            let retry = q.front().map_or(window, |&t| t + window - now);
            let mut e = RpcError::new(
                mobile_codes::RATE_LIMITED,
                format!("too many requests; retry in {} s", retry.div_ceil(1000)),
            );
            e.data = Some(serde_json::json!({ "retry_after_ms": retry }));
            return Err(e);
        }
        q.push_back(now);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        let mut r = Registry::default();
        r.pair_stub("Pixel", "pk", 0).unwrap();
        r
    }

    #[test]
    fn idle_and_absolute_expiry_on_the_boundary() {
        let clock = FakeClock::new(1_000);
        let r = reg();
        let mut s = Sessions::new(7);
        let id = s.open(&r, "m1", clock.now_ms()).unwrap();
        assert!(id.starts_with("s7-"));
        // Just inside the idle window, by a background poll: does not extend.
        clock.advance(SESSION_IDLE_MS - 1);
        assert_eq!(s.check(&r, &id, clock.now_ms(), false).unwrap(), "m1");
        clock.advance(1);
        assert_eq!(
            s.check(&r, &id, clock.now_ms(), false).unwrap_err().code,
            mobile_codes::MOBILE_AUTH,
            "15 min after the last user request"
        );
        // Gone for good, even with activity now.
        assert!(s.check(&r, &id, clock.now_ms(), true).is_err());

        // User requests keep it alive, but never past 8 h.
        let id = s.open(&r, "m1", clock.now_ms()).unwrap();
        let start = clock.now_ms();
        while clock.now_ms() + 10 * 60_000 < start + SESSION_ABSOLUTE_MS {
            clock.advance(10 * 60_000);
            s.check(&r, &id, clock.now_ms(), true).unwrap();
        }
        let left = start + SESSION_ABSOLUTE_MS - clock.now_ms();
        clock.advance(left - 1);
        s.check(&r, &id, clock.now_ms(), true).unwrap();
        clock.advance(1);
        let e = s.check(&r, &id, clock.now_ms(), true).unwrap_err();
        assert!(e.message.contains("8 h"), "{}", e.message);
    }

    #[test]
    fn revoke_restart_and_unknown_devices() {
        let mut r = reg();
        let mut s = Sessions::new(1);
        let id = s.open(&r, "m1", 0).unwrap();
        r.revoke("m1", 5).unwrap();
        assert!(
            s.check(&r, &id, 6, true)
                .unwrap_err()
                .message
                .contains("revoked")
        );
        assert!(
            s.open(&r, "m1", 6).is_err(),
            "revoked devices get no session"
        );
        assert!(s.open(&r, "m9", 6).is_err());
        // A new daemon run knows no session of the previous one.
        let mut r = reg();
        let mut old = Sessions::new(1);
        let id = old.open(&r, "m1", 0).unwrap();
        let mut fresh = Sessions::new(2);
        assert!(fresh.check(&r, &id, 1, true).is_err());
        // Re-keyed (epoch moved without revoke) also ends it.
        r.devices[0].auth_epoch += 1;
        assert!(
            old.check(&r, &id, 1, true)
                .unwrap_err()
                .message
                .contains("re-keyed")
        );
    }

    #[test]
    fn sessions_per_device_are_bounded() {
        let r = reg();
        let mut s = Sessions::new(1);
        let first = s.open(&r, "m1", 0).unwrap();
        for t in 1..=MAX_SESSIONS_PER_DEVICE as u64 {
            s.open(&r, "m1", t).unwrap();
        }
        assert_eq!(s.count("m1"), MAX_SESSIONS_PER_DEVICE);
        assert!(s.check(&r, &first, 10, false).is_err(), "oldest dropped");
        s.drop_device("m1");
        assert_eq!(s.count("m1"), 0);
    }

    #[test]
    fn rate_limits_slide() {
        let mut l = Limits::default();
        for i in 0..5 {
            l.hit("m1", Bucket::Message, 1_000 + i).unwrap();
        }
        let e = l.hit("m1", Bucket::Message, 2_000).unwrap_err();
        assert_eq!(e.code, mobile_codes::RATE_LIMITED);
        assert_eq!(e.data.unwrap()["retry_after_ms"], 59_000);
        l.hit("m2", Bucket::Message, 2_000).unwrap();
        l.hit("m1", Bucket::Read, 2_000).unwrap();
        assert!(l.hit("m1", Bucket::Message, 60_999).is_err());
        l.hit("m1", Bucket::Message, 61_000).unwrap();
        for i in 0..10 {
            l.hit("m1", Bucket::Decide, i).unwrap();
        }
        assert!(l.hit("m1", Bucket::Decide, 10).is_err());
    }
}
