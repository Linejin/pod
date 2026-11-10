//! Headless checks for the 2026-11-10 UI review fixes (docs/review/2026-11-10/ui.md):
//! the real workspace on GPUI's test platform, as in `native_qa`.

use core::prelude::v1::test;
use std::time::Duration;

use gpui_kit::*;

use crate::native_qa::{Harness, seed_two_tasks};
use crate::source::TestBackend;

fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left() - px(0.5)
        && inner.right() <= outer.right() + px(0.5)
        && inner.top() >= outer.top() - px(0.5)
        && inner.bottom() <= outer.bottom() + px(0.5)
}

/// ui-06: at the 800×600 minimum window the right group (상태 · 알림) and 응답 stay whole.
#[test]
fn minimum_window_keeps_answers_and_notifications_in_the_status_bar() {
    for scale in [1.0, 1.5] {
        let mut h = Harness::sized(
            "ui-status-min",
            scale,
            size(px(800.), px(600.)),
            seed_two_tasks,
        );
        let view = Bounds::new(point(px(0.), px(0.)), h.viewport());
        let bar = h.bounds("status-bar").expect("status bar");
        for item in ["status-notifications", "status-inspect", "status-blocked"] {
            let b = h
                .bounds(item)
                .unwrap_or_else(|| panic!("{scale}: {item} painted"));
            assert!(
                within(b, bar) && within(b, view),
                "{scale}: {item} {b:?} clipped by {bar:?} / {view:?}"
            );
        }
        let summary = h.bounds("status-summary").expect("summary group");
        let inspect = h.bounds("status-inspect").unwrap();
        assert!(
            summary.right() <= inspect.left() + px(0.5),
            "{scale}: the left group gives way, not the right one"
        );
    }
}

/// ui-18: a toast stays while keyboard focus is in the toast list (stack or its close
/// button), and times out once focus leaves.
#[test]
fn toasts_pause_while_keyboard_focus_is_in_the_list() {
    let mut h = Harness::new("ui-toast-focus", seed_two_tasks);
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.update(cx, |w, cx| {
            w.raise_toast(crate::workspace::ToastKind::Info, "알림 시험", "본문", cx)
        })
    });
    h.settle();
    let stack = h.cx.update(|_, cx| ws.read(cx).toast_focus_handle());
    h.cx.update(|window, cx| stack.focus(window, cx));
    h.settle();
    h.cx.executor().advance_clock(Duration::from_secs(12));
    h.settle();
    assert!(
        h.cx.update(|_, cx| ws.read(cx).visible_toasts())
            .contains(&"알림 시험".to_string()),
        "focus on the stack pauses"
    );
    // Tab to the close button inside the list: still paused.
    h.keys("tab");
    assert!(
        h.cx.update(|window, cx| stack.contains_focused(window, cx)),
        "close button is inside the toast list"
    );
    h.cx.executor().advance_clock(Duration::from_secs(12));
    h.settle();
    assert!(
        h.cx.update(|_, cx| ws.read(cx).visible_toasts())
            .contains(&"알림 시험".to_string()),
        "focus on the close button pauses"
    );
    // Focus leaves: the 7 s info timeout runs.
    let root = h.root_focus();
    h.cx.update(|window, cx| root.focus(window, cx));
    h.settle();
    for _ in 0..40 {
        h.cx.executor().advance_clock(Duration::from_millis(250));
        h.cx.run_until_parked();
    }
    h.settle();
    assert!(
        !h.cx
            .update(|_, cx| ws.read(cx).visible_toasts())
            .contains(&"알림 시험".to_string()),
        "times out after focus leaves"
    );
}

/// ui-10 / 추정: the notification list is a flat list rebuilt while open; an answered
/// request leaves it without reopening the dialog.
#[test]
fn notifications_list_follows_live_status() {
    let mut h = Harness::new("ui-notifications", seed_two_tasks);
    h.keys("ctrl-shift-b");
    assert!(h.dialog_open());
    let row = h.bounds("notice-b1").expect("blocked request listed");
    assert!(row.size.height <= px(32.5), "32px row: {row:?}");
    assert!(
        h.bounds("notice-w1").is_none(),
        "working terminal is not a request"
    );
    h.backend.set_status("b1", "working");
    h.settle();
    assert!(h.dialog_open());
    assert!(
        h.bounds("notice-b1").is_none(),
        "answered request left the open list"
    );
    h.backend.set_status("w1", "blocked");
    h.settle();
    assert!(
        h.bounds("notice-w1").is_some(),
        "a new request appears in the open list"
    );
}

/// ui-13: the first ↓ in the tree selects the first row; Home/End; ← from a terminal
/// goes to its task.
#[test]
fn sidebar_tree_keys() {
    let mut h = Harness::new("ui-tree-keys", seed_two_tasks);
    let sidebar = h.sidebar_focus();
    h.cx.update(|window, cx| sidebar.focus(window, cx));
    h.settle();
    let ws = h.ws.clone();
    let selected = |h: &mut Harness| {
        h.cx.update(|_, cx| ws.read(cx).sidebar.read(cx).selected.clone())
    };
    h.keys("down");
    assert_eq!(
        selected(&mut h).as_deref(),
        Some("repo:sample"),
        "first ↓ selects row 0"
    );
    h.keys("end");
    assert_eq!(selected(&mut h).as_deref(), Some("terminal:b1"));
    h.keys("left");
    assert_eq!(
        selected(&mut h).as_deref(),
        Some("task:sample/review-bot"),
        "← from a terminal goes to its task"
    );
    h.keys("home");
    assert_eq!(selected(&mut h).as_deref(), Some("repo:sample"));
}

/// ui-03: the OS window title names the app, then the task of the focused terminal.
#[test]
fn window_title_follows_the_focused_task() {
    let mut h = Harness::new("ui-title", seed_two_tasks);
    let ws = h.ws.clone();
    assert_eq!(h.cx.update(|_, cx| ws.read(cx).os_title.clone()), "Pod");
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("w1", false, window, cx)));
    h.settle();
    assert_eq!(
        h.cx.update(|_, cx| ws.read(cx).os_title.clone()),
        "로그인-수정 — Pod"
    );
}

/// ui-10: a registered repository without tasks offers 새 작업 as the one CTA.
#[test]
fn empty_workspace_has_one_contextual_action() {
    let mut h = Harness::new("ui-empty", |b: &mut TestBackend| {
        b.repos.push(crate::vm::RepoVm {
            name: "sample".into(),
            path: "C:/fixtures/sample".into(),
            default_base: Some("main".into()),
        });
    });
    let ws = h.ws.clone();
    let (connected, repos, tasks) = h.cx.update(|_, cx| {
        let w = ws.read(cx);
        (w.conn.is_connected(), w.repos.len(), w.tasks.len())
    });
    assert!(connected && repos == 1 && tasks == 0);
    assert_eq!(
        crate::workspace::EmptyState::of(connected, repos == 0 && tasks == 0, tasks == 0),
        crate::workspace::EmptyState::NoTask
    );
}
