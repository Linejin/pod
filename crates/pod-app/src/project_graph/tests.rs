//! Model regressions: membership by explicit records, which relations become
//! edges, broken records, stable order, keyboard movement, layout pruning.

use core::prelude::v1::test;

use super::model::{EdgeKind, ParentNote, ProjectGraph, build, graph_key};
use crate::agent_kind::AgentKind;
use crate::source::TestBackend;
use crate::vm::{AgentState, StatusVm, TaskVm, TerminalVm};

fn agent(id: &str, task: Option<&str>, argv: &str, created: u64) -> TerminalVm {
    let mut t = TestBackend::terminal(id, &format!("{id} 이름"), task, "working");
    t.argv = vec![argv.into()];
    t.program = argv.into();
    t.created_ms = created;
    t
}

fn shell(id: &str, task: Option<&str>, created: u64) -> TerminalVm {
    let mut t = TestBackend::terminal(id, &format!("{id} 셸"), task, "idle");
    t.created_ms = created;
    t
}

fn task(id: &str, repo: &str, agent: &str, terminals: &[&str]) -> TaskVm {
    let mut k = TestBackend::task(id, id, repo, terminals);
    k.agent = Some(agent.into());
    k
}

fn label(t: &TerminalVm) -> String {
    t.name.clone().unwrap_or_else(|| t.id.clone())
}

fn graph(repo: &str, tasks: &[TaskVm], terms: &[TerminalVm]) -> ProjectGraph {
    build(repo, true, tasks, terms, &label)
}

fn ids(g: &ProjectGraph) -> Vec<&str> {
    g.nodes.iter().map(|n| n.id.as_str()).collect()
}

fn edges(g: &ProjectGraph) -> Vec<(String, String, EdgeKind)> {
    g.edges
        .iter()
        .map(|e| (g.nodes[e.from].id.clone(), g.nodes[e.to].id.clone(), e.kind))
        .collect()
}

fn edge(a: &str, b: &str, k: EdgeKind) -> (String, String, EdgeKind) {
    (a.into(), b.into(), k)
}

/// `sample`: lead task (Claude c1 + shell s1); child task (Codex x1) created
/// from c1 in group `sample/fan`; loose `pod run` r1 (Claude) from x1.
/// `sample-other` (a name that starts like `sample`): o1 with a cwd and a
/// name that mention `sample`. A task without a repo: n1.
fn fixture() -> (Vec<TaskVm>, Vec<TerminalVm>) {
    let mut child = task("sample/child", "sample", "codex", &["x1"]);
    child.parent_terminal = Some("c1".into());
    child.parent_task = Some("sample/lead".into());
    child.group = Some("sample/fan".into());
    let mut solo = task("solo/n", "solo", "claude", &["n1"]);
    solo.repo = None;
    let tasks = vec![
        task("sample/lead", "sample", "claude", &["c1", "s1"]),
        child,
        task("sample-other/else", "sample-other", "claude", &["o1"]),
        solo,
    ];
    let mut r1 = agent("r1", None, "claude", 4);
    r1.parent_terminal = Some("x1".into());
    r1.parent_task = Some("sample/child".into());
    r1.status = None;
    let mut o1 = agent("o1", Some("sample-other/else"), "claude", 5);
    o1.cwd = "C:/fixtures/sample".into();
    o1.name = Some("sample 리더".into());
    let terms = vec![
        agent("c1", Some("sample/lead"), "claude", 1),
        shell("s1", Some("sample/lead"), 2),
        agent("x1", Some("sample/child"), "codex", 3),
        r1,
        o1,
        agent("n1", Some("solo/n"), "claude", 6),
        shell("z1", None, 7),
    ];
    (tasks, terms)
}

#[test]
fn membership_is_the_repo_of_the_owning_task_not_a_name_prefix() {
    let (tasks, terms) = fixture();
    let g = graph("sample", &tasks, &terms);
    assert_eq!(ids(&g), ["c1", "x1", "r1"], "agents only; shells s1/z1 out");
    assert_eq!(g.tasks, 2);
    assert!(g.known);
    let other = graph("sample-other", &tasks, &terms);
    assert_eq!(ids(&other), ["o1"], "isolation both ways");
    assert_eq!(other.edges, []);
    // A terminal of another repo that claims a sample task by `task_id` only
    // stays with the task that lists it.
    let mut terms2 = terms.clone();
    terms2[4].task_id = Some("sample/lead".into());
    assert_eq!(ids(&graph("sample", &tasks, &terms2)), ["c1", "x1", "r1"]);
    // Kinds and task text come from the records.
    let x1 = &g.nodes[1];
    assert_eq!(x1.kind, AgentKind::Codex);
    assert_eq!(x1.task.as_deref(), Some("sample/child"));
    assert_eq!(x1.group.as_deref(), Some("sample/fan"));
    assert_eq!(x1.name, "x1 이름");
    assert!(g.nodes.iter().all(|n| n.present));
}

