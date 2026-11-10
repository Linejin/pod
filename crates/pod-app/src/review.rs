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

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{
    Diff, DiffAnnotation, DiffEvent, DiffFile, DiffHoverHighlight, DiffLinePosition, DiffLineRange,
    DiffMode, DiffSide, DiffState,
};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_diff::{CONTEXT_LINES, ChangedFile, FileRow, SourceFile};
use podd::protocol::{
    self as p, CommentState, DeliveryInfo, DeliveryState, NewComment, ReviewComment, Side, codes,
    orch_codes,
};
use serde_json::json;

use crate::qa_marks::QaId;
use crate::source::{PoddSource, SourceError};
use crate::vm::{AgentState, StatusVm, TaskVm};
use crate::workspace::{ToastKind, Workspace};

pub const REVIEW_PANEL_NAME: &str = "PodReview";
const KEY_CONTEXT: &str = "PodReview";
/// While a delivery is queued, re-read deliveries this often.
const QUEUED_POLL: Duration = Duration::from_secs(2);
/// Coalesce refresh triggers that arrive together (status + task poll).
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(400);

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
/// UIA name of the drawer and the toggle's tooltip (strings.rs is owned elsewhere this wave).
pub const COMMENTS_DRAWER_NAME: &str = "의견 서랍";
pub const COMMENTS_DRAWER_TIP: &str = "의견 서랍 열기·닫기 (Alt+C, 닫기 Esc)";

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

fn side_of(side: DiffSide) -> Side {
    match side {
        DiffSide::Original => Side::Old,
        DiffSide::Modified => Side::New,
    }
}

fn diff_side(side: Side) -> DiffSide {
    match side {
        Side::Old => DiffSide::Original,
        Side::New => DiffSide::Modified,
    }
}

fn side_label(side: Side) -> &'static str {
    match side {
        Side::Old => crate::strings::ORIGINAL,
        Side::New => crate::strings::MODIFIED,
    }
}

fn state_label(s: CommentState) -> &'static str {
    match s {
        CommentState::Open => crate::strings::UNRESOLVED,
        CommentState::Queued => crate::strings::QUEUED,
        CommentState::Sent => crate::strings::SENT,
        CommentState::Resolved => crate::strings::RESOLVED,
    }
}

fn state_color(s: CommentState) -> u32 {
    match s {
        CommentState::Open => color::info(),
        CommentState::Queued => color::warn(),
        CommentState::Sent => color::ok(),
        CommentState::Resolved => color::muted(),
    }
}

fn location(c: &ReviewComment) -> String {
    let lines = match c.end_line {
        Some(e) if e != c.line => format!("{}–{e}", c.line),
        _ => c.line.to_string(),
    };
    format!("{}:{lines} ({})", c.file, side_label(c.side))
}

/// What the last "Send to agent" did, shown under the send box.
#[derive(Clone, Debug, PartialEq)]
pub enum SendOutcome {
    /// podd stored or typed the delivery; the live state is in `deliveries`.
    Delivery(String),
    /// podd refused right away (agent blocked, terminal exited, nothing to send).
    Refused(String),
}

/// Delivery state as one line of text and a color.
pub fn delivery_text(d: &DeliveryInfo) -> (String, u32) {
    let n = d.comment_ids.len();
    match d.state {
        DeliveryState::Queued => (
            crate::tr!(N_COMMENT_S_QUEUED_UNTIL_THE_AGENT_IS_IDLE_WAS; n; d.id, AgentState::parse(&d.agent_state).label()),
            color::warn(),
        ),
        DeliveryState::Delivered => (
            crate::tr!(N_COMMENT_S_DELIVERED_TO; n; d.id, d.terminal, AgentState::parse(d.delivered_state.as_deref().unwrap_or(&d.agent_state)).label()),
            color::ok(),
        ),
        DeliveryState::Cancelled => (crate::tr!(CANCELLED; ; d.id), color::muted()),
        DeliveryState::Failed => (
            crate::tr!(FAILED; ; d.id, d.error.as_deref().unwrap_or(crate::strings::NOT_DELIVERED)),
            color::del(),
        ),
        DeliveryState::Preview => (crate::tr!(PREVIEW; ; d.id), color::muted()),
    }
}

/// v8 submit check of a delivered prompt as a short line and a color; `None`
/// when there is nothing to say (no check, or a shell that cannot confirm).
/// `unconfirmed` means the prompt may still sit in the agent's input box.
pub fn submit_text(s: &p::SubmitCheck) -> Option<(String, u32)> {
    match s.state {
        p::SubmitState::Pending => Some((crate::strings::SUBMIT_PENDING.into(), color::muted())),
        p::SubmitState::Confirmed => Some((crate::strings::SUBMIT_CONFIRMED.into(), color::ok())),
        p::SubmitState::Unconfirmed => {
            Some((crate::strings::SUBMIT_UNCONFIRMED.into(), color::warn()))
        }
        p::SubmitState::Unverifiable => None,
    }
}

/// Whether `new` newly reports an unconfirmed submit (toast once).
pub fn became_unconfirmed(old: Option<&p::SubmitCheck>, new: &p::SubmitCheck) -> bool {
    new.state == p::SubmitState::Unconfirmed
        && old.is_none_or(|o| o.state != p::SubmitState::Unconfirmed)
}

/// Text for a refused `review.send`.
pub fn refusal_text(e: &SourceError) -> String {
    match e.code {
        // podd's message already says what the agent waits for (docs/11 §3.1).
        Some(orch_codes::AGENT_BLOCKED) => crate::tr!(REFUSED_AGENT_BLOCKED; ; e.message),
        Some(codes::TERMINAL_EXITED) => crate::tr!(REFUSED_AGENT_TERMINAL_EXITED; ; e.message),
        Some(orch_codes::INVALID_STATE) => crate::tr!(NOTHING_TO_SEND; ; e.message),
        _ => crate::tr!(SEND_FAILED; ; e.message),
    }
}

/// Deliveries that left `queued` between two reads (for toasts).
pub fn finished_deliveries<'a>(
    before: &[DeliveryInfo],
    after: &'a [DeliveryInfo],
) -> Vec<&'a DeliveryInfo> {
    let was_queued: HashSet<&str> = before
        .iter()
        .filter(|d| d.state == DeliveryState::Queued)
        .map(|d| d.id.as_str())
        .collect();
    after
        .iter()
        .filter(|d| d.state != DeliveryState::Queued && was_queued.contains(d.id.as_str()))
        .collect()
}

/// Did the agent just finish a turn (time to re-read the diff)?
pub fn turn_ended(old: Option<&StatusVm>, new: &StatusVm) -> bool {
    match old {
        Some(o) => {
            new.turn > o.turn
                || (o.state == AgentState::Working && new.state != AgentState::Working)
        }
        None => false,
    }
}

/// Which comments "Send" sends: the checked ones that can be sent, or all
/// open ones when nothing is checked. `None` = `all_open`; `Some(empty)` =
/// only resolved comments are checked, so nothing is sent (never the
/// unchecked ones instead).
pub fn send_selection(
    comments: &[ReviewComment],
    checked: &HashSet<String>,
) -> Option<Vec<String>> {
    let checked: Vec<&ReviewComment> = comments
        .iter()
        .filter(|c| checked.contains(&c.id))
        .collect();
    if checked.is_empty() {
        return None;
    }
    Some(
        checked
            .into_iter()
            .filter(|c| c.state != CommentState::Resolved)
            .map(|c| c.id.clone())
            .collect(),
    )
}

struct Draft {
    path: String,
    side: Side,
    line: u32,
    end_line: u32,
    input: Entity<TextareaState>,
}

/// Result of one background diff load.
struct Loaded {
    base: String,
    merge_base: String,
    sources: Vec<SourceFile>,
    files: Vec<DiffFile>,
    rows: Vec<FileRow>,
    error: Option<String>,
    patch_hash: u64,
    timings: String,
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

    // ---- inputs from the workspace -------------------------------------

    /// The task list was re-read: worktree, agent terminal, and a git change
    /// (new commit / dirty count) triggers a diff refresh.
    pub fn set_task(&mut self, task: &TaskVm, cx: &mut Context<Self>) {
        self.pr.update(cx, |p, cx| p.set_task(task, cx));
        self.worktree = task.worktree.clone();
        self.agent_terminal = task.terminals.first().cloned();
        let sig = git_signature(task);
        if sig != self.git_signature {
            self.git_signature = sig;
            self.schedule_refresh(cx);
        }
    }

