//! Records of the orchestration model (docs/19 §2, §4.2). They are both the
//! stored form (`run.json`, `mail.jsonl`) and the wire form of the `orch.*`
//! RPCs, so every later field must be `#[serde(default)]`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Unix epoch milliseconds.
pub type Ms = u64;

/// A run: one objective and its coordinator. Also the coordinator's mailbox
/// (`run:<id>`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Run {
    pub id: String,
    pub objective: String,
    /// The coordinator's terminal (woken for mail to `run:<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_terminal: Option<String>,
    /// Set when the coordinator is itself a worker (nesting).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator_attempt: Option<String>,
    /// Nesting depth of the coordinator: 0 for a person's coordinator, the
    /// attempt's depth for a worker that coordinates. Its workers get depth + 1.
    #[serde(default)]
    pub depth: u32,
    pub created_ms: Ms,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    /// Some dependency is not completed yet.
    Pending,
    Ready,
    /// An attempt is open.
    Assigned,
    Completed,
    Failed,
    /// Waiting on a gate (docs/19 §4.5).
    Blocked,
}

/// A work item of a run.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Item {
    pub id: String,
    pub run: String,
    pub title: String,
    pub spec: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub state: ItemState,
    /// Bumped by every new attempt; only the attempt of the current
    /// generation may complete the item.
    #[serde(default)]
    pub generation: u32,
    /// Consecutive start failures (the circuit breaker opens at the limit).
    #[serde(default)]
    pub start_failures: u32,
    /// The breaker is open: no more starts until `reopen`.
    #[serde(default)]
    pub circuit_open: bool,
    /// The attempt of the current generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// Pending gates on this item (docs/19 §4.5): it is `blocked` while any
    /// is pending.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<String>,
    pub created_ms: Ms,
    pub updated_ms: Ms,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Starting,
    /// The agent was seen starting its first turn.
    Ready,
    Succeeded,
    Failed,
    StartFailed,
    Stopped,
    Abandoned,
    /// The worker vanished (host restart, the agent exited without `done`).
    Lost,
    /// A stop could not be proven.
    Unknown,
}

impl Attempt {
    /// The record without its token hash (for RPC results and events).
    pub fn public(&self) -> Attempt {
        Attempt {
            token_hash: String::new(),
            launch: None,
            ..self.clone()
        }
    }
}

impl AttemptState {
    /// Still running as far as the record goes.
    pub fn is_open(self) -> bool {
        matches!(self, AttemptState::Starting | AttemptState::Ready)
    }
}

impl Attempt {
    /// Waiting in the resource queue (not started yet).
    pub fn is_queued(&self) -> bool {
        self.state == AttemptState::Starting && self.queue.is_some()
    }

    /// Holds a running worker: open and out of the queue.
    pub fn is_active(&self) -> bool {
        self.state.is_open() && self.queue.is_none()
    }
}

/// The worker's resources (task terminal), separate from the attempt record.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WorkerState {
    Active,
    /// The attempt ended; the terminal is still there until released.
    Finished,
    /// The terminal was closed (output archived; worktree kept).
    Released,
    /// A person owns it now: never released automatically.
    Kept,
}

/// One try of an item by one worker.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Attempt {
    pub id: String,
    pub run: String,
    pub item: String,
    pub generation: u32,
    pub state: AttemptState,
    pub worker: WorkerState,
    /// `claude` `codex` `shell` `argv`: for `@claude` / `@codex` addresses.
    pub agent: String,
    /// `new`, `current` or an existing task id.
    pub place: String,
    pub depth: u32,
    /// SHA-256 (hex) of the attempt token. Stored in `run.json`; cleared by
    /// [`Attempt::public`] before it goes over the wire.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    pub created_ms: Ms,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready_ms: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_ms: Option<Ms>,
    /// Finished workers are released at this time unless kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_at_ms: Option<Ms>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub released_ms: Option<Ms>,
    /// Last output archived at release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    /// The `done` body (what was done / found / left).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    /// Why the start failed / how the attempt ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Agent turn counter last looked at by the supervisor.
    #[serde(default)]
    pub seen_turn: u64,
    /// The turn that already produced a `turn_ended_without_done` escalation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalated_turn: Option<u64>,
    /// Output time that already produced a `stalled` escalation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stalled_at_output_ms: Option<Ms>,
    /// Times mail to the finished worker re-opened it (follow-up work).
    #[serde(default)]
    pub reopens: u32,
    /// Waiting for resources (docs/19 §5.1): the attempt is `starting` with
    /// stage `queued_for_resources` and has no task or terminal yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queue: Option<QueueState>,
    /// What the daemon needs to start a queued worker later (its start
    /// parameters without the caller's token). Not sent over the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<Value>,
    /// Items of its run this worker waits for (`pod orch wait --item`,
    /// docs/19 §18.2): ending its turn is not a missed `done` while any is
    /// unfinished, and Pod mails it when they all finished.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub waiting_on: Vec<String>,
}

