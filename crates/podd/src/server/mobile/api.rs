//! The mobile API: the only operations a mobile channel can ever reach.
//!
//! A channel (M2 LAN TLS, M4/M5 relay + Noise) authenticates the device,
//! then calls `Daemon::mobile_call` with a session and an operation name.
//! [`route`] maps names to the closed [`Op`] set; anything else, every
//! pipe method included, is refused before any daemon state is touched.
//! There is no generic JSON-RPC proxy: each op has its own handler and DTO
//! (no prompt, argv, environment, paths or tokens leave through here).

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use super::super::Daemon;
use super::super::gates::{Decide, Decided};
use super::super::orch_store::{DeliveryOrigin, GateActor};
use super::super::review::{PromptJob, format_prompt, replay_of, takes_prompts};
use super::message;
use super::session::Bucket;
use crate::protocol::*;

/// Lines / bytes of output a phone may read at once.
const OUTPUT_LINES: usize = 100;
const OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Tasks,
    Task,
    Output,
    GateDecide,
    Message,
    Delivery,
}

impl Op {
    fn bucket(self) -> Bucket {
        match self {
            Self::Tasks | Self::Task | Self::Output | Self::Delivery => Bucket::Read,
            Self::GateDecide => Bucket::Decide,
            Self::Message => Bucket::Message,
        }
    }
}

/// The whole mobile surface. Exact names only.
pub fn route(name: &str) -> Option<Op> {
    Some(match name {
        mobile_api::TASKS => Op::Tasks,
        mobile_api::TASK => Op::Task,
        mobile_api::OUTPUT => Op::Output,
        mobile_api::GATE_DECIDE => Op::GateDecide,
        mobile_api::MESSAGE => Op::Message,
        mobile_api::DELIVERY => Op::Delivery,
        _ => return None,
    })
}

/// What the channel knows about the caller.
#[derive(Debug, Clone)]
pub struct MobileCtx {
    pub session: String,
    /// The person did this (extends the idle timer); background polls: false.
    pub user_initiated: bool,
}

fn denied(m: impl Into<String>) -> RpcError {
    RpcError::new(mobile_codes::MOBILE_DENIED, m)
}

fn params<P: DeserializeOwned>(v: Value) -> Result<P, RpcError> {
    super::super::params(v)
}

fn to_value<T: serde::Serialize>(t: T) -> Result<Value, RpcError> {
    super::super::to_value(t)
}

fn mobile_gate(g: &GateInfo) -> Option<MobileGate> {
    Some(MobileGate {
        id: g.id.clone(),
        task: g.task.clone()?,
        title: g.title.clone(),
        message: g.message.clone(),
        state: g.state,
        created_ms: g.created_ms,
        expires_ms: g.expires_ms,
        resolved_ms: g.resolved_ms,
        options: g.options.clone(),
        choice: g.choice.clone(),
    })
}

/// Terminal text for a phone: control characters removed, last lines only,
/// at most `OUTPUT_BYTES`.
pub fn clean_output(lines: Vec<String>) -> (Vec<String>, bool) {
    let mut out: Vec<String> = lines
        .into_iter()
        .map(|l| l.chars().filter(|c| !c.is_control()).collect::<String>())
        .collect();
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    let mut truncated = out.len() > OUTPUT_LINES;
    let skip = out.len().saturating_sub(OUTPUT_LINES);
    out.drain(..skip);
    let mut total: usize = out.iter().map(|l| l.len() + 1).sum();
    while total > OUTPUT_BYTES && !out.is_empty() {
        total -= out.remove(0).len() + 1;
        truncated = true;
    }
    (out, truncated)
}

/// What the outer layer of [`Daemon::mobile_call`] needs to audit a failure
/// the handler did not record itself.
struct CallAudit {
    /// Device id once the session checked out, else `unauthenticated`.
    actor: String,
    /// The handler wrote its own (richer) record for this call.
    recorded: bool,
    /// The parameters did not parse: audited without their content.
    bad_params: bool,
}

/// Parse the operation's parameters; a failure is marked for the audit.
fn parse<P: DeserializeOwned>(p: Value, audit: &mut CallAudit) -> Result<P, RpcError> {
    params(p).inspect_err(|_| audit.bad_params = true)
}

