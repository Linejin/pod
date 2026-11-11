//! 집중 레일: the pure state model, then headless GPUI flows on the real
//! workspace (native-QA harness: real bindings, focus, dock; scripted podd).
// Not `super::*` alone: gpui's glob exports its own `test` attribute.
use core::prelude::v1::test;
use std::collections::HashSet;

use super::RailState;
use super::model::{
    Drawer, Shape, attention, default_shape, fit_tabs, plan, step, tab_tasks, tab_width, task_rows,
    text_width,
};
use crate::native_qa::Harness;
use crate::source::TestBackend;

fn tabs(panels: &[u32], active: usize) -> Shape<u32> {
    Shape::Tabs {
        panels: panels.to_vec(),
        active,
    }
}

#[test]
fn default_shape_is_one_wide_group_and_a_stack() {
    assert_eq!(default_shape::<u32>(&[], &[], None), None);
    assert_eq!(default_shape(&[1], &[9], None), Some(tabs(&[1, 9], 0)));
    assert_eq!(
        default_shape(&[1, 2], &[], Some(600.)),
        Some(Shape::Split {
            horizontal: true,
            children: vec![(tabs(&[1], 0), Some(600.)), (tabs(&[2], 0), None)]
        })
    );
    let three = default_shape(&[1, 2, 3], &[9], None).expect("shape");
    assert_eq!(
        three,
        Shape::Split {
            horizontal: true,
            children: vec![
                (tabs(&[1, 9], 0), None),
                (
                    Shape::Split {
                        horizontal: false,
                        children: vec![(tabs(&[2], 0), None), (tabs(&[3], 0), None)]
                    },
                    None
                )
            ]
        }
    );
    assert_eq!(three.first_shown(), Some(1));
}

#[test]
fn plan_keeps_the_users_arrangement_drops_closed_and_shows_new_panels() {
    let user = Shape::Split {
        horizontal: false,
        children: vec![(tabs(&[1, 2], 1), Some(300.)), (tabs(&[3], 0), None)],
    };
    // Nothing changed: identical (so the dock is not rebuilt).
    assert_eq!(
        plan(Some(user.clone()), &[1, 2, 3], &[], None),
        Some(user.clone())
    );
    // 3 closed: the split collapses to its other child; the shown tab stays.
    assert_eq!(
        plan(Some(user.clone()), &[1, 2], &[], None),
        Some(tabs(&[1, 2], 1))
    );
    // 2 (the shown tab) closed: its group shows the first remaining one.
    assert_eq!(
        plan(Some(user.clone()), &[1, 3], &[], None),
        Some(Shape::Split {
            horizontal: false,
            children: vec![(tabs(&[1], 0), Some(300.)), (tabs(&[3], 0), None)]
        })
    );
    // A new panel joins the first group and is shown.
    let grown = plan(Some(user.clone()), &[1, 2, 3, 4], &[], None).expect("shape");
    assert_eq!(grown.panels(), vec![1, 2, 4, 3]);
    assert!(grown.is_shown(4));
    // Nothing remembered (or all of it closed): the default.
    assert_eq!(
        plan(Some(user), &[7, 8], &[], None),
        default_shape(&[7, 8], &[], None)
    );
    assert_eq!(plan::<u32>(None, &[], &[], None), None);
}

#[test]
fn shape_shown_and_retain() {
    let s = Shape::Split {
        horizontal: true,
        children: vec![(tabs(&[1, 2], 0), None), (tabs(&[3, 4], 1), None)],
    };
    assert!(s.is_shown(4) && s.is_shown(1) && !s.is_shown(3));
    let keep: HashSet<u32> = [4].into();
    assert_eq!(s.retain(&keep), Some(tabs(&[4], 0)));
}

