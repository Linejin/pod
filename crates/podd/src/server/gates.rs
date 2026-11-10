//! P5 backend: gates, i.e. pending human approvals. Created by the UI/API or
//! by an agent from inside its terminal (`pod gate request`), announced on
//! every `status.subscribe` stream as `gate.changed`, resolved by the human
//! or (v6, `expires_in_s`) by the expiry worker as `expired`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

use super::orch_store::{GateActor, GateOpRecord, Store};
use super::status::{self, HubEvent};
use super::{Daemon, log};
use crate::protocol::*;

const MAX_MESSAGE: usize = 16 * 1024;
/// Longest accepted `expires_in_s` (one year).
const MAX_EXPIRY_S: u64 = 365 * 24 * 3600;
/// The expiry worker re-checks at least this often.
const EXPIRY_POLL: Duration = Duration::from_secs(5);
/// Gate decision records kept in the store (`gate_ops`), newest last.
const MAX_GATE_OPS: usize = 2_000;
/// ...and none older than this (docs/17 §4.6: at least 30 days).
const GATE_OPS_KEEP_MS: u64 = 30 * 24 * 3600 * 1000;

/// One gate decision ([`decide_in`]).
#[derive(Debug, Clone)]
pub(super) struct Decide {
    pub id: String,
    pub decision: GateDecision,
    /// The chosen option: required to approve a gate with options, refused
    /// otherwise (v17, docs/19 §4.5).
    pub choice: Option<String>,
    pub note: Option<String>,
    pub actor: GateActor,
    /// Idempotency key (mobile: always; PC: none).
    pub operation_id: Option<String>,
    /// Only decide gates of this task (mobile scope; checked under the lock).
    pub task_scope: Option<String>,
}

/// Result of [`decide_in`].
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Decided {
    /// This call resolved the gate (and committed it with its record).
    Now(GateInfo),
    /// The same operation was committed before: its gate, unchanged.
    Replayed(GateInfo),
}

fn already_resolved(g: &GateInfo) -> RpcError {
    let mut e = RpcError::new(
        orch_codes::GATE_RESOLVED,
        format!("gate {} is already {:?}", g.id, g.state).to_lowercase(),
    );
    e.data = serde_json::to_value(g).ok();
    e
}

