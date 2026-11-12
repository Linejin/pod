//! Mail between coordinators, workers and people: send, reply, check
//! (optionally blocked until mail arrives), ask (blocked until the answer),
//! done, wait for other items, and the app's thread view.

use std::time::Duration;

use pod_orch::engine::{NewMail, Orch};
use pod_orch::model::{attempt_mailbox, run_mailbox};
use serde_json::{Value, json};

use super::identity::{public, who};
use super::rpc::oe;
use super::state::{Waiting, cached, remember};
use super::{ASK_DEFAULT_MS, ASK_MAX_MS};
use crate::protocol::*;
use crate::server::{Daemon, log, status};

impl Daemon {
    /// `orch.mail.list`: the run's whole thread for the app (nothing acked).
    pub(super) fn orch_mail_list(
        &self,
        p: OrchMailListParams,
    ) -> Result<OrchMailListResult, RpcError> {
        let messages = self.orch().mail_of(&p.run).map_err(oe)?;
        Ok(OrchMailListResult { messages })
    }

    /// Record a message and announce it (event + waiters + supervisor).
    pub(super) fn post(
        &self,
        o: &mut Orch,
        ctx_run: Option<&str>,
        m: NewMail,
    ) -> Result<Mail, RpcError> {
        let idle = |a: &Attempt| {
            a.terminal
                .as_deref()
                .and_then(|t| self.terminal_by_id(t))
                .is_some_and(|t| {
                    matches!(t.status.info().state.as_str(), "idle" | "done" | "unknown")
                })
        };
        let mail = o.send(ctx_run, m, idle, crate::now_ms()).map_err(oe)?;
        self.announce_mail(&mail);
        Ok(mail)
    }

    fn announce_mail(&self, mail: &Mail) {
        status::publish_orch_mail(mail);
        status::publish_orch_changed(&mail.run, None, None);
        self.runs.mail.notify_waiters();
        self.runs.wake.notify_one();
    }

