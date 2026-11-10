//! 칸반 보드 (preset 12, docs/10 §D5.5): one card per agent/worker terminal in
//! three columns — 작업 중 / 응답 필요 / 완료 — and a terminal drawer below.
//!
//! - **Columns follow the agent.** A card's column comes only from the status
//!   podd reports (`StatusVm`) and the terminal's lifecycle ([`column_of`]).
//!   Failures (agent `error`, non-zero exit, lost holder) sit in 응답 필요 with
//!   a danger stripe: they need the user just like a blocked agent.
//! - **Dragging never changes state.** A card can be dropped on a card of its
//!   own column (reorder) or on its column header (pin to the top). Drops on
//!   another column are refused (`can_drop`); the handle's tooltip says why.
//!   Alt+↑/↓ and Alt+Home do the same from the keyboard.
//! - **No forked state.** Cards are built every frame from the workspace's
//!   terminals, tasks and pending gates; actions go through the existing paths
//!   (`gate.resolve`, `terminal.run`, terminal panels). The board keeps only
//!   view state: order, pins, selection, filters, drawer and a last-line cache.
//! - **No polling.** The last output line is read (`terminal.read`, 1 line)
//!   for visible cards when their status key changes.
//! - **Switching is live.** The dock is not touched: terminal panels the
//!   workbench has open are shown in the drawer as they are (same entity, same
//!   attach), so switching back finds the dock exactly as it was. A terminal
//!   without a panel gets a drawer-owned one, dropped when the layout changes.
//!   Panels opened while the board is shown (settings, review …) appear in the
//!   drawer's "패널" view, which renders the dock itself.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::Button;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use podd::protocol::{self as p, GateDecision, GateInfo};

use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::panel::TerminalPanel;
use crate::qa_marks::QaId;
use crate::source::RunSpec;
use crate::vm::{AgentState, Liveness, TaskVm, TerminalVm};
use crate::workspace::{ToastKind, Workspace};

actions!(pod_kanban, [SwitchRegion]);

/// Key context of the board (F6 is bound only inside it).
pub const KEY_CONTEXT: &str = "PodKanban";

/// Korean UI text of the board (kept here so the layout stays one file).
pub mod s {
    pub const ALL_WORK: &str = "모든 작업";
    pub const WORKING: &str = "작업 중";
    pub const NEEDS_YOU: &str = "응답 필요";
    pub const DONE: &str = "완료";
    pub const APPROVE: &str = "승인";
    pub const REJECT: &str = "거부";
    pub const LOG: &str = "로그";
    pub const ANSWER: &str = "응답하기";
    pub const RERUN: &str = "다시 실행";
    pub const PINNED: &str = "고정";
    pub const NO_TASK: &str = "작업 없음";
    pub const NO_CARDS: &str = "카드 없음";
    pub const NO_OUTPUT: &str = "출력 없음";
    pub const ALL_REPOS: &str = "모든 저장소";
    pub const ALL_AGENTS: &str = "모든 에이전트";
    pub const REPO: &str = "저장소";
    pub const AGENT: &str = "에이전트";
    pub const CLEAR_FILTERS: &str = "필터 지우기";
    pub const DRAWER: &str = "터미널 서랍";
    pub const DRAWER_TERMINAL: &str = "터미널";
    pub const DRAWER_PANELS: &str = "패널";
    pub const DRAWER_EMPTY: &str = "카드를 고르고 Enter를 누르면 여기에 터미널이 열립니다.";
    pub const DRAWER_GROW: &str = "서랍 크게";
    pub const DRAWER_SHRINK: &str = "서랍 작게";
    pub const DRAWER_CLOSE: &str = "서랍 닫기";
    pub const DRAWER_OPEN: &str = "서랍 열기";
    pub const HINT: &str = "끌어서 순서·고정 정리 · 상태는 에이전트가 보고 · F6 보드↔서랍";
    pub const DRAG_TIP: &str = "끌어서 이 열 안에서 순서를 바꾸거나 열 머리에 놓아 맨 위에 고정합니다 (Alt+↑/↓, Alt+Home). \
        열(상태)은 에이전트가 보고하므로 끌어서 바꿀 수 없습니다.";
    pub const NO_GATE_TIP: &str = "승인 요청(gate)이 없는 질문입니다. 터미널에서 직접 답하세요.";
    pub const COULD_NOT_RERUN: &str = "다시 실행하지 못했습니다";
}

// ───────────────────────────── model ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Working,
    NeedsYou,
    Done,
}

impl Column {
    pub const ALL: [Self; 3] = [Self::Working, Self::NeedsYou, Self::Done];
    pub fn index(self) -> usize {
        match self {
            Self::Working => 0,
            Self::NeedsYou => 1,
            Self::Done => 2,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => s::WORKING,
            Self::NeedsYou => s::NEEDS_YOU,
            Self::Done => s::DONE,
        }
    }
    fn icon(self) -> pod_ui::Icon {
        match self {
            Self::Working => pod_ui::Icon::LoaderCircle,
            Self::NeedsYou => pod_ui::Icon::ShieldQuestion,
            Self::Done => pod_ui::Icon::CircleCheck,
        }
    }
    fn fg(self, t: &pod_ui::Tokens) -> u32 {
        match self {
            Self::Working => t.info_fg,
            Self::NeedsYou => t.warning_fg,
            Self::Done => t.success_fg,
        }
    }
}

/// The column a terminal belongs in, and whether it failed — from the agent's
/// reported state and the process lifecycle only.
pub fn column_of(t: &TerminalVm) -> (Column, bool) {
    match t.live {
        Liveness::Exited { killed: true, .. }
        | Liveness::Exited {
            code: Some(0) | None,
            ..
        } => (Column::Done, false),
        Liveness::Exited { .. } | Liveness::Lost => (Column::NeedsYou, true),
        Liveness::Running => match t.status.as_ref().map(|s| s.state) {
            None | Some(AgentState::Working | AgentState::Unknown) => (Column::Working, false),
            Some(AgentState::Blocked) => (Column::NeedsYou, false),
            Some(AgentState::Error) => (Column::NeedsYou, true),
            Some(AgentState::Done | AgentState::Idle | AgentState::Exited) => (Column::Done, false),
        },
    }
}

/// One terminal as the board shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: String,
    /// `Claude · 이름`.
    pub title: String,
    pub task: Option<String>,
    pub task_title: String,
    pub repo: Option<String>,
    /// Filter value: the agent's name (`Claude`), or the program for a plain shell.
    pub agent: String,
    /// Which service runs here (docs/10 §D5.11): the mark before the title.
    pub kind: crate::agent_kind::AgentKind,
    pub column: Column,
    pub failed: bool,
    /// The status label (`승인 필요`, `실패 종료 (1)` …), always shown.
    pub status: String,
    /// The pending gate this card can approve or reject.
    pub gate: Option<GateInfo>,
    /// The process ended: `다시 실행` starts the same command again.
    pub rerun: bool,
    /// Last-line cache key: changes when the status or lifecycle changes.
    pub line_key: String,
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// Cards for every terminal, in the workspace's order.
pub fn build_cards(
    terminals: &[TerminalVm],
    tasks: &[TaskVm],
    pending: &[GateInfo],
    label: impl Fn(&TerminalVm) -> String,
) -> Vec<Card> {
    let mut gate_of: HashMap<&str, &GateInfo> = HashMap::new();
    for g in pending {
        if let Some(t) = g
            .terminal
            .as_deref()
            .filter(|t| terminals.iter().any(|x| x.id == *t))
        {
            gate_of.entry(t).or_insert(g);
        } else if let Some(task) = g.task.as_deref()
            && let Some(first) = terminals
                .iter()
                .find(|x| x.task_id.as_deref() == Some(task))
        {
            gate_of.entry(first.id.as_str()).or_insert(g);
        }
    }
    terminals
        .iter()
        .map(|t| {
            let task = t
                .task_id
                .as_deref()
                .and_then(|id| tasks.iter().find(|k| k.id == id));
            let (column, failed) = column_of(t);
            let kind = crate::agent_kind::identify(t, task).kind;
            let agent = if kind.is_agent() {
                kind.name()
            } else {
                capitalized(&t.program)
            };
            let status = t.status.as_ref();
            Card {
                id: t.id.clone(),
                title: format!("{agent} · {}", label(t)),
                task: t.task_id.clone(),
                task_title: task
                    .map(|k| k.title.clone())
                    .unwrap_or_else(|| s::NO_TASK.to_string()),
                repo: task.and_then(|k| k.repo.clone()),
                agent,
                kind,
                column,
                failed,
                status: crate::orch::terminal_badge(t).label(),
                gate: gate_of.get(t.id.as_str()).map(|g| (*g).clone()),
                rerun: !t.is_running(),
                line_key: format!(
                    "{:?}|{:?}|{}|{}",
                    t.live,
                    status.map(|s| s.state),
                    status.map_or(0, |s| s.since),
                    status.map_or(0, |s| s.turn)
                ),
            }
        })
        .collect()
}

