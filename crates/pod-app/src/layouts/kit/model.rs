//! The narrow contract between a layout mode and the window that hosts it
//! (docs/20-refactor/app.md §2 "Layout kit와 상태 소유").
//!
//! - [`LayoutFrame`]: read-only projection of what a body shows, borrowed for
//!   one render; nothing is copied per frame.
//! - [`LayoutHost`]: host views lent as display slots (dock, sidebar, toasts,
//!   hook banner) and the [`LayoutActions`] handle.
//! - [`LayoutActions`]: clone-able handle for event closures and views that
//!   outlive a render. The weak workspace stays inside the host's [`ActionSink`].
//! - [`LayoutRenderer`]: the body renderer every mode offers through
//!   `LayoutModule::renderer` (required since T2: there is no Workspace
//!   fallback).
//!
//! Only `workspace/layout_host.rs` (A-F1) implements [`LayoutHost`] and
//! [`ActionSink`], so a mode written against this contract never names the
//! Workspace type. [`UiIntent`] is a UI request, not a wire message: gate
//! decisions and RPCs stay with their existing controllers.
//!
//! # Two phases (rf2-kit)
//!
//! A renderer's `render_body` and `title_bar_extra` run inside the host's own
//! render, while the host (the Workspace) is leased; reading or updating it
//! there is a double lease. So the host side has two halves, and
//! [`LayoutActions`] groups its methods the same way (`kit/host.rs`):
//!
//! - [`HostHandles`], **render-safe**: the window key, the state slots, the
//!   observers, the dock, sidebar and source handles and the slot focus. The
//!   host captured them when it made the actions; none of them borrows the
//!   host, so they work anywhere, a host render included. In a render the
//!   frame is the argument the renderer gets.
//! - [`HostCommands`], **update phase**: a fresh frame (`read`), `dispatch`,
//!   `dispatch_then`, `post`, `resolve_gate`, the new panels, `current_task`
//!   and the panel lists. They read or update the host, so they run outside
//!   any host render or update: from event handlers, observers, deferred
//!   closures, async completions, or a layout's own view's render.
//!
//! Neither half has default methods: a host that misses one does not compile.
//!
//! A-T2a added what the layout cards (A-P10…P14) still read or did through the
//! workspace: saved labels, every gate, toasts, the tree selection and the
//! fixture flag on the frame; the per-window state slots, change
//! observation, dock and panel access, window-less updates and the gate path
//! on [`LayoutActions`] (`kit/host.rs`). T2 kept only what a mode uses and
//! added the repositories (the workbench's empty state).
use std::collections::BTreeMap;
use std::rc::Rc;

use gpui_kit::component::dock::DockArea;
use gpui_kit::{AnyElement, AnyView, App, Entity, FocusHandle, SharedString, Subscription, Window};
use podd::protocol::{GateInfo, GateResolveParams};

use super::host::{AppOnce, GateDone, HostChange, HostUpdate, OnChange, OnChangeIn, WindowKey};
use super::metrics::LayoutMetrics;
use crate::panel::TerminalPanel;
use crate::review::ReviewPanel;
use crate::source::PoddSource;
use crate::vm::{ConnState, RepoVm, TaskVm, TerminalVm};
use crate::workspace::{Toast, ToastKind};

/// Read-only projection of the window state for one render.
#[derive(Clone, Debug)]
pub(crate) struct LayoutFrame<'a> {
    pub(crate) tasks: &'a [TaskVm],
    /// Registered repositories (the workbench's empty state).
    pub(crate) repos: &'a [RepoVm],
    pub(crate) terminals: &'a [TerminalVm],
    pub(crate) conn: &'a ConnState,
    /// Pending gates, oldest first (`OrchState::pending`).
    pub(crate) gates: &'a [GateInfo],
    /// Terminal with keyboard focus (`Workspace::focused_terminal`).
    pub(crate) focused_terminal: Option<&'a str>,
    pub(crate) panels: OpenPanels,
    /// The orchestration panels one by one and the app-level pages.
    pub(crate) panel_detail: PanelDetail,
    pub(crate) metrics: LayoutMetrics,
    /// Terminal names the user saved (`layout::Saved::labels`, keyed by
    /// `layout::label_key`); read through [`Self::label_of`].
    pub(crate) labels: &'a BTreeMap<String, String>,
    /// Every gate seen this session, decided ones too, by id
    /// (`OrchState::gates`); [`Self::gates`] holds the pending ones only.
    pub(crate) all_gates: &'a BTreeMap<String, GateInfo>,
    /// The toast stack as data, oldest first (`Workspace::toasts`, hidden
    /// ones included, as the dense log reads it).
    pub(crate) toasts: FrameToasts<'a>,
    /// Row key selected in the tree sidebar (`Sidebar::selected`).
    pub(crate) tree_selection: Option<String>,
    /// The window runs on the `--ui-fixture` source, without podd.
    pub(crate) fixture: bool,
}

