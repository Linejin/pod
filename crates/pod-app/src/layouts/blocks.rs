//! 블록 터미널 (preset 11, docs/10 §D5.8): the selected task as a chronological
//! feed of command blocks — one per shell command or agent step — with the
//! approval inside the block and one input at the bottom.
//!
//! - **Blocks come from the output** ([`super::blocks_model`]): OSC 133 marks
//!   (Pod's own shells emit them, [`super::shell_integration`]), agent status
//!   transitions (hook / OSC 7501 via podd), prompt detection otherwise. A
//!   full-screen program is one live block; "전체 터미널로 보기" opens the raw
//!   terminal, which is never cut or re-spawned.
//! - **No forked state.** Tasks, terminals and gates are the workspace's;
//!   approvals go through `gate.resolve` (the gates inbox path), input through
//!   `terminal.input`. The feed keeps view state only: task, filter, target,
//!   selection, folds and the block text it read.
//! - **No polling.** Output arrives on a `terminal.attach` stream per terminal
//!   of the selected task (event driven, dropped when the task or layout
//!   changes); status changes arrive with the workspace's events.
//! - **Switching is live.** The dock is never touched; the raw view shows the
//!   workbench's own terminal panel when it has one (same entity, same
//!   attach), else a feed-owned one dropped when the layout changes.
//! - **Panels in a drawer.** Panels opened while the layout shows (settings,
//!   gates, agents, inspector, Review, editor, orchestration) appear in a
//!   drawer at the right that draws the dock itself, as the kanban drawer's
//!   패널 view does: it opens when a panel is asked for, F6 reaches it, Esc
//!   closes it and the keyboard goes back where it was (`controller.rs`).
//!
//! Files (docs/20-refactor/app.md A-P13): [`super::blocks_model`] holds the
//! pure model (output → blocks, and the feed on top), `controller` the
//! [`Feed`] entity with its attach streams and event handlers, `view` the
//! render, `copy` the strings. This file keeps the layout module, its kit
//! renderer ([`LayoutModule::renderer`], A-T2d) and the window's feed, kept
//! in the kit's per-window state slot. The feed reaches the host only through
//! [`crate::layouts::kit::LayoutActions`]: frame reads, UI intents, the podd
//! source, the host's terminal panels and window-less updates.
mod controller;
pub mod copy;
#[cfg(test)]
mod tests;
mod view;

use std::rc::Rc;

use gpui_kit::*;

pub use self::controller::Feed;
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{LayoutFrame, LayoutHost, LayoutRenderer, Slot};

actions!(
    pod_blocks,
    [SwitchRegion, NextTarget, PrevTarget, LeaveInput]
);

/// Key context of the layout (F6) and of its input (Tab changes the target).
pub const KEY_CONTEXT: &str = "PodBlocks";
pub const INPUT_CONTEXT: &str = "PodBlocksInput";

/// The layout module (registered in [`crate::layout_mode::LayoutRegistry::new`]).
pub struct Blocks;

impl LayoutModule for Blocks {
    fn mode(&self) -> LayoutMode {
        LayoutMode::BlockTerminal
    }
    fn renderer(&self) -> Rc<dyn LayoutRenderer> {
        Rc::new(BlocksRenderer)
    }
}

struct BlocksKeys;
impl Global for BlocksKeys {}

#[cfg(test)]
/// The feed of the window that showed the layout (tests run one window).
pub fn feed(cx: &App) -> Option<Entity<Feed>> {
    crate::layouts::kit::states::<Feed>(cx).into_iter().next()
}

/// The body through the layout kit: the window's feed in the shell.
struct BlocksRenderer;

impl LayoutRenderer for BlocksRenderer {
    /// The keys are bound once per app. The feed is made on first use with
    /// the host's actions and kept in the window's state slot (its view state
    /// lives as long as the window); whether the window runs on the
    /// read-only UI fixture is read then.
    fn render_body(
        &self,
        frame: &LayoutFrame<'_>,
        host: &mut dyn LayoutHost,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if cx.try_global::<BlocksKeys>().is_none() {
            cx.bind_keys([
                KeyBinding::new("f6", SwitchRegion, Some(KEY_CONTEXT)),
                KeyBinding::new("tab", NextTarget, Some(INPUT_CONTEXT)),
                KeyBinding::new("shift-tab", PrevTarget, Some(INPUT_CONTEXT)),
                KeyBinding::new("escape", LeaveInput, Some(INPUT_CONTEXT)),
            ]);
            cx.set_global(BlocksKeys);
        }
        let actions = host.actions();
        let fixture = frame.fixture;
        let feed = actions.state_or_insert_with(window, cx, {
            let actions = actions.clone();
            move |window, cx| cx.new(|cx| Feed::new(actions, fixture, window, cx))
        });
        view::shell(
            frame.warning(),
            host.slot(Slot::HookBanner, window, cx),
            feed.into_any_element(),
            host.slot(Slot::Toasts, window, cx)
                .unwrap_or_else(|| div().into_any_element()),
        )
    }
}
