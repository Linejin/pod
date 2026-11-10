//! 분할 리뷰 중심 (preset 13, docs/10 §D5.9): the selected task's Review
//! (side-by-side diff, comments, delivery, PR tab) is the main canvas, the
//! task list sits on the left and the task's terminals are docked small at
//! the bottom (up to [`MAX_TILES`], each expandable to the whole canvas).
//!
//! - **No forked state.** Task rows, the header's agent statuses and pending
//!   gates are built every frame from the workspace's tasks, terminals and
//!   gates. The canvas is the real [`ReviewPanel`]: the workbench's own panel
//!   for that task when the dock has one (same entity, nothing reloads), else
//!   a layout-owned one registered with the workspace like any Review panel
//!   (task/status/delivery events reach it). Approve/reject go through
//!   `gate.resolve`, as the gates inbox does.
//! - **No polling.** Everything re-renders from workspace change events.
//! - **Switching is live.** The dock is never touched: terminal panels the
//!   workbench has open are shown in the tiles as they are (no new attach),
//!   so switching back finds the dock exactly as it was. Terminals without a
//!   panel get layout-owned ones, dropped with the layout-owned Review when
//!   the layout changes. Panels opened while the layout shows (settings,
//!   editor …) appear in the canvas's "패널" view, which renders the dock.
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use podd::protocol::{self as p, GateDecision, GateInfo};

use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::panel::TerminalPanel;
use crate::qa_marks::QaId;
use crate::review::ReviewPanel;
use crate::vm::{Liveness, TaskVm, TerminalVm};
use crate::workspace::{ToastKind, Workspace};

actions!(pod_review_layout, [NextRegion, PrevRegion, ToggleExpand]);

/// Key context of the layout (its keys are bound only inside it).
pub const KEY_CONTEXT: &str = "PodReviewLayout";
/// Terminals docked at the bottom per task.
pub const MAX_TILES: usize = 4;

/// Korean UI text of the layout (kept here so the layout stays one file).
pub mod s {
    pub const TASKS: &str = "작업";
    pub const NO_TASKS: &str = "작업이 없습니다. 새 작업을 만들면 여기에서 변경 사항을 검토합니다.";
    pub const NO_REVIEW: &str = "이 podd는 리뷰를 지원하지 않습니다 (프로토콜 v5 필요).";
    pub const REVIEW_TAB: &str = "리뷰";
    pub const PANELS_TAB: &str = "패널";
    pub const CANVAS: &str = "리뷰 화면";
    pub const TERMINALS: &str = "터미널";
    pub const NO_TERMINALS: &str = "이 작업에는 터미널이 없습니다.";
    pub const TERMINALS_HINT: &str = "F6 영역 이동 · Ctrl+Shift+X 크게/작게";
    pub const STRIP_OPEN: &str = "터미널 펼치기";
    pub const STRIP_CLOSE: &str = "터미널 접기";
    pub const GROW: &str = "크게 보기";
    pub const SHRINK: &str = "작게 보기";
    pub const APPROVE: &str = "승인";
    pub const REJECT: &str = "거부";
    pub const ANSWER: &str = "응답하기";
    pub const COMMENTS: &str = "의견";
    pub const GATES: &str = "승인 대기";
    pub const AGENTS: &str = "에이전트 상태";
    pub const NO_BRANCH: &str = "브랜치 없음";
    pub const COULD_NOT_RESOLVE: &str = "승인 요청을 처리하지 못했습니다";
}

// ───────────────────────────── model ─────────────────────────────

/// One task as the left list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub branch: Option<String>,
    pub base: Option<String>,
    /// The task's terminals in display order (see [`task_terminals`]).
    pub terminals: Vec<String>,
    /// Short status counts (`작업 2 · 응답 1`).
    pub status: String,
    pub open_comments: usize,
    pub gates: usize,
}

/// The task's terminals: its own list first (podd's order, agent first),
/// then any other terminal that reports this task. Unknown ids are skipped.
pub fn task_terminals(task: &TaskVm, terminals: &[TerminalVm]) -> Vec<String> {
    let mut ids: Vec<String> = task
        .terminals
        .iter()
        .filter(|id| terminals.iter().any(|t| &t.id == *id))
        .cloned()
        .collect();
    for t in terminals
        .iter()
        .filter(|t| t.task_id.as_deref() == Some(task.id.as_str()))
    {
        if !ids.contains(&t.id) {
            ids.push(t.id.clone());
        }
    }
    ids
}

/// Pending gates of a task: raised for the task or one of its terminals.
pub fn task_gates(task: &str, terminals: &[String], pending: &[GateInfo]) -> Vec<GateInfo> {
    pending
        .iter()
        .filter(|g| {
            g.task.as_deref() == Some(task)
                || g.terminal.as_ref().is_some_and(|t| terminals.contains(t))
        })
        .cloned()
        .collect()
}

/// Rows for every task, in the workspace's order.
pub fn build_rows(
    tasks: &[TaskVm],
    terminals: &[TerminalVm],
    pending: &[GateInfo],
) -> Vec<TaskRow> {
    tasks
        .iter()
        .map(|k| {
            let ids = task_terminals(k, terminals);
            let summary = crate::orch::summary_of(&ids, terminals);
            TaskRow {
                id: k.id.clone(),
                title: k.title.clone(),
                branch: k.branch.clone(),
                base: k.base.clone(),
                status: crate::orch::summary_short(&summary),
                open_comments: k.review_open,
                gates: task_gates(&k.id, &ids, pending).len(),
                terminals: ids,
            }
        })
        .collect()
}

/// `branch → base` (the header's first words).
pub fn branch_line(branch: Option<&str>, base: Option<&str>) -> String {
    match (branch, base) {
        (Some(b), Some(base)) => format!("{b} → {base}"),
        (Some(b), None) => b.to_string(),
        (None, _) => s::NO_BRANCH.to_string(),
    }
}

/// The row's accessible name: everything the row shows, in reading order.
pub fn row_name(r: &TaskRow) -> String {
    let mut name = format!(
        "{}, {}, 상태: {}",
        r.title,
        branch_line(r.branch.as_deref(), r.base.as_deref()),
        r.status
    );
    if r.open_comments > 0 {
        name.push_str(&format!(", {} {}개", s::COMMENTS, r.open_comments));
    }
    if r.gates > 0 {
        name.push_str(&format!(", {} {}개", s::GATES, r.gates));
    }
    name
}

/// The docked tiles (first [`MAX_TILES`]) and how many terminals did not fit.
pub fn tiles(terminals: &[String]) -> (Vec<String>, usize) {
    let shown: Vec<String> = terminals.iter().take(MAX_TILES).cloned().collect();
    let hidden = terminals.len() - shown.len();
    (shown, hidden)
}

/// Keyboard movement in the task list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Home,
    End,
}

/// A keyboard region (F6 / Shift+F6 cycle through them in this order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Region {
    Tasks,
    /// The Review (or the dock in the 패널 view).
    Canvas,
    Terminal(String),
}

/// The layout's own view state (never the tasks' or terminals' state).
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewModel {
    /// The task whose Review is the canvas.
    pub selected: Option<String>,
    /// A terminal shown over the whole canvas.
    pub expanded: Option<String>,
    /// The bottom terminals show their tiles (else only their header).
    pub strip_open: bool,
    /// The canvas shows the dock (other panels) instead of the Review.
    pub show_dock: bool,
}

impl Default for ReviewModel {
    fn default() -> Self {
        Self {
            selected: None,
            expanded: None,
            strip_open: true,
            show_dock: false,
        }
    }
}

impl ReviewModel {
    /// Keep the selection on an existing task: `prefer` (the task the user
    /// was in) when the selection is gone, else the first task.
    pub fn settle(&mut self, rows: &[TaskRow], prefer: Option<&str>) {
        if self
            .selected
            .as_deref()
            .is_none_or(|id| !rows.iter().any(|r| r.id == id))
        {
            self.selected = prefer
                .filter(|p| rows.iter().any(|r| r.id == *p))
                .map(str::to_owned)
                .or_else(|| rows.first().map(|r| r.id.clone()));
        }
        // An expanded terminal must still be one of the selected task's tiles.
        let tiles = self
            .selected
            .as_deref()
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .map(|r| tiles(&r.terminals).0)
            .unwrap_or_default();
        if self.expanded.as_ref().is_some_and(|e| !tiles.contains(e)) {
            self.expanded = None;
        }
    }

