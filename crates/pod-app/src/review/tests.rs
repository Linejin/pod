//! Review policy tests without a window. The five tests at the top moved
//! from review.rs unchanged (`review::tests::*`); `editor_tests` was
//! `review::editor_tests` and is now `review::tests::editor_tests` (same test
//! name). The rest pin the plans A-P3 moved out of the controller and the
//! render code: each expectation is what the inline code produced.

use super::model::{
    became_unconfirmed, finished_deliveries, refusal_text, send_selection, submit_text, turn_ended,
};
use crate::source::SourceError;
use crate::vm::{AgentState, StatusVm};
use podd::protocol::{CommentState, DeliveryInfo, DeliveryState, ReviewComment, Side, orch_codes};
use std::collections::HashSet;

fn comment(id: &str, state: CommentState) -> ReviewComment {
    ReviewComment {
        id: id.into(),
        task: "r/t".into(),
        file: "a.rs".into(),
        side: Side::New,
        line: 1,
        end_line: None,
        body: "b".into(),
        code: None,
        state,
        created_ms: 0,
        delivery: None,
        resolved_ms: None,
        note: None,
    }
}

fn delivery(id: &str, state: DeliveryState) -> DeliveryInfo {
    DeliveryInfo {
        id: id.into(),
        task: "r/t".into(),
        terminal: "t1".into(),
        comment_ids: vec!["c1".into()],
        state,
        prompt: String::new(),
        created_ms: 0,
        delivered_ms: None,
        agent_state: "working".into(),
        delivered_state: None,
        app_bracketed_paste: None,
        error: None,
    }
}

#[test]
fn selection_or_all_open() {
    let cs = vec![
        comment("c1", CommentState::Open),
        comment("c2", CommentState::Resolved),
        comment("c3", CommentState::Sent),
    ];
    assert_eq!(send_selection(&cs, &HashSet::new()), None);
    let checked: HashSet<String> = ["c2", "c3"].map(String::from).into();
    assert_eq!(send_selection(&cs, &checked), Some(vec!["c3".to_string()]));
    // editor-11: checking only resolved comments sends nothing, never the
    // unchecked open ones.
    let only_resolved: HashSet<String> = ["c2"].map(String::from).into();
    assert_eq!(send_selection(&cs, &only_resolved), Some(vec![]));
    let stale: HashSet<String> = ["gone"].map(String::from).into();
    assert_eq!(
        send_selection(&cs, &stale),
        None,
        "ids no longer listed count as unchecked"
    );
}

#[test]
fn queued_to_delivered_is_reported_once() {
    let before = vec![
        delivery("d1", DeliveryState::Queued),
        delivery("d2", DeliveryState::Delivered),
    ];
    let after = vec![
        delivery("d1", DeliveryState::Delivered),
        delivery("d2", DeliveryState::Delivered),
        delivery("d3", DeliveryState::Queued),
    ];
    let f = finished_deliveries(&before, &after);
    assert_eq!(f.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), ["d1"]);
    assert!(finished_deliveries(&after, &after).is_empty());
}

#[test]
fn turn_end_detection() {
    let mut w = StatusVm::of(AgentState::Working);
    w.turn = 3;
    let mut idle = StatusVm::of(AgentState::Idle);
    idle.turn = 4;
    assert!(turn_ended(Some(&w), &idle));
    assert!(!turn_ended(None, &idle));
    assert!(!turn_ended(Some(&idle), &idle));
    let mut w2 = StatusVm::of(AgentState::Working);
    w2.turn = 4;
    assert!(!turn_ended(Some(&idle), &w2));
    // A shell without turns: working -> unknown still counts.
    assert!(turn_ended(
        Some(&StatusVm::of(AgentState::Working)),
        &StatusVm::of(AgentState::Unknown)
    ));
}

#[test]
fn refusal_messages() {
    let blocked = SourceError {
        code: Some(orch_codes::AGENT_BLOCKED),
        message: "agent is blocked:question".into(),
    };
    assert!(refusal_text(&blocked).starts_with("보낼 수 없습니다. 에이전트 응답 대기"));
    let other = SourceError {
        code: None,
        message: "pipe".into(),
    };
    assert_eq!(refusal_text(&other), "전송 실패: pipe");
}

