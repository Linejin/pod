//! Wire types of the orchestration RPCs `orch.*` (protocol v16, docs/19 §7):
//! runs, work items, workers (attempts), mail, `done`; v17 adds the resource
//! queue (`orch.queue`, docs/19 §5.1) and decision gates with options
//! (`orch.gate.*`, §4.5).
//!
//! The records themselves (`Run`, `Item`, `Attempt`, `Mail`) come from
//! `pod-orch`. Every mutating call takes an optional `request_id`: a retry
//! with the same id returns the first result. The caller's identity comes
//! from [`OrchCaller`], which the `pod` CLI fills from `POD_TERMINAL_ID`,
//! `POD_ATTEMPT_ID` and `POD_ATTEMPT_TOKEN`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use pod_orch::model::{
    Attempt, AttemptState, Item, ItemState, Mail, MailType, Priority, Run, WorkerState,
};
/// The kinds of [`MsgText`] (docs/19 §16.3).
pub use pod_orch::text as msg_text;
pub use pod_orch::text::MsgText;

use crate::protocol::{AgentSpec, Empty, GateInfo, StatusInfo};

method!(
    OrchRunCreate,
    "orch.run.create",
    OrchRunCreateParams,
    RunInfo
);
method!(
    OrchRunList,
    "orch.run.list",
    OrchRunListParams,
    OrchRunListResult
);
method!(OrchRunShow, "orch.run.show", OrchRunRef, RunInfo);
method!(
    OrchItemCreate,
    "orch.item.create",
    OrchItemCreateParams,
    Item
);
method!(
    OrchItemList,
    "orch.item.list",
    OrchItemListParams,
    OrchItemListResult
);
method!(
    OrchItemUpdate,
    "orch.item.update",
    OrchItemUpdateParams,
    Item
);
method!(
    OrchWorkerStart,
    "orch.worker.start",
    OrchWorkerStartParams,
    OrchWorkerStartResult
);
method!(
    OrchWorkerList,
    "orch.worker.list",
    OrchRunRef,
    OrchWorkerListResult
);
method!(
    OrchWorkerShow,
    "orch.worker.show",
    OrchAttemptRef,
    WorkerInfo
);
method!(
    OrchWorkerRead,
    "orch.worker.read",
    OrchWorkerReadParams,
    OrchWorkerReadResult
);
method!(
    OrchWorkerStop,
    "orch.worker.stop",
    OrchAttemptRef,
    WorkerInfo
);
method!(
    OrchWorkerAbandon,
    "orch.worker.abandon",
    OrchAttemptRef,
    WorkerInfo
);
method!(
    OrchWorkerKeep,
    "orch.worker.keep",
    OrchAttemptRef,
    WorkerInfo
);
method!(
    OrchWorkerRelease,
    "orch.worker.release",
    OrchAttemptRef,
    WorkerInfo
);
method!(OrchMailSend, "orch.mail.send", OrchMailSendParams, Mail);
method!(
    OrchMailCheck,
    "orch.mail.check",
    OrchMailCheckParams,
    OrchMailCheckResult
);
method!(OrchMailReply, "orch.mail.reply", OrchMailReplyParams, Mail);
method!(
    OrchMailAsk,
    "orch.mail.ask",
    OrchMailAskParams,
    OrchMailAskResult
);
method!(OrchDone, "orch.done", OrchDoneParams, OrchDoneResult);
method!(
    OrchMailList,
    "orch.mail.list",
    OrchMailListParams,
    OrchMailListResult
);
method!(OrchConfigGet, "orch.config.get", Empty, OrchSettings);
method!(OrchConfigSet, "orch.config.set", OrchSettings, OrchSettings);
// v17 (docs/19 S6, §4.5)
method!(OrchQueue, "orch.queue", OrchRunRef, OrchQueueResult);
method!(
    OrchGateRequest,
    "orch.gate.request",
    OrchGateRequestParams,
    GateInfo
);
method!(
    OrchGateDecide,
    "orch.gate.decide",
    OrchGateDecideParams,
    GateInfo
);
// v18 (docs/19 §14.4)
method!(
    OrchLimitsGet,
    "orch.limits.get",
    OrchLimitsGetParams,
    OrchLimitsResult
);
method!(
    OrchLimitsSet,
    "orch.limits.set",
    OrchLimitsSetParams,
    OrchLimitsResult
);
// docs/19 §18.2 (additive, protocol number unchanged)
method!(OrchWait, "orch.wait", OrchWaitParams, OrchWaitResult);
method!(
    OrchCommit,
    "orch.commit",
    OrchCommitParams,
    OrchCommitResult
);

