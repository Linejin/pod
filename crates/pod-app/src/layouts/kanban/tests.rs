//! Kanban board: the pure model (plain tests). The live board on GPUI's
//! headless platform is in `layouts::kanban_qa`.

// Not `super::*` at the top: gpui's glob exports its own `test` attribute.
use super::{BoardModel, Card, Column, Nav, Refused, build_cards, card_name, filter_choices};
use crate::source::TestBackend;
use crate::vm::{Liveness, TerminalVm};
use core::prelude::v1::test;

use super::copy;
use super::model::{
    CardAction, CardButton, CardKey, Stripe, card_buttons, card_height, card_key, column_of,
    dock_rose, drawer_height, follow_focus, nav_key, shown_line, stripe, task_count,
};

fn term(id: &str, state: &str, task: Option<&str>) -> TerminalVm {
    TestBackend::terminal(id, &format!("이름 {id}"), task, state)
}

fn gate(id: &str, terminal: Option<&str>, task: Option<&str>) -> podd::protocol::GateInfo {
    podd::protocol::GateInfo {
        id: id.into(),
        title: "배포 승인".into(),
        message: "main에 병합할까요?".into(),
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

#[test]
fn columns_come_from_the_agent_state_and_lifecycle() {
    let cases = [
        ("working", Liveness::Running, Column::Working, false),
        ("unknown", Liveness::Running, Column::Working, false),
        ("blocked", Liveness::Running, Column::NeedsYou, false),
        ("error", Liveness::Running, Column::NeedsYou, true),
        ("done", Liveness::Running, Column::Done, false),
        ("idle", Liveness::Running, Column::Done, false),
        (
            "working",
            Liveness::Exited {
                code: Some(0),
                killed: false,
            },
            Column::Done,
            false,
        ),
        (
            "working",
            Liveness::Exited {
                code: Some(1),
                killed: false,
            },
            Column::NeedsYou,
            true,
        ),
        (
            "working",
            Liveness::Exited {
                code: Some(1),
                killed: true,
            },
            Column::Done,
            false,
        ),
        ("working", Liveness::Lost, Column::NeedsYou, true),
    ];
    for (state, live, column, failed) in cases {
        let mut t = term("t1", state, None);
        t.live = live;
        assert_eq!(column_of(&t), (column, failed), "{state} {live:?}");
    }
    let mut plain = term("t2", "working", None);
    plain.status = None;
    assert_eq!(
        column_of(&plain),
        (Column::Working, false),
        "a shell without agent status is 작업 중"
    );
}

fn sample() -> Vec<Card> {
    let tasks = vec![
        TestBackend::task("pod/a", "글자 정렬", "pod", &["a1", "a2"]),
        TestBackend::task("web/b", "설정 화면", "web", &["b1"]),
    ];
    let mut terms = vec![
        term("a1", "working", Some("pod/a")),
        term("a2", "working", Some("pod/a")),
        term("b1", "blocked", Some("web/b")),
        term("c1", "working", None),
    ];
    terms[3].program = "codex".into();
    terms[3].argv = vec!["codex".into()];
    build_cards(&terms, &tasks, &[gate("g1", None, Some("web/b"))], |t| {
        t.name.clone().unwrap_or_default()
    })
}

#[test]
fn cards_reuse_workspace_data_and_gates() {
    let cards = sample();
    assert_eq!(cards.len(), 4);
    assert_eq!(cards[0].title, "Powershell · 이름 a1");
    assert_eq!(
        (cards[0].task_title.as_str(), cards[0].repo.as_deref()),
        ("글자 정렬", Some("pod"))
    );
    assert_eq!(
        cards[2].gate.as_ref().map(|g| g.id.as_str()),
        Some("g1"),
        "a task gate goes to the task's first terminal"
    );
    assert_eq!(cards[3].task_title, "작업 없음");
    assert!(!cards[0].rerun);
    let name = card_name(&cards[2], Some("PS> y/n"), true);
    assert!(
        name.contains("응답 필요")
            && name.contains("승인 요청: 배포 승인")
            && name.contains("마지막 줄: PS> y/n")
            && name.ends_with("고정됨"),
        "{name}"
    );
    let (repos, agents) = filter_choices(&cards);
    assert_eq!(repos, ["pod", "web"]);
    assert_eq!(agents, ["Codex", "Powershell"]);
    // A terminal gate wins over a task gate.
    let tasks = vec![TestBackend::task(
        "web/b",
        "설정 화면",
        "web",
        &["b1", "b2"],
    )];
    let terms = vec![
        term("b1", "blocked", Some("web/b")),
        term("b2", "blocked", Some("web/b")),
    ];
    let cards = build_cards(
        &terms,
        &tasks,
        &[gate("g2", Some("b2"), Some("web/b"))],
        |t| t.id.clone(),
    );
    assert_eq!(
        (
            cards[0].gate.is_none(),
            cards[1].gate.as_ref().map(|g| g.id.as_str())
        ),
        (true, Some("g2"))
    );
}

fn ids(col: &[Card]) -> Vec<&str> {
    col.iter().map(|c| c.id.as_str()).collect()
}

#[test]
fn drag_reorders_and_pins_within_a_column_but_never_changes_state() {
    let cards = sample();
    let mut m = BoardModel::default();
    assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "a2", "c1"]);
    m.reorder(&cards, "c1", Some("a1")).unwrap();
    assert_eq!(ids(&m.columns(&cards)[0]), ["c1", "a1", "a2"]);
    m.reorder(&cards, "c1", None).unwrap();
    assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "a2", "c1"]);
    // Another column: refused, nothing moves, the column is unchanged.
    let before = m.clone();
    assert_eq!(
        m.reorder(&cards, "a1", Some("b1")),
        Err(Refused::StateIsTheAgents)
    );
    assert_eq!(
        m.pin_top(&cards, "a1", Column::NeedsYou),
        Err(Refused::StateIsTheAgents)
    );
    assert_eq!(m, before);
    assert_eq!(m.columns(&cards)[1][0].column, Column::NeedsYou);
    // Pin to the top; later cards stay below it.
    m.pin_top(&cards, "a2", Column::Working).unwrap();
    assert_eq!(ids(&m.columns(&cards)[0]), ["a2", "a1", "c1"]);
    assert!(m.pinned.contains("a2"));
    // Keyboard: Alt+↓ swaps with the neighbour and takes its (unpinned) state.
    m.move_by(&cards, "a2", true).unwrap();
    assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "a2", "c1"]);
    assert!(!m.pinned.contains("a2"));
    m.move_by(&cards, "c1", false).unwrap();
    assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "c1", "a2"]);
    m.move_by(&cards, "a1", false).unwrap();
    assert_eq!(
        ids(&m.columns(&cards)[0]),
        ["a1", "c1", "a2"],
        "top stays top"
    );
    // The order survives a state change: a card that moves column keeps its rank when it returns.
    let mut moved = cards.clone();
    moved[1].column = Column::Done;
    assert_eq!(ids(&m.columns(&moved)[2]), ["a2"]);
    assert_eq!(ids(&m.columns(&cards)[0]), ["a1", "c1", "a2"]);
}

