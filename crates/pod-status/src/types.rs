//! Shared vocabulary: agent states, evidence sources, agent kinds.

use serde::{Deserialize, Serialize};

/// Why an agent is blocked on the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
    Other,
}

/// Unified agent state (docs/04 §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "state", content = "kind", rename_all = "lowercase")]
pub enum AgentState {
    /// Initial / nothing reported yet.
    Unknown,
    Idle,
    Working,
    Blocked(BlockedKind),
    /// Turn finished and the user has not looked at the pane yet.
    Done,
    Error,
    /// The agent process is gone (terminal-level). The last state is kept in
    /// [`crate::Snapshot::final_state`].
    Exited,
}

impl AgentState {
    /// Aggregate priority: blocked > error > done > working > idle > unknown > exited.
    pub fn priority(self) -> u8 {
        match self {
            AgentState::Blocked(_) => 6,
            AgentState::Error => 5,
            AgentState::Done => 4,
            AgentState::Working => 3,
            AgentState::Idle => 2,
            AgentState::Unknown => 1,
            AgentState::Exited => 0,
        }
    }

    pub fn is_blocked(self) -> bool {
        matches!(self, AgentState::Blocked(_))
    }

    /// Short lowercase name (`blocked` for every kind).
    pub fn name(self) -> &'static str {
        match self {
            AgentState::Unknown => "unknown",
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Blocked(_) => "blocked",
            AgentState::Done => "done",
            AgentState::Error => "error",
            AgentState::Exited => "exited",
        }
    }
}

/// Pick the highest-priority state (`blocked > error > done > working > idle > unknown`).
/// Returns `Unknown` for an empty iterator.
pub fn aggregate<I: IntoIterator<Item = AgentState>>(states: I) -> AgentState {
    states
        .into_iter()
        .max_by_key(|s| s.priority())
        .unwrap_or(AgentState::Unknown)
}

/// Where a piece of evidence came from. Ordered by trust:
/// `Hook > Osc > Mod > Heuristic > Process`. `Mod` is a status detector or
/// hint from a Pod extension (docs/16 §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Process = 0,
    Heuristic = 1,
    Mod = 2,
    Osc = 3,
    Hook = 4,
}

impl Source {
    pub fn rank(self) -> u8 {
        self as u8
    }
}

/// Which agent CLI runs in the terminal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    /// Any other program that self-reports via OSC 7501 (`app=` value).
    Other(String),
}

impl AgentKind {
    /// Map an OSC 7501 `app=` value to an agent.
    pub fn from_osc_app(app: &str) -> AgentKind {
        match app {
            "claude-code" | "claude" => AgentKind::Claude,
            "codex" | "codex-cli" => AgentKind::Codex,
            other => AgentKind::Other(other.to_string()),
        }
    }
}

/// How the last turn ended (only known from hooks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Completed,
    Cancelled,
    Failed,
}

/// Milliseconds on a monotonic clock chosen by the caller (podd). All engine
/// APIs take `now` explicitly so the engine stays pure and testable.
pub type Millis = u64;
