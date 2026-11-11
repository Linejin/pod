//! Inputs of the engine: PTY output and OSC reports, keys typed into an
//! agent's prompt, title/screen/foreground process, hook messages, Pod
//! extension states, ack and ticks. Each one updates its evidence slot and
//! then runs the transition step.

use super::evidence::{CONF_ANSWERED, CONF_HOOK, CONF_MOD, CONF_OSC, Evidence};
use super::{OutputResult, SubRecord, TerminalStatus, Transition};
use crate::heuristic::{self, ProcessInfo};
use crate::hook::{ChildEffect, HookMessage, RootEffect, map_message};
use crate::osc::{Osc7501Parser, OscEvent, Report, ReportKind, ReportState};
use crate::types::{AgentKind, AgentState, BlockedKind, Millis, Source};

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

impl TerminalStatus {
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
