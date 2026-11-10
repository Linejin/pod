//! (e) DPI: chrome rows and dialogs stay inside the window at each Windows scale.

use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

use super::fixtures::seed_two_tasks;
use super::harness::{Harness, within};

/// The logical viewport a maximized window gets on a 1920×1080 monitor at
/// each Windows scale (the smallest common desktop; worst case for overflow).
const SCALES: [f32; 4] = [1.0, 1.25, 1.5, 1.75];

#[test]
fn e_dpi_scales_keep_chrome_rows_dialogs_inside_the_window() {
    for scale in SCALES {
        let logical = size(
            px((1920. / scale).floor()),
            px((1080. / scale).floor() - 48. / scale),
        );
        let mut h = Harness::sized("e-dpi", scale, logical, seed_two_tasks);
        let scale_seen = h.cx.update(|window, _| window.scale_factor());
        assert_eq!(scale_seen, scale);
        let view = Bounds::new(point(px(0.), px(0.)), h.viewport());
        println!("[native-qa] e-dpi | scale {scale} viewport {:?}", view.size);
        let check = |h: &mut Harness, sel: &str| -> Bounds<Pixels> {
            let b = h
                .bounds(sel)
                .unwrap_or_else(|| panic!("scale {scale}: {sel} not painted"));
            println!("[native-qa] e-dpi | scale {scale} {sel} {b:?}");
            assert!(
                within(b, view),
                "scale {scale}: {sel} {b:?} outside viewport {view:?}"
            );
            b
        };
        let title = check(&mut h, "title-actions");
        assert!(
            title.size.height <= px(36.),
            "title actions fit the 36px title bar"
        );
        let sidebar = check(&mut h, "sidebar");
        let actions = check(&mut h, "sidebar-actions");
        assert!(
            within(actions, sidebar),
            "scale {scale}: sidebar buttons {actions:?} clipped by sidebar {sidebar:?}"
        );
        for row in ["row-w1", "row-b1"] {
            let r = check(&mut h, row);
            assert!(
                r.right() <= sidebar.right() + px(0.5),
                "scale {scale}: {row} wider than the sidebar"
            );
            // Rows keep the active style's row height (docs/10 §D5: 28 in the default 04 조용한 편집기).
            let row_h = pod_ui::themes::metrics().density.row;
            assert!(
                r.size.height >= px(row_h - 0.5),
                "scale {scale}: {row} keeps its {row_h}px height"
            );
        }
        let bar = check(&mut h, "status-bar");
        assert!(
            (bar.bottom() - view.bottom()).abs() < px(1.),
            "status bar at the bottom edge"
        );
        for item in ["status-inspect", "status-notifications"] {
            let b = check(&mut h, item);
            assert!(
                within(b, bar),
                "scale {scale}: {item} {b:?} clipped by status bar {bar:?}"
            );
        }
        for (name, keys) in [
            ("palette", "ctrl-k"),
            ("new task", "ctrl-n"),
            ("new terminal", "ctrl-shift-t"),
            ("notifications", "ctrl-shift-b"),
            ("new group", "ctrl-shift-n"),
        ] {
            h.keys(keys);
            let d = check(&mut h, "dialog-0");
            println!("[native-qa] e-dpi | scale {scale} dialog {name} {d:?}");
            h.keys("escape");
        }
    }
}
