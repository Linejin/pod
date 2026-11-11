//! Language-server actions on top of `LspStore` (its API unchanged):
//! diagnostics and next problem, find references, rename with preview,
//! workspace edits, completion snippets, and approving the repository's
//! server config.

use std::path::{Path, PathBuf};

use gpui_kit::component::input::{EditorState, InputEvent, InputState, Rope, RopeExt as _};
use gpui_kit::*;
use lsp_types as lt;

use crate::lsp::snippet::{self, SnippetSession};
use crate::lsp::store::back_to_front;
use crate::lsp::{CompletionExtra, FileEdit};
use crate::uri::path_key;

use super::{EditorWorkspace, Overlay, set_editor_diagnostics};

impl EditorWorkspace {
    /// The user approved the `.pod/lsp.json` content shown in the banner.
    pub fn trust_repo_lsp(&mut self, hash: &str, cx: &mut Context<Self>) {
        match self.lsp.update(cx, |s, cx| s.trust_repo_config(hash, cx)) {
            Ok(()) => self.flash(
                "저장소 언어 서버 설정을 승인했습니다. 서버를 다시 시작합니다",
                false,
                cx,
            ),
            Err(e) => self.flash(e, true, cx),
        }
        cx.notify();
    }

    pub(super) fn apply_diagnostics(
        &mut self,
        path: &Path,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = self.find_buffer(path) {
            let diags = self.lsp.read(cx).diagnostics_for(path).to_vec();
            set_editor_diagnostics(&self.buffers[i].state, diags, cx);
        }
        cx.notify();
    }

