//! Buffers: finding a file's buffer in any spelling, creating one (editor
//! state, language, subscriptions, change sync) and releasing it, and
//! registering it with its language server once (`didOpen`/`didClose`).

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::{
    EditorState, InputEditsEvent, InputEvent, RopeExt as _, ShowDocumentHandler,
};
use gpui_kit::*;
use lsp_types as lt;

use crate::lsp::language_for_path;
use crate::lsp::providers::LspBridge;
use crate::lsp::snippet;
use crate::text;
use crate::uri::{path_key, simplify, under_root_spelling, uri_to_path};

use super::{
    Buffer, DiskState, EditorEvent, EditorWorkspace, TabKey, rel_of, set_editor_diagnostics,
};

impl EditorWorkspace {
    /// The buffer of `path` in any spelling: buffers under the root are
    /// keyed in the root's canonical spelling, and a host or server may name
    /// the same file through an 8.3 short name or another letter case.
    pub(super) fn find_buffer(&self, path: &Path) -> Option<usize> {
        let k = path_key(path);
        if let Some(i) = self.buffers.iter().position(|b| path_key(&b.path) == k) {
            return Some(i);
        }
        if !path.is_absolute() {
            return None;
        }
        let k2 = path_key(&under_root_spelling(&self.root, &simplify(path)));
        if k2 == k {
            return None;
        }
        self.buffers.iter().position(|b| path_key(&b.path) == k2)
    }

