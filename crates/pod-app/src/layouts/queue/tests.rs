//! 응답 대기열 layout: state model (plain tests) and the live layout on GPUI's
//! headless platform (switching, keyboard flows, 06 chrome, DPI fit).
use std::collections::HashSet;

use super::*;
// After the glob: GPUI exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;
use serde_json::json;

use crate::native_qa::Harness;
use crate::source::{SourceEvent, TestBackend};
use crate::theme::Settings;

fn term(id: &str, task: Option<&str>, state: &str) -> TerminalVm {
    TestBackend::terminal(id, &format!("작업자 {id}"), task, state)
}

fn kind(mut t: TerminalVm, k: &str) -> TerminalVm {
    t.status.as_mut().expect("status").kind = Some(k.into());
    t
}

fn gate(id: &str, terminal: Option<&str>, task: Option<&str>, created: u64) -> GateInfo {
    GateInfo {
        id: id.into(),
        title: "명령 실행 승인".into(),
        message: "$ cargo test".into(),
        task: task.map(str::to_owned),
        group: None,
        terminal: terminal.map(str::to_owned),
        state: GateState::Pending,
        created_ms: created,
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

fn label(t: &TerminalVm) -> String {
    t.name.clone().unwrap_or_default()
}

fn fixture() -> (Vec<TerminalVm>, Vec<TaskVm>) {
    let mut x1 = term("x1", None, "done");
    x1.live = Liveness::Exited {
        code: Some(2),
        killed: false,
    };
    let mut x2 = term("x2", None, "done");
    x2.live = Liveness::Exited {
        code: Some(0),
        killed: false,
    };
    let mut k1 = term("k1", None, "working");
    k1.live = Liveness::Exited {
        code: Some(1),
        killed: true,
    };
    let mut l1 = term("l1", None, "working");
    l1.live = Liveness::Lost;
    let mut s1 = term("s1", None, "idle");
    s1.status = None;
    let terminals = vec![
        term("w1", Some("A"), "working"),
        term("b1", Some("B"), "blocked"),
        kind(term("b2", Some("C"), "blocked"), "question"),
        term("b3", None, "blocked"),
        term("d1", Some("A"), "done"),
        term("e1", Some("C"), "error"),
        x1,
        x2,
        k1,
        l1,
        term("i1", None, "idle"),
        s1,
    ];
    let tasks = vec![
        TestBackend::task("A", "로그인 수정", "sample", &["w1", "d1"]),
        TestBackend::task("B", "한글 작업과 레이아웃", "sample", &["b1"]),
        TestBackend::task("C", "리뷰 코멘트 정리", "sample", &["b2", "e1"]),
    ];
    (terminals, tasks)
}

#[test]
fn model_puts_requests_first_merges_gates_and_counts_each_state_once() {
    let (terminals, tasks) = fixture();
    let mut resolved = gate("g3", Some("b3"), None, 50);
    resolved.state = GateState::Approved;
    // Oldest first, as `OrchState::pending` gives them.
    let pending = [
        gate("g1", None, Some("A"), 100),
        gate("g2", Some("b1"), Some("B"), 200),
        resolved,
    ];
    let m = build(&terminals, &tasks, &pending, &label, "", &HashSet::new());
    let keys: Vec<&str> = m.needs.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "gate:g1", "gate:g2", "term:b2", "term:b3", "fail:e1", "fail:x1", "fail:l1"
        ]
    );
    // A gate raised from a blocked terminal is that terminal's one card.
    assert!(!keys.contains(&"term:b1"));
    let g2 = &m.needs[1];
    assert_eq!(
        (g2.kind, g2.gate.as_deref(), g2.terminal.as_deref()),
        (NeedKind::Approval, Some("g2"), Some("b1"))
    );
    assert_eq!(g2.title, "한글 작업과 레이아웃");
    assert_eq!(g2.meta, "작업자 b1 · 명령 실행 승인");
    assert_eq!(g2.detail, "$ cargo test");
    // A task-level gate opens the task's first terminal.
    assert_eq!(m.needs[0].terminal.as_deref(), Some("w1"));
    assert_eq!(m.needs[2].kind, NeedKind::Question);
    assert_eq!(m.needs[2].meta, "작업자 b2 · 질문");
    assert_eq!(
        (m.needs[3].kind, m.needs[3].gate.as_deref()),
        (NeedKind::Approval, None),
        "permission without a gate: answer in the terminal"
    );
    assert_eq!(m.needs[3].title, "작업자 b3", "no task: the terminal label");
    assert!(
        m.needs[5].meta.contains("실패 종료 (2)"),
        "{}",
        m.needs[5].meta
    );
    assert_eq!(
        m.counts,
        Counts {
            working: 1,
            needs: 4,
            done: 1,
            failed: 3
        }
    );
    let rows: Vec<&str> = m
        .working
        .iter()
        .chain(&m.done)
        .map(|r| r.key.as_str())
        .collect();
    assert_eq!(
        rows,
        ["term:w1", "term:d1"],
        "killed, clean exits, idle and status-less shells stay out"
    );
    assert_eq!(m.working[0].task, "로그인 수정");
    let order: Vec<String> = m.keys(false, false).into_iter().map(|(k, _)| k).collect();
    assert_eq!(order.len(), 9);
    assert_eq!(&order[7..], ["term:w1", "term:d1"]);
    assert_eq!(m.first_of(Section::Needs).as_deref(), Some("gate:g1"));
    assert_eq!(m.first_of(Section::Failed).as_deref(), Some("fail:e1"));
    assert_eq!(m.first_of(Section::Working).as_deref(), Some("term:w1"));
    assert_eq!(m.first_of(Section::Done).as_deref(), Some("term:d1"));
}

