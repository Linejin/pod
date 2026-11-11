//! 고밀도 + 세부 열 layout: the event-log model (plain tests) and the live
//! layout on GPUI's headless platform (switching, keyboard flows, DPI fit).
use std::collections::BTreeMap;

use super::*;
// After the glob: GPUI exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;
use serde_json::json;

use pod_ui::{ControlState, Icon, Tone};

use super::log::{EventKind, LOG_CAP, SEED_CAP, clock, offset_between};
use super::model::{Act, BarEntry, Link, Panel, details_width, output_height};
use crate::agent_kind::AgentKind;
use crate::native_qa::Harness;
use crate::source::{SourceEvent, TestBackend};
use crate::theme::Settings;
use crate::vm::TaskVm;

fn term(id: &str, task: Option<&str>, state: &str) -> TerminalVm {
    TestBackend::terminal(id, &format!("작업자 {id}"), task, state)
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

fn observe(
    log: &mut EventLog,
    terminals: &[TerminalVm],
    gates: &BTreeMap<String, GateInfo>,
    toasts: Vec<ToastIn>,
    now: u64,
) -> bool {
    log.observe(
        &Inputs {
            terminals,
            tasks: &[],
            gates,
            toasts,
            label: &label,
        },
        now,
    )
}

fn texts(log: &EventLog) -> Vec<(EventKind, String, String)> {
    log.events()
        .map(|e| (e.kind, e.who.clone(), e.text.clone()))
        .collect()
}

#[test]
fn first_observation_seeds_only_what_needs_attention() {
    let mut x1 = term("x1", None, "done");
    x1.live = Liveness::Exited {
        code: Some(2),
        killed: false,
    };
    let mut x0 = term("x0", None, "done");
    x0.live = Liveness::Exited {
        code: Some(0),
        killed: false,
    };
    let mut b1 = term("b1", Some("B"), "blocked");
    b1.status.as_mut().unwrap().since = 5_000;
    let terminals = vec![
        term("w1", Some("A"), "working"),
        b1,
        term("e1", None, "error"),
        term("d1", None, "done"),
        x1,
        x0,
    ];
    let gates: BTreeMap<_, _> =
        [("g1".to_string(), gate("g1", Some("b1"), Some("B"), 3_000))].into();
    let toast = ToastIn {
        id: 0,
        kind: ToastKind::Error,
        title: "실행 실패".into(),
        body: "포트 사용 중".into(),
    };
    let mut log = EventLog::default();
    assert!(observe(&mut log, &terminals, &gates, vec![toast], 10_000));
    let t = texts(&log);
    assert_eq!(t[0].0, EventKind::Start);
    assert!(t[0].2.contains("터미널 4개"), "{}", t[0].2);
    // Time order: the gate (3 s) before b1's block (5 s); unknown times (0) are "now".
    let kinds: Vec<EventKind> = t.iter().skip(1).map(|e| e.0).collect();
    assert_eq!(
        kinds,
        [
            EventKind::GateRequested,
            EventKind::Status,
            EventKind::Status,
            EventKind::Exit,
            EventKind::Notice
        ]
    );
    assert_eq!(
        t[1].1, "작업자 b1",
        "a gate on a known terminal is that agent's line"
    );
    assert!(
        t[1].2
            .starts_with("승인 요청 · 명령 실행 승인: $ cargo test"),
        "{}",
        t[1].2
    );
    assert_eq!(t[2].1, "작업자 b1");
    assert_eq!(t[3].1, "작업자 e1");
    assert_eq!(
        t[4],
        (EventKind::Exit, "작업자 x1".into(), "실패 종료 (2)".into())
    );
    assert_eq!(
        t[5],
        (EventKind::Notice, "실행 실패".into(), "포트 사용 중".into())
    );
    let ev: Vec<&LogEvent> = log.events().collect();
    assert_eq!(ev[1].at, 3_000);
    assert_eq!(ev[2].at, 5_000);
    assert_eq!(ev[3].at, 10_000);
    assert_eq!(
        (ev[2].tone, ev[3].tone, ev[4].tone, ev[5].tone),
        (Tone::Warning, Tone::Danger, Tone::Danger, Tone::Danger)
    );
    // Working, done and clean exits are not seeded; nothing new → no change.
    assert!(
        !t.iter()
            .any(|e| e.1 == "작업자 w1" || e.1 == "작업자 d1" || e.1 == "작업자 x0")
    );
    let toast = ToastIn {
        id: 0,
        kind: ToastKind::Error,
        title: "실행 실패".into(),
        body: "포트 사용 중".into(),
    };
    assert!(
        !observe(&mut log, &terminals, &gates, vec![toast], 11_000),
        "same data, no new lines"
    );
}

#[test]
fn later_observations_record_every_change_once() {
    let mut terminals = vec![
        term("w1", Some("A"), "working"),
        term("b1", Some("B"), "idle"),
    ];
    let mut gates = BTreeMap::new();
    let mut log = EventLog::default();
    observe(&mut log, &terminals, &gates, vec![], 1_000);
    assert_eq!(log.len(), 1, "only the start line");

    // Status change, a new gate, new working/idle terminals (not news), a notice.
    terminals[1].status.as_mut().unwrap().state = AgentState::Blocked;
    terminals.push(term("n1", None, "working"));
    terminals.push(term("n2", None, "idle"));
    gates.insert("g1".to_string(), gate("g1", Some("b1"), Some("B"), 1_500));
    let notice = |id| ToastIn {
        id,
        kind: ToastKind::Success,
        title: "완료".into(),
        body: format!("알림 {id}"),
    };
    assert!(observe(
        &mut log,
        &terminals,
        &gates,
        vec![notice(0)],
        2_000
    ));
    let t = texts(&log);
    assert_eq!(
        &t[1..],
        [
            (
                EventKind::Status,
                "작업자 b1".to_string(),
                "승인 필요".to_string()
            ),
            (
                EventKind::GateRequested,
                "작업자 b1".to_string(),
                "승인 요청 · 명령 실행 승인: $ cargo test".to_string()
            ),
            (EventKind::Notice, "완료".to_string(), "알림 0".to_string()),
        ]
    );
    assert!(
        log.events().skip(1).all(|e| e.at == 2_000),
        "changes are stamped when the app learns them"
    );

    // Gate approved, the agent exits with 1, the next notice; the old notice is not repeated.
    gates.get_mut("g1").unwrap().state = GateState::Approved;
    terminals[0].live = Liveness::Exited {
        code: Some(1),
        killed: false,
    };
    terminals[3].live = Liveness::Lost;
    assert!(observe(
        &mut log,
        &terminals,
        &gates,
        vec![notice(0), notice(1)],
        3_000
    ));
    let t = texts(&log);
    assert_eq!(
        &t[4..],
        [
            (
                EventKind::Exit,
                "작업자 w1".to_string(),
                "실패 종료 (1)".to_string()
            ),
            (
                EventKind::Exit,
                "작업자 n2".to_string(),
                copy::LOST.to_string()
            ),
            (
                EventKind::GateResolved,
                "작업자 b1".to_string(),
                "승인됨 · 명령 실행 승인".to_string()
            ),
            (EventKind::Notice, "완료".to_string(), "알림 1".to_string()),
        ]
    );
    // Toasts drop out of the workspace list (cap 100): ids already seen stay seen.
    assert!(!observe(
        &mut log,
        &terminals,
        &gates,
        vec![notice(1)],
        4_000
    ));
    // History of one agent, newest first; of a task's agents together.
    let b1 = ["b1".to_string()];
    let h: Vec<&str> = log.history(&b1).map(|e| e.text.as_str()).collect();
    assert_eq!(
        h,
        [
            "승인됨 · 명령 실행 승인",
            "승인 요청 · 명령 실행 승인: $ cargo test",
            "승인 필요"
        ]
    );
    assert_eq!(
        log.history(&["b1".to_string(), "w1".to_string()]).count(),
        4
    );
    // Sequence numbers increase (selection keys).
    let seqs: Vec<u64> = log.events().map(|e| e.seq).collect();
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
}

#[test]
fn log_and_seed_are_capped() {
    let terminals: Vec<TerminalVm> = (0..SEED_CAP + 10)
        .map(|i| term(&format!("b{i}"), None, "blocked"))
        .collect();
    let mut log = EventLog::default();
    observe(&mut log, &terminals, &BTreeMap::new(), vec![], 1);
    assert_eq!(log.len(), SEED_CAP + 1, "start line + newest seeds");
    let toasts: Vec<ToastIn> = (0..LOG_CAP as u64 + 20)
        .map(|id| ToastIn {
            id,
            kind: ToastKind::Info,
            title: "t".into(),
            body: id.to_string(),
        })
        .collect();
    observe(&mut log, &terminals, &BTreeMap::new(), toasts, 2);
    assert_eq!(log.len(), LOG_CAP);
    assert_eq!(
        log.events().next_back().map(|e| e.text.clone()),
        Some((LOG_CAP as u64 + 19).to_string())
    );
}

#[test]
fn clock_offsets_selection_keys_and_preset() {
    assert_eq!(clock(0, 0), "00:00:00");
    assert_eq!(clock(45_296_000, 0), "12:34:56");
    assert_eq!(clock(45_296_000, 9 * 60), "21:34:56");
    assert_eq!(clock(3_600_000, -120), "23:00:00", "before midnight wraps");
    // Same day, next day, previous day, month ends.
    assert_eq!(offset_between((10, 21, 0), (10, 12, 0)), 540);
    assert_eq!(offset_between((11, 2, 30), (10, 17, 0)), 570);
    assert_eq!(offset_between((9, 20, 0), (10, 1, 0)), -300);
    assert_eq!(offset_between((1, 3, 0), (31, 18, 0)), 540);
    assert_eq!(offset_between((30, 20, 0), (1, 1, 0)), -300);
    assert_eq!(
        offset_between((10, 12, 44), (10, 12, 0)),
        45,
        "quarter hours"
    );
    // Tree rows → details selection.
    assert_eq!(
        Sel::from_sidebar_key("task:sample/a"),
        Some(Sel::Task("sample/a".into()))
    );
    assert_eq!(
        Sel::from_sidebar_key("terminal:t1"),
        Some(Sel::Terminal("t1".into()))
    );
    assert_eq!(Sel::from_sidebar_key("repo:sample"), None);
    // Preset 07 is this layout; Settings › 모양 enables its card once registered.
    let presets: Vec<_> = pod_ui::themes::PRESETS
        .into_iter()
        .filter(|p| p.layout == LayoutMode::DenseDetails.id())
        .collect();
    assert_eq!(
        presets
            .iter()
            .map(|p| (p.number, p.style))
            .collect::<Vec<_>>(),
        [("07", "dense")]
    );
    let r = crate::layout_mode::LayoutRegistry::new();
    assert!(r.available(LayoutMode::DenseDetails));
    assert_eq!(r.resolve("dense-details").mode(), LayoutMode::DenseDetails);
}

#[test]
fn rows_grow_with_the_font_and_keep_the_style_minimum() {
    let row = pod_ui::themes::metrics().density.row;
    assert!(row_height(10.) >= row);
    for ui in [12., 15., 20., 24.] {
        let h = row_height(ui);
        assert!(h >= row && h >= ui * 1.5 + 4., "ui {ui}: row {h}");
    }
    assert!(output_height(20.) > output_height(12.));
    assert!(details_width(24.) >= 24. * 20.);
}

// ───────────────────────────── live layout ─────────────────────────────

fn seed(b: &mut TestBackend) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    let mut login = TestBackend::task("sample/로그인-수정", "로그인 수정", "sample", &["w1", "d1"]);
    login.worktree = Some("C:/fixtures/worktrees/로그인-수정".into());
    b.tasks.push(login);
    b.tasks.push(TestBackend::task(
        "sample/레이아웃",
        "한글 작업과 레이아웃이 아주 길게 이어지는 작업 제목",
        "sample",
        &["b1"],
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
    b.terminals
        .push(TestBackend::terminal("e1", "LSP 작업", None, "error"));
    // 마지막 줄 and gate decisions.
    b.rpc = Some(Box::new(|method, params| match method {
        "terminal.read" => Some(Ok(
            json!({ "id": params["id"], "mode": "screen", "lines": ["", "• 수정 crates/pod-app/src/notify.rs", "셀 경계 테스트 추가 중 · 42초", ""], "cursor": [0, 0], "state": "running" }),
        )),
        "gate.resolve" => {
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
            Some(Ok(serde_json::to_value(g).expect("gate")))
        }
        _ => None,
    }));
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

fn dense(h: &mut Harness) -> Entity<DenseView> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| existing_view(ws.entity_id(), cx))
        .expect("dense view")
}

fn selected(h: &mut Harness) -> Option<Sel> {
    let v = dense(h);
    h.cx.update(|_, cx| v.read(cx).selected.clone())
}

fn log_focused(h: &mut Harness) -> bool {
    let v = dense(h);
    let f = h.cx.update(|_, cx| v.read(cx).log_focus.clone());
    h.is_focused(&f)
}

fn details_focused(h: &mut Harness) -> bool {
    let v = dense(h);
    let f = h.cx.update(|_, cx| v.read(cx).details_focus.clone());
    h.is_focused(&f)
}

/// Panel entity per terminal + the serialized dock arrangement (split shares).
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

/// Split sizes as shares of their split: the pixels follow the dock's width.
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

fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let e = px(0.5);
    inner.left() >= outer.left() - e
        && inner.top() >= outer.top() - e
        && inner.right() <= outer.right() + e
        && inner.bottom() <= outer.bottom() + e
}

