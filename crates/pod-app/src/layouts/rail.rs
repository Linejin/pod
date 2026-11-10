//! 집중 레일 (preset 02, docs/10 §D5.6): a 44px icon rail, the current task's
//! tabs on top and a dock that shows only the current task.
//!
//! The rail keeps its own [`DockArea`]. The workbench dock stays the owner of
//! every panel (all `open_*` functions add there and the layout file saves
//! it); the rail dock holds [`Slot`]s, thin handles to the same panel
//! entities that do not pass `on_removed` on when a task switch takes them
//! out. So switching tasks or layouts never detaches a terminal, and the
//! workbench comes back exactly as it was. Closing a tab in the rail closes
//! the panel in the workbench (the same as there).
//!
//! State comes only from change events: the workbench dock's `LayoutChanged`
//! and the workspace's own notify (tasks, terminals, statuses, gates). The
//! derived rows (tabs, 응답 대기) are computed then, not per frame.
use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dock::{
    BasePanelView, DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, PaneNode, PaneRef,
    PanelControl, PanelHandle, PanelId, PanelState, PanelStyle, PanelView, TabGroup, TitleStyle,
};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::qa_marks::QaId;
use crate::vm::{TaskVm, TerminalVm};
use crate::workspace::Workspace;

actions!(
    pod_rail,
    [
        NextTask,
        PrevTask,
        TaskSwitcher,
        AttentionDrawer,
        WorkspaceDrawer
    ]
);

/// Key context of the rail body: its shortcuts exist only in this layout.
const CONTEXT: &str = "PodRail";
/// Rail width (docs/10 §D4 B: 44px navigation rail).
pub const RAIL_WIDTH: f32 = 44.;
/// Longest task title in a tab before it ellipsizes (the tooltip has the rest).
pub const TAB_TEXT_MAX: f32 = 200.;
const RECENT_MAX: usize = 8;

mod s {
    pub const RAIL: &str = "레일 탐색";
    pub const WORKSPACE: &str = "작업 공간";
    pub const WORKSPACE_TIP: &str = "작업 공간 — 저장소·작업·터미널 (Ctrl+Alt+W)";
    pub const TERMINAL: &str = "터미널";
    pub const TERMINAL_TIP: &str = "현재 작업의 터미널로 이동";
    pub const FILES: &str = "파일";
    pub const FILES_TIP: &str = "현재 작업의 파일 (Ctrl+P)";
    pub const AGENTS: &str = "에이전트";
    pub const AGENTS_TIP: &str = "에이전트 트리 (Ctrl+Shift+E)";
    pub const ATTENTION: &str = "응답 대기";
    pub const ATTENTION_TITLE: &str = "응답이 필요한 다른 작업";
    pub const ATTENTION_EMPTY: &str = "다른 작업에 기다리는 요청이 없습니다.";
    pub const ALL_NOTIFICATIONS: &str = "알림 모두 보기";
    pub const GATES: &str = "승인함 열기";
    pub const TABS: &str = "작업 탭";
    pub const SWITCHER: &str = "작업 전환";
    pub const SWITCHER_TIP: &str =
        "작업 전환 (Ctrl+Alt+T) · 다음/이전 작업 Ctrl+Alt+PageDown/PageUp";
    pub const ALL_TASKS: &str = "모든 작업";
    pub const NO_TASKS: &str = "작업이 없습니다";
    pub const EMPTY_TITLE: &str = "이 작업에 열린 패널이 없습니다";
    pub const EMPTY_BODY: &str = "터미널이나 파일을 열어 시작하세요. 다른 작업은 위 탭이나 작업 전환(Ctrl+Alt+T)에서 고릅니다.";
    pub const NO_TASK_BODY: &str = "새 작업을 만들거나 작업 전환에서 작업을 고르세요.";
    pub const CLOSE: &str = "닫기";
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-alt-pagedown", NextTask, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-pageup", PrevTask, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-t", TaskSwitcher, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-a", AttentionDrawer, Some(CONTEXT)),
        KeyBinding::new("ctrl-alt-w", WorkspaceDrawer, Some(CONTEXT)),
    ]);
}

/// The layout module (registered in `LayoutRegistry::new`).
pub struct FocusRail;
impl LayoutModule for FocusRail {
    fn mode(&self) -> LayoutMode {
        LayoutMode::FocusRail
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

// ---- state model (pure; unit-tested) ---------------------------------------

/// Which task a dock panel belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Owner {
    Task(String),
    /// Settings, gates, agent tree, groups, extensions …: shown with every task.
    Global,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawer {
    /// The repository/task/terminal tree (the workbench sidebar).
    Workspace,
    /// 작업 전환: every task.
    Tasks,
    /// 응답 대기: other tasks that wait for the user.
    Attention,
}

/// A dock arrangement in panel ids (the rail remembers one per task).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Shape<P> {
    Split {
        horizontal: bool,
        children: Vec<(Shape<P>, Option<f32>)>,
    },
    Tabs {
        panels: Vec<P>,
        active: usize,
    },
}

impl<P: Copy + Eq + Hash> Shape<P> {
    pub fn panels(&self) -> Vec<P> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }
    fn collect(&self, out: &mut Vec<P>) {
        match self {
            Self::Split { children, .. } => children.iter().for_each(|(c, _)| c.collect(out)),
            Self::Tabs { panels, .. } => out.extend(panels.iter().copied()),
        }
    }
    /// Keep only `keep`; empty containers go, a split left with one child becomes that child.
    pub fn retain(self, keep: &HashSet<P>) -> Option<Self> {
        match self {
            Self::Tabs { panels, active } => {
                let shown = panels.get(active).copied();
                let kept: Vec<P> = panels.into_iter().filter(|p| keep.contains(p)).collect();
                if kept.is_empty() {
                    return None;
                }
                let active = shown
                    .and_then(|s| kept.iter().position(|p| *p == s))
                    .unwrap_or(0);
                Some(Self::Tabs {
                    panels: kept,
                    active,
                })
            }
            Self::Split {
                horizontal,
                children,
            } => {
                let mut kept: Vec<_> = children
                    .into_iter()
                    .filter_map(|(c, size)| c.retain(keep).map(|c| (c, size)))
                    .collect();
                match kept.len() {
                    0 => None,
                    1 => kept.pop().map(|(c, _)| c),
                    _ => Some(Self::Split {
                        horizontal,
                        children: kept,
                    }),
                }
            }
        }
    }
    /// Add `p` to the first tab group and show it.
    pub fn append(&mut self, p: P) {
        match self {
            Self::Tabs { panels, active } => {
                panels.push(p);
                *active = panels.len() - 1;
            }
            Self::Split { children, .. } => {
                if let Some((c, _)) = children.first_mut() {
                    c.append(p);
                }
            }
        }
    }
    /// Whether `p` is the visible tab of its group.
    pub fn is_shown(&self, p: P) -> bool {
        match self {
            Self::Tabs { panels, active } => panels.get(*active) == Some(&p),
            Self::Split { children, .. } => children.iter().any(|(c, _)| c.is_shown(p)),
        }
    }
    /// The visible tab of the first group.
    pub fn first_shown(&self) -> Option<P> {
        match self {
            Self::Tabs { panels, active } => panels.get(*active).copied(),
            Self::Split { children, .. } => children.first().and_then(|(c, _)| c.first_shown()),
        }
    }
}

/// The first view of a task (gallery 02): its first terminal with the task's
/// other panels as tabs, wide on the left, and the other terminals stacked on
/// the right.
pub(crate) fn default_shape<P: Copy>(
    terminals: &[P],
    others: &[P],
    main_width: Option<f32>,
) -> Option<Shape<P>> {
    let first: Vec<P> = terminals.iter().take(1).chain(others).copied().collect();
    if terminals.len() <= 1 {
        return (!first.is_empty()).then_some(Shape::Tabs {
            panels: first,
            active: 0,
        });
    }
    let mut side: Vec<(Shape<P>, Option<f32>)> = terminals[1..]
        .iter()
        .map(|p| {
            (
                Shape::Tabs {
                    panels: vec![*p],
                    active: 0,
                },
                None,
            )
        })
        .collect();
    let side = if side.len() == 1 {
        side.remove(0).0
    } else {
        Shape::Split {
            horizontal: false,
            children: side,
        }
    };
    Some(Shape::Split {
        horizontal: true,
        children: vec![
            (
                Shape::Tabs {
                    panels: first,
                    active: 0,
                },
                main_width,
            ),
            (side, None),
        ],
    })
}

