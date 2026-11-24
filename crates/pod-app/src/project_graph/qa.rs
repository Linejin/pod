//! The project agent graph in the real workspace on GPUI's headless platform:
//! the sidebar repository action (pointer and keyboard), one panel that
//! follows the chosen repository, drawn nodes and connectors, terminal
//! navigation, source events, and the empty / unknown / disconnected states.

use core::prelude::v1::test;
use gpui_kit::*;

use crate::native_qa::Harness;
use crate::source::{SourceEvent, TestBackend};
use crate::vm::{ConnState, RepoVm, TaskVm, TerminalVm};

fn agent(
    id: &str,
    name: &str,
    task: Option<&str>,
    argv: &str,
    state: &str,
    created: u64,
) -> TerminalVm {
    let mut t = TestBackend::terminal(id, name, task, state);
    t.argv = vec![argv.into()];
    t.program = argv.into();
    t.created_ms = created;
    t
}

fn task(id: &str, title: &str, repo: &str, agent: &str, terminals: &[&str]) -> TaskVm {
    let mut k = TestBackend::task(id, title, repo, terminals);
    k.agent = Some(agent.into());
    k
}

fn repo(name: &str) -> RepoVm {
    RepoVm {
        name: name.into(),
        path: format!("C:/fixtures/{name}"),
        default_base: Some("main".into()),
    }
}

/// `sample`: Claude c1 (+ shell s1) leads; Codex x1's task was created from
/// c1; Claude r1 was `pod run` from x1 and has no status yet. `sample-other`
/// (same prefix, cwd under `sample`): Claude o1. `empty`: registered, no task.
fn seed_graph(b: &mut TestBackend) {
    b.repos
        .extend([repo("empty"), repo("sample"), repo("sample-other")]);
    let mut child = task("sample/child", "검토", "sample", "codex", &["x1"]);
    child.parent_terminal = Some("c1".into());
    b.tasks.extend([
        task("sample/lead", "리드", "sample", "claude", &["c1", "s1"]),
        child,
        task(
            "sample-other/else",
            "다른",
            "sample-other",
            "claude",
            &["o1"],
        ),
    ]);
    let mut s1 = TestBackend::terminal("s1", "빌드 셸", Some("sample/lead"), "idle");
    s1.created_ms = 2;
    let mut r1 = agent("r1", "실행 봇", None, "claude", "idle", 4);
    r1.parent_terminal = Some("x1".into());
    r1.parent_task = Some("sample/child".into());
    r1.status = None;
    let mut o1 = agent(
        "o1",
        "sample 리더",
        Some("sample-other/else"),
        "claude",
        "working",
        5,
    );
    o1.cwd = "C:/fixtures/sample".into();
    b.terminals.extend([
        agent("c1", "리더", Some("sample/lead"), "claude", "working", 1),
        s1,
        agent("x1", "검토자", Some("sample/child"), "codex", "blocked", 3),
        r1,
        o1,
    ]);
}

fn center(b: Bounds<Pixels>) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width / 2.,
        b.origin.y + b.size.height / 2.,
    )
}

fn click(h: &mut Harness, mark: &str) {
    let b = h.bounds(mark).unwrap_or_else(|| panic!("{mark} is drawn"));
    h.cx.simulate_click(center(b), Modifiers::none());
    h.settle();
}

/// (repo, panel entity id, selected) of the open graph panel.
fn panel(h: &mut Harness) -> Option<(String, EntityId, Option<String>)> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx).project_graph_panel().map(|p| {
            let r = p.read(cx);
            (r.repo.clone(), p.entity_id(), r.selected.clone())
        })
    })
}

fn graph_leaves(h: &mut Harness) -> usize {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let d = ws.read(cx).dock.read(cx).dump(cx);
        serde_json::to_string(&d)
            .unwrap_or_default()
            .matches(super::PROJECT_GRAPH_PANEL_NAME)
            .count()
    })
}

