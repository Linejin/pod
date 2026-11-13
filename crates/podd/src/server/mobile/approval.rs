//! P3 owns persistent revision/provenance, read receipts and atomic CAS/audit.
use super::{auth::AuthenticatedDevice, session::Clock};
use crate::protocol::{GateInfo, mobile_v1 as wire};
use crate::server::{
    gates::{self, Decide},
    orch_store::{GateActor, MobileDecisionRecord, Store},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    path::Path,
    sync::Mutex,
};

const RECEIPT_MS: u64 = 30_000;
const RECEIPTS_PER_SESSION: usize = 16;

#[derive(Debug)]
struct Receipt {
    boot: String,
    task: String,
    gate: String,
    revision: u64,
    at: u64,
}

/// T1 calls `record_detail` only after constructing a complete task-detail
/// response. Page/list handlers must never call it. Kept under mobile state.
#[derive(Debug, Default)]
pub struct ReadReceipts(BTreeMap<String, VecDeque<Receipt>>);

impl ReadReceipts {
    pub fn record_detail(&mut self, session: &str, gate: &wire::Gate, clock: &dyn Clock) {
        let now = clock.now_ms();
        self.prune(now);
        if !gate.complete || gate.revision == 0 {
            return;
        }
        let receipts = self.0.entry(session.to_owned()).or_default();
        receipts.retain(|r| {
            !(r.boot == gate.boot.as_str()
                && r.task == gate.task.as_str()
                && r.gate == gate.gate.as_str())
        });
        if receipts.len() == RECEIPTS_PER_SESSION {
            receipts.pop_front();
        }
        receipts.push_back(Receipt {
            boot: gate.boot.as_str().into(),
            task: gate.task.as_str().into(),
            gate: gate.gate.as_str().into(),
            revision: gate.revision,
            at: now,
        });
    }

    pub fn forget_session(&mut self, session: &str) {
        self.0.remove(session);
    }

    /// Expiry is strict: an age of exactly 30 seconds is already stale.
    pub fn matches(&mut self, session: &str, boot: &str, request: &wire::Decide, now: u64) -> bool {
        self.prune(now);
        self.0.get(session).is_some_and(|rs| {
            rs.iter().any(|r| {
                r.boot == boot
                    && r.task == request.task.as_str()
                    && r.gate == request.gate.as_str()
                    && r.revision == request.revision
            })
        })
    }

    fn prune(&mut self, now: u64) {
        self.0.retain(|_, rs| {
            rs.retain(|r| now.checked_sub(r.at).is_some_and(|age| age < RECEIPT_MS));
            !rs.is_empty()
        });
    }
}

/// Server identity passed to revalidation by the authenticated decide entry.
#[derive(Clone, Copy)]
pub struct ApprovalIdentity<'a> {
    pub device: &'a str,
    pub session: &'a str,
    pub boot: &'a str,
    pub auth_epoch: u64,
}

/// P2b implements this on its locked mobile state. Recheck must validate boot,
/// live session, absolute/idle expiry, epoch, current Status+Approve and task
/// ownership using `now`; it must not refresh idle before validating expiry.
/// Implementations perform no I/O and acquire no other locks.
pub trait ApprovalState {
    fn recheck(
        &self,
        identity: ApprovalIdentity<'_>,
        request: &wire::Decide,
        now: u64,
    ) -> Result<(), wire::Error>;
    fn receipts(&mut self) -> &mut ReadReceipts;
    /// Fingerprint of the effective scope authorizing this operation. P2b can
    /// override this to include the complete current grant set.
    fn approval_scope(&self, request: &wire::Decide) -> Result<wire::Fingerprint, wire::Error> {
        scope_fingerprint(&[wire::Scope {
            task: request.task.clone(),
            caps: wire::List::new(vec![
                crate::protocol::MobileCap::Status,
                crate::protocol::MobileCap::Approve,
            ])
            .map_err(|_| wire::Error::refusal())?,
        }])
    }
    /// Reflect activity only after a successful, fully validated transition.
    /// P2b may update its idle timestamp here, never in `recheck`.
    fn committed(&mut self, _identity: ApprovalIdentity<'_>, _now: u64) {}
}

/// Replay is authorized under the same locks, before new-transition checks.
/// T1 must publish returned transitions only after this call releases locks.
pub struct LockedApproval<'a, S> {
    pub mobile: &'a Mutex<S>,
    pub orch: &'a Mutex<Store>,
    pub machine: wire::Id,
    pub state_dir: &'a Path,
}