/// The arrangement for a task: the remembered one minus closed panels plus new
/// ones (shown), or the default when nothing is remembered.
pub(crate) fn plan<P: Copy + Eq + Hash>(
    prev: Option<Shape<P>>,
    terminals: &[P],
    others: &[P],
    main_width: Option<f32>,
) -> Option<Shape<P>> {
    let all: Vec<P> = terminals.iter().chain(others).copied().collect();
    let keep: HashSet<P> = all.iter().copied().collect();
    match prev.and_then(|s| s.retain(&keep)) {
        Some(mut shape) => {
            let have: HashSet<P> = shape.panels().into_iter().collect();
            for p in all.into_iter().filter(|p| !have.contains(p)) {
                shape.append(p);
            }
            Some(shape)
        }
        None => default_shape(terminals, others, main_width),
    }
}

/// One task as the rail shows it (tabs, 작업 전환, 응답 대기).
#[derive(Clone, Debug, Default)]
pub(crate) struct TaskRow {
    pub id: String,
    pub title: String,
    pub branch: Option<String>,
    pub summary: pod_ui::StatusSummary,
    /// Pending orchestration gates of this task.
    pub gates: usize,
    /// First terminal that waits for the user (blocked), else the first failed one.
    pub first_attention: Option<String>,
    /// The task has panels in the dock.
    pub open: bool,
    /// The task's agents (docs/10 §D5.11): small marks on the tab and drawer rows.
    pub agents: Vec<crate::agent_kind::AgentKind>,
}
impl TaskRow {
    pub fn needs_you(&self) -> bool {
        self.summary.blocked > 0 || self.gates > 0 || self.summary.error > 0
    }
    /// `승인 필요 2 · 실패 1 · 승인 요청 1`, empty when nothing waits.
    pub fn attention_label(&self) -> String {
        let mut parts = Vec::new();
        if self.summary.blocked > 0 {
            parts.push(format!("승인 필요 {}", self.summary.blocked));
        }
        if self.gates > 0 {
            parts.push(format!("승인 요청 {}", self.gates));
        }
        if self.summary.error > 0 {
            parts.push(format!("실패 {}", self.summary.error));
        }
        parts.join(" · ")
    }
    /// Accessible name of a tab or row: title, its agents, then what waits.
    pub fn accessible_name(&self) -> String {
        let label = self.attention_label();
        let mut name = self.title.clone();
        if !self.agents.is_empty() {
            name.push_str(" · ");
            name.push_str(
                &self
                    .agents
                    .iter()
                    .map(|k| k.accessible_name())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if label.is_empty() {
            name
        } else {
            format!("{name} — {label}")
        }
    }
}

pub(crate) fn task_rows(
    tasks: &[TaskVm],
    terminals: &[TerminalVm],
    pending_gates: &[podd::protocol::GateInfo],
    open: &HashSet<String>,
) -> Vec<TaskRow> {
    tasks
        .iter()
        .map(|t| {
            let summary = crate::orch::summary_of(t.terminals.iter(), terminals);
            let state_of = |id: &String| {
                terminals
                    .iter()
                    .find(|x| &x.id == id && x.is_running())
                    .and_then(|x| x.status.as_ref())
                    .map(|s| s.state)
            };
            let first_attention = t
                .terminals
                .iter()
                .find(|id| state_of(id) == Some(crate::vm::AgentState::Blocked))
                .or_else(|| {
                    t.terminals
                        .iter()
                        .find(|id| state_of(id) == Some(crate::vm::AgentState::Error))
                })
                .cloned();
            TaskRow {
                id: t.id.clone(),
                title: t.title.clone(),
                branch: t.branch.clone(),
                summary,
                gates: pending_gates
                    .iter()
                    .filter(|g| g.task.as_deref() == Some(t.id.as_str()))
                    .count(),
                first_attention,
                open: open.contains(&t.id),
                agents: crate::agent_kind::task_kinds(t, terminals),
            }
        })
        .collect()
}

/// 응답 대기: other tasks that wait for the user, requests (blocked/gates) before failures.
pub(crate) fn attention(rows: &[TaskRow], current: Option<&str>) -> Vec<usize> {
    let mut out: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| Some(r.id.as_str()) != current && r.needs_you())
        .map(|(i, _)| i)
        .collect();
    out.sort_by_key(|i| rows[*i].summary.blocked + rows[*i].gates == 0);
    out
}

/// Tasks with a tab: open panels, recently used, and the current one (task order).
pub(crate) fn tab_tasks(rows: &[TaskRow], recent: &[String], current: Option<&str>) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.open || recent.contains(&r.id) || Some(r.id.as_str()) == current)
        .map(|(i, _)| i)
        .collect()
}

/// The task `delta` tabs away from `current`, wrapping.
pub(crate) fn step(ids: &[String], current: Option<&str>, delta: isize) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let n = ids.len() as isize;
    let at = current
        .and_then(|c| ids.iter().position(|x| x == c))
        .map(|i| i as isize)
        .unwrap_or(if delta > 0 { -1 } else { 0 });
    Some(ids[(at + delta).rem_euclid(n) as usize].clone())
}

/// CJK and other full-width characters.
fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF)
}

/// Conservative width estimate for a single-line label (no text system needed:
/// Hangul is one em wide in Malgun Gothic/Segoe UI, Latin at most 0.6 em).
pub(crate) fn text_width(text: &str, font: f32) -> f32 {
    text.chars()
        .map(|c| {
            if is_wide(c) {
                font
            } else if c.is_ascii() {
                font * 0.6
            } else {
                font * 0.8
            }
        })
        .sum()
}

/// A task tab: padding 12+12, status dot + gap, title (at most [`TAB_TEXT_MAX`]), badge,
/// agent marks (12px each, 2px apart, at most 3 + `+N`).
pub(crate) fn tab_width(title: &str, font: f32, badge: bool, agents: usize) -> f32 {
    let marks = if agents == 0 {
        0.
    } else {
        8. + agents.min(3) as f32 * 14. + if agents > 3 { 20. } else { 0. }
    };
    24. + 16.
        + text_width(title, font).min(TAB_TEXT_MAX)
        + if badge { 28. } else { 0. }
        + marks
        + 2.
}

/// Tabs that fit in `avail`, in order; the current tab is always shown (last,
/// in place of the ones that no longer fit).
pub(crate) fn fit_tabs(widths: &[f32], current: Option<usize>, avail: f32) -> Vec<usize> {
    let mut out = Vec::new();
    let mut used = 0.;
    for (i, w) in widths.iter().enumerate() {
        if used + w > avail {
            break;
        }
        used += w;
        out.push(i);
    }
    if let Some(c) = current.filter(|c| *c < widths.len() && !out.contains(c)) {
        while used + widths[c] > avail {
            let Some(last) = out.pop() else { break };
            used -= widths[last];
        }
        out.push(c);
    }
    out
}

// ---- dock slot ---------------------------------------------------------------

/// A rail dock tab for a panel the workbench dock owns. Everything is the
/// panel's own except removal: a task switch takes slots out quietly; a user
/// closing the tab closes the panel in the workbench.
struct Slot {
    inner: Arc<dyn PanelView>,
    workbench: WeakEntity<DockArea>,
    quiet: Arc<AtomicBool>,
}

