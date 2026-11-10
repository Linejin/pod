//! Wire types for agent status (P1): `terminal.show`, `terminal.ack`,
//! `status.wait`, `status.subscribe` and the `status` field of
//! [`TerminalInfo`](crate::protocol::TerminalInfo).
//!
//! Kept apart from `protocol.rs` so the status work stays additive; everything
//! here is re-exported from [`crate::protocol`]. The engine itself lives in the
//! `pod-status` crate and is wired into the daemon by `server/status.rs`
//! (docs/07 §8).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::{
    GateInfo, Method, TaskGit, TaskInfo, TerminalInfo, TerminalRef, WaitOutcome,
};

/// Agent status of one terminal, as computed by the `pod-status` engine.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StatusInfo {
    /// `unknown` `idle` `working` `blocked` `done` `error` `exited`.
    pub state: String,
    /// For `blocked`: `permission` `question` `auth` `other`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Evidence that decided the state: `hook` `osc` `heuristic` `process`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// 0..1 (hook 0.95, osc 0.9, screen 0.7, title 0.6, quiet 0.4, process 0.2;
    /// halved when the deciding evidence is stale).
    pub confidence: f32,
    /// Unix ms when the state (including the blocked kind) last changed.
    pub since: u64,
    /// Incremented every time the state leaves `working`.
    pub turn: u64,
    /// `claude`, `codex`, or the OSC `app=` of another self-reporting program.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// OSC 7501 `app=` value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Task title (OSC 7501 `title=`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Human-readable detail: question text, error, tool name...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    /// How the last turn ended (hooks only): `completed` `cancelled` `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// For `exited`: the state before the program exited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_state: Option<String>,
    /// Badge text, e.g. `done (+2 running)`, `blocked:permission`.
    pub badge: String,
    /// Sub-records (OSC `id=` records, hook subagents).
    #[serde(default)]
    pub records: usize,
    /// Sub-records still `working` (held open: `done (+N running)`).
    #[serde(default)]
    pub running_children: usize,
    #[serde(default)]
    pub blocked_children: usize,
    /// Hook messages accepted / rejected (bad token) for this terminal.
    #[serde(default)]
    pub hooks_accepted: u64,
    #[serde(default)]
    pub hooks_rejected: u64,
}

impl StatusInfo {
    /// `blocked:<kind>` for blocked states, else the state name.
    pub fn state_label(&self) -> String {
        match &self.kind {
            Some(k) if self.state == "blocked" => format!("blocked:{k}"),
            _ => self.state.clone(),
        }
    }
}

pub struct TerminalShow;
impl Method for TerminalShow {
    const NAME: &'static str = "terminal.show";
    type Params = TerminalRef;
    type Result = TerminalInfo;
}

/// The user looked at the terminal: `done` becomes `idle`.
pub struct TerminalAck;
impl Method for TerminalAck {
    const NAME: &'static str = "terminal.ack";
    type Params = TerminalRef;
    type Result = TerminalInfo;
}

pub struct StatusWait;
impl Method for StatusWait {
    const NAME: &'static str = "status.wait";
    type Params = StatusWaitParams;
    type Result = StatusWaitResult;
}

/// Turns the connection into a stream of `status.changed` notifications.
pub struct StatusSubscribe;
impl Method for StatusSubscribe {
    const NAME: &'static str = "status.subscribe";
    type Params = StatusSubscribeParams;
    type Result = StatusSubscribeResult;
}

pub const NOTIFY_STATUS_CHANGED: &str = "status.changed";
/// The subscriber fell behind and `missed` events were dropped; re-read
/// `terminal.list` to resynchronize.
pub const NOTIFY_STATUS_LAGGED: &str = "status.lagged";