impl ItemState {
    /// Completed or failed: nothing more will happen without a person.
    pub fn is_finished(self) -> bool {
        matches!(self, ItemState::Completed | ItemState::Failed)
    }
}

/// Why a queued worker waits (docs/19 §5.1).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum WaitKind {
    /// Not measured yet (just queued).
    Queued,
    /// The global limit of concurrent workers.
    Workers,
    /// The run's limit of open workers.
    RunWorkers,
    Cpu,
    Memory,
    Disk,
    /// Earlier queued workers go first (FIFO).
    Behind,
}

/// Queue state of a waiting attempt.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct QueueState {
    /// When it was queued: the FIFO order (ties by attempt number).
    pub since_ms: Ms,
    pub kind: WaitKind,
    /// Human-readable reason (Korean, for the board and `pod orch queue`).
    pub reason: String,
    /// 1-based place in the global queue.
    pub position: usize,
    /// The drive whose free space is checked (the worktrees root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_path: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MailType {
    Question,
    Answer,
    Done,
    Escalation,
    Status,
    Handoff,
    Heartbeat,
    /// A decision gate was requested or decided (payload `{gate, ...}`).
    Gate,
}

impl MailType {
    pub fn as_str(self) -> &'static str {
        match self {
            MailType::Question => "question",
            MailType::Answer => "answer",
            MailType::Done => "done",
            MailType::Escalation => "escalation",
            MailType::Status => "status",
            MailType::Handoff => "handoff",
            MailType::Heartbeat => "heartbeat",
            MailType::Gate => "gate",
        }
    }

    /// Heartbeats show on the board only; they never wake anyone.
    pub fn wakes(self) -> bool {
        self != MailType::Heartbeat
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    #[default]
    Normal,
    High,
}

/// A message of a run.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Mail {
    pub id: String,
    pub run: String,
    /// `run:<id>`, `attempt:<id>` or `human`.
    pub from: String,
    /// As addressed: `run:<id>`, `attempt:<id>`, `@all` `@idle` `@claude` `@codex`.
    pub to: String,
    /// The mailboxes it was delivered to (a group expands to attempts).
    pub recipients: Vec<String>,
    #[serde(rename = "type")]
    pub kind: MailType,
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default)]
    pub priority: Priority,
    pub created_ms: Ms,
    /// A late report from a superseded attempt: recorded, changed nothing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fenced: bool,
    /// The subject as data when Pod wrote it (`subject` holds its English
    /// rendering; the app renders Korean). docs/19 §16.3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<crate::text::MsgText>,
    /// The body as data when Pod wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_text: Option<crate::text::MsgText>,
    /// When the reading mailbox acknowledged it (filled per mailbox in
    /// `check` results; not stored on the record).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_ms: Option<Ms>,
}

/// One line of `mail.jsonl`: a message, or read receipts for a mailbox.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MailLine {
    Mail {
        mail: Box<Mail>,
    },
    Read {
        mailbox: String,
        ids: Vec<String>,
        ms: Ms,
    },
}

/// Wake-up bookkeeping of one mailbox (orch-02). Only a delivered
/// notification is persisted, so a restart neither repeats it nor forgets
/// one that was scheduled but never confirmed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WakeState {
    /// A notification line was typed and taken by the agent (or could not
    /// be checked), and the mailbox was not checked since.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_ms: Option<Ms>,
    /// A notification is being typed and not confirmed yet. Memory only: a
    /// restart forgets it and the wake is tried again.
    #[serde(skip)]
    pub pending_ms: Option<Ms>,
    /// Notifications typed but not taken since the last check. Memory only.
    #[serde(skip)]
    pub unconfirmed: u32,
}

/// `run.json`: a run with its items and attempts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunRecord {
    pub version: u32,
    pub run: Run,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
    #[serde(default)]
    pub wakes: BTreeMap<String, WakeState>,
}

/// Mailbox address of a run's coordinator.
pub fn run_mailbox(run: &str) -> String {
    format!("run:{run}")
}

/// Mailbox address of a worker.
pub fn attempt_mailbox(attempt: &str) -> String {
    format!("attempt:{attempt}")
}

/// Group addresses (live workers of the run).
pub const GROUPS: &[&str] = &["@all", "@idle", "@claude", "@codex"];
