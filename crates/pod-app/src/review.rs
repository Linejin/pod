//! Review panel (P4): one task's changes as a diff, line comments stored in
//! podd (`review.comments.*`), and "Send to agent" (`review.send`) with the
//! delivery state (docs/09 §7, docs/11).
//!
//! Data flow:
//! - `task.diff` (request connection) → the merge base and changed files →
//!   both versions of each file loaded in the background ([`pod_diff::model`],
//!   one `git cat-file --batch`) → full-context patch → `DiffState`.
//! - Refresh: manual (F5 / button), when the task's agent ends a turn (status
//!   event: `turn` grows or it leaves `working`), when the task's git summary
//!   from the regular 3 s task poll changes, and after a delivery. No timer
//!   polling of the diff.
//! - Comments and deliveries are re-read after every change we make and on
//!   agent status events. Delivery changes arrive as `review.delivery` events
//!   on the status stream (podd, additive in v6); only against a daemon that
//!   does not send them, deliveries are re-read every 2 s while one is queued.
//!
//! Layout (A-P3): this file owns the panel's state and creation, its key
//! bindings and its dock identity (`PodReview`, the `{"task"}` dump). Policy
//! and display plans are in [`model`], requests, refreshes, the delivery poll
//! and the draft/drawer/tab transitions in [`controller`], the toolbar, file
//! list, comment column, drawer, cards and diff in [`view`], and panel-local
//! wording in [`copy`]. Tests: `review::tests` (pure; the former
//! `review::editor_tests` is `review::tests::editor_tests`) and `review::qa`
//! (the panel in the headless workspace).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::component::diff::{DiffLinePosition, DiffLineRange, DiffState};
use gpui_kit::component::input::TextareaState;
use gpui_kit::*;
use pod_diff::{FileRow, SourceFile};
use podd::protocol::{self as p, DeliveryInfo, ReviewComment, Side};
use serde_json::json;

use crate::source::PoddSource;
use crate::vm::{StatusVm, TaskVm};
use crate::workspace::Workspace;

mod controller;
mod copy;
mod model;
#[cfg(test)]
mod qa;
#[cfg(test)]
mod tests;
mod view;

use controller::{new_state, watch_state};
use model::{SendOutcome, git_signature};
pub(crate) use view::words;

pub const REVIEW_PANEL_NAME: &str = "PodReview";
const KEY_CONTEXT: &str = "PodReview";

actions!(
    pod_review,
    [
        NextChange,
        PrevChange,
        NextFile,
        PrevFile,
        AddComment,
        SendToAgent,
        RefreshDiff,
        OpenInEditor,
        ToggleComments,
        CloseCommentsDrawer
    ]
);

/// Key context of the comments drawer (docs/10 §D4.6: below 560 px the comment column is a drawer).
const DRAWER_CONTEXT: &str = "PodReviewDrawer";
/// D4.6 widths: comments min 224, 340 here; below 560 px of canvas the column becomes a drawer.
pub const COMMENTS_W: f32 = 340.;
pub const COMMENTS_MIN_W: f32 = 224.;
pub const COMMENTS_DRAWER_BELOW: f32 = 560.;

/// Review keys (only while focus is inside a Review panel). No plain letters:
/// they would be taken from the comment text fields.
pub fn bind_keys(cx: &mut App) {
    let c = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("f7", NextChange, c),
        KeyBinding::new("shift-f7", PrevChange, c),
        KeyBinding::new("alt-down", NextChange, c),
        KeyBinding::new("alt-up", PrevChange, c),
        KeyBinding::new("alt-pagedown", NextFile, c),
        KeyBinding::new("alt-pageup", PrevFile, c),
        KeyBinding::new("ctrl-shift-c", AddComment, c),
        KeyBinding::new("ctrl-shift-enter", SendToAgent, c),
        KeyBinding::new("f5", RefreshDiff, c),
        KeyBinding::new("ctrl-shift-o", OpenInEditor, c),
        KeyBinding::new("alt-c", ToggleComments, c),
        KeyBinding::new("escape", CloseCommentsDrawer, Some(DRAWER_CONTEXT)),
    ]);
}

// ---- colors: one place, restyled later by pod-ui (docs/10 §5) -------------
mod color {
    pub fn bg() -> u32 {
        crate::theme::tokens().bg_canvas
    }
    pub fn pane() -> u32 {
        crate::theme::tokens().bg_chrome
    }
    pub fn bar() -> u32 {
        crate::theme::tokens().bg_chrome
    }
    pub fn border() -> u32 {
        crate::theme::tokens().border_subtle
    }
    pub fn card() -> u32 {
        crate::theme::tokens().bg_surface
    }
    pub fn hover() -> u32 {
        crate::theme::tokens().bg_raised
    }
    pub fn selected() -> u32 {
        crate::theme::tokens().bg_selected
    }
    pub fn muted() -> u32 {
        crate::theme::tokens().text_muted
    }
    pub fn add() -> u32 {
        crate::theme::tokens().success_fg
    }
    pub fn del() -> u32 {
        crate::theme::tokens().danger_fg
    }
    pub fn accent() -> u32 {
        crate::theme::tokens().info_fg
    }
    pub fn draft_bg() -> u32 {
        crate::theme::tokens().bg_selected
    }
    pub fn warn() -> u32 {
        crate::theme::tokens().warning_fg
    }
    pub fn ok() -> u32 {
        crate::theme::tokens().success_fg
    }
    pub fn info() -> u32 {
        crate::theme::tokens().info_fg
    }
}