pub const STATUS_METHODS: &[&str] = &[
    TerminalShow::NAME,
    TerminalAck::NAME,
    StatusWait::NAME,
    StatusSubscribe::NAME,
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StatusWaitParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    /// A state name (`idle` `working` `done` `blocked` `error` `unknown`
    /// `exited`) to wait for (matches immediately if already in it), or
    /// `any_change` for the next state change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Wait until the turn counter advances and the state is no longer
    /// `working` (the current or next turn ended). Combined with `status`,
    /// both must hold.
    #[serde(default)]
    pub turn_end: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StatusWaitResult {
    /// `exited`: the terminal exited without the condition being met.
    pub outcome: WaitOutcome,
    pub status: StatusInfo,
    pub terminal: TerminalInfo,
    pub waited_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StatusSubscribeParams {
    /// Only these terminal ids (default: all, including terminals created later).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ids: Vec<String>,
    /// v9: also stream lifecycle events: `terminal.changed`, and (only
    /// without `ids`) `task.changed` and `task.removing`. Opt-in so older
    /// clients see no new notification kinds.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lifecycle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StatusEntry {
    pub id: String,
    /// Sequence number of the event that produced this status.
    pub seq: u64,
    pub status: StatusInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StatusSubscribeResult {
    /// Identifies this daemon instance (start time, unix ms); sequence numbers
    /// restart with every daemon.
    pub epoch: u64,
    /// Highest sequence number reflected in `terminals`. Every later
    /// `status.changed` has a larger `seq`.
    pub seq: u64,
    pub terminals: Vec<StatusEntry>,
    /// Pending gates (v5). Later creations and resolutions arrive as
    /// `gate.changed` notifications on the same stream.
    #[serde(default)]
    pub gates: Vec<GateInfo>,
}

/// `status.changed`: one terminal's status changed (or it was removed).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StatusChangedEvent {
    /// Daemon-wide, strictly increasing.
    pub seq: u64,
    pub id: String,
    pub status: StatusInfo,
    /// Previous state label (`blocked:<kind>` for blocked), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The terminal was removed (`terminal.rm`); last event for this id.
    #[serde(default)]
    pub removed: bool,
}

/// v9 lifecycle notifications (with `StatusSubscribeParams.lifecycle`).
pub const NOTIFY_TERMINAL_CHANGED: &str = "terminal.changed";
pub const NOTIFY_TASK_CHANGED: &str = "task.changed";
pub const NOTIFY_TASK_REMOVING: &str = "task.removing";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalChange {
    /// Started (`terminal.run`, also inside a task).
    Created,
    /// The program exited (or its holder was lost).
    Exited,
    /// Removed (`terminal.rm`, task removal, retention); last event for the id.
    Removed,
    /// Reserved: podd has no rename method yet (names are fixed at creation).
    Renamed,
}

/// `terminal.changed` (v9): a terminal appeared, exited or was removed.
/// Status changes stay on `status.changed`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TerminalChangedEvent {
    /// Same daemon-wide sequence as `status.changed`.
    pub seq: u64,
    pub id: String,
    pub change: TerminalChange,
    /// The terminal after the change (absent for `removed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<Box<TerminalInfo>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskChange {
    /// `task.create` finished; `task` carries the full `TaskInfo`.
    Created,
    /// `task.remove` finished; last event for the id.
    Removed,
    /// Reserved: podd has no rename method yet (the name fixes branch and worktree).
    Renamed,
    /// The aggregate agent status changed; `status` is the new one (absent:
    /// no terminal of the task has a status any more).
    Status,
    /// Git facts changed (ahead, behind, dirty, unpushed or HEAD); `git` is new.
    Git,
    /// The task's terminal list changed; `terminals` is the new list.
    Terminals,
    /// A client saved files (`task.touch`): refresh diffs even if the counts
    /// did not change. `paths` as given by the client.
    Touched,
    /// Session fields changed (lifecycle, setup, brief, agent session;
    /// docs/19): re-read the task.
    Updated,
}

/// `task.changed` (v9). Only the fields of the change are set; `task.show`
/// gives the rest.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskChangedEvent {
    /// Same daemon-wide sequence as `status.changed`.
    pub seq: u64,
    /// Task id `<repo>/<name>`.
    pub id: String,
    pub change: TaskChange,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<Box<TaskInfo>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<TaskGit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terminals: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    /// `created`: how the app should open the task (docs/19 §3.1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_hint: Option<crate::session_protocol::OpenHint>,
}

/// `task.removing` (v9): `task.remove` passed its checks and is about to kill
/// the task's terminals and delete its worktree. Editors showing files from
/// `worktree` should save or close them now. The daemon waits `grace_ms`
/// (and, without `force`, re-checks for uncommitted changes) before deleting.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskRemovingEvent {
    pub seq: u64,
    pub id: String,
    pub worktree: String,
    pub force: bool,
    pub grace_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StatusLaggedEvent {
    pub missed: u64,
}

