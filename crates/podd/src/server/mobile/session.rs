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

use super::challenge::{equal, hash, random_secret, secret_value, unauthenticated, valid_secret};
use super::registry::Registry;
use crate::protocol::mobile_v1 as wire;
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
    /// Latest observed wall clock, including background reads.
    observed_ms: Option<u64>,
    credentials: HashMap<String, SessionCredentials>,
}

struct SessionCredentials {
    bearer: [u8; 32],
    csrf: [u8; 32],
    boot: wire::Id,
    scopes: wire::List<wire::Scope, 20>,
}
impl std::fmt::Debug for SessionCredentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionCredentials([redacted])")
    }
}

pub(crate) fn policy_scopes(
    reg: &Registry,
    device: &str,
) -> Result<wire::List<wire::Scope, 20>, wire::Error> {
    let d = reg
        .device(device)
        .filter(|d| d.active())
        .ok_or_else(unauthenticated)?;
    wire::List::new(
        d.policy
            .iter()
            .filter(|(_, caps)| caps.contains(&MobileCap::Status))
            .map(|(task, caps)| {
                Ok(wire::Scope {
                    task: wire::Id::new(task.clone()).map_err(|_| unauthenticated())?,
                    caps: wire::List::new(caps.iter().copied().collect())
                        .map_err(|_| unauthenticated())?,
                })
            })
            .collect::<Result<Vec<_>, wire::Error>>()?,
    )
    .map_err(|_| unauthenticated())
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
            observed_ms: None,
            credentials: HashMap::new(),
        }
    }

    /// Open a session for an authenticated, active device.
    pub fn open(&mut self, reg: &Registry, device: &str, now: u64) -> Result<String, RpcError> {
        self.observe(now)?;
        self.check_storage(reg)?;
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
        mine.sort_by_key(|(time, id)| {
            (
                *time,
                id.rsplit('-').next().and_then(|n| n.parse::<u64>().ok()),
            )
        });
        while mine.len() >= MAX_SESSIONS_PER_DEVICE {
            let (_, id) = mine.remove(0);
            self.map.remove(&id);
            self.credentials.remove(&id);
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
        self.observe(now)?;
        self.check_storage(reg)?;
        let Some(s) = self.map.get_mut(id) else {
            return Err(auth(
                "no such session (expired, revoked or from before a restart)",
            ));
        };
        let reason = match reg.device(&s.device) {
            None => Some("device unknown"),
            Some(d) if !d.active() => Some("device revoked"),
            Some(d) if d.auth_epoch != s.epoch => Some("device re-keyed"),
            Some(_) if now.saturating_sub(s.created_ms) >= SESSION_ABSOLUTE_MS => {
                Some("session expired (8 h)")
            }
            Some(_) if now.saturating_sub(s.last_ms) >= SESSION_IDLE_MS => {
                Some("session idle for 15 min")
            }
            Some(_) => None,
        };
        if let Some(r) = reason {
            self.map.remove(id);
            self.credentials.remove(id);
            return Err(auth(r));
        }
        if user {
            s.last_ms = now;
        }
        Ok(s.device.clone())
    }

    pub fn drop_device(&mut self, device: &str) {
        self.map.retain(|_, s| s.device != device);
        self.credentials.retain(|id, _| self.map.contains_key(id));
    }

    /// Off, shutdown, or an uncertain resume invalidates every session.
    /// T1 calls this when suspend/resume elapsed time cannot be trusted.
    pub fn clear(&mut self) {
        self.map.clear();
        self.credentials.clear();
    }

    pub(super) fn observe(&mut self, now: u64) -> Result<(), RpcError> {
        if self.observed_ms.is_some_and(|last| now < last) {
            self.clear();
            self.observed_ms = Some(now);
            return Err(auth("clock regressed; sessions ended"));
        }
        self.observed_ms = Some(now);
        Ok(())
    }

    fn check_storage(&mut self, reg: &Registry) -> Result<(), RpcError> {
        if reg.storage_failed {
            self.clear();
            return Err(auth("mobile storage failed; sessions ended"));
        }
        Ok(())
    }

    pub fn count(&self, device: &str) -> usize {
        self.map.values().filter(|s| s.device == device).count()
    }

    pub(crate) fn issue(
        &mut self,
        reg: &Registry,
        device: &str,
        boot: &wire::Id,
        now: u64,
    ) -> Result<wire::Session, wire::Error> {
        let bearer = random_secret()?;
        let csrf = random_secret()?;
        let expires_ms = now
            .checked_add(SESSION_ABSOLUTE_MS)
            .ok_or_else(unauthenticated)?;
        let scopes = policy_scopes(reg, device)?;
        let epoch = reg.device(device).ok_or_else(unauthenticated)?.auth_epoch;
        let device_id = wire::Id::new(device).map_err(|_| unauthenticated())?;
        let id = self.open(reg, device, now).map_err(|_| unauthenticated())?;
        self.credentials.insert(
            id,
            SessionCredentials {
                bearer: hash(bearer.as_bytes()),
                csrf: hash(csrf.as_bytes()),
                boot: boot.clone(),
                scopes: scopes.clone(),
            },
        );
        Ok(wire::Session {
            device: device_id,
            boot: boot.clone(),
            auth_epoch: epoch,
            bearer: secret_value(bearer).expect("hex"),
            csrf: secret_value(csrf).expect("hex"),
            expires_ms,
            scopes,
        })
    }

    pub(crate) fn authenticate(
        &mut self,
        reg: &Registry,
        credentials: super::auth::Credentials<'_>,
        boot: &wire::Id,
        now: u64,
    ) -> Result<super::auth::AuthenticatedDevice, wire::Error> {
        if !valid_secret(credentials.bearer) || !valid_secret(credentials.csrf) {
            return Err(unauthenticated());
        }
        let bearer = hash(credentials.bearer.as_bytes());
        let csrf = hash(credentials.csrf.as_bytes());
        let id = self
            .credentials
            .iter()
            .find(|(_, c)| equal(&c.bearer, &bearer) & equal(&c.csrf, &csrf))
            .map(|(id, _)| id.clone())
            .ok_or_else(unauthenticated)?;
        let device = self
            .check(reg, &id, now, false)
            .map_err(|_| unauthenticated())?;
        let c = self.credentials.get(&id).ok_or_else(unauthenticated)?;
        if &c.boot != boot {
            return Err(unauthenticated());
        }
        let scopes = Self::effective_scopes(reg, &device, &c.scopes)?;
        let epoch = self.map[&id].epoch;
        Ok(super::auth::AuthenticatedDevice::new(
            device,
            id,
            boot.clone(),
            epoch,
            scopes,
        ))
    }

    fn effective_scopes(
        reg: &Registry,
        device: &str,
        snapshot: &wire::List<wire::Scope, 20>,
    ) -> Result<wire::List<wire::Scope, 20>, wire::Error> {
        let current = policy_scopes(reg, device)?;
        wire::List::new(
            snapshot
                .as_slice()
                .iter()
                .filter_map(|s| {
                    let policy = current.as_slice().iter().find(|p| p.task == s.task)?;
                    let caps: Vec<_> = s
                        .caps
                        .as_slice()
                        .iter()
                        .copied()
                        .filter(|c| policy.caps.as_slice().contains(c))
                        .collect();
                    caps.contains(&MobileCap::Status).then(|| wire::Scope {
                        task: s.task.clone(),
                        caps: wire::List::new(caps).expect("subset"),
                    })
                })
                .collect(),
        )
        .map_err(|_| unauthenticated())
    }

    pub(crate) fn recheck(
        &mut self,
        reg: &Registry,
        ctx: &super::auth::AuthenticatedDevice,
        boot: &wire::Id,
        now: u64,
    ) -> Result<(), wire::Error> {
        let device = self
            .check(reg, ctx.session(), now, false)
            .map_err(|_| unauthenticated())?;
        let c = self
            .credentials
            .get(ctx.session())
            .ok_or_else(unauthenticated)?;
        let effective = Self::effective_scopes(reg, &device, &c.scopes)?;
        if ctx.boot() != boot.as_str()
            || c.boot != *boot
            || ctx.device() != device
            || ctx.auth_epoch() != self.map[ctx.session()].epoch
            || ctx.scopes() != effective.as_slice()
        {
            return Err(unauthenticated());
        }
        Ok(())
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

    #[test]
    fn effective_scope_requires_status_and_never_adds_tasks() {
        let mut r = reg();
        r.set_policy(
            "m1",
            "t1",
            &[MobileCap::Status, MobileCap::Approve],
            &[],
            false,
        )
        .unwrap();
        let snapshot = policy_scopes(&r, "m1").unwrap();
        r.set_policy("m1", "t2", &[MobileCap::Status], &[], false)
            .unwrap();
        assert_eq!(
            Sessions::effective_scopes(&r, "m1", &snapshot).unwrap(),
            snapshot
        );
        r.devices[0]
            .policy
            .get_mut("t1")
            .unwrap()
            .remove(&MobileCap::Status);
        assert!(
            Sessions::effective_scopes(&r, "m1", &snapshot)
                .unwrap()
                .as_slice()
                .is_empty()
        );
    }

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