#[test]
fn filters_and_keyboard_navigation() {
    let cards = sample();
    let mut m = BoardModel {
        repo: Some("pod".into()),
        ..Default::default()
    };
    let cols = m.columns(&cards);
    assert_eq!((ids(&cols[0]), cols[1].len()), (vec!["a1", "a2"], 0));
    m.repo = None;
    m.agent = Some("Codex".into());
    assert_eq!(ids(&m.columns(&cards)[0]), ["c1"]);
    m.agent = None;
    let cols = m.columns(&cards);
    assert_eq!(
        m.nav(&cols, None, Nav::Down).as_deref(),
        Some("a1"),
        "no selection: first card"
    );
    assert_eq!(m.nav(&cols, Some("a1"), Nav::Down).as_deref(), Some("a2"));
    assert_eq!(m.nav(&cols, Some("a1"), Nav::Up), None);
    assert_eq!(
        m.nav(&cols, Some("a2"), Nav::Right).as_deref(),
        Some("b1"),
        "row clamps to the shorter column"
    );
    assert_eq!(
        m.nav(&cols, Some("b1"), Nav::Right),
        None,
        "완료 is empty: nowhere to go"
    );
    assert_eq!(m.nav(&cols, Some("b1"), Nav::Left).as_deref(), Some("a1"));
    assert_eq!(m.nav(&cols, Some("a1"), Nav::End).as_deref(), Some("c1"));
    // Selection follows filters.
    m.selected = Some("b1".into());
    m.repo = Some("pod".into());
    let cols = m.columns(&cards);
    m.settle_selection(&cols);
    assert_eq!(m.selected.as_deref(), Some("a1"));
}