#[test]
fn tabs_fit_and_the_current_one_is_always_visible() {
    let w = [100., 100., 100., 100.];
    assert_eq!(fit_tabs(&w, Some(0), 1000.), vec![0, 1, 2, 3]);
    assert_eq!(fit_tabs(&w, Some(1), 250.), vec![0, 1]);
    // The current task is past the edge: it replaces the last visible one.
    assert_eq!(fit_tabs(&w, Some(3), 250.), vec![0, 3]);
    // No room at all: still the current one (it ellipsizes).
    assert_eq!(fit_tabs(&w, Some(2), 10.), vec![2]);
    assert_eq!(fit_tabs(&w, None, 10.), Vec::<usize>::new());
}

#[test]
fn korean_titles_are_measured_as_full_width() {
    // 13px UI: Hangul ~1em, ASCII 0.6em.
    assert_eq!(text_width("한글", 13.), 26.);
    assert!((text_width("ab", 10.) - 12.).abs() < 0.01);
    // Long titles cap at TAB_TEXT_MAX (ellipsis + tooltip).
    let long = "아주 긴 한국어 작업 이름이 계속 이어집니다 정말 길게 길게";
    assert_eq!(
        tab_width(long, 15., false, 0),
        tab_width(&long.repeat(3), 15., false, 0)
    );
    assert!(tab_width("작업", 13., true, 0) > tab_width("작업", 13., false, 0));
    // Agent marks reserve their width (docs/10 §D5.11), capped at three + `+N`.
    assert!(tab_width("작업", 13., false, 2) > tab_width("작업", 13., false, 1));
    assert_eq!(
        tab_width("작업", 13., false, 9) - tab_width("작업", 13., false, 3),
        20.
    );
}

#[test]
fn step_wraps_through_the_tabs() {
    let ids: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
    assert_eq!(step(&ids, Some("a"), 1).as_deref(), Some("b"));
    assert_eq!(step(&ids, Some("c"), 1).as_deref(), Some("a"));
    assert_eq!(step(&ids, Some("a"), -1).as_deref(), Some("c"));
    assert_eq!(step(&ids, None, 1).as_deref(), Some("a"));
    assert_eq!(step(&[], Some("a"), 1), None);
}

fn fixture_rows() -> Vec<super::TaskRow> {
    let tasks = vec![
        TestBackend::task("r/a", "가 작업", "r", &["a1", "a2"]),
        TestBackend::task("r/b", "나 작업", "r", &["b1"]),
        TestBackend::task("r/c", "다 작업", "r", &["c1"]),
        TestBackend::task("r/d", "라 작업", "r", &["d1"]),
    ];
    let terminals = vec![
        TestBackend::terminal("a1", "A1", Some("r/a"), "working"),
        TestBackend::terminal("a2", "A2", Some("r/a"), "blocked"),
        TestBackend::terminal("b1", "B1", Some("r/b"), "error"),
        TestBackend::terminal("c1", "C1", Some("r/c"), "blocked"),
        TestBackend::terminal("d1", "D1", Some("r/d"), "idle"),
    ];
    let open: HashSet<String> = ["r/b".to_string()].into();
    task_rows(&tasks, &terminals, &[], &open)
}

#[test]
fn attention_lists_other_tasks_requests_before_failures() {
    let rows = fixture_rows();
    assert_eq!(
        rows[0].first_attention.as_deref(),
        Some("a2"),
        "the blocked terminal, not the first one"
    );
    assert_eq!(
        rows[1].first_attention.as_deref(),
        Some("b1"),
        "a failed one when nothing is blocked"
    );
    assert_eq!(rows[0].attention_label(), "승인 필요 1");
    assert_eq!(rows[1].accessible_name(), "나 작업 — 실패 1");
    // Current task a: c (blocked) before b (failed); d needs nothing.
    let ids: Vec<&str> = attention(&rows, Some("r/a"))
        .into_iter()
        .map(|i| rows[i].id.as_str())
        .collect();
    assert_eq!(ids, ["r/c", "r/b"]);
    let ids: Vec<&str> = attention(&rows, Some("r/c"))
        .into_iter()
        .map(|i| rows[i].id.as_str())
        .collect();
    assert_eq!(ids, ["r/a", "r/b"]);
}

