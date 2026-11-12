//! Decision gates in orchestration (docs/19 §4.5): a gate blocks its item
//! until it is decided, a worker's gate is mailed to its coordinator, the
//! decision is mailed back; `orch.gate.request` / `orch.gate.decide` go
//! through the gate store's single decision path.

use pod_orch::engine::{NewMail, Orch};
use pod_orch::model::{attempt_mailbox, run_mailbox};
use pod_orch::policy::{self, GateFacts, Policy};
use serde_json::json;

use super::identity::who;
use super::rpc::{NEEDS_PERSON, oe};
use super::state::{cached, remember};
use crate::protocol::*;
use crate::server::gates::{Decide, Decided};
use crate::server::orch_store::GateActor;
use crate::server::{Daemon, log, status};

/// The broker's view of a gate: its kind from what it asks, the rest from
/// the daemon's records (the requesting attempt's run).
fn gate_facts(o: &Orch, g: &GateInfo) -> GateFacts {
    let requester_run = g
        .requested_by
        .as_deref()
        .and_then(|r| r.strip_prefix("attempt:"))
        .and_then(|a| o.attempt(a).ok())
        .map(|a| a.run.clone());
    GateFacts {
        id: g.id.clone(),
        run: g.run.clone(),
        requested_by: g.requested_by.clone(),
        requester_run,
        kind: policy::classify(&g.title, &g.message, &g.options),
    }
}

impl Daemon {
    /// The person's agent trust policy (`<state>\orch\policy.json`, O1),
    /// read at each use so an edit applies at once. No file: the default.
    pub(super) fn orch_policy(&self) -> Policy {
        let path = self.runs.state_dir.join("orch").join(policy::POLICY_FILE);
        let (p, notes) = Policy::load(&path);
        for n in notes {
            log(&format!("orch policy: {n}"));
        }
        p
    }

