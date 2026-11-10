//! The main window: sidebar (terminals, later grouped by task), dock area of
//! terminal panels, status bar, toasts and dialogs.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::dock::{
    DockArea, DockEvent, DockLayout, DockPlacement, DockSkin, PanelBuildContext, PanelId,
    PanelStyle, panel_handle,
};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Disableable, Sizable as _, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::editor_panel::EditorPanel;
use crate::layout::{self, Saved, label_key};
use crate::panel::TerminalPanel;
use crate::qa_marks::QaId;
use crate::review::ReviewPanel;
use crate::source::{PoddSource, Program, RunSpec, SourceEvent, TaskSpec};
use crate::vm::{
    self, AgentState, Capabilities, ConnState, Liveness, RepoVm, StatusVm, TaskVm, TerminalVm,
};
use podd::protocol::{AgentKind, codes};

actions!(
    pod_app,
    [
        NewTerminal,
        ArrangeGrid,
        OpenReview,
        QuickOpenFiles,
        OpenFilesPanel
    ]
);

/// Language servers allowed to run at once across all task editors
/// (rust-analyzer is memory heavy); the least recently used one is
/// suspended. `POD_APP_LSP_MAX` overrides it (tests).
fn lsp_cap() -> usize {
    if crate::fixture_scenes::lsp_max() > 0 {
        return crate::fixture_scenes::lsp_max();
    }
    std::env::var("POD_APP_LSP_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2)
}

fn request_age(since: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    relative_age(now.saturating_sub(since) / 1000)
}

fn relative_age(seconds: u64) -> String {
    match seconds {
        0..60 => "방금".into(),
        60..3600 => format!("{}분 전", seconds / 60),
        3600..86400 => format!("{}시간 전", seconds / 3600),
        _ => format!("{}일 전", seconds / 86400),
    }
}

/// Panel-level actions (tab menu, sidebar buttons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Open the terminal in one more panel.
    Duplicate,
    Rename,
    /// Kill the terminal (asks first).
    Kill,
    /// Close this panel only (detach).
    ClosePanel,
}

/// App-wide handles the dock's panel builder needs (it runs inside
/// `DockArea::load`, so it must not borrow the workspace).
pub struct AppGlobals {
    pub source: PoddSource,
    pub workspace: WeakEntity<Workspace>,
    pub panels: Rc<RefCell<Vec<WeakEntity<TerminalPanel>>>>,
    pub labels: Rc<RefCell<HashMap<String, String>>>,
    /// Data for the non-terminal panel builders during a layout restore.
    pub restore: Rc<RefCell<RestoreCtx>>,
}

impl Global for AppGlobals {}

/// Shared between [`Workspace::restore`] and the panel builders while
/// `DockArea::load` runs (the builders cannot read the leased workspace).
#[derive(Default)]
pub struct RestoreCtx {
    /// Tasks known at restore time (Review panels need the task).
    pub tasks: Vec<TaskVm>,
    /// Panels the builders made; the workspace adopts them after the load.
    pub built: Vec<RestoredPanel>,
}

pub enum RestoredPanel {
    Review(WeakEntity<ReviewPanel>),
    Group(WeakEntity<crate::groups::GroupPanel>),
    Gates(WeakEntity<crate::gates::GatesPanel>),
    Tree(WeakEntity<crate::tree::AgentTreePanel>),
    Settings(WeakEntity<crate::chrome::ChromePanel>),
    Inspector(WeakEntity<crate::chrome::ChromePanel>),
    Editor(WeakEntity<EditorPanel>),
}