#[test]
fn edges_are_the_recorded_parents_with_their_own_kind() {
    let (tasks, terms) = fixture();
    let g = graph("sample", &tasks, &terms);
    assert_eq!(
        edges(&g),
        [
            edge("c1", "x1", EdgeKind::StartedTask),
            edge("x1", "r1", EdgeKind::RanTerminal)
        ]
    );
    assert_eq!(
        g.nodes.iter().map(|n| (n.depth, n.col)).collect::<Vec<_>>(),
        [(0, 0.), (1, 0.), (2, 0.)]
    );
    assert_eq!(g.nodes[0].children, [1]);
    assert_eq!(g.nodes[2].parent, Some(1));
    assert_eq!((g.cols, g.rows), (1, 3));
    // Group members without a parent terminal are not joined: a group is no edge.
    let tasks = vec![
        {
            let mut k = task("s/a", "s", "claude", &["a1"]);
            k.group = Some("s/g".into());
            k
        },
        {
            let mut k = task("s/b", "s", "codex", &["b1"]);
            k.group = Some("s/g".into());
            k.parent_task = Some("s/a".into()); // a task link alone draws nothing
            k
        },
    ];
    let terms = vec![
        agent("a1", Some("s/a"), "claude", 1),
        agent("b1", Some("s/b"), "codex", 2),
    ];
    let g = graph("s", &tasks, &terms);
    assert_eq!(edges(&g), []);
    assert_eq!(
        g.nodes.iter().map(|n| (n.depth, n.col)).collect::<Vec<_>>(),
        [(0, 0.), (0, 1.)],
        "isolated nodes side by side"
    );
}

#[test]
fn only_the_tasks_first_terminal_was_started_from_the_parent() {
    // Task created from c1 with Claude t1; Codex t2 added to it later.
    let mut k = task("s/k", "s", "claude", &["t1", "t2"]);
    k.parent_terminal = Some("c1".into());
    let tasks = vec![task("s/lead", "s", "claude", &["c1"]), k];
    let terms = vec![
        agent("c1", Some("s/lead"), "claude", 1),
        agent("t1", Some("s/k"), "claude", 2),
        agent("t2", Some("s/k"), "codex", 3),
    ];
    let g = graph("s", &tasks, &terms);
    assert_eq!(edges(&g), [edge("c1", "t1", EdgeKind::StartedTask)]);
    let t2 = &g.nodes[g.index_of("t2").unwrap()];
    assert_eq!((t2.parent, t2.depth), (None, 0));
}

#[test]
fn broken_records_degrade_to_notes_without_edges() {
    let mut a = task("s/a", "s", "claude", &["a1"]);
    a.parent_terminal = Some("b1".into());
    let mut b = task("s/b", "s", "codex", &["b1"]);
    b.parent_terminal = Some("a1".into()); // a <-> b
    let mut gone = task("s/gone", "s", "claude", &["g1"]);
    gone.parent_terminal = Some("t404".into());
    let mut by_shell = task("s/sh", "s", "claude", &["h1"]);
    by_shell.parent_terminal = Some("s1".into());
    let mut cross = task("s/x", "s", "claude", &["k1"]);
    cross.parent_terminal = Some("o1".into());
    let tasks = vec![
        a,
        b,
        gone,
        by_shell,
        cross,
        task("s/s", "s", "shell", &["s1"]),
        task("o/o", "o", "claude", &["o1"]),
        // Duplicates: d1 listed twice in one task and by a second task.
        task("s/d", "s", "claude", &["d1", "d1"]),
        task("o/d", "o", "claude", &["d1"]),
        // Its agent terminal is gone from podd's list.
        task("s/stale", "s", "codex", &["m1"]),
    ];
    let mut me = agent("self", None, "claude", 9);
    me.parent_terminal = Some("self".into());
    me.parent_task = Some("s/a".into());
    let terms = vec![
        agent("a1", Some("s/a"), "claude", 1),
        agent("b1", Some("s/b"), "codex", 2),
        agent("g1", Some("s/gone"), "claude", 3),
        agent("h1", Some("s/sh"), "claude", 4),
        agent("k1", Some("s/x"), "claude", 5),
        shell("s1", Some("s/s"), 6),
        agent("o1", Some("o/o"), "claude", 7),
        agent("d1", Some("s/d"), "claude", 8),
        agent("d1", Some("s/d"), "codex", 8),
        me,
    ];
    let g = graph("s", &tasks, &terms);
    let note = |id: &str| g.nodes[g.index_of(id).unwrap()].note.clone();
    // Every node once; the stale one last (no creation time).
    assert_eq!(
        {
            let mut v = ids(&g);
            v.sort();
            v
        },
        ["a1", "b1", "d1", "g1", "h1", "k1", "m1", "self"]
    );
    // The ring a1 <-> b1 keeps one link (a1 is first in the stable order).
    assert_eq!(edges(&g), [edge("a1", "b1", EdgeKind::StartedTask)]);
    assert_eq!(note("a1"), Some(ParentNote::Cycle("b1".into())));
    assert_eq!(note("g1"), Some(ParentNote::Missing("t404".into())));
    assert_eq!(note("h1"), Some(ParentNote::Shell("s1".into())));
    assert_eq!(note("k1"), Some(ParentNote::OtherProject("o1".into())));
    assert_eq!(note("self"), Some(ParentNote::SelfParent));
    assert_eq!(g.duplicates, ["d1"]);
    let d1 = &g.nodes[g.index_of("d1").unwrap()];
    assert_eq!(d1.kind, AgentKind::Claude, "the first record wins");
    let m1 = &g.nodes[g.index_of("m1").unwrap()];
    assert!(!m1.present, "listed by its task, not by podd");
    assert_eq!(m1.kind, AgentKind::Codex);
    assert_eq!(m1.col, (g.cols - 1) as f32, "no creation time: last root");
    // Every node is reachable in the layout (no node lost in the ring).
    assert!(g.nodes.iter().all(|n| n.col < g.cols as f32));
}