impl Daemon {
    /// Entry point of every mobile request. Every failure is audited, here
    /// at the outermost layer unless the handler recorded it with more
    /// context (security-05). Parameter errors are audited without the
    /// parameters (no message body in the log).
    pub(crate) async fn mobile_call(
        self: &Arc<Self>,
        ctx: &MobileCtx,
        method: &str,
        p: Value,
    ) -> Result<Value, RpcError> {
        let mut audit = CallAudit {
            actor: "unauthenticated".into(),
            recorded: false,
            bad_params: false,
        };
        let res = self.mobile_op(ctx, method, p, &mut audit);
        if let Err(e) = &res
            && !audit.recorded
        {
            let mut entry = self.mobile.entry(&audit.actor, method, Err(e));
            if audit.bad_params {
                entry.detail = Some("invalid params (not recorded)".into());
            }
            self.mobile.record(entry);
        }
        res
    }

    fn mobile_op(
        self: &Arc<Self>,
        ctx: &MobileCtx,
        method: &str,
        p: Value,
        audit: &mut CallAudit,
    ) -> Result<Value, RpcError> {
        let now = self.mobile.clock.now_ms();
        let device = {
            let mut m = self.mobile.state.lock().unwrap();
            let st = &mut *m;
            let d = st
                .sessions
                .check(&st.registry, &ctx.session, now, ctx.user_initiated)?;
            if let Ok(dev) = st.registry.device_mut(&d) {
                dev.last_seen_ms = Some(now);
            }
            d
        };
        audit.actor = device.clone();
        let Some(op) = route(method) else {
            return Err(denied(format!(
                "{method:?} is not available to mobile devices"
            )));
        };
        self.mobile
            .state
            .lock()
            .unwrap()
            .limits
            .hit(&device, op.bucket(), now)?;
        match op {
            Op::Tasks => to_value(self.mobile_tasks(&device)),
            Op::Task => {
                let p: MobileTaskParams = parse(p, audit)?;
                let r = self.mobile_task(&device, &p.task);
                if let Err(e) = &r {
                    let mut a = self.mobile.entry(&device, method, Err(e));
                    a.task = Some(p.task.clone());
                    self.mobile.record(a);
                    audit.recorded = true;
                }
                to_value(r?)
            }
            Op::Output => {
                let p: MobileTaskParams = parse(p, audit)?;
                let r = self.mobile_output(&device, &p.task);
                let mut e = self.mobile.entry(&device, method, r.as_ref().map(|_| "ok"));
                e.task = Some(p.task);
                self.mobile.record(e);
                audit.recorded = true;
                to_value(r?)
            }
            Op::GateDecide => {
                let p: MobileGateDecideParams = parse(p, audit)?;
                let r = self.mobile_decide(ctx, &device, &p, now);
                let mut e = self.mobile.entry(
                    &device,
                    method,
                    r.as_ref()
                        .map(|x| if x.replayed { "replayed" } else { "ok" }),
                );
                e.gate = Some(p.gate.clone());
                e.task = r.as_ref().ok().map(|x| x.gate.task.clone());
                e.operation_id = Some(p.operation_id.clone());
                e.detail = Some(format!("{:?}", p.decision).to_lowercase());
                self.mobile.record(e);
                audit.recorded = true;
                to_value(r?)
            }
            Op::Message => {
                let p: MobileMessageParams = parse(p, audit)?;
                // Audits every outcome itself (with length and digest).
                audit.recorded = true;
                let r = self.mobile_message(ctx, &device, p, now);
                to_value(r?)
            }
            Op::Delivery => {
                let p: MobileDeliveryParams = parse(p, audit)?;
                to_value(self.mobile_delivery(&device, &p.delivery)?)
            }
        }
    }

    /// `MOBILE_DENIED` unless the device holds `cap` on exactly this task id
    /// (also for tasks that do not exist: nothing to enumerate).
    fn mobile_require(&self, device: &str, task: &str, cap: MobileCap) -> Result<(), RpcError> {
        if self
            .mobile
            .state
            .lock()
            .unwrap()
            .registry
            .allows(device, task, cap)
        {
            Ok(())
        } else {
            Err(denied(format!(
                "not allowed: {} on task {task:?}",
                cap.as_str()
            )))
        }
    }