/// `PanelRegistry` builder for persisted editor panels (`{"task", "session"}`).
/// Pruning keeps only tasks whose worktree exists ([`layout::Alive::worktrees`]).
pub fn build_editor_panel(
    ctx: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let task_id = layout::leaf_field(ctx.state(), "task")
        .unwrap_or_default()
        .to_string();
    let session = EditorPanel::session_of_state(ctx.state());
    let g = cx.global::<AppGlobals>();
    let (ws, restore) = (g.workspace.clone(), g.restore.clone());
    let root = restore
        .borrow()
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .and_then(|t| t.worktree.clone())
        .unwrap_or_default();
    let panel = cx.new(|cx| {
        EditorPanel::new(
            task_id,
            std::path::PathBuf::from(root),
            session.as_ref(),
            ws,
            window,
            cx,
        )
    });
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Editor(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for persisted Review panels (`{"task": id}`).
pub fn build_review_panel(
    ctx: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let task_id = layout::leaf_field(ctx.state(), "task")
        .unwrap_or_default()
        .to_string();
    let g = cx.global::<AppGlobals>();
    let (source, ws, restore) = (g.source.clone(), g.workspace.clone(), g.restore.clone());
    // Pruning keeps only reviews of existing tasks; a bare task still works
    // (the next task poll fills it in through `set_task`).
    let task = restore
        .borrow()
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .cloned();
    let task = task
        .or_else(|| TaskVm::from_json(&serde_json::json!({ "id": task_id })))
        .expect("task id");
    let panel = cx.new(|cx| ReviewPanel::new(source, ws, &task, window, cx));
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Review(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for persisted Group panels (`{"group": id}`).
pub fn build_group_panel(
    ctx: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let group = layout::leaf_field(ctx.state(), "group")
        .unwrap_or_default()
        .to_string();
    let g = cx.global::<AppGlobals>();
    let (source, ws, restore) = (g.source.clone(), g.workspace.clone(), g.restore.clone());
    let panel = cx.new(|cx| crate::groups::GroupPanel::new(source, ws, group, None, window, cx));
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Group(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for the persisted gates inbox.
pub fn build_gates_panel(
    _: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let g = cx.global::<AppGlobals>();
    let (source, ws, restore) = (g.source.clone(), g.workspace.clone(), g.restore.clone());
    let panel = cx.new(|cx| crate::gates::GatesPanel::new(source, ws, window, cx));
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Gates(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for the persisted agent tree.
pub fn build_tree_panel(
    _: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let g = cx.global::<AppGlobals>();
    let (ws, restore) = (g.workspace.clone(), g.restore.clone());
    let panel = cx.new(|cx| crate::tree::AgentTreePanel::new(ws, cx));
    panel.update(cx, |p, cx| p.forward_focus(window, cx));
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Tree(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for persisted terminal panels.
pub fn build_panel(
    ctx: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let id = TerminalPanel::terminal_of_state(ctx.state()).unwrap_or_default();
    let panel = new_panel(id, window, cx);
    panel_handle(panel)
}

fn new_panel(id: String, window: &mut Window, cx: &mut App) -> Entity<TerminalPanel> {
    let g = cx.global::<AppGlobals>();
    let (source, ws, panels) = (g.source.clone(), g.workspace.clone(), g.panels.clone());
    let label: SharedString = g
        .labels
        .borrow()
        .get(&id)
        .cloned()
        .unwrap_or_else(|| id.clone())
        .into();
    let panel = cx.new(|cx| TerminalPanel::new(source, ws, id, label, window, cx));
    panels.borrow_mut().push(panel.downgrade());
    panel
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

pub(crate) struct Toast {
    pub(crate) id: u64,
    visible: bool,
    paused: Rc<Cell<bool>>,
    pub(crate) kind: ToastKind,
    pub(crate) title: SharedString,
    pub(crate) body: SharedString,
    terminal_request: Option<String>,
    /// Raised at (ms since the epoch), shown as `N분 전` in 알림.
    pub(crate) at: u64,
}

/// Startup options from the command line.
#[derive(Clone, Debug, Default)]
pub struct StartOptions {
    /// Beep when a terminal turns blocked.
    pub sound: bool,
    /// Open every running terminal and arrange a grid (ignores the saved layout's placement).
    pub open_all: bool,
    /// Open the Review panel of this task once the task list arrives.
    pub review: Option<String>,
    /// Open the "확장" settings page once the task list arrives (`--extensions`, screenshots).
    pub settings: bool,
    /// Check agent hooks at launch with this CLI (`None`: no check, e.g. tests).
    pub hook_cli: Option<crate::hooks_prompt::HookCliHandle>,
}

pub struct Workspace {
    pub(crate) source: PoddSource,
    pub(crate) dock: Entity<DockArea>,
    pub(crate) conn: ConnState,
    pub(crate) caps: Capabilities,
    pub(crate) terminals: Vec<TerminalVm>,
    pub(crate) tasks: Vec<TaskVm>,
    pub(crate) repos: Vec<RepoVm>,
    /// Orchestration UI state (P5): gates, group / gates / agent-tree panels.
    pub(crate) orch: crate::orch::OrchState,
    panels: Rc<RefCell<Vec<WeakEntity<TerminalPanel>>>>,
    labels: Rc<RefCell<HashMap<String, String>>>,
    saved: Saved,
    pub(crate) restored: bool,
    have_list: bool,
    /// A `task.list` result arrived since start (restore waits for it).
    have_tasks: bool,
    restore_ctx: Rc<RefCell<RestoreCtx>>,
    restore_timer: Option<Task<()>>,
    pub(crate) options: StartOptions,
    pub(crate) toasts: Vec<Toast>,
    next_toast: u64,
    toast_focus: FocusHandle,
    toast_focus_paused: Rc<Cell<bool>>,
    /// The workspace root. Holds focus when nothing else does (launch, after
    /// the focused panel closed) so app shortcuts (Ctrl+K, Ctrl+N …) still
    /// reach the workspace: GPUI dispatches keys along the focus path only.
    root_focus: FocusHandle,
    pub stats_line: Option<SharedString>,
    /// The OS window title last set (`Pod`, `<작업> — Pod`): taskbar, Alt+Tab, screen readers.
    pub(crate) os_title: String,
    save_task: Option<Task<()>>,
    pub(crate) sidebar: Entity<Sidebar>,
    pub(crate) focused_terminal: Option<String>,
    pub(crate) settings_panel: Option<WeakEntity<crate::chrome::ChromePanel>>,
    pub(crate) inspector_panel: Option<WeakEntity<crate::chrome::ChromePanel>>,
    /// Open Review panels (one per task).
    pub(crate) reviews: Vec<WeakEntity<ReviewPanel>>,
    /// Open file/editor panels (one per task worktree).
    editors: Vec<WeakEntity<EditorPanel>>,
    /// Task of the editor used last (quick open target, status bar).
    last_editor: Option<String>,
    /// The user confirmed closing with unsaved files.
    pub(crate) quit_confirmed: bool,
    /// Tasks whose removal the user confirmed in the app, unsaved files
    /// included (`task.removing` then closes without saving).
    pub(crate) removal_confirmed: HashSet<String>,
    /// Links offered by the last "터미널에서 링크 열기" picker: (terminal, target).
    pub(crate) link_choices: Vec<(String, pod_terminal_view::LinkTarget)>,
    /// Agent hook check at launch and its banner (docs/14 §11).
    pub(crate) hooks: crate::hooks_prompt::HookPromptState,
    _pump: Task<()>,
    _subs: Vec<Subscription>,
}

impl Workspace {
    pub fn new(
        source: PoddSource,
        events: UnboundedReceiver<SourceEvent>,
        options: StartOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let panels = Rc::new(RefCell::new(Vec::new()));
        let labels = Rc::new(RefCell::new(HashMap::new()));
        let restore_ctx = Rc::new(RefCell::new(RestoreCtx::default()));
        cx.set_global(AppGlobals {
            source: source.clone(),
            workspace: cx.entity().downgrade(),
            panels: panels.clone(),
            labels: labels.clone(),
            restore: restore_ctx.clone(),
        });

        let (dock, skin) =
            DockSkin::dock_area("pod-app", Some(layout::LAYOUT_VERSION as usize), window, cx);
        skin.set_close_button_visible(true, cx);
        // Auto: a lone panel gets a plain title bar (cheaper than a tab bar, see docs/09 §6).
        skin.set_panel_style(PanelStyle::Auto, cx);
        let mut subs =
            vec![
                cx.subscribe_in(&dock, window, |w: &mut Self, _, ev: &DockEvent, _, cx| {
                    if let DockEvent::LayoutChanged = ev {
                        w.schedule_save(cx);
                        w.touch(cx);
                    }
                }),
            ];

        subs.push(cx.observe_window_appearance(window, |w, _, cx| {
            let settings = cx.global::<crate::theme::Settings>().clone();
            if settings.theme == "system" {
                settings.apply(cx);
                w.retheme(cx);
            } else if crate::theme::refresh_os_high_contrast(crate::theme::os_high_contrast, cx) {
                w.retheme(cx);
            }
        }));
        // Windows contrast themes have no GPUI event of their own; re-check
        // when the window is activated (e.g. returning from Settings).
        subs.push(cx.observe_window_activation(window, |w, window, cx| {
            if window.is_window_active()
                && crate::theme::refresh_os_high_contrast(crate::theme::os_high_contrast, cx)
            {
                w.retheme(cx);
            }
        }));
        let root_focus = cx.focus_handle();
        // Keyboard-only start: no click needed before the first shortcut.
        {
            let root = root_focus.clone();
            window.defer(cx, move |window, cx| {
                if window.focused(cx).is_none() {
                    root.focus(window, cx);
                }
            });
        }
        subs.push(cx.on_focus_lost(window, |w, window, cx| {
            if !window.has_active_dialog(cx) {
                w.root_focus.focus(window, cx);
            }
        }));
        // `.tab_index()` only applies to element-owned handles; a tracked handle must be a tab stop itself.
        let toast_focus = cx.focus_handle().tab_stop(true);
        let toast_focus_paused = Rc::new(Cell::new(false));
        let pause = toast_focus_paused.clone();
        subs.push(cx.on_focus_in(&toast_focus, window, move |_, _, _| pause.set(true)));
        let pause = toast_focus_paused.clone();
        subs.push(cx.on_focus_out(&toast_focus, window, move |_, _, _, _| pause.set(false)));
        let pump = cx.spawn_in(window, async move |this, cx| {
            let mut events = events;
            while let Some(ev) = events.next().await {
                if this
                    .update_in(cx, |w, window, cx| w.on_source_event(ev, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });

        // Agent hooks: checked in the background, never installed without a click.
        if let Some(h) = options.hook_cli.clone() {
            cx.defer_in(window, move |w: &mut Self, _, cx| {
                w.start_hook_check(h.0, cx)
            });
        }

        Self {
            source,
            dock,
            conn: ConnState::Connecting,
            caps: Capabilities::default(),
            terminals: Vec::new(),
            tasks: Vec::new(),
            repos: Vec::new(),
            panels,
            labels,
            saved: layout::load(),
            restored: false,
            have_list: false,
            have_tasks: false,
            restore_ctx,
            restore_timer: None,
            options,
            toasts: Vec::new(),
            next_toast: 1,
            toast_focus,
            toast_focus_paused,
            root_focus,
            stats_line: None,
            os_title: String::new(),
            save_task: None,
            focused_terminal: None,
            settings_panel: None,
            inspector_panel: None,
            reviews: Vec::new(),
            editors: Vec::new(),
            last_editor: None,
            quit_confirmed: false,
            link_choices: Vec::new(),
            removal_confirmed: HashSet::new(),
            hooks: Default::default(),
            orch: Default::default(),
            sidebar: {
                let ws = cx.entity().downgrade();
                cx.new(|cx| Sidebar::new(ws, window, cx))
            },
            _pump: pump,
            _subs: subs,
        }
    }

    pub(crate) fn sidebar_focus_handle(&self, cx: &App) -> FocusHandle {
        self.sidebar.read(cx).focus.clone()
    }

    #[cfg(test)]
    pub(crate) fn root_focus_handle(&self) -> FocusHandle {
        self.root_focus.clone()
    }

    #[cfg(test)]
    pub(crate) fn sidebar_filter(&self, cx: &App) -> Entity<InputState> {
        self.sidebar.read(cx).filter.clone()
    }

    /// Type `query` into the sidebar search (render tests).
    #[cfg(test)]
    pub(crate) fn set_sidebar_query(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let filter = self.sidebar_filter(cx);
        filter.update(cx, |s, cx| s.set_value(query.to_string(), window, cx));
        self.sidebar.update(cx, |_, cx| cx.notify());
    }

    /// Tokens changed (theme or contrast): repaint terminals, editors, chrome.
    fn retheme(&mut self, cx: &mut Context<Self>) {
        for panel in self.live_panels() {
            let view = panel.read(cx).view.clone();
            view.update(cx, |v, cx| {
                v.set_palette(crate::theme::terminal_palette(), cx)
            });
        }
        self.refresh_editor_theme(cx);
        self.touch(cx);
    }

    /// Data shown in the sidebar changed.
    pub(crate) fn touch(&mut self, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |_, cx| cx.notify());
        cx.notify();
    }

    // ---- data source ----------------------------------------------------

    fn on_source_event(&mut self, ev: SourceEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            SourceEvent::Conn(state) => {
                let was = self.conn.clone();
                match (&was, &state) {
                    (ConnState::Connected { .. }, ConnState::Reconnecting { error, .. }) => {
                        self.raise_toast(
                            ToastKind::Warning,
                            crate::strings::PODD_DISCONNECTED,
                            crate::tr!(RECONNECTING_ERROR; error;),
                            cx,
                        );
                    }
                    (ConnState::Reconnecting { .. }, ConnState::Connected { pid, .. }) => {
                        self.raise_toast(
                            ToastKind::Success,
                            crate::strings::PODD_RECONNECTED,
                            crate::tr!(DAEMON_PID_PID_PANELS_RE_ATTACH; pid;),
                            cx,
                        );
                    }
                    _ => {}
                }
                self.conn = state;
            }
            SourceEvent::Caps(caps) => {
                self.caps = caps;
                crate::mods_ui::refresh(self, cx);
            }
            SourceEvent::ModNotice(n) => crate::mods_ui::on_notice(self, *n, window, cx),
            SourceEvent::ModChanged(m) => crate::mods_ui::on_changed(*m, cx),
            SourceEvent::Terminals(list) => self.on_terminals(list, window, cx),
            SourceEvent::Tasks(tasks) => {
                self.tasks = tasks;
                self.have_tasks = true;
                if !self.restored && self.have_list {
                    self.restore(window, cx);
                }
                for r in self.live_reviews() {
                    let id = r.read(cx).task_id.clone();
                    if let Some(t) = self.task(&id).cloned() {
                        r.update(cx, |r, cx| r.set_task(&t, cx));
                    }
                }
                if self.restored {
                    let gone: Vec<String> = self
                        .live_editors()
                        .iter()
                        .map(|e| e.read(cx).task_id.clone())
                        .filter(|id| self.task(id).is_none())
                        .collect();
                    for id in gone {
                        self.close_editor_of(&id, window, cx);
                    }
                }
                if let Some(id) = self.options.review.take() {
                    self.open_review(&id, window, cx);
                }
                if std::mem::take(&mut self.options.settings) {
                    crate::mods_ui::open_page(self, window, cx);
                }
            }
            SourceEvent::Repos(repos) => self.repos = repos,
            SourceEvent::TaskFilesChanged(id) => {
                for r in self.live_reviews() {
                    if r.read(cx).task_id == id {
                        r.update(cx, |r, cx| r.refresh_diff(cx));
                    }
                }
            }
            SourceEvent::TaskRemoving { id, worktree } => {
                self.task_removing(&id, &worktree, window, cx)
            }
            SourceEvent::TaskCreated { terminal, hint, .. } => {
                crate::auto_open::on_task_created(self, terminal, hint, window, cx)
            }
            SourceEvent::Gates(_)
            | SourceEvent::Gate(_)
            | SourceEvent::GroupCheck(_)
            | SourceEvent::Delivery(..) => {
                self.on_orch_event(ev, window, cx);
            }
            SourceEvent::OrchChanged(_) | SourceEvent::OrchMail(_) | SourceEvent::OrchResync => {
                crate::orch_ui::on_event(self, ev, window, cx)
            }
            SourceEvent::Status { id, status } => {
                if let Some(t) = self.terminals.iter_mut().find(|t| t.id == id) {
                    let old = std::mem::replace(&mut t.status, status.clone());
                    if let Some(new) = status {
                        self.status_changed(&id, old.as_ref(), &new, window, cx);
                        for r in self.live_reviews() {
                            r.update(cx, |r, cx| r.agent_status(&id, &new, cx));
                        }
                    }
                }
            }
        }
        self.orch_data_changed(cx);
        self.refresh_link_roots(cx);
        self.touch(cx);
    }

    fn on_terminals(&mut self, list: Vec<TerminalVm>, window: &mut Window, cx: &mut Context<Self>) {
        if self.have_list {
            let before: HashMap<&str, Liveness> = self
                .terminals
                .iter()
                .map(|t| (t.id.as_str(), t.live))
                .collect();
            let exits: Vec<(String, String)> = list
                .iter()
                .filter(|t| {
                    !t.is_running() && before.get(t.id.as_str()) == Some(&Liveness::Running)
                })
                .map(|t| {
                    (
                        self.label_of(t),
                        format!("{} ({}) {}", t.id, t.program, t.live_label()),
                    )
                })
                .collect();
            for (title, body) in exits {
                self.terminal_exited(&title, &body, cx);
            }
            // Status changes the subscription stream did not deliver (e.g. while reconnecting).
            let old: HashMap<String, Option<StatusVm>> = self
                .terminals
                .iter()
                .map(|t| (t.id.clone(), t.status.clone()))
                .collect();
            for t in &list {
                if let (Some(prev), Some(new)) = (old.get(&t.id), &t.status) {
                    self.status_changed(&t.id, prev.as_ref(), new, window, cx);
                }
            }
        }
        self.toasts.retain(|toast| {
            toast.terminal_request.as_ref().is_none_or(|id| {
                list.iter()
                    .any(|terminal| &terminal.id == id && terminal.is_running())
            })
        });
        self.terminals = list;
        self.have_list = true;
        self.refresh_labels(cx);
        if !self.restored {
            // Review/Group panels need the task list, which the source sends
            // right after the first terminal list. Give it a moment, then
            // restore without it (a daemon whose task.list fails).
            if self.have_tasks || !self.caps.tasks {
                self.restore(window, cx);
            } else if self.restore_timer.is_none() {
                self.restore_timer = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                    let _ = this.update_in(cx, |w, window, cx| {
                        if !w.restored {
                            w.restore(window, cx);
                        }
                    });
                }));
            }
        }
    }

    /// A terminal's agent status changed. Turning `blocked` or `done` raises a
    /// toast (blocked also beeps) unless the user is looking at that terminal;
    /// a `done` the user is looking at is acknowledged right away.
    fn status_changed(
        &mut self,
        id: &str,
        old: Option<&StatusVm>,
        new: &StatusVm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A request's lifetime follows its terminal status, including cancellation
        // and exit. Remove resolved requests from both the stack and history.
        if new.state != AgentState::Blocked {
            self.toasts
                .retain(|toast| toast.terminal_request.as_deref() != Some(id));
        }
        let entered = |s: AgentState| {
            new.state == s && old.is_none_or(|o| o.state != s || o.kind != new.kind)
        };
        let blocked = entered(AgentState::Blocked);
        let done = entered(AgentState::Done);
        if !blocked && !done {
            return;
        }
        let viewed = self.terminal_in_view(window, cx);
        if watching(window.is_window_active(), viewed.as_deref(), id) {
            if done {
                self.source.ack(id);
            }
            return;
        }
        let Some(t) = self.terminal(id).cloned() else {
            return;
        };
        let title = self.label_of(&t);
        if blocked {
            let why = new.kind.clone().unwrap_or_else(|| "input".into());
            self.toasts
                .retain(|toast| toast.terminal_request.as_deref() != Some(id));
            let age = request_age(new.since);
            self.raise_toast(
                ToastKind::Warning,
                crate::tr!(TITLE_NEEDS_YOU; title;),
                crate::tr!(BLOCKED_ON_WHY_TURN_AT; why; t.id, new.turn, age),
                cx,
            );
            if let Some(toast) = self.toasts.last_mut() {
                toast.terminal_request = Some(id.to_owned());
            }
            if self.options.sound {
                beep();
            }
        } else {
            self.raise_toast(
                ToastKind::Success,
                crate::tr!(TITLE_IS_DONE; title;),
                crate::tr!(TURN; ; t.id, new.label(), new.turn),
                cx,
            );
        }
    }

    /// The terminal whose panel holds keyboard focus in the last rendered
    /// frame. Unlike `focused_terminal` (the last terminal pane focused, kept
    /// for selection), this is `None` while a Review/Group/Gates/Agents panel,
    /// the sidebar or a dialog has focus, or when the panel is not rendered
    /// (hidden tab, closed dock).
    fn terminal_in_view(&self, window: &Window, cx: &App) -> Option<String> {
        self.live_panels().into_iter().find_map(|p| {
            let p = p.read(cx);
            p.view
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
                .then(|| p.terminal_id.clone())
        })
    }

    /// A terminal pane got keyboard focus: remember it and tell podd the user
    /// looked at it (`terminal.ack`, done -> idle).
    pub fn pane_focused(&mut self, id: &str, cx: &mut Context<Self>) {
        if std::env::var_os("POD_APP_DEBUG").is_some() {
            println!(
                "[pod-app] pane focused: {id} (status caps: {})",
                self.caps.status
            );
        }
        self.focused_terminal = Some(id.to_string());
        if self.caps.status {
            self.source.ack(id);
        }
        cx.notify();
    }

    /// Notification hook for a finished terminal process.
    fn terminal_exited(&mut self, title: &str, body: &str, cx: &mut Context<Self>) {
        self.raise_toast(
            ToastKind::Info,
            crate::tr!(TITLE_EXITED; title;),
            body.to_string(),
            cx,
        );
    }

    pub(crate) fn label_of(&self, t: &TerminalVm) -> String {
        self.saved
            .labels
            .get(&label_key(&t.id, t.created_ms))
            .cloned()
            .or_else(|| t.name.clone())
            .unwrap_or_else(|| t.id.clone())
    }

    fn refresh_labels(&mut self, cx: &mut Context<Self>) {
        let map: HashMap<String, String> = self
            .terminals
            .iter()
            .map(|t| (t.id.clone(), self.label_of(t)))
            .collect();
        for p in self.live_panels() {
            let id = p.read(cx).terminal_id.clone();
            if let Some(l) = map.get(&id) {
                let l: SharedString = l.clone().into();
                p.update(cx, |p, cx| p.set_label(l, cx));
            }
            if let Some(t) = self.terminal(&id) {
                let agent = crate::agent_kind::identify_in(t, &self.tasks).kind;
                p.update(cx, |p, cx| p.set_agent(agent, cx));
            }
        }
        *self.labels.borrow_mut() = map;
    }

    pub(crate) fn live_panels(&self) -> Vec<Entity<TerminalPanel>> {
        self.panels
            .borrow()
            .iter()
            .filter_map(|w| w.upgrade())
            .collect()
    }

    // ---- layout ---------------------------------------------------------

    fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restored = true;
        let alive = layout::Alive {
            terminals: self.terminals.iter().map(|t| t.id.clone()).collect(),
            tasks: self.tasks.iter().map(|t| t.id.clone()).collect(),
            groups: self.tasks.iter().filter_map(|t| t.group.clone()).collect(),
            review: self.caps.review,
            orch: self.caps.orch,
            worktrees: self
                .tasks
                .iter()
                .filter(|t| {
                    t.worktree
                        .as_deref()
                        .is_some_and(|w| !w.is_empty() && std::path::Path::new(w).is_dir())
                })
                .map(|t| t.id.clone())
                .collect(),
        };
        if let Some(state) = self
            .saved
            .dock
            .as_ref()
            .and_then(|d| layout::prune_dock(d, &alive))
        {
            self.restore_ctx.borrow_mut().tasks = self.tasks.clone();
            let _ = self.dock.update(cx, |a, cx| a.load(state, window, cx));
            let built = {
                let mut ctx = self.restore_ctx.borrow_mut();
                ctx.tasks.clear();
                std::mem::take(&mut ctx.built)
            };
            self.adopt_restored(built, cx);
        }
        if self.options.open_all {
            let running: Vec<String> = self
                .terminals
                .iter()
                .filter(|t| t.is_running())
                .map(|t| t.id.clone())
                .collect();
            let open: HashSet<String> = self
                .live_panels()
                .iter()
                .map(|p| p.read(cx).terminal_id.clone())
                .collect();
            for id in running.into_iter().filter(|id| !open.contains(id)) {
                self.add_panel(id, window, cx);
            }
            self.arrange_grid(window, cx);
        }
        self.schedule_save(cx);
    }

    /// Track the non-terminal panels the dock rebuilt from the saved layout,
    /// like the `open_*` functions do for new ones.
    fn adopt_restored(&mut self, built: Vec<RestoredPanel>, cx: &mut Context<Self>) {
        for p in built {
            match p {
                RestoredPanel::Review(r) => {
                    if let Some(panel) = r.upgrade() {
                        let task = panel.read(cx).task_id.clone();
                        let status = self
                            .task(&task)
                            .and_then(|t| t.terminals.first().cloned())
                            .and_then(|id| {
                                self.terminal(&id)
                                    .and_then(|t| t.status.clone())
                                    .map(|s| (id, s))
                            });
                        if let Some((term, s)) = status {
                            panel.update(cx, |r, cx| r.agent_status(&term, &s, cx));
                        }
                    }
                    self.reviews.push(r);
                }
                RestoredPanel::Group(g) => self.orch.groups.push(g),
                RestoredPanel::Gates(g) => self.orch.gates_panel = Some(g),
                RestoredPanel::Tree(t) => self.orch.tree_panel = Some(t),
                RestoredPanel::Settings(p) => self.settings_panel = Some(p),
                RestoredPanel::Inspector(p) => self.inspector_panel = Some(p),
                RestoredPanel::Editor(e) => {
                    if let Some(p) = e.upgrade() {
                        self.last_editor = Some(p.read(cx).task_id.clone());
                    }
                    self.editors.push(e);
                }
            }
        }
        // Restored terminal panels get their label and agent mark now, not at the next list.
        self.refresh_labels(cx);
        self.touch(cx);
    }

    pub(crate) fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if !self.restored {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |w, cx| w.save_now(cx));
        }));
    }

    pub(crate) fn save_now(&mut self, cx: &mut Context<Self>) {
        self.saved.version = layout::LAYOUT_VERSION;
        self.saved.dock = Some(self.dock.read(cx).dump(cx));
        if let Err(e) = layout::save(&self.saved) {
            self.raise_toast(
                ToastKind::Error,
                crate::strings::COULD_NOT_SAVE_LAYOUT,
                e.to_string(),
                cx,
            );
        }
    }

    /// New panel for `id` in the center (first tab group).
    pub(crate) fn add_panel(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        let panel = new_panel(id, window, cx);
        let handle = panel_handle(panel.clone());
        self.dock.update(cx, |a, cx| {
            a.add_panel_view(handle, DockPlacement::Center, None, window, cx)
        });
        let roots = self.link_roots(&panel.read(cx).terminal_id);
        panel.update(cx, |p, cx| p.set_link_roots(roots, cx));
        if let Some(t) = self.terminal(&panel.read(cx).terminal_id) {
            let agent = crate::agent_kind::identify_in(t, &self.tasks).kind;
            panel.update(cx, |p, cx| p.set_agent(agent, cx));
        }
        panel
    }

    /// Show terminal `id`: focus an existing panel, or open one (always a new one with `another`).
    pub fn open_terminal(
        &mut self,
        id: &str,
        another: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = if another {
            None
        } else {
            self.live_panels()
                .into_iter()
                .find(|p| p.read(cx).terminal_id == id)
        };
        let panel = match existing {
            Some(p) => {
                let pid = PanelId::from(p.entity_id());
                self.dock
                    .update(cx, |a, cx| a.select_panel(pid, window, cx));
                p
            }
            None => self.add_panel(id.to_string(), window, cx),
        };
        let focus = panel.read(cx).view.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Rebuild the center as a near-square grid of the open panels (2x2 for 4).
    pub fn arrange_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panels = self.live_panels();
        if panels.is_empty() {
            return;
        }
        let (_, cols) = layout::grid_shape(panels.len());
        let mut v = DockLayout::v_split();
        for row in panels.chunks(cols) {
            let mut h = DockLayout::h_split();
            for p in row {
                h = h.child(
                    DockLayout::tabs().panel_view(panel_handle(p.clone()), cx),
                    None,
                );
            }
            v = v.child(h, None);
        }
        self.dock.update(cx, |a, cx| a.set_center(v, window, cx));
    }

    pub fn panel_removed(&mut self, panel: &WeakEntity<TerminalPanel>, cx: &mut Context<Self>) {
        self.panels
            .borrow_mut()
            .retain(|w| w != panel && w.upgrade().is_some());
        self.schedule_save(cx);
        self.touch(cx);
    }

    pub(crate) fn close_panels_of(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panels: Vec<_> = self
            .live_panels()
            .into_iter()
            .filter(|p| p.read(cx).terminal_id == id)
            .collect();
        for p in panels {
            self.dock.update(cx, |a, cx| a.remove_panel(p, window, cx));
        }
    }

    // ---- actions --------------------------------------------------------

    pub fn on_action(
        &mut self,
        action: WorkspaceAction,
        id: String,
        panel: Option<WeakEntity<TerminalPanel>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            WorkspaceAction::Duplicate => self.open_terminal(&id, true, window, cx),
            WorkspaceAction::Rename => self.open_rename_dialog(id, window, cx),
            WorkspaceAction::Kill => self.confirm_kill(id, window, cx),
            WorkspaceAction::ClosePanel => {
                if let Some(p) = panel.and_then(|p| p.upgrade()) {
                    self.dock.update(cx, |a, cx| a.remove_panel(p, window, cx));
                }
            }
        }
    }

    pub(crate) fn terminal(&self, id: &str) -> Option<&TerminalVm> {
        self.terminals.iter().find(|t| t.id == id)
    }

    fn confirm_kill(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.terminal(&id).cloned() else {
            return;
        };
        let ws = cx.entity().downgrade();
        let label = self.label_of(&t);
        let body = crate::tr!(KILL_THE_WHOLE_PROCESS_TREE_ENDS_PANELS_STAY_OPEN_SHOWING_THE_FIN; ; label, t.id, t.program);
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let (ws_ok, id_ok) = (ws.clone(), id.clone());
            dialog
                .title(crate::strings::KILL_TERMINAL)
                .w(px(460.))
                .child(div().text_sm().child(body.clone()))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("kill-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("kill-ok")
                                .label(crate::strings::KILL)
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = ws_ok.update(cx, |w, cx| w.kill(id_ok.clone(), cx));
                                }),
                        ),
                )
        });
    }

    fn kill(&mut self, id: String, cx: &mut Context<Self>) {
        let rx = self.source.kill(&id);
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = rx.await {
                let _ = this.update(cx, |w, cx| {
                    w.raise_toast(
                        ToastKind::Error,
                        crate::tr!(KILL_ID_FAILED; id;),
                        e.message,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// Remove an exited terminal from podd (and close its panels).
    fn remove(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.close_panels_of(&id, window, cx);
        let rx = self.source.remove(&id);
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = rx.await {
                let _ = this.update(cx, |w, cx| {
                    w.raise_toast(
                        ToastKind::Error,
                        crate::tr!(REMOVE_ID_FAILED; id;),
                        e.message,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    fn open_rename_dialog(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.terminal(&id).cloned() else {
            return;
        };
        let current = self.label_of(&t);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let ws = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog: Dialog, window, cx| {
            if first.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let (ws_ok, id_ok, input_ok) = (ws.clone(), id.clone(), input.clone());
            dialog
                .title(crate::tr!(RENAME_ID; id;))
                .w(px(420.))
                .child(Input::new(&input))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("rn-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("rn-ok")
                                .label(crate::strings::RENAME)
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let name = input_ok.read(cx).value().trim().to_string();
                                    window.close_dialog(cx);
                                    let _ =
                                        ws_ok.update(cx, |w, cx| w.rename(id_ok.clone(), name, cx));
                                }),
                        ),
                )
        });
    }

    fn rename(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        let Some(t) = self.terminal(&id).cloned() else {
            return;
        };
        let key = label_key(&t.id, t.created_ms);
        if self.caps.rename {
            let rx = self.source.rename(&id, &name);
            cx.spawn(async move |this, cx| {
                if let Ok(Err(e)) = rx.await {
                    let _ = this.update(cx, |w, cx| {
                        w.raise_toast(
                            ToastKind::Error,
                            crate::strings::RENAME_FAILED,
                            e.message,
                            cx,
                        )
                    });
                }
            })
            .detach();
            self.saved.labels.remove(&key);
        } else if name.is_empty() {
            self.saved.labels.remove(&key);
        } else {
            // podd has no rename yet: keep an app-local label.
            self.saved.labels.insert(key, name);
        }
        self.refresh_labels(cx);
        self.schedule_save(cx);
        self.touch(cx);
    }

    pub fn open_new_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let program = Rc::new(Cell::new(Program::Shell));
        let cwd = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(home)
                .placeholder(crate::strings::WORKING_DIRECTORY)
        });
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::strings::OPTIONAL_E_G_REVIEW_BOT)
        });
        let custom =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::strings::COMMAND_EXAMPLE));
        let ws = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog: Dialog, window, cx| {
            if first.replace(false) {
                name.focus_handle(cx).focus(window, cx);
            }
            let selected = program.get();
            // One choice: a radio segment group, not filled buttons (docs/10 §D4.6).
            let mut programs =
                pod_ui::radio_group("prog-group", crate::strings::PROGRAM, crate::theme::mode())
                    .self_start();
            for (i, p) in Program::ALL.into_iter().enumerate() {
                let cell = program.clone();
                programs = programs.child(pod_ui::segment(
                    ("prog", i),
                    p.label(),
                    None,
                    p == selected,
                    crate::theme::mode(),
                    move |window, _| {
                        cell.set(p);
                        window.refresh();
                    },
                ));
            }
            let field = |label: &'static str, input: &Entity<InputState>| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(crate::theme::tokens().text_muted))
                            .child(label),
                    )
                    .child(Input::new(input))
            };
            let (ws_ok, prog_ok, cwd_ok, name_ok, custom_ok) = (
                ws.clone(),
                program.clone(),
                cwd.clone(),
                name.clone(),
                custom.clone(),
            );
            dialog
                .title(crate::strings::NEW_TERMINAL)
                .w(px(520.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(crate::theme::tokens().text_muted))
                                        .child(crate::strings::PROGRAM),
                                )
                                .child(programs),
                        )
                        .when(selected == Program::Custom, |d| {
                            d.child(field(crate::strings::COMMAND, &custom))
                        })
                        .child(field(crate::strings::WORKING_DIRECTORY, &cwd))
                        .child(field(crate::strings::NAME, &name)),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("new-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("new-run")
                                .label(crate::strings::RUN)
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let argv =
                                        prog_ok.get().argv(custom_ok.read(cx).value().as_ref());
                                    let text = |e: &Entity<InputState>, cx: &App| {
                                        let v = e.read(cx).value().trim().to_string();
                                        (!v.is_empty()).then_some(v)
                                    };
                                    let spec = RunSpec {
                                        argv,
                                        cwd: text(&cwd_ok, cx),
                                        name: text(&name_ok, cx),
                                        ..Default::default()
                                    };
                                    if spec.argv.is_empty() {
                                        return;
                                    }
                                    window.close_dialog(cx);
                                    let _ = ws_ok.update(cx, |w, cx| w.run(spec, window, cx));
                                }),
                        ),
                )
        });
    }

    fn run(&mut self, spec: RunSpec, window: &mut Window, cx: &mut Context<Self>) {
        let rx = self.source.run(spec.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.await.unwrap_or_else(|_| {
                Err(crate::source::SourceError {
                    code: None,
                    message: crate::strings::PODD_CLIENT_STOPPED.into(),
                })
            });
            let _ = this.update_in(cx, |w, window, cx| match result {
                Ok(t) => {
                    if !w.terminals.iter().any(|x| x.id == t.id) {
                        w.terminals.push(t.clone());
                        w.refresh_labels(cx);
                    }
                    w.open_terminal(&t.id, false, window, cx);
                }
                Err(e) => w.raise_toast(
                    ToastKind::Error,
                    crate::tr!(COULD_NOT_START; ; spec.argv.join(" ")),
                    e.message,
                    cx,
                ),
            });
        })
        .detach();
    }

    // ---- tasks (podd P3, docs/08) ----------------------------------------

    pub(crate) fn task(&self, id: &str) -> Option<&TaskVm> {
        self.tasks.iter().find(|t| t.id == id)
    }

    // ---- review (P4, docs/09 §7) -------------------------------------------

    pub(crate) fn live_reviews(&self) -> Vec<Entity<ReviewPanel>> {
        self.reviews.iter().filter_map(|w| w.upgrade()).collect()
    }

    /// Show the Review panel of `task_id` (focus the open one, else open it in the center).
    pub fn open_review(&mut self, task_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.caps.review {
            self.raise_toast(
                ToastKind::Warning,
                crate::strings::REVIEW_NEEDS_A_NEWER_PODD,
                crate::strings::PODD_PROTOCOL_V5_REVIEW_IS_REQUIRED,
                cx,
            );
            return;
        }
        let existing = self
            .live_reviews()
            .into_iter()
            .find(|r| r.read(cx).task_id == task_id);
        let panel = match existing {
            Some(p) => {
                let pid = PanelId::from(p.entity_id());
                self.dock
                    .update(cx, |a, cx| a.select_panel(pid, window, cx));
                p
            }
            None => {
                let Some(task) = self.task(task_id).cloned() else {
                    self.raise_toast(
                        ToastKind::Error,
                        crate::strings::NO_SUCH_TASK,
                        task_id.to_string(),
                        cx,
                    );
                    return;
                };
                let (source, ws) = (self.source.clone(), cx.entity().downgrade());
                let panel = cx.new(|cx| ReviewPanel::new(source, ws, &task, window, cx));
                // Seed the agent status for queue/turn hints.
                if let Some(s) = task
                    .terminals
                    .first()
                    .and_then(|t| self.terminal(t))
                    .and_then(|t| t.status.clone())
                {
                    let term = task.terminals[0].clone();
                    panel.update(cx, |r, cx| r.agent_status(&term, &s, cx));
                }
                self.reviews.push(panel.downgrade());
                self.dock.update(cx, |a, cx| {
                    a.add_panel_view(
                        panel_handle(panel.clone()),
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    )
                });
                panel
            }
        };
        let focus = panel.read(cx).focus_handle();
        window.focus(&focus, cx);
        self.touch(cx);
    }

    /// Ctrl+Shift+R: review the task of the focused terminal, else the only/first task.
    fn open_review_for_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let by_focus = self.focused_terminal.as_deref().and_then(|t| {
            self.tasks
                .iter()
                .find(|x| x.terminals.iter().any(|y| y == t))
        });
        match by_focus.or(self.tasks.first()).map(|t| t.id.clone()) {
            Some(id) => self.open_review(&id, window, cx),
            None => self.raise_toast(
                ToastKind::Info,
                crate::strings::NO_TASK_TO_REVIEW,
                crate::strings::CREATE_A_TASK_FIRST_TASK,
                cx,
            ),
        }
    }

    /// The Review panel of `task_id` on its PR tab (docs/15); `form` opens
    /// "브랜치 푸시 및 PR 만들기" when the task has no pull request yet.
    pub fn open_pr(
        &mut self,
        task_id: &str,
        form: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_review(task_id, window, cx);
        if let Some(r) = self
            .live_reviews()
            .into_iter()
            .find(|r| r.read(cx).task_id == task_id)
        {
            r.update(cx, |r, cx| r.show_pr(form, window, cx));
        }
    }

    pub fn review_removed(&mut self, panel: &WeakEntity<ReviewPanel>, cx: &mut Context<Self>) {
        self.reviews.retain(|w| w != panel && w.upgrade().is_some());
        self.touch(cx);
    }

    /// New Task: repo, name, base, agent, optional prompt -> `task.create`,
    /// then open the task's agent terminal.
    pub fn open_new_task_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let busy = Rc::new(Cell::new(false));
        let error = Rc::new(RefCell::new(String::new()));
        let repos = self.repos.clone();
        let repo = Rc::new(RefCell::new(repos.first().map(|r| r.name.clone())));
        let agent = Rc::new(Cell::new(AgentKind::Shell));
        let repo_path =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::strings::REPO_PATH));
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::TASK_NAME_E_G_FIX_LOGIN_BRANCH_POD_NAME)
        });
        let base = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::BASE_REF_DEFAULT_THE_REPO_S_DEFAULT_BRANCH)
        });
        let prompt = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::OPTIONAL_FIRST_PROMPT_CLAUDE_CODEX)
        });
        let ws = cx.entity().downgrade();
        let submit: crate::chrome::Submit = {
            let (busy, error, repo, agent) =
                (busy.clone(), error.clone(), repo.clone(), agent.clone());
            let (repo_path, name, base, prompt) = (
                repo_path.clone(),
                name.clone(),
                base.clone(),
                prompt.clone(),
            );
            Rc::new(move |window: &mut Window, cx: &mut App| {
                if busy.get() {
                    return;
                }
                let text = |e: &Entity<InputState>, cx: &App| {
                    let v = e.read(cx).value().trim().to_string();
                    (!v.is_empty()).then_some(v)
                };
                let Some(task_name) = text(&name, cx) else {
                    *error.borrow_mut() = crate::strings::ENTER_A_TASK_NAME.into();
                    window.refresh();
                    return;
                };
                let new_repo_path = text(&repo_path, cx);
                let repo_name = repo.borrow().clone();
                if repo_name.is_none() && new_repo_path.is_none() {
                    *error.borrow_mut() =
                        crate::strings::CHOOSE_A_REPOSITORY_OR_ENTER_ITS_PATH.into();
                    window.refresh();
                    return;
                }
                let kind = agent.get();
                let spec = TaskSpec {
                    repo: repo_name.clone().unwrap_or_default(),
                    name: task_name,
                    base: text(&base, cx),
                    agent: kind,
                    prompt: if kind == AgentKind::Shell {
                        None
                    } else {
                        text(&prompt, cx)
                    },
                    cols: None,
                    rows: None,
                };
                busy.set(true);
                error.borrow_mut().clear();
                window.refresh();
                let add = if repo_name.is_none() {
                    new_repo_path
                } else {
                    None
                };
                let _ = ws.update(cx, |w, cx| {
                    w.create_task(add, spec, busy.clone(), error.clone(), window, cx)
                });
            })
        };
        // Enter in any text field creates, like the button (keyboard-only flow).
        for input in [&repo_path, &name, &base, &prompt] {
            crate::chrome::submit_on_enter(input, submit.clone(), window, cx);
        }
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog: Dialog, window, cx| {
            if first.replace(false) {
                name.focus_handle(cx).focus(window, cx);
            }
            let label = |s: &'static str| {
                div()
                    .text_xs()
                    .text_color(rgb(crate::theme::tokens().text_muted))
                    .child(s)
            };
            let selected_repo = repo.borrow().clone();
            let mode = crate::theme::mode();
            // Choices are radio segments (docs/10 §D4.6); the one filled button is 만들기.
            let mut repo_row =
                pod_ui::radio_group("repo-group", crate::strings::REPOSITORY, mode).self_start();
            for (i, r) in repos.iter().enumerate() {
                let (cell, rname) = (repo.clone(), r.name.clone());
                let path: SharedString = r.path.clone().into();
                repo_row = repo_row.child(
                    pod_ui::choice(
                        ("repo", i),
                        r.name.clone(),
                        format!("{}: {} ({})", crate::strings::REPOSITORY, r.name, r.path),
                        None,
                        selected_repo.as_deref() == Some(r.name.as_str()),
                        true,
                        mode,
                        move |window, _| {
                            *cell.borrow_mut() = Some(rname.clone());
                            window.refresh();
                        },
                    )
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)
                    }),
                );
            }
            {
                let cell = repo.clone();
                repo_row = repo_row.child(pod_ui::segment(
                    "repo-new",
                    crate::strings::ADD_REPOSITORY,
                    Some(pod_ui::Icon::Plus),
                    selected_repo.is_none(),
                    mode,
                    move |window, _| {
                        *cell.borrow_mut() = None;
                        window.refresh();
                    },
                ));
            }
            let selected_agent = agent.get();
            let mut agent_row =
                pod_ui::radio_group("agent-group", crate::strings::AGENT, mode).self_start();
            for (i, (k, l)) in [
                (AgentKind::Shell, crate::strings::SHELL),
                (AgentKind::Claude, "Claude"),
                (AgentKind::Codex, "Codex"),
            ]
            .into_iter()
            .enumerate()
            {
                let cell = agent.clone();
                agent_row = agent_row.child(pod_ui::segment(
                    ("agent", i),
                    l,
                    None,
                    k == selected_agent,
                    mode,
                    move |window, _| {
                        cell.set(k);
                        window.refresh();
                    },
                ));
            }
            dialog
                .title(crate::strings::NEW_TASK)
                .w(px(560.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_color(rgb(crate::theme::tokens().danger_fg))
                                .child(error.borrow().clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label(crate::strings::REPOSITORY))
                                .child(repo_row),
                        )
                        .when(selected_repo.is_none(), |d| d.child(Input::new(&repo_path)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label(crate::strings::NAME))
                                .child(Input::new(&name)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label(crate::strings::BASE))
                                .child(Input::new(&base)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label(crate::strings::AGENT))
                                .child(agent_row),
                        )
                        .when(selected_agent != AgentKind::Shell, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(label(crate::strings::PROMPT))
                                    .child(Input::new(&prompt)),
                            )
                        }),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("task-cancel")
                                .label(crate::strings::CANCEL)
                                .disabled(busy.get())
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("task-create")
                                .label(if busy.get() {
                                    crate::strings::CREATING
                                } else {
                                    crate::strings::CREATE
                                })
                                .disabled(busy.get())
                                .primary()
                                .on_click({
                                    let submit = submit.clone();
                                    move |_, window, cx| submit(window, cx)
                                }),
                        ),
                )
        });
    }

    fn create_task(
        &mut self,
        add_repo: Option<String>,
        mut spec: TaskSpec,
        busy: Rc<Cell<bool>>,
        error: Rc<RefCell<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.clone();
        cx.spawn_in(window, async move |this, cx| {
            let fail = |e: crate::source::SourceError| e.message;
            if let Some(path) = add_repo {
                match source.add_repo(&path, None).await {
                    Ok(Ok(r)) => spec.repo = r.name,
                    Ok(Err(e)) => {
                        busy.set(false);
                        *error.borrow_mut() = fail(e);
                        let _ = this.update_in(cx, |_, window, _| window.refresh());
                        return;
                    }
                    Err(_) => {
                        busy.set(false);
                        *error.borrow_mut() = crate::strings::CONNECTION_CLOSED_WHILE_ADDING_REPOSITORY.into();
                        let _ = this.update_in(cx, |_, window, _| window.refresh());
                        return;
                    }
                }
            }
            let title = format!("{}/{}", spec.repo, spec.name);
            let result = source.create_task(spec).await;
            busy.set(false);
            let _ = this.update_in(cx, |w, window, cx| {
                window.refresh();
                match result {
                    Ok(Ok(task)) => {
                        window.close_dialog(cx);
                        w.raise_toast(ToastKind::Success, crate::tr!(TASK_CREATED; ; task.id), task.worktree.clone().unwrap_or_default(), cx);
                        if let Some(t) = task.terminals.first().cloned() {
                            w.tasks.retain(|x| x.id != task.id);
                            w.tasks.push(task);
                            w.open_terminal(&t, false, window, cx);
                        }
                    }
                    Ok(Err(e)) => {
                        *error.borrow_mut() = crate::tr!(COULD_NOT_CREATE_TASK_TITLE; title; fail(e));
                    }
                    Err(_) => {
                        *error.borrow_mut() = crate::strings::CONNECTION_CLOSED_WHILE_CREATING_TASK_CHECK_THE_TASK_LIST_BEFORE_.into();
                    }
                }
            });
        })
        .detach();
    }

    /// Add a shell terminal to a task (cwd = its worktree).
    pub(crate) fn task_terminal(
        &mut self,
        task: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run(
            RunSpec {
                argv: Program::Shell.argv(""),
                task: Some(task),
                ..Default::default()
            },
            window,
            cx,
        );
    }

    /// Task details (branch, base, ahead/behind, dirty, last commit) and removal.
    pub(crate) fn open_task_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = cx.entity().downgrade();
        let delete_branch = Rc::new(Cell::new(false));
        window.open_dialog(cx, move |dialog: Dialog, _, cx| {
            let review = ws.upgrade().is_some_and(|w| w.read(cx).caps.review);
            let Some(task) = ws.upgrade().and_then(|w| w.read(cx).task(&id).cloned()) else {
                return dialog.title(crate::tr!(TASK_ID; id;)).child(crate::strings::THIS_TASK_NO_LONGER_EXISTS);
            };
            let row = |k: &'static str, v: String| {
                div()
                    .flex()
                    .gap_3()
                    .child(div().w(px(90.)).text_color(rgb(crate::theme::tokens().text_muted)).child(crate::strings::field_label(k)))
                    .child(div().flex_1().min_w_0().overflow_hidden().child(v))
            };
            let git = task.git.clone().unwrap_or_default();
            let del = delete_branch.get();
            let (ws_rm, id_rm, del_cell) = (ws.clone(), id.clone(), delete_branch.clone());
            let cell = delete_branch.clone();
            dialog
                .title(crate::tr!(TASK; ; task.id))
                .w(px(600.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_sm()
                        .child(row("branch", task.branch.clone().unwrap_or_default()))
                        .child(row("base", task.base.clone().unwrap_or_default()))
                        .child(row("agent", task.agent.clone().unwrap_or_default()))
                        .child(row(
                            "worktree",
                            format!("{}{}", task.worktree.clone().unwrap_or_default(), if task.worktree_exists { "" } else { crate::strings::MISSING }),
                        ))
                        .child(row("terminals", task.terminals.join(", ")))
                        .child(row(
                            "git",
                            match &task.git_error {
                                Some(e) => crate::tr!(UNAVAILABLE_E; e;),
                                None => crate::tr!(AHEAD_BEHIND_DIRTY_UNPUSHED; ; git.ahead, git.behind, git.dirty, git.unpushed),
                            },
                        ))
                        .child(row(crate::strings::LAST_COMMIT, git.last_commit.clone().unwrap_or_else(|| "—".into()))),
                )
                .footer(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("task-delbranch")
                                .label(if del { crate::strings::DELETE_BRANCH } else { crate::strings::DELETE_BRANCH_94 })
                                .ghost()
                                .on_click(move |_, window, _| {
                                    cell.set(!cell.get());
                                    window.refresh();
                                }),
                        )
                        .child(div().flex_1())
                        .when(review, |d| {
                            let (ws_rv, id_rv) = (ws_rm.clone(), id_rm.clone());
                            d.child(Button::new("task-review").label(crate::strings::REVIEW_CHANGES).primary().on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = ws_rv.update(cx, |w, cx| w.open_review(&id_rv, window, cx));
                            }))
                        })
                        .child(Button::new("task-close").label(crate::strings::CLOSE).on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("task-remove").label(crate::strings::REMOVE_TASK).danger().on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = ws_rm.update(cx, |w, cx| w.request_remove_task(id_rm.clone(), del_cell.get(), window, cx));
                        })),
                )
        });
    }

    /// `task.remove`; a `TASK_DIRTY` refusal is shown with a "Remove anyway" choice.
    fn remove_task(
        &mut self,
        id: String,
        force: bool,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = self.source.remove_task(&id, force, delete_branch);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |w, window, cx| match result {
                Ok(terminals) => {
                    for t in &terminals {
                        w.close_panels_of(t, window, cx);
                    }
                    w.close_editor_of(&id, window, cx);
                    w.tasks.retain(|t| t.id != id);
                    w.raise_toast(
                        ToastKind::Success,
                        crate::tr!(TASK_ID_REMOVED; id;),
                        crate::tr!(TERMINALS_100; ; terminals.join(", ")),
                        cx,
                    );
                }
                Err(e) if e.code == Some(codes::TASK_DIRTY) => {
                    w.confirm_force_remove(id.clone(), delete_branch, e.message, window, cx)
                }
                Err(e) => w.raise_toast(
                    ToastKind::Error,
                    crate::tr!(COULD_NOT_REMOVE_ID; id;),
                    e.message,
                    cx,
                ),
            });
        })
        .detach();
    }

    fn confirm_force_remove(
        &mut self,
        id: String,
        delete_branch: bool,
        reason: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let (ws_ok, id_ok) = (ws.clone(), id.clone());
            dialog
                .title(crate::strings::PODD_REFUSED_TO_REMOVE_THE_TASK)
                .w(px(520.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .child(div().text_color(rgb(crate::theme::tokens().warning_fg)).child(reason.clone()))
                        .child(crate::strings::REMOVING_ANYWAY_DELETES_THE_WORKTREE_WITH_ITS_UNCOMMITTED_CHANGES),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(Button::new("force-cancel").label(crate::strings::KEEP_TASK).on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("force-ok").label(crate::strings::REMOVE_ANYWAY).danger().on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = ws_ok.update(cx, |w, cx| w.remove_task(id_ok.clone(), true, delete_branch, window, cx));
                        })),
                )
        });
    }

    // ---- files and editor (P3, docs/13 §2) --------------------------------

    pub(crate) fn live_editors(&self) -> Vec<Entity<EditorPanel>> {
        self.editors.iter().filter_map(|w| w.upgrade()).collect()
    }

    fn editor_of(&self, task: &str, cx: &App) -> Option<Entity<EditorPanel>> {
        self.live_editors()
            .into_iter()
            .find(|e| e.read(cx).task_id == task)
    }

    /// The task the user is working in: an editor or Review panel holding
    /// focus, else the focused terminal's task, else the last editor used,
    /// else the first task with a worktree.
    pub(crate) fn current_task(&self, window: &Window, cx: &App) -> Option<String> {
        self.focused_task(window, cx).or_else(|| {
            self.tasks
                .iter()
                .find(|t| t.worktree.is_some())
                .map(|t| t.id.clone())
        })
    }

    /// Current panel context, retained while chrome has focus. Unlike command
    /// defaults, the status bar must not invent a context from the first task.
    fn focused_task(&self, window: &Window, cx: &App) -> Option<String> {
        for e in self.live_editors() {
            let e = e.read(cx);
            if e.editor
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
            {
                return Some(e.task_id.clone());
            }
        }
        for r in self.live_reviews() {
            let r = r.read(cx);
            if r.focus_handle().contains_focused(window, cx) {
                return Some(r.task_id.clone());
            }
        }
        let by_terminal = self.focused_terminal.as_deref().and_then(|t| {
            self.tasks.iter().find(|x| {
                x.terminals.iter().any(|y| y == t)
                    || self.terminal(t).and_then(|v| v.task_id.as_deref()) == Some(x.id.as_str())
            })
        });
        by_terminal.map(|t| t.id.clone()).or_else(|| {
            self.last_editor
                .clone()
                .filter(|id| self.task(id).is_some())
        })
    }

    /// Show the files/editor panel of `task_id` (open it if needed), then
    /// optionally open `rel` at 1-based `line`/`col` or show quick open.
    pub fn open_editor(
        &mut self,
        task_id: &str,
        target: Option<(String, u32, u32)>,
        quick_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = match self.editor_of(task_id, cx) {
            Some(p) => {
                let pid = PanelId::from(p.entity_id());
                self.dock
                    .update(cx, |a, cx| a.select_panel(pid, window, cx));
                p
            }
            None => {
                let Some(task) = self.task(task_id).cloned() else {
                    self.raise_toast(
                        ToastKind::Error,
                        crate::strings::NO_SUCH_TASK,
                        task_id.to_string(),
                        cx,
                    );
                    return;
                };
                let root = task
                    .worktree
                    .clone()
                    .filter(|w| !w.is_empty() && std::path::Path::new(w).is_dir());
                let Some(root) = root else {
                    self.raise_toast(
                        ToastKind::Warning,
                        crate::tr!(NO_WORKTREE_FOR_FILES; ; task.id),
                        task.worktree.clone().unwrap_or_default(),
                        cx,
                    );
                    return;
                };
                let ws = cx.entity().downgrade();
                let panel = cx.new(|cx| {
                    EditorPanel::new(
                        task.id.clone(),
                        std::path::PathBuf::from(root),
                        None,
                        ws,
                        window,
                        cx,
                    )
                });
                self.editors.push(panel.downgrade());
                self.dock.update(cx, |a, cx| {
                    a.add_panel_view(
                        panel_handle(panel.clone()),
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    )
                });
                panel
            }
        };
        self.last_editor = Some(task_id.to_string());
        let editor = panel.read(cx).editor.clone();
        let err = editor.update(cx, |e, cx| {
            if let Some((rel, line, col)) = &target {
                let err = e
                    .goto(std::path::Path::new(rel), *line, *col, window, cx)
                    .err()
                    .map(|err| format!("{rel}: {err}"));
                // goto only moves the cursor: put keyboard focus in the editor too.
                if let Some(st) = e.active_editor() {
                    st.update(cx, |s, cx| s.focus(window, cx));
                }
                return err;
            }
            if quick_open {
                e.open_quick_open(window, cx);
            } else {
                e.focus_handle(cx).focus(window, cx);
            }
            None
        });
        if let Some(e) = err {
            self.raise_toast(ToastKind::Error, crate::strings::OPEN_FILES, e, cx);
        }
        self.touch(cx);
    }

    /// Ctrl+P / palette: quick open in the current task's worktree.
    pub(crate) fn quick_open_files(
        &mut self,
        quick_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.current_task(window, cx) {
            Some(id) => self.open_editor(&id, None, quick_open, window, cx),
            None => self.raise_toast(
                ToastKind::Info,
                crate::strings::NO_TASK_FOR_FILES,
                crate::strings::PICK_A_TASK_FIRST,
                cx,
            ),
        }
    }

    pub(crate) fn save_all_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for e in self.live_editors() {
            let ed = e.read(cx).editor.clone();
            ed.update(cx, |e, cx| e.save_all(window, cx));
        }
    }

    /// Unsaved files across all editors, as `task: path` labels.
    pub(crate) fn unsaved_files(&self, cx: &App) -> Vec<String> {
        let mut out = Vec::new();
        for e in self.live_editors() {
            let e = e.read(cx);
            for p in e.editor.read(cx).dirty_paths() {
                out.push(format!(
                    "{} · {}",
                    e.task_id,
                    crate::editor_panel::rel(&e.root, &p)
                ));
            }
        }
        out
    }

    pub(crate) fn editor_layout_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_save(cx);
        self.touch(cx);
    }

    /// A file was saved in a task's editor: its Review diff is stale.
    pub(crate) fn editor_saved(&mut self, task: &str, rel: Option<String>, cx: &mut Context<Self>) {
        // v9: podd recomputes git now and announces `task.changed touched`.
        self.source.touch_task(task, rel.into_iter().collect());
        for r in self.live_reviews() {
            if r.read(cx).task_id == task {
                r.update(cx, |r, cx| r.refresh_diff(cx));
            }
        }
    }

    pub fn editor_removed(&mut self, panel: &WeakEntity<EditorPanel>, cx: &mut Context<Self>) {
        self.editors.retain(|w| w != panel && w.upgrade().is_some());
        self.enforce_lsp_cap(cx);
        self.schedule_save(cx);
        self.touch(cx);
    }

    /// v9 `task.removing`: podd is about to delete this task's worktree
    /// (from the app or `pod task remove`). Unsaved edits are saved: podd
    /// re-checks after its grace period and then stops with `TASK_DIRTY`,
    /// so nothing typed is lost. A clean editor is closed (its language
    /// servers release the folder). Removals the user already confirmed in
    /// the app (with "저장하지 않고") only close.
    pub(crate) fn task_removing(
        &mut self,
        id: &str,
        worktree: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(e) = self.editor_of(id, cx) else {
            return;
        };
        let ed = e.read(cx).editor.clone();
        let dirty = ed.read(cx).dirty_paths();
        let confirmed = self.removal_confirmed.remove(id);
        if dirty.is_empty() || confirmed {
            self.close_editor_of(id, window, cx);
            return;
        }
        ed.update(cx, |e, cx| e.save_all(window, cx));
        self.raise_toast(
            ToastKind::Warning,
            crate::strings::TASK_REMOVING_SAVED,
            crate::tr!(TASK_REMOVING_SAVED_BODY; ; id, dirty.len(), worktree),
            cx,
        );
    }

    /// Task removed: close its editor (stops its language servers).
    pub(crate) fn close_editor_of(
        &mut self,
        task: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.editor_of(task, cx) {
            self.dock.update(cx, |a, cx| a.remove_panel(p, window, cx));
        }
    }

    /// Ctrl+Shift+T: a new terminal, except inside a files panel, where it
    /// reopens the last closed editor tab (docs/13 §9). Context-free
    /// bindings outrank the editor's `PodEditor` binding in GPUI, so the
    /// choice is made here.
    pub(crate) fn new_terminal_or_reopen_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused = self
            .live_editors()
            .into_iter()
            .map(|e| e.read(cx).editor.clone())
            .find(|ed| ed.read(cx).focus_handle(cx).contains_focused(window, cx));
        match focused {
            Some(ed) => {
                ed.update(cx, |e, cx| e.reopen_closed(window, cx));
            }
            None => self.open_new_dialog(window, cx),
        }
    }

    /// Theme switched: re-skin editors (the global theme is already applied).
    pub(crate) fn refresh_editor_theme(&mut self, cx: &mut Context<Self>) {
        for e in self.live_editors() {
            let ed = e.read(cx).editor.clone();
            ed.update(cx, |e, cx| e.set_mode(crate::theme::mode(), cx));
        }
    }

    /// Keep at most [`lsp_cap`] language servers running across all task
    /// editors; suspend the least recently used ones. Called whenever a
    /// store's servers change.
    pub fn enforce_lsp_cap(&mut self, cx: &mut Context<Self>) {
        let mut all = Vec::new();
        for e in self.live_editors() {
            let (task, store) = (
                e.read(cx).task_id.clone(),
                e.read(cx).editor.read(cx).lsp().clone(),
            );
            for r in store.read(cx).running_servers() {
                all.push((task.clone(), store.clone(), r));
            }
        }
        let cap = lsp_cap();
        if all.len() > cap {
            all.sort_by_key(|(_, _, r)| r.last_used);
            let excess = all.len() - cap;
            for (task, store, r) in all.into_iter().take(excess) {
                store.update(cx, |s, cx| s.suspend(&r.key, cx)).detach();
                self.raise_toast(
                    ToastKind::Info,
                    crate::strings::LSP_SUSPENDED,
                    crate::tr!(LSP_SUSPENDED_BODY; ; cap, r.key.id, task),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Status-bar text for language servers: the server of the active file in
    /// the editor used last (short label), plus how many others run.
    fn lsp_status(&self, cx: &App) -> Option<(String, u32)> {
        let editors = self.live_editors();
        if let Some((store, _)) = self.lsp_target(cx)
            && !store.read(cx).is_enabled()
        {
            return Some((
                crate::strings::LSP_OFF_LABEL.into(),
                crate::theme::tokens().text_muted,
            ));
        }
        let mut running = 0usize;
        let mut main: Option<pod_editor::lsp::ServerStatus> = None;
        for e in &editors {
            let e = e.read(cx);
            let ed = e.editor.read(cx);
            let store = ed.lsp().read(cx);
            running += store.running_servers().len();
            if self.last_editor.as_deref() == Some(e.task_id.as_str())
                && let Some(p) = ed.active_path()
            {
                main = store.status_for(p).or(main);
            }
        }
        if main.is_none() {
            main = editors.iter().find_map(|e| {
                e.read(cx)
                    .editor
                    .read(cx)
                    .lsp()
                    .read(cx)
                    .statuses()
                    .into_iter()
                    .find(|s| s.state.is_running())
            });
        }
        let t = crate::theme::tokens();
        let main = main?;
        use pod_editor::lsp::ServerState as S;
        let color = match (&main.state, &main.progress) {
            (S::Running, None) => t.success_fg,
            (S::Running, Some(_)) | (S::Starting | S::Checking, _) => t.info_fg,
            (S::Restarting { .. }, _) => t.warning_fg,
            (S::Failed(_), _) => t.danger_fg,
            _ => t.text_muted,
        };
        let others = running.saturating_sub(usize::from(
            main.state.is_running()
                || matches!(main.state, S::Starting | S::Checking | S::Restarting { .. }),
        ));
        let label = main.short_label();
        Some((
            if others > 0 {
                crate::tr!(LSP_MORE; ; label, others)
            } else {
                label
            },
            color,
        ))
    }

    /// The language-server store the status bar acts on: the editor used
    /// last (else the first open one) and its active file.
    pub(crate) fn lsp_target(
        &self,
        cx: &App,
    ) -> Option<(
        Entity<pod_editor::lsp::LspStore>,
        Option<std::path::PathBuf>,
    )> {
        let editors = self.live_editors();
        let e = editors
            .iter()
            .find(|e| self.last_editor.as_deref() == Some(e.read(cx).task_id.as_str()))
            .or(editors.first())?;
        let ed = e.read(cx).editor.read(cx);
        Some((ed.lsp().clone(), ed.active_path().map(|p| p.to_path_buf())))
    }

    /// Status-bar menu: 다시 시작 (also turns a stopped store back on).
    pub(crate) fn lsp_restart(&mut self, cx: &mut Context<Self>) {
        let Some((store, path)) = self.lsp_target(cx) else {
            return;
        };
        store.update(cx, |s, cx| {
            if !s.is_enabled() {
                s.set_enabled(true, cx);
            } else if let Some(p) = &path {
                s.restart(p, cx);
            }
        });
        self.enforce_lsp_cap(cx);
    }

    /// Status-bar menu: 끄기 (this task's editor; 다시 시작 turns it on).
    pub(crate) fn lsp_turn_off(&mut self, cx: &mut Context<Self>) {
        let Some((store, _)) = self.lsp_target(cx) else {
            return;
        };
        store.update(cx, |s, cx| s.set_enabled(false, cx));
        self.raise_toast(
            ToastKind::Info,
            crate::strings::LSP_TURNED_OFF,
            crate::strings::LSP_TURNED_OFF_BODY,
            cx,
        );
        cx.notify();
    }

    /// Status-bar menu: 로그 보기.
    pub(crate) fn lsp_show_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((store, path)) = self.lsp_target(cx) else {
            return;
        };
        let text = store.read(cx).server_log(path.as_deref());
        let text = if text.trim().is_empty() {
            crate::strings::LSP_LOG_EMPTY.to_string()
        } else {
            text
        };
        let font = cx.global::<crate::theme::Settings>().terminal_font.clone();
        window.open_dialog(cx, move |d: Dialog, _, _| {
            d.title(crate::strings::LSP_LOG_TITLE).w(px(760.)).child(
                div()
                    .id("lsp-log")
                    .role(Role::Document)
                    .aria_label(crate::strings::LSP_LOG_TITLE)
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .font_family(font.clone())
                    .text_xs()
                    .children(
                        text.lines()
                            .map(|l| div().child(l.to_string()))
                            .collect::<Vec<_>>(),
                    ),
            )
        });
    }

    /// Task removal from the details dialog: unsaved editor files ask first.
    pub(crate) fn request_remove_task(
        &mut self,
        id: String,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dirty: Vec<String> = self
            .editor_of(&id, cx)
            .map(|e| {
                let e = e.read(cx);
                e.editor
                    .read(cx)
                    .dirty_paths()
                    .iter()
                    .map(|p| crate::editor_panel::rel(&e.root, p))
                    .collect()
            })
            .unwrap_or_default();
        if dirty.is_empty() {
            self.remove_task(id, false, delete_branch, window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        let body = crate::tr!(REMOVE_TASK_UNSAVED_BODY; ; id, dirty.len(), dirty.join(", "));
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let (ws_ok, id_ok) = (ws.clone(), id.clone());
            dialog
                .title(crate::strings::UNSAVED_FILES_TITLE)
                .w(px(520.))
                .child(div().text_sm().child(body.clone()))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("rm-unsaved-cancel")
                                .label(crate::strings::KEEP_TASK)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("rm-unsaved-ok")
                                .label(crate::strings::REMOVE_WITHOUT_SAVING)
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = ws_ok.update(cx, |w, cx| {
                                        w.removal_confirmed.insert(id_ok.clone());
                                        w.remove_task(
                                            id_ok.clone(),
                                            false,
                                            delete_branch,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        ),
                )
        });
    }

    /// Window close with unsaved editor files: ask (save all / discard / cancel).
    /// Returns whether the window may close now.
    pub(crate) fn confirm_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let unsaved = self.unsaved_files(cx);
        if self.quit_confirmed || unsaved.is_empty() {
            return true;
        }
        let ws = cx.entity().downgrade();
        let body = crate::tr!(UNSAVED_FILES_BODY; ; unsaved.len(), unsaved.join(", "));
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let (ws_save, ws_quit) = (ws.clone(), ws.clone());
            dialog
                .title(crate::strings::UNSAVED_FILES_TITLE)
                .w(px(560.))
                .child(div().text_sm().child(body.clone()))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("quit-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("quit-discard")
                                .label(crate::strings::QUIT_WITHOUT_SAVING)
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = ws_quit.update(cx, |w, _| w.quit_confirmed = true);
                                    cx.quit();
                                }),
                        )
                        .child(
                            Button::new("quit-save")
                                .label(crate::strings::SAVE_ALL_AND_QUIT)
                                .primary()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let ok = ws_save
                                        .update(cx, |w, cx| {
                                            w.save_all_editors(window, cx);
                                            let left = w.unsaved_files(cx).is_empty();
                                            w.quit_confirmed = left;
                                            left
                                        })
                                        .unwrap_or(true);
                                    if ok {
                                        cx.quit();
                                    }
                                }),
                        ),
                )
        });
        false
    }

    /// App quit: persist the layout (editor cursors included) and stop every
    /// language server (Job Objects kill the trees even if this is cut short).
    pub(crate) fn on_quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.restored {
            self.save_now(cx);
        }
        let mut tasks: Vec<Task<()>> = Vec::new();
        for e in self.live_editors() {
            let ed = e.read(cx).editor.clone();
            tasks.push(ed.update(cx, |e, cx| e.shutdown(cx)));
        }
        cx.background_spawn(async move {
            futures::future::join_all(tasks).await;
        })
    }

    // ---- notifications --------------------------------------------------

    /// Show a toast in the bottom-right corner for a few seconds.
    pub fn raise_toast(
        &mut self,
        kind: ToastKind,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let title = title.into();
        let body = body.into();
        if self
            .toasts
            .iter()
            .any(|t| t.visible && t.kind == kind && t.title == title && t.body == body)
        {
            return;
        }
        let id = self.next_toast;
        self.next_toast += 1;
        let paused = Rc::new(Cell::new(false));
        self.toasts.push(Toast {
            id,
            kind,
            title,
            body,
            visible: true,
            paused: paused.clone(),
            terminal_request: None,
            at: crate::orch::now_ms(),
        });
        if self.toasts.len() > 100 {
            self.toasts.remove(0);
        }
        if matches!(kind, ToastKind::Info | ToastKind::Success) {
            let focus_paused = self.toast_focus_paused.clone();
            let ttl = if kind == ToastKind::Success { 5. } else { 7. };
            cx.spawn(async move |this, cx| {
                let mut elapsed = 0.;
                while elapsed < ttl {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    if !paused.get() && !focus_paused.get() {
                        elapsed += 0.25;
                    }
                    if this.upgrade().is_none() {
                        return;
                    }
                }
                let _ = this.update(cx, |w, cx| {
                    if let Some(t) = w.toasts.iter_mut().find(|t| t.id == id) {
                        t.visible = false;
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Titles of the toasts still on screen (tests).
    #[cfg(test)]
    pub(crate) fn visible_toasts(&self) -> Vec<String> {
        self.toasts
            .iter()
            .filter(|t| t.visible)
            .map(|t| t.title.to_string())
            .collect()
    }
    #[cfg(test)]
    pub(crate) fn toast_focus_handle(&self) -> FocusHandle {
        self.toast_focus.clone()
    }

    // ---- rendering ------------------------------------------------------

    fn render_sidebar(&self, ws: &WeakEntity<Self>, list: AnyElement) -> impl IntoElement {
        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        // Icon buttons keep the earlier text labels as accessible names (UIA/QA unchanged).
        let button = |id: &'static str,
                      glyph: pod_ui::Icon,
                      name: &'static str,
                      tip: &'static str,
                      f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let ws = ws.clone();
            pod_ui::IconButton::new(id, glyph, name)
                .tooltip(tip)
                .render(mode, move |window, cx| {
                    let _ = ws.update(cx, |w, cx| f(w, window, cx));
                })
        };
        div()
            .qa_id(|| "sidebar".into())
            .w(px(m.density.sidebar))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_chrome))
            .when(m.surface == pod_ui::themes::Surface::Flat, |d| d.border_r(px(m.border_width)).border_color(rgb(t.border_subtle)))
            .child(
                // One compact header row (docs/10 §D4.6): title + icon actions, never wrapping.
                div()
                    .h(px(m.density.panel_header.max(32.)))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_3()
                    .pr_1()
                    .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(crate::strings::YOUR_WORKSPACE))
                    .child(
                        div()
                            .qa_id(|| "sidebar-actions".into())
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(2.))
                            .child(button("grid", pod_ui::Icon::LayoutGrid, crate::strings::GRID, crate::strings::ARRANGE_OPEN_PANELS_IN_A_GRID_CTRL_SHIFT_G, |w, window, cx| w.arrange_grid(window, cx)))
                            .when(self.caps.tasks, |d| {
                                d.child(button("new-task", pod_ui::Icon::Plus, crate::strings::TASK_107, crate::strings::NEW_TASK_GIT_WORKTREE_AGENT_TERMINAL, |w, window, cx| w.open_new_task_dialog(window, cx)))
                            })
                            .child(button("new", pod_ui::Icon::SquareTerminal, crate::strings::NEW, crate::strings::NEW_TERMINAL_CTRL_SHIFT_T, |w, window, cx| w.open_new_dialog(window, cx)))
                            .when(self.caps.orch, |d| {
                                d.child(button("new-group", pod_ui::Icon::Users, crate::strings::GROUP_330, crate::strings::FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS_CTRL_SHIFT_N, |w, window, cx| w.open_new_group_dialog(window, cx)))
                                    .child(button("agent-tree", pod_ui::Icon::ListTree, crate::strings::AGENT, crate::strings::AGENT_TREE_GROUPS_TASKS_AND_TERMINALS_STARTED_FROM_OTHER_TERMINAL, |w, window, cx| w.open_agent_tree(window, cx)))
                            }),
                    ),
            )
            .child(list)
    }

    fn render_row(&self, t: &TerminalVm, ws: &WeakEntity<Self>) -> impl IntoElement {
        let mut badge = crate::orch::terminal_badge(t);
        if !self.conn.is_connected() {
            badge.stale_since = Some(crate::strings::CONNECTION_LOST.into());
        }
        let detail = terminal_row_detail(t, &badge);
        let id = t.id.clone();
        let open_ws = ws.clone();
        let menu_ws = ws.clone();
        let selected = self.focused_terminal.as_deref() == Some(&t.id);
        let tk = crate::theme::tokens();
        let agent = crate::agent_kind::identify_in(t, &self.tasks).kind;
        // Under a task: chevron slot (16) + gap; loose terminals sit at the task level.
        let indent = if self.tasks.iter().any(|task| task.terminals.contains(&t.id)) {
            34.
        } else {
            12.
        };
        div()
            .id(SharedString::from(format!("row-{id}")))
            .qa_id({
                let id = id.clone();
                move || format!("row-{id}")
            })
            .role(Role::TreeItem)
            .aria_label(format!(
                "{} · {} | {detail}",
                self.label_of(t),
                agent.accessible_name()
            ))
            .w_full()
            .h_full()
            .flex()
            .items_center()
            .gap_2()
            .pl(px(indent))
            .pr_2()
            .min_w_0()
            .border_l_2()
            .border_color(if selected {
                rgb(tk.accent_fg)
            } else {
                gpui_kit::transparent_black().into()
            })
            .when(selected, |d| d.bg(rgb(tk.bg_selected)))
            .hover(|s| {
                s.bg(rgb(if selected {
                    tk.bg_selected
                } else {
                    tk.bg_hover
                }))
            })
            // Worker row (docs/10 §D4.6, §D5.11): agent mark + name + status icon and text on the right.
            .child(crate::agent_kind::mark_el(
                &format!("row-{id}"),
                &agent,
                pod_ui::tokens::ICON_SIZE,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(if selected {
                        tk.text_primary
                    } else {
                        tk.text_secondary
                    }))
                    .child(self.label_of(t)),
            )
            .child(badge.render_compact(crate::theme::mode()))
            .on_click(move |_, window, cx| {
                let _ = open_ws.update(cx, |w, cx| w.open_terminal(&id, false, window, cx));
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(detail.clone()).build(window, cx)
            })
            .context_menu({
                let id = t.id.clone();
                let running = t.is_running();
                move |menu, _, _| {
                    let ws = menu_ws.clone();
                    let id = id.clone();
                    menu.item(
                        PopupMenuItem::new(if running {
                            crate::strings::KILL_TERMINAL_112
                        } else {
                            crate::strings::REMOVE_EXITED_TERMINAL
                        })
                        .on_click(move |_, window, cx| {
                            let _ = ws.update(cx, |w, cx| {
                                if running {
                                    w.confirm_kill(id.clone(), window, cx)
                                } else {
                                    w.remove(id.clone(), window, cx)
                                }
                            });
                        }),
                    )
                }
            })
    }

    pub(crate) fn status_branch(&self, window: &Window, cx: &App) -> Option<&str> {
        self.focused_task(window, cx)
            .and_then(|id| self.task(&id))
            .and_then(|task| task.branch.as_deref())
            .filter(|name| !name.is_empty())
    }

    fn render_status_bar(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = vm::counts(&self.terminals);
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let (conn_text, conn_detail, conn_color, conn_icon) = match &self.conn {
            ConnState::Connecting => (
                crate::strings::PODD_CONNECTING.to_string(),
                None,
                tk.warning_fg,
                pod_ui::Icon::Plug,
            ),
            // PID/version stay out of the bar (docs/10 §D4.6): tooltip and accessible name.
            ConnState::Connected { pid, version } => (
                crate::strings::PODD_CONNECTED.to_string(),
                Some(format!("podd {version} · pid {pid}")),
                tk.success_fg,
                pod_ui::Icon::Plug,
            ),
            ConnState::Reconnecting { attempt, .. } => (
                crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT; attempt;),
                None,
                tk.warning_fg,
                pod_ui::Icon::Unplug,
            ),
        };
        let panels = self.live_panels().len();
        let plan = StatusPlan::for_width(
            f32::from(window.viewport_size().width),
            crate::layout_mode::status_counts(cx),
            self.caps.status,
        );
        let branch = self.status_branch(window, cx);
        let terminals = crate::tr!(TERMINALS_RUNNING_PANELS; ; c.total, c.running, panels);
        // The whole summary as one status (screen readers do not see the painted text).
        let mut summary = vec![
            conn_detail
                .clone()
                .map_or(conn_text.clone(), |d| format!("{conn_text} ({d})")),
            terminals.clone(),
        ];
        if self.caps.status {
            summary.extend([
                crate::tr!(WORKING; ; c.working),
                crate::tr!(BLOCKED; ; c.blocked),
                crate::tr!(DONE; ; c.done),
            ]);
        }
        if self.caps.tasks {
            summary.push(crate::tr!(TASKS; ; self.tasks.len()));
        }
        // Grouped items (docs/10 §D4.6): icon + text, 1×12 separators between groups.
        let sep = || {
            div()
                .w(px(1.))
                .h(px(12.))
                .flex_shrink_0()
                .bg(rgb(tk.border_subtle))
        };
        let item = |glyph: pod_ui::Icon, fg: u32, text: String| {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .child(pod_ui::icon_sized(glyph, 14., fg))
                .child(div().text_color(rgb(fg)).child(text))
        };
        let conn_tip: SharedString = conn_detail
            .map_or(conn_text.clone(), |d| {
                format!("{conn_text} · {d} · {terminals}")
            })
            .into();
        // Left: connection and counts. It may clip (items drop by `plan` first); the right
        // group (상태 · 알림) never shrinks, so it stays reachable at the 800 px minimum.
        let left = div()
            .id("status-summary")
            .qa_id(|| "status-summary".into())
            .role(Role::Status)
            .aria_label(summary.join(" · "))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap_3()
            .child(
                item(conn_icon, conn_color, conn_text)
                    .id("status-conn")
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(conn_tip.clone())
                            .build(window, cx)
                    }),
            )
            .when(plan.terminals, |d| {
                d.child(sep()).child(item(
                    pod_ui::Icon::SquareTerminal,
                    tk.text_secondary,
                    terminals.clone(),
                ))
            })
            .when(plan.counts, |d| {
                d.child(sep())
                    .when(plan.working, |d| {
                        d.child(item(
                            pod_ui::Icon::LoaderCircle,
                            tk.info_fg,
                            crate::tr!(WORKING; ; c.working),
                        ))
                    })
                    .child(
                        div()
                            .qa_id(|| "status-blocked".into())
                            .flex_shrink_0()
                            .child(item(
                                pod_ui::Icon::ShieldQuestion,
                                tk.warning_fg,
                                crate::tr!(BLOCKED; ; c.blocked),
                            )),
                    )
                    .when(plan.done, |d| {
                        d.child(item(
                            pod_ui::Icon::CircleCheck,
                            tk.success_fg,
                            crate::tr!(DONE; ; c.done),
                        ))
                    })
            })
            .when(self.caps.tasks && plan.tasks, |d| {
                d.child(item(
                    pod_ui::Icon::Layers,
                    tk.text_secondary,
                    crate::tr!(TASKS; ; self.tasks.len()),
                ))
            })
            .when_some(branch.filter(|_| plan.branch), |d, branch| {
                let name = format!("{}: {branch}", crate::strings::CURRENT_BRANCH);
                let tip: SharedString = name.clone().into();
                d.child(sep()).child(
                    div()
                        .id("status-branch")
                        .qa_id(|| "status-branch".into())
                        .role(Role::Status)
                        .aria_label(name)
                        .h(px(24.))
                        .max_w(px(180.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(pod_ui::icon_sized(
                            pod_ui::Icon::GitBranch,
                            14.,
                            tk.text_secondary,
                        ))
                        .child(div().min_w_0().truncate().child(branch.to_string()))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                .build(window, cx)
                        }),
                )
            })
            .when_some(self.lsp_status(cx), |d, (label, color)| {
                let ws = cx.entity().downgrade();
                d.child(sep()).child(
                    div()
                        .qa_id(|| "status-lsp".into())
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .child(div().size(px(7.)).rounded_full().bg(rgb(color)))
                        .child(
                            Button::new("status-lsp")
                                .label(label)
                                .tooltip(crate::strings::LSP_STATUS_TOOLTIP)
                                .xsmall()
                                .compact()
                                .dropdown_menu(move |menu, _, _| {
                                    let item = |menu: gpui_kit::component::menu::PopupMenu,
                                                label: &'static str,
                                                f: fn(
                                        &mut Self,
                                        &mut Window,
                                        &mut Context<Self>,
                                    )| {
                                        let ws = ws.clone();
                                        menu.item(
                                            gpui_kit::component::menu::PopupMenuItem::new(label)
                                                .on_click(move |_, window, cx| {
                                                    let _ = ws.update(cx, |w, cx| f(w, window, cx));
                                                }),
                                        )
                                    };
                                    let menu =
                                        item(menu, crate::strings::LSP_MENU_RESTART, |w, _, cx| {
                                            w.lsp_restart(cx)
                                        });
                                    let menu =
                                        item(menu, crate::strings::LSP_MENU_OFF, |w, _, cx| {
                                            w.lsp_turn_off(cx)
                                        });
                                    item(menu, crate::strings::LSP_MENU_LOG, |w, window, cx| {
                                        w.lsp_show_log(window, cx)
                                    })
                                }),
                        ),
                )
            })
            .when(self.caps.orch, |d| {
                d.child(sep()).child(
                    div()
                        .flex_shrink_0()
                        .child(crate::orch::status_bar_gates(self, cx)),
                )
            })
            .child(div().flex_1())
            .when_some(self.stats_line.clone().filter(|_| plan.stats), |d, s| {
                d.child(div().text_color(rgb(tk.text_muted)).truncate().child(s))
            });
        div()
            .qa_id(|| "status-bar".into())
            .h(px(m.density.statusbar))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_3()
            .overflow_hidden()
            .text_xs()
            .text_color(rgb(tk.text_secondary))
            .bg(rgb(tk.bg_chrome))
            .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
                d.border_t(px(m.border_width))
                    .border_color(rgb(tk.border_subtle))
            })
            .child(left)
            .child(
                div()
                    .qa_id(|| "status-inspect".into())
                    .flex_shrink_0()
                    .child(
                        Button::new("status-inspect")
                            .label(crate::strings::STATUS)
                            .icon(pod_ui::kit_icon(pod_ui::Icon::Info))
                            .tooltip("상태 인스펙터 (Ctrl+Shift+S)")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|w, _, window, cx| w.open_inspector(window, cx))),
                    ),
            )
            .child(
                div()
                    .qa_id(|| "status-notifications".into())
                    .flex_shrink_0()
                    .child(
                        Button::new("notifications")
                            .label(crate::tr!(NOTIFICATIONS_122; ; self.toasts.len()))
                            .icon(pod_ui::kit_icon(pod_ui::Icon::Bell))
                            .tooltip("알림 (Ctrl+Shift+B)")
                            // xsmall: a compact (26px) button overflowed the 24px bar.
                            .xsmall()
                            .ghost()
                            .on_click(
                                cx.listener(|w, _, window, cx| w.open_notifications(window, cx)),
                            ),
                    ),
            )
    }

    pub(crate) fn render_toasts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .max_h(px(240.))
            .id("toast-stack")
            .role(Role::List)
            .aria_label("최근 알림")
            .track_focus(&self.toast_focus)
            .tab_index(0)
            .focus_visible(|s| pod_ui::focus_style(s, crate::theme::mode()))
            .overflow_y_scroll()
            .children(
                self.toasts
                    .iter()
                    .rev()
                    .filter(|t| t.visible)
                    .take(3)
                    .map(|t| {
                        let paused = t.paused.clone();
                        let id = t.id;
                        let accent = match t.kind {
                            ToastKind::Info => rgb(crate::theme::tokens().info_fg),
                            ToastKind::Success => rgb(crate::theme::tokens().success_fg),
                            ToastKind::Warning => rgb(crate::theme::tokens().warning_fg),
                            ToastKind::Error => rgb(crate::theme::tokens().danger_fg),
                        };
                        div()
                            .id(("toast", t.id as usize))
                            .role(if matches!(t.kind, ToastKind::Warning | ToastKind::Error) {
                                Role::Alert
                            } else {
                                Role::Status
                            })
                            .aria_label(format!("{}: {}", t.title, t.body))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(crate::theme::tokens().bg_raised))
                            .border_l_4()
                            .border_color(accent)
                            .on_hover(move |hover, _, _| paused.set(*hover))
                            .shadow_lg()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(t.title.clone()),
                                    )
                                    .child({
                                        // Close: a 28px icon button (docs/10 §D4.6), a tab stop inside the
                                        // stack, so keyboard focus there pauses the timers too.
                                        let ws = cx.entity().downgrade();
                                        pod_ui::IconButton::new(
                                            ("dismiss", id as usize),
                                            pod_ui::Icon::X,
                                            crate::strings::CLOSE,
                                        )
                                        .render(
                                            crate::theme::mode(),
                                            move |_, cx| {
                                                let _ = ws.update(cx, |w, cx| {
                                                    if let Some(t) =
                                                        w.toasts.iter_mut().find(|t| t.id == id)
                                                    {
                                                        t.visible = false;
                                                    }
                                                    cx.notify();
                                                });
                                            },
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(crate::theme::tokens().text_secondary))
                                    .child(t.body.clone()),
                            )
                    }),
            )
    }
}