#[test]
fn tabs_are_open_recent_and_current_tasks_in_task_order() {
    let rows = fixture_rows();
    let ids = |v: Vec<usize>| {
        v.into_iter()
            .map(|i| rows[i].id.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(tab_tasks(&rows, &[], Some("r/d"))), ["r/b", "r/d"]);
    assert_eq!(
        ids(tab_tasks(&rows, &["r/a".into()], Some("r/d"))),
        ["r/a", "r/b", "r/d"]
    );
}

#[test]
fn badges_marks_and_attention_targets_follow_what_waits() {
    use super::model::{AttentionTarget, Badge, RowMark};
    let mut rows = fixture_rows();
    // a: a blocked agent; b: only a failure; d: nothing waits.
    assert_eq!(rows[0].badge(), Some(Badge::Waiting(1)));
    assert_eq!(rows[1].badge(), Some(Badge::Failed(1)));
    assert_eq!(rows[3].badge(), None);
    assert_eq!(rows[0].mark(false), RowMark::Waiting);
    assert_eq!(
        rows[1].mark(true),
        RowMark::Failed,
        "a failure outranks current"
    );
    assert_eq!(rows[3].mark(true), RowMark::Current);
    assert_eq!(rows[3].mark(false), RowMark::Idle);
    assert_eq!(
        rows[0].attention_target(),
        AttentionTarget::Terminal("a2".into())
    );
    assert_eq!(rows[3].attention_target(), AttentionTarget::Nothing);
    // Gates without a waiting terminal open the inbox; a request outranks a failure.
    rows[1].gates = 2;
    rows[1].first_attention = None;
    assert_eq!(rows[1].attention_target(), AttentionTarget::Gates);
    assert_eq!(rows[1].badge(), Some(Badge::Waiting(2)));
    assert_eq!(rows[1].mark(false), RowMark::Waiting);
    assert_eq!(rows[1].attention_label(), "승인 요청 2 · 실패 1");
}

#[test]
fn panels_belong_to_their_task_and_the_rest_is_global() {
    use super::model::{Owner, panel_owner, task_panels};
    let tasks = vec![TestBackend::task("r/a", "가", "r", &["a1"])];
    let terminals = vec![
        TestBackend::terminal("a1", "A1", Some("r/a"), "idle"),
        TestBackend::terminal("x1", "X1", Some("r/x"), "idle"),
    ];
    let task = |t: &str| Owner::Task(t.into());
    let term = crate::panel::PANEL_NAME;
    assert_eq!(
        panel_owner(term, Some("a1"), None, &tasks, &terminals),
        task("r/a")
    );
    // Not listed by a task: the terminal's own task; unknown: global.
    assert_eq!(
        panel_owner(term, Some("x1"), None, &tasks, &terminals),
        task("r/x")
    );
    assert_eq!(
        panel_owner(term, Some("zz"), None, &tasks, &terminals),
        Owner::Global
    );
    // Editor and review panels by their saved task; other panels are global.
    let editor = crate::editor_panel::EDITOR_PANEL_NAME;
    let review = crate::review::REVIEW_PANEL_NAME;
    assert_eq!(
        panel_owner(editor, None, Some("r/a"), &tasks, &terminals),
        task("r/a")
    );
    assert_eq!(
        panel_owner(review, None, Some("r/b"), &tasks, &terminals),
        task("r/b")
    );
    assert_eq!(
        panel_owner("pod-settings", None, Some("r/a"), &tasks, &terminals),
        Owner::Global
    );
    assert_eq!(
        panel_owner(editor, None, None, &tasks, &terminals),
        Owner::Global
    );
    // The current task's terminals first, then its other panels, then the global ones.
    let (a, b, g) = (task("r/a"), task("r/b"), Owner::Global);
    let panels = [
        (1, &g, false),
        (2, &a, true),
        (3, &b, true),
        (4, &a, false),
        (5, &a, true),
        (6, &g, false),
    ];
    assert_eq!(
        task_panels(panels.iter().copied(), Some("r/a")),
        (vec![2, 5], vec![4, 1, 6])
    );
    assert_eq!(
        task_panels(panels.iter().copied(), None),
        (vec![], vec![1, 6])
    );
    assert_eq!(
        task_panels(panels.iter().copied(), Some("r/z")),
        (vec![], vec![1, 6])
    );
}

#[test]
fn current_task_is_kept_else_focused_open_or_first() {
    use super::model::keep_or_pick;
    let rows = fixture_rows();
    let never = || -> Option<String> { panic!("not asked while the current task exists") };
    assert_eq!(
        keep_or_pick(Some("r/c".into()), &rows, never).as_deref(),
        Some("r/c")
    );
    assert_eq!(
        keep_or_pick(Some("r/gone".into()), &rows, || Some("r/d".into())).as_deref(),
        Some("r/d")
    );
    // The focused task is unknown: the first task with panels (b), else the first.
    assert_eq!(
        keep_or_pick(None, &rows, || Some("r/zz".into())).as_deref(),
        Some("r/b")
    );
    assert_eq!(
        keep_or_pick(None, &rows[2..], || None).as_deref(),
        Some("r/c")
    );
    assert_eq!(keep_or_pick(None, &[], || Some("r/a".into())), None);
}

#[test]
fn recent_tasks_are_newest_first_without_duplicates_and_capped() {
    use super::model::{RECENT_MAX, push_recent};
    let mut recent = Vec::new();
    for t in ["a", "b", "a"] {
        push_recent(&mut recent, t.to_string());
    }
    assert_eq!(recent, ["a", "b"]);
    for n in 0..20 {
        push_recent(&mut recent, format!("t{n}"));
    }
    assert_eq!(recent.len(), RECENT_MAX);
    assert_eq!(recent[0], "t19");
}

#[test]
fn drawer_cursor_and_keys_step_within_the_list() {
    use super::model::{KeyStep, drawer_cursor, key_step};
    let rows = fixture_rows();
    assert_eq!(drawer_cursor(Drawer::Tasks, &rows, Some("r/c")), 2);
    assert_eq!(drawer_cursor(Drawer::Tasks, &rows, Some("r/zz")), 0);
    assert_eq!(drawer_cursor(Drawer::Attention, &rows, Some("r/c")), 0);
    let strip = |k: &str, at: usize| key_step(k, at, 3, "left", "right");
    assert_eq!(strip("left", 0), Some(KeyStep::To(0)));
    assert_eq!(strip("right", 1), Some(KeyStep::To(2)));
    assert_eq!(strip("right", 2), Some(KeyStep::To(2)), "stops at the end");
    assert_eq!(strip("home", 2), Some(KeyStep::To(0)));
    assert_eq!(strip("end", 0), Some(KeyStep::To(2)));
    assert_eq!(strip("enter", 1), Some(KeyStep::Activate));
    assert_eq!(strip("space", 1), Some(KeyStep::Activate));
    assert_eq!(strip("down", 1), None, "the strip ignores the list keys");
    let list = |k: &str, at: usize| key_step(k, at, 3, "up", "down");
    assert_eq!(list("down", 0), Some(KeyStep::To(1)));
    assert_eq!(list("up", 0), Some(KeyStep::To(0)));
    assert_eq!(list("left", 1), None);
    assert_eq!(
        key_step("end", 0, 0, "up", "down"),
        Some(KeyStep::To(0)),
        "empty list"
    );
}

#[test]
fn strip_room_leaves_the_rail_gaps_buttons_and_switcher() {
    use super::model::strip_room;
    let switcher = text_width(super::copy::SWITCHER, 13.) + 48.;
    let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
    assert!(close(
        strip_room(1000., 0., 13.),
        1000. - 44. - 36. - 56. - switcher - 16.
    ));
    assert!(close(
        strip_room(1000., 8., 13.),
        strip_room(1000., 0., 13.) - 24.
    ));
    assert!(strip_room(100., 0., 13.) < 0., "the caller clamps");
}

// ---- headless GPUI flows ---------------------------------------------------------

/// Two tasks with terminals (one blocked) plus a third, the rail layout on.
fn seed(b: &mut TestBackend) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    b.tasks.push(TestBackend::task(
        "sample/로그인-수정",
        "로그인-수정",
        "sample",
        &["w1", "w2", "w3"],
    ));
    b.tasks.push(TestBackend::task(
        "sample/review-bot",
        "review-bot",
        "sample",
        &["b1"],
    ));
    b.tasks
        .push(TestBackend::task("sample/문서", "문서", "sample", &["d1"]));
    b.terminals.push(TestBackend::terminal(
        "w1",
        "로그인 작업",
        Some("sample/로그인-수정"),
        "working",
    ));
    b.terminals.push(TestBackend::terminal(
        "w2",
        "테스트",
        Some("sample/로그인-수정"),
        "idle",
    ));
    b.terminals.push(TestBackend::terminal(
        "w3",
        "빌드",
        Some("sample/로그인-수정"),
        "done",
    ));
    b.terminals.push(TestBackend::terminal(
        "b1",
        "리뷰 봇",
        Some("sample/review-bot"),
        "blocked",
    ));
    b.terminals.push(TestBackend::terminal(
        "d1",
        "문서 셸",
        Some("sample/문서"),
        "idle",
    ));
}

