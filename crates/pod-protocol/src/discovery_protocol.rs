//! Wire types for session discovery (protocol v12, docs/18): `sessions.*`
//! methods and the `sessions.changed` notification. Re-exported from
//! [`crate::protocol`]. Daemon side: `server/sessions.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use pod_discover::Agent as DiscoveredAgent;
pub use pod_discover::matching::CwdMatch;

use crate::protocol::{Method, TerminalInfo};

method!(
    SessionsListMethod,
    "sessions.list",
    SessionsListParams,
    SessionsList
);
method!(
    SessionsScan,
    "sessions.scan",
    SessionsScanParams,
    SessionsList
);
method!(
    SessionsSettings,
    "sessions.settings",
    SessionsSettingsParams,
    SessionsList
);
method!(
    SessionsRegister,
    "sessions.register",
    SessionsRegisterParams,
    DiscoveredSession
);
method!(
    SessionsResume,
    "sessions.resume",
    SessionsResumeParams,
    TerminalInfo
);
method!(
    SessionsHide,
    "sessions.hide",
    SessionsHideParams,
    DiscoveredSession
);

/// `sessions.changed` on `status.subscribe` with `lifecycle: true`: the
/// discovered sessions, their state or the settings changed. Re-read with
/// `sessions.list`.
pub const NOTIFY_SESSIONS_CHANGED: &str = "sessions.changed";

pub const SESSIONS_METHODS: &[&str] = &[
    SessionsListMethod::NAME,
    SessionsScan::NAME,
    SessionsSettings::NAME,
    SessionsRegister::NAME,
    SessionsResume::NAME,
    SessionsHide::NAME,
];

/// Application error codes added with protocol v12.
pub mod sessions_codes {
    /// No discovered session has that key (or the id prefix is ambiguous).
    pub const SESSION_NOT_FOUND: i64 = -32040;
    /// The session is running (in a Pod terminal: `data.pod_terminal`, or
    /// in another app's terminal); it can be resumed after it ends.
    pub const SESSION_RUNNING: i64 = -32041;
    /// Session discovery is off (`sessions.settings {enabled: true}` or
    /// `sessions.scan {enable: true}` turns it on).
    pub const DISCOVERY_OFF: i64 = -32042;
    /// The change is in effect in the daemon but could not be saved, or
    /// session summaries could not be deleted from disk (security-04).
    /// `data = {paths: [...], error}`; retried on the next `sessions.*` call.
    pub const PERSIST_FAILED: i64 = -32043;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionsListParams {
    /// Also list hidden sessions.
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionsScanParams {
    /// Turn discovery on first (it is off by default).
    #[serde(default)]
    pub enable: bool,
    #[serde(default)]
    pub include_hidden: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionsSettingsParams {
    /// Discovery on/off. Turning it off keeps hidden flags and links but
    /// forgets the scanned sessions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Store and show the first prompt of each session. Turning it off
    /// deletes the stored previews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionsRegisterParams {
    /// `"<agent>:<id>"`, or a unique id (prefix).
    pub session: String,
    /// Link to this task instead of the one matching the cwd.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// No registered repository holds the cwd: register its git repository
    /// (the main checkout of a linked worktree) first.
    #[serde(default)]
    pub add_repo: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionsResumeParams {
    pub session: String,
    /// Terminal name (must be unique); default none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionsHideParams {
    pub session: String,
    /// `false` shows the session again. Default true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// Running in one of Pod's terminals (`running.pod_terminal`).
    RunningPod,
    /// Running in another app's terminal: read-only until it ends.
    RunningOther,
    /// Can be resumed.
    #[default]
    Resumable,
    /// Last activity over 30 days ago, or the cwd folder is gone.
    Stale,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionRunning {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_terminal: Option<String>,
    /// The process was tied to this session by cwd and timing only.
    #[serde(default)]
    pub guessed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DiscoveredSession {
    /// `"<agent>:<id>"`.
    pub key: String,
    pub agent: DiscoveredAgent,
    pub id: String,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// First prompt (one line, truncated); absent when previews are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// How it was started when recorded (Codex `source`, Claude entrypoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ms: Option<u64>,
    /// Approximate message count; absent for very large or not yet counted files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<u64>,
    pub state: SessionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub running: Option<SessionRunning>,
    /// Repository / task the cwd belongs to (auto-match).
    #[serde(rename = "match", default, skip_serializing_if = "CwdMatch::is_empty")]
    pub matched: CwdMatch,
    /// Set by `sessions.register`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<CwdMatch>,
    #[serde(default)]
    pub hidden: bool,
    pub cwd_exists: bool,
    /// What `sessions.resume` runs (in `cwd`).
    #[serde(default)]
    pub resume_argv: Vec<String>,
}

/// An interactive agent process whose session could not be identified.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UnknownRunning {
    pub agent: Option<DiscoveredAgent>,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_terminal: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionsList {
    pub enabled: bool,
    pub preview: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scanned_ms: Option<u64>,
    /// The last scan stopped at a limit (more files than read).
    #[serde(default)]
    pub truncated: bool,
    /// Newest first.
    #[serde(default)]
    pub sessions: Vec<DiscoveredSession>,
    /// Hidden sessions not listed (without `include_hidden`).
    #[serde(default)]
    pub hidden: usize,
    #[serde(default)]
    pub unknown_running: Vec<UnknownRunning>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SessionsChangedEvent {
    pub seq: u64,
    pub enabled: bool,
    pub count: usize,
    pub running: usize,
}
