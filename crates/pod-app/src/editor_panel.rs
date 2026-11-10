//! Files/editor dock panel (P3 integration, docs/13 §2): one
//! `pod_editor::EditorWorkspace` (file tree + editor tabs + quick open +
//! problems) per task worktree, docked next to terminals and Review.
//!
//! One view = one worktree = one `LspStore`, so language servers never mix
//! tasks. Servers start lazily on the first opened supported file (restored
//! tabs wait for the first focus); the workspace caps how many run at once
//! ([`crate::workspace::Workspace::enforce_lsp_cap`]).
//!
//! Persisted as `PodEditor {"task": id, "session": {tabs, active}}`.

use std::path::{Path, PathBuf};

use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_editor::lsp::LspStoreEvent;
use pod_editor::{EditorEvent, EditorSession, EditorWorkspace};
use serde_json::json;

use crate::workspace::{ToastKind, Workspace};

pub const EDITOR_PANEL_NAME: &str = "PodEditor";

pub struct EditorPanel {
    pub task_id: String,
    pub root: PathBuf,
    pub editor: Entity<EditorWorkspace>,
    workspace: WeakEntity<Workspace>,
    _subs: Vec<Subscription>,
}

impl EditorPanel {
    pub fn new(
        task_id: String,
        root: PathBuf,
        session: Option<&EditorSession>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor =
            cx.new(|cx| EditorWorkspace::new(root.clone(), crate::theme::mode(), window, cx));
        if crate::fixture_scenes::no_lsp() || std::env::var_os("POD_APP_NO_LSP").is_some() {
            editor.update(cx, |e, cx| {
                e.lsp().clone().update(cx, |s, cx| s.set_enabled(false, cx))
            });
        }
        if let Some(s) = session.filter(|s| !s.tabs.is_empty()) {
            editor.update(cx, |e, cx| e.restore_session(s, window, cx));
        }
        let lsp = editor.read(cx).lsp().clone();
        let subs = vec![
            cx.subscribe_in(&editor, window, Self::on_editor_event),
            cx.observe(&editor, |_, _, cx| cx.notify()),
            cx.subscribe(&lsp, |this: &mut Self, _, ev: &LspStoreEvent, cx| {
                if matches!(ev, LspStoreEvent::ServersChanged) {
                    let ws = this.workspace.clone();
                    cx.defer(move |cx| {
                        let _ = ws.update(cx, |w, cx| w.enforce_lsp_cap(cx));
                    });
                }
            }),
        ];
        Self {
            task_id,
            root,
            editor,
            workspace,
            _subs: subs,
        }
    }

    fn on_editor_event(
        &mut self,
        _: &Entity<EditorWorkspace>,
        ev: &EditorEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = self.workspace.clone();
        let task = self.task_id.clone();
        match ev {
            EditorEvent::FileOpened(_)
            | EditorEvent::FileClosed(_)
            | EditorEvent::ActiveFileChanged(_)
            | EditorEvent::TabsChanged => {
                cx.defer(move |cx| {
                    let _ = ws.update(cx, |w, cx| w.editor_layout_changed(cx));
                });
            }
            EditorEvent::DirtyChanged { .. } => {
                cx.notify();
                cx.defer(move |cx| {
                    let _ = ws.update(cx, |w, cx| w.touch(cx));
                });
            }
            EditorEvent::FileSaved(path) => {
                let rel = rel(&self.root, path);
                cx.defer(move |cx| {
                    let _ = ws.update(cx, |w, cx| w.editor_saved(&task, Some(rel), cx));
                });
            }
            EditorEvent::ExternalChange {
                path,
                reloaded: false,
            } => {
                let rel = rel(&self.root, path);
                cx.defer(move |cx| {
                    let _ = ws.update(cx, |w, cx| {
                        w.raise_toast(
                            ToastKind::Warning,
                            crate::strings::FILE_CHANGED_ON_DISK,
                            crate::tr!(FILE_CHANGED_ON_DISK_BODY; ; rel, task),
                            cx,
                        )
                    });
                });
            }
            EditorEvent::ExternalChange { reloaded: true, .. } => {}
        }
    }

