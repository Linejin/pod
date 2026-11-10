//! (b) Focus return after every modal, (c) Tab / Shift+Tab traversal, and
//! window activation that must not open dock panels.

use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

use super::fixtures::seed_two_tasks;
use super::harness::{Harness, jump_to, palette};

#[test]
fn dock_activation_does_not_open_inspector() {
    let mut h = Harness::new("dock-uia", seed_two_tasks);
    h.keys("ctrl-,");
    let ws = h.ws.clone();
    for _ in 0..3 {
        h.cx.deactivate_window();
        h.cx.update(|window, _| window.activate_window());
        h.cx.run_until_parked();
        // Do not use Harness::settle: its full refresh masks view-cache bugs.
        h.cx.update(|_, cx| ws.update(cx, |_, cx| cx.notify()));
        h.cx.run_until_parked();
        h.cx.update(|window, cx| {
            let _ = window.draw(cx);
            assert!(
                ws.read(cx).inspector_panel.is_none(),
                "activation opened 상태"
            );
        });
    }
}

// ---- (b) focus return ------------------------------------------------------

#[test]
fn b_escape_returns_focus_to_the_opener() {
    let mut h = Harness::new("b-focus-return", seed_two_tasks);
    jump_to(&mut h, "로그인 작업");
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));

    // Openers reachable by keyboard, each closed with Esc.
    // (name, shortcut, or palette query + extra keys)
    type Opener<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
    let openers: &[Opener] = &[
        ("palette (Ctrl+K)", "ctrl-k", None),
        ("new task (Ctrl+N)", "ctrl-n", None),
        ("new terminal (Ctrl+Shift+T)", "ctrl-shift-t", None),
        ("notifications (Ctrl+Shift+B)", "ctrl-shift-b", None),
        ("new comparison group (Ctrl+Shift+N)", "ctrl-shift-n", None),
        (
            "shortcut help (palette)",
            "",
            Some(("키보드 단축키 도움말", "")),
        ),
        ("open repository (palette)", "", Some(("저장소 열기", ""))),
        // Rows for "review-bot": terminal b1 (detail), then the task itself.
        ("task details (palette)", "", Some(("review-bot", "down"))),
    ];
    for (name, keys, query) in openers {
        match query {
            Some((q, extra)) => palette(&mut h, q, extra),
            None => h.keys(keys),
        }
        assert!(h.dialog_open(), "{name}: dialog open");
        let inside = h.focus_name();
        assert!(
            inside.starts_with("dialog"),
            "{name}: focus moved into the dialog, got {inside}"
        );
        h.keys("escape");
        assert!(!h.dialog_open(), "{name}: Esc closed it");
        assert_eq!(
            h.focused_terminal().as_deref(),
            Some("w1"),
            "{name}: focus returned to terminal w1, got {}",
            h.focus_name()
        );
    }
    // Typing after the last return reaches w1.
    h.backend.typed.clear();
    h.type_text("ok");
    assert_eq!(h.backend.typed_text("w1"), "ok");

    // From the sidebar tree as the opener.
    let sidebar = h.sidebar_focus();
    h.cx.update(|window, cx| sidebar.focus(window, cx));
    h.settle();
    h.log("sidebar focused");
    for keys in ["ctrl-n", "ctrl-k", "ctrl-shift-b"] {
        h.keys(keys);
        assert!(h.dialog_open());
        h.keys("escape");
        assert!(
            h.is_focused(&sidebar),
            "{keys} → Esc returns to the sidebar tree, got {}",
            h.focus_name()
        );
    }

    // Settings and the status inspector are dock tabs, not modals: Ctrl+, moves
    // focus into Settings; Ctrl+K → terminal is the keyboard way back.
    h.keys("ctrl-,");
    assert!(
        h.focus_name().starts_with("settings panel"),
        "Ctrl+, focuses Settings: {}",
        h.focus_name()
    );
    jump_to(&mut h, "로그인 작업");
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    h.keys("ctrl-shift-s");
    assert!(
        h.focus_name().starts_with("inspector panel"),
        "Ctrl+Shift+S focuses the inspector: {}",
        h.focus_name()
    );
}

// ---- (c) Tab / Shift+Tab traversal ----------------------------------------

