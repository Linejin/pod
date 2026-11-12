//! Pure chrome tests: the palette's matching and theme commands, the static
//! command table and its keys, settings options, and the notification times.
use core::prelude::v1::test;

use super::commands::COMMANDS;
use super::copy::SHORTCUTS;
use super::notifications::ago;
use super::palette::fuzzy_positions;
use super::settings::{
    CATEGORIES, TERMINAL_FONTS, TERMINAL_SIZES, UI_FONTS, UI_SIZES, select_options, theme_command,
    theme_palette_entries,
};
use crate::theme::Settings;

// ---- palette ----------------------------------------------------------

#[test]
fn fuzzy_unicode_ranges_and_order() {
    assert_eq!(
        fuzzy_positions(crate::strings::FUZZY_SAMPLE, crate::strings::FUZZY_QUERY),
        Some(vec![0..3, 4..7])
    );
    assert!(fuzzy_positions("Terminals", "tml").is_some());
    assert!(fuzzy_positions("abc", "ca").is_none());
    assert_eq!(fuzzy_positions("abc", ""), Some(vec![]));
}

#[test]
fn theme_palette_commands_map_to_settings() {
    let base = crate::theme::Settings::default();
    let entries = theme_palette_entries();
    assert!(
        entries
            .iter()
            .any(|(id, title, _)| id == "theme-preset:08" && title == "테마: 08 섬형")
    );
    for (id, _, _) in &entries {
        assert!(theme_command(id, &base).is_some(), "{id} runs");
    }
    let s = theme_command("theme-preset:14", &base).unwrap();
    assert_eq!(
        (s.style.as_str(), s.layout.as_str()),
        ("access", "workbench")
    );
    assert_eq!(
        theme_command("theme-mode:light", &base).unwrap().theme,
        "light"
    );
    assert!(theme_command("theme-preset:99", &base).is_none());
    assert!(theme_command("theme-style:nope", &base).is_none());
    assert!(theme_command("new-task", &base).is_none());
}

#[test]
fn documented_shortcuts_match_in_app_help() {
    let design = include_str!("../../../../docs/10-ui-design.md");
    for (keys, label) in SHORTCUTS {
        assert!(
            design.contains(&format!("| {keys} | {label} |")),
            "missing shortcut {keys}"
        );
    }
}

/// The static palette commands, in palette order, with the text they had
/// before the command table (A-P1): ids are saved in recents and named by
/// fixture scenes, so they never change.
#[test]
fn static_palette_ids_and_text_are_fixed() {
    let ids: Vec<_> = COMMANDS.iter().map(|c| c.id).collect();
    assert_eq!(
        ids,
        [
            "shortcuts",
            "new-task",
            "quick-open",
            "files",
            "save-all",
            "open-repo",
            "terminal",
            "settings",
            "machines",
            "extensions",
            "sessions",
            "blocked",
            "inspect",
            "notifications",
            "ctrl-k",
            "ctrl-p",
            "terminal-links",
            "install-hooks",
            "grid",
            "new-group",
            "gates",
            "agents",
        ]
    );
    let text = |id: &str| {
        let c = COMMANDS.iter().find(|c| c.id == id).unwrap();
        (c.label, c.detail)
    };
    assert_eq!(
        text("shortcuts"),
        ("키보드 단축키 도움말", "단축키 · Keyboard shortcuts")
    );
    assert_eq!(text("new-task"), (crate::strings::NEW_TASK, "Ctrl+N"));
    assert_eq!(
        text("terminal"),
        (crate::strings::NEW_TERMINAL, "Ctrl+Shift+T")
    );
    assert_eq!(text("settings"), (crate::strings::SETTINGS, "Ctrl+,"));
    // rm-r4ui: remote machines (settings category), no key of its own.
    assert_eq!(
        text("machines"),
        ("원격 머신", "SSH 머신 프로필 추가·편집, 연결 확인")
    );
    assert_eq!(
        text("grid"),
        (crate::strings::ARRANGE_PANELS_IN_A_GRID, "Ctrl+Shift+G")
    );
    assert_eq!(
        text("inspect"),
        (
            crate::strings::INSPECT_CURRENT_TERMINAL_STATUS,
            crate::strings::SOURCE_AND_FRESHNESS
        )
    );
    // `>` in the palette keeps ids without ':' (commands); unique ids route one way.
    for (i, c) in COMMANDS.iter().enumerate() {
        assert!(!c.id.contains(':'), "{}", c.id);
        assert!(
            COMMANDS[..i].iter().all(|o| o.id != c.id),
            "duplicate {}",
            c.id
        );
    }
}

