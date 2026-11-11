use crate::layouts::blocks_model::{
    Approval, Block, BlockKey, FeedBlock, Kind, Origin, TermBlocks, block_name, build_feed,
    cycle_target, decision, default_expanded, gates_by_terminal, nav, state_label, status_text,
    task_terminals, who,
};
use crate::source::TestBackend;
use core::prelude::v1::test;
use podd::protocol::{GateDecision, GateInfo, GateState};
use std::collections::HashMap;

pub(crate) fn gate(
    id: &str,
    terminal: Option<&str>,
    task: Option<&str>,
    options: &[&str],
) -> GateInfo {
    GateInfo {
        id: id.into(),
        title: "명령 실행".into(),
        message: "cargo clippy 실행 승인이 필요합니다.".into(),
        task: task.map(Into::into),
        group: None,
        terminal: terminal.map(Into::into),
        state: GateState::Pending,
        created_ms: 5,
        resolved_ms: None,
        note: None,
        expires_ms: None,
        run: None,
        item: None,
        options: options.iter().map(|s| s.to_string()).collect(),
        choice: None,
        requested_by: None,
        decided_by: None,
    }
}

fn block(
    seq: u64,
    kind: Kind,
    title: &str,
    at: Option<u64>,
    end: Option<u64>,
    exit: Option<i32>,
) -> Block {
    Block {
        seq,
        kind,
        origin: if at.is_some() {
            Origin::Marks
        } else {
            Origin::Earlier
        },
        title: title.into(),
        lines: vec!["줄".into()],
        dropped: 0,
        started_ms: at,
        ended_ms: end,
        exit,
        open: end.is_none() && at.is_some(),
    }
}

#[test]
fn feed_merges_terminals_chronologically_with_gates_on_the_newest_block() {
    let task = TestBackend::task("pod/a", "글자 정렬", "pod", &["a1", "a2", "a3"]);
    let mut terms = vec![
        TestBackend::terminal("a1", "구현", Some("pod/a"), "working"),
        TestBackend::terminal("a2", "리뷰", Some("pod/a"), "blocked"),
        TestBackend::terminal("a3", "빌드", Some("pod/a"), "done"),
    ];
    terms[0].program = "claude".into();
    terms[1].program = "codex".into();
    let refs = task_terminals(&task, &terms);
    assert_eq!(who(refs[0], "구현"), "Claude · 구현");
    assert_eq!(who(refs[2], "빌드"), "셸 · 빌드");
    let mut blocks = HashMap::new();
    blocks.insert(
        "a1".to_string(),
        vec![
            block(0, Kind::Output, "", None, None, None),
            block(
                1,
                Kind::Command,
                "cargo test",
                Some(100),
                Some(3200),
                Some(0),
            ),
            block(2, Kind::Step, "작업 중", Some(5000), None, None),
        ],
    );
    blocks.insert(
        "a3".to_string(),
        vec![block(
            0,
            Kind::Command,
            "git status --short",
            Some(4000),
            Some(4400),
            Some(0),
        )],
    );
    let gates = gates_by_terminal(
        &refs,
        "pod/a",
        &[
            gate("g1", None, Some("pod/a"), &[]),
            gate("g2", Some("a2"), None, &[]),
        ],
    );
    assert_eq!(
        (gates["a1"].id.as_str(), gates["a2"].id.as_str()),
        ("g1", "g2"),
        "task gate → first terminal, own gate wins"
    );
    let feed = build_feed(&refs, &blocks, &gates, None, |t| {
        t.name.clone().unwrap_or_default()
    });
    let order: Vec<(&str, u64)> = feed
        .iter()
        .map(|f| (f.key.terminal.as_str(), f.key.seq))
        .collect();
    assert_eq!(
        order,
        [("a1", 0), ("a2", u64::MAX), ("a1", 1), ("a3", 0), ("a1", 2)],
        "earlier output first, then by start time"
    );
    let a1_last = feed
        .iter()
        .find(|f| {
            f.key
                == BlockKey {
                    terminal: "a1".into(),
                    seq: 2,
                }
        })
        .unwrap();
    assert!(a1_last.gate.is_some() && a1_last.last);
    assert!(
        feed.iter()
            .filter(|f| f.key.terminal == "a1")
            .filter(|f| f.gate.is_some())
            .count()
            == 1,
        "only the newest block carries the gate"
    );
    let approval = &feed[1];
    assert_eq!(
        (
            approval.block.title.as_str(),
            approval.gate.as_ref().map(|g| g.id.as_str())
        ),
        ("승인 요청", Some("g2")),
        "a gate without blocks still shows"
    );
    // Filter: one terminal.
    let only = build_feed(&refs, &blocks, &gates, Some("a3"), |t| t.id.clone());
    assert_eq!(only.len(), 1);
    // Status texts and default folds.
    let st: Vec<String> = feed.iter().map(status_text).collect();
    assert_eq!(
        st,
        [
            "이전 출력",
            "승인 대기",
            "3.1초 · 종료 0",
            "0.4초 · 종료 0",
            "승인 대기"
        ]
    );
    let exp: Vec<bool> = feed.iter().map(default_expanded).collect();
    assert_eq!(
        exp,
        [false, true, false, true, true],
        "finished older blocks start folded"
    );
}