#[test]
fn switching_layouts_live_keeps_panels_dock_and_view_state() {
    let _config = Config::new();
    let mut h = Harness::new("dense-switch", seed);
    open(&mut h, "w1");
    open(&mut h, "b1");
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.arrange_grid(window, cx)));
    h.settle();
    let before = dock_state(&mut h);
    assert_eq!(before.0.len(), 2);
    assert!(h.bounds("dense-layout").is_none(), "workbench first");

    set_layout(&mut h, "dense-details");
    for part in [
        "dense-layout",
        "dense-activity",
        "dense-tree",
        "dense-output",
        "dense-log",
        "dense-details",
    ] {
        assert!(h.bounds(part).is_some(), "{part} renders");
    }
    assert_eq!(
        dock_state(&mut h),
        before,
        "same panel and view entities (no re-attach), same dock arrangement"
    );
    // The terminal focused last (b1) is in the details column.
    assert_eq!(selected(&mut h), Some(Sel::Terminal("b1".into())));

    // View state made here survives a round trip through other layouts.
    let v = dense(&mut h);
    h.cx.update(|_, cx| {
        v.update(cx, |v, cx| {
            v.show_output = false;
            cx.notify();
        })
    });
    h.settle();
    assert!(h.bounds("dense-output").is_none(), "출력 hidden");
    let here = dock_state(&mut h);
    set_layout(&mut h, "queue");
    assert_eq!(
        dock_state(&mut h).0,
        before.0,
        "queue shows the same panels"
    );
    set_layout(&mut h, "workbench");
    assert!(h.bounds("dense-layout").is_none());
    assert_eq!(
        dock_state(&mut h),
        here,
        "switching back keeps the dock as it was"
    );
    set_layout(&mut h, "dense-details");
    assert_eq!(dense(&mut h).entity_id(), v.entity_id());
    assert!(h.bounds("dense-output").is_none(), "출력 still hidden");
    assert_eq!(selected(&mut h), Some(Sel::Terminal("b1".into())));
    assert_eq!(dock_state(&mut h).0, before.0, "still the original panels");
}

