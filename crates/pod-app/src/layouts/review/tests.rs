//! 리뷰 중심 layout: the pure model (plain tests). The live layout on GPUI's
//! headless platform is in `layouts::review_qa`.

// Not `super::*` at the top: gpui's glob exports its own `test` attribute.
use super::{
    MAX_TILES, Nav, Region, ReviewModel, TaskRow, Who, approve_label, branch_line, build_rows,
    next_region, review_comments_drawer, review_files_fit, row_name, task_gates, task_terminals,
    tiles,
};
use crate::source::TestBackend;
use core::prelude::v1::test;

use super::copy;
use super::model::{
    Accent, Canvas, RowKey, Selected, accent, approve_choices, canvas_width, dock_rose,
    follow_focus, follow_hidden, header_height, island_gap, list_key, review_opened, row_badges,
    row_height, row_key, strip_height,
};
use crate::vm::Liveness;

fn gate(id: &str, terminal: Option<&str>, task: Option<&str>) -> podd::protocol::GateInfo {
    podd::protocol::GateInfo {
        id: id.into(),
        title: "clippy 실행".into(),
        message: "명령 실행 승인이 필요합니다".into(),
        task: task.map(Into::into),
        group: None,
        terminal: terminal.map(Into::into),
        state: podd::protocol::GateState::Pending,
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

/// ui-05: at 960 logical (150% monitor) the canvas is 960 − 240; the file list
/// gives way so diff (320) + comments (340) fit.
#[test]
fn file_list_gives_way_before_the_comment_column() {
    assert!(!review_files_fit(960. - 240.));
    assert!(review_files_fit(1440. - 240.));
    assert!(review_files_fit(910.) && !review_files_fit(909.));
    assert!(!review_comments_drawer(560.) && review_comments_drawer(559.));
}

/// ui-04: a Claude session started from PowerShell is "Claude", as in kanban/dense.
#[test]
fn tiles_name_the_agent_kind_not_the_program() {
    let task = TestBackend::task("pod/a", "작업", "pod", &["w0", "s0"]);
    let mut claude = TestBackend::terminal("w0", "Worker 0", Some("pod/a"), "working");
    claude.program = "powershell".into();
    claude.status.as_mut().unwrap().agent = Some("claude".into());
    let mut shell = TestBackend::terminal("s0", "셸", Some("pod/a"), "idle");
    shell.program = "powershell".into();
    let tasks = [task];
    assert_eq!(
        Who::of(&claude, &tasks, "Worker 0".into()).title(),
        "Claude · Worker 0"
    );
    assert_eq!(
        Who::of(&claude, &tasks, "Worker 0".into()).kind,
        crate::agent_kind::identify_in(&claude, &tasks).kind,
        "same rule as other layouts"
    );
    assert!(
        !Who::of(&shell, &tasks, "셸".into())
            .title()
            .contains("Powershell")
    );
}

fn sample() -> Vec<TaskRow> {
    let mut a = TestBackend::task("pod/a", "넓은 글자 정렬", "pod", &["a1", "gone", "a2"]);
    a.base = Some("main".into());
    a.review_open = 1;
    let b = TestBackend::task("pod/b", "설정 화면", "pod", &["b1"]);
    let c = TestBackend::task("pod/c", "빈 작업", "pod", &[]);
    let terms = vec![
        TestBackend::terminal("a1", "구현", Some("pod/a"), "working"),
        TestBackend::terminal("a2", "리뷰", Some("pod/a"), "blocked"),
        TestBackend::terminal("a3", "셸", Some("pod/a"), "idle"),
        TestBackend::terminal("b1", "문서", Some("pod/b"), "done"),
    ];
    build_rows(
        &[a, b, c],
        &terms,
        &[
            gate("g1", Some("a2"), None),
            gate("g2", None, Some("pod/b")),
            gate("g3", Some("zz"), None),
        ],
    )
}

#[test]
fn rows_reuse_workspace_tasks_terminals_and_gates() {
    let rows = sample();
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0].terminals,
        ["a1", "a2", "a3"],
        "task order first, unknown ids skipped, task_id members appended"
    );
    assert_eq!(
        (rows[0].gates, rows[1].gates, rows[2].gates),
        (1, 1, 0),
        "terminal and task gates; others ignored"
    );
    assert_eq!(rows[0].branch.as_deref(), Some("pod/넓은 글자 정렬"));
    let name = row_name(&rows[0]);
    assert!(
        name.starts_with("넓은 글자 정렬, pod/넓은 글자 정렬 → main, 상태: ")
            && name.contains("의견 1개")
            && name.ends_with("승인 대기 1개"),
        "{name}"
    );
    assert_eq!(branch_line(None, Some("main")), "브랜치 없음");
    assert_eq!(
        (approve_label(None), approve_label(Some("빠르게"))),
        ("승인".to_string(), "승인: 빠르게".to_string()),
        "a gate with options is approved with a choice"
    );
    let ids = task_terminals(&TestBackend::task("x", "x", "r", &["q"]), &[]);
    assert!(ids.is_empty());
    assert!(task_gates("pod/c", &[], &[gate("g", None, Some("pod/a"))]).is_empty());
}

