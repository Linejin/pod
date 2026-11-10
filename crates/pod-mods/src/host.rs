//! What the embedding process (podd) provides: read-only facts and the
//! side effects mods may request. `pod-mods` checks permissions and scope
//! before calling any of these.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::types::ModInfo;

/// A terminal as mods see it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TerminalFacts {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// Agent state (`idle`, `working`, `blocked`, ...).
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// File name of `argv[0]`.
    pub argv0: String,
    pub running: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskFacts {
    pub id: String,
    pub name: String,
    pub repo: String,
    /// Aggregate agent state of the task's terminals.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    pub terminals: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroupMemberFacts {
    pub task: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct GroupFacts {
    pub id: String,
    pub name: String,
    pub members: Vec<GroupMemberFacts>,
}

/// `pod.notify`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Notice {
    #[serde(rename = "mod")]
    pub mod_name: String,
    pub title: String,
    pub body: String,
    /// `info`, `success`, `warning`, `error`.
    pub level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

/// A status a mod reports for a terminal (detector result or `setStatus`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusHint {
    /// `idle`, `working`, `blocked`, `done`, `error`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
}

pub const HINT_STATES: &[&str] = &["idle", "working", "blocked", "done", "error"];
pub const BLOCKED_KINDS: &[&str] = &["permission", "question", "auth", "other"];

/// The task (and terminal) a callback may act on (docs/16 §5–§6).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Scope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

/// Implemented by podd. Called on the mod thread; must not block for long.
pub trait Host: Send + Sync + 'static {
    fn terminals(&self) -> Vec<TerminalFacts>;
    fn tasks(&self) -> Vec<TaskFacts>;
    fn groups(&self) -> Vec<GroupFacts>;
    fn notify(&self, notice: Notice);
    fn send_text(&self, terminal: &str, text: &str, enter: bool) -> Result<(), String>;
    /// `None` clears this mod's evidence on that terminal.
    fn set_status(
        &self,
        owner: &str,
        terminal: &str,
        hint: Option<StatusHint>,
    ) -> Result<(), String>;
    /// Drop every status hint `owner` set (mod stopped or disabled).
    fn clear_status(&self, owner: &str);
    /// Start `argv` in the task's worktree in the background; the host posts
    /// a `check.finished` event to `owner` (see `ModHandle::post_to`).
    fn run_check(
        &self,
        owner: &str,
        task: &str,
        check: &str,
        argv: Vec<String>,
    ) -> Result<(), String>;
    /// Create a pending gate; returns its id.
    fn request_gate(
        &self,
        owner: &str,
        task: &str,
        title: &str,
        message: &str,
    ) -> Result<String, String>;
    /// A mod was enabled, disabled, stopped or reloaded.
    fn mod_changed(&self, info: &ModInfo);
}