    /// A terminal's agent status changed (any terminal; we filter).
    pub fn agent_status(&mut self, terminal: &str, status: &StatusVm, cx: &mut Context<Self>) {
        if self.agent_terminal.as_deref() != Some(terminal) {
            return;
        }
        let ended = turn_ended(self.agent_status.as_ref(), status);
        self.agent_status = Some(status.clone());
        if ended {
            self.schedule_refresh(cx);
        }
        // podd's delivery worker reacts to the same event; read its result a
        // bit later (not needed when podd sends `review.delivery` events).
        if self
            .deliveries
            .iter()
            .any(|d| d.state == DeliveryState::Queued)
            && !crate::source::delivery_events()
        {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(600))
                    .await;
                let _ = this.update(cx, |p, cx| {
                    p.reload_deliveries(cx);
                    p.reload_comments(cx);
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// A `review.delivery` event for this task: replace the record, toast a
    /// queued delivery that went out (or failed), refresh comments.
    pub fn delivery_event(
        &mut self,
        d: &DeliveryInfo,
        submit: Option<&p::SubmitCheck>,
        cx: &mut Context<Self>,
    ) {
        if let Some(s) = submit {
            self.submit_changed(&d.id, s.clone(), cx);
        }
        let before = self.deliveries.clone();
        self.deliveries.retain(|x| x.id != d.id);
        self.deliveries.push(d.clone());
        self.deliveries.sort_by_key(|x| x.created_ms);
        for f in finished_deliveries(&before, &self.deliveries) {
            let (text, _) = delivery_text(f);
            let ok = f.state == DeliveryState::Delivered;
            let kind = if ok {
                ToastKind::Success
            } else {
                ToastKind::Warning
            };
            self.toast(kind, crate::tr!(REVIEW_FOR; ; if ok { crate::strings::DELIVERED } else { crate::strings::NOT_DELIVERED }, self.task_id), text, cx);
        }
        // Comment states follow the delivery (queued / sent / open again).
        self.reload_comments(cx);
        self.ensure_poll(cx);
        cx.notify();
    }

    fn submit_changed(&mut self, id: &str, s: p::SubmitCheck, cx: &mut Context<Self>) {
        if became_unconfirmed(self.submits.get(id), &s) {
            self.toast(
                ToastKind::Warning,
                crate::strings::REVIEW_SUBMIT_UNCONFIRMED.into(),
                crate::tr!(CHECK_AGENT_INPUT; id; self.task_id),
                cx,
            );
        }
        self.submits.insert(id.to_string(), s);
    }

    /// "Enter 누르기" on an unconfirmed delivery: one Enter to its terminal.
    fn press_enter(&mut self, terminal: &str, cx: &mut Context<Self>) {
        self.source.input(terminal, b"\r".to_vec());
        cx.notify();
    }

    fn toast(&self, kind: ToastKind, title: String, body: String, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        // Deferred: the workspace may be the one calling into this panel.
        cx.defer(move |cx| {
            let _ = ws.update(cx, |w, cx| w.raise_toast(kind, title, body, cx));
        });
    }

    // ---- diff ------------------------------------------------------------

    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REFRESH_DEBOUNCE).await;
            let _ = this.update(cx, |p, cx| p.refresh_diff(cx));
        }));
    }

    /// Manual refresh: diff, comments (they may change from the `pod review` CLI) and deliveries.
    pub fn refresh_all(&mut self, cx: &mut Context<Self>) {
        self.refresh_diff(cx);
        self.reload_comments(cx);
        self.reload_deliveries(cx);
    }

    /// Re-read `task.diff` and the file contents; replaces the diff only when it changed.
    pub fn refresh_diff(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            self.refresh_again = true;
            return;
        }
        let Some(worktree) = self.worktree.clone() else {
            self.load_error = Some(crate::strings::TASK_HAS_NO_WORKTREE.into());
            cx.notify();
            return;
        };
        self.loading = true;
        cx.notify();
        let diff = self.source.call::<p::TaskDiff>(&p::TaskRef {
            task: self.task_id.clone(),
        });
        cx.spawn(async move |this, cx| {
            let t0 = Instant::now();
            let result = diff.await;
            let rpc_ms = t0.elapsed().as_secs_f64() * 1e3;
            let loaded = match result {
                Ok(d) => {
                    cx.background_executor()
                        .spawn(async move {
                            let t1 = Instant::now();
                            let changed: Vec<ChangedFile> = d
                                .files
                                .iter()
                                .map(|f| ChangedFile { path: f.path.clone(), old_path: f.old_path.clone(), status: f.status.clone(), binary: f.binary })
                                .collect();
                            match pod_diff::model::load_changed(&PathBuf::from(&worktree), &d.merge_base, &changed) {
                                Ok(sources) => {
                                    let load_ms = t1.elapsed().as_secs_f64() * 1e3;
                                    let t2 = Instant::now();
                                    let patch = pod_diff::model::patch_of(&sources);
                                    let patch_hash = hash(&patch);
                                    let (files, rows, error) = pod_diff::parse(&sources);
                                    let lines: usize = sources.iter().map(SourceFile::line_count).sum();
                                    let timings = format!(
                                        "task.diff {rpc_ms:.0}ms, load {load_ms:.0}ms, patch+parse {:.0}ms, {} files, {lines} lines",
                                        t2.elapsed().as_secs_f64() * 1e3,
                                        rows.len()
                                    );
                                    Ok(Loaded { base: d.base, merge_base: d.merge_base, sources, files, rows, error, patch_hash, timings })
                                }
                                Err(e) => Err(crate::tr!(READING_FILES_E; e;)),
                            }
                        })
                        .await
                }
                Err(e) => Err(e.message),
            };
            let _ = this.update(cx, |p, cx| p.loaded(loaded, cx));
        })
        .detach();
    }

    fn loaded(&mut self, result: Result<Loaded, String>, cx: &mut Context<Self>) {
        self.loading = false;
        match result {
            Ok(l) => {
                self.load_error = l.error;
                let short = &l.merge_base[..l.merge_base.len().min(9)];
                self.base_label = crate::tr!(WORKTREE_VS_SHORT; short; l.base).into();
                if l.patch_hash != self.patch_hash || !self.loaded_once {
                    let keep = self.files.get(self.current_file).map(|f| f.path.clone());
                    self.patch_hash = l.patch_hash;
                    self.sources = Rc::new(l.sources);
                    self.files = Rc::new(l.rows);
                    // A fresh state per diff: `set_files` on a state that has not been
                    // painted yet left the list empty until the next interaction,
                    // and it resets expansion and viewport anyway. ~1ms for 10k lines.
                    self.set_state(new_state(l.files, self.split, self.collapsed, cx), cx);
                    // set_files resets the viewport: go back to the file being read.
                    self.current_file = keep
                        .and_then(|k| self.files.iter().position(|f| f.path == k))
                        .unwrap_or(0);
                    if self.loaded_once
                        && let Some(f) = self.files.get(self.current_file)
                    {
                        let path = f.path.clone();
                        self.state.update(cx, |s, cx| s.scroll_to_file(&path, cx));
                    }
                    println!(
                        "[review] {} loaded: {}{}",
                        self.task_id,
                        l.timings,
                        if self.loaded_once {
                            String::new()
                        } else {
                            format!(
                                " · ready {:.0}ms after open",
                                self.opened.elapsed().as_secs_f64() * 1e3
                            )
                        }
                    );
                }
                self.loaded_once = true;
            }
            Err(e) => self.load_error = Some(e),
        }
        if std::mem::take(&mut self.refresh_again) {
            self.refresh_diff(cx);
        }
        cx.notify();
    }

    fn set_state(&mut self, state: Entity<DiffState>, cx: &mut Context<Self>) {
        self._state_subs = watch_state(&state, cx);
        self.state = state;
    }

    /// Same files in a fresh state: the component only ever widens its
    /// content width (docs/05 §5-1), so after the diff column got narrower
    /// (split on, a pane shown) the right column would stay cut off.
    fn relayout(&mut self, cx: &mut Context<Self>) {
        let files = self.state.read(cx).files().to_vec();
        let state = new_state(files, self.split, self.collapsed, cx);
        self.set_state(state, cx);
        if let Some(f) = self
            .files
            .get(self.current_file)
            .filter(|_| self.current_file > 0)
        {
            let path = f.path.clone();
            self.state.update(cx, |s, cx| s.scroll_to_file(&path, cx));
        }
        cx.notify();
    }

    pub(crate) fn set_split(&mut self, split: bool, cx: &mut Context<Self>) {
        debug(format_args!("set_split {split}"));
        if self.split == split {
            return;
        }
        self.split = split;
        // Side by side needs wrapping in a dock-sized panel (docs/05 §5-1).
        self.wrap = split;
        self.relayout(cx);
    }

    fn toggle_collapse(&mut self, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        let lines = self.collapsed.then_some(CONTEXT_LINES);
        self.state
            .update(cx, |s, cx| s.set_context_lines(lines, cx));
        cx.notify();
    }

    fn go_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        debug(format_args!("go_file {ix}"));
        let Some(f) = self.files.get(ix) else { return };
        self.current_file = ix;
        let path = f.path.clone();
        self.state.update(cx, |s, cx| s.scroll_to_file(&path, cx));
        cx.notify();
    }

    fn step_file(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.files.len();
        if n == 0 {
            return;
        }
        let ix = if forward {
            (self.current_file + 1) % n
        } else {
            (self.current_file + n - 1) % n
        };
        self.go_file(ix, cx);
    }

    // ---- comments ---------------------------------------------------------

    fn reload_comments(&mut self, cx: &mut Context<Self>) {
        let rx = self
            .source
            .call::<p::ReviewCommentsList>(&p::ReviewListParams {
                task: Some(self.task_id.clone()),
                state: None,
            });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| match r {
                Ok(list) => {
                    p.comments = list.comments;
                    let ids: HashSet<&String> = p.comments.iter().map(|c| &c.id).collect();
                    p.checked.retain(|id| ids.contains(id));
                    cx.notify();
                }
                Err(e) => p.toast(
                    ToastKind::Error,
                    crate::strings::COULD_NOT_READ_REVIEW_COMMENTS.into(),
                    e.message,
                    cx,
                ),
            });
        })
        .detach();
    }

    fn open_draft(
        &mut self,
        path: String,
        side: DiffSide,
        line: usize,
        end_line: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        debug(format_args!("open_draft {path}:{line}-{end_line} {side:?}"));
        let (line, end_line) = (line.min(end_line) as u32, line.max(end_line) as u32);
        let side = side_of(side);
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .submit_on_enter(true)
                .placeholder(crate::tr!(COMMENT_ON_LINE_ENTER_SAVES_SHIFT_ENTER_NEW_LINE; line; side_label(side)))
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { shift: false, .. } = ev {
                this.save_draft(window, cx);
            }
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        self._subs.push(sub);
        self.draft = Some(Draft {
            path,
            side,
            line,
            end_line,
            input,
        });
        cx.notify();
    }

    /// "편집기에서 열기": `path` in this task's editor at `line` (new side),
    /// else at the hovered line of that file, else at its first change.
    pub fn open_in_editor(
        &mut self,
        path: String,
        line: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let hovered = self
            .hovered
            .as_ref()
            .filter(|h| h.path().as_ref() == path)
            .map(|h| h.line());
        let line = line
            .or(hovered)
            .or_else(|| {
                self.sources
                    .iter()
                    .find(|f| f.path == path)
                    .map(first_changed_line)
            })
            .unwrap_or(1)
            .max(1);
        let (ws, task) = (self.workspace.clone(), self.task_id.clone());
        window.defer(cx, move |window, cx| {
            let _ = ws.update(cx, |w, cx| {
                w.open_editor(&task, Some((path, line as u32, 1)), false, window, cx)
            });
        });
    }

    /// Ctrl+Shift+O: the hovered line, else the current file's first change.
    fn open_hovered_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .hovered
            .as_ref()
            .map(|h| (h.path().to_string(), Some(h.line())));
        let target = target.or_else(|| {
            self.files
                .get(self.current_file)
                .map(|f| (f.path.to_string(), None))
        });
        if let Some((path, line)) = target {
            self.open_in_editor(path, line, window, cx);
        }
    }

    /// Keyboard "Add comment": the selected lines, else the hovered line.
    fn add_comment_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(r) = self.state.read(cx).selected_lines() {
            self.open_draft(
                r.path().to_string(),
                r.side(),
                r.start(),
                r.end(),
                window,
                cx,
            );
        } else if let Some(pos) = self.hovered.clone() {
            self.open_draft(
                pos.path().to_string(),
                pos.side(),
                pos.line(),
                pos.line(),
                window,
                cx,
            );
        } else if let Some(source) = self
            .files
            .get(self.current_file)
            .and_then(|f| self.sources.iter().find(|s| s.path == f.path.as_ref()))
        {
            // Keyboard only (Alt+PageDown/F7, no pointer): the current file's
            // first change, so Ctrl+Shift+C never needs a mouse.
            let (path, line) = (source.path.clone(), first_changed_line(source));
            let side = if source.text(true).is_empty() {
                DiffSide::Original
            } else {
                DiffSide::Modified
            };
            self.open_draft(path, side, line, line, window, cx);
        } else {
            self.toast(
                ToastKind::Info,
                crate::strings::ADD_COMMENT_152.into(),
                crate::strings::SELECT_LINES_CLICK_A_LINE_NUMBER_SHIFT_OR_POINT_AT_A_LINE_FIRST
                    .into(),
                cx,
            );
        }
    }

    fn save_draft(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.draft.take() else { return };
        let body = d.input.read(cx).value().trim().to_string();
        cx.notify();
        if body.is_empty() {
            return;
        }
        // Quote what the reviewer saw (podd would otherwise read it now, docs/11 §1).
        let code = self
            .sources
            .iter()
            .find(|f| f.path == d.path)
            .map(|f| {
                f.lines(d.side == Side::New, d.line as usize, d.end_line as usize)
                    .trim_start_matches('\u{feff}')
                    .to_string()
            })
            .filter(|c| !c.is_empty());
        let comment = NewComment {
            file: d.path,
            side: d.side,
            line: d.line,
            end_line: (d.end_line != d.line).then_some(d.end_line),
            body,
            code,
        };
        let rx = self
            .source
            .call::<p::ReviewCommentsAdd>(&p::ReviewAddParams {
                task: self.task_id.clone(),
                comments: vec![comment],
            });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| match r {
                Ok(added) => {
                    p.comments.extend(added.comments);
                    cx.notify();
                }
                Err(e) => p.toast(
                    ToastKind::Error,
                    crate::strings::COMMENT_NOT_SAVED.into(),
                    e.message,
                    cx,
                ),
            });
        })
        .detach();
    }

    fn resolve(&mut self, id: String, reopen: bool, cx: &mut Context<Self>) {
        let rx = self
            .source
            .call::<p::ReviewCommentsResolve>(&p::ReviewResolveParams {
                ids: vec![id],
                note: None,
                reopen,
            });
        self.apply_comment_change(rx, cx);
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        let rx = self
            .source
            .call::<p::ReviewCommentsRemove>(&p::ReviewIdsParams {
                ids: vec![id.clone()],
            });
        self.checked.remove(&id);
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| match r {
                Ok(removed) => {
                    let gone: HashSet<String> =
                        removed.comments.into_iter().map(|c| c.id).collect();
                    p.comments.retain(|c| !gone.contains(&c.id));
                    cx.notify();
                }
                Err(e) => p.toast(
                    ToastKind::Error,
                    crate::strings::COULD_NOT_REMOVE_COMMENT.into(),
                    e.message,
                    cx,
                ),
            });
        })
        .detach();
    }

    fn apply_comment_change(
        &mut self,
        rx: impl std::future::Future<Output = Result<p::ReviewCommentsResult, SourceError>> + 'static,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| match r {
                Ok(changed) => {
                    for c in changed.comments {
                        if let Some(x) = p.comments.iter_mut().find(|x| x.id == c.id) {
                            *x = c;
                        }
                    }
                    cx.notify();
                }
                Err(e) => p.toast(
                    ToastKind::Error,
                    crate::strings::REVIEW_UPDATE_FAILED.into(),
                    e.message,
                    cx,
                ),
            });
        })
        .detach();
    }

    fn scroll_to_comment(&mut self, c: &ReviewComment, cx: &mut Context<Self>) {
        let pos = DiffLinePosition::new(c.file.clone(), diff_side(c.side), c.line as usize);
        if let Some(ix) = self.files.iter().position(|f| f.path.as_ref() == c.file) {
            self.current_file = ix;
        }
        self.state.update(cx, |s, cx| s.scroll_to_line(pos, cx));
        cx.notify();
    }

    // ---- send / deliveries ---------------------------------------------------

    fn send(&mut self, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let selection = send_selection(&self.comments, &self.checked);
        if selection.as_ref().is_some_and(|ids| ids.is_empty()) {
            let text = crate::strings::CHECKED_COMMENTS_ALL_RESOLVED.to_string();
            self.toast(
                ToastKind::Warning,
                crate::tr!(REVIEW_NOT_SENT_TO; ; self.task_id),
                text.clone(),
                cx,
            );
            self.outcome = Some(SendOutcome::Refused(text));
            cx.notify();
            return;
        }
        let instructions = self.instructions.read(cx).value().trim().to_string();
        let params = p::ReviewSendParams {
            task: self.task_id.clone(),
            all_open: selection.is_none(),
            comment_ids: selection.unwrap_or_default(),
            instructions: (!instructions.is_empty()).then_some(instructions),
            ..Default::default()
        };
        let rx = self.source.call::<p::ReviewSend>(&params);
        self.sending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                p.sending = false;
                match r {
                    Ok(d) => {
                        let (text, _) = delivery_text(&d);
                        match d.state {
                            DeliveryState::Delivered => p.toast(
                                ToastKind::Success,
                                crate::tr!(REVIEW_SENT_TO; ; p.task_id),
                                text,
                                cx,
                            ),
                            _ => p.toast(
                                ToastKind::Info,
                                crate::tr!(REVIEW_QUEUED_FOR; ; p.task_id),
                                text,
                                cx,
                            ),
                        }
                        p.outcome = Some(SendOutcome::Delivery(d.id.clone()));
                        p.deliveries.retain(|x| x.id != d.id);
                        p.deliveries.push(d);
                        p.checked.clear();
                        p.clear_instructions = true;
                        p.ensure_poll(cx);
                    }
                    Err(e) => {
                        let text = refusal_text(&e);
                        p.toast(
                            ToastKind::Warning,
                            crate::tr!(REVIEW_NOT_SENT_TO; ; p.task_id),
                            text.clone(),
                            cx,
                        );
                        p.outcome = Some(SendOutcome::Refused(text));
                    }
                }
                p.reload_comments(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn cancel_delivery(&mut self, id: String, cx: &mut Context<Self>) {
        let rx = self.source.call::<p::ReviewCancel>(&p::DeliveryRef { id });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                if let Err(e) = r {
                    p.toast(
                        ToastKind::Error,
                        crate::strings::CANCEL_FAILED.into(),
                        e.message,
                        cx,
                    );
                }
                p.reload_deliveries(cx);
                p.reload_comments(cx);
            });
        })
        .detach();
    }

    fn reload_deliveries(&mut self, cx: &mut Context<Self>) {
        let rx = self
            .source
            .call::<p::ReviewDeliveries>(&p::ReviewDeliveriesParams {
                task: Some(self.task_id.clone()),
            });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                let Ok(list) = r else { return };
                for (id, s) in list.submits {
                    p.submit_changed(&id, s, cx);
                }
                let mut after = list.deliveries;
                after.sort_by_key(|d| d.created_ms);
                let finished: Vec<DeliveryInfo> = finished_deliveries(&p.deliveries, &after).into_iter().cloned().collect();
                let delivered = finished.iter().any(|d| d.state == DeliveryState::Delivered);
                for d in finished {
                    let (text, _) = delivery_text(&d);
                    let kind = if d.state == DeliveryState::Delivered { ToastKind::Success } else { ToastKind::Warning };
                    p.toast(
                        kind,
                        crate::tr!(REVIEW_FOR; ; if d.state == DeliveryState::Delivered { crate::strings::DELIVERED } else { crate::strings::NOT_DELIVERED }, p.task_id),
                        text,
                        cx,
                    );
                }
                p.deliveries = after;
                if delivered {
                    p.reload_comments(cx);
                }
                p.ensure_poll(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Re-read deliveries every 2 s while one is queued — only when podd does
    /// not announce delivery changes (`review.delivery`, see module docs).
    fn ensure_poll(&mut self, cx: &mut Context<Self>) {
        let queued = self
            .deliveries
            .iter()
            .any(|d| d.state == DeliveryState::Queued);
        if !queued || crate::source::delivery_events() {
            self.poll_task = None;
            return;
        }
        if self.poll_task.is_some() {
            return;
        }
        self.poll_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(QUEUED_POLL).await;
                if crate::source::delivery_events() {
                    break; // events took over (first one arrived after we started)
                }
                let more = this
                    .update(cx, |p, cx| {
                        p.reload_deliveries(cx);
                        p.deliveries
                            .iter()
                            .any(|d| d.state == DeliveryState::Queued)
                    })
                    .unwrap_or(false);
                if !more {
                    break;
                }
            }
            let _ = this.update(cx, |p, _| p.poll_task = None);
        }));
    }

    // ---- rendering ------------------------------------------------------------

    fn open_count(&self) -> usize {
        self.comments
            .iter()
            .filter(|c| c.state == CommentState::Open)
            .count()
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (adds, dels) = self
            .files
            .iter()
            .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
        // Icon tools (docs/10 §D4.6). Accessible names stay what they were: the
        // tooltip for icon-only tools, the earlier label (with its ✓ state) for toggles.
        let icon_tool = |id: &'static str, glyph: pod_ui::Icon, tip: &'static str| {
            Button::new(id)
                .icon(pod_ui::kit_icon(glyph))
                .small()
                .ghost()
                .tooltip(tip)
                .accessibility_label(tip)
        };
        let toggle = |id: &'static str,
                      glyph: pod_ui::Icon,
                      visible: &'static str,
                      name: &'static str,
                      on: bool,
                      tip: &'static str| {
            Button::new(id)
                .icon(pod_ui::kit_icon(glyph))
                .label(visible)
                .small()
                .ghost()
                .when(on, |b| gpui_kit::component::Selectable::selected(b, true))
                .tooltip(tip)
                .accessibility_label(name)
        };
        let segment = |id: &'static str, glyph: pod_ui::Icon, label: &'static str, on: bool| {
            Button::new(id)
                .icon(pod_ui::kit_icon(glyph))
                .label(label)
                .small()
                .ghost()
                .when(on, |b| gpui_kit::component::Selectable::selected(b, true))
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .min_h(px(36.))
            .py_1()
            .bg(rgb(color::bar()))
            .border_b_1()
            .border_color(rgb(color::border()))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(color::muted()))
                    .child(self.base_label.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .flex()
                    .gap_1()
                    .child(crate::tr!(FILES; ; self.files.len()))
                    .child(
                        div()
                            .text_color(rgb(color::add()))
                            .child(format!("+{adds}")),
                    )
                    .child(
                        div()
                            .text_color(rgb(color::del()))
                            .child(format!("−{dels}")),
                    ),
            )
            .when(self.loading, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(color::warn()))
                        .child(crate::strings::REFRESHING),
                )
            })
            .child(div().flex_1())
            // Segmented "나란히 / 한 줄로": neutral selected face, no filled accent.
            .child(
                pod_ui::segmented(crate::theme::mode())
                    .child(
                        segment(
                            "rv-split",
                            pod_ui::Icon::Columns2,
                            crate::strings::SPLIT,
                            self.split,
                        )
                        .tooltip(
                            crate::strings::SIDE_BY_SIDE_NEEDS_A_WIDE_PANEL_HIDE_FILES_COMMENTS,
                        )
                        .on_click(cx.listener(|p, _, _, cx| p.set_split(true, cx))),
                    )
                    .child(
                        segment(
                            "rv-unified",
                            pod_ui::Icon::Rows2,
                            crate::strings::INLINE,
                            !self.split,
                        )
                        .on_click(cx.listener(|p, _, _, cx| p.set_split(false, cx))),
                    ),
            )
            .child(
                Button::new("rv-collapse")
                    .icon(pod_ui::kit_icon(pod_ui::Icon::ChevronsUpDown))
                    .label(if self.collapsed {
                        crate::strings::EXPAND
                    } else {
                        crate::strings::COLLAPSE
                    })
                    .small()
                    .ghost()
                    .tooltip(crate::strings::SHOW_OR_FOLD_UNCHANGED_LINES)
                    .on_click(cx.listener(|p, _, _, cx| p.toggle_collapse(cx))),
            )
            .child(
                toggle(
                    "rv-wrap",
                    pod_ui::Icon::WrapText,
                    crate::strings::WRAP_169,
                    if self.wrap {
                        crate::strings::WRAP
                    } else {
                        crate::strings::WRAP_169
                    },
                    self.wrap,
                    crate::strings::SOFT_WRAP_LONG_LINES,
                )
                .on_click(cx.listener(|p, _, _, cx| {
                    p.wrap = !p.wrap;
                    cx.notify();
                })),
            )
            .child(
                icon_tool(
                    "rv-prev",
                    pod_ui::Icon::ArrowUp,
                    crate::strings::PREVIOUS_CHANGE_SHIFT_F7_ALT,
                )
                .on_click(
                    cx.listener(|p, _, _, cx| p.state.update(cx, |s, cx| s.previous_change(cx))),
                ),
            )
            .child(
                icon_tool(
                    "rv-next",
                    pod_ui::Icon::ArrowDown,
                    crate::strings::NEXT_CHANGE_F7_ALT,
                )
                .on_click(cx.listener(|p, _, _, cx| p.state.update(cx, |s, cx| s.next_change(cx)))),
            )
            .child(
                icon_tool(
                    "rv-pfile",
                    pod_ui::Icon::ChevronLeft,
                    crate::strings::PREVIOUS_FILE_ALT_PGUP,
                )
                .on_click(cx.listener(|p, _, _, cx| p.step_file(false, cx))),
            )
            .child(
                icon_tool(
                    "rv-nfile",
                    pod_ui::Icon::ChevronRight,
                    crate::strings::NEXT_FILE_ALT_PGDN,
                )
                .on_click(cx.listener(|p, _, _, cx| p.step_file(true, cx))),
            )
            .child(
                toggle(
                    "rv-files",
                    pod_ui::Icon::PanelLeft,
                    crate::strings::FILES_176,
                    if self.show_files {
                        crate::strings::FILES_175
                    } else {
                        crate::strings::FILES_176
                    },
                    self.show_files,
                    crate::strings::SHOW_OR_HIDE_THE_FILE_LIST,
                )
                .on_click(cx.listener(|p, _, _, cx| {
                    p.show_files = !p.show_files;
                    p.relayout(cx);
                })),
            )
            .child({
                let (on, tip) = if self.comments_drawer {
                    (self.drawer_open, COMMENTS_DRAWER_TIP)
                } else {
                    (
                        self.show_comments,
                        crate::strings::SHOW_OR_HIDE_THE_COMMENTS_PANE,
                    )
                };
                toggle(
                    "rv-notes",
                    pod_ui::Icon::MessageSquare,
                    crate::strings::COMMENTS_179,
                    if on {
                        crate::strings::COMMENTS
                    } else {
                        crate::strings::COMMENTS_179
                    },
                    on,
                    tip,
                )
                .on_click(cx.listener(|p, _, window, cx| p.toggle_comments(window, cx)))
            })
            .child(
                icon_tool(
                    "rv-refresh",
                    pod_ui::Icon::RefreshCw,
                    crate::strings::RE_READ_CHANGES_COMMENTS_AND_DELIVERIES_F5,
                )
                .on_click(cx.listener(|p, _, _, cx| p.refresh_all(cx))),
            )
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.files.clone();
        let mut per_file: HashMap<String, usize> = HashMap::new();
        for c in self
            .comments
            .iter()
            .filter(|c| c.state != CommentState::Resolved)
        {
            *per_file.entry(c.file.clone()).or_default() += 1;
        }
        let per_file = Rc::new(per_file);
        let current = self.current_file;
        div()
            .w(px(250.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(color::pane()))
            .border_r_1()
            .border_color(rgb(color::border()))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(color::muted()))
                    .child(crate::tr!(FILES_182; ; files.len())),
            )
            .child(
                uniform_list(
                    "rv-files",
                    files.len(),
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        range
                            .map(|ix| {
                                let f = &this.files[ix];
                                let (letter, mark) = f.status_mark();
                                let notes = per_file.get(f.path.as_ref()).copied().unwrap_or(0);
                                div()
                                    .id(("rv-file", ix))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_3()
                                    .h(px(24.))
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(ix == current, |d| d.bg(rgb(color::selected())))
                                    .hover(|s| s.bg(rgb(color::hover())))
                                    .on_click(cx.listener(move |p, e: &ClickEvent, window, cx| {
                                        if e.click_count() > 1 {
                                            let path = p.files[ix].path.to_string();
                                            p.open_in_editor(path, None, window, cx);
                                        } else {
                                            p.go_file(ix, cx);
                                        }
                                    }))
                                    .context_menu({
                                        let (weak, path) =
                                            (cx.entity().downgrade(), f.path.to_string());
                                        move |menu, _, _| {
                                            let (weak, path) = (weak.clone(), path.clone());
                                            menu.item(
                                                PopupMenuItem::new(crate::strings::OPEN_IN_EDITOR)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = weak.update(cx, |p, cx| {
                                                            p.open_in_editor(
                                                                path.clone(),
                                                                None,
                                                                window,
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                            )
                                        }
                                    })
                                    .child(div().flex_none().text_color(rgb(mark)).child(letter))
                                    .child({
                                        // Ellipsized path; the whole path is the tooltip (no mid-word wrap).
                                        let tip = f.path.clone();
                                        div()
                                            .id(("rv-file-path", ix))
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .tooltip(move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    tip.clone(),
                                                )
                                                .build(window, cx)
                                            })
                                            .child(f.path.clone())
                                    })
                                    .when(notes > 0, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::info()))
                                                .child(format!("💬{notes}")),
                                        )
                                    })
                                    .when(f.eol_only, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::muted()))
                                                .child(crate::strings::EOL_ONLY),
                                        )
                                    })
                                    .when(f.binary, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::muted()))
                                                .child(crate::strings::BINARY),
                                        )
                                    })
                                    .when(!f.binary, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::add()))
                                                .child(format!("+{}", f.additions)),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::del()))
                                                .child(format!("−{}", f.deletions)),
                                        )
                                    })
                            })
                            .collect()
                    }),
                )
                .flex_1(),
            )
    }

    /// "코멘트 | PR" switch at the top of the sidebar (P6).
    fn render_sidebar_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = |id: &'static str,
                   label: &'static str,
                   tip: &'static str,
                   on: bool,
                   pr: bool,
                   cx: &mut Context<Self>| {
            Button::new(id)
                .label(label)
                .xsmall()
                .tooltip(tip)
                .when(on, |b| b.primary())
                .when(!on, |b| b.ghost())
                .on_click(cx.listener(move |p, _, _, cx| p.set_pr_tab(pr, cx)))
        };
        let pr_label = self
            .pr
            .read(cx)
            .info()
            .and_then(|i| i.status.pr.as_ref())
            .map(|p| format!("#{}", p.number));
        div()
            .qa_id(|| "review-sidebar-tabs".into())
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .pt_1()
            .child(tab(
                "rv-tab-comments",
                crate::pr_panel::s::TAB_COMMENTS,
                crate::pr_panel::s::TAB_COMMENTS_TIP,
                !self.pr_tab,
                false,
                cx,
            ))
            .child(tab(
                "rv-tab-pr",
                crate::pr_panel::s::TAB_PR,
                crate::pr_panel::s::TAB_PR_TIP,
                self.pr_tab,
                true,
                cx,
            ))
            .when_some(pr_label, |d, l| {
                d.child(div().text_xs().text_color(rgb(color::muted())).child(l))
            })
            .into_any_element()
    }

    /// Show or hide the file list (fixture scenes).
    pub fn set_file_list(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_files = show;
        self.relayout(cx);
    }

    /// D4.6: below [`COMMENTS_DRAWER_BELOW`] px of canvas the comment column leaves the
    /// row and opens over the diff as a drawer (toolbar 의견, Alt+C; Esc closes).
    pub fn set_comments_drawer(&mut self, drawer: bool, cx: &mut Context<Self>) {
        if self.comments_drawer == drawer {
            return;
        }
        self.comments_drawer = drawer;
        self.drawer_open = false;
        self.relayout(cx);
    }

    #[cfg(test)]
    pub fn comments_drawer(&self) -> bool {
        self.comments_drawer
    }

    pub fn comments_drawer_open(&self) -> bool {
        self.comments_drawer && self.drawer_open
    }

    /// Open the drawer and move keyboard focus into it.
    pub fn open_comments_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.comments_drawer {
            return;
        }
        self.drawer_open = true;
        window.focus(&self.drawer_focus, cx);
        cx.notify();
    }

    /// Close the drawer; focus goes back to the Review panel.
    pub fn close_comments_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.drawer_open {
            return;
        }
        self.drawer_open = false;
        if self.drawer_focus.contains_focused(window, cx) {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    #[cfg(test)]
    pub fn drawer_focus_handle(&self) -> FocusHandle {
        self.drawer_focus.clone()
    }

    /// Focus the instructions box (in the drawer when it is open) and return its handle.
    #[cfg(test)]
    pub fn instructions_focus(&self, window: &mut Window, cx: &mut App) -> FocusHandle {
        let f = self.instructions.focus_handle(cx);
        window.focus(&f, cx);
        f
    }

    /// Toolbar 의견 / Alt+C: the column shows or hides, or the drawer opens or closes.
    fn toggle_comments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.comments_drawer {
            if self.drawer_open {
                self.close_comments_drawer(window, cx);
            } else {
                self.open_comments_drawer(window, cx);
            }
        } else {
            self.show_comments = !self.show_comments;
            self.relayout(cx);
        }
    }

    fn set_pr_tab(&mut self, on: bool, cx: &mut Context<Self>) {
        self.pr_tab = on;
        self.pr.update(cx, |p, cx| p.set_visible(on, cx));
        cx.notify();
    }

    /// Show the PR tab (and the sidebar); orm opens the create form when
    /// the task has no pull request yet (workspace open_pr).
    pub fn show_pr(&mut self, form: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.comments_drawer {
            self.open_comments_drawer(window, cx);
        } else if !self.show_comments {
            self.show_comments = true;
            self.relayout(cx);
        }
        self.set_pr_tab(true, cx);
        if form {
            self.pr.update(cx, |p, cx| {
                if p.info().is_none_or(|i| i.status.pr.is_none()) {
                    p.open_form(window, cx);
                }
            });
        }
    }

    #[cfg(test)]
    pub fn pr_section(&self) -> Entity<crate::pr_panel::PrSection> {
        self.pr.clone()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.render_sidebar_tabs(cx);
        if self.pr_tab {
            return div()
                .w(px(COMMENTS_W))
                .min_w(px(COMMENTS_MIN_W))
                .flex_shrink(1.)
                .h_full()
                .flex()
                .flex_col()
                .bg(rgb(color::pane()))
                .border_l_1()
                .border_color(rgb(color::border()))
                .child(tabs)
                .child(div().flex_1().min_h_0().child(self.pr.clone()))
                .into_any_element();
        }
        let visible: Vec<ReviewComment> = self
            .comments
            .iter()
            .filter(|c| self.show_resolved || c.state != CommentState::Resolved)
            .cloned()
            .collect();
        let resolved = self
            .comments
            .iter()
            .filter(|c| c.state == CommentState::Resolved)
            .count();
        let selection = send_selection(&self.comments, &self.checked);
        let send_label = match (&selection, self.open_count()) {
            (Some(ids), _) => crate::tr!(SEND_SELECTED_TO_AGENT; ; ids.len()),
            (None, n) => crate::tr!(SEND_N_OPEN_TO_AGENT; n;),
        };
        let can_send = !self.sending && (selection.is_some() || self.open_count() > 0);
        let agent = match &self.agent_status {
            Some(s) => {
                let hint = match s.state {
                    AgentState::Working => crate::strings::WILL_QUEUE_UNTIL_IDLE,
                    AgentState::Blocked => crate::strings::SEND_WILL_BE_REFUSED,
                    _ => crate::strings::DELIVERS_NOW,
                };
                crate::tr!(AGENT_HINT; hint; self.agent_terminal.as_deref().unwrap_or("?"), s.label())
            }
            None => crate::tr!(AGENT_189; ; self.agent_terminal.as_deref().unwrap_or("—")),
        };
        let weak = cx.entity().downgrade();
        let mut list = div()
            .id("rv-comments")
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if visible.is_empty() {
            list = list.child(
                words(crate::strings::NO_COMMENTS_YET_CLICK_A_LINE_OR_DRAG_LINE_NUMBERS_THEN_TO_COMMENT, "rv-hint").text_xs().text_color(rgb(color::muted())),
            );
        }
        for c in &visible {
            let c2 = c.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("rv-card-{}", c.id)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |p, _, _, cx| p.scroll_to_comment(&c2, cx)))
                    .child(comment_card(c, true, self.checked.contains(&c.id), &weak)),
            );
        }
        let deliveries = self.deliveries.iter().rev().take(4).map(|d| {
            let (text, col) = delivery_text(d);
            let id = d.id.clone();
            let submit = self
                .submits
                .get(&d.id)
                .filter(|_| d.state == DeliveryState::Delivered);
            let unconfirmed = submit.is_some_and(|s| s.state == p::SubmitState::Unconfirmed);
            let term = d.terminal.clone();
            div()
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(words(&text, "rv-delivery").text_color(rgb(col)))
                        .when_some(submit.and_then(submit_text), |c, (t, sc)| {
                            c.child(words(&t, "rv-submit").text_color(rgb(sc)))
                        }),
                )
                .when(unconfirmed, |row| {
                    row.child(
                        Button::new(SharedString::from(format!("rv-enter-{}", d.id)))
                            .label(crate::strings::PRESS_ENTER)
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(move |p, _, _, cx| p.press_enter(&term, cx))),
                    )
                })
                .when(d.state == DeliveryState::Queued, |row| {
                    row.child(
                        Button::new(SharedString::from(format!("rv-cancel-{}", d.id)))
                            .label(crate::strings::CANCEL)
                            .xsmall()
                            .ghost()
                            .on_click(
                                cx.listener(move |p, _, _, cx| p.cancel_delivery(id.clone(), cx)),
                            ),
                    )
                })
        });
        let outcome = match &self.outcome {
            Some(SendOutcome::Refused(t)) => Some((t.clone(), color::del())),
            _ => None,
        };
        div()
            .w(px(COMMENTS_W))
            .min_w(px(COMMENTS_MIN_W))
            .flex_shrink(1.)
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(color::pane()))
            .border_l_1()
            .border_color(rgb(color::border()))
            .child(tabs)
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(color::muted()))
                    .child(words(&crate::tr!(COMMENTS_OPEN_TOTAL; ; self.open_count(), self.comments.len()), "rv-count").flex_1())
                    .when(resolved > 0, |d| {
                        d.child(
                            Button::new("rv-show-resolved")
                                .label(if self.show_resolved {
                                    crate::tr!(HIDE_RESOLVED_RESOLVED; resolved;)
                                } else {
                                    crate::tr!(SHOW_RESOLVED_RESOLVED; resolved;)
                                })
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(|p, _, _, cx| {
                                    p.show_resolved = !p.show_resolved;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .child(list)
            .child(
                div()
                    .qa_id(|| "review-composer".into())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .border_t_1()
                    .border_color(rgb(color::border()))
                    .child(words(crate::strings::SEND_TO_AGENT_194, "rv-send-title").text_xs().text_color(rgb(color::muted())))
                    .child(Textarea::new(&self.instructions))
                    .child(
                        Button::new("rv-send")
                            .label(if self.sending { crate::strings::SENDING.to_string() } else { send_label })
                            .primary()
                            .small()
                            .disabled(!can_send)
                            .tooltip(crate::strings::REVIEW_SEND_CTRL_SHIFT_ENTER_CHECKED_COMMENTS_ELSE_ALL_OPEN_ONES)
                            .on_click(cx.listener(|p, _, _, cx| p.send(cx))),
                    )
                    .child(words(&agent, "rv-agent").text_xs().text_color(rgb(color::muted())))
                    .when_some(outcome, |d, (t, col)| d.child(words(&t, "rv-outcome").text_xs().text_color(rgb(col))))
                    .children(deliveries),
            )
            .into_any_element()
    }

    /// The comment column as a drawer over the diff (D4.6, < 560 px): a dialog with
    /// its own focus and key context, so Esc closes it and focus returns to the panel.
    fn render_drawer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("rv-comments-drawer")
            .qa_id(|| "review-comments-drawer".into())
            .track_focus(&self.drawer_focus)
            .key_context(DRAWER_CONTEXT)
            .role(Role::Dialog)
            .aria_label(COMMENTS_DRAWER_NAME)
            .on_action(cx.listener(|p, _: &CloseCommentsDrawer, window, cx| {
                p.close_comments_drawer(window, cx)
            }))
            .occlude()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(COMMENTS_W))
            .max_w_full()
            .flex()
            .shadow_lg()
            .child(self.render_sidebar(cx))
            .into_any_element()
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> Diff {
        let present: HashSet<&str> = self.files.iter().map(|f| f.path.as_ref()).collect();
        let mut views: HashMap<ElementId, AnnView> = HashMap::new();
        let mut annotations = Vec::new();
        for c in self
            .comments
            .iter()
            .filter(|c| c.state != CommentState::Resolved && present.contains(c.file.as_str()))
        {
            let id = ElementId::from(SharedString::from(format!("rv-ann-{}", c.id)));
            let line = c.end_line.unwrap_or(c.line) as usize;
            annotations.push(DiffAnnotation::line(
                id.clone(),
                DiffLinePosition::new(c.file.clone(), diff_side(c.side), line),
            ));
            views.insert(id, AnnView::Comment(c.clone()));
        }
        if let Some(d) = &self.draft {
            let id = ElementId::from(SharedString::from("rv-draft"));
            annotations.push(DiffAnnotation::line(
                id.clone(),
                DiffLinePosition::new(d.path.clone(), diff_side(d.side), d.end_line as usize),
            ));
            let label = if d.line == d.end_line {
                format!("{}:{}", side_label(d.side), d.line)
            } else {
                format!("{}:{}–{}", side_label(d.side), d.line, d.end_line)
            };
            views.insert(
                id,
                AnnView::Draft {
                    input: d.input.clone(),
                    title: crate::tr!(NEW_COMMENT_ON_LABEL; label; d.path).into(),
                },
            );
        }
        let views = Rc::new(views);
        let weak = cx.entity().downgrade();
        let (weak_ann, weak_click, weak_add, weak_hover, weak_open) =
            (weak.clone(), weak.clone(), weak.clone(), weak.clone(), weak);
        Diff::new(&self.state)
            .size_full()
            .soft_wrap(self.wrap)
            .hover_highlight(DiffHoverHighlight::Both)
            .annotations(annotations)
            .render_annotation(move |ann, _window, _cx| match views.get(ann.id()) {
                Some(AnnView::Comment(c)) => div()
                    .mx_2()
                    .my_1()
                    .child(comment_card(c, false, false, &weak_ann))
                    .into_any_element(),
                Some(AnnView::Draft { input, title }) => {
                    let (w_save, w_cancel) = (weak_ann.clone(), weak_ann.clone());
                    div()
                        .mx_2()
                        .my_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(color::accent()))
                        .bg(rgb(color::draft_bg()))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(color::muted()))
                                .child(title.clone()),
                        )
                        .child(Textarea::new(input))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new("rv-draft-save")
                                        .label(crate::strings::COMMENTS_179)
                                        .small()
                                        .primary()
                                        .on_click(move |_, window, cx| {
                                            let _ =
                                                w_save.update(cx, |p, cx| p.save_draft(window, cx));
                                        }),
                                )
                                .child(
                                    Button::new("rv-draft-cancel")
                                        .label(crate::strings::CANCEL)
                                        .small()
                                        .ghost()
                                        .on_click(move |_, _, cx| {
                                            let _ = w_cancel.update(cx, |p, cx| {
                                                p.draft = None;
                                                cx.notify();
                                            });
                                        }),
                                ),
                        )
                        .into_any_element()
                }
                None => div().into_any_element(),
            })
            .on_line_click(move |pos, _ev, window, cx| {
                let (path, side, line) = (pos.path().to_string(), pos.side(), pos.line());
                let _ =
                    weak_click.update(cx, |p, cx| p.open_draft(path, side, line, line, window, cx));
            })
            .on_add_annotation(move |range, window, cx| {
                let (path, side, start, end) = (
                    range.path().to_string(),
                    range.side(),
                    range.start(),
                    range.end(),
                );
                let _ =
                    weak_add.update(cx, |p, cx| p.open_draft(path, side, start, end, window, cx));
            })
            .on_line_hover(move |pos, _window, cx| {
                let pos = pos.cloned();
                let _ = weak_hover.update(cx, |p, _| p.hovered = pos);
            })
            .render_header_prefix(move |file, _window, _cx| {
                let (weak, path) = (weak_open.clone(), file.path().to_string());
                Button::new(SharedString::from(format!("rv-open-{path}")))
                    .label(crate::strings::OPEN_IN_EDITOR_SHORT)
                    .accessibility_label(format!("{} {path}", crate::strings::OPEN_IN_EDITOR))
                    .tooltip(crate::strings::OPEN_IN_EDITOR_TIP)
                    .xsmall()
                    .ghost()
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let _ = weak
                            .update(cx, |p, cx| p.open_in_editor(path.clone(), None, window, cx));
                    })
            })
    }
}