// ── helpers split out of the board's controller and render (A-P12) ──

#[test]
fn card_keys_open_move_pin_and_navigate() {
    // Real key events: Alt is one of the modifiers.
    assert_eq!(card_key("enter", false, false), Some(CardKey::Open));
    assert_eq!(card_key("enter", false, true), None, "Ctrl/Shift+Enter");
    assert_eq!(card_key("enter", true, true), None, "Alt+Enter");
    assert_eq!(
        card_key("down", false, false),
        Some(CardKey::Nav(Nav::Down))
    );
    assert_eq!(card_key("end", false, false), Some(CardKey::Nav(Nav::End)));
    assert_eq!(
        card_key("up", true, true),
        Some(CardKey::Move { down: false })
    );
    assert_eq!(
        card_key("down", true, true),
        Some(CardKey::Move { down: true })
    );
    assert_eq!(card_key("home", true, true), Some(CardKey::PinTop));
    for key in ["left", "right", "end"] {
        assert_eq!(
            card_key(key, true, true),
            None,
            "Alt+{key} is not the card's"
        );
    }
    assert_eq!(card_key("up", false, true), None, "Ctrl/Shift+↑");
    assert_eq!(card_key("tab", false, false), None);
    assert_eq!(card_key("space", false, false), None);
    // The board's own keys (no card focused yet) are the same six.
    let keys: Vec<Option<Nav>> = [
        "up", "down", "left", "right", "home", "end", "enter", "pageup",
    ]
    .into_iter()
    .map(nav_key)
    .collect();
    assert_eq!(
        keys,
        [
            Some(Nav::Up),
            Some(Nav::Down),
            Some(Nav::Left),
            Some(Nav::Right),
            Some(Nav::Home),
            Some(Nav::End),
            None,
            None
        ]
    );
}

fn decide(gate: &str, approve: bool, choice: Option<&str>, label: &str) -> CardAction {
    CardAction::Decide {
        gate: gate.into(),
        approve,
        choice: choice.map(Into::into),
        label: label.into(),
    }
}

fn button(id: &str, action: CardAction) -> CardButton {
    CardButton {
        id: id.into(),
        action,
    }
}

#[test]
fn card_buttons_carry_the_gate_choice_in_tab_order() {
    let cards = sample();
    // A gate without options: approve, reject.
    assert_eq!(
        card_buttons(&cards[2]),
        [
            button(
                "kanban-approve-b1",
                decide("g1", true, None, crate::strings::APPROVE)
            ),
            button(
                "kanban-reject-b1",
                decide("g1", false, None, crate::strings::REJECT)
            ),
        ]
    );
    // v17 options: one approve button per option, each with its choice; reject has none.
    let mut options = cards[2].clone();
    if let Some(g) = options.gate.as_mut() {
        g.options = vec!["스테이징 먼저".into(), "바로 운영".into()];
    }
    assert_eq!(
        card_buttons(&options),
        [
            button(
                "kanban-approve-b1-0",
                decide("g1", true, Some("스테이징 먼저"), "스테이징 먼저")
            ),
            button(
                "kanban-approve-b1-1",
                decide("g1", true, Some("바로 운영"), "바로 운영")
            ),
            button(
                "kanban-reject-b1",
                decide("g1", false, None, crate::strings::REJECT)
            ),
        ]
    );
    assert_eq!(copy::approve_option("바로 운영"), "승인: 바로 운영");
    // A question without a gate: 응답하기. Working: 로그.
    let mut question = cards[2].clone();
    question.gate = None;
    assert_eq!(
        card_buttons(&question),
        [button("kanban-answer-b1", CardAction::Answer)]
    );
    assert_eq!(
        card_buttons(&cards[0]),
        [button("kanban-log-a1", CardAction::Log)]
    );
    // A failed worker: 로그, then 다시 실행.
    let mut x = term("x1", "working", None);
    x.live = Liveness::Exited {
        code: Some(1),
        killed: false,
    };
    let failed = build_cards(&[x], &[], &[], |t| t.id.clone());
    assert_eq!(
        (failed[0].column, failed[0].failed),
        (Column::NeedsYou, true)
    );
    assert_eq!(
        card_buttons(&failed[0]),
        [
            button("kanban-log-x1", CardAction::Log),
            button("kanban-rerun-x1", CardAction::Rerun),
        ]
    );
}

