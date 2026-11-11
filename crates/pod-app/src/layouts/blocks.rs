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
//!
//! Files (docs/20-refactor/app.md A-P13): [`super::blocks_model`] holds the
//! pure model (output → blocks, and the feed on top), `controller` the
//! [`Feed`] entity with its attach streams and event handlers, `view` the
//! render, `copy` the strings. This file keeps the layout module, the app's
//! one feed and the legacy adapter that renders it through
//! `LayoutModule::render_body`. The feed reads through the kit's frame and
//! hands UI intents to [`crate::layouts::kit::LayoutActions`]; what the kit
//! does not carry yet stays on the workspace (see `controller`).
mod controller;
pub mod copy;
#[cfg(test)]
mod tests;
mod view;

use gpui_kit::*;

pub use self::controller::Feed;
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit;
use crate::workspace::Workspace;

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
    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        render_body(ws, window, cx)
    }
}

struct BlocksKeys;
impl Global for BlocksKeys {}
struct FeedGlobal(Entity<Feed>);
impl Global for FeedGlobal {}

#[cfg(test)]
/// The feed of this app, if the layout was shown once (tests).
pub fn feed(cx: &App) -> Option<Entity<Feed>> {
    cx.try_global::<FeedGlobal>().map(|g| g.0.clone())
}

// ───────── legacy adapter: the body through `LayoutModule::render_body` ─────────

/// The window body: the app's one feed in the shell. The feed is made once,
/// with the workspace (legacy adapter) and the host's actions; whether the
/// workspace runs on the read-only UI fixture is read then.
fn render_body(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    if cx.try_global::<BlocksKeys>().is_none() {
        cx.bind_keys([
            KeyBinding::new("f6", SwitchRegion, Some(KEY_CONTEXT)),
            KeyBinding::new("tab", NextTarget, Some(INPUT_CONTEXT)),
            KeyBinding::new("shift-tab", PrevTarget, Some(INPUT_CONTEXT)),
            KeyBinding::new("escape", LeaveInput, Some(INPUT_CONTEXT)),
        ]);
        cx.set_global(BlocksKeys);
    }
    let feed = match cx.try_global::<FeedGlobal>().map(|g| g.0.clone()) {
        Some(f) => f,
        None => {
            let weak = cx.entity().downgrade();
            let actions = ws.layout_actions(cx);
            let fixture = ws.source.pipe() == "ui-fixture-no-daemon";
            let f = cx.new(|cx| Feed::new(weak, actions, fixture, window, cx));
            // The app keeps the feed (and its view state) while it runs.
            cx.set_global(FeedGlobal(f.clone()));
            f
        }
    };
    view::shell(
        kit::connection_warning(&ws.conn),
        ws.render_hook_banner(cx),
        feed.into_any_element(),
        ws.render_toasts(cx).into_any_element(),
    )
}
