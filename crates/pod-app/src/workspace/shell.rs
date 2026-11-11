//! The window shell: title, app-level actions, body and status bar, the
//! workbench body and its empty state, and quit.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::layouts::kit;

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
        // A mode on the layout kit draws through the host (layout_host.rs);
        // the others keep their `LayoutModule::render_body`.
        let body = match crate::layout_mode::renderer(cx) {
            Some(renderer) => self.render_layout_body(&*renderer, window, cx),
            None => crate::layout_mode::render_body(self, window, cx),
        };
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

impl Workspace {
    /// 작업대 layout body (crate::layout_mode): sidebar column + dock, styled by the
    /// active surface model (docs/10 §D5.2): flat hairlines, layered/islands rounded
    /// panels with gaps, mono without borders. The connection warning and the
    /// islands are the layout kit's.
    pub(crate) fn render_workbench_body(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let banner = kit::connection_warning(&self.conn);
        let empty = self.live_panels().is_empty()
            && self.live_reviews().is_empty()
            && self.live_editors().is_empty()
            && !self.orch.has_panels()
            && self
                .settings_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_none()
            && !crate::mods_ui::page_open(cx)
            && !crate::sessions_ui::page_open(cx)
            && !crate::orch_ui::page_open(cx)
            && self
                .inspector_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_none();
        let m = pod_ui::themes::metrics();
        let island = kit::Island::current();
        let gap = island.gap();
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
            .child(
                div()
                    .w(px(m.density.sidebar))
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(gap))
                    .child(
                        island
                            .wrap(div().flex_1().min_h_0())
                            .child(self.sidebar.clone()),
                    )
                    .child(self.render_toasts(cx)),
            )
            .child(
                island
                    .wrap(div())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .relative()
                    .when_some(banner, |d, b| d.child(kit::warning_banner(b)))
                    .when_some(self.render_hook_banner(cx), |d, b| d.child(b))
                    .child(div().flex_1().min_h_0().child(self.dock.clone()))
                    .when(empty, |d| {
                        d.child(
                            div()
                                .absolute()
                                .inset_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_sm()
                                .text_color(rgb(crate::theme::tokens().text_muted))
                                .child({
                                    // One CTA for the situation (docs/10 §D4.6): no repository → open
                                    // one; registered → a new task; still connecting → none.
                                    let state = EmptyState::of(
                                        self.conn.is_connected(),
                                        self.repos.is_empty() && self.tasks.is_empty(),
                                        self.tasks.is_empty(),
                                    );
                                    let (title, body, icon) = state.copy();
                                    let cta = match state {
                                        EmptyState::Connecting => {
                                            crate::chrome::loading_placeholder(
                                                crate::strings::CONNECTING_TO_YOUR_WORKSPACE,
                                            )
                                            .into_any_element()
                                        }
                                        EmptyState::NoRepository => Button::new("empty-repo")
                                            .label(crate::strings::OPEN_REPOSITORY)
                                            .primary()
                                            .on_click(cx.listener(|w, _, window, cx| {
                                                w.open_repository(window, cx)
                                            }))
                                            .into_any_element(),
                                        EmptyState::NoTask | EmptyState::PickTask => {
                                            Button::new("empty-task")
                                                .label(crate::strings::NEW_TASK)
                                                .primary()
                                                .on_click(cx.listener(|w, _, window, cx| {
                                                    w.open_new_task_dialog(window, cx)
                                                }))
                                                .into_any_element()
                                        }
                                    };
                                    pod_ui::Frame::new(
                                        "workspace-empty",
                                        pod_ui::FrameKind::Empty,
                                        title,
                                        body,
                                    )
                                    .icon(icon)
                                    .render(
                                        crate::theme::mode(),
                                        [div().pt_2().child(cta).into_any_element()],
                                    )
                                }),
                        )
                    }),
            )
            .into_any_element()
    }
}

/// What the empty workspace says, with exactly one action (docs/10 §D4.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EmptyState {
    Connecting,
    NoRepository,
    NoTask,
    PickTask,
}

impl EmptyState {
    pub(crate) fn of(connected: bool, no_repository: bool, no_task: bool) -> Self {
        match (connected, no_repository, no_task) {
            (false, _, _) => Self::Connecting,
            (true, true, _) => Self::NoRepository,
            (true, false, true) => Self::NoTask,
            _ => Self::PickTask,
        }
    }
    /// Title, description and icon.
    pub(crate) fn copy(self) -> (&'static str, &'static str, pod_ui::Icon) {
        match self {
            Self::Connecting => (
                crate::strings::CONNECTING_TO_YOUR_WORKSPACE,
                crate::strings::YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_,
                pod_ui::Icon::Plug,
            ),
            Self::NoRepository => (
                crate::strings::A_CLEAR_SPACE_FOR_YOUR_NEXT_TASK,
                crate::strings::OPEN_A_REPOSITORY_CREATE_A_TASK_OR_RETURN_TO_AN_EXISTING_TERMINAL,
                pod_ui::Icon::FolderGit2,
            ),
            Self::NoTask => (
                crate::strings::EMPTY_NO_TASK_TITLE,
                crate::strings::EMPTY_NO_TASK,
                pod_ui::Icon::Layers,
            ),
            Self::PickTask => (
                crate::strings::EMPTY_PICK_TASK_TITLE,
                crate::strings::EMPTY_PICK_TASK,
                pod_ui::Icon::Layers,
            ),
        }
    }
}

/// The OS window title (ui-03): `Pod`, or `<작업> — Pod` while a task's terminal has focus.
pub(crate) fn os_window_title(task: Option<&str>) -> String {
    match task.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => format!("{t} — Pod"),
        None => "Pod".into(),
    }
}