#[test]
fn model_filter_dismissal_and_compact_sections() {
    let (terminals, tasks) = fixture();
    let m = build(&terminals, &tasks, &[], &label, "리뷰", &HashSet::new());
    let keys: Vec<&str> = m.needs.iter().map(|n| n.key.as_str()).collect();
    assert_eq!(keys, ["term:b2", "fail:e1"], "filter matches task titles");
    assert_eq!(m.counts.needs, 3, "counts ignore the filter");
    assert!(m.working.is_empty() && m.done.is_empty());
    // An acknowledged failure stays hidden until it fails again (new since).
    let since = terminals
        .iter()
        .find(|t| t.id == "e1")
        .and_then(|t| t.status.as_ref())
        .map(|s| s.since)
        .unwrap();
    let dismissed: HashSet<(String, u64)> = [("fail:e1".to_string(), since)].into();
    let m = build(&terminals, &tasks, &[], &label, "", &dismissed);
    assert!(!m.needs.iter().any(|n| n.key == "fail:e1"));
    assert_eq!(m.counts.failed, 3, "still counted as failed");
    let mut again = terminals.clone();
    again
        .iter_mut()
        .find(|t| t.id == "e1")
        .unwrap()
        .status
        .as_mut()
        .unwrap()
        .since = since + 1;
    assert!(
        build(&again, &tasks, &[], &label, "", &dismissed)
            .needs
            .iter()
            .any(|n| n.key == "fail:e1")
    );
    // 작업 중 shows COMPACT_ROWS rows until "모두 보기".
    let many: Vec<TerminalVm> = (0..8)
        .map(|i| term(&format!("w{i}"), None, "working"))
        .collect();
    let m = build(&many, &[], &[], &label, "", &HashSet::new());
    assert_eq!(m.keys(false, false).len(), COMPACT_ROWS);
    assert_eq!(m.keys(true, false).len(), 8);
}