    /// Switch the canvas to task `id`. Returns whether the task changed.
    pub fn select(&mut self, id: &str) -> bool {
        self.show_dock = false;
        if self.selected.as_deref() == Some(id) {
            return false;
        }
        self.selected = Some(id.to_string());
        self.expanded = None;
        true
    }

    /// The task a key moves the selection to (`None`: nowhere to go).
    pub fn nav(&self, rows: &[TaskRow], key: Nav) -> Option<String> {
        let i = self
            .selected
            .as_deref()
            .and_then(|id| rows.iter().position(|r| r.id == id));
        let j = match (key, i) {
            (_, None) | (Nav::Home, _) => 0,
            (Nav::End, _) => rows.len().checked_sub(1)?,
            (Nav::Up, Some(i)) => i.checked_sub(1)?,
            (Nav::Down, Some(i)) => i + 1,
        };
        rows.get(j).map(|r| r.id.clone())
    }

    /// Ctrl+Shift+X / the tile's button: `id` over the whole canvas, or back to its tile.
    pub fn toggle_expand(&mut self, id: &str) {
        self.expanded = if self.expanded.as_deref() == Some(id) {
            None
        } else {
            Some(id.to_string())
        };
        if self.expanded.is_some() {
            self.strip_open = true;
            self.show_dock = false;
        }
    }

    /// The F6 stops, in order: tasks, canvas (unless a terminal covers it), each shown terminal.
    pub fn regions(&self, tiles: &[String]) -> Vec<Region> {
        let mut r = vec![Region::Tasks];
        match &self.expanded {
            Some(id) => r.push(Region::Terminal(id.clone())),
            None => {
                r.push(Region::Canvas);
                if self.strip_open && !self.show_dock {
                    r.extend(tiles.iter().cloned().map(Region::Terminal));
                }
            }
        }
        r
    }
}

/// The region after (or before) `current`; an unknown region starts at the tasks.
pub fn next_region(regions: &[Region], current: Option<&Region>, forward: bool) -> Region {
    let n = regions.len();
    match current.and_then(|c| regions.iter().position(|r| r == c)) {
        Some(i) if forward => regions[(i + 1) % n].clone(),
        Some(i) => regions[(i + n - 1) % n].clone(),
        None => Region::Tasks,
    }
}

// ───────────────────────────── module ─────────────────────────────

/// The layout module (registered in [`crate::layout_mode::LayoutRegistry::new`]).
pub struct ReviewLayout;