/// One comment card (sidebar and inline). Handlers go through a weak handle
/// because inline cards are built inside the diff's annotation renderer.
fn comment_card(
    c: &ReviewComment,
    in_sidebar: bool,
    checked: bool,
    weak: &WeakEntity<ReviewPanel>,
) -> Div {
    let key = |s: &str| {
        SharedString::from(format!(
            "{s}-{}-{}",
            if in_sidebar { "s" } else { "i" },
            c.id
        ))
    };
    let resolved = c.state == CommentState::Resolved;
    let (id_res, id_rm, id_chk) = (c.id.clone(), c.id.clone(), c.id.clone());
    let (w_res, w_rm, w_chk) = (weak.clone(), weak.clone(), weak.clone());
    let header = div()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(rgb(color::muted()))
        .when(in_sidebar && !resolved, |d| {
            d.child(
                div()
                    .id(key("chk"))
                    .cursor_pointer()
                    .px_1()
                    .child(if checked { "☑" } else { "☐" })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = w_chk.update(cx, |p, cx| {
                            if !p.checked.remove(&id_chk) {
                                p.checked.insert(id_chk.clone());
                            }
                            cx.notify();
                        });
                    }),
            )
        })
        .child({
            let place: SharedString = if in_sidebar {
                format!("{} · {}", c.id, location(c))
            } else {
                format!("💬 {} · {}", c.id, location(c))
            }
            .into();
            let tip = place.clone();
            div()
                .id(key("place"))
                .flex_1()
                .min_w_0()
                .truncate()
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .child(place)
        })
        .child(
            div()
                .text_color(rgb(state_color(c.state)))
                .child(state_label(c.state)),
        );
    let actions = div()
        .flex()
        .gap_1()
        .child(
            Button::new(key("res"))
                .label(if resolved {
                    crate::strings::REOPEN
                } else {
                    crate::strings::RESOLVE
                })
                .xsmall()
                .ghost()
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = w_res.update(cx, |p, cx| p.resolve(id_res.clone(), resolved, cx));
                }),
        )
        .child(
            Button::new(key("rm"))
                .label(crate::strings::DELETE)
                .xsmall()
                .ghost()
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = w_rm.update(cx, |p, cx| p.remove(id_rm.clone(), cx));
                }),
        );
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(if checked {
            color::accent()
        } else {
            color::border()
        }))
        .bg(rgb(if in_sidebar {
            color::card()
        } else {
            color::draft_bg()
        }))
        .when(resolved, |d| d.opacity(0.6))
        .child(header)
        .child(
            div()
                .flex()
                .flex_col()
                .text_sm()
                .children(c.body.lines().map(|l| words(l, "rv-body").min_h(px(18.)))),
        )
        .when_some(c.note.clone(), |d, n| {
            d.child(
                words(&crate::tr!(NOTE_N; n;), "rv-note")
                    .text_xs()
                    .text_color(rgb(color::muted())),
            )
        })
        .child(actions)
}

