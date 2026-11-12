//! 칸반 보드 (preset 12, docs/10 §D5.5): one card per agent/worker terminal in
//! three columns — 작업 중 / 응답 필요 / 완료 — and a terminal drawer below.
//!
//! - **Columns follow the agent.** A card's column comes only from the status
//!   podd reports (`StatusVm`) and the terminal's lifecycle (`model::column_of`).
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
//!
//! Files (docs/20-refactor/app.md A-P12): `model` holds the cards, the board
//! model (order, pins, selection, filters, keyboard), the card actions and
//! the size formulas, `view` the render, `copy` the strings. This file keeps
//! the layout module with its narrow renderer and the [`Board`] with its
//! controller methods.
//!
//! A-T2c: the board is drawn through `LayoutModule::renderer` and reaches the
//! window only through the kit: the frame (terminals, tasks, gates, labels,
//! the focused terminal, the panel counts, the connection warning, fonts),
//! the host's slots (hook banner, toasts) and [`LayoutActions`] (the board as
//! the window's state slot, the dock's view and focus, the live terminal
//! panels and a drawer-owned one, the podd source, the gate path, window-less
//! updates for a rerun terminal, toasts and the redraw).
pub mod copy;
mod model;
mod view;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::*;
use podd::protocol::{self as p, GateDecision};

pub use self::copy as s;
pub use self::model::{
    BoardModel, Card, Column, Nav, Refused, build_cards, card_name, filter_choices,
};

use self::model::{CardKey, card_key, dock_rose, follow_focus, nav_key, task_count};
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{
    HostUpdate, LayoutActions, LayoutFrame, LayoutHost, LayoutMetrics, LayoutRenderer, PanelWatch,
    Slot,
};
use crate::panel::TerminalPanel;
use crate::source::RunSpec;
use crate::workspace::ToastKind;

actions!(pod_kanban, [SwitchRegion]);

/// Key context of the board (F6 is bound only inside it).
const KEY_CONTEXT: &str = "PodKanban";

// ───────────────────────────── module ─────────────────────────────

/// The layout module (registered in [`crate::layout_mode::LayoutRegistry::new`]).
pub struct Kanban;

impl LayoutModule for Kanban {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Kanban
    }
    fn renderer(&self) -> Rc<dyn LayoutRenderer> {
        Rc::new(BoardRenderer)
    }
}

struct KanbanKeys;
impl Global for KanbanKeys {}

/// The window body over the kit: the board under the connection warning and
/// the hook prompt, the toasts over it.
struct BoardRenderer;

impl LayoutRenderer for BoardRenderer {
    fn render_body(
        &self,
        frame: &LayoutFrame<'_>,
        host: &mut dyn LayoutHost,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if cx.try_global::<KanbanKeys>().is_none() {
            cx.bind_keys([KeyBinding::new("f6", SwitchRegion, Some(KEY_CONTEXT))]);
            cx.set_global(KanbanKeys);
        }
        // The window's board, made on first use; it lives as long as the
        // window's host, across layout switches.
        let actions = host.actions();
        let board = actions.state_or_insert_with(window, cx, |window, cx| {
            let actions = actions.clone();
            cx.new(|cx| Board::new(actions, window, cx))
        });
        let hook_banner = host.slot(Slot::HookBanner, window, cx);
        let toasts = host
            .slot(Slot::Toasts, window, cx)
            .unwrap_or_else(|| div().into_any_element());
        view::shell(
            frame.warning(),
            hook_banner,
            board.into_any_element(),
            toasts,
        )
    }
}

#[cfg(test)]
/// The board of this app's window, if the kanban layout was shown once (tests).
pub(super) fn board(cx: &App) -> Option<Entity<Board>> {
    crate::layouts::kit::states::<Board>(cx).into_iter().next()
}

// ───────────────────────────── board ─────────────────────────────

pub struct Board {
    /// The host: frame reads, focus slots, panels, podd and updates.
    actions: LayoutActions,
    pub(super) model: BoardModel,
    /// Focus of the board when it has no card.
    focus: FocusHandle,
    card_focus: HashMap<String, FocusHandle>,
    scroll: [UniformListScrollHandle; 3],
    /// Last output line per terminal: (status key it was read for, line).
    lines: HashMap<String, (String, Option<String>)>,
    reading: HashSet<String>,
    /// Gates being resolved (a second press is ignored; the buttons stay focusable).
    busy: HashSet<String>,
    pub(super) drawer_open: bool,
    drawer_max: bool,
    /// The drawer shows the dock (other panels) instead of the opened card.
    pub(super) show_dock: bool,
    /// Drawer panel for a terminal the workbench has no panel for.
    own_panel: Option<Entity<TerminalPanel>>,
    dock_signature: usize,
    /// A dock panel asked for by focus (an orchestration page next to
    /// another, a panel brought back): the drawer shows the panels.
    panel_watch: PanelWatch,
    seen_focus: Option<String>,
    /// The layout was just switched to (or the app started in it): place focus once.
    entered: bool,
    /// The settings name this layout (to see the switch *to* it).
    shown: bool,
    _subs: Vec<Subscription>,
}

