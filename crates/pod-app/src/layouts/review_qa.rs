//! The 리뷰 중심 layout in the real workspace on GPUI's headless platform
//! (native QA, docs/10 §12): keyboard only, the scripted podd double
//! ([`TestBackend`]), no mouse events. Run with `--nocapture` for the
//! `[native-qa]` step log.
use core::prelude::v1::test;
use gpui_kit::*;
use podd::protocol::{GateInfo, GateState};

use super::review::ReviewView;
use crate::native_qa::{Harness, seed_two_tasks};
use crate::source::{SourceEvent, TestBackend};

const LOGIN: &str = "sample/로그인-수정";
const BOT: &str = "sample/review-bot";

fn set_layout(h: &mut Harness, id: &str) {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let s = crate::theme::Settings {
            layout: id.into(),
            ..cx.global::<crate::theme::Settings>().clone()
        };
        s.apply_with(false, cx);
        ws.update(cx, |_, cx| cx.notify());
    });
    h.settle();
    h.settle();
}

fn view(h: &mut Harness) -> Entity<ReviewView> {
    h.cx.update(|_, cx| super::review::view(cx).expect("the layout was shown"))
}

fn selected(h: &mut Harness) -> Option<String> {
    let v = view(h);
    h.cx.update(|_, cx| v.read(cx).model.selected.clone())
}

fn row_focused(h: &mut Harness, id: &str) -> bool {
    let v = view(h);
    let f =
        h.cx.update(|_, cx| v.update(cx, |v, cx| v.row_focus(id, cx)));
    h.is_focused(&f)
}

fn panel_ids(h: &mut Harness) -> Vec<EntityId> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_panels()
            .iter()
            .map(|p| p.entity_id())
            .collect()
    })
}

fn dock_state(h: &mut Harness) -> String {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        serde_json::to_string(&ws.read(cx).dock.read(cx).dump(cx)).expect("dock state")
    })
}

fn review_tasks(h: &mut Harness) -> Vec<String> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_reviews()
            .iter()
            .map(|r| r.read(cx).task_id.clone())
            .collect()
    })
}

fn pending_gate(id: &str, terminal: &str) -> GateInfo {
    GateInfo {
        id: id.into(),
        title: "clippy 실행".into(),
        message: "명령 실행 승인이 필요합니다".into(),
        task: None,
        group: None,
        terminal: Some(terminal.into()),
        state: GateState::Pending,
        created_ms: 1,
        resolved_ms: None,
        note: None,
        expires_ms: None,
        run: None,
        item: None,
        options: Vec::new(),
        choice: None,
        requested_by: None,
        decided_by: None,
    }
}

/// `gate.resolve` answers (approved/rejected).
fn answers(b: &mut TestBackend) {
    b.rpc = Some(Box::new(|method, params| match method {
        "gate.resolve" => {
            let mut g = pending_gate(params["id"].as_str().unwrap_or_default(), "b1");
            g.state = if params["decision"] == "approve" {
                GateState::Approved
            } else {
                GateState::Rejected
            };
            Some(Ok(serde_json::to_value(g).unwrap()))
        }
        _ => None,
    }));
}