/// Tab from the current focus until it cycles; Shift+Tab must retrace the
/// exact sequence in reverse. Every Tab moves focus; none loses it.
fn traverse(h: &mut Harness, label: &str, max: usize) -> Vec<String> {
    let (stops, mismatches) = traverse_report(h, label, max);
    assert!(
        mismatches.is_empty(),
        "{label}: Shift+Tab is not the reverse of Tab: {mismatches:?}"
    );
    stops
}

/// [`traverse`] that reports Shift+Tab mismatches instead of failing.
fn traverse_report(h: &mut Harness, label: &str, max: usize) -> (Vec<String>, Vec<String>) {
    let start = h.focused().expect("a focused start");
    let mut forward = Vec::new();
    for i in 0..max {
        h.cx.simulate_keystrokes("tab");
        h.settle();
        let f = h
            .focused()
            .unwrap_or_else(|| panic!("{label}: Tab {i} lost focus"));
        let name = h.focus_name();
        println!("[native-qa] {label} | tab {i} → {name}");
        if forward
            .last()
            .is_some_and(|(prev, _): &(FocusHandle, String)| *prev == f)
        {
            panic!("{label}: Tab {i} did not move focus ({name})");
        }
        if forward
            .first()
            .is_some_and(|(first, _): &(FocusHandle, String)| *first == f)
        {
            break;
        }
        forward.push((f, name));
    }
    assert!(
        forward.len() >= 3,
        "{label}: at least three tab stops, got {forward:?}",
        forward = forward.iter().map(|x| &x.1).collect::<Vec<_>>()
    );
    // Shift+Tab walks back: from stop[0] to stop[n-1], …, stop[1], stop[0].
    let n = forward.len();
    let mut mismatches = Vec::new();
    for i in 0..n {
        h.cx.simulate_keystrokes("shift-tab");
        h.settle();
        let expected = &forward[(2 * n - 1 - i) % n];
        let got = h.focused();
        if got.as_ref() != Some(&expected.0) {
            let m = format!(
                "Shift+Tab {i}: expected {}, got {}",
                expected.1,
                h.focus_name()
            );
            println!("[native-qa] {label} | MISMATCH {m}");
            mismatches.push(m);
            // Resynchronise on the expected stop and continue.
            let f = expected.0.clone();
            h.cx.update(|window, cx| f.focus(window, cx));
            h.settle();
        }
    }
    let _ = start;
    println!(
        "[native-qa] {label} | {n} tab stops, Shift+Tab mismatches: {}",
        mismatches.len()
    );
    (forward.into_iter().map(|x| x.1).collect(), mismatches)
}

#[test]
fn c_tab_order_cycles_and_reverses_on_main_screens() {
    let mut h = Harness::new("c-tab", seed_two_tasks);
    // Workspace without panels: title bar, sidebar, filter, status bar.
    let stops = traverse(&mut h, "workspace", 60);
    assert!(
        stops.iter().any(|s| s == "sidebar tree"),
        "sidebar tree is a tab stop: {stops:?}"
    );
    assert!(
        stops.iter().any(|s| s.starts_with("sidebar filter")),
        "sidebar filter is a tab stop: {stops:?}"
    );
    let tree = stops.iter().position(|s| s == "sidebar tree").unwrap();
    let filter = stops
        .iter()
        .position(|s| s.starts_with("sidebar filter"))
        .unwrap();
    assert!(
        tree < filter,
        "tree before its filter (visual order): {stops:?}"
    );

    // Settings open (dock tab with category/value choices).
    h.keys("ctrl-,");
    // 모양 (docs/10 §D5.3) adds ~50 choices (18 preset cards, styles, accents, layouts).
    let settings = traverse(&mut h, "settings", 200);
    assert!(
        settings
            .iter()
            .filter(|s| s.starts_with("settings panel"))
            .count()
            >= 3,
        "settings choices are tab stops: {settings:?}"
    );

    // Agents tree (Ctrl+Shift+E) is a tab stop with arrow-key navigation inside.
    h.keys("ctrl-shift-e");
    assert!(
        h.focus_name().starts_with("agents panel"),
        "Ctrl+Shift+E focuses the agent tree: {}",
        h.focus_name()
    );
    let agents = traverse(&mut h, "agents", 200);
    assert!(
        agents.iter().any(|s| s.starts_with("agents panel")),
        "agent tree is a tab stop: {agents:?}"
    );
    h.keys("ctrl-shift-i");
    assert!(
        h.focus_name().starts_with("gates panel"),
        "Ctrl+Shift+I focuses the approvals inbox: {}",
        h.focus_name()
    );

    // Inside a dialog Tab stays in the dialog (focus trap) and Esc still closes it.
    h.keys("ctrl-n");
    let dialog = traverse(&mut h, "new-task dialog", 40);
    assert!(
        dialog.iter().all(|s| s.starts_with("dialog")),
        "focus trapped in the dialog: {dialog:?}"
    );
    assert!(
        dialog
            .iter()
            .filter(|s| s.starts_with("dialog text input"))
            .count()
            >= 2,
        "name and base fields: {dialog:?}"
    );
    h.keys("escape");
    assert!(!h.dialog_open());
}

