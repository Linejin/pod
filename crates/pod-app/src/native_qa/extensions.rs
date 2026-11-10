//! (f) The agent hook install prompt, and (h) layout presets from mods chosen
//! in the palette (docs/16 §7.2).

use std::sync::Arc;

use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

use super::fixtures::seed_two_tasks;
use super::harness::{Harness, center, center_and_right, jump_to, palette};
use crate::source::TestBackend;
use crate::workspace::StartOptions;

#[test]
fn f_hook_prompt_installs_only_after_the_user_confirms() {
    use crate::hooks_prompt::{HookCliHandle, MockHookCli};
    let cli = Arc::new(MockHookCli::missing());
    let options = StartOptions {
        hook_cli: Some(HookCliHandle(cli.clone())),
        ..Default::default()
    };
    let mut h = Harness::with_options("f-hooks", options, seed_two_tasks);
    assert!(
        h.bounds("hooks-banner").is_some(),
        "banner shown for missing hooks"
    );
    assert!(
        cli.installs.lock().unwrap().is_empty(),
        "nothing runs at launch but status"
    );
    // The banner does not take focus (non-blocking).
    let root = h.root_focus();
    assert!(
        h.is_focused(&root),
        "launch focus stays on the workspace: {}",
        h.focus_name()
    );

    // 훅 설치: dry run, preview; 취소 installs nothing.
    let install = h.bounds("hooks-install").expect("install button");
    h.cx.simulate_click(center(install), Modifiers::none());
    h.settle();
    assert!(h.dialog_open(), "preview dialog");
    assert_eq!(
        *cli.installs.lock().unwrap(),
        vec![true],
        "only a dry run so far"
    );
    let cancel = h.bounds("hooks-cancel").expect("cancel");
    h.cx.simulate_click(center(cancel), Modifiers::none());
    h.settle();
    assert!(!h.dialog_open());
    assert_eq!(
        *cli.installs.lock().unwrap(),
        vec![true],
        "cancel: no install"
    );
    assert!(
        h.bounds("hooks-banner").is_some(),
        "banner stays until installed or dismissed"
    );

    // Keyboard path: palette command, preview, 설치.
    palette(&mut h, "에이전트 훅 설치", "");
    assert!(h.dialog_open());
    let confirm = h.bounds("hooks-confirm").expect("confirm");
    h.cx.simulate_click(center(confirm), Modifiers::none());
    h.settle();
    h.log("confirmed install");
    assert_eq!(
        *cli.installs.lock().unwrap(),
        vec![true, true, false],
        "the real install ran once, after confirmation"
    );
    assert!(
        h.bounds("hooks-banner").is_none(),
        "banner gone after install"
    );
    assert!(
        h.toast_titles()
            .iter()
            .any(|t| t == crate::strings::HOOKS_INSTALLED),
        "{:?}",
        h.toast_titles()
    );
}

#[test]
fn f_hook_prompt_later_is_remembered_per_build() {
    use crate::hooks_prompt::{HookCliHandle, MockHookCli};
    let cli = Arc::new(MockHookCli::missing());
    let options = StartOptions {
        hook_cli: Some(HookCliHandle(cli.clone())),
        ..Default::default()
    };
    let mut h = Harness::with_options("f-hooks-later", options, seed_two_tasks);
    let later = h.bounds("hooks-later").expect("later button");
    h.cx.simulate_click(center(later), Modifiers::none());
    h.settle();
    assert!(h.bounds("hooks-banner").is_none());
    let saved = h.cx.update(|_, cx| {
        cx.global::<crate::theme::Settings>()
            .hooks_dismissed
            .clone()
    });
    assert_eq!(saved, crate::hooks_prompt::dismiss_key("fixture-hook-1"));
    assert!(cli.installs.lock().unwrap().is_empty());
    // Same build again: no banner. A new pod-hook build asks again.
    let ws = h.ws.clone();
    let again: Arc<dyn crate::hooks_prompt::HookCli> = cli.clone();
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.start_hook_check(again, cx)));
    h.settle();
    assert!(
        h.bounds("hooks-banner").is_none(),
        "dismissed for this build"
    );
    let newer: Arc<dyn crate::hooks_prompt::HookCli> = Arc::new(MockHookCli {
        key: "fixture-hook-2".into(),
        ..MockHookCli::missing()
    });
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.start_hook_check(newer, cx)));
    h.settle();
    assert!(
        h.bounds("hooks-banner").is_some(),
        "a new pod-hook build asks again"
    );
}

// ---- (h) extensions: layout presets chosen from the palette (docs/16 §7.2) ----