/// A small git repository as the 로그인-수정 task's worktree (real `task.diff`).
fn with_worktree(name: &str) -> impl FnOnce(&mut TestBackend) {
    let wt = std::env::temp_dir().join(format!("pod-native-qa-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&wt);
    let wt = pod_editor::fixture::create(&wt, true).expect("fixture repo");
    move |b| {
        seed_two_tasks(b);
        b.tasks[0].worktree = Some(wt.to_string_lossy().into_owned());
        b.worktrees.insert(LOGIN.into(), wt);
    }
}

#[test]
fn switching_layouts_keeps_terminals_attached_and_the_dock_as_it_was() {
    let mut h = Harness::new("review-switch", with_worktree("review-switch"));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("w1", false, window, cx)));
    h.settle();
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    let (panels, dock) = (panel_ids(&mut h), dock_state(&mut h));
    assert!(review_tasks(&mut h).is_empty());

    set_layout(&mut h, "review");
    assert!(h.bounds("review-layout").is_some(), "the layout renders");
    assert!(
        h.bounds("review-task-sample/로그인-수정").is_some()
            && h.bounds("review-task-sample/review-bot").is_some(),
        "task list"
    );
    // The focused terminal's task is the canvas; the terminal is docked as it was: same panel, still focused.
    assert_eq!(selected(&mut h).as_deref(), Some(LOGIN));
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("w1"),
        "focus stays in the user's terminal"
    );
    assert!(
        h.bounds("review-tile-w1").is_some(),
        "w1 docked at the bottom"
    );
    assert_eq!(panel_ids(&mut h), panels, "no new panel or attach");
    assert_eq!(
        review_tasks(&mut h),
        [LOGIN],
        "a layout-owned Review of the task, registered with the workspace"
    );
    assert!(
        h.backend.calls.iter().any(|c| c.starts_with("task.diff")),
        "its diff loads: {:?}",
        h.backend.calls
    );

    // F6 from the terminal wraps to the task list; ↓ switches the canvas to the next task.
    h.keys("f6");
    assert!(
        row_focused(&mut h, LOGIN),
        "F6: tasks after the last terminal"
    );
    h.keys("down");
    assert!(row_focused(&mut h, BOT));
    assert_eq!(
        selected(&mut h).as_deref(),
        Some(BOT),
        "↓ switches the Review"
    );
    assert_eq!(
        review_tasks(&mut h),
        [BOT],
        "the previous layout-owned Review is dropped"
    );
    assert!(
        h.bounds("review-tile-b1").is_some() && h.bounds("review-tile-w1").is_none(),
        "the task's own terminals"
    );
    assert_eq!(
        panel_ids(&mut h).len(),
        panels.len() + 1,
        "a tile panel for b1 (no dock panel)"
    );
    // Enter: into the Review. F6: the docked terminal. Shift+F6 back to the Review.
    h.keys("enter");
    assert!(
        h.focus_name().starts_with("review panel"),
        "Enter focuses the Review: {}",
        h.focus_name()
    );
    h.keys("f6");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "F6: Review → terminal"
    );
    h.keys("shift-f6");
    assert!(
        h.focus_name().starts_with("review panel"),
        "Shift+F6 back: {}",
        h.focus_name()
    );
    // Ctrl+Shift+X from the terminal: over the whole canvas, and back.
    h.keys("f6");
    let small = h.bounds("review-tile-b1").expect("tile");
    h.keys("ctrl-shift-x");
    let big = h.bounds("review-tile-b1").expect("expanded tile");
    assert!(
        big.size.height > small.size.height * 2.,
        "expanded {big:?} vs docked {small:?}"
    );
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "focus stays in the terminal"
    );
    assert!(
        h.bounds("review-terminals")
            .is_some_and(|s| s.size.height < small.size.height),
        "the strip shrinks to its header"
    );
    h.keys("ctrl-shift-x");
    assert!(
        h.bounds("review-tile-b1")
            .is_some_and(|b| b.size.height < big.size.height)
    );

    // Back to the workbench: the dock exactly as it was; the layout's panels and Review are gone.
    set_layout(&mut h, "workbench");
    assert!(h.bounds("review-layout").is_none());
    assert_eq!(dock_state(&mut h), dock, "dock state restored");
    assert_eq!(
        panel_ids(&mut h),
        panels,
        "the workbench's panels kept, the layout's dropped"
    );
    assert!(
        review_tasks(&mut h).is_empty(),
        "no stray Review left behind"
    );
    let v = view(&mut h);
    assert!(
        h.cx.update(|_, cx| v.read(cx).own_panel_ids().is_empty()),
        "tile panels dropped"
    );

    // And again: the layout remembers its task.
    set_layout(&mut h, "review");
    assert_eq!(selected(&mut h).as_deref(), Some(BOT));
}

