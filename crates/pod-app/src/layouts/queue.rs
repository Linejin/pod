//! 응답 대기열·관제 layout (presets 03 "응답 중심 관제" and 06 "관제형", docs/10 §D5.7).
//!
//! Left: the response queue — counts, a card for everything that needs the
//! user (pending gates with inline 이번만 허용 / 거부, blocked agents, failures),
//! then compact 작업 중 and 완료 sections. Right: the workspace's own dock, the
//! same entity every layout renders, so switching layouts keeps every panel,
//! terminal attachment and the dock arrangement as they were.
//!
//! 06 ([`Variant::Mission`], chosen by the `mission` style) moves the counts
//! into title-bar pills and adds an activity bar on the far left.
//!
//! The data is the workspace's (terminals, tasks, gates, the gate decision
//! path in [`crate::gates::resolve_gate`]). The queue keeps view state only
//! (selection, expanded sections, filter, dismissed failures) in one
//! [`QueueColumn`] per workspace, held in a global so it survives layout
//! switches. It re-renders on the workspace's change notifications; no timer.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::Sizable as _;
use gpui_kit::component::dock::PanelId;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{ControlState, Icon, Tone, semantic_button};
use podd::protocol::{GateDecision, GateInfo, GateState};

use crate::agent_kind::AgentKind;
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::qa_marks::QaId;
use crate::vm::{AgentState, Liveness, TaskVm, TerminalVm};
use crate::workspace::Workspace;

actions!(pod_queue, [FocusQueue]);

/// Moves keyboard focus to the queue list (from a terminal, the title bar …).
pub const FOCUS_QUEUE_KEY: &str = "ctrl-shift-q";
/// `--ui-fixture --fixture-scene queue`: a pending gate, a question and four open panels.
pub const FIXTURE_SCENE: &str = "queue";
/// Rows shown in 작업 중 / 완료 before "모두 보기".
pub const COMPACT_ROWS: usize = 5;

/// Product copy of this layout (kept here so layout modules merge independently).
pub mod copy {
    pub const TITLE: &str = "작업 관제";
    pub const LIST: &str = "응답 대기열";
    pub const WORKING: &str = "작업 중";
    pub const NEEDS: &str = "응답 필요";
    pub const DONE: &str = "완료";
    pub const FAILED: &str = "실패";
    pub const SEARCH: &str = "작업과 요청 검색";
    pub const ALLOW_ONCE: &str = "이번만 허용";
    pub const REJECT: &str = "거부";
    /// Accessible-name prefix of a gate option button (`선택: <option>: <title>`).
    pub const CHOOSE: &str = "선택";
    pub const OPEN: &str = "열기";
    pub const ANSWER: &str = "답하기";
    pub const DISMISS: &str = "확인";
    pub const SHOW_ALL: &str = "모두 보기";
    pub const SHOW_LESS: &str = "접기";
    pub const APPROVAL: &str = "승인 필요";
    pub const QUESTION: &str = "질문";
    pub const EMPTY: &str = "응답을 기다리는 요청이 없습니다";
    pub const NO_MATCH: &str = "검색과 맞는 항목이 없습니다";
    pub const DOCK_EMPTY: &str = "대기열에서 카드를 고르면 그 터미널이 여기에 열립니다";
    pub const NEW_TASK: &str = "새 작업";
    pub const FILTER: &str = "검색 열기/닫기";
    pub const ACTIVITY: &str = "활동 막대";
    pub const SHOW_QUEUE: &str = "응답 대기열 보이기/숨기기";
    pub const AGENTS: &str = "에이전트 트리";
    pub const FILES: &str = "파일";
    pub const REVIEW: &str = "변경 검토";
    pub const GATES: &str = "승인 요청";
    pub const SETTINGS: &str = "설정";
    pub const PILLS: &str = "작업 상태 요약";
}

// ───────────────────────────── state model ─────────────────────────────

/// Why a card is in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedKind {
    /// Permission request; decidable inline when it is a pending gate.
    Approval,
    /// The agent asked a question (answer in its terminal).
    Question,
    /// Blocked for another reason (auth, unknown).
    Other,
    /// Error state, lost, or a non-zero exit.
    Failed,
}

impl NeedKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Approval => copy::APPROVAL,
            Self::Question => copy::QUESTION,
            Self::Other => copy::NEEDS,
            Self::Failed => copy::FAILED,
        }
    }
    fn icon(self) -> Icon {
        match self {
            Self::Approval | Self::Other => Icon::ShieldQuestion,
            Self::Question => Icon::MessageCircleQuestion,
            Self::Failed => Icon::OctagonX,
        }
    }
    fn tone(self) -> Tone {
        if self == Self::Failed {
            Tone::Danger
        } else {
            Tone::Warning
        }
    }
}

/// One card of the 응답 필요 section.
#[derive(Clone, Debug, PartialEq)]
pub struct Need {
    /// Stable selection key: `gate:<id>`, `term:<id>` (blocked) or `fail:<id>`.
    pub key: String,
    pub kind: NeedKind,
    /// Pending gate behind this card: 이번만 허용 / 거부 decide it inline.
    pub gate: Option<String>,
    /// The gate's choices (v17): one approve button each, sending that choice.
    pub options: Vec<String>,
    /// Terminal that 열기 focuses.
    pub terminal: Option<String>,
    /// Task title (else the terminal label, else the gate title).
    pub title: String,
    /// `Worker 1 · 승인 필요`.
    pub meta: String,
    /// Gate message or task id.
    pub detail: String,
    /// Unix ms the request started (0: unknown).
    pub since: u64,
    /// The worker's agent (docs/10 §D5.11); none for a gate without a terminal.
    pub agent: Option<AgentKind>,
}

/// One compact row of 작업 중 / 완료.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: String,
    pub terminal: String,
    pub label: String,
    pub task: String,
    pub since: u64,
    pub agent: AgentKind,
}

