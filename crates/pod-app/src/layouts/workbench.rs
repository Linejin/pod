//! 작업대 (docs/10 §D5.4): the tree sidebar beside the dock, the base of every
//! preset and the mode an unknown layout id falls back to.
//!
//! It draws through the layout kit like the six other modes (card T2): the
//! sidebar, dock, toast stack and hook prompt are the host's slots, the open
//! panels and the connection come from the frame, and the empty state's
//! buttons are intents. Nothing here names the workspace.
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{self, LayoutFrame, LayoutHost, LayoutRenderer, Slot, UiIntent};

/// The layout module (registered in [`crate::layout_mode::LayoutRegistry::new`]).
pub struct Workbench;

impl LayoutModule for Workbench {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Workbench
    }
    fn renderer(&self) -> Rc<dyn LayoutRenderer> {
        Rc::new(Renderer)
    }
}

struct Renderer;

impl LayoutRenderer for Renderer {
    /// Sidebar column + dock, styled by the active surface model (docs/10
    /// §D5.2): flat hairlines, layered/islands rounded panels with gaps, mono
    /// without borders. The connection warning and the islands are the kit's.
    fn render_body(
        &self,
        frame: &LayoutFrame<'_>,
        host: &mut dyn LayoutHost,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let banner = frame.warning();
        // Nothing open in the dock or beside it (the app-level pages count).
        let detail = &frame.panel_detail;
        let empty = frame.panels.dock_empty()
            && !detail.mods_page
            && !detail.sessions_page
            && !detail.orch_page;
        let sidebar = host.slot(Slot::Sidebar, window, cx);
        let toasts = host.slot(Slot::Toasts, window, cx);
        let hook_banner = host.slot(Slot::HookBanner, window, cx);
        let dock = host.slot(Slot::Dock, window, cx);
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
                    .child(island.wrap(div().flex_1().min_h_0()).children(sidebar))
                    .children(toasts),
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
                    .children(hook_banner)
                    .child(div().flex_1().min_h_0().children(dock))
                    .when(empty, |d| {
                        // One CTA for the situation (docs/10 §D4.6): no repository → open
                        // one; registered → a new task; still connecting → none.
                        let state = EmptyState::of(
                            frame.conn.is_connected(),
                            frame.repos.is_empty() && frame.tasks.is_empty(),
                            frame.tasks.is_empty(),
                        );
                        d.child(empty_state(state, host))
                    }),
            )
            .into_any_element()
    }
}

/// The empty workspace's frame over the dock, with its one button.
fn empty_state(state: EmptyState, host: &dyn LayoutHost) -> Div {
    let (title, body, icon) = state.copy();
    let intent = |intent: UiIntent| {
        let actions = host.actions();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
            actions.dispatch(intent.clone(), window, cx)
        }
    };
    let cta = match state {
        EmptyState::Connecting => {
            crate::chrome::loading_placeholder(crate::strings::CONNECTING_TO_YOUR_WORKSPACE)
                .into_any_element()
        }
        EmptyState::NoRepository => Button::new("empty-repo")
            .label(crate::strings::OPEN_REPOSITORY)
            .primary()
            .on_click(intent(UiIntent::OpenRepository))
            .into_any_element(),
        EmptyState::NoTask | EmptyState::PickTask => Button::new("empty-task")
            .label(crate::strings::NEW_TASK)
            .primary()
            .on_click(intent(UiIntent::NewTask))
            .into_any_element(),
    };
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .text_color(rgb(crate::theme::tokens().text_muted))
        .child(
            pod_ui::Frame::new("workspace-empty", pod_ui::FrameKind::Empty, title, body)
                .icon(icon)
                .render(
                    crate::theme::mode(),
                    [div().pt_2().child(cta).into_any_element()],
                ),
        )
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
