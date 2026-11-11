//! What the Review panel does: reads the diff, comments and deliveries,
//! debounces refreshes, re-reads a queued delivery while podd sends no
//! delivery events, opens and saves drafts, sends, and moves between files,
//! the comment column, its drawer and the PR tab. Policy comes from
//! [`super::model`]; each request, its reply handling, `notify` and `defer`
//! stays where and in the order it was before the split.

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::diff::{
    DiffEvent, DiffFile, DiffLinePosition, DiffMode, DiffSide, DiffState,
};
use gpui_kit::component::input::{InputEvent, TextareaState};
use gpui_kit::*;
use pod_diff::{CONTEXT_LINES, ChangedFile, FileRow, SourceFile};
use podd::protocol::{self as p, DeliveryInfo, DeliveryState, ReviewComment};

use super::copy::side_label;
use super::model::{
    SendOutcome, became_unconfirmed, diff_side, editor_line, finished_deliveries, finished_toast,
    first_change_target, git_signature, hash, new_comment, refusal_text, send_params,
    send_selection, sent_toast, side_of, stepped_file, turn_ended,
};
use super::{Draft, ReviewPanel, debug};
use crate::source::SourceError;
use crate::vm::{StatusVm, TaskVm};
use crate::workspace::ToastKind;

/// While a delivery is queued, re-read deliveries this often.
const QUEUED_POLL: Duration = Duration::from_secs(2);
/// Coalesce refresh triggers that arrive together (status + task poll).
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(400);

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

impl ReviewPanel {
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
            let (kind, title, text) = finished_toast(f, &self.task_id);
            self.toast(kind, title, text, cx);
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
    pub(super) fn press_enter(&mut self, terminal: &str, cx: &mut Context<Self>) {
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

    pub(super) fn toggle_collapse(&mut self, cx: &mut Context<Self>) {
        self.collapsed = !self.collapsed;
        let lines = self.collapsed.then_some(CONTEXT_LINES);
        self.state
            .update(cx, |s, cx| s.set_context_lines(lines, cx));
        cx.notify();
    }

    /// Toolbar 줄바꿈: soft-wrap long lines (no relayout).
    pub(super) fn toggle_wrap(&mut self, cx: &mut Context<Self>) {
        self.wrap = !self.wrap;
        cx.notify();
    }

    /// F7 / Alt+↓ / toolbar: the next change.
    pub(super) fn next_change(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.next_change(cx))
    }

