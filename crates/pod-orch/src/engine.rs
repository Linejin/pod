//! The orchestration state machine (docs/19 §4): runs, items with
//! dependencies, attempts with fences, mail with read receipts and wake-up
//! bookkeeping, request idempotency. Pure and synchronous: every mutation is
//! written to disk before it returns; the daemon wraps it in a lock and does
//! the terminal work (starting agents, typing notifications) itself.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::*;
use crate::resource::{self, ResourceLimits, Sample};
use crate::store;
use crate::text::{self, MsgText};

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

/// Constant-time-ish comparison of two hashes.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn id_number(id: &str) -> u64 {
    id.trim_start_matches(|c: char| c.is_ascii_alphabetic())
        .parse()
        .unwrap_or(0)
}

impl Orch {
    /// Load everything under `root` (`<state>\orch\runs`).
    pub fn load(root: &Path, limits: Limits, now: Ms) -> Orch {
        let mut notes = Vec::new();
        let mut index: Index =
            store::read_json(&store::index_json(root), now, &mut notes).unwrap_or_default();
        let mut runs = BTreeMap::new();
        if let Ok(entries) = std::fs::read_dir(root) {
            for e in entries.flatten() {
                let path = e.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                let Some(rec) =
                    store::read_json::<RunRecord>(&store::run_json(root, &name), now, &mut notes)
                else {
                    continue;
                };
                let load = match store::load_mail(&store::mail_jsonl(root, &name)) {
                    Ok(l) => l,
                    Err(e) => {
                        notes.push(format!("run {name}: cannot read mail: {e}"));
                        Default::default()
                    }
                };
                if load.truncated_bytes > 0 {
                    notes.push(format!(
                        "run {name}: dropped a torn last mail line ({} bytes)",
                        load.truncated_bytes
                    ));
                }
                if load.skipped_lines > 0 {
                    notes.push(format!(
                        "run {name}: skipped {} unreadable mail line(s)",
                        load.skipped_lines
                    ));
                }
                let mut st = RunState {
                    rec,
                    mail: Vec::new(),
                    read: HashMap::new(),
                };
                for line in load.lines {
                    match line {
                        MailLine::Mail { mail } => st.mail.push(*mail),
                        MailLine::Read { mailbox, ids, ms } => {
                            let r = st.read.entry(mailbox).or_default();
                            for id in ids {
                                r.entry(id).or_insert(ms);
                            }
                        }
                    }
                }
                let closed = Self::replay_done(&mut st);
                if !closed.is_empty() {
                    notes.push(format!(
                        "run {name}: applied the recorded done of {}",
                        closed.join(", ")
                    ));
                    if let Err(e) = store::write_json(&store::run_json(root, &name), &st.rec) {
                        notes.push(format!("run {name}: cannot save the replayed done: {e}"));
                    }
                }
                runs.insert(st.rec.run.id.clone(), st);
            }
        }
        // Never hand out an id that exists, whatever the index says.
        let mut bump = |prefix: &str, n: u64| {
            let c = index.counters.entry(prefix.to_string()).or_insert(0);
            *c = (*c).max(n);
        };
        for st in runs.values() {
            bump("r", id_number(&st.rec.run.id));
            for i in &st.rec.items {
                bump("i", id_number(&i.id));
            }
            for a in &st.rec.attempts {
                bump("a", id_number(&a.id));
            }
            for m in &st.mail {
                bump("m", id_number(&m.id));
            }
        }
        Orch {
            root: root.to_path_buf(),
            index,
            runs,
            limits,
            notes,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn save_index(&self) -> Result<()> {
        store::write_json(&store::index_json(&self.root), &self.index).map_err(io)
    }

    fn next_id(&mut self, prefix: &str) -> Result<String> {
        let c = self.index.counters.entry(prefix.to_string()).or_insert(0);
        *c += 1;
        let id = format!("{prefix}{c}");
        self.save_index()?;
        Ok(id)
    }

    fn save_run(&self, run: &str) -> Result<()> {
        let st = self
            .runs
            .get(run)
            .ok_or_else(|| not_found(ErrorKind::RunNotFound, "run", run))?;
        store::write_json(&store::run_json(&self.root, run), &st.rec).map_err(io)
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

    // ── runs and items ───────────────────────────────────────────────

    /// A new run coordinated by `terminal` (and `attempt`, when the
    /// coordinator is a worker: the run inherits its depth).
    pub fn create_run(
        &mut self,
        objective: &str,
        terminal: Option<&str>,
        attempt: Option<&str>,
        now: Ms,
    ) -> Result<Run> {
        let depth = match attempt {
            Some(a) => self.attempt(a)?.depth,
            None => 0,
        };
        let objective = objective.trim();
        if objective.is_empty() {
            return Err(OrchError::new(
                ErrorKind::InvalidParams,
                "the objective must not be empty",
            ));
        }
        let id = self.next_id("r")?;
        let run = Run {
            id: id.clone(),
            objective: objective.to_string(),
            coordinator_terminal: terminal.map(str::to_string),
            coordinator_attempt: attempt.map(str::to_string),
            depth,
            created_ms: now,
        };
        self.runs.insert(
            id.clone(),
            RunState {
                rec: RunRecord {
                    version: RUN_VERSION,
                    run: run.clone(),
                    items: Vec::new(),
                    attempts: Vec::new(),
                    wakes: BTreeMap::new(),
                },
                mail: Vec::new(),
                read: HashMap::new(),
            },
        );
        self.save_run(&id)?;
        Ok(run)
    }

    fn deps_state(st: &RunState, deps: &[String]) -> ItemState {
        let done = deps
            .iter()
            .all(|d| st.item(d).is_some_and(|i| i.state == ItemState::Completed));
        if done {
            ItemState::Ready
        } else {
            ItemState::Pending
        }
    }

    pub fn add_item(
        &mut self,
        run: &str,
        title: Option<&str>,
        spec: &str,
        deps: &[String],
        parent: Option<&str>,
        now: Ms,
    ) -> Result<Item> {
        if spec.trim().is_empty() {
            return Err(OrchError::new(
                ErrorKind::InvalidParams,
                "the spec must not be empty",
            ));
        }
        {
            let st = self.run(run)?;
            for d in deps.iter().map(String::as_str).chain(parent) {
                if st.item(d).is_none() {
                    return Err(OrchError::new(
                        ErrorKind::ItemNotFound,
                        format!("no item {d} in run {run}"),
                    ));
                }
            }
        }
        let id = self.next_id("i")?;
        let st = self.run_mut(run)?;
        let title = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| spec_title(spec));
        let item = Item {
            id,
            run: run.to_string(),
            title,
            spec: spec.to_string(),
            deps: deps.to_vec(),
            parent: parent.map(str::to_string),
            state: Self::deps_state(st, deps),
            generation: 0,
            start_failures: 0,
            circuit_open: false,
            attempt: None,
            gates: Vec::new(),
            created_ms: now,
            updated_ms: now,
        };
        st.rec.items.push(item.clone());
        self.save_run(run)?;
        Ok(item)
    }

    /// Open the item again: clears the breaker; completed/failed items become
    /// ready (or pending) for a new attempt.
    pub fn reopen_item(&mut self, item: &str, now: Ms) -> Result<Item> {
        let run = self.item(item)?.run.clone();
        let st = self.run_mut(&run)?;
        let deps = st.item(item).map(|i| i.deps.clone()).unwrap_or_default();
        let state = Self::deps_state(st, &deps);
        let open_attempt = st
            .rec
            .attempts
            .iter()
            .any(|a| a.item == item && a.state.is_open());
        let it = st.rec.items.iter_mut().find(|i| i.id == item).unwrap();
        it.start_failures = 0;
        it.circuit_open = false;
        if !open_attempt && it.gates.is_empty() {
            it.state = state;
        }
        it.updated_ms = now;
        let it = it.clone();
        self.save_run(&run)?;
        Ok(it)
    }

    /// A gate now blocks this item (docs/19 §4.5).
    pub fn block_item(&mut self, item: &str, gate: &str, now: Ms) -> Result<Item> {
        let run = self.item(item)?.run.clone();
        let st = self.run_mut(&run)?;
        let it = st.rec.items.iter_mut().find(|i| i.id == item).unwrap();
        if matches!(it.state, ItemState::Completed | ItemState::Failed) {
            let state = if it.state == ItemState::Completed {
                "completed"
            } else {
                "failed"
            };
            return Err(OrchError::new(
                ErrorKind::InvalidState,
                format!("item {item} is already {state}"),
            ));
        }
        if !it.gates.iter().any(|g| g == gate) {
            it.gates.push(gate.to_string());
        }
        it.state = ItemState::Blocked;
        it.updated_ms = now;
        let it = it.clone();
        self.save_run(&run)?;
        Ok(it)
    }

    /// The gate was decided or expired. When no gate is left the item goes
    /// back to `assigned` (its attempt is open) or `ready`/`pending`.
    /// Idempotent; returns `None` when nothing changed.
    pub fn unblock_item(&mut self, item: &str, gate: &str, now: Ms) -> Result<Option<Item>> {
        let run = self.item(item)?.run.clone();
        let st = self.run_mut(&run)?;
        let Some(idx) = st.rec.items.iter().position(|i| i.id == item) else {
            return Ok(None);
        };
        if !st.rec.items[idx].gates.iter().any(|g| g == gate) {
            return Ok(None);
        }
        let deps = st.rec.items[idx].deps.clone();
        let next = Self::deps_state(st, &deps);
        let (cur, gen_) = (
            st.rec.items[idx].attempt.clone(),
            st.rec.items[idx].generation,
        );
        let open = st
            .rec
            .attempts
            .iter()
            .any(|a| Some(&a.id) == cur.as_ref() && a.generation == gen_ && a.state.is_open());
        let it = &mut st.rec.items[idx];
        it.gates.retain(|g| g != gate);
        if it.gates.is_empty() && it.state == ItemState::Blocked {
            it.state = if open { ItemState::Assigned } else { next };
        }
        it.updated_ms = now;
        let it = it.clone();
        self.save_run(&run)?;
        Ok(Some(it))
    }

    /// Items whose dependencies are now all completed become ready.
    fn promote(st: &mut RunState, now: Ms) {
        let completed: Vec<String> = st
            .rec
            .items
            .iter()
            .filter(|i| i.state == ItemState::Completed)
            .map(|i| i.id.clone())
            .collect();
        for it in st.rec.items.iter_mut() {
            if it.state == ItemState::Pending && it.deps.iter().all(|d| completed.contains(d)) {
                it.state = ItemState::Ready;
                it.updated_ms = now;
            }
        }
    }

    // ── attempts ─────────────────────────────────────────────────────

    /// Check the limits and record a new attempt (`starting`).
    pub fn begin_attempt(&mut self, b: BeginAttempt, now: Ms) -> Result<Attempt> {
        let limits = self.limits;
        let (depth, item_state, circuit, current) = {
            let st = self.run(&b.run)?;
            let it = st.item(&b.item).ok_or_else(|| {
                OrchError::new(
                    ErrorKind::ItemNotFound,
                    format!("no item {} in run {}", b.item, b.run),
                )
            })?;
            (
                st.rec.run.depth + 1,
                it.state,
                it.circuit_open,
                it.attempt.clone(),
            )
        };
        if depth > limits.max_depth {
            return Err(OrchError::new(
                ErrorKind::DepthExceeded,
                format!(
                    "nesting depth {depth} is over the limit {}: do this work yourself instead of starting a worker",
                    limits.max_depth
                ),
            ));
        }
        if circuit {
            return Err(OrchError::new(
                ErrorKind::CircuitOpen,
                format!(
                    "item {} failed to start {} times in a row; fix the cause, then `pod orch item reopen {}`",
                    b.item, limits.start_strikes, b.item
                ),
            ));
        }
        match item_state {
            ItemState::Pending => {
                return Err(OrchError::new(
                    ErrorKind::DepsNotDone,
                    format!("item {} waits for its dependencies", b.item),
                ));
            }
            ItemState::Completed | ItemState::Failed if b.retry_of.is_none() => {
                return Err(OrchError::new(
                    ErrorKind::InvalidState,
                    format!("item {} is finished; reopen it or pass --retry-of", b.item),
                ));
            }
            ItemState::Blocked if b.retry_of.is_none() => {
                return Err(OrchError::new(
                    ErrorKind::InvalidState,
                    format!(
                        "item {} waits for a decision (gate {}); decide it first",
                        b.item,
                        self.item(&b.item)
                            .map(|i| i.gates.join(", "))
                            .unwrap_or_default()
                    ),
                ));
            }
            ItemState::Assigned if b.retry_of.is_none() => {
                return Err(OrchError::new(
                    ErrorKind::InvalidState,
                    format!(
                        "item {} already has attempt {}; pass --retry-of {} to replace it",
                        b.item,
                        current.as_deref().unwrap_or("?"),
                        current.as_deref().unwrap_or("<attempt>")
                    ),
                ));
            }
            _ => {}
        }
        if let Some(old) = &b.retry_of {
            let a = self.attempt(old)?;
            if a.item != b.item {
                return Err(OrchError::new(
                    ErrorKind::InvalidParams,
                    format!("attempt {old} belongs to item {}, not {}", a.item, b.item),
                ));
            }
        }
        // Worker limits (per run and global) and resources are not refused
        // here: the daemon queues the attempt (`enqueue`, `admit`).
        if let Some(old) = &b.retry_of {
            let a = self.attempt_mut(old)?;
            if a.state.is_open() {
                a.state = AttemptState::Abandoned;
                a.ended_ms = Some(now);
                a.error = Some("replaced by a retry".into());
                a.queue = None;
                a.launch = None;
                if a.worker == WorkerState::Active {
                    // The old terminal stays with a person until released.
                    a.worker = WorkerState::Kept;
                }
            }
        }
        let id = self.next_id("a")?;
        let st = self.run_mut(&b.run)?;
        let it = st.rec.items.iter_mut().find(|i| i.id == b.item).unwrap();
        it.generation += 1;
        it.state = if it.gates.is_empty() {
            ItemState::Assigned
        } else {
            ItemState::Blocked
        };
        it.attempt = Some(id.clone());
        it.updated_ms = now;
        let a = Attempt {
            id,
            run: b.run.clone(),
            item: b.item.clone(),
            generation: it.generation,
            state: AttemptState::Starting,
            worker: WorkerState::Active,
            agent: b.agent,
            place: b.place,
            depth,
            token_hash: b.token_hash,
            task: None,
            terminal: None,
            brief_path: None,
            retry_of: b.retry_of,
            created_ms: now,
            ready_ms: None,
            ended_ms: None,
            release_at_ms: None,
            released_ms: None,
            log_path: None,
            body: None,
            files: Vec::new(),
            report: None,
            error: None,
            seen_turn: 0,
            escalated_turn: None,
            stalled_at_output_ms: None,
            reopens: 0,
            queue: None,
            launch: None,
            waiting_on: Vec::new(),
        };
        st.rec.attempts.push(a.clone());
        self.save_run(&b.run)?;
        Ok(a)
    }

    // ── resource queue (docs/19 §5.1) ────────────────────────────────

    /// Put a new attempt into the resource queue with what the daemon needs
    /// to start it later.
    pub fn enqueue(
        &mut self,
        attempt: &str,
        launch: Value,
        disk_path: Option<String>,
        now: Ms,
    ) -> Result<Attempt> {
        self.update_attempt(attempt, |a| {
            a.queue = Some(QueueState {
                since_ms: now,
                kind: WaitKind::Queued,
                reason: "자원 확인을 기다리는 중이에요.".into(),
                position: 0,
                disk_path,
            });
            a.launch = Some(launch);
        })
    }

    /// Queued attempts in FIFO order.
    pub fn queue(&self) -> Vec<&Attempt> {
        resource::queue_order(self.attempts())
    }

    /// Active workers (open, not queued) over all runs.
    pub fn active_workers(&self) -> usize {
        self.attempts().filter(|a| a.is_active()).count()
    }

    /// One pass of the resource gate. Stores the changed verdicts (kind or
    /// position) of waiting attempts and takes the admitted one out of the queue with its new
    /// token hash (the token is made only now, so none was ever stored).
    /// Returns the admitted attempt with its launch data, and every attempt
    /// whose queue state changed.
    pub fn admit(
        &mut self,
        limits: &ResourceLimits,
        sample: &Sample,
        free_disk: impl FnMut(&str) -> Option<u64>,
        token_hash: Option<String>,
        now: Ms,
    ) -> Result<Admitted> {
        let all: Vec<&Attempt> = self.attempts().collect();
        let plan = resource::plan(&all, limits, sample, free_disk, now);
        let mut out = Admitted::default();
        let mut dirty: Vec<String> = Vec::new();
        for v in &plan.waiting {
            let Ok(a) = self.attempt_mut(&v.attempt) else {
                continue;
            };
            let Some(q) = a.queue.as_mut() else { continue };
            // The reason text carries live numbers (CPU %, free GiB): store
            // it only when the kind or place changes, so a waiting queue does
            // not rewrite run.json every pass. `orch.queue` shows fresh text.
            if q.kind != v.kind || q.position != v.position {
                q.kind = v.kind;
                q.reason = v.reason.clone();
                q.position = v.position;
                let a = a.clone();
                if !dirty.contains(&a.run) {
                    dirty.push(a.run.clone());
                }
                out.changed.push(a.public());
            }
        }
        if let (Some(id), Some(hash)) = (&plan.start, token_hash) {
            let a = self.attempt_mut(id)?;
            a.queue = None;
            a.token_hash = hash;
            let launch = a.launch.take().unwrap_or(Value::Null);
            let a = a.clone();
            if !dirty.contains(&a.run) {
                dirty.push(a.run.clone());
            }
            out.start = Some((a.public(), launch));
        }
        for run in dirty {
            self.save_run(&run)?;
        }
        Ok(out)
    }

    /// The worker's task and terminal are up.
    pub fn attach_worker(
        &mut self,
        attempt: &str,
        task: Option<&str>,
        terminal: &str,
        brief: Option<&str>,
        turn: u64,
    ) -> Result<Attempt> {
        let run = self.attempt_run(attempt)?;
        let a = self.attempt_mut(attempt)?;
        a.task = task.map(str::to_string);
        a.terminal = Some(terminal.to_string());
        a.brief_path = brief.map(str::to_string);
        a.seen_turn = turn;
        let a = a.clone();
        self.save_run(&run)?;
        Ok(a)
    }

    /// The worker could not be started. Returns whether this opened the
    /// item's circuit breaker (the item is then `failed`).
    pub fn start_failed(&mut self, attempt: &str, error: &str, now: Ms) -> Result<bool> {
        let strikes = self.limits.start_strikes;
        let run = self.attempt_run(attempt)?;
        let st = self.run_mut(&run)?;
        let a = st
            .rec
            .attempts
            .iter_mut()
            .find(|a| a.id == attempt)
            .unwrap();
        a.state = AttemptState::StartFailed;
        a.queue = None;
        a.launch = None;
        a.worker = WorkerState::Released;
        a.ended_ms = Some(now);
        a.released_ms = Some(now);
        a.error = Some(error.to_string());
        let (item, gen_) = (a.item.clone(), a.generation);
        let it = st.rec.items.iter_mut().find(|i| i.id == item).unwrap();
        let mut tripped = false;
        if it.generation == gen_ {
            it.start_failures += 1;
            it.updated_ms = now;
            if it.start_failures >= strikes {
                it.state = ItemState::Failed;
                it.circuit_open = true;
                tripped = true;
            } else {
                it.state = ItemState::Ready;
            }
        }
        self.save_run(&run)?;
        Ok(tripped)
    }

    /// The agent was seen working: the start succeeded (resets the breaker).
    pub fn mark_ready(&mut self, attempt: &str, now: Ms) -> Result<bool> {
        let run = self.attempt_run(attempt)?;
        let st = self.run_mut(&run)?;
        let a = st
            .rec
            .attempts
            .iter_mut()
            .find(|a| a.id == attempt)
            .unwrap();
        if a.state != AttemptState::Starting {
            return Ok(false);
        }
        a.state = AttemptState::Ready;
        a.ready_ms = Some(now);
        let (item, gen_) = (a.item.clone(), a.generation);
        if let Some(it) = st
            .rec
            .items
            .iter_mut()
            .find(|i| i.id == item && i.generation == gen_)
        {
            it.start_failures = 0;
        }
        self.save_run(&run)?;
        Ok(true)
    }

    /// Check an attempt's token (`ATTEMPT_FENCED` on mismatch).
    pub fn verify_token(&self, attempt: &str, token: Option<&str>) -> Result<&Attempt> {
        let a = self.attempt(attempt)?;
        let ok = token.is_some_and(|t| same(&hash_token(t), &a.token_hash));
        if !ok {
            return Err(OrchError::new(
                ErrorKind::AttemptFenced,
                format!("the token does not belong to attempt {attempt} (POD_ATTEMPT_TOKEN)"),
            ));
        }
        Ok(a)
    }

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
    fn replay_done(st: &mut RunState) -> Vec<String> {
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

    /// End an open attempt without a report (stop, lost, unknown,
    /// abandon). The item becomes ready again when this was its current
    /// attempt. Returns false when the attempt was not open.
    pub fn end_attempt(
        &mut self,
        attempt: &str,
        state: AttemptState,
        why: &str,
        worker: Option<WorkerState>,
        release_after_ms: Option<Ms>,
        now: Ms,
    ) -> Result<bool> {
        let run = self.attempt_run(attempt)?;
        let st = self.run_mut(&run)?;
        let a = st
            .rec
            .attempts
            .iter_mut()
            .find(|a| a.id == attempt)
            .unwrap();
        if !a.state.is_open() {
            return Ok(false);
        }
        a.state = state;
        a.ended_ms = Some(now);
        a.error = Some(why.to_string());
        a.queue = None;
        a.launch = None;
        match worker {
            Some(w) => a.worker = w,
            None if a.worker == WorkerState::Active => {
                a.worker = WorkerState::Finished;
                a.release_at_ms = release_after_ms.map(|d| now + d);
            }
            None => {}
        }
        let (item, gen_) = (a.item.clone(), a.generation);
        if let Some(it) = st
            .rec
            .items
            .iter_mut()
            .find(|i| i.id == item && i.generation == gen_ && i.state == ItemState::Assigned)
        {
            it.state = ItemState::Ready;
            it.updated_ms = now;
        }
        self.save_run(&run)?;
        Ok(true)
    }

    /// Set the worker's resource state (`keep`, a person typing, release).
    pub fn set_worker(
        &mut self,
        attempt: &str,
        w: WorkerState,
        log_path: Option<&str>,
        now: Ms,
    ) -> Result<Attempt> {
        let run = self.attempt_run(attempt)?;
        let a = self.attempt_mut(attempt)?;
        a.worker = w;
        match w {
            WorkerState::Released => {
                a.released_ms = Some(now);
                a.release_at_ms = None;
                if let Some(p) = log_path {
                    a.log_path = Some(p.to_string());
                }
            }
            WorkerState::Kept => a.release_at_ms = None,
            _ => {}
        }
        let a = a.clone();
        self.save_run(&run)?;
        Ok(a)
    }

    /// Supervisor bookkeeping (turn seen, escalation marks).
    pub fn update_attempt(
        &mut self,
        attempt: &str,
        f: impl FnOnce(&mut Attempt),
    ) -> Result<Attempt> {
        let run = self.attempt_run(attempt)?;
        let a = self.attempt_mut(attempt)?;
        f(a);
        let a = a.clone();
        self.save_run(&run)?;
        Ok(a)
    }

    // ── waiting for other items (docs/19 §18.2) ─────────────────────

    /// An open worker waits for `items` of its run (not its own). Returns
    /// each item with its state; only unfinished ones are recorded (none
    /// unfinished: nothing to wait for, the record is cleared).
    pub fn wait_for_items(
        &mut self,
        attempt: &str,
        items: &[String],
    ) -> Result<Vec<(String, ItemState)>> {
        let a = self.attempt(attempt)?.clone();
        if !a.state.is_open() {
            return Err(OrchError::new(
                ErrorKind::InvalidState,
                format!("worker {attempt} is {:?}, not running", a.state),
            ));
        }
        if items.is_empty() {
            return Err(OrchError::new(
                ErrorKind::InvalidParams,
                "name at least one item (--item)",
            ));
        }
        let mut states = Vec::new();
        for id in items {
            if *id == a.item {
                return Err(OrchError::new(
                    ErrorKind::InvalidParams,
                    format!("{id} is this worker's own item"),
                ));
            }
            let it = self.item(id)?;
            if it.run != a.run {
                return Err(OrchError::new(
                    ErrorKind::InvalidParams,
                    format!("{id} is not an item of run {}", a.run),
                ));
            }
            if !states.iter().any(|(i, _)| i == id) {
                states.push((id.clone(), it.state));
            }
        }
        let open: Vec<String> = states
            .iter()
            .filter(|(_, s)| !s.is_finished())
            .map(|(i, _)| i.clone())
            .collect();
        self.update_attempt(attempt, |x| x.waiting_on = open)?;
        Ok(states)
    }

    /// Open workers whose awaited items have all finished: their wait is
    /// cleared and they are returned with the items' final states.
    pub fn settle_waits(&mut self) -> Result<Vec<(Attempt, ItemStates)>> {
        let mut due = Vec::new();
        for a in self.attempts().filter(|a| !a.waiting_on.is_empty()) {
            let states: Vec<(String, ItemState)> = a
                .waiting_on
                .iter()
                .map(|i| {
                    (
                        i.clone(),
                        self.item(i).map(|it| it.state).unwrap_or(ItemState::Failed),
                    )
                })
                .collect();
            if !a.state.is_open() || states.iter().all(|(_, s)| s.is_finished()) {
                due.push((a.id.clone(), a.state.is_open(), states));
            }
        }
        let mut out = Vec::new();
        for (id, open, states) in due {
            let a = self.update_attempt(&id, |x| x.waiting_on.clear())?;
            if open {
                out.push((a, states));
            }
        }
        Ok(out)
    }

    /// Finished workers whose release time has come.
    pub fn due_releases(&self, now: Ms) -> Vec<String> {
        self.attempts()
            .filter(|a| {
                a.worker == WorkerState::Finished && a.release_at_ms.is_some_and(|t| t <= now)
            })
            .map(|a| a.id.clone())
            .collect()
    }

    // ── mail ─────────────────────────────────────────────────────────

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

    // ── wake-ups ─────────────────────────────────────────────────────

    pub fn mailbox_run(&self, mailbox: &str) -> Option<String> {
        if let Some(r) = mailbox.strip_prefix("run:") {
            return self.runs.contains_key(r).then(|| r.to_string());
        }
        mailbox
            .strip_prefix("attempt:")
            .and_then(|a| self.attempt(a).ok())
            .map(|a| a.run.clone())
    }

    pub fn notified(&self, mailbox: &str) -> Option<Ms> {
        let run = self.mailbox_run(mailbox)?;
        self.runs.get(&run)?.rec.wakes.get(mailbox)?.notified_ms
    }

    fn wake_state(&self, mailbox: &str) -> Option<&WakeState> {
        let run = self.mailbox_run(mailbox)?;
        self.runs.get(&run)?.rec.wakes.get(mailbox)
    }

    /// A notification for this mailbox is being typed (not confirmed yet).
    pub fn wake_pending(&self, mailbox: &str) -> bool {
        self.wake_state(mailbox)
            .is_some_and(|w| w.pending_ms.is_some())
    }

    /// Notifications typed but not taken since the last check.
    pub fn wake_unconfirmed(&self, mailbox: &str) -> u32 {
        self.wake_state(mailbox).map_or(0, |w| w.unconfirmed)
    }

    /// The mailbox has waking mail, no notification delivered or on its
    /// way, and has not used up its unconfirmed tries.
    pub fn wants_wake(&self, mailbox: &str) -> bool {
        let w = self.wake_state(mailbox);
        w.is_none_or(|w| {
            w.notified_ms.is_none()
                && w.pending_ms.is_none()
                && w.unconfirmed < MAX_UNCONFIRMED_WAKES
        }) && self.unread_waking(mailbox) > 0
    }

    /// orch-02, step 1: a notification is about to be typed for these
    /// mailboxes. Nothing is persisted; [`Orch::wake_delivered`] or
    /// [`Orch::wake_failed`] settles it.
    pub fn wake_scheduled(&mut self, mailboxes: &[String], now: Ms) -> Result<()> {
        for mb in mailboxes {
            if let Some(run) = self.mailbox_run(mb) {
                self.run_mut(&run)?
                    .rec
                    .wakes
                    .entry(mb.clone())
                    .or_default()
                    .pending_ms = Some(now);
            }
        }
        Ok(())
    }

    /// orch-02, step 2: the agent took the notification (or nothing can
    /// confirm it). Persisted: no second notification until a check.
    pub fn wake_delivered(&mut self, mailboxes: &[String], now: Ms) -> Result<()> {
        for mb in mailboxes {
            if let Some(run) = self.mailbox_run(mb) {
                let w = self.run_mut(&run)?.rec.wakes.entry(mb.clone()).or_default();
                w.pending_ms = None;
                w.notified_ms = Some(now);
                self.save_run(&run)?;
            }
        }
        Ok(())
    }

    /// orch-02: the notification was not taken (`unconfirmed`: typed but
    /// no reaction; otherwise not typed at all). The mailbox wants a wake
    /// again; unconfirmed tries are limited ([`MAX_UNCONFIRMED_WAKES`]).
    pub fn wake_failed(&mut self, mailboxes: &[String], unconfirmed: bool) -> Result<()> {
        for mb in mailboxes {
            if let Some(run) = self.mailbox_run(mb)
                && let Some(w) = self.run_mut(&run)?.rec.wakes.get_mut(mb)
            {
                w.pending_ms = None;
                if unconfirmed {
                    w.unconfirmed += 1;
                }
            }
        }
        Ok(())
    }

    /// Typed and delivered in one step (tests and callers that cannot
    /// confirm).
    pub fn mark_notified(&mut self, mailboxes: &[String], now: Ms) -> Result<()> {
        self.wake_scheduled(mailboxes, now)?;
        self.wake_delivered(mailboxes, now)
    }

    fn clear_notified(&mut self, mailbox: &str) -> Result<()> {
        if let Some(run) = self.mailbox_run(mailbox) {
            let st = self.run_mut(&run)?;
            if st.rec.wakes.remove(mailbox).is_some() {
                self.save_run(&run)?;
            }
        }
        Ok(())
    }

    // ── request ids ──────────────────────────────────────────────────

    /// The stored result of an earlier call with this request id.
    pub fn request(&self, id: &str, method: &str) -> Result<Option<Value>> {
        match self.index.requests.iter().find(|r| r.id == id) {
            Some(r) if r.method == method => Ok(Some(r.result.clone())),
            Some(r) => Err(OrchError::new(
                ErrorKind::InvalidParams,
                format!("request_id {id} was used for {}", r.method),
            )),
            None => Ok(None),
        }
    }

    /// Remember a result for a request id.
    pub fn remember(&mut self, id: &str, method: &str, result: Value, now: Ms) -> Result<()> {
        self.index.requests.retain(|r| r.id != id);
        self.index.requests.push(RequestEntry {
            id: id.to_string(),
            method: method.to_string(),
            result,
            ms: now,
        });
        let n = self.index.requests.len();
        if n > MAX_REQUESTS {
            self.index.requests.drain(..n - MAX_REQUESTS);
        }
        self.save_index()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "pod-orch-eng-{tag}-{}-{nanos}-{seq}",
            std::process::id()
        ))
    }

    struct T {
        dir: PathBuf,
        o: Orch,
    }

    impl Drop for T {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn setup(tag: &str) -> T {
        let dir = temp(tag);
        T {
            o: Orch::load(&dir, Limits::default(), 1),
            dir,
        }
    }

    fn begin(
        o: &mut Orch,
        run: &str,
        item: &str,
        token: &str,
        retry_of: Option<&str>,
    ) -> Result<Attempt> {
        o.begin_attempt(
            BeginAttempt {
                run: run.into(),
                item: item.into(),
                agent: "codex".into(),
                place: "new".into(),
                retry_of: retry_of.map(str::to_string),
                token_hash: hash_token(token),
            },
            10,
        )
    }

    /// docs/19 §18.2: a worker waits for other items of its run; the wait
    /// settles once they all finished (completed or failed), survives a
    /// reload, and is dropped when the worker ends first.
    #[test]
    fn waiting_for_other_items() {
        let mut t = setup("wait");
        let o = &mut t.o;
        o.create_run("r", Some("t1"), None, 5).unwrap();
        for spec in ["a", "b", "c"] {
            o.add_item("r1", None, spec, &[], None, 5).unwrap();
        }
        begin(o, "r1", "i1", "k1", None).unwrap();
        begin(o, "r1", "i2", "k2", None).unwrap();
        begin(o, "r1", "i3", "k3", None).unwrap();
        // Bad requests.
        assert_eq!(
            o.wait_for_items("a2", &[]).unwrap_err().kind,
            ErrorKind::InvalidParams
        );
        assert_eq!(
            o.wait_for_items("a2", &["i2".into()]).unwrap_err().kind,
            ErrorKind::InvalidParams,
            "own item"
        );
        assert_eq!(
            o.wait_for_items("a2", &["i9".into()]).unwrap_err().kind,
            ErrorKind::ItemNotFound
        );
        // a2 waits for i1 and i3.
        let st = o
            .wait_for_items("a2", &["i1".into(), "i3".into(), "i1".into()])
            .unwrap();
        assert_eq!(
            st,
            vec![
                ("i1".to_string(), ItemState::Assigned),
                ("i3".to_string(), ItemState::Assigned)
            ]
        );
        assert_eq!(o.attempt("a2").unwrap().waiting_on, vec!["i1", "i3"]);
        assert!(o.settle_waits().unwrap().is_empty());
        o.done("a1", Some("k1"), true, "ok", &[], None, None, 20)
            .unwrap();
        assert!(o.settle_waits().unwrap().is_empty(), "i3 still runs");
        // Survives a reload.
        let mut o2 = Orch::load(&t.dir, Limits::default(), 30);
        assert_eq!(o2.attempt("a2").unwrap().waiting_on, vec!["i1", "i3"]);
        o2.done("a3", Some("k3"), false, "broke", &[], None, None, 40)
            .unwrap();
        let due = o2.settle_waits().unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0.id, "a2");
        assert_eq!(
            due[0].1,
            vec![
                ("i1".to_string(), ItemState::Completed),
                ("i3".to_string(), ItemState::Failed)
            ]
        );
        assert!(o2.attempt("a2").unwrap().waiting_on.is_empty());
        assert!(o2.settle_waits().unwrap().is_empty(), "settled once");
        // Everything already finished: nothing is recorded.
        assert_eq!(
            o2.wait_for_items("a2", &["i1".into()]).unwrap(),
            vec![("i1".to_string(), ItemState::Completed)]
        );
        assert!(o2.attempt("a2").unwrap().waiting_on.is_empty());
        // A finished worker cannot wait; one that ends while waiting is dropped silently.
        assert_eq!(
            o2.wait_for_items("a1", &["i2".into()]).unwrap_err().kind,
            ErrorKind::InvalidState
        );
        o2.add_item("r1", None, "d", &[], None, 50).unwrap();
        o2.wait_for_items("a2", &["i4".into()]).unwrap();
        o2.done(
            "a2",
            Some("k2"),
            true,
            "gave up waiting",
            &[],
            None,
            None,
            60,
        )
        .unwrap();
        assert!(o2.settle_waits().unwrap().is_empty());
        assert!(o2.attempt("a2").unwrap().waiting_on.is_empty());
    }

    #[test]
    fn ids_items_deps_and_reload() {
        let mut t = setup("deps");
        let o = &mut t.o;
        let r = o.create_run("ship it", Some("t1"), None, 5).unwrap();
        assert_eq!((r.id.as_str(), r.depth), ("r1", 0));
        let a = o
            .add_item("r1", None, "# Build the thing\nmore", &[], None, 5)
            .unwrap();
        assert_eq!(
            (a.id.as_str(), a.title.as_str(), a.state),
            ("i1", "Build the thing", ItemState::Ready)
        );
        let b = o
            .add_item("r1", Some("Test"), "test it", &["i1".into()], None, 5)
            .unwrap();
        assert_eq!(b.state, ItemState::Pending);
        assert_eq!(
            o.add_item("r1", None, "x", &["i9".into()], None, 5)
                .unwrap_err()
                .kind,
            ErrorKind::ItemNotFound
        );
        assert_eq!(
            begin(o, "r1", "i2", "k", None).unwrap_err().kind,
            ErrorKind::DepsNotDone
        );
        let at = begin(o, "r1", "i1", "tok", None).unwrap();
        assert_eq!((at.id.as_str(), at.generation, at.depth), ("a1", 1, 1));
        let d = o
            .done(
                "a1",
                Some("tok"),
                true,
                "did it",
                &[],
                None,
                Some(600_000),
                20,
            )
            .unwrap();
        assert!(d.accepted);
        assert_eq!(d.attempt.worker, WorkerState::Finished);
        assert_eq!(d.attempt.release_at_ms, Some(600_020));
        assert_eq!(
            o.item("i2").unwrap().state,
            ItemState::Ready,
            "dependent promoted"
        );
        assert_eq!(d.mail.recipients, vec!["run:r1".to_string()]);
        // The subject is data too (docs/19 §16.3), and survives a reload.
        let t1 = d.mail.text.clone().expect("structured subject");
        assert_eq!(
            (t1.kind.as_str(), t1.p("outcome"), t1.p("attempt")),
            (text::DONE, "succeeded", "a1")
        );
        assert_eq!(d.mail.subject, t1.en());
        assert!(
            d.mail.body_text.is_none(),
            "the worker's own body is not Pod's text"
        );
        // Everything survives a reload; counters continue.
        let o2 = Orch::load(&t.dir, Limits::default(), 30);
        assert!(o2.notes.is_empty(), "{:?}", o2.notes);
        assert_eq!(o2.item("i1").unwrap().state, ItemState::Completed);
        assert_eq!(o2.attempt("a1").unwrap().token_hash, hash_token("tok"));
        assert_eq!(o2.unread("run:r1").len(), 1);
        assert_eq!(o2.unread("run:r1")[0].text, d.mail.text);
        let mut o2 = o2;
        assert_eq!(o2.create_run("next", None, None, 31).unwrap().id, "r2");
    }

    #[test]
    fn fences_token_and_generation() {
        let mut t = setup("fence");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.add_item("r1", None, "spec", &[], None, 1).unwrap();
        begin(o, "r1", "i1", "old", None).unwrap();
        assert_eq!(
            begin(o, "r1", "i1", "k", None).unwrap_err().kind,
            ErrorKind::InvalidState
        );
        let a2 = begin(o, "r1", "i1", "new", Some("a1")).unwrap();
        assert_eq!(a2.generation, 2);
        assert_eq!(o.attempt("a1").unwrap().state, AttemptState::Abandoned);
        // Wrong token: refused outright.
        let e = o
            .done("a1", Some("new"), true, "", &[], None, None, 5)
            .unwrap_err();
        assert_eq!(e.kind, ErrorKind::AttemptFenced);
        assert_eq!(
            o.done("a1", None, true, "", &[], None, None, 5)
                .unwrap_err()
                .kind,
            ErrorKind::AttemptFenced
        );
        // Late done of the old generation: recorded, item untouched.
        let late = o
            .done("a1", Some("old"), true, "late", &[], None, None, 5)
            .unwrap();
        assert!(!late.accepted);
        assert!(late.mail.fenced);
        assert_eq!(
            late.mail.text.as_ref().map(|t| t.kind.as_str()),
            Some(text::DONE_LATE)
        );
        assert_eq!(
            late.mail.subject,
            "late done (succeeded) from superseded attempt a1; ignored"
        );
        assert_eq!(o.item("i1").unwrap().state, ItemState::Assigned);
        assert_eq!(o.item("i1").unwrap().attempt.as_deref(), Some("a2"));
        let ok = o
            .done("a2", Some("new"), false, "nope", &[], None, None, 6)
            .unwrap();
        assert!(ok.accepted);
        assert_eq!(o.item("i1").unwrap().state, ItemState::Failed);
        assert_eq!(
            o.done("a2", Some("new"), true, "again", &[], None, None, 7)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidState
        );
        // A worker-reported failure is not a start failure.
        assert_eq!(o.item("i1").unwrap().start_failures, 0);
    }

    #[test]
    fn breaker_opens_after_three_start_failures() {
        let mut t = setup("breaker");
        let o = &mut t.o;
        o.create_run("x", None, None, 1).unwrap();
        o.add_item("r1", None, "spec", &[], None, 1).unwrap();
        for n in 1..=3 {
            let a = begin(o, "r1", "i1", "k", None).unwrap();
            let tripped = o.start_failed(&a.id, "no such program", 2).unwrap();
            assert_eq!(tripped, n == 3);
        }
        let it = o.item("i1").unwrap();
        assert_eq!(
            (it.state, it.start_failures, it.circuit_open),
            (ItemState::Failed, 3, true)
        );
        assert_eq!(
            begin(o, "r1", "i1", "k", None).unwrap_err().kind,
            ErrorKind::CircuitOpen
        );
        assert_eq!(
            begin(o, "r1", "i1", "k", Some("a3")).unwrap_err().kind,
            ErrorKind::CircuitOpen
        );
        let it = o.reopen_item("i1", 3).unwrap();
        assert_eq!(
            (it.state, it.start_failures, it.circuit_open),
            (ItemState::Ready, 0, false)
        );
        let a = begin(o, "r1", "i1", "k", None).unwrap();
        o.start_failed(&a.id, "again", 4).unwrap();
        let a = begin(o, "r1", "i1", "k", None).unwrap();
        assert!(o.mark_ready(&a.id, 5).unwrap());
        assert_eq!(
            o.item("i1").unwrap().start_failures,
            0,
            "a good start resets the count"
        );
    }

    #[test]
    fn depth_and_worker_caps() {
        let mut t = setup("caps");
        let o = &mut t.o;
        o.limits = Limits {
            max_depth: 2,
            run_worker_cap: 2,
            start_strikes: 3,
        };
        o.create_run("top", Some("t1"), None, 1).unwrap();
        for _ in 0..3 {
            o.add_item("r1", None, "s", &[], None, 1).unwrap();
        }
        begin(o, "r1", "i1", "k", None).unwrap();
        begin(o, "r1", "i2", "k", None).unwrap();
        // Over the run's worker limit: queued, not refused (docs/19 §5.1).
        let a3 = begin(o, "r1", "i3", "k", None).unwrap();
        o.enqueue(&a3.id, serde_json::json!({"x": 1}), None, 1)
            .unwrap();
        let lim = ResourceLimits {
            run_workers: 2,
            cpu_max_pct: None,
            min_free_mem: None,
            min_free_disk: None,
            ..Default::default()
        };
        let ad = o
            .admit(&lim, &Sample::default(), |_| None, Some(hash_token("q")), 2)
            .unwrap();
        assert!(ad.start.is_none());
        assert_eq!(
            ad.changed[0].queue.as_ref().unwrap().kind,
            WaitKind::RunWorkers
        );
        // A retry replaces a slot instead of taking one.
        begin(o, "r1", "i2", "k2", Some("a2")).unwrap();
        // Worker a1 (depth 1) coordinates r2 (depth 1): its workers are depth 2.
        let r2 = o.create_run("sub", Some("t9"), Some("a1"), 2).unwrap();
        assert_eq!(r2.depth, 1);
        o.add_item("r2", None, "s", &[], None, 2).unwrap();
        let deep = begin(o, "r2", "i4", "k", None).unwrap();
        assert_eq!(deep.depth, 2);
        let r3 = o
            .create_run("subsub", Some("t10"), Some(&deep.id), 3)
            .unwrap();
        o.add_item(&r3.id, None, "s", &[], None, 3).unwrap();
        let e = begin(o, &r3.id, "i5", "k", None).unwrap_err();
        assert_eq!(e.kind, ErrorKind::DepthExceeded);
        assert!(e.message.contains("yourself"), "{}", e.message);
    }

    #[test]
    fn mail_of_lists_a_run_without_acking() {
        let mut t = setup("mail-of");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.create_run("y", Some("t2"), None, 1).unwrap();
        o.add_item("r1", None, "s", &[], None, 1).unwrap();
        begin(o, "r1", "i1", "k1", None).unwrap();
        o.attach_worker("a1", Some("demo/w1"), "t5", None, 0)
            .unwrap();
        let q = o
            .send(
                None,
                NewMail::new("attempt:a1", "run:r1", MailType::Question, "which db?"),
                |_| false,
                2,
            )
            .unwrap();
        o.send(
            None,
            NewMail::new("run:r2", "run:r2", MailType::Status, "other run"),
            |_| false,
            2,
        )
        .unwrap();
        let all = o.mail_of("r1").unwrap();
        assert_eq!(
            all.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            [q.id.as_str()]
        );
        assert_eq!(all[0].read_ms, None);
        assert_eq!(o.unread("run:r1").len(), 1, "listing marks nothing read");
        assert!(o.wants_wake("run:r1"), "and leaves the wake-up alone");
        o.check(&["run:r1".into()], true, 9).unwrap();
        assert_eq!(o.mail_of("r1").unwrap()[0].read_ms, Some(9));
        assert_eq!(o.mail_of("r9").unwrap_err().kind, ErrorKind::RunNotFound);
    }

    #[test]
    fn queue_admits_in_fifo_order_and_survives_reload() {
        let mut t = setup("queue");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        for _ in 0..3 {
            o.add_item("r1", None, "s", &[], None, 1).unwrap();
        }
        let lim = ResourceLimits {
            max_workers: 1,
            cpu_max_pct: None,
            min_free_mem: None,
            min_free_disk: None,
            ..Default::default()
        };
        // a1 runs; a2 and a3 queue behind it.
        begin(o, "r1", "i1", "k", None).unwrap();
        for (n, item) in [(2, "i2"), (3, "i3")] {
            let a = begin(o, "r1", item, "unused", None).unwrap();
            o.enqueue(
                &a.id,
                serde_json::json!({ "n": n }),
                Some(r"C:\wt".into()),
                10 + n,
            )
            .unwrap();
        }
        assert_eq!(o.active_workers(), 1);
        let ad = o
            .admit(
                &lim,
                &Sample::default(),
                |_| None,
                Some(hash_token("t2")),
                20,
            )
            .unwrap();
        assert!(ad.start.is_none());
        assert_eq!(ad.changed.len(), 2);
        let q = o.attempt("a2").unwrap().queue.clone().unwrap();
        assert_eq!((q.kind, q.position), (WaitKind::Workers, 1));
        // The same verdicts again: nothing changes, nothing is written.
        let ad = o
            .admit(
                &lim,
                &Sample::default(),
                |_| None,
                Some(hash_token("t2")),
                21,
            )
            .unwrap();
        assert!(ad.changed.is_empty() && ad.start.is_none());
        // Restart while queued: the order and reasons are kept.
        let mut o2 = Orch::load(&t.dir, Limits::default(), 30);
        let ids: Vec<&str> = o2.queue().iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["a2", "a3"]);
        assert_eq!(
            o2.attempt("a2").unwrap().launch,
            Some(serde_json::json!({"n": 2}))
        );
        assert!(
            o2.attempt("a2").unwrap().public().launch.is_none(),
            "launch data stays off the wire"
        );
        // a1 ends: a2 (the oldest) is taken out with its new token hash.
        o2.end_attempt("a1", AttemptState::Stopped, "stop", None, None, 31)
            .unwrap();
        let ad = o2
            .admit(
                &lim,
                &Sample::default(),
                |_| None,
                Some(hash_token("t2")),
                32,
            )
            .unwrap();
        let (a, launch) = ad.start.unwrap();
        assert_eq!((a.id.as_str(), launch), ("a2", serde_json::json!({"n": 2})));
        assert!(
            a.queue.is_none() && a.token_hash.is_empty(),
            "public record"
        );
        assert!(o2.verify_token("a2", Some("t2")).is_ok());
        assert_eq!(
            o2.attempt("a3").unwrap().queue.as_ref().unwrap().kind,
            WaitKind::Behind
        );
        assert_eq!(o2.active_workers(), 1);
    }