/// A refused board edit, with the reason the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The drop target is in another column: columns are the agent's state.
    StateIsTheAgents,
    Unknown,
}

/// Keyboard movement on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

/// The board's own view state (never the terminals' state).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardModel {
    /// Manual order (drag / Alt+↑↓); cards not listed keep the natural order after it.
    pub order: Vec<String>,
    /// Cards pinned to the top of their column.
    pub pinned: HashSet<String>,
    /// The card with the keyboard selection.
    pub selected: Option<String>,
    /// The card shown in the drawer.
    pub opened: Option<String>,
    pub repo: Option<String>,
    pub agent: Option<String>,
}

impl BoardModel {
    pub fn visible(&self, c: &Card) -> bool {
        self.repo
            .as_ref()
            .is_none_or(|r| c.repo.as_ref() == Some(r))
            && self.agent.as_ref().is_none_or(|a| &c.agent == a)
    }

    /// The filtered cards per column: pinned first, then the manual order, then the natural one.
    pub fn columns(&self, cards: &[Card]) -> [Vec<Card>; 3] {
        let rank: HashMap<&str, usize> = self
            .order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let mut cols: [Vec<(usize, Card)>; 3] = Default::default();
        for (natural, c) in cards.iter().enumerate().filter(|(_, c)| self.visible(c)) {
            cols[c.column.index()].push((natural, c.clone()));
        }
        cols.map(|mut v| {
            v.sort_by_key(|(natural, c)| {
                (
                    !self.pinned.contains(&c.id),
                    rank.get(c.id.as_str()).copied().unwrap_or(usize::MAX),
                    *natural,
                )
            });
            v.into_iter().map(|(_, c)| c).collect()
        })
    }

    /// `id` placed at `index` of its column's list (without it), pinned or not.
    fn place(&mut self, column: &[Card], id: &str, index: usize, pinned: bool) {
        let mut ids: Vec<String> = column
            .iter()
            .map(|c| c.id.clone())
            .filter(|c| c != id)
            .collect();
        ids.insert(index.min(ids.len()), id.to_string());
        self.order.retain(|x| !ids.contains(x));
        self.order.extend(ids);
        if pinned {
            self.pinned.insert(id.to_string());
        } else {
            self.pinned.remove(id);
        }
    }

    fn column_with<'a>(
        &self,
        cols: &'a [Vec<Card>; 3],
        id: &str,
    ) -> Option<(usize, &'a Vec<Card>, usize)> {
        cols.iter()
            .enumerate()
            .find_map(|(ci, col)| col.iter().position(|c| c.id == id).map(|i| (ci, col, i)))
    }

    /// Drop `id` on `target` (before it) or, with `None`, at the end of its column.
    /// The card takes the target's pinned state. Another column is refused.
    pub fn reorder(
        &mut self,
        cards: &[Card],
        id: &str,
        target: Option<&str>,
    ) -> Result<(), Refused> {
        let cols = self.columns(cards);
        let (ci, col, _) = self.column_with(&cols, id).ok_or(Refused::Unknown)?;
        match target {
            Some(t) if t == id => Ok(()),
            Some(t) => {
                let (ti, _, _) = self.column_with(&cols, t).ok_or(Refused::Unknown)?;
                if ti != ci {
                    return Err(Refused::StateIsTheAgents);
                }
                let without: Vec<&Card> = col.iter().filter(|c| c.id != id).collect();
                let index = without
                    .iter()
                    .position(|c| c.id == t)
                    .unwrap_or(without.len());
                let pinned = self.pinned.contains(t);
                self.place(col, id, index, pinned);
                Ok(())
            }
            None => {
                self.place(col, id, usize::MAX, false);
                Ok(())
            }
        }
    }

    /// Drop on a column header: pinned at the very top. Another column is refused.
    pub fn pin_top(&mut self, cards: &[Card], id: &str, column: Column) -> Result<(), Refused> {
        let cols = self.columns(cards);
        let (ci, col, _) = self.column_with(&cols, id).ok_or(Refused::Unknown)?;
        if ci != column.index() {
            return Err(Refused::StateIsTheAgents);
        }
        self.place(col, id, 0, true);
        Ok(())
    }

    /// Alt+↑/↓: swap with the neighbour, taking its pinned state.
    pub fn move_by(&mut self, cards: &[Card], id: &str, down: bool) -> Result<(), Refused> {
        let cols = self.columns(cards);
        let (_, col, i) = self.column_with(&cols, id).ok_or(Refused::Unknown)?;
        let j = if down { i + 1 } else { i.wrapping_sub(1) };
        let Some(neighbour) = col.get(j) else {
            return Ok(());
        };
        let pinned = self.pinned.contains(&neighbour.id);
        self.place(col, id, j, pinned);
        Ok(())
    }

    /// The card a key moves the selection to (`None`: nowhere to go).
    pub fn nav(&self, cols: &[Vec<Card>; 3], from: Option<&str>, key: Nav) -> Option<String> {
        let first = || cols.iter().find_map(|c| c.first()).map(|c| c.id.clone());
        let Some((ci, col, i)) = from.and_then(|id| self.column_with(cols, id)) else {
            return first();
        };
        let side = |step: isize| {
            let mut c = ci as isize + step;
            while (0..3).contains(&c) {
                let target = &cols[c as usize];
                if !target.is_empty() {
                    return Some(target[i.min(target.len() - 1)].id.clone());
                }
                c += step;
            }
            None
        };
        match key {
            Nav::Up => i.checked_sub(1).map(|j| col[j].id.clone()),
            Nav::Down => col.get(i + 1).map(|c| c.id.clone()),
            Nav::Home => col.first().map(|c| c.id.clone()),
            Nav::End => col.last().map(|c| c.id.clone()),
            Nav::Left => side(-1),
            Nav::Right => side(1),
        }
    }

    /// Keep the selection on a visible card (first of the board otherwise).
    pub fn settle_selection(&mut self, cols: &[Vec<Card>; 3]) {
        if self
            .selected
            .as_deref()
            .is_none_or(|id| self.column_with(cols, id).is_none())
        {
            self.selected = cols.iter().find_map(|c| c.first()).map(|c| c.id.clone());
        }
    }
}

/// Repo and agent choices for the filters, sorted.
pub fn filter_choices(cards: &[Card]) -> (Vec<String>, Vec<String>) {
    let mut repos: Vec<String> = cards.iter().filter_map(|c| c.repo.clone()).collect();
    let mut agents: Vec<String> = cards.iter().map(|c| c.agent.clone()).collect();
    for v in [&mut repos, &mut agents] {
        v.sort();
        v.dedup();
    }
    (repos, agents)
}

/// The card's accessible name: everything the card shows, in reading order.
pub fn card_name(c: &Card, line: Option<&str>, pinned: bool) -> String {
    let mut name = format!(
        "{}, {}: {}, 작업: {}",
        c.title,
        c.column.label(),
        c.status,
        c.task_title
    );
    if let Some(g) = &c.gate {
        name.push_str(&format!(", 승인 요청: {}", g.title));
    }
    if let Some(l) = line.filter(|l| !l.is_empty()) {
        name.push_str(&format!(", 마지막 줄: {l}"));
    }
    if pinned {
        name.push_str(", 고정됨");
    }
    name
}

// ───────────────────────────── module ─────────────────────────────

/// The layout module (registered in [`crate::layout_mode::LayoutRegistry::new`]).
pub struct Kanban;

