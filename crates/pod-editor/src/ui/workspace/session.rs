//! Layout persistence: the open tabs, cursor positions, preview tab,
//! active tab and Ctrl+Tab order the host saves with its layout, and their
//! restore (missing files skipped; the language server waits for focus,
//! `defer_lsp`).

use std::path::Path;

use gpui_kit::*;
use lsp_types as lt;
use serde::{Deserialize, Serialize};

use crate::text;

use super::{EditorEvent, EditorWorkspace, TabKey};

/// One open tab for layout persistence (path relative to the root, `/`
/// separators; 1-based line/column).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSession {
    pub path: String,
    #[serde(default = "one")]
    pub line: u32,
    #[serde(default = "one")]
    pub col: u32,
    /// A preview tab (replaced by the next single-click open).
    #[serde(default, skip_serializing_if = "is_false")]
    pub preview: bool,
}

fn one() -> u32 {
    1
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Open tabs and the active one, restored with the host layout.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSession {
    #[serde(default)]
    pub tabs: Vec<TabSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<usize>,
    /// Ctrl+Tab order: tab indices, most recently used first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mru: Vec<usize>,
}

impl EditorWorkspace {
    /// Open tabs with cursor positions, for the host's layout file.
    pub fn session(&self, cx: &App) -> EditorSession {
        let tabs = self
            .buffers
            .iter()
            .map(|b| {
                let p = b.state.read(cx).cursor_position();
                TabSession {
                    path: b.rel.clone(),
                    line: p.line + 1,
                    col: p.character + 1,
                    preview: self.tabs.is_preview(&TabKey::of(&b.path)),
                }
            })
            .collect();
        let mru = self.tabs.mru_indices();
        EditorSession {
            tabs,
            active: self.tabs.active(),
            mru: if mru.len() > 1 { mru } else { vec![] },
        }
    }

    /// Reopens saved tabs (missing files are skipped) without starting a
    /// language server; the server starts when the view first gets focus.
    pub fn restore_session(
        &mut self,
        session: &EditorSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.defer_lsp = true;
        let mut active = None;
        // Saved index → restored index (skipped files shift the rest).
        let mut restored: Vec<Option<usize>> = vec![None; session.tabs.len()];
        for (i, t) in session.tabs.iter().enumerate() {
            let path = self.root.join(&t.path);
            if self.find_buffer(&path).is_some() {
                continue;
            }
            let Ok(loaded) = text::load(&path) else {
                continue;
            };
            let b = self.make_buffer(&path, loaded, window, cx);
            let pos = lt::Position::new(t.line.saturating_sub(1), t.col.saturating_sub(1));
            b.state
                .update(cx, |s, cx| s.set_cursor_position(pos, window, cx));
            let ix = self.tabs.push(TabKey::of(&b.path), t.preview);
            debug_assert_eq!(ix, self.buffers.len());
            self.buffers.push(b);
            restored[i] = Some(ix);
            cx.emit(EditorEvent::FileOpened(path));
            if session.active == Some(i) {
                active = Some(ix);
            }
        }
        let mru: Vec<usize> = session
            .mru
            .iter()
            .filter_map(|&i| restored.get(i).copied().flatten())
            .collect();
        self.tabs.set_mru_indices(&mru);
        if let Some(ix) = active.or((!self.buffers.is_empty()).then_some(0)) {
            self.activate(ix, window, cx);
            if let Some(p) = self.active_path().map(Path::to_path_buf) {
                self.tree.reveal(&p);
            }
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[core::prelude::v1::test]
    fn session_json_is_stable() {
        let s = EditorSession {
            tabs: vec![TabSession {
                path: "src/main.rs".into(),
                line: 3,
                col: 5,
                preview: false,
            }],
            active: Some(0),
            mru: vec![],
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(
            j,
            serde_json::json!({"tabs":[{"path":"src/main.rs","line":3,"col":5}],"active":0})
        );
        let back: EditorSession =
            serde_json::from_value(serde_json::json!({"tabs":[{"path":"a"}]})).unwrap();
        assert_eq!(back.tabs[0].line, 1);
        assert_eq!(back.active, None);
        assert!(!back.tabs[0].preview && back.mru.is_empty());
        // Preview flag and Ctrl+Tab order round-trip (older layouts lack them).
        let s = EditorSession {
            tabs: vec![
                TabSession {
                    path: "a".into(),
                    line: 1,
                    col: 1,
                    preview: false,
                },
                TabSession {
                    path: "b".into(),
                    line: 2,
                    col: 1,
                    preview: true,
                },
            ],
            active: Some(1),
            mru: vec![1, 0],
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["tabs"][1]["preview"], true);
        assert!(j["tabs"][0].get("preview").is_none());
        assert_eq!(serde_json::from_value::<EditorSession>(j).unwrap(), s);
    }
}
