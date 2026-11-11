//! Running agent processes (docs/18 §2): which interactive Claude Code /
//! Codex processes run right now, in which cwd, for which session, and
//! whether they run inside one of Pod's own terminals.
//!
//! [`snapshot`] reads the process list (Windows); everything else is pure and
//! tested with synthetic process lists.
//!
//! The pure part is in `classify`, the Windows snapshot in `windows`.

mod classify;
#[cfg(windows)]
mod windows;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Agent;

pub use classify::{
    attach_sessions, find_agents, is_candidate_name, mark_pod_terminals, split_cmdline,
};

/// One process as seen by [`snapshot`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    /// Executable file name, e.g. `claude.exe`.
    pub name: String,
    /// Creation time (unix ms), 0 when unknown.
    pub start_ms: u64,
    /// Only read for agent candidates.
    pub cmdline: Option<String>,
    pub cwd: Option<String>,
}

/// An interactive agent process (the outermost of a shim -> binary chain).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunningAgent {
    pub agent: Option<Agent>,
    pub pid: u32,
    /// This process and the same-agent processes below it.
    #[serde(default)]
    pub pids: Vec<u32>,
    pub start_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The session was inferred from cwd and timing, not recorded.
    #[serde(default)]
    pub guessed: bool,
    /// The Pod terminal this process runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_terminal: Option<String>,
}

/// The current process list: pid, parent, name and start time for all;
/// command line and cwd for agent candidates ([`is_candidate_name`]).
/// Read-only; processes we may not open just lack details.
#[cfg(windows)]
pub fn snapshot() -> Vec<ProcInfo> {
    windows::snapshot()
}

#[cfg(not(windows))]
pub fn snapshot() -> Vec<ProcInfo> {
    Vec::new()
}