fn error(code: wire::ErrorCode) -> wire::Error {
    wire::Error {
        code,
        retry_after_ms: None,
    }
}

impl<S: ApprovalState> LockedApproval<'_, S> {
    pub fn decide_identity(
        &self,
        identity: ApprovalIdentity<'_>,
        request: &wire::Decide,
        clock: &dyn Clock,
    ) -> Result<wire::Gate, wire::Error> {
        self.decide_identity_with_save(identity, request, clock, |s| s.save(self.state_dir))
    }

    /// Injectable store writer for deterministic storage/crash testing.
    /// Production callers use `decide` and the durable orch writer.
    pub fn decide_identity_with_save(
        &self,
        identity: ApprovalIdentity<'_>,
        request: &wire::Decide,
        clock: &dyn Clock,
        save: impl FnOnce(&mut Store) -> Result<(), crate::protocol::RpcError>,
    ) -> Result<wire::Gate, wire::Error> {
        self.decide_result_with_save(identity, request, clock, save)
            .map(|r| r.gate)
    }

    fn decide_result_with_save(
        &self,
        identity: ApprovalIdentity<'_>,
        request: &wire::Decide,
        clock: &dyn Clock,
        save: impl FnOnce(&mut Store) -> Result<(), crate::protocol::RpcError>,
    ) -> Result<wire::DecisionResult, wire::Error> {
        let mut mobile = self.mobile.lock().map_err(|_| wire::Error::refusal())?;
        let mut store = self.orch.lock().map_err(|_| wire::Error::refusal())?;
        let now = clock.now_ms();
        mobile.recheck(identity, request, now)?;
        if !store.mobile_recovered {
            return Err(error(wire::ErrorCode::Storage));
        }
        let fingerprint = decision_fingerprint(request)?;
        if let Some(record) = store
            .mobile_decisions
            .iter()
            .find(|r| r.device == identity.device && r.request.operation_id == request.operation_id)
        {
            return if record.fingerprint == fingerprint {
                Ok(wire::DecisionResult {
                    gate: record.result.clone(),
                    replayed: true,
                })
            } else {
                Err(error(wire::ErrorCode::Conflict))
            };
        }
        if request.machine != self.machine {
            return Err(error(wire::ErrorCode::Denied));
        }
        let gate = store
            .gates
            .iter()
            .find(|g| {
                g.id == request.gate.as_str() && g.task.as_deref() == Some(request.task.as_str())
            })
            .ok_or_else(|| error(wire::ErrorCode::Denied))?;
        let metadata = store
            .gate_metadata
            .get(&gate.id)
            .cloned()
            .unwrap_or_default();
        if metadata.revision == 0 || metadata.revision != request.revision {
            return Err(error(wire::ErrorCode::Stale));
        }
        if gate.state != crate::protocol::GateState::Pending
            || gate.expires_ms.is_some_and(|at| at <= now)
        {
            return Err(error(wire::ErrorCode::Stale));
        }
        if !mobile
            .receipts()
            .matches(identity.session, identity.boot, request, now)
        {
            return Err(error(wire::ErrorCode::Stale));
        }
        let boot = wire::Id::new(identity.boot).map_err(|_| wire::Error::refusal())?;
        // Validate the post-transition response budgets before mutating the Gate.
        let mut preview = gate.clone();
        preview.state = match request.decision {
            crate::protocol::GateDecision::Approve => crate::protocol::GateState::Approved,
            crate::protocol::GateDecision::Reject => crate::protocol::GateState::Rejected,
        };
        let preview_meta = GateMetadata {
            revision: metadata
                .revision
                .checked_add(1)
                .ok_or_else(|| error(wire::ErrorCode::Storage))?,
            origin: metadata.origin.clone(),
        };
        if !project_gate(self.machine.clone(), boot.clone(), &preview, &preview_meta)?.complete {
            return Err(error(wire::ErrorCode::Stale));
        }
        let req = Decide {
            id: gate.id.clone(),
            decision: request.decision,
            choice: request.choice.as_ref().map(|s| s.as_str().to_owned()),
            note: None,
            actor: GateActor::Mobile {
                device: identity.device.into(),
            },
            operation_id: None,
            task_scope: Some(request.task.as_str().into()),
        };
        // Refresh checks before preparing the shared Gate transition.
        let admission = clock.now_ms();
        mobile.recheck(identity, request, admission)?;
        if !mobile
            .receipts()
            .matches(identity.session, identity.boot, request, admission)
        {
            return Err(error(wire::ErrorCode::Stale));
        }
        let scope_fingerprint = mobile.approval_scope(request)?;
        gates::decide_in(&mut store, &req, admission, |s| {
            // Final expiry admission: with both locks still held, read a
            // fresh time immediately before the one save. Disk flush duration
            // after this check does not retroactively expire the decision.
            // The successful atomic replace is the commit/linearisation point.
            let at = clock.now_ms();
            mobile.recheck(identity, request, at).map_err(|_| {
                crate::protocol::RpcError::new(
                    crate::protocol::codes::INVALID_PARAMS,
                    "approval expired",
                )
            })?;
            if preview.expires_ms.is_some_and(|expiry| at >= expiry)
                || !mobile
                    .receipts()
                    .matches(identity.session, identity.boot, request, at)
            {
                return Err(crate::protocol::RpcError::new(
                    crate::protocol::codes::INVALID_PARAMS,
                    "approval expired",
                ));
            }
            let result = project_gate(
                self.machine.clone(),
                boot.clone(),
                s.gates
                    .iter()
                    .find(|g| g.id == req.id)
                    .expect("validated gate"),
                &preview_meta,
            )
            .map_err(|_| {
                crate::protocol::RpcError::new(
                    crate::protocol::codes::INTERNAL_ERROR,
                    "invalid result",
                )
            })?;
            s.mobile_decisions.push(MobileDecisionRecord {
                device: identity.device.into(),
                request: request.clone(),
                fingerprint,
                result,
                actor: req.actor.clone(),
                auth_epoch: identity.auth_epoch,
                scope_fingerprint,
                previous: crate::protocol::GateState::Pending,
                new: preview.state,
                at_ms: at,
            });
            if let Err(e) = save(s) {
                s.mobile_decisions.pop();
                return Err(e);
            }
            Ok(())
        })
        .map_err(|e| {
            error(if e.code == crate::protocol::codes::INTERNAL_ERROR {
                wire::ErrorCode::Storage
            } else {
                wire::ErrorCode::Stale
            })
        })?;
        // Consume only after commit, so a failed save retains the receipt.
        if let Some(rs) = mobile.receipts().0.get_mut(identity.session) {
            rs.retain(|r| r.gate != request.gate.as_str() || r.task != request.task.as_str());
        }
        mobile.committed(
            identity,
            store
                .mobile_decisions
                .last()
                .expect("committed result")
                .at_ms,
        );
        Ok(wire::DecisionResult {
            gate: store
                .mobile_decisions
                .last()
                .expect("committed result")
                .result
                .clone(),
            replayed: false,
        })
    }

    /// T1's `task.operation` decision selector. Unknown and another device's
    /// identifier take the same authorization path and return the same error.
    pub fn lookup_identity(
        &self,
        identity: ApprovalIdentity<'_>,
        task: &wire::Id,
        operation: &wire::Operation,
        clock: &dyn Clock,
    ) -> Result<wire::Gate, wire::Error> {
        let mobile = self.mobile.lock().map_err(|_| wire::Error::refusal())?;
        let store = self.orch.lock().map_err(|_| wire::Error::refusal())?;
        let record = store
            .mobile_decisions
            .iter()
            .find(|r| r.device == identity.device && r.request.operation_id == operation.id);
        // Recheck uses task ownership and Status+Approve, even for unknown IDs.
        let probe = wire::Decide {
            machine: self.machine.clone(),
            task: task.clone(),
            gate: wire::Id::new("lookup").map_err(|_| wire::Error::refusal())?,
            revision: 0,
            operation_id: operation.id.clone(),
            decision: crate::protocol::GateDecision::Approve,
            choice: None,
        };
        mobile.recheck(identity, &probe, clock.now_ms())?;
        if !store.mobile_recovered {
            return Err(error(wire::ErrorCode::Storage));
        }
        let record = record
            .filter(|r| r.request.task == *task)
            .ok_or_else(|| error(wire::ErrorCode::UnknownOperation))?;
        if operation.kind != wire::OperationKind::Decision
            || record.fingerprint != operation.body_fingerprint
        {
            return Err(error(wire::ErrorCode::Conflict));
        }
        Ok(record.result.clone())
    }
}