/// Prose that wraps only between words (CSS `word-break: keep-all`, docs/10 §D4.6):
/// GPUI breaks Korean between any two syllables, so each word is its own flex piece
/// (pod_ui::keep_all_words), left-aligned. A piece is laid out at its full width, so a
/// word that fits the line is never split; only a word wider than the whole line (a long
/// path, URL or identifier) is capped at the line and wraps inside itself (break-all),
/// so no text is ever hidden (ui-editor-05). Tests find piece `i` as `{qa}-w{i}`.
pub(crate) fn words(text: &str, qa: &'static str) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .gap_x(px(4.))
        .children(
            pod_ui::keep_all_words(text)
                .into_iter()
                .enumerate()
                .map(|(i, w)| {
                    div()
                        .max_w_full()
                        .min_w_0()
                        .qa_id(move || format!("{qa}-w{i}"))
                        .child(SharedString::from(w.to_string()))
                }),
        )
}

/// What an inline annotation shows.
#[derive(Clone)]
enum AnnView {
    Comment(ReviewComment),
    Draft {
        input: Entity<TextareaState>,
        title: SharedString,
    },
}

fn debug(args: std::fmt::Arguments) {
    if std::env::var_os("POD_APP_DEBUG").is_some() {
        println!("[review] {args}");
    }
}