fn set_layout(h: &mut Harness, layout: &str) {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let mut s = cx.global::<crate::theme::Settings>().clone();
        s.layout = layout.into();
        cx.set_global(s);
        ws.update(cx, |w, cx| w.touch(cx));
    });
    h.settle();
}

/// The window's rail state (the kit's state slot).
fn rail(h: &mut Harness) -> gpui_kit::Entity<RailState> {
    h.cx.update(|window, cx| {
        crate::layouts::kit::state_in_window::<RailState>(window.window_handle(), cx)
    })
    .expect("the rail was rendered")
}

fn open(h: &mut Harness, id: &str) {
    let ws = h.ws.clone();
    let id = id.to_string();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal(&id, false, window, cx)));
    h.settle();
}

/// Terminal panels (entity ids) the workspace has open.
fn panels(h: &mut Harness) -> Vec<(String, gpui_kit::EntityId)> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_panels()
            .into_iter()
            .map(|p| (p.read(cx).terminal_id.clone(), p.entity_id()))
            .collect()
    })
}

/// Terminal ids the rail dock shows.
fn shown(h: &mut Harness) -> Vec<String> {
    let ws = h.ws.clone();
    let r = rail(h);
    h.cx.update(|_, cx| {
        let ids = r.read(cx).shown_panels(cx);
        ws.read(cx)
            .live_panels()
            .into_iter()
            .filter(|p| ids.contains(&gpui_kit::component::dock::PanelId::from(p.entity_id())))
            .map(|p| p.read(cx).terminal_id.clone())
            .collect()
    })
}

