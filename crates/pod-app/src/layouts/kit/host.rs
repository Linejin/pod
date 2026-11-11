//! What a layout's own views need from the window that hosts them, outside a
//! host render (A-T2a, docs/20-refactor/app.md "A-T2a 구현"):
//!
//! - [`WindowKey`] and the per-window state slots: one entity per state type
//!   and window (the rail state, the queue column, the dense view, …),
//!   dropped when the window's host is released;
//! - [`HostChange`]: change observation through the host, so a view never
//!   observes the workspace itself;
//! - dock and panel access, panels made outside the dock;
//! - [`HostUpdate`]: updates that need no window (async completions);
//! - the one gate decision path and the podd source.
//!
//! Everything goes through [`LayoutActions`]; only `workspace/layout_host.rs`
//! implements the host side.
use std::any::TypeId;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::dock::DockArea;
use gpui_kit::{
    AnyEntity, AnyView, AnyWindowHandle, App, Entity, EntityId, Global, SharedString, Subscription,
    Window,
};
use podd::protocol::{GateInfo, GateResolveParams};

use super::model::{LayoutActions, Slot, UiIntent};
use crate::gates::ResolveError;
use crate::panel::TerminalPanel;
use crate::review::ReviewPanel;
use crate::source::PoddSource;
use crate::vm::TerminalVm;
use crate::workspace::ToastKind;

/// Runs once with the app (a release hook).
pub(crate) type AppOnce = Box<dyn FnOnce(&mut App)>;
/// Runs after each observed change.
pub(crate) type OnChange = Box<dyn FnMut(&mut App)>;
/// Runs after each observed change, with the window.
pub(crate) type OnChangeIn = Box<dyn FnMut(&mut Window, &mut App)>;
/// Runs once podd answered a gate decision (or the request failed).
pub(crate) type GateDone = Box<dyn FnOnce(Result<(), ResolveError>, &mut App)>;

/// "A dock panel was asked for", for a layout that draws the dock only in a
/// drawer or a tab (kanban, review, blocks): [`Self::asked`] is true once
/// each time the focus lands on a dock panel whose name is not skipped.
/// The workspace focuses each panel it opens or brings back, so this also
/// sees an orchestration page opened next to another one and a panel asked
/// for again, which no panel count shows.
#[derive(Debug, Default)]
pub(crate) struct PanelWatch {
    /// The dock panel that held the focus last frame (not a skipped one).
    seen: Option<EntityId>,
}

impl PanelWatch {
    /// Call once per frame. `skip`: panel names the layout shows on its own
    /// (its terminals, the review layout's Reviews).
    pub(crate) fn asked(
        &mut self,
        actions: &LayoutActions,
        skip: &[&str],
        window: &Window,
        cx: &App,
    ) -> bool {
        let now = actions
            .focused_dock_panel(window, cx)
            .filter(|(_, name)| !skip.contains(name))
            .map(|(id, _)| id);
        let asked = now.is_some() && now != self.seen;
        self.seen = now;
        asked
    }
}

/// Stable key of one hosted window (the host entity's id). It outlives the
/// host, so it also names the window's state slots while they are dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct WindowKey(pub(crate) EntityId);

/// What changed in the host, for [`LayoutActions::observe`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum HostChange {
    /// The window's data changed: the host notified (tasks, terminals,
    /// statuses, gates, toasts, labels, panels opened or closed).
    Data,
    /// The dock's layout changed (`DockEvent::LayoutChanged`): a panel was
    /// added, removed, moved or a tab selected.
    DockLayout,
    /// The tree sidebar notified (its selection among others).
    Tree,
}

/// An update that needs no window: what async completions (a rerun, a gate
/// answer, a copy) do to the host today. [`LayoutActions::post`] applies a
/// list in order inside one host update.
#[derive(Clone, Debug)]
pub(crate) enum HostUpdate {
    /// Redraw the window (`cx.notify()` on the host): title, status bar and
    /// any body the host draws.
    Notify,
    /// Raise a toast (`Workspace::raise_toast`).
    Toast {
        kind: ToastKind,
        title: SharedString,
        body: SharedString,
    },
    /// Merge podd's answer for a gate (`OrchState::apply`). Follow it with
    /// `Notify` as the caller's path did.
    ApplyGate(GateInfo),
    /// A terminal podd just started (rerun) joins the list unless it is
    /// there already; the next refresh replaces the list anyway.
    AddTerminal(TerminalVm),
}

// ───────────────────────────── state slots ─────────────────────────────

/// Per-window layout state: one entity per type, plus the window's handle
/// for app-level shortcuts that start from a window.
#[derive(Default)]
struct LayoutStates(HashMap<WindowKey, WindowStates>);
impl Global for LayoutStates {}

struct WindowStates {
    window: AnyWindowHandle,
    states: HashMap<TypeId, AnyEntity>,
}

impl LayoutStates {
    fn get<T: 'static>(&self, key: WindowKey) -> Option<Entity<T>> {
        self.0
            .get(&key)?
            .states
            .get(&TypeId::of::<T>())?
            .clone()
            .downcast::<T>()
            .ok()
    }
}