fn drawn(h: &mut Harness, ids: &[&str]) -> Vec<String> {
    ids.iter()
        .filter(|id| h.bounds(&format!("graph-node:{id}")).is_some())
        .map(|s| s.to_string())
        .collect()
}

fn panels_of(h: &mut Harness, id: &str) -> usize {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_panels()
            .iter()
            .filter(|p| p.read(cx).terminal_id == id)
            .count()
    })
}

fn collapsed(h: &mut Harness) -> Vec<String> {
    h.cx.update(|_, cx| {
        cx.global::<crate::theme::Settings>()
            .collapsed
            .iter()
            .cloned()
            .collect()
    })
}

const ALL: [&str; 5] = ["c1", "s1", "x1", "r1", "o1"];

#[test]
fn repo_action_opens_one_graph_that_switches_repository() {
    let mut h = Harness::new("project-graph-switch", seed_graph);
    for r in ["empty", "sample", "sample-other"] {
        assert!(
            h.bounds(&format!("graph-action:{r}")).is_some(),
            "{r}: the repository row shows the action"
        );
    }
    let before = collapsed(&mut h);
    click(&mut h, "graph-action:sample");
    let (repo, first, _) = panel(&mut h).expect("graph panel opened");
    assert_eq!(repo, "sample");
    assert_eq!(
        collapsed(&mut h),
        before,
        "the action does not fold the row"
    );
    assert_eq!(
        drawn(&mut h, &ALL),
        ["c1", "x1", "r1"],
        "agents of sample only"
    );
    for e in ["c1->x1", "x1->r1"] {
        let line = h
            .bounds(&format!("graph-edge:{e}"))
            .unwrap_or_else(|| panic!("{e}: connector drawn"));
        let (from, to) = e.split_once("->").unwrap();
        let parent = h.bounds(&format!("graph-node:{from}")).unwrap();
        let child = h.bounds(&format!("graph-node:{to}")).unwrap();
        assert!(
            line.top() >= parent.bottom() - px(1.),
            "{e}: below the parent"
        );
        assert!(
            (line.bottom() - child.top()).abs() <= px(2.),
            "{e}: ends at the child {line:?} {child:?}"
        );
        assert!(
            line.left() >= child.left() && line.right() <= child.right(),
            "{e}: drops into the child"
        );
        assert!(
            h.bounds(&format!("graph-link:{e}")).is_some(),
            "{e}: as text"
        );
    }
    assert_eq!(graph_leaves(&mut h), 1);
    let ws = h.ws.clone();
    let focus = h.cx.update(|window, cx| {
        let p = ws.read(cx).project_graph_panel().unwrap();
        p.read(cx).focus_handle(cx).contains_focused(window, cx)
    });
    assert!(focus, "the graph takes focus");

    click(&mut h, "graph-action:sample-other");
    let (repo, second, selected) = panel(&mut h).expect("still open");
    assert_eq!(
        (repo.as_str(), second),
        ("sample-other", first),
        "same panel, new subject"
    );
    assert_eq!(selected, None);
    assert_eq!(
        drawn(&mut h, &ALL),
        ["o1"],
        "a shared name prefix and cwd do not leak"
    );
    assert!(h.bounds("graph-edge:c1->x1").is_none());
    assert_eq!(graph_leaves(&mut h), 1, "one graph panel");

    click(&mut h, "graph-action:empty");
    assert!(h.bounds("graph-empty").is_some(), "registered, no agents");
    assert!(h.bounds("graph-unknown").is_none());
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_project_graph("gone", window, cx)));
    h.settle();
    assert!(
        h.bounds("graph-unknown").is_some(),
        "not a registered repository"
    );

    // The agent tree is still there and separate.
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_agent_tree(window, cx)));
    h.settle();
    let both = h.cx.update(|_, cx| {
        let w = ws.read(cx);
        (
            w.orch
                .tree_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_some(),
            w.project_graph_panel().is_some(),
        )
    });
    assert_eq!(both, (true, true));
}