impl LayoutModule for Kanban {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Kanban
    }
    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        if cx.try_global::<KanbanKeys>().is_none() {
            cx.bind_keys([KeyBinding::new("f6", SwitchRegion, Some(KEY_CONTEXT))]);
            cx.set_global(KanbanKeys);
        }
        let board = match cx.try_global::<BoardGlobal>().and_then(|g| g.0.upgrade()) {
            Some(b) => b,
            None => {
                let weak = cx.entity().downgrade();
                let b = cx.new(|cx| Board::new(weak, window, cx));
                // The workspace owns the board; the global only finds it again.
                ws_board_slot(cx).replace(Some(b.clone()));
                cx.set_global(BoardGlobal(b.downgrade()));
                b
            }
        };
        let tk = crate::theme::tokens();
        let banner = match &ws.conn {
            crate::vm::ConnState::Reconnecting { attempt, error } => Some(
                crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR; attempt,error;),
            ),
            crate::vm::ConnState::Connecting => {
                Some(crate::strings::CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED.to_string())
            }
            crate::vm::ConnState::Connected { .. } => None,
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .relative()
            .bg(rgb(tk.bg_canvas))
            .when_some(banner, |d, b| {
                d.child(div().px_2().pt_2().child(
                    pod_ui::banner(pod_ui::Tone::Warning, crate::theme::mode(), b, None).text_xs(),
                ))
            })
            .when_some(ws.render_hook_banner(cx), |d, b| d.child(b))
            .child(div().flex().flex_col().flex_1().min_h_0().child(board))
            .child(
                div()
                    .absolute()
                    .top(px(52.))
                    .right(px(16.))
                    .w(px(340.))
                    .child(ws.render_toasts(cx)),
            )
            .into_any_element()
    }
}

struct KanbanKeys;
impl Global for KanbanKeys {}
struct BoardGlobal(WeakEntity<Board>);
impl Global for BoardGlobal {}
struct BoardSlot(Rc<RefCell<Option<Entity<Board>>>>);
impl Global for BoardSlot {}

/// Strong owner of the board, per app (the board lives as long as the app).
fn ws_board_slot(cx: &mut App) -> Rc<RefCell<Option<Entity<Board>>>> {
    if cx.try_global::<BoardSlot>().is_none() {
        cx.set_global(BoardSlot(Rc::default()));
    }
    cx.global::<BoardSlot>().0.clone()
}

#[cfg(test)]
/// The board of this app, if the kanban layout was shown once (tests).
pub fn board(cx: &App) -> Option<Entity<Board>> {
    cx.try_global::<BoardGlobal>().and_then(|g| g.0.upgrade())
}

// ───────────────────────────── view ─────────────────────────────

/// Payload of a card drag.
#[derive(Clone)]
struct DragCard {
    id: String,
    column: Column,
    title: SharedString,
}

struct DragGhost(SharedString);
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        div()
            .px_3()
            .py_2()
            .rounded(px(m.radius.card))
            .bg(rgb(t.bg_raised))
            .border_2()
            .border_color(rgb(t.accent_fg))
            .text_color(rgb(t.text_primary))
            .shadow(pod_ui::tokens::elevation(2, crate::theme::mode()))
            .child(self.0.clone())
    }
}

pub struct Board {
    ws: WeakEntity<Workspace>,
    pub model: BoardModel,
    /// Focus of the board when it has no card.
    focus: FocusHandle,
    card_focus: HashMap<String, FocusHandle>,
    scroll: [UniformListScrollHandle; 3],
    /// Last output line per terminal: (status key it was read for, line).
    lines: HashMap<String, (String, Option<String>)>,
    reading: HashSet<String>,
    /// Gates being resolved (a second press is ignored; the buttons stay focusable).
    busy: HashSet<String>,
    pub drawer_open: bool,
    drawer_max: bool,
    /// The drawer shows the dock (other panels) instead of the opened card.
    pub show_dock: bool,
    /// Drawer panel for a terminal the workbench has no panel for.
    own_panel: Option<Entity<TerminalPanel>>,
    dock_signature: usize,
    seen_focus: Option<String>,
    /// The layout was just switched to (or the app started in it): place focus once.
    entered: bool,
    /// The settings name this layout (to see the switch *to* it).
    shown: bool,
    _subs: Vec<Subscription>,
}

impl Board {
    fn new(ws: WeakEntity<Workspace>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Leaving the layout drops the drawer-owned panel (its attach); the
        // dock and its panels were never touched.
        let subs = vec![
            cx.observe_global::<crate::theme::Settings>(|b: &mut Self, cx| {
                if cx.global::<crate::theme::Settings>().layout != LayoutMode::Kanban.id() {
                    b.own_panel = None;
                    b.entered = false;
                } else if !b.shown {
                    b.entered = true;
                }
                b.shown = cx.global::<crate::theme::Settings>().layout == LayoutMode::Kanban.id();
            }),
        ];
        Self {
            ws,
            model: BoardModel::default(),
            focus: cx.focus_handle().tab_stop(true),
            card_focus: HashMap::new(),
            scroll: Default::default(),
            lines: HashMap::new(),
            reading: HashSet::new(),
            busy: HashSet::new(),
            drawer_open: true,
            drawer_max: false,
            show_dock: false,
            own_panel: None,
            dock_signature: usize::MAX,
            seen_focus: None,
            entered: true,
            shown: true,
            _subs: subs,
        }
    }

    #[cfg(test)]
    /// The drawer-owned panel, if any (tests).
    pub fn own_panel(&self) -> Option<Entity<TerminalPanel>> {
        self.own_panel.clone()
    }

    pub fn card_focus(&mut self, id: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.card_focus
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    pub fn cards(&self, cx: &App) -> Vec<Card> {
        let Some(ws) = self.ws.upgrade() else {
            return Vec::new();
        };
        let w = ws.read(cx);
        build_cards(&w.terminals, &w.tasks, &w.orch.pending(), |t| w.label_of(t))
    }

    /// Panels other than terminals the dock holds (a rise switches the drawer to them).
    fn dock_signature(w: &Workspace, cx: &App) -> usize {
        let live = |p: &Option<WeakEntity<crate::chrome::ChromePanel>>| {
            p.as_ref().is_some_and(|p| p.upgrade().is_some()) as usize
        };
        live(&w.settings_panel)
            + live(&w.inspector_panel)
            + w.live_reviews().len()
            + w.live_editors().len()
            + w.orch.live_groups().len()
            + w.orch
                .gates_panel
                .as_ref()
                .is_some_and(|p| p.upgrade().is_some()) as usize
            + w.orch
                .tree_panel
                .as_ref()
                .is_some_and(|p| p.upgrade().is_some()) as usize
            + crate::mods_ui::page_open(cx) as usize
            + crate::sessions_ui::page_open(cx) as usize
    }

    /// The panel the drawer shows for the opened card: the workbench's own
    /// panel when it has one (no new attach), else a drawer-owned one.
    fn drawer_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalPanel>> {
        let id = self.model.opened.clone()?;
        let ws = self.ws.upgrade()?;
        let own = self.own_panel.as_ref().map(|p| p.entity_id());
        let existing = ws
            .read(cx)
            .live_panels()
            .into_iter()
            .find(|p| Some(p.entity_id()) != own && p.read(cx).terminal_id == id);
        if let Some(p) = existing {
            self.own_panel = None;
            return Some(p);
        }
        if let Some(p) = self
            .own_panel
            .as_ref()
            .filter(|p| p.read(cx).terminal_id == id)
        {
            return Some(p.clone());
        }
        ws.read(cx).terminal(&id)?;
        let g = cx.global::<crate::workspace::AppGlobals>();
        let (source, panels) = (g.source.clone(), g.panels.clone());
        let label: SharedString = g
            .labels
            .borrow()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.clone())
            .into();
        let weak = self.ws.clone();
        let panel = cx.new(|cx| TerminalPanel::new(source, weak, id, label, window, cx));
        // Registered like dock panels: labels, theme changes and "terminal in view" reach it.
        panels.borrow_mut().push(panel.downgrade());
        self.own_panel = Some(panel.clone());
        Some(panel)
    }

