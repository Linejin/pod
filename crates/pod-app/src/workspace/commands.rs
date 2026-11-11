//! Commands the workspace routes: panel actions ([`WorkspaceAction`]) and the
//! terminal/task requests they send to podd.

use gpui_kit::*;

use crate::layout::label_key;
use crate::panel::TerminalPanel;
use crate::source::{Program, RunSpec};

use super::Workspace;
use super::notifications::ToastKind;

/// Panel-level actions (tab menu, sidebar buttons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceAction {
    /// Open the terminal in one more panel.
    Duplicate,
    Rename,
    /// Kill the terminal (asks first).
    Kill,
    /// Close this panel only (detach).
    ClosePanel,
}

impl Workspace {
    // ---- actions --------------------------------------------------------

    pub fn on_action(
        &mut self,
        action: WorkspaceAction,
        id: String,
        panel: Option<WeakEntity<TerminalPanel>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            WorkspaceAction::Duplicate => self.open_terminal(&id, true, window, cx),
            WorkspaceAction::Rename => self.open_rename_dialog(id, window, cx),
            WorkspaceAction::Kill => self.confirm_kill(id, window, cx),
            WorkspaceAction::ClosePanel => {
                if let Some(p) = panel.and_then(|p| p.upgrade()) {
                    self.dock.update(cx, |a, cx| a.remove_panel(p, window, cx));
                }
            }
        }
    }

    pub(super) fn kill(&mut self, id: String, cx: &mut Context<Self>) {
        let rx = self.source.kill(&id);
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = rx.await {
                let _ = this.update(cx, |w, cx| {
                    w.raise_toast(
                        ToastKind::Error,
                        crate::tr!(KILL_ID_FAILED; id;),
                        e.message,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    /// Remove an exited terminal from podd (and close its panels).
    pub(super) fn remove(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.close_panels_of(&id, window, cx);
        let rx = self.source.remove(&id);
        cx.spawn(async move |this, cx| {
            if let Ok(Err(e)) = rx.await {
                let _ = this.update(cx, |w, cx| {
                    w.raise_toast(
                        ToastKind::Error,
                        crate::tr!(REMOVE_ID_FAILED; id;),
                        e.message,
                        cx,
                    )
                });
            }
        })
        .detach();
    }

    pub(super) fn rename(&mut self, id: String, name: String, cx: &mut Context<Self>) {
        let Some(t) = self.terminal(&id).cloned() else {
            return;
        };
        let key = label_key(&t.id, t.created_ms);
        if self.caps.rename {
            let rx = self.source.rename(&id, &name);
            cx.spawn(async move |this, cx| {
                if let Ok(Err(e)) = rx.await {
                    let _ = this.update(cx, |w, cx| {
                        w.raise_toast(
                            ToastKind::Error,
                            crate::strings::RENAME_FAILED,
                            e.message,
                            cx,
                        )
                    });
                }
            })
            .detach();
            self.saved.labels.remove(&key);
        } else if name.is_empty() {
            self.saved.labels.remove(&key);
        } else {
            // podd has no rename yet: keep an app-local label.
            self.saved.labels.insert(key, name);
        }
        self.refresh_labels(cx);
        self.schedule_save(cx);
        self.touch(cx);
    }

    pub(super) fn run(&mut self, spec: RunSpec, window: &mut Window, cx: &mut Context<Self>) {
        let rx = self.source.run(spec.clone());
        cx.spawn_in(window, async move |this, cx| {
            let result = rx.await.unwrap_or_else(|_| {
                Err(crate::source::SourceError {
                    code: None,
                    message: crate::strings::PODD_CLIENT_STOPPED.into(),
                })
            });
            let _ = this.update_in(cx, |w, window, cx| match result {
                Ok(t) => {
                    if !w.terminals.iter().any(|x| x.id == t.id) {
                        w.terminals.push(t.clone());
                        w.refresh_labels(cx);
                    }
                    w.open_terminal(&t.id, false, window, cx);
                }
                Err(e) => w.raise_toast(
                    ToastKind::Error,
                    crate::tr!(COULD_NOT_START; ; spec.argv.join(" ")),
                    e.message,
                    cx,
                ),
            });
        })
        .detach();
    }

    /// Ctrl+Shift+R: review the task of the focused terminal, else the only/first task.
    pub(super) fn open_review_for_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let by_focus = self.focused_terminal.as_deref().and_then(|t| {
            self.tasks
                .iter()
                .find(|x| x.terminals.iter().any(|y| y == t))
        });
        match by_focus.or(self.tasks.first()).map(|t| t.id.clone()) {
            Some(id) => self.open_review(&id, window, cx),
            None => self.raise_toast(
                ToastKind::Info,
                crate::strings::NO_TASK_TO_REVIEW,
                crate::strings::CREATE_A_TASK_FIRST_TASK,
                cx,
            ),
        }
    }

    /// 다음 응답 요청 (palette `blocked`): the first blocked terminal, else
    /// a toast that none is.
    pub(crate) fn open_next_blocked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let id = self
            .terminals
            .iter()
            .find(|t| {
                t.status
                    .as_ref()
                    .is_some_and(|s| s.state == crate::vm::AgentState::Blocked)
            })
            .map(|t| t.id.clone());
        if let Some(id) = id {
            self.open_terminal(&id, false, window, cx);
        } else {
            self.raise_toast(
                ToastKind::Info,
                crate::strings::NO_BLOCKED_REQUESTS,
                crate::strings::ALL_TERMINALS_ARE_CLEAR_OF_REPORTED_REQUESTS,
                cx,
            );
        }
    }

    /// Add a shell terminal to a task (cwd = its worktree).
    pub(crate) fn task_terminal(
        &mut self,
        task: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run(
            RunSpec {
                argv: Program::Shell.argv(""),
                task: Some(task),
                ..Default::default()
            },
            window,
            cx,
        );
    }
}
