//! P2 owns bearer/CSRF authentication and per-request revalidation.
use super::challenge::unauthenticated;
use crate::protocol::mobile_v1::{self as wire, Error};

/// Server-derived identity. No Deserialize or public constructor: a request
/// body, local session id or device id cannot manufacture this context.
#[derive(Debug)]
pub struct AuthenticatedDevice {
    device: wire::Id,
    session: wire::Id,
    boot: wire::Id,
    auth_epoch: u64,
    scopes: wire::List<wire::Scope, 20>,
}
impl AuthenticatedDevice {
    pub(crate) fn new(
        device: String,
        session: String,
        boot: wire::Id,
        auth_epoch: u64,
        scopes: wire::List<wire::Scope, 20>,
    ) -> Self {
        Self {
            device: wire::Id::new(device).expect("server id"),
            session: wire::Id::new(session).expect("server id"),
            boot,
            auth_epoch,
            scopes,
        }
    }
    pub fn device(&self) -> &str {
        self.device.as_str()
    }
    pub fn session(&self) -> &str {
        self.session.as_str()
    }
    pub fn boot(&self) -> &str {
        self.boot.as_str()
    }
    pub fn auth_epoch(&self) -> u64 {
        self.auth_epoch
    }
    pub fn scopes(&self) -> &[wire::Scope] {
        self.scopes.as_slice()
    }
}

/// Header bytes are borrowed and are never Debug/logged by this seam.
pub struct Credentials<'a> {
    pub bearer: &'a str,
    pub csrf: &'a str,
}
/// One instance per daemon, sharing the Mobile session table with Pairing.
pub struct Auth {
    mobile: std::sync::Arc<super::Mobile>,
    challenges: std::sync::Mutex<super::challenge::Challenges>,
    boot: wire::Id,
}
impl Auth {
    /// T1 invokes on off or uncertain resume, alongside Pairing::off.
    pub fn off(&self) -> Result<(), Error> {
        let mut state = self.mobile.state.lock().map_err(|_| unauthenticated())?;
        let mut challenges = self.challenges.lock().map_err(|_| unauthenticated())?;
        state.sessions.clear();
        challenges.clear();
        Ok(())
    }

    pub fn new(mobile: std::sync::Arc<super::Mobile>, machine: wire::Id, boot: wire::Id) -> Self {
        Self {
            mobile,
            challenges: std::sync::Mutex::new(super::challenge::Challenges::new(
                machine,
                boot.clone(),
                128,
            )),
            boot,
        }
    }

    pub fn authenticate(&self, credentials: Credentials<'_>) -> Result<AuthenticatedDevice, Error> {
        let mut state = self.mobile.state.lock().map_err(|_| unauthenticated())?;
        let now = self.mobile.clock.now_ms();
        let super::MobileState {
            registry, sessions, ..
        } = &mut *state;
        sessions.authenticate(registry, credentials, &self.boot, now)
    }

    /// Entry and post-wait validation never refresh idle time.
    pub fn recheck(&self, ctx: &AuthenticatedDevice, now: u64) -> Result<(), Error> {
        let mut state = self.mobile.state.lock().map_err(|_| unauthenticated())?;
        let fresh = self.mobile.clock.now_ms();
        if fresh < now {
            state.sessions.clear();
            return Err(unauthenticated());
        }
        let super::MobileState {
            registry, sessions, ..
        } = &mut *state;
        sessions.recheck(registry, ctx, &self.boot, fresh)
    }

    /// Bootstrap subject and epoch come only from the active registry record.
    pub fn challenge(&self, device: &wire::Id) -> Result<wire::Challenge, Error> {
        let mut state = self.mobile.state.lock().map_err(|_| unauthenticated())?;
        let mut challenges = self.challenges.lock().map_err(|_| unauthenticated())?;
        let now = self.mobile.clock.now_ms();
        if state.sessions.observe(now).is_err() {
            challenges.clear();
            return Err(unauthenticated());
        }
        if state.registry.storage_failed {
            return Err(unauthenticated());
        }
        let d = state
            .registry
            .device(device.as_str())
            .filter(|d| d.active())
            .ok_or_else(unauthenticated)?;
        challenges.issue(wire::Purpose::Session, device.clone(), d.auth_epoch, now)
    }