/// The workspace's toasts lent to a frame (`Toast` has no `Debug`).
#[derive(Clone, Copy)]
pub(crate) struct FrameToasts<'a>(pub(crate) &'a [Toast]);

impl<'a> std::ops::Deref for FrameToasts<'a> {
    type Target = [Toast];
    fn deref(&self) -> &[Toast] {
        self.0
    }
}

impl std::fmt::Debug for FrameToasts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.iter().map(|t| t.id)).finish()
    }
}

impl<'a> LayoutFrame<'a> {
    /// Panels other than terminals and reviews, counted one each, plus the
    /// extensions, sessions and orchestration pages: the review layout's dock
    /// signature (a rise means "show the 패널 view"); the kanban and block
    /// ones add `panels.reviews`. The orchestration pages count as one while
    /// any is open (the frame has no count); a page opened next to another
    /// is seen through its focus ([`super::LayoutActions::focused_dock_panel`]).
    pub(crate) fn other_panels(&self) -> usize {
        let (p, d) = (&self.panels, &self.panel_detail);
        p.settings as usize
            + p.inspector as usize
            + p.editors
            + p.remote_terminals
            + d.groups
            + d.gates_panel as usize
            + d.tree_panel as usize
            + d.mods_page as usize
            + d.sessions_page as usize
            + d.orch_page as usize
    }

    /// The name a terminal shows: the saved label, else its name, else its
    /// id — `Workspace::label_of`.
    pub(crate) fn label_of(&self, t: &TerminalVm) -> String {
        self.labels
            .get(&crate::layout::label_key(&t.id, t.created_ms))
            .cloned()
            .or_else(|| t.name.clone())
            .unwrap_or_else(|| t.id.clone())
    }

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
    /// Live remote terminal panels (`machines_ui`, docs/17 R4).
    pub(crate) remote_terminals: usize,
}

/// What [`OpenPanels`] folds into `orch` or leaves out: the orchestration
/// panels one by one and the app-level pages beside the dock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PanelDetail {
    /// Group panels open (`OrchState::live_groups`).
    pub(crate) groups: usize,
    /// The gates (approval) panel is open.
    pub(crate) gates_panel: bool,
    /// The agent-tree panel is open.
    pub(crate) tree_panel: bool,
    /// The extensions page (`mods_ui::page_open`).
    pub(crate) mods_page: bool,
    /// The sessions page (`sessions_ui::page_open`).
    pub(crate) sessions_page: bool,
    /// The orchestration page (`orch_ui::page_open`).
    pub(crate) orch_page: bool,
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
            && self.remote_terminals == 0
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
    /// The new-task dialog.
    NewTask,
    /// The open-repository dialog (the workbench's empty state).
    OpenRepository,
    /// The gates (approval) inbox.
    OpenGates,
    /// The agent tree.
    OpenAgentTree,
    OpenSettings,
    OpenNotifications,
    /// The jump-to palette.
    OpenJump,
    /// Raise a toast.
    Toast {
        kind: ToastKind,
        title: SharedString,
        body: SharedString,
    },
    /// Bring the terminal's panel forward in the dock when it has one;
    /// keyboard focus stays where it is (the queue's and dense's selection).
    RevealTerminal {
        id: String,
    },
    /// The task's running terminals, opened in reverse task order so the
    /// first ends in front, or a shell in its worktree when none runs — all
    /// in one host update (the rail's 터미널 button). Follow up with
    /// [`LayoutActions::dispatch_then`].
    OpenTaskTerminals {
        task: String,
    },
    /// The review command for the focused panel, dispatched as the
    /// `OpenReview` action like its key (the queue's 06 activity bar).
    OpenFocusedReview,
}

