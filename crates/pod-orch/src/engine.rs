//! The orchestration state machine (docs/19 §4): runs, items with
//! dependencies, attempts with fences, mail with read receipts and wake-up
//! bookkeeping, request idempotency. Pure and synchronous: every mutation is
//! written to disk before it returns; the daemon wraps it in a lock and does
//! the terminal work (starting agents, typing notifications) itself.
//!
//! This file is the façade: the state, error and input/output types, the
//! public constants and the lookups. The [`Orch`] methods live in private
//! modules by what they do, all on the one `Orch` state: `persistence`
//! (load, save, ids), `items` (runs, the dependency DAG, gates, waits),
//! `attempts` (limits, tokens, generations, worker lifetime), `admission`
//! (the resource queue), `completion` (done and its replay), `mail` (mail
//! and read receipts), `wake_state` (notifications) and `requests`
//! (request ids).

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::*;
use crate::text::MsgText;

mod admission;
mod attempts;
mod completion;
mod items;
mod mail;
mod persistence;
mod requests;
#[cfg(test)]
mod tests;
mod wake_state;

/// Run records this daemon writes.
pub const RUN_VERSION: u32 = 1;
/// Remembered `request_id` results (oldest dropped first).
pub const MAX_REQUESTS: usize = 512;

/// Safety limits (docs/19 §4.2, §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Deepest allowed worker: a person's coordinator is 0, its workers 1.
    pub max_depth: u32,
    /// Open workers per run.
    pub run_worker_cap: usize,
    /// Consecutive start failures of one item that open its circuit breaker.
    pub start_strikes: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_depth: 3,
            run_worker_cap: 8,
            start_strikes: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidParams,
    InvalidState,
    RunNotFound,
    ItemNotFound,
    AttemptNotFound,
    MailNotFound,
    /// Token or generation mismatch.
    AttemptFenced,
    DepthExceeded,
    CircuitOpen,
    DepsNotDone,
    WorkerLimit,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchError {
    pub kind: ErrorKind,
    pub message: String,
}

impl OrchError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        OrchError {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for OrchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for OrchError {}

pub type Result<T> = std::result::Result<T, OrchError>;

/// Items with their states (`wait_for_items`, `settle_waits`).
pub type ItemStates = Vec<(String, ItemState)>;

fn io(e: std::io::Error) -> OrchError {
    OrchError::new(ErrorKind::Io, format!("orchestration state: {e}"))
}

fn not_found(kind: ErrorKind, what: &str, id: &str) -> OrchError {
    OrchError::new(kind, format!("no {what} {id}"))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    counters: BTreeMap<String, u64>,
    #[serde(default)]
    requests: Vec<RequestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestEntry {
    id: String,
    method: String,
    result: Value,
    ms: Ms,
}

/// A run in memory: its record, its mail and the read receipts.
#[derive(Debug, Clone)]
pub struct RunState {
    pub rec: RunRecord,
    pub mail: Vec<Mail>,
    /// mailbox -> mail id -> read time.
    pub read: HashMap<String, BTreeMap<String, Ms>>,
}

impl RunState {
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.rec.items.iter().find(|i| i.id == id)
    }

    pub fn attempt(&self, id: &str) -> Option<&Attempt> {
        self.rec.attempts.iter().find(|a| a.id == id)
    }

    /// Workers that still hold their slot (open attempts).
    pub fn open_workers(&self) -> usize {
        self.rec
            .attempts
            .iter()
            .filter(|a| a.state.is_open())
            .count()
    }
}

/// What a new attempt needs.
#[derive(Debug, Clone)]
pub struct BeginAttempt {
    pub run: String,
    pub item: String,
    pub agent: String,
    pub place: String,
    pub retry_of: Option<String>,
    pub token_hash: String,
}

/// A message to record.
#[derive(Debug, Clone)]
pub struct NewMail {
    pub from: String,
    pub to: String,
    pub kind: MailType,
    pub subject: String,
    pub body: String,
    pub reply_to: Option<String>,
    pub thread: Option<String>,
    pub payload: Option<Value>,
    pub priority: Priority,
    pub fenced: bool,
    pub text: Option<MsgText>,
    pub body_text: Option<MsgText>,
}

impl NewMail {
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        kind: MailType,
        subject: impl Into<String>,
    ) -> Self {
        NewMail {
            from: from.into(),
            to: to.into(),
            kind,
            subject: subject.into(),
            body: String::new(),
            reply_to: None,
            thread: None,
            payload: None,
            priority: Priority::Normal,
            fenced: false,
            text: None,
            body_text: None,
        }
    }

    /// A message Pod writes: the subject is `text` (English in `subject`).
    pub fn pod(
        from: impl Into<String>,
        to: impl Into<String>,
        kind: MailType,
        text: MsgText,
    ) -> Self {
        let mut m = NewMail::new(from, to, kind, text.en());
        m.text = Some(text);
        m
    }