#[test]
fn ages_variants_and_presets() {
    assert_eq!(age(0, 5_000), "", "unknown start");
    assert_eq!(age(1_000, 13_000), "12초");
    assert_eq!(age(1, 3 * 60_000 + 1), "3분");
    assert_eq!(age(1, 2 * 3_600_000 + 1), "2시간");
    assert_eq!(age(10_000, 5_000), "", "clock skew");
    // Presets 03 and 06 are the queue layout; 06's style gets the 06 structure.
    let queue: Vec<_> = pod_ui::themes::PRESETS
        .into_iter()
        .filter(|p| p.layout == LayoutMode::Queue.id())
        .collect();
    assert_eq!(
        queue.iter().map(|p| p.number).collect::<Vec<_>>(),
        ["03", "06"]
    );
    assert_eq!(Variant::of_style(queue[0].style), Variant::Classic);
    assert_eq!(Variant::of_style(queue[1].style), Variant::Mission);
    // Settings › 모양 enables a preset card when its layout is registered.
    let r = crate::layout_mode::LayoutRegistry::new();
    assert!(r.available(LayoutMode::Queue));
    assert_eq!(r.resolve("queue").mode(), LayoutMode::Queue);
}

// ───────────────────────────── live layout ─────────────────────────────

fn seed(b: &mut TestBackend) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    b.tasks.push(TestBackend::task(
        "sample/로그인-수정",
        "로그인 수정",
        "sample",
        &["w1", "d1"],
    ));
    b.tasks.push(TestBackend::task(
        "sample/레이아웃",
        "한글 작업과 레이아웃",
        "sample",
        &["b1"],
    ));
    b.tasks.push(TestBackend::task(
        "sample/리뷰",
        "리뷰 코멘트 정리",
        "sample",
        &["b2", "e1"],
    ));
    b.terminals.push(TestBackend::terminal(
        "w1",
        "로그인 작업",
        Some("sample/로그인-수정"),
        "working",
    ));
    b.terminals.push(TestBackend::terminal(
        "d1",
        "문서 작업",
        Some("sample/로그인-수정"),
        "done",
    ));
    b.terminals.push(TestBackend::terminal(
        "b1",
        "레이아웃 작업",
        Some("sample/레이아웃"),
        "blocked",
    ));
    b.terminals.push(kind(
        TestBackend::terminal("b2", "리뷰 봇", Some("sample/리뷰"), "blocked"),
        "question",
    ));
    b.terminals.push(TestBackend::terminal(
        "e1",
        "LSP 작업",
        Some("sample/리뷰"),
        "error",
    ));
}

/// Isolated config per test (settings.json / layout.json written by the switches).
struct Config(#[allow(dead_code)] tempfile::TempDir);
impl Config {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("config dir");
        crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
        Self(dir)
    }
}
impl Drop for Config {
    fn drop(&mut self) {
        crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = None);
    }
}

/// The user's path: Settings › 모양 commits the layout (saved, applied, window refreshed).
fn set_layout(h: &mut Harness, layout: &str) {
    let ws = h.ws.downgrade();
    h.cx.update(|window, cx| {
        let next = Settings {
            layout: layout.into(),
            ..cx.global::<Settings>().clone()
        };
        crate::chrome::commit_settings(next, &ws, window, cx);
    });
    h.settle();
}

/// 06 without re-activating the process-wide style (other tests run in parallel):
/// the variant comes from the settings' style id.
fn set_mission(h: &mut Harness) {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        let next = Settings {
            style: "mission".into(),
            layout: "queue".into(),
            ..cx.global::<Settings>().clone()
        };
        cx.set_global(next);
        ws.update(cx, |w, cx| w.touch(cx));
        window.refresh();
    });
    h.settle();
}

fn queue(h: &mut Harness) -> Entity<QueueColumn> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| existing_column(ws.entity_id(), cx))
        .expect("queue column")
}

fn list_focused(h: &mut Harness) -> bool {
    let q = queue(h);
    let f = h.cx.update(|_, cx| q.read(cx).focus.clone());
    h.is_focused(&f)
}

fn selected(h: &mut Harness) -> Option<String> {
    let q = queue(h);
    h.cx.update(|_, cx| q.read(cx).selected.clone())
}