/// The render-safe half of the host side: handles the host captured when it
/// made the [`LayoutActions`]. None of these reads or updates the host
/// itself, so they also work inside a host render (`render_body`,
/// `title_bar_extra`) where the host is leased. A gone host answers `None` /
/// `false` and keeps nothing.
pub(crate) trait HostHandles {
    /// Key of the window: stable for the host's life, also once it is gone.
    fn window_key(&self) -> WindowKey;
    /// Focus handle of a slot, if the slot has one.
    fn focus_handle(&self, slot: Slot, cx: &App) -> Option<FocusHandle>;
    /// Runs `f` once when the host is released (detached); `false` and `f`
    /// dropped when it is already gone.
    fn on_release(&self, cx: &mut App, f: AppOnce) -> bool;
    /// Calls `f` after each `change`; `None` when the host is gone.
    fn observe(&self, change: HostChange, cx: &mut App, f: OnChange) -> Option<Subscription>;
    /// [`Self::observe`] with the window, like `Context::observe_in`.
    fn observe_in(
        &self,
        change: HostChange,
        window: &mut Window,
        cx: &mut App,
        f: OnChangeIn,
    ) -> Option<Subscription>;
    /// The live dock of the window.
    fn dock(&self, cx: &App) -> Option<Entity<DockArea>>;
    /// A host view to draw outside a host render (dock, sidebar).
    fn view(&self, slot: Slot, cx: &App) -> Option<AnyView>;
    /// The podd source of the window.
    fn source(&self, cx: &App) -> Option<PoddSource>;
}

/// The update-phase half of the host side: these read or update the host,
/// so they are never called inside a host render or a host update (no
/// re-entrant workspace lease). A gone host answers `None` / empty /
/// `false`, runs nothing and sends nothing.
pub(crate) trait HostCommands {
    /// Calls `f` with a frame of the current window state; does nothing when
    /// the window is gone.
    fn with_frame(&self, cx: &App, f: &mut dyn FnMut(&LayoutFrame<'_>));
    /// Carries the intent out now, in one host update.
    fn dispatch(&self, intent: UiIntent, window: &mut Window, cx: &mut App);
    /// `Workspace::current_task`.
    fn current_task(&self, window: &Window, cx: &App) -> Option<String>;
    /// Live terminal panels, dock order of creation (`Workspace::live_panels`).
    fn terminal_panels(&self, cx: &App) -> Vec<Entity<TerminalPanel>>;
    /// Live Review panels (`Workspace::live_reviews`).
    fn review_panels(&self, cx: &App) -> Vec<Entity<ReviewPanel>>;
    /// A terminal panel outside the dock, registered like dock panels.
    fn new_terminal_panel(
        &self,
        id: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<TerminalPanel>>;
    /// A Review panel outside the dock, registered like dock reviews.
    fn new_review_panel(
        &self,
        task: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Entity<ReviewPanel>>;
    /// Applies the updates in order inside one host update; `false` when
    /// the host is gone.
    fn post(&self, updates: Vec<HostUpdate>, cx: &mut App) -> bool;
    /// The one gate decision path (`gates::resolve_gate`); `false` and `done`
    /// dropped when the host is gone.
    fn resolve_gate(&self, params: GateResolveParams, cx: &mut App, done: GateDone) -> bool;
}

/// What the host side does for [`LayoutActions`] (it holds the weak
/// workspace): both halves, every method required. The product host is
/// `workspace/layout_host.rs`; the kit's tests have their own stub.
pub(crate) trait ActionSink: HostHandles + HostCommands {}

impl<T: HostHandles + HostCommands + ?Sized> ActionSink for T {}

/// Clone-able handle to the host for event closures and long-lived views.
/// Closures capture this and stable ids, never a borrowed frame. Its
/// methods come in the two phases of the module doc (`kit/host.rs`).
#[derive(Clone)]
pub(crate) struct LayoutActions(pub(super) Rc<dyn ActionSink>);

impl LayoutActions {
    pub(crate) fn new(sink: Rc<dyn ActionSink>) -> Self {
        Self(sink)
    }
}

/// The host side of one render.
pub(crate) trait LayoutHost {
    /// Lends a slot for this render; `None` when it has nothing to show.
    fn slot(&mut self, slot: Slot, window: &mut Window, cx: &mut App) -> Option<AnyElement>;
    /// Handle for closures and views that outlive this render. During the
    /// render itself only its render-safe half applies ([`HostHandles`]).
    fn actions(&self) -> LayoutActions;
}

/// A mode's narrow renderer (`LayoutModule::renderer`, required): the host
/// uses it for the body and the title-bar extra alike. Both run inside the
/// host's render, so they read the `frame` they get and call only the
/// render-safe half of [`LayoutActions`]; update-phase calls go into the
/// event handlers and views they build.
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
