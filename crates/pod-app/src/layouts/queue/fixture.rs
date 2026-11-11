//! The queue's half of the visual-QA scene `queue` (`--ui-fixture
//! --fixture-scene queue`). `fixture_scenes::queue` seeds the gate and the
//! statuses in the workspace, then hands over the host's actions; the column
//! is the kit's state slot.
use gpui_kit::*;

use crate::layouts::kit::LayoutActions;

/// The window's column with the gate card selected and the list focused.
pub fn fixture_scene(actions: &LayoutActions, window: &mut Window, cx: &mut App) {
    let col = super::column(actions, window, cx);
    col.update(cx, |q, cx| {
        q.selected = Some("gate:g1".into());
        q.focus_list(window, cx);
    });
}