    /// The task by exact id (policies name ids; no name lookup from phones).
    fn mobile_task_record(&self, task: &str) -> Result<super::super::tasks::TaskRecord, RpcError> {
        match self.task_record(task) {
            Ok(r) if r.id == task => Ok(r),
            _ => Err(RpcError::new(
                codes::TASK_NOT_FOUND,
                format!("task {task:?} no longer exists"),
            )),
        }
    }

    fn mobile_task_view(
        &self,
        rec: &super::super::tasks::TaskRecord,
        caps: Vec<MobileCap>,
    ) -> MobileTask {
        let status = self.task_status_now(&rec.id);
        let gates = self
            .orch
            .lock()
            .unwrap()
            .gates
            .iter()
            .filter(|g| g.state == GateState::Pending && g.task.as_ref() == Some(&rec.id))
            .filter_map(mobile_gate)
            .collect();
        MobileTask {
            id: rec.id.clone(),
            repo: rec.repo.clone(),
            name: rec.name.clone(),
            state: status
                .as_ref()
                .map_or_else(|| "unknown".into(), |s| s.state.clone()),
            kind: status.and_then(|s| s.kind),
            caps,
            gates,
        }
    }

    fn mobile_tasks(&self, device: &str) -> MobileTasksResult {
        let visible = self
            .mobile
            .state
            .lock()
            .unwrap()
            .registry
            .visible_tasks(device);
        let tasks = visible
            .into_iter()
            .filter_map(|(id, caps)| {
                let rec = self.mobile_task_record(&id).ok()?;
                Some(self.mobile_task_view(&rec, caps))
            })
            .collect();
        MobileTasksResult {
            server_ms: self.mobile.clock.now_ms(),
            tasks,
        }
    }

    fn mobile_task(&self, device: &str, task: &str) -> Result<MobileTask, RpcError> {
        self.mobile_require(device, task, MobileCap::Status)?;
        let rec = self.mobile_task_record(task)?;
        let caps = self
            .mobile
            .state
            .lock()
            .unwrap()
            .registry
            .visible_tasks(device)
            .into_iter()
            .find(|(t, _)| t == task)
            .map(|(_, c)| c)
            .unwrap_or_default();
        Ok(self.mobile_task_view(&rec, caps))
    }

    fn mobile_output(&self, device: &str, task: &str) -> Result<MobileOutputResult, RpcError> {
        self.mobile_require(device, task, MobileCap::Output)?;
        let rec = self.mobile_task_record(task)?;
        let tid = rec.terminals.first().ok_or_else(|| {
            RpcError::new(
                codes::TERMINAL_NOT_FOUND,
                format!("task {task} has no terminal"),
            )
        })?;
        let t = self.resolve(&TerminalRef::parse(tid))?;
        let (lines, _) = t.read_lines(ReadMode::History);
        let (lines, truncated) = clean_output(lines);
        Ok(MobileOutputResult {
            task: task.to_string(),
            lines,
            truncated,
        })
    }

    fn mobile_decide(
        &self,
        ctx: &MobileCtx,
        device: &str,
        p: &MobileGateDecideParams,
        now: u64,
    ) -> Result<MobileGateDecideResult, RpcError> {
        message::check_operation_id(&p.operation_id)?;
        let not_found =
            || RpcError::new(orch_codes::GATE_NOT_FOUND, format!("no gate {:?}", p.gate));
        // Only task gates: group gates and gates of hidden tasks look absent.
        let task = self
            .orch
            .lock()
            .unwrap()
            .gates
            .iter()
            .find(|g| g.id == p.gate)
            .and_then(|g| g.task.clone())
            .ok_or_else(not_found)?;
        if self
            .mobile_require(device, &task, MobileCap::Status)
            .is_err()
        {
            return Err(not_found());
        }
        self.mobile_require(device, &task, MobileCap::Approve)?;
        let name = self
            .mobile
            .state
            .lock()
            .unwrap()
            .registry
            .device(device)
            .map(|d| d.name.clone())
            .unwrap_or_default();
        let req = Decide {
            id: p.gate.clone(),
            decision: p.decision,
            choice: p.choice.clone(),
            note: Some(format!("decided on mobile device {device} ({name})")),
            actor: GateActor::Mobile {
                device: device.to_string(),
            },
            operation_id: Some(p.operation_id.clone()),
            task_scope: Some(task.clone()),
        };
        // Revocation and policy are re-checked under the mobile lock, held
        // across the commit: a revoke lands either before (refused) or
        // after (the decision stands), never in between.
        let mut st = self.mobile.state.lock().unwrap();
        let s = &mut *st;
        s.sessions.check(&s.registry, &ctx.session, now, false)?;
        if !s.registry.allows(device, &task, MobileCap::Approve) {
            return Err(denied(format!("not allowed: approve on task {task:?}")));
        }
        let res = self.gate_decide(&req, now);
        drop(st);
        let (g, replayed) = match res? {
            Decided::Now(g) => (g, false),
            Decided::Replayed(g) => (g, true),
        };
        Ok(MobileGateDecideResult {
            gate: mobile_gate(&g).ok_or_else(not_found)?,
            replayed,
        })
    }