impl<S: ApprovalState> ApprovalBoundary for LockedApproval<'_, S> {
    fn decide_result(
        &self,
        ctx: &AuthenticatedDevice,
        request: &wire::Decide,
        clock: &dyn Clock,
    ) -> Result<wire::DecisionResult, wire::Error> {
        self.decide_result_with_save(
            ApprovalIdentity {
                device: ctx.device(),
                session: ctx.session(),
                boot: ctx.boot(),
                auth_epoch: ctx.auth_epoch(),
            },
            request,
            clock,
            |s| s.save(self.state_dir),
        )
    }
    fn lookup(
        &self,
        ctx: &AuthenticatedDevice,
        task: &wire::Id,
        operation: &wire::Operation,
        clock: &dyn Clock,
    ) -> Result<wire::Gate, wire::Error> {
        self.lookup_identity(
            ApprovalIdentity {
                device: ctx.device(),
                session: ctx.session(),
                boot: ctx.boot(),
                auth_epoch: ctx.auth_epoch(),
            },
            task,
            operation,
            clock,
        )
    }
    fn decide(
        &self,
        ctx: &AuthenticatedDevice,
        request: &wire::Decide,
        clock: &dyn Clock,
    ) -> Result<wire::Gate, wire::Error> {
        self.decide_identity(
            ApprovalIdentity {
                device: ctx.device(),
                session: ctx.session(),
                boot: ctx.boot(),
                auth_epoch: ctx.auth_epoch(),
            },
            request,
            clock,
        )
    }
}

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
        complete: metadata.revision > 0 && metadata.origin != wire::Origin::Unknown,
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
/// Replay precedes CAS; the stored result and success audit share the save.
pub trait ApprovalBoundary {
    /// T1 emits a transition event only when `replayed` is false, after this
    /// method releases both locks. The Gate payload is identical on replay.
    fn decide_result(
        &self,
        ctx: &AuthenticatedDevice,
        request: &wire::Decide,
        clock: &dyn Clock,
    ) -> Result<wire::DecisionResult, wire::Error> {
        self.decide(ctx, request, clock)
            .map(|gate| wire::DecisionResult {
                gate,
                replayed: false,
            })
    }
    fn lookup(
        &self,
        _ctx: &AuthenticatedDevice,
        _task: &wire::Id,
        _operation: &wire::Operation,
        _clock: &dyn Clock,
    ) -> Result<wire::Gate, wire::Error> {
        Err(wire::Error::refusal())
    }
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
    use super::super::session::FakeClock;
    use super::*;
    use std::sync::{Arc, Barrier};