/// The sidebar as a separate view so it can be embedded with `cached`: it is
/// rebuilt only when the workspace data changes, not on every terminal frame.
#[derive(Clone)]
enum SidebarItem {
    /// (name, accessible label `name · 작업 N`, task count)
    Repo(String, String, usize),
    Task(String),
    Terminal(String),
}
impl SidebarItem {
    fn key(&self) -> String {
        match self {
            Self::Repo(id, _, _) => format!("repo:{id}"),
            Self::Task(id) => format!("task:{id}"),
            Self::Terminal(id) => format!("terminal:{id}"),
        }
    }
}
pub struct Sidebar {
    ws: WeakEntity<Workspace>,
    focus: FocusHandle,
    pub(crate) selected: Option<String>,
    scroll: UniformListScrollHandle,
    filter: Entity<InputState>,
    _filter_sub: Subscription,
}
impl Sidebar {
    fn new(ws: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::strings::SIDEBAR_SEARCH));
        let sub = cx.subscribe(
            &filter,
            |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify(),
        );
        // `.tab_index()` only applies to element-owned handles; a tracked handle must be a tab stop itself.
        Self {
            ws,
            focus: cx.focus_handle().tab_stop(true),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            filter,
            _filter_sub: sub,
        }
    }

    fn items(&self, cx: &App) -> Vec<SidebarItem> {
        let Some(ws) = self.ws.upgrade() else {
            return vec![];
        };
        let w = ws.read(cx);
        let query = self.filter.read(cx).value().to_lowercase();
        let query = query.trim();
        let empty_collapsed = Default::default();
        let collapsed = if query.is_empty() {
            &cx.global::<crate::theme::Settings>().collapsed
        } else {
            &empty_collapsed
        };
        let mut rows = Vec::new();
        let mut included = HashSet::new();
        let repos: std::collections::BTreeSet<_> = w
            .repos
            .iter()
            .map(|r| r.name.clone())
            .chain(w.tasks.iter().filter_map(|t| t.repo.clone()))
            .collect();
        for repo in repos {
            let tasks: Vec<_> = w
                .tasks
                .iter()
                .filter(|t| t.repo.as_deref() == Some(&repo))
                .collect();
            rows.push(SidebarItem::Repo(
                repo.clone(),
                crate::tr!(TASKS_123; ; repo, tasks.len()),
                tasks.len(),
            ));
            for t in &tasks {
                included.extend(t.terminals.iter().cloned());
            }
            if collapsed.contains(&format!("repo:{repo}")) {
                continue;
            }
            for t in tasks {
                rows.push(SidebarItem::Task(t.id.clone()));
                if !collapsed.contains(&format!("task:{}", t.id)) {
                    for id in &t.terminals {
                        if w.terminal(id).is_some() {
                            rows.push(SidebarItem::Terminal(id.clone()));
                        }
                    }
                }
            }
        }
        for t in &w.tasks {
            if t.repo.is_none() {
                rows.push(SidebarItem::Task(t.id.clone()));
                for id in &t.terminals {
                    included.insert(id.clone());
                    if !collapsed.contains(&format!("task:{}", t.id)) {
                        rows.push(SidebarItem::Terminal(id.clone()));
                    }
                }
            }
        }
        for t in &w.terminals {
            if !included.contains(&t.id) {
                rows.push(SidebarItem::Terminal(t.id.clone()));
            }
        }
        if !query.is_empty() {
            // Agent kinds match too: `claude`, `codex`, `셸` (docs/10 §D5.11).
            let matches_terminal = |id: &str| {
                w.terminal(id).is_some_and(|t| {
                    let kind = crate::agent_kind::identify_in(t, &w.tasks).kind;
                    format!(
                        "{} {} {} {}",
                        w.label_of(t),
                        t.id,
                        t.program,
                        kind.search_terms()
                    )
                    .to_lowercase()
                    .contains(query)
                })
            };
            let matches_task_text = |t: &TaskVm| {
                format!(
                    "{} {} {} {}",
                    t.title,
                    t.id,
                    t.branch.as_deref().unwrap_or(""),
                    t.repo.as_deref().unwrap_or("")
                )
                .to_lowercase()
                .contains(query)
            };
            // The task's own agent keeps the task row, not its other (e.g. Codex) terminals.
            let matches_task = |t: &TaskVm| {
                matches_task_text(t)
                    || t.agent
                        .as_deref()
                        .and_then(crate::agent_kind::from_task_agent)
                        .is_some_and(|k| k.search_terms().contains(query))
            };
            rows.retain(|row| match row {
                SidebarItem::Repo(repo, _, _) => {
                    repo.to_lowercase().contains(query)
                        || w.tasks.iter().any(|t| {
                            t.repo.as_ref() == Some(repo)
                                && (matches_task(t)
                                    || t.terminals.iter().any(|id| matches_terminal(id)))
                        })
                }
                SidebarItem::Task(id) => w.task(id).is_some_and(|t| {
                    matches_task(t) || t.terminals.iter().any(|id| matches_terminal(id))
                }),
                SidebarItem::Terminal(id) => {
                    matches_terminal(id)
                        || w.tasks
                            .iter()
                            .any(|t| t.terminals.contains(id) && matches_task_text(t))
                }
            });
        }
        rows
    }
    /// Tree depth of every row key: repo 0, task 1 (0 without a repo), terminal one below its task.
    fn depths(&self, cx: &App) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        let Some(ws) = self.ws.upgrade() else {
            return out;
        };
        let w = ws.read(cx);
        for t in &w.tasks {
            let depth = usize::from(t.repo.is_some());
            if let Some(repo) = &t.repo {
                out.insert(format!("repo:{repo}"), 0);
            }
            out.insert(format!("task:{}", t.id), depth);
            for id in &t.terminals {
                out.insert(format!("terminal:{id}"), depth + 1);
            }
        }
        out
    }
    fn activate(&mut self, item: &SidebarItem, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(item.key());
        match item {
            SidebarItem::Terminal(id) => {
                let _ = self
                    .ws
                    .update(cx, |w, cx| w.open_terminal(id, false, window, cx));
            }
            _ => {
                let mut settings = cx.global::<crate::theme::Settings>().clone();
                let key = item.key();
                if !settings.collapsed.remove(&key) {
                    settings.collapsed.insert(key);
                }
                let _ = settings.save();
                cx.set_global(settings);
                cx.notify();
            }
        }
    }
}
impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else {
            return div().into_any_element();
        };
        let items = Rc::new(self.items(cx));
        let rows = items.clone();
        let depths = self.depths(cx);
        // One row height per theme density (docs/10 §D4.6: task and worker are single lines).
        let height = pod_ui::themes::metrics().density.row;
        let collapsed = cx.global::<crate::theme::Settings>().collapsed.clone();
        let filtering = !self.filter.read(cx).value().trim().is_empty();
        let list = uniform_list(
            "workspace-tree",
            items.len(),
            cx.processor(move |sidebar, range: std::ops::Range<usize>, _, cx| {
                let Some(ws) = sidebar.ws.upgrade() else {
                    return Vec::new();
                };
                let tk = crate::theme::tokens();
                let mode = crate::theme::mode();
                // The tree owns the one chevron (docs/10 §D4.5); leaves get none.
                let chevron = |key: &str| {
                    pod_ui::icon_sized(
                        if !filtering && collapsed.contains(key) {
                            pod_ui::Icon::ChevronRight
                        } else {
                            pod_ui::Icon::ChevronDown
                        },
                        14.,
                        tk.text_muted,
                    )
                };
                range
                    .map(|i| {
                        let item = rows[i].clone();
                        let key = item.key();
                        let action = item.clone();
                        let row = match &item {
                            SidebarItem::Terminal(id) => ws
                                .read(cx)
                                .terminal(id)
                                .map(|t| ws.read(cx).render_row(t, &sidebar.ws).into_any_element())
                                .unwrap_or_else(|| div().into_any_element()),
                            SidebarItem::Repo(name, _, count) => div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .pl_2()
                                .pr_3()
                                .text_color(rgb(tk.text_secondary))
                                .child(chevron(&key))
                                .child(pod_ui::icon_sized(
                                    pod_ui::Icon::FolderGit2,
                                    14.,
                                    tk.text_muted,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(rgb(tk.text_muted))
                                        .child(crate::tr!(TASKS; ; count)),
                                )
                                .into_any_element(),
                            SidebarItem::Task(id) => {
                                let w = ws.read(cx);
                                let task = w.task(id);
                                let terminals: Vec<_> = task
                                    .into_iter()
                                    .flat_map(|t| t.terminals.iter())
                                    .filter_map(|id| w.terminal(id))
                                    .collect();
                                let kinds = task
                                    .map(|k| crate::agent_kind::task_kinds(k, &w.terminals))
                                    .unwrap_or_default();
                                let summary = crate::orch::summary_of(
                                    terminals.iter().map(|t| &t.id),
                                    &w.terminals,
                                );
                                // 8px status dot + title + count badge: needs-you counts stay visible (docs/10 §D4.6).
                                let dot = match summary.priority() {
                                    Some(state) => pod_ui::status_visual(state).tone.colors(tk).0,
                                    None => tk.text_muted,
                                };
                                let badge = if summary.error > 0 {
                                    Some(pod_ui::count_badge(
                                        summary.error,
                                        tk.danger_fg,
                                        tk.danger_bg,
                                    ))
                                } else if summary.blocked > 0 {
                                    Some(pod_ui::count_badge(
                                        summary.blocked,
                                        tk.warning_fg,
                                        tk.warning_bg,
                                    ))
                                } else {
                                    None
                                };
                                let _ = mode;
                                div()
                                    .id(SharedString::from(format!("task-label-{id}")))
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl(px(14.))
                                    .pr_3()
                                    .child(chevron(&key))
                                    .child(pod_ui::status_dot(dot))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(
                                                task.map(|t| t.title.clone()).unwrap_or_default(),
                                            ),
                                    )
                                    // The task's agents as small marks (docs/10 §D5.11).
                                    .when(!kinds.is_empty(), |d| {
                                        d.child(crate::agent_kind::stack_el(
                                            &format!("task-{id}"),
                                            &kinds,
                                            12.,
                                        ))
                                    })
                                    .child(match badge {
                                        Some(b) => b.into_any_element(),
                                        None => div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(rgb(tk.text_muted))
                                            .child(summary.total.to_string())
                                            .into_any_element(),
                                    })
                                    .tooltip({
                                        let detail = format!(
                                            "{} · {}",
                                            task.and_then(|t| t.branch.as_deref()).unwrap_or("—"),
                                            summary.label()
                                        );
                                        move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                detail.clone(),
                                            )
                                            .build(window, cx)
                                        }
                                    })
                                    .context_menu({
                                        let id = id.clone();
                                        let ws = sidebar.ws.clone();
                                        move |menu, _, _| {
                                            let (a, b, c, f) =
                                                (ws.clone(), ws.clone(), ws.clone(), ws.clone());
                                            let (ia, ib, ic, ifl) =
                                                (id.clone(), id.clone(), id.clone(), id.clone());
                                            menu.item(
                                                PopupMenuItem::new(crate::strings::TASK_DETAILS)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = a.update(cx, |w, cx| {
                                                            w.open_task_dialog(
                                                                ia.clone(),
                                                                window,
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new(crate::strings::REVIEW_CHANGES)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = b.update(cx, |w, cx| {
                                                            w.open_review(&ib, window, cx)
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new(crate::strings::OPEN_FILES)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = f.update(cx, |w, cx| {
                                                            w.open_editor(
                                                                &ifl, None, false, window, cx,
                                                            )
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new(
                                                    crate::strings::ADD_SHELL_TERMINAL,
                                                )
                                                .on_click(move |_, window, cx| {
                                                    let _ = c.update(cx, |w, cx| {
                                                        w.task_terminal(ic.clone(), window, cx)
                                                    });
                                                }),
                                            )
                                        }
                                    })
                                    .into_any_element()
                            }
                        };
                        div()
                            .id(SharedString::from(key.clone()))
                            .when(!matches!(item, SidebarItem::Terminal(_)), |d| {
                                let name = match &item {
                                    SidebarItem::Repo(_, label, _) => label.clone(),
                                    SidebarItem::Task(id) => {
                                        let w = ws.read(cx);
                                        match w.task(id) {
                                            Some(t) => task_accessible_name(t, &w.terminals),
                                            None => id.clone(),
                                        }
                                    }
                                    SidebarItem::Terminal(id) => id.clone(),
                                };
                                // Repo/task rows fold (←/→): their state is part of the tree semantics.
                                d.role(Role::TreeItem)
                                    .aria_label(name)
                                    .aria_selected(sidebar.selected.as_ref() == Some(&key))
                                    .aria_expanded(filtering || !collapsed.contains(&key))
                            })
                            .h(px(height))
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .w_full()
                            .when(sidebar.selected.as_ref() == Some(&key), |d| {
                                d.bg(rgb(tk.bg_selected))
                            })
                            .when(
                                sidebar.selected.as_ref() != Some(&key)
                                    && !matches!(item, SidebarItem::Terminal(_)),
                                |d| d.hover(|s| s.bg(rgb(tk.bg_hover))),
                            )
                            .on_click(
                                cx.listener(move |s, _, window, cx| {
                                    s.activate(&action, window, cx)
                                }),
                            )
                            .child(row)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0()
        .into_any_element();
        let list = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .child(Input::new(&self.filter).small().prefix(pod_ui::icon_sized(
                        pod_ui::Icon::Search,
                        14.,
                        crate::theme::tokens().text_muted,
                    ))),
            )
            .child(list)
            .into_any_element();
        let content = ws.read(cx).render_sidebar(&self.ws, list);
        div()
            .id("sidebar-navigation")
            .h_full()
            .track_focus(&self.focus)
            .tab_index(0)
            .role(Role::Tree)
            .aria_label("저장소와 작업 탐색")
            .focus_visible(|s| pod_ui::focus_style(s, crate::theme::mode()))
            .on_key_down(cx.listener(move |s, e: &KeyDownEvent, window, cx| {
                let selected = items
                    .iter()
                    .position(|r| Some(&r.key()) == s.selected.as_ref());
                if e.keystroke.key == "enter" {
                    if let Some(row) = selected.and_then(|i| items.get(i)) {
                        s.activate(row, window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                let filtering = !s.filter.read(cx).value().trim().is_empty();
                let collapsed = cx.global::<crate::theme::Settings>().collapsed.clone();
                let nodes: Vec<TreeNode> = items
                    .iter()
                    .map(|r| TreeNode {
                        depth: depths.get(&r.key()).copied().unwrap_or(0),
                        parent: !matches!(r, SidebarItem::Terminal(_)),
                        expanded: filtering || !collapsed.contains(&r.key()),
                    })
                    .collect();
                let Some(step) = tree_key(&nodes, selected, e.keystroke.key.as_str()) else {
                    return;
                };
                match step {
                    TreeStep::Select(i) => {
                        if let Some(row) = items.get(i) {
                            s.selected = Some(row.key());
                            // Nearest: a row below the edge scrolls into view at the bottom
                            // instead of jumping to the top on every ↓ (the list stays put).
                            s.scroll.scroll_to_item(i, ScrollStrategy::Nearest);
                            cx.notify();
                        }
                    }
                    TreeStep::Fold(i, expand) => {
                        if let Some(row) = items.get(i).filter(|_| !filtering) {
                            let mut settings = cx.global::<crate::theme::Settings>().clone();
                            if expand {
                                settings.collapsed.remove(&row.key());
                            } else {
                                settings.collapsed.insert(row.key());
                            }
                            let _ = settings.save();
                            cx.set_global(settings);
                            cx.notify();
                        }
                    }
                }
                cx.stop_propagation();
            }))
            .child(content)
            .into_any_element()
    }
}

/// Tooltip and accessible-name detail of a sidebar terminal row. An
/// unreachable (v19) terminal is dimmed like a lost one; its reason leads.
fn terminal_row_detail(t: &TerminalVm, badge: &pod_ui::StatusBadge) -> String {
    let detail = format!(
        "{} | {}",
        badge.detail(),
        crate::chrome::freshness(t.status.as_ref())
    );
    match t.lost_reason() {
        Some(reason) => format!("{reason} | {detail}"),
        None => detail,
    }
}

/// Short system sound (the default "asterisk" sound).
/// Whether the user is looking at terminal `id`, so blocked/done needs no toast
/// (E2E B5): the window is active and that terminal's panel holds focus in the
/// rendered frame. `in_view` must come from real focus
/// ([`Workspace::terminal_in_view`]), not the last-focused cache.
fn watching(window_active: bool, in_view: Option<&str>, id: &str) -> bool {
    window_active && in_view == Some(id)
}

fn beep() {
    // SAFETY: plain Win32 call without pointers.
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(
            windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
        );
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::stats::record_frame();
        let task = self
            .focused_terminal
            .as_deref()
            .and_then(|id| self.terminal(id))
            .and_then(|t| {
                t.task_id
                    .as_deref()
                    .or_else(|| {
                        self.tasks
                            .iter()
                            .find(|k| k.terminals.contains(&t.id))
                            .map(|k| k.id.as_str())
                    })
                    .and_then(|id| self.task(id))
                    .map(|k| k.title.clone())
            });
        let title = os_window_title(task.as_deref());
        if title != self.os_title {
            window.set_window_title(&title);
            self.os_title = title;
        }
        let body = crate::layout_mode::render_body(self, window, cx);
        div()
            .id("workspace-root")
            .track_focus(&self.root_focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(crate::theme::tokens().bg_canvas))
            .text_color(rgb(crate::theme::tokens().text_primary))
            .on_action(cx.listener(|w, _: &crate::chrome::Jump, window, cx| {
                if cx.global::<crate::theme::Settings>().jump_shortcut == "ctrl-k" {
                    w.open_jump(window, cx);
                } else if let Some(id) = &w.focused_terminal {
                    w.source.input(id, vec![11]);
                }
            }))
            .on_action(
                cx.listener(|w, _: &crate::chrome::JumpAlternate, window, cx| {
                    if cx.global::<crate::theme::Settings>().jump_shortcut == "ctrl-shift-k" {
                        w.open_jump(window, cx);
                    } else {
                        cx.propagate();
                    }
                }),
            )
            .on_action(
                cx.listener(|w, _: &crate::chrome::SettingsAction, window, cx| {
                    w.open_settings(window, cx)
                }),
            )
            .on_action(cx.listener(|w, _: &crate::chrome::NewTask, window, cx| {
                w.open_new_task_dialog(window, cx)
            }))
            .on_action(
                cx.listener(|w, _: &crate::chrome::Notifications, window, cx| {
                    w.open_notifications(window, cx)
                }),
            )
            .on_action(
                cx.listener(|w, _: &crate::chrome::InspectStatus, window, cx| {
                    w.open_inspector(window, cx)
                }),
            )
            .child(self.title_bar(cx))
            .on_action(cx.listener(|w, _: &NewTerminal, window, cx| {
                w.new_terminal_or_reopen_tab(window, cx)
            }))
            .on_action(cx.listener(|w, _: &ArrangeGrid, window, cx| w.arrange_grid(window, cx)))
            .on_action(
                cx.listener(|w, _: &OpenReview, window, cx| w.open_review_for_focus(window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &QuickOpenFiles, window, cx| {
                    w.quick_open_files(true, window, cx)
                }),
            )
            .on_action(cx.listener(|w, _: &OpenFilesPanel, window, cx| {
                w.quick_open_files(false, window, cx)
            }))
            .on_action(cx.listener(|w, _: &crate::orch::NewGroup, window, cx| {
                w.open_new_group_dialog(window, cx)
            }))
            .on_action(
                cx.listener(|w, _: &crate::orch::OpenGates, window, cx| w.open_gates(window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &crate::orch::OpenAgentTree, window, cx| {
                    w.open_agent_tree(window, cx)
                }),
            )
            .on_action(
                cx.listener(|w, _: &crate::orch_ui::OpenRunBoard, window, cx| {
                    crate::orch_ui::open_page(w, crate::orch_ui::Page::Board, window, cx)
                }),
            )
            .on_action(
                cx.listener(|w, _: &crate::orch_ui::OpenOrchInbox, window, cx| {
                    crate::orch_ui::open_inbox(w, window, cx)
                }),
            )
            .child(body)
            .child(self.render_status_bar(window, cx))
    }
}

impl Workspace {
    /// 작업대 layout body (crate::layout_mode): sidebar column + dock, styled by the
    /// active surface model (docs/10 §D5.2): flat hairlines, layered/islands rounded
    /// panels with gaps, mono without borders.
    pub(crate) fn render_workbench_body(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let banner = match &self.conn {
            ConnState::Reconnecting { attempt, error } => Some(
                crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR; attempt,error;),
            ),
            ConnState::Connecting => {
                Some(crate::strings::CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED.to_string())
            }
            ConnState::Connected { .. } => None,
        };
        let empty = self.live_panels().is_empty()
            && self.live_reviews().is_empty()
            && self.live_editors().is_empty()
            && !self.orch.has_panels()
            && self
                .settings_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_none()
            && !crate::mods_ui::page_open(cx)
            && !crate::sessions_ui::page_open(cx)
            && !crate::orch_ui::page_open(cx)
            && self
                .inspector_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_none();
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
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
            .child(
                div()
                    .w(px(m.density.sidebar))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(gap))
                    .child(island(div().flex_1().min_h_0()).child(self.sidebar.clone()))
                    .child(self.render_toasts(cx)),
            )
            .child(
                island(div())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .relative()
                    .when_some(banner, |d, b| {
                        d.child(
                            div().px_2().pt_2().child(
                                pod_ui::banner(
                                    pod_ui::Tone::Warning,
                                    crate::theme::mode(),
                                    b,
                                    None,
                                )
                                .text_xs(),
                            ),
                        )
                    })
                    .when_some(self.render_hook_banner(cx), |d, b| d.child(b))
                    .child(div().flex_1().min_h_0().child(self.dock.clone()))
                    .when(empty, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(rgb(crate::theme::tokens().text_muted))
                                .child({
                                    // One CTA for the situation (docs/10 §D4.6): no repository → open
                                    // one; registered → a new task; still connecting → none.
                                    let state = EmptyState::of(
                                        self.conn.is_connected(),
                                        self.repos.is_empty() && self.tasks.is_empty(),
                                        self.tasks.is_empty(),
                                    );
                                    let (title, body, icon) = state.copy();
                                    let cta = match state {
                                        EmptyState::Connecting => {
                                            crate::chrome::loading_placeholder(
                                                crate::strings::CONNECTING_TO_YOUR_WORKSPACE,
                                            )
                                            .into_any_element()
                                        }
                                        EmptyState::NoRepository => Button::new("empty-repo")
                                            .label(crate::strings::OPEN_REPOSITORY)
                                            .primary()
                                            .on_click(cx.listener(|w, _, window, cx| {
                                                w.open_repository(window, cx)
                                            }))
                                            .into_any_element(),
                                        EmptyState::NoTask | EmptyState::PickTask => {
                                            Button::new("empty-task")
                                                .label(crate::strings::NEW_TASK)
                                                .primary()
                                                .on_click(cx.listener(|w, _, window, cx| {
                                                    w.open_new_task_dialog(window, cx)
                                                }))
                                                .into_any_element()
                                        }
                                    };
                                    pod_ui::Frame::new(
                                        "workspace-empty",
                                        pod_ui::FrameKind::Empty,
                                        title,
                                        body,
                                    )
                                    .icon(icon)
                                    .render(
                                        crate::theme::mode(),
                                        [div().pt_2().child(cta).into_any_element()],
                                    )
                                }),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// What the empty workspace says, with exactly one action (docs/10 §D4.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmptyState {
    Connecting,
    NoRepository,
    NoTask,
    PickTask,
}
impl EmptyState {
    pub(crate) fn of(connected: bool, no_repository: bool, no_task: bool) -> Self {
        match (connected, no_repository, no_task) {
            (false, _, _) => Self::Connecting,
            (true, true, _) => Self::NoRepository,
            (true, false, true) => Self::NoTask,
            _ => Self::PickTask,
        }
    }
    /// Title, description and icon.
    pub(crate) fn copy(self) -> (&'static str, &'static str, pod_ui::Icon) {
        match self {
            Self::Connecting => (
                crate::strings::CONNECTING_TO_YOUR_WORKSPACE,
                crate::strings::YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_,
                pod_ui::Icon::Plug,
            ),
            Self::NoRepository => (
                crate::strings::A_CLEAR_SPACE_FOR_YOUR_NEXT_TASK,
                crate::strings::OPEN_A_REPOSITORY_CREATE_A_TASK_OR_RETURN_TO_AN_EXISTING_TERMINAL,
                pod_ui::Icon::FolderGit2,
            ),
            Self::NoTask => (
                crate::strings::EMPTY_NO_TASK_TITLE,
                crate::strings::EMPTY_NO_TASK,
                pod_ui::Icon::Layers,
            ),
            Self::PickTask => (
                crate::strings::EMPTY_PICK_TASK_TITLE,
                crate::strings::EMPTY_PICK_TASK,
                pod_ui::Icon::Layers,
            ),
        }
    }
}

/// The OS window title (ui-03): `Pod`, or `<작업> — Pod` while a task's terminal has focus.
pub(crate) fn os_window_title(task: Option<&str>) -> String {
    match task.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => format!("{t} — Pod"),
        None => "Pod".into(),
    }
}

/// A sidebar row as the keyboard sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TreeNode {
    pub depth: usize,
    /// Repo/task rows fold; terminals are leaves.
    pub parent: bool,
    pub expanded: bool,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TreeStep {
    Select(usize),
    /// Fold row `i` (true = expand).
    Fold(usize, bool),
}
/// WAI-ARIA tree keys (ui-13): ↑/↓ move (the first ↓ with nothing selected picks the
/// first row), Home/End, → expands a folded parent or enters its first child,
/// ← folds an open parent or moves to the parent row.
pub(crate) fn tree_key(nodes: &[TreeNode], selected: Option<usize>, key: &str) -> Option<TreeStep> {
    let last = nodes.len().checked_sub(1)?;
    let Some(i) = selected.filter(|i| *i <= last) else {
        return matches!(key, "down" | "up" | "home" | "right")
            .then_some(TreeStep::Select(if key == "up" { last } else { 0 }))
            .or((key == "end").then_some(TreeStep::Select(last)));
    };
    let node = nodes[i];
    match key {
        "down" => Some(TreeStep::Select((i + 1).min(last))),
        "up" => Some(TreeStep::Select(i.saturating_sub(1))),
        "home" => Some(TreeStep::Select(0)),
        "end" => Some(TreeStep::Select(last)),
        "right" if node.parent && !node.expanded => Some(TreeStep::Fold(i, true)),
        "right" if node.parent => nodes
            .get(i + 1)
            .filter(|n| n.depth > node.depth)
            .map(|_| TreeStep::Select(i + 1))
            .or(Some(TreeStep::Select(i))),
        "left" if node.parent && node.expanded => Some(TreeStep::Fold(i, false)),
        "left" => Some(TreeStep::Select(
            (0..i)
                .rev()
                .find(|j| nodes[*j].depth < node.depth)
                .unwrap_or(i),
        )),
        "right" => Some(TreeStep::Select(i)),
        _ => None,
    }
}

/// A task row's accessible name: title, agents and the counts the dot/badge show
/// (`응답 1 · 실패 1 …`, docs/10 §4.4), so a screen reader finds the task that needs the user.
pub(crate) fn task_accessible_name(t: &TaskVm, terminals: &[TerminalVm]) -> String {
    let kinds = crate::agent_kind::task_kinds(t, terminals);
    let summary = crate::orch::summary_of(t.terminals.iter(), terminals);
    let mut name = t.title.clone();
    if !kinds.is_empty() {
        name = format!(
            "{name} · {}",
            kinds
                .iter()
                .map(|k| k.accessible_name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if summary.total > 0 {
        name = format!("{name} · {}", crate::orch::summary_short(&summary));
    }
    name
}

/// Which status-bar items fit (docs/10 §D4.6): as the window narrows, branch/terminal
/// totals go first, then 완료, then 작업 중. 연결, 응답 and 알림 always stay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StatusPlan {
    pub branch: bool,
    pub terminals: bool,
    pub counts: bool,
    pub working: bool,
    pub done: bool,
    pub tasks: bool,
    pub stats: bool,
}
impl StatusPlan {
    /// `width`: window width in logical px; `counts`: the layout wants 작업 중/응답/완료
    /// here (06 shows them in the title bar); `status`: podd reports agent status.
    pub(crate) fn for_width(width: f32, counts: bool, status: bool) -> Self {
        let counts = counts && status;
        Self {
            branch: width >= 1400.,
            terminals: width >= 1180.,
            counts,
            working: width >= 900.,
            done: width >= 1000.,
            tasks: width >= 1080.,
            stats: width >= 1400.,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::watching;

    #[test]
    fn unreachable_terminal_row_tooltip_gives_the_reason() {
        use crate::vm::{Liveness, TerminalVm};
        let base = serde_json::json!({
            "id": "t9", "argv": ["claude"], "cwd": "C:/w", "cols": 80, "rows": 24,
            "created_ms": 1, "attached": 0
        });
        let with = |extra: serde_json::Value| {
            let mut v = base.clone();
            let m = v.as_object_mut().unwrap();
            m.extend(extra.as_object().unwrap().clone());
            TerminalVm::from_json(&v).unwrap()
        };
        let t = with(serde_json::json!({
            "state": "unreachable", "unreachable": "holder pipe closed"
        }));
        assert_eq!(t.live, Liveness::Lost, "still dimmed as lost");
        let detail = super::terminal_row_detail(&t, &crate::orch::terminal_badge(&t));
        assert!(
            detail.starts_with("podd가 이 터미널에 연결할 수 없음: holder pipe closed | "),
            "{detail}"
        );
        // No reason from podd: the tooltip still says why it is dimmed.
        let t = with(serde_json::json!({"state": "unreachable"}));
        let detail = super::terminal_row_detail(&t, &crate::orch::terminal_badge(&t));
        assert!(
            detail.starts_with("podd가 이 터미널에 연결할 수 없음 | "),
            "{detail}"
        );
        // A plain lost terminal has no unreachable reason.
        let t = with(serde_json::json!({"state": "exited", "lost": true}));
        assert_eq!(t.lost_reason(), None);
        let detail = super::terminal_row_detail(&t, &crate::orch::terminal_badge(&t));
        assert!(!detail.contains("연결할 수 없음"), "{detail}");
    }

    #[test]
    fn tree_keys_follow_the_aria_tree_pattern() {
        use super::{TreeNode, TreeStep::*, tree_key};
        let n = |depth, parent, expanded| TreeNode {
            depth,
            parent,
            expanded,
        };
        // repo / task (open) / two terminals / task (folded)
        let nodes = [
            n(0, true, true),
            n(1, true, true),
            n(2, false, true),
            n(2, false, true),
            n(1, true, false),
        ];
        assert_eq!(
            tree_key(&nodes, None, "down"),
            Some(Select(0)),
            "first ↓ selects the first row, not the second"
        );
        assert_eq!(tree_key(&nodes, None, "end"), Some(Select(4)));
        assert_eq!(tree_key(&nodes, Some(2), "home"), Some(Select(0)));
        assert_eq!(tree_key(&nodes, Some(0), "end"), Some(Select(4)));
        assert_eq!(
            tree_key(&nodes, Some(3), "left"),
            Some(Select(1)),
            "← on a terminal goes to its task"
        );
        assert_eq!(
            tree_key(&nodes, Some(1), "left"),
            Some(Fold(1, false)),
            "← on an open task folds it"
        );
        assert_eq!(
            tree_key(&nodes, Some(4), "left"),
            Some(Select(0)),
            "← on a folded task goes to the repo"
        );
        assert_eq!(tree_key(&nodes, Some(4), "right"), Some(Fold(4, true)));
        assert_eq!(
            tree_key(&nodes, Some(1), "right"),
            Some(Select(2)),
            "→ on an open task enters its first child"
        );
        assert_eq!(
            tree_key(&nodes, Some(4), "down"),
            Some(Select(4)),
            "stays on the last row"
        );
        assert_eq!(tree_key(&nodes, Some(0), "x"), None);
        assert_eq!(tree_key(&[], None, "down"), None);
    }

    #[test]
    fn task_name_carries_answer_and_failure_counts() {
        use crate::source::TestBackend;
        let t = TestBackend::task("s/t", "로그인", "s", &["b1", "w1"]);
        let terminals = [
            TestBackend::terminal("b1", "리뷰 봇", Some("s/t"), "blocked"),
            TestBackend::terminal("w1", "작업자", Some("s/t"), "working"),
        ];
        let name = super::task_accessible_name(&t, &terminals);
        assert!(name.starts_with("로그인"), "{name}");
        assert!(
            name.contains("응답 1") && name.contains("작업 중 1"),
            "{name}"
        );
        assert!(!name.contains("오류 0"), "only non-zero counts: {name}");
    }

    #[test]
    fn empty_workspace_offers_one_action_for_its_situation() {
        use super::EmptyState::{self, *};
        assert_eq!(EmptyState::of(false, true, true), Connecting);
        assert_eq!(
            EmptyState::of(true, true, true),
            NoRepository,
            "nothing yet: open a repository"
        );
        assert_eq!(
            EmptyState::of(true, false, true),
            NoTask,
            "registered repository: new task, not 저장소 열기"
        );
        assert_eq!(EmptyState::of(true, false, false), PickTask);
        assert!(!NoTask.copy().1.contains("저장소"), "{}", NoTask.copy().1);
    }

    #[test]
    fn window_title_names_the_app_and_the_task() {
        assert_eq!(super::os_window_title(None), "Pod");
        assert_eq!(super::os_window_title(Some("  ")), "Pod");
        assert_eq!(
            super::os_window_title(Some("로그인-수정")),
            "로그인-수정 — Pod"
        );
    }

    #[test]
    fn working_and_task_counts_read_differently() {
        assert_eq!(crate::tr!(WORKING; ; 49), "작업 중 49");
        assert_eq!(crate::tr!(TASKS; ; 100), "작업 100개");
        assert_ne!(crate::tr!(WORKING; ; 1), crate::tr!(TASKS; ; 1));
    }

    #[test]
    fn status_bar_drops_totals_before_answers_and_notifications() {
        use super::StatusPlan;
        let wide = StatusPlan::for_width(1600., true, true);
        assert!(
            wide.terminals && wide.counts && wide.working && wide.done && wide.tasks && wide.stats
        );
        let min = StatusPlan::for_width(800., true, true);
        assert!(min.counts, "응답 stays at the minimum window");
        assert!(!min.terminals && !min.working && !min.done && !min.tasks && !min.stats);
        let mid = StatusPlan::for_width(1050., true, true);
        assert!(
            mid.working && mid.done && !mid.tasks && !mid.terminals,
            "{mid:?}"
        );
        assert!(
            !StatusPlan::for_width(1600., false, true).counts,
            "06 관제형: counts are in the title bar"
        );
        assert!(
            !StatusPlan::for_width(1600., true, false).counts,
            "no status capability"
        );
    }

    #[test]
    fn needs_you_toast_suppressed_only_for_the_terminal_in_view() {
        // Looking at t1 in the active window: no toast for t1, toast for t2.
        assert!(watching(true, Some("t1"), "t1"));
        assert!(!watching(true, Some("t1"), "t2"));
        // E2E B5: t1 was the last terminal focused, but a Review/Group panel
        // has focus now -> nothing in view -> toast.
        assert!(!watching(true, None, "t1"));
        // Window in the background: always toast.
        assert!(!watching(false, Some("t1"), "t1"));
    }
}
