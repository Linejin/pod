//! Local pairing window and claim. No RPC or listener wiring.
use super::{
    Mobile,
    challenge::{Challenges, equal, hash, random_secret, secret_value, valid_secret},
};
use crate::protocol::{MobileCap, mobile_v1 as wire};
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    sync::{Arc, Mutex},
};

/// Secret-bearing local result. Display `code` separately from the QR.
pub struct OpenedWindow {
    pub pairing_id: wire::Id,
    pub secret: wire::Secret,
    pub expires_at: u64,
    pub code: wire::Text<8>,
}

/// Self-claimed metadata, not an approved identity or session.
#[derive(Clone)]
pub struct PendingDevice {
    pub public_key: wire::Text<256>,
    pub name: wire::Text<128>,
    pub scopes: wire::List<wire::Scope, 20>,
    pub ticket_hash: [u8; 32],
    pub expires_ms: u64,
    pub auth_epoch: u64,
}

enum Phase {
    Open,
    Claimed(PendingDevice),
    Approved(PendingDevice, String),
    Closed,
}
struct Window {
    id: wire::Id,
    secret: [u8; 32],
    salt: [u8; 32],
    code: [u8; 32],
    expires: u64,
    code_failures: u8,
    phase: Phase,
}

#[derive(Default)]
struct Failures {
    // At most 20 IPs, 5 timestamps per IP and 20 global timestamps.
    ips: HashMap<IpAddr, VecDeque<u64>>,
    global: VecDeque<u64>,
}
impl Failures {
    fn reserve(&mut self, ip: IpAddr, now: u64) -> Result<(), wire::Error> {
        self.global.retain(|t| now.saturating_sub(*t) < 60_000);
        self.ips.retain(|_, times| {
            times.retain(|t| now.saturating_sub(*t) < 60_000);
            !times.is_empty()
        });
        let local = self
            .ips
            .get(&ip)
            .filter(|v| v.len() >= 5)
            .and_then(|v| v.front());
        let global = (self.global.len() >= 20)
            .then(|| self.global.front())
            .flatten();
        if let Some(until) = local
            .into_iter()
            .chain(global)
            .map(|t| t.saturating_add(60_000))
            .max()
        {
            return Err(wire::Error {
                code: wire::ErrorCode::RateLimited,
                retry_after_ms: Some(until.saturating_sub(now).clamp(1, 60_000) as u32),
            });
        }
        self.global.push_back(now);
        self.ips.entry(ip).or_default().push_back(now);
        Ok(())
    }
    fn success(&mut self, ip: IpAddr) {
        self.global.pop_back();
        if let Some(times) = self.ips.get_mut(&ip) {
            times.pop_back();
        }
        self.ips.retain(|_, times| !times.is_empty());
    }
}

fn normalized(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v) => v.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        _ => ip,
    }
}
fn code_hash(salt: &[u8; 32], code: &str) -> [u8; 32] {
    let mut bytes = salt.to_vec();
    bytes.extend_from_slice(code.as_bytes());
    hash(&bytes)
}
fn new_code() -> Result<String, wire::Error> {
    // Rejection sampling avoids modulo bias across the 100 million codes.
    loop {
        let random = random_secret()?;
        let n = u32::from_str_radix(&random[..8], 16).expect("hex");
        if n < 4_200_000_000 {
            return Ok(format!("{:08}", n % 100_000_000));
        }
    }
}

struct State {
    challenges: Challenges,
    window: Option<Window>,
    failures: Failures,
    last_now: u64,
}
impl State {
    fn tick(&mut self, now: u64) -> bool {
        let backwards = now < self.last_now;
        self.last_now = now;
        if backwards {
            self.challenges.clear();
        }
        if let Some(w) = &mut self.window
            && (backwards || now >= w.expires)
        {
            w.phase = Phase::Closed;
        }
        !backwards
    }
}

