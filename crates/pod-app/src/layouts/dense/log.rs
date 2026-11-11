//! The 출력 log's model: status changes, gates, exits and notifications in
//! arrival order, recorded by diffing the workspace data against the previous
//! observation. Pure: "now" is an argument, the data comes in [`Inputs`].
//!
//! The first observation seeds the log with what needs attention (gates,
//! blocked/failed agents, failed exits, notifications), at most [`SEED_CAP`]
//! lines; the log keeps the newest [`LOG_CAP`].
use std::collections::{BTreeMap, HashMap, VecDeque};

use pod_ui::Tone;
use podd::protocol::{GateInfo, GateState};

use super::copy;
use crate::agent_kind::AgentKind;
use crate::vm::{AgentState, Liveness, TaskVm, TerminalVm};
use crate::workspace::ToastKind;

/// Events kept in the log (oldest dropped first).
pub const LOG_CAP: usize = 500;
/// Seeded events at most (the first observation of a busy workspace).
pub const SEED_CAP: usize = 40;

/// What a log line records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// The log started (first observation).
    Start,
    /// An agent's reported status changed.
    Status,
    /// The terminal's process ended or its holder was lost.
    Exit,
    GateRequested,
    GateResolved,
    /// A notification (toast) the workspace raised.
    Notice,
}

/// One line of the 출력 log.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEvent {
    /// Increasing id (selection key).
    pub seq: u64,
    /// Unix ms.
    pub at: u64,
    pub kind: EventKind,
    pub tone: Tone,
    /// `hook`, `osc`, `gate`, `알림`, `pod` …
    pub source: String,
    /// Agent label, gate origin or notification title.
    pub who: String,
    pub text: String,
    pub terminal: Option<String>,
    pub task: Option<String>,
    /// Which agent the line is about (docs/10 §D5.11), for its mark.
    pub agent: Option<AgentKind>,
}

/// A toast as the log sees it.
#[derive(Clone, Debug)]
pub struct ToastIn {
    pub id: u64,
    pub kind: ToastKind,
    pub title: String,
    pub body: String,
}

/// The workspace data the log is built from.
pub struct Inputs<'a> {
    pub terminals: &'a [TerminalVm],
    /// For the agent of a terminal (`agent_kind::identify_in`).
    pub tasks: &'a [TaskVm],
    /// Every gate seen this session ([`crate::orch::OrchState::gates`]).
    pub gates: &'a BTreeMap<String, GateInfo>,
    pub toasts: Vec<ToastIn>,
    pub label: &'a dyn Fn(&TerminalVm) -> String,
}

#[derive(Clone, Debug, PartialEq)]
struct TermSnap {
    state: Option<AgentState>,
    kind: Option<String>,
    live: Liveness,
}

