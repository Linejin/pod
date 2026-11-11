//! Mail: addresses and groups, recording a message (which re-opens a
//! finished worker it reaches), read receipts, `check` and open questions.

use std::collections::BTreeMap;

use super::{ErrorKind, NewMail, Orch, OrchError, Result, id_number, io};
use crate::model::{
    Attempt, AttemptState, GROUPS, ItemState, Mail, MailLine, MailType, Ms, WorkerState,
    attempt_mailbox,
};
use crate::store;

impl Orch {
    /// Mailboxes a message to `to` reaches. Groups need the sender's run
    /// and expand to its live workers (`idle` decides `@idle`).
    fn recipients(
        &self,
        ctx_run: Option<&str>,
        to: &str,
        kind: MailType,
        idle: &impl Fn(&Attempt) -> bool,
    ) -> Result<(String, Vec<String>)> {
        if let Some(run) = to.strip_prefix("run:") {
            self.run(run)?;
            return Ok((run.to_string(), vec![to.to_string()]));
        }
        if let Some(a) = to.strip_prefix("attempt:") {
            let a = self.attempt(a)?;
            return Ok((a.run.clone(), vec![to.to_string()]));
        }
        if GROUPS.contains(&to) {
            if matches!(kind, MailType::Done | MailType::Heartbeat) {
                return Err(OrchError::new(
                    ErrorKind::InvalidParams,
                    format!("{} cannot go to a group", kind.as_str()),
                ));
            }
            let run = ctx_run.ok_or_else(|| {
                OrchError::new(
                    ErrorKind::InvalidParams,
                    format!("{to} needs a run (--run)"),
                )
            })?;
            let st = self.run(run)?;
            let live = st.rec.attempts.iter().filter(|a| {
                a.state.is_open()
                    && match to {
                        "@idle" => idle(a),
                        "@claude" => a.agent == "claude",
                        "@codex" => a.agent == "codex",
                        _ => true,
                    }
            });
            return Ok((
                run.to_string(),
                live.map(|a| attempt_mailbox(&a.id)).collect(),
            ));
        }
        Err(OrchError::new(
            ErrorKind::InvalidParams,
            format!(
                "bad address {to:?}: use run:<id>, attempt:<id>, @all, @idle, @claude or @codex"
            ),
        ))
    }

    /// Record a message. Mail to a worker that already finished (and was
    /// not released) re-opens it for follow-up work.
    pub fn send(
        &mut self,
        ctx_run: Option<&str>,
        m: NewMail,
        idle: impl Fn(&Attempt) -> bool,
        now: Ms,
    ) -> Result<Mail> {
        let (run, recipients) = self.recipients(ctx_run, &m.to, m.kind, &idle)?;
        if m.subject.trim().is_empty() {
            return Err(OrchError::new(
                ErrorKind::InvalidParams,
                "the subject must not be empty",
            ));
        }
        let reopen = matches!(
            m.kind,
            MailType::Question | MailType::Answer | MailType::Status | MailType::Handoff
        );
        let mut reopened = Vec::new();
        for r in &recipients {
            if let Some(aid) = r.strip_prefix("attempt:") {
                let a = self.attempt(aid)?;
                if !reopen || a.state.is_open() || attempt_mailbox(aid) == m.from {
                    continue;
                }
                if a.worker == WorkerState::Released || a.terminal.is_none() {
                    return Err(OrchError::new(
                        ErrorKind::InvalidState,
                        format!(
                            "worker {aid} was released; start a new attempt (--retry-of {aid})"
                        ),
                    ));
                }
                if matches!(a.state, AttemptState::Succeeded | AttemptState::Failed) {
                    reopened.push(aid.to_string());
                }
            }
        }
        let thread = m.thread.clone().or_else(|| {
            m.reply_to
                .as_deref()
                .and_then(|r| self.find_mail(r))
                .map(|q| q.thread.clone().unwrap_or_else(|| q.id.clone()))
        });
        let id = self.next_id("m")?;
        let mail = Mail {
            id,
            run: run.clone(),
            from: m.from,
            to: m.to,
            recipients,
            kind: m.kind,
            subject: m.subject,
            body: m.body,
            thread,
            reply_to: m.reply_to,
            payload: m.payload,
            priority: m.priority,
            created_ms: now,
            fenced: m.fenced,
            text: m.text,
            body_text: m.body_text,
            read_ms: None,
        };
        store::append_mail(
            &store::mail_jsonl(&self.root, &run),
            &MailLine::Mail {
                mail: Box::new(mail.clone()),
            },
        )
        .map_err(io)?;
        self.run_mut(&run)?.mail.push(mail.clone());
        for aid in reopened {
            let arun = self.attempt_run(&aid)?;
            let st = self.run_mut(&arun)?;
            let a = st.rec.attempts.iter_mut().find(|a| a.id == aid).unwrap();
            a.state = AttemptState::Ready;
            a.ended_ms = None;
            a.release_at_ms = None;
            a.reopens += 1;
            if a.worker == WorkerState::Finished {
                a.worker = WorkerState::Active;
            }
            let (item, gen_) = (a.item.clone(), a.generation);
            if let Some(it) = st
                .rec
                .items
                .iter_mut()
                .find(|i| i.id == item && i.generation == gen_ && i.gates.is_empty())
            {
                it.state = ItemState::Assigned;
                it.updated_ms = now;
            }
            self.save_run(&arun)?;
        }
        Ok(mail)
    }

