//! The rail's half of the visual-QA scenes `rail…` (`--ui-fixture
//! --fixture-scene rail…`). `fixture_scenes::rail` opens the fixture
//! terminals in the workspace, then hands over the host's actions; the rail
//! is reached through the kit's state slot.
use gpui_kit::*;

use super::RailState;
use super::model::Drawer;
use crate::layouts::kit::LayoutActions;

/// After the panels reached the rail: task-000 in front, laid out, and the
/// drawer the scene names open (else the content focused).
pub fn fixture_scene(scene: &str, actions: LayoutActions, window: &mut Window, cx: &mut App) {
    let scene = scene.to_owned();
    window.defer(cx, move |window, cx| {
        let Some(rail) = actions.state::<RailState>(cx) else {
            return;
        };
        rail.update(cx, |r, cx| {
            r.switch_to("sample/task-000", false, window, cx);
            r.rearrange(window, cx);
            match scene.as_str() {
                "rail-attention" => r.toggle_drawer(Drawer::Attention, window, cx),
                "rail-switcher" => r.toggle_drawer(Drawer::Tasks, window, cx),
                "rail-workspace" => r.toggle_drawer(Drawer::Workspace, window, cx),
                _ => r.focus_content(window, cx),
            }
        });
    });
}