/// Header counts (unfiltered).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub working: usize,
    /// Approvals, questions and other blocked requests (failures are counted apart).
    pub needs: usize,
    pub done: usize,
    pub failed: usize,
}

/// A section of the queue (title-bar pills jump to one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Needs,
    Failed,
    Working,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueModel {
    /// Requests first (oldest gate first, then blocked terminals), failures after.
    pub needs: Vec<Need>,
    pub working: Vec<Row>,
    pub done: Vec<Row>,
    pub counts: Counts,
}

fn failed(t: &TerminalVm) -> bool {
    t.status
        .as_ref()
        .is_some_and(|s| s.state == AgentState::Error)
        || t.live == Liveness::Lost
        || matches!(t.live, Liveness::Exited { code: Some(c), killed: false } if c != 0)
}

/// Build the queue from the workspace's data. `pending` are the pending gates
/// ([`crate::orch::OrchState::pending`]); `query` filters the lists (not the counts);
/// `dismissed` hides failure cards the user acknowledged (`fail:<id>` + since).
pub fn build(
    terminals: &[TerminalVm],
    tasks: &[TaskVm],
    pending: &[GateInfo],
    label: &dyn Fn(&TerminalVm) -> String,
    query: &str,
    dismissed: &HashSet<(String, u64)>,
) -> QueueModel {
    let task_title = |t: &TerminalVm| {
        t.task_id.as_deref().map(|id| {
            tasks
                .iter()
                .find(|k| k.id == id)
                .map(|k| k.title.clone())
                .unwrap_or_else(|| id.to_string())
        })
    };
    let mut m = QueueModel::default();
    let mut blocked = Vec::new();
    let mut failures = Vec::new();
    let agent = |t: &TerminalVm| crate::agent_kind::identify_in(t, tasks).kind;
    for t in terminals {
        let since = t.status.as_ref().map(|s| s.since).unwrap_or(0);
        if failed(t) {
            let badge = crate::orch::terminal_badge(t).label();
            failures.push(Need {
                key: format!("fail:{}", t.id),
                kind: NeedKind::Failed,
                gate: None,
                options: Vec::new(),
                terminal: Some(t.id.clone()),
                title: task_title(t).unwrap_or_else(|| label(t)),
                meta: format!("{} · {badge}", label(t)),
                detail: t.task_id.clone().unwrap_or_default(),
                since,
                agent: Some(agent(t)),
            });
            continue;
        }
        if !t.is_running() {
            continue;
        }
        let Some(s) = &t.status else { continue };
        let row = || Row {
            key: format!("term:{}", t.id),
            terminal: t.id.clone(),
            label: label(t),
            task: task_title(t).unwrap_or_default(),
            since,
            agent: agent(t),
        };
        match s.state {
            AgentState::Working => m.working.push(row()),
            AgentState::Done => m.done.push(row()),
            AgentState::Blocked => blocked.push(t),
            _ => {}
        }
    }
    m.counts.working = m.working.len();
    m.counts.done = m.done.len();
    m.counts.failed = failures.len();
    // Gates first; a gate raised from a blocked terminal is that terminal's card.
    let mut merged = HashSet::new();
    for g in pending.iter().filter(|g| g.state == GateState::Pending) {
        let term = g
            .terminal
            .clone()
            .filter(|id| terminals.iter().any(|t| &t.id == id))
            .or_else(|| {
                g.task
                    .as_deref()
                    .and_then(|k| tasks.iter().find(|t| t.id == k))
                    .and_then(|t| t.terminals.first().cloned())
            });
        let t = term
            .as_deref()
            .and_then(|id| terminals.iter().find(|t| t.id == id));
        if let Some(t) = t.filter(|t| blocked.iter().any(|b| b.id == t.id)) {
            merged.insert(t.id.clone());
        }
        let task = g
            .task
            .as_deref()
            .and_then(|k| tasks.iter().find(|t| t.id == k))
            .map(|t| t.title.clone())
            .or_else(|| t.and_then(task_title));
        m.needs.push(Need {
            key: format!("gate:{}", g.id),
            kind: NeedKind::Approval,
            gate: Some(g.id.clone()),
            options: g.options.clone(),
            terminal: term,
            title: task.unwrap_or_else(|| g.title.clone()),
            meta: format!(
                "{} · {}",
                t.map(label).unwrap_or_else(|| crate::orch::gate_origin(g)),
                g.title
            ),
            detail: g.message.clone(),
            since: g.created_ms,
            agent: t.map(agent),
        });
    }
    for t in blocked.into_iter().filter(|t| !merged.contains(&t.id)) {
        let kind = match t.status.as_ref().and_then(|s| s.kind.as_deref()) {
            Some("permission") => NeedKind::Approval,
            Some("question") => NeedKind::Question,
            _ => NeedKind::Other,
        };
        m.needs.push(Need {
            key: format!("term:{}", t.id),
            kind,
            gate: None,
            options: Vec::new(),
            terminal: Some(t.id.clone()),
            title: task_title(t).unwrap_or_else(|| label(t)),
            meta: format!("{} · {}", label(t), kind.label()),
            detail: t.task_id.clone().unwrap_or_default(),
            since: t.status.as_ref().map(|s| s.since).unwrap_or(0),
            agent: Some(agent(t)),
        });
    }
    m.counts.needs = m.needs.len();
    m.needs.extend(
        failures
            .into_iter()
            .filter(|n| !dismissed.contains(&(n.key.clone(), n.since))),
    );
    let q = query.trim().to_lowercase();
    if !q.is_empty() {
        let hit = |parts: &[&str]| parts.iter().any(|p| p.to_lowercase().contains(&q));
        // Agent kinds match too (`claude`, `codex`).
        let terms = |a: Option<&AgentKind>| a.map(|a| a.search_terms()).unwrap_or_default();
        m.needs
            .retain(|n| hit(&[&n.title, &n.meta, &n.detail, &terms(n.agent.as_ref())]));
        m.working
            .retain(|r| hit(&[&r.label, &r.task, &terms(Some(&r.agent))]));
        m.done
            .retain(|r| hit(&[&r.label, &r.task, &terms(Some(&r.agent))]));
    }
    m
}