#[test]
fn at_most_four_terminals_are_docked() {
    let ids: Vec<String> = (0..6).map(|i| format!("t{i}")).collect();
    let (shown, hidden) = tiles(&ids);
    assert_eq!((shown.len(), hidden), (MAX_TILES, 2));
    assert_eq!(tiles(&ids[..2]), (ids[..2].to_vec(), 0));
}

#[test]
fn switching_tasks_switches_the_canvas_and_resets_the_expanded_terminal() {
    let rows = sample();
    let mut m = ReviewModel::default();
    m.settle(&rows, Some("pod/b"));
    assert_eq!(
        m.selected.as_deref(),
        Some("pod/b"),
        "the task the user was in"
    );
    m.selected = Some("gone".into());
    m.settle(&rows, None);
    assert_eq!(
        m.selected.as_deref(),
        Some("pod/a"),
        "a removed task falls back to the first"
    );
    m.toggle_expand("a2");
    assert_eq!(m.expanded.as_deref(), Some("a2"));
    m.show_dock = true;
    assert!(!m.select("pod/a"), "same task: no switch");
    assert!(!m.show_dock, "selecting a task shows its Review");
    assert_eq!(
        m.expanded.as_deref(),
        Some("a2"),
        "same task keeps the expanded terminal"
    );
    assert!(m.select("pod/b"));
    assert_eq!(m.expanded, None, "another task: its own tiles");
    m.toggle_expand("b1");
    m.toggle_expand("b1");
    assert_eq!(m.expanded, None, "toggle back");
    m.expanded = Some("a1".into());
    m.settle(&rows, None);
    assert_eq!(
        m.expanded, None,
        "an expanded terminal of another task is dropped"
    );
}

#[test]
fn list_keys() {
    let rows = sample();
    let mut m = ReviewModel::default();
    assert_eq!(
        m.nav(&rows, Nav::Down).as_deref(),
        Some("pod/a"),
        "no selection: first"
    );
    m.selected = Some("pod/a".into());
    assert_eq!(m.nav(&rows, Nav::Down).as_deref(), Some("pod/b"));
    assert_eq!(m.nav(&rows, Nav::Up), None);
    assert_eq!(m.nav(&rows, Nav::End).as_deref(), Some("pod/c"));
    m.selected = Some("pod/c".into());
    assert_eq!(m.nav(&rows, Nav::Down), None);
    assert_eq!(m.nav(&rows, Nav::Home).as_deref(), Some("pod/a"));
    assert_eq!(m.nav(&[], Nav::End), None);
}