#[test]
fn status_changes_and_list_order_never_move_nodes() {
    let (tasks, terms) = fixture();
    let shape = |g: &ProjectGraph| {
        g.nodes
            .iter()
            .map(|n| (n.id.clone(), n.depth, n.col))
            .collect::<Vec<_>>()
    };
    let before = graph("sample", &tasks, &terms);
    let mut changed = terms.clone();
    for (t, s) in changed.iter_mut().zip([
        AgentState::Blocked,
        AgentState::Done,
        AgentState::Error,
        AgentState::Exited,
        AgentState::Idle,
        AgentState::Unknown,
        AgentState::Working,
    ]) {
        t.status = Some(StatusVm::of(s));
    }
    changed[0].live = crate::vm::Liveness::Exited {
        code: Some(1),
        killed: false,
    };
    assert_eq!(shape(&graph("sample", &tasks, &changed)), shape(&before));
    changed.reverse();
    let mut tasks_rev = tasks.clone();
    tasks_rev.reverse();
    assert_eq!(
        shape(&graph("sample", &tasks_rev, &changed)),
        shape(&before),
        "podd's list order does not matter either"
    );
}

#[test]
fn empty_and_unregistered_projects_differ() {
    let (tasks, terms) = fixture();
    let empty = build("fresh", true, &tasks, &terms, &label);
    assert!(empty.known && empty.nodes.is_empty() && empty.tasks == 0);
    let gone = build("gone", false, &tasks, &terms, &label);
    assert!(!gone.known && gone.nodes.is_empty());
    // Shell-only project: known, no agents.
    let tasks = vec![task("s/s", "s", "shell", &["s1"])];
    let g = build("s", false, &tasks, &[shell("s1", Some("s/s"), 1)], &label);
    assert!(g.known, "named by a task");
    assert_eq!((g.tasks, g.nodes.len()), (1, 0));
}

#[test]
fn keys_walk_parents_children_and_layers() {
    // c1 → (x1, y1); x1 → r1.
    let mut kx = task("s/x", "s", "codex", &["x1"]);
    kx.parent_terminal = Some("c1".into());
    let mut ky = task("s/y", "s", "codex", &["y1"]);
    ky.parent_terminal = Some("c1".into());
    let mut r1 = agent("r1", None, "claude", 4);
    r1.parent_terminal = Some("x1".into());
    r1.parent_task = Some("s/x".into());
    let tasks = vec![task("s/c", "s", "claude", &["c1"]), kx, ky];
    let terms = vec![
        agent("c1", Some("s/c"), "claude", 1),
        agent("x1", Some("s/x"), "codex", 2),
        agent("y1", Some("s/y"), "codex", 3),
        r1,
    ];
    let g = graph("s", &tasks, &terms);
    assert_eq!(ids(&g), ["c1", "x1", "y1", "r1"]);
    assert_eq!(g.nodes[0].col, 0.5, "parent centred over its children");
    let at = |id: &str| g.index_of(id);
    assert_eq!(
        graph_key(&g, None, "down"),
        Some(0),
        "first key picks the first"
    );
    assert_eq!(graph_key(&g, None, "end"), Some(3));
    assert_eq!(graph_key(&g, at("c1"), "down"), at("x1"));
    assert_eq!(graph_key(&g, at("x1"), "right"), at("y1"));
    assert_eq!(
        graph_key(&g, at("y1"), "right"),
        at("y1"),
        "edge of the layer"
    );
    assert_eq!(graph_key(&g, at("y1"), "left"), at("x1"));
    assert_eq!(graph_key(&g, at("x1"), "down"), at("r1"));
    assert_eq!(graph_key(&g, at("r1"), "up"), at("x1"));
    assert_eq!(graph_key(&g, at("c1"), "up"), at("c1"));
    assert_eq!(graph_key(&g, at("r1"), "home"), Some(0));
    assert_eq!(graph_key(&g, at("r1"), "x"), None);
    assert_eq!(graph_key(&ProjectGraph::default(), None, "down"), None);
}

