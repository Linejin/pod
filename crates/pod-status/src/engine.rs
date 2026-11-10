//! Per-terminal status engine: merges hook / OSC / heuristic / process evidence
//! into one [`AgentState`] (docs/04 §4.1–§4.2).
//!
//! The engine is pure and synchronous: podd calls the `on_*` methods with the
//! terminal's output bytes, title changes, screen text, foreground process,
//! hook messages and the current monotonic time, and reads [`Snapshot`]s.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::heuristic::{self, HeuristicTracker, ProcessInfo};
use crate::hook::{ChildEffect, HookMessage, RootEffect, map_message};
use crate::osc::{Osc7501Parser, OscEvent, Report, ReportKind, ReportState};
use crate::types::{AgentKind, AgentState, BlockedKind, Millis, Outcome, Source};

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

pub const CONF_HOOK: f32 = 0.95;
pub const CONF_OSC: f32 = 0.9;
pub const CONF_PROCESS: f32 = 0.2;
/// A Pod extension's status detector or hint (docs/16 §7.1).
pub const CONF_MOD: f32 = 0.75;
/// `working` inferred from the user answering a hook permission/question
/// prompt (no hook reports the answer; docs/12 B2). Reported as `heuristic`.
pub const CONF_ANSWERED: f32 = 0.6;
/// An answer only counts when the program draws something within this long.
pub const ANSWER_OUTPUT_WINDOW_MS: Millis = 3_000;

