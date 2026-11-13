//! Bounded, hash-only, single-use server challenges. Caller serializes access.
use crate::protocol::mobile_v1 as wire;
use sha2::{Digest, Sha256};

pub(crate) fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

// Fixed-length digest comparison, without a data-dependent early exit.
pub(crate) fn equal(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0;
    for (a, b) in a.iter().zip(b) {
        diff |= std::hint::black_box(a ^ b);
    }
    std::hint::black_box(diff) == 0
}

pub(crate) fn valid_secret(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub(crate) fn random_secret() -> Result<String, wire::Error> {
    #[cfg(feature = "mobile-lan")]
    {
        let bytes = pod_mobile::crypto::random_256().map_err(|_| wire::Error::refusal())?;
        Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
    }
    #[cfg(not(feature = "mobile-lan"))]
    Err(wire::Error::refusal())
}

struct Entry {
    nonce: [u8; 32],
    csrf: [u8; 32],
    purpose: wire::Purpose,
    subject: wire::Id,
    epoch: u64,
    expires: u64,
}

/// Machine and boot are immutable for the lifetime of this table.
/// No Debug implementation: neither live values nor digests are logged.
pub struct Challenges {
    machine: wire::Id,
    boot: wire::Id,
    capacity: usize,
    entries: Vec<Entry>,
    observed_ms: Option<u64>,
}

impl Challenges {
    pub fn new(machine: wire::Id, boot: wire::Id, capacity: usize) -> Self {
        Self {
            machine,
            boot,
            capacity,
            entries: Vec::new(),
            observed_ms: None,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn observe(&mut self, now: u64) -> Result<(), wire::Error> {
        let backwards = self.observed_ms.is_some_and(|last| now < last);
        self.observed_ms = Some(now);
        if backwards {
            self.clear();
            return Err(wire::Error::refusal());
        }
        Ok(())
    }

    pub fn issue(
        &mut self,
        purpose: wire::Purpose,
        subject: wire::Id,
        epoch: u64,
        now: u64,
    ) -> Result<wire::Challenge, wire::Error> {
        self.observe(now)?;
        // In insertion/expiry order; never evict an unexpired challenge.
        self.entries.retain(|e| now < e.expires);
        if self.entries.len() >= self.capacity {
            return Err(wire::Error::refusal());
        }
        let nonce = random_secret()?;
        let csrf = random_secret()?;
        let expires = now.checked_add(60_000).ok_or_else(wire::Error::refusal)?;
        self.entries.push(Entry {
            nonce: hash(nonce.as_bytes()),
            csrf: hash(csrf.as_bytes()),
            purpose: purpose.clone(),
            subject: subject.clone(),
            epoch,
            expires,
        });
        Ok(wire::Challenge {
            purpose,
            machine: self.machine.clone(),
            boot: self.boot.clone(),
            subject,
            auth_epoch: epoch,
            nonce: secret_value(nonce).expect("hex"),
            csrf: secret_value(csrf).expect("hex"),
            expires_ms: expires,
        })
    }

    /// Reconstruct the signed DTO from supplied secrets only after all bindings
    /// match. Removal happens before signature verification by the caller.
    pub fn consume(
        &mut self,
        nonce: &str,
        csrf: &str,
        purpose: wire::Purpose,
        subject: &wire::Id,
        epoch: u64,
        now: u64,
    ) -> Result<wire::Challenge, wire::Error> {
        let challenge = self.validate(nonce, csrf, purpose, subject, epoch, now)?;
        let digest = hash(nonce.as_bytes());
        self.entries.retain(|e| !equal(&e.nonce, &digest));
        Ok(challenge)
    }

    /// Pending finish may validate while preserving its nonce for the final
    /// decision. The caller holds the same lock through validation and use.
    pub(crate) fn validate(
        &mut self,
        nonce: &str,
        csrf: &str,
        purpose: wire::Purpose,
        subject: &wire::Id,
        epoch: u64,
        now: u64,
    ) -> Result<wire::Challenge, wire::Error> {
        self.observe(now)?;
        if !valid_secret(nonce) || !valid_secret(csrf) {
            return Err(wire::Error::refusal());
        }
        let digest = hash(nonce.as_bytes());
        let index = self
            .entries
            .iter()
            .position(|e| equal(&e.nonce, &digest))
            .ok_or_else(wire::Error::refusal)?;
        let e = &self.entries[index];
        if !equal(&e.csrf, &hash(csrf.as_bytes()))
            || e.purpose != purpose
            || &e.subject != subject
            || e.epoch != epoch
            || now >= e.expires
        {
            return Err(wire::Error::refusal());
        }
        Ok(wire::Challenge {
            purpose,
            machine: self.machine.clone(),
            boot: self.boot.clone(),
            subject: e.subject.clone(),
            auth_epoch: epoch,
            nonce: secret_value(nonce).expect("hex"),
            csrf: secret_value(csrf).expect("hex"),
            expires_ms: e.expires,
        })
    }
}

pub(crate) fn secret_value(s: impl Into<String>) -> Result<wire::Secret, serde_json::Error> {
    serde_json::from_value(serde_json::Value::String(s.into()))
}

pub(crate) fn verify_signature(
    key: &str,
    signature: &str,
    preimage: &[u8],
) -> Result<(), wire::Error> {
    #[cfg(feature = "mobile-lan")]
    {
        use base64::{Engine, engine::general_purpose::STANDARD};
        let key = STANDARD.decode(key).map_err(|_| wire::Error::refusal())?;
        let signature = STANDARD
            .decode(signature)
            .map_err(|_| wire::Error::refusal())?;
        pod_mobile::crypto::verify_p256_sha256(&key, preimage, &signature)
            .map_err(|_| wire::Error::refusal())
    }
    #[cfg(not(feature = "mobile-lan"))]
    {
        let _ = (key, signature, preimage);
        Err(wire::Error::refusal())
    }
}

#[cfg(all(test, feature = "mobile-lan"))]
mod tests {
    use super::super::session::{Clock, FakeClock};
    use super::*;
    #[test]
    fn bindings_expiry_capacity_and_single_use() {
        let clock = FakeClock::new(100);
        let id = wire::Id::new("p").unwrap();
        let mut table = Challenges::new(id.clone(), id.clone(), 1);
        let c = table
            .issue(wire::Purpose::Claim, id.clone(), 2, clock.now_ms())
            .unwrap();
        assert!(
            table
                .issue(wire::Purpose::Claim, id.clone(), 2, clock.now_ms())
                .is_err()
        );
        for (csrf, purpose, subject, epoch) in [
            ("0".repeat(64), wire::Purpose::Claim, id.clone(), 2),
            (c.csrf.as_str().into(), wire::Purpose::Finish, id.clone(), 2),
            (
                c.csrf.as_str().into(),
                wire::Purpose::Claim,
                wire::Id::new("other").unwrap(),
                2,
            ),
            (c.csrf.as_str().into(), wire::Purpose::Claim, id.clone(), 3),
        ] {
            assert!(
                table
                    .consume(
                        c.nonce.as_str(),
                        &csrf,
                        purpose,
                        &subject,
                        epoch,
                        clock.now_ms()
                    )
                    .is_err()
            );
        }
        let rebuilt = table
            .consume(
                c.nonce.as_str(),
                c.csrf.as_str(),
                wire::Purpose::Claim,
                &id,
                2,
                clock.now_ms(),
            )
            .unwrap();
        assert!(rebuilt == c);
        assert!(
            table
                .consume(
                    c.nonce.as_str(),
                    c.csrf.as_str(),
                    wire::Purpose::Claim,
                    &id,
                    2,
                    clock.now_ms()
                )
                .is_err()
        );
        let c = table
            .issue(wire::Purpose::Session, id.clone(), 2, clock.now_ms())
            .unwrap();
        clock.advance(60_000);
        assert!(
            table
                .consume(
                    c.nonce.as_str(),
                    c.csrf.as_str(),
                    wire::Purpose::Session,
                    &id,
                    2,
                    clock.now_ms()
                )
                .is_err()
        );
        assert!(
            table
                .issue(wire::Purpose::Finish, id, 2, clock.now_ms())
                .is_ok()
        );
    }
}

pub(crate) fn unauthenticated() -> wire::Error {
    wire::Error {
        code: wire::ErrorCode::Unauthenticated,
        retry_after_ms: None,
    }
}