/// Panel entity per terminal + the serialized dock arrangement.
fn dock_state(h: &mut Harness) -> (Vec<(String, EntityId, EntityId)>, serde_json::Value) {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let w = ws.read(cx);
        let mut panels: Vec<_> = w
            .live_panels()
            .into_iter()
            .map(|p| {
                (
                    p.read(cx).terminal_id.clone(),
                    p.entity_id(),
                    p.read(cx).view.entity_id(),
                )
            })
            .collect();
        panels.sort();
        (
            panels,
            proportions(serde_json::to_value(w.dock.read(cx).dump(cx)).expect("dock state")),
        )
    })
}

/// Split sizes as shares of their split: the pixels follow the dock's width
/// (queue column vs sidebar), the arrangement is the proportions.
fn proportions(mut v: serde_json::Value) -> serde_json::Value {
    match &mut v {
        serde_json::Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if k == "sizes"
                    && let Some(a) = x.as_array()
                {
                    let total: f64 = a.iter().filter_map(|n| n.as_f64()).sum();
                    *x = json!(
                        a.iter()
                            .map(|n| ((n.as_f64().unwrap_or(0.) / total.max(1.)) * 100.).round())
                            .collect::<Vec<_>>()
                    );
                } else {
                    *x = proportions(x.take());
                }
            }
        }
        serde_json::Value::Array(a) => {
            for x in a.iter_mut() {
                *x = proportions(x.take());
            }
        }
        _ => {}
    }
    v
}

fn open(h: &mut Harness, id: &str) {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal(id, false, window, cx)));
    h.settle();
}

#[test]
fn switching_layouts_live_keeps_panels_dock_and_queue_state() {
    let _config = Config::new();
    let mut h = Harness::new("queue-switch", seed);
    open(&mut h, "w1");
    open(&mut h, "b1");
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.arrange_grid(window, cx)));
    h.settle();
    let before = dock_state(&mut h);
    assert_eq!(before.0.len(), 2);
    assert!(h.bounds("queue-list").is_none(), "workbench first");

    set_layout(&mut h, "queue");
    assert!(h.bounds("queue-list").is_some(), "the queue renders");
    assert!(
        h.bounds("queue-counts").is_some(),
        "03: counts in the column"
    );
    assert!(
        h.bounds("activity-bar").is_none() && h.bounds("queue-pills").is_none(),
        "03 has no 06 chrome"
    );
    assert_eq!(
        dock_state(&mut h),
        before,
        "same panel and view entities (no re-attach), same dock arrangement"
    );

    // Queue view state made in this layout …
    h.keys(FOCUS_QUEUE_KEY);
    assert!(list_focused(&mut h), "Ctrl+Shift+Q focuses the list");
    h.keys("down");
    let sel = selected(&mut h);
    assert!(sel.is_some());
    let q = queue(&mut h);
    h.cx.update(|_, cx| q.update(cx, |q, _| q.all_done = true));
    let in_queue = dock_state(&mut h);

    set_layout(&mut h, "workbench");
    assert!(h.bounds("queue-list").is_none());
    assert_eq!(
        dock_state(&mut h),
        in_queue,
        "switching back keeps the dock as it was"
    );
    // … is still there when coming back (the column outlives the switch).
    set_layout(&mut h, "queue");
    assert_eq!(selected(&mut h), sel);
    assert!(h.cx.update(|_, cx| q.read(cx).all_done));
    assert_eq!(queue(&mut h).entity_id(), q.entity_id());
    assert_eq!(dock_state(&mut h).0, before.0, "still the original panels");
}