    fn notify_all(&self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |_, cx| cx.notify());
        }
    }

    pub fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.selected = Some(id.to_string());
        let cols = self.model.columns(&self.cards(cx));
        if let Some((ci, _, i)) = self.model.column_with(&cols, id) {
            self.scroll[ci].scroll_to_item(i, ScrollStrategy::Nearest);
        }
        let f = self.card_focus(id, cx);
        f.focus(window, cx);
        self.notify_all(cx);
    }

    /// Show `id` in the drawer; `focus` moves the keyboard into its terminal.
    pub fn open(&mut self, id: &str, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.model.selected = Some(id.to_string());
        self.model.opened = Some(id.to_string());
        self.drawer_open = true;
        self.show_dock = false;
        if let Some(panel) = self.drawer_panel(window, cx)
            && focus
        {
            let f = panel.read(cx).view.read(cx).focus_handle(cx);
            f.focus(window, cx);
        }
        self.notify_all(cx);
    }

    /// F6: board ↔ drawer.
    pub fn switch_region(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let in_drawer = self.drawer_open
            && self.ws.upgrade().is_some_and(|ws| {
                ws.read(cx).live_panels().iter().any(|p| {
                    p.read(cx)
                        .view
                        .read(cx)
                        .focus_handle(cx)
                        .contains_focused(window, cx)
                }) || self.show_dock && !self.board_focused(window, cx)
            });
        if in_drawer || !self.drawer_open {
            match self.model.selected.clone() {
                Some(id) => self.select(&id, window, cx),
                None => self.focus.focus(window, cx),
            }
            return;
        }
        if self.show_dock {
            if let Some(ws) = self.ws.upgrade() {
                let dock = ws.read(cx).dock.clone();
                dock.read(cx).focus_handle(cx).focus(window, cx);
            }
            return;
        }
        match self.model.opened.clone().or(self.model.selected.clone()) {
            Some(id) => self.open(&id, true, window, cx),
            None => self.focus.focus(window, cx),
        }
    }

    /// Entering the board: a focused terminal panel keeps focus (it moves to
    /// the drawer as it is), a focused settings/review/… panel keeps focus in
    /// the drawer's 패널 view; otherwise the selected card takes focus.
    fn place_focus_on_enter(
        &mut self,
        dock: &Entity<gpui_kit::component::dock::DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            return;
        }
        let focused_terminal = self.ws.upgrade().and_then(|ws| {
            ws.read(cx)
                .live_panels()
                .into_iter()
                .find(|p| {
                    p.read(cx)
                        .view
                        .read(cx)
                        .focus_handle(cx)
                        .contains_focused(window, cx)
                })
                .map(|p| p.read(cx).terminal_id.clone())
        });
        if let Some(id) = focused_terminal {
            self.model.selected = Some(id.clone());
            self.model.opened = Some(id);
            self.drawer_open = true;
            self.show_dock = false;
            return;
        }
        if dock.read(cx).focus_handle(cx).contains_focused(window, cx) {
            self.drawer_open = true;
            self.show_dock = true;
            return;
        }
        let target = self.model.selected.clone();
        let handle = match target {
            Some(id) => self.card_focus(&id, cx),
            None => self.focus.clone(),
        };
        window.defer(cx, move |window, cx| handle.focus(window, cx));
    }

    fn board_focused(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx)
            || self
                .card_focus
                .values()
                .any(|f| f.contains_focused(window, cx))
    }

    fn on_card_key(
        &mut self,
        id: &str,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &ev.keystroke;
        let nav = match k.key.as_str() {
            "up" => Nav::Up,
            "down" => Nav::Down,
            "left" => Nav::Left,
            "right" => Nav::Right,
            "home" => Nav::Home,
            "end" => Nav::End,
            "enter" if !k.modifiers.modified() => {
                self.open(id, true, window, cx);
                return true;
            }
            _ => return false,
        };
        let cards = self.cards(cx);
        if k.modifiers.alt {
            let r = match nav {
                Nav::Up | Nav::Down => self.model.move_by(&cards, id, nav == Nav::Down),
                Nav::Home => {
                    let column = cards.iter().find(|c| c.id == id).map(|c| c.column);
                    column.map_or(Err(Refused::Unknown), |c| self.model.pin_top(&cards, id, c))
                }
                _ => return false,
            };
            if r.is_ok() {
                self.select(id, window, cx);
            }
            return true;
        }
        if k.modifiers.modified() {
            return false;
        }
        let cols = self.model.columns(&cards);
        if let Some(next) = self.model.nav(&cols, Some(id), nav) {
            self.select(&next, window, cx);
        }
        true
    }

    fn drop_card(
        &mut self,
        d: &DragCard,
        target: Option<&str>,
        header: Option<Column>,
        cx: &mut Context<Self>,
    ) {
        let cards = self.cards(cx);
        let r = match header {
            Some(col) => self.model.pin_top(&cards, &d.id, col),
            None => self.model.reorder(&cards, &d.id, target),
        };
        if r.is_ok() {
            self.model.selected = Some(d.id.clone());
        }
        self.notify_all(cx);
    }

    /// Approve / reject through the one gate path (`gates::resolve_gate`);
    /// `choice` picks an option of a gate that has them (v17).
    /// Focus returns to the card when it was on the card's (now gone) buttons.
    pub fn resolve(
        &mut self,
        card: String,
        gate: String,
        decision: GateDecision,
        choice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.ws.upgrade() else { return };
        if !self.busy.insert(gate.clone()) {
            return;
        }
        let source = ws.read(cx).source.clone();
        let me = cx.entity().downgrade();
        let window_handle = window.window_handle();
        let params = p::GateResolveParams {
            id: gate.clone(),
            decision,
            note: None,
            choice,
        };
        crate::gates::resolve_gate(&source, ws.downgrade(), params, cx, move |_, cx| {
            let _ = window_handle.update(cx, |_, window, cx| {
                let _ = me.update(cx, |b, cx| {
                    b.busy.remove(&gate);
                    let f = b.card_focus(&card, cx);
                    if f.contains_focused(window, cx) {
                        f.focus(window, cx);
                    }
                    cx.notify();
                });
            });
        });
        cx.notify();
    }

    /// `다시 실행`: the same command in the same folder and task.
    pub fn rerun(&mut self, id: &str, cx: &mut Context<Self>) {
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
            let _ = this.update(cx, |b, cx| {
                match r {
                    Ok(Ok(new)) => {
                        ws.update(cx, |w, cx| {
                            if !w.terminals.iter().any(|x| x.id == new.id) {
                                w.terminals.push(new.clone());
                            }
                            cx.notify();
                        });
                        b.model.selected = Some(new.id.clone());
                        b.model.opened = Some(new.id);
                        b.drawer_open = true;
                        b.show_dock = false;
                    }
                    Ok(Err(e)) => ws.update(cx, |w, cx| {
                        w.raise_toast(ToastKind::Error, s::COULD_NOT_RERUN, e.message, cx)
                    }),
                    Err(_) => ws.update(cx, |w, cx| {
                        w.raise_toast(
                            ToastKind::Error,
                            s::COULD_NOT_RERUN,
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

    /// Read the last line of visible cards whose status changed since the last read.
    fn want_lines(&mut self, wanted: Vec<(String, String)>, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        let source = ws.read(cx).source.clone();
        for (id, key) in wanted {
            if self.reading.contains(&id) || self.lines.get(&id).is_some_and(|(k, _)| *k == key) {
                continue;
            }
            self.reading.insert(id.clone());
            let rx = source.call::<p::TerminalRead>(&p::ReadParams {
                target: p::TerminalRef::id(id.clone()),
                mode: p::ReadMode::Screen,
                lines: Some(1),
                raw: false,
            });
            cx.spawn(async move |this, cx| {
                let line = rx
                    .await
                    .ok()
                    .and_then(|r| r.lines.into_iter().rev().find(|l| !l.trim().is_empty()));
                let _ = this.update(cx, |b, cx| {
                    b.reading.remove(&id);
                    b.lines.insert(id, (key, line));
                    cx.notify();
                });
            })
            .detach();
        }
    }

    #[cfg(test)]
    pub fn line(&self, id: &str) -> Option<&str> {
        self.lines.get(id).and_then(|(_, l)| l.as_deref())
    }

    /// Item height of every card (uniform list): grows with the UI font so
    /// Korean text is never clipped at any font size or scale.
    pub fn card_height(cx: &App) -> f32 {
        let ui = Self::ui_font(cx);
        let line = (ui * 1.5).ceil();
        let m = pod_ui::themes::metrics();
        let action = pod_ui::hit_area().max(line + 6.);
        12. * 2. + line * 2. + (line + 8.) + action + 6. * 3. + m.border_width * 2.
    }

    fn ui_font(cx: &App) -> f32 {
        let user = cx
            .try_global::<crate::theme::Settings>()
            .map_or(13., |s| s.font_size);
        crate::theme::ui_font_size(user, pod_ui::themes::metrics().density.ui_font)
    }
}

impl Focusable for Board {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A small text button for card and drawer actions (Enter/Space, Korean name).
fn action_button(
    id: impl Into<ElementId>,
    label: &'static str,
    icon: Option<pod_ui::Icon>,
    primary: bool,
    enabled: bool,
    tip: Option<&'static str>,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let on = Rc::new(on);
    let key = on.clone();
    let (fg, bg, border) = if !enabled {
        (t.text_disabled, t.bg_surface, t.border_subtle)
    } else if primary {
        (
            t.action_primary_fg,
            t.action_primary_bg,
            t.action_primary_bg,
        )
    } else {
        (t.text_primary, t.bg_surface, t.border_control)
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .when(enabled, |d| d.tab_index(0))
        .h(px(pod_ui::hit_area()))
        .px(px(10.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .rounded(px(m.radius.control))
        .border(px(m.border_width))
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .font_weight(FontWeight::SEMIBOLD)
        .when_some(icon, |d, i| d.child(pod_ui::icon_sized(i, 14., fg)))
        .child(label)
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| {
                    if primary {
                        s.bg(rgb(t.action_primary_hover))
                    } else {
                        s.bg(rgb(t.bg_hover))
                    }
                })
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on(window, cx);
                })
                .on_key_down(move |e, window, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space")
                        && !e.keystroke.modifiers.modified()
                    {
                        cx.stop_propagation();
                        key(window, cx);
                    }
                })
        })
        .when_some(tip, |d, tip| {
            d.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
            })
        })
}

/// A UI callback shared by click and key handlers.
type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// What one card element needs besides the card.
struct CardCtx {
    board: WeakEntity<Board>,
    selected: Option<String>,
    opened: Option<String>,
    pinned: HashSet<String>,
    lines: HashMap<String, Option<String>>,
    focus: HashMap<String, FocusHandle>,
    height: f32,
    gap: f32,
    mono: SharedString,
}

fn render_card(c: &Card, ctx: &CardCtx) -> AnyElement {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    use pod_ui::themes::Surface;
    let selected = ctx.selected.as_deref() == Some(c.id.as_str());
    let opened = ctx.opened.as_deref() == Some(c.id.as_str());
    let pinned = ctx.pinned.contains(&c.id);
    let line = ctx.lines.get(&c.id).cloned().flatten();
    let name = card_name(c, line.as_deref(), pinned);
    let shown_line = c
        .gate
        .as_ref()
        .map(|g| g.message.clone())
        .or(line)
        .unwrap_or_else(|| s::NO_OUTPUT.to_string());
    let (glyph, glyph_fg) = if c.failed {
        (pod_ui::Icon::OctagonX, t.danger_fg)
    } else {
        let b = pod_ui::StatusBadge::fixture(match c.column {
            Column::Working => pod_status::AgentState::Working,
            Column::NeedsYou => pod_status::AgentState::Blocked(pod_status::BlockedKind::Other),
            Column::Done => pod_status::AgentState::Done,
        });
        b.glyph(mode)
    };
    let stripe = match c.column {
        Column::NeedsYou if c.failed => Some(t.danger_fg),
        Column::NeedsYou => Some(t.warning_fg),
        _ => None,
    };
    let card_bg = match m.surface {
        Surface::Flat => t.bg_surface,
        _ => t.bg_raised,
    };
    let border = if selected {
        t.accent_fg
    } else if m.surface == Surface::Mono {
        card_bg
    } else {
        t.border_subtle
    };
    let focus = ctx.focus.get(&c.id).cloned();
    let board = ctx.board.clone();
    let id = c.id.clone();
    let drag = DragCard {
        id: c.id.clone(),
        column: c.column,
        title: c.title.clone().into(),
    };
    let column = c.column;

    // Actions (only the selected card's are tab stops: one card at a time in the tab order).
    let mut actions = div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .overflow_hidden();
    let act = |f: fn(&mut Board, &str, &mut Window, &mut Context<Board>)| {
        let (board, id) = (board.clone(), id.clone());
        move |window: &mut Window, cx: &mut App| {
            let _ = board.update(cx, |b, cx| f(b, &id, window, cx));
        }
    };
    let tab = |d: Stateful<Div>| if selected { d } else { d.tab_stop(false) };
    if let Some(g) = &c.gate {
        // v17: one approve button per option (`orch_ui::model::gate_buttons`).
        for (n, btn) in crate::orch_ui::model::gate_buttons(g)
            .into_iter()
            .enumerate()
        {
            let (bd, cd, gd, choice) =
                (board.clone(), id.clone(), g.id.clone(), btn.choice.clone());
            let decision = if btn.approve {
                GateDecision::Approve
            } else {
                GateDecision::Reject
            };
            let bid = match (&btn.choice, btn.approve) {
                (Some(_), _) => format!("kanban-approve-{id}-{n}"),
                (None, true) => format!("kanban-approve-{id}"),
                (None, false) => format!("kanban-reject-{id}"),
            };
            let label: &'static str = if btn.choice.is_some() {
                ""
            } else if btn.approve {
                s::APPROVE
            } else {
                s::REJECT
            };
            let button = action_button(
                SharedString::from(bid),
                label,
                Some(if btn.approve {
                    pod_ui::Icon::Check
                } else {
                    pod_ui::Icon::X
                }),
                btn.approve,
                true,
                None,
                move |window, cx| {
                    let _ = bd.update(cx, |b, cx| {
                        b.resolve(cd.clone(), gd.clone(), decision, choice.clone(), window, cx)
                    });
                },
            );
            actions = actions.child(tab(if btn.choice.is_some() {
                button
                    .aria_label(format!("{}: {}", s::APPROVE, btn.label))
                    .child(btn.label.clone())
            } else {
                button
            }));
        }
    } else if c.column == Column::NeedsYou && !c.failed {
        actions = actions.child(tab(action_button(
            SharedString::from(format!("kanban-answer-{id}")),
            s::ANSWER,
            Some(pod_ui::Icon::SquareTerminal),
            false,
            true,
            Some(s::NO_GATE_TIP),
            act(|b, id, window, cx| b.open(id, true, window, cx)),
        )));
    } else {
        actions = actions.child(tab(action_button(
            SharedString::from(format!("kanban-log-{id}")),
            s::LOG,
            Some(pod_ui::Icon::SquareTerminal),
            false,
            true,
            None,
            act(|b, id, window, cx| b.open(id, true, window, cx)),
        )));
    }
    if c.rerun {
        actions = actions.child(tab(action_button(
            SharedString::from(format!("kanban-rerun-{id}")),
            s::RERUN,
            Some(pod_ui::Icon::RotateCcw),
            false,
            true,
            None,
            act(|b, id, _, cx| b.rerun(id, cx)),
        )));
    }

    let mono = ctx.mono.clone();
    let card = div()
        .id(SharedString::from(format!("kanban-card-{id}")))
        .qa_id({
            let id = id.clone();
            move || format!("kanban-card-{id}")
        })
        .when_some(focus, |d, f| d.track_focus(&f.tab_stop(selected)))
        .role(Role::ListItem)
        .aria_label(name)
        .aria_selected(selected)
        .h(px(ctx.height - ctx.gap))
        .w_full()
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(12.))
        .when(stripe.is_some(), |d| d.pl(px(14.)))
        .rounded(px(m.radius.card))
        .bg(rgb(if selected { t.bg_selected } else { card_bg }))
        .border(px(if selected {
            m.border_width.max(2.)
        } else {
            m.border_width
        }))
        .border_color(rgb(border))
        // Status stripe (응답 필요: warning, 실패: danger); never the only carrier: icon + status text too.
        .when_some(stripe, |d, s| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(4.))
                    .rounded_l(px(m.radius.card))
                    .bg(rgb(s)),
            )
        })
        .relative()
        .when(
            m.shadow != pod_ui::themes::Shadow::None && m.surface != Surface::Flat,
            |d| d.shadow(pod_ui::tokens::elevation(1, mode)),
        )
        .cursor_pointer()
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .hover(move |s| {
            if selected {
                s
            } else {
                s.border_color(rgb(t.border_strong))
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(pod_ui::icon(glyph, glyph_fg))
                .child(crate::agent_kind::mark_el(
                    &format!("kanban-{id}"),
                    &c.kind,
                    pod_ui::tokens::ICON_SIZE,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_primary))
                        .child(c.title.clone()),
                )
                .when(pinned, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .px_1()
                            .rounded(px(m.radius.control))
                            .bg(rgb(t.bg_hover))
                            .text_color(rgb(t.text_secondary))
                            .child(s::PINNED),
                    )
                })
                .when(opened, |d| {
                    d.child(pod_ui::icon_sized(
                        pod_ui::Icon::SquareTerminal,
                        14.,
                        t.accent_fg,
                    ))
                })
                .child(
                    div()
                        .id(SharedString::from(format!("kanban-grip-{id}")))
                        .flex_shrink_0()
                        .child(pod_ui::icon_sized(
                            pod_ui::Icon::ChevronsUpDown,
                            14.,
                            t.text_muted,
                        ))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(s::DRAG_TIP)
                                .build(window, cx)
                        }),
                ),
        )
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(t.text_secondary))
                .child(c.task_title.clone()),
        )
        .child(
            div()
                .min_w_0()
                .px_2()
                .py(px(4.))
                .rounded(px(m.radius.control))
                .bg(rgb(t.bg_canvas))
                .font_family(mono)
                .text_color(rgb(t.text_secondary))
                .truncate()
                .child(shown_line),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(actions)
                .child(div().flex_1())
                .child(
                    div()
                        .qa_id({
                            let id = id.clone();
                            move || format!("kanban-status-{id}")
                        })
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(if c.failed { t.danger_fg } else { t.text_muted }))
                        .child(c.status.clone()),
                ),
        )
        .on_click({
            let (board, id) = (board.clone(), id.clone());
            move |_, window, cx| {
                let _ = board.update(cx, |b, cx| b.open(&id, false, window, cx));
                let _ = board.update(cx, |b, cx| b.select(&id, window, cx));
            }
        })
        .on_key_down({
            let (board, id) = (board.clone(), id.clone());
            move |e, window, cx| {
                if board
                    .update(cx, |b, cx| b.on_card_key(&id, e, window, cx))
                    .unwrap_or(false)
                {
                    cx.stop_propagation();
                }
            }
        })
        .on_drag(drag, |d, _, _, cx| cx.new(|_| DragGhost(d.title.clone())))
        .can_drop(move |v, _, _| {
            v.downcast_ref::<DragCard>()
                .is_some_and(|d| d.column == column)
        })
        .drag_over::<DragCard>(move |s, d, _, _| {
            if d.column == column {
                s.border_color(rgb(t.accent_fg))
            } else {
                s
            }
        })
        .on_drop::<DragCard>({
            let (board, id) = (board.clone(), id.clone());
            move |d, _, cx| {
                let _ = board.update(cx, |b, cx| b.drop_card(d, Some(&id), None, cx));
            }
        });
    div()
        .h(px(ctx.height))
        .w_full()
        .pb(px(ctx.gap))
        .child(card)
        .into_any_element()
}