#[test]
fn f6_cycles_tasks_canvas_and_each_docked_terminal() {
    let mut m = ReviewModel::default();
    let tiles = vec!["a1".to_string(), "a2".to_string()];
    let r = m.regions(&tiles);
    assert_eq!(
        r,
        [
            Region::Tasks,
            Region::Canvas,
            Region::Terminal("a1".into()),
            Region::Terminal("a2".into())
        ]
    );
    assert_eq!(next_region(&r, Some(&Region::Tasks), true), Region::Canvas);
    assert_eq!(
        next_region(&r, Some(&Region::Terminal("a2".into())), true),
        Region::Tasks,
        "wraps"
    );
    assert_eq!(
        next_region(&r, Some(&Region::Tasks), false),
        Region::Terminal("a2".into()),
        "Shift+F6 wraps back"
    );
    assert_eq!(
        next_region(&r, None, true),
        Region::Tasks,
        "focus outside: start at the tasks"
    );
    m.strip_open = false;
    assert_eq!(
        m.regions(&tiles),
        [Region::Tasks, Region::Canvas],
        "collapsed strip: no terminal stops"
    );
    m.strip_open = true;
    m.show_dock = true;
    assert_eq!(
        m.regions(&tiles),
        [Region::Tasks, Region::Canvas],
        "panel view hides the tiles"
    );
    m.toggle_expand("a2");
    assert!(!m.show_dock && m.strip_open);
    assert_eq!(
        m.regions(&tiles),
        [Region::Tasks, Region::Terminal("a2".into())],
        "an expanded terminal covers the canvas"
    );
}

// ───────────────── helpers pulled out of the view and its render ─────────────────

#[test]
fn row_keys_are_unmodified_and_the_list_takes_arrows_only() {
    assert_eq!(row_key("down", false), Some(RowKey::Nav(Nav::Down)));
    assert_eq!(row_key("up", false), Some(RowKey::Nav(Nav::Up)));
    assert_eq!(row_key("home", false), Some(RowKey::Nav(Nav::Home)));
    assert_eq!(row_key("end", false), Some(RowKey::Nav(Nav::End)));
    assert_eq!(row_key("enter", false), Some(RowKey::Open));
    assert_eq!(
        row_key("down", true),
        None,
        "modified keys are not the row's"
    );
    assert_eq!(row_key("enter", true), None);
    assert_eq!(row_key("space", false), None);
    assert!(list_key("down") && list_key("end"));
    assert!(!list_key("enter"), "the list itself has no row to open");
}

#[test]
fn a_terminal_focused_elsewhere_brings_its_task_in() {
    let rows = sample();
    let mut m = ReviewModel::default();
    m.settle(&rows, None);
    // Shown already (docked tile of the selected task): nothing changes.
    let before = m.clone();
    assert!(!follow_hidden(&mut m, &rows, "a1", "pod/a"));
    assert_eq!(m, before);
    // The 패널 view hides it: back to the Review with the tile in view.
    m.show_dock = true;
    m.strip_open = false;
    assert!(follow_hidden(&mut m, &rows, "a1", "pod/a"));
    assert!(!m.show_dock && m.strip_open && m.expanded.is_none());
    // Another task's terminal: that task, expanded only when it is not a tile.
    assert!(follow_hidden(&mut m, &rows, "b1", "pod/b"));
    assert_eq!(
        (m.selected.as_deref(), m.expanded.as_deref()),
        (Some("pod/b"), None)
    );
    assert!(follow_hidden(&mut m, &rows, "zz", "pod/a"));
    assert_eq!(
        (m.selected.as_deref(), m.expanded.as_deref()),
        (Some("pod/a"), Some("zz")),
        "not one of the tiles: over the canvas"
    );
    // An expanded terminal counts as shown.
    assert!(!follow_hidden(&mut m, &rows, "zz", "pod/a"));

    // The workspace's focused terminal: only a change since the last frame counts.
    let mut m = ReviewModel::default();
    let mut seen = None;
    follow_focus(&mut m, &mut seen, Some("b1"), Some("pod/b"), &rows);
    assert_eq!(
        (seen.as_deref(), m.selected.as_deref()),
        (Some("b1"), Some("pod/b"))
    );
    m.select("pod/a");
    follow_focus(&mut m, &mut seen, Some("b1"), Some("pod/b"), &rows);
    assert_eq!(
        m.selected.as_deref(),
        Some("pod/a"),
        "same focus: no switch"
    );
    follow_focus(&mut m, &mut seen, Some("zz"), Some("pod/b"), &rows);
    assert_eq!(
        (m.selected.as_deref(), m.expanded.as_deref()),
        (Some("pod/b"), Some("zz"))
    );
    follow_focus(&mut m, &mut seen, Some("t9"), None, &rows);
    assert_eq!(
        (seen.as_deref(), m.selected.as_deref()),
        (Some("t9"), Some("pod/b")),
        "a terminal without a task only moves the seen focus"
    );
    follow_focus(&mut m, &mut seen, None, None, &rows);
    assert_eq!(seen, None);
}

