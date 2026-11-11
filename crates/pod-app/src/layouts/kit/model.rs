//! The narrow contract between a layout mode and the window that hosts it
//! (docs/20-refactor/app.md §2 "Layout kit와 상태 소유").
//!
//! - [`LayoutFrame`]: read-only projection of what a body shows, borrowed for
//!   one render; nothing is copied per frame.
//! - [`LayoutHost`]: host views lent as display slots (dock, sidebar, toasts,
//!   hook banner) and the [`LayoutActions`] handle.
//! - [`LayoutActions`]: clone-able handle for event closures and views that
//!   outlive a render. The weak workspace stays inside the host's [`ActionSink`].
//! - [`LayoutRenderer`]: the narrow body renderer a mode may offer through
//!   `LayoutModule::renderer`; without one the Workspace renderer is used.
//!
//! Only `workspace/layout_host.rs` (A-F1) implements [`LayoutHost`] and
//! [`ActionSink`], so a mode written against this contract never names the
//! Workspace type. [`UiIntent`] is a UI request, not a wire message: gate
//! decisions and RPCs stay with their existing controllers.
use std::rc::Rc;

use gpui_kit::{AnyElement, App, FocusHandle, SharedString, Window};
use podd::protocol::GateInfo;

use super::metrics::LayoutMetrics;
use crate::vm::{ConnState, Counts, TaskVm, TerminalVm};
use crate::workspace::ToastKind;

/// Read-only projection of the window state for one render.
#[derive(Clone, Debug)]
pub(crate) struct LayoutFrame<'a> {
    pub(crate) tasks: &'a [TaskVm],
    pub(crate) terminals: &'a [TerminalVm],
    pub(crate) conn: &'a ConnState,
    /// Pending gates, oldest first (`OrchState::pending`).
    pub(crate) gates: &'a [GateInfo],
    /// Terminal with keyboard focus (`Workspace::focused_terminal`).
    pub(crate) focused_terminal: Option<&'a str>,
    pub(crate) panels: OpenPanels,
    pub(crate) metrics: LayoutMetrics,
}

impl<'a> LayoutFrame<'a> {
    /// First task with this id, like `Workspace::task`.
    pub(crate) fn task(&self, id: &str) -> Option<&'a TaskVm> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// First terminal with this id, like `Workspace::terminal`.
    pub(crate) fn terminal(&self, id: &str) -> Option<&'a TerminalVm> {
        self.terminals.iter().find(|t| t.id == id)
    }

    /// Text of the connection warning above the body; none while connected.
    pub(crate) fn warning(&self) -> Option<String> {
        connection_warning(self.conn)
    }

    /// Agent state counts of the terminals (`vm::counts`).
    pub(crate) fn counts(&self) -> Counts {
        crate::vm::counts(self.terminals)
    }
}

/// The connection warning every mode shows above its body: the same text in
/// the workbench and in all six layout files.
pub(crate) fn connection_warning(conn: &ConnState) -> Option<String> {
    match conn {
        ConnState::Reconnecting { attempt, error } => {
            Some(crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR; attempt,error;))
        }
        ConnState::Connecting => {
            Some(crate::strings::CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED.to_string())
        }
        ConnState::Connected { .. } => None,
    }
}

/// Panels open in the dock and beside it, for the "nothing open" hint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct OpenPanels {
    /// Live terminal panels (`Workspace::live_panels`).
    pub(crate) terminals: usize,
    /// Live review panels (`Workspace::live_reviews`).
    pub(crate) reviews: usize,
    /// Live editor panels (`Workspace::live_editors`).
    pub(crate) editors: usize,
    /// Group, gates or agent-tree panel (`OrchState::has_panels`).
    pub(crate) orch: bool,
    /// Settings panel still open.
    pub(crate) settings: bool,
    /// Inspector panel still open.
    pub(crate) inspector: bool,
}

impl OpenPanels {
    /// None of these panels is open. The app-level pages (mods, sessions,
    /// orchestration) are globals each mode checks on top of this, as today:
    /// the workbench checks one more page than the queue and dense layouts.
    pub(crate) fn dock_empty(&self) -> bool {
        self.terminals == 0
            && self.reviews == 0
            && self.editors == 0
            && !self.orch
            && !self.settings
            && !self.inspector
    }
}

/// A host view or element a mode can show; the host keeps owning it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    /// Terminal, review and editor panels.
    Dock,
    /// The tree sidebar.
    Sidebar,
    /// The toast stack.
    Toasts,
    /// The hook-trust prompt, only while one is waiting.
    HookBanner,
}