    pub fn open(
        &self,
        request: wire::SessionOpen,
        csrf_header: &str,
        now: u64,
    ) -> Result<wire::Session, Error> {
        let mut state = self.mobile.state.lock().map_err(|_| unauthenticated())?;
        let mut challenges = self.challenges.lock().map_err(|_| unauthenticated())?;
        let fresh = self.mobile.clock.now_ms();
        if fresh < now {
            state.sessions.clear();
            challenges.clear();
            return Err(unauthenticated());
        }
        if state.sessions.observe(fresh).is_err() {
            challenges.clear();
            return Err(unauthenticated());
        }
        let d = state
            .registry
            .device(request.device.as_str())
            .filter(|d| d.active())
            .ok_or_else(unauthenticated)?;
        if request.auth_epoch != d.auth_epoch {
            return Err(unauthenticated());
        }
        let challenge = challenges
            .consume(
                request.nonce.as_str(),
                csrf_header,
                wire::Purpose::Session,
                &request.device,
                d.auth_epoch,
                fresh,
            )
            .map_err(|_| unauthenticated())?;
        super::challenge::verify_signature(
            &d.public_key,
            request.signature.as_str(),
            &wire::session_preimage(&challenge, &request),
        )
        .map_err(|_| unauthenticated())?;
        let admitted = self.mobile.clock.now_ms();
        if admitted < fresh || admitted >= challenge.expires_ms {
            return Err(unauthenticated());
        }
        let super::MobileState {
            registry, sessions, ..
        } = &mut *state;
        sessions.issue(registry, request.device.as_str(), &self.boot, admitted)
    }

    /// Durable revoke/re-key is the following card, P2c.
    pub fn revoke(&self, _request: wire::Revoke) -> Result<(), Error> {
        Err(unauthenticated())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_missing_credentials_never_authenticate() {
        let clock = super::super::session::FakeClock::new(1);
        let dir = std::env::temp_dir().join(format!("pod-auth-unit-{}", std::process::id()));
        let mobile = std::sync::Arc::new(super::super::Mobile::load(&dir, clock));
        let auth = Auth::new(
            mobile,
            wire::Id::new("machine").unwrap(),
            wire::Id::new("boot").unwrap(),
        );
        for token in ["", "s1-1", "m1", "opaque-token"] {
            assert_eq!(
                auth.authenticate(Credentials {
                    bearer: token,
                    csrf: "csrf"
                })
                .unwrap_err(),
                unauthenticated()
            );
        }
        assert!(auth.challenge(&wire::Id::new("m1").unwrap()).is_err());
        let ctx = AuthenticatedDevice::new(
            "m1".into(),
            "s1-1".into(),
            wire::Id::new("boot").unwrap(),
            1,
            wire::List::new(vec![]).unwrap(),
        );
        assert_eq!(auth.recheck(&ctx, 1), Err(unauthenticated()));
        let call = wire::parse_call(br#"{"op":"tasks","params":{}}"#).unwrap();
        assert_eq!(
            super::super::listener::Listener.call(&ctx, call),
            Err(Error::refusal())
        );
        let request: wire::Decide = serde_json::from_str(r#"{"machine":"m1","task":"t1","gate":"g1","revision":1,"operation_id":"o1","decision":"approve","choice":null}"#).unwrap();
        use super::super::approval::ApprovalBoundary;
        assert_eq!(
            super::super::approval::Approval.decide(
                &ctx,
                &request,
                super::super::session::FakeClock::new(1).as_ref()
            ),
            Err(Error::refusal())
        );
    }
}