#[test]
fn submit_check_lines_and_unconfirmed_toast_once() {
    use podd::protocol::{SubmitCheck, SubmitState};
    let check = |state| SubmitCheck {
        state,
        enters: 1,
        signal: None,
        entered_ms: None,
        settled_ms: None,
    };
    assert!(
        submit_text(&check(SubmitState::Unverifiable)).is_none(),
        "shells cannot confirm: say nothing"
    );
    assert_eq!(
        submit_text(&check(SubmitState::Confirmed)).unwrap().0,
        "에이전트가 받음"
    );
    assert!(
        submit_text(&check(SubmitState::Unconfirmed))
            .unwrap()
            .0
            .contains("입력칸")
    );
    let (pending, unconfirmed) = (check(SubmitState::Pending), check(SubmitState::Unconfirmed));
    assert!(became_unconfirmed(None, &unconfirmed));
    assert!(became_unconfirmed(Some(&pending), &unconfirmed));
    // The same state again (second event, or a deliveries reload): no second toast.
    assert!(!became_unconfirmed(Some(&unconfirmed), &unconfirmed));
    assert!(!became_unconfirmed(None, &check(SubmitState::Confirmed)));
}

mod editor_tests {
    use super::super::model::first_changed_line;
    use pod_diff::SourceFile;

    #[test]
    fn first_change_line_for_open_in_editor() {
        let f = |old: Option<&str>, new: Option<&str>| SourceFile {
            path: "a".into(),
            old_path: None,
            old: old.map(Into::into),
            new: new.map(Into::into),
            binary: false,
        };
        assert_eq!(
            first_changed_line(&f(Some("a\nb\nc\n"), Some("a\nB\nc\n"))),
            2
        );
        assert_eq!(first_changed_line(&f(None, Some("x\ny\n"))), 1);
        assert_eq!(first_changed_line(&f(Some("a\n"), Some("a\nb\n"))), 2);
        assert_eq!(first_changed_line(&f(Some("a\nb\n"), Some("a\n"))), 1);
    }
}

// ---- plans moved out of the controller and render code (A-P3) ----------------

mod plans {
    use std::collections::{HashMap, HashSet};

    use gpui_kit::component::diff::{DiffFileStatus, DiffSide};
    use pod_diff::{FileRow, SourceFile};
    use podd::protocol::{CommentState, DeliveryState, Side, SubmitCheck, SubmitState};

    use super::super::model::{
        SendOutcome, delivery_rows, draft_label, editor_line, finished_toast, first_change_target,
        inline_comments, line_totals, location, new_comment, notes_per_file, open_count,
        send_params, sent_toast, sidebar_plan, stepped_file,
    };
    use super::{comment, delivery};
    use crate::vm::{AgentState, StatusVm};
    use crate::workspace::ToastKind;

    fn row(path: &str, additions: usize, deletions: usize) -> FileRow {
        FileRow {
            path: path.to_string().into(),
            status: DiffFileStatus::Modified,
            additions,
            deletions,
            binary: false,
            eol_only: false,
        }
    }

    fn source(path: &str, old: Option<&str>, new: Option<&str>) -> SourceFile {
        SourceFile {
            path: path.into(),
            old_path: None,
            old: old.map(Into::into),
            new: new.map(Into::into),
            binary: false,
        }
    }

    #[test]
    fn sidebar_counts_send_label_and_agent_line() {
        let mut cs = vec![
            comment("c1", CommentState::Open),
            comment("c2", CommentState::Open),
            comment("c3", CommentState::Resolved),
            comment("c4", CommentState::Sent),
        ];
        cs[1].file = "b.rs".into();
        let none = HashSet::new();
        let p = sidebar_plan(&cs, &none, false, false, None, Some("w1"), None);
        let ids: Vec<&str> = p.visible.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["c1", "c2", "c4"], "resolved hidden");
        assert_eq!((p.resolved, p.open, p.total), (1, 2, 4));
        assert_eq!(p.send_label, "미해결 2개 보내기");
        assert!(p.can_send);
        assert_eq!(p.agent, "에이전트 w1");
        assert_eq!(p.refusal, None);

        let shown = sidebar_plan(&cs, &none, true, false, None, None, None);
        assert_eq!(shown.visible.len(), 4, "show resolved");
        assert_eq!(shown.agent, "에이전트 —");

        let checked: HashSet<String> = ["c3", "c4"].map(String::from).into();
        let working = StatusVm::of(AgentState::Working);
        let refused = SendOutcome::Refused("왜".into());
        let p = sidebar_plan(
            &cs,
            &checked,
            false,
            true,
            Some(&working),
            Some("w1"),
            Some(&refused),
        );
        assert_eq!(
            p.send_label, "선택한 1개 보내기",
            "resolved checks do not count"
        );
        assert!(!p.can_send, "not while a send is in flight");
        assert_eq!(
            p.agent,
            format!("에이전트 w1: {} · 대기 상태가 되면 전달", working.label())
        );
        assert_eq!(p.refusal.as_deref(), Some("왜"));