/// The keys the chrome binds: the same keystrokes, actions and (global)
/// context as before the table, each on the command it opens.
#[test]
fn chrome_keys_bind_the_same_actions_to_their_commands() {
    let bound: Vec<_> = COMMANDS
        .iter()
        .filter_map(|c| c.shortcut.map(|s| (c.id, s)))
        .map(|(id, s)| {
            let binding = (s.bind)(s.keys, s.context);
            assert!(binding.predicate().is_none(), "{id}: no key context");
            (id, s.keys, s.context, binding.action().name())
        })
        .collect();
    assert_eq!(
        bound,
        [
            ("new-task", "ctrl-n", None, "pod_chrome::NewTask"),
            ("settings", "ctrl-,", None, "pod_chrome::SettingsAction"),
            ("inspect", "ctrl-shift-s", None, "pod_chrome::InspectStatus"),
            (
                "notifications",
                "ctrl-shift-b",
                None,
                "pod_chrome::Notifications"
            ),
        ]
    );
}

/// `ctrl-shift-s` → `Ctrl+Shift+S`, the way the help and the palette write keys.
fn key_label(keys: &str) -> String {
    keys.split('-')
        .map(|k| {
            let mut c = k.chars();
            match c.next() {
                Some(first) => first.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

/// A bound key is in the shortcut help, and a palette detail that names a
/// key names the bound one.
#[test]
fn bound_keys_are_in_the_help_and_the_palette_detail() {
    for c in COMMANDS {
        let Some(s) = c.shortcut else { continue };
        let label = key_label(s.keys);
        assert!(
            SHORTCUTS.iter().any(|(keys, _)| *keys == label),
            "{}: {label} in the help",
            c.id
        );
        if c.detail.starts_with("Ctrl") {
            assert_eq!(c.detail, label, "{}", c.id);
        }
    }
}

// ---- notifications ------------------------------------------------------

#[test]
fn notification_times_read_just_now_minutes_hours_and_days() {
    let now = 10 * 86_400_000;
    assert_eq!(ago(now, now), crate::strings::JUST_NOW);
    assert_eq!(ago(now, now - 59_999), crate::strings::JUST_NOW);
    assert_eq!(ago(now, now - 60_000), "1분 전");
    assert_eq!(ago(now, now - 3_599_999), "59분 전");
    assert_eq!(ago(now, now - 3_600_000), "1시간 전");
    assert_eq!(ago(now, now - 86_400_000), "1일 전");
    assert_eq!(
        ago(now, now + 5_000),
        crate::strings::JUST_NOW,
        "clock skew"
    );
}

// ---- settings -----------------------------------------------------------

#[test]
fn settings_categories_are_korean_and_ordered() {
    let labels: Vec<_> = CATEGORIES.iter().map(|(l, _)| *l).collect();
    assert_eq!(labels, ["모양", "글꼴", "일반", "원격 머신"]);
    assert_eq!(
        CATEGORIES[crate::machines_ui::SETTINGS_CATEGORY].0,
        crate::machines_ui::CATEGORY
    );
    assert_eq!(crate::strings::SETTINGS_CATEGORIES, "설정 분류");
}

#[test]
fn select_keeps_a_custom_value_as_its_own_option() {
    assert_eq!(
        select_options(UI_FONTS, "Consolas"),
        (
            vec!["Segoe UI".into(), "Malgun Gothic".into(), "Consolas".into()],
            2
        )
    );
    let (opts, at) = select_options(UI_FONTS, "Pretendard");
    assert_eq!((opts.len(), at, opts[at].as_str()), (4, 3, "Pretendard"));
}

#[test]
fn default_settings_match_a_listed_option() {
    let d = Settings::default();
    assert_eq!(
        select_options(UI_FONTS, &d.font).0.len(),
        UI_FONTS.len(),
        "default UI font is listed"
    );
    assert_eq!(
        select_options(TERMINAL_FONTS, &d.terminal_font).0.len(),
        TERMINAL_FONTS.len()
    );
    // Sizes are written without a trailing `.0` so 13.0 selects "13".
    assert_eq!(
        select_options(UI_SIZES, &d.font_size.to_string()).0.len(),
        UI_SIZES.len()
    );
    assert_eq!(
        select_options(TERMINAL_SIZES, &d.terminal_font_size.to_string())
            .0
            .len(),
        TERMINAL_SIZES.len()
    );
}

#[test]
fn boolean_settings_have_korean_switch_labels() {
    for l in [
        crate::strings::REDUCE_MOTION,
        crate::strings::HIGH_CONTRAST_PREVIEW,
        crate::strings::OS_NOTIFICATIONS,
    ] {
        assert!(l.chars().any(|c| ('가'..='힣').contains(&c)), "{l}");
    }
}