fn current(h: &mut Harness) -> Option<String> {
    let r = rail(h);
    h.cx.update(|_, cx| r.read(cx).current.clone())
}

fn workbench_dump(h: &mut Harness) -> String {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| serde_json::to_string(&ws.read(cx).dock.read(cx).dump(cx)).expect("dump"))
}

#[test]
fn layout_switch_keeps_terminals_and_restores_the_workbench_dock() {
    let mut h = Harness::new("rail-switch", seed);
    open(&mut h, "w1");
    open(&mut h, "w2");
    open(&mut h, "b1");
    let before = panels(&mut h);
    let wb_before = workbench_dump(&mut h);
    let calls_before = h.backend.calls.len();

    set_layout(&mut h, "focus-rail");
    // The rail follows the focused terminal (b1, opened last): only its task.
    assert_eq!(current(&mut h).as_deref(), Some("sample/review-bot"));
    assert_eq!(shown(&mut h), ["b1"]);
    // `qa_id` measures the padding box: 44px rail = 43 + its 1px right border.
    assert!(
        h.bounds("rail")
            .is_some_and(|b| (42.5..=44.5).contains(&b.size.width.to_f64())),
        "44px rail: {:?}",
        h.bounds("rail")
    );
    // Same panel entities: nothing detached or re-spawned.
    assert_eq!(panels(&mut h), before);

    // Switch task by keyboard (Ctrl+Alt+PageUp from the terminal): only that task's panels.
    h.keys("ctrl-alt-pageup");
    assert_eq!(current(&mut h).as_deref(), Some("sample/로그인-수정"));
    let mut s = shown(&mut h);
    s.sort();
    assert_eq!(s, ["w1", "w2"]);
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("w1"),
        "focus moved into the task's first panel"
    );
    assert_eq!(panels(&mut h), before, "task switch keeps every panel");

    // Back to the workbench: its dock is exactly what it was.
    set_layout(&mut h, "workbench");
    assert_eq!(workbench_dump(&mut h), wb_before);
    assert_eq!(panels(&mut h), before);
    assert!(
        h.backend.calls[calls_before..]
            .iter()
            .all(|c| !c.starts_with("terminal.run")),
        "no terminal re-spawned: {:?}",
        &h.backend.calls[calls_before..]
    );

    // And to the rail again: the rail's own state (current task) is back.
    set_layout(&mut h, "focus-rail");
    assert_eq!(current(&mut h).as_deref(), Some("sample/로그인-수정"));
    let mut s = shown(&mut h);
    s.sort();
    assert_eq!(s, ["w1", "w2"]);
}