#[test]
fn reviews_and_panels_opened_meanwhile_take_the_canvas() {
    let seen: std::collections::HashSet<u32> = [1].into();
    let reviews = [(1, "pod/a".to_string()), (2, "pod/b".to_string())];
    assert_eq!(
        review_opened(usize::MAX, &seen, &reviews),
        None,
        "first frame"
    );
    assert_eq!(review_opened(0, &seen, &reviews), Some("pod/b"));
    assert_eq!(review_opened(3, &seen, &reviews[..1]), None);
    assert!(!dock_rose(usize::MAX, 4), "first frame");
    assert!(dock_rose(1, 2) && !dock_rose(2, 2) && !dock_rose(2, 1));

    let mut m = ReviewModel {
        selected: Some("pod/a".into()),
        expanded: Some("a1".into()),
        ..ReviewModel::default()
    };
    assert_eq!(m.canvas(), Canvas::Terminal("a1".into()));
    assert!(!m.strip_tiles(), "an expanded terminal hides the tiles");
    m.show_panels(true);
    assert_eq!((m.canvas(), m.expanded.as_deref()), (Canvas::Dock, None));
    assert!(!m.strip_tiles());
    m.show_panels(false);
    assert_eq!(m.canvas(), Canvas::Review("pod/a".into()));
    assert!(m.strip_tiles());
    m.expanded = Some("a1".into());
    m.show_panels(false);
    assert_eq!(
        m.expanded.as_deref(),
        Some("a1"),
        "the 리뷰 tab keeps an expanded terminal"
    );
    m.selected = None;
    m.expanded = None;
    assert_eq!(m.canvas(), Canvas::NoTask);
}

#[test]
fn answer_and_entering_keep_the_terminal_in_view() {
    let rows = sample();
    let mut m = ReviewModel {
        selected: Some("pod/a".into()),
        strip_open: false,
        show_dock: true,
        ..ReviewModel::default()
    };
    let tiles = m.tiles_in(&rows);
    assert_eq!(tiles, ["a1", "a2", "a3"]);
    m.answer(&tiles, "a2");
    assert!(m.strip_open && !m.show_dock && m.expanded.is_none());
    m.answer(&tiles, "zz");
    assert_eq!(
        m.expanded.as_deref(),
        Some("zz"),
        "not docked: over the canvas"
    );

    let mut m = ReviewModel {
        show_dock: true,
        strip_open: false,
        ..ReviewModel::default()
    };
    m.enter_on_terminal(&rows, "b1", "pod/b");
    assert_eq!(
        (m.selected.as_deref(), m.expanded.as_deref()),
        (Some("pod/b"), None)
    );
    assert!(m.strip_open && !m.show_dock);
    m.enter_on_terminal(&rows, "a9", "gone");
    assert_eq!(
        (m.selected.as_deref(), m.expanded.as_deref()),
        (Some("pod/a"), Some("a9")),
        "a gone task settles on the first; the terminal goes over the canvas"
    );
}

