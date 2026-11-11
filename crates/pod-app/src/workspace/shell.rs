//! The window shell: title, app-level actions, the layout's body, the status
//! bar, and quit.

use gpui_kit::*;

use super::{ArrangeGrid, NewTerminal, OpenFilesPanel, OpenReview, QuickOpenFiles, Workspace};

impl Workspace {
    /// App quit: persist the layout (editor cursors included) and stop every
    /// language server (Job Objects kill the trees even if this is cut short).
    pub(crate) fn on_quit(&mut self, cx: &mut Context<Self>) -> Task<()> {
        if self.restored {
            self.save_now(cx);
        }
        let mut tasks: Vec<Task<()>> = Vec::new();
        for e in self.live_editors() {
            let ed = e.read(cx).editor.clone();
            tasks.push(ed.update(cx, |e, cx| e.shutdown(cx)));
        }
        cx.background_spawn(async move {
            futures::future::join_all(tasks).await;
        })
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::stats::record_frame();
        let task = self
            .focused_terminal
            .as_deref()
            .and_then(|id| self.terminal(id))
            .and_then(|t| {
                t.task_id
                    .as_deref()
                    .or_else(|| {
                        self.tasks
                            .iter()
                            .find(|k| k.terminals.contains(&t.id))
                            .map(|k| k.id.as_str())
                    })
                    .and_then(|id| self.task(id))
                    .map(|k| k.title.clone())
            });
        let title = os_window_title(task.as_deref());
        if title != self.os_title {
            window.set_window_title(&title);
            self.os_title = title;
        }
        // Every mode draws its body through the layout kit (layout_host.rs).
        let renderer = crate::layout_mode::renderer(cx);
        let body = self.render_layout_body(&*renderer, window, cx);
        let root = div()
            .id("workspace-root")
            .track_focus(&self.root_focus)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(crate::theme::tokens().bg_canvas))
            .text_color(rgb(crate::theme::tokens().text_primary));
        // The chrome's keys: the palette, then each static command's key.
        crate::chrome::key_actions(root, cx)
            .child(self.title_bar(cx))
            .on_action(cx.listener(|w, _: &NewTerminal, window, cx| {
                w.new_terminal_or_reopen_tab(window, cx)
            }))
            .on_action(cx.listener(|w, _: &ArrangeGrid, window, cx| w.arrange_grid(window, cx)))
            .on_action(
                cx.listener(|w, _: &OpenReview, window, cx| w.open_review_for_focus(window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &QuickOpenFiles, window, cx| {
                    w.quick_open_files(true, window, cx)
                }),
            )
            .on_action(cx.listener(|w, _: &OpenFilesPanel, window, cx| {
                w.quick_open_files(false, window, cx)
            }))
            .on_action(cx.listener(|w, _: &crate::orch::NewGroup, window, cx| {
                w.open_new_group_dialog(window, cx)
            }))
            .on_action(
                cx.listener(|w, _: &crate::orch::OpenGates, window, cx| w.open_gates(window, cx)),
            )
            .on_action(
                cx.listener(|w, _: &crate::orch::OpenAgentTree, window, cx| {
                    w.open_agent_tree(window, cx)
                }),
            )
            .on_action(
                cx.listener(|w, _: &crate::orch_ui::OpenRunBoard, window, cx| {
                    crate::orch_ui::open_page(w, crate::orch_ui::Page::Board, window, cx)
                }),
            )
            .on_action(
                cx.listener(|w, _: &crate::orch_ui::OpenOrchInbox, window, cx| {
                    crate::orch_ui::open_inbox(w, window, cx)
                }),
            )
            .child(body)
            .child(self.render_status_bar(window, cx))
    }
}

/// The OS window title (ui-03): `Pod`, or `<작업> — Pod` while a task's terminal has focus.
pub(crate) fn os_window_title(task: Option<&str>) -> String {
    match task.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => format!("{t} — Pod"),
        None => "Pod".into(),
    }
}