impl QueueModel {
    /// Keyboard order (cards, then the visible rows of each section) with each item's terminal.
    pub fn keys(&self, all_working: bool, all_done: bool) -> Vec<(String, Option<String>)> {
        let cap = |all: bool, n: usize| if all { n } else { n.min(COMPACT_ROWS) };
        self.needs
            .iter()
            .map(|n| (n.key.clone(), n.terminal.clone()))
            .chain(
                self.working
                    .iter()
                    .take(cap(all_working, self.working.len()))
                    .map(|r| (r.key.clone(), Some(r.terminal.clone()))),
            )
            .chain(
                self.done
                    .iter()
                    .take(cap(all_done, self.done.len()))
                    .map(|r| (r.key.clone(), Some(r.terminal.clone()))),
            )
            .collect()
    }

    /// First item of a section, if any.
    pub fn first_of(&self, s: Section) -> Option<String> {
        match s {
            Section::Needs => self
                .needs
                .iter()
                .find(|n| n.kind != NeedKind::Failed)
                .map(|n| n.key.clone()),
            Section::Failed => self
                .needs
                .iter()
                .find(|n| n.kind == NeedKind::Failed)
                .map(|n| n.key.clone()),
            Section::Working => self.working.first().map(|r| r.key.clone()),
            Section::Done => self.done.first().map(|r| r.key.clone()),
        }
    }
}

/// Which preset structure: 03 (counts in the column) or 06 (pills + activity bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Classic,
    Mission,
}

impl Variant {
    /// 06's style (`mission`) gets the 06 structure; every other style the 03 one.
    pub fn of_style(style: &str) -> Self {
        if style == "mission" {
            Self::Mission
        } else {
            Self::Classic
        }
    }
    pub fn current(cx: &App) -> Self {
        cx.try_global::<crate::theme::Settings>()
            .map(|s| Self::of_style(&s.style))
            .unwrap_or(Self::Classic)
    }
}

/// `12초`, `3분`, `2시간` since `since` (ms); empty when unknown.
pub fn age(since: u64, now: u64) -> String {
    if since == 0 || since > now {
        return String::new();
    }
    match (now - since) / 1000 {
        s @ 0..60 => format!("{s}초"),
        s @ 60..3600 => format!("{}분", s / 60),
        s @ 3600..86_400 => format!("{}시간", s / 3600),
        s => format!("{}일", s / 86_400),
    }
}

fn model_of(w: &Workspace, query: &str, dismissed: &HashSet<(String, u64)>) -> QueueModel {
    build(
        &w.terminals,
        &w.tasks,
        &w.orch.pending(),
        &|t| w.label_of(t),
        query,
        dismissed,
    )
}

// ───────────────────────────── the column view ─────────────────────────────

/// View state of the queue column (one per workspace, kept across layout switches).
pub struct QueueColumn {
    ws: WeakEntity<Workspace>,
    pub(crate) focus: FocusHandle,
    filter: Entity<InputState>,
    pub(crate) selected: Option<String>,
    /// Index of the selection, so the next card is selected when one is decided.
    sel_index: usize,
    pub(crate) all_working: bool,
    pub(crate) all_done: bool,
    /// 06: queue column hidden by the activity bar.
    pub(crate) hidden: bool,
    /// 06: the search field is shown.
    filter_open: bool,
    busy: HashMap<String, GateDecision>,
    errors: HashMap<String, String>,
    dismissed: HashSet<(String, u64)>,
    scroll: ScrollHandle,
    /// Scroll child index of each item in the last frame.
    child_index: HashMap<String, usize>,
    _subs: Vec<Subscription>,
}

/// One queue column per workspace window.
#[derive(Default)]
struct QueueViews(HashMap<EntityId, Entity<QueueColumn>>);
impl Global for QueueViews {}

/// The workspace's queue column, created on first use.
pub(crate) fn column(window: &mut Window, cx: &mut Context<Workspace>) -> Entity<QueueColumn> {
    let id = cx.entity_id();
    if let Some(c) = cx
        .try_global::<QueueViews>()
        .and_then(|v| v.0.get(&id))
        .cloned()
    {
        return c;
    }
    let ws = cx.entity();
    let c = cx.new(|cx| QueueColumn::new(ws, window, cx));
    cx.default_global::<QueueViews>().0.insert(id, c.clone());
    c
}

/// The column if it was ever shown (title-bar pills of a workspace).
fn existing_column(id: EntityId, cx: &App) -> Option<Entity<QueueColumn>> {
    cx.try_global::<QueueViews>()
        .and_then(|v| v.0.get(&id))
        .cloned()
}

struct KeysBound;
impl Global for KeysBound {}