impl LayoutModule for ReviewLayout {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Review
    }
    fn render_body(
        &self,
        ws: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        if cx.try_global::<ReviewKeys>().is_none() {
            let c = Some(KEY_CONTEXT);
            cx.bind_keys([
                KeyBinding::new("f6", NextRegion, c),
                KeyBinding::new("shift-f6", PrevRegion, c),
                KeyBinding::new("ctrl-shift-x", ToggleExpand, c),
            ]);
            cx.set_global(ReviewKeys);
        }
        let view = match cx.try_global::<ViewGlobal>().and_then(|g| g.0.upgrade()) {
            Some(v) => v,
            None => {
                let weak = cx.entity().downgrade();
                let v = cx.new(|cx| ReviewView::new(weak, cx));
                // The app owns the view; the global only finds it again.
                view_slot(cx).replace(Some(v.clone()));
                cx.set_global(ViewGlobal(v.downgrade()));
                v
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
            .child(div().flex().flex_col().flex_1().min_h_0().child(view))
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

struct ReviewKeys;
impl Global for ReviewKeys {}
struct ViewGlobal(WeakEntity<ReviewView>);
impl Global for ViewGlobal {}
struct ViewSlot(Rc<RefCell<Option<Entity<ReviewView>>>>);
impl Global for ViewSlot {}

/// Strong owner of the view, per app (it lives as long as the app).
fn view_slot(cx: &mut App) -> Rc<RefCell<Option<Entity<ReviewView>>>> {
    if cx.try_global::<ViewSlot>().is_none() {
        cx.set_global(ViewSlot(Rc::default()));
    }
    cx.global::<ViewSlot>().0.clone()
}

#[cfg(test)]
/// The view of this app, if the layout was shown once (tests).
pub fn view(cx: &App) -> Option<Entity<ReviewView>> {
    cx.try_global::<ViewGlobal>().and_then(|g| g.0.upgrade())
}

// ───────────────────────────── view ─────────────────────────────

pub struct ReviewView {
    ws: WeakEntity<Workspace>,
    pub model: ReviewModel,
    /// Focus of the task list when it has no row.
    focus: FocusHandle,
    row_focus: HashMap<String, FocusHandle>,
    scroll: UniformListScrollHandle,
    /// Review panel for a task the dock has none for.
    own_review: Option<Entity<ReviewPanel>>,
    /// Tile panels for terminals the workbench has no panel for.
    own_panels: HashMap<String, Entity<TerminalPanel>>,
    /// Gates being resolved (a second press is ignored).
    busy: HashSet<String>,
    dock_signature: usize,
    dock_reviews: HashSet<EntityId>,
    seen_focus: Option<String>,
    /// The layout was just switched to (or the app started in it): place focus once.
    entered: bool,
    /// The settings name this layout (to see the switch *to* it).
    shown: bool,
    /// The canvas Review, whether its file list shows and whether its comments are a
    /// drawer, as last fitted to the width (ui-05).
    fitted: Option<(EntityId, bool, bool)>,
    _subs: Vec<Subscription>,
}

impl ReviewView {
    fn new(ws: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        // Leaving the layout drops what it owns (Review, tile attaches); the
        // dock and its panels were never touched.
        let subs = vec![
            cx.observe_global::<crate::theme::Settings>(|v: &mut Self, cx| {
                let here = cx.global::<crate::theme::Settings>().layout == LayoutMode::Review.id();
                if !here {
                    v.own_review = None;
                    v.own_panels.clear();
                    v.entered = false;
                } else if !v.shown {
                    v.entered = true;
                }
                v.shown = here;
            }),
        ];
        Self {
            ws,
            model: ReviewModel::default(),
            focus: cx.focus_handle().tab_stop(true),
            row_focus: HashMap::new(),
            scroll: UniformListScrollHandle::new(),
            own_review: None,
            fitted: None,
            own_panels: HashMap::new(),
            busy: HashSet::new(),
            dock_signature: usize::MAX,
            dock_reviews: HashSet::new(),
            seen_focus: None,
            entered: true,
            shown: true,
            _subs: subs,
        }
    }

    #[cfg(test)]
    pub fn own_review(&self) -> Option<Entity<ReviewPanel>> {
        self.own_review.clone()
    }

    #[cfg(test)]
    pub fn own_panel_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.own_panels.keys().cloned().collect();
        v.sort();
        v
    }

    pub fn row_focus(&mut self, id: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.row_focus
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    pub fn rows(&self, cx: &App) -> Vec<TaskRow> {
        let Some(ws) = self.ws.upgrade() else {
            return Vec::new();
        };
        let w = ws.read(cx);
        build_rows(&w.tasks, &w.terminals, &w.orch.pending())
    }

    /// The selected task's docked tiles.
    pub fn tiles(&self, cx: &App) -> Vec<String> {
        let rows = self.rows(cx);
        self.model
            .selected
            .as_deref()
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .map(|r| tiles(&r.terminals).0)
            .unwrap_or_default()
    }

    /// Panels other than terminals and reviews the dock holds (a rise shows the dock).
    fn dock_signature(w: &Workspace, cx: &App) -> usize {
        let live = |p: &Option<WeakEntity<crate::chrome::ChromePanel>>| {
            p.as_ref().is_some_and(|p| p.upgrade().is_some()) as usize
        };
        live(&w.settings_panel)
            + live(&w.inspector_panel)
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

    /// The Review of `task`: the workbench's own panel when the dock has one
    /// (nothing reloads), else a layout-owned one (side by side).
    fn review_for(
        &mut self,
        task: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ReviewPanel>> {
        let ws = self.ws.upgrade()?;
        let own = self.own_review.as_ref().map(|p| p.entity_id());
        let existing = ws
            .read(cx)
            .live_reviews()
            .into_iter()
            .find(|r| Some(r.entity_id()) != own && r.read(cx).task_id == task);
        if let Some(p) = existing {
            self.own_review = None;
            return Some(p);
        }
        if let Some(p) = self
            .own_review
            .as_ref()
            .filter(|p| p.read(cx).task_id == task)
        {
            return Some(p.clone());
        }
        let (k, source, caps) = {
            let w = ws.read(cx);
            (w.task(task).cloned(), w.source.clone(), w.caps.review)
        };
        let k = k.filter(|_| caps)?;
        let status = k
            .terminals
            .first()
            .and_then(|t| ws.read(cx).terminal(t))
            .and_then(|t| t.status.clone().map(|s| (t.id.clone(), s)));
        let weak = self.ws.clone();
        let panel = cx.new(|cx| ReviewPanel::new(source, weak, &k, window, cx));
        panel.update(cx, |r, cx| {
            // The canvas is wide: side by side, as the preset shows it.
            r.set_split(true, cx);
            if let Some((t, s)) = &status {
                r.agent_status(t, s, cx);
            }
        });
        // Registered like dock reviews: task, status and delivery events reach it.
        ws.update(cx, |w, _| w.reviews.push(panel.downgrade()));
        self.own_review = Some(panel.clone());
        Some(panel)
    }

    /// The panel of terminal `id`: the workbench's own when it has one (no
    /// new attach), else a layout-owned one.
    fn panel_for(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalPanel>> {
        let ws = self.ws.upgrade()?;
        let own: HashSet<EntityId> = self.own_panels.values().map(|p| p.entity_id()).collect();
        let existing = ws
            .read(cx)
            .live_panels()
            .into_iter()
            .find(|p| !own.contains(&p.entity_id()) && p.read(cx).terminal_id == id);
        if let Some(p) = existing {
            self.own_panels.remove(id);
            return Some(p);
        }
        if let Some(p) = self.own_panels.get(id) {
            return Some(p.clone());
        }
        ws.read(cx).terminal(id)?;
        let g = cx.global::<crate::workspace::AppGlobals>();
        let (source, panels) = (g.source.clone(), g.panels.clone());
        let label: SharedString = g
            .labels
            .borrow()
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
            .into();
        let weak = self.ws.clone();
        let panel =
            cx.new(|cx| TerminalPanel::new(source, weak, id.to_string(), label, window, cx));
        // Registered like dock panels: labels, theme changes and "terminal in view" reach it.
        panels.borrow_mut().push(panel.downgrade());
        self.own_panels.insert(id.to_string(), panel.clone());
        Some(panel)
    }

    fn notify_all(&self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |_, cx| cx.notify());
        }
    }

    fn terminal_focus(&self, id: &str, cx: &App) -> Option<FocusHandle> {
        let ws = self.ws.upgrade()?;
        let own = self.own_panels.get(id).cloned();
        let p = own.or_else(|| {
            ws.read(cx)
                .live_panels()
                .into_iter()
                .find(|p| p.read(cx).terminal_id == id)
        })?;
        Some(p.read(cx).view.read(cx).focus_handle(cx))
    }

    /// Select task `id` in the list (switching the canvas) and focus its row.
    pub fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.select(id);
        let rows = self.rows(cx);
        if let Some(i) = rows.iter().position(|r| r.id == id) {
            self.scroll.scroll_to_item(i, ScrollStrategy::Nearest);
        }
        let f = self.row_focus(id, cx);
        f.focus(window, cx);
        self.notify_all(cx);
    }

    /// Move the keyboard into `region` (creating the panel it needs).
    pub fn focus_region(&mut self, region: &Region, window: &mut Window, cx: &mut Context<Self>) {
        match region {
            Region::Tasks => match self.model.selected.clone() {
                Some(id) => self.select(&id, window, cx),
                None => self.focus.focus(window, cx),
            },
            Region::Canvas => {
                if self.model.show_dock {
                    if let Some(ws) = self.ws.upgrade() {
                        let dock = ws.read(cx).dock.clone();
                        dock.read(cx).focus_handle(cx).focus(window, cx);
                    }
                } else if let Some(r) = self
                    .model
                    .selected
                    .clone()
                    .and_then(|t| self.review_for(&t, window, cx))
                {
                    r.read(cx).focus_handle().focus(window, cx);
                }
            }
            Region::Terminal(id) => {
                if let Some(p) = self.panel_for(id, window, cx) {
                    p.read(cx).view.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
        }
        self.notify_all(cx);
    }

    /// The region holding the keyboard focus, if any.
    fn current_region(&self, window: &Window, cx: &App) -> Option<Region> {
        // `focus` is on the layout's root: only its own focus (no row yet) counts.
        if self.focus.is_focused(window)
            || self
                .row_focus
                .values()
                .any(|f| f.contains_focused(window, cx))
        {
            return Some(Region::Tasks);
        }
        for id in self
            .tiles(cx)
            .into_iter()
            .chain(self.model.expanded.clone())
        {
            if self
                .terminal_focus(&id, cx)
                .is_some_and(|f| f.contains_focused(window, cx))
            {
                return Some(Region::Terminal(id));
            }
        }
        let ws = self.ws.upgrade()?;
        let w = ws.read(cx);
        let in_review = w
            .live_reviews()
            .iter()
            .any(|r| r.read(cx).focus_handle().contains_focused(window, cx));
        let in_dock = self.model.show_dock
            && w.dock
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx);
        (in_review || in_dock).then_some(Region::Canvas)
    }

    /// F6 / Shift+F6.
    pub fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let regions = self.model.regions(&self.tiles(cx));
        let current = self.current_region(window, cx);
        let next = next_region(&regions, current.as_ref(), forward);
        self.focus_region(&next, window, cx);
    }

    /// Ctrl+Shift+X: the focused tile (else the expanded one, else the first) over the canvas, or back.
    pub fn toggle_expand(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focused = match self.current_region(window, cx) {
            Some(Region::Terminal(id)) => Some(id),
            _ => None,
        };
        let Some(id) = focused
            .or(self.model.expanded.clone())
            .or_else(|| self.tiles(cx).first().cloned())
        else {
            return;
        };
        self.model.toggle_expand(&id);
        self.focus_region(&Region::Terminal(id), window, cx);
    }

    /// Focus the terminal tile `id` (expanding it when it is not docked).
    pub fn answer(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.strip_open = true;
        self.model.show_dock = false;
        if !self.tiles(cx).iter().any(|t| t == id) {
            self.model.expanded = Some(id.to_string());
        }
        self.focus_region(&Region::Terminal(id.to_string()), window, cx);
    }

    /// Approve / reject through the gate path (`gate.resolve`), as the gates inbox does.
    pub fn resolve(
        &mut self,
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
        let rx = ws
            .read(cx)
            .source
            .call::<p::GateResolve>(&p::GateResolveParams {
                id: gate.clone(),
                decision,
                note: None,
                choice,
            });
        cx.spawn_in(window, async move |this, cx| {
            let r = rx.await;
            let _ = this.update_in(cx, |v, window, cx| {
                v.busy.remove(&gate);
                // The buttons go away with the gate: keep the keyboard in the layout.
                if v.current_region(window, cx).is_none() {
                    v.focus_region(&Region::Tasks, window, cx);
                }
                ws.update(cx, |w, cx| {
                    match r {
                        Ok(g) => {
                            w.orch.apply(g);
                        }
                        Err(e) => {
                            w.raise_toast(ToastKind::Error, s::COULD_NOT_RESOLVE, e.message, cx)
                        }
                    }
                    cx.notify();
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Entering the layout: a focused terminal or Review keeps focus (its task
    /// becomes the canvas), a focused settings/editor/… panel keeps focus in
    /// the 패널 view; otherwise the selected task's row takes focus.
    fn place_focus_on_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            return;
        }
        let Some(ws) = self.ws.upgrade() else { return };
        let (terminal_task, review_task, in_dock) = {
            let w = ws.read(cx);
            let term = w
                .live_panels()
                .into_iter()
                .find(|p| {
                    p.read(cx)
                        .view
                        .read(cx)
                        .focus_handle(cx)
                        .contains_focused(window, cx)
                })
                .and_then(|p| {
                    w.terminal(&p.read(cx).terminal_id)
                        .map(|t| (t.id.clone(), t.task_id.clone()))
                });
            let review = w
                .live_reviews()
                .into_iter()
                .find(|r| r.read(cx).focus_handle().contains_focused(window, cx))
                .map(|r| r.read(cx).task_id.clone());
            (
                term,
                review,
                w.dock
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx),
            )
        };
        if let Some((id, Some(task))) = terminal_task {
            self.model.select(&task);
            let rows = self.rows(cx);
            self.model.settle(&rows, None);
            if !self.tiles(cx).contains(&id) {
                self.model.expanded = Some(id);
            }
            self.model.strip_open = true;
            return;
        }
        if let Some(task) = review_task {
            self.model.select(&task);
            return;
        }
        if in_dock {
            self.model.show_dock = true;
            return;
        }
        let handle = match self.model.selected.clone() {
            Some(id) => self.row_focus(&id, cx),
            None => self.focus.clone(),
        };
        window.defer(cx, move |window, cx| handle.focus(window, cx));
    }

    fn on_row_key(
        &mut self,
        id: &str,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &ev.keystroke;
        if k.modifiers.modified() {
            return false;
        }
        let nav = match k.key.as_str() {
            "up" => Nav::Up,
            "down" => Nav::Down,
            "home" => Nav::Home,
            "end" => Nav::End,
            "enter" => {
                self.model.select(id);
                self.focus_region(&Region::Canvas, window, cx);
                return true;
            }
            _ => return false,
        };
        let rows = self.rows(cx);
        if let Some(next) = self.model.nav(&rows, nav) {
            self.select(&next, window, cx);
        }
        true
    }

    /// Row height of the task list (uniform list): grows with the UI font so
    /// Korean text is never clipped at any font size or scale.
    pub fn row_height(cx: &App) -> f32 {
        let line = (Self::ui_font(cx) * 1.5).ceil();
        let m = pod_ui::themes::metrics();
        line * 2. + 2. + 8. * 2. + m.border_width * 2. + 4.
    }

    fn ui_font(cx: &App) -> f32 {
        let user = cx
            .try_global::<crate::theme::Settings>()
            .map_or(13., |s| s.font_size);
        crate::theme::ui_font_size(user, pod_ui::themes::metrics().density.ui_font)
    }
}

impl Focusable for ReviewView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A small text button (Enter/Space, Korean name).
fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: Option<pod_ui::Icon>,
    primary: bool,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let on = Rc::new(on);
    let key = on.clone();
    let label: SharedString = label.into();
    let (fg, bg, border) = if primary {
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
        .aria_label(label.clone())
        .tab_index(0)
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
        .cursor_pointer()
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
}

/// 승인 — or, for a gate with options (v17), one `승인: <option>` per choice
/// (podd refuses an approval of such a gate without a choice).
fn approve_buttons(g: &GateInfo, view: WeakEntity<ReviewView>, gate: String) -> Vec<Stateful<Div>> {
    let choices: Vec<Option<String>> = if g.options.is_empty() {
        vec![None]
    } else {
        g.options.iter().cloned().map(Some).collect()
    };
    choices
        .into_iter()
        .enumerate()
        .map(|(i, choice)| {
            let (view, gate) = (view.clone(), gate.clone());
            let label = approve_label(choice.as_deref());
            text_button(
                SharedString::from(format!("review-approve-{}-{i}", g.id)),
                label,
                Some(pod_ui::Icon::Check),
                i == 0,
                move |window, cx| {
                    let _ = view.update(cx, |v, cx| {
                        v.resolve(
                            gate.clone(),
                            GateDecision::Approve,
                            choice.clone(),
                            window,
                            cx,
                        )
                    });
                },
            )
        })
        .collect()
}

/// `승인` or `승인: <option>`.
pub fn approve_label(choice: Option<&str>) -> String {
    match choice {
        Some(c) => format!("{}: {c}", s::APPROVE),
        None => s::APPROVE.to_string(),
    }
}

/// A UI callback shared by click and key handlers.
type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// What one task row needs besides the row.
struct RowCtx {
    view: WeakEntity<ReviewView>,
    selected: Option<String>,
    focus: HashMap<String, FocusHandle>,
    badges: HashMap<String, Option<pod_ui::StatusBadge>>,
    height: f32,
}

fn render_row(r: &TaskRow, ctx: &RowCtx) -> AnyElement {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let selected = ctx.selected.as_deref() == Some(r.id.as_str());
    let (glyph, glyph_fg) = match ctx.badges.get(&r.id).cloned().flatten() {
        Some(b) => b.glyph(mode),
        None => (pod_ui::Icon::Circle, t.text_muted),
    };
    let focus = ctx.focus.get(&r.id).cloned();
    let view = ctx.view.clone();
    let id = r.id.clone();
    let sub = branch_line(r.branch.as_deref(), r.base.as_deref());
    let row = div()
        .id(SharedString::from(format!("review-task-{id}")))
        .qa_id({
            let id = id.clone();
            move || format!("review-task-{id}")
        })
        .when_some(focus, |d, f| d.track_focus(&f.tab_stop(selected)))
        .role(Role::ListItem)
        .aria_label(row_name(r))
        .aria_selected(selected)
        .h(px(ctx.height - 4.))
        .w_full()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(2.))
        .px(px(10.))
        .rounded(px(m.radius.control))
        .border(px(m.border_width))
        .border_color(rgb(
            if selected && m.surface == pod_ui::themes::Surface::Mono {
                t.text_primary
            } else {
                t.bg_surface
            },
        ))
        .bg(rgb(if selected {
            t.bg_selected
        } else {
            t.bg_surface
        }))
        .when(!selected, |d| d.hover(move |s| s.bg(rgb(t.bg_hover))))
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(pod_ui::icon(glyph, glyph_fg))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .text_color(rgb(t.text_primary))
                        .child(r.title.clone()),
                )
                .when(r.gates > 0, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(rgb(t.warning_fg))
                            .child(pod_ui::icon_sized(
                                pod_ui::Icon::ShieldQuestion,
                                12.,
                                t.warning_fg,
                            ))
                            .child(r.gates.to_string()),
                    )
                })
                .when(r.open_comments > 0, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(rgb(t.text_secondary))
                            .child(pod_ui::icon_sized(
                                pod_ui::Icon::MessageSquare,
                                12.,
                                t.text_secondary,
                            ))
                            .child(r.open_comments.to_string()),
                    )
                }),
        )
        .child(
            div()
                .qa_id({
                    let id = id.clone();
                    move || format!("review-task-sub-{id}")
                })
                .pl(px(22.))
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(format!("{sub} · {}", r.status)),
        )
        .on_click({
            let (view, id) = (view.clone(), id.clone());
            move |_, window, cx| {
                let _ = view.update(cx, |v, cx| v.select(&id, window, cx));
            }
        })
        .on_key_down(move |e, window, cx| {
            if view
                .update(cx, |v, cx| v.on_row_key(&id, e, window, cx))
                .unwrap_or(false)
            {
                cx.stop_propagation();
            }
        });
    div()
        .h(px(ctx.height))
        .w_full()
        .px(px(6.))
        .pb(px(4.))
        .child(row)
        .into_any_element()
}

/// One docked (or expanded) terminal: a header with status and a size button, then the panel.
fn render_tile(
    view: WeakEntity<ReviewView>,
    t: &TerminalVm,
    who: &Who,
    panel: Option<Entity<TerminalPanel>>,
    expanded: bool,
    header_h: f32,
) -> Stateful<Div> {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let badge = crate::orch::terminal_badge(t);
    let (glyph, glyph_fg) = badge.glyph(mode);
    let status = badge.label();
    let title = who.title();
    let accent = match t.status.as_ref().map(|s| s.state) {
        Some(crate::vm::AgentState::Blocked) if t.is_running() => Some(tk.warning_fg),
        Some(crate::vm::AgentState::Error) if t.is_running() => Some(tk.danger_fg),
        _ if matches!(t.live, Liveness::Exited { code: Some(c), killed: false } if c != 0)
            || t.live == Liveness::Lost =>
        {
            Some(tk.danger_fg)
        }
        _ => None,
    };
    let id = t.id.clone();
    div()
        .id(SharedString::from(format!("review-tile-{id}")))
        .qa_id({
            let id = id.clone();
            move || format!("review-tile-{id}")
        })
        .role(Role::Group)
        .aria_label(format!("{title}, {status}"))
        .flex_1()
        .min_w(px(160.))
        .h_full()
        .flex()
        .flex_col()
        .min_h_0()
        .bg(rgb(tk.bg_surface))
        .border_r(px(m.border_width))
        .border_color(rgb(tk.border_subtle))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .min_h(px(header_h))
                .flex_shrink_0()
                .border_b(px(m.border_width))
                .border_color(rgb(tk.border_subtle))
                // Attention stripe (blocked: warning, failed: danger); never the only carrier: icon + status text too.
                .when_some(accent, |d, c| d.border_t(px(2.)).border_color(rgb(c)))
                .child(crate::agent_kind::mark_el(
                    &format!("review-tile-{id}"),
                    &who.kind,
                    pod_ui::tokens::ICON_SIZE,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(pod_ui::icon(glyph, glyph_fg))
                // The status is never the part cut off: the name shrinks first.
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(rgb(glyph_fg))
                        .child(status),
                )
                .child(
                    pod_ui::IconButton::new(
                        SharedString::from(format!("review-tile-size-{id}")),
                        if expanded {
                            pod_ui::Icon::Minimize2
                        } else {
                            pod_ui::Icon::Maximize2
                        },
                        if expanded { s::SHRINK } else { s::GROW },
                    )
                    .render(mode, move |window, cx| {
                        let _ = view.update(cx, |v, cx| {
                            v.model.toggle_expand(&id);
                            let id = id.clone();
                            v.focus_region(&Region::Terminal(id), window, cx);
                        });
                    }),
                ),
        )
        .child(div().flex_1().min_h_0().children(panel))
}

/// The Review panel's columns (review.rs): file list 250, diff min 320, comments 340.
const REVIEW_FILES_W: f32 = 250.;
const REVIEW_DIFF_MIN_W: f32 = 320.;
const REVIEW_COMMENTS_W: f32 = crate::review::COMMENTS_W;
/// Whether the file list fits beside the diff and the comment column in `canvas_w`.
pub fn review_files_fit(canvas_w: f32) -> bool {
    canvas_w >= REVIEW_FILES_W + REVIEW_DIFF_MIN_W + REVIEW_COMMENTS_W
}
/// D4.6: below 560 px of canvas the comment column becomes a drawer over the diff
/// (between 560 and 660 it narrows towards its 224 minimum beside the 320 diff).
pub fn review_comments_drawer(canvas_w: f32) -> bool {
    canvas_w < crate::review::COMMENTS_DRAWER_BELOW
}

/// Who a terminal is, by the same rule as every other layout (docs/10 §D5.11):
/// the agent kind from status/task/argv (`agent_kind::identify_in`), not the program
/// name (a Claude session in PowerShell is Claude), plus the terminal's label.
#[derive(Clone, Debug)]
pub struct Who {
    pub kind: crate::agent_kind::AgentKind,
    pub label: String,
}
impl Default for Who {
    fn default() -> Self {
        Self {
            kind: crate::agent_kind::AgentKind::Shell,
            label: String::new(),
        }
    }
}
impl Who {
    pub fn of(t: &TerminalVm, tasks: &[TaskVm], label: String) -> Self {
        Self {
            kind: crate::agent_kind::identify_in(t, tasks).kind,
            label,
        }
    }
    /// `Claude · Worker 0`.
    pub fn title(&self) -> String {
        format!("{} · {}", self.kind.name(), self.label)
    }
}

impl Render for ReviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else {
            return div().into_any_element();
        };
        let rows = self.rows(cx);
        let (focused, signature, reviews, dock) = {
            let w = ws.read(cx);
            let own = self.own_review.as_ref().map(|p| p.entity_id());
            let reviews: Vec<(EntityId, String)> = w
                .live_reviews()
                .into_iter()
                .filter(|r| Some(r.entity_id()) != own)
                .map(|r| (r.entity_id(), r.read(cx).task_id.clone()))
                .collect();
            (
                w.focused_terminal.clone(),
                Self::dock_signature(w, cx),
                reviews,
                w.dock.clone(),
            )
        };
        let task_of = |cx: &App, terminal: &str| {
            ws.read(cx)
                .terminal(terminal)
                .and_then(|t| t.task_id.clone())
        };
        // A terminal the workspace just focused (palette jump, 새 터미널) whose panel
        // this layout does not show yet: bring its task in and focus it there.
        let hidden = {
            let w = ws.read(cx);
            w.live_panels()
                .into_iter()
                .find(|p| p.read(cx).view.read(cx).focus_handle(cx).is_focused(window))
                .map(|p| p.read(cx).terminal_id.clone())
        };
        if let Some(id) = hidden
            && let Some(task) = task_of(cx, &id)
            && (self.model.selected.as_deref() != Some(task.as_str())
                || self.model.show_dock
                || !self.tiles(cx).contains(&id)
                    && self.model.expanded.as_deref() != Some(id.as_str()))
        {
            self.model.select(&task);
            self.model.strip_open = true;
            let shown = rows
                .iter()
                .find(|r| r.id == task)
                .is_some_and(|r| tiles(&r.terminals).0.contains(&id));
            self.model.expanded = (!shown).then(|| id.clone());
            let target = Region::Terminal(id);
            cx.defer_in(window, move |v, window, cx| {
                v.focus_region(&target, window, cx)
            });
        }
        // A terminal focused elsewhere (palette jump, new terminal) brings its task in.
        if focused != self.seen_focus {
            self.seen_focus = focused.clone();
            if let Some(id) = focused
                && let Some(task) = task_of(cx, &id)
            {
                self.model.select(&task);
                let row = rows.iter().find(|r| r.id == task);
                if row.is_some_and(|r| !tiles(&r.terminals).0.contains(&id)) {
                    self.model.expanded = Some(id);
                }
            }
        }
        // A Review opened while the layout shows (Ctrl+Shift+R, PR button) becomes the canvas.
        let ids: HashSet<EntityId> = reviews.iter().map(|(e, _)| *e).collect();
        if self.dock_signature != usize::MAX
            && let Some((_, task)) = reviews.iter().find(|(e, _)| !self.dock_reviews.contains(e))
        {
            self.model.select(task);
        }
        self.dock_reviews = ids;
        // Another panel opened while the layout shows (settings, editor …) appears in the canvas.
        if signature > self.dock_signature && self.dock_signature != usize::MAX {
            self.model.show_dock = true;
            self.model.expanded = None;
        }
        self.dock_signature = signature;
        let prefer = self.seen_focus.as_deref().and_then(|id| task_of(cx, id));
        self.model.settle(&rows, prefer.as_deref());
        if std::mem::take(&mut self.entered) {
            self.place_focus_on_enter(window, cx);
        }
        for r in &rows {
            self.row_focus(&r.id, cx);
        }

        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        use pod_ui::themes::Surface;
        let rounded = matches!(m.surface, Surface::Layered | Surface::Islands);
        let gap = if rounded { m.gap.max(6.) } else { 0. };
        let ui = Self::ui_font(cx);
        let header_h = m.density.panel_header.max((ui * 1.5).ceil() + 10.);
        let me = cx.entity().downgrade();
        let island = move |d: Stateful<Div>| -> Stateful<Div> {
            if !rounded {
                return d;
            }
            let d = d.rounded(px(m.radius.card)).overflow_hidden();
            match m.surface {
                Surface::Islands => d
                    .shadow(pod_ui::tokens::elevation(1, mode))
                    .border(px(m.border_width))
                    .border_color(rgb(t.border_subtle)),
                _ if m.shadow != pod_ui::themes::Shadow::None => {
                    d.shadow(pod_ui::tokens::elevation(1, mode))
                }
                _ => d,
            }
        };

        // ── left: task list ──
        let height = Self::row_height(cx);
        let n = rows.len();
        let list_rows = Rc::new(rows.clone());
        let ctx = Rc::new(RowCtx {
            view: me.clone(),
            selected: self.model.selected.clone(),
            focus: self.row_focus.clone(),
            badges: {
                let w = ws.read(cx);
                rows.iter()
                    .map(|r| {
                        (
                            r.id.clone(),
                            crate::orch::summary_badge(&crate::orch::summary_of(
                                &r.terminals,
                                &w.terminals,
                            )),
                        )
                    })
                    .collect()
            },
            height,
        });
        let list = if rows.is_empty() {
            div()
                .flex_1()
                .p_3()
                .text_color(rgb(t.text_muted))
                .child(s::NO_TASKS)
                .into_any_element()
        } else {
            uniform_list("review-task-list", n, move |range, _, _| {
                list_rows[range]
                    .iter()
                    .map(|r| render_row(r, &ctx))
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .pt(px(4.))
            .into_any_element()
        };
        let left = island(
            div()
                .id("review-tasks")
                .qa_id(|| "review-tasks".into())
                .role(Role::List)
                .aria_label(format!("{}, {n}개", s::TASKS))
                .w(px(m.density.sidebar))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .min_h_0()
                .bg(rgb(t.bg_surface))
                .when(!rounded && m.surface != Surface::Mono, |d| {
                    d.border_r(px(m.border_width))
                        .border_color(rgb(t.border_subtle))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .min_h(px(header_h))
                        .flex_shrink_0()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(t.text_secondary))
                                .child(s::TASKS),
                        )
                        .child(div().text_color(rgb(t.text_muted)).child(n.to_string())),
                )
                .child(list),
        );

        // ── header of the canvas: branch → base, title, agent statuses, gates ──
        let selected = self.model.selected.clone();
        let (task, terms, gates): (Option<TaskVm>, Vec<TerminalVm>, Vec<GateInfo>) = {
            let w = ws.read(cx);
            let task = selected.as_deref().and_then(|id| w.task(id)).cloned();
            let ids = task
                .as_ref()
                .map(|k| task_terminals(k, &w.terminals))
                .unwrap_or_default();
            let terms: Vec<TerminalVm> = ids
                .iter()
                .filter_map(|id| w.terminal(id).cloned())
                .collect();
            let gates = task
                .as_ref()
                .map(|k| task_gates(&k.id, &ids, &w.orch.pending()))
                .unwrap_or_default();
            (task, terms, gates)
        };
        let who: HashMap<String, Who> = {
            let w = ws.read(cx);
            terms
                .iter()
                .map(|x| (x.id.clone(), Who::of(x, &w.tasks, w.label_of(x))))
                .collect()
        };
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
        let (v1, v2) = (me.clone(), me.clone());
        let tabs = div()
            .id("review-canvas-tabs")
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .role(Role::TabList)
            .child(seg(
                "review-canvas-review",
                s::REVIEW_TAB,
                !self.model.show_dock,
                Rc::new(move |window, cx| {
                    let _ = v1.update(cx, |v, cx| {
                        v.model.show_dock = false;
                        v.focus_region(&Region::Canvas, window, cx);
                    });
                }),
            ))
            .child(seg(
                "review-canvas-panels",
                s::PANELS_TAB,
                self.model.show_dock,
                Rc::new(move |window, cx| {
                    let _ = v2.update(cx, |v, cx| {
                        v.model.show_dock = true;
                        v.model.expanded = None;
                        v.focus_region(&Region::Canvas, window, cx);
                    });
                }),
            ));
        let mut agents = div()
            .id("review-agents")
            .qa_id(|| "review-agents".into())
            .role(Role::Group)
            .aria_label(s::AGENTS)
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_3()
            .gap_y_1()
            .min_w_0();
        for x in &terms {
            let badge = crate::orch::terminal_badge(x);
            let (glyph, fg) = badge.glyph(mode);
            let w = who.get(&x.id).cloned().unwrap_or_default();
            agents = agents.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(crate::agent_kind::mark_el(
                        &format!("review-agent-{}", x.id),
                        &w.kind,
                        14.,
                    ))
                    .child(div().text_color(rgb(t.text_primary)).child(w.title()))
                    .child(pod_ui::icon_sized(glyph, 14., fg))
                    .child(div().text_color(rgb(fg)).child(badge.label())),
            );
        }
        let mut gate_rows = div().flex().flex_col().gap_1();
        for g in &gates {
            let (ga, gr) = (g.id.clone(), g.id.clone());
            let (va, vr) = (me.clone(), me.clone());
            gate_rows = gate_rows.child(
                div()
                    .id(SharedString::from(format!("review-gate-{}", g.id)))
                    .qa_id({
                        let id = g.id.clone();
                        move || format!("review-gate-{id}")
                    })
                    .role(Role::Group)
                    .aria_label(format!("{}: {}, {}", s::GATES, g.title, g.message))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(m.radius.control))
                    .bg(rgb(t.warning_bg))
                    .child(pod_ui::icon(pod_ui::Icon::ShieldQuestion, t.warning_fg))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(rgb(t.text_primary))
                            .child(g.title.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .text_color(rgb(t.text_secondary))
                            .child(g.message.clone()),
                    )
                    .children(approve_buttons(g, va, ga))
                    .child(text_button(
                        SharedString::from(format!("review-reject-{}", g.id)),
                        s::REJECT,
                        Some(pod_ui::Icon::X),
                        false,
                        move |window, cx| {
                            let _ = vr.update(cx, |v, cx| {
                                v.resolve(gr.clone(), GateDecision::Reject, None, window, cx)
                            });
                        },
                    )),
            );
        }
        // A blocked agent without a gate: answer it in its terminal.
        for x in terms.iter().filter(|x| {
            x.is_running()
                && x.status
                    .as_ref()
                    .is_some_and(|s| s.state == crate::vm::AgentState::Blocked)
        }) {
            if gates
                .iter()
                .any(|g| g.terminal.as_deref() == Some(x.id.as_str()))
            {
                continue;
            }
            let (va, id) = (me.clone(), x.id.clone());
            let w = who.get(&x.id).cloned().unwrap_or_default();
            gate_rows = gate_rows.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(m.radius.control))
                    .bg(rgb(t.warning_bg))
                    .child(pod_ui::icon(
                        pod_ui::Icon::MessageCircleQuestion,
                        t.warning_fg,
                    ))
                    .child(crate::agent_kind::mark_el(
                        &format!("review-ask-{}", x.id),
                        &w.kind,
                        14.,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .text_color(rgb(t.text_primary))
                            .child(format!(
                                "{} — {}",
                                w.title(),
                                crate::orch::terminal_badge(x).label()
                            )),
                    )
                    .child(text_button(
                        SharedString::from(format!("review-answer-{id}")),
                        s::ANSWER,
                        Some(pod_ui::Icon::SquareTerminal),
                        false,
                        move |window, cx| {
                            let _ = va.update(cx, |v, cx| v.answer(&id, window, cx));
                        },
                    )),
            );
        }
        let header = div()
            .id("review-header")
            .qa_id(|| "review-header".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .px_3()
            .py_1()
            .border_b(px(m.border_width))
            .border_color(rgb(if m.surface == Surface::Mono {
                t.bg_surface
            } else {
                t.border_subtle
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h(px(header_h))
                    .min_w_0()
                    .child(pod_ui::icon(
                        pod_ui::Icon::GitCompareArrows,
                        t.text_secondary,
                    ))
                    .when_some(task.as_ref(), |d, k| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(branch_line(k.branch.as_deref(), k.base.as_deref())),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(rgb(t.text_secondary))
                                .child(k.title.clone()),
                        )
                        .when_some(k.git_summary(), |d, g| {
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(rgb(t.text_muted))
                                    .child(g),
                            )
                        })
                    })
                    .child(div().flex_1())
                    .child(tabs),
            )
            .when(!terms.is_empty(), |d| d.child(agents))
            .when(
                !gates.is_empty()
                    || terms.iter().any(|x| {
                        x.status
                            .as_ref()
                            .is_some_and(|s| s.state == crate::vm::AgentState::Blocked)
                    }),
                |d| d.child(gate_rows.pb_1()),
            );

        // ── canvas: Review / dock / an expanded terminal ──
        let tile_ids = selected
            .as_deref()
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .map(|r| tiles(&r.terminals))
            .unwrap_or_default();
        let (tile_ids, hidden) = tile_ids;
        let expanded = self.model.expanded.clone();
        let canvas: AnyElement = if self.model.show_dock {
            div().flex_1().min_h_0().child(dock).into_any_element()
        } else if let Some(id) = expanded.as_ref() {
            let panel = self.panel_for(id, window, cx);
            match terms.iter().find(|x| &x.id == id) {
                Some(x) => render_tile(
                    me.clone(),
                    x,
                    &who.get(id).cloned().unwrap_or_default(),
                    panel,
                    true,
                    header_h,
                )
                .into_any_element(),
                None => div().into_any_element(),
            }
        } else {
            match selected.clone() {
                Some(task) => match self.review_for(&task, window, cx) {
                    Some(r) => {
                        // The canvas is the window minus this layout's task list: below the
                        // Review's own minimum (files + diff + comments) the file list goes
                        // first, then (< 560) the comment column becomes a drawer, so the
                        // column and 보내기 are never cut off (D4.6).
                        let canvas_w =
                            f32::from(window.viewport_size().width) - m.density.sidebar - gap * 3.;
                        let (files, drawer) =
                            (review_files_fit(canvas_w), review_comments_drawer(canvas_w));
                        if self.fitted != Some((r.entity_id(), files, drawer)) {
                            self.fitted = Some((r.entity_id(), files, drawer));
                            r.update(cx, |p, cx| {
                                p.set_file_list(files, cx);
                                p.set_comments_drawer(drawer, cx);
                            });
                        }
                        div()
                            .flex_1()
                            .min_h_0()
                            .size_full()
                            .child(r)
                            .into_any_element()
                    }
                    None => div()
                        .flex_1()
                        .p_4()
                        .text_color(rgb(t.text_muted))
                        .child(s::NO_REVIEW)
                        .into_any_element(),
                },
                None => div()
                    .flex_1()
                    .p_4()
                    .text_color(rgb(t.text_muted))
                    .child(s::NO_TASKS)
                    .into_any_element(),
            }
        };
        // Drop tile panels the canvas no longer shows (their attach ends).
        let keep: HashSet<String> = tile_ids.iter().cloned().chain(expanded.clone()).collect();
        self.own_panels.retain(|id, _| keep.contains(id));

        // ── bottom: the task's terminals, docked small ──
        let strip_tiles = self.model.strip_open && !self.model.show_dock && expanded.is_none();
        let strip = (!terms.is_empty() || task.is_some()).then(|| {
            let vh = f32::from(window.viewport_size().height);
            let h = if strip_tiles && !tile_ids.is_empty() {
                (vh * 0.3).max(header_h * 2. + 120.)
            } else {
                header_h
            };
            let open = self.model.strip_open;
            let toggle = me.clone();
            let mut row = div().flex().flex_1().min_h_0();
            if strip_tiles {
                for id in &tile_ids {
                    let panel = self.panel_for(id, window, cx);
                    if let Some(x) = terms.iter().find(|x| &x.id == id) {
                        row = row.child(render_tile(
                            me.clone(),
                            x,
                            &who.get(id).cloned().unwrap_or_default(),
                            panel,
                            false,
                            header_h,
                        ));
                    }
                }
            }
            div()
                .id("review-terminals")
                .qa_id(|| "review-terminals".into())
                .role(Role::Region)
                .aria_label(format!("{}, {}개", s::TERMINALS, terms.len()))
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
                        .min_h(px(header_h))
                        .flex_shrink_0()
                        .when(strip_tiles && !tile_ids.is_empty(), |d| {
                            d.border_b(px(m.border_width))
                                .border_color(rgb(t.border_subtle))
                        })
                        .child(pod_ui::icon(pod_ui::Icon::SquareTerminal, t.text_secondary))
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("{} {}", s::TERMINALS, terms.len())),
                        )
                        .when(hidden > 0, |d| {
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(rgb(t.text_muted))
                                    .child(format!("(+{hidden})")),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(t.text_muted))
                                .child(if terms.is_empty() {
                                    s::NO_TERMINALS
                                } else {
                                    s::TERMINALS_HINT
                                }),
                        )
                        .child(
                            pod_ui::IconButton::new(
                                "review-strip-toggle",
                                if open {
                                    pod_ui::Icon::ChevronDown
                                } else {
                                    pod_ui::Icon::ChevronUp
                                },
                                if open { s::STRIP_CLOSE } else { s::STRIP_OPEN },
                            )
                            .render(mode, move |_, cx| {
                                let _ = toggle.update(cx, |v, cx| {
                                    v.model.strip_open = !v.model.strip_open;
                                    v.notify_all(cx);
                                });
                            }),
                        ),
                )
                .when(strip_tiles, |d| d.child(row))
        });

        let main = island(
            div()
                .id("review-main")
                .qa_id(|| "review-main".into())
                .role(Role::Region)
                .aria_label(s::CANVAS)
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .min_h_0()
                .bg(rgb(t.bg_canvas))
                .child(header)
                .child(div().flex().flex_col().flex_1().min_h_0().child(canvas))
                .when_some(strip, |d, s| d.child(s)),
        );

        div()
            .id("review-layout")
            .qa_id(|| "review-layout".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|v, _: &NextRegion, window, cx| v.cycle(true, window, cx)))
            .on_action(cx.listener(|v, _: &PrevRegion, window, cx| v.cycle(false, window, cx)))
            .on_action(cx.listener(|v, _: &ToggleExpand, window, cx| v.toggle_expand(window, cx)))
            .on_key_down(cx.listener(|v, e: &KeyDownEvent, window, cx| {
                // The list itself (no row focused yet): arrows pick the first task.
                if v.focus.is_focused(window)
                    && matches!(e.keystroke.key.as_str(), "up" | "down" | "home" | "end")
                {
                    let rows = v.rows(cx);
                    if let Some(id) = rows.first().map(|r| r.id.clone()) {
                        v.select(&id, window, cx);
                    }
                }
            }))
            .size_full()
            .flex()
            .min_h_0()
            .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
            .text_color(rgb(t.text_primary))
            .child(left)
            .child(main)
            .into_any_element()
    }
}

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    // Not `super::*` at the top: gpui's glob exports its own `test` attribute.
    use super::{
        MAX_TILES, Nav, Region, ReviewModel, TaskRow, Who, approve_label, branch_line, build_rows,
        next_region, review_comments_drawer, review_files_fit, row_name, task_gates,
        task_terminals, tiles,
    };
    use crate::source::TestBackend;
    use core::prelude::v1::test;

    fn gate(id: &str, terminal: Option<&str>, task: Option<&str>) -> podd::protocol::GateInfo {
        podd::protocol::GateInfo {
            id: id.into(),
            title: "clippy 실행".into(),
            message: "명령 실행 승인이 필요합니다".into(),
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

    /// ui-05: at 960 logical (150% monitor) the canvas is 960 − 240; the file list
    /// gives way so diff (320) + comments (340) fit.
    #[test]
    fn file_list_gives_way_before_the_comment_column() {
        assert!(!review_files_fit(960. - 240.));
        assert!(review_files_fit(1440. - 240.));
        assert!(review_files_fit(910.) && !review_files_fit(909.));
        assert!(!review_comments_drawer(560.) && review_comments_drawer(559.));
    }

    /// ui-04: a Claude session started from PowerShell is "Claude", as in kanban/dense.
    #[test]
    fn tiles_name_the_agent_kind_not_the_program() {
        let task = TestBackend::task("pod/a", "작업", "pod", &["w0", "s0"]);
        let mut claude = TestBackend::terminal("w0", "Worker 0", Some("pod/a"), "working");
        claude.program = "powershell".into();
        claude.status.as_mut().unwrap().agent = Some("claude".into());
        let mut shell = TestBackend::terminal("s0", "셸", Some("pod/a"), "idle");
        shell.program = "powershell".into();
        let tasks = [task];
        assert_eq!(
            Who::of(&claude, &tasks, "Worker 0".into()).title(),
            "Claude · Worker 0"
        );
        assert_eq!(
            Who::of(&claude, &tasks, "Worker 0".into()).kind,
            crate::agent_kind::identify_in(&claude, &tasks).kind,
            "same rule as other layouts"
        );
        assert!(
            !Who::of(&shell, &tasks, "셸".into())
                .title()
                .contains("Powershell")
        );
    }

    fn sample() -> Vec<TaskRow> {
        let mut a = TestBackend::task("pod/a", "넓은 글자 정렬", "pod", &["a1", "gone", "a2"]);
        a.base = Some("main".into());
        a.review_open = 1;
        let b = TestBackend::task("pod/b", "설정 화면", "pod", &["b1"]);
        let c = TestBackend::task("pod/c", "빈 작업", "pod", &[]);
        let terms = vec![
            TestBackend::terminal("a1", "구현", Some("pod/a"), "working"),
            TestBackend::terminal("a2", "리뷰", Some("pod/a"), "blocked"),
            TestBackend::terminal("a3", "셸", Some("pod/a"), "idle"),
            TestBackend::terminal("b1", "문서", Some("pod/b"), "done"),
        ];
        build_rows(
            &[a, b, c],
            &terms,
            &[
                gate("g1", Some("a2"), None),
                gate("g2", None, Some("pod/b")),
                gate("g3", Some("zz"), None),
            ],
        )
    }

    #[test]
    fn rows_reuse_workspace_tasks_terminals_and_gates() {
        let rows = sample();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].terminals,
            ["a1", "a2", "a3"],
            "task order first, unknown ids skipped, task_id members appended"
        );
        assert_eq!(
            (rows[0].gates, rows[1].gates, rows[2].gates),
            (1, 1, 0),
            "terminal and task gates; others ignored"
        );
        assert_eq!(rows[0].branch.as_deref(), Some("pod/넓은 글자 정렬"));
        let name = row_name(&rows[0]);
        assert!(
            name.starts_with("넓은 글자 정렬, pod/넓은 글자 정렬 → main, 상태: ")
                && name.contains("의견 1개")
                && name.ends_with("승인 대기 1개"),
            "{name}"
        );
        assert_eq!(branch_line(None, Some("main")), "브랜치 없음");
        assert_eq!(
            (approve_label(None), approve_label(Some("빠르게"))),
            ("승인".to_string(), "승인: 빠르게".to_string()),
            "a gate with options is approved with a choice"
        );
        let ids = task_terminals(&TestBackend::task("x", "x", "r", &["q"]), &[]);
        assert!(ids.is_empty());
        assert!(task_gates("pod/c", &[], &[gate("g", None, Some("pod/a"))]).is_empty());
    }

    #[test]
    fn at_most_four_terminals_are_docked() {
        let ids: Vec<String> = (0..6).map(|i| format!("t{i}")).collect();
        let (shown, hidden) = tiles(&ids);
        assert_eq!((shown.len(), hidden), (MAX_TILES, 2));
        assert_eq!(tiles(&ids[..2]), (ids[..2].to_vec(), 0));
    }

    #[test]
    fn switching_tasks_switches_the_canvas_and_resets_the_expanded_terminal() {
        let rows = sample();
        let mut m = ReviewModel::default();
        m.settle(&rows, Some("pod/b"));
        assert_eq!(
            m.selected.as_deref(),
            Some("pod/b"),
            "the task the user was in"
        );
        m.selected = Some("gone".into());
        m.settle(&rows, None);
        assert_eq!(
            m.selected.as_deref(),
            Some("pod/a"),
            "a removed task falls back to the first"
        );
        m.toggle_expand("a2");
        assert_eq!(m.expanded.as_deref(), Some("a2"));
        m.show_dock = true;
        assert!(!m.select("pod/a"), "same task: no switch");
        assert!(!m.show_dock, "selecting a task shows its Review");
        assert_eq!(
            m.expanded.as_deref(),
            Some("a2"),
            "same task keeps the expanded terminal"
        );
        assert!(m.select("pod/b"));
        assert_eq!(m.expanded, None, "another task: its own tiles");
        m.toggle_expand("b1");
        m.toggle_expand("b1");
        assert_eq!(m.expanded, None, "toggle back");
        m.expanded = Some("a1".into());
        m.settle(&rows, None);
        assert_eq!(
            m.expanded, None,
            "an expanded terminal of another task is dropped"
        );
    }

    #[test]
    fn list_keys() {
        let rows = sample();
        let mut m = ReviewModel::default();
        assert_eq!(
            m.nav(&rows, Nav::Down).as_deref(),
            Some("pod/a"),
            "no selection: first"
        );
        m.selected = Some("pod/a".into());
        assert_eq!(m.nav(&rows, Nav::Down).as_deref(), Some("pod/b"));
        assert_eq!(m.nav(&rows, Nav::Up), None);
        assert_eq!(m.nav(&rows, Nav::End).as_deref(), Some("pod/c"));
        m.selected = Some("pod/c".into());
        assert_eq!(m.nav(&rows, Nav::Down), None);
        assert_eq!(m.nav(&rows, Nav::Home).as_deref(), Some("pod/a"));
        assert_eq!(m.nav(&[], Nav::End), None);
    }

    #[test]
    fn f6_cycles_tasks_canvas_and_each_docked_terminal() {
        let mut m = ReviewModel::default();
        let tiles = vec!["a1".to_string(), "a2".to_string()];
        let r = m.regions(&tiles);
        assert_eq!(
            r,
            [
                Region::Tasks,
                Region::Canvas,
                Region::Terminal("a1".into()),
                Region::Terminal("a2".into())
            ]
        );
        assert_eq!(next_region(&r, Some(&Region::Tasks), true), Region::Canvas);
        assert_eq!(
            next_region(&r, Some(&Region::Terminal("a2".into())), true),
            Region::Tasks,
            "wraps"
        );
        assert_eq!(
            next_region(&r, Some(&Region::Tasks), false),
            Region::Terminal("a2".into()),
            "Shift+F6 wraps back"
        );
        assert_eq!(
            next_region(&r, None, true),
            Region::Tasks,
            "focus outside: start at the tasks"
        );
        m.strip_open = false;
        assert_eq!(
            m.regions(&tiles),
            [Region::Tasks, Region::Canvas],
            "collapsed strip: no terminal stops"
        );
        m.strip_open = true;
        m.show_dock = true;
        assert_eq!(
            m.regions(&tiles),
            [Region::Tasks, Region::Canvas],
            "panel view hides the tiles"
        );
        m.toggle_expand("a2");
        assert!(!m.show_dock && m.strip_open);
        assert_eq!(
            m.regions(&tiles),
            [Region::Tasks, Region::Terminal("a2".into())],
            "an expanded terminal covers the canvas"
        );
    }
}
