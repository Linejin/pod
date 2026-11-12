//! 분할 리뷰 중심 (preset 13, docs/10 §D5.9): the selected task's Review
//! (side-by-side diff, comments, delivery, PR tab) is the main canvas, the
//! task list sits on the left and the task's terminals are docked small at
//! the bottom (up to [`model::MAX_TILES`], each expandable to the whole canvas).
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
//!
//! Files (docs/20-refactor/app.md A-P14): `model` holds the task rows, the
//! layout's view state (selection, expanded terminal, strip, 패널 view), list
//! keys and F6 regions, the frame's bookkeeping rules and the size formulas,
//! `view` the render, `copy` the strings. This file keeps the layout module,
//! its kit renderer ([`LayoutModule::renderer`], A-T2d) and the
//! [`ReviewView`] with its controller methods, kept in the kit's per-window
//! state slot. This is the layout named "review"; the Review *panel* it shows
//! is `crate::review` and stays that module's. The view reaches the host only
//! through [`LayoutActions`]: frame reads (tasks, terminals, gates, the
//! focused terminal, labels, the dock signature, the connection warning, font
//! metrics), the dock's focus slot and entity, the host's live terminal and
//! Review panels and new layout-owned ones, the podd source for its own
//! `gate.resolve`, and the gate answer, failure toast and host notify as
//! window-less [`HostUpdate`]s. The gate keeps its own path (fixed toast
//! title, notify only), not the kit's `resolve_gate`.
pub mod copy;
mod model;
#[cfg(test)]
mod tests;
mod view;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::dock::DockArea;
use gpui_kit::*;
use podd::protocol::{self as p, GateDecision};

pub use self::copy as s;
pub use self::model::{
    Region, ReviewModel, TaskRow, Who, approve_label, branch_line, build_rows, next_region,
    review_comments_drawer, review_files_fit, row_name, tiles,
};
// The model's other items at their old path, for the tests.
#[cfg(test)]
pub use self::model::{MAX_TILES, Nav, task_gates, task_terminals};

use self::model::{
    RowKey, Selected, dock_rose, follow_focus, follow_hidden, list_key, review_opened, row_badges,
    row_key,
};
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{
    HostUpdate, LayoutActions, LayoutFrame, LayoutHost, LayoutMetrics, LayoutRenderer, PanelWatch,
    Slot,
};
use crate::panel::TerminalPanel;
use crate::review::ReviewPanel;
use crate::workspace::ToastKind;

actions!(pod_review_layout, [NextRegion, PrevRegion, ToggleExpand]);

/// Key context of the layout (its keys are bound only inside it).
const KEY_CONTEXT: &str = "PodReviewLayout";

// ───────────────────────────── module ─────────────────────────────

/// The layout module (registered in [`crate::layout_mode::LayoutRegistry::new`]).
pub struct ReviewLayout;

impl LayoutModule for ReviewLayout {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Review
    }
    fn renderer(&self) -> Rc<dyn LayoutRenderer> {
        Rc::new(ReviewRenderer)
    }
}

struct ReviewKeys;
impl Global for ReviewKeys {}

#[cfg(test)]
/// The view of the window that showed the layout (tests run one window).
pub(super) fn view(cx: &App) -> Option<Entity<ReviewView>> {
    crate::layouts::kit::states::<ReviewView>(cx)
        .into_iter()
        .next()
}

// ───────────────────────────── view ─────────────────────────────

pub struct ReviewView {
    /// The host: frame reads, focus slots, dock, panels, source and updates.
    actions: LayoutActions,
    pub(super) model: ReviewModel,
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
    /// A dock panel asked for by focus (an orchestration page next to
    /// another, a panel brought back): the canvas shows 패널.
    panel_watch: PanelWatch,
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

/// What the render needs after the frame's bookkeeping ([`ReviewView::sync`]).
struct Synced {
    rows: Vec<TaskRow>,
    /// The status glyph of each row.
    badges: HashMap<String, Option<pod_ui::StatusBadge>>,
    /// The selected task, its terminals and gates.
    parts: Selected,
    /// Who each of the selected task's terminals is.
    who: HashMap<String, Who>,
    /// The host's dock, shown by the canvas's 패널 view.
    dock: Entity<DockArea>,
}

impl ReviewView {
    fn new(actions: LayoutActions, cx: &mut Context<Self>) -> Self {
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
            actions,
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
            panel_watch: PanelWatch::default(),
            seen_focus: None,
            entered: true,
            shown: true,
            _subs: subs,
        }
    }

    #[cfg(test)]
    pub(super) fn own_review(&self) -> Option<Entity<ReviewPanel>> {
        self.own_review.clone()
    }