/// Retain exactly one instance per daemon (T2 owns wiring). All lifecycle
/// transitions acquire Mobile's lock, then this private state lock. Neither
/// lock is held across network or terminal I/O; approval serializes registry storage.
pub struct Pairing {
    mobile: Arc<Mobile>,
    state: Mutex<State>,
    boot: wire::Id,
}
impl Pairing {
    pub fn new(mobile: Arc<Mobile>, machine: wire::Id, boot: wire::Id) -> Self {
        Self {
            mobile,
            boot: boot.clone(),
            state: Mutex::new(State {
                challenges: Challenges::new(machine, boot, 128),
                window: None,
                failures: Failures::default(),
                last_now: 0,
            }),
        }
    }

    /// Explicit PC action only. Never expose this method as an app operation.
    pub fn open_window(&self) -> Result<OpenedWindow, wire::Error> {
        let _mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        let now = self.mobile.clock.now_ms();
        state.tick(now);
        // Even entropy failure must not leave the previous QR usable.
        state.window = None;
        state.challenges.clear();
        let id = wire::Id::new(random_secret()?).expect("hex");
        let secret = random_secret()?;
        let salt = hash(random_secret()?.as_bytes());
        let code = new_code()?;
        let expires = now.checked_add(120_000).ok_or_else(wire::Error::refusal)?;
        state.window = Some(Window {
            id: id.clone(),
            secret: hash(secret.as_bytes()),
            salt,
            code: code_hash(&salt, &code),
            expires,
            code_failures: 0,
            phase: Phase::Open,
        });
        Ok(OpenedWindow {
            pairing_id: id,
            secret: secret_value(secret).expect("hex"),
            expires_at: expires,
            code: wire::Text::new(code).expect("8 digits"),
        })
    }

    /// Local off discards transient state. Rate budgets survive off/reopen.
    pub fn off(&self) -> Result<(), wire::Error> {
        let _mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        state.window = None;
        state.challenges.clear();
        Ok(())
    }

    pub fn challenge(
        &self,
        purpose: wire::Purpose,
        subject: wire::Id,
        auth_epoch: u64,
    ) -> Result<wire::Challenge, wire::Error> {
        let _mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        let now = self.mobile.clock.now_ms();
        if !state.tick(now) {
            return Err(wire::Error::refusal());
        }
        state.challenges.issue(purpose, subject, auth_epoch, now)
    }

    pub fn pending(&self) -> Result<Option<PendingDevice>, wire::Error> {
        let _mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        state.tick(self.mobile.clock.now_ms());
        Ok(state.window.as_ref().and_then(|w| match &w.phase {
            Phase::Claimed(p) => Some(p.clone()),
            _ => None,
        }))
    }

    pub fn claim(
        &self,
        request: wire::Claim,
        peer_ip: IpAddr,
        csrf_header: &str,
        now: u64,
    ) -> Result<wire::Claimed, wire::Error> {
        let _mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        // Fresh clock under both locks prevents queued requests extending life.
        let now = now.max(self.mobile.clock.now_ms());
        let timely = state.tick(now);
        let ip = normalized(peer_ip);
        // Charge before even hashing attacker input; refund only a winning claim.
        state.failures.reserve(ip, now)?;
        if !timely {
            return Err(wire::Error::refusal());
        }
        let result = Self::claim_locked(&mut state, request, csrf_header, now, &*self.mobile.clock);
        if result.is_ok() {
            state.failures.success(ip);
        }
        result
    }