impl TermSnap {
    fn of(t: &TerminalVm) -> Self {
        Self {
            state: t.status.as_ref().map(|s| s.state),
            kind: t.status.as_ref().and_then(|s| s.kind.clone()),
            live: t.live,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Snapshot {
    terms: HashMap<String, TermSnap>,
    gates: HashMap<String, GateState>,
    /// First toast id not logged yet.
    next_toast: u64,
}

/// Status changes, gates, exits and notifications in arrival order.
#[derive(Default)]
pub struct EventLog {
    events: VecDeque<LogEvent>,
    snap: Option<Snapshot>,
    seq: u64,
    /// Agent of each terminal at the last observation.
    agents: HashMap<String, AgentKind>,
}

fn state_tone(s: AgentState) -> Tone {
    match s {
        AgentState::Blocked => Tone::Warning,
        AgentState::Error => Tone::Danger,
        AgentState::Done => Tone::Success,
        AgentState::Working => Tone::Info,
        _ => Tone::Muted,
    }
}

fn failed_exit(live: Liveness) -> bool {
    matches!(live, Liveness::Lost)
        || matches!(live, Liveness::Exited { code: Some(c), killed: false } if c != 0)
}

fn exit_text(t: &TerminalVm) -> String {
    if t.live == Liveness::Lost {
        copy::LOST.into()
    } else {
        crate::orch::terminal_badge(t).label()
    }
}

fn gate_who(g: &GateInfo, inp: &Inputs) -> String {
    g.terminal
        .as_deref()
        .and_then(|id| inp.terminals.iter().find(|t| t.id == id))
        .map(|t| (inp.label)(t))
        .unwrap_or_else(|| crate::orch::gate_origin(g))
}

fn gate_terminal(g: &GateInfo, inp: &Inputs) -> Option<String> {
    g.terminal
        .clone()
        .filter(|id| inp.terminals.iter().any(|t| &t.id == id))
}

fn resolved_text(s: GateState) -> (&'static str, Tone) {
    match s {
        GateState::Approved => (copy::APPROVED, Tone::Success),
        GateState::Rejected => (copy::REJECTED, Tone::Danger),
        GateState::Expired => (copy::EXPIRED, Tone::Muted),
        GateState::Pending => (copy::GATE_REQUESTED, Tone::Warning),
    }
}

impl EventLog {
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn events(&self) -> impl DoubleEndedIterator<Item = &LogEvent> + ExactSizeIterator {
        self.events.iter()
    }

    /// Newest first: the events of one terminal (or of every terminal of a task).
    pub fn history<'a>(
        &'a self,
        terminals: &'a [String],
    ) -> impl Iterator<Item = &'a LogEvent> + 'a {
        self.events
            .iter()
            .rev()
            .filter(move |e| e.terminal.as_ref().is_some_and(|t| terminals.contains(t)))
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        at: u64,
        kind: EventKind,
        tone: Tone,
        source: &str,
        who: String,
        text: String,
        terminal: Option<String>,
        task: Option<String>,
    ) {
        self.seq += 1;
        let agent = terminal
            .as_ref()
            .and_then(|id| self.agents.get(id).cloned());
        self.events.push_back(LogEvent {
            seq: self.seq,
            at,
            kind,
            tone,
            source: source.into(),
            who,
            text,
            terminal,
            task,
            agent,
        });
        while self.events.len() > LOG_CAP {
            self.events.pop_front();
        }
    }

    /// Record what changed since the previous call. The first call seeds the
    /// log with what needs attention now (gates, blocked/failed agents, failed
    /// exits, notifications), newest [`SEED_CAP`]. Returns whether lines were added.
    pub fn observe(&mut self, inp: &Inputs, now: u64) -> bool {
        // Before the first terminal list (launch, reconnect) there is nothing to seed from:
        // seeding then would log every terminal of the list as new.
        if self.snap.is_none() && inp.terminals.is_empty() && inp.gates.is_empty() {
            return false;
        }
        let before = self.seq;
        self.agents = inp
            .terminals
            .iter()
            .map(|t| {
                (
                    t.id.clone(),
                    crate::agent_kind::identify_in(t, inp.tasks).kind,
                )
            })
            .collect();
        let mut next = Snapshot {
            terms: inp
                .terminals
                .iter()
                .map(|t| (t.id.clone(), TermSnap::of(t)))
                .collect(),
            gates: inp
                .gates
                .iter()
                .map(|(id, g)| (id.clone(), g.state))
                .collect(),
            next_toast: inp.toasts.iter().map(|t| t.id + 1).max().unwrap_or(0),
        };
        match self.snap.take() {
            None => self.seed(inp, now),
            Some(prev) => {
                for t in inp.terminals {
                    let cur = &next.terms[&t.id];
                    let old = prev.terms.get(&t.id);
                    let who = (inp.label)(t);
                    let status_changed =
                        old.is_none_or(|o| o.state != cur.state || o.kind != cur.kind);
                    if status_changed
                        && t.live == Liveness::Running
                        && let Some(s) = &t.status
                    {
                        // A first report (new terminal, or its status arriving after the list) is news
                        // only when it needs the user; later changes always are.
                        let first = old.is_none_or(|o| o.state.is_none());
                        if !first || matches!(s.state, AgentState::Blocked | AgentState::Error) {
                            let source = s.source.clone().unwrap_or_else(|| "pod".into());
                            self.push(
                                now,
                                EventKind::Status,
                                state_tone(s.state),
                                &source,
                                who.clone(),
                                crate::orch::status_badge(Some(s)).label(),
                                Some(t.id.clone()),
                                t.task_id.clone(),
                            );
                        }
                    }
                    if old.is_some_and(|o| o.live != cur.live) && t.live != Liveness::Running {
                        let tone = if failed_exit(t.live) {
                            Tone::Danger
                        } else {
                            Tone::Muted
                        };
                        self.push(
                            now,
                            EventKind::Exit,
                            tone,
                            "pod",
                            who,
                            exit_text(t),
                            Some(t.id.clone()),
                            t.task_id.clone(),
                        );
                    }
                }
                for (id, g) in inp.gates {
                    match prev.gates.get(id) {
                        None => {
                            if g.state == GateState::Pending {
                                self.push(
                                    now,
                                    EventKind::GateRequested,
                                    Tone::Warning,
                                    copy::GATE,
                                    gate_who(g, inp),
                                    format!(
                                        "{} · {}: {}",
                                        copy::GATE_REQUESTED,
                                        g.title,
                                        g.message
                                    ),
                                    gate_terminal(g, inp),
                                    g.task.clone(),
                                );
                            } else {
                                let (text, tone) = resolved_text(g.state);
                                self.push(
                                    now,
                                    EventKind::GateResolved,
                                    tone,
                                    copy::GATE,
                                    gate_who(g, inp),
                                    format!("{text} · {}", g.title),
                                    gate_terminal(g, inp),
                                    g.task.clone(),
                                );
                            }
                        }
                        Some(old) if *old != g.state && g.state != GateState::Pending => {
                            let (text, tone) = resolved_text(g.state);
                            self.push(
                                now,
                                EventKind::GateResolved,
                                tone,
                                copy::GATE,
                                gate_who(g, inp),
                                format!("{text} · {}", g.title),
                                gate_terminal(g, inp),
                                g.task.clone(),
                            );
                        }
                        _ => {}
                    }
                }
                for t in inp.toasts.iter().filter(|t| t.id >= prev.next_toast) {
                    self.push_notice(t, now);
                }
                next.next_toast = next.next_toast.max(prev.next_toast);
            }
        }
        self.snap = Some(next);
        self.seq != before
    }

    fn push_notice(&mut self, t: &ToastIn, now: u64) {
        let tone = match t.kind {
            ToastKind::Error => Tone::Danger,
            ToastKind::Warning => Tone::Warning,
            ToastKind::Success => Tone::Success,
            ToastKind::Info => Tone::Secondary,
        };
        self.push(
            now,
            EventKind::Notice,
            tone,
            copy::NOTICE,
            t.title.clone(),
            t.body.clone(),
            None,
            None,
        );
    }

    fn seed(&mut self, inp: &Inputs, now: u64) {
        let when = |ms: u64| if ms == 0 || ms > now { now } else { ms };
        struct Seed {
            at: u64,
            kind: EventKind,
            tone: Tone,
            source: String,
            who: String,
            text: String,
            terminal: Option<String>,
            task: Option<String>,
        }
        let mut seeds = Vec::new();
        for t in inp.terminals {
            let who = (inp.label)(t);
            if t.live != Liveness::Running {
                if failed_exit(t.live) {
                    seeds.push(Seed {
                        at: now,
                        kind: EventKind::Exit,
                        tone: Tone::Danger,
                        source: "pod".into(),
                        who,
                        text: exit_text(t),
                        terminal: Some(t.id.clone()),
                        task: t.task_id.clone(),
                    });
                }
                continue;
            }
            let Some(s) = &t.status else { continue };
            if matches!(s.state, AgentState::Blocked | AgentState::Error) {
                seeds.push(Seed {
                    at: when(s.since),
                    kind: EventKind::Status,
                    tone: state_tone(s.state),
                    source: s.source.clone().unwrap_or_else(|| "pod".into()),
                    who,
                    text: crate::orch::status_badge(Some(s)).label(),
                    terminal: Some(t.id.clone()),
                    task: t.task_id.clone(),
                });
            }
        }
        for g in inp.gates.values() {
            seeds.push(Seed {
                at: when(g.created_ms),
                kind: EventKind::GateRequested,
                tone: Tone::Warning,
                source: copy::GATE.into(),
                who: gate_who(g, inp),
                text: format!("{} · {}: {}", copy::GATE_REQUESTED, g.title, g.message),
                terminal: gate_terminal(g, inp),
                task: g.task.clone(),
            });
            if g.state != GateState::Pending {
                let (text, tone) = resolved_text(g.state);
                seeds.push(Seed {
                    at: when(g.resolved_ms.unwrap_or(0)),
                    kind: EventKind::GateResolved,
                    tone,
                    source: copy::GATE.into(),
                    who: gate_who(g, inp),
                    text: format!("{text} · {}", g.title),
                    terminal: gate_terminal(g, inp),
                    task: g.task.clone(),
                });
            }
        }
        // Stable: equal times keep the order above (agents, then gates).
        seeds.sort_by_key(|s| s.at);
        let skip = seeds.len().saturating_sub(SEED_CAP);
        let running = inp.terminals.iter().filter(|t| t.is_running()).count();
        self.push(
            now,
            EventKind::Start,
            Tone::Muted,
            copy::POD,
            copy::STARTED.into(),
            copy::started(running),
            None,
            None,
        );
        for s in seeds.into_iter().skip(skip) {
            self.push(
                s.at, s.kind, s.tone, &s.source, s.who, s.text, s.terminal, s.task,
            );
        }
        for t in &inp.toasts {
            self.push_notice(t, now);
        }
    }
}

/// `HH:MM:SS` of unix `ms` at `offset_min` minutes from UTC.
pub fn clock(ms: u64, offset_min: i64) -> String {
    let secs = (ms / 1000) as i64 + offset_min * 60;
    let day = secs.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, day / 60 % 60, day % 60)
}

/// Minutes from UTC to local time, from the (day of month, hour, minute) of
/// both clocks read at the same instant. Offsets are under a day, so a day
/// difference means midnight lies between them (also across a month end).
pub fn offset_between(local: (u16, u16, u16), utc: (u16, u16, u16)) -> i64 {
    let mins = |(_, h, m): (u16, u16, u16)| h as i64 * 60 + m as i64;
    let mut d = mins(local) - mins(utc);
    if local.0 != utc.0 {
        // Local is the next day when its day number is larger, or when it is the 1st and UTC is at a month end.
        let local_ahead =
            (local.0 > utc.0 && !(utc.0 == 1 && local.0 >= 28)) || (local.0 == 1 && utc.0 >= 28);
        d += if local_ahead { 1440 } else { -1440 };
    }
    // Offsets are whole quarter hours.
    ((d as f64 / 15.).round() * 15.) as i64
}