#[test]
fn per_task_arrangement_survives_task_switches() {
    let mut h = Harness::new("rail-shapes", seed);
    set_layout(&mut h, "focus-rail");
    for id in ["w1", "w2", "w3"] {
        open(&mut h, id);
    }
    // Ctrl+Shift+G in the rail: the gallery arrangement (wide + stack).
    h.keys("ctrl-shift-g");
    let r = rail(&mut h);
    let shape = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            r.read(cx)
                .dock
                .read(cx)
                .layout(gpui_kit::component::dock::DockPlacement::Center)
                .and_then(|t| super::shape_of(t.root()))
        })
    };
    let arranged = shape(&mut h).expect("arranged");
    assert!(
        matches!(&arranged, Shape::Split { horizontal: true, children } if children.len() == 2),
        "{arranged:?}"
    );
    open(&mut h, "b1");
    assert_eq!(
        current(&mut h).as_deref(),
        Some("sample/review-bot"),
        "opening another task's terminal follows it"
    );
    assert_eq!(shown(&mut h), ["b1"]);
    h.keys("ctrl-alt-pageup");
    assert_eq!(current(&mut h).as_deref(), Some("sample/로그인-수정"));
    assert_eq!(shape(&mut h), Some(arranged), "the arrangement came back");
}

#[test]
fn closing_a_rail_tab_closes_the_panel_and_a_switch_does_not() {
    let mut h = Harness::new("rail-close", seed);
    set_layout(&mut h, "focus-rail");
    open(&mut h, "w1");
    open(&mut h, "b1");
    assert_eq!(panels(&mut h).len(), 2);
    // A switch takes w1 out of the rail dock quietly.
    assert_eq!(shown(&mut h), ["b1"]);
    assert_eq!(panels(&mut h).len(), 2);
    // Closing b1's tab in the rail is a real close (the workbench loses it too).
    let r = rail(&mut h);
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        let p = ws
            .read(cx)
            .live_panels()
            .into_iter()
            .find(|p| p.read(cx).terminal_id == "b1")
            .expect("b1");
        let id = gpui_kit::component::dock::PanelId::from(p.entity_id());
        let dock = r.read(cx).dock.clone();
        assert!(dock.read(cx).panel(id).is_some(), "b1 is in the rail dock");
        // Removal not made by the rail itself (the tab bar's close button): not quiet.
        dock.update(cx, |a, cx| {
            a.set_center(gpui_kit::component::dock::DockLayout::h_split(), window, cx)
        });
    });
    h.settle();
    let left: Vec<String> = panels(&mut h).into_iter().map(|(t, _)| t).collect();
    assert_eq!(left, ["w1"], "b1 closed in the workbench; w1 untouched");
}