    #[test]
    fn gates_block_items_until_decided() {
        let mut t = setup("gates");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.add_item("r1", None, "s", &[], None, 1).unwrap();
        o.add_item("r1", None, "s", &[], None, 1).unwrap();
        // A ready item blocked, then free again.
        assert_eq!(
            o.block_item("i2", "g1", 2).unwrap().state,
            ItemState::Blocked
        );
        assert_eq!(
            begin(o, "r1", "i2", "k", None).unwrap_err().kind,
            ErrorKind::InvalidState
        );
        assert_eq!(
            o.reopen_item("i2", 3).unwrap().state,
            ItemState::Blocked,
            "reopen keeps the block"
        );
        assert_eq!(
            o.unblock_item("i2", "g1", 4).unwrap().unwrap().state,
            ItemState::Ready
        );
        assert_eq!(o.unblock_item("i2", "g1", 5).unwrap(), None, "idempotent");
        // An assigned item: two gates need both decided.
        begin(o, "r1", "i1", "tok", None).unwrap();
        o.block_item("i1", "g2", 6).unwrap();
        o.block_item("i1", "g3", 6).unwrap();
        assert_eq!(
            o.unblock_item("i1", "g2", 7).unwrap().unwrap().state,
            ItemState::Blocked
        );
        // Ending the attempt does not clear the block.
        o.end_attempt("a1", AttemptState::Stopped, "x", None, None, 8)
            .unwrap();
        assert_eq!(o.item("i1").unwrap().state, ItemState::Blocked);
        let it = o.unblock_item("i1", "g3", 9).unwrap().unwrap();
        assert_eq!(
            (it.state, it.gates.len()),
            (ItemState::Ready, 0),
            "no open attempt any more"
        );
        let a2 = begin(o, "r1", "i1", "t2", None).unwrap();
        o.block_item("i1", "g4", 10).unwrap();
        assert_eq!(
            o.unblock_item("i1", "g4", 11).unwrap().unwrap().state,
            ItemState::Assigned
        );
        o.done(&a2.id, Some("t2"), true, "ok", &[], None, None, 12)
            .unwrap();
        assert_eq!(
            o.block_item("i1", "g5", 13).unwrap_err().kind,
            ErrorKind::InvalidState
        );
        // Survives a reload.
        o.block_item("i2", "g6", 14).unwrap();
        let o2 = Orch::load(&t.dir, Limits::default(), 15);
        assert_eq!(o2.item("i2").unwrap().gates, ["g6"]);
    }