/// A UI request a mode hands to the host.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum UiIntent {
    /// Show a terminal in the dock; `another` opens a second view of it.
    OpenTerminal {
        id: String,
        another: bool,
    },
    /// Open a task's review panel.
    OpenReview {
        task: String,
    },
    /// Open a task's pull-request section; `form` opens the create form.
    OpenPullRequest {
        task: String,
        form: bool,
    },
    /// Open a task's files: an optional `(path, line, column)` target, or
    /// quick open.
    OpenEditor {
        task: String,
        target: Option<(String, u32, u32)>,
        quick_open: bool,
    },
    /// Files of the current task (quick open or the file tree).
    QuickOpenFiles {
        quick_open: bool,
    },
    /// A shell terminal in a task's worktree.
    TaskTerminal {
        task: String,
    },
    /// The new-task dialog.
    NewTask,
    /// The gates (approval) inbox.
    OpenGates,
    /// The agent tree.
    OpenAgentTree,
    OpenSettings,
    OpenNotifications,
    /// The jump-to palette.
    OpenJump,
    /// Arrange the open terminals in a grid.
    ArrangeGrid,
    /// Move keyboard focus into a host slot.
    FocusRegion(Slot),
    /// Raise a toast.
    Toast {
        kind: ToastKind,
        title: SharedString,
        body: SharedString,
    },
}

/// What the host side does for [`LayoutActions`]; it holds the weak workspace.
pub(crate) trait ActionSink {
    /// Calls `f` with a frame of the current window state; does nothing when
    /// the window is gone.
    fn with_frame(&self, cx: &App, f: &mut dyn FnMut(&LayoutFrame<'_>));
    /// Focus handle of a slot, if the slot has one.
    fn focus_handle(&self, slot: Slot, cx: &App) -> Option<FocusHandle>;
    /// Carries the intent out now. Never called while the host itself is
    /// being updated (no re-entrant workspace update).
    fn dispatch(&self, intent: UiIntent, window: &mut Window, cx: &mut App);
}

/// Clone-able handle to the host for event closures and long-lived views.
/// Closures capture this and stable ids, never a borrowed frame.
#[derive(Clone)]
pub(crate) struct LayoutActions(Rc<dyn ActionSink>);

impl LayoutActions {
    pub(crate) fn new(sink: Rc<dyn ActionSink>) -> Self {
        Self(sink)
    }

    /// Reads the window state through a fresh frame; `None` when it is gone.
    pub(crate) fn read<R>(&self, cx: &App, f: impl FnOnce(&LayoutFrame<'_>) -> R) -> Option<R> {
        let mut f = Some(f);
        let mut out = None;
        self.0.with_frame(cx, &mut |frame| {
            if let Some(f) = f.take() {
                out = Some(f(frame));
            }
        });
        out
    }

    pub(crate) fn focus_handle(&self, slot: Slot, cx: &App) -> Option<FocusHandle> {
        self.0.focus_handle(slot, cx)
    }

    /// Carries the intent out now, in the caller's order: for paths that call
    /// the workspace synchronously today (e.g. focus that must move at once).
    pub(crate) fn dispatch(&self, intent: UiIntent, window: &mut Window, cx: &mut App) {
        self.0.dispatch(intent, window, cx);
    }

    /// Carries the intent out after the current effects (`Window::defer`):
    /// only for paths that defer today.
    pub(crate) fn defer(&self, intent: UiIntent, window: &mut Window, cx: &mut App) {
        let sink = self.0.clone();
        window.defer(cx, move |window, cx| sink.dispatch(intent, window, cx));
    }
}

/// The host side of one render.
pub(crate) trait LayoutHost {
    /// Lends a slot for this render; `None` when it has nothing to show.
    fn slot(&mut self, slot: Slot, window: &mut Window, cx: &mut App) -> Option<AnyElement>;
    /// Handle for closures and views that outlive this render.
    fn actions(&self) -> LayoutActions;
}

/// A mode's narrow renderer (`LayoutModule::renderer`). When a mode has one,
/// the host uses it for the body and the title-bar extra alike.
pub(crate) trait LayoutRenderer {
    /// Everything between the title bar and the status bar.
    fn render_body(
        &self,
        frame: &LayoutFrame<'_>,
        host: &mut dyn LayoutHost,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement;

    /// Extra title-bar content after the context line; none by default.
    fn title_bar_extra(
        &self,
        _frame: &LayoutFrame<'_>,
        _actions: &LayoutActions,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }
}