/// Re-render on state changes, and turn a finished multi-line line-number
/// drag into a range draft.
fn watch_state(state: &Entity<DiffState>, cx: &mut Context<ReviewPanel>) -> [Subscription; 2] {
    [
        cx.observe(state, |_, _, cx| cx.notify()),
        cx.subscribe(state, |p, _, ev: &DiffEvent, cx| {
            if let DiffEvent::SelectionEnded(r) = ev
                && r.start() != r.end()
            {
                p.pending_range = Some(r.clone());
                cx.notify();
            }
        }),
    ]
}

fn new_state<T: 'static>(
    files: Vec<DiffFile>,
    split: bool,
    collapsed: bool,
    cx: &mut Context<T>,
) -> Entity<DiffState> {
    cx.new(|cx| {
        DiffState::new(files, cx)
            .with_mode(if split {
                DiffMode::Split
            } else {
                DiffMode::Unified
            })
            .with_context_lines(collapsed.then_some(CONTEXT_LINES))
    })
}

/// What, in the task summary, changes when the agent changes files.
fn git_signature(task: &TaskVm) -> Option<String> {
    task.git.as_ref().map(|g| {
        format!(
            "{}|{}|{}",
            g.ahead,
            g.dirty,
            g.last_commit.as_deref().unwrap_or("")
        )
    })
}