    #[test]
    fn mail_check_ack_wake_and_reopen() {
        let mut t = setup("mail");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.add_item("r1", None, "s", &[], None, 1).unwrap();
        o.add_item("r1", None, "s2", &[], None, 1).unwrap();
        begin(o, "r1", "i1", "k1", None).unwrap();
        o.attach_worker("a1", Some("demo/w1"), "t5", None, 0)
            .unwrap();
        let mut a2 = begin(o, "r1", "i2", "k2", None).unwrap();
        a2.agent = "claude".into();
        o.update_attempt("a2", |a| a.agent = "claude".into())
            .unwrap();
        o.attach_worker("a2", Some("demo/w2"), "t6", None, 0)
            .unwrap();
        assert_eq!(o.terminal_mailboxes("t1"), vec!["run:r1"]);
        assert_eq!(o.terminal_mailboxes("t5"), vec!["attempt:a1"]);
        assert_eq!(o.mailbox_terminal("run:r1").as_deref(), Some("t1"));

        let q = o
            .send(
                None,
                NewMail::new("attempt:a1", "run:r1", MailType::Question, "which db?"),
                |_| false,
                2,
            )
            .unwrap();
        assert!(o.has_open_question("a1"));
        assert!(o.wants_wake("run:r1"));
        o.mark_notified(&["run:r1".into()], 3).unwrap();
        assert!(!o.wants_wake("run:r1"), "one notification per mailbox");
        o.send(
            None,
            NewMail::new("attempt:a2", "run:r1", MailType::Status, "fyi"),
            |_| false,
            3,
        )
        .unwrap();
        assert!(!o.wants_wake("run:r1"), "still only one");
        let mut reply =
            NewMail::new("run:r1", "attempt:a1", MailType::Answer, "re: which db?").body("sqlite");
        reply.reply_to = Some(q.id.clone());
        let ans = o.send(None, reply, |_| false, 4).unwrap();
        assert_eq!(ans.thread.as_deref(), Some(q.id.as_str()));
        assert_eq!(o.answer_to(&q.id).unwrap().id, ans.id);
        assert!(!o.has_open_question("a1"));
        let got = o.check(&["run:r1".into()], true, 5).unwrap();
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|m| m.read_ms == Some(5)));
        assert!(o.unread("run:r1").is_empty());
        assert_eq!(o.notified("run:r1"), None, "check clears the notification");
        // Groups expand to live workers; done cannot go to a group.
        let g = o
            .send(
                Some("r1"),
                NewMail::new("run:r1", "@claude", MailType::Status, "hi"),
                |_| false,
                6,
            )
            .unwrap();
        assert_eq!(g.recipients, vec!["attempt:a2".to_string()]);
        let all = o
            .send(
                Some("r1"),
                NewMail::new("run:r1", "@idle", MailType::Status, "hi"),
                |a| a.id == "a1",
                6,
            )
            .unwrap();
        assert_eq!(all.recipients, vec!["attempt:a1".to_string()]);
        assert_eq!(
            o.send(
                Some("r1"),
                NewMail::new("attempt:a1", "@all", MailType::Done, "x"),
                |_| true,
                6
            )
            .unwrap_err()
            .kind,
            ErrorKind::InvalidParams
        );
        assert_eq!(
            o.send(
                None,
                NewMail::new("run:r1", "@all", MailType::Status, "x"),
                |_| true,
                6
            )
            .unwrap_err()
            .kind,
            ErrorKind::InvalidParams
        );
        // Heartbeats never wake.
        o.check(&["run:r1".into()], true, 7).unwrap();
        o.send(
            None,
            NewMail::new("attempt:a1", "run:r1", MailType::Heartbeat, "50%"),
            |_| false,
            7,
        )
        .unwrap();
        assert!(!o.wants_wake("run:r1"));
        // Mail to a finished worker re-opens it.
        o.done("a1", Some("k1"), true, "ok", &[], None, Some(1000), 8)
            .unwrap();
        o.send(
            None,
            NewMail::new("run:r1", "attempt:a1", MailType::Status, "one more thing"),
            |_| false,
            9,
        )
        .unwrap();
        let a1 = o.attempt("a1").unwrap();
        assert_eq!(
            (a1.state, a1.worker, a1.reopens, a1.release_at_ms),
            (AttemptState::Ready, WorkerState::Active, 1, None)
        );
        assert_eq!(o.item("i1").unwrap().state, ItemState::Assigned);
        // ...but not once released.
        o.done("a1", Some("k1"), true, "ok", &[], None, Some(0), 10)
            .unwrap();
        assert_eq!(o.due_releases(10), vec!["a1".to_string()]);
        o.set_worker("a1", WorkerState::Released, Some("a1.log"), 11)
            .unwrap();
        assert_eq!(
            o.send(
                None,
                NewMail::new("run:r1", "attempt:a1", MailType::Status, "?"),
                |_| false,
                12
            )
            .unwrap_err()
            .kind,
            ErrorKind::InvalidState
        );
        assert_eq!(o.mailbox_terminal("attempt:a1"), None);
        // Read receipts and wake marks survive a reload.
        o.mark_notified(&["attempt:a2".into()], 13).unwrap();
        let o2 = Orch::load(&t.dir, Limits::default(), 14);
        assert!(
            o2.unread("run:r1")
                .iter()
                .all(|m| m.kind == MailType::Heartbeat || m.kind == MailType::Done)
        );
        assert_eq!(o2.notified("attempt:a2"), Some(13));
    }

    #[test]
    fn end_attempt_requests_and_titles() {
        let mut t = setup("end");
        let o = &mut t.o;
        o.create_run("x", None, None, 1).unwrap();
        o.add_item("r1", None, "s", &[], None, 1).unwrap();
        begin(o, "r1", "i1", "k", None).unwrap();
        assert!(
            o.end_attempt(
                "a1",
                AttemptState::Stopped,
                "stopped by run:r1",
                None,
                Some(50),
                2
            )
            .unwrap()
        );
        let a = o.attempt("a1").unwrap();
        assert_eq!(
            (a.state, a.worker, a.release_at_ms),
            (AttemptState::Stopped, WorkerState::Finished, Some(52))
        );
        assert_eq!(o.item("i1").unwrap().state, ItemState::Ready);
        assert!(
            !o.end_attempt("a1", AttemptState::Lost, "x", None, None, 3)
                .unwrap()
        );
        // request ids
        assert_eq!(o.request("q1", "orch.run.create").unwrap(), None);
        o.remember("q1", "orch.run.create", serde_json::json!({"id": "r1"}), 4)
            .unwrap();
        assert_eq!(
            o.request("q1", "orch.run.create").unwrap().unwrap()["id"],
            "r1"
        );
        assert_eq!(
            o.request("q1", "orch.done").unwrap_err().kind,
            ErrorKind::InvalidParams
        );
        for n in 0..(MAX_REQUESTS + 5) {
            o.remember(&format!("x{n}"), "m", Value::Null, 5).unwrap();
        }
        assert_eq!(
            o.request("q1", "orch.run.create").unwrap(),
            None,
            "oldest dropped"
        );
        let o2 = Orch::load(&t.dir, Limits::default(), 6);
        assert!(
            o2.request(&format!("x{}", MAX_REQUESTS + 4), "m")
                .unwrap()
                .is_some()
        );
        assert_eq!(spec_title("\n\n  ## Fix the bug  \nbody"), "Fix the bug");
        assert_eq!(spec_title(&"y".repeat(100)).chars().count(), 81);
        assert_eq!(hash_token("abc").len(), 64);
    }

    #[test]
    fn torn_mail_line_recovered_on_load() {
        let mut t = setup("torn");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.send(
            None,
            NewMail::new("human", "run:r1", MailType::Status, "one"),
            |_| false,
            2,
        )
        .unwrap();
        o.send(
            None,
            NewMail::new("human", "run:r1", MailType::Status, "two"),
            |_| false,
            3,
        )
        .unwrap();
        let p = store::mail_jsonl(&t.dir, "r1");
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(br#"{"kind":"mail","mail":{"id":"m3","#)
            .unwrap();
        drop(f);
        let mut o2 = Orch::load(&t.dir, Limits::default(), 4);
        assert_eq!(o2.unread("run:r1").len(), 2);
        assert!(
            o2.notes.iter().any(|n| n.contains("torn")),
            "{:?}",
            o2.notes
        );
        let m = o2
            .send(
                None,
                NewMail::new("human", "run:r1", MailType::Status, "three"),
                |_| false,
                5,
            )
            .unwrap();
        assert_eq!(m.id, "m3");
        let o3 = Orch::load(&t.dir, Limits::default(), 6);
        assert!(o3.notes.is_empty(), "{:?}", o3.notes);
        assert_eq!(o3.unread("run:r1").len(), 3);
    }

    /// Opens `path` with no sharing: other writes and renames onto it fail
    /// until the handle drops (an injected disk failure).
    #[cfg(windows)]
    fn lock(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(path)
            .unwrap()
    }

    fn done_mails(o: &Orch, run: &str) -> Vec<Mail> {
        o.mail_of(run)
            .unwrap()
            .into_iter()
            .filter(|m| m.kind == MailType::Done)
            .collect()
    }

    /// orch-01: the done mail cannot be written. Nothing changes (the
    /// item stays assigned, no dependent item starts), on disk or in
    /// memory, and the same report succeeds once the disk is back.
    #[cfg(windows)]
    #[test]
    fn done_with_a_failing_mail_write_changes_nothing() {
        let mut t = setup("done-mailfail");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.add_item("r1", None, "a", &[], None, 1).unwrap();
        o.add_item("r1", None, "b", &["i1".into()], None, 1)
            .unwrap();
        begin(o, "r1", "i1", "k1", None).unwrap();
        let held = lock(&store::mail_jsonl(&t.dir, "r1"));
        o.done("a1", Some("k1"), true, "ok", &[], None, None, 5)
            .unwrap_err();
        assert!(o.attempt("a1").unwrap().state.is_open());
        assert_eq!(
            (o.item("i1").unwrap().state, o.item("i2").unwrap().state),
            (ItemState::Assigned, ItemState::Pending)
        );
        drop(held);
        let disk = Orch::load(&t.dir, Limits::default(), 6);
        assert!(
            disk.attempt("a1").unwrap().state.is_open(),
            "nothing reached the disk"
        );
        let d = o
            .done("a1", Some("k1"), true, "ok", &[], None, None, 7)
            .unwrap();
        assert!(d.accepted && !d.repeated);
        assert_eq!(o.item("i2").unwrap().state, ItemState::Ready);
        assert_eq!(done_mails(o, "r1").len(), 1);
    }

    /// orch-01: the mail is written but `run.json` is not (write failure or
    /// a crash in between). A reload applies the recorded done; the same
    /// report again returns the recorded result without a second mail; a
    /// different report is still refused.
    #[cfg(windows)]
    #[test]
    fn done_recorded_in_mail_survives_a_failed_run_write() {
        let mut t = setup("done-runfail");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.add_item("r1", None, "a", &[], None, 1).unwrap();
        o.add_item("r1", None, "b", &["i1".into()], None, 1)
            .unwrap();
        begin(o, "r1", "i1", "k1", None).unwrap();
        o.attach_worker("a1", Some("demo/w1"), "t5", None, 0)
            .unwrap();
        let held = lock(&store::run_json(&t.dir, "r1"));
        let files = vec!["src/a.rs".to_string()];
        o.done(
            "a1",
            Some("k1"),
            true,
            "did it",
            &files,
            None,
            Some(1000),
            5,
        )
        .unwrap_err();
        drop(held);
        assert_eq!(done_mails(o, "r1").len(), 1, "the journal line is there");

        // A restart at this point: the record still says assigned.
        let mut o2 = Orch::load(&t.dir, Limits::default(), 6);
        assert!(
            o2.notes
                .iter()
                .any(|n| n.contains("applied the recorded done of a1")),
            "{:?}",
            o2.notes
        );
        let a1 = o2.attempt("a1").unwrap().clone();
        assert_eq!(
            (a1.state, a1.worker, a1.release_at_ms, a1.files.clone()),
            (
                AttemptState::Succeeded,
                WorkerState::Finished,
                Some(1005),
                files.clone()
            )
        );
        assert_eq!(
            (o2.item("i1").unwrap().state, o2.item("i2").unwrap().state),
            (ItemState::Completed, ItemState::Ready)
        );
        // ...and it was saved: the next load has nothing to replay.
        assert!(Orch::load(&t.dir, Limits::default(), 7).notes.is_empty());

        // The worker retries the same report: same mail, nothing new.
        let again = o2
            .done(
                "a1",
                Some("k1"),
                true,
                "did it",
                &files,
                None,
                Some(1000),
                8,
            )
            .unwrap();
        assert!(again.accepted && again.repeated);
        assert_eq!(again.mail.id, done_mails(&o2, "r1")[0].id);
        assert_eq!(done_mails(&o2, "r1").len(), 1);
        let other = o2
            .done(
                "a1",
                Some("k1"),
                false,
                "changed my mind",
                &[],
                None,
                None,
                9,
            )
            .unwrap_err();
        assert_eq!(other.kind, ErrorKind::InvalidState);
        // The original process (memory already moved on) also answers the retry.
        let again = o
            .done(
                "a1",
                Some("k1"),
                true,
                "did it",
                &files,
                None,
                Some(1000),
                10,
            )
            .unwrap();
        assert!(again.repeated);
    }

    /// orch-02: a wake is scheduled, then delivered or failed. Only a
    /// delivered one is persisted; a scheduled one that never settled (a
    /// restart in between) wants a wake again after the reload; an
    /// unconfirmed one is not typed twice until the mailbox is checked.
    #[test]
    fn wake_scheduled_delivered_failed_and_restart() {
        let mut t = setup("wake");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.add_item("r1", None, "a", &[], None, 1).unwrap();
        begin(o, "r1", "i1", "k1", None).unwrap();
        let q = || NewMail::new("attempt:a1", "run:r1", MailType::Question, "which db?");
        o.send(None, q(), |_| false, 2).unwrap();
        let rb = vec!["run:r1".to_string()];
        assert!(o.wants_wake("run:r1"));

        // Scheduled: no second wake meanwhile, nothing on disk.
        o.wake_scheduled(&rb, 3).unwrap();
        assert!(!o.wants_wake("run:r1") && o.wake_pending("run:r1"));
        assert_eq!(o.notified("run:r1"), None);
        o.update_attempt("a1", |a| a.seen_turn = 1).unwrap(); // any other save
        let mut o2 = Orch::load(&t.dir, Limits::default(), 4);
        assert!(
            o2.wants_wake("run:r1"),
            "a restart forgets the unsettled wake and tries again"
        );

        // Not typed (holder gone): wants a wake again right away.
        o2.wake_scheduled(&rb, 5).unwrap();
        o2.wake_failed(&rb, false).unwrap();
        assert!(o2.wants_wake("run:r1"));
        // Typed but not taken: not typed a second time...
        o2.wake_scheduled(&rb, 6).unwrap();
        o2.wake_failed(&rb, true).unwrap();
        assert!(!o2.wants_wake("run:r1"));
        assert_eq!(
            (o2.wake_unconfirmed("run:r1"), o2.notified("run:r1")),
            (1, None)
        );
        // ...until a check, or a restart.
        assert!(Orch::load(&t.dir, Limits::default(), 7).wants_wake("run:r1"));
        o2.check(&rb, true, 8).unwrap();
        o2.send(None, q(), |_| false, 9).unwrap();
        assert!(o2.wants_wake("run:r1"));

        // Delivered: persisted, no further wake even after a restart.
        o2.wake_scheduled(&rb, 10).unwrap();
        o2.wake_delivered(&rb, 11).unwrap();
        assert_eq!(o2.notified("run:r1"), Some(11));
        assert!(!o2.wants_wake("run:r1"));
        let o3 = Orch::load(&t.dir, Limits::default(), 12);
        assert_eq!(o3.notified("run:r1"), Some(11));
        assert!(!o3.wants_wake("run:r1"));
    }

    /// A done that was followed by a reopen (mail to the finished worker)
    /// is not replayed over the newer opening.
    #[test]
    fn replay_skips_a_reopened_attempt() {
        let mut t = setup("done-reopen");
        let o = &mut t.o;
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.add_item("r1", None, "a", &[], None, 1).unwrap();
        begin(o, "r1", "i1", "k1", None).unwrap();
        o.attach_worker("a1", Some("demo/w1"), "t5", None, 0)
            .unwrap();
        o.done("a1", Some("k1"), true, "ok", &[], None, None, 5)
            .unwrap();
        o.send(
            None,
            NewMail::new("run:r1", "attempt:a1", MailType::Status, "one more"),
            |_| false,
            6,
        )
        .unwrap();
        assert!(o.attempt("a1").unwrap().state.is_open());
        let o2 = Orch::load(&t.dir, Limits::default(), 7);
        assert!(o2.notes.is_empty(), "{:?}", o2.notes);
        assert!(o2.attempt("a1").unwrap().state.is_open());
        // The repeat check also only looks at the current opening.
        let mut o2 = o2;
        o2.done("a1", Some("k1"), true, "ok", &[], None, None, 8)
            .unwrap();
        let again = o2
            .done("a1", Some("k1"), true, "ok", &[], None, None, 9)
            .unwrap();
        assert!(again.repeated);
        assert_eq!(done_mails(&o2, "r1").len(), 2);
    }
}
