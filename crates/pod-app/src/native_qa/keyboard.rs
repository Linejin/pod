//! (a) Keyboard-only flows: repo → task → terminal → blocked → done,
//! shortcuts at launch and after the focused panel closes, Enter in dialogs,
//! the status-bar branch of the focused task, and Ctrl+P/Ctrl+K sent from
//! the palette to the current terminal.

use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

use super::fixtures::seed_two_tasks;
use super::harness::{Harness, jump_to, palette};
use crate::source::TestBackend;

#[test]
fn status_branch_tracks_task_and_hides_before_totals() {
    let full = "pod/로그인-수정/very-long-worktree-branch-name";
    let worktree = tempfile::tempdir().unwrap();
    let mut h = Harness::new("status-branch", |b| {
        seed_two_tasks(b);
        b.tasks[0].branch = Some(full.into());
        b.tasks[0].worktree = Some(worktree.path().to_string_lossy().into_owned());
        b.tasks[1].branch = Some("pod/review-bot".into());
    });
    assert!(h.bounds("status-branch").is_none(), "no focused task yet");
    jump_to(&mut h, "로그인 작업");
    assert!(h.bounds("status-branch").is_some());
    let ws = h.ws.clone();
    h.cx.update(|w, cx| {
        assert_eq!(
            ws.read(cx).status_branch(w, cx),
            Some(full),
            "full name is retained"
        );
    });
    jump_to(&mut h, "리뷰 봇");
    h.cx.update(|w, cx| {
        assert_eq!(ws.read(cx).status_branch(w, cx), Some("pod/review-bot"));
    });
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor("sample/로그인-수정", None, false, window, cx);
        })
    });
    h.settle();
    h.cx.update(|window, cx| {
        assert_eq!(
            ws.read(cx).status_branch(window, cx),
            Some(full),
            "focused editor wins over last terminal"
        );
    });
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("b1", false, window, cx)));
    h.settle();
    h.cx.simulate_resize(size(px(1399.), px(960.)));
    h.settle();
    assert!(h.bounds("status-branch").is_none());
    let plan = crate::workspace::StatusPlan::for_width(1399., true, true);
    assert!(!plan.branch && plan.terminals && plan.tasks && plan.done && plan.working);
    h.cx.simulate_resize(size(px(1440.), px(960.)));
    h.settle();
    assert!(h.bounds("status-branch").is_some());
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.update(cx, |w, cx| {
            w.tasks
                .iter_mut()
                .find(|t| t.id == "sample/review-bot")
                .unwrap()
                .branch = None;
            cx.notify();
        })
    });
    h.settle();
    assert!(
        h.bounds("status-branch").is_none(),
        "missing branch stays hidden"
    );
}

#[test]
fn a_keyboard_only_repo_task_terminal_blocked_work_done() {
    let mut h = Harness::new("a-flow", |b| {
        // A blocked terminal already exists (from an earlier session); no repository is registered.
        b.tasks.push(TestBackend::task(
            "other/review-bot",
            "review-bot",
            "other",
            &["b1"],
        ));
        b.terminals.push(TestBackend::terminal(
            "b1",
            "리뷰 봇",
            Some("other/review-bot"),
            "blocked",
        ));
    });
    h.log("launch");
    // Launch: the workspace root holds focus, so shortcuts work without a click.
    let root = h.root_focus();
    assert!(h.is_focused(&root), "launch focus: {}", h.focus_name());

    // 1. Open repository: Ctrl+K → "저장소 열기" → Enter → path → Enter.
    palette(&mut h, "저장소 열기", "");
    assert!(h.dialog_open(), "repository dialog open");
    assert!(
        h.focus_name().starts_with("dialog text input"),
        "path field focused: {}",
        h.focus_name()
    );
    h.type_text("C:/fixtures/sample");
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c == "repo.add C:/fixtures/sample"),
        "Enter submitted the path: {:?}",
        h.backend.calls
    );
    assert!(!h.dialog_open(), "repository dialog closed after success");
    let after_repo = h.focus_name();
    assert!(
        h.is_focused(&root),
        "focus back on the workspace after the repository dialog: {after_repo}"
    );

    // 2. New task: Ctrl+N → name → Enter.
    h.keys("ctrl-n");
    assert!(h.dialog_open(), "new task dialog open");
    assert!(
        h.focus_name().starts_with("dialog text input"),
        "name field focused: {}",
        h.focus_name()
    );
    h.type_text("login-fix");
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("task.create sample/login-fix Shell")),
        "Enter created a shell task: {:?}",
        h.backend.calls
    );
    assert!(!h.dialog_open(), "dialog closed after creation");
    let term = h
        .backend
        .tasks
        .iter()
        .find(|t| t.id == "sample/login-fix")
        .map(|t| t.terminals[0].clone())
        .expect("task terminal");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some(term.as_str()),
        "the new task's terminal has focus"
    );

    // 3. Typing goes to that terminal.
    h.type_text("echo hi");
    h.keys("enter");
    assert_eq!(h.backend.typed_text(&term), "echo hi\r");

    // 4. Find the blocked request: Ctrl+Shift+B notifications → Tab → Enter.
    h.keys("ctrl-shift-b");
    assert!(h.dialog_open(), "notifications open");
    // Dialog tab order: the blocked notice, then the dialog's Close button.
    h.press("tab");
    let on_notice = h.focus_name();
    // Enter (not only Space) activates the focused notice button.
    h.press("enter");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "Enter on the blocked notice opened b1 (Tab landed on: {on_notice})"
    );
    assert!(!h.dialog_open());

    // 4b. The palette command reaches it too.
    jump_to(&mut h, "login-fix");
    assert_eq!(h.focused_terminal().as_deref(), Some(term.as_str()));
    jump_to(&mut h, "응답할 요청 찾기");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "palette → next blocked request"
    );

    // 5. Back to work.
    jump_to(&mut h, "login-fix");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some(term.as_str()),
        "back in the task terminal"
    );

    // 6. Done confirmation: b1 finishes while we work elsewhere → toast; jumping
    //    to it acknowledges (terminal.ack, done → idle).
    h.backend.acks.clear();
    h.backend.set_status("b1", "done");
    h.settle();
    assert!(
        h.toast_titles()
            .iter()
            .any(|t| t == "리뷰 봇 · 완료했습니다"),
        "done toast: {:?}",
        h.toast_titles()
    );
    assert!(
        !h.backend.acks.contains(&"b1".to_string()),
        "not acknowledged before the user looks"
    );
    jump_to(&mut h, "리뷰 봇");
    assert_eq!(h.focused_terminal().as_deref(), Some("b1"));
    assert!(
        h.backend.acks.contains(&"b1".to_string()),
        "focusing b1 sent terminal.ack: {:?}",
        h.backend.calls
    );
}

