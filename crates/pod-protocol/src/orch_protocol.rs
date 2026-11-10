//! Wire types for review comments (P4), groups / fan-out and gates (P5):
//! `review.*`, `group.*`, `gate.*` (docs/11).
//!
//! Kept apart from `protocol.rs` so this work stays additive; everything here
//! is re-exported from [`crate::protocol`]. The daemon side lives in
//! `server/review.rs`, `server/groups.rs`, `server/gates.rs` and
//! `server/orch_store.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::{AgentSpec, DiffFile, Method, TaskInfo, TaskRemoveResult, WaitOutcome};

// ───────────────────────────── review ─────────────────────────────

method!(
    ReviewCommentsAdd,
    "review.comments.add",
    ReviewAddParams,
    ReviewCommentsResult
);
method!(
    ReviewCommentsList,
    "review.comments.list",
    ReviewListParams,
    ReviewCommentsResult
);
method!(
    ReviewCommentsResolve,
    "review.comments.resolve",
    ReviewResolveParams,
    ReviewCommentsResult
);
method!(
    ReviewCommentsRemove,
    "review.comments.remove",
    ReviewIdsParams,
    ReviewCommentsResult
);
method!(ReviewSend, "review.send", ReviewSendParams, DeliveryInfo);
method!(
    ReviewDeliveries,
    "review.deliveries",
    ReviewDeliveriesParams,
    ReviewDeliveriesResult
);
method!(ReviewCancel, "review.cancel", DeliveryRef, DeliveryInfo);

// ───────────────────────────── groups ─────────────────────────────

method!(GroupCreate, "group.create", GroupCreateParams, GroupInfo);
method!(GroupList, "group.list", GroupListParams, GroupListResult);
method!(GroupShow, "group.show", GroupRef, GroupInfo);
method!(GroupCompare, "group.compare", GroupRef, GroupCompareResult);
method!(
    GroupCheck,
    "group.check",
    GroupCheckParams,
    GroupCheckResult
);
method!(
    GroupCheckCancel,
    "group.check_cancel",
    GroupRef,
    GroupCheckResult
);
method!(GroupPick, "group.pick", GroupPickParams, GroupPickResult);
method!(GroupRemove, "group.remove", GroupRef, GroupRemoveResult);

// ───────────────────────────── gates ─────────────────────────────

method!(GateCreate, "gate.create", GateCreateParams, GateInfo);
method!(GateResolve, "gate.resolve", GateResolveParams, GateInfo);
method!(GateList, "gate.list", GateListParams, GateListResult);
method!(GateShow, "gate.show", GateRef, GateInfo);
method!(GateWait, "gate.wait", GateWaitParams, GateWaitResult);

/// `gate.changed` on a `status.subscribe` stream: a gate was created or resolved.
pub const NOTIFY_GATE_CHANGED: &str = "gate.changed";
/// `group.check` on a `status.subscribe` stream (v6): a background
/// `group.check` job started, a member finished, or the job ended.
pub const NOTIFY_GROUP_CHECK: &str = "group.check";
/// `review.delivery` on a `status.subscribe` stream (additive, still v6): a
/// review delivery was recorded or changed state (queued, delivered,
/// cancelled, failed). Lets UIs drop polling of `review.deliveries`.
pub const NOTIFY_REVIEW_DELIVERY: &str = "review.delivery";

pub const ORCH_METHODS: &[&str] = &[
    ReviewCommentsAdd::NAME,
    ReviewCommentsList::NAME,
    ReviewCommentsResolve::NAME,
    ReviewCommentsRemove::NAME,
    ReviewSend::NAME,
    ReviewDeliveries::NAME,
    ReviewCancel::NAME,
    GroupCreate::NAME,
    GroupList::NAME,
    GroupShow::NAME,
    GroupCompare::NAME,
    GroupCheck::NAME,
    GroupCheckCancel::NAME,
    GroupPick::NAME,
    GroupRemove::NAME,
    GateCreate::NAME,
    GateResolve::NAME,
    GateList::NAME,
    GateShow::NAME,
    GateWait::NAME,
];