/// The single decision path for every actor (PC `gate.resolve` and the
/// mobile API). Everything is checked and changed under the caller's store
/// lock, so with concurrent deciders (PC, phone, expiry worker) exactly one
/// wins: operation replay/conflict, scope, pending state and expiry at `now`
/// (a gate past its expiry is refused even before the worker marks it), then
/// the transition plus its [`GateOpRecord`] are committed in one `save`. If
/// the save fails, memory is rolled back and the error returned: no decision
/// without its record.
pub(super) fn decide_in(
    st: &mut Store,
    req: &Decide,
    now: u64,
    save: impl FnOnce(&mut Store) -> Result<(), RpcError>,
) -> Result<Decided, RpcError> {
    if let Some(op) = &req.operation_id
        && let Some(prev) = st
            .gate_ops
            .iter()
            .find(|r| r.operation_id.as_deref() == Some(op) && r.actor == req.actor)
    {
        if prev.gate != req.id || prev.decision != req.decision || prev.choice != req.choice {
            return Err(RpcError::new(
                mobile_codes::OPERATION_CONFLICT,
                format!(
                    "operation {op} already {} gate {}",
                    match prev.decision {
                        GateDecision::Approve => "approved",
                        GateDecision::Reject => "rejected",
                    },
                    prev.gate
                ),
            ));
        }
        let g = st
            .gates
            .iter()
            .find(|g| g.id == req.id)
            .cloned()
            .ok_or_else(|| gate_not_found(&req.id))?;
        return Ok(Decided::Replayed(g));
    }
    let Some(i) = st.gates.iter().position(|g| g.id == req.id) else {
        return Err(gate_not_found(&req.id));
    };
    if let Some(task) = &req.task_scope
        && st.gates[i].task.as_ref() != Some(task)
    {
        // Same answer as an unknown gate: do not reveal other tasks' gates.
        return Err(gate_not_found(&req.id));
    }
    let g = &st.gates[i];
    if g.state != GateState::Pending {
        return Err(already_resolved(g));
    }
    if g.expires_ms.is_some_and(|at| at <= now) {
        // The expiry worker will record it; nobody may decide it any more.
        let mut shown = g.clone();
        shown.state = GateState::Expired;
        return Err(already_resolved(&shown));
    }
    check_choice(g, req)?;
    let before = g.clone();
    let new = match req.decision {
        GateDecision::Approve => GateState::Approved,
        GateDecision::Reject => GateState::Rejected,
    };
    {
        let g = &mut st.gates[i];
        g.state = new;
        g.resolved_ms = Some(now);
        g.note = req.note.clone();
        g.choice = req.choice.clone();
        g.decided_by = Some(req.actor.label());
    }
    st.gate_ops.push(GateOpRecord {
        operation_id: req.operation_id.clone(),
        gate: req.id.clone(),
        decision: req.decision,
        choice: req.choice.clone(),
        actor: req.actor.clone(),
        prev: before.state,
        new,
        at_ms: now,
    });
    // Bound the records; dropped ones are restored on rollback.
    let cut = st.gate_ops.len().saturating_sub(MAX_GATE_OPS);
    let mut dropped: Vec<GateOpRecord> = st.gate_ops.drain(..cut).collect();
    let keep_from = st
        .gate_ops
        .iter()
        .position(|r| now.saturating_sub(r.at_ms) < GATE_OPS_KEEP_MS)
        .unwrap_or(st.gate_ops.len());
    dropped.extend(st.gate_ops.drain(..keep_from));
    if let Err(e) = save(st) {
        st.gate_ops.pop();
        let rest = std::mem::take(&mut st.gate_ops);
        st.gate_ops = dropped;
        st.gate_ops.extend(rest);
        st.gates[i] = before;
        return Err(e);
    }
    Ok(Decided::Now(st.gates[i].clone()))
}

/// A decision must fit the gate's options: approving a gate with options
/// names one of them; a rejection or a gate without options has no choice.
fn check_choice(g: &GateInfo, req: &Decide) -> Result<(), RpcError> {
    let bad = |m: String| Err(RpcError::new(codes::INVALID_PARAMS, m));
    match (req.decision, req.choice.as_deref()) {
        (GateDecision::Reject, Some(_)) => bad("a rejection has no choice".into()),
        (GateDecision::Approve, Some(c)) if g.options.is_empty() => {
            bad(format!("gate {} has no options (got choice {c:?})", g.id))
        }
        (GateDecision::Approve, Some(c)) if !g.options.iter().any(|o| o == c) => bad(format!(
            "{c:?} is not an option of gate {}: choose one of {}",
            g.id,
            g.options.join(" | ")
        )),
        (GateDecision::Approve, None) if !g.options.is_empty() => bad(format!(
            "gate {} needs a choice: one of {}",
            g.id,
            g.options.join(" | ")
        )),
        _ => Ok(()),
    }
}

/// Options of a new gate: trimmed, non-empty, unique, at most 16 of at most
/// 200 characters.
pub(super) fn clean_options(options: &[String]) -> Result<Vec<String>, RpcError> {
    let mut out: Vec<String> = Vec::new();
    for o in options {
        let o = o.trim();
        if o.is_empty() || o.chars().count() > 200 {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "each option must be 1..=200 characters",
            ));
        }
        if out.iter().any(|x| x == o) {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("option {o:?} is given twice"),
            ));
        }
        out.push(o.to_string());
    }
    if out.len() == 1 || out.len() > 16 {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            "give 2..=16 options (or none)",
        ));
    }
    Ok(out)
}

