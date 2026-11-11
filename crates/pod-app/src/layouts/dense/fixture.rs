//! The layout's half of the visual-QA scene `dense` (`--ui-fixture
//! --fixture-scene dense`). `fixture_scenes::dense` sets the fixture data up
//! inside a workspace update and lends a frame of it, so nothing here names
//! the workspace: the view is reached through the kit's state slot.
use gpui_kit::*;

use super::{DenseView, Sel, view};
use crate::layouts::kit::{LayoutActions, LayoutFrame};

/// The window's view, made and synced with the scene's first frame, and the
/// log clock's now (the scene's gate is dated from it).
pub(crate) fn fixture_view(
    actions: &LayoutActions,
    frame: &LayoutFrame<'_>,
    window: &mut Window,
    cx: &mut App,
) -> (Entity<DenseView>, u64) {
    let v = view(actions, window, cx);
    v.update(cx, |v, cx| v.sync(frame, cx));
    let now = v.read(cx).clock.now_ms();
    (v, now)
}

/// fixture-1 in the details column, synced with the scene's live changes.
/// Not `select`: that reads the host's frame, i.e. the workspace the scene
/// is updating.
pub(crate) fn fixture_select(v: &Entity<DenseView>, frame: &LayoutFrame<'_>, cx: &mut App) {
    v.update(cx, |v, cx| {
        v.selected = Some(Sel::Terminal("fixture-1".into()));
        v.sync(frame, cx);
    });
}