impl Render for Board {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else {
            return div().into_any_element();
        };
        let cards = self.cards(cx);
        let (focused, signature, dock, task_count) = {
            let w = ws.read(cx);
            let tasks: HashSet<&str> = w
                .terminals
                .iter()
                .filter_map(|t| t.task_id.as_deref())
                .collect();
            (
                w.focused_terminal.clone(),
                Self::dock_signature(w, cx),
                w.dock.clone(),
                tasks.len(),
            )
        };
        // A terminal focused elsewhere (palette jump, new terminal) opens in the drawer.
        if focused != self.seen_focus {
            self.seen_focus = focused.clone();
            if let Some(id) = focused.filter(|id| cards.iter().any(|c| &c.id == id)) {
                self.model.selected = Some(id.clone());
                self.model.opened = Some(id);
                self.drawer_open = true;
                self.show_dock = false;
            }
        }
        // A panel opened while the board shows (settings, review …) appears in the drawer.
        if signature > self.dock_signature && self.dock_signature != usize::MAX {
            self.drawer_open = true;
            self.show_dock = true;
        }
        self.dock_signature = signature;
        let cols = self.model.columns(&cards);
        self.model.settle_selection(&cols);
        if std::mem::take(&mut self.entered) {
            self.place_focus_on_enter(&dock, window, cx);
        }
        for c in &cards {
            self.card_focus(&c.id, cx);
        }

        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        use pod_ui::themes::Surface;
        let gap = m.gap.max(8.);
        let height = Self::card_height(cx) + 8.;
        let ctx = Rc::new(CardCtx {
            board: cx.entity().downgrade(),
            selected: self.model.selected.clone(),
            opened: self.model.opened.clone(),
            pinned: self.model.pinned.clone(),
            lines: self
                .lines
                .iter()
                .map(|(k, (_, l))| (k.clone(), l.clone()))
                .collect(),
            focus: self.card_focus.clone(),
            height,
            gap: 8.,
            mono: gpui_kit::component::theme::Theme::global(cx)
                .mono_font_family
                .clone(),
        });
        let me = cx.entity().downgrade();