    /// A body Pod writes (English in `body`).
    pub fn body_text(mut self, text: MsgText) -> Self {
        self.body = text.en();
        self.body_text = Some(text);
        self
    }

    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.body = body.into();
        self
    }

    pub fn payload(mut self, v: Value) -> Self {
        self.payload = Some(v);
        self
    }
}

/// The result of [`Orch::admit`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Admitted {
    /// Start this attempt now (its record and launch data).
    pub start: Option<(Attempt, Value)>,
    /// Waiting attempts whose queue state changed (for events).
    pub changed: Vec<Attempt>,
}

/// Unconfirmed notifications per mailbox before Pod stops typing them
/// (until the mailbox is checked): the text may still sit in the input box.
pub const MAX_UNCONFIRMED_WAKES: u32 = 1;

/// The result of a `done` report.
#[derive(Debug, Clone, PartialEq)]
pub struct DoneResult {
    /// False for a late report of a superseded attempt (recorded only).
    pub accepted: bool,
    pub attempt: Attempt,
    pub mail: Mail,
    /// The same report was already recorded; nothing new was sent.
    pub repeated: bool,
}

pub struct Orch {
    root: PathBuf,
    index: Index,
    runs: BTreeMap<String, RunState>,
    pub limits: Limits,
    /// Problems found while loading (corrupt files, torn mail lines).
    pub notes: Vec<String>,
}

/// SHA-256 of an attempt token, as lowercase hex.
pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(token.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn id_number(id: &str) -> u64 {
    id.trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .parse()
        .unwrap_or(0)
}

impl Orch {
    pub fn root(&self) -> &Path {
        &self.root
    }

    // ── lookups ──────────────────────────────────────────────────────

    pub fn runs(&self) -> impl Iterator<Item = &RunState> {
        self.runs.values()
    }

    pub fn run(&self, id: &str) -> Result<&RunState> {
        self.runs
            .get(id)
            .ok_or_else(|| not_found(ErrorKind::RunNotFound, "run", id))
    }

    fn run_mut(&mut self, id: &str) -> Result<&mut RunState> {
        self.runs
            .get_mut(id)
            .ok_or_else(|| not_found(ErrorKind::RunNotFound, "run", id))
    }

    pub fn attempt(&self, id: &str) -> Result<&Attempt> {
        self.runs
            .values()
            .find_map(|r| r.attempt(id))
            .ok_or_else(|| not_found(ErrorKind::AttemptNotFound, "attempt", id))
    }

    fn attempt_run(&self, id: &str) -> Result<String> {
        Ok(self.attempt(id)?.run.clone())
    }

    fn attempt_mut(&mut self, id: &str) -> Result<&mut Attempt> {
        self.runs
            .values_mut()
            .find_map(|r| r.rec.attempts.iter_mut().find(|a| a.id == id))
            .ok_or_else(|| not_found(ErrorKind::AttemptNotFound, "attempt", id))
    }

    pub fn item(&self, id: &str) -> Result<&Item> {
        self.runs
            .values()
            .find_map(|r| r.item(id))
            .ok_or_else(|| not_found(ErrorKind::ItemNotFound, "item", id))
    }

    pub fn attempts(&self) -> impl Iterator<Item = &Attempt> {
        self.runs.values().flat_map(|r| r.rec.attempts.iter())
    }

    /// Runs this terminal coordinates.
    pub fn runs_of_terminal(&self, terminal: &str) -> Vec<String> {
        self.runs
            .values()
            .filter(|r| r.rec.run.coordinator_terminal.as_deref() == Some(terminal))
            .map(|r| r.rec.run.id.clone())
            .collect()
    }

    /// The terminal woken for mail to `mailbox`.
    pub fn mailbox_terminal(&self, mailbox: &str) -> Option<String> {
        if let Some(run) = mailbox.strip_prefix("run:") {
            return self.runs.get(run)?.rec.run.coordinator_terminal.clone();
        }
        if let Some(a) = mailbox.strip_prefix("attempt:") {
            let a = self.attempt(a).ok()?;
            return (a.worker != WorkerState::Released)
                .then(|| a.terminal.clone())
                .flatten();
        }
        None
    }

    /// Every mailbox read in `terminal`: the runs it coordinates and the
    /// workers it runs.
    pub fn terminal_mailboxes(&self, terminal: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .runs_of_terminal(terminal)
            .iter()
            .map(|r| run_mailbox(r))
            .collect();
        out.extend(
            self.attempts()
                .filter(|a| {
                    a.terminal.as_deref() == Some(terminal) && a.worker != WorkerState::Released
                })
                .map(|a| attempt_mailbox(&a.id)),
        );
        out
    }
}

/// First non-empty line of a spec, shortened: the default item title.
pub fn spec_title(spec: &str) -> String {
    let line = spec
        .lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("work item");
    let mut t: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        t.push('…');
    }
    t
}