impl QueueColumn {
    fn new(ws: Entity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        if cx.try_global::<KeysBound>().is_none() {
            cx.bind_keys([KeyBinding::new(FOCUS_QUEUE_KEY, FocusQueue, None)]);
            // App level: the shortcut must work wherever focus is (workspace root,
            // terminal, title bar), and the root sits above any layout element.
            let app: &mut App = cx;
            app.on_action(|_: &FocusQueue, cx: &mut App| {
                let Some(window) = cx.active_window() else {
                    return;
                };
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let layout = cx
                            .try_global::<crate::theme::Settings>()
                            .map(|s| s.layout.clone())
                            .unwrap_or_default();
                        if layout != LayoutMode::Queue.id() {
                            return;
                        }
                        let Some(views) = cx.try_global::<QueueViews>() else {
                            return;
                        };
                        let cols: Vec<Entity<QueueColumn>> = views.0.values().cloned().collect();
                        // The column whose workspace lives in this window.
                        let ws = cx
                            .try_global::<crate::workspace::AppGlobals>()
                            .map(|g| g.workspace.entity_id());
                        let col = ws
                            .and_then(|id| views.0.get(&id).cloned())
                            .or_else(|| (cols.len() == 1).then(|| cols[0].clone()));
                        if let Some(col) = col {
                            col.update(cx, |q, cx| q.focus_list(window, cx));
                        }
                    });
                });
            });
            cx.set_global(KeysBound);
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(copy::SEARCH));
        let subs = vec![
            // Tasks, terminals, statuses and gates all notify the workspace: that is the only refresh.
            cx.observe(&ws, |_, _, cx| cx.notify()),
            cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            ws: ws.downgrade(),
            // A tracked handle must be a tab stop itself (`.tab_index()` is for element-owned ones).
            focus: cx.focus_handle().tab_stop(true),
            filter,
            selected: None,
            sel_index: 0,
            all_working: false,
            all_done: false,
            hidden: false,
            filter_open: false,
            busy: HashMap::new(),
            errors: HashMap::new(),
            dismissed: HashSet::new(),
            scroll: ScrollHandle::new(),
            child_index: HashMap::new(),
            _subs: subs,
        }
    }

    fn model(&self, cx: &App) -> QueueModel {
        let query = self.filter.read(cx).value().to_string();
        self.ws
            .upgrade()
            .map(|w| model_of(w.read(cx), &query, &self.dismissed))
            .unwrap_or_default()
    }

    /// Keyboard focus to the list (selecting the first item when nothing is).
    pub(crate) fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hidden = false;
        if self.selected.is_none() {
            self.selected = self
                .model(cx)
                .keys(self.all_working, self.all_done)
                .first()
                .map(|(k, _)| k.clone());
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Title-bar pill: show the section, select its first item, focus the list.
    pub(crate) fn focus_section(
        &mut self,
        s: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(k) = self.model(cx).first_of(s) {
            self.selected = Some(k);
        }
        self.focus_list(window, cx);
        if let Some(ix) = self.selected.as_ref().and_then(|k| self.child_index.get(k)) {
            self.scroll.scroll_to_item(*ix);
        }
    }

    fn select(
        &mut self,
        key: &str,
        terminal: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(key.to_string());
        if let Some(ix) = self.child_index.get(key) {
            self.scroll.scroll_to_item(*ix);
        }
        // Show the terminal if it already has a panel; keyboard focus stays here.
        if let (Some(t), Some(ws)) = (terminal, self.ws.upgrade()) {
            ws.update(cx, |w, cx| {
                if let Some(p) = w
                    .live_panels()
                    .into_iter()
                    .find(|p| p.read(cx).terminal_id == t)
                {
                    let pid = PanelId::from(p.entity_id());
                    w.dock.update(cx, |a, cx| a.select_panel(pid, window, cx));
                }
            });
        }
        cx.notify();
    }

    /// Select the item and focus its terminal in the dock (opening a panel when needed).
    fn open(
        &mut self,
        key: &str,
        terminal: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(key.to_string());
        if let (Some(t), Some(ws)) = (terminal.map(str::to_owned), self.ws.upgrade()) {
            ws.update(cx, |w, cx| w.open_terminal(&t, false, window, cx));
        }
        cx.notify();
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
        // The card goes away when podd answers: keep the keyboard in the queue.
        self.focus.focus(window, cx);
        let me = cx.entity().downgrade();
        let id = gate.clone();
        let params = podd::protocol::GateResolveParams {
            id: gate,
            decision,
            note: None,
            choice,
        };
        crate::gates::resolve_gate(&source, self.ws.clone(), params, cx, move |r, cx| {
            let _ = me.update(cx, |q, cx| {
                q.busy.remove(&id);
                if let Err(e) = r {
                    q.errors.insert(id, e.text);
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    fn dismiss(&mut self, key: &str, since: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.dismissed.insert((key.to_string(), since));
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn on_key(
        &mut self,
        e: &KeyDownEvent,
        keys: &[(String, Option<String>)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|s| keys.iter().position(|(k, _)| k == s));
        let next = match e.keystroke.key.as_str() {
            "down" => Some(at.map_or(0, |i| (i + 1).min(keys.len() - 1))),
            "up" => Some(at.map_or(0, |i| i.saturating_sub(1))),
            "home" => Some(0),
            "end" => Some(keys.len() - 1),
            "enter" | "space" => {
                if let Some(i) = at {
                    let (k, t) = keys[i].clone();
                    self.open(&k, t.as_deref(), window, cx);
                }
                cx.stop_propagation();
                return;
            }
            _ => None,
        };
        if let Some(i) = next {
            let (k, t) = keys[i].clone();
            self.select(&k, t.as_deref(), window, cx);
            cx.stop_propagation();
        }
    }
}

/// A section's "모두 보기" toggle.
type Activate = Box<dyn Fn(&mut Window, &mut App)>;

fn tc(hex: u32) -> Hsla {
    pod_ui::tokens::color(hex)
}

impl Render for QueueColumn {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let variant = Variant::current(cx);
        let model = self.model(cx);
        let keys = model.keys(self.all_working, self.all_done);
        // A decided / vanished item hands the selection to the one that took its place.
        if let Some(s) = &self.selected {
            match keys.iter().position(|(k, _)| k == s) {
                Some(i) => self.sel_index = i,
                None => {
                    self.selected = keys
                        .get(self.sel_index.min(keys.len().saturating_sub(1)))
                        .map(|(k, _)| k.clone())
                }
            }
        }
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        let mono = m.surface == pod_ui::themes::Surface::Mono;
        let rounded = matches!(
            m.surface,
            pod_ui::themes::Surface::Layered | pod_ui::themes::Surface::Islands
        );
        let now = crate::orch::now_ms();
        let list_focused = self.focus.contains_focused(window, cx);
        let me = cx.entity().downgrade();

        // ── header ──
        let new_task = {
            let ws = self.ws.clone();
            pod_ui::IconButton::new("queue-new-task", Icon::Plus, copy::NEW_TASK)
                .tooltip("새 작업 (Ctrl+N)")
                .render(mode, move |window, cx| {
                    let _ = ws.update(cx, |w, cx| w.open_new_task_dialog(window, cx));
                })
        };
        let stat = |n: usize, label: &'static str, fg: u32| {
            div()
                .qa_id(move || format!("queue-stat-{label}"))
                .flex()
                .flex_col()
                .min_w(px(56.))
                .flex_1()
                .child(
                    div()
                        .text_size(px(m.density.ui_font * 1.7))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tc(fg))
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(tc(tk.text_secondary))
                        .whitespace_nowrap()
                        .child(label),
                )
        };
        let search = Input::new(&self.filter).small().prefix(pod_ui::icon_sized(
            Icon::Search,
            14.,
            tk.text_muted,
        ));
        let header =
            match variant {
                Variant::Classic => div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .pt_2()
                    .pb_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_h(px(m.density.panel_header))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(copy::TITLE),
                            )
                            .child(new_task),
                    )
                    .child(
                        div()
                            .qa_id(|| "queue-counts".into())
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(stat(model.counts.working, copy::WORKING, tk.text_primary))
                            .child(stat(model.counts.needs, copy::NEEDS, tk.warning_fg))
                            .child(stat(model.counts.done, copy::DONE, tk.text_primary))
                            .child(stat(
                                model.counts.failed,
                                copy::FAILED,
                                if model.counts.failed > 0 {
                                    tk.danger_fg
                                } else {
                                    tk.text_primary
                                },
                            )),
                    )
                    .child(search)
                    .into_any_element(),
                Variant::Mission => {
                    let toggle =
                        pod_ui::IconButton::new("queue-filter", Icon::ListFilter, copy::FILTER)
                            .render(mode, {
                                let me = me.clone();
                                move |window, cx| {
                                    let _ = me.update(cx, |q, cx| {
                                        q.filter_open = !q.filter_open;
                                        if q.filter_open {
                                            q.filter.update(cx, |f, cx| f.focus(window, cx));
                                        }
                                        cx.notify();
                                    });
                                }
                            });
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .px_3()
                        .pt_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_h(px(m.density.panel_header))
                                .child(pod_ui::icon_sized(Icon::ShieldQuestion, 16., tk.warning_fg))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .whitespace_nowrap()
                                        .child(format!("{} {}", copy::NEEDS, model.counts.needs)),
                                )
                                .child(div().flex_1())
                                .child(new_task)
                                .child(toggle),
                        )
                        .when(self.filter_open, |d| d.child(search))
                        .into_any_element()
                }
            };

        // ── list ──
        let mut children: Vec<AnyElement> = Vec::new();
        let mut index = HashMap::new();
        let section_head = |id: &'static str,
                            icon: Icon,
                            fg: u32,
                            label: &'static str,
                            n: usize,
                            toggle: Option<(bool, Activate)>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .pt_3()
                .pb_1()
                .min_h(px(m.density.row))
                .child(pod_ui::icon_sized(icon, 14., fg))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_nowrap()
                        .child(format!("{label} {n}")),
                )
                .child(div().flex_1())
                .when_some(toggle, |d, (all, f)| {
                    let name = if all { copy::SHOW_LESS } else { copy::SHOW_ALL };
                    d.child(
                        div()
                            .id(SharedString::from(format!("{id}-all")))
                            .role(Role::Button)
                            .aria_label(SharedString::from(format!("{label} {name}")))
                            .tab_index(0)
                            .px_1()
                            .rounded(px(m.radius.control))
                            .text_xs()
                            .text_color(tc(tk.accent_fg))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|s| s.bg(tc(tk.bg_hover)))
                            .focus_visible(move |s| pod_ui::focus_style(s, mode))
                            .child(name)
                            .on_click(move |_, window, cx| f(window, cx)),
                    )
                })
                .into_any_element()
        };

        if variant == Variant::Classic {
            children.push(section_head(
                "queue-needs",
                Icon::ShieldQuestion,
                tk.warning_fg,
                copy::NEEDS,
                model.counts.needs,
                None,
            ));
        }
        if model.needs.is_empty() {
            let text = if self.filter.read(cx).value().trim().is_empty() {
                copy::EMPTY
            } else {
                copy::NO_MATCH
            };
            children.push(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(tc(tk.text_muted))
                    .child(text)
                    .into_any_element(),
            );
        }
        for n in &model.needs {
            index.insert(n.key.clone(), children.len());
            let selected = self.selected.as_ref() == Some(&n.key);
            children.push(self.render_card(n, selected, list_focused, variant, now, cx));
        }
        let rows = |this: &Self,
                    rows: &[Row],
                    all: bool,
                    icon: Icon,
                    fg: u32,
                    done: bool,
                    children: &mut Vec<AnyElement>,
                    index: &mut HashMap<String, usize>| {
            for r in rows
                .iter()
                .take(if all { rows.len() } else { COMPACT_ROWS })
            {
                index.insert(r.key.clone(), children.len());
                let selected = this.selected.as_ref() == Some(&r.key);
                let (key, term) = (r.key.clone(), r.terminal.clone());
                let me = me.clone();
                children.push(
                    div()
                        .id(SharedString::from(format!("queue-row-{}", r.key)))
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(format!(
                            "{}, {}, {}, {}",
                            r.label,
                            r.agent.accessible_name(),
                            if done { copy::DONE } else { copy::WORKING },
                            r.task
                        )))
                        .aria_selected(selected)
                        .mx_2()
                        .px_2()
                        .min_h(px(m.density.row))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(m.radius.control))
                        .when(selected, |d| d.bg(tc(tk.bg_selected)))
                        .when(selected && list_focused, |d| {
                            d.border_1().border_color(tc(tk.focus_ring))
                        })
                        .when(!selected, |d| d.hover(|s| s.bg(tc(tk.bg_hover))))
                        .cursor_pointer()
                        .child(pod_ui::icon_sized(icon, 14., fg))
                        .child(crate::agent_kind::mark_el(
                            &format!("queue-{}", r.key),
                            &r.agent,
                            14.,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .gap_1()
                                .text_sm()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .max_w(px(140.))
                                        .truncate()
                                        .child(r.label.clone()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(tc(tk.text_muted))
                                        .child(format!("· {}", r.task)),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(tc(tk.text_muted))
                                .child(age(r.since, now)),
                        )
                        .on_click(move |_, window, cx| {
                            let _ = me.update(cx, |q, cx| q.open(&key, Some(&term), window, cx));
                        })
                        .into_any_element(),
                );
            }
        };
        let toggle_of = |which: Section| -> Activate {
            let me = me.clone();
            Box::new(move |_: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |q, cx| {
                    match which {
                        Section::Working => q.all_working = !q.all_working,
                        _ => q.all_done = !q.all_done,
                    }
                    cx.notify();
                });
            })
        };
        let wtoggle = (model.working.len() > COMPACT_ROWS)
            .then(|| (self.all_working, toggle_of(Section::Working)));
        children.push(section_head(
            "queue-working",
            Icon::LoaderCircle,
            tk.info_fg,
            copy::WORKING,
            model.working.len(),
            wtoggle,
        ));
        rows(
            self,
            &model.working,
            self.all_working,
            Icon::LoaderCircle,
            tk.info_fg,
            false,
            &mut children,
            &mut index,
        );
        let dtoggle =
            (model.done.len() > COMPACT_ROWS).then(|| (self.all_done, toggle_of(Section::Done)));
        children.push(section_head(
            "queue-done",
            Icon::CircleCheck,
            tk.success_fg,
            copy::DONE,
            model.done.len(),
            dtoggle,
        ));
        rows(
            self,
            &model.done,
            self.all_done,
            Icon::CircleCheck,
            tk.success_fg,
            true,
            &mut children,
            &mut index,
        );
        children.push(div().h(px(8.)).into_any_element());
        self.child_index = index;

        let keys_for_handler = keys.clone();
        let list = div()
            .id("queue-list")
            .qa_id(|| "queue-list".into())
            .track_focus(&self.focus)
            .role(Role::List)
            .aria_label(copy::LIST)
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_key_down(cx.listener(move |q, e: &KeyDownEvent, window, cx| {
                q.on_key(e, &keys_for_handler, window, cx)
            }))
            .children(children);

        div()
            .id("queue-column")
            .qa_id(|| "queue-column".into())
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .when(!rounded, |d| d.bg(tc(tk.bg_chrome)))
            .when(!rounded && !mono, |d| {
                d.border_r(px(m.border_width))
                    .border_color(tc(tk.border_subtle))
            })
            .child(header)
            .child(list)
    }
}

