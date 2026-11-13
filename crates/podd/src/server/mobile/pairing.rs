//! P2 pairing lifecycle. These are not app operations or pipe RPCs.
use crate::protocol::mobile_v1 as wire;
#[derive(Default)]
pub struct Pairing;
impl Pairing {
    pub fn challenge(&self, _purpose: wire::Purpose) -> Result<wire::Challenge, wire::Error> {
        Err(wire::Error::refusal())
    }
    pub fn claim(&self, _request: wire::Claim) -> Result<wire::Claimed, wire::Error> {
        Err(wire::Error::refusal())
    }
    pub fn finish(&self, _request: wire::Finish) -> Result<wire::Session, wire::Error> {
        Err(wire::Error::refusal())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_challenge_is_issued_before_p2() {
        let claim = serde_json::from_str(r#"{"pairing_id":"p1","secret":"secret","code":"12345678","nonce":"nonce","public_key":"key","signature":"sig","name":"fixture","scopes":[]}"#).unwrap();
        assert!(matches!(Pairing.claim(claim), Err(e) if e == wire::Error::refusal()));
        let finish =
            serde_json::from_str(r#"{"ticket":"ticket","nonce":"nonce","signature":"sig"}"#)
                .unwrap();
        assert!(matches!(Pairing.finish(finish), Err(e) if e == wire::Error::refusal()));
        for purpose in [
            wire::Purpose::Claim,
            wire::Purpose::Finish,
            wire::Purpose::Session,
        ] {
            assert_eq!(Pairing.challenge(purpose), Err(wire::Error::refusal()));
        }
    }
}