#[test]
fn log_records_live_changes_and_details_follow_the_tree() {
    let _config = Config::new();
    let mut h = Harness::new("dense-log", seed);
    set_layout(&mut h, "dense-details");
    let v = dense(&mut h);
    let n0 = h.cx.update(|_, cx| v.read(cx).log.len());
    assert!(n0 >= 3, "start + b1 blocked + e1 error");
    // A status change arrives as a podd event: one new line, no timer involved.
    h.backend.set_status("w1", "blocked");
    h.settle();
    let new: Vec<LogEvent> =
        h.cx.update(|_, cx| v.read(cx).log.events().skip(n0).cloned().collect());
    assert_eq!(
        new.iter()
            .map(|e| (e.kind, e.who.as_str(), e.terminal.as_deref()))
            .collect::<Vec<_>>(),
        [
            (EventKind::Status, "로그인 작업", Some("w1")),
            // The workspace's own "응답 필요" notification for the same change.
            (EventKind::Notice, "로그인 작업 · 응답이 필요합니다", None),
        ]
    );
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gate(Box::new(gate(
            "g9",
            Some("w1"),
            Some("sample/로그인-수정"),
            1,
        ))));
    h.settle();
    let n1 = n0 + new.len();
    let kinds: Vec<EventKind> =
        h.cx.update(|_, cx| v.read(cx).log.events().skip(n1).map(|e| e.kind).collect());
    assert_eq!(kinds.first(), Some(&EventKind::GateRequested), "{kinds:?}");

    // The tree's keyboard selection drives the details column.
    let tree = h.sidebar_focus();
    h.cx.update(|window, cx| tree.focus(window, cx));
    h.settle();
    h.keys("down");
    h.keys("down");
    let sel = selected(&mut h);
    assert!(
        matches!(sel, Some(Sel::Task(_)) | Some(Sel::Terminal(_))),
        "{sel:?}"
    );
    // A terminal row → its details: state, task, worktree, last lines (read once).
    let ws = h.ws.clone();
    let key =
        h.cx.update(|_, cx| ws.read(cx).sidebar.read(cx).selected.clone());
    assert_eq!(sel, key.as_deref().and_then(Sel::from_sidebar_key));
    h.cx.update(|_, cx| v.update(cx, |v, cx| v.select(Sel::Terminal("w1".into()), cx)));
    h.settle();
    let lines =
        h.cx.update(|_, cx| v.read(cx).last_lines("w1").map(|l| l.to_vec()));
    assert_eq!(
        lines.as_deref(),
        Some(
            &[
                "• 수정 crates/pod-app/src/notify.rs".to_string(),
                "셀 경계 테스트 추가 중 · 42초".to_string()
            ][..]
        )
    );
    let reads = h
        .backend
        .calls
        .iter()
        .filter(|c| c.starts_with("terminal.read"))
        .count();
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.touch(cx)));
    h.settle();
    assert_eq!(
        h.backend
            .calls
            .iter()
            .filter(|c| c.starts_with("terminal.read"))
            .count(),
        reads,
        "no re-read until the status changes"
    );
    assert!(
        h.bounds("dense-last-lines").is_some()
            && h.bounds("dense-history").is_some()
            && h.bounds("dense-actions").is_some()
    );
}

