//! A worker's `done` and its journal (orch-01): the done mail is written
//! first, then the transition in memory and the run record. A failed mail
//! write changes nothing; a record write that failed or never happened is
//! redone from the mail on load or by the same report again.

use serde_json::Value;

use super::{DoneResult, ErrorKind, NewMail, Orch, OrchError, Result, RunState};
use crate::model::{
    Attempt, AttemptState, ItemState, Mail, MailType, Ms, WorkerState, attempt_mailbox, run_mailbox,
};
use crate::text::{self, MsgText};

impl Orch {
    /// A worker's `done`. Superseded attempts (an older generation, or an
    /// attempt that was stopped/abandoned) are recorded but change nothing.
    #[allow(clippy::too_many_arguments)]
    pub fn done(
        &mut self,
        attempt: &str,
        token: Option<&str>,
        succeeded: bool,
        body: &str,
        files: &[String],
        report: Option<&str>,
        release_after_ms: Option<Ms>,
        now: Ms,
    ) -> Result<DoneResult> {
        let a = self.verify_token(attempt, token)?.clone();
        let item_gen = self.item(&a.item)?.generation;
        if matches!(a.state, AttemptState::Succeeded | AttemptState::Failed) {
            // orch-01: the same report again (a retry after a failed answer
            // or request record) gets the recorded result; anything else is
            // a second, different report.
            if let Some(mail) = self.recorded_done(&a, succeeded, body, files, report) {
                self.save_run(&a.run)?;
                return Ok(DoneResult {
                    accepted: true,
                    attempt: a,
                    mail,
                    repeated: true,
                });
            }
            return Err(OrchError::new(
                ErrorKind::InvalidState,
                format!("attempt {attempt} already reported done; send a status message instead"),
            ));
        }
        let accepted = a.state.is_open() && a.generation == item_gen;
        let outcome = if succeeded { "succeeded" } else { "failed" };
        let subject = if accepted {
            MsgText::new(text::DONE)
                .with("outcome", outcome)
                .with("title", &self.item(&a.item)?.title)
        } else {
            MsgText::new(text::DONE_LATE)
                .with("outcome", outcome)
                .with("attempt", attempt)
        }
        .with("attempt", attempt)
        .with("item", &a.item);
        let mut m = NewMail::pod(
            attempt_mailbox(attempt),
            run_mailbox(&a.run),
            MailType::Done,
            subject,
        )
        .body(body)
        .payload(serde_json::json!({
            "outcome": outcome,
            "item": a.item,
            "attempt": attempt,
            "files": files,
            "report": report,
            "accepted": accepted,
            // For the replay on load (orch-01): which opening of the
            // attempt this report closes, and when its worker is released.
            "generation": a.generation,
            "reopens": a.reopens,
            "release_at_ms": release_after_ms.map(|d| now + d),
        }));
        m.fenced = !accepted;
        // orch-01: the mail line is the journal of the transition. It is
        // written first; the run record follows. A failed mail write changes
        // nothing; a failed or missed record write is redone from the mail
        // on load ([`Orch::replay_done`]) or by the same report again.
        let mail = self.send(Some(&a.run), m, |_| false, now)?;
        if accepted {
            let st = self.run_mut(&a.run)?;
            Self::apply_done(
                st,
                attempt,
                succeeded,
                body,
                files,
                report,
                release_after_ms.map(|d| now + d),
                now,
            );
            self.save_run(&a.run)?;
        }
        Ok(DoneResult {
            accepted,
            attempt: self.attempt(attempt)?.clone(),
            mail,
            repeated: false,
        })
    }

    /// The accepted `done` transition (in memory).
    #[allow(clippy::too_many_arguments)]
    fn apply_done(
        st: &mut RunState,
        attempt: &str,
        succeeded: bool,
        body: &str,
        files: &[String],
        report: Option<&str>,
        release_at_ms: Option<Ms>,
        now: Ms,
    ) {
        let Some(at) = st.rec.attempts.iter_mut().find(|x| x.id == attempt) else {
            return;
        };
        at.state = if succeeded {
            AttemptState::Succeeded
        } else {
            AttemptState::Failed
        };
        at.ended_ms = Some(now);
        at.body = Some(body.to_string());
        at.files = files.to_vec();
        at.report = report.map(str::to_string);
        if at.worker == WorkerState::Active {
            at.worker = WorkerState::Finished;
            at.release_at_ms = release_at_ms;
        }
        let item = at.item.clone();
        if let Some(it) = st.rec.items.iter_mut().find(|i| i.id == item) {
            it.state = if succeeded {
                ItemState::Completed
            } else {
                ItemState::Failed
            };
            it.updated_ms = now;
        }
        Self::promote(st, now);
    }

    /// The accepted done mail that closed `a` in its current opening.
    fn done_mail_of<'m>(st: &'m RunState, a: &Attempt) -> Option<&'m Mail> {
        let from = attempt_mailbox(&a.id);
        st.mail.iter().rev().find(|m| {
            let p = m.payload.as_ref();
            let num = |k: &str| p.and_then(|p| p.get(k)).and_then(Value::as_u64);
            m.kind == MailType::Done
                && !m.fenced
                && m.from == from
                && num("generation") == Some(u64::from(a.generation))
                && num("reopens") == Some(u64::from(a.reopens))
        })
    }

    /// The recorded done mail when it says the same as this report.
    fn recorded_done(
        &self,
        a: &Attempt,
        succeeded: bool,
        body: &str,
        files: &[String],
        report: Option<&str>,
    ) -> Option<Mail> {
        let m = Self::done_mail_of(self.runs.get(&a.run)?, a)?;
        let p = m.payload.as_ref()?;
        let outcome = if succeeded { "succeeded" } else { "failed" };
        let same = p.get("outcome").and_then(Value::as_str) == Some(outcome)
            && m.body == body
            && p.get("files") == Some(&serde_json::json!(files))
            && p.get("report").and_then(Value::as_str) == report;
        same.then(|| m.clone())
    }

    /// orch-01: an accepted done mail whose transition did not reach
    /// `run.json` (crash or write failure in between) is applied again.
    /// Only the opening it was sent in counts (same generation and reopen
    /// count, attempt still open). Returns the attempts closed.
    pub(super) fn replay_done(st: &mut RunState) -> Vec<String> {
        let mut closed = Vec::new();
        let todo: Vec<(String, Mail)> = st
            .rec
            .attempts
            .iter()
            .filter(|a| a.state.is_open())
            .filter(|a| {
                st.rec
                    .items
                    .iter()
                    .any(|i| i.id == a.item && i.generation == a.generation)
            })
            .filter_map(|a| Self::done_mail_of(st, a).map(|m| (a.id.clone(), m.clone())))
            .collect();
        for (aid, m) in todo {
            let p = m.payload.clone().unwrap_or(Value::Null);
            let files: Vec<String> = p
                .get("files")
                .and_then(|f| serde_json::from_value(f.clone()).ok())
                .unwrap_or_default();
            Self::apply_done(
                st,
                &aid,
                p.get("outcome").and_then(Value::as_str) == Some("succeeded"),
                &m.body,
                &files,
                p.get("report").and_then(Value::as_str),
                p.get("release_at_ms").and_then(Value::as_u64),
                m.created_ms,
            );
            closed.push(aid);
        }
        closed
    }
}