#[test]
fn workbench_review_is_reused_and_panels_open_in_the_canvas() {
    let mut h = Harness::new("review-reuse", with_worktree("review-reuse"));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_review(LOGIN, window, cx)));
    h.settle();
    let dock_review =
        h.cx.update(|_, cx| ws.read(cx).live_reviews()[0].entity_id());
    let dock = dock_state(&mut h);

    set_layout(&mut h, "review");
    assert_eq!(
        selected(&mut h).as_deref(),
        Some(LOGIN),
        "the focused Review's task"
    );
    assert!(
        h.focus_name().starts_with("review panel"),
        "focus stays in the Review: {}",
        h.focus_name()
    );
    let v = view(&mut h);
    assert!(
        h.cx.update(|_, cx| v.read(cx).own_review().is_none()),
        "the dock's own Review is the canvas"
    );
    assert_eq!(
        h.cx.update(|_, cx| ws
            .read(cx)
            .live_reviews()
            .iter()
            .map(|r| r.entity_id())
            .collect::<Vec<_>>()),
        [dock_review]
    );

    // A palette jump to another task's terminal brings that task in.
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("b1", false, window, cx)));
    h.settle();
    assert_eq!(selected(&mut h).as_deref(), Some(BOT));
    assert_eq!(h.focused_terminal().as_deref(), Some("b1"));

    // Settings while the layout shows: the canvas shows the dock (패널), focus in settings.
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_settings(window, cx)));
    h.settle();
    assert!(h.cx.update(|_, cx| v.read(cx).model.show_dock), "패널 view");
    assert!(
        h.focus_name().starts_with("settings panel"),
        "{}",
        h.focus_name()
    );
    // F6 → tasks, Enter on a task → its Review again.
    h.keys("f6");
    assert!(row_focused(&mut h, BOT));
    h.keys("up");
    h.keys("enter");
    assert!(h.cx.update(|_, cx| !v.read(cx).model.show_dock));
    assert!(
        h.focus_name().starts_with("review panel"),
        "{}",
        h.focus_name()
    );

    set_layout(&mut h, "workbench");
    let after = dock_state(&mut h);
    // The settings page opened meanwhile is the only addition to the dock.
    assert_ne!(after, dock);
    assert_eq!(
        h.cx.update(|_, cx| ws
            .read(cx)
            .live_reviews()
            .iter()
            .map(|r| r.entity_id())
            .collect::<Vec<_>>()),
        [dock_review],
        "the dock's Review kept"
    );
}

#[test]
fn keyboard_approve_a_pending_gate_from_the_review_header() {
    let mut h = Harness::new("review-gate", |b| {
        seed_two_tasks(b);
        answers(b);
    });
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gates(vec![pending_gate("g1", "b1")]));
    h.settle();
    set_layout(&mut h, "review");
    assert!(
        row_focused(&mut h, LOGIN),
        "entering with nothing focused: the first task's row"
    );
    assert!(
        h.bounds("review-gate-g1").is_none(),
        "the gate belongs to the other task"
    );
    h.keys("down");
    assert!(
        h.bounds("review-gate-g1").is_some(),
        "pending gate in the review header"
    );
    assert!(
        h.bounds("review-agents").is_some(),
        "agent status in the header"
    );
    // Tab from the row: 리뷰, 패널, 승인.
    h.keys("tab");
    h.keys("tab");
    h.keys("tab");
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("gate.resolve") && c.contains("approve") && c.contains("g1")),
        "{:?}",
        h.backend.calls
    );
    let ws = h.ws.clone();
    assert!(
        h.cx.update(|_, cx| ws.read(cx).orch.pending().is_empty()),
        "the workspace's gate state updated"
    );
    assert!(h.bounds("review-gate-g1").is_none());
    assert_ne!(h.focus_name(), "NONE", "focus is not lost with the button");
}

/// Whether keyboard focus is in the workspace's dock (the canvas's 패널 view).
fn dock_has_focus(h: &mut Harness) -> bool {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.read(cx)
            .dock
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    })
}

