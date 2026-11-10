//! (d) Korean IME composition through each control's own input handler:
//! terminal, palette, sidebar filter, New Task name, Review comment.

use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

use super::fixtures::seed_two_tasks;
use super::harness::{Harness, Ime, jump_to};

#[test]
fn d_ime_terminal_sends_bytes_only_on_commit() {
    let mut h = Harness::new("d-ime-terminal", seed_two_tasks);
    jump_to(&mut h, "로그인 작업");
    h.backend.typed.clear();
    h.ime(&[Ime::Compose("ㅎ"), Ime::Compose("하"), Ime::Compose("한")]);
    assert_eq!(h.backend.typed_text("w1"), "", "no bytes while composing");
    let view = h.focused_terminal_view().expect("terminal view");
    let marked =
        h.cx.update(|_, cx| view.read(cx).marked_text().map(str::to_owned));
    assert_eq!(marked.as_deref(), Some("한"), "preedit kept for drawing");
    // Candidate window anchor = the cursor cell (preedit drawn there).
    let anchor = h.cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.bounds_for_range(0..1, Bounds::default(), window, cx)
        })
    });
    let anchor = anchor.expect("IME anchor");
    println!("[native-qa] d-ime-terminal | IME anchor {anchor:?}");
    assert!(
        anchor.size.width > px(0.) && anchor.size.height > px(0.),
        "anchor is a cell"
    );
    h.ime(&[
        Ime::Commit("한"),
        Ime::Compose("ㄱ"),
        Ime::Compose("그"),
        Ime::Compose("글"),
    ]);
    assert_eq!(
        h.backend.typed_text("w1"),
        "한",
        "first syllable sent on commit only"
    );
    h.ime(&[Ime::Commit("글")]);
    assert_eq!(h.backend.typed_text("w1"), "한글");
    // Esc while composing belongs to the IME: no ESC byte; cancel clears the preedit.
    h.ime(&[Ime::Compose("ㅁ")]);
    h.keys("escape");
    h.ime(&[Ime::Compose("")]);
    let marked =
        h.cx.update(|_, cx| view.read(cx).marked_text().map(str::to_owned));
    assert_eq!(marked, None, "cancel cleared the preedit");
    assert_eq!(
        h.backend.typed_text("w1"),
        "한글",
        "no ESC and no cancelled syllable sent"
    );
}

#[test]
fn d_ime_palette_sidebar_filter_new_task_name() {
    let mut h = Harness::new("d-ime-inputs", seed_two_tasks);
    // Palette: Enter while composing must not activate a half-typed query.
    h.keys("ctrl-k");
    h.ime(&[Ime::Compose("ㄹ"), Ime::Compose("리")]);
    assert_eq!(
        h.input_state(),
        (String::from("리"), Some(String::from("리")))
    );
    h.keys("enter");
    assert!(
        h.dialog_open(),
        "Enter during composition does not activate"
    );
    h.ime(&[
        Ime::Commit("리"),
        Ime::Compose("뷰"),
        Ime::Commit("뷰"),
        Ime::Commit(" 봇"),
    ]);
    assert_eq!(h.input_state(), (String::from("리뷰 봇"), None));
    // Cancel a composition: value unchanged.
    h.ime(&[Ime::Compose("ㅋ"), Ime::Compose("")]);
    assert_eq!(h.input_state(), (String::from("리뷰 봇"), None));
    h.keys("enter");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "committed Korean query opened b1"
    );

    // Sidebar filter: Tab from the tree to its filter, compose, commit, filter applies.
    let sidebar = h.sidebar_focus();
    h.cx.update(|window, cx| sidebar.focus(window, cx));
    h.settle();
    // Visual order: tree, header buttons (grid / task / new, orchestration), then the filter.
    let mut tabs = 0;
    while !h.focus_name().starts_with("sidebar filter") {
        tabs += 1;
        assert!(
            tabs <= 8,
            "Tab reaches the filter within the sidebar header: {}",
            h.focus_name()
        );
        h.keys("tab");
    }
    assert!(
        h.bounds("row-b1").is_some(),
        "b1 row painted before filtering"
    );
    h.ime(&[Ime::Compose("로"), Ime::Compose("로그")]);
    assert_eq!(
        h.input_state(),
        (String::from("로그"), Some(String::from("로그")))
    );
    h.ime(&[Ime::Commit("로그"), Ime::Commit("인")]);
    assert_eq!(h.input_state(), (String::from("로그인"), None));
    assert!(h.bounds("row-w1").is_some(), "matching row stays");
    assert!(
        h.bounds("row-b1").is_none(),
        "non-matching row filtered out"
    );

    // New task name: compose, cancel (Esc in the IME), compose again, commit, Enter.
    h.keys("ctrl-n");
    h.ime(&[Ime::Commit("로그인"), Ime::Compose("ㅅ"), Ime::Compose("")]);
    assert_eq!(
        h.input_state(),
        (String::from("로그인"), None),
        "cancelled syllable dropped"
    );
    h.ime(&[Ime::Compose("수"), Ime::Compose("수정")]);
    assert!(
        !h.backend.calls.iter().any(|c| c.starts_with("task.create")),
        "no create while composing: {:?}",
        h.backend.calls
    );
    // Korean IME order: commit, then the Enter key arrives.
    h.ime(&[Ime::Commit("수정")]);
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("task.create sample/로그인수정")),
        "created with the full name: {:?}",
        h.backend.calls
    );
}

#[test]
fn d_ime_review_comment_editor() {
    let wt = std::env::temp_dir().join(format!("pod-native-qa-ime-review-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&wt);
    let wt = pod_editor::fixture::create(&wt, true).expect("fixture repo");
    let mut h = Harness::new("d-ime-review", |b| {
        seed_two_tasks(b);
        b.tasks[0].worktree = Some(wt.to_string_lossy().into_owned());
        b.worktrees.insert("sample/로그인-수정".into(), wt.clone());
    });
    jump_to(&mut h, "로그인 작업");
    h.keys("ctrl-shift-r");
    h.keys("ctrl-shift-c");
    assert!(
        h.focus_name().starts_with("review panel text input"),
        "draft focused: {}",
        h.focus_name()
    );
    h.ime(&[
        Ime::Compose("좋"),
        Ime::Commit("좋"),
        Ime::Compose("아"),
        Ime::Commit("아"),
        Ime::Compose("요"),
    ]);
    assert_eq!(
        h.input_state(),
        (String::from("좋아요"), Some(String::from("요")))
    );
    assert!(
        !h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("review.comments.add")),
        "nothing saved while composing"
    );
    // Microsoft Korean IME: Enter first commits the syllable (GCS_RESULTSTR),
    // then VK_RETURN reaches the app.
    h.ime(&[Ime::Commit("요")]);
    assert_eq!(h.input_state(), (String::from("좋아요"), None));
    h.keys("enter");
    let adds: Vec<_> = h
        .backend
        .calls
        .iter()
        .filter(|c| c.starts_with("review.comments.add"))
        .cloned()
        .collect();
    assert_eq!(adds.len(), 1, "saved once: {adds:?}");
    assert!(
        adds[0].contains("\"body\":\"좋아요\""),
        "saved with the full text: {adds:?}"
    );
}