/// The `T` state of a window, if one was made.
pub(crate) fn state<T: 'static>(key: WindowKey, cx: &App) -> Option<Entity<T>> {
    cx.try_global::<LayoutStates>()?.get(key)
}

/// The `T` state of the hosted window behind `window` (app-level actions
/// such as Ctrl+Shift+U start from the active window, not from a host).
pub(crate) fn state_in_window<T: 'static>(window: AnyWindowHandle, cx: &App) -> Option<Entity<T>> {
    let states = cx.try_global::<LayoutStates>()?;
    let key = states
        .0
        .iter()
        .find(|(_, w)| w.window == window)
        .map(|(k, _)| *k)?;
    states.get(key)
}

/// Every window's `T` state, in no particular order.
pub(crate) fn states<T: 'static>(cx: &App) -> Vec<Entity<T>> {
    cx.try_global::<LayoutStates>()
        .map(|s| s.0.keys().filter_map(|k| s.get::<T>(*k)).collect())
        .unwrap_or_default()
}

/// Drops every state of the window. The host calls this when it is
/// released; the states' subscriptions go with them.
pub(crate) fn release_window(key: WindowKey, cx: &mut App) {
    let dropped = cx
        .try_global::<LayoutStates>()
        .is_some_and(|s| s.0.contains_key(&key));
    if dropped {
        // Take the entities out first: dropping them outside the global's
        // lease lets their own release hooks read it.
        let gone = cx.global_mut::<LayoutStates>().0.remove(&key);
        drop(gone);
    }
}

// ───────────────────────────── the handle ─────────────────────────────

impl LayoutActions {
    /// Key of the window this handle reaches.
    pub(crate) fn window_key(&self) -> WindowKey {
        self.0.window_key()
    }