/// The canvas tabs and the 응답하기 button by keyboard: 패널 shows the dock with
/// the focus in it, F6 goes to the tasks (a row shows its Review again), 리뷰
/// focuses the Review, 응답하기 focuses the blocked agent's terminal tile, and
/// a collapsed strip has no terminal stop.
#[test]
fn canvas_tabs_answer_and_strip_by_keyboard() {
    let mut h = Harness::new("review-tabs", with_worktree("review-tabs"));
    set_layout(&mut h, "review");
    assert!(row_focused(&mut h, LOGIN), "the first task's row");
    let v = view(&mut h);
    // Tab from the row: 리뷰, 패널. Enter on 패널: the dock in the canvas, focus in it.
    h.keys("tab");
    h.keys("tab");
    h.keys("enter");
    assert!(h.cx.update(|_, cx| v.read(cx).model.show_dock), "패널 view");
    assert!(dock_has_focus(&mut h), "패널: {}", h.focus_name());
    assert!(
        h.bounds("review-tile-w1").is_none(),
        "the 패널 view hides the tiles"
    );
    // F6 from the dock: the tasks; selecting the row leaves the 패널 view.
    h.keys("f6");
    assert!(row_focused(&mut h, LOGIN), "F6 from the dock: the tasks");
    assert!(
        h.cx.update(|_, cx| !v.read(cx).model.show_dock),
        "a task row shows its Review"
    );
    assert!(h.bounds("review-tile-w1").is_some(), "tiles back");
    // 리뷰 tab: focus in the Review.
    h.keys("tab");
    h.keys("enter");
    assert!(
        h.cx.update(|_, cx| !v.read(cx).model.show_dock),
        "리뷰 view"
    );
    assert!(
        h.focus_name().starts_with("review panel"),
        "리뷰: {}",
        h.focus_name()
    );

    // The other task's agent is blocked without a gate: 응답하기 (after 리뷰, 패널).
    h.keys("shift-f6");
    assert!(row_focused(&mut h, LOGIN));
    h.keys("down");
    assert_eq!(selected(&mut h).as_deref(), Some(BOT));
    h.keys("tab");
    h.keys("tab");
    h.keys("tab");
    h.keys("enter");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "응답하기 focuses the agent's terminal: {}",
        h.focus_name()
    );
    assert!(h.bounds("review-tile-b1").is_some(), "docked, not expanded");
    assert_eq!(h.cx.update(|_, cx| v.read(cx).model.expanded.clone()), None);

    // A collapsed strip: F6 goes tasks → Review → tasks, never into the terminal.
    h.keys("shift-f6");
    assert!(h.focus_name().starts_with("review panel"));
    h.cx.update(|_, cx| {
        v.update(cx, |v, cx| {
            v.model.strip_open = false;
            cx.notify();
        })
    });
    h.settle();
    assert!(
        h.bounds("review-tile-b1").is_none(),
        "only the strip's header"
    );
    h.keys("f6");
    assert!(row_focused(&mut h, BOT), "F6 from the Review: the tasks");
    h.keys("f6");
    assert!(h.focus_name().starts_with("review panel"));
    h.keys("f6");
    assert!(row_focused(&mut h, BOT), "no terminal stop");
}

/// A Review opened while the layout shows (Ctrl+Shift+R, a PR button) becomes
/// the canvas: its task is selected and the dock's panel is shown as it is.
#[test]
fn a_review_opened_meanwhile_becomes_the_canvas() {
    let mut h = Harness::new("review-opened", with_worktree("review-opened"));
    set_layout(&mut h, "review");
    assert_eq!(selected(&mut h).as_deref(), Some(LOGIN));
    let v = view(&mut h);
    assert!(
        h.cx.update(|_, cx| v.read(cx).own_review().is_some()),
        "a layout-owned Review"
    );
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_review(BOT, window, cx)));
    h.settle();
    assert_eq!(
        selected(&mut h).as_deref(),
        Some(BOT),
        "the opened Review's task"
    );
    assert!(
        h.cx.update(|_, cx| v.read(cx).own_review().is_none()),
        "the dock's Review is the canvas; the layout's own is dropped"
    );
    assert_eq!(review_tasks(&mut h), [BOT]);
    assert!(
        h.cx.update(|_, cx| !v.read(cx).model.show_dock),
        "a Review is not one of the 패널"
    );
    assert!(
        h.focus_name().starts_with("review panel"),
        "focus in the opened Review: {}",
        h.focus_name()
    );
    assert!(h.bounds("review-tile-b1").is_some(), "its task's tiles");
}

