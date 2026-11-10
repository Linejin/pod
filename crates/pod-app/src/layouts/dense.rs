//! 고밀도 + 세부 열 layout (preset 07 "고밀도 편집기", docs/10 §D5.10).
//!
//! Everything for supervising many agents on one screen:
//! activity bar · the workspace's own task tree · the workspace's own dock,
//! with an **출력** panel under it (status changes, gates, exits and
//! notifications as a compact log) · a **세부 정보** column on the right for
//! the selected task or agent (branch, worktree, status history, last lines,
//! PR, actions).
//!
//! - **No forked state.** Tree, dock, terminals, tasks, gates and toasts are
//!   the workspace's. The layout keeps view state only ([`DenseView`]: panel
//!   visibility, selection, the event log it recorded, a last-lines cache),
//!   one per workspace in a global, so it survives layout switches.
//! - **The log is recorded, not polled.** [`EventLog::observe`] diffs the
//!   workspace's data against the previous snapshot on every workspace change
//!   notification; the first observation seeds it with what needs attention.
//! - **Switching is live.** The dock entity is the one every layout renders:
//!   no panel is created, dropped or re-attached by switching.
//! - **Density from the style.** Rows, header heights, sidebar width, UI font
//!   and radii come from `pod_ui::themes::metrics()`; row heights grow with
//!   the user's font size so Korean text is never clipped.
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::rc::Rc;

use gpui_kit::component::dock::PanelId;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{ControlState, Icon, Tone, semantic_button};
use podd::protocol::{self as p, GateDecision, GateInfo, GateState};

use crate::agent_kind::AgentKind;
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::qa_marks::QaId;
use crate::source::RunSpec;
use crate::vm::{AgentState, Liveness, TaskVm, TerminalVm};
use crate::workspace::{ToastKind, Workspace};

actions!(
    pod_dense,
    [NextRegion, PrevRegion, FocusOutput, FocusDetails]
);

/// Key context of the layout body (its shortcuts work only here).
pub const KEY_CONTEXT: &str = "PodDense";
/// Focus the 출력 log (shows the panel when hidden).
pub const FOCUS_OUTPUT_KEY: &str = "ctrl-shift-u";
/// Focus the 세부 정보 column (shows it when hidden).
pub const FOCUS_DETAILS_KEY: &str = "ctrl-shift-d";
/// `--ui-fixture --fixture-scene dense`: gate on fixture-1, four panels, fixture-1 selected.
pub const FIXTURE_SCENE: &str = "dense";
/// Events kept in the log (oldest dropped first).
pub const LOG_CAP: usize = 500;
/// Seeded events at most (the first observation of a busy workspace).
pub const SEED_CAP: usize = 40;
/// Visible rows of the 출력 panel.
pub const LOG_ROWS: usize = 6;
/// Lines read for 마지막 줄.
pub const LAST_LINES: usize = 3;
/// Entries of 상태 기록.
pub const HISTORY_ROWS: usize = 6;

/// Product copy of this layout (kept here so layout modules merge independently).
pub mod copy {
    pub const ACTIVITY: &str = "활동 막대";
    pub const EXPLORER: &str = "작업 탐색 보이기/숨기기";
    pub const FILES: &str = "파일";
    pub const AGENTS: &str = "에이전트 트리";
    pub const SEARCH: &str = "검색";
    pub const GATES: &str = "승인 요청";
    pub const OUTPUT_TOGGLE: &str = "출력 패널 보이기/숨기기";
    pub const DETAILS_TOGGLE: &str = "세부 정보 보이기/숨기기";
    pub const SETTINGS: &str = "설정";
    pub const OUTPUT: &str = "출력";
    pub const LOG: &str = "출력 · 이벤트 기록";
    pub const CLEAR: &str = "출력 지우기";
    pub const CLOSE_OUTPUT: &str = "출력 패널 닫기";
    pub const EVENTS: &str = "이벤트";
    pub const LOG_EMPTY: &str = "아직 기록된 이벤트가 없습니다";
    pub const DETAILS: &str = "세부 정보";
    pub const CLOSE_DETAILS: &str = "세부 정보 닫기";
    pub const NOTHING: &str = "작업이나 에이전트를 고르면 여기에 세부 정보가 보입니다";
    pub const NAME: &str = "이름";
    pub const AGENT: &str = "에이전트";
    pub const STATE: &str = "상태";
    pub const TASK: &str = "작업";
    pub const PROGRAM: &str = "프로그램";
    pub const COMMAND: &str = "명령";
    pub const REPO: &str = "저장소";
    pub const BRANCH: &str = "브랜치";
    pub const WORKTREE: &str = "작업 폴더";
    pub const ELAPSED: &str = "경과";
    pub const ORIGIN: &str = "출처";
    pub const GIT: &str = "변경";
    pub const COMMENTS: &str = "검토 의견";
    pub const PR: &str = "PR";
    pub const NO_PR: &str = "없음";
    pub const ACTIONS: &str = "동작";
    pub const ALLOW_ONCE: &str = "이번만 허용";
    pub const REJECT: &str = "거부";
    pub const CHOOSE: &str = "선택";
    pub const OPEN_TERMINAL: &str = "터미널 열기";
    pub const REVIEW: &str = "변경 검토";
    pub const OPEN_FILES: &str = "파일 열기";
    pub const OPEN_PR: &str = "PR 보기";
    pub const RERUN: &str = "다시 실행";
    pub const LAST_LINES: &str = "마지막 줄";
    pub const NO_LINES: &str = "출력 없음";
    pub const HISTORY: &str = "상태 기록";
    pub const NO_HISTORY: &str = "이 레이아웃을 연 뒤로 바뀐 것이 없습니다";
    pub const AGENTS_OF_TASK: &str = "에이전트";
    pub const STARTED: &str = "기록 시작";
    pub const GATE_REQUESTED: &str = "승인 요청";
    pub const APPROVED: &str = "승인됨";
    pub const REJECTED: &str = "거부됨";
    pub const EXPIRED: &str = "만료됨";
    pub const LOST: &str = "연결 끊김";
    pub const NOTICE: &str = "알림";
    pub const POD: &str = "pod";
    pub const GATE: &str = "gate";
    pub const COULD_NOT_RERUN: &str = "다시 실행하지 못했습니다";
}

// ───────────────────────────── event log model ─────────────────────────────

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
            format!("터미널 {running}개 · 기존 상태에서 확인이 필요한 것만"),
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

/// Local time zone offset in minutes (read once; DST changes during a session are ignored).
fn local_offset_min() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::SYSTEMTIME;
            use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
            // SAFETY: both calls only fill the SYSTEMTIME passed in.
            let (mut l, mut u): (SYSTEMTIME, SYSTEMTIME) =
                unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
            unsafe {
                GetLocalTime(&mut l);
                GetSystemTime(&mut u);
            }
            offset_between((l.wDay, l.wHour, l.wMinute), (u.wDay, u.wHour, u.wMinute))
        }
        #[cfg(not(windows))]
        {
            0
        }
    })
}

/// What the 세부 정보 column shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Sel {
    Task(String),
    Terminal(String),
}

impl Sel {
    /// From a sidebar row key (`task:<id>` / `terminal:<id>`); repository rows select nothing.
    pub fn from_sidebar_key(k: &str) -> Option<Self> {
        if let Some(id) = k.strip_prefix("task:") {
            Some(Self::Task(id.into()))
        } else {
            k.strip_prefix("terminal:")
                .map(|id| Self::Terminal(id.into()))
        }
    }
}