/// Before the root focus fix, nothing was focused at launch and GPUI
/// dispatched Ctrl+K to the window root only: the shortcut did nothing until
/// the user clicked. Also covers focus recovery after the focused panel closes.
#[test]
fn a_shortcuts_work_at_launch_and_after_the_focused_panel_closes() {
    let mut h = Harness::new("a-launch", seed_two_tasks);
    h.keys("ctrl-k");
    assert!(h.dialog_open(), "Ctrl+K at launch opens the palette");
    h.keys("escape");
    let root = h.root_focus();
    assert!(
        h.is_focused(&root),
        "Esc returns to the workspace root: {}",
        h.focus_name()
    );

    jump_to(&mut h, "로그인 작업");
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    // Close the focused panel the way the dock does (its close action).
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| w.close_panels_of("w1", window, cx));
    });
    h.settle();
    h.log("closed w1 panel");
    assert!(
        h.focused().is_some(),
        "focus is not lost when the focused panel closes"
    );
    h.keys("ctrl-n");
    assert!(
        h.dialog_open(),
        "Ctrl+N still works after the focused panel closed"
    );
}

/// Before the dialog patch, Enter anywhere in a Pod dialog ran the toolkit's
/// default Confirm (close): a focused button never got its keyboard click and
/// Enter in a field closed the dialog even when validation failed.
#[test]
fn a_enter_in_dialogs_goes_to_the_focused_control() {
    let mut h = Harness::new("a-dialog-enter", seed_two_tasks);
    h.keys("ctrl-n");
    h.press("enter");
    assert!(
        h.dialog_open(),
        "Enter with an empty name keeps the dialog open (validation error shown)"
    );
    assert!(!h.backend.calls.iter().any(|c| c.starts_with("task.create")));
    // Tab past the name/base fields to the agent choice buttons; Enter selects, never closes.
    let mut agent_buttons = 0;
    for _ in 0..8 {
        h.press("tab");
        if h.focus_name().starts_with("dialog control") {
            h.press("enter");
            assert!(
                h.dialog_open(),
                "Enter on a focused dialog button did not close the dialog"
            );
            agent_buttons += 1;
            if agent_buttons == 2 {
                break;
            }
        }
    }
    assert_eq!(agent_buttons, 2, "two option buttons reached by Tab");
    // Back to the name field (Shift+Tab past the base field), type, Enter.
    while !h.focus_name().starts_with("dialog text input") {
        h.press("shift-tab");
    }
    h.press("shift-tab");
    assert!(
        h.focus_name().starts_with("dialog text input"),
        "name field: {}",
        h.focus_name()
    );
    h.type_text("enter-fix");
    h.press("enter");
    let created = h
        .backend
        .calls
        .iter()
        .find(|c| c.starts_with("task.create"))
        .cloned();
    println!("[native-qa] a-dialog-enter | {created:?}");
    assert!(
        created.is_some(),
        "Enter in a field submits: {:?}",
        h.backend.calls
    );
}

#[test]
fn f_palette_sends_ctrl_p_and_ctrl_k_to_the_current_terminal() {
    let mut h = Harness::new("f-passthrough", seed_two_tasks);
    jump_to(&mut h, "로그인 작업");
    palette(&mut h, "현재 터미널에 Ctrl+P 보내기", "");
    palette(&mut h, "현재 터미널에 Ctrl+K 보내기", "");
    assert_eq!(
        h.backend.typed_text("w1"),
        "\u{10}\u{b}",
        "^P then ^K reached the terminal"
    );
}