#[test]
fn saved_layouts_keep_a_graph_with_its_repository() {
    use gpui_kit::base::dock::{PanelInfo, PanelState};
    let leaf = |v: serde_json::Value| {
        let mut s = PanelState::new(super::PROJECT_GRAPH_PANEL_NAME);
        s.info = PanelInfo::panel(v);
        s
    };
    let alive = crate::layout::Alive::default();
    let kept = crate::layout::prune(&leaf(serde_json::json!({"repo": "sample"})), &alive);
    assert_eq!(
        kept.as_ref()
            .and_then(|s| crate::layout::leaf_field(s, "repo")),
        Some("sample")
    );
    assert!(crate::layout::prune(&leaf(serde_json::json!({})), &alive).is_none());
    assert!(crate::layout::prune(&leaf(serde_json::json!({"repo": " "})), &alive).is_none());
}

#[test]
fn copy_names_edges_and_notes() {
    use super::copy;
    assert_eq!(copy::action_label("sample"), "sample 에이전트 그래프");
    assert_eq!(
        copy::edge_text("리더 (c1)", "x1", EdgeKind::StartedTask),
        "리더 (c1) → x1 · 작업 시작"
    );
    assert_eq!(
        copy::note_text(&ParentNote::Missing("t9".into())),
        "상위 터미널 t9 없음 (종료·제거됨)"
    );
    assert_eq!(
        copy::task_text(Some("로그인"), Some("s/g")),
        "작업 로그인 · 그룹 s/g"
    );
}

#[test]
fn cards_fit_the_visible_graph_width() {
    use super::Geom;
    assert_eq!(
        Geom::for_width(None).node_w,
        236.,
        "not measured yet: full card"
    );
    assert_eq!(Geom::for_width(Some(1000.)).node_w, 236.);
    // 822 px dense-details: about 246 px of graph column are on screen.
    let dense = Geom::for_width(Some(246.));
    assert_eq!(dense.node_w, 214., "one whole card with its padding");
    assert!(dense.x(0.) + dense.node_w + 16. <= 246.);
    assert_eq!(Geom::for_width(Some(120.)).node_w, 148., "compact floor");
    assert_eq!(dense.x(3.), 16. + 3. * (214. + 28.));
}

#[test]
fn reveal_keeps_the_card_inside_the_visible_viewport() {
    use super::reveal_axis;
    // The QA case: a 214 px card at column 3 (x 742) in a 246 px viewport.
    let off = reveal_axis(0., 742., 214., 246.);
    assert_eq!(off, 246. - 16. - 742. - 214.);
    let (l, r) = (742. + off, 742. + 214. + off);
    assert!(l >= 0. && r <= 246., "{l}..{r} inside 0..246");
    // Back to column 0: aligned after the padding.
    assert_eq!(reveal_axis(off, 16., 214., 246.), 0.);
    // Already visible: no jump.
    assert_eq!(reveal_axis(-10., 100., 50., 400.), -10.);
    // Wider than the viewport: its start (name, id) is shown.
    assert_eq!(reveal_axis(0., 500., 148., 120.), -500.);
}

#[test]
fn a_new_graph_dock_leaves_the_centre_room() {
    use super::dock_width;
    assert_eq!(dock_width(0.), 480., "not laid out yet");
    assert_eq!(dock_width(1400.), 480.);
    assert_eq!(
        dock_width(560.),
        280.,
        "822 px workbench: half the dock area"
    );
    assert_eq!(dock_width(400.), 268., "one whole card");
    assert_eq!(
        dock_width(246.),
        246.,
        "822 px dense column: the column, no overflow"
    );
    assert_eq!(dock_width(60.), 100., "never below the dock minimum");
}

#[test]
fn row_actions_fit_inside_their_row() {
    use crate::workspace::sidebar_row_action_height as fit;
    assert_eq!(fit(28., 22.), 22., "dense: the 22 px row, not 28");
    assert_eq!(fit(28., 28.), 28., "default density unchanged");
    assert_eq!(fit(36., 34.), 34., "comfortable: not 36 in a 34 px row");
    assert_eq!(fit(36., 40.), 36., "accessible: the full hit area");
}
