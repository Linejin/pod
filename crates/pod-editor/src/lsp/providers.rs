//! Adapters from gpui-component's editor provider traits to [`LspStore`].
//!
//! The editor hands providers its rope and a byte offset. Each call first
//! flushes the rope to the store (full-text `didChange` if it changed), so a
//! request never races a pending edit, then converts the offset to an editor
//! (char) position. The store converts to/from the server's column unit.

use std::path::PathBuf;
use std::rc::Rc;

use anyhow::Result;
use gpui_kit::component::input::{
    CodeActionProvider, CompletionProvider, DefinitionProvider, EditorState, HoverProvider, Rope,
    RopeExt as _,
};
use gpui_kit::{App, Entity, SharedString, Task, Window};
use lsp_types as lt;

use super::store::{FileEdit, LspStore};

use crate::uri::path_to_uri;

/// Applies file edits through the editor workspace (opens files as needed).
pub type ApplyEdits = Rc<dyn Fn(Vec<FileEdit>, &mut Window, &mut App)>;

pub struct LspBridge {
    pub store: Entity<LspStore>,
    pub path: PathBuf,
    /// Server trigger characters (e.g. `.` `:`), captured at attach time.
    pub triggers: Vec<String>,
    /// Where code-action edits go (every file they touch).
    pub apply: Option<ApplyEdits>,
}

impl LspBridge {
    pub fn new(store: Entity<LspStore>, path: PathBuf) -> Rc<Self> {
        Self::with_apply(store, path, None)
    }

    pub fn with_apply(
        store: Entity<LspStore>,
        path: PathBuf,
        apply: Option<ApplyEdits>,
    ) -> Rc<Self> {
        Rc::new(LspBridge {
            store,
            path,
            triggers: vec![".".into(), ":".into(), ">".into()],
            apply,
        })
    }

    fn flush(&self, text: &Rope, cx: &mut App) -> lt::Position {
        let s = text.to_string();
        self.store
            .update(cx, |st, _| st.change_document(&self.path, s));
        lt::Position::default()
    }
}

impl CompletionProvider for LspBridge {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        trigger: lt::CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<lt::CompletionResponse>> {
        self.flush(text, cx);
        let pos = text.offset_to_position(offset);
        self.store
            .update(cx, |st, cx| st.completion(&self.path, pos, trigger, cx))
    }

    fn is_completion_trigger(&self, _offset: usize, new_text: &str, _cx: &mut App) -> bool {
        if new_text.is_empty() || new_text.chars().count() > 1 {
            return false;
        }
        new_text.chars().all(|c| c.is_alphanumeric() || c == '_')
            || self.triggers.iter().any(|t| t == new_text)
    }
}

/// Quick fixes and refactors (`textDocument/codeAction`, Ctrl+.).
impl CodeActionProvider for LspBridge {
    fn id(&self) -> SharedString {
        "lsp".into()
    }

    fn code_actions(
        &self,
        state: Entity<EditorState>,
        range: std::ops::Range<usize>,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<lt::CodeAction>>> {
        let (start, end) = {
            let text = state.read(cx).text();
            (
                text.offset_to_position(range.start),
                text.offset_to_position(range.end),
            )
        };
        let text = state.read(cx).text().clone();
        self.flush(&text, cx);
        self.store.update(cx, |st, cx| {
            st.code_actions(&self.path, lt::Range::new(start, end), cx)
        })
    }

    fn perform_code_action(
        &self,
        _: Entity<EditorState>,
        action: lt::CodeAction,
        _push_to_history: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<Result<()>> {
        let resolved = self
            .store
            .update(cx, |st, cx| st.resolve_code_action(&self.path, action, cx));
        let (store, path, apply) = (self.store.clone(), self.path.clone(), self.apply.clone());
        window.spawn(cx, async move |cx| {
            let (edits, command) = resolved.await?;
            if !edits.is_empty()
                && let Some(apply) = apply
            {
                cx.update(|window, cx| apply(edits, window, cx))?;
            }
            if let Some(command) = command {
                cx.update(|_, cx| {
                    store.update(cx, |st, cx| st.execute_command(&path, command, cx))
                })?
                .await?;
            }
            Ok(())
        })
    }
}

impl HoverProvider for LspBridge {
    fn hover(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Option<lt::Hover>>> {
        self.flush(text, cx);
        let pos = text.offset_to_position(offset);
        self.store
            .update(cx, |st, cx| st.hover(&self.path, pos, cx))
    }
}

impl DefinitionProvider for LspBridge {
    fn definitions(
        &self,
        text: &Rope,
        offset: usize,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<Result<Vec<lt::LocationLink>>> {
        self.flush(text, cx);
        let pos = text.offset_to_position(offset);
        let task = self
            .store
            .update(cx, |st, cx| st.definition(&self.path, pos, cx));
        cx.foreground_executor().spawn(async move {
            let locs = task.await?;
            Ok(locs
                .into_iter()
                .map(|l| lt::LocationLink {
                    origin_selection_range: None,
                    target_uri: path_to_uri(&l.path),
                    target_range: l.range,
                    target_selection_range: l.range,
                })
                .collect())
        })
    }
}