/// Keys that answer an agent's permission/question prompt: Enter, or a
/// single option key (Codex `y`/`a`/`n`/`p`, Claude Code `1`-`9`). Escape
/// sequences (arrows, focus reports, terminal query replies) are not
/// answers; neither is a bare Esc (cancel: the agent reports that itself),
/// nor longer typed text.
pub fn is_answer_input(bytes: &[u8]) -> bool {
    let mut rest: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            match bytes.get(i + 1) {
                // CSI: parameters/intermediates, then a final byte.
                Some(b'[') => {
                    i += 2;
                    while i < bytes.len() && !(0x40..=0x7e).contains(&bytes[i]) {
                        i += 1;
                    }
                    i += 1;
                }
                // OSC / DCS / APC: up to BEL or ST.
                Some(b']' | b'P' | b'_') => {
                    i += 2;
                    while i < bytes.len() && bytes[i] != 0x07 && bytes[i] != 0x1b {
                        i += 1;
                    }
                    if bytes.get(i) == Some(&0x1b) {
                        i += 1; // the `\` of ST
                    }
                    i += 1;
                }
                // SS3 (`ESC O x`) or Alt+key.
                Some(b'O') => i += 3,
                Some(_) => i += 2,
                // A bare Esc.
                None => i += 1,
            }
            continue;
        }
        if b == b'\r' || (0x20..0x7f).contains(&b) {
            rest.push(b);
        }
        i += 1;
    }
    match rest.as_slice() {
        [] => false,
        [k] => *k == b'\r' || b"yYaAnNpP123456789".contains(k),
        r => r.len() <= 2 && r.ends_with(b"\r"),
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Evidence {
    state: AgentState,
    /// Last time this source confirmed or changed its state.
    at: Millis,
    confidence: f32,
    msg: Option<String>,
    progress: Option<u8>,
    title: Option<String>,
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

    /// Tell the engine which agent runs here (e.g. podd launched it itself).
    pub fn set_agent(&mut self, agent: AgentKind, now: Millis) -> Option<Transition> {
        self.agent = Some(agent);
        self.heur.on_agent(self.agent.as_ref(), now);
        self.recompute(now)
    }

    // ─────────────────────────── inputs ───────────────────────────

    /// Raw PTY output bytes, in arrival order and arbitrary chunking.
    pub fn feed_output(&mut self, bytes: &[u8], now: Millis) -> OutputResult {
        let mut events = Vec::new();
        self.parser.feed_with(bytes, |e| events.push(e));
        self.heur.on_output(now);
        let mut res = OutputResult::default();
        for ev in events {
            match ev {
                OscEvent::Query => res.queries += 1,
                OscEvent::Invalid(_) => res.invalid += 1,
                OscEvent::Report(r) => {
                    if self.exited.is_some() {
                        self.reset_run(now); // a new program reports in this terminal
                    }
                    res.changed |= self.apply_report(r, now);
                }
            }
        }
        self.output_after_answer(now);
        res.transition = self.recompute(now);
        res.changed |= res.transition.is_some();
        res
    }

    /// Input the user typed into the terminal (keys, attach input, `pod
    /// send`). While a *hook* says `blocked:permission`/`blocked:question`,
    /// an answer-like key ([`is_answer_input`]) followed by output means the
    /// prompt was answered and the agent went on: no hook reports an
    /// approval (Codex and Claude Code send PostToolUse only when the tool
    /// finishes), so the state would otherwise stay blocked while the
    /// approved command runs (docs/12 B2). The engine then shows `working`
    /// (source `heuristic`) until the next hook. Esc is not an answer: the
    /// agent reports a cancel itself (Interrupt/Stop hook or OSC).
    pub fn on_input(&mut self, bytes: &[u8], now: Millis) {
        let hook_blocked = self.hook.as_ref().is_some_and(|h| {
            matches!(
                h.state,
                AgentState::Blocked(BlockedKind::Permission | BlockedKind::Question)
            )
        });
        if self.exited.is_none()
            && hook_blocked
            && self.current.is_blocked()
            && self.source == Some(Source::Hook)
            && is_answer_input(bytes)
        {
            self.answered_at = Some(now);
        }
    }

    /// When the current `working` was inferred from an answered prompt.
    pub fn answer_inferred_at(&self) -> Option<Millis> {
        self.hook_inferred
    }

    /// Restore an inferred answer after a daemon restart (the persisted hook
    /// message is still the blocked one): applies only if that hook was
    /// accepted before `answered_at`.
    pub fn restore_answer(&mut self, answered_at: Millis, now: Millis) -> Option<Transition> {
        if self.hook.as_ref().is_some_and(|h| h.at <= answered_at) {
            self.answered_at = Some(answered_at);
            self.infer_answered(answered_at);
        }
        self.recompute(now)
    }

    fn output_after_answer(&mut self, now: Millis) {
        let Some(at) = self.answered_at else { return };
        if now.saturating_sub(at) > ANSWER_OUTPUT_WINDOW_MS {
            self.answered_at = None; // nothing happened: not an answer
            return;
        }
        let still_blocked = self.hook.as_ref().is_some_and(|h| h.state.is_blocked());
        if still_blocked {
            self.infer_answered(now);
        } else {
            self.answered_at = None;
        }
    }

    fn infer_answered(&mut self, now: Millis) {
        self.answered_at = None;
        let st = self.hook_before_wait.take().unwrap_or(AgentState::Working);
        let st = if st.is_blocked() {
            AgentState::Working
        } else {
            st
        };
        self.order += 1;
        self.hook_set_at = self.order;
        self.hook = Some(Evidence {
            state: st,
            at: now,
            confidence: CONF_ANSWERED,
            msg: None,
            progress: None,
            title: None,
        });
        self.hook_inferred = Some(now);
    }

    /// Apply an already-parsed OSC 7501 report (e.g. replayed by podd).
    pub fn on_report(&mut self, r: Report, now: Millis) -> Option<Transition> {
        self.apply_report(r, now);
        self.recompute(now)
    }

    /// The terminal title changed (OSC 0/2 as decoded by the VT layer).
    pub fn on_title(&mut self, title: &str, now: Millis) -> Option<Transition> {
        if self.agent.is_none()
            && let Some(a) = heuristic::agent_from_title(title)
        {
            self.agent = Some(a);
        }
        self.heur.on_title(self.agent.as_ref(), title, now);
        self.recompute(now)
    }

    /// A Pod extension reported a state for this terminal (`None` clears
    /// its evidence). Ranked below hook and OSC, above the heuristics; stays
    /// until replaced, cleared or the process exits.
    pub fn on_mod(
        &mut self,
        state: Option<AgentState>,
        msg: Option<String>,
        now: Millis,
    ) -> Option<Transition> {
        match state {
            None | Some(AgentState::Unknown) | Some(AgentState::Exited) => self.modev = None,
            Some(st) => {
                self.modev = Some(Evidence {
                    state: st,
                    at: now,
                    confidence: CONF_MOD,
                    msg,
                    progress: None,
                    title: None,
                });
            }
        }
        self.recompute(now)
    }

    /// The visible screen text changed (podd may throttle this, e.g. last N rows).
    pub fn on_screen(&mut self, screen: &str, now: Millis) -> Option<Transition> {
        self.heur.on_screen(self.agent.as_ref(), screen, now);
        self.recompute(now)
    }

    /// Foreground process changed. `None` or a non-agent process (shell) after
    /// an agent was seen means the agent exited.
    pub fn on_foreground(
        &mut self,
        proc_info: Option<&ProcessInfo>,
        now: Millis,
    ) -> Option<Transition> {
        match proc_info.and_then(heuristic::detect_agent) {
            Some(kind) => {
                let new_run = self.exited.is_some() || !self.agent_from_process;
                if self.exited.is_some() {
                    self.reset_run(now);
                }
                if new_run || self.agent.as_ref() != Some(&kind) {
                    self.agent = Some(kind);
                    self.agent_from_process = true;
                    self.heur.on_agent(self.agent.as_ref(), now);
                }
                self.recompute(now)
            }
            None => {
                if self.agent_from_process && self.exited.is_none() {
                    self.on_process_exit(now)
                } else {
                    None
                }
            }
        }
    }

    /// The agent process (or the PTY child) exited. The last state is kept as
    /// `final_state`; OSC/hook/heuristic evidence is dropped.
    pub fn on_process_exit(&mut self, now: Millis) -> Option<Transition> {
        if self.exited.is_some() {
            return None;
        }
        let last = self.current;
        self.exited = Some(last);
        self.agent_from_process = false;
        self.osc = None;
        self.hook = None;
        self.modev = None;
        self.hook_before_wait = None;
        self.answered_at = None;
        self.hook_inferred = None;
        self.heur.reset();
        self.parser = Osc7501Parser::new();
        self.recompute(now)
    }

    /// Forget everything about the previous run (keeps `turn` and the OSC parser state).
    pub fn reset_run(&mut self, now: Millis) {
        let turn = self.turn;
        let cfg = self.cfg;
        let parser = std::mem::take(&mut self.parser);
        *self = Self::with_config(cfg, now);
        self.turn = turn;
        self.parser = parser; // keep a partially received sequence
    }

    /// A hook message for this terminal (already routed by `terminal_id`).
    pub fn on_hook(&mut self, msg: &HookMessage, now: Millis) -> Option<Transition> {
        let eff = map_message(msg);
        if self.exited.is_some() && eff.session_boundary {
            self.reset_run(now); // the agent was restarted
        }
        if self.agent.is_none() {
            self.agent = Some(match msg.agent {
                crate::hook::HookAgent::Claude => AgentKind::Claude,
                crate::hook::HookAgent::Codex => AgentKind::Codex,
            });
        }
        if eff.session_id.is_some() {
            self.session_id = eff.session_id.clone();
        }
        if eff.session_boundary {
            self.records.retain(|_, r| r.source != Source::Hook);
            self.hook_before_wait = None;
            self.boundary_no_turn = true;
        }
        if !matches!(eff.root, RootEffect::None) {
            // A real hook decides again.
            self.answered_at = None;
            self.hook_inferred = None;
        }
        if let Some(c) = eff.child {
            match c {
                ChildEffect::Set { id, state, title } => {
                    let rec = self.records.entry(id.clone()).or_insert(SubRecord {
                        id,
                        state,
                        source: Source::Hook,
                        title: None,
                        msg: None,
                        progress: None,
                        since: now,
                    });
                    if rec.state != state {
                        rec.state = state;
                        rec.since = now;
                    }
                    if title.is_some() {
                        rec.title = title;
                    }
                }
                ChildEffect::Remove { id } => {
                    self.records.remove(&id);
                }
            }
        }
        match eff.root {
            RootEffect::None => {
                if let Some(h) = self.hook.as_mut() {
                    h.at = now; // any hook traffic keeps hook evidence fresh
                }
            }
            RootEffect::Set { state, msg } => {
                let prev = self.hook.as_ref().map(|h| h.state);
                if state.is_blocked() && !prev.is_some_and(AgentState::is_blocked) {
                    self.hook_before_wait = Some(prev.unwrap_or(AgentState::Working));
                }
                if !state.is_blocked() {
                    self.hook_before_wait = None;
                }
                if let Some(t) = &eff.tool_name {
                    self.tool_name = Some(t.clone());
                }
                if eff.outcome.is_some() {
                    self.outcome = eff.outcome;
                } else if state == AgentState::Working {
                    self.outcome = None;
                }
                self.order += 1;
                self.hook_set_at = self.order;
                self.hook = Some(Evidence {
                    state,
                    at: now,
                    confidence: CONF_HOOK,
                    msg,
                    progress: None,
                    title: None,
                });
            }
            RootEffect::RestoreFromWait => {
                let st = self.hook_before_wait.take().unwrap_or(AgentState::Working);
                self.order += 1;
                self.hook_set_at = self.order;
                self.hook = Some(Evidence {
                    state: st,
                    at: now,
                    confidence: CONF_HOOK,
                    msg: None,
                    progress: None,
                    title: None,
                });
            }
            RootEffect::EndSession => {
                self.hook = None;
                self.hook_before_wait = None;
                self.records.retain(|_, r| r.source != Source::Hook);
            }
        }
        self.recompute(now)
    }

    /// The user looked at the pane: `done` becomes `idle` (acknowledged).
    pub fn ack(&mut self, now: Millis) -> Option<Transition> {
        if self.current != AgentState::Done {
            return None;
        }
        for ev in [self.hook.as_mut(), self.osc.as_mut(), self.modev.as_mut()]
            .into_iter()
            .flatten()
        {
            if ev.state == AgentState::Done {
                ev.state = AgentState::Idle;
            }
        }
        self.heur.ack();
        self.recompute(now)
    }

    /// Advance time: quiet-time idle, hook staleness.
    pub fn tick(&mut self, now: Millis) -> Option<Transition> {
        self.heur.tick(now, self.cfg.quiet_idle_ms);
        self.recompute(now)
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

    // ─────────────────────────── internals ───────────────────────────

    fn chosen_evidence(&self) -> Option<&Evidence> {
        match self.source {
            Some(Source::Hook) => self.hook.as_ref(),
            Some(Source::Osc) => self.osc.as_ref(),
            Some(Source::Mod) => self.modev.as_ref(),
            _ => None,
        }
    }

    fn apply_report(&mut self, r: Report, now: Millis) -> bool {
        if let Some(app) = &r.app {
            self.app = Some(app.clone());
            if self.agent.is_none() || matches!(self.agent, Some(AgentKind::Other(_))) {
                self.agent = Some(AgentKind::from_osc_app(app));
            }
        }
        let state = report_state(&r);
        match r.id {
            Some(id) => {
                match state {
                    None => {
                        self.records.remove(&id);
                    }
                    Some(st) => {
                        let rec = self.records.entry(id.clone()).or_insert(SubRecord {
                            id,
                            state: st,
                            source: Source::Osc,
                            title: None,
                            msg: None,
                            progress: None,
                            since: now,
                        });
                        if rec.state != st {
                            rec.since = now;
                        }
                        rec.state = st;
                        rec.source = Source::Osc;
                        if r.title.is_some() {
                            rec.title = r.title;
                        }
                        rec.msg = r.msg;
                        rec.progress = r.progress;
                    }
                }
                true
            }
            None => {
                match state {
                    None => {
                        // Root clear: demote to lower sources.
                        self.osc = None;
                    }
                    Some(st) => {
                        if r.title.is_some() {
                            self.osc_title = r.title.clone();
                        }
                        if self.osc.as_ref().map(|o| o.state) != Some(st) {
                            self.order += 1;
                            self.osc_changed_at = self.order;
                        }
                        self.osc = Some(Evidence {
                            state: st,
                            at: now,
                            confidence: CONF_OSC,
                            msg: r.msg,
                            progress: r.progress,
                            title: r.title,
                        });
                    }
                }
                true
            }
        }
    }

    /// How long hook evidence beats lower sources. Normally `hook_stale_ms`,
    /// but a hook `blocked` (permission/question) is over at once when the
    /// agent's own OSC report later *changes* to the end of a turn
    /// (idle/done/error): the user answered with Esc/interrupt, which sends no
    /// hook (docs/07 §7). `hook_stale_ms` stays the fallback for agents
    /// without OSC reports. A later OSC `working` does not clear it (the
    /// prompt may still be up while the report lags).
    fn hook_stale_after(&self) -> Millis {
        let superseded = match (&self.hook, &self.osc) {
            (Some(h), Some(o)) => {
                h.state.is_blocked()
                    && self.osc_changed_at > self.hook_set_at
                    && matches!(
                        o.state,
                        AgentState::Idle | AgentState::Done | AgentState::Error
                    )
            }
            _ => false,
        };
        if superseded {
            0
        } else {
            self.cfg.hook_stale_ms
        }
    }

    /// Choose the effective state from the evidence slots.
    fn resolve(&self, now: Millis) -> (AgentState, Option<Source>, f32) {
        if self.exited.is_some() {
            return (AgentState::Exited, Some(Source::Process), 1.0);
        }
        let heur = self.heur.verdict().map(|v| Evidence {
            state: v.state,
            at: v.at,
            confidence: v.confidence,
            msg: v.msg,
            progress: None,
            title: None,
        });
        let slots: [(Source, Option<&Evidence>, Millis); 4] = [
            (Source::Hook, self.hook.as_ref(), self.hook_stale_after()),
            (Source::Osc, self.osc.as_ref(), Millis::MAX), // fresh until process exit
            (Source::Mod, self.modev.as_ref(), Millis::MAX), // until cleared or exit
            (Source::Heuristic, heur.as_ref(), Millis::MAX),
        ];
        let mut stale_best: Option<(Source, &Evidence)> = None;
        let mut chosen: Option<(Source, &Evidence, f32)> = None;
        for (src, ev, stale_after) in slots {
            let Some(ev) = ev else { continue };
            if let Some((_, sb)) = stale_best {
                // A stale higher source only yields to newer lower evidence.
                if ev.at <= sb.at {
                    continue;
                }
            }
            if now.saturating_sub(ev.at) < stale_after {
                chosen = Some((src, ev, ev.confidence));
                break;
            }
            if stale_best.is_none() {
                stale_best = Some((src, ev));
            }
        }
        let (src, ev, conf) = match (chosen, stale_best) {
            (Some(c), _) => c,
            (None, Some((s, e))) => (s, e, e.confidence * 0.5),
            (None, None) => {
                let src = self.agent_from_process.then_some(Source::Process);
                let conf = if src.is_some() { CONF_PROCESS } else { 0.0 };
                return (AgentState::Unknown, src, conf);
            }
        };
        // A lower source saying "idle" must not silently acknowledge a "done"
        // from a stale higher source: done → idle only via ack().
        if ev.state == AgentState::Idle
            && let Some((s, e)) = stale_best
            && s > src
            && e.state == AgentState::Done
        {
            return (AgentState::Done, Some(s), e.confidence * 0.5);
        }
        (ev.state, Some(src), conf)
    }

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

fn report_state(r: &Report) -> Option<AgentState> {
    Some(match r.state() {
        ReportState::Idle => AgentState::Idle,
        ReportState::Working => AgentState::Working,
        ReportState::Done => AgentState::Done,
        ReportState::Error => AgentState::Error,
        ReportState::Blocked => AgentState::Blocked(match r.kind {
            Some(ReportKind::Permission) => BlockedKind::Permission,
            Some(ReportKind::Question) => BlockedKind::Question,
            Some(ReportKind::Auth) => BlockedKind::Auth,
            None => BlockedKind::Other,
        }),
        ReportState::Clear => return None,
    })
}
