//! Files and editor panels (docs/13 §2) and the language-server cap.

use gpui_kit::component::WindowExt;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::dock::{DockPlacement, PanelId, panel_handle};
use gpui_kit::*;

use crate::editor_panel::EditorPanel;

use super::Workspace;
use super::notifications::ToastKind;

/// Language servers allowed to run at once across all task editors
/// (rust-analyzer is memory heavy); the least recently used one is
/// suspended. `POD_APP_LSP_MAX` overrides it (tests).
fn lsp_cap() -> usize {
    if crate::fixture_scenes::lsp_max() > 0 {
        return crate::fixture_scenes::lsp_max();
    }
    std::env::var("POD_APP_LSP_MAX")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(2)
}

impl Workspace {
    // ---- files and editor (P3, docs/13 §2) --------------------------------

    pub(crate) fn live_editors(&self) -> Vec<Entity<EditorPanel>> {
        self.editors.iter().filter_map(|w| w.upgrade()).collect()
    }

    pub(super) fn editor_of(&self, task: &str, cx: &App) -> Option<Entity<EditorPanel>> {
        self.live_editors()
            .into_iter()
            .find(|e| e.read(cx).task_id == task)
    }

    /// The task the user is working in: an editor or Review panel holding
    /// focus, else the focused terminal's task, else the last editor used,
    /// else the first task with a worktree.
    pub(crate) fn current_task(&self, window: &Window, cx: &App) -> Option<String> {
        self.focused_task(window, cx).or_else(|| {
            self.tasks
                .iter()
                .find(|t| t.worktree.is_some())
                .map(|t| t.id.clone())
        })
    }

    /// Current panel context, retained while chrome has focus. Unlike command
    /// defaults, the status bar must not invent a context from the first task.
    pub(super) fn focused_task(&self, window: &Window, cx: &App) -> Option<String> {
        for e in self.live_editors() {
            let e = e.read(cx);
            if e.editor
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
            {
                return Some(e.task_id.clone());
            }
        }
        for r in self.live_reviews() {
            let r = r.read(cx);
            if r.focus_handle().contains_focused(window, cx) {
                return Some(r.task_id.clone());
            }
        }
        let by_terminal = self.focused_terminal.as_deref().and_then(|t| {
            self.tasks.iter().find(|x| {
                x.terminals.iter().any(|y| y == t)
                    || self.terminal(t).and_then(|v| v.task_id.as_deref()) == Some(x.id.as_str())
            })
        });
        by_terminal.map(|t| t.id.clone()).or_else(|| {
            self.last_editor
                .clone()
                .filter(|id| self.task(id).is_some())
        })
    }

    /// Show the files/editor panel of `task_id` (open it if needed), then
    /// optionally open `rel` at 1-based `line`/`col` or show quick open.
    pub fn open_editor(
        &mut self,
        task_id: &str,
        target: Option<(String, u32, u32)>,
        quick_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = match self.editor_of(task_id, cx) {
            Some(p) => {
                let pid = PanelId::from(p.entity_id());
                self.dock
                    .update(cx, |a, cx| a.select_panel(pid, window, cx));
                p
            }
            None => {
                let Some(task) = self.task(task_id).cloned() else {
                    self.raise_toast(
                        ToastKind::Error,
                        crate::strings::NO_SUCH_TASK,
                        task_id.to_string(),
                        cx,
                    );
                    return;
                };
                let root = task
                    .worktree
                    .clone()
                    .filter(|w| !w.is_empty() && std::path::Path::new(w).is_dir());
                let Some(root) = root else {
                    self.raise_toast(
                        ToastKind::Warning,
                        crate::tr!(NO_WORKTREE_FOR_FILES; ; task.id),
                        task.worktree.clone().unwrap_or_default(),
                        cx,
                    );
                    return;
                };
                let ws = cx.entity().downgrade();
                let panel = cx.new(|cx| {
                    EditorPanel::new(
                        task.id.clone(),
                        std::path::PathBuf::from(root),
                        None,
                        ws,
                        window,
                        cx,
                    )
                });
                self.editors.push(panel.downgrade());
                self.dock.update(cx, |a, cx| {
                    a.add_panel_view(
                        panel_handle(panel.clone()),
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    )
                });
                panel
            }
        };
        self.last_editor = Some(task_id.to_string());
        let editor = panel.read(cx).editor.clone();
        let err = editor.update(cx, |e, cx| {
            if let Some((rel, line, col)) = &target {
                let err = e
                    .goto(std::path::Path::new(rel), *line, *col, window, cx)
                    .err()
                    .map(|err| format!("{rel}: {err}"));
                // goto only moves the cursor: put keyboard focus in the editor too.
                if let Some(st) = e.active_editor() {
                    st.update(cx, |s, cx| s.focus(window, cx));
                }
                return err;
            }
            if quick_open {
                e.open_quick_open(window, cx);
            } else {
                e.focus_handle(cx).focus(window, cx);
            }
            None
        });
        if let Some(e) = err {
            self.raise_toast(ToastKind::Error, crate::strings::OPEN_FILES, e, cx);
        }
        self.touch(cx);
    }