        // ── header: title, counts, hint, filters ──
        let (repos, agents) = filter_choices(&cards);
        let filtered = self.model.repo.is_some() || self.model.agent.is_some();
        let filter_menu = |id: &'static str,
                           what: &'static str,
                           all: &'static str,
                           current: Option<String>,
                           choices: Vec<String>,
                           set: fn(&mut BoardModel, Option<String>)| {
            let me = me.clone();
            let text = format!(
                "{what}: {}",
                current.clone().unwrap_or_else(|| all.to_string())
            );
            Button::new(id)
                .label(text.clone())
                .accessibility_label(text)
                .small()
                .dropdown_menu(move |menu: PopupMenu, _, _| {
                    let pick = |menu: PopupMenu,
                                label: String,
                                value: Option<String>,
                                me: WeakEntity<Board>| {
                        menu.item(PopupMenuItem::new(label).on_click(move |_, _, cx| {
                            let _ = me.update(cx, |b, cx| {
                                set(&mut b.model, value.clone());
                                b.notify_all(cx);
                            });
                        }))
                    };
                    let mut menu = pick(menu, all.to_string(), None, me.clone());
                    for c in &choices {
                        menu = pick(menu, c.clone(), Some(c.clone()), me.clone());
                    }
                    menu
                })
        };
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px(px(gap + 4.))
            .pt(px(gap))
            .min_h(px(m.density.panel_header.max(36.)))
            .child(
                div()
                    .flex_shrink_0()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::ALL_WORK),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(rgb(t.text_muted))
                    .child(format!("에이전트 {} · 작업 {}", cards.len(), task_count)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .text_color(rgb(t.text_muted))
                    .child(s::HINT),
            )
            .child(filter_menu(
                "kanban-filter-repo",
                s::REPO,
                s::ALL_REPOS,
                self.model.repo.clone(),
                repos,
                |m, v| m.repo = v,
            ))
            .child(filter_menu(
                "kanban-filter-agent",
                s::AGENT,
                s::ALL_AGENTS,
                self.model.agent.clone(),
                agents,
                |m, v| m.agent = v,
            ))
            .when(filtered, |d| {
                let me = me.clone();
                d.child(action_button(
                    "kanban-filter-clear",
                    s::CLEAR_FILTERS,
                    Some(pod_ui::Icon::X),
                    false,
                    true,
                    None,
                    move |_, cx| {
                        let _ = me.update(cx, |b, cx| {
                            b.model.repo = None;
                            b.model.agent = None;
                            b.notify_all(cx);
                        });
                    },
                ))
            })
            .when(!self.drawer_open, |d| {
                let me = me.clone();
                d.child(
                    pod_ui::IconButton::new(
                        "kanban-drawer-open",
                        pod_ui::Icon::Rows2,
                        s::DRAWER_OPEN,
                    )
                    .render(mode, move |window, cx| {
                        let _ = me.update(cx, |b, cx| {
                            b.drawer_open = true;
                            b.switch_region(window, cx);
                        });
                    }),
                )
            });