/// Mark every pending gate past its expiry as `expired` (one save). On a
/// failed save nothing changes in memory (the next pass retries). Returns
/// the newly expired gates and the next expiry.
pub(super) fn expire_in(
    st: &mut Store,
    now: u64,
    save: impl FnOnce(&mut Store) -> Result<(), RpcError>,
) -> (Vec<GateInfo>, Option<u64>) {
    let mut expired = Vec::new();
    let mut before = Vec::new();
    let mut next: Option<u64> = None;
    for (i, g) in st.gates.iter_mut().enumerate() {
        if g.state != GateState::Pending {
            continue;
        }
        let Some(at) = g.expires_ms else { continue };
        if at <= now {
            before.push((i, g.clone()));
            g.state = GateState::Expired;
            g.resolved_ms = Some(now);
            g.note = Some(format!(
                "expired after {} s without a decision",
                at.saturating_sub(g.created_ms) / 1000
            ));
            expired.push(g.clone());
        } else {
            next = Some(next.map_or(at, |n| n.min(at)));
        }
    }
    if !expired.is_empty()
        && let Err(e) = save(st)
    {
        log(&e.message);
        for (i, g) in before {
            st.gates[i] = g;
        }
        // Retry soon.
        return (Vec::new(), Some(now + 1_000));
    }
    (expired, next)
}

fn gate_not_found(id: &str) -> RpcError {
    RpcError::new(orch_codes::GATE_NOT_FOUND, format!("no gate {id:?}"))
}

impl Daemon {
    pub(super) fn gate_create(&self, p: GateCreateParams) -> Result<GateInfo, RpcError> {
        self.gate_create_as(p, None)
    }