/// Application error codes added with protocol v5.
pub mod orch_codes {
    /// `review.send`: the agent waits on the user (permission, question...).
    pub const AGENT_BLOCKED: i64 = -32014;
    /// No comment / delivery with that id (or it belongs to another task).
    pub const REVIEW_NOT_FOUND: i64 = -32015;
    /// No group matches the reference.
    pub const GROUP_NOT_FOUND: i64 = -32016;
    /// `group.create`: the group (or one of its member tasks) exists.
    pub const GROUP_EXISTS: i64 = -32017;
    /// No gate with that id.
    pub const GATE_NOT_FOUND: i64 = -32018;
    /// `gate.resolve` on a gate that is no longer pending.
    pub const GATE_RESOLVED: i64 = -32019;
    /// The operation does not apply in the current state (e.g. nothing to
    /// send, delivery no longer queued). `data` may explain.
    pub const INVALID_STATE: i64 = -32020;
}

// ───────────────────────────── review types ─────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The base version (left side of the diff; removed or changed lines).
    Old,
    /// The worktree version (right side).
    #[default]
    New,
}

/// One comment as the review UI exports it (docs/05 §2 shape: `path` is
/// accepted as an alias of `file`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NewComment {
    /// Path relative to the worktree root, `/` or `\` separated.
    #[serde(alias = "path")]
    pub file: String,
    #[serde(default)]
    pub side: Side,
    /// 1-based first line.
    pub line: u32,
    /// 1-based last line (inclusive) of a range comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    /// The reviewer's text.
    pub body: String,
    /// Code the comment refers to, as the reviewer saw it. When absent the
    /// daemon reads it from the worktree (`new`) or the merge base (`old`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewAddParams {
    /// Task id (`<repo>/<name>`) or unique name.
    pub task: String,
    pub comments: Vec<NewComment>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommentState {
    /// Not sent to the agent yet.
    Open,
    /// In a delivery waiting for the agent to become idle.
    Queued,
    /// Delivered to the agent (still unresolved).
    Sent,
    /// Closed by the reviewer.
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewComment {
    /// `c<N>`, unique in this state dir.
    pub id: String,
    /// Owning task id.
    pub task: String,
    /// `/`-separated path relative to the worktree root.
    pub file: String,
    pub side: Side,
    pub line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    pub body: String,
    /// Quoted code (captured when the comment was added; may be absent when
    /// the file or line did not exist).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub state: CommentState,
    pub created_ms: u64,
    /// Last delivery that carried this comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewListParams {
    /// Only this task's comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Only comments in this state (default: all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<CommentState>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewResolveParams {
    pub ids: Vec<String>,
    /// Stored on each comment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Re-open instead of resolve (state becomes `open`).
    #[serde(default)]
    pub reopen: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewIdsParams {
    pub ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewCommentsResult {
    pub comments: Vec<ReviewComment>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewSendParams {
    pub task: String,
    /// Comments to send (any state but `resolved`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comment_ids: Vec<String>,
    /// Send every `open` comment of the task (in addition to `comment_ids`).
    #[serde(default)]
    pub all_open: bool,
    /// Extra instructions placed before the comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    /// Terminal to type into (default: the task's agent terminal, its first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// Only build and return the prompt; deliver nothing.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// The agent was working; delivered on its next idle/done.
    Queued,
    /// Typed into the terminal (bracketed paste + Enter).
    Delivered,
    /// Cancelled with `review.cancel`.
    Cancelled,
    /// Could not be delivered (terminal exited / removed); see `error`.
    Failed,
    /// `dry_run`: prompt only.
    Preview,
}

/// Record of one `review.send`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DeliveryInfo {
    /// `d<N>` (`preview` for a dry run).
    pub id: String,
    pub task: String,
    /// Terminal the prompt is (or was) typed into.
    pub terminal: String,
    pub comment_ids: Vec<String>,
    pub state: DeliveryState,
    /// The exact text pasted (line breaks as `\n`; sent as `\r`).
    pub prompt: String,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_ms: Option<u64>,
    /// Agent state label when `review.send` was called (`working`, `idle`...).
    pub agent_state: String,
    /// Agent state label at delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_state: Option<String>,
    /// Whether the program had turned on bracketed paste (DECSET 2004) at
    /// delivery, as seen by the daemon's VT mirror. The paste markers are
    /// always sent; this is diagnostic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_bracketed_paste: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewDeliveriesParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewDeliveriesResult {
    pub deliveries: Vec<DeliveryInfo>,
    /// v8: submit confirmation of delivered prompts, by delivery id (only
    /// deliveries made by a v8 daemon). `delivered` means typed in; this
    /// says whether the agent took it as a new turn (docs/12 B1).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub submits: std::collections::BTreeMap<String, crate::protocol::SubmitCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DeliveryRef {
    pub id: String,
}

/// Counts of a task's review comments (`TaskInfo.review`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewCounts {
    pub open: usize,
    pub queued: usize,
    pub sent: usize,
    pub resolved: usize,
}

// ───────────────────────────── group types ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupCreateParams {
    /// Repository name or path.
    pub repo: String,
    /// Group name (same rules as task names). Members are named
    /// `<name>-<agent>-<n>` (agent = `claude` `codex` `shell` or `argv`).
    pub name: String,
    /// Starting point for every member (resolved once to a commit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Same prompt for every member. claude/codex get it as their CLI prompt
    /// argument; shell and argv agents get it typed in followed by Enter.
    pub prompt: String,
    /// One member task per entry.
    pub agents: Vec<AgentSpec>,
    /// Check command run in each worktree by `group.check` (e.g.
    /// `["cmd", "/c", "cargo test"]`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Vec<String>>,
    /// Caller's `POD_TERMINAL_ID` (recorded as each member's parent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// A group by id (`<repo>/<name>`) or unique name.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupRef {
    pub group: String,
}

/// Result of running the group's check command in one worktree.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CheckResult {
    pub argv: Vec<String>,
    /// Exit code 0 and not timed out.
    pub passed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub timed_out: bool,
    pub duration_ms: u64,
    pub finished_ms: u64,
    /// Last 4 KiB of stdout + stderr.
    pub output_tail: String,
    /// The command could not be started (or the worktree is missing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupMember {
    pub task: String,
    pub agent: AgentSpec,
    /// `None` when the task was removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<Box<TaskInfo>>,
    #[serde(default)]
    pub removed: bool,
    #[serde(default)]
    pub picked: bool,
    /// Last `group.check` result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupInfo {
    /// `<repo>/<name>`.
    pub id: String,
    pub repo: String,
    pub name: String,
    pub base: String,
    pub base_commit: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Vec<String>>,
    pub created_ms: u64,
    /// Winner chosen with `group.pick`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picked_ms: Option<u64>,
    pub members: Vec<GroupMember>,
    /// The last (or running) `group.check` job (v6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_job: Option<CheckJob>,
    /// Gates of the group and of its members.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<GateInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupListResult {
    pub groups: Vec<GroupInfo>,
}