    /// Shift+F7 / Alt+↑ / toolbar: the previous change.
    pub(super) fn previous_change(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.previous_change(cx))
    }

    pub(super) fn go_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        debug(format_args!("go_file {ix}"));
        let Some(f) = self.files.get(ix) else { return };
        self.current_file = ix;
        let path = f.path.clone();
        self.state.update(cx, |s, cx| s.scroll_to_file(&path, cx));
        cx.notify();
    }

    pub(super) fn step_file(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(ix) = stepped_file(self.current_file, self.files.len(), forward) else {
            return;
        };
        self.go_file(ix, cx);
    }

    // ---- comments ---------------------------------------------------------

    pub(super) fn reload_comments(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn open_draft(
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
        let line = editor_line(line, hovered, &self.sources, &path);
        let (ws, task) = (self.workspace.clone(), self.task_id.clone());
        window.defer(cx, move |window, cx| {
            let _ = ws.update(cx, |w, cx| {
                w.open_editor(&task, Some((path, line as u32, 1)), false, window, cx)
            });
        });
    }

    /// Ctrl+Shift+O: the hovered line, else the current file's first change.
    pub(super) fn open_hovered_in_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn add_comment_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let (path, side, line) = first_change_target(source);
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

    pub(super) fn save_draft(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.draft.take() else { return };
        let body = d.input.read(cx).value().trim().to_string();
        cx.notify();
        if body.is_empty() {
            return;
        }
        let comment = new_comment(&self.sources, d.path, d.side, d.line, d.end_line, body);
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

    /// The draft's 취소: drop it unsaved.
    pub(super) fn cancel_draft(&mut self, cx: &mut Context<Self>) {
        self.draft = None;
        cx.notify();
    }

    /// The line under the pointer (Ctrl+Shift+C / Ctrl+Shift+O target); no redraw.
    pub(super) fn set_hovered(&mut self, pos: Option<DiffLinePosition>) {
        self.hovered = pos;
    }

    /// A card's ☐/☑: check or uncheck it for "Send".
    pub(super) fn toggle_checked(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.checked.remove(id) {
            self.checked.insert(id.to_string());
        }
        cx.notify();
    }

    /// "해결됨 보기/숨기기".
    pub(super) fn toggle_show_resolved(&mut self, cx: &mut Context<Self>) {
        self.show_resolved = !self.show_resolved;
        cx.notify();
    }

    pub(super) fn resolve(&mut self, id: String, reopen: bool, cx: &mut Context<Self>) {
        let rx = self
            .source
            .call::<p::ReviewCommentsResolve>(&p::ReviewResolveParams {
                ids: vec![id],
                note: None,
                reopen,
            });
        self.apply_comment_change(rx, cx);
    }

    pub(super) fn remove(&mut self, id: String, cx: &mut Context<Self>) {
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

    pub(super) fn scroll_to_comment(&mut self, c: &ReviewComment, cx: &mut Context<Self>) {
        let pos = DiffLinePosition::new(c.file.clone(), diff_side(c.side), c.line as usize);
        if let Some(ix) = self.files.iter().position(|f| f.path.as_ref() == c.file) {
            self.current_file = ix;
        }
        self.state.update(cx, |s, cx| s.scroll_to_line(pos, cx));
        cx.notify();
    }

    // ---- send / deliveries ---------------------------------------------------

    pub(super) fn send(&mut self, cx: &mut Context<Self>) {
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
        let params = send_params(&self.task_id, selection, instructions);
        let rx = self.source.call::<p::ReviewSend>(&params);
        self.sending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                p.sending = false;
                match r {
                    Ok(d) => {
                        let (kind, title, text) = sent_toast(&d, &p.task_id);
                        p.toast(kind, title, text, cx);
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

    pub(super) fn cancel_delivery(&mut self, id: String, cx: &mut Context<Self>) {
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

    pub(super) fn reload_deliveries(&mut self, cx: &mut Context<Self>) {
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
                let finished: Vec<DeliveryInfo> = finished_deliveries(&p.deliveries, &after)
                    .into_iter()
                    .cloned()
                    .collect();
                let delivered = finished.iter().any(|d| d.state == DeliveryState::Delivered);
                for d in finished {
                    let (kind, title, text) = finished_toast(&d, &p.task_id);
                    p.toast(kind, title, text, cx);
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

    // ---- columns, drawer, PR tab -------------------------------------------

    /// Show or hide the file list (fixture scenes).
    pub fn set_file_list(&mut self, show: bool, cx: &mut Context<Self>) {
        self.show_files = show;
        self.relayout(cx);
    }

    /// Toolbar 파일: the file list shows or hides.
    pub(super) fn toggle_file_list(&mut self, cx: &mut Context<Self>) {
        self.show_files = !self.show_files;
        self.relayout(cx);
    }

    /// D4.6: below [`super::COMMENTS_DRAWER_BELOW`] px of canvas the comment column leaves the
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
    pub(super) fn toggle_comments(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    pub(super) fn set_pr_tab(&mut self, on: bool, cx: &mut Context<Self>) {
        self.pr_tab = on;
        self.pr.update(cx, |p, cx| p.set_visible(on, cx));
        cx.notify();
    }

    /// Show the PR tab (and the sidebar); orm opens the create form when
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

    // ---- at render ---------------------------------------------------------------

    /// What waits for a window: clear the instructions after a delivery, and
    /// turn a finished multi-line line-number drag into a range draft (after
    /// this frame).
    pub(super) fn take_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    }
}

/// Re-render on state changes, and turn a finished multi-line line-number
/// drag into a range draft.
pub(super) fn watch_state(
    state: &Entity<DiffState>,
    cx: &mut Context<ReviewPanel>,
) -> [Subscription; 2] {
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

pub(super) fn new_state<T: 'static>(
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