        // ── columns ──
        let column_bg = match m.surface {
            Surface::Flat => t.bg_canvas,
            _ => t.bg_surface,
        };
        let mut row = div().flex().flex_1().min_h_0().gap(px(gap)).p(px(gap));
        for (ci, col) in cols.iter().enumerate() {
            let column = Column::ALL[ci];
            let n = col.len();
            let list_cards = Rc::new(col.clone());
            let wanted_board = me.clone();
            let ctx = ctx.clone();
            let list = if col.is_empty() {
                div()
                    .flex_1()
                    .flex()
                    .items_start()
                    .justify_center()
                    .pt_4()
                    .text_color(rgb(t.text_muted))
                    .child(s::NO_CARDS)
                    .into_any_element()
            } else {
                uniform_list(
                    SharedString::from(format!("kanban-list-{ci}")),
                    n,
                    move |range, _, cx| {
                        let wanted: Vec<(String, String)> = list_cards[range.clone()]
                            .iter()
                            .map(|c| (c.id.clone(), c.line_key.clone()))
                            .collect();
                        let _ = wanted_board.update(cx, |b, cx| b.want_lines(wanted, cx));
                        list_cards[range]
                            .iter()
                            .map(|c| render_card(c, &ctx))
                            .collect::<Vec<_>>()
                    },
                )
                .track_scroll(&self.scroll[ci])
                .flex_1()
                .min_h_0()
                .px(px(8.))
                .into_any_element()
            };
            let header_board = me.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("kanban-column-{ci}")))
                    .qa_id(move || format!("kanban-column-{ci}"))
                    .role(Role::List)
                    .aria_label(format!("{}, 카드 {n}개", column.label()))
                    .flex_1()
                    .min_w(px(180.))
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .rounded(px(m.radius.card))
                    .bg(rgb(column_bg))
                    .when(
                        m.surface == Surface::Flat || m.surface == Surface::Islands,
                        |d| {
                            d.border(px(m.border_width))
                                .border_color(rgb(t.border_subtle))
                        },
                    )
                    .when(m.surface == Surface::Islands, |d| {
                        d.shadow(pod_ui::tokens::elevation(1, mode))
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("kanban-column-head-{ci}")))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .min_h(px(m.density.panel_header.max(36.)))
                            .flex_shrink_0()
                            .child(pod_ui::icon(column.icon(), column.fg(&t)))
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(column.label()),
                            )
                            .child(div().text_color(rgb(t.text_muted)).child(n.to_string()))
                            .can_drop(move |v, _, _| {
                                v.downcast_ref::<DragCard>()
                                    .is_some_and(|d| d.column == column)
                            })
                            .drag_over::<DragCard>(move |s, d, _, _| {
                                if d.column == column {
                                    s.bg(rgb(t.bg_hover))
                                } else {
                                    s
                                }
                            })
                            .on_drop::<DragCard>(move |d, _, cx| {
                                let _ = header_board
                                    .update(cx, |b, cx| b.drop_card(d, None, Some(column), cx));
                            }),
                    )
                    .child(list),
            );
        }

        // ── drawer ──
        let drawer = self.drawer_open.then(|| {
            let opened = self.model.opened.clone();
            let card = opened
                .as_ref()
                .and_then(|id| cards.iter().find(|c| &c.id == id))
                .cloned();
            let panel = if self.show_dock {
                None
            } else {
                self.drawer_panel(window, cx)
            };
            let vh = f32::from(window.viewport_size().height);
            let h = if panel.is_none() && !self.show_dock {
                // Nothing opened yet: header + one line of guidance, the board keeps the room.
                m.density.panel_header.max(32.) + Self::ui_font(cx) * 1.5 + 32.
            } else {
                (vh * if self.drawer_max { 0.6 } else { 0.32 }).max(160.)
            };
            let (b1, b2, b3, b4) = (me.clone(), me.clone(), me.clone(), me.clone());
            let seg = |id: &'static str, label: &'static str, on: bool, f: Action| {
                let key = f.clone();
                div()
                    .id(id)
                    .role(Role::Tab)
                    .aria_label(label)
                    .aria_selected(on)
                    .tab_index(0)
                    .px_2()
                    .h(px(pod_ui::hit_area()))
                    .flex()
                    .items_center()
                    .rounded(px(m.radius.control))
                    .text_color(rgb(if on { t.text_primary } else { t.text_secondary }))
                    .when(on, |d| {
                        d.bg(rgb(t.bg_selected)).font_weight(FontWeight::SEMIBOLD)
                    })
                    .hover(|s| s.bg(rgb(t.bg_hover)))
                    .focus_visible(move |s| pod_ui::focus_style(s, mode))
                    .cursor_pointer()
                    .child(label)
                    .on_click(move |_, window, cx| f(window, cx))
                    .on_key_down(move |e, window, cx| {
                        if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                            cx.stop_propagation();
                            key(window, cx);
                        }
                    })
            };
            div()
                .id("kanban-drawer")
                .qa_id(|| "kanban-drawer".into())
                .role(Role::Region)
                .aria_label(s::DRAWER)
                .h(px(h))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .bg(rgb(t.bg_surface))
                .border_t(px(m.border_width))
                .border_color(rgb(t.border_subtle))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .min_h(px(m.density.panel_header.max(32.)))
                        .flex_shrink_0()
                        .border_b(px(m.border_width))
                        .border_color(rgb(t.border_subtle))
                        .when_some(card.clone().filter(|_| !self.show_dock), |d, c| {
                            d.child(pod_ui::icon(
                                c.column.icon(),
                                if c.failed {
                                    t.danger_fg
                                } else {
                                    c.column.fg(&t)
                                },
                            ))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(c.title.clone()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(t.text_secondary))
                                    .child(c.task_title.clone()),
                            )
                        })
                        .when(card.is_none() || self.show_dock, |d| {
                            d.child(div().font_weight(FontWeight::SEMIBOLD).child(s::DRAWER))
                        })
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("kanban-drawer-tabs")
                                .flex()
                                .items_center()
                                .gap_1()
                                .role(Role::TabList)
                                .child(seg(
                                    "kanban-drawer-terminal",
                                    s::DRAWER_TERMINAL,
                                    !self.show_dock,
                                    Rc::new(move |window, cx| {
                                        let _ = b1.update(cx, |b, cx| {
                                            b.show_dock = false;
                                            b.switch_region(window, cx);
                                        });
                                    }),
                                ))
                                .child(seg(
                                    "kanban-drawer-panels",
                                    s::DRAWER_PANELS,
                                    self.show_dock,
                                    Rc::new(move |_, cx| {
                                        let _ = b2.update(cx, |b, cx| {
                                            b.show_dock = true;
                                            b.notify_all(cx);
                                        });
                                    }),
                                )),
                        )
                        .child(
                            pod_ui::IconButton::new(
                                "kanban-drawer-size",
                                if self.drawer_max {
                                    pod_ui::Icon::Minimize2
                                } else {
                                    pod_ui::Icon::Maximize2
                                },
                                if self.drawer_max {
                                    s::DRAWER_SHRINK
                                } else {
                                    s::DRAWER_GROW
                                },
                            )
                            .render(mode, move |_, cx| {
                                let _ = b3.update(cx, |b, cx| {
                                    b.drawer_max = !b.drawer_max;
                                    b.notify_all(cx);
                                });
                            }),
                        )
                        .child(
                            pod_ui::IconButton::new(
                                "kanban-drawer-close",
                                pod_ui::Icon::X,
                                s::DRAWER_CLOSE,
                            )
                            .render(mode, move |window, cx| {
                                let _ = b4.update(cx, |b, cx| {
                                    b.drawer_open = false;
                                    b.switch_region(window, cx);
                                });
                            }),
                        ),
                )
                .child(match (self.show_dock, panel) {
                    (true, _) => div().flex_1().min_h_0().child(dock).into_any_element(),
                    (false, Some(p)) => div().flex_1().min_h_0().child(p).into_any_element(),
                    (false, None) => div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(rgb(t.text_muted))
                        .child(s::DRAWER_EMPTY)
                        .into_any_element(),
                })
        });

        div()
            .id("kanban-board")
            .qa_id(|| "kanban-board".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|b, _: &SwitchRegion, window, cx| b.switch_region(window, cx)))
            .on_key_down(cx.listener(|b, e: &KeyDownEvent, window, cx| {
                // The board itself (no card focused yet): arrows pick the first card.
                if b.focus.is_focused(window)
                    && matches!(
                        e.keystroke.key.as_str(),
                        "up" | "down" | "left" | "right" | "home" | "end"
                    )
                {
                    let cols = b.model.columns(&b.cards(cx));
                    if let Some(id) = b.model.nav(&cols, None, Nav::Home) {
                        b.select(&id, window, cx);
                    }
                }
            }))
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .text_color(rgb(t.text_primary))
            .child(header)
            .child(row)
            .when_some(drawer, |d, dr| d.child(dr))
            .into_any_element()
    }
}

// ───────────────────────────── fixture ─────────────────────────────

