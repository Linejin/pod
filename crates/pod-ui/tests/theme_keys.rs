//! `pod_ui::apply_theme` binds pod-ui's three modal keys (Tab, Shift+Tab,
//! Escape in `PodModal`) once, however often the theme is applied. Regression
//! (fx-app): every settings apply bound them again, so the keymap grew by
//! three bindings each time.

use ::gpui::TestAppContext;
use gpui_kit::*;
use pod_ui::Mode;
// GPUI's glob exports its own `test` attribute; this is a plain Rust test.
use core::prelude::v1::test;

fn modal_keys(cx: &App) -> Vec<String> {
    cx.key_bindings()
        .borrow()
        .bindings()
        .filter(|b| b.action().name().starts_with("pod_modal::"))
        .map(|b| b.action().name().to_string())
        .collect()
}

fn keymap_len(cx: &App) -> usize {
    cx.key_bindings().borrow().bindings().count()
}

#[test]
fn applying_the_theme_again_keeps_the_keymap() {
    let app = TestAppContext::single();
    app.update(|cx| {
        gpui_kit::init(cx);
        pod_ui::apply_theme(Mode::Dark, true, cx);
    });
    let (first, len) = app.update(|cx| (modal_keys(cx), keymap_len(cx)));
    assert_eq!(
        first,
        [
            "pod_modal::ModalNext",
            "pod_modal::ModalPrevious",
            "pod_modal::ModalClose"
        ]
    );
    for (mode, reduced) in [
        (Mode::Light, false),
        (Mode::Dark, true),
        (Mode::Dark, false),
    ] {
        app.update(|cx| pod_ui::apply_theme(mode, reduced, cx));
    }
    app.update(pod_ui::install_key_bindings);
    assert_eq!(app.update(|cx| modal_keys(cx)), first, "bound once");
    assert_eq!(
        app.update(|cx| keymap_len(cx)),
        len,
        "the keymap does not grow"
    );
}