/// Aggregate agent status of a task's terminals (`TaskInfo.status`): the
/// highest-priority display state, `blocked > error > done > working > idle >
/// unknown > exited` (a blocked sub-record counts as blocked).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskStatus {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// The terminal that decided the aggregate.
    pub terminal: String,
    /// That terminal's badge, e.g. `done (+1 running)`.
    pub badge: String,
    /// Terminals with a known status.
    pub terminals: usize,
}

/// Aggregate priority of a state name (higher wins).
pub fn state_priority(state: &str) -> u8 {
    match state {
        "blocked" => 6,
        "error" => 5,
        "done" => 4,
        "working" => 3,
        "idle" => 2,
        "unknown" => 1,
        _ => 0, // exited
    }
}

impl StatusInfo {
    /// (state, kind) used for aggregation: the badge's state, i.e. the root
    /// state raised to `blocked` when a sub-record waits on the user.
    pub fn display_state(&self) -> (String, Option<String>) {
        let label = self.badge.split(" (").next().unwrap_or(&self.badge);
        match label.split_once(':') {
            Some((s, k)) => (s.to_string(), Some(k.to_string())),
            None => (label.to_string(), None),
        }
    }
}

/// Aggregate the statuses of a task's terminals (`None` when none has one).
pub fn aggregate_task_status<'a, I>(terminals: I) -> Option<TaskStatus>
where
    I: IntoIterator<Item = &'a TerminalInfo>,
{
    let mut best: Option<(u8, TaskStatus)> = None;
    let mut n = 0;
    for t in terminals {
        let Some(s) = &t.status else { continue };
        n += 1;
        let (state, kind) = s.display_state();
        let p = state_priority(&state);
        // Ties keep the first (oldest) terminal.
        if best.as_ref().is_none_or(|(bp, _)| p > *bp) {
            best = Some((
                p,
                TaskStatus {
                    state,
                    kind,
                    terminal: t.id.clone(),
                    badge: s.badge.clone(),
                    terminals: 0,
                },
            ));
        }
    }
    best.map(|(_, mut s)| {
        s.terminals = n;
        s
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TerminalState;

    fn term(id: &str, state: &str, kind: Option<&str>, badge: &str) -> TerminalInfo {
        TerminalInfo {
            id: id.into(),
            name: None,
            argv: vec![],
            cwd: String::new(),
            state: TerminalState::Running,
            exit_code: None,
            pid: None,
            holder_pid: None,
            cols: 80,
            rows: 24,
            created_ms: 0,
            exited_ms: None,
            killed: false,
            lost: false,
            attached: 0,
            conpty: None,
            status: Some(StatusInfo {
                state: state.into(),
                kind: kind.map(Into::into),
                source: None,
                confidence: 0.0,
                since: 0,
                turn: 0,
                agent: None,
                app: None,
                title: None,
                msg: None,
                progress: None,
                outcome: None,
                final_state: None,
                badge: badge.into(),
                records: 0,
                running_children: 0,
                blocked_children: 0,
                hooks_accepted: 0,
                hooks_rejected: 0,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn task_aggregate_uses_priority_and_display_state() {
        assert_eq!(aggregate_task_status(&[]), None);
        let ts = [
            term("t1", "working", None, "working"),
            term("t2", "done", None, "done (+1 running)"),
            term("t3", "exited", None, "exited"),
        ];
        let a = aggregate_task_status(&ts).unwrap();
        assert_eq!((a.state.as_str(), a.terminal.as_str()), ("done", "t2"));
        assert_eq!(a.badge, "done (+1 running)");
        assert_eq!(a.terminals, 3);
        // A blocked sub-record raises the display state.
        let ts = [
            term("t1", "error", None, "error"),
            term("t2", "idle", None, "blocked:question"),
        ];
        let a = aggregate_task_status(&ts).unwrap();
        assert_eq!(a.state, "blocked");
        assert_eq!(a.kind.as_deref(), Some("question"));
        assert_eq!(a.terminal, "t2");
    }
}