#[test]
fn the_action_is_reached_with_tab_and_opened_with_enter() {
    let mut h = Harness::new("project-graph-keyboard", seed_graph);
    let filter = {
        let ws = h.ws.clone();
        h.cx.update(|_, cx| ws.read(cx).sidebar_filter(cx).focus_handle(cx))
    };
    h.cx.update(|window, cx| window.focus(&filter, cx));
    h.settle();
    let before = collapsed(&mut h);
    // Search field → the first repository row's action ("empty").
    h.press("tab");
    h.press("enter");
    let (repo, ..) = panel(&mut h).expect("Enter on the action opens the graph");
    assert_eq!(repo, "empty");
    assert_eq!(collapsed(&mut h), before, "Enter does not fold the row");
}

#[test]
fn selecting_a_node_opens_or_focuses_its_terminal() {
    let mut h = Harness::new("project-graph-navigate", seed_graph);
    click(&mut h, "graph-action:sample");
    click(&mut h, "graph-node:x1");
    assert_eq!(h.focused_terminal().as_deref(), Some("x1"));
    assert_eq!(panels_of(&mut h, "x1"), 1);
    click(&mut h, "graph-node:x1");
    assert_eq!(panels_of(&mut h, "x1"), 1, "focused, not opened twice");

    // Keyboard: back in the graph, Home / ↓ / ↑ / Enter.
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_project_graph("sample", window, cx)));
    h.settle();
    h.press("home");
    assert_eq!(panel(&mut h).unwrap().2.as_deref(), Some("c1"));
    h.press("down");
    h.press("down");
    assert_eq!(panel(&mut h).unwrap().2.as_deref(), Some("r1"));
    h.press("up");
    assert_eq!(panel(&mut h).unwrap().2.as_deref(), Some("x1"));
    h.press("home");
    h.press("enter");
    assert_eq!(h.focused_terminal().as_deref(), Some("c1"));
    assert_eq!(panels_of(&mut h, "c1"), 1);
    assert_eq!(panels_of(&mut h, "s1"), 0, "the shell is not a node");
}

#[test]
fn a_node_podd_no_longer_lists_is_shown_but_not_opened() {
    let mut h = Harness::new("project-graph-stale-node", |b| {
        seed_graph(b);
        b.tasks.push(task(
            "sample/stale",
            "남은 작업",
            "sample",
            "codex",
            &["m1"],
        ));
    });
    click(&mut h, "graph-action:sample");
    click(&mut h, "graph-node:m1");
    assert_eq!(panels_of(&mut h, "m1"), 0);
    assert!(
        h.bounds("graph-notice").is_some(),
        "says why nothing opened"
    );
}

#[test]
fn source_events_update_the_graph_in_place() {
    let mut h = Harness::new("project-graph-events", seed_graph);
    click(&mut h, "graph-action:sample");
    let at = |h: &mut Harness| {
        ["c1", "x1", "r1"].map(|id| h.bounds(&format!("graph-node:{id}")).expect("node"))
    };
    let before = at(&mut h);
    let ws = h.ws.clone();
    let order = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            super::snapshot(ws.read(cx), "sample")
                .nodes
                .iter()
                .map(|n| n.id.clone())
                .collect::<Vec<_>>()
        })
    };
    let ids = order(&mut h);
    h.backend.set_status("c1", "blocked");
    h.backend.set_status("x1", "done");
    h.backend.set_status("r1", "working");
    h.settle();
    assert_eq!(order(&mut h), ids, "status-only events keep the order");
    assert_eq!(at(&mut h), before, "and the place of every node");
    let states = h.cx.update(|_, cx| {
        let w = ws.read(cx);
        ["c1", "x1", "r1"].map(|id| {
            w.terminal(id)
                .and_then(|t| t.status.clone())
                .map(|s| s.state)
        })
    });
    use crate::vm::AgentState::*;
    assert_eq!(states, [Some(Blocked), Some(Done), Some(Working)]);

    // A new agent of this project arrives with the next terminal list.
    let mut n1 = agent("n1", "새 에이전트", None, "claude", "working", 9);
    n1.parent_terminal = Some("c1".into());
    n1.parent_task = Some("sample/lead".into());
    h.backend.terminals.push(n1);
    h.backend.publish();
    h.settle();
    assert!(h.bounds("graph-node:n1").is_some());
    assert!(h.bounds("graph-edge:c1->n1").is_some());

    // Daemon lost: the last data stays, marked stale; back: the banner goes.
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Conn(ConnState::Reconnecting {
            attempt: 1,
            error: "pipe closed".into(),
        }));
    h.settle();
    assert!(h.bounds("graph-disconnected").is_some());
    assert!(h.bounds("graph-node:c1").is_some(), "last known graph kept");
    h.backend.connect();
    h.settle();
    assert!(h.bounds("graph-disconnected").is_none());
}

