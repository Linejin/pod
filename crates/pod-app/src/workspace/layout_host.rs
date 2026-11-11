//! The host side of the layout kit (docs/20-refactor/app.md §2 "Layout kit와
//! 상태 소유"): the one place that implements [`LayoutHost`] and
//! [`ActionSink`] over the workspace, so a mode on the kit never names it.
//!
//! - frame: a read-only projection borrowed from the workspace for one render;
//! - slots: the dock, sidebar, toast stack and hook banner the workspace keeps
//!   owning, lent to the body;
//! - actions: the workspace held weakly behind [`LayoutActions`]; each intent
//!   runs through the workspace method the modes call today.
//!
//! A mode without a renderer keeps `LayoutModule::render_body` (see the shell)
//! and `LayoutModule::title_bar_extra` ([`Workspace::layout_title_bar_extra`]).
use std::rc::Rc;

use gpui_kit::component::dock::DockArea;
use gpui_kit::*;
use podd::protocol::GateInfo;

use crate::layouts::kit::{
    ActionSink, LayoutActions, LayoutFrame, LayoutHost, LayoutMetrics, LayoutRenderer, OpenPanels,
    Slot, UiIntent,
};

use super::Workspace;
use super::sidebar::Sidebar;

impl Workspace {
    /// Read-only projection of the window for one render. `gates` are the
    /// pending gates (`OrchState::pending`), kept alive by the caller.
    pub(crate) fn layout_frame<'a>(&'a self, gates: &'a [GateInfo], cx: &App) -> LayoutFrame<'a> {
        LayoutFrame {
            tasks: &self.tasks,
            terminals: &self.terminals,
            conn: &self.conn,
            gates,
            focused_terminal: self.focused_terminal.as_deref(),
            panels: self.open_panels(),
            metrics: LayoutMetrics::current(cx),
        }
    }

    /// The panels the workbench's "nothing open" hint checks, minus the
    /// app-level pages (each mode checks those itself).
    fn open_panels(&self) -> OpenPanels {
        OpenPanels {
            terminals: self.live_panels().len(),
            reviews: self.live_reviews().len(),
            editors: self.live_editors().len(),
            orch: self.orch.has_panels(),
            settings: self
                .settings_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_some(),
            inspector: self
                .inspector_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_some(),
        }
    }

    /// Handle for a mode's event closures and long-lived views.
    pub(crate) fn layout_actions(&self, cx: &Context<Self>) -> LayoutActions {
        LayoutActions::new(Rc::new(Sink(cx.entity().downgrade())))
    }

    /// The body through a mode's narrow renderer. The shell calls this only
    /// for a mode that has one, and `LayoutModule::render_body` otherwise.
    pub(super) fn render_layout_body(
        &mut self,
        renderer: &dyn LayoutRenderer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // Toasts and the hook banner need the workspace context, the body only
        // `App`: build them before the frame borrows the workspace.
        let mut host = Host {
            dock: self.dock.clone(),
            sidebar: self.sidebar.clone(),
            toasts: Some(self.render_toasts(cx).into_any_element()),
            hook_banner: self.render_hook_banner(cx),
            actions: self.layout_actions(cx),
        };
        let gates = self.orch.pending();
        let frame = self.layout_frame(&gates, cx);
        renderer.render_body(&frame, &mut host, window, cx)
    }

    /// Title-bar extra of the layout in settings (the title bar in chrome):
    /// the mode's renderer when it has one, else `LayoutModule::title_bar_extra`.
    pub(crate) fn layout_title_bar_extra(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        match crate::layout_mode::renderer(cx) {
            Some(renderer) => {
                let actions = self.layout_actions(cx);
                let gates = self.orch.pending();
                let frame = self.layout_frame(&gates, cx);
                renderer.title_bar_extra(&frame, &actions, cx)
            }
            None => crate::layout_mode::title_bar_extra(self, cx),
        }
    }

