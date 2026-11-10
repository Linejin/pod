//! Wire types for worktree sessions (docs/19 S1–S3): auto names, briefs,
//! open hints, repository config (`.pod/config.toml`) with content-hash
//! approval, setup terminals, archive / cleanup and agent-session resume.
//!
//! Kept apart from `protocol.rs` so this work stays additive; everything here
//! is re-exported from [`crate::protocol`]. The daemon side lives in
//! `server/sessions.rs`, `server/repo_config.rs` and `server/task_archive.rs`.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::{Method, TerminalInfo};

method!(
    TaskArchive,
    "task.archive",
    TaskArchiveParams,
    TaskArchiveResult
);
method!(
    TaskUnarchive,
    "task.unarchive",
    TaskUnarchiveParams,
    TaskResumeResult
);
method!(
    TaskResume,
    "task.resume",
    TaskResumeParams,
    TaskResumeResult
);
method!(
    TaskCleanupCandidates,
    "task.cleanup_candidates",
    CleanupParams,
    CleanupResult
);
method!(
    TaskSuggestName,
    "task.suggest_name",
    SuggestNameParams,
    SuggestNameResult
);
method!(
    RepoConfigMethod,
    "repo.config",
    RepoConfigParams,
    RepoConfigResult
);
method!(RepoSet, "repo.set", RepoSetParams, RepoConfigResult);

/// Methods of this module (for `schema()` tests and capability probes).
pub const SESSION_METHODS: &[&str] = &[
    TaskArchive::NAME,
    TaskUnarchive::NAME,
    TaskResume::NAME,
    TaskCleanupCandidates::NAME,
    TaskSuggestName::NAME,
    RepoConfigMethod::NAME,
    RepoSet::NAME,
];

/// Error codes of this module. -32050 is `SERVER_STOPPING`; the
/// orchestration core (docs/19 S4) takes the -3205x/-3206x range, so these
/// sit at -3207x.
pub mod session_codes {
    /// `task.archive` without `force` while an agent of the task is working
    /// or blocked. `data = {terminal, state}`.
    pub const TASK_BUSY: i64 = -32070;
    /// The archive script failed (or is not approved); nothing was changed.
    /// `data = {exit_code?, output}`.
    pub const ARCHIVE_FAILED: i64 = -32071;
    /// `task.remove` refused: the worktree is locked (`git worktree lock`)
    /// or another worktree lives inside it.
    pub const WORKTREE_LOCKED: i64 = -32072;
}

// ───────────────────────────── task.create additions ─────────────────────────────

/// Who asked for a task, and whether the app should move focus to it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenReason {
    /// A person (app dialog, `pod task new` from an ordinary shell).
    User,
    /// An agent started it from a Pod terminal (worker).
    Worker,
    /// Re-opened / resumed session.
    Session,
}

/// `task.changed created` carries this so the app opens the task's agent
/// terminal by itself: `focus` = also move keyboard focus (user tasks);
/// worker tasks get a tab without focus.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct OpenHint {
    pub focus: bool,
    pub reason: OpenReason,
}

/// `TaskCreateParams.setup`.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupPolicy {
    /// Run the setup script if it is approved (or personal).
    Run,
    /// Do not run setup (or the default tabs).
    Skip,
    /// Same as `run` when the repository has a setup script (default).
    #[default]
    Inherit,
}

/// When the brief / first line goes to the agent relative to setup.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupMode {
    /// Setup and agent start together (default).
    #[default]
    Parallel,
    /// The first line is typed only after setup finished (ok or failed).
    Wait,
}

// ───────────────────────────── TaskInfo additions ─────────────────────────────

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    #[default]
    Active,
    /// Terminals closed, worktree and branch kept (`task.archive`).
    Archived,
}

impl Lifecycle {
    pub fn is_active(&self) -> bool {
        *self == Lifecycle::Active
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SetupState {
    Running,
    Ok,
    Failed,
    Skipped,
    /// The repository's commands are not approved for this content hash.
    Untrusted,
}

/// `TaskInfo.setup`: the setup terminal's state.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SetupInfo {
    pub state: SetupState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// Terminal running the script.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    /// `repo` (`.pod/config.toml`) or `personal` (`repo.set`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// For `untrusted`: the content hash to approve (`repo.set {trust}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub finished_ms: u64,
}

fn is_zero(v: &u64) -> bool {
    *v == 0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BriefDelivery {
    /// The first line is the agent CLI's prompt argument (claude/codex).
    Argv,
    /// The first line is typed into the agent terminal (+ Enter).
    Typed,
    /// Only `POD_BRIEF` is set (shell agents: typing would run a command).
    Env,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BriefState {
    /// Waiting (for setup in `wait` mode, or for the agent to start).
    Pending,
    Sent,
    Failed,
}

/// `TaskInfo.brief`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct BriefInfo {
    /// The brief file (`POD_BRIEF`).
    pub path: String,
    /// The line given to the agent (no `"`, `%`, CR or LF).
    pub first_line: String,
    pub delivery: BriefDelivery,
    pub state: BriefState,
    /// Version of Pod's header text in the brief file.
    pub brief_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// `wait` mode: typed only after setup finished.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub after_setup: bool,
}

/// `TaskInfo.agent_session`: the agent's own conversation id, for resume.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AgentSession {
    /// `claude` or `codex`.
    pub agent: String,
    pub id: String,
    pub captured_ms: u64,
    /// `hook` (status hook payload), `discovery` (session discovery) or `user`.
    pub source: String,
}

// ───────────────────────────── archive / resume ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskArchiveParams {
    pub task: String,
    /// Archive even while an agent is working or blocked.
    #[serde(default)]
    pub force: bool,
    /// Archive even when the archive script fails or is not approved.
    #[serde(default)]
    pub ignore_script_failure: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskArchiveResult {
    pub task: String,
    /// Terminals closed.
    pub terminals: Vec<String>,
    /// The archive script ran (`None`: there is none or it was skipped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<ScriptRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ScriptRun {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    /// Last 4 KiB of combined output.
    pub output: String,
    /// Not run: why (e.g. not approved), with `ignore_script_failure`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskUnarchiveParams {
    pub task: String,
    /// Restart the agent (resume its session when one is recorded). Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_agent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskResumeParams {
    pub task: String,
    /// Session id to resume (default: the recorded `agent_session`), e.g.
    /// from session discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    /// `claude` / `codex` for `session` (default: the recorded one or the
    /// task's agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskResumeResult {
    pub task: String,
    /// The new agent terminal (`None` for `task.unarchive` without agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalInfo>,
    /// The command it runs.
    #[serde(default)]
    pub argv: Vec<String>,
    /// The resumed agent session (`None`: a fresh agent was started).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resumed_session: Option<String>,
}

// ───────────────────────────── cleanup ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CleanupParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Clean tasks idle this long are `ready` (default 7).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inactive_days: Option<u32>,
}

