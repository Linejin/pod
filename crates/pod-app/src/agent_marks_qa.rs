//! Agent marks in the real workspace on GPUI's headless platform (docs/10 §D5.11):
//! every surface that lists agents paints the mark with its accessible name
//! (`agent-mark:<place>:<name>`, recorded by [`crate::agent_kind::mark_el`]),
//! task rows/tabs paint the stack, and the sidebar search matches by kind.
use core::prelude::v1::test;
use gpui_kit::*;

use crate::native_qa::Harness;
use crate::source::TestBackend;

const TASK: &str = "sample/로그인-수정";

/// One task with a Claude (task agent + argv), a Codex (hook) and a shell
/// terminal; a loose Gemini (OSC `app=`) terminal.
pub(crate) fn seed_agents(b: &mut TestBackend) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    let mut task = TestBackend::task(TASK, "로그인 오류 고치기", "sample", &["c1", "x1", "s1"]);
    task.agent = Some("claude".into());
    b.tasks.push(task);
    let mut c1 = TestBackend::terminal("c1", "클로드 작업자", Some(TASK), "working");
    c1.argv = vec!["claude".into(), "--resume".into()];
    c1.program = "claude".into();
    let mut x1 = TestBackend::terminal("x1", "코덱스 검토자", Some(TASK), "blocked");
    if let Some(s) = x1.status.as_mut() {
        s.source = Some("hook".into());
        s.agent = Some("codex".into());
        s.kind = Some("question".into());
    }
    let s1 = TestBackend::terminal("s1", "빌드 셸", Some(TASK), "idle");
    let mut g1 = TestBackend::terminal("g1", "제미나이 실험", None, "working");
    if let Some(s) = g1.status.as_mut() {
        s.app = Some("gemini".into());
    }
    b.terminals.extend([c1, x1, s1, g1]);
}

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

fn search(h: &mut Harness, query: &str) {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.set_sidebar_query(query, window, cx)));
    h.settle();
}

fn mark(h: &mut Harness, place: &str, name: &str) -> Option<Bounds<Pixels>> {
    h.bounds(&format!("agent-mark:{place}:{name}"))
}

#[test]
fn sidebar_rows_marks_task_stack_and_kind_search() {
    let mut h = Harness::new("agent-marks-sidebar", seed_agents);
    for (id, name) in [
        ("c1", "Claude 에이전트"),
        ("x1", "Codex 에이전트"),
        ("s1", "셸 터미널"),
        ("g1", "Gemini 에이전트"),
    ] {
        let m = mark(&mut h, &format!("row-{id}"), name)
            .unwrap_or_else(|| panic!("{id}: mark named {name}"));
        let row = h.bounds(&format!("row-{id}")).expect("row");
        // Before the name (left edge of the row), inside the theme's row height.
        assert!(
            m.left() < row.left() + px(48.),
            "{id}: mark leads the row {m:?} {row:?}"
        );
        assert!(
            m.size.height <= row.size.height && m.top() >= row.top() && m.bottom() <= row.bottom(),
            "{id}: mark inside the row"
        );
        assert_eq!(
            row.size.height,
            px(pod_ui::themes::metrics().density.row),
            "{id}: row height from the theme metrics"
        );
    }
    assert!(
        h.bounds(&format!("agent-stack:task-{TASK}:Claude,Codex"))
            .is_some(),
        "task row: its agents, shells left out"
    );

    search(&mut h, "codex");
    assert!(
        h.bounds("row-x1").is_some(),
        "codex matches the Codex terminal"
    );
    assert!(
        h.bounds("row-c1").is_none()
            && h.bounds("row-s1").is_none()
            && h.bounds("row-g1").is_none(),
        "only Codex rows stay"
    );
    search(&mut h, "Claude");
    assert!(
        h.bounds("row-c1").is_some() && h.bounds("row-x1").is_none(),
        "case-insensitive kind match"
    );
    search(&mut h, "코덱스");
    assert!(h.bounds("row-x1").is_some(), "Korean alias");
    search(&mut h, "gemini");
    assert!(h.bounds("row-g1").is_some() && h.bounds("row-c1").is_none());
    search(&mut h, "");
    assert!(h.bounds("row-c1").is_some() && h.bounds("row-g1").is_some());
}

#[test]
fn sidebar_marks_fit_at_high_dpi_and_large_font() {
    for scale in [1.25, 1.5, 1.75] {
        let mut h = Harness::sized(
            "agent-marks-dpi",
            scale,
            size(px(960.), px(640.)),
            seed_agents,
        );
        h.cx.update(|_, cx| {
            crate::theme::Settings {
                font_size: 18.,
                ..cx.global::<crate::theme::Settings>().clone()
            }
            .apply_with(false, cx)
        });
        h.settle();
        let row = h.bounds("row-x1").expect("row");
        let m = mark(&mut h, "row-x1", "Codex 에이전트").expect("mark");
        assert!(
            m.top() >= row.top() && m.bottom() <= row.bottom(),
            "{scale}: mark inside the row {m:?} {row:?}"
        );
    }
}

#[test]
fn agent_tree_and_terminal_panel_header() {
    let mut h = Harness::new("agent-marks-tree", seed_agents);
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_agent_tree(window, cx)));
    h.settle();
    assert!(
        mark(&mut h, "tree-t:c1", "Claude 에이전트").is_some(),
        "tree terminal row"
    );
    assert!(mark(&mut h, "tree-t:x1", "Codex 에이전트").is_some());
    assert!(mark(&mut h, "tree-t:s1", "셸 터미널").is_some());
    assert!(
        h.bounds(&format!("agent-stack:tree-k:{TASK}:Claude,Codex"))
            .is_some(),
        "tree task row: stacked marks"
    );

    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("x1", false, window, cx)));
    h.settle();
    assert!(
        mark(&mut h, "panel-x1", "Codex 에이전트").is_some(),
        "terminal panel header/tab"
    );
    let tab = h.cx.update(|_, cx| {
        ws.read(cx)
            .live_panels()
            .iter()
            .find(|p| p.read(cx).terminal_id == "x1")
            .map(|p| gpui_kit::component::dock::Panel::tab_name(p.read(cx), cx))
    });
    assert!(
        tab.flatten()
            .is_some_and(|t| t.ends_with("· Codex 에이전트")),
        "tab accessible name carries the agent"
    );
}

#[test]
fn kanban_queue_and_rail_cards() {
    let mut h = Harness::new("agent-marks-layouts", seed_agents);
    set_layout(&mut h, "kanban");
    assert!(
        mark(&mut h, "kanban-c1", "Claude 에이전트").is_some(),
        "kanban card"
    );
    assert!(mark(&mut h, "kanban-x1", "Codex 에이전트").is_some());
    assert!(mark(&mut h, "kanban-g1", "Gemini 에이전트").is_some());

    set_layout(&mut h, "queue");
    assert!(
        mark(&mut h, "queue-term:x1", "Codex 에이전트").is_some(),
        "queue card (blocked question)"
    );
    assert!(
        mark(&mut h, "queue-term:c1", "Claude 에이전트").is_some(),
        "queue working row"
    );

    // The rail shows tabs for tasks with open panels.
    set_layout(&mut h, "workbench");
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("c1", false, window, cx)));
    h.settle();
    set_layout(&mut h, "focus-rail");
    assert!(
        h.bounds(&format!("agent-stack:rail-tab-{TASK}:Claude,Codex"))
            .is_some(),
        "rail task tab"
    );
}