#[test]
fn keyboard_regions_log_and_inline_gate() {
    let _config = Config::new();
    let mut h = Harness::new("dense-keys", seed);
    open(&mut h, "w1");
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
    set_layout(&mut h, "dense-details");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("w1"),
        "switching keeps focus in the terminal"
    );

    // F6 walks tree → dock → 출력 → 세부 정보 → tree; Shift+F6 back.
    h.keys("f6");
    assert!(log_focused(&mut h), "dock → 출력");
    h.keys("f6");
    assert!(details_focused(&mut h), "출력 → 세부 정보");
    h.keys("f6");
    let tree = h.sidebar_focus();
    assert!(h.is_focused(&tree), "세부 정보 → tree");
    h.keys("f6");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("w1"),
        "tree → the terminal used last"
    );
    h.keys("shift-f6");
    assert!(h.is_focused(&tree));

    // Ctrl+Shift+U: the log; arrows walk lines; Enter shows the agent and opens its terminal.
    h.keys(FOCUS_OUTPUT_KEY);
    assert!(log_focused(&mut h));
    h.keys("home");
    h.keys("down");
    let v = dense(&mut h);
    let sel_line = h.cx.update(|_, cx| {
        let v = v.read(cx);
        v.log.events().find(|e| Some(e.seq) == v.log_sel).cloned()
    });
    let line = sel_line.expect("a line is selected");
    let target = line
        .terminal
        .clone()
        .expect("the second line is an agent's");
    h.keys("enter");
    assert_eq!(selected(&mut h), Some(Sel::Terminal(target.clone())));
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some(target.as_str()),
        "Enter opens the terminal"
    );

    // Ctrl+Shift+D on b1: Tab reaches 이번만 허용; Enter decides through gate.resolve.
    h.cx.update(|_, cx| v.update(cx, |v, cx| v.select(Sel::Terminal("b1".into()), cx)));
    h.keys(FOCUS_DETAILS_KEY);
    assert!(details_focused(&mut h));
    h.press("tab");
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
    assert!(
        details_focused(&mut h),
        "focus stays in the column when the buttons go away"
    );
    let ws = h.ws.clone();
    assert!(h.cx.update(|_, cx| ws.read(cx).orch.pending().is_empty()));
    let last =
        h.cx.update(|_, cx| v.read(cx).log.events().next_back().cloned())
            .expect("line");
    assert_eq!(
        (last.kind, last.text.as_str()),
        (EventKind::GateResolved, "승인됨 · 명령 실행 승인")
    );
}

#[test]
fn korean_text_fits_at_100_to_175_percent_and_large_fonts() {
    let _config = Config::new();
    for (scale, font) in [
        (1.0f32, 13.),
        (1.25, 13.),
        (1.5, 13.),
        (1.75, 13.),
        (1.0, 20.),
    ] {
        // The capture window: 1440×960 physical pixels.
        let logical = size(px((1440. / scale).floor()), px((960. / scale).floor()));
        let mut h = Harness::sized("dense-dpi", scale, logical, seed);
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
        h.cx.update(|window, cx| {
            let next = Settings {
                font_size: font,
                ..cx.global::<Settings>().clone()
            };
            cx.set_global(next);
            window.refresh();
        });
        set_layout(&mut h, "dense-details");
        let v = dense(&mut h);
        h.cx.update(|_, cx| v.update(cx, |v, cx| v.select(Sel::Terminal("b1".into()), cx)));
        h.settle();
        let view = Bounds::new(point(px(0.), px(0.)), h.viewport());
        let details = h.bounds("dense-details").expect("details");
        let output = h.bounds("dense-output").expect("output");
        assert!(
            within(details, view),
            "scale {scale} font {font}: details {details:?}"
        );
        assert!(
            within(output, view),
            "scale {scale} font {font}: output {output:?}"
        );
        assert!(
            output.right() <= details.left() + px(0.5),
            "output under the dock, left of the details"
        );
        let actions = h.bounds("dense-actions").expect("actions");
        assert!(
            actions.left() >= details.left() - px(0.5)
                && actions.right() <= details.right() + px(0.5),
            "scale {scale} font {font}: 이번만 허용/거부/열기 wrap inside the column: {actions:?}"
        );
        for header in ["dense-output-header", "dense-details-header"] {
            let b = h.bounds(header).expect(header);
            let ui = h.cx.update(|_, cx| ui_font(cx));
            // The QA mark measures inside the 1px bottom border.
            assert!(
                b.size.height + px(pod_ui::themes::metrics().border_width)
                    >= px(row_height(ui) - 0.5),
                "scale {scale} font {font}: {header} {b:?} shorter than a row"
            );
        }
    }
}