/// Status key of a terminal: 마지막 줄 is re-read only when it changes.
fn line_key(t: &TerminalVm) -> String {
    let s = t.status.as_ref();
    format!(
        "{:?}|{}|{}|{:?}",
        s.map(|s| s.state),
        s.map_or(0, |s| s.since),
        s.map_or(0, |s| s.turn),
        t.live
    )
}

/// A pending `terminal.read` for 마지막 줄.
struct LineRead {
    id: String,
    key: String,
    source: crate::source::PoddSource,
}

/// The regions F6 cycles through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Tree,
    Dock,
    Output,
    Details,
}

// ───────────────────────────── view state ─────────────────────────────

/// View state of the layout (one per workspace, kept across layout switches).
pub struct DenseView {
    ws: WeakEntity<Workspace>,
    pub(crate) log: EventLog,
    pub(crate) selected: Option<Sel>,
    seen_sidebar: Option<String>,
    seen_focus: Option<String>,
    pub(crate) log_focus: FocusHandle,
    pub(crate) details_focus: FocusHandle,
    /// Selected log line (`LogEvent::seq`).
    pub(crate) log_sel: Option<u64>,
    log_scroll: UniformListScrollHandle,
    /// Lines before this `seq` are hidden (출력 지우기).
    cleared: u64,
    pub(crate) show_tree: bool,
    pub(crate) show_output: bool,
    pub(crate) show_details: bool,
    lines: HashMap<String, (String, Vec<String>)>,
    reading: HashSet<String>,
    busy: HashMap<String, GateDecision>,
    errors: HashMap<String, String>,
    _subs: Vec<Subscription>,
}

#[derive(Default)]
struct DenseViews(HashMap<EntityId, Entity<DenseView>>);
impl Global for DenseViews {}

/// The workspace's view state, created on first use.
pub(crate) fn view(ws: &Workspace, cx: &mut Context<Workspace>) -> Entity<DenseView> {
    let id = cx.entity_id();
    if let Some(v) = cx
        .try_global::<DenseViews>()
        .and_then(|m| m.0.get(&id))
        .cloned()
    {
        return v;
    }
    let me = cx.entity().downgrade();
    let sidebar = ws.sidebar.clone();
    let v = cx.new(|cx| DenseView::new(me, &sidebar, cx));
    // The body is drawn by the workspace: redraw it when the view changes.
    let sub = cx.observe(&v, |_, _, cx| cx.notify());
    v.update(cx, |v, _| v._subs.push(sub));
    cx.default_global::<DenseViews>().0.insert(id, v.clone());
    v
}

/// The view if the layout was ever shown.
#[cfg(test)]
pub(crate) fn existing_view(ws: EntityId, cx: &App) -> Option<Entity<DenseView>> {
    cx.try_global::<DenseViews>()
        .and_then(|m| m.0.get(&ws))
        .cloned()
}

fn toasts_of(w: &Workspace) -> Vec<ToastIn> {
    w.toasts
        .iter()
        .map(|t| ToastIn {
            id: t.id,
            kind: t.kind,
            title: t.title.to_string(),
            body: t.body.to_string(),
        })
        .collect()
}

impl DenseView {
    fn new(
        ws: WeakEntity<Workspace>,
        sidebar: &Entity<crate::workspace::Sidebar>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = Vec::new();
        if let Some(w) = ws.upgrade() {
            // Tasks, terminals, statuses, gates and toasts all notify the workspace: the only refresh.
            subs.push(cx.observe(&w, |v, w, cx| {
                let (changed, read) = v.absorb(w.read(cx), cx);
                v.finish(changed, read, cx);
            }));
        }
        // The tree's selection drives the details column.
        subs.push(cx.observe(sidebar, |v, s, cx| {
            let key = s.read(cx).selected.clone();
            if key != v.seen_sidebar {
                v.seen_sidebar = key.clone();
                if let Some(sel) = key.as_deref().and_then(Sel::from_sidebar_key) {
                    v.selected = Some(sel);
                    cx.notify();
                }
            }
        }));
        Self {
            ws,
            log: EventLog::default(),
            selected: None,
            seen_sidebar: None,
            seen_focus: None,
            log_focus: cx.focus_handle().tab_stop(true),
            details_focus: cx.focus_handle().tab_stop(true),
            log_sel: None,
            log_scroll: UniformListScrollHandle::new(),
            cleared: 0,
            show_tree: true,
            show_output: true,
            show_details: true,
            lines: HashMap::new(),
            reading: HashSet::new(),
            busy: HashMap::new(),
            errors: HashMap::new(),
            _subs: subs,
        }
    }

    /// Record the workspace's changes and follow its focused terminal.
    pub(crate) fn sync(&mut self, w: &Workspace, cx: &mut Context<Self>) {
        let (changed, read) = self.absorb(w, cx);
        self.finish(changed, read, cx);
    }

    /// [`Self::sync`] without touching the context mutably (the workspace may be borrowed from it).
    fn absorb(&mut self, w: &Workspace, cx: &App) -> (bool, Option<LineRead>) {
        let mut changed = false;
        if w.focused_terminal != self.seen_focus {
            self.seen_focus = w.focused_terminal.clone();
            if let Some(t) = &self.seen_focus {
                self.selected = Some(Sel::Terminal(t.clone()));
                changed = true;
            }
        }
        if self.selected.is_none()
            && let Some(sel) = w
                .sidebar
                .read(cx)
                .selected
                .as_deref()
                .and_then(Sel::from_sidebar_key)
        {
            self.selected = Some(sel);
            changed = true;
        }
        let label = |t: &TerminalVm| w.label_of(t);
        let inputs = Inputs {
            terminals: &w.terminals,
            tasks: &w.tasks,
            gates: &w.orch.gates,
            toasts: toasts_of(w),
            label: &label,
        };
        let at_tail = self
            .log_sel
            .is_none_or(|s| self.log.events().next_back().is_none_or(|e| e.seq == s));
        if self.log.observe(&inputs, crate::orch::now_ms()) {
            changed = true;
            if at_tail {
                // Follow the newest line unless the user is reading older ones.
                self.log_scroll.scroll_to_item(
                    self.visible_events().saturating_sub(1),
                    ScrollStrategy::Bottom,
                );
            }
        }
        (changed, self.line_read(w))
    }

    fn finish(&mut self, changed: bool, read: Option<LineRead>, cx: &mut Context<Self>) {
        if let Some(r) = read {
            self.start_read(r, cx);
        }
        if changed {
            cx.notify();
        }
    }

    fn visible_events(&self) -> usize {
        self.log.events().filter(|e| e.seq >= self.cleared).count()
    }

    fn visible(&self) -> Vec<LogEvent> {
        self.log
            .events()
            .filter(|e| e.seq >= self.cleared)
            .cloned()
            .collect()
    }

    /// The selected terminal's last lines need a read when its status key changed.
    fn line_read(&self, w: &Workspace) -> Option<LineRead> {
        let Some(Sel::Terminal(id)) = &self.selected else {
            return None;
        };
        let t = w.terminal(id)?;
        let key = line_key(t);
        if self.reading.contains(id) || self.lines.get(id).is_some_and(|(k, _)| *k == key) {
            return None;
        }
        Some(LineRead {
            id: id.clone(),
            key,
            source: w.source.clone(),
        })
    }