    fn mobile_message(
        self: &Arc<Self>,
        ctx: &MobileCtx,
        device: &str,
        p: MobileMessageParams,
        now: u64,
    ) -> Result<MobileMessageResult, RpcError> {
        let audit_base = |result: Result<&str, &RpcError>, text: Option<&str>| {
            let mut e = self.mobile.entry(device, mobile_api::MESSAGE, result);
            e.task = Some(p.task.clone());
            e.operation_id = Some(p.operation_id.clone());
            if p.with_open_comments {
                e.detail = Some("with open review comments".into());
            }
            if let Some(t) = text {
                e.msg_len = Some(t.chars().count());
                e.msg_sha256 = Some(message::sha256_hex(t.as_bytes()));
                if self
                    .mobile
                    .state
                    .lock()
                    .unwrap()
                    .registry
                    .settings
                    .audit_message_bodies
                {
                    e.msg_body = Some(t.to_string());
                }
            }
            e
        };
        let checked = message::check_operation_id(&p.operation_id)
            .and_then(|()| self.mobile_require(device, &p.task, MobileCap::Message))
            .and_then(|()| message::validate(&p.text, p.with_open_comments));
        let text = match checked {
            Ok(t) => t,
            Err(e) => {
                self.mobile.record(audit_base(Err(&e), None));
                return Err(e);
            }
        };
        // No delivery without its audit record.
        if let Err(io) = self
            .mobile
            .audit
            .append(&audit_base(Ok("accepted"), Some(&text)))
        {
            return Err(RpcError::new(
                codes::INTERNAL_ERROR,
                format!("message refused: the audit log cannot be written ({io})"),
            ));
        }
        let res = self.mobile_deliver(ctx, device, &p, &text, now);
        let r = res.as_ref().map(|x| match (x.replayed, x.state) {
            (true, _) => "replayed",
            (_, DeliveryState::Queued) => "queued",
            _ => "ok",
        });
        let mut e = audit_base(r, Some(&text));
        if let Ok(x) = &res {
            e.detail = Some(format!("delivery {}", x.delivery));
        }
        self.mobile.record(e);
        res
    }