#[test]
fn fixture_scene_runs_inside_a_workspace_update() {
    // Regression: the scene once called `select`, which reads the workspace it is updating (double lease panic).
    let _config = Config::new();
    let mut h = Harness::new("dense-fixture", |b| {
        b.tasks.push(TestBackend::task(
            "sample/task-000",
            "Task 000 - 한글 작업과 레이아웃",
            "sample",
            &["fixture-0", "fixture-1", "fixture-2", "fixture-3"],
        ));
        for (i, s) in ["working", "blocked", "done", "working"]
            .into_iter()
            .enumerate()
        {
            b.terminals.push(TestBackend::terminal(
                &format!("fixture-{i}"),
                &format!("Worker {i}"),
                Some("sample/task-000"),
                s,
            ));
        }
    });
    set_layout(&mut h, "dense-details");
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| fixture_scene(w, window, cx)));
    h.settle();
    assert_eq!(selected(&mut h), Some(Sel::Terminal("fixture-1".into())));
    let v = dense(&mut h);
    let kinds: Vec<EventKind> =
        h.cx.update(|_, cx| v.read(cx).log.events().map(|e| e.kind).collect());
    for k in [EventKind::GateRequested, EventKind::Exit, EventKind::Notice] {
        assert!(kinds.contains(&k), "{k:?} in {kinds:?}");
    }
    assert!(h.bounds("dense-allow-g1").is_some() || h.bounds("dense-actions").is_some());
}

#[test]
fn fixture_runs_print_one_frozen_time_and_real_runs_the_system_clock() {
    // A-F0b: the 출력 log and 상태 기록 print HH:MM:SS, so the fixture's capture
    // changed with the wall clock. Real runs never freeze it.
    assert_eq!(fixture_clock(), Clock::System);
    let _config = Config::new();
    let mut h = Harness::new("dense-fixture-clock", |b| {
        b.tasks.push(TestBackend::task(
            "sample/task-000",
            "Task 000 - 한글 작업과 레이아웃",
            "sample",
            &["fixture-0", "fixture-1", "fixture-2", "fixture-3"],
        ));
        for (i, s) in ["working", "blocked", "done", "working"]
            .into_iter()
            .enumerate()
        {
            b.terminals.push(TestBackend::terminal(
                &format!("fixture-{i}"),
                &format!("Worker {i}"),
                Some("sample/task-000"),
                s,
            ));
        }
    });
    set_layout(&mut h, "dense-details");
    let v = dense(&mut h);
    assert_eq!(h.cx.update(|_, cx| v.read(cx).clock), Clock::System);
    // As created by a fixture run (`freeze_fixture_clock`): frozen before the first observation.
    h.cx.update(|_, cx| {
        v.update(cx, |v, _| {
            v.clock = Clock::Frozen(FIXTURE_NOW);
            v.log = EventLog::default();
        })
    });
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| fixture_scene(w, window, cx)));
    h.settle();
    let events: Vec<(EventKind, u64)> =
        h.cx.update(|_, cx| v.read(cx).log.events().map(|e| (e.kind, e.at)).collect());
    for k in [EventKind::GateRequested, EventKind::Exit, EventKind::Notice] {
        assert!(events.iter().any(|(e, _)| *e == k), "{k:?} in {events:?}");
    }
    // The scene's gate is 12 s old; everything else happens "now".
    for (kind, at) in &events {
        assert!(
            *at == FIXTURE_NOW || *at == FIXTURE_NOW - 12_000,
            "{kind:?} at {at}"
        );
    }
    let gate =
        h.cx.update(|_, cx| ws.read(cx).orch.gates.get("g1").map(|g| g.created_ms));
    assert_eq!(gate, Some(FIXTURE_NOW - 12_000));
    assert_eq!(clock(FIXTURE_NOW, 0), "09:30:00");
}

#[test]
fn no_seed_before_the_terminal_list_and_first_reports_are_quiet() {
    // Launch: the layout renders before podd's lists arrive.
    let mut log = EventLog::default();
    assert!(!observe(&mut log, &[], &BTreeMap::new(), vec![], 1));
    assert!(log.is_empty(), "not seeded from an empty workspace");
    // The list arrives (300 terminals in the UI fixture): seeded once, attention only.
    let terminals: Vec<TerminalVm> = (0..300)
        .map(|i| {
            term(
                &format!("t{i}"),
                None,
                ["working", "done", "blocked"][i % 3],
            )
        })
        .collect();
    observe(&mut log, &terminals, &BTreeMap::new(), vec![], 2);
    assert_eq!(log.len(), 1 + SEED_CAP);
    // Statuses that arrive after the list: only blocked/error first reports are logged.
    let mut bare = vec![term("a", None, "idle"), term("b", None, "idle")];
    bare.iter_mut().for_each(|t| t.status = None);
    let mut log = EventLog::default();
    observe(&mut log, &bare, &BTreeMap::new(), vec![], 1);
    bare[0].status = term("a", None, "working").status;
    bare[1].status = term("b", None, "error").status;
    observe(&mut log, &bare, &BTreeMap::new(), vec![], 2);
    assert_eq!(
        texts(&log)
            .iter()
            .skip(1)
            .map(|e| e.1.as_str())
            .collect::<Vec<_>>(),
        ["작업자 b"]
    );
    // Then a working → done change is logged.
    bare[0].status.as_mut().unwrap().state = AgentState::Done;
    assert!(observe(&mut log, &bare, &BTreeMap::new(), vec![], 3));
}