    fn start_read(&mut self, r: LineRead, cx: &mut Context<Self>) {
        let LineRead { id, key, source } = r;
        if !self.reading.insert(id.clone()) {
            return;
        }
        let rx = source.call::<p::TerminalRead>(&p::ReadParams {
            target: p::TerminalRef::id(id.clone()),
            mode: p::ReadMode::Screen,
            lines: Some(LAST_LINES + 4),
            raw: false,
        });
        cx.spawn(async move |this, cx| {
            let lines: Vec<String> = rx
                .await
                .ok()
                .map(|r| {
                    r.lines
                        .into_iter()
                        .map(|l| l.trim_end().to_string())
                        .filter(|l| !l.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let start = lines.len().saturating_sub(LAST_LINES);
            let _ = this.update(cx, |v, cx| {
                v.reading.remove(&id);
                v.lines.insert(id, (key, lines[start..].to_vec()));
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn last_lines(&self, id: &str) -> Option<&[String]> {
        self.lines.get(id).map(|(_, l)| l.as_slice())
    }

    pub(crate) fn select(&mut self, sel: Sel, cx: &mut Context<Self>) {
        self.selected = Some(sel);
        let read = self.ws.upgrade().and_then(|w| self.line_read(w.read(cx)));
        self.finish(true, read, cx);
    }

    /// The regions in F6 order that are shown now, with the handle to focus.
    fn regions(&self, window: &Window, cx: &App) -> Vec<(Region, FocusHandle, bool)> {
        let Some(ws) = self.ws.upgrade() else {
            return Vec::new();
        };
        let w = ws.read(cx);
        let mut r = Vec::new();
        if self.show_tree {
            let f = w.sidebar_focus_handle(cx);
            let inside = f.contains_focused(window, cx);
            r.push((Region::Tree, f, inside));
        }
        let dock = w.dock.read(cx).focus_handle(cx);
        let dock_in = dock.contains_focused(window, cx);
        // Prefer the terminal the user worked in last.
        let panels = w.live_panels();
        let term = w
            .focused_terminal
            .as_ref()
            .and_then(|id| panels.iter().find(|p| &p.read(cx).terminal_id == id))
            .or(panels.first())
            .map(|p| p.read(cx).view.read(cx).focus_handle(cx));
        let term_in = panels.iter().any(|p| {
            p.read(cx)
                .view
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        });
        r.push((Region::Dock, term.unwrap_or(dock), dock_in || term_in));
        if self.show_output {
            r.push((
                Region::Output,
                self.log_focus.clone(),
                self.log_focus.contains_focused(window, cx),
            ));
        }
        if self.show_details {
            r.push((
                Region::Details,
                self.details_focus.clone(),
                self.details_focus.contains_focused(window, cx),
            ));
        }
        r
    }

    /// F6 / Shift+F6: the next / previous region.
    pub(crate) fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let regions = self.regions(window, cx);
        if regions.is_empty() {
            return;
        }
        let n = regions.len();
        let at = regions.iter().position(|(_, _, inside)| *inside);
        let next = match at {
            Some(i) if forward => (i + 1) % n,
            Some(i) => (i + n - 1) % n,
            None => 0,
        };
        self.focus_region(regions[next].0, regions[next].1.clone(), window, cx);
    }

    fn focus_region(
        &mut self,
        r: Region,
        f: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if r == Region::Output && self.log_sel.is_none() {
            self.log_sel = self.log.events().next_back().map(|e| e.seq);
        }
        f.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn focus_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_output = true;
        self.focus_region(Region::Output, self.log_focus.clone(), window, cx);
    }

    pub(crate) fn focus_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_details = true;
        self.focus_region(Region::Details, self.details_focus.clone(), window, cx);
    }

    /// Keyboard of the log: ↑↓ Home End PageUp PageDown select, Enter shows the line's agent.
    fn on_log_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let events = self.visible();
        if events.is_empty() {
            return;
        }
        let last = events.len() - 1;
        let at = self
            .log_sel
            .and_then(|s| events.iter().position(|e| e.seq == s));
        let next = match e.keystroke.key.as_str() {
            "down" => Some(at.map_or(last, |i| (i + 1).min(last))),
            "up" => Some(at.map_or(last, |i| i.saturating_sub(1))),
            "home" => Some(0),
            "end" => Some(last),
            "pagedown" => Some(at.map_or(last, |i| (i + LOG_ROWS).min(last))),
            "pageup" => Some(at.map_or(0, |i| i.saturating_sub(LOG_ROWS))),
            "enter" | "space" => {
                if let Some(ev) = at.map(|i| events[i].clone()) {
                    self.activate_event(&ev, e.keystroke.key == "enter", window, cx);
                }
                cx.stop_propagation();
                return;
            }
            _ => None,
        };
        if let Some(i) = next {
            self.log_sel = Some(events[i].seq);
            self.log_scroll.scroll_to_item(i, ScrollStrategy::Nearest);
            cx.notify();
            cx.stop_propagation();
        }
    }

    /// A log line: its agent (or task) into the details column, its panel brought forward.
    /// `open` (Enter) also opens the terminal and moves focus there.
    fn activate_event(
        &mut self,
        ev: &LogEvent,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.log_sel = Some(ev.seq);
        let sel = ev
            .terminal
            .clone()
            .map(Sel::Terminal)
            .or_else(|| ev.task.clone().map(Sel::Task));
        let Some(sel) = sel else {
            cx.notify();
            return;
        };
        self.select(sel.clone(), cx);
        if let (Sel::Terminal(t), Some(ws)) = (sel, self.ws.upgrade()) {
            ws.update(cx, |w, cx| {
                if open {
                    w.open_terminal(&t, false, window, cx);
                } else if let Some(p) = w
                    .live_panels()
                    .into_iter()
                    .find(|p| p.read(cx).terminal_id == t)
                {
                    let pid = PanelId::from(p.entity_id());
                    w.dock.update(cx, |a, cx| a.select_panel(pid, window, cx));
                }
            });
        }
    }

    /// 이번만 허용 / 거부 through the one gate decision path.
    fn decide(
        &mut self,
        gate: String,
        decision: GateDecision,
        choice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.contains_key(&gate) {
            return;
        }
        let Some(ws) = self.ws.upgrade() else { return };
        let source = ws.read(cx).source.clone();
        self.busy.insert(gate.clone(), decision);
        self.errors.remove(&gate);
        // The buttons go away when podd answers: keep the keyboard in the column.
        self.details_focus.focus(window, cx);
        let me = cx.entity().downgrade();
        let id = gate.clone();
        let params = p::GateResolveParams {
            id: gate,
            decision,
            note: None,
            choice,
        };
        crate::gates::resolve_gate(&source, self.ws.clone(), params, cx, move |r, cx| {
            let _ = me.update(cx, |v, cx| {
                v.busy.remove(&id);
                if let Err(e) = r {
                    v.errors.insert(id, e.text);
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    /// 다시 실행: the same command in the same folder and task (`terminal.run`).
    fn rerun(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        let Some(t) = ws.read(cx).terminal(id).cloned() else {
            return;
        };
        let spec = RunSpec {
            argv: t.argv.clone(),
            cwd: Some(t.cwd.clone()),
            name: None,
            cols: None,
            rows: None,
            task: t.task_id.clone(),
        };
        let rx = ws.read(cx).source.run(spec);
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(Ok(new)) => {
                        ws.update(cx, |w, cx| {
                            if !w.terminals.iter().any(|x| x.id == new.id) {
                                w.terminals.push(new.clone());
                            }
                            cx.notify();
                        });
                        v.selected = Some(Sel::Terminal(new.id));
                    }
                    Ok(Err(e)) => ws.update(cx, |w, cx| {
                        w.raise_toast(ToastKind::Error, copy::COULD_NOT_RERUN, e.message, cx)
                    }),
                    Err(_) => ws.update(cx, |w, cx| {
                        w.raise_toast(
                            ToastKind::Error,
                            copy::COULD_NOT_RERUN,
                            crate::strings::PODD_CLIENT_STOPPED,
                            cx,
                        )
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

// ───────────────────────────── metrics ─────────────────────────────

fn tc(hex: u32) -> Hsla {
    pod_ui::tokens::color(hex)
}

/// UI font of the style at the user's font size.
pub fn ui_font(cx: &App) -> f32 {
    let user = cx
        .try_global::<crate::theme::Settings>()
        .map_or(13., |s| s.font_size);
    crate::theme::ui_font_size(user, pod_ui::themes::metrics().density.ui_font)
}

/// Row height: the style's row, grown with the font so text is never clipped.
pub fn row_height(ui: f32) -> f32 {
    pod_ui::themes::metrics()
        .density
        .row
        .max((ui * 1.5).ceil() + 4.)
}

/// Height of the 출력 panel (header + [`LOG_ROWS`] rows).
pub fn output_height(ui: f32) -> f32 {
    let m = pod_ui::themes::metrics();
    let row = row_height(ui);
    m.density.panel_header.max(row) + LOG_ROWS as f32 * row + m.border_width + 4.
}

/// Width of the 세부 정보 column.
pub fn details_width(ui: f32) -> f32 {
    (pod_ui::themes::metrics().density.sidebar + 24.).max(ui * 20.)
}

// ───────────────────────────── rendering ─────────────────────────────

fn mono_font(cx: &App) -> SharedString {
    cx.try_global::<crate::theme::Settings>()
        .map(|s| s.terminal_font.clone())
        .unwrap_or_default()
        .into()
}

/// Panel header: a title and trailing buttons.
fn panel_header(
    id: &'static str,
    title: impl IntoElement,
    buttons: Vec<AnyElement>,
    ui: f32,
) -> Div {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    div()
        .qa_id(move || id.to_string())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .pl_3()
        .pr_1()
        .min_h(px(m.density.panel_header.max(row_height(ui))))
        .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
            d.border_b(px(m.border_width))
                .border_color(tc(tk.border_subtle))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(title),
        )
        .children(buttons)
}

/// Text action: accent text, keyboard operable, at least the style's hit area tall.
fn link_button(
    id: SharedString,
    label: &'static str,
    name: String,
    icon: Option<Icon>,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let activate = Rc::new(activate);
    let key = activate.clone();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(SharedString::from(name))
        .tab_index(0)
        .min_h(px(pod_ui::hit_area()))
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .rounded(px(m.radius.control))
        .text_color(tc(tk.accent_fg))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(tc(tk.bg_hover)))
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .when_some(icon, |d, i| {
            d.child(pod_ui::icon_sized(i, 14., tk.accent_fg))
        })
        .child(label)
        .on_click(move |_, window, cx| activate(window, cx))
        .on_key_down(move |e, window, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                key(window, cx);
                cx.stop_propagation();
            }
        })
}

/// Activity bar: tree toggle, files, agent tree, search, gates; output/details toggles and settings at the bottom.
fn activity_bar(view: &Entity<DenseView>, cx: &mut Context<Workspace>) -> AnyElement {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let ws = cx.entity().downgrade();
    let v = view.read(cx);
    let (tree, output, details) = (v.show_tree, v.show_output, v.show_details);
    let toggle = |id: &'static str,
                  icon: Icon,
                  label: &'static str,
                  tip: &'static str,
                  on: bool,
                  f: fn(&mut DenseView, &mut Window, &mut Context<DenseView>)| {
        let view = view.downgrade();
        div()
            .rounded(px(m.radius.control))
            .relative()
            .when(on, |d| {
                d.bg(tc(tk.bg_selected)).child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(6.))
                        .bottom(px(6.))
                        .w(px(2.))
                        .bg(tc(tk.accent_fg)),
                )
            })
            .child(
                pod_ui::IconButton::new(id, icon, label)
                    .tooltip(tip)
                    .render(mode, move |window, cx| {
                        let _ = view.update(cx, |v, cx| {
                            f(v, window, cx);
                            cx.notify();
                        });
                    }),
            )
    };
    let item = |id: &'static str,
                icon: Icon,
                label: &'static str,
                tip: &'static str,
                f: fn(&mut Workspace, &mut Window, &mut Context<Workspace>)| {
        let ws = ws.clone();
        pod_ui::IconButton::new(id, icon, label)
            .tooltip(tip)
            .render(mode, move |window, cx| {
                let _ = ws.update(cx, |w, cx| f(w, window, cx));
            })
    };
    div()
        .id("dense-activity")
        .qa_id(|| "dense-activity".into())
        .role(Role::Toolbar)
        .aria_label(copy::ACTIVITY)
        .w(px(pod_ui::hit_area() + 12.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .py_1()
        .bg(tc(tk.bg_chrome))
        .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
            d.border_r(px(m.border_width))
                .border_color(tc(tk.border_subtle))
        })
        .child(toggle(
            "dense-toggle-tree",
            Icon::Layers,
            copy::EXPLORER,
            "작업 탐색 보이기/숨기기",
            tree,
            |v, window, cx| {
                v.show_tree = !v.show_tree;
                if v.show_tree
                    && let Some(w) = v.ws.upgrade()
                {
                    let f = w.read(cx).sidebar_focus_handle(cx);
                    f.focus(window, cx);
                }
            },
        ))
        .child(item(
            "dense-files",
            Icon::FileText,
            copy::FILES,
            "파일 (Ctrl+P)",
            |w, window, cx| w.quick_open_files(true, window, cx),
        ))
        .child(item(
            "dense-agents",
            Icon::Bot,
            copy::AGENTS,
            "에이전트 트리 (Ctrl+Shift+E)",
            |w, window, cx| w.open_agent_tree(window, cx),
        ))
        .child(item(
            "dense-search",
            Icon::Search,
            copy::SEARCH,
            "작업, 터미널, 명령 검색 (Ctrl+K)",
            |w, window, cx| w.open_jump(window, cx),
        ))
        .child(item(
            "dense-gates",
            Icon::ShieldQuestion,
            copy::GATES,
            "승인 요청 (Ctrl+Shift+I)",
            |w, window, cx| w.open_gates(window, cx),
        ))
        .child(div().flex_1())
        .child(toggle(
            "dense-toggle-output",
            Icon::Rows2,
            copy::OUTPUT_TOGGLE,
            "출력 패널 (Ctrl+Shift+U)",
            output,
            |v, _, _| v.show_output = !v.show_output,
        ))
        .child(toggle(
            "dense-toggle-details",
            Icon::PanelRight,
            copy::DETAILS_TOGGLE,
            "세부 정보 (Ctrl+Shift+D)",
            details,
            |v, _, _| v.show_details = !v.show_details,
        ))
        .child(item(
            "dense-settings",
            Icon::Settings,
            copy::SETTINGS,
            "설정 (Ctrl+,)",
            |w, window, cx| w.open_settings(window, cx),
        ))
        .into_any_element()
}

/// The 출력 panel: a compact, virtualized log.
fn output_panel(
    view: &Entity<DenseView>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let ui = ui_font(cx);
    let row = row_height(ui);
    let mono = mono_font(cx);
    let offset = local_offset_min();
    let v = view.read(cx);
    let events = Rc::new(v.visible());
    let focused = v.log_focus.contains_focused(window, cx);
    let sel = v.log_sel;
    let focus = v.log_focus.clone();
    let scroll = v.log_scroll.clone();
    let n = events.len();
    let weak = view.downgrade();
    let close = {
        let weak = weak.clone();
        pod_ui::IconButton::new("dense-output-close", Icon::X, copy::CLOSE_OUTPUT).render(
            mode,
            move |window, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.show_output = false;
                    if let Some(w) = v.ws.upgrade() {
                        let f = w.read(cx).dock.read(cx).focus_handle(cx);
                        f.focus(window, cx);
                    }
                    cx.notify();
                });
            },
        )
    };
    let clear = {
        let weak = weak.clone();
        pod_ui::IconButton::new("dense-output-clear", Icon::Trash2, copy::CLEAR).render(
            mode,
            move |_, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.cleared = v.log.events().next_back().map_or(0, |e| e.seq + 1);
                    v.log_sel = None;
                    cx.notify();
                });
            },
        )
    };
    let title = div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .child(copy::OUTPUT),
        )
        .child(
            div()
                .text_color(tc(tk.text_muted))
                .whitespace_nowrap()
                .child(format!("{} {n}", copy::EVENTS)),
        );
    let list = if n == 0 {
        div()
            .px_3()
            .py_2()
            .text_color(tc(tk.text_muted))
            .child(copy::LOG_EMPTY)
            .into_any_element()
    } else {
        let rows = events.clone();
        let weak = weak.clone();
        let mono = mono.clone();
        uniform_list("dense-log-rows", n, move |range, _, _| {
            rows[range]
                .iter()
                .map(|e| {
                    let selected = sel == Some(e.seq);
                    let (fg, _) = e.tone.colors(tk);
                    let text_fg = if matches!(e.tone, Tone::Muted | Tone::Secondary | Tone::Info) {
                        tk.text_primary
                    } else {
                        fg
                    };
                    let time = clock(e.at, offset);
                    let ev = e.clone();
                    let weak = weak.clone();
                    div()
                        .id(SharedString::from(format!("dense-log-{}", e.seq)))
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(format!(
                            "{time}, {}, {}, {}",
                            e.source, e.who, e.text
                        )))
                        .aria_selected(selected)
                        .h(px(row))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_3()
                        .font_family(mono.clone())
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .cursor_pointer()
                        .when(selected, |d| d.bg(tc(tk.bg_selected)))
                        .when(selected && focused, |d| {
                            d.border_1().border_color(tc(tk.focus_ring))
                        })
                        .when(!selected, |d| d.hover(|s| s.bg(tc(tk.bg_hover))))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(tc(tk.text_muted))
                                .child(time),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(px(ui * 3.))
                                .truncate()
                                .text_color(tc(tk.text_secondary))
                                .child(e.source.clone()),
                        )
                        .when_some(e.agent.as_ref(), |d, a| {
                            d.child(crate::agent_kind::mark_el(
                                &format!("dense-log-{}", e.seq),
                                a,
                                14.,
                            ))
                        })
                        .child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(ui * 14.))
                                .truncate()
                                .text_color(tc(tk.text_primary))
                                .child(e.who.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(tc(tk.text_muted))
                                .child("→"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(tc(text_fg))
                                .child(e.text.clone()),
                        )
                        .on_click(move |click, window, cx| {
                            let _ = weak.update(cx, |v, cx| {
                                v.log_focus.focus(window, cx);
                                v.activate_event(&ev, click.click_count() > 1, window, cx);
                            });
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&scroll)
        .flex_1()
        .min_h_0()
        .into_any_element()
    };
    let on_key = view.downgrade();
    div()
        .id("dense-output")
        .qa_id(|| "dense-output".into())
        .role(Role::Region)
        .aria_label(copy::OUTPUT)
        .flex_shrink_0()
        .h(px(output_height(ui)))
        .flex()
        .flex_col()
        .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
            d.border_t(px(m.border_width))
                .border_color(tc(tk.border_subtle))
        })
        .bg(tc(tk.bg_surface))
        .child(panel_header(
            "dense-output-header",
            title,
            vec![clear.into_any_element(), close.into_any_element()],
            ui,
        ))
        .child(
            div()
                .id("dense-log")
                .qa_id(|| "dense-log".into())
                .track_focus(&focus)
                .role(Role::List)
                .aria_label(copy::LOG)
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .py_1()
                .on_key_down(move |e, window, cx| {
                    let _ = on_key.update(cx, |v, cx| v.on_log_key(e, window, cx));
                })
                .child(list),
        )
        .into_any_element()
}

/// A `label  element` row of the details column.
fn field_el(label: &'static str, value: impl IntoElement, ui: f32) -> Div {
    let tk = crate::theme::tokens();
    div()
        .flex()
        .items_center()
        .gap_2()
        .min_h(px(row_height(ui)))
        .py(px(2.))
        .child(
            div()
                .flex_shrink_0()
                .w(px(ui * 5.5))
                .text_color(tc(tk.text_muted))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(tc(tk.text_primary))
                .child(value),
        )
}

/// One `label  value` row of the details column.
fn field(
    label: &'static str,
    value: impl Into<SharedString>,
    fg: u32,
    ui: f32,
    truncate: bool,
) -> Div {
    let tk = crate::theme::tokens();
    let value = value.into();
    let v = div().flex_1().min_w_0().text_color(tc(fg));
    let v = if truncate {
        v.truncate().child(value.clone())
    } else {
        v.child(value.clone())
    };
    div()
        .flex()
        .items_start()
        .gap_2()
        .min_h(px(row_height(ui)))
        .py(px(2.))
        .child(
            div()
                .flex_shrink_0()
                .w(px(ui * 5.5))
                .text_color(tc(tk.text_muted))
                .child(label),
        )
        .child(v)
}

fn section_title(text: &'static str) -> Div {
    let tk = crate::theme::tokens();
    div()
        .pt_3()
        .pb_1()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tc(tk.text_secondary))
        .child(text)
}

fn pr_text(pr: &p::TaskPr) -> String {
    let state = format!("{:?}", pr.state).to_lowercase();
    let state = match state.as_str() {
        "open" if pr.draft => "초안",
        "open" => "열림",
        "merged" => "병합됨",
        "closed" => "닫힘",
        _ => "",
    };
    format!("#{} {} · {}", pr.number, state, pr.title)
}

/// The 세부 정보 column for the selected task or agent.
fn details_column(
    view: &Entity<DenseView>,
    ws: &Workspace,
    _window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let tk = crate::theme::tokens();
    let mode = crate::theme::mode();
    let ui = ui_font(cx);
    let mono = mono_font(cx);
    let now = crate::orch::now_ms();
    let offset = local_offset_min();
    let v = view.read(cx);
    let weak = view.downgrade();
    let wsw = cx.entity().downgrade();
    let focus = v.details_focus.clone();
    let mut body = div().w_full().flex().flex_col().px_3().pb_3();
    let mut name = String::from(copy::NOTHING);

    // The selection, or nothing when it vanished.
    let sel = v.selected.clone().filter(|s| match s {
        Sel::Task(id) => ws.task(id).is_some(),
        Sel::Terminal(id) => ws.terminal(id).is_some(),
    });
    let mut history_ids: Vec<String> = Vec::new();
    let mut actions: Vec<AnyElement> = Vec::new();
    let action_ws = |f: WsAction| {
        let wsw = wsw.clone();
        move |window: &mut Window, cx: &mut App| {
            let _ = wsw.update(cx, |w, cx| f(w, window, cx));
        }
    };
    match &sel {
        None => {
            body = body.child(
                div()
                    .pt_2()
                    .text_color(tc(tk.text_muted))
                    .child(copy::NOTHING),
            );
        }
        Some(Sel::Terminal(id)) => {
            let t = ws.terminal(id).cloned().expect("filtered above");
            let task: Option<TaskVm> = t.task_id.as_deref().and_then(|k| ws.task(k)).cloned();
            let label = ws.label_of(&t);
            let badge = crate::orch::terminal_badge(&t);
            let (_, state_fg) = badge.glyph(mode);
            let agent = crate::agent_kind::identify_in(&t, &ws.tasks).kind;
            name = format!("{label}, {}, {}", agent.accessible_name(), badge.label());
            body = body
                .child(field(copy::NAME, label.clone(), tk.text_primary, ui, false))
                .child(field_el(
                    copy::AGENT,
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(crate::agent_kind::mark_el("dense-details", &agent, 16.))
                        .child(agent.name()),
                    ui,
                ))
                .child(field(copy::STATE, badge.label(), state_fg, ui, false))
                .when_some(task.as_ref(), |d, k| {
                    d.child(field(
                        copy::TASK,
                        k.title.clone(),
                        tk.text_primary,
                        ui,
                        false,
                    ))
                })
                .child(field(
                    copy::PROGRAM,
                    t.program.clone(),
                    tk.text_primary,
                    ui,
                    false,
                ))
                .child(field(
                    copy::COMMAND,
                    t.argv.join(" "),
                    tk.text_secondary,
                    ui,
                    true,
                ))
                .when_some(task.as_ref().and_then(|k| k.branch.clone()), |d, b| {
                    d.child(field(copy::BRANCH, b, tk.text_primary, ui, true))
                })
                .child(field(
                    copy::WORKTREE,
                    task.as_ref()
                        .and_then(|k| k.worktree.clone())
                        .unwrap_or_else(|| t.cwd.clone()),
                    tk.text_secondary,
                    ui,
                    true,
                ))
                .when_some(
                    t.status
                        .as_ref()
                        .map(|s| crate::layouts::queue::age(s.since, now))
                        .filter(|a| !a.is_empty()),
                    |d, a| d.child(field(copy::ELAPSED, a, tk.text_primary, ui, false)),
                )
                .when_some(t.status.as_ref().and_then(|s| s.source.clone()), |d, s| {
                    d.child(field(copy::ORIGIN, s, tk.text_secondary, ui, false))
                })
                .child(field(
                    copy::PR,
                    task.as_ref()
                        .and_then(|k| k.pr.as_ref())
                        .map(pr_text)
                        .unwrap_or_else(|| copy::NO_PR.into()),
                    tk.text_primary,
                    ui,
                    false,
                ));
            // ── actions ──
            if let Some(g) = ws.orch.gates.values().find(|g| {
                g.state == GateState::Pending && g.terminal.as_deref() == Some(id.as_str())
            }) {
                let busy = v.busy.get(&g.id).copied();
                let state = |d: GateDecision| match busy {
                    Some(b) if b == d => ControlState::Loading,
                    Some(_) => ControlState::Disabled,
                    None => ControlState::Ready,
                };
                body = body
                    .child(div().pt_2().text_color(tc(tk.warning_fg)).child(format!(
                        "{} · {}",
                        copy::GATE_REQUESTED,
                        g.title
                    )))
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_color(tc(tk.text_secondary))
                            .child(g.message.clone()),
                    );
                // A gate with options (v17) is approved by picking one; otherwise 이번만 허용.
                let approve: Vec<(String, String, Option<String>)> = if g.options.is_empty() {
                    vec![(
                        copy::ALLOW_ONCE.to_string(),
                        format!("dense-allow-{}", g.id),
                        None,
                    )]
                } else {
                    g.options
                        .iter()
                        .enumerate()
                        .map(|(i, o)| {
                            (
                                o.clone(),
                                format!("dense-allow-{}-{i}", g.id),
                                Some(o.clone()),
                            )
                        })
                        .collect()
                };
                let buttons = approve
                    .into_iter()
                    .map(|(label, id, choice)| {
                        (GateDecision::Approve, label, id, choice, Tone::Success)
                    })
                    .chain(std::iter::once((
                        GateDecision::Reject,
                        copy::REJECT.to_string(),
                        format!("dense-reject-{}", g.id),
                        None,
                        Tone::Danger,
                    )));
                for (d, lbl, bid, choice, tone) in buttons {
                    let (weak, gid) = (weak.clone(), g.id.clone());
                    let name = match &choice {
                        Some(_) => format!("{}: {lbl}: {}", copy::CHOOSE, g.title),
                        None => format!("{lbl}: {}", g.title),
                    };
                    actions.push(
                        semantic_button(
                            SharedString::from(bid),
                            lbl,
                            tone,
                            mode,
                            state(d),
                            move |window, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    v.decide(gid.clone(), d, choice.clone(), window, cx)
                                });
                            },
                        )
                        .aria_label(SharedString::from(name))
                        .into_any_element(),
                    );
                }
                if let Some(e) = v.errors.get(&g.id) {
                    body = body.child(div().text_color(tc(tk.danger_fg)).child(e.clone()));
                }
            }
            if t.is_running() {
                let tid = t.id.clone();
                actions.push(
                    link_button(
                        "dense-open-terminal".into(),
                        copy::OPEN_TERMINAL,
                        format!("{}: {label}", copy::OPEN_TERMINAL),
                        Some(Icon::SquareTerminal),
                        action_ws(Rc::new(move |w, window, cx| {
                            w.open_terminal(&tid, false, window, cx)
                        })),
                    )
                    .into_any_element(),
                );
            } else {
                let (weak, tid) = (weak.clone(), t.id.clone());
                actions.push(
                    link_button(
                        "dense-rerun".into(),
                        copy::RERUN,
                        format!("{}: {label}", copy::RERUN),
                        Some(Icon::RotateCcw),
                        move |_, cx| {
                            let _ = weak.update(cx, |v, cx| v.rerun(&tid, cx));
                        },
                    )
                    .into_any_element(),
                );
            }
            if let Some(k) = &task {
                push_task_actions(&mut actions, k, &action_ws);
            }
            history_ids.push(t.id.clone());
            // ── last lines ──
            let lines = v
                .lines
                .get(&t.id)
                .map(|(_, l)| l.clone())
                .unwrap_or_default();
            let lines_el = div()
                .qa_id(|| "dense-last-lines".into())
                .flex()
                .flex_col()
                .p_2()
                .rounded(px(pod_ui::themes::metrics().radius.control))
                .bg(tc(tk.bg_raised))
                .font_family(mono.clone())
                .text_color(tc(tk.text_secondary))
                .when(lines.is_empty(), |d| {
                    d.text_color(tc(tk.text_muted)).child(copy::NO_LINES)
                })
                .children(lines.into_iter().map(|l| div().truncate().child(l)));
            body = body
                .child(section_title(copy::ACTIONS))
                .child(
                    div()
                        .qa_id(|| "dense-actions".into())
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .items_center()
                        .children(actions),
                )
                .child(section_title(copy::LAST_LINES))
                .child(lines_el);
        }
        Some(Sel::Task(id)) => {
            let k = ws.task(id).cloned().expect("filtered above");
            let badge = crate::orch::status_badge(k.status.as_ref());
            let (_, state_fg) = badge.glyph(mode);
            name = format!("{}, {}", k.title, badge.label());
            let kinds = crate::agent_kind::task_kinds(&k, &ws.terminals);
            body = body
                .child(field(
                    copy::NAME,
                    k.title.clone(),
                    tk.text_primary,
                    ui,
                    false,
                ))
                .when(!kinds.is_empty(), |d| {
                    d.child(field_el(
                        copy::AGENT,
                        crate::agent_kind::stack_el("dense-details", &kinds, 16.),
                        ui,
                    ))
                })
                .child(field(copy::STATE, badge.label(), state_fg, ui, false))
                .when_some(k.repo.clone(), |d, r| {
                    d.child(field(copy::REPO, r, tk.text_primary, ui, true))
                })
                .when_some(k.branch.clone(), |d, b| {
                    d.child(field(copy::BRANCH, b, tk.text_primary, ui, true))
                })
                .when_some(k.worktree.clone(), |d, w| {
                    d.child(field(copy::WORKTREE, w, tk.text_secondary, ui, true))
                })
                .when_some(k.git_summary(), |d, g| {
                    d.child(field(copy::GIT, g, tk.text_primary, ui, false))
                })
                .when(k.review_open > 0, |d| {
                    d.child(field(
                        copy::COMMENTS,
                        format!("{}개", k.review_open),
                        tk.warning_fg,
                        ui,
                        false,
                    ))
                })
                .child(field(
                    copy::PR,
                    k.pr.as_ref()
                        .map(pr_text)
                        .unwrap_or_else(|| copy::NO_PR.into()),
                    tk.text_primary,
                    ui,
                    false,
                ));
            push_task_actions(&mut actions, &k, &action_ws);
            body = body.child(section_title(copy::ACTIONS)).child(
                div()
                    .qa_id(|| "dense-actions".into())
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .children(actions),
            );
            // ── its agents ──
            let mut agents = div().flex().flex_col();
            for tid in &k.terminals {
                let Some(t) = ws.terminal(tid) else { continue };
                let label = ws.label_of(t);
                let b = crate::orch::terminal_badge(t);
                let (icon, fg) = b.glyph(mode);
                let (weak, tid2) = (weak.clone(), tid.clone());
                agents = agents.child(
                    div()
                        .id(SharedString::from(format!("dense-agent-{tid}")))
                        .role(Role::Button)
                        .aria_label(SharedString::from(format!("{label}, {}", b.label())))
                        .tab_index(0)
                        .min_h(px(row_height(ui).max(pod_ui::hit_area())))
                        .px_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(pod_ui::themes::metrics().radius.control))
                        .cursor_pointer()
                        .hover(|s| s.bg(tc(tk.bg_hover)))
                        .focus_visible(move |s| pod_ui::focus_style(s, mode))
                        .child(crate::agent_kind::mark_el(
                            &format!("dense-agent-{tid}"),
                            &crate::agent_kind::identify(t, Some(&k)).kind,
                            14.,
                        ))
                        .child(pod_ui::icon_sized(icon, 14., fg))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(div().flex_shrink_0().text_color(tc(fg)).child(b.label()))
                        .on_click({
                            let (weak, tid2) = (weak.clone(), tid2.clone());
                            move |_, _, cx| {
                                let _ = weak
                                    .update(cx, |v, cx| v.select(Sel::Terminal(tid2.clone()), cx));
                            }
                        })
                        .on_key_down(move |e, _, cx| {
                            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                                let _ = weak
                                    .update(cx, |v, cx| v.select(Sel::Terminal(tid2.clone()), cx));
                                cx.stop_propagation();
                            }
                        }),
                );
                history_ids.push(tid.clone());
            }
            body = body
                .child(section_title(copy::AGENTS_OF_TASK))
                .child(agents);
        }
    }
    if sel.is_some() {
        let history: Vec<LogEvent> = v
            .log
            .history(&history_ids)
            .take(HISTORY_ROWS)
            .cloned()
            .collect();
        let mut list = div().qa_id(|| "dense-history".into()).flex().flex_col();
        if history.is_empty() {
            list = list.child(div().text_color(tc(tk.text_muted)).child(copy::NO_HISTORY));
        }
        for e in history {
            let (fg, _) = e.tone.colors(tk);
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .min_h(px(row_height(ui)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(mono.clone())
                            .text_color(tc(tk.text_muted))
                            .child(clock(e.at, offset)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(tc(
                                if matches!(e.tone, Tone::Muted | Tone::Secondary | Tone::Info) {
                                    tk.text_primary
                                } else {
                                    fg
                                },
                            ))
                            .child(e.text.clone()),
                    ),
            );
        }
        body = body.child(section_title(copy::HISTORY)).child(list);
    }
    let close = {
        let weak = weak.clone();
        pod_ui::IconButton::new("dense-details-close", Icon::X, copy::CLOSE_DETAILS).render(
            mode,
            move |_, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.show_details = false;
                    cx.notify();
                });
            },
        )
    };
    let m = pod_ui::themes::metrics();
    div()
        .id("dense-details")
        .qa_id(|| "dense-details".into())
        .w(px(details_width(ui)))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .min_h_0()
        .bg(tc(tk.bg_chrome))
        .when(
            m.surface != pod_ui::themes::Surface::Mono
                && !matches!(
                    m.surface,
                    pod_ui::themes::Surface::Layered | pod_ui::themes::Surface::Islands
                ),
            |d| {
                d.border_l(px(m.border_width))
                    .border_color(tc(tk.border_subtle))
            },
        )
        .child(panel_header(
            "dense-details-header",
            div().font_weight(FontWeight::SEMIBOLD).child(copy::DETAILS),
            vec![close.into_any_element()],
            ui,
        ))
        .child(
            div()
                .id("dense-details-body")
                .track_focus(&focus)
                .role(Role::Region)
                .aria_label(SharedString::from(format!("{}: {name}", copy::DETAILS)))
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
        .into_any_element()
}