    /// Create a gate; `requested_by` is the orchestration mailbox that gets
    /// the decision as mail (v17). A gate with an `item` blocks that item.
    pub(super) fn gate_create_as(
        &self,
        p: GateCreateParams,
        requested_by: Option<String>,
    ) -> Result<GateInfo, RpcError> {
        let options = clean_options(&p.options)?;
        let (run, item) = self.orch_gate_target(p.run.as_deref(), p.item.as_deref())?;
        if p.message.trim().is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "message must not be empty",
            ));
        }
        if p.message.len() > MAX_MESSAGE {
            return Err(RpcError::new(codes::INVALID_PARAMS, "message too long"));
        }
        if p.expires_in_s.is_some_and(|s| s == 0 || s > MAX_EXPIRY_S) {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("expires_in_s must be 1..={MAX_EXPIRY_S}"),
            ));
        }
        let terminal = match &p.terminal {
            Some(t) => Some(self.resolve(&TerminalRef::parse(t))?.id.clone()),
            None => None,
        };
        let mut task = match &p.task {
            Some(t) => Some(self.task_record(t)?.id),
            None => None,
        };
        if task.is_none()
            && let Some(tid) = &terminal
        {
            task = self
                .registry
                .lock()
                .unwrap()
                .tasks
                .iter()
                .find(|t| t.terminals.contains(tid))
                .map(|t| t.id.clone());
        }
        let group = match &p.group {
            Some(g) => Some(self.orch.lock().unwrap().group(g)?.id.clone()),
            None => None,
        };
        let gate = {
            let mut st = self.orch.lock().unwrap();
            let now = crate::now_ms();
            let g = GateInfo {
                id: st.next_id("g"),
                title: p
                    .title
                    .filter(|t| !t.trim().is_empty())
                    .unwrap_or_else(|| "Approval needed".into()),
                message: p.message,
                task,
                group,
                terminal,
                state: GateState::Pending,
                created_ms: now,
                resolved_ms: None,
                note: None,
                expires_ms: p.expires_in_s.map(|s| now + s * 1000),
                run,
                item,
                options,
                choice: None,
                requested_by,
                decided_by: None,
            };
            st.gates.push(g.clone());
            self.save_orch(&mut st)?;
            g
        };
        log(&format!(
            "gate {} requested (task {}, terminal {})",
            gate.id,
            gate.task.as_deref().unwrap_or("-"),
            gate.terminal.as_deref().unwrap_or("-")
        ));
        status::publish_gate(gate.clone());
        if gate.expires_ms.is_some() {
            self.gate_wake.notify_one();
        }
        self.orch_gate_opened(&gate);
        Ok(gate)
    }

    /// Resolve every pending gate whose expiry has passed as `expired`
    /// (persisted, announced with `gate.changed`). Returns the next expiry.
    pub(super) fn expire_gates(&self, now: u64) -> Option<u64> {
        let (expired, next) = {
            let mut st = self.orch.lock().unwrap();
            expire_in(&mut st, now, |st| self.save_orch(st))
        };
        for g in expired {
            log(&format!("gate {} expired", g.id));
            status::publish_gate(g.clone());
            self.orch_gate_settled(&g);
        }
        next
    }

    /// Decide a gate through [`decide_in`] and announce it.
    pub(super) fn gate_decide(&self, req: &Decide, now: u64) -> Result<Decided, RpcError> {
        let res = {
            let mut st = self.orch.lock().unwrap();
            decide_in(&mut st, req, now, |st| self.save_orch(st))
        };
        match &res {
            Ok(Decided::Now(g)) => {
                log(&format!(
                    "gate {} {:?} by {}",
                    g.id,
                    g.state,
                    req.actor.label()
                ));
                status::publish_gate(g.clone());
                self.orch_gate_settled(g);
            }
            Err(e) if e.code == orch_codes::GATE_RESOLVED => {
                // Possibly past its expiry and not marked yet.
                self.gate_wake.notify_one();
            }
            _ => {}
        }
        res
    }

    pub(super) fn gate_resolve(&self, p: GateResolveParams) -> Result<GateInfo, RpcError> {
        let req = Decide {
            id: p.id,
            decision: p.decision,
            choice: p.choice,
            note: p.note,
            actor: GateActor::Local,
            operation_id: None,
            task_scope: None,
        };
        match self.gate_decide(&req, crate::now_ms())? {
            Decided::Now(g) | Decided::Replayed(g) => Ok(g),
        }
    }

    pub(super) fn gate_list(&self, p: GateListParams) -> Result<GateListResult, RpcError> {
        let task = p.task.as_ref().map(|t| {
            self.task_record(t)
                .map(|r| r.id)
                .unwrap_or_else(|_| t.clone())
        });
        let st = self.orch.lock().unwrap();
        let group = p.group.as_ref().map(|g| {
            st.group(g)
                .map(|r| r.id.clone())
                .unwrap_or_else(|_| g.clone())
        });
        Ok(GateListResult {
            gates: st
                .gates
                .iter()
                .filter(|g| task.is_none() || g.task == task)
                .filter(|g| group.is_none() || g.group == group)
                .filter(|g| p.state.is_none_or(|s| g.state == s))
                .cloned()
                .collect(),
        })
    }

    pub(super) fn gate_show(&self, p: GateRef) -> Result<GateInfo, RpcError> {
        self.orch
            .lock()
            .unwrap()
            .gates
            .iter()
            .find(|g| g.id == p.id)
            .cloned()
            .ok_or_else(|| gate_not_found(&p.id))
    }

    pub(super) async fn gate_wait(&self, p: GateWaitParams) -> Result<GateWaitResult, RpcError> {
        // Subscribe before the first check so no resolution is missed.
        let mut rx = status::subscribe();
        let start = Instant::now();
        let deadline = p
            .timeout_ms
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        loop {
            let g = self.gate_show(GateRef { id: p.id.clone() })?;
            if g.state != GateState::Pending {
                return Ok(GateWaitResult {
                    outcome: WaitOutcome::Matched,
                    gate: g,
                    waited_ms: start.elapsed().as_millis() as u64,
                });
            }
            let next = async {
                loop {
                    match rx.recv().await {
                        Ok(HubEvent::Gate(ev)) if ev.gate.id == p.id => return,
                        Ok(_) => {}
                        Err(broadcast::error::RecvError::Lagged(_)) => return,
                        Err(broadcast::error::RecvError::Closed) => {
                            std::future::pending::<()>().await
                        }
                    }
                }
            };
            let timed_out = match deadline {
                Some(d) => tokio::time::timeout_at(d, next).await.is_err(),
                None => {
                    next.await;
                    false
                }
            };
            if timed_out {
                return Ok(GateWaitResult {
                    outcome: WaitOutcome::TimedOut,
                    gate: g,
                    waited_ms: start.elapsed().as_millis() as u64,
                });
            }
        }
    }
}