impl BasePanelView for Slot {
    fn panel_name(&self, cx: &App) -> &'static str {
        self.inner.panel_name(cx)
    }
    fn panel_id(&self, cx: &App) -> PanelId {
        self.inner.panel_id(cx)
    }
    fn closable(&self, cx: &App) -> bool {
        self.inner.closable(cx)
    }
    fn zoomable(&self, cx: &App) -> bool {
        self.inner.zoomable(cx)
    }
    fn visible(&self, cx: &App) -> bool {
        self.inner.visible(cx)
    }
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.inner.set_active(active, window, cx);
    }
    fn set_zoomed(&self, zoomed: bool, window: &mut Window, cx: &mut App) {
        self.inner.set_zoomed(zoomed, window, cx);
    }
    /// Not passed on: the panel's group is the workbench's.
    fn on_added_to(&self, _: WeakEntity<TabGroup>, _: &mut Window, _: &mut App) {}
    fn on_removed(&self, window: &mut Window, cx: &mut App) {
        if self.quiet.load(Ordering::Relaxed) {
            return;
        }
        let view = self.inner.view();
        let workbench = self.workbench.clone();
        window.defer(cx, move |window, cx| {
            close_in_workbench(&workbench, view, window, cx)
        });
    }
    fn view(&self) -> AnyView {
        self.inner.view()
    }
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.inner.focus_handle(cx)
    }
    fn dump(&self, cx: &App) -> PanelState {
        self.inner.dump(cx)
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl PanelView for Slot {
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        self.inner.tab_name(cx)
    }
    fn title(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        self.inner.title(window, cx)
    }
    fn title_style(&self, cx: &App) -> Option<TitleStyle> {
        self.inner.title_style(cx)
    }
    fn title_suffix(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        self.inner.title_suffix(window, cx)
    }
    fn toolbar_buttons(&self, window: &mut Window, cx: &mut App) -> Option<Vec<Button>> {
        self.inner.toolbar_buttons(window, cx)
    }
    fn dropdown_menu(&self, menu: PopupMenu, window: &mut Window, cx: &mut App) -> PopupMenu {
        self.inner.dropdown_menu(menu, window, cx)
    }
    fn zoom_control(&self, cx: &App) -> Option<PanelControl> {
        self.inner.zoom_control(cx)
    }
    fn inner_padding(&self, cx: &App) -> bool {
        self.inner.inner_padding(cx)
    }
    fn title_bar(&self, cx: &App) -> bool {
        self.inner.title_bar(cx)
    }
}

/// Close a panel in the workbench dock (the real removal: a terminal detaches).
fn close_in_workbench(
    workbench: &WeakEntity<DockArea>,
    view: AnyView,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(workbench) = workbench.upgrade() else {
        return;
    };
    macro_rules! close {
        ($($t:ty),*) => {
            $(if let Ok(e) = view.clone().downcast::<$t>() {
                workbench.update(cx, |a, cx| a.remove_panel(e, window, cx));
                return;
            })*
        };
    }
    close!(
        crate::panel::TerminalPanel,
        crate::editor_panel::EditorPanel,
        crate::review::ReviewPanel,
        crate::chrome::ChromePanel,
        crate::gates::GatesPanel,
        crate::groups::GroupPanel,
        crate::tree::AgentTreePanel,
        crate::mods_ui::ExtensionsPanel,
        crate::sessions_ui::SessionsPanel
    );
}

fn shape_of(node: &PaneNode) -> Option<Shape<PanelId>> {
    match node.kind() {
        PaneRef::Tabs { panels, active_ix } => (!panels.is_empty()).then(|| Shape::Tabs {
            panels: panels.to_vec(),
            active: active_ix,
        }),
        PaneRef::Split {
            axis,
            children,
            sizes,
        } => {
            let mut kept: Vec<(Shape<PanelId>, Option<f32>)> = children
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    shape_of(c).map(|s| {
                        (
                            s,
                            sizes.get(i).copied().flatten().map(|p| p.to_f64() as f32),
                        )
                    })
                })
                .collect();
            match kept.len() {
                0 => None,
                1 => kept.pop().map(|(s, _)| s),
                _ => Some(Shape::Split {
                    horizontal: axis == Axis::Horizontal,
                    children: kept,
                }),
            }
        }
    }
}

fn to_layout(
    shape: &Shape<PanelId>,
    slots: &HashMap<PanelId, Arc<dyn BasePanelView>>,
    cx: &App,
) -> DockLayout {
    match shape {
        Shape::Tabs { panels, active } => {
            let mut l = DockLayout::tabs();
            for p in panels {
                if let Some(slot) = slots.get(p) {
                    l = l.panel_view(slot.clone(), cx);
                }
            }
            l.active_index(*active)
        }
        Shape::Split {
            horizontal,
            children,
        } => {
            let mut l = if *horizontal {
                DockLayout::h_split()
            } else {
                DockLayout::v_split()
            };
            for (c, size) in children {
                l = l.child(to_layout(c, slots, cx), size.map(px));
            }
            l
        }
    }
}

// ---- the rail state (one per workspace window) ------------------------------

#[derive(Default)]
struct RailStates(HashMap<EntityId, Entity<RailState>>);
impl Global for RailStates {}

/// The rail of workspace `ws`, if it was shown this session.
pub(crate) fn state_of(ws: EntityId, cx: &App) -> Option<Entity<RailState>> {
    cx.try_global::<RailStates>()
        .and_then(|g| g.0.get(&ws).cloned())
}

pub struct RailState {
    ws: WeakEntity<Workspace>,
    workbench: Entity<DockArea>,
    pub(crate) dock: Entity<DockArea>,
    pub(crate) current: Option<String>,
    pub(crate) drawer: Option<Drawer>,
    recent: Vec<String>,
    /// Workbench panels at the last sync: (id, presentation, owner), dock order.
    inner: Vec<(PanelId, Arc<dyn PanelView>, Owner)>,
    /// Which of `inner` are terminal panels.
    terminal_panels: HashSet<PanelId>,
    slots: HashMap<PanelId, Arc<dyn BasePanelView>>,
    /// Arrangement per task, taken when the rail leaves the task.
    shapes: HashMap<String, Shape<PanelId>>,
    /// Task whose panels the rail dock holds.
    shown_task: Option<String>,
    quiet: Arc<AtomicBool>,
    pub(crate) rows: Vec<TaskRow>,
    pub(crate) tabs: Vec<usize>,
    pub(crate) attention: Vec<usize>,
    font: f32,
    last_focus: Option<FocusHandle>,
    // Keyboard: tracked handles so focus can come back to the opener.
    pub(crate) buttons: [FocusHandle; 5],
    pub(crate) strip_focus: FocusHandle,
    pub(crate) switcher_focus: FocusHandle,
    pub(crate) list_focus: FocusHandle,
    strip_cursor: Option<String>,
    list_cursor: usize,
    list_scroll: UniformListScrollHandle,
    opener: Option<FocusHandle>,
    _subs: Vec<Subscription>,
}

/// Rail buttons, top to bottom (index into [`RailState::buttons`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RailButton {
    Workspace = 0,
    Terminal = 1,
    Files = 2,
    Agents = 3,
    Attention = 4,
}

impl RailState {
    fn new(
        ws: Entity<Workspace>,
        workbench: Entity<DockArea>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (dock, skin) = DockSkin::dock_area("pod-rail", None, window, cx);
        skin.set_close_button_visible(true, cx);
        skin.set_panel_style(PanelStyle::Auto, cx);
        let subs = vec![
            cx.subscribe_in(
                &workbench,
                window,
                |r: &mut Self, _, ev: &DockEvent, window, cx| {
                    if let DockEvent::LayoutChanged = ev {
                        r.sync(window, cx);
                    }
                },
            ),
            cx.observe_in(&ws, window, |r: &mut Self, _, window, cx| {
                r.sync(window, cx)
            }),
        ];
        let tab = |cx: &mut Context<Self>| cx.focus_handle().tab_stop(true);
        Self {
            ws: ws.downgrade(),
            workbench,
            dock,
            current: None,
            drawer: None,
            recent: Vec::new(),
            inner: Vec::new(),
            terminal_panels: HashSet::new(),
            slots: HashMap::new(),
            shapes: HashMap::new(),
            shown_task: None,
            quiet: Arc::new(AtomicBool::new(false)),
            rows: Vec::new(),
            tabs: Vec::new(),
            attention: Vec::new(),
            font: 13.,
            last_focus: None,
            buttons: [tab(cx), tab(cx), tab(cx), tab(cx), tab(cx)],
            strip_focus: tab(cx),
            switcher_focus: tab(cx),
            list_focus: tab(cx),
            strip_cursor: None,
            list_cursor: 0,
            list_scroll: UniformListScrollHandle::new(),
            opener: None,
            _subs: subs,
        }
    }

    /// Panels of the current task (terminals first, in task order) and the global ones.
    fn desired(&self) -> (Vec<PanelId>, Vec<PanelId>) {
        let Some(cur) = self.current.as_deref() else {
            return (
                Vec::new(),
                self.inner
                    .iter()
                    .filter(|(_, _, o)| *o == Owner::Global)
                    .map(|(id, _, _)| *id)
                    .collect(),
            );
        };
        let mine = |o: &Owner| matches!(o, Owner::Task(t) if t == cur);
        let mut terminals = Vec::new();
        let mut others = Vec::new();
        for (id, _, owner) in &self.inner {
            if !mine(owner) {
                continue;
            }
            if self.terminal_panels.contains(id) {
                terminals.push(*id);
            } else {
                others.push(*id);
            }
        }
        others.extend(
            self.inner
                .iter()
                .filter(|(_, _, o)| *o == Owner::Global)
                .map(|(id, _, _)| *id),
        );
        (terminals, others)
    }