type WsAction = Rc<dyn Fn(&mut Workspace, &mut Window, &mut Context<Workspace>)>;

/// 변경 검토 · 파일 열기 · PR 보기 of a task.
fn push_task_actions<F: Fn(&mut Window, &mut App) + 'static>(
    actions: &mut Vec<AnyElement>,
    k: &TaskVm,
    wrap: &dyn Fn(WsAction) -> F,
) {
    let id = k.id.clone();
    let i = id.clone();
    actions.push(
        link_button(
            "dense-review".into(),
            copy::REVIEW,
            format!("{}: {}", copy::REVIEW, k.title),
            Some(Icon::GitCompareArrows),
            wrap(Rc::new(move |w, window, cx| w.open_review(&i, window, cx))),
        )
        .into_any_element(),
    );
    if k.worktree_exists || k.worktree.is_some() {
        let i = id.clone();
        actions.push(
            link_button(
                "dense-files-open".into(),
                copy::OPEN_FILES,
                format!("{}: {}", copy::OPEN_FILES, k.title),
                Some(Icon::FileText),
                wrap(Rc::new(move |w, window, cx| {
                    w.open_editor(&i, None, false, window, cx)
                })),
            )
            .into_any_element(),
        );
    }
    let i = id;
    actions.push(
        link_button(
            "dense-pr".into(),
            copy::OPEN_PR,
            format!("{}: {}", copy::OPEN_PR, k.title),
            Some(Icon::GitPullRequest),
            wrap(Rc::new(move |w, window, cx| {
                w.open_pr(&i, false, window, cx)
            })),
        )
        .into_any_element(),
    );
}