#[test]
fn an_open_graph_follows_the_repository_selected_in_the_sidebar() {
    let mut h = Harness::new("project-graph-follow", seed_graph);
    click(&mut h, "graph-action:sample");
    let sidebar = h.sidebar_focus();
    h.cx.update(|window, cx| window.focus(&sidebar, cx));
    h.settle();
    // Rows: repo:empty, repo:sample, task:sample/lead, terminal:c1,
    // terminal:s1, task:sample/child, terminal:x1, repo:sample-other …
    for _ in 0..8 {
        h.press("down");
    }
    let ws = h.ws.clone();
    let selected =
        h.cx.update(|_, cx| ws.read(cx).sidebar.read(cx).selected.clone());
    assert_eq!(selected.as_deref(), Some("repo:sample-other"));
    assert_eq!(panel(&mut h).unwrap().0, "sample-other");
    assert!(h.is_focused(&sidebar), "following does not take focus");
}

// ---- native follow-up (2026-11-24 QA G1-G3) --------------------------------

/// The QA's graph-a shape: four root agents t1..t4 and t7 started from t1.
fn seed_wide(b: &mut TestBackend) {
    b.repos.extend([repo("wide"), repo("narrow")]);
    let ids = ["t1", "t2", "t3", "t4"];
    for (i, id) in ids.iter().enumerate() {
        let name = format!("wide/{id}");
        b.tasks
            .push(task(&name, &format!("작업 {id}"), "wide", "claude", &[*id]));
        b.terminals.push(agent(
            id,
            &format!("에이전트 {id}"),
            Some(&name),
            "claude",
            "working",
            i as u64 + 1,
        ));
    }
    let mut child = task("wide/t7", "하위", "wide", "codex", &["t7"]);
    child.parent_terminal = Some("t1".into());
    b.tasks.push(child);
    b.tasks
        .push(task("narrow/t5", "다른", "narrow", "claude", &["t5"]));
    b.terminals.extend([
        agent("t7", "하위 에이전트", Some("wide/t7"), "codex", "idle", 7),
        agent("t5", "B 에이전트", Some("narrow/t5"), "claude", "idle", 5),
    ]);
}

fn inside(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let e = px(0.5);
    inner.left() >= outer.left() - e
        && inner.top() >= outer.top() - e
        && inner.right() <= outer.right() + e
        && inner.bottom() <= outer.bottom() + e
}

/// A test-owned config dir (layout.json, settings) for the life of the guard.
struct OwnConfig {
    _dir: tempfile::TempDir,
}

impl OwnConfig {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("config dir");
        crate::layout::set_test_config_dir(Some(dir.path().to_path_buf()));
        Self { _dir: dir }
    }
}

impl Drop for OwnConfig {
    fn drop(&mut self) {
        crate::layout::set_test_config_dir(None);
    }
}