    struct State {
        receipts: ReadReceipts,
        expires: u64,
        epoch: u64,
        allowed: bool,
        last_activity: Option<u64>,
    }
    impl ApprovalState for State {
        fn recheck(
            &self,
            identity: ApprovalIdentity<'_>,
            request: &wire::Decide,
            now: u64,
        ) -> Result<(), wire::Error> {
            if identity.boot != "b1"
                || identity.session != "s1"
                || identity.auth_epoch != self.epoch
                || now >= self.expires
                || !self.allowed
                || request.task.as_str() != "t1"
            {
                return Err(error(wire::ErrorCode::Denied));
            }
            Ok(())
        }
        fn receipts(&mut self) -> &mut ReadReceipts {
            &mut self.receipts
        }
        fn committed(&mut self, _: ApprovalIdentity<'_>, now: u64) {
            self.last_activity = Some(now);
        }
    }
    fn identity() -> ApprovalIdentity<'static> {
        ApprovalIdentity {
            device: "d1",
            session: "s1",
            boot: "b1",
            auth_epoch: 1,
        }
    }
    fn request() -> wire::Decide {
        serde_json::from_value(serde_json::json!({"machine":"m1","task":"t1","gate":"g1","revision":1,"operation_id":"op1","decision":"approve","choice":null})).unwrap()
    }
    fn fixture(
        label: &str,
    ) -> (
        Mutex<State>,
        Mutex<Store>,
        std::path::PathBuf,
        Arc<FakeClock>,
    ) {
        let dir = std::env::temp_dir().join(format!("pod-boundary-{}-{label}", std::process::id()));
        let clock = FakeClock::new(1);
        let mut store = Store::default();
        let gate: GateInfo = serde_json::from_value(serde_json::json!({"id":"g1","task":"t1","title":"title","message":"body","state":"pending","created_ms":1,"expires_ms":10000})).unwrap();
        let meta = GateMetadata {
            revision: 1,
            origin: wire::Origin::LocalUser,
        };
        let projection = project_gate(
            wire::Id::new("m1").unwrap(),
            wire::Id::new("b1").unwrap(),
            &gate,
            &meta,
        )
        .unwrap();
        store.gates.push(gate);
        store.gate_metadata.insert("g1".into(), meta);
        store.save(&dir).unwrap();
        let mut state = State {
            receipts: ReadReceipts::default(),
            expires: 20000,
            epoch: 1,
            allowed: true,
            last_activity: None,
        };
        state
            .receipts
            .record_detail("s1", &projection, clock.as_ref());
        (Mutex::new(state), Mutex::new(store), dir, clock)
    }

    #[test]
    fn replay_marks_event_suppression_without_another_save() {
        let (mobile, orch, dir, clock) = fixture("replay-marker");
        let boundary = LockedApproval {
            mobile: &mobile,
            orch: &orch,
            machine: wire::Id::new("m1").unwrap(),
            state_dir: &dir,
        };
        let first = boundary
            .decide_result_with_save(identity(), &request(), clock.as_ref(), |s| s.save(&dir))
            .unwrap();
        let replay = boundary
            .decide_result_with_save(identity(), &request(), clock.as_ref(), |_| {
                panic!("replay must not save")
            })
            .unwrap();
        assert!(!first.replayed);
        assert!(replay.replayed);
        assert_eq!(first.gate, replay.gate);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn waiting_for_either_lock_cannot_rescue_expired_session_or_gate() {
        for mobile_lock in [true, false] {
            for session_expiry in [true, false] {
                let (mobile, orch, dir, clock) =
                    fixture(&format!("wait-{mobile_lock}-{session_expiry}"));
                if session_expiry {
                    mobile.lock().unwrap().expires = 100;
                } else {
                    orch.lock().unwrap().gates[0].expires_ms = Some(100);
                }
                let boundary = LockedApproval {
                    mobile: &mobile,
                    orch: &orch,
                    machine: wire::Id::new("m1").unwrap(),
                    state_dir: &dir,
                };
                let mobile_guard = mobile_lock.then(|| mobile.lock().unwrap());
                let orch_guard = (!mobile_lock).then(|| orch.lock().unwrap());
                let barrier = Barrier::new(2);
                std::thread::scope(|scope| {
                    let worker = scope.spawn(|| {
                        barrier.wait();
                        boundary.decide_identity(identity(), &request(), clock.as_ref())
                    });
                    barrier.wait();
                    clock.advance(99);
                    drop(mobile_guard);
                    drop(orch_guard);
                    assert!(worker.join().unwrap().is_err());
                });
                assert_eq!(
                    orch.lock().unwrap().gates[0].state,
                    crate::protocol::GateState::Pending
                );
                assert!(orch.lock().unwrap().gate_ops.is_empty());
                assert_eq!(mobile.lock().unwrap().last_activity, None);
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }

    #[test]
    fn stale_revision_receipt_policy_and_epoch_are_refused() {
        for case in ["revision", "receipt", "incomplete", "policy", "epoch"] {
            let (mobile, orch, dir, clock) = fixture(case);
            match case {
                "revision" => {
                    let mut store = orch.lock().unwrap();
                    store.gates[0].message.push('!');
                    store.save(&dir).unwrap();
                }
                "receipt" => {
                    mobile.lock().unwrap().expires = 100000;
                    orch.lock().unwrap().gates[0].expires_ms = None;
                    clock.advance(30000);
                }
                "incomplete" => mobile.lock().unwrap().receipts.forget_session("s1"),
                "policy" => mobile.lock().unwrap().allowed = false,
                "epoch" => mobile.lock().unwrap().epoch = 2,
                _ => unreachable!(),
            }
            let boundary = LockedApproval {
                mobile: &mobile,
                orch: &orch,
                machine: wire::Id::new("m1").unwrap(),
                state_dir: &dir,
            };
            let err = boundary
                .decide_identity(identity(), &request(), clock.as_ref())
                .unwrap_err();
            assert_eq!(
                err.code,
                if ["policy", "epoch"].contains(&case) {
                    wire::ErrorCode::Denied
                } else {
                    wire::ErrorCode::Stale
                }
            );
            assert!(orch.lock().unwrap().gate_ops.is_empty());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn pc_and_phone_commit_exactly_one_transition() {
        let (mobile, orch, dir, clock) = fixture("race");
        let boundary = LockedApproval {
            mobile: &mobile,
            orch: &orch,
            machine: wire::Id::new("m1").unwrap(),
            state_dir: &dir,
        };
        let barrier = Barrier::new(2);
        let successes = std::thread::scope(|scope| {
            let pc = scope.spawn(|| {
                barrier.wait();
                let mut store = orch.lock().unwrap();
                let req = Decide {
                    id: "g1".into(),
                    decision: crate::protocol::GateDecision::Reject,
                    choice: None,
                    note: None,
                    actor: GateActor::Local,
                    operation_id: None,
                    task_scope: None,
                };
                gates::decide_in(&mut store, &req, clock.now_ms(), |s| s.save(&dir)).is_ok()
            });
            barrier.wait();
            let phone = boundary
                .decide_identity(identity(), &request(), clock.as_ref())
                .is_ok();
            usize::from(phone) + usize::from(pc.join().unwrap())
        });
        assert_eq!(successes, 1);
        let store = orch.lock().unwrap();
        assert_eq!(store.gate_ops.len(), 1);
        assert_eq!(store.gate_metadata["g1"].revision, 2);
        assert_eq!(Store::load(&dir).gates[0], store.gates[0]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn clock_is_read_with_both_locks_and_activity_follows_commit() {
        struct LockedClock<'a> {
            mobile: &'a Mutex<State>,
            orch: &'a Mutex<Store>,
        }
        impl Clock for LockedClock<'_> {
            fn now_ms(&self) -> u64 {
                assert!(matches!(
                    self.mobile.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                ));
                assert!(matches!(
                    self.orch.try_lock(),
                    Err(std::sync::TryLockError::WouldBlock)
                ));
                10
            }
        }
        let (mobile, orch, dir, _) = fixture("locked-clock");
        let boundary = LockedApproval {
            mobile: &mobile,
            orch: &orch,
            machine: wire::Id::new("m1").unwrap(),
            state_dir: &dir,
        };
        boundary
            .decide_identity(
                identity(),
                &request(),
                &LockedClock {
                    mobile: &mobile,
                    orch: &orch,
                },
            )
            .unwrap();
        assert_eq!(mobile.lock().unwrap().last_activity, Some(10));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn failed_save_keeps_revision_receipt_and_activity_and_expiry_uses_same_commit() {
        let (mobile, orch, dir, clock) = fixture("save-failure");
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"fixture").unwrap();
        let boundary = LockedApproval {
            mobile: &mobile,
            orch: &orch,
            machine: wire::Id::new("m1").unwrap(),
            state_dir: &blocked,
        };
        assert_eq!(
            boundary
                .decide_identity(identity(), &request(), clock.as_ref())
                .unwrap_err()
                .code,
            wire::ErrorCode::Storage
        );
        assert_eq!(mobile.lock().unwrap().last_activity, None);
        assert!(
            mobile
                .lock()
                .unwrap()
                .receipts
                .matches("s1", "b1", &request(), 1)
        );
        let mut store = orch.lock().unwrap();
        assert_eq!(store.gate_metadata["g1"].revision, 1);
        assert_eq!(store.gates[0].state, crate::protocol::GateState::Pending);
        assert!(store.gate_ops.is_empty());
        let (expired, _) = gates::expire_in(&mut store, 10000, |s| s.save(&dir));
        assert_eq!(expired.len(), 1);
        assert_eq!(store.gate_metadata["g1"].revision, 2);
        assert_eq!(
            Store::load(&dir).gates[0].state,
            crate::protocol::GateState::Expired
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
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