/// The window body: activity bar · tree (+ toasts) · dock over 출력 · 세부 정보.
pub fn render_body(
    ws: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let view = view(ws, cx);
    // First frame of a fresh view: record the current state (later changes arrive by observation).
    if view.read(cx).log.is_empty() {
        let w: &Workspace = ws;
        view.update(cx, |v, cx| v.sync(w, cx));
    }
    let m = pod_ui::themes::metrics();
    let tk = crate::theme::tokens();
    use pod_ui::themes::{Shadow, Surface};
    let rounded = matches!(m.surface, Surface::Layered | Surface::Islands);
    let gap = if rounded { m.gap } else { 0. };
    let island = move |d: Div| -> Div {
        if !rounded {
            return d;
        }
        let d = d
            .rounded(px(m.radius.card))
            .overflow_hidden()
            .bg(rgb(tk.bg_surface));
        match (m.surface, m.shadow) {
            (Surface::Islands, _) | (_, Shadow::Raised) => d
                .shadow(pod_ui::tokens::elevation(1, crate::theme::mode()))
                .border_1()
                .border_color(rgb(tk.border_subtle)),
            (_, Shadow::Soft) => d.shadow(pod_ui::tokens::elevation(1, crate::theme::mode())),
            _ => d,
        }
    };
    let banner = match &ws.conn {
        crate::vm::ConnState::Reconnecting { attempt, error } => {
            Some(crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR; attempt,error;))
        }
        crate::vm::ConnState::Connecting => {
            Some(crate::strings::CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED.to_string())
        }
        crate::vm::ConnState::Connected { .. } => None,
    };
    let empty = ws.live_panels().is_empty()
        && ws.live_reviews().is_empty()
        && ws.live_editors().is_empty()
        && !ws.orch.has_panels()
        && ws
            .settings_panel
            .as_ref()
            .and_then(|p| p.upgrade())
            .is_none()
        && !crate::mods_ui::page_open(cx)
        && !crate::sessions_ui::page_open(cx)
        && ws
            .inspector_panel
            .as_ref()
            .and_then(|p| p.upgrade())
            .is_none();
    let (show_tree, show_output, show_details) = {
        let v = view.read(cx);
        (v.show_tree, v.show_output, v.show_details)
    };
    let toasts = ws.render_toasts(cx).into_any_element();
    let bar = activity_bar(&view, cx);
    let output = show_output.then(|| output_panel(&view, window, cx));
    let details = show_details.then(|| details_column(&view, ws, window, cx));
    let sidebar_w = m.density.sidebar;
    let actions_view = view.downgrade();
    let (v1, v2, v3) = (
        actions_view.clone(),
        actions_view.clone(),
        actions_view.clone(),
    );
    div()
        .id("dense-layout")
        .qa_id(|| "dense-layout".into())
        .key_context(KEY_CONTEXT)
        .on_action(move |_: &NextRegion, window, cx| {
            let _ = actions_view.update(cx, |v, cx| v.cycle(true, window, cx));
        })
        .on_action(move |_: &PrevRegion, window, cx| {
            let _ = v1.update(cx, |v, cx| v.cycle(false, window, cx));
        })
        .on_action(move |_: &FocusOutput, window, cx| {
            let _ = v2.update(cx, |v, cx| v.focus_output(window, cx));
        })
        .on_action(move |_: &FocusDetails, window, cx| {
            let _ = v3.update(cx, |v, cx| v.focus_details(window, cx));
        })
        .flex()
        .flex_1()
        .min_h_0()
        .child(bar)
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
                .when(show_tree, |d| {
                    d.child(
                        div()
                            .id("dense-tree")
                            .qa_id(|| "dense-tree".into())
                            .w(px(sidebar_w))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(gap))
                            .child(island(div().flex_1().min_h_0()).child(ws.sidebar.clone()))
                            .child(toasts),
                    )
                })
                .child(
                    island(div())
                        .id("dense-center")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .when_some(banner, |d, b| d.child(div().px_2().pt_2().child(pod_ui::banner(Tone::Warning, crate::theme::mode(), b, None).text_xs())))
                        .when_some(ws.render_hook_banner(cx), |d, b| d.child(b))
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(ws.dock.clone())
                                .when(empty, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .px_4()
                                            .text_color(rgb(tk.text_muted))
                                            .child(pod_ui::Frame::new("dense-dock-empty", pod_ui::FrameKind::Empty, crate::strings::A_CLEAR_SPACE_FOR_YOUR_NEXT_TASK, crate::strings::YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_).render(crate::theme::mode(), [])),
                                    )
                                }),
                        )
                        .when_some(output, |d, o| d.child(o)),
                )
                .when_some(details, |d, x| d.child(island(div().flex().min_h_0()).child(x))),
        )
        .into_any_element()
}

