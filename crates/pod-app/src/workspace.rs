//! The main window: sidebar (terminals, later grouped by task), dock area of
//! terminal panels, status bar, toasts and dialogs.
//!
//! This file owns the [`Workspace`] state and its creation; each concern is a
//! child module (docs/20-refactor/app.md §2): dock panels and restore (`dock`),
//! the source event pump (`binding`), command routing (`commands`), dialogs,
//! the editor panels, toasts (`notifications`), the sidebar, the status bar,
//! the window shell, and the layout-kit host (`layout_host`).

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::component::WindowExt;
use gpui_kit::component::dock::{DockArea, DockEvent, DockSkin, PanelStyle};
use gpui_kit::*;

use crate::editor_panel::EditorPanel;
use crate::layout::{self, Saved};
use crate::panel::TerminalPanel;
use crate::review::ReviewPanel;
use crate::source::{PoddSource, SourceEvent};
use crate::vm::{Capabilities, ConnState, RepoVm, TaskVm, TerminalVm};

mod binding;
mod commands;
mod dialogs;
mod dock;
mod editor;
mod layout_host;
mod notifications;
mod shell;
mod sidebar;
mod status_bar;
#[cfg(test)]
mod tests;

pub use commands::WorkspaceAction;
pub use dock::{
    AppGlobals, RestoreCtx, RestoredPanel, build_editor_panel, build_gates_panel,
    build_group_panel, build_panel, build_review_panel, build_tree_panel,
};
pub(crate) use notifications::Toast;
pub use notifications::ToastKind;
pub use sidebar::Sidebar;
// Named by the QA modules (`ui_review_qa`, `native_qa`) at their old path.
#[cfg(test)]
pub(crate) use status_bar::StatusPlan;

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
        let pump = Self::pump(events, window, cx);

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

    #[cfg(test)]
    pub(crate) fn root_focus_handle(&self) -> FocusHandle {
        self.root_focus.clone()
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

    pub(crate) fn terminal(&self, id: &str) -> Option<&TerminalVm> {
        self.terminals.iter().find(|t| t.id == id)
    }

    // ---- tasks (podd P3, docs/08) ----------------------------------------

    pub(crate) fn task(&self, id: &str) -> Option<&TaskVm> {
        self.tasks.iter().find(|t| t.id == id)
    }
}