/// Every `orch.*` method (schema test, docs).
pub const ORCH_RUN_METHODS: &[&str] = &[
    "orch.run.create",
    "orch.run.list",
    "orch.run.show",
    "orch.item.create",
    "orch.item.list",
    "orch.item.update",
    "orch.worker.start",
    "orch.worker.list",
    "orch.worker.show",
    "orch.worker.read",
    "orch.worker.stop",
    "orch.worker.abandon",
    "orch.worker.keep",
    "orch.worker.release",
    "orch.mail.send",
    "orch.mail.check",
    "orch.mail.reply",
    "orch.mail.ask",
    "orch.done",
    "orch.mail.list",
    "orch.config.get",
    "orch.config.set",
    "orch.queue",
    "orch.gate.request",
    "orch.gate.decide",
    "orch.limits.get",
    "orch.limits.set",
    "orch.wait",
    "orch.commit",
];

/// `status.subscribe` notification: a run, item or attempt changed.
pub const NOTIFY_ORCH_CHANGED: &str = "orch.changed";
/// `status.subscribe` notification: a message was recorded (no body).
pub const NOTIFY_ORCH_MAIL: &str = "orch.mail";

/// Error codes of `orch.*` (docs/19 §7). -32050/-32051, -32060..-32064 and
/// -32070..-32072, -32040..-32043 are taken (stopping, protocol mismatch,
/// mobile, sessions, session discovery).
pub mod run_codes {
    pub const RUN_NOT_FOUND: i64 = -32080;
    pub const ITEM_NOT_FOUND: i64 = -32081;
    pub const ATTEMPT_NOT_FOUND: i64 = -32082;
    /// Token or generation mismatch.
    pub const ATTEMPT_FENCED: i64 = -32083;
    /// Starting this worker would nest deeper than allowed.
    pub const DEPTH_EXCEEDED: i64 = -32084;
    /// The item's start circuit breaker is open (3 failed starts).
    pub const CIRCUIT_OPEN: i64 = -32085;
    /// The item's dependencies are not completed.
    pub const DEPS_NOT_DONE: i64 = -32086;
    /// The run already has its maximum of open workers (v16; since v17 such
    /// workers are queued instead).
    pub const WORKER_LIMIT: i64 = -32087;
    pub const MAIL_NOT_FOUND: i64 = -32088;
}