/// One member in `group.compare`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CompareMember {
    pub task: String,
    pub agent: AgentSpec,
    #[serde(default)]
    pub removed: bool,
    #[serde(default)]
    pub picked: bool,
    /// Aggregate agent status label (`working`, `done`, `blocked:question`...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub files: Vec<DiffFile>,
    pub files_changed: usize,
    pub added: u64,
    pub deleted: u64,
    /// Commits ahead of the base / uncommitted entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirty: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckResult>,
    /// Why the diff is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupCompareResult {
    pub group: String,
    pub base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Vec<String>>,
    pub members: Vec<CompareMember>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupCheckParams {
    pub group: String,
    /// Set (and store) the check command first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Vec<String>>,
    /// Per-member time limit (default 600000 = 10 min).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Only these member tasks (default: every member not removed).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<String>,
    /// v6: `group.check` starts a background job and returns at once
    /// (`job.state = running`, `results` empty); results arrive per member in
    /// `group.show` and as `group.check` events on `status.subscribe`. With
    /// `wait`, the reply comes when the job has ended (only this connection
    /// waits; the daemon keeps serving others).
    #[serde(default)]
    pub wait: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CheckJobState {
    Running,
    /// Every selected member has a result (see `all_passed`).
    Finished,
    /// `group.check_cancel`: running commands were killed.
    Cancelled,
    /// The daemon stopped while the job ran.
    Interrupted,
}

/// A background `group.check` run (v6).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CheckJob {
    /// `k<N>`.
    pub id: String,
    pub state: CheckJobState,
    pub argv: Vec<String>,
    /// Member tasks being checked.
    pub tasks: Vec<String>,
    /// Members with a result so far.
    #[serde(default)]
    pub done: Vec<String>,
    pub timeout_ms: u64,
    pub started_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_ms: Option<u64>,
    /// Set when the job finished: every member passed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub all_passed: Option<bool>,
}

