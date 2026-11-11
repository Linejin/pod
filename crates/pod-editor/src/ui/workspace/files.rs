//! Files on disk: saving (refused on an unseen disk change, failures with
//! retry/overwrite), reopening in another encoding, external changes (watcher
//! batches, disk prompt and diff, reload), the quick-open index and git
//! status.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use gpui_kit::*;

use crate::git;
use crate::index;
use crate::text;
use crate::uri::path_key;
use crate::watch::Batch;

use super::{DiskState, EditorEvent, EditorWorkspace, Overlay, SaveFailure, rel_of};

impl EditorWorkspace {
    pub fn save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.active() {
            self.save_buffer(i, window, cx);
        }
    }

    pub fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for i in 0..self.buffers.len() {
            if self.buffers[i].dirty {
                self.save_buffer(i, window, cx);
            }
        }
    }

    /// Answers the active tab's save-failure banner: retry the atomic save,
    /// or (`overwrite`) write the file in place, not atomically.
    pub fn answer_save_failure(
        &mut self,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = self.tabs.active() {
            self.save_buffer_ex(i, overwrite, window, cx);
        }
    }

    pub(super) fn dismiss_save_failure(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.tabs.active().and_then(|i| self.buffers.get_mut(i)) {
            b.save_failed = None;
        }
        cx.notify();
    }

    pub(super) fn save_buffer(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.save_buffer_ex(i, false, window, cx);
    }

    /// `in_place`: the user chose to overwrite after an atomic replace
    /// failed; otherwise always the atomic write (never a silent fallback).
    fn save_buffer_ex(
        &mut self,
        i: usize,
        in_place: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffers[i].save_failed = None;
        let b = &self.buffers[i];
        let path = b.path.clone();
        if b.undecodable || b.large {
            self.flash(
                format!("읽기 전용으로 연 파일은 저장하지 않습니다: {}", b.rel),
                true,
                cx,
            );
            return;
        }
        let text = b.state.read(cx).value().to_string();
        let bytes = match text::encode(&text, &b.format) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.flash(format!("저장하지 못했습니다: {e}"), true, cx);
                return;
            }
        };
        // Never overwrite a change on disk the buffer has not seen (an agent
        // may have written the file just now, before the watcher said so).
        match text::disk_stamp(&path) {
            Ok(now) if now != b.stamp => {
                self.block_save(i, now.is_none(), window, cx);
                return;
            }
            Ok(_) => {}
            Err(e) => {
                self.flash(format!("저장하지 못했습니다: {e}"), true, cx);
                return;
            }
        }
        let written = if in_place {
            text::write_in_place(&path, &bytes)
        } else {
            text::write_atomic(&path, &bytes)
        };
        match written {
            Ok(()) => {
                let encoding = self.buffers[i].format.encoding;
                let b = &mut self.buffers[i];
                b.stamp = Some(text::stamp_of(&bytes));
                // The written bytes are the new base for mixed line endings.
                if let Ok(l) = text::decode_as(bytes, 0, encoding) {
                    b.format = l.format;
                }
                b.saved = text.clone();
                b.save_blocked = false;
                b.disk = DiskState::Same;
                if b.dirty {
                    b.dirty = false;
                    cx.emit(EditorEvent::DirtyChanged {
                        path: path.clone(),
                        dirty: false,
                    });
                }
                self.lsp.update(cx, |s, _| {
                    s.change_document(&path, text);
                    s.save_document(&path);
                });
                cx.emit(EditorEvent::FileSaved(path.clone()));
                let rel = self.buffers[i].rel.clone();
                self.flash(format!("저장했습니다: {rel}"), false, cx);
            }
            Err(e) => {
                self.buffers[i].save_failed = Some(SaveFailure {
                    message: e.to_string(),
                    replace: text::is_replace_failure(&e),
                });
                self.flash(format!("저장하지 못했습니다: {e}"), true, cx);
            }
        }
        cx.notify();
    }

    /// A save found the disk changed since the buffer read it: nothing is
    /// written; the banner offers reload, overwrite or a diff.
    fn block_save(&mut self, i: usize, deleted: bool, _: &mut Window, cx: &mut Context<Self>) {
        let disk = if deleted {
            DiskState::Deleted
        } else {
            match text::load_as(&self.buffers[i].path, self.buffers[i].format.encoding) {
                Ok(l) => DiskState::Changed(l.text),
                Err(e) => {
                    self.flash(
                        format!("저장하지 않았습니다: 디스크의 파일을 다시 읽지 못했습니다 ({e})"),
                        true,
                        cx,
                    );
                    return;
                }
            }
        };
        let b = &mut self.buffers[i];
        b.save_blocked = true;
        b.disk = disk;
        let rel = b.rel.clone();
        self.flash(
            format!("저장하지 않았습니다: 디스크의 {rel}이(가) 열어 둔 뒤 바뀌었습니다"),
            true,
            cx,
        );
        cx.notify();
    }

    /// Reopens tab `i` reading its bytes in `encoding` (editor-02). It
    /// becomes editable only when the bytes decode without loss.
    pub fn reopen_with_encoding(
        &mut self,
        i: usize,
        encoding: &'static text::Encoding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.buffers.get(i) else { return };
        if b.dirty {
            self.flash("저장하지 않은 변경이 있어 다시 열 수 없습니다", true, cx);
            return;
        }
        match text::load_as(&b.path, encoding) {
            Ok(l) if l.lossless => {
                self.reload_buffer(i, l, window, cx);
                self.flash(
                    format!("{}(으)로 다시 열었습니다", encoding.name()),
                    false,
                    cx,
                );
            }
            Ok(_) => self.flash(
                format!(
                    "{}(으)로도 손실 없이 읽을 수 없습니다. 읽기 전용으로 둡니다",
                    encoding.name()
                ),
                true,
                cx,
            ),
            Err(e) => self.flash(e.to_string(), true, cx),
        }
    }

    /// Shows the line diff between the file on disk and the active buffer.
    pub fn show_disk_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.tabs.active() else { return };
        let b = &self.buffers[i];
        let disk = match &b.disk {
            DiskState::Changed(t) => t.clone(),
            DiskState::Deleted => String::new(),
            DiskState::Same => b.saved.clone(),
        };
        let mine = b.state.read(cx).value().to_string();
        let lines = disk_diff_lines(&disk, &mine);
        self.overlay = Overlay::DiskDiff {
            rel: b.rel.clone(),
            lines,
        };
        self.prompt_focus.focus(window, cx);
        cx.notify();
    }

    /// Lines of the open diff overlay (`-` disk, `+` buffer), for tests.
    pub fn disk_diff(&self) -> Option<Vec<(char, String)>> {
        match &self.overlay {
            Overlay::DiskDiff { lines, .. } => Some(lines.clone()),
            _ => None,
        }
    }

    /// Answers the active tab's disk banner: `reload` takes the disk
    /// version; otherwise keep the buffer (and, after a refused save,
    /// overwrite the file now).
    pub fn answer_disk_prompt(
        &mut self,
        reload: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resolve_disk_prompt(reload, window, cx);
    }

    pub(super) fn rebuild_index(&mut self, cx: &mut Context<Self>) {
        self.index_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.index_cancel = cancel.clone();
        self.indexing = true;
        let root = self.root.clone();
        let job = cx.background_spawn(async move { index::build(&root, &cancel) });
        self._index_task = cx.spawn(async move |this, cx| {
            let snap = job.await;
            let _ = this.update(cx, |s, cx| {
                s.index = snap;
                s.indexing = false;
                if let Overlay::QuickOpen(qo) = &s.overlay {
                    let (files, tr) = (s.index.files.clone(), s.index.truncated);
                    qo.update(cx, |q, cx| q.set_files(files, false, tr, cx));
                }
                cx.notify();
            });
        });
    }

    pub(super) fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let job = cx.background_spawn(async move { git::read_status(&root) });
        self._git_task = cx.spawn(async move |this, cx| {
            let st = job.await;
            let _ = this.update(cx, |s, cx| {
                s.branch = st.branch.clone();
                s.git_changes = st.changed_count();
                s.tree.set_git(st);
                cx.notify();
            });
        });
    }

    pub(super) fn on_fs_batch(
        &mut self,
        batch: Batch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !batch.paths.is_empty() {
            if batch.paths.iter().any(|p| p == &self.root) {
                self.tree.refresh_all();
            } else {
                self.tree.refresh_paths(&batch.paths);
            }
            // Re-index on creations/deletions only, decided off the UI
            // thread (a build can report thousands of paths at once).
            let lookup = self.index.lookup.clone();
            let paths = batch.paths.clone();
            let root = self.root.clone();
            let job = cx.background_spawn(async move {
                index::is_structural(&paths, &lookup, |p| rel_of(&root, p))
            });
            cx.spawn(async move |this, cx| {
                if job.await {
                    let _ = this.update(cx, |s, cx| s.rebuild_index(cx));
                }
            })
            .detach();
            // A changed `.pod/lsp.json` is checked against the approvals again.
            let lsp_json = path_key(&self.root.join(".pod").join("lsp.json"));
            if batch.paths.iter().any(|p| path_key(p) == lsp_json) {
                let dir = crate::lsp::registry::user_config_dir();
                self.lsp
                    .update(cx, |s, cx| s.reload_registry(dir.as_deref(), cx));
                self.repo_lsp_dismissed = false;
            }
            for p in &batch.paths {
                if let Some(i) = self.find_buffer(p) {
                    self.check_disk(i, window, cx);
                }
            }
        }
        self.refresh_git(cx);
        cx.notify();
    }

    /// Compares an open buffer with the file on disk after a change event.
    fn check_disk(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.buffers[i].path.clone();
        if !path.exists() {
            self.buffers[i].disk = DiskState::Deleted;
            cx.emit(EditorEvent::ExternalChange {
                path,
                reloaded: false,
            });
            return;
        }
        let Ok(loaded) = text::load_as(&path, self.buffers[i].format.encoding) else {
            return;
        };
        let b = &mut self.buffers[i];
        if loaded.text == b.saved {
            // Our own save, a touch, or only the line endings changed: the
            // disk bytes become the base for the next save.
            b.disk = DiskState::Same;
            b.stamp = Some(loaded.stamp);
            b.format = loaded.format;
            b.save_blocked = false;
            return;
        }
        if !b.dirty && self.auto_reload_clean {
            self.reload_buffer(i, loaded, window, cx);
            let rel = self.buffers[i].rel.clone();
            self.flash(
                format!("디스크에서 바뀐 내용을 다시 불러왔습니다: {rel}"),
                false,
                cx,
            );
            cx.emit(EditorEvent::ExternalChange {
                path,
                reloaded: true,
            });
        } else {
            b.disk = DiskState::Changed(loaded.text);
            cx.emit(EditorEvent::ExternalChange {
                path,
                reloaded: false,
            });
        }
    }

    fn reload_buffer(
        &mut self,
        i: usize,
        loaded: text::Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = &mut self.buffers[i];
        b.format = loaded.format;
        b.stamp = Some(loaded.stamp);
        b.undecodable = !loaded.lossless;
        b.save_blocked = false;
        let read_only = b.large || b.undecodable;
        b.saved = loaded.text.clone();
        b.disk = DiskState::Same;
        let was_dirty = b.dirty;
        b.dirty = false;
        let state = b.state.clone();
        let path = b.path.clone();
        let text_v = loaded.text.clone();
        state.update(cx, |s, cx| {
            let pos = s.cursor_position();
            s.set_readonly(false, cx);
            s.set_value(text_v, window, cx);
            s.set_cursor_position(pos, window, cx);
            s.set_readonly(read_only, cx);
        });
        self.lsp
            .update(cx, |s, _| s.change_document(&path, loaded.text));
        if was_dirty {
            cx.emit(EditorEvent::DirtyChanged { path, dirty: false });
        }
        cx.notify();
    }

    pub(super) fn resolve_disk_prompt(
        &mut self,
        reload: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.tabs.active() else { return };
        let blocked = std::mem::take(&mut self.buffers[i].save_blocked);
        let path = self.buffers[i].path.clone();
        match std::mem::replace(&mut self.buffers[i].disk, DiskState::Same) {
            DiskState::Changed(_) if reload => {
                if let Ok(l) = text::load_as(&path, self.buffers[i].format.encoding) {
                    self.reload_buffer(i, l, window, cx);
                }
            }
            DiskState::Changed(disk_text) => {
                // Keep mine: the disk version becomes the comparison base so
                // the buffer stays dirty and saving overwrites it (an
                // explicit choice, so the current disk bytes are accepted).
                self.buffers[i].saved = disk_text;
                self.buffers[i].stamp = text::disk_stamp(&path).ok().flatten();
            }
            DiskState::Deleted if reload => {
                self.buffers[i].dirty = false;
                self.remove_tab_ex(i, false, window, cx);
            }
            DiskState::Deleted => {
                self.buffers[i].saved = String::new();
                self.buffers[i].dirty = true;
                self.buffers[i].stamp = text::disk_stamp(&path).ok().flatten();
            }
            DiskState::Same => {}
        }
        if blocked && !reload && self.find_buffer(&path) == Some(i) {
            self.save_buffer(i, window, cx);
        }
        cx.notify();
    }
}

/// Changed lines (with 2 lines of context, `…` between hunks) of `disk`
/// against `mine`: `-` only on disk, `+` only in the buffer, ` ` context.
fn disk_diff_lines(disk: &str, mine: &str) -> Vec<(char, String)> {
    let diff = similar::TextDiff::configure()
        .timeout(Duration::from_secs(1))
        .diff_lines(disk, mine);
    let mut out = Vec::new();
    for (n, group) in diff.grouped_ops(2).iter().enumerate() {
        if n > 0 {
            out.push((' ', "…".to_string()));
        }
        for op in group {
            for ch in diff.iter_changes(op) {
                let tag = match ch.tag() {
                    similar::ChangeTag::Delete => '-',
                    similar::ChangeTag::Insert => '+',
                    similar::ChangeTag::Equal => ' ',
                };
                out.push((tag, ch.value().trim_end_matches('\n').to_string()));
            }
        }
    }
    out
}