#[test]
fn a_gate_with_options_is_approved_by_picking_one() {
    let _config = Config::new();
    let mut h = Harness::new("dense-choice", seed);
    let mut g = gate("g2", Some("b1"), Some("sample/레이아웃"), 1);
    g.options = vec!["main에 병합".into(), "초안으로".into()];
    let _ = h.backend.events.unbounded_send(SourceEvent::Gates(vec![g]));
    h.settle();
    set_layout(&mut h, "dense-details");
    let v = dense(&mut h);
    h.cx.update(|_, cx| v.update(cx, |v, cx| v.select(Sel::Terminal("b1".into()), cx)));
    h.settle();
    assert!(
        h.bounds("dense-allow-g2").is_none(),
        "no plain 이번만 허용 when the gate has options"
    );
    h.keys(FOCUS_DETAILS_KEY);
    h.press("tab");
    h.press("tab");
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
    assert_eq!(
        params,
        json!({"id": "g2", "decision": "approve", "choice": "초안으로"})
    );
}

#[test]
fn log_lines_and_details_carry_the_agent_mark() {
    // Agent kind per line (docs/10 §D5.11): from the terminal's identity at that time.
    let mut claude = term("c1", None, "working");
    claude.argv = vec!["claude".into(), "--resume".into()];
    let mut terminals = vec![claude, term("s1", None, "working")];
    let mut log = EventLog::default();
    observe(&mut log, &terminals, &BTreeMap::new(), vec![], 1);
    terminals[0].status.as_mut().unwrap().state = AgentState::Blocked;
    terminals[1].status.as_mut().unwrap().state = AgentState::Done;
    observe(
        &mut log,
        &terminals,
        &BTreeMap::new(),
        vec![ToastIn {
            id: 0,
            kind: ToastKind::Info,
            title: "t".into(),
            body: "b".into(),
        }],
        2,
    );
    let agents: Vec<Option<AgentKind>> = log.events().skip(1).map(|e| e.agent.clone()).collect();
    assert_eq!(
        agents,
        [Some(AgentKind::Claude), Some(AgentKind::Shell), None],
        "notices have no agent"
    );

    // Live: the details column and the log rows draw the mark with its accessible name.
    let _config = Config::new();
    let mut h = Harness::new("dense-marks", |b| {
        seed(b);
        b.terminals.iter_mut().find(|t| t.id == "b1").unwrap().argv = vec!["codex".into()];
    });
    set_layout(&mut h, "dense-details");
    let v = dense(&mut h);
    h.cx.update(|_, cx| v.update(cx, |v, cx| v.select(Sel::Terminal("b1".into()), cx)));
    h.settle();
    assert!(
        h.bounds("agent-mark:dense-details:Codex 에이전트")
            .is_some(),
        "details: Codex mark"
    );
    let seq =
        h.cx.update(|_, cx| {
            v.read(cx)
                .log
                .events()
                .find(|e| e.terminal.as_deref() == Some("b1"))
                .map(|e| e.seq)
        })
        .expect("b1 line");
    assert!(
        h.bounds(&format!("agent-mark:dense-log-{seq}:Codex 에이전트"))
            .is_some(),
        "log row: Codex mark"
    );
}

// ─────────────── model (A-P11): the plans the render used to inline ───────────────

#[test]
fn inline_gate_is_the_terminals_pending_gate_with_the_lowest_id() {
    // Before the split the column took the first pending gate of the session's
    // gate map (`BTreeMap` by id); the frame's pending list is oldest first.
    let mut done = gate("g0", Some("b1"), None, 0);
    done.state = GateState::Approved;
    let gates = vec![
        gate("g3", Some("b1"), None, 1),
        gate("g10", Some("b1"), None, 5),
        gate("g1", Some("w1"), None, 0),
        done,
    ];
    let map: BTreeMap<String, GateInfo> = gates.iter().map(|g| (g.id.clone(), g.clone())).collect();
    let by_map = map
        .values()
        .find(|g| g.state == GateState::Pending && g.terminal.as_deref() == Some("b1"))
        .map(|g| g.id.clone());
    assert_eq!(by_map.as_deref(), Some("g10"));
    for order in [gates.clone(), gates.iter().rev().cloned().collect()] {
        assert_eq!(
            model::pending_gate(&order, "b1").map(|g| g.id.clone()),
            by_map
        );
    }
    assert_eq!(
        model::pending_gate(&gates, "w1").map(|g| g.id.as_str()),
        Some("g1")
    );
    assert!(model::pending_gate(&gates, "d1").is_none());
}