        let blocked = StatusVm::of(AgentState::Blocked);
        let idle = StatusVm::of(AgentState::Idle);
        let agent =
            |s: &StatusVm, t| sidebar_plan(&cs, &none, false, false, Some(s), t, None).agent;
        assert!(agent(&blocked, None).ends_with(" · 지금은 보낼 수 없음"));
        assert!(agent(&blocked, None).starts_with("에이전트 ?: "));
        assert!(agent(&idle, Some("w1")).ends_with(" · 바로 전달"));
        let delivered = SendOutcome::Delivery("d1".into());
        assert_eq!(
            sidebar_plan(&cs, &none, false, false, None, None, Some(&delivered)).refusal,
            None
        );

        let only_resolved: HashSet<String> = ["c3"].map(String::from).into();
        let p = sidebar_plan(&cs, &only_resolved, false, false, None, None, None);
        assert_eq!(p.send_label, "선택한 0개 보내기");
        assert!(p.can_send, "the click explains that nothing is sent");
        let all_resolved = [comment("c3", CommentState::Resolved)];
        assert!(!sidebar_plan(&all_resolved, &none, false, false, None, None, None).can_send);
    }

    #[test]
    fn delivery_rows_are_the_newest_four_with_checks_and_actions() {
        let mut ds: Vec<_> = (1..=5)
            .map(|i| delivery(&format!("d{i}"), DeliveryState::Delivered))
            .collect();
        ds[4].state = DeliveryState::Queued;
        let check = |state| SubmitCheck {
            state,
            enters: 2,
            signal: None,
            entered_ms: None,
            settled_ms: None,
        };
        let submits: HashMap<String, SubmitCheck> = [
            ("d4".to_string(), check(SubmitState::Unconfirmed)),
            ("d5".to_string(), check(SubmitState::Unconfirmed)),
            ("d3".to_string(), check(SubmitState::Confirmed)),
        ]
        .into();
        let rows = delivery_rows(&ds, &submits);
        let ids: Vec<&str> = rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["d5", "d4", "d3", "d2"]);
        assert_eq!(
            rows[0].text,
            "d5 · 의견 1개 · 에이전트 대기 중 전송 예약 (이전 작업 중)"
        );
        assert!(rows[0].queued && !rows[0].unconfirmed && rows[0].submit.is_none());
        assert!(
            rows[1].unconfirmed && !rows[1].queued,
            "delivered + unconfirmed"
        );
        assert_eq!(
            rows[1].submit.as_ref().map(|s| s.0.as_str()),
            Some("제출 확인 안 됨: 입력칸에 남아 있을 수 있음")
        );
        assert_eq!(
            rows[2].submit.as_ref().map(|s| s.0.as_str()),
            Some("에이전트가 받음")
        );
        assert!(!rows[2].unconfirmed);
        assert_eq!(
            (rows[3].terminal.as_str(), rows[3].submit.is_none()),
            ("t1", true)
        );
    }

    #[test]
    fn delivery_toasts_title_kind_and_text() {
        let mut d = delivery("d1", DeliveryState::Delivered);
        assert_eq!(
            finished_toast(&d, "r/t"),
            (
                ToastKind::Success,
                "전달됨 리뷰 · r/t".to_string(),
                "d1 · 의견 1개 · t1에 전달 (작업 중)".to_string()
            )
        );
        assert_eq!(
            sent_toast(&d, "r/t"),
            (
                ToastKind::Success,
                "r/t에 리뷰를 보냈습니다".to_string(),
                "d1 · 의견 1개 · t1에 전달 (작업 중)".to_string()
            )
        );
        d.state = DeliveryState::Cancelled;
        assert_eq!(
            finished_toast(&d, "r/t"),
            (
                ToastKind::Warning,
                "전달 안 됨 리뷰 · r/t".to_string(),
                "d1 · 취소됨".to_string()
            )
        );
        d.state = DeliveryState::Queued;
        assert_eq!(
            sent_toast(&d, "r/t"),
            (
                ToastKind::Info,
                "r/t에 리뷰 전송을 예약했습니다".to_string(),
                "d1 · 의견 1개 · 에이전트 대기 중 전송 예약 (이전 작업 중)".to_string()
            )
        );
    }

    #[test]
    fn send_params_and_the_saved_comment_keep_their_wire_shape() {
        let all = send_params("r/t", None, String::new());
        assert_eq!(
            serde_json::to_string(&all).unwrap(),
            r#"{"task":"r/t","all_open":true,"dry_run":false}"#
        );
        let some = send_params("r/t", Some(vec!["c1".into()]), "먼저".into());
        assert_eq!(
            serde_json::to_string(&some).unwrap(),
            r#"{"task":"r/t","comment_ids":["c1"],"all_open":false,"instructions":"먼저","dry_run":false}"#
        );
        let none = send_params("r/t", Some(vec![]), String::new());
        assert_eq!(
            serde_json::to_string(&none).unwrap(),
            r#"{"task":"r/t","all_open":false,"dry_run":false}"#
        );

        let sources = [source(
            "a.rs",
            Some("\u{feff}x\ny\n"),
            Some("\u{feff}x\nY\nz\n"),
        )];
        let one = new_comment(&sources, "a.rs".into(), Side::New, 1, 1, "b".into());
        assert_eq!(
            serde_json::to_string(&one).unwrap(),
            r#"{"file":"a.rs","side":"new","line":1,"body":"b","code":"x\n"}"#,
            "BOM dropped from the quote"
        );
        let range = new_comment(&sources, "a.rs".into(), Side::Old, 1, 2, "b".into());
        assert_eq!(range.end_line, Some(2));
        assert_eq!(range.code.as_deref(), Some("x\ny\n"));
        let missing = new_comment(&sources, "gone.rs".into(), Side::New, 3, 3, "b".into());
        assert_eq!((missing.end_line, missing.code), (None, None));
    }

    #[test]
    fn file_steps_wrap_and_toolbar_counts_add_up() {
        assert_eq!(stepped_file(0, 0, true), None);
        assert_eq!(stepped_file(0, 3, true), Some(1));
        assert_eq!(stepped_file(2, 3, true), Some(0));
        assert_eq!(stepped_file(0, 3, false), Some(2));
        assert_eq!(stepped_file(1, 3, false), Some(0));
        assert_eq!(line_totals(&[row("a", 3, 1), row("b", 0, 7)]), (3, 8));
        assert_eq!(line_totals(&[]), (0, 0));

        let mut cs = vec![
            comment("c1", CommentState::Open),
            comment("c2", CommentState::Queued),
            comment("c3", CommentState::Resolved),
            comment("c4", CommentState::Open),
        ];
        cs[3].file = "b.rs".into();
        let notes = notes_per_file(&cs);
        assert_eq!((notes.get("a.rs"), notes.get("b.rs")), (Some(&2), Some(&1)));
        assert_eq!(open_count(&cs), 2);
    }

    #[test]
    fn inline_cards_skip_resolved_and_files_outside_the_diff() {
        let mut cs = vec![
            comment("c1", CommentState::Open),
            comment("c2", CommentState::Resolved),
            comment("c3", CommentState::Sent),
            comment("c4", CommentState::Open),
        ];
        cs[2].end_line = Some(5);
        cs[3].file = "gone.rs".into();
        let at: Vec<(&str, usize)> = inline_comments(&cs, &[row("a.rs", 1, 0)])
            .into_iter()
            .map(|(c, line)| (c.id.as_str(), line))
            .collect();
        assert_eq!(at, [("c1", 1), ("c3", 5)], "each below its last line");

        assert_eq!(draft_label(Side::New, 3, 3), "변경:3");
        assert_eq!(draft_label(Side::Old, 3, 5), "이전:3–5");
        let mut c = comment("c1", CommentState::Open);
        assert_eq!(location(&c), "a.rs:1 (변경)");
        c.end_line = Some(4);
        assert_eq!(location(&c), "a.rs:1–4 (변경)");
        c.end_line = Some(1);
        assert_eq!(
            location(&c),
            "a.rs:1 (변경)",
            "a one-line range reads as a line"
        );
    }

    #[test]
    fn editor_targets_given_then_hovered_then_first_change() {
        let sources = [
            source("a.rs", Some("a\nb\n"), Some("a\nB\n")),
            source("gone.rs", Some("x\n"), None),
        ];
        assert_eq!(editor_line(Some(7), Some(3), &sources, "a.rs"), 7);
        assert_eq!(editor_line(Some(0), None, &sources, "a.rs"), 1, "never 0");
        assert_eq!(editor_line(None, Some(3), &sources, "a.rs"), 3);
        assert_eq!(editor_line(None, None, &sources, "a.rs"), 2);
        assert_eq!(editor_line(None, None, &sources, "other.rs"), 1);

        assert_eq!(
            first_change_target(&sources[0]),
            ("a.rs".to_string(), DiffSide::Modified, 2)
        );
        assert_eq!(
            first_change_target(&sources[1]),
            ("gone.rs".to_string(), DiffSide::Original, 1),
            "a deleted file is commented on its old side"
        );
    }
}