#[test]
fn approval_buttons_map_to_gate_resolve_decisions() {
    let plain = gate("g", None, None, &[]);
    assert_eq!(
        decision(&plain, Approval::Once),
        Some((GateDecision::Approve, None))
    );
    assert_eq!(
        decision(&plain, Approval::Always),
        None,
        "항상 허용 only when the gate offers it"
    );
    assert_eq!(
        decision(&plain, Approval::Reject),
        Some((GateDecision::Reject, None))
    );
    let opts = gate("g", None, None, &["항상 허용", "이번만 허용"]);
    assert_eq!(
        decision(&opts, Approval::Once),
        Some((GateDecision::Approve, Some("이번만 허용".into())))
    );
    assert_eq!(
        decision(&opts, Approval::Always),
        Some((GateDecision::Approve, Some("항상 허용".into())))
    );
    let en = gate("g", None, None, &["Yes", "Yes, and don't ask again"]);
    assert_eq!(
        decision(&en, Approval::Once).map(|d| d.1),
        Some(Some("Yes".into()))
    );
    assert_eq!(
        decision(&en, Approval::Always).map(|d| d.1),
        Some(Some("Yes, and don't ask again".into()))
    );
    assert_eq!(
        decision(&opts, Approval::Reject),
        Some((GateDecision::Reject, None)),
        "reject never carries a choice"
    );
}

#[test]
fn keyboard_navigation_and_input_target_cycle() {
    let key = |s: u64| BlockKey {
        terminal: "t".into(),
        seq: s,
    };
    let fb = |s: u64| FeedBlock {
        key: key(s),
        who: "셸 · t".into(),
        kind: crate::agent_kind::AgentKind::Shell,
        block: block(s, Kind::Command, "x", Some(s), Some(s + 1), Some(0)),
        gate: None,
        last: false,
    };
    let feed = vec![fb(0), fb(1), fb(2)];
    assert_eq!(
        nav(&feed, None, "up"),
        Some(key(2)),
        "no selection: the newest"
    );
    assert_eq!(nav(&feed, Some(&key(1)), "up"), Some(key(0)));
    assert_eq!(nav(&feed, Some(&key(0)), "up"), None);
    assert_eq!(nav(&feed, Some(&key(1)), "down"), Some(key(2)));
    assert_eq!(nav(&feed, Some(&key(2)), "down"), None);
    assert_eq!(nav(&feed, Some(&key(2)), "home"), Some(key(0)));
    assert_eq!(nav(&feed, Some(&key(0)), "end"), Some(key(2)));
    let mut terms = [
        TestBackend::terminal("a", "a", None, "working"),
        TestBackend::terminal("b", "b", None, "done"),
        TestBackend::terminal("c", "c", None, "idle"),
    ];
    terms[1].live = crate::vm::Liveness::Exited {
        code: Some(0),
        killed: false,
    };
    let refs: Vec<_> = terms.iter().collect();
    assert_eq!(cycle_target(&refs, None, false).as_deref(), Some("a"));
    assert_eq!(
        cycle_target(&refs, Some("a"), false).as_deref(),
        Some("c"),
        "exited terminals are skipped"
    );
    assert_eq!(
        cycle_target(&refs, Some("c"), false).as_deref(),
        Some("a"),
        "wraps"
    );
    assert_eq!(cycle_target(&refs, Some("a"), true).as_deref(), Some("c"));
    assert_eq!(cycle_target(&refs[1..2], None, false), None);
}