#[test]
fn keyboard_decides_a_gate_inline_and_opens_terminals() {
    let _config = Config::new();
    let mut h = Harness::new("queue-keys", seed);
    h.backend.rpc = Some(Box::new(|method, params| {
        (method == "gate.resolve").then(|| {
            let mut g = gate(
                params["id"].as_str().unwrap_or_default(),
                Some("b1"),
                Some("sample/레이아웃"),
                1,
            );
            g.state = if params["decision"] == "approve" {
                GateState::Approved
            } else {
                GateState::Rejected
            };
            Ok(serde_json::to_value(g).expect("gate"))
        })
    }));
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gates(vec![gate(
            "g1",
            Some("b1"),
            Some("sample/레이아웃"),
            1,
        )]));
    h.settle();
    set_layout(&mut h, "queue");

    // From the workspace root: Ctrl+Shift+Q → the list, the oldest request selected.
    h.keys(FOCUS_QUEUE_KEY);
    assert!(list_focused(&mut h));
    assert_eq!(selected(&mut h).as_deref(), Some("gate:g1"));
    // Tab → the selected card's 이번만 허용; Enter decides through gate.resolve.
    h.press("tab");
    assert!(
        !list_focused(&mut h),
        "Tab left the list for the card's first action"
    );
    h.press("enter");
    let call = h
        .backend
        .calls
        .iter()
        .find(|c| c.starts_with("gate.resolve"))
        .cloned()
        .expect("gate.resolve sent");
    let params: serde_json::Value =
        serde_json::from_str(call.trim_start_matches("gate.resolve ")).expect("params");
    assert_eq!(params, json!({"id": "g1", "decision": "approve"}));
    // The decided card is gone. b1 still reports blocked until its agent moves on,
    // so its own (terminal-answered) card takes the place and stays selected.
    assert_eq!(selected(&mut h).as_deref(), Some("term:b1"));
    assert!(list_focused(&mut h), "focus back on the list");
    let ws = h.ws.clone();
    assert!(
        h.cx.update(|_, cx| ws.read(cx).orch.pending().is_empty()),
        "the workspace's gate set has the decision"
    );

    // Down → the question; Enter: its terminal opens and takes focus.
    h.keys("down");
    assert_eq!(selected(&mut h).as_deref(), Some("term:b2"));
    h.keys("enter");
    assert_eq!(h.focused_terminal().as_deref(), Some("b2"));
    // Typing goes to the agent, Ctrl+Shift+Q comes back from the terminal.
    h.type_text("y");
    assert_eq!(h.backend.typed_text("b2"), "y");
    h.keys(FOCUS_QUEUE_KEY);
    assert!(list_focused(&mut h), "Ctrl+Shift+Q from a terminal");
    // Arrows walk cards → 작업 중 → 완료; selecting reveals an open panel without taking focus.
    h.keys("end");
    assert_eq!(selected(&mut h).as_deref(), Some("term:d1"));
    h.keys("up");
    assert_eq!(selected(&mut h).as_deref(), Some("term:w1"));
    assert!(list_focused(&mut h));
    h.keys("enter");
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));

    // 거부: second action of a new gate's card.
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gate(Box::new(gate(
            "g2",
            Some("b1"),
            Some("sample/레이아웃"),
            2,
        ))));
    h.settle();
    h.keys(FOCUS_QUEUE_KEY);
    h.keys("home");
    assert_eq!(selected(&mut h).as_deref(), Some("gate:g2"));
    h.press("tab");
    h.press("tab");
    h.press("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("gate.resolve") && c.contains("\"reject\"") && c.contains("g2")),
        "{:?}",
        h.backend.calls
    );

    // A gate with options (v17) is approved by picking one: one button per option, the choice is sent.
    let mut g3 = gate("g3", Some("b1"), Some("sample/레이아웃"), 3);
    g3.options = vec!["main에 병합".into(), "PR만 열기".into()];
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gate(Box::new(g3)));
    h.settle();
    h.keys(FOCUS_QUEUE_KEY);
    h.keys("home");
    assert_eq!(selected(&mut h).as_deref(), Some("gate:g3"));
    h.press("tab");
    h.press("tab");
    h.press("enter");
    let call = h
        .backend
        .calls
        .iter()
        .rev()
        .find(|c| c.starts_with("gate.resolve") && c.contains("g3"))
        .cloned()
        .expect("g3 decided");
    let params: serde_json::Value =
        serde_json::from_str(call.trim_start_matches("gate.resolve ")).expect("params");
    assert_eq!(
        params,
        json!({"id": "g3", "decision": "approve", "choice": "PR만 열기"}),
        "second option button"
    );
}