    pub(super) async fn orch_mail_send(&self, p: OrchMailSendParams) -> Result<Mail, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchMailSend::NAME)? {
            return Ok(r);
        }
        let w = who(&o, &p.caller)?;
        if p.kind == MailType::Done {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "use orch.done (pod orch done) to report completion",
            ));
        }
        // O1: gate mail comes from the decision path only, so a sent
        // "approved" can never look like a decision record.
        if let Err(r) = pod_orch::policy::check(
            &pod_orch::policy::Policy::default(),
            &pod_orch::policy::Actor::Person,
            &pod_orch::policy::Action::SendMail(p.kind),
        ) {
            return Err(RpcError::new(codes::INVALID_PARAMS, r.message("")));
        }
        let to = match (&p.to, &w.works_in) {
            (Some(t), _) => t.clone(),
            (None, Some(r)) => run_mailbox(r),
            (None, None) => return Err(RpcError::new(codes::INVALID_PARAMS, "--to is required")),
        };
        let ctx = p
            .run
            .clone()
            .or(w.coordinates.clone())
            .or(w.works_in.clone());
        let mut m =
            NewMail::new(w.from.clone(), to, p.kind, p.subject.clone()).body(p.body.clone());
        m.reply_to = p.reply_to.clone();
        m.payload = p.payload.clone();
        m.priority = p.priority;
        let mail = self.post(&mut o, ctx.as_deref(), m)?;
        remember(&mut o, p.request_id.as_deref(), OrchMailSend::NAME, &mail)?;
        Ok(mail)
    }

    pub(super) async fn orch_mail_reply(&self, p: OrchMailReplyParams) -> Result<Mail, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchMailReply::NAME)? {
            return Ok(r);
        }
        let w = who(&o, &p.caller)?;
        let q = o.find_mail(&p.id).cloned().ok_or_else(|| {
            RpcError::new(run_codes::MAIL_NOT_FOUND, format!("no message {}", p.id))
        })?;
        if !q.from.starts_with("run:") && !q.from.starts_with("attempt:") {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!(
                    "message {} came from {}, which has no mailbox",
                    q.id, q.from
                ),
            ));
        }
        // A person may answer for the coordinator.
        let from = if w.from == "human" && q.recipients.iter().any(|r| r.starts_with("run:")) {
            "human".to_string()
        } else {
            w.from.clone()
        };
        let kind = if q.kind == MailType::Question {
            MailType::Answer
        } else {
            MailType::Status
        };
        let subject = if q.subject.starts_with("re: ") {
            q.subject.clone()
        } else {
            format!("re: {}", q.subject)
        };
        let mut m = NewMail::new(from, q.from.clone(), kind, subject).body(p.body.clone());
        m.reply_to = Some(q.id.clone());
        let mail = self.post(&mut o, Some(&q.run), m)?;
        remember(&mut o, p.request_id.as_deref(), OrchMailReply::NAME, &mail)?;
        Ok(mail)
    }

    pub(super) async fn orch_mail_check(
        &self,
        p: OrchMailCheckParams,
    ) -> Result<OrchMailCheckResult, RpcError> {
        let boxes = {
            let o = self.orch();
            let w = who(&o, &p.caller)?;
            let b = w.mailboxes(&o, p.run.as_deref());
            if b.is_empty() {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    "no mailbox: call from a coordinator or worker terminal, or pass --run",
                ));
            }
            b
        };
        let timeout = p.timeout_ms.unwrap_or(ASK_DEFAULT_MS).min(ASK_MAX_MS);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
        let _waiting = p.wait.then(|| Waiting::new(&self.runs, boxes.clone()));
        loop {
            let notified = self.runs.mail.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (msgs, settled) = {
                let mut o = self.orch();
                let open: Vec<&String> =
                    boxes.iter().filter(|b| o.wake_unconfirmed(b) > 0).collect();
                let msgs = o.check(&boxes, p.ack, crate::now_ms()).map_err(oe)?;
                // orch-02: a check settles an unconfirmed wake-up; the app shows those.
                let settled: Vec<String> = open
                    .into_iter()
                    .filter(|b| o.wake_unconfirmed(b) == 0)
                    .filter_map(|b| o.mailbox_run(b))
                    .collect();
                (msgs, settled)
            };
            for run in settled {
                status::publish_orch_changed(&run, None, None);
            }
            if !msgs.is_empty() || !p.wait {
                let next = if msgs.is_empty() {
                    "No mail. Do not poll: end your turn; Pod wakes you when mail arrives."
                        .to_string()
                } else {
                    "Answer questions with `pod orch reply --id <id> --body ...`; workers report with `pod orch done`. Do not poll; Pod wakes you for new mail.".to_string()
                };
                return Ok(OrchMailCheckResult {
                    mailboxes: boxes,
                    messages: msgs,
                    timed_out: false,
                    next,
                });
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(OrchMailCheckResult {
                    mailboxes: boxes,
                    messages: Vec::new(),
                    timed_out: true,
                    next: "No mail yet. End your turn; Pod wakes you when mail arrives.".into(),
                });
            }
        }
    }

    pub(super) async fn orch_mail_ask(
        &self,
        p: OrchMailAskParams,
    ) -> Result<OrchMailAskResult, RpcError> {
        const M: &str = OrchMailAsk::NAME;
        let (question, mailbox) = {
            let _ops = self.runs.ops.lock().await;
            let mut o = self.orch();
            let w = who(&o, &p.caller)?;
            let Some(attempt) = w.attempt.clone() else {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    "ask is for workers (POD_ATTEMPT_ID); coordinators use `pod orch send`",
                ));
            };
            let mailbox = attempt_mailbox(&attempt);
            let stored: Option<Value> = cached(&o, p.request_id.as_deref(), M)?;
            let q = match stored.as_ref().and_then(|v| v["question"].as_str()) {
                Some(id) => o.find_mail(id).cloned().ok_or_else(|| {
                    RpcError::new(run_codes::MAIL_NOT_FOUND, format!("no message {id}"))
                })?,
                None => {
                    if p.question.trim().is_empty() {
                        return Err(RpcError::new(
                            codes::INVALID_PARAMS,
                            "the question must not be empty",
                        ));
                    }
                    let subject = p
                        .subject
                        .clone()
                        .unwrap_or_else(|| pod_orch::spec_title(&p.question));
                    let to = run_mailbox(w.works_in.as_deref().unwrap_or_default());
                    let m = NewMail::new(w.from.clone(), to, MailType::Question, subject)
                        .body(p.question.clone());
                    let q = self.post(&mut o, None, m)?;
                    remember(
                        &mut o,
                        p.request_id.as_deref(),
                        M,
                        &json!({"question": q.id}),
                    )?;
                    q
                }
            };
            (q, mailbox)
        };
        let timeout = p.timeout_ms.unwrap_or(ASK_DEFAULT_MS).min(ASK_MAX_MS);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
        let _waiting = Waiting::new(&self.runs, vec![mailbox.clone()]);
        loop {
            let notified = self.runs.mail.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let answer = {
                let mut o = self.orch();
                match o.answer_to(&question.id) {
                    Some(mut a) => {
                        let now = crate::now_ms();
                        o.ack(&mailbox, std::slice::from_ref(&a.id), now)
                            .map_err(oe)?;
                        a.read_ms = Some(now);
                        Some(a)
                    }
                    None => None,
                }
            };
            if let Some(a) = answer {
                return Ok(OrchMailAskResult {
                    question,
                    answer: Some(a),
                    timed_out: false,
                    next: "Continue the task with this answer. Report with `pod orch done` when finished.".into(),
                });
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(OrchMailAskResult {
                    question,
                    answer: None,
                    timed_out: true,
                    next: "No answer yet. End your turn now; Pod wakes you (`pod mail: ...`) when the answer arrives, then run `pod orch check`.".into(),
                });
            }
        }
    }

    pub(super) async fn orch_done(&self, p: OrchDoneParams) -> Result<OrchDoneResult, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchDone::NAME)? {
            return Ok(r);
        }
        let Some(attempt) = p.caller.attempt.clone() else {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "done is for workers: POD_ATTEMPT_ID and POD_ATTEMPT_TOKEN are not set",
            ));
        };
        let d = o
            .done(
                &attempt,
                p.caller.token.as_deref(),
                p.outcome == DoneOutcome::Succeeded,
                &p.body,
                &p.files,
                p.report.as_deref(),
                self.orch_release_ms(),
                crate::now_ms(),
            )
            .map_err(oe)?;
        if d.repeated {
            // orch-01: the same report again (the first answer or its
            // request record was lost); the mail exists, wake anyway.
            status::publish_orch_changed(&d.mail.run, None, None);
            self.runs.mail.notify_waiters();
            self.runs.wake.notify_one();
        } else {
            self.announce_mail(&d.mail);
        }
        let next = if d.accepted {
            "Reported. Your work here is done: end your turn and stay idle (do not exit); your coordinator may send follow-up mail.".to_string()
        } else {
            "This attempt was replaced by a newer one; your report was recorded but changes nothing. Stop working on it.".to_string()
        };
        let res = OrchDoneResult {
            attempt: public(d.attempt),
            accepted: d.accepted,
            mail: d.mail,
            next,
        };
        remember(&mut o, p.request_id.as_deref(), OrchDone::NAME, &res)?;
        drop(o);
        log(&format!(
            "orch: done {attempt} ({:?}, accepted {})",
            p.outcome, res.accepted
        ));
        status::publish_orch_changed(&res.attempt.run, Some(&res.attempt.item), Some(&attempt));
        Ok(res)
    }

    /// `orch.wait` (docs/19 §18.2): the calling worker waits for other items
    /// of its run. Everything finished already: nothing is recorded.
    pub(super) async fn orch_wait(&self, p: OrchWaitParams) -> Result<OrchWaitResult, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let mut o = self.orch();
        let Some(attempt) = p.caller.attempt.clone() else {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "wait is for workers (POD_ATTEMPT_ID is not set); a coordinator orders work with item deps",
            ));
        };
        let a = o
            .verify_token(&attempt, p.caller.token.as_deref())
            .map_err(oe)?
            .clone();
        let states = o.wait_for_items(&attempt, &p.items).map_err(oe)?;
        drop(o);
        let waiting_on: Vec<String> = states
            .iter()
            .filter(|(_, s)| !s.is_finished())
            .map(|(i, _)| i.clone())
            .collect();
        let next = if waiting_on.is_empty() {
            "Every item you named has finished already: go on (`pod orch item ls` shows their state).".to_string()
        } else {
            format!(
                "Waiting for {}. End your turn now; that is not a missed done. Pod types `pod mail: ...` into your terminal when they have all finished; then run `pod orch check`.",
                waiting_on.join(", ")
            )
        };
        log(&format!(
            "orch: {attempt} waits for {}",
            if waiting_on.is_empty() {
                "nothing (all finished)".into()
            } else {
                waiting_on.join(", ")
            }
        ));
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&attempt));
        Ok(OrchWaitResult {
            items: states
                .into_iter()
                .map(|(item, state)| OrchWaitItem { item, state })
                .collect(),
            waiting_on,
            next,
        })
    }

    /// Workers whose awaited items all finished get a `status` mail (which
    /// wakes them like any mail).
    pub(super) fn orch_settle_waits(&self) {
        let mut o = self.orch();
        let due = match o.settle_waits() {
            Ok(d) => d,
            Err(e) => {
                log(&format!("orch: settle waits: {}", e.message));
                return;
            }
        };
        for (a, states) in due {
            let items: Vec<String> = states
                .iter()
                .map(|(i, s)| format!("{i} {}", item_state_en(*s)))
                .collect();
            let text = MsgText::new(pod_orch::text::WAIT_FINISHED)
                .with("attempt", &a.id)
                .with("item", &a.item)
                .with("items", items.join(", "));
            let payload: Vec<Value> = states
                .iter()
                .map(|(i, s)| json!({"item": i, "state": s}))
                .collect();
            let m = NewMail::pod("pod", attempt_mailbox(&a.id), MailType::Status, text)
                .payload(json!({"waited": payload}));
            match self.post(&mut o, None, m) {
                Ok(_) => log(&format!(
                    "orch: {} stops waiting ({})",
                    a.id,
                    items.join(", ")
                )),
                Err(e) => log(&format!("orch: wait mail for {}: {}", a.id, e.message)),
            }
            status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        }
    }
}

fn item_state_en(s: ItemState) -> &'static str {
    match s {
        ItemState::Pending => "pending",
        ItemState::Ready => "ready",
        ItemState::Assigned => "assigned",
        ItemState::Completed => "completed",
        ItemState::Failed => "failed",
        ItemState::Blocked => "blocked",
    }
}