/// The dock's tab group tracks the active panel's focus handle on its frame,
/// before its tab bar. When that handle was also the agent tree's tab stop it
/// was registered twice: Tab from Settings skipped the agents header straight
/// into the tree (first copy) while Shift+Tab from the tree left from the
/// second copy to the header menu, so Shift+Tab did not retrace Tab.
#[test]
fn c_shift_tab_from_the_agents_tree_reverses_tab() {
    let mut h = Harness::new("c-agents-shift-tab", seed_two_tasks);
    h.keys("ctrl-,");
    h.keys("ctrl-shift-e");
    let tree = h.focused().expect("agents tree focused");
    assert!(
        h.focus_name().starts_with("agents panel"),
        "{}",
        h.focus_name()
    );
    // The stop visually before the tree is its own panel header's menu button.
    h.keys("shift-tab");
    assert_eq!(
        h.focus_name(),
        "agents panel control [Root > Popover]",
        "Shift+Tab from the tree goes to its header menu"
    );
    h.keys("tab");
    assert_eq!(
        h.focused().as_ref(),
        Some(&tree),
        "Tab from the header menu returns to the tree, got {}",
        h.focus_name()
    );
    // Walking back past the header into Settings and forward again retraces every stop.
    let mut back = vec![tree.clone()];
    for _ in 0..6 {
        h.keys("shift-tab");
        back.push(h.focused().expect("a stop"));
    }
    for i in (0..6).rev() {
        h.keys("tab");
        assert_eq!(
            h.focused().as_ref(),
            Some(&back[i]),
            "Tab retraces Shift+Tab step {i}, got {}",
            h.focus_name()
        );
    }
    assert_eq!(h.focused().as_ref(), Some(&tree));
}

#[test]
fn c_review_controls_are_reachable_by_keyboard() {
    let wt = std::env::temp_dir().join(format!("pod-native-qa-review-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&wt);
    let wt = pod_editor::fixture::create(&wt, true).expect("fixture repo");
    let mut h = Harness::new("c-review", |b| {
        seed_two_tasks(b);
        b.tasks[0].worktree = Some(wt.to_string_lossy().into_owned());
        b.worktrees.insert("sample/로그인-수정".into(), wt.clone());
    });
    jump_to(&mut h, "로그인 작업");
    h.keys("ctrl-shift-r");
    assert!(
        h.focus_name().starts_with("review panel"),
        "Ctrl+Shift+R focuses Review: {}",
        h.focus_name()
    );
    assert!(
        h.backend.calls.iter().any(|c| c.starts_with("task.diff")),
        "diff requested: {:?}",
        h.backend.calls
    );
    let stops = traverse(&mut h, "review", 200);
    assert!(
        stops
            .iter()
            .any(|s| s.starts_with("review panel text input")),
        "agent instructions box reachable: {stops:?}"
    );
    // Keyboard comment: Alt+PageDown (file) then Ctrl+Shift+C opens a draft on its first change.
    h.keys("alt-pagedown");
    h.keys("ctrl-shift-c");
    assert!(
        h.focus_name().starts_with("review panel text input"),
        "comment draft focused: {}",
        h.focus_name()
    );
}