// ───────────────────────────── layout module ─────────────────────────────

/// The 고밀도 + 세부 열 layout module (preset 07).
pub struct DenseLayout;

impl LayoutModule for DenseLayout {
    fn mode(&self) -> LayoutMode {
        LayoutMode::DenseDetails
    }

    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        render_body(ws, window, cx)
    }
}

/// Region and panel shortcuts. F6 is bound only inside the layout (terminal
/// apps keep F6 elsewhere); Ctrl+Shift+U/D work from anywhere in the window
/// (the workspace root holds focus outside the layout) while it is shown.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f6", NextRegion, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-f6", PrevRegion, Some(KEY_CONTEXT)),
        KeyBinding::new(FOCUS_OUTPUT_KEY, FocusOutput, None),
        KeyBinding::new(FOCUS_DETAILS_KEY, FocusDetails, None),
    ]);
    cx.on_action(|_: &FocusOutput, cx: &mut App| {
        with_active_view(cx, |v, window, cx| v.focus_output(window, cx))
    });
    cx.on_action(|_: &FocusDetails, cx: &mut App| {
        with_active_view(cx, |v, window, cx| v.focus_details(window, cx))
    });
}

/// Run `f` on the view of the active window's workspace when this layout is shown.
fn with_active_view(cx: &mut App, f: fn(&mut DenseView, &mut Window, &mut Context<DenseView>)) {
    let Some(window) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| {
            let layout = cx
                .try_global::<crate::theme::Settings>()
                .map(|s| s.layout.clone())
                .unwrap_or_default();
            if layout != LayoutMode::DenseDetails.id() {
                return;
            }
            let ws = cx
                .try_global::<crate::workspace::AppGlobals>()
                .map(|g| g.workspace.entity_id());
            let Some(v) = ws.and_then(|id| {
                cx.try_global::<DenseViews>()
                    .and_then(|m| m.0.get(&id).cloned())
            }) else {
                return;
            };
            v.update(cx, |v, cx| f(v, window, cx));
        });
    });
}