    /// Re-read the workbench dock and the workspace data, then show the current
    /// task's panels. Runs on change events only.
    pub(crate) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        // Workbench panels and their owners.
        let mut terminal_panels = HashSet::new();
        let inner: Vec<(PanelId, Arc<dyn PanelView>, Owner)> = {
            let w = ws.read(cx);
            let area = self.workbench.read(cx);
            let mut ids = Vec::new();
            for placement in [
                DockPlacement::Center,
                DockPlacement::Left,
                DockPlacement::Bottom,
                DockPlacement::Right,
            ] {
                if let Some(tree) = area.layout(placement) {
                    ids.extend(tree.panels());
                }
            }
            let mut order: HashMap<&str, usize> = HashMap::new();
            for t in &w.tasks {
                for (i, id) in t.terminals.iter().enumerate() {
                    order.insert(id.as_str(), i);
                }
            }
            let mut v: Vec<(PanelId, Arc<dyn PanelView>, Owner, usize)> = ids
                .into_iter()
                .filter_map(|id| {
                    area.panel(id)
                        .and_then(PanelHandle::of)
                        .map(|h| (id, h.panel()))
                })
                .map(|(id, h)| {
                    let state = h.dump(cx);
                    let terminal = (state.panel_name == crate::panel::PANEL_NAME)
                        .then(|| crate::layout::leaf_field(&state, "terminal").map(str::to_owned))
                        .flatten();
                    let owner = match &terminal {
                        Some(t) => w
                            .tasks
                            .iter()
                            .find(|x| x.terminals.iter().any(|y| y == t))
                            .map(|x| x.id.clone())
                            .or_else(|| w.terminal(t).and_then(|x| x.task_id.clone()))
                            .map(Owner::Task)
                            .unwrap_or(Owner::Global),
                        None => match crate::layout::leaf_field(&state, "task") {
                            Some(task)
                                if matches!(
                                    state.panel_name.as_str(),
                                    crate::editor_panel::EDITOR_PANEL_NAME
                                        | crate::review::REVIEW_PANEL_NAME
                                ) =>
                            {
                                Owner::Task(task.to_owned())
                            }
                            _ => Owner::Global,
                        },
                    };
                    if terminal.is_some() {
                        terminal_panels.insert(id);
                    }
                    let rank = terminal
                        .as_deref()
                        .and_then(|t| order.get(t).copied())
                        .unwrap_or(usize::MAX);
                    (id, h, owner, rank)
                })
                .collect();
            // Terminals in task order (stable for everything else).
            v.sort_by_key(|(_, _, _, rank)| *rank);
            v.into_iter().map(|(id, h, o, _)| (id, h, o)).collect()
        };
        self.inner = inner;
        self.terminal_panels = terminal_panels;
        let open: HashSet<String> = self
            .inner
            .iter()
            .filter_map(|(_, _, o)| {
                if let Owner::Task(t) = o {
                    Some(t.clone())
                } else {
                    None
                }
            })
            .collect();
        // Rows, current task, tabs, 응답 대기.
        {
            let w = ws.read(cx);
            self.rows = task_rows(&w.tasks, &w.terminals, &w.orch.pending(), &open);
            if self
                .current
                .as_ref()
                .is_none_or(|c| !self.rows.iter().any(|r| &r.id == c))
            {
                self.current = w
                    .current_task(window, cx)
                    .filter(|t| self.rows.iter().any(|r| &r.id == t))
                    .or_else(|| self.rows.iter().find(|r| r.open).map(|r| r.id.clone()))
                    .or_else(|| self.rows.first().map(|r| r.id.clone()));
            }
            let settings = cx.global::<crate::theme::Settings>();
            self.font = crate::theme::ui_font_size(
                settings.font_size,
                pod_ui::themes::metrics().density.ui_font,
            );
        }
        self.recent
            .retain(|id| self.rows.iter().any(|r| &r.id == id));
        self.tabs = tab_tasks(&self.rows, &self.recent, self.current.as_deref());
        self.attention = attention(&self.rows, self.current.as_deref());
        self.show_current(window, cx);
        cx.notify();
    }

    /// Put the current task's panels into the rail dock (no-op when they already are).
    fn show_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let live: HashSet<PanelId> = self.inner.iter().map(|(id, _, _)| *id).collect();
        self.slots.retain(|id, _| live.contains(id));
        let (terminals, others) = self.desired();
        for (id, inner, _) in &self.inner {
            if !self.slots.contains_key(id) && (terminals.contains(id) || others.contains(id)) {
                let slot = Slot {
                    inner: inner.clone(),
                    workbench: self.workbench.downgrade(),
                    quiet: self.quiet.clone(),
                };
                self.slots
                    .insert(*id, Arc::new(PanelHandle::from_view(Arc::new(slot))));
            }
        }
        let now = self
            .dock
            .read(cx)
            .layout(DockPlacement::Center)
            .and_then(|t| shape_of(t.root()));
        if self.shown_task != self.current
            && let (Some(task), Some(shape)) = (self.shown_task.clone(), now.clone())
        {
            self.shapes.insert(task, shape);
        }
        let prev = if self.shown_task == self.current {
            now.clone()
        } else {
            self.current
                .as_ref()
                .and_then(|t| self.shapes.get(t).cloned())
        };
        let main = window.viewport_size().width.to_f64() as f32 * 0.62;
        let next = plan(prev, &terminals, &others, Some(main));
        self.shown_task = self.current.clone();
        if next == now {
            return;
        }
        let layout = match &next {
            Some(Shape::Tabs { .. }) => DockLayout::h_split().child(
                to_layout(next.as_ref().expect("tabs"), &self.slots, cx),
                None,
            ),
            Some(shape) => to_layout(shape, &self.slots, cx),
            None => DockLayout::h_split(),
        };
        self.quiet.store(true, Ordering::Relaxed);
        self.dock
            .update(cx, |a, cx| a.set_center(layout, window, cx));
        self.quiet.store(false, Ordering::Relaxed);
    }

    /// Make `task` the current task. `focus_content` moves focus to its shown panel.
    pub(crate) fn switch_to(
        &mut self,
        task: &str,
        focus_content: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.rows.iter().any(|r| r.id == task) {
            return;
        }
        if self.current.as_deref() != Some(task) {
            if let Some(prev) = self.current.take() {
                self.recent.retain(|x| x != &prev);
                self.recent.insert(0, prev);
                self.recent.truncate(RECENT_MAX);
            }
            self.current = Some(task.to_owned());
            self.strip_cursor = Some(task.to_owned());
            self.tabs = tab_tasks(&self.rows, &self.recent, self.current.as_deref());
            self.attention = attention(&self.rows, self.current.as_deref());
            self.show_current(window, cx);
        }
        if focus_content {
            self.focus_content(window, cx);
        }
        cx.notify();
    }

    /// Focus the visible panel of the first group, else the task tabs.
    fn focus_content(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shown = self
            .dock
            .read(cx)
            .layout(DockPlacement::Center)
            .and_then(|t| shape_of(t.root()))
            .and_then(|s| s.first_shown());
        match shown.and_then(|id| self.inner.iter().find(|(x, _, _)| *x == id)) {
            Some((_, inner, _)) => window.focus(&inner.focus_handle(cx), cx),
            None => window.focus(&self.strip_focus, cx),
        }
    }

    fn select_in_rail(&mut self, id: PanelId, window: &mut Window, cx: &mut Context<Self>) {
        self.dock.update(cx, |a, cx| a.select_panel(id, window, cx));
    }

    /// Forget the current task's arrangement and lay it out again (Ctrl+Shift+G here).
    pub(crate) fn rearrange(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.current.clone() {
            self.shapes.remove(&t);
            self.shown_task = None;
            self.show_current(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn current_row(&self) -> Option<&TaskRow> {
        self.current
            .as_deref()
            .and_then(|c| self.rows.iter().find(|r| r.id == c))
    }

    /// Panels in the rail dock right now.
    pub(crate) fn shown_panels(&self, cx: &App) -> Vec<PanelId> {
        self.dock
            .read(cx)
            .layout(DockPlacement::Center)
            .map(|t| t.panels().collect())
            .unwrap_or_default()
    }

    // ---- drawers ----

    pub(crate) fn toggle_drawer(
        &mut self,
        which: Drawer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.drawer == Some(which) {
            self.close_drawer(window, cx);
            return;
        }
        self.opener = window
            .focused(cx)
            .or_else(|| Some(self.opener_of(which).clone()));
        self.drawer = Some(which);
        self.list_cursor = match which {
            Drawer::Tasks => self
                .current
                .as_deref()
                .and_then(|c| self.rows.iter().position(|r| r.id == c))
                .unwrap_or(0),
            _ => 0,
        };
        self.list_scroll
            .scroll_to_item(self.list_cursor, ScrollStrategy::Center);
        match which {
            Drawer::Workspace => {
                if let Some(ws) = self.ws.upgrade() {
                    let h = ws.read(cx).sidebar_focus_handle(cx);
                    window.focus(&h, cx);
                }
            }
            _ => window.focus(&self.list_focus, cx),
        }
        cx.notify();
    }

    fn opener_of(&self, which: Drawer) -> &FocusHandle {
        match which {
            Drawer::Workspace => &self.buttons[RailButton::Workspace as usize],
            Drawer::Attention => &self.buttons[RailButton::Attention as usize],
            Drawer::Tasks => &self.switcher_focus,
        }
    }

    pub(crate) fn close_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(which) = self.drawer.take() else {
            return;
        };
        let back = self
            .opener
            .take()
            .unwrap_or_else(|| self.opener_of(which).clone());
        window.focus(&back, cx);
        cx.notify();
    }

    fn list_len(&self) -> usize {
        match self.drawer {
            Some(Drawer::Tasks) => self.rows.len(),
            Some(Drawer::Attention) => self.attention.len(),
            _ => 0,
        }
    }

    /// Enter on a drawer row.
    fn activate_row(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let row = match self.drawer {
            Some(Drawer::Tasks) => self.rows.get(ix).cloned(),
            Some(Drawer::Attention) => self
                .attention
                .get(ix)
                .and_then(|i| self.rows.get(*i))
                .cloned(),
            _ => None,
        };
        let Some(row) = row else { return };
        let attention = self.drawer == Some(Drawer::Attention);
        self.drawer = None;
        self.opener = None;
        self.switch_to(&row.id, true, window, cx);
        if attention {
            let ws = self.ws.clone();
            let target = row.first_attention.clone();
            let gates_only = target.is_none() && row.gates > 0;
            // After this update: the workspace opens (or focuses) the waiting terminal.
            cx.defer_in(window, move |_, window, cx| {
                let _ = ws.update(cx, |w, cx| match target {
                    Some(id) => w.open_terminal(&id, false, window, cx),
                    None if gates_only => w.open_gates(window, cx),
                    None => {}
                });
            });
        }
    }

    // ---- rail buttons ----

    fn press(&mut self, b: RailButton, window: &mut Window, cx: &mut Context<Self>) {
        match b {
            RailButton::Workspace => self.toggle_drawer(Drawer::Workspace, window, cx),
            RailButton::Attention => self.toggle_drawer(Drawer::Attention, window, cx),
            RailButton::Terminal => {
                self.drawer = None;
                let (terminals, _) = self.desired();
                if let Some(id) = terminals.first().copied() {
                    let shown = terminals
                        .iter()
                        .copied()
                        .find(|t| {
                            self.dock
                                .read(cx)
                                .layout(DockPlacement::Center)
                                .and_then(|x| shape_of(x.root()))
                                .is_some_and(|s| s.is_shown(*t))
                        })
                        .unwrap_or(id);
                    self.select_in_rail(shown, window, cx);
                    if let Some((_, inner, _)) = self.inner.iter().find(|(x, _, _)| *x == shown) {
                        window.focus(&inner.focus_handle(cx), cx);
                    }
                } else {
                    self.open_task_terminals(window, cx);
                }
            }
            RailButton::Files => {
                self.drawer = None;
                if let (Some(ws), Some(task)) = (self.ws.upgrade(), self.current.clone()) {
                    cx.defer_in(window, move |_, window, cx| {
                        ws.update(cx, |w, cx| w.open_editor(&task, None, false, window, cx))
                    });
                }
            }
            RailButton::Agents => {
                self.drawer = None;
                if let Some(ws) = self.ws.upgrade() {
                    cx.defer_in(window, move |_, window, cx| {
                        ws.update(cx, |w, cx| w.open_agent_tree(window, cx))
                    });
                }
            }
        }
        cx.notify();
    }

    /// Open every running terminal of the current task (or start a shell when it has none), laid out fresh.
    pub(crate) fn open_task_terminals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(ws), Some(task)) = (self.ws.upgrade(), self.current.clone()) else {
            return;
        };
        let rail = cx.entity().downgrade();
        cx.defer_in(window, move |_, window, cx| {
            ws.update(cx, |w, cx| {
                let ids: Vec<String> = w
                    .task(&task)
                    .map(|t| {
                        t.terminals
                            .iter()
                            .filter(|id| w.terminal(id).is_some_and(|x| x.is_running()))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default();
                if ids.is_empty() {
                    w.task_terminal(task.clone(), window, cx);
                }
                for id in ids.iter().rev() {
                    w.open_terminal(id, false, window, cx);
                }
            });
            let _ = rail.update(cx, |r, cx| {
                r.rearrange(window, cx);
                r.focus_content(window, cx);
            });
        });
    }

    /// Focus moved to a dock panel: follow it to its task, show it, close a drawer.
    fn follow_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = window.focused(cx);
        if focused == self.last_focus {
            return;
        }
        self.last_focus = focused.clone();
        let Some(f) = focused else { return };
        let hit = self.inner.iter().find(|(_, inner, _)| {
            let h = inner.focus_handle(cx);
            h == f || h.contains_focused(window, cx)
        });
        let Some((id, owner)) = hit.map(|(id, _, o)| (*id, o.clone())) else {
            return;
        };
        // The panel may not be on screen yet (another task): GPUI drops focus
        // from an element it did not paint, so focus it again once it is.
        cx.defer_in(window, move |r, window, cx| {
            if r.drawer.is_some() {
                r.drawer = None;
                r.opener = None;
            }
            if let Owner::Task(t) = &owner
                && r.current.as_deref() != Some(t.as_str())
            {
                r.switch_to(t, false, window, cx);
            }
            let shown = r
                .dock
                .read(cx)
                .layout(DockPlacement::Center)
                .and_then(|x| shape_of(x.root()))
                .is_some_and(|s| s.is_shown(id));
            if !shown {
                r.select_in_rail(id, window, cx);
            }
            if !f.is_focused(window) {
                window.focus(&f, cx);
                r.last_focus = Some(f.clone());
            }
            cx.notify();
        });
    }
}