#[test]
fn attention_drawer_by_keyboard_reaches_another_tasks_request() {
    let mut h = Harness::new("rail-attention", seed);
    set_layout(&mut h, "focus-rail");
    open(&mut h, "w1");
    assert_eq!(current(&mut h).as_deref(), Some("sample/로그인-수정"));
    let r = rail(&mut h);
    let count = h.cx.update(|_, cx| r.read(cx).attention.len());
    assert_eq!(count, 1, "review-bot is blocked");
    // Ctrl+Alt+A from the terminal: the drawer opens with focus in its list.
    h.keys("ctrl-alt-a");
    assert_eq!(
        h.cx.update(|_, cx| r.read(cx).drawer),
        Some(Drawer::Attention)
    );
    let list = h.cx.update(|_, cx| r.read(cx).list_focus.clone());
    assert!(
        h.is_focused(&list),
        "focus in the 응답 대기 list: {}",
        h.focus_name()
    );
    assert!(h.bounds("rail-drawer").is_some());
    // Escape closes it and focus returns to the terminal.
    h.keys("escape");
    assert_eq!(h.cx.update(|_, cx| r.read(cx).drawer), None);
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    // Open again, Enter: switch to review-bot and focus its blocked terminal.
    h.keys("ctrl-alt-a");
    h.keys("enter");
    assert_eq!(current(&mut h).as_deref(), Some("sample/review-bot"));
    assert_eq!(h.focused_terminal().as_deref(), Some("b1"));
    assert_eq!(shown(&mut h), ["b1"]);
    assert_eq!(
        h.cx.update(|_, cx| r.read(cx).attention.len()),
        0,
        "the current task is not listed"
    );
    // A new request elsewhere raises the badge again (status event, no polling).
    h.backend.set_status("d1", "blocked");
    h.settle();
    assert_eq!(h.cx.update(|_, cx| r.read(cx).attention.len()), 1);
}

#[test]
fn task_switcher_and_tab_strip_by_keyboard() {
    let mut h = Harness::new("rail-switcher", seed);
    set_layout(&mut h, "focus-rail");
    open(&mut h, "w1");
    // Ctrl+Alt+T: 작업 전환 lists every task, the current one selected.
    h.keys("ctrl-alt-t");
    let r = rail(&mut h);
    assert_eq!(h.cx.update(|_, cx| r.read(cx).drawer), Some(Drawer::Tasks));
    // The auto-height drawer shows every row (regression: the list collapsed to one row).
    let list = h.bounds("rail-list").expect("task list");
    assert!(
        list.size.height.to_f64() >= 3. * 30. - 1.,
        "three task rows visible: {list:?}"
    );
    h.keys("down");
    h.keys("down");
    h.keys("enter");
    assert_eq!(
        current(&mut h).as_deref(),
        Some("sample/문서"),
        "third task picked"
    );
    assert_eq!(h.cx.update(|_, cx| r.read(cx).drawer), None);
    // Its empty state offers to open the terminals: no panel was opened by switching.
    assert!(shown(&mut h).is_empty());
    assert!(!panels(&mut h).iter().any(|(t, _)| t == "d1"));
    // The tab strip: focus it, Left, Enter switches back.
    let strip = h.cx.update(|_, cx| r.read(cx).strip_focus.clone());
    h.cx.update(|window, cx| window.focus(&strip, cx));
    h.settle();
    h.keys("left");
    h.keys("enter");
    assert_eq!(current(&mut h).as_deref(), Some("sample/로그인-수정"));
    assert!(
        h.is_focused(&strip),
        "Enter on a tab keeps focus on the strip"
    );
}