/// 1-based first line (new side) where the file differs from its old version.
fn first_changed_line(f: &SourceFile) -> usize {
    let (old, new) = (f.text(false), f.text(true));
    let n = old
        .lines()
        .zip(new.lines())
        .take_while(|(a, b)| a == b)
        .count();
    (n + 1).min(new.lines().count().max(1))
}

fn hash(s: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
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

impl gpui_kit::component::dock::Panel for ReviewPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(crate::tr!(REVIEW; ; self.task_id).into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let open = self.open_count();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(crate::tr!(REVIEW; ; self.task_id))
            .when(open > 0, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(color::info()))
                        .child(format!("💬{open}")),
                )
            })
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for ReviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        debug(format_args!(
            "render: {} files, draft {}",
            self.files.len(),
            self.draft.is_some()
        ));
        if std::mem::take(&mut self.clear_instructions) {
            self.instructions
                .update(cx, |i, cx| i.set_value("", window, cx));
        }
        if let Some(r) = self.pending_range.take() {
            cx.defer_in(window, move |p, window, cx| {
                p.open_draft(
                    r.path().to_string(),
                    r.side(),
                    r.start(),
                    r.end(),
                    window,
                    cx,
                )
            });
        }
        let center = if let Some(e) = self.load_error.clone().filter(|_| self.files.is_empty()) {
            // Inline error banner (docs/10 §D4.6) with a retry, instead of raw red text.
            div()
                .size_full()
                .p_4()
                .text_sm()
                .child(
                    pod_ui::banner(
                        pod_ui::Tone::Danger,
                        crate::theme::mode(),
                        crate::tr!(COULD_NOT_LOAD_THE_DIFF_E; e;),
                        None,
                    )
                    .child(
                        div().flex_shrink_0().child(
                            Button::new("rv-retry")
                                .icon(pod_ui::kit_icon(pod_ui::Icon::RotateCcw))
                                .label(crate::strings::RETRY)
                                .small()
                                .ghost()
                                .on_click(cx.listener(|p, _, _, cx| p.refresh_all(cx))),
                        ),
                    ),
                )
                .into_any_element()
        } else if !self.loaded_once {
            crate::chrome::loading_placeholder(crate::strings::LOADING_CHANGES)
                .size_full()
                .p_4()
                .into_any_element()
        } else if self.files.is_empty() {
            div()
                .size_full()
                .p_4()
                .text_sm()
                .text_color(rgb(color::muted()))
                .child(words(
                    crate::strings::NO_CHANGES_AGAINST_THE_TASK_BASE_YET,
                    "rv-no-changes",
                ))
                .into_any_element()
        } else {
            self.render_diff(cx).into_any_element()
        };
        div()
            .id("review-panel")
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(color::bg()))
            .text_color(rgb(crate::theme::tokens().text_primary))
            .on_action(
                cx.listener(|p, _: &NextChange, _, cx| {
                    p.state.update(cx, |s, cx| s.next_change(cx))
                }),
            )
            .on_action(cx.listener(|p, _: &PrevChange, _, cx| {
                p.state.update(cx, |s, cx| s.previous_change(cx))
            }))
            .on_action(cx.listener(|p, _: &NextFile, _, cx| p.step_file(true, cx)))
            .on_action(cx.listener(|p, _: &PrevFile, _, cx| p.step_file(false, cx)))
            .on_action(cx.listener(|p, _: &AddComment, window, cx| p.add_comment_here(window, cx)))
            .on_action(cx.listener(|p, _: &SendToAgent, _, cx| p.send(cx)))
            .on_action(cx.listener(|p, _: &RefreshDiff, _, cx| p.refresh_all(cx)))
            .on_action(
                cx.listener(|p, _: &OpenInEditor, window, cx| p.open_hovered_in_editor(window, cx)),
            )
            .on_action(
                cx.listener(|p, _: &ToggleComments, window, cx| p.toggle_comments(window, cx)),
            )
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .id("review-columns")
                            .qa_id(|| "review-columns".into())
                            .size_full()
                            .flex()
                            .overflow_x_scroll()
                            .when(self.show_files && !self.files.is_empty(), |d| {
                                d.child(self.render_file_list(cx))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(320.))
                                    .h_full()
                                    .child(center),
                            )
                            .when(self.show_comments && !self.comments_drawer, |d| {
                                d.child(
                                    div()
                                        .qa_id(|| "review-comments".into())
                                        .h_full()
                                        .flex()
                                        .min_w(px(COMMENTS_MIN_W))
                                        .flex_shrink(1.)
                                        .child(self.render_sidebar(cx)),
                                )
                            }),
                    )
                    .when(self.comments_drawer_open(), |d| {
                        d.child(self.render_drawer(cx))
                    }),
            )
    }
}