/// Settings › 모양: the layout, as the user commits it.
fn set_layout(h: &mut Harness, layout: &str) {
    let ws = h.ws.downgrade();
    h.cx.update(|window, cx| {
        let next = crate::theme::Settings {
            layout: layout.into(),
            ..cx.global::<crate::theme::Settings>().clone()
        };
        crate::chrome::commit_settings(next, &ws, window, cx);
    });
    h.settle();
}

fn selected_row(h: &mut Harness) -> Option<String> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| ws.read(cx).sidebar.read(cx).selected.clone())
}

/// G1: every pixel row the action occupies (what UIA advertises) is inside
/// its tree row and takes the click; the click neither folds nor selects the
/// row, and the row beside the action still selects the repository.
fn assert_full_action_hit_area(h: &mut Harness, case: &str) {
    let row_h = pod_ui::themes::metrics().density.row;
    let mut spans = Vec::new();
    for r in ["sample", "sample-other"] {
        // A qa mark is the padding box; the button's 2 px border is part of
        // its box, of its hitbox and of the rectangle UIA advertises.
        let inner = h.bounds(&format!("graph-action:{r}")).expect("action");
        let border = px(2.);
        let action = Bounds::new(
            point(inner.left() - border, inner.top() - border),
            size(
                inner.size.width + border * 2.,
                inner.size.height + border * 2.,
            ),
        );
        let row = h.bounds(&format!("sidebar-row:repo:{r}")).expect("row");
        println!("[graph-qa] {case}: {r} action {action:?} row {row:?} density row {row_h}");
        assert!(
            inside(action, row),
            "{case}: {r} action {action:?} inside its row {row:?}"
        );
        assert_eq!(
            action.size.height,
            px(pod_ui::hit_area().min(row_h)),
            "{case}: the hit area, or the whole row when that is lower"
        );
        spans.push((r, action));
    }
    let (folded, rows) = (collapsed(h), selected_row(h));
    let height = f32::from(spans[0].1.size.height).round() as usize;
    for dy in 0..height {
        for (r, b) in &spans {
            let at = point(b.left() + b.size.width / 2., b.top() + px(dy as f32 + 0.5));
            h.cx.simulate_click(at, Modifiers::none());
            h.settle();
            assert_eq!(
                panel(h).map(|p| p.0).as_deref(),
                Some(*r),
                "{case}: offset {dy}/{height}: the click reached the {r} action"
            );
        }
    }
    assert_eq!(collapsed(h), folded, "{case}: no click folded a row");
    assert_eq!(selected_row(h), rows, "{case}: nor selected one");
    let row = h.bounds("sidebar-row:repo:sample").unwrap();
    h.cx.simulate_click(
        point(row.left() + px(40.), row.top() + row.size.height / 2.),
        Modifiers::none(),
    );
    h.settle();
    assert_eq!(selected_row(h).as_deref(), Some("repo:sample"), "{case}");
}

#[test]
fn the_repo_action_takes_clicks_across_its_whole_advertised_height() {
    let _config = OwnConfig::new();
    let mut h = Harness::new("project-graph-action-hit-area", seed_graph);
    assert_full_action_hit_area(&mut h, "default density");
}

