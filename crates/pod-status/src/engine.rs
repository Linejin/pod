//! Per-terminal status engine: merges hook / OSC / heuristic / process evidence
//! into one [`AgentState`] (docs/04 §4.1–§4.2).
//!
//! The engine is pure and synchronous: podd calls the `on_*` methods with the
//! terminal's output bytes, title changes, screen text, foreground process,
//! hook messages and the current monotonic time, and reads [`Snapshot`]s.
//!
//! Layout: this file holds the public types, the per-terminal struct, the
//! snapshot and the transition step; `engine/evidence.rs` ranks the evidence
//! slots (priority, staleness, done-until-ack) and `engine/input.rs` applies
//! each input (PTY output and OSC reports, typed answers, title/screen/process,
//! hooks, mods, ack, ticks).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::heuristic::HeuristicTracker;
use crate::osc::Osc7501Parser;
use crate::types::{AgentKind, AgentState, BlockedKind, Millis, Outcome, Source};

mod evidence;
mod input;

use evidence::Evidence;
pub use evidence::{CONF_ANSWERED, CONF_HOOK, CONF_MOD, CONF_OSC, CONF_PROCESS};
pub use input::{ANSWER_OUTPUT_WINDOW_MS, is_answer_input};

/// Tunables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusConfig {
    /// Hook evidence older than this may be overridden by newer lower-source evidence.
    pub hook_stale_ms: Millis,
    /// Quiet time (no output, no spinner) before a no-glyph agent (Codex) is idle.
    pub quiet_idle_ms: Millis,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            hook_stale_ms: 120_000,
            quiet_idle_ms: 3_000,
        }
    }
}

/// A sub-task record (OSC `id=` record or a hook subagent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubRecord {
    pub id: String,
    pub state: AgentState,
    pub source: Source,
    pub title: Option<String>,
    pub msg: Option<String>,
    pub progress: Option<u8>,
    pub since: Millis,
}

/// A state change of the root record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transition {
    pub from: AgentState,
    pub to: AgentState,
    pub source: Option<Source>,
    pub turn: u64,
    pub at: Millis,
}

/// Result of [`TerminalStatus::feed_output`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutputResult {
    /// Number of `ESC]7501;?` queries seen; podd writes [`crate::QUERY_REPLY`]
    /// to the PTY input once per query.
    pub queries: usize,
    /// Root transition caused by this chunk, if any.
    pub transition: Option<Transition>,
    /// Any change visible in the snapshot (state, records, msg, progress…).
    pub changed: bool,
    /// Number of dropped (invalid) 7501 sequences, for diagnostics.
    pub invalid: usize,
}

/// Everything a UI needs to render a terminal's agent status.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Root state (Exited when the agent process is gone).
    pub state: AgentState,
    /// Root state before exit (only when `state == Exited`).
    pub final_state: Option<AgentState>,
    pub source: Option<Source>,
    pub confidence: f32,
    pub since: Millis,
    /// Incremented every time the root leaves `working`.
    pub turn: u64,
    pub agent: Option<AgentKind>,
    /// OSC `app=` value if reported.
    pub app: Option<String>,
    pub msg: Option<String>,
    pub title: Option<String>,
    pub progress: Option<u8>,
    pub outcome: Option<Outcome>,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub records: Vec<SubRecord>,
    /// Records in `working`.
    pub running_children: usize,
    /// Records in `blocked`.
    pub blocked_children: usize,
}

impl Snapshot {
    /// State used for badges and aggregation: the root state, raised to
    /// `blocked` if any sub-record waits on the user.
    pub fn display_state(&self) -> AgentState {
        if !self.state.is_blocked()
            && self.state != AgentState::Exited
            && let Some(r) = self.records.iter().find(|r| r.state.is_blocked())
        {
            return r.state;
        }
        self.state
    }

    /// Short badge text, e.g. `done (+2 running)`, `blocked:permission`.
    pub fn badge(&self) -> String {
        let st = self.display_state();
        let mut s = match st {
            AgentState::Blocked(k) => format!("blocked:{}", blocked_name(k)),
            other => other.name().to_string(),
        };
        if self.running_children > 0 && st != AgentState::Working {
            s.push_str(&format!(" (+{} running)", self.running_children));
        }
        s
    }
}

fn blocked_name(k: BlockedKind) -> &'static str {
    match k {
        BlockedKind::Permission => "permission",
        BlockedKind::Question => "question",
        BlockedKind::Auth => "auth",
        BlockedKind::Other => "other",
    }
}

/// Aggregate display state over several terminals (panel/tab/task badge).
pub fn aggregate_snapshots<'a, I: IntoIterator<Item = &'a Snapshot>>(snaps: I) -> AgentState {
    crate::types::aggregate(snaps.into_iter().map(Snapshot::display_state))
}

