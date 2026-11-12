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
//! Every mode, the workbench included, draws its body and title-bar extra
//! through its renderer here (card T2): the workspace has no other body path.
//!
//! A-T2a: the window key and release hook behind the kit's state slots,
//! change observation (workspace notify, dock `LayoutChanged`, tree), dock and
//! panel access, window-less [`HostUpdate`]s and the gate path, so a mode's
//! own views reach all of it through [`LayoutActions`].
//!
//! rf2-kit: [`Sink`] implements both halves of the kit's host side with no
//! defaults — [`HostHandles`] from handles captured at creation (never the
//! workspace, so they work while it renders) and [`HostCommands`] through
//! the weak workspace.
use std::rc::Rc;

use gpui_kit::component::dock::{DockArea, DockEvent, PanelId};
use gpui_kit::*;
use podd::protocol::{GateInfo, GateResolveParams};

use crate::layouts::kit::{
    AppOnce, FrameToasts, GateDone, HostChange, HostCommands, HostHandles, HostUpdate,
    LayoutActions, LayoutFrame, LayoutHost, LayoutMetrics, LayoutRenderer, OnChange, OnChangeIn,
    OpenPanels, PanelDetail, Slot, UiIntent, WindowKey,
};
use crate::panel::TerminalPanel;
use crate::review::ReviewPanel;
use crate::source::PoddSource;

use super::Workspace;
use super::sidebar::Sidebar;

/// Pipe name of the `--ui-fixture` source (`source/fixture.rs`).
const FIXTURE_PIPE: &str = "ui-fixture-no-daemon";

impl Workspace {
    /// Read-only projection of the window for one render. `gates` are the
    /// pending gates (`OrchState::pending`), kept alive by the caller.
    pub(crate) fn layout_frame<'a>(&'a self, gates: &'a [GateInfo], cx: &App) -> LayoutFrame<'a> {
        LayoutFrame {
            tasks: &self.tasks,
            repos: &self.repos,
            terminals: &self.terminals,
            conn: &self.conn,
            gates,
            focused_terminal: self.focused_terminal.as_deref(),
            panels: self.open_panels(cx),
            panel_detail: self.panel_detail(cx),
            metrics: LayoutMetrics::current(cx),
            labels: &self.saved.labels,
            all_gates: &self.orch.gates,
            toasts: FrameToasts(&self.toasts),
            tree_selection: self.sidebar.read(cx).selected.clone(),
            fixture: self.source.pipe() == FIXTURE_PIPE,
        }
    }