/// G1 as the native QA saw it: dense density (22 px rows) in dense-details.
/// Ignored in the parallel suite: it switches the process-wide theme metrics
/// every other harness test reads. Run it alone:
/// `cargo test -p pod-app dense_rows -- --ignored --test-threads=1`.
#[test]
#[ignore = "changes process-wide theme metrics; run alone with --test-threads=1"]
fn the_repo_action_fits_dense_rows() {
    let _config = OwnConfig::new();
    let mut h = Harness::new("project-graph-action-dense-rows", seed_graph);
    for layout in ["workbench", "dense-details"] {
        let ws = h.ws.downgrade();
        h.cx.update(|window, cx| {
            let next = crate::theme::Settings {
                density: "dense".into(),
                layout: layout.into(),
                ..cx.global::<crate::theme::Settings>().clone()
            };
            crate::chrome::commit_settings(next, &ws, window, cx);
        });
        h.settle();
        assert_eq!(pod_ui::themes::metrics().density.row, 22.);
        assert_full_action_hit_area(&mut h, layout);
    }
    let ws = h.ws.downgrade();
    h.cx.update(|window, cx| {
        let next = crate::theme::Settings {
            density: "theme".into(),
            layout: "workbench".into(),
            ..cx.global::<crate::theme::Settings>().clone()
        };
        crate::chrome::commit_settings(next, &ws, window, cx);
    });
    h.settle();
}

fn select_t4(h: &mut Harness) {
    h.press("home");
    for _ in 0..3 {
        h.press("right");
    }
    assert_eq!(panel(h).unwrap().2.as_deref(), Some("t4"));
}

/// The graph panel's measured viewport (its box and the part on screen).
fn viewport(h: &mut Harness) -> super::Viewport {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .project_graph_panel()
            .expect("graph open")
            .read(cx)
            .viewport
            .get()
    })
}

/// G2: 822×760, dense-details with details and output open: the keyboard-
/// selected node is drawn inside the visible centre column, both with the
/// dock the graph opens and with a wider (saved) right dock the column clips.
#[test]
fn the_selected_node_stays_in_the_visible_dense_column_at_822() {
    let _config = OwnConfig::new();
    let mut h = Harness::sized(
        "project-graph-dense-822",
        1.0,
        size(px(822.), px(760.)),
        seed_wide,
    );
    set_layout(&mut h, "dense-details");
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_project_graph("wide", window, cx)));
    h.settle();
    let check = |h: &mut Harness, case: &str| {
        select_t4(h);
        let node = h.bounds("graph-node:t4").expect("t4 drawn");
        let column = h.bounds("dense-center").expect("centre column");
        let details = h.bounds("dense-details").expect("details stay open");
        assert!(
            h.bounds("dense-output").is_some(),
            "{case}: output stays open"
        );
        let content = h.bounds("project-graph-content").expect("graph content");
        println!("[graph-qa] {case}: node {node:?} column {column:?} content {content:?}");
        assert!(
            inside(node, column),
            "{case}: t4 {node:?} is inside the visible column {column:?}"
        );
        assert!(node.right() <= details.left(), "{case}: not under details");
        assert!(inside(content, column), "{case}: content {content:?}");
        let vp = viewport(h);
        assert!(
            inside(node, vp.visible),
            "{case}: within the measured viewport"
        );
        vp
    };
    let opened = check(&mut h, "opened in dense");
    assert!(
        opened.area.size.width <= opened.visible.size.width + px(0.5),
        "a dock the graph opens fits the column: {opened:?}"
    );
    // A wider right dock (saved in a roomier layout) overflows the column;
    // the graph lays itself out in the part on screen.
    h.cx.update(|window, cx| {
        let dock = ws.read(cx).dock.clone();
        dock.update(cx, |a, cx| {
            a.set_dock_size(
                gpui_kit::component::dock::DockPlacement::Right,
                px(480.),
                window,
                cx,
            )
        });
    });
    h.settle();
    h.settle();
    let wide = check(&mut h, "saved 480 px dock");
    assert!(
        wide.area.size.width > wide.visible.size.width + px(100.),
        "the dock really is clipped by its column: {wide:?}"
    );
}