/// Status engine for one terminal.
#[derive(Debug, Clone)]
pub struct TerminalStatus {
    cfg: StatusConfig,
    parser: Osc7501Parser,
    heur: HeuristicTracker,
    hook: Option<Evidence>,
    osc: Option<Evidence>,
    /// Evidence from a Pod extension (`Source::Mod`), below OSC.
    modev: Option<Evidence>,
    /// Event order (not time: podd replays OSC before restored hooks after a
    /// restart): bumped on every root OSC state change and hook root set.
    order: u64,
    /// `order` of the last root OSC state change (re-confirmations do not count).
    osc_changed_at: u64,
    /// `order` of the last hook root set.
    hook_set_at: u64,
    hook_before_wait: Option<AgentState>,
    /// Set by a session-start hook: the next recompute leaving `working` is
    /// not the end of a turn. An agent starting up (Codex loading MCP
    /// servers) looks busy to the screen heuristic before any prompt ran;
    /// its SessionStart hook then says idle, which used to count a turn and
    /// made Pod report "ended its turn" seconds after a worker started.
    boundary_no_turn: bool,
    /// When the user answered a hook `blocked` prompt (see [`Self::on_input`]).
    answered_at: Option<Millis>,
    /// The hook slot holds `working` inferred from that answer, not a hook.
    hook_inferred: Option<Millis>,
    records: BTreeMap<String, SubRecord>,
    agent: Option<AgentKind>,
    agent_from_process: bool,
    app: Option<String>,
    exited: Option<AgentState>,
    current: AgentState,
    source: Option<Source>,
    confidence: f32,
    since: Millis,
    turn: u64,
    outcome: Option<Outcome>,
    session_id: Option<String>,
    tool_name: Option<String>,
    osc_title: Option<String>,
}

impl TerminalStatus {
    pub fn new(now: Millis) -> Self {
        Self::with_config(StatusConfig::default(), now)
    }

    pub fn with_config(cfg: StatusConfig, now: Millis) -> Self {
        Self {
            cfg,
            parser: Osc7501Parser::new(),
            heur: HeuristicTracker::new(),
            hook: None,
            osc: None,
            modev: None,
            order: 0,
            osc_changed_at: 0,
            hook_set_at: 0,
            hook_before_wait: None,
            boundary_no_turn: false,
            answered_at: None,
            hook_inferred: None,
            records: BTreeMap::new(),
            agent: None,
            agent_from_process: false,
            app: None,
            exited: None,
            current: AgentState::Unknown,
            source: None,
            confidence: 0.0,
            since: now,
            turn: 0,
            outcome: None,
            session_id: None,
            tool_name: None,
            osc_title: None,
        }
    }

    pub fn config(&self) -> &StatusConfig {
        &self.cfg
    }

    /// Current root state.
    pub fn state(&self) -> AgentState {
        self.current
    }

    pub fn turn(&self) -> u64 {
        self.turn
    }

    /// Continue counting turns from at least `turn` (state restored after a
    /// daemon restart).
    pub fn restore_turn(&mut self, turn: u64) {
        self.turn = self.turn.max(turn);
    }

    pub fn agent(&self) -> Option<&AgentKind> {
        self.agent.as_ref()
    }

    /// What the program said about itself in this run: the state of its
    /// newest root report (a hook, or an OSC 7501 change; event order, not
    /// time), `Unknown` when it took its report back (OSC `clear`, a hook
    /// session end) and nothing replaced it, `None` when it never reported.
    /// Unlike [`Self::state`] a quiet screen does not change it: the wake
    /// rule trusts it over heuristics (docs/19 §21.1, TM-22).
    pub fn self_report(&self) -> Option<AgentState> {
        let newest = match (&self.hook, &self.osc) {
            (Some(h), Some(o)) if self.hook_set_at >= self.osc_changed_at => Some(h.state),
            (_, Some(o)) => Some(o.state),
            (Some(h), None) => Some(h.state),
            (None, None) => None,
        };
        newest.or((self.order > 0).then_some(AgentState::Unknown))
    }

    // ─────────────────────────── output ───────────────────────────

    pub fn snapshot(&self) -> Snapshot {
        let records: Vec<SubRecord> = self.records.values().cloned().collect();
        let running_children = records
            .iter()
            .filter(|r| r.state == AgentState::Working)
            .count();
        let blocked_children = records.iter().filter(|r| r.state.is_blocked()).count();
        let chosen = self.chosen_evidence();
        let osc = self.osc.as_ref();
        Snapshot {
            state: self.current,
            final_state: if self.current == AgentState::Exited {
                self.exited
            } else {
                None
            },
            source: self.source,
            confidence: self.confidence,
            since: self.since,
            turn: self.turn,
            agent: self.agent.clone(),
            app: self.app.clone(),
            msg: chosen.and_then(|e| e.msg.clone()),
            title: osc
                .and_then(|e| e.title.clone())
                .or_else(|| self.osc_title.clone()),
            progress: chosen
                .and_then(|e| e.progress)
                .or_else(|| osc.and_then(|e| e.progress)),
            outcome: self.outcome,
            session_id: self.session_id.clone(),
            tool_name: self.tool_name.clone(),
            records,
            running_children,
            blocked_children,
        }
    }

    // ─────────────────────────── transition ───────────────────────────

    fn recompute(&mut self, now: Millis) -> Option<Transition> {
        let boundary = std::mem::take(&mut self.boundary_no_turn);
        let (state, mut source, conf) = self.resolve(now);
        if source == Some(Source::Hook) && self.hook_inferred.is_some() {
            source = Some(Source::Heuristic); // inferred, not reported by a hook
        }
        self.source = source;
        self.confidence = conf;
        if state == self.current {
            return None;
        }
        let from = self.current;
        if from == AgentState::Working && !boundary {
            self.turn += 1;
        }
        if state == AgentState::Working {
            self.outcome = None; // a new turn starts
        }
        self.current = state;
        self.since = now;
        Some(Transition {
            from,
            to: state,
            source,
            turn: self.turn,
            at: now,
        })
    }
}
