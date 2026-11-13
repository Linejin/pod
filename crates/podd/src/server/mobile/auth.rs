//! P2 owns bearer/CSRF authentication and per-request revalidation.
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
#[derive(Default)]
pub struct Auth;
impl Auth {
    pub fn authenticate(
        &self,
        _credentials: Credentials<'_>,
    ) -> Result<AuthenticatedDevice, Error> {
        Err(Error::refusal())
    }
    /// Called after lock acquisition and again before returning long-poll data.
    /// This check must not refresh idle time on entry.
    pub fn recheck(&self, _ctx: &AuthenticatedDevice, _now_ms: u64) -> Result<(), Error> {
        Err(Error::refusal())
    }
    pub fn open(&self, _request: wire::SessionOpen) -> Result<wire::Session, Error> {
        Err(Error::refusal())
    }
    pub fn revoke(&self, _request: wire::Revoke) -> Result<(), Error> {
        Err(Error::refusal())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_are_not_credentials() {
        for token in ["", "s1-1", "m1", "opaque-token"] {
            assert_eq!(
                Auth.authenticate(Credentials {
                    bearer: token,
                    csrf: "csrf"
                })
                .unwrap_err(),
                Error::refusal()
            );
        }
        let ctx = AuthenticatedDevice {
            device: wire::Id::new("m1").unwrap(),
            session: wire::Id::new("s1").unwrap(),
            boot: wire::Id::new("b1").unwrap(),
            auth_epoch: 1,
            scopes: wire::List::new(vec![]).unwrap(),
        };
        assert_eq!(Auth.recheck(&ctx, 1), Err(Error::refusal()));
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
        let open = serde_json::from_str(
            r#"{"device":"m1","auth_epoch":1,"nonce":"nonce","signature":"sig"}"#,
        )
        .unwrap();
        assert!(matches!(Auth.open(open), Err(e) if e == Error::refusal()));
        assert!(
            Auth.revoke(wire::Revoke {
                device: wire::Id::new("m1").unwrap(),
                auth_epoch: 1
            })
            .is_err()
        );
    }
}