struct Draft {
    path: String,
    side: Side,
    line: u32,
    end_line: u32,
    input: Entity<TextareaState>,
}

pub struct ReviewPanel {
    pub task_id: String,
    source: PoddSource,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    worktree: Option<String>,
    agent_terminal: Option<String>,
    git_signature: Option<String>,
    agent_status: Option<StatusVm>,

    state: Entity<DiffState>,
    /// Re-render on `DiffState` changes (background highlighting, folds):
    /// the dock draws panels as cached views, which do not see them otherwise.
    _state_subs: [Subscription; 2],
    /// A multi-line line-number drag ended: open a range draft on the next render.
    pending_range: Option<DiffLineRange>,
    /// Clear the instructions box on the next render (needs a window).
    clear_instructions: bool,
    sources: Rc<Vec<SourceFile>>,
    files: Rc<Vec<FileRow>>,
    patch_hash: u64,
    base_label: SharedString,
    current_file: usize,
    loading: bool,
    refresh_again: bool,
    load_error: Option<String>,
    loaded_once: bool,
    opened: Instant,

    split: bool,
    collapsed: bool,
    wrap: bool,
    show_files: bool,
    show_comments: bool,
    /// The host's canvas is too narrow for the comment column (D4.6): comments open as a drawer.
    comments_drawer: bool,
    drawer_open: bool,
    drawer_focus: FocusHandle,

    comments: Vec<ReviewComment>,
    checked: HashSet<String>,
    show_resolved: bool,
    draft: Option<Draft>,
    hovered: Option<DiffLinePosition>,
    instructions: Entity<TextareaState>,
    deliveries: Vec<DeliveryInfo>,
    /// v8 submit checks by delivery id (`review.delivery` events, `submits`).
    submits: HashMap<String, p::SubmitCheck>,
    outcome: Option<SendOutcome>,
    sending: bool,

    refresh_task: Option<Task<()>>,
    poll_task: Option<Task<()>>,
    _subs: Vec<Subscription>,

    /// P6: the pull request section (docs/15) and whether its tab is shown.
    pr: Entity<crate::pr_panel::PrSection>,
    pr_tab: bool,
}

impl ReviewPanel {
    pub fn new(
        source: PoddSource,
        workspace: WeakEntity<Workspace>,
        task: &TaskVm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = new_state(Vec::new(), false, true, cx);
        let instructions = cx.new(|cx| {
            TextareaState::new(window, cx).auto_grow(2, 6).placeholder(
                crate::strings::OPTIONAL_INSTRUCTIONS_FOR_THE_AGENT_SENT_BEFORE_THE_COMMENTS,
            )
        });
        let state_subs = watch_state(&state, cx);
        let pr = cx
            .new(|cx| crate::pr_panel::PrSection::new(source.clone(), workspace.clone(), task, cx));
        let mut this = Self {
            task_id: task.id.clone(),
            source,
            workspace,
            focus: cx.focus_handle(),
            worktree: task.worktree.clone(),
            agent_terminal: task.terminals.first().cloned(),
            git_signature: git_signature(task),
            agent_status: None,
            state,
            _state_subs: state_subs,
            pending_range: None,
            clear_instructions: false,
            sources: Rc::new(Vec::new()),
            files: Rc::new(Vec::new()),
            patch_hash: 0,
            base_label: "".into(),
            current_file: 0,
            loading: false,
            refresh_again: false,
            load_error: None,
            loaded_once: false,
            opened: Instant::now(),
            // Inline by default: the dock center is narrow next to the sidebars,
            // and side-by-side needs room for two columns (docs/09 §7).
            split: false,
            collapsed: true,
            wrap: false,
            show_files: true,
            show_comments: true,
            comments_drawer: false,
            drawer_open: false,
            drawer_focus: cx.focus_handle(),
            comments: Vec::new(),
            checked: HashSet::new(),
            show_resolved: false,
            draft: None,
            hovered: None,
            instructions,
            deliveries: Vec::new(),
            submits: HashMap::new(),
            outcome: None,
            sending: false,
            refresh_task: None,
            poll_task: None,
            _subs: Vec::new(),
            pr,
            pr_tab: false,
        };
        this.refresh_diff(cx);
        this.reload_comments(cx);
        this.reload_deliveries(cx);
        this
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}

fn debug(args: std::fmt::Arguments) {
    if std::env::var_os("POD_APP_DEBUG").is_some() {
        println!("[review] {args}");
    }
}

impl EventEmitter<PanelEvent> for ReviewPanel {}

impl Focusable for ReviewPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for ReviewPanel {
    fn panel_name(&self) -> &'static str {
        REVIEW_PANEL_NAME
    }

    /// Persisted shape; restored at start while the task exists (`layout::prune`).
    fn dump(&self, _cx: &App) -> PanelState {
        let mut s = PanelState::new(REVIEW_PANEL_NAME);
        s.info = PanelInfo::panel(json!({ "task": self.task_id }));
        s
    }

    fn on_removed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.poll_task = None;
        if let Some(ws) = self.workspace.upgrade() {
            let me = cx.entity().downgrade();
            cx.defer(move |cx| ws.update(cx, |w, cx| w.review_removed(&me, cx)));
        }
    }
}