#[test]
fn gate_buttons_keep_their_ids_labels_names_and_busy_states() {
    let plain = gate("g1", Some("b1"), None, 1);
    let row = |b: &model::GateButton| {
        (
            b.decision,
            b.label.clone(),
            b.id.clone(),
            b.choice.clone(),
            b.tone,
            b.name.clone(),
        )
    };
    let title = plain.title.clone();
    assert_eq!(
        model::gate_buttons(&plain)
            .iter()
            .map(row)
            .collect::<Vec<_>>(),
        [
            (
                GateDecision::Approve,
                "이번만 허용".to_string(),
                "dense-allow-g1".to_string(),
                None,
                Tone::Success,
                format!("이번만 허용: {title}"),
            ),
            (
                GateDecision::Reject,
                "거부".to_string(),
                "dense-reject-g1".to_string(),
                None,
                Tone::Danger,
                format!("거부: {title}"),
            ),
        ]
    );
    let mut options = gate("g2", Some("b1"), None, 1);
    options.options = vec!["main에 병합".into(), "초안으로".into()];
    assert_eq!(
        model::gate_buttons(&options)
            .iter()
            .map(row)
            .collect::<Vec<_>>(),
        [
            (
                GateDecision::Approve,
                "main에 병합".to_string(),
                "dense-allow-g2-0".to_string(),
                Some("main에 병합".to_string()),
                Tone::Success,
                format!("선택: main에 병합: {title}"),
            ),
            (
                GateDecision::Approve,
                "초안으로".to_string(),
                "dense-allow-g2-1".to_string(),
                Some("초안으로".to_string()),
                Tone::Success,
                format!("선택: 초안으로: {title}"),
            ),
            (
                GateDecision::Reject,
                "거부".to_string(),
                "dense-reject-g2".to_string(),
                None,
                Tone::Danger,
                format!("거부: {title}"),
            ),
        ]
    );
    let (approve, reject) = (GateDecision::Approve, GateDecision::Reject);
    assert_eq!(model::button_state(None, approve), ControlState::Ready);
    assert_eq!(
        model::button_state(Some(approve), approve),
        ControlState::Loading
    );
    assert_eq!(
        model::button_state(Some(approve), reject),
        ControlState::Disabled
    );
}

#[test]
fn log_keys_walk_lines_and_regions_cycle() {
    use model::{LogKey::*, at_tail, cycle_step, log_key};
    assert_eq!(log_key("down", None, 0), None, "an empty log takes no key");
    assert_eq!(log_key("down", None, 10), Some(Select(9)));
    assert_eq!(log_key("down", Some(3), 10), Some(Select(4)));
    assert_eq!(log_key("down", Some(9), 10), Some(Select(9)));
    assert_eq!(log_key("up", None, 10), Some(Select(9)));
    assert_eq!(log_key("up", Some(0), 10), Some(Select(0)));
    assert_eq!(log_key("home", Some(5), 10), Some(Select(0)));
    assert_eq!(log_key("end", Some(5), 10), Some(Select(9)));
    assert_eq!(log_key("pagedown", Some(1), 10), Some(Select(1 + LOG_ROWS)));
    assert_eq!(log_key("pagedown", Some(8), 10), Some(Select(9)));
    assert_eq!(log_key("pagedown", None, 10), Some(Select(9)));
    assert_eq!(log_key("pageup", Some(8), 10), Some(Select(8 - LOG_ROWS)));
    assert_eq!(log_key("pageup", None, 10), Some(Select(0)));
    assert_eq!(
        log_key("enter", Some(2), 10),
        Some(Activate {
            line: Some(2),
            open: true
        })
    );
    assert_eq!(
        log_key("space", None, 10),
        Some(Activate {
            line: None,
            open: false
        })
    );
    assert_eq!(log_key("tab", Some(2), 10), None);
    // F6 from outside the regions starts at the first; both ways wrap.
    assert_eq!(cycle_step(None, 4, true), 0);
    assert_eq!(cycle_step(None, 4, false), 0);
    assert_eq!(cycle_step(Some(1), 4, true), 2);
    assert_eq!(cycle_step(Some(3), 4, true), 0);
    assert_eq!(cycle_step(Some(0), 4, false), 3);
    // The log follows new lines unless an older line is selected.
    assert!(at_tail(None, None));
    assert!(at_tail(None, Some(4)));
    assert!(at_tail(Some(4), Some(4)));
    assert!(at_tail(Some(3), None));
    assert!(!at_tail(Some(3), Some(4)));
}

#[test]
fn activity_bar_entries_keep_their_order_tips_and_commands() {
    let rows: Vec<(&str, &str, Option<Panel>, Option<UiIntent>)> = model::activity_bar()
        .into_iter()
        .map(|e| match e {
            BarEntry::Toggle { panel, id, tip, .. } => (id, tip, Some(panel), None),
            BarEntry::Command {
                id, tip, intent, ..
            } => (id, tip, None, Some(intent)),
            BarEntry::Spacer => ("", "", None, None),
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "dense-toggle-tree",
                "작업 탐색 보이기/숨기기",
                Some(Panel::Tree),
                None
            ),
            (
                "dense-files",
                "파일 (Ctrl+P)",
                None,
                Some(UiIntent::QuickOpenFiles { quick_open: true })
            ),
            (
                "dense-agents",
                "에이전트 트리 (Ctrl+Shift+E)",
                None,
                Some(UiIntent::OpenAgentTree)
            ),
            (
                "dense-search",
                "작업, 터미널, 명령 검색 (Ctrl+K)",
                None,
                Some(UiIntent::OpenJump)
            ),
            (
                "dense-gates",
                "승인 요청 (Ctrl+Shift+I)",
                None,
                Some(UiIntent::OpenGates)
            ),
            ("", "", None, None),
            (
                "dense-toggle-output",
                "출력 패널 (Ctrl+Shift+U)",
                Some(Panel::Output),
                None
            ),
            (
                "dense-toggle-details",
                "세부 정보 (Ctrl+Shift+D)",
                Some(Panel::Details),
                None
            ),
            (
                "dense-settings",
                "설정 (Ctrl+,)",
                None,
                Some(UiIntent::OpenSettings)
            ),
        ]
    );
}