    /// Keyboard focus of a slot: the dock area, the sidebar tree and the toast
    /// stack; the hook banner has none of its own.
    fn slot_focus(&self, slot: Slot, cx: &App) -> Option<FocusHandle> {
        match slot {
            Slot::Dock => Some(self.dock.read(cx).focus_handle(cx)),
            Slot::Sidebar => Some(self.sidebar_focus_handle(cx)),
            Slot::Toasts => Some(self.toast_focus.clone()),
            Slot::HookBanner => None,
        }
    }

    /// Carries out a mode's intent with the workspace method it maps to.
    fn dispatch_intent(&mut self, intent: UiIntent, window: &mut Window, cx: &mut Context<Self>) {
        match intent {
            UiIntent::OpenTerminal { id, another } => self.open_terminal(&id, another, window, cx),
            UiIntent::OpenReview { task } => self.open_review(&task, window, cx),
            UiIntent::OpenPullRequest { task, form } => self.open_pr(&task, form, window, cx),
            UiIntent::OpenEditor {
                task,
                target,
                quick_open,
            } => self.open_editor(&task, target, quick_open, window, cx),
            UiIntent::QuickOpenFiles { quick_open } => {
                self.quick_open_files(quick_open, window, cx)
            }
            UiIntent::TaskTerminal { task } => self.task_terminal(task, window, cx),
            UiIntent::NewTask => self.open_new_task_dialog(window, cx),
            UiIntent::OpenGates => self.open_gates(window, cx),
            UiIntent::OpenAgentTree => self.open_agent_tree(window, cx),
            UiIntent::OpenSettings => self.open_settings(window, cx),
            UiIntent::OpenNotifications => self.open_notifications(window, cx),
            UiIntent::OpenJump => self.open_jump(window, cx),
            UiIntent::ArrangeGrid => self.arrange_grid(window, cx),
            UiIntent::FocusRegion(slot) => {
                if let Some(focus) = self.slot_focus(slot, cx) {
                    window.focus(&focus, cx);
                }
            }
            UiIntent::Toast { kind, title, body } => self.raise_toast(kind, title, body, cx),
        }
    }
}

/// The slots of one render. The dock and the sidebar are views the workspace
/// keeps owning; the toast stack and the hook banner are built for this render
/// and lent once.
struct Host {
    dock: Entity<DockArea>,
    sidebar: Entity<Sidebar>,
    toasts: Option<AnyElement>,
    hook_banner: Option<AnyElement>,
    actions: LayoutActions,
}

impl LayoutHost for Host {
    fn slot(&mut self, slot: Slot, _: &mut Window, _: &mut App) -> Option<AnyElement> {
        match slot {
            Slot::Dock => Some(self.dock.clone().into_any_element()),
            Slot::Sidebar => Some(self.sidebar.clone().into_any_element()),
            Slot::Toasts => self.toasts.take(),
            Slot::HookBanner => self.hook_banner.take(),
        }
    }

    fn actions(&self) -> LayoutActions {
        self.actions.clone()
    }
}

/// The workspace behind [`LayoutActions`], held weakly: once the window is
/// gone there is no frame and intents do nothing.
struct Sink(WeakEntity<Workspace>);

impl ActionSink for Sink {
    fn with_frame(&self, cx: &App, f: &mut dyn FnMut(&LayoutFrame<'_>)) {
        let Some(ws) = self.0.upgrade() else {
            return;
        };
        let ws = ws.read(cx);
        let gates = ws.orch.pending();
        f(&ws.layout_frame(&gates, cx));
    }

    fn focus_handle(&self, slot: Slot, cx: &App) -> Option<FocusHandle> {
        self.0.upgrade()?.read(cx).slot_focus(slot, cx)
    }

    fn dispatch(&self, intent: UiIntent, window: &mut Window, cx: &mut App) {
        let _ = self
            .0
            .update(cx, |ws, cx| ws.dispatch_intent(intent, window, cx));
    }
}