    fn mobile_deliver(
        self: &Arc<Self>,
        ctx: &MobileCtx,
        device: &str,
        p: &MobileMessageParams,
        text: &str,
        now: u64,
    ) -> Result<MobileMessageResult, RpcError> {
        let origin = DeliveryOrigin {
            device: device.to_string(),
            operation_id: p.operation_id.clone(),
            digest: message::request_digest(&p.task, text, p.with_open_comments),
        };
        let replayed = |info: DeliveryInfo| MobileMessageResult {
            delivery: info.id,
            state: info.state,
            agent_state: info.agent_state,
            replayed: true,
        };
        // A retry of a recorded message: its result, whatever the agent does now.
        if let Some(prev) = replay_of(&self.orch.lock().unwrap(), &origin)? {
            return Ok(replayed(prev));
        }
        let rec = self.mobile_task_record(&p.task)?;
        let tid = rec.terminals.first().ok_or_else(|| {
            RpcError::new(
                codes::TERMINAL_NOT_FOUND,
                format!("task {} has no terminal", rec.id),
            )
        })?;
        let t = self.resolve(&TerminalRef::parse(tid))?;
        let (status, label) = takes_prompts(&t)?;
        let (prompt, comment_ids) = if p.with_open_comments {
            let mut open: Vec<ReviewComment> = self
                .orch
                .lock()
                .unwrap()
                .comments
                .iter()
                .filter(|c| c.task == rec.id && c.state == CommentState::Open)
                .cloned()
                .collect();
            if open.is_empty() {
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!("task {} has no open review comments", rec.id),
                ));
            }
            open.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
            let instructions = (!text.is_empty()).then_some(text);
            (
                format_prompt(&rec.id, &open, instructions),
                open.into_iter().map(|c| c.id).collect(),
            )
        } else {
            (message::frame(text), Vec::new())
        };
        let job = PromptJob {
            task: rec.id.clone(),
            terminal: t,
            prompt,
            comment_ids,
            origin: Some(origin),
        };
        // Same rule as gate decisions: revoke/policy re-checked across the commit.
        let mut m = self.mobile.state.lock().unwrap();
        let s = &mut *m;
        s.sessions.check(&s.registry, &ctx.session, now, false)?;
        if !s.registry.allows(device, &rec.id, MobileCap::Message) {
            return Err(denied(format!("not allowed: message on task {:?}", rec.id)));
        }
        let res = self.enqueue_prompt(job, &status, label);
        drop(m);
        let (info, was_replay) = res?;
        if was_replay {
            return Ok(replayed(info));
        }
        Ok(MobileMessageResult {
            delivery: info.id,
            state: info.state,
            agent_state: info.agent_state,
            replayed: false,
        })
    }

    fn mobile_delivery(&self, device: &str, id: &str) -> Result<MobileDeliveryResult, RpcError> {
        let st = self.orch.lock().unwrap();
        let d = st
            .deliveries
            .iter()
            .find(|d| d.info.id == id && d.origin.as_ref().is_some_and(|o| o.device == device))
            .ok_or_else(|| {
                RpcError::new(
                    orch_codes::REVIEW_NOT_FOUND,
                    format!("no delivery {id:?} of this device"),
                )
            })?;
        Ok(MobileDeliveryResult {
            delivery: d.info.id.clone(),
            task: d.info.task.clone(),
            state: d.info.state,
            submit: d.submit.clone(),
            error: d.info.error.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pipe method (and the PC mobile methods, and the test hooks) is
    /// refused by the router; only the six mobile operations route.
    #[test]
    fn nothing_but_the_mobile_ops_routes() {
        let schema = crate::protocol::schema();
        let methods = schema["methods"].as_object().unwrap();
        assert!(methods.len() > 50, "schema lists the pipe methods");
        let hooks = [
            super::super::TEST_PAIR,
            super::super::TEST_SESSION,
            super::super::TEST_CALL,
        ];
        let extra = [
            "terminal.attach",
            "status.subscribe",
            "server.stop",
            "mod.command",
            "group.pick",
            "task.pr.merge",
            "gate.resolve",
            "review.send",
            "TASKS",
            " tasks",
            "tasks ",
            "gate.decide\n",
            "",
            "mobile.tasks",
            "mobile/v1/tasks",
        ];
        for m in methods
            .keys()
            .map(String::as_str)
            .chain(MOBILE_METHODS.iter().copied())
            .chain(hooks)
            .chain(extra)
        {
            assert_eq!(route(m), None, "{m:?} must not be reachable from a phone");
        }
        for m in mobile_api::ALL {
            assert!(route(m).is_some(), "{m}");
            assert!(
                !methods.contains_key(m),
                "{m} must not also be a pipe method"
            );
        }
    }

    #[test]
    fn output_is_bounded_and_clean() {
        let lines: Vec<String> = (0..150).map(|i| format!("line {i}\x1b[31m\x07")).collect();
        let (out, truncated) = clean_output(lines);
        assert!(truncated);
        assert_eq!(out.len(), OUTPUT_LINES);
        assert_eq!(out[0], "line 50[31m");
        assert!(out.iter().all(|l| !l.chars().any(char::is_control)));
        let big: Vec<String> = (0..50).map(|_| "x".repeat(1000)).collect();
        let (out, truncated) = clean_output(big);
        assert!(truncated);
        assert!(out.iter().map(|l| l.len() + 1).sum::<usize>() <= OUTPUT_BYTES);
        let (out, truncated) = clean_output(vec!["a".into(), "".into(), "  ".into()]);
        assert_eq!((out, truncated), (vec!["a".to_string()], false));
    }
}