// ---- rendering ----------------------------------------------------------------

fn render_body(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let id = cx.entity_id();
    let rail = match state_of(id, cx) {
        Some(r) => r,
        None => {
            let me = cx.entity();
            let workbench = ws.dock.clone();
            let r = cx.new(|cx| RailState::new(me, workbench, window, cx));
            cx.default_global::<RailStates>().0.insert(id, r.clone());
            // The first frame has an empty rail dock: keep the focused panel focused.
            let first = r.clone();
            let focused = window.focused(cx);
            window.defer(cx, move |window, cx| {
                first.update(cx, |r, cx| r.sync(window, cx));
                if let Some(f) = focused {
                    window.focus(&f, cx);
                }
            });
            r
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
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .relative()
        .when_some(banner, |d, b| {
            d.child(div().px_2().pt_2().child(
                pod_ui::banner(pod_ui::Tone::Warning, crate::theme::mode(), b, None).text_xs(),
            ))
        })
        .when_some(ws.render_hook_banner(cx), |d, b| d.child(b))
        .child(div().flex().flex_1().min_h_0().child(rail))
        .child(
            div()
                .absolute()
                .left(px(RAIL_WIDTH + 8.))
                .bottom(px(8.))
                .w(px(320.))
                .child(ws.render_toasts(cx)),
        )
        .into_any_element()
}

impl Render for RailState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_focus(window, cx);
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        use pod_ui::themes::{Shadow, Surface};
        let rounded = matches!(m.surface, Surface::Layered | Surface::Islands);
        let gap = if rounded { m.gap } else { 0. };
        let lines = !matches!(m.surface, Surface::Mono);
        let border = m.border_width.max(1.);
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
                    .shadow(pod_ui::tokens::elevation(1, mode))
                    .border_1()
                    .border_color(rgb(tk.border_subtle)),
                (_, Shadow::Soft) => d.shadow(pod_ui::tokens::elevation(1, mode)),
                _ => d,
            }
        };

        // Rail.
        let attention_n = self.attention.len();
        let rail_button = |this: &Self,
                           b: RailButton,
                           icon: pod_ui::Icon,
                           label: String,
                           tip: &'static str,
                           selected: bool,
                           expanded: Option<bool>,
                           badge: Option<(usize, u32, u32)>,
                           cx: &mut Context<Self>| {
            let focus = this.buttons[b as usize].clone();
            div()
                .id(SharedString::from(format!("rail-{b:?}").to_lowercase()))
                .track_focus(&focus)
                .role(Role::Button)
                .aria_label(label)
                .when_some(expanded, |d, e| d.aria_expanded(e))
                .size(px(36.))
                .flex_shrink_0()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(m.radius.control))
                .cursor_pointer()
                .when(selected, |d| d.bg(rgb(tk.bg_selected)))
                .when(!selected, |d| d.hover(|s| s.bg(rgb(tk.bg_hover))))
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .when(selected, |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(-4.))
                            .top(px(8.))
                            .bottom(px(8.))
                            .w(px(2.))
                            .rounded(px(1.))
                            .bg(rgb(tk.accent_fg)),
                    )
                })
                .child(pod_ui::icon_sized(
                    icon,
                    18.,
                    if selected {
                        tk.text_primary
                    } else {
                        tk.text_secondary
                    },
                ))
                .when_some(badge, |d, (n, fg, bg)| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-2.))
                            .right(px(-4.))
                            .child(pod_ui::count_badge(n, fg, bg)),
                    )
                })
                .on_click(cx.listener(move |r, _: &ClickEvent, window, cx| r.press(b, window, cx)))
                .on_key_down(cx.listener(move |r, e: &KeyDownEvent, window, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        r.press(b, window, cx);
                        cx.stop_propagation();
                    }
                }))
        };
        let attention_label = if attention_n == 0 {
            format!("{}: 없음", s::ATTENTION)
        } else {
            format!("{}: 다른 작업 {attention_n}개", s::ATTENTION)
        };
        let drawer = self.drawer;
        let rail = island(div())
            .id("rail")
            .role(Role::Toolbar)
            .aria_label(s::RAIL)
            .w(px(RAIL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py(px(6.))
            .when(!rounded, |d| d.bg(rgb(tk.bg_chrome)))
            .when(!rounded && lines, |d| {
                d.border_r(px(border)).border_color(rgb(tk.border_subtle))
            })
            .qa_id(|| "rail".into())
            .child(rail_button(
                self,
                RailButton::Workspace,
                pod_ui::Icon::PanelLeft,
                s::WORKSPACE.into(),
                s::WORKSPACE_TIP,
                drawer == Some(Drawer::Workspace),
                Some(drawer == Some(Drawer::Workspace)),
                None,
                cx,
            ))
            .child(rail_button(
                self,
                RailButton::Terminal,
                pod_ui::Icon::SquareTerminal,
                s::TERMINAL.into(),
                s::TERMINAL_TIP,
                false,
                None,
                None,
                cx,
            ))
            .child(rail_button(
                self,
                RailButton::Files,
                pod_ui::Icon::FileText,
                s::FILES.into(),
                s::FILES_TIP,
                false,
                None,
                None,
                cx,
            ))
            .child(rail_button(
                self,
                RailButton::Agents,
                pod_ui::Icon::ListTree,
                s::AGENTS.into(),
                s::AGENTS_TIP,
                false,
                None,
                None,
                cx,
            ))
            .child(div().flex_1())
            .child(rail_button(
                self,
                RailButton::Attention,
                pod_ui::Icon::Bell,
                attention_label,
                "응답 대기 — 다른 작업의 승인·실패 (Ctrl+Alt+A)",
                drawer == Some(Drawer::Attention),
                Some(drawer == Some(Drawer::Attention)),
                (attention_n > 0).then_some((attention_n, tk.warning_fg, tk.warning_bg)),
                cx,
            ));

        // Task tabs.
        let tab_h = m.density.tabbar.max(32.);
        let font = self.font;
        let widths: Vec<f32> = self
            .tabs
            .iter()
            .map(|i| {
                tab_width(
                    &self.rows[*i].title,
                    font,
                    self.rows[*i].needs_you(),
                    self.rows[*i].agents.len(),
                )
            })
            .collect();
        let current_ix = self
            .current
            .as_deref()
            .and_then(|c| self.tabs.iter().position(|i| self.rows[*i].id == c));
        // Room: window minus the rail, the gaps, "+", "+N" and 작업 전환.
        let switcher_w = text_width(s::SWITCHER, font) + 48.;
        let avail = window.viewport_size().width.to_f64() as f32
            - RAIL_WIDTH
            - 3. * gap
            - 36.
            - 56.
            - switcher_w
            - 16.;
        let visible: Vec<usize> = fit_tabs(&widths, current_ix, avail.max(0.))
            .into_iter()
            .map(|k| self.tabs[k])
            .collect();
        let hidden = self.tabs.len().saturating_sub(visible.len());
        let strip_focused = self.strip_focus.is_focused(window);
        let cursor = self.strip_cursor.clone().or_else(|| self.current.clone());
        let n_tabs = visible.len();
        let tabs = visible
            .iter()
            .enumerate()
            .map(|(k, i)| {
                let row = &self.rows[*i];
                let is_current = self.current.as_deref() == Some(row.id.as_str());
                let at_cursor = strip_focused && cursor.as_deref() == Some(row.id.as_str());
                let dot = row
                    .summary
                    .priority()
                    .map(|st| pod_ui::status_visual(st).tone.colors(tk).0);
                let badge = if row.summary.error > 0 && row.summary.blocked + row.gates == 0 {
                    Some(pod_ui::count_badge(
                        row.summary.error,
                        tk.danger_fg,
                        tk.danger_bg,
                    ))
                } else if row.summary.blocked + row.gates > 0 {
                    Some(pod_ui::count_badge(
                        row.summary.blocked + row.gates,
                        tk.warning_fg,
                        tk.warning_bg,
                    ))
                } else {
                    None
                };
                let id = row.id.clone();
                let tip = format!(
                    "{} · {}",
                    row.branch.as_deref().unwrap_or("—"),
                    row.summary.label()
                );
                div()
                    .id(SharedString::from(format!("rail-tab-{}", row.id)))
                    .role(Role::Tab)
                    .aria_label(row.accessible_name())
                    .aria_selected(is_current)
                    .aria_position_in_set(k + 1)
                    .aria_size_of_set(n_tabs)
                    .h_full()
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_2()
                    .border_color(if is_current {
                        rgb(tk.accent_fg).into()
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .text_color(rgb(if is_current {
                        tk.text_primary
                    } else {
                        tk.text_secondary
                    }))
                    .when(is_current, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(at_cursor, |d| {
                        d.bg(rgb(tk.bg_hover)).border_color(rgb(tk.focus_ring))
                    })
                    .when(!is_current, |d| d.hover(|s| s.bg(rgb(tk.bg_hover))))
                    .cursor_pointer()
                    .when(is_current, |d| d.qa_id(|| "rail-current-tab".into()))
                    .child(
                        div()
                            .w(px(8.))
                            .flex_shrink_0()
                            .when_some(dot, |d, c| d.child(pod_ui::status_dot(c))),
                    )
                    .child(
                        div()
                            .max_w(px(TAB_TEXT_MAX))
                            .min_w_0()
                            .truncate()
                            .child(row.title.clone()),
                    )
                    .when(!row.agents.is_empty(), |d| {
                        d.child(crate::agent_kind::stack_el(
                            &format!("rail-tab-{}", row.id),
                            &row.agents,
                            12.,
                        ))
                    })
                    .when_some(badge, |d, b| d.child(b))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(move |r, _: &ClickEvent, window, cx| {
                        r.strip_cursor = Some(id.clone());
                        r.switch_to(&id, true, window, cx);
                    }))
            })
            .collect::<Vec<_>>();
        let visible_ids: Vec<String> = visible.iter().map(|i| self.rows[*i].id.clone()).collect();
        let strip = div()
            .id("rail-task-tabs")
            .track_focus(&self.strip_focus)
            .role(Role::TabList)
            .aria_label(s::TABS)
            .aria_keyshortcuts("Control+Alt+PageDown Control+Alt+PageUp")
            .h_full()
            .min_w_0()
            .flex()
            .overflow_hidden()
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .children(tabs)
            .on_key_down(cx.listener(move |r, e: &KeyDownEvent, window, cx| {
                let at = r
                    .strip_cursor
                    .clone()
                    .or_else(|| r.current.clone())
                    .and_then(|c| visible_ids.iter().position(|x| *x == c))
                    .unwrap_or(0);
                let next = match e.keystroke.key.as_str() {
                    "left" => Some(at.saturating_sub(1)),
                    "right" => Some((at + 1).min(visible_ids.len().saturating_sub(1))),
                    "home" => Some(0),
                    "end" => Some(visible_ids.len().saturating_sub(1)),
                    "enter" | "space" => {
                        if let Some(id) = visible_ids.get(at).cloned() {
                            r.switch_to(&id, false, window, cx);
                        }
                        cx.stop_propagation();
                        None
                    }
                    _ => None,
                };
                if let Some(i) = next {
                    r.strip_cursor = visible_ids.get(i).cloned();
                    cx.stop_propagation();
                    cx.notify();
                }
            }));
        let ws_new = self.ws.clone();
        let tasks_open = drawer == Some(Drawer::Tasks);
        let switcher = div()
            .id("rail-switcher")
            .track_focus(&self.switcher_focus)
            .role(Role::Button)
            .aria_label(s::SWITCHER)
            .aria_expanded(tasks_open)
            .aria_keyshortcuts("Control+Alt+T")
            .h(px(28.))
            .px_2()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .rounded(px(m.radius.control))
            .text_color(rgb(tk.text_secondary))
            .cursor_pointer()
            .when(tasks_open, |d| d.bg(rgb(tk.bg_selected)))
            .hover(|s| s.bg(rgb(tk.bg_hover)))
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(s::SWITCHER_TIP).build(window, cx)
            })
            .qa_id(|| "rail-switcher".into())
            .child(div().flex_shrink_0().child(s::SWITCHER))
            .child(pod_ui::icon_sized(
                pod_ui::Icon::ChevronDown,
                14.,
                tk.text_muted,
            ))
            .on_click(cx.listener(|r, _: &ClickEvent, window, cx| {
                r.toggle_drawer(Drawer::Tasks, window, cx)
            }))
            .on_key_down(cx.listener(|r, e: &KeyDownEvent, window, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space" | "down") {
                    r.toggle_drawer(Drawer::Tasks, window, cx);
                    cx.stop_propagation();
                }
            }));
        let more = (hidden > 0).then(|| {
            div()
                .id("rail-more-tasks")
                .role(Role::Button)
                .aria_label(format!("숨은 작업 {hidden}개 — {}", s::SWITCHER))
                .tab_index(0)
                .h(px(24.))
                .px_2()
                .flex_shrink_0()
                .flex()
                .items_center()
                .rounded(px(m.radius.control))
                .border_1()
                .border_color(rgb(tk.border_subtle))
                .text_xs()
                .text_color(rgb(tk.text_secondary))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk.bg_hover)))
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .child(format!("+{hidden}"))
                .on_click(cx.listener(|r, _: &ClickEvent, window, cx| {
                    r.toggle_drawer(Drawer::Tasks, window, cx)
                }))
                .on_key_down(cx.listener(|r, e: &KeyDownEvent, window, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        r.toggle_drawer(Drawer::Tasks, window, cx);
                        cx.stop_propagation();
                    }
                }))
        });
        let top = div()
            .h(px(tab_h))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .pr_2()
            .text_size(px(font))
            .when(!rounded, |d| d.bg(rgb(tk.bg_chrome)))
            .when(!rounded && lines, |d| {
                d.border_b(px(border)).border_color(rgb(tk.border_subtle))
            })
            .qa_id(|| "rail-tabs".into())
            .child(strip)
            .child(
                pod_ui::IconButton::new(
                    "rail-new-task",
                    pod_ui::Icon::Plus,
                    crate::strings::NEW_TASK,
                )
                .tooltip("새 작업 (Ctrl+N)")
                .render(mode, move |window, cx| {
                    let _ = ws_new.update(cx, |w, cx| w.open_new_task_dialog(window, cx));
                }),
            )
            .child(div().flex_1())
            .children(more)
            .child(switcher);

        // Dock (or the empty state).
        let shown_empty = self.shown_panels(cx).is_empty();
        let empty = shown_empty.then(|| self.render_empty(cx));
        let main = island(div())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .relative()
            .child(top)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.dock.clone())
                    .children(empty)
                    .children(self.render_drawer(window, cx)),
            );

        div()
            .id("rail-body")
            .key_context(CONTEXT)
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
            .on_action(cx.listener(|r, _: &NextTask, window, cx| r.step_task(1, window, cx)))
            .on_action(cx.listener(|r, _: &PrevTask, window, cx| r.step_task(-1, window, cx)))
            .on_action(cx.listener(|r, _: &TaskSwitcher, window, cx| {
                r.toggle_drawer(Drawer::Tasks, window, cx)
            }))
            .on_action(cx.listener(|r, _: &AttentionDrawer, window, cx| {
                r.toggle_drawer(Drawer::Attention, window, cx)
            }))
            .on_action(cx.listener(|r, _: &WorkspaceDrawer, window, cx| {
                r.toggle_drawer(Drawer::Workspace, window, cx)
            }))
            .on_action(
                cx.listener(|r, _: &crate::workspace::ArrangeGrid, window, cx| {
                    r.rearrange(window, cx)
                }),
            )
            .child(rail)
            .child(main)
    }
}