#[test]
fn korean_text_fits_at_100_to_175_percent_and_large_fonts() {
    for (scale, font) in [
        (1.0, 13.),
        (1.25, 13.),
        (1.5, 13.),
        (1.75, 13.),
        (1.75, 20.),
    ] {
        let mut h = Harness::sized(
            "review-dpi",
            scale,
            size(px(1440. / scale), px(960. / scale)),
            |b| {
                seed_two_tasks(b);
                answers(b);
            },
        );
        let _ = h
            .backend
            .events
            .unbounded_send(SourceEvent::Gates(vec![pending_gate("g1", "b1")]));
        h.cx.update(|_, cx| {
            crate::theme::Settings {
                font_size: font,
                ..cx.global::<crate::theme::Settings>().clone()
            }
            .apply_with(false, cx)
        });
        h.settle();
        set_layout(&mut h, "review");
        h.keys("down");
        let view = h.viewport();
        for id in ["sample/로그인-수정", "sample/review-bot"] {
            let row = h.bounds(&format!("review-task-{id}")).expect("row painted");
            let sub = h
                .bounds(&format!("review-task-sub-{id}"))
                .expect("second line painted");
            assert!(
                sub.bottom() <= row.bottom() && sub.top() >= row.top(),
                "{scale} {font}: second line inside the row {sub:?} {row:?}"
            );
        }
        let header = h.bounds("review-header").expect("header");
        let gate = h.bounds("review-gate-g1").expect("gate");
        assert!(
            gate.bottom() <= header.bottom() && gate.right() <= view.width + px(1.),
            "{scale} {font}: gate row inside the header"
        );
        let strip = h.bounds("review-terminals").expect("terminal strip");
        assert!(
            strip.bottom() <= view.height + px(1.),
            "{scale}: strip inside the window"
        );
        let tile = h.bounds("review-tile-b1").expect("tile");
        assert!(
            tile.right() <= view.width + px(1.),
            "{scale}: tile inside the window"
        );
    }
}

#[test]
fn preset_13_maps_to_the_review_layout() {
    let p = pod_ui::themes::PRESETS
        .into_iter()
        .find(|p| p.number == "13")
        .expect("preset 13");
    assert_eq!(p.layout, "review");
    let r = crate::layout_mode::LayoutRegistry::new();
    assert!(
        r.available(crate::layout_mode::LayoutMode::Review),
        "settings show preset 13 as ready, not 준비 중"
    );
    assert_eq!(
        r.resolve(p.layout).mode(),
        crate::layout_mode::LayoutMode::Review
    );
}

/// A-T2d: the layout draws through its kit renderer (no Workspace adapter)
/// and the window's state slot keeps its view: the same view across a layout
/// round trip (its layout-owned Review made and registered by the host, and
/// dropped on leaving as before); one view per window, dropped with it.
#[test]
fn the_kit_renderer_keeps_one_view_per_window_in_its_state_slot() {
    use super::blocks_qa::second_window;
    use crate::layout_mode::{LayoutMode, LayoutRegistry};
    use crate::layouts::kit::{self, WindowKey};
    assert_eq!(
        LayoutRegistry::new()
            .resolve(LayoutMode::Review.id())
            .mode(),
        LayoutMode::Review,
        "the review layout is registered (every module has a kit renderer)"
    );
    let mut h = Harness::new("review-slot", with_worktree("review-slot"));
    let key = WindowKey(h.ws.entity_id());
    let in_slot = |h: &mut Harness, key| h.cx.update(|_, cx| kit::state::<ReviewView>(key, cx));
    assert!(in_slot(&mut h, key).is_none(), "no view before the layout");
    set_layout(&mut h, "review");
    assert!(h.bounds("review-layout").is_some(), "the renderer drew it");
    let v = view(&mut h);
    assert_eq!(
        in_slot(&mut h, key).map(|s| s.entity_id()),
        Some(v.entity_id()),
        "the view sits in this window's slot"
    );
    assert_eq!(
        selected(&mut h).as_deref(),
        Some(LOGIN),
        "the window's first task, read through the host"
    );
    let ws = h.ws.clone();
    let (own, registered) = h.cx.update(|_, cx| {
        (
            v.read(cx).own_review().map(|r| r.entity_id()),
            ws.read(cx).live_reviews().first().map(|r| r.entity_id()),
        )
    });
    assert!(own.is_some(), "a layout-owned canvas Review");
    assert_eq!(own, registered, "made and registered by the host");
    assert_eq!(review_tasks(&mut h), [LOGIN]);

    set_layout(&mut h, "workbench");
    assert_eq!(
        in_slot(&mut h, key).map(|s| s.entity_id()),
        Some(v.entity_id()),
        "the slot keeps the view while another layout shows"
    );
    assert!(
        review_tasks(&mut h).is_empty(),
        "leaving drops the layout-owned Review"
    );
    set_layout(&mut h, "review");
    assert_eq!(
        view(&mut h).entity_id(),
        v.entity_id(),
        "and shows it again"
    );
    assert_eq!(review_tasks(&mut h), [LOGIN]);

    // A second window: its own view over its own workspace, gone with it.
    let second = second_window(&mut h, seed_two_tasks);
    let other = in_slot(&mut h, second.key).expect("the second window's view");
    assert_ne!(other.entity_id(), v.entity_id(), "one view per window");
    assert_eq!(
        h.cx.update(|_, cx| other.read(cx).model.selected.clone())
            .as_deref(),
        Some(LOGIN)
    );
    assert_eq!(h.cx.update(|_, cx| kit::states::<ReviewView>(cx).len()), 2);
    let weak = other.downgrade();
    drop(other);
    let other = weak;
    let second_key = second.key;
    second.close(&mut h);
    assert!(in_slot(&mut h, second_key).is_none(), "its slot is dropped");
    assert!(other.upgrade().is_none(), "and its view released");
    assert_eq!(
        in_slot(&mut h, key).map(|s| s.entity_id()),
        Some(v.entity_id()),
        "the first window keeps its view"
    );
}