#[test]
fn header_parts_asks_and_gate_choices() {
    let mut a = TestBackend::task("pod/a", "작업", "pod", &["a1", "a2", "a3"]);
    a.base = Some("main".into());
    let tasks = [a];
    let mut stopped = TestBackend::terminal("a3", "멈춤", Some("pod/a"), "blocked");
    stopped.live = Liveness::Exited {
        code: Some(0),
        killed: false,
    };
    let terms = vec![
        TestBackend::terminal("a1", "구현", Some("pod/a"), "blocked"),
        TestBackend::terminal("a2", "리뷰", Some("pod/a"), "blocked"),
        stopped,
    ];
    let pending = [gate("g1", Some("a2"), None), gate("g2", Some("x"), None)];
    let p = Selected::of(Some("pod/a"), &tasks, &terms, &pending);
    assert_eq!(p.task.as_ref().map(|k| k.id.as_str()), Some("pod/a"));
    assert_eq!(
        p.terms.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["a1", "a2", "a3"]
    );
    assert_eq!(p.gates.len(), 1);
    assert_eq!(
        p.asks().iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["a1"],
        "running, blocked and without a gate of its own"
    );
    assert!(p.shows_gate_rows());
    let none = Selected::of(None, &tasks, &terms, &pending);
    assert_eq!(none, Selected::default());
    assert!(!none.shows_gate_rows());
    // Only a stopped agent still reporting blocked: the (empty) gate rows show.
    let quiet = Selected::of(Some("pod/a"), &tasks, &terms[2..], &[]);
    assert!(quiet.asks().is_empty() && quiet.shows_gate_rows());

    let mut g = gate("g3", None, Some("pod/a"));
    assert_eq!(approve_choices(&g), [None]);
    g.options = vec!["빠르게".into(), "꼼꼼하게".into()];
    assert_eq!(
        approve_choices(&g),
        [Some("빠르게".to_string()), Some("꼼꼼하게".to_string())]
    );

    let rows = sample();
    let badges = row_badges(&rows, &terms);
    assert_eq!(badges.len(), 3);
    assert!(badges["pod/c"].is_none(), "no terminals: no badge");
}

#[test]
fn tile_accent_marks_blocked_and_failed_terminals() {
    let mut t = TestBackend::terminal("t", "이름", None, "blocked");
    assert_eq!(accent(&t), Some(Accent::Warning));
    t = TestBackend::terminal("t", "이름", None, "error");
    assert_eq!(accent(&t), Some(Accent::Danger));
    t = TestBackend::terminal("t", "이름", None, "working");
    assert_eq!(accent(&t), None);
    t.live = Liveness::Exited {
        code: Some(2),
        killed: false,
    };
    assert_eq!(accent(&t), Some(Accent::Danger), "non-zero exit");
    t.live = Liveness::Exited {
        code: Some(2),
        killed: true,
    };
    assert_eq!(accent(&t), None, "killed by the user");
    t = TestBackend::terminal("t", "이름", None, "blocked");
    t.live = Liveness::Exited {
        code: Some(0),
        killed: false,
    };
    assert_eq!(accent(&t), None, "a stopped agent is not waiting");
    t.live = Liveness::Lost;
    assert_eq!(accent(&t), Some(Accent::Danger));
}

#[test]
fn size_formulas_are_the_render_values() {
    for (line, border) in [(20., 1.), (30., 0.), (42., 2.)] {
        assert_eq!(
            row_height(line, border),
            line * 2. + 2. + 8. * 2. + border * 2. + 4.
        );
    }
    assert_eq!(header_height(32., 20.), 32., "the style's panel header");
    assert_eq!(header_height(32., 30.), 40., "grows with the font");
    assert_eq!(island_gap(true, 4.), 6.);
    assert_eq!(island_gap(true, 10.), 10.);
    assert_eq!(island_gap(false, 10.), 0.);
    assert_eq!(canvas_width(1440., 240., 6.), 1440. - 240. - 18.);
    assert_eq!(strip_height(true, 960., 32.), 288.);
    assert_eq!(strip_height(true, 400., 32.), 184., "two headers and 120");
    assert_eq!(strip_height(false, 960., 32.), 32.);
}

#[test]
fn copy_keeps_the_old_formats() {
    assert_eq!(copy::list_name(3), "작업, 3개");
    assert_eq!(
        copy::row_sub("pod/a → main", "작업 1"),
        "pod/a → main · 작업 1"
    );
    assert_eq!(
        copy::tile_name("Claude · W", "작업 중"),
        "Claude · W, 작업 중"
    );
    assert_eq!(copy::gate_name("제목", "내용"), "승인 대기: 제목, 내용");
    assert_eq!(
        copy::ask("Claude · W", "승인 필요"),
        "Claude · W — 승인 필요"
    );
    assert_eq!(copy::terminals_name(2), "터미널, 2개");
    assert_eq!(copy::terminals_title(2), "터미널 2");
    assert_eq!(copy::hidden_tiles(2), "(+2)");
    assert_eq!(copy::branch_to_base("pod/a", "main"), "pod/a → main");
}
