//! P3 owns persistent revision/provenance, read receipts and atomic CAS/audit.
use super::{auth::AuthenticatedDevice, session::Clock};
use crate::protocol::{GateInfo, mobile_v1 as wire};
use serde::{Deserialize, Serialize};

/// Stored alongside GateInfo by P3, never added to its local wire format.
/// Legacy records migrate to revision zero / Unknown; they cannot be approved
/// over mobile until P3 commits a fresh revision and complete detail receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GateMetadata {
    pub revision: u64,
    pub origin: wire::Origin,
}
impl Default for GateMetadata {
    fn default() -> Self {
        Self {
            revision: 0,
            origin: wire::Origin::Unknown,
        }
    }
}

/// SHA-256 fingerprint, also available in default-off builds. The same `sha2`
/// the M1 message path already uses (no unsafe, no Windows-only call), so the
/// vectors hold on the Linux daemon too.
pub fn fingerprint(bytes: &[u8]) -> Result<wire::Fingerprint, wire::Error> {
    use sha2::{Digest, Sha256};
    let out = Sha256::digest(bytes);
    wire::Fingerprint::new(out.iter().map(|b| format!("{b:02x}")).collect::<String>())
        .map_err(|_| wire::Error::refusal())
}
pub fn scope_fingerprint(scopes: &[wire::Scope]) -> Result<wire::Fingerprint, wire::Error> {
    fingerprint(&wire::scope_preimage(scopes))
}
pub fn decision_fingerprint(p: &wire::Decide) -> Result<wire::Fingerprint, wire::Error> {
    fingerprint(&wire::decision_preimage(p))
}

/// Full detail only. Oversized data is refused, never silently truncated into
/// an approvable projection. T1 represents this error as incomplete detail.
pub fn project_gate(
    machine: wire::Id,
    boot: wire::Id,
    gate: &GateInfo,
    metadata: &GateMetadata,
) -> Result<wire::Gate, wire::Error> {
    let too_large = || wire::Error {
        code: wire::ErrorCode::TooLarge,
        retry_after_ms: None,
    };
    // Check source budgets before allocating/cloning a projection.
    if gate.id.len() > 128
        || gate.task.as_ref().is_some_and(|t| t.len() > 128)
        || gate.title.len() > 4096
        || gate.message.len() > 49152
        || gate.options.len() > 20
        || gate.options.iter().any(|s| s.len() > 4096)
    {
        return Err(too_large());
    }
    let mut result = wire::Gate {
        machine,
        boot,
        task: wire::Id::new(gate.task.as_ref().ok_or_else(wire::Error::refusal)?.clone())
            .map_err(|_| too_large())?,
        gate: wire::Id::new(gate.id.clone()).map_err(|_| too_large())?,
        revision: metadata.revision,
        issued_ms: gate.created_ms,
        expires_ms: gate.expires_ms,
        origin: metadata.origin.clone(),
        title: wire::Text::new(gate.title.clone()).map_err(|_| too_large())?,
        body: wire::Text::new(gate.message.clone()).map_err(|_| too_large())?,
        options: wire::List::new(
            gate.options
                .iter()
                .map(|s| wire::Text::new(s.clone()).map_err(|_| too_large()))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .map_err(|_| too_large())?,
        state: gate.state,
        complete: metadata.revision > 0,
        body_fingerprint: wire::Fingerprint::new("").expect("empty bounded string"),
    };
    result.body_fingerprint = fingerprint(&wire::gate_preimage(&result))?;
    wire::response_bytes(&result)?;
    Ok(result)
}

/// P3 implements the synchronous atomic boundary. Implementations acquire
/// mobile state THEN orch store, read clock only AFTER both locks, recheck
/// auth/scope/receipt/revision, and read again immediately before commit.
/// No await, network or terminal I/O may run while these locks are held.
/// Replay lookup precedes pending/revision CAS; commit includes audit + result.
pub trait ApprovalBoundary {
    fn decide(
        &self,
        ctx: &AuthenticatedDevice,
        request: &wire::Decide,
        clock: &dyn Clock,
    ) -> Result<wire::Gate, wire::Error>;
}
#[derive(Default)]
pub struct Approval;
impl ApprovalBoundary for Approval {
    fn decide(
        &self,
        _ctx: &AuthenticatedDevice,
        _request: &wire::Decide,
        _clock: &dyn Clock,
    ) -> Result<wire::Gate, wire::Error> {
        Err(wire::Error::refusal())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn platform_hash_vector() {
        assert_eq!(
            fingerprint(b"abc").unwrap().as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
    #[test]
    fn projection_preserves_local_wire_and_binds_body() {
        let gate: GateInfo = serde_json::from_value(serde_json::json!({"id":"g1","task":"t1","title":"title","message":"body","state":"pending","created_ms":1})).unwrap();
        let before = serde_json::to_value(&gate).unwrap();
        let project = |g: &GateInfo, meta: &GateMetadata| {
            project_gate(
                wire::Id::new("m1").unwrap(),
                wire::Id::new("b1").unwrap(),
                g,
                meta,
            )
            .unwrap()
        };
        assert!(!project(&gate, &GateMetadata::default()).complete);
        let meta = GateMetadata {
            revision: 1,
            origin: wire::Origin::LocalUser,
        };
        let p = project(&gate, &meta);
        assert!(p.complete);
        assert_eq!(serde_json::to_value(&gate).unwrap(), before);
        let mut changed = gate.clone();
        changed.message.push('!');
        assert_ne!(
            p.body_fingerprint,
            project(&changed, &meta).body_fingerprint
        );
        assert_ne!(
            p.body_fingerprint,
            project(
                &gate,
                &GateMetadata {
                    revision: 2,
                    ..meta
                }
            )
            .body_fingerprint
        );
        changed.message = "x".repeat(49153);
        assert_eq!(
            project_gate(
                wire::Id::new("m1").unwrap(),
                wire::Id::new("b1").unwrap(),
                &changed,
                &GateMetadata::default()
            )
            .unwrap_err()
            .code,
            wire::ErrorCode::TooLarge
        );
    }
}