/// `terminal.read` for the read-only UI fixture (visual captures): a fixed
/// last line per terminal, so the board looks populated without a daemon.
pub fn fixture_answer(
    method: &str,
    params: &serde_json::Value,
) -> Option<Result<serde_json::Value, crate::source::SourceError>> {
    if method != "terminal.read" {
        return None;
    }
    let id = params
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let n: usize = id
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0);
    const LINES: [&str; 6] = [
        "셀 경계 테스트 추가 중 · 42초",
        "$ cargo clippy -p pod-ui -- -D warnings",
        "작업 완료 · 1분 12초 · 토큰 18.2k",
        "PS C:\\src\\pod> 종료 0",
        "• 수정 crates/pod-app/src/notify.rs",
        "error[E0599]: no method `spawn_lazy`",
    ];
    Some(Ok(
        serde_json::json!({ "id": id, "mode": "screen", "lines": [LINES[n % LINES.len()]], "cursor": [0, 0], "state": "running" }),
    ))
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    // Not `super::*` at the top: gpui's glob exports its own `test` attribute.
    use super::{
        BoardModel, Card, Column, Nav, Refused, build_cards, card_name, column_of, filter_choices,
    };
    use crate::source::TestBackend;
    use crate::vm::{Liveness, TerminalVm};
    use core::prelude::v1::test;

    fn term(id: &str, state: &str, task: Option<&str>) -> TerminalVm {
        TestBackend::terminal(id, &format!("이름 {id}"), task, state)
    }

    fn gate(id: &str, terminal: Option<&str>, task: Option<&str>) -> podd::protocol::GateInfo {
        podd::protocol::GateInfo {
            id: id.into(),
            title: "배포 승인".into(),
            message: "main에 병합할까요?".into(),
            task: task.map(Into::into),
            group: None,
            terminal: terminal.map(Into::into),
            state: podd::protocol::GateState::Pending,
            created_ms: 1,
            resolved_ms: None,
            note: None,
            expires_ms: None,
            run: None,
            item: None,
            options: Vec::new(),
            choice: None,
            requested_by: None,
            decided_by: None,
        }
    }

    #[test]
    fn columns_come_from_the_agent_state_and_lifecycle() {
        let cases = [
            ("working", Liveness::Running, Column::Working, false),
            ("unknown", Liveness::Running, Column::Working, false),
            ("blocked", Liveness::Running, Column::NeedsYou, false),
            ("error", Liveness::Running, Column::NeedsYou, true),
            ("done", Liveness::Running, Column::Done, false),
            ("idle", Liveness::Running, Column::Done, false),
            (
                "working",
                Liveness::Exited {
                    code: Some(0),
                    killed: false,
                },
                Column::Done,
                false,
            ),
            (
                "working",
                Liveness::Exited {
                    code: Some(1),
                    killed: false,
                },
                Column::NeedsYou,
                true,
            ),
            (
                "working",
                Liveness::Exited {
                    code: Some(1),
                    killed: true,
                },
                Column::Done,
                false,
            ),
            ("working", Liveness::Lost, Column::NeedsYou, true),
        ];
        for (state, live, column, failed) in cases {
            let mut t = term("t1", state, None);
            t.live = live;
            assert_eq!(column_of(&t), (column, failed), "{state} {live:?}");
        }
        let mut plain = term("t2", "working", None);
        plain.status = None;
        assert_eq!(
            column_of(&plain),
            (Column::Working, false),
            "a shell without agent status is 작업 중"
        );
    }

    fn sample() -> Vec<Card> {
        let tasks = vec![
            TestBackend::task("pod/a", "글자 정렬", "pod", &["a1", "a2"]),
            TestBackend::task("web/b", "설정 화면", "web", &["b1"]),
        ];
        let mut terms = vec![
            term("a1", "working", Some("pod/a")),
            term("a2", "working", Some("pod/a")),
            term("b1", "blocked", Some("web/b")),
            term("c1", "working", None),
        ];
        terms[3].program = "codex".into();
        terms[3].argv = vec!["codex".into()];
        build_cards(&terms, &tasks, &[gate("g1", None, Some("web/b"))], |t| {
            t.name.clone().unwrap_or_default()
        })
    }

    #[test]
    fn cards_reuse_workspace_data_and_gates() {
        let cards = sample();
        assert_eq!(cards.len(), 4);
        assert_eq!(cards[0].title, "Powershell · 이름 a1");
        assert_eq!(
            (cards[0].task_title.as_str(), cards[0].repo.as_deref()),
            ("글자 정렬", Some("pod"))
        );
        assert_eq!(
            cards[2].gate.as_ref().map(|g| g.id.as_str()),
            Some("g1"),
            "a task gate goes to the task's first terminal"
        );
        assert_eq!(cards[3].task_title, "작업 없음");
        assert!(!cards[0].rerun);
        let name = card_name(&cards[2], Some("PS> y/n"), true);
        assert!(
            name.contains("응답 필요")
                && name.contains("승인 요청: 배포 승인")
                && name.contains("마지막 줄: PS> y/n")
                && name.ends_with("고정됨"),
            "{name}"
        );
        let (repos, agents) = filter_choices(&cards);
        assert_eq!(repos, ["pod", "web"]);
        assert_eq!(agents, ["Codex", "Powershell"]);
        // A terminal gate wins over a task gate.
        let tasks = vec![TestBackend::task(
            "web/b",
            "설정 화면",
            "web",
            &["b1", "b2"],
        )];
        let terms = vec![
            term("b1", "blocked", Some("web/b")),
            term("b2", "blocked", Some("web/b")),
        ];
        let cards = build_cards(
            &terms,
            &tasks,
            &[gate("g2", Some("b2"), Some("web/b"))],
            |t| t.id.clone(),
        );
        assert_eq!(
            (
                cards[0].gate.is_none(),
                cards[1].gate.as_ref().map(|g| g.id.as_str())
            ),
            (true, Some("g2"))
        );
    }

    fn ids(col: &[Card]) -> Vec<&str> {
        col.iter().map(|c| c.id.as_str()).collect()
    }

    #[test]
    fn drag_reorders_and_pins_within_a_column_but_never_changes_state() {
        let cards = sample();
        let mut m = BoardModel::default();
        assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "a2", "c1"]);
        m.reorder(&cards, "c1", Some("a1")).unwrap();
        assert_eq!(ids(&m.columns(&cards)[0]), ["c1", "a1", "a2"]);
        m.reorder(&cards, "c1", None).unwrap();
        assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "a2", "c1"]);
        // Another column: refused, nothing moves, the column is unchanged.
        let before = m.clone();
        assert_eq!(
            m.reorder(&cards, "a1", Some("b1")),
            Err(Refused::StateIsTheAgents)
        );
        assert_eq!(
            m.pin_top(&cards, "a1", Column::NeedsYou),
            Err(Refused::StateIsTheAgents)
        );
        assert_eq!(m, before);
        assert_eq!(m.columns(&cards)[1][0].column, Column::NeedsYou);
        // Pin to the top; later cards stay below it.
        m.pin_top(&cards, "a2", Column::Working).unwrap();
        assert_eq!(ids(&m.columns(&cards)[0]), ["a2", "a1", "c1"]);
        assert!(m.pinned.contains("a2"));
        // Keyboard: Alt+↓ swaps with the neighbour and takes its (unpinned) state.
        m.move_by(&cards, "a2", true).unwrap();
        assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "a2", "c1"]);
        assert!(!m.pinned.contains("a2"));
        m.move_by(&cards, "c1", false).unwrap();
        assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "c1", "a2"]);
        m.move_by(&cards, "a1", false).unwrap();
        assert_eq!(
            ids(&m.columns(&cards)[0]),
            ["a1", "c1", "a2"],
            "top stays top"
        );
        // The order survives a state change: a card that moves column keeps its rank when it returns.
        let mut moved = cards.clone();
        moved[1].column = Column::Done;
        assert_eq!(ids(&m.columns(&moved)[2]), ["a2"]);
        assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "c1", "a2"]);
    }

    #[test]
    fn filters_and_keyboard_navigation() {
        let cards = sample();
        let mut m = BoardModel {
            repo: Some("pod".into()),
            ..Default::default()
        };
        let cols = m.columns(&cards);
        assert_eq!((ids(&cols[0]), cols[1].len()), (vec!["a1", "a2"], 0));
        m.repo = None;
        m.agent = Some("Codex".into());
        assert_eq!(ids(&m.columns(&cards)[0]), ["c1"]);
        m.agent = None;
        let cols = m.columns(&cards);
        assert_eq!(
            m.nav(&cols, None, Nav::Down).as_deref(),
            Some("a1"),
            "no selection: first card"
        );
        assert_eq!(m.nav(&cols, Some("a1"), Nav::Down).as_deref(), Some("a2"));
        assert_eq!(m.nav(&cols, Some("a1"), Nav::Up), None);
        assert_eq!(
            m.nav(&cols, Some("a2"), Nav::Right).as_deref(),
            Some("b1"),
            "row clamps to the shorter column"
        );
        assert_eq!(
            m.nav(&cols, Some("b1"), Nav::Right),
            None,
            "완료 is empty: nowhere to go"
        );
        assert_eq!(m.nav(&cols, Some("b1"), Nav::Left).as_deref(), Some("a1"));
        assert_eq!(m.nav(&cols, Some("a1"), Nav::End).as_deref(), Some("c1"));
        // Selection follows filters.
        m.selected = Some("b1".into());
        m.repo = Some("pod".into());
        let cols = m.columns(&cards);
        m.settle_selection(&cols);
        assert_eq!(m.selected.as_deref(), Some("a1"));
    }
}