#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(rename_all = "snake_case")]
pub enum CleanupTier {
    /// Never touched by bulk cleanup (running agent, unknown state, locked, recent).
    Protected,
    /// Would lose or hide work (dirty, unpushed, base gone): look first.
    Review,
    /// Clean and (idle, merged or archived): selected by default.
    Ready,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CleanupCandidate {
    pub task: String,
    pub tier: CleanupTier,
    pub reasons: Vec<String>,
    pub lifecycle: Lifecycle,
    /// Last activity (last commit, creation or terminal start), Unix ms.
    pub last_activity_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CleanupResult {
    pub candidates: Vec<CleanupCandidate>,
}

// ───────────────────────────── names ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SuggestNameParams {
    pub repo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SuggestNameResult {
    /// Not used by a live or removed task, branch or worktree directory.
    pub name: String,
}

// ───────────────────────────── repository config ─────────────────────────────

/// `.pod/config.toml` in the repository (all sections optional):
///
/// ```toml
/// [setup]
/// script = "pnpm install --frozen-lockfile"   # once per new worktree
/// mode = "parallel"                           # parallel | wait
///
/// [[tabs]]                                    # extra terminals of a new task
/// title = "dev"
/// command = "pnpm dev"
///
/// [archive]
/// script = "cargo clean"                      # before archive
///
/// [worker]
/// agent = "claude"                            # default agent for workers
/// codex_allowance = true                      # docs/19 §18.1 (default on)
/// ```
///
/// Commands run in Windows PowerShell (`powershell -NoProfile`) in the
/// task's worktree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RepoConfigFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupSection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tabs: Vec<TabSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<ArchiveSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerSection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<WorktreeSection>,
}

/// `[worktree]` (docs/19 §3.2, S8): how new task worktrees are checked out.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorktreeSection {
    /// Cone-mode sparse checkout: only these directories (plus the files
    /// at the root and `.pod`) are checked out, e.g. `["crates/podd"]`.
    /// Read from the base commit; data only, so no approval is needed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sparse: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SetupSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<SetupMode>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TabSection {
    pub title: String,
    pub command: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArchiveSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkerSection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Extra environment of worker agent terminals.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Narrow allowance for Pod-started Codex workers (docs/19 §18.1): `pod
    /// orch` coordination and `git add`/`commit` without approval, all else
    /// sandboxed. Default on; `false` turns it off for this repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_allowance: Option<bool>,
}

/// The user's own settings for a repository (`repo.set personal`). They win
/// over `.pod/config.toml` section by section and need no approval.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PersonalConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupSection>,
    /// `Some(vec![])` = no tabs even if the repository lists some.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tabs: Option<Vec<TabSection>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<ArchiveSection>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RepoConfigParams {
    pub repo: String,
    /// Read `.pod/config.toml` from this task's worktree instead of the
    /// repository root (what that task's setup used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TrustParams {
    /// The `hash` shown by `repo.config` (must match the current file).
    pub hash: String,
    /// `true` approves, `false` withdraws the approval.
    pub approve: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RepoSetParams {
    pub repo: String,
    /// Worktree root for this repository's new tasks; `""` = the daemon default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees_root: Option<String>,
    /// Replace the personal settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal: Option<PersonalConfig>,
    /// Remove the personal settings.
    #[serde(default)]
    pub clear_personal: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trust: Option<TrustParams>,
    /// For `trust`: read the file from this task's worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

/// One command of the effective configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ConfigCommand {
    /// `setup`, `tab:<title>` or `archive`.
    pub kind: String,
    pub command: String,
    /// `repo` or `personal`.
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RepoConfigResult {
    pub repo: String,
    /// The file read.
    pub path: String,
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<RepoConfigFile>,
    /// Parse error (the file is then ignored).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// SHA-256 (hex) of the file's commands (setup, tabs, archive); `None`
    /// when it has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The current hash is approved.
    pub trusted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub personal: Option<PersonalConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktrees_root: Option<String>,
    /// Effective commands (personal sections replace the file's).
    #[serde(default)]
    pub commands: Vec<ConfigCommand>,
    pub setup_mode: SetupMode,
    /// Effective worker agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_agent: Option<String>,
    /// Effective `[worker] codex_allowance` (docs/19 §18.1). `None` from a
    /// daemon before it (unknown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_allowance: Option<bool>,
}