    fn claim_locked(
        state: &mut State,
        request: wire::Claim,
        csrf: &str,
        now: u64,
        clock: &dyn super::session::Clock,
    ) -> Result<wire::Claimed, wire::Error> {
        let refused = wire::Error::refusal;
        let w = state.window.as_mut().ok_or_else(refused)?;
        if !matches!(w.phase, Phase::Open) || now >= w.expires {
            return Err(refused());
        }
        if request.pairing_id != w.id {
            return Err(refused());
        }
        if !valid_secret(request.secret.as_str())
            || !equal(&w.secret, &hash(request.secret.as_str().as_bytes()))
        {
            return Err(refused());
        }
        let code_valid = request.code.as_str().len() == 8
            && request.code.as_str().bytes().all(|b| b.is_ascii_digit());
        if !equal(&w.code, &code_hash(&w.salt, request.code.as_str())) || !code_valid {
            w.code_failures += 1;
            if w.code_failures >= 5 {
                w.phase = Phase::Closed;
            }
            return Err(refused());
        }
        // Claim has no registered device epoch yet; the fixed epoch is zero.
        let challenge = state.challenges.consume(
            request.nonce.as_str(),
            csrf,
            wire::Purpose::Claim,
            &w.id,
            0,
            now,
        )?;
        verify(&request, &challenge)?;
        let mut tasks = std::collections::HashSet::new();
        for scope in request.scopes.as_slice() {
            let caps = scope.caps.as_slice();
            if scope.task.as_str().is_empty()
                || !tasks.insert(scope.task.as_str())
                || !caps.contains(&MobileCap::Status)
                || caps.iter().collect::<std::collections::HashSet<_>>().len() != caps.len()
            {
                return Err(refused());
            }
        }
        let ticket = random_secret()?;
        let fresh = clock.now_ms();
        if fresh < now || fresh >= w.expires || fresh >= challenge.expires_ms {
            if fresh < now || fresh >= w.expires {
                w.phase = Phase::Closed;
            }
            return Err(refused());
        }
        w.phase = Phase::Claimed(PendingDevice {
            public_key: request.public_key,
            name: request.name,
            scopes: request.scopes,
            ticket_hash: hash(ticket.as_bytes()),
            expires_ms: w.expires,
            auth_epoch: 1,
        });
        Ok(wire::Claimed {
            ticket: secret_value(ticket).expect("hex"),
            expires_ms: w.expires,
        })
    }

    /// Explicit PC choice only; never expose as a mobile HTTP operation.
    pub fn approve(
        &self,
        pairing_id: &wire::Id,
        chosen: &wire::List<wire::Scope, 20>,
    ) -> Result<wire::Id, wire::Error> {
        let mut mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        let now = self.mobile.clock.now_ms();
        state.tick(now);
        let w = state
            .window
            .as_mut()
            .filter(|w| &w.id == pairing_id)
            .ok_or_else(wire::Error::refusal)?;
        let Phase::Claimed(pending) = &w.phase else {
            return Err(wire::Error::refusal());
        };
        let pending = pending.clone();
        // Any failed approval closes this QR, including storage or capacity failure.
        w.phase = Phase::Closed;
        let mut policy = std::collections::BTreeMap::new();
        let mut seen = std::collections::HashSet::new();
        for choice in chosen.as_slice() {
            if !seen.insert(choice.task.as_str()) {
                return Err(wire::Error::refusal());
            }
            if let Some(requested) = pending
                .scopes
                .as_slice()
                .iter()
                .find(|s| s.task == choice.task)
            {
                let caps: std::collections::BTreeSet<_> = requested
                    .caps
                    .as_slice()
                    .iter()
                    .copied()
                    .filter(|c| choice.caps.as_slice().contains(c))
                    .collect();
                if caps.contains(&MobileCap::Status) {
                    policy.insert(choice.task.as_str().into(), caps);
                }
            }
        }
        // The admission timestamp is checked under both locks immediately before storage.
        let admitted = self.mobile.clock.now_ms();
        if admitted < now || admitted >= w.expires {
            return Err(wire::Error::refusal());
        }
        let device = mobile.registry.approve(
            &self.mobile.state_dir,
            pending.name.as_str(),
            pending.public_key.as_str(),
            policy,
            admitted,
        )?;
        let id = wire::Id::new(device.id.clone()).expect("server id");
        w.phase = Phase::Approved(pending, device.id);
        Ok(id)
    }

    pub fn reject(&self, pairing_id: &wire::Id) -> Result<(), wire::Error> {
        let _mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        state.tick(self.mobile.clock.now_ms());
        let w = state
            .window
            .as_mut()
            .filter(|w| &w.id == pairing_id)
            .ok_or_else(wire::Error::refusal)?;
        w.phase = Phase::Closed;
        state.challenges.clear();
        Ok(())
    }