    /// The run and item a gate is about (checked against the engine).
    pub(in crate::server) fn orch_gate_target(
        &self,
        run: Option<&str>,
        item: Option<&str>,
    ) -> Result<(Option<String>, Option<String>), RpcError> {
        let o = self.orch();
        match (run, item) {
            (_, Some(i)) => {
                let it = o.item(i).map_err(oe)?;
                if let Some(r) = run
                    && r != it.run
                {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        format!("item {i} belongs to run {}, not {r}", it.run),
                    ));
                }
                if matches!(it.state, ItemState::Completed | ItemState::Failed) {
                    return Err(RpcError::new(
                        orch_codes::INVALID_STATE,
                        format!("item {i} is already finished; a gate cannot block it"),
                    ));
                }
                Ok((Some(it.run.clone()), Some(i.to_string())))
            }
            (Some(r), None) => {
                o.run(r).map_err(oe)?;
                Ok((Some(r.to_string()), None))
            }
            (None, None) => Ok((None, None)),
        }
    }

    /// A new gate: block its item; a worker's gate goes to its coordinator.
    pub(in crate::server) fn orch_gate_opened(&self, g: &GateInfo) {
        let Some(run) = g.run.as_deref() else { return };
        let mut o = self.orch();
        if let Some(item) = &g.item {
            match o.block_item(item, &g.id, crate::now_ms()) {
                Ok(_) => {
                    status::publish_orch_changed(run, Some(item), None);
                }
                Err(e) => log(&format!(
                    "orch: gate {} cannot block {item}: {}",
                    g.id, e.message
                )),
            }
        }
        if g.requested_by
            .as_deref()
            .is_some_and(|r| r.starts_with("attempt:"))
        {
            // O1: say up front whether the coordinator may decide it.
            let facts = gate_facts(&o, g);
            let kind = facts.kind;
            let decider = match policy::check(
                &self.orch_policy(),
                &policy::Actor::Coordinator {
                    run: run.to_string(),
                },
                &policy::Action::DecideGate(facts),
            ) {
                Ok(()) => "coordinator",
                Err(_) => "person",
            };
            let subject = MsgText::new(pod_orch::text::GATE_REQUESTED)
                .with("gate", &g.id)
                .with("title", &g.title);
            let mut body = MsgText::new(pod_orch::text::GATE_REQUESTED_BODY)
                .with("gate", &g.id)
                .with("message", &g.message);
            if !g.options.is_empty() {
                body = body.with("options", g.options.join(" | "));
            }
            if decider == "person" {
                body = body.with("kind", kind.as_str()).with("decider", decider);
            }
            let m = NewMail::pod(
                g.requested_by.clone().unwrap_or_default(),
                run_mailbox(run),
                MailType::Gate,
                subject,
            )
            .body_text(body)
            .payload(json!({
                "gate": g.id, "options": g.options, "item": g.item, "state": "pending",
                "kind": kind.as_str(), "decider": decider,
            }));
            if let Err(e) = self.post(&mut o, None, m) {
                log(&format!("orch: gate {} mail: {}", g.id, e.message));
            }
        }
    }

    /// A gate was decided or expired: unblock its item and mail the
    /// requester (and the coordinator, when someone else decided a worker's
    /// gate). Called after the single decision path committed.
    pub(in crate::server) fn orch_gate_settled(&self, g: &GateInfo) {
        let Some(run) = g.run.as_deref() else { return };
        let mut o = self.orch();
        if let Some(item) = &g.item {
            match o.unblock_item(item, &g.id, crate::now_ms()) {
                Ok(Some(_)) => {
                    status::publish_orch_changed(run, Some(item), None);
                }
                Ok(None) => {}
                Err(e) => log(&format!(
                    "orch: gate {} unblock {item}: {}",
                    g.id, e.message
                )),
            }
        }
        let state = match g.state {
            GateState::Approved => "approved",
            GateState::Rejected => "rejected",
            GateState::Expired => "expired",
            GateState::Pending => return,
        };
        let outcome = match (&g.choice, g.state) {
            (Some(_), _) => "decided",
            _ => state,
        };
        let settled = |kind: &str| {
            let mut t = MsgText::new(kind)
                .with("gate", &g.id)
                .with("title", &g.title)
                .with("outcome", outcome);
            if let Some(c) = &g.choice {
                t = t.with("choice", c);
            }
            if let Some(n) = g.note.as_deref().filter(|n| !n.is_empty()) {
                t = t.with("note", n);
            }
            t
        };
        let from = match g.decided_by.as_deref() {
            Some(d) if d.starts_with("run:") => d.to_string(),
            Some(_) => "human".to_string(),
            None => "pod".to_string(),
        };
        let mut to: Vec<String> = g.requested_by.iter().cloned().collect();
        if g.requested_by
            .as_deref()
            .is_some_and(|r| r.starts_with("attempt:"))
            && !from.starts_with("run:")
        {
            to.push(run_mailbox(run));
        }
        for t in to {
            let m = NewMail::pod(from.clone(), t, MailType::Gate, settled(pod_orch::text::GATE_SETTLED))
                .body_text(settled(pod_orch::text::GATE_SETTLED_BODY))
                .payload(json!({"gate": g.id, "state": state, "choice": g.choice, "decided_by": g.decided_by, "item": g.item}));
            if let Err(e) = self.post(&mut o, None, m) {
                log(&format!("orch: gate {} decision mail: {}", g.id, e.message));
            }
        }
    }

    /// After a restart: items still blocked by gates that were decided (or
    /// lost) are freed; pending item gates that the item misses block it.
    pub(super) fn orch_reconcile_gates(&self) {
        let gates: Vec<GateInfo> = self
            .orch
            .lock()
            .unwrap()
            .gates
            .iter()
            .filter(|g| g.item.is_some())
            .cloned()
            .collect();
        let blocked: Vec<(String, String)> = self
            .orch()
            .runs()
            .flat_map(|r| r.rec.items.iter())
            .flat_map(|i| i.gates.iter().map(|g| (i.id.clone(), g.clone())))
            .collect();
        let now = crate::now_ms();
        for (item, gid) in blocked {
            let pending = gates
                .iter()
                .any(|g| g.id == gid && g.state == GateState::Pending);
            if !pending && let Ok(Some(it)) = self.orch().unblock_item(&item, &gid, now) {
                log(&format!("orch: item {item} freed from settled gate {gid}"));
                status::publish_orch_changed(&it.run, Some(&item), None);
            }
        }
        for g in gates.iter().filter(|g| g.state == GateState::Pending) {
            let item = g.item.as_deref().unwrap_or_default();
            let missing = self
                .orch()
                .item(item)
                .is_ok_and(|i| !i.gates.contains(&g.id));
            if missing {
                let _ = self.orch().block_item(item, &g.id, now);
            }
        }
    }

    /// The worker asked for a decision that is still pending.
    pub(super) fn orch_waits_on_gate(&self, attempt: &str) -> bool {
        let mb = attempt_mailbox(attempt);
        self.orch.lock().unwrap().gates.iter().any(|g| {
            g.state == GateState::Pending && g.requested_by.as_deref() == Some(mb.as_str())
        })
    }

    /// `orch.gate.request`.
    pub(super) fn orch_gate_request(&self, p: OrchGateRequestParams) -> Result<GateInfo, RpcError> {
        if let Some(r) = cached(&self.orch(), p.request_id.as_deref(), OrchGateRequest::NAME)? {
            return Ok(r);
        }
        let (w, item, run) = {
            let o = self.orch();
            let w = who(&o, &p.caller)?;
            // A worker's gate is about its own item unless it names another.
            let item = p.item.clone().or_else(|| {
                (p.run.is_none())
                    .then(|| {
                        w.attempt
                            .as_deref()
                            .and_then(|a| o.attempt(a).ok())
                            .map(|a| a.item.clone())
                    })
                    .flatten()
            });
            let run = p.run.clone().or_else(|| {
                if item.is_none() {
                    w.coordinates.clone().or(w.works_in.clone())
                } else {
                    None
                }
            });
            (w, item, run)
        };
        if item.is_none() && run.is_none() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "no run: pass --run or --item, or call from a coordinator or worker terminal",
            ));
        }
        let requested_by = (w.from != "human").then(|| w.from.clone());
        let g = self.gate_create_as(
            GateCreateParams {
                task: None,
                group: None,
                terminal: w.terminal.clone(),
                title: p.title.clone(),
                message: p.message.clone(),
                expires_in_s: p.expires_in_s,
                item,
                run,
                options: p.options.clone(),
            },
            requested_by,
        )?;
        remember(
            &mut self.orch(),
            p.request_id.as_deref(),
            OrchGateRequest::NAME,
            &g,
        )?;
        Ok(g)
    }

    /// `orch.gate.decide`: a person, or the coordinator of the gate's run
    /// (never for a gate it asked itself). Through the single decision path.
    pub(super) fn orch_gate_decide(&self, p: OrchGateDecideParams) -> Result<GateInfo, RpcError> {
        let g = self.gate_show(GateRef { id: p.id.clone() })?;
        let actor = {
            let o = self.orch();
            let w = who(&o, &p.caller)?;
            if w.from == "human" {
                GateActor::Local
            } else {
                let run = g.run.clone().ok_or_else(|| {
                    RpcError::new(
                        codes::INVALID_PARAMS,
                        format!("gate {} belongs to no run: a person decides it (Gates panel, `pod gate`)", g.id),
                    )
                })?;
                let coordinates = w
                    .terminal
                    .as_deref()
                    .is_some_and(|t| o.runs_of_terminal(t).contains(&run))
                    || o.run(&run).is_ok_and(|r| {
                        r.rec.run.coordinator_attempt.is_some()
                            && r.rec.run.coordinator_attempt == w.attempt
                    });
                if !coordinates {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        format!(
                            "only the coordinator of run {run} or a person can decide gate {}",
                            g.id
                        ),
                    ));
                }
                // O1: the broker. Text in the gate or in any mail plays no
                // part; the kind comes from the gate, the rest from records.
                let facts = gate_facts(&o, &g);
                let policy = self.orch_policy();
                if let Err(r) = policy::check(
                    &policy,
                    &policy::Actor::Coordinator { run: run.clone() },
                    &policy::Action::DecideGate(facts),
                ) {
                    let code = match r {
                        policy::Refusal::OwnGate => codes::INVALID_PARAMS,
                        _ => NEEDS_PERSON,
                    };
                    log(&format!(
                        "orch: {} may not decide gate {} ({:?})",
                        run_mailbox(&run),
                        g.id,
                        r
                    ));
                    return Err(RpcError::new(code, r.message(&g.id)));
                }
                GateActor::Agent {
                    from: run_mailbox(&run),
                }
            }
        };
        let req = Decide {
            id: p.id.clone(),
            decision: if p.reject {
                GateDecision::Reject
            } else {
                GateDecision::Approve
            },
            choice: p.choice.clone(),
            note: p.note.clone(),
            actor,
            operation_id: p.request_id.clone(),
            task_scope: None,
        };
        match self.gate_decide(&req, crate::now_ms())? {
            Decided::Now(g) | Decided::Replayed(g) => Ok(g),
        }
    }
}
