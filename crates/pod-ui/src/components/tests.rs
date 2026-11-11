use super::buttons::button_text;
use super::*;
use crate::Tone;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;
#[test]
fn keep_all_breaks_only_between_words() {
    assert_eq!(
        keep_all_words("대기열에서 카드를 고르면  그 터미널이 여기에 열립니다"),
        [
            "대기열에서",
            "카드를",
            "고르면",
            "그",
            "터미널이",
            "여기에",
            "열립니다"
        ]
    );
    assert!(keep_all_words("   ").is_empty());
}
#[test]
fn banner_wraps_between_words_only() {
    assert_eq!(
        keep_all_words("작업 폴더가 없습니다"),
        ["작업", "폴더가", "없습니다"]
    );
    assert!(
        keep_all_words("작업 폴더가 없습니다")
            .into_iter()
            .all(keep_all_unbreakable)
    );
    assert!(
        keep_all_words("Review failed: worktree missing")
            .into_iter()
            .all(keep_all_unbreakable)
    );
    // A long path may break so the banner never overflows.
    assert!(!keep_all_unbreakable(
        "C:/Users/me/AppData/Roaming/Pod/themes/broken.json"
    ));
}
#[test]
fn busy_button_shows_korean_not_english() {
    assert_eq!(
        button_text("승인".into(), &ControlState::Loading).as_ref(),
        "처리 중…"
    );
    assert_eq!(
        button_text("승인".into(), &ControlState::Ready).as_ref(),
        "승인"
    );
}
#[test]
fn icons_parse_and_follow_the_style_stroke() {
    for i in Icon::ALL {
        let svg = std::str::from_utf8(i.source()).unwrap();
        assert!(
            svg.contains("<svg") && svg.contains("stroke-width=\"2\""),
            "{}",
            i.name()
        );
        assert!(
            !svg.contains("<script") && !svg.contains("href"),
            "{}",
            i.name()
        );
    }
    let thin = std::str::from_utf8(icons::stroked_for_test(Icon::X, 1.5)).unwrap();
    assert!(thin.contains("stroke-width=\"1.5\""));
    assert!(
        std::ptr::eq(
            icons::stroked_for_test(Icon::X, 1.5),
            icons::stroked_for_test(Icon::X, 1.5)
        ),
        "cached"
    );
    assert!(
        std::ptr::eq(icons::stroked_for_test(Icon::X, 2.), Icon::X.source()),
        "2px is the source"
    );
}
#[test]
fn nested_focus_round_trip_and_keyboard_wrap() {
    let mut stack = FocusStack::default();
    stack.enter("terminal");
    stack.enter("palette-query");
    assert_eq!(stack.leave(), Some("palette-query"));
    assert_eq!(stack.leave(), Some("terminal"));
    assert_eq!(stack.leave(), None);
    assert_eq!(next_focus(Some(2), 3, false), 0);
    assert_eq!(next_focus(Some(0), 3, true), 2);
    assert_eq!(next_focus(None, 3, false), 0);
    assert_eq!(next_focus(None, 3, true), 2);
}
#[test]
fn toast_pause_and_persistent_attention() {
    assert!(toast_timeout(Tone::Warning, false, false).is_none());
    assert!(toast_timeout(Tone::Danger, false, false).is_none());
    assert!(toast_timeout(Tone::Success, true, false).is_none());
    assert!(toast_timeout(Tone::Info, false, true).is_none());
    assert_eq!(
        toast_timeout(Tone::Success, false, false)
            .unwrap()
            .as_secs(),
        5
    );
}
#[test]
fn list_selection_survives_refresh_and_ime_never_executes() {
    let mut list = ListSelection::default();
    list.replace(vec!["a".into(), "b".into()]);
    list.navigate(false);
    assert_eq!(list.selected.as_deref(), Some("b"));
    list.replace(vec!["b".into(), "c".into()]);
    assert_eq!(list.selected.as_deref(), Some("b"));
    assert_eq!(list.activation(true, &ControlState::Ready), None);
    assert_eq!(list.activation(false, &ControlState::Loading), None);
    assert_eq!(list.activation(false, &ControlState::Ready), Some("b"));
}