    /// Conflict + retry hint is the defined undecided refusal. T1 may wait
    /// without locks, then retry; this response carries no identity or session.
    pub fn pending_refusal() -> wire::Error {
        wire::Error {
            code: wire::ErrorCode::Conflict,
            retry_after_ms: Some(250),
        }
    }

    pub fn finish_challenge(&self, ticket: &str) -> Result<wire::Challenge, wire::Error> {
        let _mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        let now = self.mobile.clock.now_ms();
        state.tick(now);
        let w = state.window.as_ref().ok_or_else(wire::Error::refusal)?;
        let p = match &w.phase {
            Phase::Claimed(p) | Phase::Approved(p, _) => p,
            _ => return Err(wire::Error::refusal()),
        };
        if !valid_secret(ticket) || !equal(&p.ticket_hash, &hash(ticket.as_bytes())) {
            return Err(wire::Error::refusal());
        }
        let (subject, epoch) = (w.id.clone(), p.auth_epoch);
        state
            .challenges
            .issue(wire::Purpose::Finish, subject, epoch, now)
    }

    pub fn finish(
        &self,
        request: wire::Finish,
        csrf_header: &str,
        now: u64,
    ) -> Result<wire::Session, wire::Error> {
        let mut mobile = self
            .mobile
            .state
            .lock()
            .map_err(|_| wire::Error::refusal())?;
        let mut state = self.state.lock().map_err(|_| wire::Error::refusal())?;
        let fresh = self.mobile.clock.now_ms();
        if fresh < now || !state.tick(fresh) {
            return Err(wire::Error::refusal());
        }
        let w = state.window.as_ref().ok_or_else(wire::Error::refusal)?;
        let (p, device) = match &w.phase {
            Phase::Claimed(p) => (p, None),
            Phase::Approved(p, device) => (p, Some(device.clone())),
            _ => return Err(wire::Error::refusal()),
        };
        if !valid_secret(request.ticket.as_str())
            || !equal(&p.ticket_hash, &hash(request.ticket.as_str().as_bytes()))
        {
            return Err(wire::Error::refusal());
        }
        let (subject, p) = (w.id.clone(), p.clone());
        let Some(device) = device else {
            let challenge = state.challenges.validate(
                request.nonce.as_str(),
                csrf_header,
                wire::Purpose::Finish,
                &subject,
                p.auth_epoch,
                fresh,
            )?;
            if let Err(error) = super::challenge::verify_signature(
                p.public_key.as_str(),
                request.signature.as_str(),
                &wire::finish_preimage(&challenge, &request),
            ) {
                // Failed signatures burn the nonce even while awaiting approval.
                state.challenges.consume(
                    request.nonce.as_str(),
                    csrf_header,
                    wire::Purpose::Finish,
                    &subject,
                    p.auth_epoch,
                    fresh,
                )?;
                return Err(error);
            }
            let checked = self.mobile.clock.now_ms();
            if checked < fresh || checked >= p.expires_ms || checked >= challenge.expires_ms {
                return Err(wire::Error::refusal());
            }
            return Err(Self::pending_refusal());
        };
        mobile
            .registry
            .device(&device)
            .filter(|d| {
                d.active() && d.auth_epoch == p.auth_epoch && d.public_key == p.public_key.as_str()
            })
            .ok_or_else(wire::Error::refusal)?;
        let challenge = state.challenges.consume(
            request.nonce.as_str(),
            csrf_header,
            wire::Purpose::Finish,
            &subject,
            p.auth_epoch,
            fresh,
        )?;
        super::challenge::verify_signature(
            p.public_key.as_str(),
            request.signature.as_str(),
            &wire::finish_preimage(&challenge, &request),
        )?;
        let admitted = self.mobile.clock.now_ms();
        if admitted < fresh || admitted >= p.expires_ms || admitted >= challenge.expires_ms {
            return Err(wire::Error::refusal());
        }
        let super::MobileState {
            registry, sessions, ..
        } = &mut *mobile;
        let session = sessions.issue(registry, &device, &self.boot, admitted)?;
        state.window.as_mut().expect("window locked").phase = Phase::Closed;
        Ok(session)
    }
}