impl RailState {
    fn step_task(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.tabs.iter().map(|i| self.rows[*i].id.clone()).collect();
        if let Some(next) = step(&ids, self.current.as_deref(), delta) {
            let keep_strip = self.strip_focus.is_focused(window);
            self.switch_to(&next, !keep_strip, window, cx);
        }
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = crate::theme::tokens();
        let mode = crate::theme::mode();
        let row = self.current_row().cloned();
        let running = row
            .as_ref()
            .and_then(|r| self.ws.upgrade().map(|ws| (r, ws)))
            .map(|(r, ws)| {
                ws.read(cx)
                    .task(&r.id)
                    .map(|t| {
                        t.terminals
                            .iter()
                            .filter(|id| ws.read(cx).terminal(id).is_some_and(|x| x.is_running()))
                            .count()
                    })
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        let (title, body) = match &row {
            Some(r) => (format!("{} — {}", r.title, s::EMPTY_TITLE), s::EMPTY_BODY),
            None => (s::NO_TASKS.to_string(), s::NO_TASK_BODY),
        };
        let ws = self.ws.clone();
        let task = row.as_ref().map(|r| r.id.clone());
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .bg(rgb(tk.bg_canvas))
            .text_sm()
            .text_color(rgb(tk.text_muted))
            .child(
                pod_ui::Frame::new("rail-empty", pod_ui::FrameKind::Empty, title, body).render(
                    mode,
                    [div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .when(row.is_some(), |d| {
                            d.child(
                                Button::new("rail-open-terminals")
                                    .label(if running > 0 {
                                        format!("터미널 열기 ({running})")
                                    } else {
                                        "새 셸 터미널".to_string()
                                    })
                                    .primary()
                                    .on_click(cx.listener(|r, _, window, cx| {
                                        r.open_task_terminals(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("rail-open-files")
                                    .label(crate::strings::OPEN_FILES)
                                    .on_click({
                                        let ws = ws.clone();
                                        let task = task.clone();
                                        move |_, window, cx| {
                                            if let Some(task) = &task {
                                                let _ = ws.update(cx, |w, cx| {
                                                    w.open_editor(task, None, false, window, cx)
                                                });
                                            }
                                        }
                                    }),
                            )
                        })
                        .child(
                            Button::new("rail-empty-new-task")
                                .label(crate::strings::NEW_TASK)
                                .on_click(move |_, window, cx| {
                                    let _ =
                                        ws.update(cx, |w, cx| w.open_new_task_dialog(window, cx));
                                }),
                        )
                        .into_any_element()],
                ),
            )
            .into_any_element()
    }

    fn render_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let which = self.drawer?;
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        let (title, width) = match which {
            Drawer::Workspace => (s::WORKSPACE, m.density.sidebar),
            Drawer::Attention => (s::ATTENTION_TITLE, 360.),
            Drawer::Tasks => (s::ALL_TASKS, 380.),
        };
        let header = div()
            .h(px(m.density.panel_header.max(32.)))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .pl_3()
            .pr_1()
            .border_b_1()
            .border_color(rgb(tk.border_subtle))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                pod_ui::IconButton::new("rail-drawer-close", pod_ui::Icon::X, s::CLOSE)
                    .tooltip("닫기 (Esc)")
                    .render(mode, {
                        let me = cx.entity().downgrade();
                        move |window, cx| {
                            let _ = me.update(cx, |r, cx| r.close_drawer(window, cx));
                        }
                    }),
            );
        let body: AnyElement = match which {
            Drawer::Workspace => match self.ws.upgrade() {
                Some(ws) => div()
                    .flex_1()
                    .min_h_0()
                    .pt_2()
                    .child(ws.read(cx).sidebar.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Drawer::Tasks | Drawer::Attention => self.render_list(which, window, cx),
        };
        let footer = (which == Drawer::Attention).then(|| {
            let ws = self.ws.clone();
            let ws2 = self.ws.clone();
            let gates = self.rows.iter().map(|r| r.gates).sum::<usize>();
            div()
                .flex_shrink_0()
                .flex()
                .flex_wrap()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(rgb(tk.border_subtle))
                .child(
                    Button::new("rail-all-notifications")
                        .label(s::ALL_NOTIFICATIONS)
                        .on_click(move |_, window, cx| {
                            let _ = ws.update(cx, |w, cx| w.open_notifications(window, cx));
                        }),
                )
                .when(gates > 0, |d| {
                    d.child(
                        Button::new("rail-open-gates")
                            .label(format!("{} ({gates})", s::GATES))
                            .on_click(move |_, window, cx| {
                                let _ = ws2.update(cx, |w, cx| w.open_gates(window, cx));
                            }),
                    )
                })
        });
        let left = which != Drawer::Tasks;
        Some(
            div()
                .id("rail-drawer")
                .role(Role::Dialog)
                .aria_label(title)
                .absolute()
                .top_0()
                .when(left, |d| d.left_0().bottom_0())
                .when(!left, |d| d.right(px(8.)).max_h(relative(0.8)))
                .w(px(width))
                .max_w(relative(0.9))
                .flex()
                .flex_col()
                .bg(rgb(tk.bg_raised))
                .border_1()
                .border_color(rgb(tk.border_strong))
                .when(!left, |d| d.rounded_b(px(m.radius.overlay)))
                .shadow(pod_ui::tokens::elevation(2, mode))
                .occlude()
                .qa_id(|| "rail-drawer".into())
                .on_key_down(cx.listener(|r, e: &KeyDownEvent, window, cx| {
                    if e.keystroke.key == "escape" {
                        r.close_drawer(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(header)
                .child(body)
                .children(footer)
                .into_any_element(),
        )
    }

    fn render_list(
        &mut self,
        which: Drawer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        let len = self.list_len();
        let focused = self.list_focus.is_focused(window);
        if len == 0 {
            let text = if which == Drawer::Attention {
                s::ATTENTION_EMPTY
            } else {
                s::NO_TASKS
            };
            return div()
                .id("rail-list")
                .track_focus(&self.list_focus)
                .role(Role::List)
                .aria_label(text)
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .p_3()
                .text_sm()
                .text_color(rgb(tk.text_muted))
                .child(text)
                .into_any_element();
        }
        let row_h = (m.density.row + 4.).max(30.);
        let list = uniform_list(
            "rail-list-rows",
            len,
            cx.processor(move |r, range: std::ops::Range<usize>, _, cx| {
                let tk = crate::theme::tokens();
                range
                    .map(|ix| {
                        let row_ix = match which {
                            Drawer::Attention => r.attention[ix],
                            _ => ix,
                        };
                        let row = &r.rows[row_ix];
                        let selected = ix == r.list_cursor;
                        let is_current = r.current.as_deref() == Some(row.id.as_str());
                        let (icon, fg) = if row.summary.blocked + row.gates > 0 {
                            (pod_ui::Icon::ShieldQuestion, tk.warning_fg)
                        } else if row.summary.error > 0 {
                            (pod_ui::Icon::OctagonX, tk.danger_fg)
                        } else if is_current {
                            (pod_ui::Icon::Check, tk.accent_fg)
                        } else {
                            (pod_ui::Icon::Circle, tk.text_muted)
                        };
                        let detail = if which == Drawer::Attention {
                            row.attention_label()
                        } else {
                            row.branch.clone().unwrap_or_default()
                        };
                        div()
                            .id(("rail-row", ix))
                            .role(Role::ListItem)
                            .aria_label(if is_current {
                                format!("{} (현재 작업)", row.accessible_name())
                            } else {
                                row.accessible_name()
                            })
                            .aria_selected(selected)
                            .aria_position_in_set(ix + 1)
                            .aria_size_of_set(len)
                            .h(px(row_h))
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .when(selected, |d| d.bg(rgb(tk.bg_selected)))
                            .when(selected && focused, |d| {
                                d.border_l_2().border_color(rgb(tk.focus_ring))
                            })
                            .when(!selected, |d| d.hover(|s| s.bg(rgb(tk.bg_hover))))
                            .child(pod_ui::icon_sized(icon, 14., fg))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(tk.text_primary))
                                    .when(is_current, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .child(row.title.clone()),
                            )
                            .when(!row.agents.is_empty(), |d| {
                                d.child(crate::agent_kind::stack_el(
                                    &format!("rail-row-{}", row.id),
                                    &row.agents,
                                    12.,
                                ))
                            })
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w(px(160.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(if which == Drawer::Attention {
                                        fg
                                    } else {
                                        tk.text_muted
                                    }))
                                    .child(detail),
                            )
                            .on_click(cx.listener(move |r, _: &ClickEvent, window, cx| {
                                r.list_cursor = ix;
                                r.activate_row(ix, window, cx);
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h(px(row_h));
        div()
            .id("rail-list")
            .track_focus(&self.list_focus)
            .role(Role::List)
            .aria_label(if which == Drawer::Attention {
                s::ATTENTION_TITLE
            } else {
                s::ALL_TASKS
            })
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .qa_id(|| "rail-list".into())
            .flex()
            .flex_col()
            .min_h(px(row_h))
            // The switcher drawer sizes to its rows (basis = height, may shrink
            // under max_h); the side drawers fill their full height.
            .when(which == Drawer::Tasks, |d| {
                d.flex_initial().h(px((len as f32 * row_h).min(480.)))
            })
            .when(which != Drawer::Tasks, |d| d.flex_1())
            .child(list)
            .on_key_down(cx.listener(move |r, e: &KeyDownEvent, window, cx| {
                let len = r.list_len();
                let next = match e.keystroke.key.as_str() {
                    "down" => Some((r.list_cursor + 1).min(len.saturating_sub(1))),
                    "up" => Some(r.list_cursor.saturating_sub(1)),
                    "home" => Some(0),
                    "end" => Some(len.saturating_sub(1)),
                    "enter" | "space" => {
                        let ix = r.list_cursor;
                        r.activate_row(ix, window, cx);
                        cx.stop_propagation();
                        None
                    }
                    _ => None,
                };
                if let Some(i) = next {
                    r.list_cursor = i;
                    r.list_scroll.scroll_to_item(i, ScrollStrategy::Center);
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .into_any_element()
    }
}

// ---- fixture scenes (visual QA) ------------------------------------------------

/// `--fixture-scene rail[-attention|-switcher|-workspace]`: task-000's three
/// terminals laid out like the gallery mockup, two more task tabs, and the
/// named drawer open.
pub fn fixture_scene(
    scene: &str,
    w: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    for task in ["sample/task-002", "sample/task-001", "sample/task-000"] {
        let ids: Vec<String> = w
            .task(task)
            .map(|t| t.terminals.clone())
            .unwrap_or_default();
        for id in ids.iter().rev() {
            w.open_terminal(id, false, window, cx);
        }
    }
    let ws = cx.entity_id();
    let scene = scene.to_owned();
    window.defer(cx, move |window, cx| {
        let Some(rail) = state_of(ws, cx) else { return };
        rail.update(cx, |r, cx| {
            r.switch_to("sample/task-000", false, window, cx);
            r.rearrange(window, cx);
            match scene.as_str() {
                "rail-attention" => r.toggle_drawer(Drawer::Attention, window, cx),
                "rail-switcher" => r.toggle_drawer(Drawer::Tasks, window, cx),
                "rail-workspace" => r.toggle_drawer(Drawer::Workspace, window, cx),
                _ => r.focus_content(window, cx),
            }
        });
    });
}

#[cfg(test)]
mod tests;