    /// The panels the workbench's "nothing open" hint checks, minus the
    /// app-level pages (each mode checks those itself).
    fn open_panels(&self, cx: &App) -> OpenPanels {
        OpenPanels {
            remote_terminals: self
                .machines
                .read(cx)
                .panels
                .iter()
                .filter(|p| p.upgrade().is_some())
                .count(),
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

    /// The orchestration panels one by one and the app-level pages.
    fn panel_detail(&self, cx: &App) -> PanelDetail {
        PanelDetail {
            groups: self.orch.live_groups().len(),
            gates_panel: self
                .orch
                .gates_panel
                .as_ref()
                .is_some_and(|p| p.upgrade().is_some()),
            tree_panel: self
                .orch
                .tree_panel
                .as_ref()
                .is_some_and(|p| p.upgrade().is_some()),
            mods_page: crate::mods_ui::page_open(cx),
            sessions_page: crate::sessions_ui::page_open(cx),
            orch_page: crate::orch_ui::page_open(cx),
        }
    }

    /// Handle for a mode's event closures and long-lived views.
    pub(crate) fn layout_actions(&self, cx: &Context<Self>) -> LayoutActions {
        // The dock, the sidebar, both focus handles and the source are set
        // once in Workspace::new and never replaced: capture them so a
        // renderer can use them while the workspace itself is leased (inside
        // Workspace::render) — the kit's render-safe half.
        LayoutActions::new(Rc::new(Sink {
            ws: cx.entity().downgrade(),
            dock: self.dock.clone(),
            sidebar: self.sidebar.clone(),
            sidebar_focus: self.sidebar_focus_handle(cx),
            toast_focus: self.toast_focus.clone(),
            source: self.source.clone(),
        }))
    }

    /// The body through the mode's narrow renderer (the shell's only body).
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

    /// Title-bar extra of the layout in settings (the title bar in chrome),
    /// through the mode's renderer.
    pub(crate) fn layout_title_bar_extra(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let renderer = crate::layout_mode::renderer(cx);
        let actions = self.layout_actions(cx);
        let gates = self.orch.pending();
        let frame = self.layout_frame(&gates, cx);
        renderer.title_bar_extra(&frame, &actions, cx)
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
            UiIntent::NewTask => self.open_new_task_dialog(window, cx),
            UiIntent::OpenRepository => self.open_repository(window, cx),
            UiIntent::OpenGates => self.open_gates(window, cx),
            UiIntent::OpenAgentTree => self.open_agent_tree(window, cx),
            UiIntent::OpenSettings => self.open_settings(window, cx),
            UiIntent::OpenNotifications => self.open_notifications(window, cx),
            UiIntent::OpenJump => self.open_jump(window, cx),
            UiIntent::Toast { kind, title, body } => self.raise_toast(kind, title, body, cx),
            UiIntent::RevealTerminal { id } => self.reveal_terminal(&id, window, cx),
            UiIntent::OpenTaskTerminals { task } => self.open_task_terminals(task, window, cx),
            // As from the keyboard: along the focus path to the root's handler.
            UiIntent::OpenFocusedReview => window.dispatch_action(Box::new(super::OpenReview), cx),
        }
    }

    /// Selects the first panel of terminal `id` in the dock; focus stays.
    fn reveal_terminal(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self
            .live_panels()
            .into_iter()
            .find(|p| p.read(cx).terminal_id == id)
        {
            let pid = PanelId::from(p.entity_id());
            self.dock
                .update(cx, |a, cx| a.select_panel(pid, window, cx));
        }
    }

    /// The task's running terminals in reverse task order (the first ends in
    /// front), or a shell in its worktree when none runs.
    fn open_task_terminals(&mut self, task: String, window: &mut Window, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .task(&task)
            .map(|t| {
                t.terminals
                    .iter()
                    .filter(|id| self.terminal(id).is_some_and(|x| x.is_running()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            self.task_terminal(task, window, cx);
        }
        for id in ids.iter().rev() {
            self.open_terminal(id, false, window, cx);
        }
    }

    /// One window-less update, in the caller's order.
    fn apply_host_update(&mut self, update: HostUpdate, cx: &mut Context<Self>) {
        match update {
            HostUpdate::Notify => cx.notify(),
            HostUpdate::Toast { kind, title, body } => self.raise_toast(kind, title, body, cx),
            HostUpdate::ApplyGate(g) => {
                self.orch.apply(g);
            }
            HostUpdate::AddTerminal(t) => {
                if !self.terminals.iter().any(|x| x.id == t.id) {
                    self.terminals.push(t);
                }
            }
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
/// gone there is no frame and intents do nothing. The dock, the sidebar, the
/// sidebar and toast focus handles and the source are captured at creation
/// (never reassigned): the render-safe half ([`HostHandles`]) uses only them
/// and `upgrade`, so it works during render, while the workspace is leased.
struct Sink {
    ws: WeakEntity<Workspace>,
    dock: Entity<DockArea>,
    sidebar: Entity<Sidebar>,
    sidebar_focus: FocusHandle,
    toast_focus: FocusHandle,
    source: PoddSource,
}

impl HostHandles for Sink {
    fn window_key(&self) -> WindowKey {
        WindowKey(self.ws.entity_id())
    }

    fn focus_handle(&self, slot: Slot, cx: &App) -> Option<FocusHandle> {
        match slot {
            Slot::Dock => Some(self.dock.read(cx).focus_handle(cx)),
            Slot::Sidebar => Some(self.sidebar_focus.clone()),
            Slot::Toasts => {
                self.ws.upgrade()?;
                Some(self.toast_focus.clone())
            }
            Slot::HookBanner => None,
        }
    }

    fn on_release(&self, cx: &mut App, f: AppOnce) -> bool {
        let Some(ws) = self.ws.upgrade() else {
            return false;
        };
        cx.observe_release(&ws, move |_, cx| f(cx)).detach();
        true
    }

    fn observe(&self, change: HostChange, cx: &mut App, mut f: OnChange) -> Option<Subscription> {
        let ws = self.ws.upgrade()?;
        Some(match change {
            HostChange::Data => cx.observe(&ws, move |_, cx| f(cx)),
            HostChange::DockLayout => cx.subscribe(&self.dock, move |_, ev: &DockEvent, cx| {
                if let DockEvent::LayoutChanged = ev {
                    f(cx)
                }
            }),
            HostChange::Tree => cx.observe(&self.sidebar, move |_, cx| f(cx)),
        })
    }

    fn observe_in(
        &self,
        change: HostChange,
        window: &mut Window,
        cx: &mut App,
        mut f: OnChangeIn,
    ) -> Option<Subscription> {
        let ws = self.ws.upgrade()?;
        Some(match change {
            HostChange::Data => window.observe(&ws, cx, move |_, window, cx| f(window, cx)),
            HostChange::DockLayout => {
                window.subscribe(&self.dock, cx, move |_, ev: &DockEvent, window, cx| {
                    if let DockEvent::LayoutChanged = ev {
                        f(window, cx)
                    }
                })
            }
            HostChange::Tree => {
                window.observe(&self.sidebar, cx, move |_, window, cx| f(window, cx))
            }
        })
    }

    fn dock(&self, _cx: &App) -> Option<Entity<DockArea>> {
        self.ws.upgrade()?;
        Some(self.dock.clone())
    }

    fn view(&self, slot: Slot, _cx: &App) -> Option<AnyView> {
        self.ws.upgrade()?;
        match slot {
            Slot::Dock => Some(self.dock.clone().into()),
            Slot::Sidebar => Some(self.sidebar.clone().into()),
            Slot::Toasts | Slot::HookBanner => None,
        }
    }

    fn source(&self, _cx: &App) -> Option<PoddSource> {
        self.ws.upgrade()?;
        Some(self.source.clone())
    }
}

impl HostCommands for Sink {
    fn with_frame(&self, cx: &App, f: &mut dyn FnMut(&LayoutFrame<'_>)) {
        let Some(ws) = self.ws.upgrade() else {
            return;
        };
        let ws = ws.read(cx);
        let gates = ws.orch.pending();
        f(&ws.layout_frame(&gates, cx));
    }

    fn dispatch(&self, intent: UiIntent, window: &mut Window, cx: &mut App) {
        let _ = self
            .ws
            .update(cx, |ws, cx| ws.dispatch_intent(intent, window, cx));
    }

    fn current_task(&self, window: &Window, cx: &App) -> Option<String> {
        self.ws.upgrade()?.read(cx).current_task(window, cx)
    }

    fn terminal_panels(&self, cx: &App) -> Vec<Entity<TerminalPanel>> {
        self.ws
            .upgrade()
            .map(|ws| ws.read(cx).live_panels())
            .unwrap_or_default()
    }

    fn review_panels(&self, cx: &App) -> Vec<Entity<ReviewPanel>> {
        self.ws
            .upgrade()
            .map(|ws| ws.read(cx).live_reviews())
            .unwrap_or_default()
    }

    fn new_terminal_panel(
        &self,
        id: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<TerminalPanel>> {
        let ws = self.ws.upgrade()?;
        let (source, panels, label) = {
            let w = ws.read(cx);
            w.terminal(id)?;
            let label: SharedString = w
                .labels
                .borrow()
                .get(id)
                .cloned()
                .unwrap_or_else(|| id.to_string())
                .into();
            (w.source.clone(), w.panels.clone(), label)
        };
        let weak = self.ws.clone();
        let panel =
            cx.new(|cx| TerminalPanel::new(source, weak, id.to_string(), label, window, cx));
        // Registered like dock panels: labels, theme changes and "terminal in view" reach it.
        panels.borrow_mut().push(panel.downgrade());
        Some(panel)
    }

    fn new_review_panel(
        &self,
        task: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<ReviewPanel>> {
        let ws = self.ws.upgrade()?;
        let (source, k) = {
            let w = ws.read(cx);
            (
                w.source.clone(),
                w.task(task).filter(|_| w.caps.review).cloned(),
            )
        };
        let k = k?;
        let weak = self.ws.clone();
        let panel = cx.new(|cx| ReviewPanel::new(source, weak, &k, window, cx));
        // Registered like dock reviews: task, status and delivery events reach it.
        ws.update(cx, |w, _| w.reviews.push(panel.downgrade()));
        Some(panel)
    }

    fn post(&self, updates: Vec<HostUpdate>, cx: &mut App) -> bool {
        self.ws
            .update(cx, |ws, cx| {
                for u in updates {
                    ws.apply_host_update(u, cx);
                }
            })
            .is_ok()
    }

    fn resolve_gate(&self, params: GateResolveParams, cx: &mut App, done: GateDone) -> bool {
        if self.ws.upgrade().is_none() {
            return false;
        }
        crate::gates::resolve_gate(&self.source, self.ws.clone(), params, cx, done);
        true
    }
}

#[cfg(test)]
mod tests {
    //! The A-T2a host additions on a real workspace (native-QA harness): what
    //! each read, view, intent and update reaches is the workspace's own.
    //!
    //! Not `gpui_kit::*`: gpui's own `test` attribute would shadow the std one.
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui_kit::{
        AnyWindowHandle, App, AppContext as _, Entity, Subscription, WeakEntity, Window,
        WindowOptions,
    };
    use podd::protocol::{GateDecision, GateInfo, GateResolve, GateResolveParams, orch_codes};

    use super::{Sink, Workspace};
    use crate::layouts::kit::{
        self, HostChange, HostUpdate, LayoutActions, Slot, UiIntent, WindowKey,
    };
    use crate::native_qa::{Harness, seed_two_tasks};
    use crate::panel::TerminalPanel;
    use crate::source::{PoddSource, SourceError, TestBackend};
    use crate::workspace::{StartOptions, ToastKind};

    const TASK: &str = "sample/로그인-수정";

    fn actions(h: &mut Harness) -> LayoutActions {
        let ws = h.ws.clone();
        h.cx.update(|_, cx| ws.update(cx, |w, cx| w.layout_actions(cx)))
    }

    fn dock_json(h: &mut Harness) -> String {
        let ws = h.ws.clone();
        h.cx.update(|_, cx| {
            serde_json::to_string(&ws.read(cx).dock.read(cx).dump(cx)).expect("dock state")
        })
    }

    /// Opens `ids` in order through the workspace (the last one keeps focus).
    fn open(h: &mut Harness, ids: &[&str]) {
        let ws = h.ws.clone();
        h.cx.update(|window, cx| {
            ws.update(cx, |w, cx| {
                for id in ids {
                    w.open_terminal(id, false, window, cx);
                }
            })
        });
        h.settle();
    }

    fn params(id: &str) -> GateResolveParams {
        GateResolveParams {
            id: id.into(),
            decision: GateDecision::Approve,
            note: None,
            choice: None,
        }
    }

    /// A view state kept in a slot or observing the host.
    #[derive(Default)]
    struct Probe {
        seen: Vec<HostChange>,
        sub: Option<Subscription>,
    }

    fn fixture_gate() -> GateInfo {
        crate::orch_ui::fixture::gates_for_test()
            .into_iter()
            .next()
            .expect("a fixture gate")
    }

    /// A second window with its own workspace and scripted backend (served
    /// only when the test calls it), and the actions of that workspace.
    fn second_window(
        h: &mut Harness,
    ) -> (
        AnyWindowHandle,
        WeakEntity<Workspace>,
        LayoutActions,
        TestBackend,
    ) {
        let (source, events, backend) = PoddSource::scripted();
        let (handle, weak_ws) = h.cx.update(|_, cx| {
            let slot = RefCell::new(None);
            let handle = cx
                .open_window(WindowOptions::default(), |window, cx| {
                    let ws = cx.new(|cx| {
                        Workspace::new(source, events, StartOptions::default(), window, cx)
                    });
                    *slot.borrow_mut() = Some(ws.downgrade());
                    cx.new(|cx| gpui_kit::base::Root::new(ws, window, cx))
                })
                .expect("second window");
            (handle, slot.into_inner().expect("second workspace"))
        });
        let actions = h.cx.update(|_, cx| {
            weak_ws
                .upgrade()
                .expect("second workspace")
                .update(cx, |w, cx| w.layout_actions(cx))
        });
        (handle.into(), weak_ws, actions, backend)
    }

    /// Where a layout sees the focus from its actions: the number of
    /// terminal panels, the terminal whose dock panel holds the window focus
    /// and the frame's focused terminal.
    fn focus_seen(a: &LayoutActions, window: &Window, cx: &App) -> String {
        let panels = a.terminal_panels(cx);
        let window_focus = a
            .focused_dock_panel(window, cx)
            .and_then(|(id, _)| panels.iter().find(|p| p.entity_id() == id))
            .map(|p| p.read(cx).terminal_id.clone());
        let frame_focus = a
            .read(cx, |f| f.focused_terminal.map(str::to_owned))
            .flatten();
        format!(
            "panels={} window={} frame={}",
            panels.len(),
            window_focus.as_deref().unwrap_or("-"),
            frame_focus.as_deref().unwrap_or("-")
        )
    }

    /// What a layout reads about gate answers: the gates it knows, the toasts.
    fn gate_seen(a: &LayoutActions, cx: &App) -> String {
        a.read(cx, |f| {
            let ids: Vec<&str> = f.all_gates.keys().map(String::as_str).collect();
            format!("gates=[{}] toasts={}", ids.join(","), f.toasts.len())
        })
        .unwrap_or_else(|| "gone".into())
    }

    #[test]
    fn the_frame_lends_labels_gates_toasts_tree_and_theme() {
        let mut h = Harness::new("layout-host-frame-reads", seed_two_tasks);
        let ws = h.ws.clone();
        let gate = crate::orch_ui::fixture::gates_for_test()
            .into_iter()
            .next()
            .expect("a fixture gate");
        h.cx.update(|window, cx| {
            ws.update(cx, |w, cx| {
                let w1 = w.terminal("w1").cloned().expect("w1");
                w.saved.labels.insert(
                    crate::layout::label_key(&w1.id, w1.created_ms),
                    "저장한 이름".into(),
                );
                w.raise_toast(ToastKind::Info, "제목", "본문", cx);
                w.orch.gates.insert(gate.id.clone(), gate.clone());
                w.open_gates(window, cx);
                w.open_agent_tree(window, cx);
            });
        });
        h.settle();
        h.cx.update(|_, cx| {
            let sidebar = ws.read(cx).sidebar.clone();
            sidebar.update(cx, |s, _| s.selected = Some("terminal:w1".into()));
            let w = ws.read(cx);
            let pending = w.orch.pending();
            let f = w.layout_frame(&pending, cx);
            for t in &w.terminals {
                assert_eq!(f.label_of(t), w.label_of(t), "{}", t.id);
            }
            assert_eq!(f.label_of(w.terminal("w1").expect("w1")), "저장한 이름");
            assert!(std::ptr::eq(f.labels, &w.saved.labels), "labels are lent");
            assert!(
                std::ptr::eq(f.all_gates, &w.orch.gates),
                "every gate is lent"
            );
            assert_eq!(f.all_gates.get(&gate.id), Some(&gate));
            let ids = |t: &[crate::workspace::Toast]| t.iter().map(|t| t.id).collect::<Vec<_>>();
            assert!(std::ptr::eq(f.toasts.0, &w.toasts[..]), "toasts are lent");
            assert_eq!(ids(&f.toasts), ids(&w.toasts));
            assert_eq!(f.toasts.last().map(|t| t.title.as_ref()), Some("제목"));
            assert_eq!(f.tree_selection.as_deref(), Some("terminal:w1"));
            assert!(
                !f.fixture,
                "the scripted source is not the --ui-fixture one"
            );
            // The review layout's dock signature, as `layouts/review.rs` counts it.
            let live = |p: &Option<WeakEntity<crate::chrome::ChromePanel>>| {
                p.as_ref().is_some_and(|p| p.upgrade().is_some()) as usize
            };
            let gates_panel = w
                .orch
                .gates_panel
                .as_ref()
                .is_some_and(|p| p.upgrade().is_some());
            let tree_panel = w
                .orch
                .tree_panel
                .as_ref()
                .is_some_and(|p| p.upgrade().is_some());
            let signature = live(&w.settings_panel)
                + live(&w.inspector_panel)
                + w.live_editors().len()
                + w.orch.live_groups().len()
                + gates_panel as usize
                + tree_panel as usize
                + crate::mods_ui::page_open(cx) as usize
                + crate::sessions_ui::page_open(cx) as usize;
            assert!(gates_panel && tree_panel, "both panels opened");
            assert_eq!(f.other_panels(), signature);
            assert_eq!(f.other_panels(), 2);
            let d = f.panel_detail;
            assert_eq!(d.groups, w.orch.live_groups().len());
            assert!(d.gates_panel && d.tree_panel);
            assert_eq!(d.mods_page, crate::mods_ui::page_open(cx));
            assert_eq!(d.sessions_page, crate::sessions_ui::page_open(cx));
            assert_eq!(d.orch_page, crate::orch_ui::page_open(cx));
            assert!(f.panels.orch, "the folded flag agrees");
        });
    }

    #[test]
    fn actions_reach_the_windows_dock_panels_and_source() {
        let mut h = Harness::new("layout-host-dock-access", seed_two_tasks);
        open(&mut h, &["w1", "b1"]);
        let a = actions(&mut h);
        let ws = h.ws.clone();
        h.cx.update(|window, cx| {
            let w = ws.read(cx);
            assert_eq!(a.window_key(), WindowKey(ws.entity_id()));
            assert_eq!(a.dock(cx).map(|d| d.entity_id()), Some(w.dock.entity_id()));
            assert_eq!(
                a.view(Slot::Dock, cx).map(|v| v.entity_id()),
                Some(w.dock.entity_id())
            );
            assert_eq!(
                a.view(Slot::Sidebar, cx).map(|v| v.entity_id()),
                Some(w.sidebar.entity_id())
            );
            assert!(a.view(Slot::Toasts, cx).is_none());
            assert!(a.view(Slot::HookBanner, cx).is_none());
            let ids =
                |v: Vec<Entity<TerminalPanel>>| v.iter().map(|p| p.entity_id()).collect::<Vec<_>>();
            assert_eq!(ids(a.terminal_panels(cx)), ids(w.live_panels()));
            assert_eq!(a.terminal_panels(cx).len(), 2);
            let b1 = a.terminal_panel("b1", cx).expect("b1's panel");
            assert_eq!(b1.read(cx).terminal_id, "b1");
            assert!(a.terminal_panel("nope", cx).is_none());
            assert!(a.review_panels(cx).is_empty());
            assert_eq!(
                a.source(cx).map(|s| s.pipe().to_owned()),
                Some(w.source.pipe().to_owned())
            );
            assert_eq!(a.current_task(window, cx), w.current_task(window, cx));
            assert_eq!(
                a.current_task(window, cx).as_deref(),
                Some("sample/review-bot")
            );
        });
    }

    /// A renderer runs inside `Workspace::render`, where the workspace is
    /// leased: what only needs the dock, the sidebar or the source must not
    /// read the workspace (a double lease panics in GPUI).
    #[test]
    fn dock_sidebar_source_and_tree_work_while_the_workspace_is_leased() {
        let mut h = Harness::new("layout-host-leased", seed_two_tasks);
        let a = actions(&mut h);
        let ws = h.ws.clone();
        let (dock, sidebar, pipe) = h.cx.update(|_, cx| {
            let w = ws.read(cx);
            (
                w.dock.entity_id(),
                w.sidebar.entity_id(),
                w.source.pipe().to_owned(),
            )
        });
        let fired = Rc::new(Cell::new(0));
        let subs = h.cx.update(|window, cx| {
            ws.update(cx, |_, cx| {
                assert_eq!(a.dock(cx).map(|d| d.entity_id()), Some(dock));
                assert_eq!(a.view(Slot::Dock, cx).map(|v| v.entity_id()), Some(dock));
                assert_eq!(
                    a.view(Slot::Sidebar, cx).map(|v| v.entity_id()),
                    Some(sidebar)
                );
                assert!(a.focus_handle(Slot::Dock, cx).is_some());
                assert!(a.focus_handle(Slot::Sidebar, cx).is_some());
                assert_eq!(a.source(cx).map(|s| s.pipe().to_owned()), Some(pipe));
                let f = fired.clone();
                let tree = a.observe(&[HostChange::Tree], cx, move |_, _, _| f.set(f.get() + 1));
                let dock_layout =
                    a.observe_in(&[HostChange::DockLayout], window, cx, |_, _, _, _| {});
                (tree, dock_layout)
            })
        });
        assert!(subs.0.is_some() && subs.1.is_some());
        h.cx.update(|_, cx| {
            let sidebar = ws.read(cx).sidebar.clone();
            sidebar.update(cx, |_, cx| cx.notify());
        });
        h.settle();
        assert_eq!(fired.get(), 1, "the tree observer fires after the lease");
        drop(subs);
    }

    #[test]
    fn reveal_terminal_brings_its_panel_forward_and_keeps_focus() {
        let mut h = Harness::new("layout-host-reveal", seed_two_tasks);
        open(&mut h, &["w1", "b1"]);
        let a = actions(&mut h);
        let shown_b1 = dock_json(&mut h);
        let side = h.sidebar_focus();
        h.cx.update(|window, cx| window.focus(&side, cx));
        h.settle();
        let reveal = |h: &mut Harness, id: &str| {
            let a = a.clone();
            let intent = UiIntent::RevealTerminal { id: id.into() };
            h.cx.update(|window, cx| a.dispatch(intent, window, cx));
            h.settle();
        };
        reveal(&mut h, "w1");
        let shown_w1 = dock_json(&mut h);
        assert_ne!(shown_w1, shown_b1, "w1's tab is selected");
        assert_eq!(h.focused(), Some(side.clone()), "focus stays in the tree");
        reveal(&mut h, "nope");
        assert_eq!(dock_json(&mut h), shown_w1, "no panel, nothing changes");
        reveal(&mut h, "b1");
        assert_eq!(dock_json(&mut h), shown_b1, "back to b1's tab");
        assert_eq!(h.focused(), Some(side));
    }

    #[test]
    fn open_task_terminals_opens_them_in_one_update_then_calls_back() {
        let mut h = Harness::new("layout-host-task-terminals", |b| {
            seed_two_tasks(b);
            b.tasks
                .push(TestBackend::task("sample/empty", "empty", "sample", &[]));
        });
        let a = actions(&mut h);
        let seen = Rc::new(RefCell::new(Vec::<Vec<String>>::new()));
        let open_then = |h: &mut Harness, task: &str| {
            let (a2, seen) = (a.clone(), seen.clone());
            let intent = UiIntent::OpenTaskTerminals { task: task.into() };
            let a = a.clone();
            h.cx.update(|window, cx| {
                a.dispatch_then(intent, window, cx, move |_, cx| {
                    let ids = a2
                        .terminal_panels(cx)
                        .iter()
                        .map(|p| p.read(cx).terminal_id.clone())
                        .collect();
                    seen.borrow_mut().push(ids);
                })
            });
            h.settle();
        };
        open_then(&mut h, TASK);
        assert_eq!(*seen.borrow(), [vec!["w1".to_string()]], "after the update");
        assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
        assert!(
            !h.backend
                .calls
                .iter()
                .any(|c| c.starts_with("terminal.run"))
        );
        // No running terminal: a shell in the task's worktree.
        open_then(&mut h, "sample/empty");
        assert_eq!(seen.borrow().len(), 2);
        assert!(
            h.backend
                .calls
                .iter()
                .any(|c| c.starts_with("terminal.run")),
            "{:?}",
            h.backend.calls
        );
    }

    #[test]
    fn open_focused_review_is_the_review_command() {
        let mut h = Harness::new("layout-host-focused-review", seed_two_tasks);
        open(&mut h, &["w1"]);
        let a = actions(&mut h);
        h.cx.update(|window, cx| a.dispatch(UiIntent::OpenFocusedReview, window, cx));
        h.settle();
        let ws = h.ws.clone();
        let tasks: Vec<String> = h.cx.update(|_, cx| {
            let w = ws.read(cx);
            w.live_reviews()
                .iter()
                .map(|r| r.read(cx).task_id.clone())
                .collect()
        });
        assert_eq!(tasks, [TASK.to_string()], "w1's task, as Ctrl+Shift+R does");
    }

    #[test]
    fn post_applies_window_less_updates_in_order() {
        let mut h = Harness::new("layout-host-post", seed_two_tasks);
        let a = actions(&mut h);
        let ws = h.ws.clone();
        let gate = crate::orch_ui::fixture::gates_for_test()
            .into_iter()
            .next()
            .expect("a fixture gate");
        let new = TestBackend::terminal("t9", "다시 실행", Some(TASK), "idle");
        let mut twin = TestBackend::terminal("w1", "다른 이름", Some(TASK), "idle");
        twin.created_ms += 1;
        let toasts = h.cx.update(|_, cx| ws.read(cx).toasts.len());
        let updates = vec![
            HostUpdate::Toast {
                kind: ToastKind::Error,
                title: "실패".into(),
                body: "이유".into(),
            },
            HostUpdate::AddTerminal(new.clone()),
            HostUpdate::AddTerminal(twin),
            HostUpdate::ApplyGate(gate.clone()),
            HostUpdate::Notify,
        ];
        h.cx.update(|_, cx| {
            assert!(a.post(updates, cx));
            let w = ws.read(cx);
            assert_eq!(w.toasts.len(), toasts + 1);
            assert_eq!(w.toasts.last().map(|t| t.title.as_ref()), Some("실패"));
            assert_eq!(w.terminals.iter().filter(|t| t.id == "t9").count(), 1);
            let w1: Vec<_> = w.terminals.iter().filter(|t| t.id == "w1").collect();
            assert_eq!(w1.len(), 1, "a known id is not added twice");
            assert_eq!(w1[0].name.as_deref(), Some("로그인 작업"));
            assert_eq!(w.orch.gates.get(&gate.id), Some(&gate));
        });
    }

    #[test]
    fn resolve_gate_merges_the_answer_or_toasts_the_failure() {
        let mut h = Harness::new("layout-host-gate", seed_two_tasks);
        let a = actions(&mut h);
        let ws = h.ws.clone();
        let gate = crate::orch_ui::fixture::gates_for_test()
            .into_iter()
            .next()
            .expect("a fixture gate");
        let answer = serde_json::to_value(&gate).expect("gate json");
        let fail = Rc::new(Cell::new(false));
        let failing = fail.clone();
        h.backend.rpc = Some(Box::new(move |method, _| {
            (method == "gate.resolve").then(|| {
                if failing.get() {
                    Err(SourceError {
                        code: Some(orch_codes::GATE_RESOLVED),
                        message: "already decided".into(),
                    })
                } else {
                    Ok(answer.clone())
                }
            })
        }));
        let results = Rc::new(RefCell::new(Vec::new()));
        let decide = |h: &mut Harness| {
            let (a, results, p) = (a.clone(), results.clone(), params(&gate.id));
            h.cx.update(|_, cx| {
                assert!(a.resolve_gate(p, cx, move |r, _| {
                    results
                        .borrow_mut()
                        .push(r.map_err(|e| (e.text, e.already_resolved)))
                }));
            });
            h.settle();
        };
        decide(&mut h);
        {
            let r = results.borrow();
            assert!(matches!(r.as_slice(), [Ok(())]), "{r:?}");
        }
        assert!(
            h.backend
                .calls
                .iter()
                .any(|c| c.starts_with("gate.resolve")),
            "one gate.resolve"
        );
        let (merged, toasts) = h.cx.update(|_, cx| {
            (
                ws.read(cx).orch.gates.get(&gate.id).cloned(),
                ws.read(cx).toasts.len(),
            )
        });
        assert_eq!(merged, Some(gate.clone()));
        fail.set(true);
        decide(&mut h);
        {
            let r = results.borrow();
            match r.as_slice() {
                [Ok(()), Err((text, already))] => assert!(*already, "{text}"),
                other => panic!("{other:?}"),
            }
        }
        h.cx.update(|_, cx| {
            let w = ws.read(cx);
            assert_eq!(w.toasts.len(), toasts + 1, "the failure is toasted");
            let t = w.toasts.last().expect("toast");
            assert_eq!(t.kind, ToastKind::Error);
            assert!(t.title.contains(&gate.id), "{}", t.title);
        });
    }

    #[test]
    fn observe_reports_data_dock_and_tree_changes_until_dropped() {
        let mut h = Harness::new("layout-host-observe", seed_two_tasks);
        let a = actions(&mut h);
        let ws = h.ws.clone();
        let all = [HostChange::Data, HostChange::DockLayout, HostChange::Tree];
        let (with_window, without) = h.cx.update(|window, cx| {
            let a2 = a.clone();
            let p1 = cx.new(|cx| Probe {
                sub: a.observe_in(&all, window, cx, |p: &mut Probe, c, _, _| p.seen.push(c)),
                ..Default::default()
            });
            let p2 = cx.new(|cx| Probe {
                sub: a2.observe(&[HostChange::Data], cx, |p: &mut Probe, c, _| {
                    p.seen.push(c)
                }),
                ..Default::default()
            });
            (p1, p2)
        });
        let seen = |h: &mut Harness, p: &Entity<Probe>| {
            let p = p.clone();
            h.cx.update(|_, cx| p.update(cx, |p, _| std::mem::take(&mut p.seen)))
        };
        h.settle();
        seen(&mut h, &with_window);
        seen(&mut h, &without);
        h.cx.update(|_, cx| ws.update(cx, |_, cx| cx.notify()));
        h.settle();
        assert_eq!(seen(&mut h, &with_window), [HostChange::Data]);
        assert_eq!(seen(&mut h, &without), [HostChange::Data]);
        h.cx.update(|_, cx| {
            let sidebar = ws.read(cx).sidebar.clone();
            sidebar.update(cx, |_, cx| cx.notify());
        });
        h.settle();
        assert_eq!(seen(&mut h, &with_window), [HostChange::Tree]);
        assert!(seen(&mut h, &without).is_empty(), "Data only");
        open(&mut h, &["w1"]);
        assert!(seen(&mut h, &with_window).contains(&HostChange::DockLayout));
        // Dropping the one subscription ends all three.
        let p = with_window.clone();
        h.cx.update(|_, cx| p.update(cx, |p, _| p.sub = None));
        open(&mut h, &["b1"]);
        h.cx.update(|_, cx| ws.update(cx, |_, cx| cx.notify()));
        h.settle();
        assert!(seen(&mut h, &with_window).is_empty());
    }

    #[test]
    fn state_slots_are_per_window_and_go_with_the_host() {
        let mut h = Harness::new("layout-host-state", seed_two_tasks);
        let a = actions(&mut h);
        let made = Rc::new(Cell::new(0));
        let probe = |made: &Rc<Cell<i32>>| {
            let made = made.clone();
            move |_: &mut gpui_kit::Window, cx: &mut gpui_kit::App| {
                made.set(made.get() + 1);
                cx.new(|_| Probe::default())
            }
        };
        let first = h.cx.update(|window, cx| {
            let s1 = a.state_or_insert_with(window, cx, probe(&made));
            let s2 = a.state_or_insert_with(window, cx, probe(&made));
            assert_eq!(s1.entity_id(), s2.entity_id(), "made once");
            assert_eq!(made.get(), 1);
            assert_eq!(
                a.state::<Probe>(cx).map(|s| s.entity_id()),
                Some(s1.entity_id())
            );
            assert_eq!(
                kit::state_in_window::<Probe>(window.window_handle(), cx).map(|s| s.entity_id()),
                Some(s1.entity_id())
            );
            assert!(a.state::<String>(cx).is_none(), "one slot per type");
            s1.downgrade()
        });
        // A second window with its own workspace: its own slot, dropped with it.
        let (handle, weak_ws, b, _backend) = second_window(&mut h);
        let key = b.window_key();
        let second = h.cx.update(|_, cx| {
            handle
                .update(cx, |_, window, cx| {
                    b.state_or_insert_with(window, cx, probe(&made))
                })
                .expect("window")
                .downgrade()
        });
        assert_eq!(made.get(), 2);
        assert_ne!(second.entity_id(), first.entity_id(), "per window");
        h.cx.update(|_, cx| {
            assert_eq!(kit::states::<Probe>(cx).len(), 2);
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
        h.settle();
        h.cx.update(|_, cx| {
            assert!(weak_ws.upgrade().is_none(), "the second workspace is gone");
            assert!(
                kit::state::<Probe>(key, cx).is_none(),
                "its slots are dropped"
            );
            assert!(second.upgrade().is_none(), "and released");
            assert_eq!(
                a.state::<Probe>(cx).map(|s| s.entity_id()),
                Some(first.entity_id()),
                "the first window keeps its own"
            );
            kit::release_window(a.window_key(), cx);
            assert!(a.state::<Probe>(cx).is_none(), "explicit release");
        });
    }

    #[test]
    fn a_gone_host_reads_nothing_and_takes_nothing() {
        let mut h = Harness::new("layout-host-gone", seed_two_tasks);
        // Real handles, but the workspace itself is gone.
        let ws = h.ws.clone();
        let gone = h.cx.update(|_, cx| {
            let w = ws.read(cx);
            LayoutActions::new(Rc::new(Sink {
                ws: WeakEntity::new_invalid(),
                dock: w.dock.clone(),
                sidebar: w.sidebar.clone(),
                sidebar_focus: w.sidebar_focus_handle(cx),
                toast_focus: w.toast_focus.clone(),
                source: w.source.clone(),
            }))
        });
        h.cx.update(|window, cx| {
            assert!(gone.read(cx, |_| ()).is_none());
            assert!(gone.focus_handle(Slot::Toasts, cx).is_none());
            assert!(gone.dock(cx).is_none());
            assert!(gone.view(Slot::Dock, cx).is_none());
            assert!(gone.source(cx).is_none());
            assert!(gone.terminal_panels(cx).is_empty());
            assert!(gone.review_panels(cx).is_empty());
            assert!(gone.current_task(window, cx).is_none());
            assert!(!gone.post(vec![HostUpdate::Notify], cx));
            let called = Rc::new(Cell::new(false));
            let c = called.clone();
            assert!(!gone.resolve_gate(params("g1"), cx, move |_, _| c.set(true)));
            assert!(!called.get());
            assert!(gone.new_terminal_panel("w1", window, cx).is_none());
            assert!(gone.new_review_panel(TASK, window, cx).is_none());
            let _made = gone.state_or_insert_with(window, cx, |_, cx| cx.new(|_| Probe::default()));
            assert!(
                gone.state::<Probe>(cx).is_none(),
                "a gone host keeps no state"
            );
            let p = cx.new(|cx| Probe {
                sub: gone.observe(&[HostChange::Data], cx, |_: &mut Probe, _, _| {}),
                ..Default::default()
            });
            assert!(p.read(cx).sub.is_none());
        });
        h.settle();
        assert!(
            !h.backend
                .calls
                .iter()
                .any(|c| c.starts_with("gate.resolve")),
            "nothing sent"
        );
    }

    #[test]
    fn new_panels_are_registered_but_stay_out_of_the_dock() {
        let mut h = Harness::new("layout-host-new-panels", seed_two_tasks);
        let a = actions(&mut h);
        let ws = h.ws.clone();
        let before = dock_json(&mut h);
        h.cx.update(|window, cx| {
            let p = a.new_terminal_panel("w1", window, cx).expect("w1's panel");
            assert_eq!(p.read(cx).terminal_id, "w1");
            assert!(
                ws.read(cx)
                    .live_panels()
                    .iter()
                    .any(|x| x.entity_id() == p.entity_id()),
                "registered like dock panels"
            );
            assert!(
                a.new_terminal_panel("nope", window, cx).is_none(),
                "unknown id"
            );
            let r = a
                .new_review_panel("sample/review-bot", window, cx)
                .expect("a review");
            assert_eq!(r.read(cx).task_id, "sample/review-bot");
            assert!(
                ws.read(cx)
                    .live_reviews()
                    .iter()
                    .any(|x| x.entity_id() == r.entity_id()),
                "registered like dock reviews"
            );
            assert!(a.new_review_panel("nope", window, cx).is_none());
            ws.update(cx, |w, _| w.caps.review = false);
            assert!(
                a.new_review_panel("sample/review-bot", window, cx)
                    .is_none(),
                "no review capability, no panel"
            );
        });
        h.settle();
        assert_eq!(dock_json(&mut h), before, "neither went into the dock");
    }

    /// rf2-kit: every render-safe call (the kit's `HostHandles` half and the
    /// `LayoutActions` methods built on it) works while the workspace is
    /// leased, as inside a renderer's `render_body`. The toast focus used to
    /// read the workspace (a double lease there).
    #[test]
    fn every_render_safe_call_works_while_the_workspace_is_leased() {
        let mut h = Harness::new("layout-host-render-safe", seed_two_tasks);
        open(&mut h, &["w1"]);
        let a = actions(&mut h);
        let ws = h.ws.clone();
        let (toast_focus, w1) = h.cx.update(|_, cx| {
            let w = ws.read(cx);
            (w.toast_focus.clone(), w.live_panels()[0].entity_id())
        });
        let (fired, deferred) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(false)));
        let subs = h.cx.update(|window, cx| {
            let key = WindowKey(ws.entity_id());
            ws.update(cx, |_, cx| {
                assert_eq!(a.window_key(), key);
                for slot in [Slot::Dock, Slot::Sidebar] {
                    assert!(a.focus_handle(slot, cx).is_some(), "{slot:?}");
                }
                assert_eq!(a.focus_handle(Slot::Toasts, cx), Some(toast_focus.clone()));
                assert!(a.focus_handle(Slot::HookBanner, cx).is_none());
                assert!(a.dock(cx).is_some());
                assert!(a.view(Slot::Dock, cx).is_some());
                assert!(a.view(Slot::Sidebar, cx).is_some());
                assert!(a.source(cx).is_some());
                assert_eq!(a.focused_dock_panel(window, cx).map(|(id, _)| id), Some(w1));
                let made = a.state_or_insert_with(window, cx, |_, cx| cx.new(|_| Probe::default()));
                assert_eq!(
                    a.state::<Probe>(cx).map(|s| s.entity_id()),
                    Some(made.entity_id())
                );
                let f = fired.clone();
                let data = a.observe(&[HostChange::Data], cx, move |_, _, _| f.set(f.get() + 1));
                let dock = a.observe_in(&[HostChange::DockLayout], window, cx, |_, _, _, _| {});
                // The deferred intent runs after the lease, in the update phase.
                a.defer(UiIntent::RevealTerminal { id: "w1".into() }, window, cx);
                let d = deferred.clone();
                window.defer(cx, move |_, _| d.set(true));
                (data, dock)
            })
        });
        assert!(subs.0.is_some() && subs.1.is_some());
        h.cx.update(|_, cx| ws.update(cx, |_, cx| cx.notify()));
        h.settle();
        assert!(deferred.get());
        assert!(fired.get() >= 1, "the data observer fires after the lease");
        drop(subs);
    }

    /// rescore §3.4: dock change → notify → focus, as a layout sees it.
    /// `dispatch_then` calls `then` once the dispatch returned (the new panel
    /// exists, the window focus moved) but before any observer ran. Then the
    /// dock's `LayoutChanged`; the host's notify comes from its own reaction
    /// to that event (`touch`), so a `window.defer` from `then` (the rail's
    /// rearrange and focus) runs after `DockLayout` but before `Data`. The
    /// frame's focused terminal follows the window focus only once the window
    /// drew (the test platform draws a dirty window at the end of the flush,
    /// the app on its next frame), with one more `Data`.
    #[test]
    fn dock_change_then_notify_then_focus_after_dispatch_then() {
        let mut h = Harness::new("layout-host-event-order", seed_two_tasks);
        let a = actions(&mut h);
        let log = Rc::new(RefCell::new(Vec::<String>::new()));
        let probe = h.cx.update(|window, cx| {
            let (a2, log) = (a.clone(), log.clone());
            let changes = [HostChange::DockLayout, HostChange::Data];
            cx.new(|cx| Probe {
                sub: a.observe_in(&changes, window, cx, move |_: &mut Probe, c, window, cx| {
                    log.borrow_mut()
                        .push(format!("{c:?} {}", focus_seen(&a2, window, cx)));
                }),
                ..Default::default()
            })
        });
        h.settle();
        log.borrow_mut().clear();
        h.cx.update(|window, cx| {
            let (a2, log) = (a.clone(), log.clone());
            let intent = UiIntent::OpenTaskTerminals { task: TASK.into() };
            a.dispatch_then(intent, window, cx, move |window, cx| {
                log.borrow_mut()
                    .push(format!("then {}", focus_seen(&a2, window, cx)));
                window.defer(cx, move |window, cx| {
                    log.borrow_mut()
                        .push(format!("deferred {}", focus_seen(&a2, window, cx)));
                });
            });
        });
        let flushed = std::mem::take(&mut *log.borrow_mut());
        h.settle();
        let drawn = std::mem::take(&mut *log.borrow_mut());
        assert_eq!(
            flushed,
            [
                "then panels=1 window=w1 frame=-",
                "DockLayout panels=1 window=w1 frame=-",
                "deferred panels=1 window=w1 frame=-",
                "Data panels=1 window=w1 frame=-",
                "Data panels=1 window=w1 frame=w1",
            ],
            "after the draws: {drawn:?}"
        );
        assert!(drawn.is_empty(), "all of it in one flush: {drawn:?}");
        assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
        drop(probe);
    }

    /// rescore §5.1: a window closes while requests it started are out. The
    /// late answers reach nothing of the gone host and nothing panics: the
    /// gate path's `done` still runs (the caller's busy state lives in its
    /// own view), a layout's own request that hands its answer back with
    /// `post` gets `false`. After the release `post` and `resolve_gate` are
    /// no-ops that send nothing.
    #[test]
    fn a_window_closed_before_a_late_answer_takes_nothing() {
        let mut h = Harness::new("layout-host-late-answer", seed_two_tasks);
        let gate = fixture_gate();
        let (handle, weak_ws, b, mut backend) = second_window(&mut h);
        let answer = serde_json::to_value(&gate).expect("gate json");
        let mut n = 0;
        backend.rpc = Some(Box::new(move |method, _| {
            (method == "gate.resolve").then(|| {
                n += 1;
                if n == 2 {
                    Err(SourceError {
                        code: None,
                        message: "늦은 실패".into(),
                    })
                } else {
                    Ok(answer.clone())
                }
            })
        }));
        let done = Rc::new(RefCell::new(Vec::new()));
        let posted = Rc::new(RefCell::new(Vec::new()));
        h.cx.update(|_, cx| {
            for _ in 0..2 {
                let done = done.clone();
                assert!(b.resolve_gate(params(&gate.id), cx, move |r, _| {
                    done.borrow_mut().push(r.map_err(|e| e.text))
                }));
            }
            // A layout's own request (review, blocks): the answer goes back
            // through `post`, as `[ApplyGate, Notify]` or `[Toast, Notify]`.
            let rx = b
                .source(cx)
                .expect("source")
                .call::<GateResolve>(&params(&gate.id));
            let (b, posted) = (b.clone(), posted.clone());
            cx.spawn(async move |cx| {
                let r = rx.await;
                cx.update(|cx| {
                    let first = match r {
                        Ok(g) => HostUpdate::ApplyGate(g),
                        Err(e) => HostUpdate::Toast {
                            kind: ToastKind::Error,
                            title: "실패".into(),
                            body: e.message.into(),
                        },
                    };
                    posted
                        .borrow_mut()
                        .push(b.post(vec![first, HostUpdate::Notify], cx));
                });
            })
            .detach();
        });
        let ws = h.ws.clone();
        let main = |h: &mut Harness| {
            h.cx.update(|_, cx| {
                let w = ws.read(cx);
                (w.toasts.len(), w.orch.gates.len())
            })
        };
        let main_before = main(&mut h);
        h.cx.update(|_, cx| {
            let _ = handle.update(cx, |_, window, _| window.remove_window());
        });
        h.settle();
        assert!(weak_ws.upgrade().is_none(), "the second workspace is gone");
        assert!(done.borrow().is_empty() && posted.borrow().is_empty());
        // podd answers now, after the window closed.
        backend.serve();
        h.settle();
        let sent = |b: &TestBackend| {
            b.calls
                .iter()
                .filter(|c| c.starts_with("gate.resolve"))
                .count()
        };
        assert_eq!(sent(&backend), 3);
        assert_eq!(
            *done.borrow(),
            [Ok(()), Err("늦은 실패".to_string())],
            "done runs once per request"
        );
        assert_eq!(*posted.borrow(), [false], "nothing to post to");
        assert_eq!(main(&mut h), main_before, "the other window got nothing");
        // After the release: no-ops that send nothing.
        h.cx.update(|_, cx| {
            assert!(!b.post(vec![HostUpdate::Notify], cx));
            assert!(!b.resolve_gate(params(&gate.id), cx, |_, _| {
                panic!("no done for a gone host")
            }));
        });
        h.settle();
        backend.serve();
        assert_eq!(sent(&backend), 3, "nothing sent after the release");
    }

    /// rescore §3.4, §5.1: the order a gate answer lands in, on the real
    /// host. The gate path merges the answer (or raises the failure toast)
    /// and touches the host, then calls `done` in the same update: `done`
    /// reads the merged gate or the toast, and the `Data` observers run
    /// after `done`. `post` applies its list in one update, so the observer
    /// runs once and sees all of it, whatever the order of `Notify`.
    #[test]
    fn gate_answers_land_before_done_and_the_observers() {
        let mut h = Harness::new("layout-host-gate-order", seed_two_tasks);
        let a = actions(&mut h);
        let gate = fixture_gate();
        let answer = serde_json::to_value(&gate).expect("gate json");
        h.backend.rpc = Some(Box::new(move |method, params| {
            (method == "gate.resolve").then(|| {
                if params.get("id").and_then(|v| v.as_str()) == Some("g-fail") {
                    Err(SourceError {
                        code: Some(orch_codes::GATE_RESOLVED),
                        message: "already decided".into(),
                    })
                } else {
                    Ok(answer.clone())
                }
            })
        }));
        let log = Rc::new(RefCell::new(Vec::<String>::new()));
        let probe = h.cx.update(|_, cx| {
            let (a2, log) = (a.clone(), log.clone());
            cx.new(|cx| Probe {
                sub: a.observe(&[HostChange::Data], cx, move |_: &mut Probe, _, cx| {
                    log.borrow_mut()
                        .push(format!("Data {}", gate_seen(&a2, cx)));
                }),
                ..Default::default()
            })
        });
        h.settle();
        let t =
            h.cx.update(|_, cx| a.read(cx, |f| f.toasts.len()).expect("frame"));
        let step = |h: &mut Harness, run: &dyn Fn(&mut App)| {
            log.borrow_mut().clear();
            h.cx.update(|_, cx| run(cx));
            h.settle();
            std::mem::take(&mut *log.borrow_mut())
        };
        let decide = |id: &'static str| {
            let (a, log) = (a.clone(), log.clone());
            move |cx: &mut App| {
                let (a2, log) = (a.clone(), log.clone());
                assert!(a.resolve_gate(params(id), cx, move |r, cx| {
                    let r = if r.is_ok() { "Ok" } else { "Err" };
                    log.borrow_mut()
                        .push(format!("done {r} {}", gate_seen(&a2, cx)));
                }));
            }
        };
        let post = |updates: Vec<HostUpdate>| {
            let a = a.clone();
            move |cx: &mut App| assert!(a.post(updates.clone(), cx))
        };
        let g = gate.id.clone();
        let mut also = gate.clone();
        also.id = "g-post".into();
        let both = {
            let mut ids = [g.clone(), also.id.clone()];
            ids.sort();
            ids.join(",")
        };
        // The gate path: success, then a failure (nothing merged).
        let g_ok: &'static str = Box::leak(g.clone().into_boxed_str());
        assert_eq!(
            step(&mut h, &decide(g_ok)),
            [
                format!("done Ok gates=[{g}] toasts={t}"),
                format!("Data gates=[{g}] toasts={t}"),
            ],
            "success: merged and touched, then done, then the observers"
        );
        assert_eq!(
            step(&mut h, &decide("g-fail")),
            [
                format!("done Err gates=[{g}] toasts={}", t + 1),
                format!("Data gates=[{g}] toasts={}", t + 1),
            ],
            "failure: the toast, then done, then the observers"
        );
        // The review and block layouts' own path, through `post`.
        let toast = HostUpdate::Toast {
            kind: ToastKind::Error,
            title: "결정 실패".into(),
            body: "이유".into(),
        };
        assert_eq!(
            step(&mut h, &post(vec![toast, HostUpdate::Notify])),
            [format!("Data gates=[{g}] toasts={}", t + 2)],
            "[Toast, Notify]: one observer call, the toast raised"
        );
        assert_eq!(
            step(
                &mut h,
                &post(vec![HostUpdate::Notify, HostUpdate::ApplyGate(also)])
            ),
            [format!("Data gates=[{both}] toasts={}", t + 2)],
            "[Notify, ApplyGate]: the observer runs after the whole list"
        );
        drop(probe);
    }
}