impl QueueColumn {
    fn render_card(
        &self,
        n: &Need,
        selected: bool,
        list_focused: bool,
        variant: Variant,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        let mono = m.surface == pod_ui::themes::Surface::Mono;
        let (tone_fg, _) = n.kind.tone().colors(tk);
        let mono_font: SharedString = cx
            .try_global::<crate::theme::Settings>()
            .map(|s| s.terminal_font.clone())
            .unwrap_or_default()
            .into();
        let me = cx.entity().downgrade();
        let busy = n.gate.as_ref().and_then(|g| self.busy.get(g)).copied();
        // Buttons are tab stops only on the selected card (Tab goes list → its actions → dock).
        let show_actions = n.gate.is_some() || selected;
        // Pressing an action must not also "click" (open) the card under it.
        let mut actions = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .pt_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let stop = |d: Stateful<Div>| if selected { d } else { d.tab_stop(false) };
        if let Some(gate) = &n.gate {
            let state = |d: GateDecision| match busy {
                Some(b) if b == d => ControlState::Loading,
                Some(_) => ControlState::Disabled,
                None => ControlState::Ready,
            };
            // A gate with options (v17) is approved by picking one; otherwise 이번만 허용.
            let approve: Vec<(String, String, Option<String>)> = if n.options.is_empty() {
                vec![(
                    copy::ALLOW_ONCE.to_string(),
                    format!("queue-allow-{gate}"),
                    None,
                )]
            } else {
                n.options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        (
                            o.clone(),
                            format!("queue-allow-{gate}-{i}"),
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
                    format!("queue-reject-{gate}"),
                    None,
                    Tone::Danger,
                )));
            for (d, label, id, choice, tone) in buttons {
                let (me, gate) = (me.clone(), gate.clone());
                let name = match &choice {
                    Some(_) => format!("{}: {label}: {}", copy::CHOOSE, n.title),
                    None => format!("{label}: {}", n.title),
                };
                actions = actions.child(stop(
                    semantic_button(
                        SharedString::from(id),
                        label,
                        tone,
                        mode,
                        state(d),
                        move |window, cx| {
                            let _ = me.update(cx, |q, cx| {
                                q.decide(gate.clone(), d, choice.clone(), window, cx)
                            });
                        },
                    )
                    .aria_label(SharedString::from(name)),
                ));
            }
        }
        if let Some(t) = &n.terminal {
            let (me, key, t) = (me.clone(), n.key.clone(), t.clone());
            let label = if n.kind == NeedKind::Question {
                copy::ANSWER
            } else {
                copy::OPEN
            };
            actions = actions.child(stop(link_button(
                SharedString::from(format!("queue-open-{}", n.key)),
                label,
                format!("{label}: {}", n.title),
                move |window, cx| {
                    let _ = me.update(cx, |q, cx| q.open(&key, Some(&t), window, cx));
                },
            )));
        }
        if n.kind == NeedKind::Failed {
            let (me, key, since) = (me.clone(), n.key.clone(), n.since);
            actions = actions.child(stop(link_button(
                SharedString::from(format!("queue-dismiss-{}", n.key)),
                copy::DISMISS,
                format!("{}: {}", copy::DISMISS, n.title),
                move |window, cx| {
                    let _ = me.update(cx, |q, cx| q.dismiss(&key, since, window, cx));
                },
            )));
        }
        let error = n.gate.as_ref().and_then(|g| self.errors.get(g)).cloned();
        let (key, term) = (n.key.clone(), n.terminal.clone());
        let card = div()
            .id(SharedString::from(format!("queue-card-{}", n.key)))
            .qa_id({
                let k = n.key.clone();
                move || format!("queue-card-{k}")
            })
            .role(Role::ListItem)
            .aria_label(SharedString::from(match &n.agent {
                Some(a) => format!(
                    "{}, {}, {}, {}",
                    n.title,
                    n.kind.label(),
                    a.accessible_name(),
                    n.meta
                ),
                None => format!("{}, {}, {}", n.title, n.kind.label(), n.meta),
            }))
            .aria_selected(selected)
            .relative()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().pt(px(2.)).flex_shrink_0().child(pod_ui::icon_sized(
                        n.kind.icon(),
                        16.,
                        tone_fg,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(n.title.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(tc(tk.text_muted))
                            .child(age(n.since, now)),
                    ),
            )
            .child(
                div()
                    .pl(px(24.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(tc(tk.text_secondary))
                    .when_some(n.agent.as_ref(), |d, a| {
                        d.child(crate::agent_kind::mark_el(
                            &format!("queue-{}", n.key),
                            a,
                            14.,
                        ))
                    })
                    .child(div().flex_1().min_w_0().truncate().child(n.meta.clone())),
            )
            .when(!n.detail.is_empty(), |d| {
                let detail = div().pl(px(24.)).text_xs().text_color(tc(tk.text_muted));
                // A gate message is the request itself (a command): wrap it; a task id truncates.
                d.child(if n.gate.is_some() {
                    detail.font_family(mono_font).child(n.detail.clone())
                } else {
                    detail.truncate().child(n.detail.clone())
                })
            })
            .when(show_actions, |d| {
                let k = n.key.clone();
                d.child(
                    actions
                        .pl(px(24.))
                        .qa_id(move || format!("queue-actions-{k}")),
                )
            })
            .when_some(error, |d, e| {
                d.child(
                    div()
                        .pl(px(24.))
                        .text_xs()
                        .text_color(tc(tk.danger_fg))
                        .child(e),
                )
            })
            .on_click(move |_, window, cx| {
                let _ = me.update(cx, |q, cx| q.open(&key, term.as_deref(), window, cx));
            });
        let card = match variant {
            // 03: full-width rows split by hairlines; the selection is a filled row with an accent bar.
            Variant::Classic => card
                .when(!mono, |d| {
                    d.border_b(px(m.border_width))
                        .border_color(tc(tk.border_subtle))
                })
                .when(selected, |d| {
                    d.bg(tc(tk.bg_selected)).child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(3.))
                            .bg(tc(if list_focused {
                                tk.focus_ring
                            } else {
                                tk.accent_fg
                            })),
                    )
                })
                .when(!selected, |d| d.hover(|s| s.bg(tc(tk.bg_hover)))),
            // 06: separate cards with a status-colored left edge; selection gets a full status border.
            Variant::Mission => card
                .mx_2()
                .mb_2()
                .rounded(px(m.radius.card))
                .overflow_hidden()
                // Selection is the accent edge, not a fill: the success/danger button
                // fills must keep their contrast against the card in every style.
                .bg(tc(tk.bg_raised))
                .border_1()
                .border_color(tc(if selected {
                    tk.accent_fg
                } else {
                    tk.border_subtle
                }))
                .when(selected, |d| d.border_2())
                .when(!selected, |d| d.hover(|s| s.bg(tc(tk.bg_hover))))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(3.))
                        .bg(tc(tone_fg)),
                )
                .when(selected && list_focused, |d| {
                    d.border_color(tc(tk.focus_ring))
                }),
        };
        card.into_any_element()
    }
}