#[test]
fn tab_order_reaches_the_rail_tabs_and_switcher() {
    let mut h = Harness::new("rail-tab-order", seed);
    set_layout(&mut h, "focus-rail");
    let r = rail(&mut h);
    let (buttons, strip, switcher) = h.cx.update(|_, cx| {
        let r = r.read(cx);
        (
            r.buttons.clone(),
            r.strip_focus.clone(),
            r.switcher_focus.clone(),
        )
    });
    let mut order = Vec::new();
    for _ in 0..30 {
        h.press("tab");
        let f = h.focused();
        let name = if let Some(i) = buttons.iter().position(|b| Some(b) == f.as_ref()) {
            format!("rail{i}")
        } else if f.as_ref() == Some(&strip) {
            "tabs".into()
        } else if f.as_ref() == Some(&switcher) {
            "switcher".into()
        } else {
            continue;
        };
        if order.last() != Some(&name) {
            order.push(name);
        }
        if order.len() == 7 {
            break;
        }
    }
    assert_eq!(
        order,
        [
            "rail0", "rail1", "rail2", "rail3", "rail4", "tabs", "switcher"
        ],
        "focus order"
    );
}

#[test]
fn long_korean_titles_fit_at_175_percent_in_a_small_window() {
    use gpui_kit::{px, size};
    let long = "로그인 화면의 한국어 입력과 세션 복원 오류를 고치는 아주 긴 작업 이름";
    let mut h = Harness::sized("rail-175", 1.75, size(px(960.), px(640.)), |b| {
        seed(b);
        for n in 0..12 {
            let id = format!("sample/긴-{n}");
            let term = format!("l{n}");
            b.tasks.push(TestBackend::task(
                &id,
                &format!("{long} {n}"),
                "sample",
                &[&term],
            ));
            b.terminals
                .push(TestBackend::terminal(&term, "셸", Some(&id), "idle"));
        }
    });
    set_layout(&mut h, "focus-rail");
    for n in 0..12 {
        open(&mut h, &format!("l{n}"));
    }
    let view = h.viewport();
    let tabs = h.bounds("rail-tabs").expect("tab bar");
    let switcher = h.bounds("rail-switcher").expect("작업 전환");
    let current = h.bounds("rail-current-tab").expect("current tab");
    // `qa_id` measures inside the bottom border: 32px bar.
    assert!(tabs.size.height.to_f64() >= 30.5, "tab bar height {tabs:?}");
    assert!(
        switcher.right() <= view.width + px(0.5),
        "작업 전환 inside the window: {switcher:?} / {view:?}"
    );
    assert!(
        current.right() <= switcher.left(),
        "the current tab is not under 작업 전환: {current:?} {switcher:?}"
    );
    assert!(
        current.size.height.to_f64() >= 28.,
        "full-height tab: {current:?}"
    );
    // Hidden tabs are counted, not clipped.
    let r = rail(&mut h);
    let total = h.cx.update(|_, cx| r.read(cx).tabs.len());
    assert!(total > 6, "{total} tabs");
}

/// 터미널 on a task with no panel open: its running terminals open in one
/// host update (reverse task order, so the first ends in front), then the
/// rail lays the task out fresh and focuses the first group's panel. No
/// shell is started: the task has running terminals. (Before A-T2b the
/// follow-up updated the rail from inside the rail's own deferred update.)
#[test]
fn terminal_button_opens_the_tasks_running_terminals_and_focuses_the_first() {
    let mut h = Harness::new("rail-terminal-button", seed);
    set_layout(&mut h, "focus-rail");
    assert!(panels(&mut h).is_empty(), "nothing open yet");
    assert_eq!(
        current(&mut h).as_deref(),
        Some("sample/로그인-수정"),
        "no panel open: the first task"
    );
    let calls_before = h.backend.calls.len();
    let r = rail(&mut h);
    h.cx.update(|window, cx| {
        r.update(cx, |r, cx| r.press(super::RailButton::Terminal, window, cx))
    });
    h.settle();
    let mut open: Vec<String> = panels(&mut h).into_iter().map(|(t, _)| t).collect();
    open.sort();
    assert_eq!(open, ["w1", "w2", "w3"], "the task's running terminals");
    let mut s = shown(&mut h);
    s.sort();
    assert_eq!(s, ["w1", "w2", "w3"], "the rail shows them");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("w1"),
        "focus in the first group's panel"
    );
    assert!(
        h.backend.calls[calls_before..]
            .iter()
            .all(|c| !c.starts_with("terminal.run")),
        "no shell started: {:?}",
        &h.backend.calls[calls_before..]
    );
}