/// `group.check` notification (v6).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupCheckEvent {
    /// Same daemon-wide sequence as `status.changed`.
    pub seq: u64,
    pub group: String,
    pub job: CheckJob,
    /// The member that just finished, if this event is about one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<MemberCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MemberCheck {
    pub task: String,
    pub result: CheckResult,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupCheckResult {
    pub group: String,
    /// Results of the members finished so far (all of them once the job ended).
    pub results: Vec<MemberCheck>,
    /// Every checked member passed (false while the job is still running).
    pub all_passed: bool,
    /// v6: the background job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<CheckJob>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupPickParams {
    pub group: String,
    /// The winning member task.
    pub task: String,
    /// Remove the other members with the normal `task.remove` rules
    /// (refused per member when it has uncommitted or unpushed work, unless `force`).
    #[serde(default)]
    pub remove_others: bool,
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub delete_branch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MemberError {
    pub task: String,
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupPickResult {
    pub group: String,
    pub picked: String,
    pub removed: Vec<TaskRemoveResult>,
    /// Members that were not removed (e.g. `TASK_DIRTY`); nothing changed for them.
    pub kept: Vec<MemberError>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GroupRemoveResult {
    pub id: String,
}

// ───────────────────────────── gate types ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateCreateParams {
    /// Task the approval is about (id or unique name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Group the approval is about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Requesting terminal (`pod gate request` sends its `POD_TERMINAL_ID`);
    /// its task becomes the gate's task when `task` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// Short title (default "Approval needed").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub message: String,
    /// v6: resolve the gate as `expired` when nobody decided within this
    /// many seconds (default: never).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in_s: Option<u64>,
    /// v17: the orchestration item the gate blocks (docs/19 §4.5); the item
    /// is `blocked` until the gate is decided or expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// v17: the run (default: the item's run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// v17: choices; approving needs one of them (`choice`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Pending,
    Approved,
    Rejected,
    /// v6: `expires_in_s` passed while pending.
    Expired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateDecision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateInfo {
    /// `g<N>`.
    pub id: String,
    pub title: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    pub state: GateState,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// v6: unix ms when a pending gate expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
    /// v17: orchestration run and item of the gate (docs/19 §4.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<String>,
    /// v17: the choices. An `approved` gate with options has a `choice`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    /// v17: the chosen option (approved gates with options only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    /// v17: the requester's mailbox (`attempt:<id>` or `run:<id>`); it gets
    /// the decision as mail. Absent for gates of people and plain agents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<String>,
    /// v17: who decided (`local`, a mobile device, or `run:<id>` for a
    /// coordinator).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateResolveParams {
    pub id: String,
    pub decision: GateDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// v17: the chosen option; required to approve a gate with options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Only gates in this state (default: all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<GateState>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateListResult {
    pub gates: Vec<GateInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateRef {
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateWaitParams {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateWaitResult {
    /// `matched` once the gate is resolved (either way), else `timed_out`.
    pub outcome: WaitOutcome,
    pub gate: GateInfo,
    pub waited_ms: u64,
}

/// `gate.changed` notification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GateChangedEvent {
    /// Same daemon-wide sequence as `status.changed`.
    pub seq: u64,
    pub gate: GateInfo,
}

/// `review.delivery` notification: the full delivery record after the change.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewDeliveryEvent {
    /// Same daemon-wide sequence as `status.changed`.
    pub seq: u64,
    pub delivery: DeliveryInfo,
    /// v8: submit confirmation for a delivered prompt. A second event with
    /// the same delivery is sent when it becomes `confirmed`/`unconfirmed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<crate::protocol::SubmitCheck>,
}