/// Text action (열기, 답하기, 확인): accent text, keyboard operable.
fn link_button(
    id: SharedString,
    label: &'static str,
    name: String,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let activate = std::rc::Rc::new(activate);
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
        .rounded(px(m.radius.control))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tc(tk.accent_fg))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(tc(tk.bg_hover)))
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .child(label)
        .on_click(move |_, window, cx| activate(window, cx))
        .on_key_down(move |e, window, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                key(window, cx);
                cx.stop_propagation();
            }
        })
}

// ───────────────────────────── layout module ─────────────────────────────

/// The 응답 대기열 layout module (03 and 06).
pub struct QueueLayout;

impl LayoutModule for QueueLayout {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Queue
    }

    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        render_body(ws, window, cx)
    }

    fn title_bar_extra(&self, ws: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
        (Variant::current(cx) == Variant::Mission).then(|| title_pills(ws, cx))
    }
    fn status_counts(&self, cx: &App) -> bool {
        Variant::current(cx) != Variant::Mission
    }
}

/// 06: status pills in the title bar; each jumps to its section.
fn title_pills(ws: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let c = model_of(ws, "", &HashSet::new()).counts;
    let tk = crate::theme::tokens();
    let mode = crate::theme::mode();
    let ws_id = cx.entity_id();
    let ws_weak = cx.entity().downgrade();
    let pill = |section: Section, icon: Icon, tone: Tone, label: &'static str, n: usize| {
        let (fg, bg) = tone.colors(tk);
        let ws_weak = ws_weak.clone();
        let go = std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
            let col = existing_column(ws_id, cx).or_else(|| {
                ws_weak
                    .upgrade()
                    .map(|w| w.update(cx, |_, cx| column(window, cx)))
            });
            if let Some(col) = col {
                col.update(cx, |q, cx| q.focus_section(section, window, cx));
            }
        });
        let key = go.clone();
        div()
            .id(SharedString::from(format!("queue-pill-{label}")))
            .role(Role::Button)
            .aria_label(SharedString::from(format!("{label} {n}개")))
            .tab_index(0)
            .flex_shrink_0()
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .bg(tc(bg))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(tc(fg))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|s| s.border_1().border_color(tc(fg)))
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .child(pod_ui::icon_sized(icon, 14., fg))
            .child(format!("{label} {n}"))
            .on_click(move |_, window, cx| go(window, cx))
            .on_key_down(move |e, window, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    key(window, cx);
                    cx.stop_propagation();
                }
            })
    };
    div()
        .id("queue-pills")
        .qa_id(|| "queue-pills".into())
        .role(Role::Group)
        .aria_label(copy::PILLS)
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .child(pill(
            Section::Needs,
            Icon::ShieldQuestion,
            Tone::Warning,
            copy::NEEDS,
            c.needs,
        ))
        .child(pill(
            Section::Failed,
            Icon::OctagonX,
            Tone::Danger,
            copy::FAILED,
            c.failed,
        ))
        .child(pill(
            Section::Working,
            Icon::LoaderCircle,
            Tone::Info,
            copy::WORKING,
            c.working,
        ))
        .child(pill(
            Section::Done,
            Icon::CircleCheck,
            Tone::Success,
            copy::DONE,
            c.done,
        ))
        .into_any_element()
}