// ── A-P13: what moved into `copy`, the kit metrics and the given clock ──

#[test]
fn moved_copy_keeps_the_text_of_the_inline_strings() {
    use super::copy as s;
    use crate::vm::AgentState;
    let steps: Vec<(&str, bool)> = [
        AgentState::Working,
        AgentState::Blocked,
        AgentState::Done,
        AgentState::Idle,
        AgentState::Error,
        AgentState::Unknown,
        AgentState::Exited,
    ]
    .into_iter()
    .map(state_label)
    .collect();
    assert_eq!(
        steps,
        [
            ("작업 중", false),
            ("응답 필요", false),
            ("완료", true),
            ("입력 대기", true),
            ("오류", true),
            ("상태 확인 중", false),
            ("에이전트 종료", true),
        ]
    );
    assert_eq!(s::exit_code(-1), "종료 -1");
    assert_eq!(s::omitted(3), "… 앞 3줄 생략");
    assert_eq!(s::task_row("로그인-수정", 0), "로그인-수정");
    assert_eq!(s::task_row("로그인-수정", 2), "로그인-수정, 승인 대기 2개");
    assert_eq!(s::chip("셸 · 빌드", true), "셸 · 빌드, 선택됨");
    assert_eq!(s::chip("셸 · 빌드", false), "셸 · 빌드");
    assert_eq!(s::all_agents(3), "모든 에이전트 3");
    assert_eq!(s::task_list(4), "작업, 4개");
    assert_eq!(s::meta(5, 2), "블록 5 · 에이전트 2 · 시간순");
    assert_eq!(s::needs_you(1), "응답 필요 1");
    assert_eq!(s::feed(0), "블록 피드, 블록 0개");
    assert_eq!(
        s::input_placeholder("Codex · 리뷰 봇"),
        "Codex · 리뷰 봇에게 입력 · Tab으로 대상 변경"
    );
    assert_eq!(
        s::input_row(s::NO_TARGET),
        "입력: 입력할 터미널이 없습니다 · Tab 대상 변경 · Esc 블록으로 · F6 영역 이동"
    );
    assert_eq!(
        (s::COPY_TIP, s::WAITING_FOR_OUTPUT, s::NO_OUTPUT),
        ("복사 (C)", "출력 기다리는 중…", "출력 없음")
    );
    // The accessible name with a gate, both folds.
    let f = FeedBlock {
        key: BlockKey {
            terminal: "t".into(),
            seq: 0,
        },
        who: "셸 · t".into(),
        kind: crate::agent_kind::AgentKind::Shell,
        block: block(0, Kind::Command, "cargo build", Some(1), None, None),
        gate: Some(gate("g", Some("t"), None, &[])),
        last: true,
    };
    assert_eq!(
        block_name(&f, true),
        "셸 · t, cargo build, 승인 대기, 승인 요청: cargo clippy 실행 승인이 필요합니다., 펼쳐짐"
    );
    assert!(block_name(&f, false).ends_with(", 접힘"));
}