    pub(super) fn make_buffer(
        &mut self,
        path: &Path,
        loaded: text::Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Buffer {
        let language = language_for_path(path);
        let large = loaded.large;
        let undecodable = !loaded.lossless;
        let initial = loaded.text.clone();
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .line_number(true)
                .searchable(true)
                .soft_wrap(false);
            if let (Some(l), false) = (language, large) {
                s = s.language(l.highlight);
            }
            s.default_value(initial)
        });
        if large || undecodable {
            state.update(cx, |s, cx| s.set_readonly(true, cx));
        }
        if !large && language.is_some() {
            let apply: crate::lsp::providers::ApplyEdits = {
                let ws = cx.entity().downgrade();
                Rc::new(move |edits, window: &mut Window, cx: &mut App| {
                    let _ = ws.update(cx, |ws, cx| {
                        let n = ws.apply_file_edits(edits, window, cx);
                        ws.flash(
                            format!("코드 작업을 적용했습니다: 파일 {n}개 (저장 전)"),
                            false,
                            cx,
                        );
                    });
                })
            };
            let bridge = LspBridge::with_apply(self.lsp.clone(), path.to_path_buf(), Some(apply));
            let ws = cx.entity().downgrade();
            let show: ShowDocumentHandler = Rc::new(
                move |params: &lt::ShowDocumentParams, window: &mut Window, cx: &mut App| {
                    let Some(target) = uri_to_path(&params.uri) else {
                        return false;
                    };
                    let pos = params.selection.map(|r| r.start);
                    let ws = ws.clone();
                    // The editor is mid-update here: defer so a same-file jump can update it.
                    window.defer(cx, move |window, cx| {
                        let _ = ws.update(cx, |ws, cx| {
                            if let Err(e) = ws.open_at(&target, pos, window, cx) {
                                ws.flash(e, true, cx);
                            }
                        });
                    });
                    true
                },
            );
            state.update(cx, |s, cx| {
                let l = s.lsp_mut();
                l.completion_provider = Some(bridge.clone());
                l.hover_provider = Some(bridge.clone());
                l.code_action_providers = vec![bridge.clone()];
                l.definition_provider = Some(bridge);
                l.show_document = Some(show);
                cx.notify();
            });
        }
        let p2 = path.to_path_buf();
        let change = cx.subscribe_in(
            &state,
            window,
            move |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::Change = ev {
                    this.on_buffer_changed(&p2, window, cx);
                }
            },
        );
        // Incremental sync: each edit goes to the store as a ranged change
        // (the debounced full-text sync below stays as a repair path).
        // The same edits keep a snippet session's stops in place, and an
        // inserted completion picks up its tab stops and auto-imports.
        let p3 = path.to_path_buf();
        let lsp = self.lsp.clone();
        let edits = cx.subscribe_in(
            &state,
            window,
            move |this: &mut Self, state, ev: &InputEditsEvent, window, cx| {
                for (e, after) in ev.edits() {
                    let new_text = after.slice(e.start_byte..e.new_end_byte).to_string();
                    lsp.update(cx, |s, _| {
                        s.edit_document(&p3, e.start_byte, e.old_end_byte, &new_text)
                    });
                    this.track_snippet(
                        &p3,
                        snippet::Edit {
                            start: e.start_byte,
                            old_end: e.old_end_byte,
                            new_end: e.new_end_byte,
                        },
                    );
                    let line = after.offset_to_position(e.start_byte).line;
                    if let Some(extra) =
                        lsp.update(cx, |s, _| s.take_completion(&p3, line, &new_text))
                    {
                        this.after_completion(&p3, state.clone(), e.start_byte, extra, window, cx);
                    }
                }
            },
        );
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        let mut b = Buffer {
            path: path.to_path_buf(),
            rel: rel_of(&self.root, path),
            state,
            language,
            format: loaded.format,
            saved: loaded.text,
            dirty: false,
            large,
            undecodable,
            stamp: Some(loaded.stamp),
            save_blocked: false,
            save_failed: None,
            disk: DiskState::Same,
            lsp_open: false,
            _subs: vec![change, edits, observe],
            snippet: None,
            _sync: Task::ready(()),
        };
        // Diagnostics that arrived before the tab existed.
        let diags = self.lsp.read(cx).diagnostics_for(path).to_vec();
        if !diags.is_empty() {
            set_editor_diagnostics(&b.state, diags, cx);
        }
        b.dirty = false;
        b
    }

    pub(super) fn on_buffer_changed(
        &mut self,
        path: &Path,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.find_buffer(path) else {
            return;
        };
        // The buffer's own spelling: tab keys and server documents use it.
        let path = &self.buffers[i].path.clone();
        let text = self.buffers[i].state.read(cx).value().to_string();
        let dirty = text != self.buffers[i].saved;
        if dirty != self.buffers[i].dirty {
            self.buffers[i].dirty = dirty;
            cx.emit(EditorEvent::DirtyChanged {
                path: path.to_path_buf(),
                dirty,
            });
        }
        // Editing a preview tab keeps it.
        if dirty && self.tabs.pin(&TabKey::of(path)) {
            cx.emit(EditorEvent::TabsChanged);
        }
        if self.buffers[i].language.is_some() && !self.buffers[i].large {
            // Debounced full-text sync; providers flush synchronously anyway.
            let lsp = self.lsp.clone();
            let p = path.to_path_buf();
            self.buffers[i]._sync = cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                lsp.update(cx, |s, _| s.change_document(&p, text));
            });
        }
        cx.notify();
    }

    /// Releases a buffer that left the tab strip: the language server
    /// closes the document (`didClose`) and the host hears about it.
    pub(super) fn drop_buffer(&mut self, b: Buffer, cx: &mut Context<Self>) {
        if b.dirty {
            cx.emit(EditorEvent::DirtyChanged {
                path: b.path.clone(),
                dirty: false,
            });
        }
        self.lsp.update(cx, |s, _| s.close_document(&b.path));
        cx.emit(EditorEvent::FileClosed(b.path.clone()));
    }

    /// Registers buffer `ix` with its language server (starting it on first
    /// use), or marks the server used so the host's LRU cap keeps it.
    pub(super) fn ensure_lsp(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get_mut(ix) else {
            return;
        };
        if b.language.is_none() || b.large {
            return;
        }
        let path = b.path.clone();
        if b.lsp_open {
            self.lsp.update(cx, |s, cx| s.touch(&path, cx));
            return;
        }
        b.lsp_open = true;
        let text_now = b.state.read(cx).value().to_string();
        self.lsp
            .update(cx, |s, cx| s.open_document(&path, text_now, cx));
    }
}