#[test]
fn mission_variant_has_pills_activity_bar_and_section_jumps() {
    let _config = Config::new();
    let mut h = Harness::new("queue-mission", seed);
    set_mission(&mut h);
    let view = Bounds::new(point(px(0.), px(0.)), h.viewport());
    let pills = h.bounds("queue-pills").expect("06: pills in the title bar");
    assert!(
        pills.bottom() <= px(m_titlebar() + 0.5),
        "pills sit in the title bar: {pills:?}"
    );
    assert!(within(pills, view));
    assert!(h.bounds("activity-bar").is_some(), "06: activity bar");
    assert!(
        h.bounds("queue-counts").is_none(),
        "06: counts live in the pills"
    );
    // A pill jumps to its section.
    let q = queue(&mut h);
    h.cx.update(|window, cx| q.update(cx, |q, cx| q.focus_section(Section::Failed, window, cx)));
    h.settle();
    assert_eq!(selected(&mut h).as_deref(), Some("fail:e1"));
    assert!(list_focused(&mut h));
    // The activity bar hides the column; Ctrl+Shift+Q brings it back focused.
    h.cx.update(|_, cx| {
        q.update(cx, |q, cx| {
            q.hidden = true;
            cx.notify();
        })
    });
    h.settle();
    assert!(h.bounds("queue-list").is_none());
    h.keys(FOCUS_QUEUE_KEY);
    assert!(h.bounds("queue-list").is_some());
    assert!(list_focused(&mut h));
}

fn m_titlebar() -> f32 {
    pod_ui::themes::metrics().density.titlebar.max(32.)
}

fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let e = px(0.5);
    inner.left() >= outer.left() - e
        && inner.top() >= outer.top() - e
        && inner.right() <= outer.right() + e
        && inner.bottom() <= outer.bottom() + e
}

#[test]
fn korean_text_fits_at_100_to_175_percent() {
    let _config = Config::new();
    for scale in [1.0f32, 1.25, 1.5, 1.75] {
        // The capture window: 1440×960 physical pixels.
        let logical = size(px((1440. / scale).floor()), px((960. / scale).floor()));
        let mut h = Harness::sized("queue-dpi", scale, logical, seed);
        let _ = h
            .backend
            .events
            .unbounded_send(SourceEvent::Gates(vec![gate(
                "g1",
                Some("b1"),
                Some("sample/레이아웃"),
                1,
            )]));
        set_layout(&mut h, "queue");
        h.keys(FOCUS_QUEUE_KEY);
        let column = h.bounds("queue-column").expect("column");
        let view = Bounds::new(point(px(0.), px(0.)), h.viewport());
        assert!(within(column, view), "scale {scale}: column {column:?}");
        for stat in [copy::WORKING, copy::NEEDS, copy::DONE, copy::FAILED] {
            let b = h
                .bounds(&format!("queue-stat-{stat}"))
                .unwrap_or_else(|| panic!("scale {scale}: {stat} painted"));
            assert!(
                within(b, column),
                "scale {scale}: {stat} {b:?} clipped by {column:?}"
            );
        }
        for card in [
            "queue-card-gate:g1",
            "queue-card-term:b2",
            "queue-card-fail:e1",
        ] {
            let b = h
                .bounds(card)
                .unwrap_or_else(|| panic!("scale {scale}: {card} painted"));
            assert!(
                b.right() <= column.right() + px(0.5),
                "scale {scale}: {card} {b:?} wider than {column:?}"
            );
        }
        let actions = h.bounds("queue-actions-gate:g1").expect("gate actions");
        assert!(
            within(actions, h.bounds("queue-card-gate:g1").unwrap()),
            "scale {scale}: 이번만 허용/거부/열기 wrap inside the card: {actions:?}"
        );
        set_mission(&mut h);
        let pills = h.bounds("queue-pills").expect("pills");
        let title = h.bounds("title-actions").expect("title actions");
        assert!(
            within(pills, view) && pills.right() <= title.left() + px(0.5),
            "scale {scale}: pills {pills:?} before {title:?}"
        );
    }
}