#[test]
fn task_rows_grow_with_the_kit_line_as_before() {
    use crate::layouts::kit::LayoutMetrics;
    use pod_ui::themes::{Active, ThemeChoice};
    let reg = pod_ui::themes::registry();
    for style in &reg.styles {
        let choice = ThemeChoice {
            style: style.id.clone(),
            ..Default::default()
        };
        let m = Active::new(&reg, &choice).metrics;
        for font in [10., 13., 16., 20., 28.] {
            // The formula the layout had (`Feed::row_height` before A-P13).
            let ui = crate::theme::ui_font_size(font, m.density.ui_font);
            let line = (ui * 1.5).ceil();
            // `hit_area` reads the active style, which other tests may switch.
            let (kit, before) = loop {
                let hit = pod_ui::hit_area();
                let kit = super::view::row_height(&LayoutMetrics::new(font, m, ""));
                if pod_ui::hit_area() == hit {
                    break (kit, hit.max(line + 8.));
                }
            };
            assert_eq!(kit, before, "{} {font}", style.id);
        }
    }
}

#[test]
fn fixture_scenes_use_the_clock_they_are_given() {
    let now = 1_800_000_000_000;
    let blocks = |id: &str| {
        let mut b = TermBlocks::new(100, 30);
        b.reset(b"", 100, 30, 0);
        super::controller::fixture_feed(id, &mut b, now);
        b.blocks
    };
    // Same instant, same blocks; the commands sit two minutes before it.
    assert_eq!(blocks("fixture-0"), blocks("fixture-0"));
    let first = blocks("fixture-0");
    assert_eq!(
        first
            .iter()
            .filter(|b| b.kind == Kind::Command)
            .map(|b| (b.title.as_str(), b.started_ms, b.duration_ms()))
            .collect::<Vec<_>>(),
        [
            (
                "cargo test -p pod-ui wide_glyph",
                Some(now - 120_000),
                Some(3_100)
            ),
            ("cargo build -p pod-ui", Some(now - 90_000), None),
        ]
    );
    let g = super::controller::fixture_gate(now);
    assert_eq!(
        (g.created_ms, g.terminal.as_deref()),
        (now - 12_000, Some("fixture-1"))
    );
}

/// fx-layouts, the panel drawer: it opens on a rise of the dock's panel count
/// (not on the first frame), F6 visits it only while it is open, and its
/// width leaves the task list and some feed beside it.
#[test]
fn the_panel_drawer_opens_on_a_rise_and_is_in_f6_only_while_open() {
    use super::controller::{Region, next_region};
    use crate::layouts::blocks_model::{drawer_width, panels_rose};
    use Region::{Drawer, Feed, Input, Tasks};

    assert!(
        !panels_rose(usize::MAX, 3),
        "first frame: nothing to compare"
    );
    assert!(panels_rose(0, 1) && panels_rose(2, 3));
    assert!(!panels_rose(1, 1) && !panels_rose(2, 1), "same or fewer");

    let cycle = |raw: bool, drawer: bool| {
        let mut r = Tasks;
        let mut seen = vec![r];
        for _ in 0..4 {
            r = next_region(r, raw, drawer);
            seen.push(r);
        }
        seen
    };
    assert_eq!(cycle(false, false), [Tasks, Feed, Input, Tasks, Feed]);
    assert_eq!(cycle(false, true), [Tasks, Feed, Input, Drawer, Tasks]);
    // The raw view has no input row.
    assert_eq!(cycle(true, true), [Tasks, Feed, Drawer, Tasks, Feed]);
    assert_eq!(cycle(true, false), [Tasks, Feed, Tasks, Feed, Tasks]);
    assert_eq!(next_region(Drawer, false, false), Tasks);

    // 960 px window (1440 physical at 150 %): the feed keeps 360 px.
    assert_eq!(drawer_width(960., 240.), 360.);
    assert!((drawer_width(1440., 260.) - 604.8).abs() < 0.01);
    assert_eq!(drawer_width(1920., 280.), 720., "at most 720");
    assert_eq!(drawer_width(800., 240.), 280., "at least 280");
}