    pub fn dirty_count(&self, cx: &App) -> usize {
        self.editor.read(cx).dirty_paths().len()
    }

    /// Persisted shape (layout.json).
    pub fn state_of(task: &str, session: &EditorSession) -> PanelState {
        let mut s = PanelState::new(EDITOR_PANEL_NAME);
        s.info = PanelInfo::panel(json!({ "task": task, "session": session }));
        s
    }

    #[cfg(test)]
    pub fn state_for_test(task: &str) -> PanelState {
        Self::state_of(task, &EditorSession::default())
    }

    pub fn session_of_state(state: &PanelState) -> Option<EditorSession> {
        match &state.info {
            PanelInfo::Panel(v) => v
                .get("session")
                .and_then(|s| serde_json::from_value(s.clone()).ok()),
            _ => None,
        }
    }
}

pub fn rel(root: &Path, p: &Path) -> String {
    pod_editor::uri::rel_path(root, p)
}

impl EventEmitter<PanelEvent> for EditorPanel {}

impl Focusable for EditorPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl Panel for EditorPanel {
    fn panel_name(&self) -> &'static str {
        EDITOR_PANEL_NAME
    }

    fn dump(&self, cx: &App) -> PanelState {
        Self::state_of(&self.task_id, &self.editor.read(cx).session(cx))
    }

    /// Unsaved files keep the panel open: save or close those tabs first
    /// (each asks to save). The ● in the title shows why.
    fn closable(&self, cx: &App) -> bool {
        self.dirty_count(cx) == 0
    }

    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        // Stop this worktree's language servers (Job Object kills the tree
        // after the 2 s polite shutdown).
        self.editor.update(cx, |e, cx| e.shutdown(cx)).detach();
        let ws = self.workspace.clone();
        let me = cx.entity().downgrade();
        cx.defer(move |cx| {
            let _ = ws.update(cx, |w, cx| w.editor_removed(&me, cx));
        });
    }
}

impl gpui_kit::component::dock::Panel for EditorPanel {
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        let dirty = self.dirty_count(cx);
        Some(crate::tr!(FILES_TAB; ; self.task_id, if dirty > 0 { crate::tr!(UNSAVED_COUNT; ; dirty) } else { String::new() }).into())
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dirty = self.dirty_count(cx);
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(crate::tr!(FILES_TAB; ; self.task_id, ""))
            .when(dirty > 0, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(rgb(crate::theme::tokens().warning_fg))
                        .child(pod_ui::status_dot(crate::theme::tokens().warning_fg))
                        .child(dirty.to_string()),
                )
            })
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for EditorPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("editor-panel")
            .size_full()
            .child(self.editor.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pod_editor::TabSession;

    #[core::prelude::v1::test]
    fn state_roundtrip_keeps_tabs_and_cursor() {
        let s = EditorSession {
            tabs: vec![
                TabSession {
                    path: "src/main.rs".into(),
                    line: 12,
                    col: 3,
                    preview: false,
                },
                TabSession {
                    path: "docs/한글 노트.md".into(),
                    line: 1,
                    col: 1,
                    preview: true,
                },
            ],
            active: Some(1),
            mru: vec![1, 0],
        };
        let st = EditorPanel::state_of("sample/task-1", &s);
        assert_eq!(
            crate::layout::leaf_field(&st, "task"),
            Some("sample/task-1")
        );
        assert_eq!(EditorPanel::session_of_state(&st), Some(s));
        // Old/partial layouts: a missing session restores an empty editor.
        let bare = {
            let mut x = PanelState::new(EDITOR_PANEL_NAME);
            x.info = PanelInfo::panel(json!({"task": "t"}));
            x
        };
        assert_eq!(EditorPanel::session_of_state(&bare), None);
    }
}
