use super::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

#[test]
fn category_keys_move_and_stop_at_ends() {
    assert_eq!(category_step(0, 3, "down"), Some(1));
    assert_eq!(category_step(2, 3, "down"), Some(2));
    assert_eq!(category_step(0, 3, "up"), Some(0));
    assert_eq!(category_step(1, 3, "end"), Some(2));
    assert_eq!(category_step(2, 3, "home"), Some(0));
    assert_eq!(category_step(1, 3, "left"), None);
    assert_eq!(category_step(0, 0, "down"), None);
}

#[test]
fn category_is_a_selectable_tab() {
    let a = category_a11y("모양", true);
    assert_eq!(a.role, Role::Tab);
    assert_eq!(a.selected, Some(true));
    assert_eq!(
        category_a11y("글꼴", false).selected,
        Some(false),
        "unselected tabs still expose IsSelected=false"
    );
    assert_eq!(a.toggled, None);
}

#[test]
fn select_closed_keys_change_value_or_open() {
    use SelectEvent::*;
    assert_eq!(select_key("down", false, None, Some(1), 4), Some(Pick(2)));
    assert_eq!(select_key("down", false, None, Some(3), 4), Some(Pick(3)));
    assert_eq!(select_key("up", false, None, Some(0), 4), Some(Pick(0)));
    assert_eq!(
        select_key("down", false, None, None, 4),
        Some(Pick(0)),
        "custom value: first option"
    );
    assert_eq!(select_key("down", true, None, Some(2), 4), Some(Open(2)));
    assert_eq!(select_key("enter", false, None, Some(2), 4), Some(Open(2)));
    assert_eq!(select_key("space", false, None, None, 4), Some(Open(0)));
    assert_eq!(select_key("end", false, None, Some(0), 4), Some(Pick(3)));
    assert_eq!(
        select_key("escape", false, None, Some(0), 4),
        None,
        "Esc on a closed select is not ours"
    );
    assert_eq!(select_key("down", false, None, Some(0), 0), None);
}

#[test]
fn select_open_keys_move_pick_close() {
    use SelectEvent::*;
    assert_eq!(
        select_key("down", false, Some(1), Some(1), 4),
        Some(Move(2))
    );
    assert_eq!(select_key("up", false, Some(0), Some(1), 4), Some(Move(0)));
    assert_eq!(select_key("end", false, Some(0), Some(1), 4), Some(Move(3)));
    assert_eq!(
        select_key("enter", false, Some(3), Some(1), 4),
        Some(Pick(3))
    );
    assert_eq!(select_key("up", true, Some(2), Some(1), 4), Some(Pick(2)));
    assert_eq!(
        select_key("escape", false, Some(2), Some(1), 4),
        Some(Close)
    );
    assert_eq!(select_key("tab", false, Some(2), Some(1), 4), Some(Close));
}

#[test]
fn select_keyboard_click_does_not_reopen_after_enter() {
    use SelectEvent::*;
    // Enter key-down picks (and closes); the key-up click GPUI synthesizes must do nothing.
    assert_eq!(
        select_key("enter", false, Some(1), Some(0), 3),
        Some(Pick(1))
    );
    assert_eq!(select_click(true, true, None, Some(1)), None);
    assert_eq!(select_click(true, true, Some(1), Some(1)), None);
    assert_eq!(select_click(true, false, None, Some(1)), Some(Open(1)));
    assert_eq!(select_click(true, false, Some(2), Some(1)), Some(Close));
    assert_eq!(select_click(false, false, None, Some(1)), None);
}
#[test]
fn select_type_ahead_cycles_matches_case_insensitively() {
    let o: Vec<SharedString> = ["Dark", "dim", "Light", "dusk"]
        .into_iter()
        .map(Into::into)
        .collect();
    assert_eq!(select_type_ahead("d", &o, None), Some(0));
    assert_eq!(select_type_ahead("D", &o, Some(0)), Some(1));
    assert_eq!(select_type_ahead("d", &o, Some(3)), Some(0), "wraps");
    assert_eq!(select_type_ahead("l", &o, Some(2)), Some(2), "only match");
    assert_eq!(select_type_ahead("x", &o, None), None);
    assert_eq!(select_type_ahead("\t", &o, None), None);
    assert_eq!(select_type_ahead("dd", &o, None), None);
    assert_eq!(select_type_ahead("d", &[], None), None);
}

#[test]
fn select_exposes_expand_collapse_value_and_selection() {
    let closed = select_a11y("UI 글꼴", "Segoe UI", false);
    assert_eq!(closed.role, Role::ComboBox);
    assert_eq!(closed.expanded, Some(false));
    assert_eq!(closed.value.as_deref(), Some("Segoe UI"));
    assert_eq!(
        select_a11y("UI 글꼴", "Segoe UI", true).expanded,
        Some(true)
    );
    let o = option_a11y("Consolas", true);
    assert_eq!((o.role, o.selected), (Role::ListBoxOption, Some(true)));
    assert_eq!(option_a11y("D2Coding", false).selected, Some(false));
}

#[test]
fn switch_is_a_toggle_with_korean_state() {
    let on = switch_a11y("고대비 미리보기", true);
    assert_eq!(on.role, Role::Switch);
    assert_eq!(on.toggled, Some(Toggled::True));
    assert_eq!(on.selected, None, "a switch is Toggle, not SelectionItem");
    assert_eq!(
        switch_a11y("Windows 알림", false).toggled,
        Some(Toggled::False)
    );
    assert_eq!(
        (switch_state_text(true), switch_state_text(false)),
        ("켬", "끔")
    );
}