    pub(super) fn next_problem(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut all: Vec<(PathBuf, lt::Position)> = vec![];
        for f in self.lsp.read(cx).diagnostics() {
            for d in &f.items {
                all.push((f.path.clone(), d.range.start));
            }
        }
        if all.is_empty() {
            self.flash("문제가 없습니다", false, cx);
            return;
        }
        all.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then((a.1.line, a.1.character).cmp(&(b.1.line, b.1.character)))
        });
        let cur = self.tabs.active().map(|i| {
            (
                self.buffers[i].path.clone(),
                self.buffers[i].state.read(cx).cursor_position(),
            )
        });
        let next = cur
            .and_then(|(p, c)| {
                all.iter()
                    .find(|(fp, pos)| {
                        (path_key(fp) == path_key(&p)
                            && (pos.line, pos.character) > (c.line, c.character))
                            || fp > &p
                    })
                    .cloned()
            })
            .unwrap_or_else(|| all[0].clone());
        let _ = self.open_at(&next.0, Some(next.1), window, cx);
    }

    fn cursor_target(&self, cx: &App) -> Option<(PathBuf, lt::Position, Entity<EditorState>)> {
        let b = self.buffers.get(self.tabs.active()?)?;
        let st = b.state.read(cx);
        Some((
            b.path.clone(),
            st.text().offset_to_position(st.cursor()),
            b.state.clone(),
        ))
    }

    fn flush_active(&mut self, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.active() {
            let text = self.buffers[i].state.read(cx).value().to_string();
            let p = self.buffers[i].path.clone();
            self.lsp.update(cx, |s, _| s.change_document(&p, text));
        }
    }

    pub fn find_references(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, pos, _)) = self.cursor_target(cx) else {
            return;
        };
        self.flush_active(cx);
        let task = self.lsp.update(cx, |s, cx| s.references(&path, pos, cx));
        let word = self.word_at_cursor(cx).unwrap_or_default();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |s, cx| match res {
                Ok(v) => {
                    s.references_title = format!("`{word}` 참조 {}개", v.len());
                    s.references = v;
                    s.show_panel(true, cx);
                }
                Err(e) => s.flash(format!("참조를 찾지 못했습니다: {e}"), true, cx),
            });
        }));
    }

    fn word_at_cursor(&self, cx: &App) -> Option<String> {
        let b = self.buffers.get(self.tabs.active()?)?;
        let st = b.state.read(cx);
        let text: &Rope = st.text();
        let r = text.word_range(st.cursor())?;
        Some(text.slice(r).to_string())
    }

    pub fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, pos, _)) = self.cursor_target(cx) else {
            return;
        };
        let Some(word) = self.word_at_cursor(cx) else {
            self.flash("커서 위치에 이름이 없습니다", true, cx);
            return;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("새 이름")
                .default_value(word)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.request_rename(window, cx);
                }
            },
        );
        self._subs.push(sub);
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.overlay = Overlay::Rename { input, path, pos };
        cx.notify();
    }

    fn request_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Rename { input, path, pos } = &self.overlay else {
            return;
        };
        let new_name = input.read(cx).value().trim().to_string();
        if new_name.is_empty() {
            return;
        }
        let (path, pos) = (path.clone(), *pos);
        self.flush_active(cx);
        let task = self
            .lsp
            .update(cx, |s, cx| s.rename(&path, pos, new_name.clone(), cx));
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |s, window, cx| match res {
                Ok(edits) if edits.iter().all(|f| f.edits.is_empty()) => {
                    s.close_overlay(window, cx);
                    s.flash("바꿀 곳이 없습니다", false, cx);
                }
                Ok(edits) => {
                    s.overlay = Overlay::RenamePreview { new_name, edits };
                    s.focus.focus(window, cx);
                    cx.notify();
                }
                Err(e) => {
                    s.close_overlay(window, cx);
                    s.flash(format!("이름을 바꾸지 못했습니다: {e}"), true, cx);
                }
            });
        }));
    }

    /// Applies the previewed rename to buffers (opening files as needed).
    /// Files end up dirty; the user reviews and saves (Ctrl+K S saves all).
    pub fn apply_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::RenamePreview { edits, .. } =
            std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return;
        };
        let changed = self.apply_file_edits(edits, window, cx);
        self.flash(
            format!("파일 {changed}개에서 이름을 바꿨습니다 (저장 전)"),
            false,
            cx,
        );
        cx.notify();
    }

    /// Applies LSP file edits (rename, code actions) to buffers, opening
    /// files as needed; returns how many files changed. Files end up dirty.
    pub fn apply_file_edits(
        &mut self,
        edits: Vec<FileEdit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let mut changed = 0;
        let keep_active = self.active_path().map(Path::to_path_buf);
        for fe in edits {
            if self.find_buffer(&fe.path).is_none()
                && self.open_at(&fe.path, None, window, cx).is_err()
            {
                continue;
            }
            let Some(i) = self.find_buffer(&fe.path) else {
                continue;
            };
            if self.buffers[i].undecodable || self.buffers[i].large {
                continue; // read-only: an edit could not be saved without loss
            }
            let st = self.buffers[i].state.clone();
            edit_buffer(&st, &fe.edits, window, cx);
            self.on_buffer_changed(&fe.path, window, cx);
            changed += 1;
        }
        if let Some(a) = keep_active.and_then(|p| self.find_buffer(&p)) {
            self.activate(a, window, cx);
        }
        cx.notify();
        changed
    }

    /// Moves a snippet session's stops with an edit; ends it when the edit
    /// cuts across a stop.
    pub(super) fn track_snippet(&mut self, path: &Path, edit: snippet::Edit) {
        if let Some(i) = self.find_buffer(path)
            && let Some(s) = self.buffers[i].snippet.as_mut()
            && !s.apply(edit)
        {
            self.buffers[i].snippet = None;
        }
    }

    /// A completion item was just inserted at byte `at`: start its tab-stop
    /// session and add its auto-imports (`additionalTextEdits`, resolved
    /// now if the server sends them lazily).
    pub(super) fn after_completion(
        &mut self,
        path: &Path,
        state: Entity<EditorState>,
        at: usize,
        extra: CompletionExtra,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.find_buffer(path) else {
            return;
        };
        self.buffers[i].snippet = extra
            .snippet
            .as_ref()
            .and_then(|s| SnippetSession::new(s, at));
        if !extra.additional.is_empty() {
            // Edits above the insertion shift the session through `track_snippet`.
            edit_buffer(&state, &extra.additional, window, cx);
        } else if let Some(item) = extra.resolve {
            let task = self
                .lsp
                .update(cx, |s, cx| s.resolve_additional_edits(path, item, cx));
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(edits) = task.await
                    && !edits.is_empty()
                {
                    let _ =
                        this.update_in(cx, |_, window, cx| edit_buffer(&state, &edits, window, cx));
                }
            })
            .detach();
        }
        if self.buffers[i].snippet.is_some() {
            // After the queued edit events (auto-import) have moved the stops.
            let ws = cx.entity().downgrade();
            let p = path.to_path_buf();
            window.defer(cx, move |_, cx| {
                let _ = ws.update(cx, |ws, cx| ws.select_snippet_stop(&p, cx));
            });
        }
    }

    /// Selects the session's current stop; the final stop (`$0`) only
    /// places the caret and ends the session.
    fn select_snippet_stop(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(i) = self.find_buffer(path) else {
            return;
        };
        let Some(s) = &self.buffers[i].snippet else {
            return;
        };
        let (range, last) = (s.current_range(), s.at_last());
        self.buffers[i]
            .state
            .update(cx, |st, cx| st.set_selected_range(range, cx));
        if last {
            self.buffers[i].snippet = None;
        }
        cx.notify();
    }

    /// Tab / Shift+Tab inside a snippet session: next / previous stop.
    /// `false` (the key does its usual indent) when there is no session, a
    /// completion menu is open, or the caret has left the current stop.
    pub(super) fn snippet_step(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(i) = self.tabs.active() else {
            return false;
        };
        let st = self.buffers[i].state.read(cx);
        let (menu_open, sel) = (st.completion_menu_state().open, st.selected_range());
        let Some(s) = self.buffers[i].snippet.as_mut() else {
            return false;
        };
        if menu_open {
            return false;
        }
        let cur = s.current_range();
        if sel.start < cur.start || sel.end > cur.end {
            self.buffers[i].snippet = None;
            return false;
        }
        if s.step(forward).is_none() {
            self.buffers[i].snippet = None;
            return false;
        }
        let path = self.buffers[i].path.clone();
        self.select_snippet_stop(&path, cx);
        true
    }

    /// Whether the active tab has a snippet session (tests, status).
    pub fn snippet_active(&self) -> bool {
        self.tabs
            .active()
            .and_then(|i| self.buffers.get(i))
            .is_some_and(|b| b.snippet.is_some())
    }
}

/// Applies editor-column edits to an open buffer as undoable ranged
/// replacements (back to front), so the edits reach the language server
/// incrementally and the caret keeps its place in the text around them.
fn edit_buffer(
    state: &Entity<EditorState>,
    edits: &[lt::TextEdit],
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |s, cx| {
        let text = s.text().clone();
        let ranges: Vec<(std::ops::Range<usize>, &str)> = edits
            .iter()
            .map(|e| {
                let a = text.position_to_offset(&e.range.start);
                (
                    a..text.position_to_offset(&e.range.end).max(a),
                    e.new_text.as_str(),
                )
            })
            .collect();
        let starts: Vec<usize> = ranges.iter().map(|(r, _)| r.start).collect();
        let caret = s.cursor();
        let moved: isize = ranges
            .iter()
            .filter(|(r, _)| r.end <= caret)
            .map(|(r, t)| t.len() as isize - r.len() as isize)
            .sum();
        for (r, t) in back_to_front(&starts)
            .into_iter()
            .map(|i| ranges[i].clone())
        {
            s.set_selected_range(r, cx);
            s.replace(t.to_string(), window, cx);
        }
        let caret = (caret as isize + moved).max(0) as usize;
        s.set_selected_range(caret..caret, cx);
    });
}