fn canvas_review(h: &mut Harness) -> Entity<crate::review::ReviewPanel> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_reviews()
            .into_iter()
            .find(|r| r.read(cx).task_id == LOGIN)
    })
    .expect("the selected task's Review")
}

fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    inner.left() >= outer.left() - px(0.5) && inner.right() <= outer.right() + px(0.5)
}

/// ui-05 (docs/10 §D4.6): at the 800 logical minimum window the canvas is under 560 px,
/// so the comment column is a drawer (Alt+C opens it with focus inside, Esc closes it);
/// at 960 and 1440 the column sits beside the diff. Neither scrolls sideways nor cuts
/// the composer (instructions + 보내기) off.
#[test]
fn comments_column_fits_or_is_a_drawer_at_800_960_1440() {
    for w in [800., 960., 1440.] {
        let mut h = Harness::sized(
            "review-drawer",
            1.5,
            size(px(w), px(700.)),
            with_worktree(&format!("review-drawer-{w}")),
        );
        set_layout(&mut h, "review");
        let r = canvas_review(&mut h);
        let columns = h.bounds("review-columns").expect("Review columns");
        let drawer = f32::from(columns.size.width) < crate::review::COMMENTS_DRAWER_BELOW;
        assert_eq!(
            h.cx.update(|_, cx| r.read(cx).comments_drawer()),
            drawer,
            "{w}: drawer below 560 ({columns:?})"
        );
        assert_eq!(
            drawer,
            w < 900.,
            "{w}: only the minimum window needs the drawer"
        );
        if !drawer {
            let col = h.bounds("review-comments").expect("comment column");
            assert!(
                inside(col, columns),
                "{w}: comment column inside the canvas {col:?} {columns:?}"
            );
            assert!(
                f32::from(col.size.width) >= crate::review::COMMENTS_MIN_W - 0.5,
                "{w}: column at least 224"
            );
            let composer = h.bounds("review-composer").expect("composer");
            assert!(inside(composer, columns), "{w}: 보내기 not cut off");
            assert!(h.bounds("review-comments-drawer").is_none());
            continue;
        }
        assert!(
            h.bounds("review-comments").is_none(),
            "{w}: no column beside the diff"
        );
        // Keyboard: focus in the Review, Alt+C opens the drawer and focuses it.
        let focus = h.cx.update(|_, cx| r.read(cx).focus_handle());
        h.cx.update(|window, cx| window.focus(&focus, cx));
        h.settle();
        h.keys("alt-c");
        assert!(
            h.cx.update(|_, cx| r.read(cx).comments_drawer_open()),
            "{w}: Alt+C opens the drawer"
        );
        let d = h.bounds("review-comments-drawer").expect("drawer painted");
        assert!(
            inside(d, columns),
            "{w}: drawer inside the canvas {d:?} {columns:?}"
        );
        let composer = h.bounds("review-composer").expect("composer in the drawer");
        assert!(inside(composer, d), "{w}: 보내기 inside the drawer");
        let drawer_focus = h.cx.update(|_, cx| r.read(cx).drawer_focus_handle());
        assert!(
            h.is_focused(&drawer_focus),
            "{w}: focus moves into the drawer"
        );
        assert!(
            h.context_path().contains("PodReviewDrawer"),
            "{w}: drawer key context {}",
            h.context_path()
        );
        // Korean prose wraps between words only (keep-all): every word stays on one line.
        let mut rows = Vec::new();
        for i in 0.. {
            let Some(b) = h.bounds(&format!("rv-hint-w{i}")) else {
                break;
            };
            assert!(
                f32::from(b.size.height) < 24.,
                "{w}: word {i} of the hint is broken across lines {b:?}"
            );
            assert!(inside(b, d), "{w}: word {i} inside the drawer");
            if !rows.contains(&b.top()) {
                rows.push(b.top());
            }
        }
        assert!(
            rows.len() >= 2,
            "{w}: the hint wraps (between words) in the drawer: {rows:?}"
        );
        h.keys("escape");
        assert!(
            !h.cx.update(|_, cx| r.read(cx).comments_drawer_open()),
            "{w}: Esc closes the drawer"
        );
        assert!(h.bounds("review-comments-drawer").is_none());
        assert!(h.is_focused(&focus), "{w}: focus returns to the Review");
        // From the instructions box too.
        h.keys("alt-c");
        let input =
            h.cx.update(|window, cx| r.update(cx, |p, cx| p.instructions_focus(window, cx)));
        assert!(h.is_focused(&input));
        h.keys("escape");
        assert!(
            !h.cx.update(|_, cx| r.read(cx).comments_drawer_open()),
            "{w}: Esc in the instructions closes the drawer"
        );
    }
}