/// The example `pod-layouts` presets as the daemon lists them, plus a mod
/// whose preset file failed validation (as `mod.list` reports it).
fn mods_backend(b: &mut TestBackend) {
    seed_two_tasks(b);
    b.terminals.push(TestBackend::terminal(
        "w2",
        "로그인 테스트",
        Some("sample/로그인-수정"),
        "idle",
    ));
    b.tasks[0].terminals.push("w2".into());
    let preset = |f: &str| {
        let text = std::fs::read_to_string(format!(
            "{}/../../examples/mods/pod-layouts/layouts/{f}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("example preset");
        serde_json::to_value(podd::protocol::LayoutPreset::parse(&text).expect("valid preset"))
            .expect("json")
    };
    let list = serde_json::json!({
        "enabled": true, "root": "C:/fixtures/mods", "dropped_events": 0,
        "mods": [
            {"name": "pod-layouts", "version": "0.1.0", "title": "기본 레이아웃 프리셋", "enabled": true, "state": "running",
             "permissions": [], "layouts": [preset("review-focus.json"), preset("agents-quad.json")], "log_lines": 1, "memory_kib": 0},
            {"name": "broken-layout", "version": "0.1.0", "enabled": true, "state": "invalid",
             "error": "layout layouts/bad.json: a split needs 2-6 children", "permissions": [], "log_lines": 0, "memory_kib": 0}
        ]
    });
    b.rpc = Some(Box::new(move |method, _| {
        (method == "mod.list").then(|| Ok(list.clone()))
    }));
}

#[test]
fn h_layout_preset_from_the_palette_rearranges_and_survives_restart() {
    let dir = tempfile::tempdir().expect("config dir");
    crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
    {
        let mut h = Harness::new("h-layout-preset", mods_backend);
        jump_to(&mut h, "로그인 테스트");
        jump_to(&mut h, "로그인 작업");
        assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
        jump_to(&mut h, "리뷰 집중");
        assert_eq!(
            h.focused_terminal().as_deref(),
            Some("w1"),
            "focus stays in the user's terminal"
        );
        let (center, right) = center_and_right(&mut h);
        h.log(&format!("center after 리뷰 집중: {center}"));
        // Review left; the task's first terminal right; its second (not in
        // the preset) joins the terminal slot as a tab; gates go to the right dock.
        assert_eq!(
            center, "h[tabs(review:sample/로그인-수정),tabs(terminal:w1,terminal:w2)]",
            "preset 리뷰 집중 as declared"
        );
        assert!(
            right.contains(crate::gates::GATES_PANEL_NAME),
            "gates in the right dock: {right}"
        );
        // (The scripted backend serves no review comments; that toast is not about layouts.)
        let layout_errors: Vec<String> = h
            .toast_titles()
            .into_iter()
            .filter(|t| t.starts_with("레이아웃") || t.starts_with("적용할"))
            .collect();
        assert!(
            layout_errors.is_empty(),
            "no layout error: {layout_errors:?}"
        );

        jump_to(&mut h, "4분할 에이전트");
        let (center, _) = center_and_right(&mut h);
        h.log(&format!("center after 4분할: {center}"));
        assert_eq!(
            center, "h[tabs(terminal:w1,review:sample/로그인-수정),tabs(terminal:w2)]",
            "two terminals: one row; the review panel stays (as a tab), not closed"
        );

        jump_to(&mut h, "리뷰 집중");
        // The real quit path persists the layout (main.rs on_app_quit).
        let ws = h.ws.clone();
        h.cx.update(|_, cx| ws.update(cx, |w, cx| w.on_quit(cx)))
            .detach();
        h.settle();
    }
    assert!(
        dir.path().join("layout.json").is_file(),
        "layout saved on quit"
    );
    {
        let mut h = Harness::new("h-layout-restart", mods_backend);
        let (center, right) = center_and_right(&mut h);
        h.log(&format!("center after restart: {center}"));
        assert_eq!(
            center, "h[tabs(review:sample/로그인-수정),tabs(terminal:w1,terminal:w2)]",
            "restart restores the preset layout"
        );
        assert!(right.contains(crate::gates::GATES_PANEL_NAME), "{right}");
    }
    crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = None);
}

#[test]
fn h_missing_and_invalid_presets_explain_themselves_in_korean() {
    let dir = tempfile::tempdir().expect("config dir");
    crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
    let mut h = Harness::new("h-layout-errors", mods_backend);
    // A palette entry whose preset vanished (mod turned off meanwhile).
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            assert!(crate::mods_ui::run(
                w,
                "layout:pod-layouts:gone",
                window,
                cx
            ))
        })
    });
    h.settle();
    let titles = h.toast_titles();
    assert!(
        titles
            .iter()
            .any(|t| t == crate::mods_ui::s::LAYOUT_MISSING),
        "{titles:?}"
    );
    // A mod whose preset file is invalid: Settings → 확장 says so in Korean.
    let line = h.cx.update(|_, cx| {
        let list = cx
            .global::<crate::mods_ui::ModsUi>()
            .list
            .clone()
            .expect("mod.list answered");
        let m = list
            .list
            .mods
            .iter()
            .find(|m| m.name == "broken-layout")
            .cloned()
            .expect("broken mod listed");
        crate::mods_ui::error_summary(&m).expect("error line")
    });
    h.log(&format!("invalid preset card: {line}"));
    assert!(
        line.starts_with("레이아웃 프리셋 오류: layout layouts/bad.json"),
        "{line}"
    );
    // Its presets never reach the palette.
    let entries = h.cx.update(|_, cx| crate::mods_ui::palette_entries(cx));
    assert!(
        entries
            .iter()
            .all(|(id, _, _)| !id.starts_with("layout:broken-layout")),
        "{entries:?}"
    );
    assert_eq!(
        entries
            .iter()
            .filter(|(id, _, _)| id.starts_with("layout:pod-layouts:"))
            .count(),
        2
    );
    crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = None);
}