/// 06: activity bar on the far left (queue toggle, agent tree, files, review, gates; settings at the bottom).
fn activity_bar(column: &Entity<QueueColumn>, cx: &mut Context<Workspace>) -> AnyElement {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let ws = cx.entity().downgrade();
    let shown = !column.read(cx).hidden;
    let col = column.downgrade();
    let toggle =
        div()
            .rounded(px(m.radius.control))
            .when(shown, |d| d.bg(tc(tk.bg_selected)))
            .child(
                pod_ui::IconButton::new("activity-queue", Icon::LayoutGrid, copy::SHOW_QUEUE)
                    .render(mode, move |window, cx| {
                        let _ = col.update(cx, |q, cx| {
                            q.hidden = !q.hidden;
                            if !q.hidden {
                                q.focus.focus(window, cx);
                            }
                            cx.notify();
                        });
                    }),
            );
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
        .id("activity-bar")
        .qa_id(|| "activity-bar".into())
        .role(Role::Toolbar)
        .aria_label(copy::ACTIVITY)
        .w(px(pod_ui::hit_area() + 16.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .py_2()
        .bg(tc(tk.bg_chrome))
        .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
            d.border_r(px(m.border_width))
                .border_color(tc(tk.border_subtle))
        })
        .child(toggle)
        .child(item(
            "activity-agents",
            Icon::Bot,
            copy::AGENTS,
            "에이전트 트리 (Ctrl+Shift+E)",
            |w, window, cx| w.open_agent_tree(window, cx),
        ))
        .child(item(
            "activity-files",
            Icon::FileText,
            copy::FILES,
            "파일 (Ctrl+Shift+P)",
            |w, window, cx| w.quick_open_files(false, window, cx),
        ))
        .child(item(
            "activity-review",
            Icon::GitCompareArrows,
            copy::REVIEW,
            "변경 검토 (Ctrl+Shift+R)",
            |_, window, cx| window.dispatch_action(Box::new(crate::workspace::OpenReview), cx),
        ))
        .child(item(
            "activity-gates",
            Icon::ShieldQuestion,
            copy::GATES,
            "승인 요청 (Ctrl+Shift+I)",
            |w, window, cx| w.open_gates(window, cx),
        ))
        .child(div().flex_1())
        .child(item(
            "activity-settings",
            Icon::Settings,
            copy::SETTINGS,
            "설정 (Ctrl+,)",
            |w, window, cx| w.open_settings(window, cx),
        ))
        .into_any_element()
}