/// A comment body with Korean words and one path longer than the comment column.
const LONG_TOKEN: &str = "crates/pod-app/src/layouts/review_fixture/very_long_module_name/another_level/comment_body_wrapping_check.rs";

/// The task's comment list answers one open comment with [`LONG_TOKEN`] in its body.
fn with_long_comment(name: &str) -> impl FnOnce(&mut TestBackend) {
    let seed = with_worktree(name);
    move |b| {
        seed(b);
        b.rpc = Some(Box::new(|method, _| {
            (method == "review.comments.list").then(|| {
                let c = podd::protocol::ReviewComment {
                    id: "c1".into(),
                    task: LOGIN.into(),
                    file: "src/lib.rs".into(),
                    side: podd::protocol::Side::New,
                    line: 1,
                    end_line: None,
                    body: format!("이 경로를 확인해 주세요 {LONG_TOKEN} 끝"),
                    code: None,
                    state: podd::protocol::CommentState::Open,
                    created_ms: 1,
                    delivery: None,
                    resolved_ms: None,
                    note: None,
                };
                Ok(serde_json::json!({ "comments": [c] }))
            })
        }));
    }
}

/// ui-editor-05: a comment word wider than the comment column (a path) is never cut off.
/// It wraps inside the column (break-all only for that piece); the short Korean words around
/// it stay whole on one line each (keep-all). At 800 (drawer) and 960 (column).
#[test]
fn long_comment_tokens_wrap_inside_the_column_at_800_960() {
    for w in [800., 960.] {
        let mut h = Harness::sized(
            "review-long-token",
            1.5,
            size(px(w), px(700.)),
            with_long_comment(&format!("review-long-token-{w}")),
        );
        set_layout(&mut h, "review");
        let r = canvas_review(&mut h);
        let column = if h.cx.update(|_, cx| r.read(cx).comments_drawer()) {
            let focus = h.cx.update(|_, cx| r.read(cx).focus_handle());
            h.cx.update(|window, cx| window.focus(&focus, cx));
            h.settle();
            h.keys("alt-c");
            h.bounds("review-comments-drawer").expect("drawer painted")
        } else {
            h.bounds("review-comments").expect("comment column")
        };
        let words: Vec<Bounds<Pixels>> = (0..)
            .map_while(|i| h.bounds(&format!("rv-body-w{i}")))
            .collect();
        assert_eq!(words.len(), 6, "{w}: every word of the body is shown");
        for (i, b) in words.iter().enumerate() {
            assert!(
                inside(*b, column),
                "{w}: word {i} inside the column {b:?} {column:?}"
            );
        }
        let long = words[4];
        // 18 px lines: the path needs several lines in a ~224–300 px column.
        assert!(
            f32::from(long.size.height) >= 36.,
            "{w}: the long path wraps instead of being cut off {long:?}"
        );
        assert!(
            long.size.width <= column.size.width,
            "{w}: wrapped at the column width"
        );
        for i in [0, 1, 2, 3, 5] {
            assert!(
                f32::from(words[i].size.height) < 24.,
                "{w}: short word {i} stays on one line {:?}",
                words[i]
            );
        }
    }
}
