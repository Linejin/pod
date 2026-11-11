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
//!
//! Files (docs/20-refactor/app.md A-P10): `model` holds the pure arrangement
//! and selection plans, `view` the render, `copy` the strings. This file keeps
//! the layout module, the per-window [`RailState`] with its controller methods
//! and the dock slots. Reads go through the kit's frame and UI actions through
//! [`LayoutActions`]; what the kit does not carry yet stays a small legacy
//! adapter over the workspace: observing it, its current task, the workbench
//! dock entity, the sidebar view and opening a task's terminals in one update.
mod copy;
mod model;
mod view;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::button::Button;
use gpui_kit::component::dock::{
    BasePanelView, DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, PaneNode, PaneRef,
    PanelControl, PanelHandle, PanelId, PanelState, PanelStyle, PanelView, TabGroup, TitleStyle,
};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::*;

use self::model::{
    AttentionTarget, Drawer, Owner, Shape, TaskRow, attention, plan, step, tab_tasks, task_rows,
};
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{self, LayoutActions, LayoutFrame, UiIntent};
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

/// The workbench dock's panels in dock order, terminals moved into task
/// order (stable for everything else), each with its owner; and which of
/// them are terminal panels.
type WorkbenchPanels = (Vec<(PanelId, Arc<dyn PanelView>, Owner)>, HashSet<PanelId>);

fn workbench_panels(area: &DockArea, f: &LayoutFrame<'_>, cx: &App) -> WorkbenchPanels {
    let mut terminal_panels = HashSet::new();
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
    for t in f.tasks {
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
            let owner = model::panel_owner(
                &state.panel_name,
                terminal.as_deref(),
                crate::layout::leaf_field(&state, "task"),
                f.tasks,
                f.terminals,
            );
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
    (
        v.into_iter().map(|(id, h, o, _)| (id, h, o)).collect(),
        terminal_panels,
    )
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
    /// Legacy adapter: observed for changes, asked for the current task, the
    /// sidebar view and a task's terminals (not in the kit yet).
    ws: WeakEntity<Workspace>,
    /// Reads (frame), focus slots and UI intents of the host.
    actions: LayoutActions,
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
        actions: LayoutActions,
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
            actions,
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
        model::task_panels(
            self.inner
                .iter()
                .map(|(id, _, o)| (*id, o, self.terminal_panels.contains(id))),
            self.current.as_deref(),
        )
    }

    /// Re-read the workbench dock and the workspace data, then show the current
    /// task's panels. Runs on change events only.
    pub(crate) fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        // Workbench panels and their owners; rows, 응답 대기 and the font.
        let Some(((inner, terminal_panels), rows, font)) = self.actions.read(cx, |f| {
            let panels = workbench_panels(self.workbench.read(cx), f, cx);
            let open: HashSet<String> = panels
                .0
                .iter()
                .filter_map(|(_, _, o)| {
                    if let Owner::Task(t) = o {
                        Some(t.clone())
                    } else {
                        None
                    }
                })
                .collect();
            let rows = task_rows(f.tasks, f.terminals, f.gates, &open);
            (panels, rows, f.metrics.ui_font)
        }) else {
            return;
        };
        self.inner = inner;
        self.terminal_panels = terminal_panels;
        self.rows = rows;
        // Current task, tabs, 응답 대기.
        self.current = model::keep_or_pick(self.current.take(), &self.rows, || {
            ws.read(cx).current_task(window, cx)
        });
        self.font = font;
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
                model::push_recent(&mut self.recent, prev);
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

    /// Legacy adapter: the workspace's sidebar view for the 작업 공간 drawer
    /// (the kit lends `Slot::Sidebar` only inside a host render).
    fn sidebar(&self, cx: &App) -> Option<AnyElement> {
        self.ws
            .upgrade()
            .map(|ws| ws.read(cx).sidebar.clone().into_any_element())
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
        self.list_cursor = model::drawer_cursor(which, &self.rows, self.current.as_deref());
        self.list_scroll
            .scroll_to_item(self.list_cursor, ScrollStrategy::Center);
        match which {
            Drawer::Workspace => {
                if let Some(h) = self.actions.focus_handle(kit::Slot::Sidebar, cx) {
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
            // After this update: the workspace opens (or focuses) the waiting terminal.
            match row.attention_target() {
                AttentionTarget::Terminal(id) => {
                    self.actions
                        .defer(UiIntent::OpenTerminal { id, another: false }, window, cx)
                }
                AttentionTarget::Gates => self.actions.defer(UiIntent::OpenGates, window, cx),
                AttentionTarget::Nothing => {}
            }
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
                if let Some(task) = self.current.clone() {
                    self.actions.defer(
                        UiIntent::OpenEditor {
                            task,
                            target: None,
                            quick_open: false,
                        },
                        window,
                        cx,
                    );
                }
            }
            RailButton::Agents => {
                self.drawer = None;
                self.actions.defer(UiIntent::OpenAgentTree, window, cx);
            }
        }
        cx.notify();
    }

    /// Open every running terminal of the current task (or start a shell when it has none), laid out fresh.
    ///
    /// Legacy adapter: the shell and the terminals open inside one workspace
    /// update, which a sequence of intents would split.
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

    fn step_task(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.tabs.iter().map(|i| self.rows[*i].id.clone()).collect();
        if let Some(next) = step(&ids, self.current.as_deref(), delta) {
            let keep_strip = self.strip_focus.is_focused(window);
            self.switch_to(&next, !keep_strip, window, cx);
        }
    }
}

// ---- legacy adapter: the body through `LayoutModule::render_body` ----------

/// The window body. The rail state is created on first use with the
/// workspace (legacy adapter) and the host's [`LayoutActions`].
fn render_body(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let id = cx.entity_id();
    let rail = match state_of(id, cx) {
        Some(r) => r,
        None => {
            let me = cx.entity();
            let workbench = ws.dock.clone();
            let actions = ws.layout_actions(cx);
            let r = cx.new(|cx| RailState::new(me, workbench, actions, window, cx));
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
    view::shell(
        kit::connection_warning(&ws.conn),
        ws.render_hook_banner(cx),
        rail.into_any_element(),
        ws.render_toasts(cx).into_any_element(),
    )
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