#[test]
fn card_and_drawer_heights() {
    // 12×2 padding + two text lines + the output line (+8) + the action row + 6×3 gaps + border.
    assert_eq!(card_height(20., 28., 1.), 24. + 40. + 28. + 28. + 18. + 2.);
    assert_eq!(
        card_height(30., 28., 1.),
        24. + 60. + 38. + 36. + 18. + 2.,
        "a tall line wins over the hit area"
    );
    // Nothing opened: header (32 at least) + one line + 32.
    assert_eq!(drawer_height(true, false, 28., 13., 900.), 32. + 19.5 + 32.);
    assert_eq!(drawer_height(true, true, 40., 13., 900.), 40. + 19.5 + 32.);
    // Opened: 32% of the window (60% grown), 160 at least.
    assert_eq!(drawer_height(false, false, 28., 13., 1000.), 320.);
    assert_eq!(drawer_height(false, true, 28., 13., 1000.), 600.);
    assert_eq!(drawer_height(false, false, 28., 13., 400.), 160.);
}

#[test]
fn focus_from_elsewhere_and_new_panels_open_the_drawer() {
    let cards = sample();
    let mut m = BoardModel::default();
    let mut seen = None;
    assert!(follow_focus(&mut m, &mut seen, Some("b1"), &cards));
    assert_eq!(
        (m.selected.as_deref(), m.opened.as_deref(), seen.as_deref()),
        (Some("b1"), Some("b1"), Some("b1"))
    );
    // The same focus next frame changes nothing (the user moved the selection).
    m.selected = Some("a1".into());
    assert!(!follow_focus(&mut m, &mut seen, Some("b1"), &cards));
    assert_eq!(m.selected.as_deref(), Some("a1"));
    // A terminal without a card is remembered; nothing opens.
    assert!(!follow_focus(&mut m, &mut seen, Some("zz"), &cards));
    assert_eq!(
        (seen.as_deref(), m.opened.as_deref()),
        (Some("zz"), Some("b1"))
    );
    assert!(!follow_focus(&mut m, &mut seen, None, &cards));
    assert_eq!(seen, None);
    // Non-terminal dock panels: only a rise after the first frame.
    assert!(!dock_rose(usize::MAX, 3), "first frame");
    assert!(dock_rose(1, 2));
    assert!(!dock_rose(2, 2));
    assert!(!dock_rose(2, 1));
}

#[test]
fn copy_names_lines_stripes_and_counts() {
    let cards = sample();
    assert_eq!(
        card_name(&cards[2], Some("PS> y/n"), true),
        format!(
            "{}, 응답 필요: {}, 작업: 설정 화면, 승인 요청: 배포 승인, 마지막 줄: PS> y/n, 고정됨",
            cards[2].title, cards[2].status
        )
    );
    assert_eq!(
        card_name(&cards[0], Some(""), false),
        format!(
            "{}, 작업 중: {}, 작업: 글자 정렬",
            cards[0].title, cards[0].status
        ),
        "an empty line is left out"
    );
    assert_eq!(copy::counts(4, 2), "에이전트 4 · 작업 2");
    assert_eq!(copy::column_name(copy::NEEDS_YOU, 2), "응답 필요, 카드 2개");
    assert_eq!(
        copy::filter(copy::REPO, copy::ALL_REPOS),
        "저장소: 모든 저장소"
    );
    assert_eq!(
        task_count(&[
            term("a1", "working", Some("pod/a")),
            term("a2", "working", Some("pod/a")),
            term("b1", "blocked", Some("web/b")),
            term("c1", "working", None),
        ]),
        2
    );
    // The line under the task: the gate's message, else the last line, else 출력 없음.
    assert_eq!(
        shown_line(&cards[2], Some("x".into())),
        "main에 병합할까요?"
    );
    assert_eq!(shown_line(&cards[0], Some("x".into())), "x");
    assert_eq!(shown_line(&cards[0], None), "출력 없음");
    // Stripes: 응답 필요 warning, failed danger, none elsewhere.
    assert_eq!(stripe(&cards[2]), Some(Stripe::Warning));
    let mut failed = cards[2].clone();
    failed.failed = true;
    assert_eq!(stripe(&failed), Some(Stripe::Danger));
    assert_eq!(stripe(&cards[0]), None);
    let mut done = cards[0].clone();
    done.column = Column::Done;
    assert_eq!(stripe(&done), None);
}