    #[cfg(test)]
    pub(super) fn own_panel_ids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.own_panels.keys().cloned().collect();
        v.sort();
        v
    }

    pub(super) fn row_focus(&mut self, id: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.row_focus
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// Rows of the frame's tasks, terminals and pending gates.
    fn rows(&self, cx: &App) -> Vec<TaskRow> {
        self.actions
            .read(cx, |f| build_rows(f.tasks, f.terminals, f.gates))
            .unwrap_or_default()
    }

    /// The selected task's docked tiles.
    fn tiles(&self, cx: &App) -> Vec<String> {
        self.model.tiles_in(&self.rows(cx))
    }

    /// The task terminal `id` belongs to, if it is known and has one.
    fn task_of(&self, id: &str, cx: &App) -> Option<String> {
        self.actions
            .read(cx, |f| f.terminal(id).and_then(|t| t.task_id.clone()))
            .flatten()
    }

    /// The frame's bookkeeping before the layout draws: follow a terminal
    /// focused elsewhere, show Reviews and panels opened meanwhile, keep the
    /// selection on a task, place focus once after entering, and make the
    /// rows' focus handles. `None` once the host is gone.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Synced> {
        let dock = self.actions.dock(cx)?;
        let rows = self.rows(cx);
        let focused = self
            .actions
            .read(cx, |f| f.focused_terminal.map(str::to_owned))
            .flatten();
        let (signature, reviews, hidden) = {
            let own = self.own_review.as_ref().map(|p| p.entity_id());
            let reviews: Vec<(EntityId, String)> = self
                .actions
                .review_panels(cx)
                .into_iter()
                .filter(|r| Some(r.entity_id()) != own)
                .map(|r| (r.entity_id(), r.read(cx).task_id.clone()))
                .collect();
            // A terminal the host just focused (palette jump, 새 터미널)
            // whose panel this layout does not show yet.
            let hidden = self
                .actions
                .terminal_panels(cx)
                .into_iter()
                .find(|p| p.read(cx).view.read(cx).focus_handle(cx).is_focused(window))
                .map(|p| p.read(cx).terminal_id.clone());
            // Panels other than terminals and reviews (a rise shows the dock).
            let signature = self.actions.read(cx, |f| f.other_panels())?;
            (signature, reviews, hidden)
        };
        // Bring its task in and focus it there.
        if let Some(id) = hidden
            && let Some(task) = self.task_of(&id, cx)
            && follow_hidden(&mut self.model, &rows, &id, &task)
        {
            let target = Region::Terminal(id);
            cx.defer_in(window, move |v, window, cx| {
                v.focus_region(&target, window, cx)
            });
        }
        // A terminal focused elsewhere (palette jump, new terminal) brings its task in.
        let focused_task = focused.as_deref().and_then(|id| self.task_of(id, cx));
        follow_focus(
            &mut self.model,
            &mut self.seen_focus,
            focused.as_deref(),
            focused_task.as_deref(),
            &rows,
        );
        // A Review opened while the layout shows (Ctrl+Shift+R, PR button) becomes the canvas.
        let ids: HashSet<EntityId> = reviews.iter().map(|(e, _)| *e).collect();
        if let Some(task) = review_opened(self.dock_signature, &self.dock_reviews, &reviews) {
            self.model.select(task);
        }
        self.dock_reviews = ids;
        // Another panel opened while the layout shows (settings, editor …)
        // appears in the canvas; so does one the workspace focused for the
        // user (an orchestration page next to another, a panel asked for
        // again). Its terminals and Reviews the layout shows itself.
        let asked = self.panel_watch.asked(
            &self.actions,
            &[crate::panel::PANEL_NAME, crate::review::REVIEW_PANEL_NAME],
            window,
            cx,
        );
        if dock_rose(self.dock_signature, signature) || asked {
            self.model.show_dock = true;
            self.model.expanded = None;
        }
        self.dock_signature = signature;
        let prefer = self
            .seen_focus
            .as_deref()
            .and_then(|id| self.task_of(id, cx));
        self.model.settle(&rows, prefer.as_deref());
        if std::mem::take(&mut self.entered) {
            self.place_focus_on_enter(window, cx);
        }
        for r in &rows {
            self.row_focus(&r.id, cx);
        }
        let selected = self.model.selected.as_deref();
        let (badges, parts, who) = self.actions.read(cx, |f| {
            let parts = Selected::of(selected, f.tasks, f.terminals, f.gates);
            let who = parts
                .terms
                .iter()
                .map(|x| (x.id.clone(), Who::of(x, f.tasks, f.label_of(x))))
                .collect();
            (row_badges(&rows, f.terminals), parts, who)
        })?;
        Some(Synced {
            rows,
            badges,
            parts,
            who,
            dock,
        })
    }

    /// The Review of `task`: the workbench's own panel when the dock has one
    /// (nothing reloads), else a layout-owned one (side by side).
    fn review_for(
        &mut self,
        task: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<ReviewPanel>> {
        let own = self.own_review.as_ref().map(|p| p.entity_id());
        let existing = self
            .actions
            .review_panels(cx)
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
        // The task's first terminal's status, from the frame.
        let status = self
            .actions
            .read(cx, |f| {
                f.task(task)
                    .and_then(|k| k.terminals.first())
                    .and_then(|t| f.terminal(t))
                    .and_then(|t| t.status.clone().map(|s| (t.id.clone(), s)))
            })
            .flatten();
        // Made and registered by the host like dock reviews (task, status
        // and delivery events reach it); none without the review capability.
        let panel = self.actions.new_review_panel(task, window, cx)?;
        panel.update(cx, |r, cx| {
            // The canvas is wide: side by side, as the preset shows it.
            r.set_split(true, cx);
            if let Some((t, s)) = &status {
                r.agent_status(t, s, cx);
            }
        });
        self.own_review = Some(panel.clone());
        Some(panel)
    }

    /// Fit the canvas Review to `canvas_w`: below its own minimum (files +
    /// diff + comments) the file list goes first, then (< 560) the comment
    /// column becomes a drawer. Only a change reaches the panel.
    fn fit_review(&mut self, r: &Entity<ReviewPanel>, canvas_w: f32, cx: &mut Context<Self>) {
        let (files, drawer) = (review_files_fit(canvas_w), review_comments_drawer(canvas_w));
        if self.fitted != Some((r.entity_id(), files, drawer)) {
            self.fitted = Some((r.entity_id(), files, drawer));
            r.update(cx, |p, cx| {
                p.set_file_list(files, cx);
                p.set_comments_drawer(drawer, cx);
            });
        }
    }

    /// The panel of terminal `id`: the workbench's own when it has one (no
    /// new attach), else a layout-owned one.
    fn panel_for(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalPanel>> {
        let own: HashSet<EntityId> = self.own_panels.values().map(|p| p.entity_id()).collect();
        let existing = self
            .actions
            .terminal_panels(cx)
            .into_iter()
            .find(|p| !own.contains(&p.entity_id()) && p.read(cx).terminal_id == id);
        if let Some(p) = existing {
            self.own_panels.remove(id);
            return Some(p);
        }
        if let Some(p) = self.own_panels.get(id) {
            return Some(p.clone());
        }
        // Made and registered by the host like dock panels (labels, theme
        // changes and "terminal in view" reach it); none for an unknown id.
        let panel = self.actions.new_terminal_panel(id, window, cx)?;
        self.own_panels.insert(id.to_string(), panel.clone());
        Some(panel)
    }

    fn notify_all(&self, cx: &mut Context<Self>) {
        cx.notify();
        self.actions.post(vec![HostUpdate::Notify], cx);
    }

    /// Focus handle of terminal `id`'s panel: the layout's own, else the
    /// host's.
    fn terminal_focus(&self, id: &str, cx: &App) -> Option<FocusHandle> {
        let own = self.own_panels.get(id).cloned();
        let p = own.or_else(|| self.actions.terminal_panel(id, cx))?;
        Some(p.read(cx).view.read(cx).focus_handle(cx))
    }

    /// Select task `id` in the list (switching the canvas) and focus its row.
    fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
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
    fn focus_region(&mut self, region: &Region, window: &mut Window, cx: &mut Context<Self>) {
        match region {
            Region::Tasks => match self.model.selected.clone() {
                Some(id) => self.select(&id, window, cx),
                None => self.focus.focus(window, cx),
            },
            Region::Canvas => {
                if self.model.show_dock {
                    if let Some(dock) = self.actions.focus_handle(Slot::Dock, cx) {
                        dock.focus(window, cx);
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
        let in_review = self
            .actions
            .review_panels(cx)
            .iter()
            .any(|r| r.read(cx).focus_handle().contains_focused(window, cx));
        let in_dock = self.model.show_dock
            && self
                .actions
                .focus_handle(Slot::Dock, cx)
                .is_some_and(|dock| dock.contains_focused(window, cx));
        (in_review || in_dock).then_some(Region::Canvas)
    }

    /// F6 / Shift+F6.
    fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let regions = self.model.regions(&self.tiles(cx));
        let current = self.current_region(window, cx);
        let next = next_region(&regions, current.as_ref(), forward);
        self.focus_region(&next, window, cx);
    }

    /// Ctrl+Shift+X: the focused tile (else the expanded one, else the first) over the canvas, or back.
    fn toggle_expand(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.expand(&id, window, cx);
    }

    /// Terminal `id` over the canvas, or back to its tile; the keyboard goes with it.
    fn expand(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.model.toggle_expand(id);
        self.focus_region(&Region::Terminal(id.to_string()), window, cx);
    }

    /// The canvas tabs: 패널 (the dock) or 리뷰; the keyboard goes into it.
    fn show_canvas(&mut self, panels: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.model.show_panels(panels);
        self.focus_region(&Region::Canvas, window, cx);
    }

    /// The strip's button: tiles or only the header.
    fn toggle_strip(&mut self, cx: &mut Context<Self>) {
        self.model.strip_open = !self.model.strip_open;
        self.notify_all(cx);
    }

    /// Focus the terminal tile `id` (expanding it when it is not docked).
    fn answer(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let tiles = self.tiles(cx);
        self.model.answer(&tiles, id);
        self.focus_region(&Region::Terminal(id.to_string()), window, cx);
    }

    /// Approve / reject through the gate path (`gate.resolve`), as the gates
    /// inbox does: the request goes through the host's source, the answer (or
    /// the failure toast) and a notify into the host in one update.
    pub(super) fn resolve(
        &mut self,
        gate: String,
        decision: GateDecision,
        choice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(source) = self.actions.source(cx) else {
            return;
        };
        if !self.busy.insert(gate.clone()) {
            return;
        }
        let rx = source.call::<p::GateResolve>(&p::GateResolveParams {
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
                let answer = match r {
                    Ok(g) => HostUpdate::ApplyGate(g),
                    Err(e) => HostUpdate::Toast {
                        kind: ToastKind::Error,
                        title: s::COULD_NOT_RESOLVE.into(),
                        body: e.message.into(),
                    },
                };
                v.actions.post(vec![answer, HostUpdate::Notify], cx);
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
        // The host's live terminal or Review panel holding the focus.
        let terminal = self
            .actions
            .terminal_panels(cx)
            .into_iter()
            .find(|p| {
                p.read(cx)
                    .view
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
            .map(|p| p.read(cx).terminal_id.clone());
        let review_task = self
            .actions
            .review_panels(cx)
            .into_iter()
            .find(|r| r.read(cx).focus_handle().contains_focused(window, cx))
            .map(|r| r.read(cx).task_id.clone());
        // The focused panel's terminal and its task, from the frame.
        let terminal_task = terminal.and_then(|id| {
            self.actions
                .read(cx, |f| {
                    f.terminal(&id).map(|t| (t.id.clone(), t.task_id.clone()))
                })
                .flatten()
        });
        let in_dock = self
            .actions
            .focus_handle(Slot::Dock, cx)
            .is_some_and(|dock| dock.contains_focused(window, cx));
        if let Some((id, Some(task))) = terminal_task {
            let rows = self.rows(cx);
            self.model.enter_on_terminal(&rows, &id, &task);
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

    /// A key on task row `id`; true when the row handled it.
    fn on_row_key(
        &mut self,
        id: &str,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &ev.keystroke;
        let nav = match row_key(k.key.as_str(), k.modifiers.modified()) {
            None => return false,
            Some(RowKey::Open) => {
                self.model.select(id);
                self.focus_region(&Region::Canvas, window, cx);
                return true;
            }
            Some(RowKey::Nav(nav)) => nav,
        };
        let rows = self.rows(cx);
        if let Some(next) = self.model.nav(&rows, nav) {
            self.select(&next, window, cx);
        }
        true
    }

    /// A key on the list itself (no row focused yet): arrows pick the first task.
    fn on_list_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus.is_focused(window) && list_key(e.keystroke.key.as_str()) {
            let rows = self.rows(cx);
            if let Some(id) = rows.first().map(|r| r.id.clone()) {
                self.select(&id, window, cx);
            }
        }
    }

    /// Row height of the task list (uniform list): grows with the UI font so
    /// Korean text is never clipped at any font size or scale.
    pub(super) fn row_height(cx: &App) -> f32 {
        let k = LayoutMetrics::current(cx);
        model::row_height(k.line(), k.style.border_width)
    }
}

impl Focusable for ReviewView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ───────────────────────────── renderer ─────────────────────────────

/// The body through the layout kit: the window's view in the shell.
struct ReviewRenderer;

impl LayoutRenderer for ReviewRenderer {
    /// The keys are bound once per app. The view is made on first use with
    /// the host's [`LayoutActions`] and kept in the window's state slot (it
    /// lives as long as the window).
    fn render_body(
        &self,
        frame: &LayoutFrame<'_>,
        host: &mut dyn LayoutHost,
        window: &mut Window,
        cx: &mut App,
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
        let actions = host.actions();
        let view = actions.state_or_insert_with(window, cx, {
            let actions = actions.clone();
            move |_, cx| cx.new(|cx| ReviewView::new(actions, cx))
        });
        view::shell(
            frame.warning(),
            host.slot(Slot::HookBanner, window, cx),
            view.into_any_element(),
            host.slot(Slot::Toasts, window, cx)
                .unwrap_or_else(|| div().into_any_element()),
        )
    }
}