    /// This window's `T` state, if one was made.
    pub(crate) fn state<T: 'static>(&self, cx: &App) -> Option<Entity<T>> {
        state(self.window_key(), cx)
    }

    /// This window's `T` state, made by `make` on first use. It lives until
    /// the host is released (explicit lifetime: the slot, not a subscriber,
    /// owns it). When the host is already gone the new state is returned but
    /// not kept.
    pub(crate) fn state_or_insert_with<T: 'static>(
        &self,
        window: &mut Window,
        cx: &mut App,
        make: impl FnOnce(&mut Window, &mut App) -> Entity<T>,
    ) -> Entity<T> {
        let key = self.window_key();
        if let Some(s) = state::<T>(key, cx) {
            return s;
        }
        let made = make(window, cx);
        let known = cx
            .try_global::<LayoutStates>()
            .is_some_and(|s| s.0.contains_key(&key));
        if !known
            && !self
                .0
                .on_release(cx, Box::new(move |cx| release_window(key, cx)))
        {
            return made;
        }
        let handle = window.window_handle();
        cx.default_global::<LayoutStates>()
            .0
            .entry(key)
            .or_insert_with(|| WindowStates {
                window: handle,
                states: HashMap::new(),
            })
            .states
            .insert(TypeId::of::<T>(), made.clone().into_any());
        made
    }

    /// Calls `f` on the entity after each of `changes` — instead of
    /// `cx.observe(workspace)` and a dock subscription. One subscription for
    /// all; `None` when the host is gone or `changes` is empty.
    pub(crate) fn observe<T: 'static>(
        &self,
        changes: &[HostChange],
        cx: &mut gpui_kit::Context<T>,
        f: impl FnMut(&mut T, HostChange, &mut gpui_kit::Context<T>) + 'static,
    ) -> Option<Subscription> {
        let me = cx.entity().downgrade();
        let f = Rc::new(RefCell::new(f));
        let mut out: Option<Subscription> = None;
        for &change in changes {
            let (me, f) = (me.clone(), f.clone());
            let sub = self.0.observe(
                change,
                cx,
                Box::new(move |cx| {
                    let _ = me.update(cx, |t, cx| (f.borrow_mut())(t, change, cx));
                }),
            )?;
            out = Some(match out {
                Some(a) => Subscription::join(a, sub),
                None => sub,
            });
        }
        out
    }

    /// [`Self::observe`] with the window, like `Context::observe_in`.
    pub(crate) fn observe_in<T: 'static>(
        &self,
        changes: &[HostChange],
        window: &mut Window,
        cx: &mut gpui_kit::Context<T>,
        f: impl FnMut(&mut T, HostChange, &mut Window, &mut gpui_kit::Context<T>) + 'static,
    ) -> Option<Subscription> {
        let me = cx.entity().downgrade();
        let f = Rc::new(RefCell::new(f));
        let mut out: Option<Subscription> = None;
        for &change in changes {
            let (me, f) = (me.clone(), f.clone());
            let sub = self.0.observe_in(
                change,
                window,
                cx,
                Box::new(move |window, cx| {
                    let _ = me.update(cx, |t, cx| (f.borrow_mut())(t, change, window, cx));
                }),
            )?;
            out = Some(match out {
                Some(a) => Subscription::join(a, sub),
                None => sub,
            });
        }
        out
    }

    /// The live dock (read its layout, select or remove panels, subscribe).
    pub(crate) fn dock(&self, cx: &App) -> Option<Entity<DockArea>> {
        self.0.dock(cx)
    }

    /// A host view a layout's own view draws (`Slot::Dock`, `Slot::Sidebar`);
    /// the toast stack and hook banner are lent by a host render only.
    pub(crate) fn view(&self, slot: Slot, cx: &App) -> Option<AnyView> {
        self.0.view(slot, cx)
    }

    /// The dock panel holding the keyboard focus: its entity and its panel
    /// name (`Panel::panel_name`). The workspace focuses every panel it opens
    /// or brings back (settings, the gates inbox, an orchestration page …),
    /// so a layout that draws the dock in a drawer or a tab reads "a panel
    /// was asked for" here even while the dock is hidden: the panel's own
    /// focus matches before it was ever drawn. `None` when the focus is
    /// elsewhere or the host is gone.
    pub(crate) fn focused_dock_panel(
        &self,
        window: &Window,
        cx: &App,
    ) -> Option<(EntityId, &'static str)> {
        use gpui_kit::component::dock::DockPlacement;
        let dock = self.dock(cx)?;
        let area = dock.read(cx);
        let ids: Vec<_> = [
            DockPlacement::Center,
            DockPlacement::Left,
            DockPlacement::Right,
            DockPlacement::Bottom,
        ]
        .into_iter()
        .filter_map(|p| area.layout(p))
        .flat_map(|tree| tree.panels())
        .collect();
        ids.into_iter()
            .filter_map(|id| area.panel(id).cloned())
            .find(|p| p.focus_handle(cx).contains_focused(window, cx))
            .map(|p| (p.view().entity_id(), p.panel_name(cx)))
    }

    /// The task the user works in (`Workspace::current_task`): it reads
    /// panel focus, so it needs the window and is not on the frame.
    pub(crate) fn current_task(&self, window: &Window, cx: &App) -> Option<String> {
        self.0.current_task(window, cx)
    }

    /// Live terminal panels in creation order (`Workspace::live_panels`).
    pub(crate) fn terminal_panels(&self, cx: &App) -> Vec<Entity<TerminalPanel>> {
        self.0.terminal_panels(cx)
    }

    /// The first live panel showing terminal `id`.
    pub(crate) fn terminal_panel(&self, id: &str, cx: &App) -> Option<Entity<TerminalPanel>> {
        self.terminal_panels(cx)
            .into_iter()
            .find(|p| p.read(cx).terminal_id == id)
    }

    /// Live Review panels (`Workspace::live_reviews`).
    pub(crate) fn review_panels(&self, cx: &App) -> Vec<Entity<ReviewPanel>> {
        self.0.review_panels(cx)
    }

    /// A terminal panel for a layout's own view (not in the dock), registered
    /// like dock panels so labels, theme and "terminal in view" reach it.
    /// `None` for a terminal the frame does not know or a gone host.
    pub(crate) fn new_terminal_panel(
        &self,
        id: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<TerminalPanel>> {
        self.0.new_terminal_panel(id, window, cx)
    }

    /// A Review panel for a layout's own view, registered like dock reviews
    /// (task, status and delivery events reach it). `None` without the
    /// review capability, for an unknown task or a gone host.
    pub(crate) fn new_review_panel(
        &self,
        task: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<ReviewPanel>> {
        self.0.new_review_panel(task, window, cx)
    }

    /// The window's podd source, for the requests a layout's controller makes
    /// itself (`terminal.read`, `terminal.run`, attach, its own gate path).
    pub(crate) fn source(&self, cx: &App) -> Option<PoddSource> {
        self.0.source(cx)
    }

    /// Applies window-less updates in order inside one host update (async
    /// completions have no window for [`Self::dispatch`]). `false` when the
    /// host is gone. Not from inside a host update.
    pub(crate) fn post(&self, updates: Vec<HostUpdate>, cx: &mut App) -> bool {
        self.0.post(updates, cx)
    }

    /// Carries the intent out now, then `then` once the host update is over
    /// (e.g. the rail lays its dock out after `OpenTaskTerminals`).
    pub(crate) fn dispatch_then(
        &self,
        intent: UiIntent,
        window: &mut Window,
        cx: &mut App,
        then: impl FnOnce(&mut Window, &mut App),
    ) {
        self.0.dispatch(intent, window, cx);
        then(window, cx);
    }

    /// 이번만 허용 / 거부 through the one gate decision path
    /// (`gates::resolve_gate`): sends `gate.resolve`, merges the answer and
    /// touches the host, or toasts the failure; `done` follows for the
    /// caller's busy/error state. `false` (nothing sent, `done` dropped)
    /// when the host is gone.
    pub(crate) fn resolve_gate(
        &self,
        params: GateResolveParams,
        cx: &mut App,
        done: impl FnOnce(Result<(), ResolveError>, &mut App) + 'static,
    ) -> bool {
        self.0.resolve_gate(params, cx, Box::new(done))
    }
}