/// Who is calling. Inside a Pod terminal the CLI fills it from the
/// environment; a person's client (the app) leaves it empty (`human`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchCaller {
    /// `POD_TERMINAL_ID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// `POD_ATTEMPT_ID` (workers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
    /// `POD_ATTEMPT_TOKEN` (workers). Checked against the attempt's hash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl OrchCaller {
    pub fn is_empty(&self) -> bool {
        self.terminal.is_none() && self.attempt.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchRunCreateParams {
    pub objective: String,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchRunListParams {
    /// Only runs this caller coordinates or works in.
    #[serde(default)]
    pub mine: bool,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
}

/// A run by id, or the caller's run when omitted.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchRunRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RunInfo {
    pub run: Run,
    pub items: Vec<Item>,
    pub attempts: Vec<Attempt>,
    /// Unread mail of the coordinator (`run:<id>`).
    pub unread: usize,
    /// Messages recorded so far.
    pub mail: usize,
    /// orch-02: Unix ms since a wake-up line typed into the coordinator was
    /// not confirmed as taken (it may sit in the input box). No further
    /// line is typed until the coordinator checks its mail or podd
    /// restarts; pressing Enter there (`terminal.send {enter}`) retries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_unconfirmed_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchRunListResult {
    pub runs: Vec<RunInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchItemCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub spec: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchItemListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Only items that can start now.
    #[serde(default)]
    pub ready: bool,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchItemListResult {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchItemUpdateParams {
    pub item: String,
    /// Clear the start breaker and make a finished item ready again.
    #[serde(default)]
    pub reopen: bool,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchWorkerStartParams {
    /// Defaults to the run the caller coordinates (a new one when it has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// An existing item, or give `spec` to create one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    /// Objective of a run created on the way (default: the item title).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub objective: Option<String>,
    /// Defaults to `shell`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSpec>,
    /// `new` (default): a new task; `current`: the caller's worktree; or an
    /// existing task id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub place: Option<String>,
    /// Repository for a new task (default: the caller's task's repository).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Task name (default `<run>-<attempt>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Never release the worker automatically.
    #[serde(default)]
    pub keep: bool,
    /// Replace this attempt (it is abandoned; the item's generation goes up).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchWorkerStartResult {
    pub run: String,
    pub item: String,
    pub attempt: Attempt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// Where it got to: `started`, or the failed stage (`task`, `terminal`, `brief`).
    pub stage: String,
    pub state: AttemptState,
    /// What the caller should do next (one sentence).
    pub next: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchAttemptRef {
    pub attempt: String,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// A worker for supervision (`pod orch workers`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WorkerInfo {
    pub attempt: Attempt,
    pub item_title: String,
    /// The agent's status (docs/07) while its terminal exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusInfo>,
    /// Unix ms of the terminal's last output (0 = none seen).
    #[serde(default)]
    pub last_output_ms: u64,
    /// The worker waits for an answer to a question.
    #[serde(default)]
    pub waiting_answer: bool,
    /// Unread mail for the worker.
    #[serde(default)]
    pub unread: usize,
    /// What happens next, e.g. "waiting for an answer", "release at 09:41".
    pub next: String,
    /// `next` as data (docs/19 §16.3): the app renders it in Korean.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_text: Option<MsgText>,
    /// orch-02: Unix ms since a wake-up line typed into this worker was not
    /// confirmed as taken (see [`RunInfo::wake_unconfirmed_ms`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_unconfirmed_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchWorkerListResult {
    pub workers: Vec<WorkerInfo>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchWorkerReadParams {
    pub attempt: String,
    /// Last N lines (default 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchWorkerReadResult {
    pub attempt: String,
    /// `terminal` (live) or `archive` (the log kept at release).
    pub source: String,
    pub lines: Vec<String>,
    /// A grey prompt suggestion at the worker's input caret: shown by the
    /// agent, NOT typed by anyone; left out of `lines` (docs/19 §18.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailSendParams {
    /// Run for group addresses (default: the caller's run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// `run:<id>`, `attempt:<id>`, `@all` `@idle` `@claude` `@codex`.
    /// Default for workers: their coordinator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(rename = "type", default = "default_type")]
    pub kind: MailType,
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
    #[serde(default)]
    pub priority: Priority,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

fn default_type() -> MailType {
    MailType::Status
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailCheckParams {
    /// Only this run's mailbox (coordinators of several runs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// Block until mail arrives (or `timeout_ms`).
    #[serde(default)]
    pub wait: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Mark returned mail read (default true).
    #[serde(default = "yes")]
    pub ack: bool,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailCheckResult {
    /// The mailboxes read.
    pub mailboxes: Vec<String>,
    pub messages: Vec<Mail>,
    #[serde(default)]
    pub timed_out: bool,
    pub next: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailReplyParams {
    /// The message answered.
    pub id: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailAskParams {
    pub question: String,
    /// Default: the first line of the question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// How long to wait for the answer (default 10 min, at most 30 min).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    /// Reusing it after a lost connection waits for the same question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailAskResult {
    pub question: Mail,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<Mail>,
    #[serde(default)]
    pub timed_out: bool,
    pub next: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DoneOutcome {
    Succeeded,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchDoneParams {
    pub outcome: DoneOutcome,
    /// What you did / what you found / what is left.
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchDoneResult {
    pub attempt: Attempt,
    /// False: a late report from a superseded attempt (recorded only).
    pub accepted: bool,
    pub mail: Mail,
    pub next: String,
}

/// `orch.wait` (docs/19 §18.2): a worker waits for other items of its run.
/// It then ends its turn; that is not a missed `done`, and Pod mails it
/// (`status`, text `wait.finished`) once every item finished.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchWaitParams {
    pub items: Vec<String>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchWaitItem {
    pub item: String,
    pub state: ItemState,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchWaitResult {
    pub items: Vec<OrchWaitItem>,
    /// Still unfinished (empty: everything already finished, go on).
    pub waiting_on: Vec<String>,
    pub next: String,
}

/// `orch.commit` (docs/19 §18.1): a worker commits in its own task's
/// worktree through podd. podd runs git itself, in the worktree it
/// recorded, with hooks and fsmonitor off, so a sandboxed agent never gets
/// an unsandboxed git that reads a config it wrote.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchCommitParams {
    pub message: String,
    /// Stage every change first (`git add -A`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
    /// Stage these paths first (relative to the worktree).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchCommitResult {
    pub attempt: String,
    pub task: String,
    /// The new commit (full hash) and its first line.
    pub commit: String,
    pub summary: String,
    pub next: String,
}

/// `orch.mail.list`: every message of a run, marking nothing read (the
/// app's inbox; agents use `check`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailListParams {
    pub run: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailListResult {
    /// Oldest first; `read_ms` = the earliest read receipt of its recipients.
    pub messages: Vec<Mail>,
}

/// Orchestration defaults (`orch.config.get/set`, the app's settings page).
/// Worker and resource limits are `orch.limits.*`.
/// Stored in `<state>\orch\config.json`; an environment variable set for
/// the daemon wins and its field is listed in `locked`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchSettings {
    /// Minutes after a worker finished before its terminal is released
    /// (`None` = never). `POD_ORCH_RELEASE_AFTER`.
    #[serde(default)]
    pub release_after_min: Option<u64>,
    /// Minutes without output while working before a `stalled` escalation
    /// (`None` = never). `POD_ORCH_STALL_AFTER`.
    #[serde(default)]
    pub stall_after_min: Option<u64>,
    /// Deepest worker nesting. `POD_ORCH_MAX_DEPTH`.
    pub max_depth: u32,
    /// Where `orch.worker.start` puts a worker without `place`: `new` or
    /// `current`. `POD_ORCH_DEFAULT_PLACE`.
    pub default_place: String,
    /// Fields fixed by an environment variable (ignored by `set`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked: Vec<String>,
}

/// `orch.changed` (on `status.subscribe` streams without an `ids` filter).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchChangedEvent {
    pub seq: u64,
    pub run: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<String>,
}

/// `orch.mail`: a message was recorded (subject only, never the body).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchMailEvent {
    pub seq: u64,
    pub run: String,
    pub to: String,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: MailType,
    pub subject: String,
    /// The subject as data when Pod wrote it (docs/19 §16.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<MsgText>,
    /// The sender (`run:<id>`, `attempt:<id>`, `human`, `pod`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub from: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_codes_are_unique() {
        use crate::protocol::*;
        let ours = [
            run_codes::RUN_NOT_FOUND,
            run_codes::ITEM_NOT_FOUND,
            run_codes::ATTEMPT_NOT_FOUND,
            run_codes::ATTEMPT_FENCED,
            run_codes::DEPTH_EXCEEDED,
            run_codes::CIRCUIT_OPEN,
            run_codes::DEPS_NOT_DONE,
            run_codes::WORKER_LIMIT,
            run_codes::MAIL_NOT_FOUND,
        ];
        let taken = [
            codes::SERVER_STOPPING,
            remote_codes::PROTOCOL_MISMATCH,
            mobile_codes::MOBILE_DENIED,
            mobile_codes::MOBILE_AUTH,
            mobile_codes::RATE_LIMITED,
            mobile_codes::OPERATION_CONFLICT,
            mobile_codes::MESSAGE_INVALID,
            orch_codes::INVALID_STATE,
            pr_codes::FORGE_FAILED,
            session_codes::TASK_BUSY,
            session_codes::ARCHIVE_FAILED,
            session_codes::WORKTREE_LOCKED,
            sessions_codes::SESSION_NOT_FOUND,
            sessions_codes::SESSION_RUNNING,
            sessions_codes::DISCOVERY_OFF,
        ];
        for (i, c) in ours.iter().enumerate() {
            assert!(!taken.contains(c), "{c} is taken");
            assert!(!ours[i + 1..].contains(c), "{c} twice");
        }
    }

    #[test]
    fn wire_shapes() {
        let p: OrchMailSendParams = serde_json::from_value(json!({"subject": "hi"})).unwrap();
        assert_eq!(p.kind, MailType::Status);
        let p: OrchMailCheckParams = serde_json::from_value(json!({})).unwrap();
        assert!(p.ack && !p.wait);
        let p: OrchDoneParams =
            serde_json::from_value(json!({"outcome": "succeeded", "body": "x"})).unwrap();
        assert_eq!(p.outcome, DoneOutcome::Succeeded);
        let c = OrchCaller {
            terminal: Some("t1".into()),
            ..Default::default()
        };
        let v = serde_json::to_value(OrchRunRef {
            run: None,
            caller: c,
        })
        .unwrap();
        assert_eq!(v, json!({"caller": {"terminal": "t1"}}));
    }
}

// ───────────────────────────── v17 ─────────────────────────────

pub use pod_orch::model::{QueueState, WaitKind};

/// The resource gate's limits (docs/19 §5.1). `None` = check off.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ResourceLimitsInfo {
    pub max_workers: usize,
    pub run_workers: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_max_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free_mem: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_free_disk: Option<u64>,
    /// The CPU limit holds a worker at most this long.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_max_wait_ms: Option<u64>,
}

/// `orch.queue`: the measurements, limits and the workers waiting for
/// resources (FIFO). `queue[].attempt.queue` holds a fresh verdict
/// (`kind`, Korean `reason`, global `position`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchQueueResult {
    pub limits: ResourceLimitsInfo,
    /// System CPU percent over the meter's window (unknown: absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
    /// Available physical memory in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_mem: Option<u64>,
    /// Free bytes of the daemon's worktrees drive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_disk: Option<u64>,
    /// Active workers over all runs.
    pub active: usize,
    pub queue: Vec<WorkerInfo>,
    /// Supervisor passes finished since the daemon started (a barrier for
    /// "nothing happened" checks: wait for two more).
    #[serde(default)]
    pub supervisor_passes: u64,
    /// Typed wake-ups whose Enter check has not settled yet.
    #[serde(default)]
    pub wakes_in_flight: u64,
}

/// `orch.gate.request`: ask for a decision (docs/19 §4.5). A worker's gate
/// blocks its item and goes to its coordinator as mail; anyone may decide
/// it from the Gates panel or a phone. A coordinator's gate goes to a person.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchGateRequestParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub message: String,
    /// 2..=16 choices, or none for approve/reject.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// The item to block (workers: their own item by default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_s: Option<u64>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

/// `orch.gate.decide`: the coordinator of the gate's run (not for its own
/// gates) or a person. The same single decision path as `gate.resolve` and
/// the phone; `request_id` is the operation id (a retry replays).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchGateDecideParams {
    pub id: String,
    /// The chosen option (required to approve a gate with options).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    /// Reject instead of approving / choosing.
    #[serde(default)]
    pub reject: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

// ───────────────────────────── v18 ─────────────────────────────

/// `orch.limits.get`: the resource gate's limits as settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchLimitsGetParams {}

/// `orch.limits.set`: change the saved limits (`<state>\orch\limits.json`).
/// Values take the environment variables' text forms (`4`, `70%`, `4G`,
/// `30m`, `off`); `reset` returns keys to their default. All or nothing;
/// the queue is judged again at once. Workers (an `attempt` caller) may not
/// change limits. An environment variable still wins for its key.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchLimitsSetParams {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reset: Vec<String>,
    #[serde(default, skip_serializing_if = "OrchCaller::is_empty")]
    pub caller: OrchCaller,
}

/// One limit: effective value (text form), where it comes from
/// (`default` / `settings` / `env`), its variable and the saved value.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchLimitEntry {
    pub key: String,
    pub value: String,
    pub source: String,
    pub env: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saved: Option<String>,
    /// Korean description with the allowed values.
    pub help: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OrchLimitsResult {
    pub limits: ResourceLimitsInfo,
    pub entries: Vec<OrchLimitEntry>,
    /// The settings file.
    pub path: String,
}