    /// Ctrl+P / palette: quick open in the current task's worktree.
    pub(crate) fn quick_open_files(
        &mut self,
        quick_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.current_task(window, cx) {
            Some(id) => self.open_editor(&id, None, quick_open, window, cx),
            None => self.raise_toast(
                ToastKind::Info,
                crate::strings::NO_TASK_FOR_FILES,
                crate::strings::PICK_A_TASK_FIRST,
                cx,
            ),
        }
    }

    pub(crate) fn save_all_editors(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for e in self.live_editors() {
            let ed = e.read(cx).editor.clone();
            ed.update(cx, |e, cx| e.save_all(window, cx));
        }
    }

    /// Unsaved files across all editors, as `task: path` labels.
    pub(crate) fn unsaved_files(&self, cx: &App) -> Vec<String> {
        let mut out = Vec::new();
        for e in self.live_editors() {
            let e = e.read(cx);
            for p in e.editor.read(cx).dirty_paths() {
                out.push(format!(
                    "{} · {}",
                    e.task_id,
                    crate::editor_panel::rel(&e.root, &p)
                ));
            }
        }
        out
    }

    pub(crate) fn editor_layout_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_save(cx);
        self.touch(cx);
    }

    /// A file was saved in a task's editor: its Review diff is stale.
    pub(crate) fn editor_saved(&mut self, task: &str, rel: Option<String>, cx: &mut Context<Self>) {
        // v9: podd recomputes git now and announces `task.changed touched`.
        self.source.touch_task(task, rel.into_iter().collect());
        for r in self.live_reviews() {
            if r.read(cx).task_id == task {
                r.update(cx, |r, cx| r.refresh_diff(cx));
            }
        }
    }

    pub fn editor_removed(&mut self, panel: &WeakEntity<EditorPanel>, cx: &mut Context<Self>) {
        self.editors.retain(|w| w != panel && w.upgrade().is_some());
        self.enforce_lsp_cap(cx);
        self.schedule_save(cx);
        self.touch(cx);
    }

    /// v9 `task.removing`: podd is about to delete this task's worktree
    /// (from the app or `pod task remove`). Unsaved edits are saved: podd
    /// re-checks after its grace period and then stops with `TASK_DIRTY`,
    /// so nothing typed is lost. A clean editor is closed (its language
    /// servers release the folder). Removals the user already confirmed in
    /// the app (with "저장하지 않고") only close.
    pub(crate) fn task_removing(
        &mut self,
        id: &str,
        worktree: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(e) = self.editor_of(id, cx) else {
            return;
        };
        let ed = e.read(cx).editor.clone();
        let dirty = ed.read(cx).dirty_paths();
        let confirmed = self.removal_confirmed.remove(id);
        if dirty.is_empty() || confirmed {
            self.close_editor_of(id, window, cx);
            return;
        }
        ed.update(cx, |e, cx| e.save_all(window, cx));
        self.raise_toast(
            ToastKind::Warning,
            crate::strings::TASK_REMOVING_SAVED,
            crate::tr!(TASK_REMOVING_SAVED_BODY; ; id, dirty.len(), worktree),
            cx,
        );
    }

    /// Task removed: close its editor (stops its language servers).
    pub(crate) fn close_editor_of(
        &mut self,
        task: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.editor_of(task, cx) {
            self.dock.update(cx, |a, cx| a.remove_panel(p, window, cx));
        }
    }

    /// Ctrl+Shift+T: a new terminal, except inside a files panel, where it
    /// reopens the last closed editor tab (docs/13 §9). Context-free
    /// bindings outrank the editor's `PodEditor` binding in GPUI, so the
    /// choice is made here.
    pub(crate) fn new_terminal_or_reopen_tab(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let focused = self
            .live_editors()
            .into_iter()
            .map(|e| e.read(cx).editor.clone())
            .find(|ed| ed.read(cx).focus_handle(cx).contains_focused(window, cx));
        match focused {
            Some(ed) => {
                ed.update(cx, |e, cx| e.reopen_closed(window, cx));
            }
            None => self.open_new_dialog(window, cx),
        }
    }

    /// Theme switched: re-skin editors (the global theme is already applied).
    pub(crate) fn refresh_editor_theme(&mut self, cx: &mut Context<Self>) {
        for e in self.live_editors() {
            let ed = e.read(cx).editor.clone();
            ed.update(cx, |e, cx| e.set_mode(crate::theme::mode(), cx));
        }
    }

    /// Keep at most [`lsp_cap`] language servers running across all task
    /// editors; suspend the least recently used ones. Called whenever a
    /// store's servers change.
    pub fn enforce_lsp_cap(&mut self, cx: &mut Context<Self>) {
        let mut all = Vec::new();
        for e in self.live_editors() {
            let (task, store) = (
                e.read(cx).task_id.clone(),
                e.read(cx).editor.read(cx).lsp().clone(),
            );
            for r in store.read(cx).running_servers() {
                all.push((task.clone(), store.clone(), r));
            }
        }
        let cap = lsp_cap();
        if all.len() > cap {
            all.sort_by_key(|(_, _, r)| r.last_used);
            let excess = all.len() - cap;
            for (task, store, r) in all.into_iter().take(excess) {
                store.update(cx, |s, cx| s.suspend(&r.key, cx)).detach();
                self.raise_toast(
                    ToastKind::Info,
                    crate::strings::LSP_SUSPENDED,
                    crate::tr!(LSP_SUSPENDED_BODY; ; cap, r.key.id, task),
                    cx,
                );
            }
        }
        cx.notify();
    }

    /// Status-bar text for language servers: the server of the active file in
    /// the editor used last (short label), plus how many others run.
    pub(super) fn lsp_status(&self, cx: &App) -> Option<(String, u32)> {
        let editors = self.live_editors();
        if let Some((store, _)) = self.lsp_target(cx)
            && !store.read(cx).is_enabled()
        {
            return Some((
                crate::strings::LSP_OFF_LABEL.into(),
                crate::theme::tokens().text_muted,
            ));
        }
        let mut running = 0usize;
        let mut main: Option<pod_editor::lsp::ServerStatus> = None;
        for e in &editors {
            let e = e.read(cx);
            let ed = e.editor.read(cx);
            let store = ed.lsp().read(cx);
            running += store.running_servers().len();
            if self.last_editor.as_deref() == Some(e.task_id.as_str())
                && let Some(p) = ed.active_path()
            {
                main = store.status_for(p).or(main);
            }
        }
        if main.is_none() {
            main = editors.iter().find_map(|e| {
                e.read(cx)
                    .editor
                    .read(cx)
                    .lsp()
                    .read(cx)
                    .statuses()
                    .into_iter()
                    .find(|s| s.state.is_running())
            });
        }
        let t = crate::theme::tokens();
        let main = main?;
        use pod_editor::lsp::ServerState as S;
        let color = match (&main.state, &main.progress) {
            (S::Running, None) => t.success_fg,
            (S::Running, Some(_)) | (S::Starting | S::Checking, _) => t.info_fg,
            (S::Restarting { .. }, _) => t.warning_fg,
            (S::Failed(_), _) => t.danger_fg,
            _ => t.text_muted,
        };
        let others = running.saturating_sub(usize::from(
            main.state.is_running()
                || matches!(main.state, S::Starting | S::Checking | S::Restarting { .. }),
        ));
        let label = main.short_label();
        Some((
            if others > 0 {
                crate::tr!(LSP_MORE; ; label, others)
            } else {
                label
            },
            color,
        ))
    }

    /// The language-server store the status bar acts on: the editor used
    /// last (else the first open one) and its active file.
    pub(crate) fn lsp_target(
        &self,
        cx: &App,
    ) -> Option<(
        Entity<pod_editor::lsp::LspStore>,
        Option<std::path::PathBuf>,
    )> {
        let editors = self.live_editors();
        let e = editors
            .iter()
            .find(|e| self.last_editor.as_deref() == Some(e.read(cx).task_id.as_str()))
            .or(editors.first())?;
        let ed = e.read(cx).editor.read(cx);
        Some((ed.lsp().clone(), ed.active_path().map(|p| p.to_path_buf())))
    }

    /// Status-bar menu: 다시 시작 (also turns a stopped store back on).
    pub(crate) fn lsp_restart(&mut self, cx: &mut Context<Self>) {
        let Some((store, path)) = self.lsp_target(cx) else {
            return;
        };
        store.update(cx, |s, cx| {
            if !s.is_enabled() {
                s.set_enabled(true, cx);
            } else if let Some(p) = &path {
                s.restart(p, cx);
            }
        });
        self.enforce_lsp_cap(cx);
    }

    /// Status-bar menu: 끄기 (this task's editor; 다시 시작 turns it on).
    pub(crate) fn lsp_turn_off(&mut self, cx: &mut Context<Self>) {
        let Some((store, _)) = self.lsp_target(cx) else {
            return;
        };
        store.update(cx, |s, cx| s.set_enabled(false, cx));
        self.raise_toast(
            ToastKind::Info,
            crate::strings::LSP_TURNED_OFF,
            crate::strings::LSP_TURNED_OFF_BODY,
            cx,
        );
        cx.notify();
    }

    /// Status-bar menu: 로그 보기.
    pub(crate) fn lsp_show_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((store, path)) = self.lsp_target(cx) else {
            return;
        };
        let text = store.read(cx).server_log(path.as_deref());
        let text = if text.trim().is_empty() {
            crate::strings::LSP_LOG_EMPTY.to_string()
        } else {
            text
        };
        let font = cx.global::<crate::theme::Settings>().terminal_font.clone();
        window.open_dialog(cx, move |d: Dialog, _, _| {
            d.title(crate::strings::LSP_LOG_TITLE).w(px(760.)).child(
                div()
                    .id("lsp-log")
                    .role(Role::Document)
                    .aria_label(crate::strings::LSP_LOG_TITLE)
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .font_family(font.clone())
                    .text_xs()
                    .children(
                        text.lines()
                            .map(|l| div().child(l.to_string()))
                            .collect::<Vec<_>>(),
                    ),
            )
        });
    }
}