#[test]
fn details_actions_text_and_selection() {
    // 터미널 열기 while it runs, else 다시 실행 with the same command, folder and task.
    let w = term("w1", Some("sample/a"), "working");
    assert_eq!(
        model::terminal_link(&w, "로그인 작업"),
        Link {
            id: "dense-open-terminal",
            label: "터미널 열기",
            name: "터미널 열기: 로그인 작업".into(),
            icon: Icon::SquareTerminal,
            act: Act::Intent(UiIntent::OpenTerminal {
                id: "w1".into(),
                another: false
            }),
        }
    );
    let mut x = w.clone();
    x.live = Liveness::Exited {
        code: Some(1),
        killed: false,
    };
    assert_eq!(
        model::terminal_link(&x, "로그인 작업"),
        Link {
            id: "dense-rerun",
            label: "다시 실행",
            name: "다시 실행: 로그인 작업".into(),
            icon: Icon::RotateCcw,
            act: Act::Rerun("w1".into()),
        }
    );
    let spec = model::rerun_spec(&x);
    assert_eq!(spec.argv, x.argv);
    assert_eq!(spec.cwd.as_deref(), Some(x.cwd.as_str()));
    assert_eq!(spec.task.as_deref(), Some("sample/a"));
    assert!(spec.name.is_none() && spec.cols.is_none() && spec.rows.is_none());
    // 변경 검토 · 파일 열기 (with a worktree) · PR 보기.
    let mut k = TestBackend::task("sample/a", "로그인 수정", "sample", &["w1"]);
    k.worktree = None;
    k.worktree_exists = false;
    let ids = |k: &TaskVm| {
        model::task_links(k)
            .iter()
            .map(|l| l.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(&k), ["dense-review", "dense-pr"]);
    k.worktree_exists = true;
    assert_eq!(ids(&k), ["dense-review", "dense-files-open", "dense-pr"]);
    k.worktree_exists = false;
    k.worktree = Some("C:/fixtures/worktrees/a".into());
    assert_eq!(
        model::task_links(&k)
            .into_iter()
            .map(|l| (l.label, l.name, l.icon, l.act))
            .collect::<Vec<_>>(),
        [
            (
                "변경 검토",
                "변경 검토: 로그인 수정".to_string(),
                Icon::GitCompareArrows,
                Act::Intent(UiIntent::OpenReview {
                    task: "sample/a".into()
                })
            ),
            (
                "파일 열기",
                "파일 열기: 로그인 수정".to_string(),
                Icon::FileText,
                Act::Intent(UiIntent::OpenEditor {
                    task: "sample/a".into(),
                    target: None,
                    quick_open: false
                })
            ),
            (
                "PR 보기",
                "PR 보기: 로그인 수정".to_string(),
                Icon::GitPullRequest,
                Act::Intent(UiIntent::OpenPullRequest {
                    task: "sample/a".into(),
                    form: false
                })
            ),
        ]
    );
    // PR line, counts and the log's first line, byte for byte.
    let pr = |state: &str, draft: bool| {
        serde_json::from_value::<p::TaskPr>(json!({
            "number": 12, "url": "", "title": "로그인 수정",
            "state": state, "draft": draft, "updated_ms": 0
        }))
        .expect("pr")
    };
    assert_eq!(model::pr_text(&pr("open", true)), "#12 초안 · 로그인 수정");
    assert_eq!(model::pr_text(&pr("open", false)), "#12 열림 · 로그인 수정");
    assert_eq!(
        model::pr_text(&pr("merged", false)),
        "#12 병합됨 · 로그인 수정"
    );
    assert_eq!(
        model::pr_text(&pr("closed", false)),
        "#12 닫힘 · 로그인 수정"
    );
    assert_eq!(copy::comments(3u32), "3개");
    assert_eq!(
        copy::started(2),
        "터미널 2개 · 기존 상태에서 확인이 필요한 것만"
    );
    // A vanished task or terminal shows nothing.
    let tasks = vec![k.clone()];
    let terminals = vec![w.clone()];
    let task = Sel::Task("sample/a".into());
    assert_eq!(
        model::shown_selection(Some(&task), &tasks, &terminals),
        Some(task)
    );
    assert_eq!(
        model::shown_selection(Some(&Sel::Terminal("gone".into())), &tasks, &terminals),
        None
    );
    assert_eq!(model::shown_selection(None, &tasks, &terminals), None);
    // 마지막 줄 is read again only when the status key changes.
    let mut t = term("w1", None, "working");
    let k0 = model::line_key(&t);
    assert_eq!(model::line_key(&t.clone()), k0);
    t.status.as_mut().unwrap().state = AgentState::Blocked;
    let k1 = model::line_key(&t);
    assert_ne!(k1, k0);
    t.live = Liveness::Exited {
        code: Some(0),
        killed: false,
    };
    assert_ne!(model::line_key(&t), k1);
}

#[test]
fn details_link_opens_its_terminal_through_the_host() {
    // A running terminal's 터미널 열기 goes to the host as an intent: Tab
    // from the column reaches it, Enter opens the terminal and focuses it.
    let _config = Config::new();
    let mut h = Harness::new("dense-link", seed);
    set_layout(&mut h, "dense-details");
    let v = dense(&mut h);
    h.cx.update(|_, cx| v.update(cx, |v, cx| v.select(Sel::Terminal("d1".into()), cx)));
    h.settle();
    assert_ne!(h.focused_terminal().as_deref(), Some("d1"));
    h.keys(FOCUS_DETAILS_KEY);
    assert!(details_focused(&mut h));
    h.press("tab");
    h.press("enter");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("d1"),
        "터미널 열기 opened d1"
    );
    assert_eq!(selected(&mut h), Some(Sel::Terminal("d1".into())));
}