/// Fixture scene `dense`: a pending gate on the blocked fixture-1, fixture-3
/// asks a question, four panels in a grid, fixture-1 in the details column,
/// a few status changes and a notification in the log (visual QA only).
pub fn fixture_scene(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let now = crate::orch::now_ms();
    for id in ["fixture-0", "fixture-1", "fixture-2", "fixture-3"] {
        w.open_terminal(id, false, window, cx);
    }
    w.arrange_grid(window, cx);
    // The details column follows the focused terminal: end on fixture-1.
    w.open_terminal("fixture-1", false, window, cx);
    let v = view(w, cx);
    v.update(cx, |v, cx| v.sync(w, cx));
    // Live changes after the log started.
    w.orch.merge_snapshot(vec![GateInfo {
        id: "g1".into(),
        title: "명령 실행 승인".into(),
        message: "$ cargo clippy -p pod-ui -- -D warnings".into(),
        task: Some("sample/task-000".into()),
        group: None,
        terminal: Some("fixture-1".into()),
        state: GateState::Pending,
        created_ms: now.saturating_sub(12_000),
        resolved_ms: None,
        note: None,
        expires_ms: None,
        run: None,
        item: None,
        options: Vec::new(),
        choice: None,
        requested_by: None,
        decided_by: None,
    }]);
    if let Some(s) = w
        .terminals
        .iter_mut()
        .find(|t| t.id == "fixture-3")
        .and_then(|t| t.status.as_mut())
    {
        s.state = AgentState::Blocked;
        s.kind = Some("question".into());
    }
    if let Some(t) = w.terminals.iter_mut().find(|t| t.id == "fixture-2") {
        t.live = Liveness::Exited {
            code: Some(1),
            killed: false,
        };
    }
    w.raise_toast(
        ToastKind::Success,
        "Worker 2 · 작업 완료",
        "1분 12초 · 토큰 18.2k",
        cx,
    );
    v.update(cx, |v, cx| {
        // Not `select`: that reads the workspace, which this scene is updating.
        v.selected = Some(Sel::Terminal("fixture-1".into()));
        v.sync(w, cx);
    });
    w.touch(cx);
}

#[cfg(test)]
mod tests;