    pub fn find_mail(&self, id: &str) -> Option<&Mail> {
        self.runs
            .values()
            .find_map(|r| r.mail.iter().find(|m| m.id == id))
    }

    /// Every message of a run, oldest first, marking nothing read (the app's
    /// inbox). `read_ms` is the earliest read receipt of its recipients.
    pub fn mail_of(&self, run: &str) -> Result<Vec<Mail>> {
        let st = self.run(run)?;
        let mut out: Vec<Mail> = st
            .mail
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.read_ms = m
                    .recipients
                    .iter()
                    .filter_map(|r| st.read.get(r).and_then(|x| x.get(&m.id)).copied())
                    .min();
                m
            })
            .collect();
        out.sort_by_key(|m| id_number(&m.id));
        Ok(out)
    }

    /// Unread mail of a mailbox, oldest first.
    pub fn unread(&self, mailbox: &str) -> Vec<Mail> {
        let mut out = Vec::new();
        for st in self.runs.values() {
            let read = st.read.get(mailbox);
            for m in &st.mail {
                if m.recipients.iter().any(|r| r == mailbox)
                    && !read.is_some_and(|r| r.contains_key(&m.id))
                {
                    out.push(m.clone());
                }
            }
        }
        out.sort_by_key(|m| id_number(&m.id));
        out
    }

    /// Unread mail that wakes (everything but heartbeats).
    pub fn unread_waking(&self, mailbox: &str) -> usize {
        self.unread(mailbox)
            .iter()
            .filter(|m| m.kind.wakes())
            .count()
    }

    /// Mark mail read for `mailbox` and clear its notification.
    pub fn ack(&mut self, mailbox: &str, ids: &[String], now: Ms) -> Result<()> {
        // Group by run (receipts live with the run's mail).
        let mut by_run: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for id in ids {
            if let Some(m) = self.find_mail(id) {
                by_run.entry(m.run.clone()).or_default().push(id.clone());
            }
        }
        for (run, ids) in by_run {
            store::append_mail(
                &store::mail_jsonl(&self.root, &run),
                &MailLine::Read {
                    mailbox: mailbox.to_string(),
                    ids: ids.clone(),
                    ms: now,
                },
            )
            .map_err(io)?;
            let st = self.run_mut(&run)?;
            let r = st.read.entry(mailbox.to_string()).or_default();
            for id in ids {
                r.entry(id).or_insert(now);
            }
        }
        self.clear_notified(mailbox)
    }

    /// `check`: unread mail of these mailboxes, marked read when `ack`.
    pub fn check(&mut self, mailboxes: &[String], ack: bool, now: Ms) -> Result<Vec<Mail>> {
        let mut out = Vec::new();
        for mb in mailboxes {
            let mut unread = self.unread(mb);
            if ack {
                let ids: Vec<String> = unread.iter().map(|m| m.id.clone()).collect();
                self.ack(mb, &ids, now)?;
                for m in unread.iter_mut() {
                    m.read_ms = Some(now);
                }
            }
            out.extend(unread);
        }
        out.sort_by_key(|m| id_number(&m.id));
        out.dedup_by(|a, b| a.id == b.id);
        Ok(out)
    }

    /// The answer to a question, if one came.
    pub fn answer_to(&self, question: &str) -> Option<Mail> {
        self.runs
            .values()
            .flat_map(|r| r.mail.iter())
            .find(|m| m.kind == MailType::Answer && m.reply_to.as_deref() == Some(question))
            .cloned()
    }

    /// The worker asked something that has no answer yet.
    pub fn has_open_question(&self, attempt: &str) -> bool {
        let from = attempt_mailbox(attempt);
        self.runs.values().flat_map(|r| r.mail.iter()).any(|m| {
            m.kind == MailType::Question && m.from == from && self.answer_to(&m.id).is_none()
        })
    }
}