#[cfg(test)]
mod editor_tests {
    use super::first_changed_line;
    use pod_diff::SourceFile;

    #[test]
    fn first_change_line_for_open_in_editor() {
        let f = |old: Option<&str>, new: Option<&str>| SourceFile {
            path: "a".into(),
            old_path: None,
            old: old.map(Into::into),
            new: new.map(Into::into),
            binary: false,
        };
        assert_eq!(
            first_changed_line(&f(Some("a\nb\nc\n"), Some("a\nB\nc\n"))),
            2
        );
        assert_eq!(first_changed_line(&f(None, Some("x\ny\n"))), 1);
        assert_eq!(first_changed_line(&f(Some("a\n"), Some("a\nb\n"))), 2);
        assert_eq!(first_changed_line(&f(Some("a\nb\n"), Some("a\n"))), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        became_unconfirmed, finished_deliveries, refusal_text, send_selection, submit_text,
        turn_ended,
    };
    use crate::source::SourceError;
    use crate::vm::{AgentState, StatusVm};
    use podd::protocol::{
        CommentState, DeliveryInfo, DeliveryState, ReviewComment, Side, orch_codes,
    };
    use std::collections::HashSet;

    fn comment(id: &str, state: CommentState) -> ReviewComment {
        ReviewComment {
            id: id.into(),
            task: "r/t".into(),
            file: "a.rs".into(),
            side: Side::New,
            line: 1,
            end_line: None,
            body: "b".into(),
            code: None,
            state,
            created_ms: 0,
            delivery: None,
            resolved_ms: None,
            note: None,
        }
    }

    fn delivery(id: &str, state: DeliveryState) -> DeliveryInfo {
        DeliveryInfo {
            id: id.into(),
            task: "r/t".into(),
            terminal: "t1".into(),
            comment_ids: vec!["c1".into()],
            state,
            prompt: String::new(),
            created_ms: 0,
            delivered_ms: None,
            agent_state: "working".into(),
            delivered_state: None,
            app_bracketed_paste: None,
            error: None,
        }
    }

    #[test]
    fn selection_or_all_open() {
        let cs = vec![
            comment("c1", CommentState::Open),
            comment("c2", CommentState::Resolved),
            comment("c3", CommentState::Sent),
        ];
        assert_eq!(send_selection(&cs, &HashSet::new()), None);
        let checked: HashSet<String> = ["c2", "c3"].map(String::from).into();
        assert_eq!(send_selection(&cs, &checked), Some(vec!["c3".to_string()]));
        // editor-11: checking only resolved comments sends nothing, never the
        // unchecked open ones.
        let only_resolved: HashSet<String> = ["c2"].map(String::from).into();
        assert_eq!(send_selection(&cs, &only_resolved), Some(vec![]));
        let stale: HashSet<String> = ["gone"].map(String::from).into();
        assert_eq!(
            send_selection(&cs, &stale),
            None,
            "ids no longer listed count as unchecked"
        );
    }

    #[test]
    fn queued_to_delivered_is_reported_once() {
        let before = vec![
            delivery("d1", DeliveryState::Queued),
            delivery("d2", DeliveryState::Delivered),
        ];
        let after = vec![
            delivery("d1", DeliveryState::Delivered),
            delivery("d2", DeliveryState::Delivered),
            delivery("d3", DeliveryState::Queued),
        ];
        let f = finished_deliveries(&before, &after);
        assert_eq!(f.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["d1"]);
        assert!(finished_deliveries(&after, &after).is_empty());
    }

    #[test]
    fn turn_end_detection() {
        let mut w = StatusVm::of(AgentState::Working);
        w.turn = 3;
        let mut idle = StatusVm::of(AgentState::Idle);
        idle.turn = 4;
        assert!(turn_ended(Some(&w), &idle));
        assert!(!turn_ended(None, &idle));
        assert!(!turn_ended(Some(&idle), &idle));
        let mut w2 = StatusVm::of(AgentState::Working);
        w2.turn = 4;
        assert!(!turn_ended(Some(&idle), &w2));
        // A shell without turns: working -> unknown still counts.
        assert!(turn_ended(
            Some(&StatusVm::of(AgentState::Working)),
            &StatusVm::of(AgentState::Unknown)
        ));
    }

    #[test]
    fn refusal_messages() {
        let blocked = SourceError {
            code: Some(orch_codes::AGENT_BLOCKED),
            message: "agent is blocked:question".into(),
        };
        assert!(refusal_text(&blocked).starts_with("보낼 수 없습니다. 에이전트 응답 대기"));
        let other = SourceError {
            code: None,
            message: "pipe".into(),
        };
        assert_eq!(refusal_text(&other), "전송 실패: pipe");
    }

    #[test]
    fn submit_check_lines_and_unconfirmed_toast_once() {
        use podd::protocol::{SubmitCheck, SubmitState};
        let check = |state| SubmitCheck {
            state,
            enters: 1,
            signal: None,
            entered_ms: None,
            settled_ms: None,
        };
        assert!(
            submit_text(&check(SubmitState::Unverifiable)).is_none(),
            "shells cannot confirm: say nothing"
        );
        assert_eq!(
            submit_text(&check(SubmitState::Confirmed)).unwrap().0,
            "에이전트가 받음"
        );
        assert!(
            submit_text(&check(SubmitState::Unconfirmed))
                .unwrap()
                .0
                .contains("입력칸")
        );
        let (pending, unconfirmed) = (check(SubmitState::Pending), check(SubmitState::Unconfirmed));
        assert!(became_unconfirmed(None, &unconfirmed));
        assert!(became_unconfirmed(Some(&pending), &unconfirmed));
        // The same state again (second event, or a deliveries reload): no second toast.
        assert!(!became_unconfirmed(Some(&unconfirmed), &unconfirmed));
        assert!(!became_unconfirmed(None, &check(SubmitState::Confirmed)));
    }
}