fn verify(request: &wire::Claim, challenge: &wire::Challenge) -> Result<(), wire::Error> {
    #[cfg(feature = "mobile-lan")]
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let key = STANDARD
            .decode(request.public_key.as_str())
            .map_err(|_| wire::Error::refusal())?;
        let signature = STANDARD
            .decode(request.signature.as_str())
            .map_err(|_| wire::Error::refusal())?;
        pod_mobile::crypto::verify_p256_sha256(
            &key,
            &wire::claim_preimage(challenge, request),
            &signature,
        )
        .map_err(|_| wire::Error::refusal())
    }
    #[cfg(not(feature = "mobile-lan"))]
    {
        let _ = (request, challenge);
        Err(wire::Error::refusal())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(feature = "mobile-lan"))]
    #[test]
    fn disabled_crypto_refuses() {
        let request: wire::Claim = serde_json::from_str(r#"{"pairing_id":"p","secret":"s","code":"12345678","nonce":"n","public_key":"k","signature":"s","name":"fixture","scopes":[]}"#).unwrap();
        let challenge: wire::Challenge = serde_json::from_str(r#"{"purpose":"claim","machine":"m","boot":"b","subject":"p","auth_epoch":0,"nonce":"n","csrf":"c","expires_ms":1000}"#).unwrap();
        assert_eq!(verify(&request, &challenge), Err(wire::Error::refusal()));
        assert!(random_secret().is_err());
    }
    #[test]
    fn secret_bearing_types_do_not_implement_debug() {
        // Type inference becomes ambiguous (a compile error) if Debug is added.
        trait Ambiguous<A> {
            fn check() {}
        }
        impl<T: ?Sized> Ambiguous<()> for T {}
        impl<T: ?Sized + std::fmt::Debug> Ambiguous<u8> for T {}
        let _ = <wire::Challenge as Ambiguous<_>>::check;
        let _ = <wire::Claim as Ambiguous<_>>::check;
        let _ = <wire::Claimed as Ambiguous<_>>::check;
        let _ = <OpenedWindow as Ambiguous<_>>::check;
        let _ = <PendingDevice as Ambiguous<_>>::check;
        let _ = <Window as Ambiguous<_>>::check;
        let _ = <Phase as Ambiguous<_>>::check;
        let _ = <State as Ambiguous<_>>::check;
        let _ = <Pairing as Ambiguous<_>>::check;
        let _ = <Challenges as Ambiguous<_>>::check;
    }
    #[test]
    fn bounded_failure_budgets_and_mapped_ip() {
        let mut limits = Failures::default();
        let ip = normalized("::ffff:192.0.2.1".parse().unwrap());
        assert_eq!(ip, "192.0.2.1".parse::<IpAddr>().unwrap());
        for _ in 0..5 {
            limits.reserve(ip, 10).unwrap();
        }
        assert_eq!(
            limits.reserve(ip, 11).unwrap_err().retry_after_ms,
            Some(59_999)
        );
        for i in 2..=16 {
            limits.reserve(IpAddr::from([192, 0, 2, i]), 10).unwrap();
        }
        assert_eq!(
            limits
                .reserve(IpAddr::from([192, 0, 2, 99]), 11)
                .unwrap_err()
                .code,
            wire::ErrorCode::RateLimited
        );
        assert_eq!(limits.global.len(), 20);
        assert!(limits.ips.len() <= 20);
        limits.reserve(ip, 60_010).unwrap();
        assert_eq!(limits.global.len(), 1);
        limits.success(ip);
        assert!(limits.ips.is_empty());
    }
    #[test]
    fn fixed_comparisons_and_encoding() {
        assert!(equal(&hash(b"a"), &hash(b"a")));
        assert!(!equal(&hash(b"a"), &hash(b"b")));
        assert!(valid_secret(&"a".repeat(64)));
        assert!(!valid_secret(&"A".repeat(64)));
        assert_ne!(
            code_hash(&[0; 32], "12345678"),
            code_hash(&[1; 32], "12345678")
        );
    }
}