/// Start the gate expiry worker: wakes at the next expiry, when a gate with
/// an expiry is created, and every few seconds. Gates that expired while the
/// daemon was down are resolved on the first pass.
pub(super) fn start(daemon: &Arc<Daemon>) {
    let daemon = daemon.clone();
    tokio::spawn(async move {
        loop {
            let next = daemon.expire_gates(crate::now_ms());
            let wait = next
                .map(|at| Duration::from_millis(at.saturating_sub(crate::now_ms())))
                .unwrap_or(EXPIRY_POLL)
                .clamp(Duration::from_millis(20), EXPIRY_POLL);
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = daemon.gate_wake.notified() => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, Mutex};

    use super::*;

    fn gate(id: &str, task: &str, expires_ms: Option<u64>) -> GateInfo {
        GateInfo {
            id: id.into(),
            title: "Deploy?".into(),
            message: "ship it".into(),
            task: Some(task.into()),
            group: None,
            terminal: None,
            state: GateState::Pending,
            created_ms: 0,
            resolved_ms: None,
            note: None,
            expires_ms,
            run: None,
            item: None,
            options: Vec::new(),
            choice: None,
            requested_by: None,
            decided_by: None,
        }
    }

    fn store() -> Store {
        let mut st = Store::default();
        st.gates.push(gate("g1", "demo/a", Some(10_000)));
        st
    }

    fn req(actor: GateActor, decision: GateDecision, op: Option<&str>) -> Decide {
        Decide {
            id: "g1".into(),
            decision,
            choice: None,
            note: None,
            actor,
            operation_id: op.map(str::to_string),
            task_scope: None,
        }
    }

    fn phone() -> GateActor {
        GateActor::Mobile {
            device: "m1".into(),
        }
    }

    fn ok(_: &mut Store) -> Result<(), RpcError> {
        Ok(())
    }

    fn disk_full(_: &mut Store) -> Result<(), RpcError> {
        Err(RpcError::new(codes::INTERNAL_ERROR, "disk full"))
    }

    #[test]
    fn pc_and_phone_racing_one_wins() {
        for round in 0..50 {
            let st = Arc::new(Mutex::new(store()));
            let go = Arc::new(Barrier::new(2));
            let run = |actor: GateActor, d: GateDecision| {
                let (st, go) = (st.clone(), go.clone());
                std::thread::spawn(move || {
                    let r = req(actor, d, Some("op"));
                    go.wait();
                    decide_in(&mut st.lock().unwrap(), &r, 5_000, ok)
                })
            };
            let a = run(GateActor::Local, GateDecision::Approve);
            let b = run(phone(), GateDecision::Reject);
            let (a, b) = (a.join().unwrap(), b.join().unwrap());
            let wins = [&a, &b]
                .iter()
                .filter(|r| matches!(r, Ok(Decided::Now(_))))
                .count();
            assert_eq!(wins, 1, "round {round}: {a:?} / {b:?}");
            let loser = if a.is_ok() { &b } else { &a };
            assert_eq!(loser.as_ref().unwrap_err().code, orch_codes::GATE_RESOLVED);
            let st = st.lock().unwrap();
            assert_eq!(st.gate_ops.len(), 1, "one record");
            let rec = &st.gate_ops[0];
            assert_eq!(
                rec.new, st.gates[0].state,
                "record matches the committed gate"
            );
            assert_eq!(rec.prev, GateState::Pending);
            assert_eq!(rec.actor == GateActor::Local, a.is_ok());
        }
    }

    #[test]
    fn operation_ids_replay_and_conflict() {
        let mut st = store();
        let r = req(phone(), GateDecision::Approve, Some("op-1"));
        let Ok(Decided::Now(g)) = decide_in(&mut st, &r, 1_000, ok) else {
            panic!("first decision")
        };
        assert_eq!(g.state, GateState::Approved);
        assert_eq!(g.resolved_ms, Some(1_000));
        // Retry after a lost response: the stored result, nothing new.
        let again = decide_in(&mut st, &r, 2_000, ok).unwrap();
        assert_eq!(again, Decided::Replayed(g.clone()));
        assert_eq!(st.gate_ops.len(), 1);
        // Same id, different request.
        let other = req(phone(), GateDecision::Reject, Some("op-1"));
        let e = decide_in(&mut st, &other, 2_000, ok).unwrap_err();
        assert_eq!(e.code, mobile_codes::OPERATION_CONFLICT);
        // Same id from someone else is a new (stale) request.
        let pc = req(GateActor::Local, GateDecision::Approve, Some("op-1"));
        let e = decide_in(&mut st, &pc, 2_000, ok).unwrap_err();
        assert_eq!(e.code, orch_codes::GATE_RESOLVED);
        assert_eq!(e.data.unwrap()["state"], "approved", "current gate in data");
        assert_eq!(st.gates[0], g, "unchanged");
    }

    #[test]
    fn expired_gates_are_refused_at_the_clock_boundary() {
        let mut st = store();
        let r = req(phone(), GateDecision::Approve, Some("x"));
        let e = decide_in(&mut st, &r, 10_000, ok).unwrap_err();
        assert_eq!(e.code, orch_codes::GATE_RESOLVED);
        assert_eq!(e.data.unwrap()["state"], "expired");
        assert_eq!(
            st.gates[0].state,
            GateState::Pending,
            "the worker records it"
        );
        assert!(st.gate_ops.is_empty());
        // One ms earlier it is still decidable.
        assert!(matches!(
            decide_in(&mut st, &r, 9_999, ok),
            Ok(Decided::Now(_))
        ));
        // The expiry worker then leaves it alone.
        let (expired, _) = expire_in(&mut st, 20_000, ok);
        assert!(expired.is_empty());
    }

    #[test]
    fn scope_and_unknown_gates() {
        let mut st = store();
        let mut r = req(phone(), GateDecision::Approve, Some("x"));
        r.task_scope = Some("demo/b".into());
        let e = decide_in(&mut st, &r, 1, ok).unwrap_err();
        assert_eq!(e.code, orch_codes::GATE_NOT_FOUND);
        r.task_scope = None;
        r.id = "g9".into();
        let e = decide_in(&mut st, &r, 1, ok).unwrap_err();
        assert_eq!(e.code, orch_codes::GATE_NOT_FOUND);
        assert_eq!(st.gates[0].state, GateState::Pending);
    }

    #[test]
    fn storage_failure_rolls_back() {
        let mut st = store();
        st.gates[0].expires_ms = None;
        st.gate_ops.push(GateOpRecord {
            operation_id: None,
            gate: "g0".into(),
            decision: GateDecision::Approve,
            choice: None,
            actor: GateActor::Local,
            prev: GateState::Pending,
            new: GateState::Approved,
            at_ms: 0, // old enough to be pruned by the next decision
        });
        let r = req(phone(), GateDecision::Approve, Some("op"));
        let now = GATE_OPS_KEEP_MS + 1;
        let e = decide_in(&mut st, &r, now, disk_full).unwrap_err();
        assert_eq!(e.message, "disk full");
        assert_eq!(st.gates[0].state, GateState::Pending, "gate rolled back");
        assert_eq!(st.gates[0].resolved_ms, None);
        assert_eq!(st.gate_ops.len(), 1, "pruned record restored, new one gone");
        assert_eq!(st.gate_ops[0].gate, "g0");
        // The retry under the same id is a fresh decision, not a replay.
        assert!(matches!(
            decide_in(&mut st, &r, now, ok),
            Ok(Decided::Now(_))
        ));
        assert_eq!(st.gate_ops.len(), 1, "old record pruned once saved");
        assert_eq!(st.gate_ops[0].gate, "g1");

        // The same with the real store file: a directory where the temp file goes.
        let dir = std::env::temp_dir().join(format!(
            "pod-gate-rollback-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let mut st = store();
        st.save(&dir).unwrap();
        std::fs::create_dir_all(dir.join("orch").join("store.json.tmp")).unwrap();
        let r = req(GateActor::Local, GateDecision::Reject, None);
        let e = decide_in(&mut st, &r, 1, |s| s.save(&dir)).unwrap_err();
        assert_eq!(e.code, codes::INTERNAL_ERROR);
        assert_eq!(st.gates[0].state, GateState::Pending);
        assert!(st.gate_ops.is_empty());
        assert_eq!(
            Store::load(&dir).gates[0].state,
            GateState::Pending,
            "disk unchanged"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expiry_rolls_back_when_it_cannot_be_saved() {
        let mut st = store();
        let (expired, next) = expire_in(&mut st, 10_000, disk_full);
        assert!(expired.is_empty());
        assert_eq!(next, Some(11_000), "retried soon");
        assert_eq!(st.gates[0].state, GateState::Pending);
        let (expired, next) = expire_in(&mut st, 10_000, ok);
        assert_eq!(expired.len(), 1);
        assert_eq!(next, None);
        assert_eq!(st.gates[0].state, GateState::Expired);
    }

    #[test]
    fn decision_records_are_bounded() {
        let mut st = Store::default();
        for i in 0..(MAX_GATE_OPS + 5) {
            st.gates.push(gate(&format!("g{i}"), "demo/a", None));
            let mut r = req(GateActor::Local, GateDecision::Approve, None);
            r.id = format!("g{i}");
            decide_in(&mut st, &r, 1_000 + i as u64, ok).unwrap();
        }
        assert_eq!(st.gate_ops.len(), MAX_GATE_OPS);
        assert_eq!(
            st.gate_ops.last().unwrap().gate,
            format!("g{}", MAX_GATE_OPS + 4)
        );
    }

    #[test]
    fn options_need_one_choice_on_the_single_path() {
        let mut st = store();
        st.gates[0].options = vec!["A".into(), "B".into()];
        let mut r = req(phone(), GateDecision::Approve, Some("op1"));
        let code = |e: Result<Decided, RpcError>| e.unwrap_err().code;
        assert_eq!(
            code(decide_in(&mut st, &r, 1_000, ok)),
            codes::INVALID_PARAMS,
            "no choice"
        );
        r.choice = Some("C".into());
        assert_eq!(
            code(decide_in(&mut st, &r, 1_000, ok)),
            codes::INVALID_PARAMS,
            "not an option"
        );
        let mut no = req(GateActor::Local, GateDecision::Reject, None);
        no.choice = Some("A".into());
        assert_eq!(
            code(decide_in(&mut st, &no, 1_000, ok)),
            codes::INVALID_PARAMS,
            "reject has no choice"
        );
        assert!(
            st.gate_ops.is_empty() && st.gates[0].state == GateState::Pending,
            "nothing recorded"
        );
        r.choice = Some("B".into());
        let Ok(Decided::Now(g)) = decide_in(&mut st, &r, 1_000, ok) else {
            panic!()
        };
        assert_eq!(
            (g.state, g.choice.as_deref(), g.decided_by.as_deref()),
            (GateState::Approved, Some("B"), Some("m1"))
        );
        assert_eq!(st.gate_ops[0].choice.as_deref(), Some("B"));
        // Replay of the same operation; another choice under it conflicts.
        assert!(matches!(
            decide_in(&mut st, &r, 2_000, ok),
            Ok(Decided::Replayed(_))
        ));
        r.choice = Some("A".into());
        assert_eq!(
            code(decide_in(&mut st, &r, 2_000, ok)),
            mobile_codes::OPERATION_CONFLICT
        );
        // A coordinator deciding later loses like anyone else.
        let mut coord = req(
            GateActor::Agent {
                from: "run:r1".into(),
            },
            GateDecision::Approve,
            Some("x"),
        );
        coord.choice = Some("A".into());
        assert_eq!(
            code(decide_in(&mut st, &coord, 2_000, ok)),
            orch_codes::GATE_RESOLVED
        );
        // Gates without options refuse a choice.
        st.gates.push(gate("g2", "demo/a", None));
        let mut plain = req(GateActor::Local, GateDecision::Approve, None);
        plain.id = "g2".into();
        plain.choice = Some("A".into());
        assert_eq!(
            code(decide_in(&mut st, &plain, 2_000, ok)),
            codes::INVALID_PARAMS
        );
        assert_eq!(
            clean_options(&[" A ".into(), "B".into()]).unwrap(),
            ["A", "B"]
        );
        assert!(
            clean_options(&["A".into()]).is_err()
                && clean_options(&["A".into(), "A".into()]).is_err()
        );
        assert!(clean_options(&[]).unwrap().is_empty());
    }
}