/// G2, workbench at the same size: the graph's new dock leaves the centre room
/// and the selected node is fully inside the dock.
#[test]
fn the_workbench_at_822_keeps_the_centre_and_the_selected_node() {
    let _config = OwnConfig::new();
    let mut h = Harness::sized(
        "project-graph-workbench-822",
        1.0,
        size(px(822.), px(760.)),
        seed_wide,
    );
    let ws = h.ws.clone();
    let area =
        h.cx.update(|_, cx| ws.read(cx).dock.read(cx).bounds().size.width);
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_project_graph("wide", window, cx)));
    h.settle();
    let dock = h.cx.update(|_, cx| {
        ws.read(cx)
            .dock
            .read(cx)
            .dock_size(gpui_kit::component::dock::DockPlacement::Right)
    });
    let dock = dock.expect("right dock");
    println!("[graph-qa] dock area {area:?}, graph dock {dock:?}");
    assert!(
        dock <= area / 2. + px(0.5) || dock <= px(268.5),
        "{dock:?} of {area:?}"
    );
    assert!(area - dock >= px(100.), "the centre keeps room");
    select_t4(&mut h);
    let node = h.bounds("graph-node:t4").expect("t4");
    let vp = viewport(&mut h);
    assert!(inside(node, vp.visible), "{node:?} in {vp:?}");
}

/// The repository the saved layout gives the graph leaf.
fn saved_graph_repo() -> Option<String> {
    fn find(s: &gpui_kit::base::dock::PanelState) -> Option<String> {
        if s.panel_name == super::PROJECT_GRAPH_PANEL_NAME {
            return crate::layout::leaf_field(s, "repo").map(str::to_string);
        }
        s.children.iter().find_map(find)
    }
    let dock = crate::layout::load().dock?;
    [dock.right_dock, dock.left_dock, dock.bottom_dock]
        .into_iter()
        .flatten()
        .find_map(|d| find(d.panel()))
        .or_else(|| find(&dock.center))
}

fn let_save_run(h: &mut Harness) {
    h.cx.executor()
        .advance_clock(std::time::Duration::from_millis(500));
    h.settle();
}

/// G3: retargeting the open graph (sidebar action, sidebar selection) is
/// saved without any other dock change; a close right after it writes it;
/// the next start shows it.
#[test]
fn retargeting_the_open_graph_is_saved_and_restored() {
    let _config = OwnConfig::new();
    {
        let mut h = Harness::new("project-graph-retarget-save", seed_graph);
        click(&mut h, "graph-action:sample");
        let_save_run(&mut h);
        assert_eq!(saved_graph_repo().as_deref(), Some("sample"));

        click(&mut h, "graph-action:sample-other");
        let_save_run(&mut h);
        assert_eq!(
            saved_graph_repo().as_deref(),
            Some("sample-other"),
            "the action's retarget is saved"
        );

        // Sidebar selection: the first ↓ selects repo:empty, the graph follows.
        let sidebar = h.sidebar_focus();
        h.cx.update(|window, cx| window.focus(&sidebar, cx));
        h.settle();
        h.press("down");
        assert_eq!(panel(&mut h).unwrap().0, "empty");
        let_save_run(&mut h);
        assert_eq!(
            saved_graph_repo().as_deref(),
            Some("empty"),
            "following the selection is saved"
        );

        // Quick close: retarget, then close before the save debounce ran.
        click(&mut h, "graph-action:sample");
        assert_eq!(
            saved_graph_repo().as_deref(),
            Some("empty"),
            "still pending"
        );
        let ws = h.ws.clone();
        let closes =
            h.cx.update(|window, cx| ws.update(cx, |w, cx| w.close_requested(window, cx)));
        assert!(closes, "nothing unsaved: the window closes");
        assert_eq!(
            saved_graph_repo().as_deref(),
            Some("sample"),
            "the close wrote the retarget"
        );
    }
    let mut h = Harness::new("project-graph-retarget-restart", seed_graph);
    assert_eq!(
        panel(&mut h).map(|p| p.0).as_deref(),
        Some("sample"),
        "restart shows the last repository"
    );
    assert_eq!(drawn(&mut h, &ALL), ["c1", "x1", "r1"]);
    assert_eq!(graph_leaves(&mut h), 1);
}