/// The window body: [activity bar] · queue column (+ toasts) · dock.
pub fn render_body(
    ws: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let column = column(window, cx);
    let variant = Variant::current(cx);
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
    let hidden = variant == Variant::Mission && column.read(cx).hidden;
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
    let toasts = ws.render_toasts(cx).into_any_element();
    let queue_width = m.density.sidebar + 40.;
    div()
        .id("queue-layout")
        .flex()
        .flex_1()
        .min_h_0()
        .when(variant == Variant::Mission, |d| d.child(activity_bar(&column, cx)))
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
                .when(!hidden, |d| {
                    d.child(div().w(px(queue_width)).flex_shrink_0().flex().flex_col().gap(px(gap)).child(island(div().flex_1().min_h_0()).child(column.clone())))
                })
                .child(
                    island(div())
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .relative()
                        .when_some(banner, |d, b| d.child(div().px_2().pt_2().child(pod_ui::banner(Tone::Warning, crate::theme::mode(), b, None).text_xs())))
                        .when_some(ws.render_hook_banner(cx), |d, b| d.child(b))
                        .child(div().flex_1().min_h_0().child(ws.dock.clone()))
                        .when(empty, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px_4()
                                    .text_sm()
                                    .text_color(rgb(tk.text_muted))
                                    .child(pod_ui::Frame::new("queue-dock-empty", pod_ui::FrameKind::Empty, copy::DOCK_EMPTY, crate::strings::YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_).render(crate::theme::mode(), [])),
                            )
                        })
                        // Toasts sit bottom-left over the dock (the queue column is full height).
                        .child(div().absolute().left_2().bottom_2().w(px(queue_width.min(320.))).child(toasts)),
                ),
        )
        .into_any_element()
}

/// Fixture scene `queue`: a pending gate on the blocked fixture-1, a question on
/// fixture-3, four panels in a grid, the gate card selected (visual QA only).
pub fn fixture_scene(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let now = crate::orch::now_ms();
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
        s.kind = Some("question".into());
    }
    for id in ["fixture-0", "fixture-1", "fixture-2", "fixture-3"] {
        w.open_terminal(id, false, window, cx);
    }
    w.arrange_grid(window, cx);
    w.touch(cx);
    let col = column(window, cx);
    col.update(cx, |q, cx| {
        q.selected = Some("gate:g1".into());
        q.focus_list(window, cx);
    });
}

#[cfg(test)]
mod tests;