/// What the render needs after the frame's bookkeeping ([`Board::sync`]).
struct Synced {
    cards: Vec<Card>,
    cols: [Vec<Card>; 3],
    /// Tasks the terminals belong to.
    task_count: usize,
    /// The window's dock, shown by the drawer's 패널 view.
    dock: AnyView,
}

impl Board {
    fn new(actions: LayoutActions, _window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            actions,
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
            panel_watch: PanelWatch::default(),
            seen_focus: None,
            entered: true,
            shown: true,
            _subs: subs,
        }
    }

    #[cfg(test)]
    /// The drawer-owned panel, if any (tests).
    pub(super) fn own_panel(&self) -> Option<Entity<TerminalPanel>> {
        self.own_panel.clone()
    }

    pub(super) fn card_focus(&mut self, id: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.card_focus
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// Cards of the frame's terminals, tasks and gates, named as the window
    /// names them (`LayoutFrame::label_of`).
    pub(super) fn cards(&self, cx: &App) -> Vec<Card> {
        self.actions.read(cx, Self::cards_of).unwrap_or_default()
    }

    fn cards_of(f: &LayoutFrame<'_>) -> Vec<Card> {
        build_cards(f.terminals, f.tasks, f.gates, |t| f.label_of(t))
    }

    /// Panels other than terminals the dock holds (a rise switches the drawer
    /// to them): the frame's other panels, one each, and the reviews.
    fn dock_signature(f: &LayoutFrame<'_>) -> usize {
        f.other_panels() + f.panels.reviews
    }

    /// The host is still there (its dock goes with it).
    fn hosted(&self, cx: &App) -> bool {
        self.actions.dock(cx).is_some()
    }

    /// The frame's bookkeeping before the board draws: follow a terminal
    /// focused elsewhere, show panels opened meanwhile, keep the selection on
    /// a visible card, place focus once after entering, and make the cards'
    /// focus handles. `None` once the host is gone.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Synced> {
        let (cards, focused, task_count, signature) = self.actions.read(cx, |f| {
            (
                Self::cards_of(f),
                f.focused_terminal.map(str::to_owned),
                task_count(f.terminals),
                Self::dock_signature(f),
            )
        })?;
        let dock = self.actions.view(Slot::Dock, cx)?;
        // A terminal focused elsewhere (palette jump, new terminal) opens in the drawer.
        if follow_focus(
            &mut self.model,
            &mut self.seen_focus,
            focused.as_deref(),
            &cards,
        ) {
            self.drawer_open = true;
            self.show_dock = false;
        }
        // A panel opened while the board shows (settings, review …) appears
        // in the drawer; so does one the workspace focused for the user (an
        // orchestration page next to another, a panel asked for again).
        let asked = self
            .panel_watch
            .asked(&self.actions, &[crate::panel::PANEL_NAME], window, cx);
        if dock_rose(self.dock_signature, signature) || asked {
            self.drawer_open = true;
            self.show_dock = true;
        }
        self.dock_signature = signature;
        let cols = self.model.columns(&cards);
        self.model.settle_selection(&cols);
        if std::mem::take(&mut self.entered) {
            self.place_focus_on_enter(window, cx);
        }
        for c in &cards {
            self.card_focus(&c.id, cx);
        }
        Some(Synced {
            cards,
            cols,
            task_count,
            dock,
        })
    }

    /// The panel the drawer shows for the opened card: the workbench's own
    /// panel when it has one (no new attach), else a drawer-owned one.
    fn drawer_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalPanel>> {
        let id = self.model.opened.clone()?;
        if !self.hosted(cx) {
            return None;
        }
        let own = self.own_panel.as_ref().map(|p| p.entity_id());
        let existing = self
            .actions
            .terminal_panels(cx)
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
        // A terminal the frame knows, registered like dock panels (labels,
        // theme changes and "terminal in view" reach it); `None` otherwise.
        let panel = self.actions.new_terminal_panel(&id, window, cx)?;
        self.own_panel = Some(panel.clone());
        Some(panel)
    }

    /// Redraws the board and the window around it.
    fn notify_all(&self, cx: &mut Context<Self>) {
        cx.notify();
        self.actions.post(vec![HostUpdate::Notify], cx);
    }

    pub(super) fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
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
    fn open(&mut self, id: &str, focus: bool, window: &mut Window, cx: &mut Context<Self>) {
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

    /// The window's live terminal panel whose view holds the focus.
    fn focused_panel(&self, window: &Window, cx: &App) -> Option<Entity<TerminalPanel>> {
        self.actions.terminal_panels(cx).into_iter().find(|p| {
            p.read(cx)
                .view
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        })
    }

    /// F6: board ↔ drawer.
    pub(super) fn switch_region(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The dock is drawn inside the board's root, whose focus therefore
        // "contains" it: ask the dock first, or F6 from the dock lands on
        // the dock again instead of the board.
        let in_dock = self.show_dock
            && self
                .actions
                .focus_handle(Slot::Dock, cx)
                .is_some_and(|dock| dock.contains_focused(window, cx));
        let in_drawer = self.drawer_open
            && self.hosted(cx)
            && (self.focused_panel(window, cx).is_some()
                || in_dock
                || self.show_dock && !self.board_focused(window, cx));
        if in_drawer || !self.drawer_open {
            match self.model.selected.clone() {
                Some(id) => self.select(&id, window, cx),
                None => self.focus.focus(window, cx),
            }
            return;
        }
        if self.show_dock {
            if let Some(dock) = self.actions.focus_handle(Slot::Dock, cx) {
                dock.focus(window, cx);
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
    fn place_focus_on_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            return;
        }
        if let Some(panel) = self.focused_panel(window, cx) {
            let id = panel.read(cx).terminal_id.clone();
            self.model.selected = Some(id.clone());
            self.model.opened = Some(id);
            self.drawer_open = true;
            self.show_dock = false;
            return;
        }
        if self
            .actions
            .focus_handle(Slot::Dock, cx)
            .is_some_and(|dock| dock.contains_focused(window, cx))
        {
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

    /// A key on the board itself (no card focused yet): arrows pick the first card.
    fn on_board_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.focus.is_focused(window) && nav_key(e.keystroke.key.as_str()).is_some() {
            let cols = self.model.columns(&self.cards(cx));
            if let Some(id) = self.model.nav(&cols, None, Nav::Home) {
                self.select(&id, window, cx);
            }
        }
    }

    /// A key on a card; true when the card handled it.
    fn on_card_key(
        &mut self,
        id: &str,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &ev.keystroke;
        let Some(key) = card_key(k.key.as_str(), k.modifiers.alt, k.modifiers.modified()) else {
            return false;
        };
        match key {
            CardKey::Open => self.open(id, true, window, cx),
            CardKey::Move { down } => {
                let cards = self.cards(cx);
                if self.model.move_by(&cards, id, down).is_ok() {
                    self.select(id, window, cx);
                }
            }
            CardKey::PinTop => {
                let cards = self.cards(cx);
                let column = cards.iter().find(|c| c.id == id).map(|c| c.column);
                let r = column.map_or(Err(Refused::Unknown), |c| self.model.pin_top(&cards, id, c));
                if r.is_ok() {
                    self.select(id, window, cx);
                }
            }
            CardKey::Nav(nav) => {
                let cols = self.model.columns(&self.cards(cx));
                if let Some(next) = self.model.nav(&cols, Some(id), nav) {
                    self.select(&next, window, cx);
                }
            }
        }
        true
    }

    fn drop_card(
        &mut self,
        d: &view::DragCard,
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
    pub(super) fn resolve(
        &mut self,
        card: String,
        gate: String,
        decision: GateDecision,
        choice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.hosted(cx) || !self.busy.insert(gate.clone()) {
            return;
        }
        let me = cx.entity().downgrade();
        let window_handle = window.window_handle();
        let params = p::GateResolveParams {
            id: gate.clone(),
            decision,
            note: None,
            choice,
        };
        self.actions.resolve_gate(params, cx, move |_, cx| {
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
    fn rerun(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(source) = self.actions.source(cx) else {
            return;
        };
        let Some(t) = self.actions.read(cx, |f| f.terminal(id).cloned()).flatten() else {
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
        let rx = source.run(spec);
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |b, cx| {
                // One host update each, as the workspace did them: the new
                // terminal joins the list and the window redraws, or a toast.
                let error = |body: SharedString| HostUpdate::Toast {
                    kind: ToastKind::Error,
                    title: s::COULD_NOT_RERUN.into(),
                    body,
                };
                match r {
                    Ok(Ok(new)) => {
                        b.actions.post(
                            vec![HostUpdate::AddTerminal(new.clone()), HostUpdate::Notify],
                            cx,
                        );
                        b.model.selected = Some(new.id.clone());
                        b.model.opened = Some(new.id);
                        b.drawer_open = true;
                        b.show_dock = false;
                    }
                    Ok(Err(e)) => {
                        b.actions.post(vec![error(e.message.into())], cx);
                    }
                    Err(_) => {
                        b.actions
                            .post(vec![error(crate::strings::PODD_CLIENT_STOPPED.into())], cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Read the last line of visible cards whose status changed since the last read.
    fn want_lines(&mut self, wanted: Vec<(String, String)>, cx: &mut Context<Self>) {
        let Some(source) = self.actions.source(cx) else {
            return;
        };
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
    pub(super) fn line(&self, id: &str) -> Option<&str> {
        self.lines.get(id).and_then(|(_, l)| l.as_deref())
    }

    /// Item height of every card (uniform list): grows with the UI font so
    /// Korean text is never clipped at any font size or scale.
    pub(super) fn card_height(cx: &App) -> f32 {
        let k = LayoutMetrics::current(cx);
        model::card_height(k.line(), pod_ui::hit_area(), k.style.border_width)
    }
}

impl Focusable for Board {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
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

#[cfg(test)]
mod tests;
