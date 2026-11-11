//! The workbench Review panel on GPUI's headless platform (native QA,
//! docs/10 §12): one task's Review, keyboard only, against a scripted podd
//! double that keeps the task's comments and deliveries. These scenarios pin
//! what the A-P3 split moves: the exact request params (J), request and toast
//! counts, what a failure keeps, the comment column's width, focus and word
//! wrapping (B). No mouse event is simulated; a checkbox click is the same
//! state change the click handler makes. Run with `--nocapture` for the
//! `[native-qa]` step log.

use core::prelude::v1::test;
use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::*;
use podd::protocol::{
    CommentState, DeliveryInfo, DeliveryState, NewComment, ReviewComment, Side, SubmitCheck,
    SubmitState, orch_codes,
};
use serde_json::{Value, json};

use super::{ReviewPanel, SendOutcome};
use crate::native_qa::{Harness, seed_two_tasks};
use crate::source::{SourceError, SourceEvent, TestBackend};
use crate::workspace::ToastKind;

const TASK: &str = "sample/로그인-수정";

/// The review half of podd for [`TASK`].
#[derive(Default)]
struct Reviews {
    comments: Vec<ReviewComment>,
    deliveries: Vec<DeliveryInfo>,
    /// Answers of the next `review.send` calls, in order.
    sends: Vec<Result<DeliveryInfo, SourceError>>,
    /// `review.comments.add` fails with this message.
    add_error: Option<String>,
}

fn comment(id: &str, line: u32, body: &str, state: CommentState) -> ReviewComment {
    ReviewComment {
        id: id.into(),
        task: TASK.into(),
        file: "src/main.rs".into(),
        side: Side::New,
        line,
        end_line: None,
        body: body.into(),
        code: None,
        state,
        created_ms: 1,
        delivery: None,
        resolved_ms: None,
        note: None,
    }
}

fn delivery(id: &str, state: DeliveryState, ids: &[&str]) -> DeliveryInfo {
    DeliveryInfo {
        id: id.into(),
        task: TASK.into(),
        terminal: "w1".into(),
        comment_ids: ids.iter().map(|s| s.to_string()).collect(),
        state,
        prompt: "p".into(),
        created_ms: 1,
        delivered_ms: None,
        agent_state: "working".into(),
        delivered_state: None,
        app_bracketed_paste: None,
        error: None,
    }
}

fn serve(
    state: Rc<RefCell<Reviews>>,
) -> impl FnMut(&str, &Value) -> Option<Result<Value, SourceError>> {
    move |method, params| {
        let mut s = state.borrow_mut();
        Some(match method {
            "review.comments.list" => Ok(json!({ "comments": s.comments })),
            "review.deliveries" => Ok(json!({ "deliveries": s.deliveries })),
            "review.comments.add" => match s.add_error.clone() {
                Some(message) => Err(SourceError {
                    code: None,
                    message,
                }),
                None => {
                    let new: Vec<NewComment> =
                        serde_json::from_value(params["comments"].clone()).ok()?;
                    let mut added = Vec::new();
                    for c in new {
                        let mut x = comment(
                            &format!("c{}", s.comments.len() + 1),
                            c.line,
                            &c.body,
                            CommentState::Open,
                        );
                        (x.file, x.side, x.end_line, x.code) = (c.file, c.side, c.end_line, c.code);
                        s.comments.push(x.clone());
                        added.push(x);
                    }
                    Ok(json!({ "comments": added }))
                }
            },
            "review.send" => {
                let answer = if s.sends.is_empty() {
                    Err(SourceError {
                        code: None,
                        message: "no scripted answer".into(),
                    })
                } else {
                    s.sends.remove(0)
                };
                if let Ok(d) = &answer {
                    s.deliveries.push(d.clone());
                }
                answer.map(|d| json!(d))
            }
            _ => return None,
        })
    }
}

/// 1440×960, the task's worktree is the editor fixture repo (three changed
/// files), its Review is open in the workbench dock.
fn harness(name: &'static str, state: Rc<RefCell<Reviews>>) -> Harness {
    let wt = std::env::temp_dir().join(format!("pod-native-qa-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&wt);
    let wt = pod_editor::fixture::create(&wt, true).expect("fixture repo");
    let mut h = Harness::new(name, move |b: &mut TestBackend| {
        seed_two_tasks(b);
        b.tasks[0].worktree = Some(wt.to_string_lossy().into_owned());
        b.worktrees.insert(TASK.into(), wt);
        b.rpc = Some(Box::new(serve(state)));
    });
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_review(TASK, window, cx)));
    h.settle();
    h
}

fn review(h: &mut Harness) -> Entity<ReviewPanel> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_reviews()
            .into_iter()
            .find(|r| r.read(cx).task_id == TASK)
    })
    .expect("the task's Review")
}

fn focus_review(h: &mut Harness, r: &Entity<ReviewPanel>) {
    let f = h.cx.update(|_, cx| r.read(cx).focus_handle());
    h.cx.update(|window, cx| window.focus(&f, cx));
    h.settle();
}

/// Review requests (and the diff it reads) since `from`, as podd received them.
fn requests(h: &Harness, from: usize) -> Vec<String> {
    h.backend.calls[from..]
        .iter()
        .filter(|c| c.starts_with("review.") || c.starts_with("task.diff"))
        .cloned()
        .collect()
}

fn toasts(h: &mut Harness) -> Vec<(ToastKind, String, String)> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .toasts
            .iter()
            .map(|t| (t.kind, t.title.to_string(), t.body.to_string()))
            .collect()
    })
}

fn deliver(h: &mut Harness, d: DeliveryInfo, submit: Option<SubmitCheck>) {
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Delivery(Box::new(d), submit));
    h.settle();
}

fn instructions(h: &mut Harness, r: &Entity<ReviewPanel>) -> String {
    h.cx.update(|_, cx| r.read(cx).instructions.read(cx).value().to_string())
}

const OPEN_READS: [&str; 3] = [
    "task.diff {\"task\":\"sample/로그인-수정\"}",
    "review.comments.list {\"task\":\"sample/로그인-수정\"}",
    "review.deliveries {\"task\":\"sample/로그인-수정\"}",
];

/// J: opening, F5, Ctrl+Shift+C → Enter and Ctrl+Shift+Enter send what they
/// sent before, once each and in this order; the keys move files and toggle
/// the comment column.
#[test]
fn keyboard_review_sends_the_same_requests() {
    let s = Rc::new(RefCell::new(Reviews::default()));
    s.borrow_mut()
        .sends
        .push(Ok(delivery("d1", DeliveryState::Queued, &["c1"])));
    let mut h = harness("review-qa-wire", s.clone());
    assert_eq!(
        requests(&h, 0),
        OPEN_READS,
        "open reads diff, comments, deliveries"
    );
    let r = review(&mut h);
    assert_eq!(h.cx.update(|_, cx| r.read(cx).files.len()), 3);

    focus_review(&mut h, &r);
    let mark = h.backend.calls.len();
    h.keys("f5");
    assert_eq!(requests(&h, mark), OPEN_READS, "F5 re-reads all three");

    // Files by keyboard: Alt+PageDown / Alt+PageUp wrap around.
    let file = |h: &mut Harness| h.cx.update(|_, cx| r.read(cx).current_file);
    h.keys("alt-pagedown");
    assert_eq!(file(&mut h), 1);
    h.keys("alt-pageup alt-pageup");
    assert_eq!(file(&mut h), 2, "wraps to the last file");
    h.keys("alt-pagedown");
    assert_eq!(file(&mut h), 0);

    // No pointer: Ctrl+Shift+C opens a draft at the current file's first change.
    h.keys("ctrl-shift-c");
    assert!(
        h.focus_name().starts_with("review panel text input"),
        "draft focused: {}",
        h.focus_name()
    );
    let draft = h.cx.update(|_, cx| {
        r.read(cx)
            .draft
            .as_ref()
            .map(|d| (d.path.clone(), d.side, d.line, d.end_line))
    });
    assert_eq!(draft, Some(("README.md".to_string(), Side::New, 9, 9)));
    let mark = h.backend.calls.len();
    h.type_text("check this");
    h.keys("enter");
    assert_eq!(
        requests(&h, mark),
        [
            "review.comments.add {\"task\":\"sample/로그인-수정\",\"comments\":[{\"file\":\"README.md\",\"side\":\"new\",\"line\":9,\"body\":\"check this\",\"code\":\"- `docs/` — 한글 문서\\n\"}]}"
        ],
        "saved once with the quoted line"
    );
    assert!(h.cx.update(|_, cx| r.read(cx).draft.is_none()));
    assert_eq!(h.cx.update(|_, cx| r.read(cx).comments.len()), 1);

    // Nothing checked: Ctrl+Shift+Enter sends every open comment.
    focus_review(&mut h, &r);
    let mark = h.backend.calls.len();
    h.keys("ctrl-shift-enter");
    assert_eq!(
        requests(&h, mark),
        [
            "review.send {\"task\":\"sample/로그인-수정\",\"all_open\":true,\"dry_run\":false}",
            "review.comments.list {\"task\":\"sample/로그인-수정\"}",
        ],
        "one send, then the comments again"
    );
    assert_eq!(
        h.cx.update(|_, cx| r.read(cx).outcome.clone()),
        Some(SendOutcome::Delivery("d1".into()))
    );

    // Alt+C hides and shows the comment column (wide canvas: no drawer).
    assert!(h.bounds("review-comments").is_some());
    h.keys("alt-c");
    assert!(
        h.bounds("review-comments").is_none(),
        "Alt+C hides the column"
    );
    h.keys("alt-c");
    assert!(
        h.bounds("review-comments").is_some(),
        "Alt+C shows it again"
    );
}

/// B: a queued delivery that goes out is reported once (event, repeated
/// event, then a re-read), a failed one once as a warning, and an
/// unconfirmed submit once; every event re-reads the comments.
#[test]
fn queued_delivery_is_reported_once() {
    let s = Rc::new(RefCell::new(Reviews::default()));
    s.borrow_mut().comments = vec![
        comment("c1", 2, "first", CommentState::Queued),
        comment("c2", 3, "second", CommentState::Queued),
    ];
    s.borrow_mut().deliveries = vec![
        delivery("d1", DeliveryState::Queued, &["c1"]),
        delivery("d2", DeliveryState::Queued, &["c2"]),
    ];
    let mut h = harness("review-qa-delivery", s.clone());
    let r = review(&mut h);
    assert!(
        toasts(&mut h).is_empty(),
        "loading queued deliveries says nothing"
    );

    let mark = h.backend.calls.len();
    s.borrow_mut().deliveries[0].state = DeliveryState::Delivered;
    deliver(
        &mut h,
        delivery("d1", DeliveryState::Delivered, &["c1"]),
        None,
    );
    let delivered = (
        ToastKind::Success,
        "전달됨 리뷰 · sample/로그인-수정".to_string(),
        "d1 · 의견 1개 · w1에 전달 (작업 중)".to_string(),
    );
    assert_eq!(toasts(&mut h), std::slice::from_ref(&delivered));
    assert_eq!(
        requests(&h, mark),
        ["review.comments.list {\"task\":\"sample/로그인-수정\"}"]
    );

    // The same event again, then a re-read that agrees: no second toast.
    let mark = h.backend.calls.len();
    deliver(
        &mut h,
        delivery("d1", DeliveryState::Delivered, &["c1"]),
        None,
    );
    focus_review(&mut h, &r);
    h.keys("f5");
    assert_eq!(
        toasts(&mut h),
        std::slice::from_ref(&delivered),
        "reported once"
    );
    assert_eq!(
        requests(&h, mark),
        [
            "review.comments.list {\"task\":\"sample/로그인-수정\"}",
            "task.diff {\"task\":\"sample/로그인-수정\"}",
            "review.comments.list {\"task\":\"sample/로그인-수정\"}",
            "review.deliveries {\"task\":\"sample/로그인-수정\"}",
        ]
    );

    // A queued delivery that fails: one warning.
    let mut failed = delivery("d2", DeliveryState::Failed, &["c2"]);
    failed.error = Some("terminal exited".into());
    s.borrow_mut().deliveries[1] = failed.clone();
    deliver(&mut h, failed.clone(), None);
    deliver(&mut h, failed, None);
    let t = toasts(&mut h);
    assert_eq!(
        t[1..],
        [(
            ToastKind::Warning,
            "전달 안 됨 리뷰 · sample/로그인-수정".to_string(),
            "d2 · 실패: terminal exited".to_string(),
        )]
    );

    // v8 submit check: an unconfirmed Enter warns once and offers Enter.
    let check = |state| SubmitCheck {
        state,
        enters: 2,
        signal: None,
        entered_ms: None,
        settled_ms: None,
    };
    deliver(
        &mut h,
        delivery("d1", DeliveryState::Delivered, &["c1"]),
        Some(check(SubmitState::Unconfirmed)),
    );
    deliver(
        &mut h,
        delivery("d1", DeliveryState::Delivered, &["c1"]),
        Some(check(SubmitState::Unconfirmed)),
    );
    let t = toasts(&mut h);
    assert_eq!(
        t[2..],
        [(
            ToastKind::Warning,
            "리뷰 제출 확인 안 됨".to_string(),
            "sample/로그인-수정 · d1: 에이전트 입력칸을 확인하고 Enter를 누르세요".to_string(),
        )],
        "unconfirmed is said once"
    );
    assert!(
        h.bounds("rv-submit-w0").is_some(),
        "the submit line is shown"
    );
    h.cx.update(|_, cx| r.update(cx, |p, cx| p.press_enter("w1", cx)));
    h.settle();
    assert_eq!(h.backend.typed_text("w1"), "\r", "one Enter to the agent");
}

/// B: a refused send keeps the instructions and the checked comments and
/// says why under the send box; the next send that goes through clears both.
/// A failed comment save closes the draft and says so (pinned as it is).
#[test]
fn refused_send_keeps_the_instructions_and_checks() {
    let s = Rc::new(RefCell::new(Reviews::default()));
    s.borrow_mut().comments = vec![
        comment("c1", 2, "first", CommentState::Open),
        comment("c2", 3, "second", CommentState::Open),
    ];
    s.borrow_mut().sends = vec![
        Err(SourceError {
            code: Some(orch_codes::AGENT_BLOCKED),
            message: "agent is blocked:question".into(),
        }),
        Ok(delivery("d1", DeliveryState::Queued, &["c1"])),
    ];
    let mut h = harness("review-qa-refused", s.clone());
    let r = review(&mut h);
    focus_review(&mut h, &r);
    h.cx.update(|window, cx| r.update(cx, |p, cx| p.instructions_focus(window, cx)));
    h.settle();
    h.type_text("run tests first");
    h.cx.update(|_, cx| {
        r.update(cx, |p, cx| {
            p.checked.insert("c1".into());
            cx.notify();
        })
    });
    h.settle();

    let mark = h.backend.calls.len();
    h.keys("ctrl-shift-enter");
    assert_eq!(
        requests(&h, mark),
        [
            "review.send {\"task\":\"sample/로그인-수정\",\"comment_ids\":[\"c1\"],\"all_open\":false,\"instructions\":\"run tests first\",\"dry_run\":false}",
            "review.comments.list {\"task\":\"sample/로그인-수정\"}",
        ]
    );
    let refused = "보낼 수 없습니다. 에이전트 응답 대기: agent is blocked:question".to_string();
    assert_eq!(
        toasts(&mut h),
        [(
            ToastKind::Warning,
            "sample/로그인-수정에 리뷰를 보내지 못했습니다".to_string(),
            refused.clone()
        )]
    );
    assert_eq!(
        instructions(&mut h, &r),
        "run tests first",
        "instructions kept"
    );
    let checked: Vec<String> =
        h.cx.update(|_, cx| r.read(cx).checked.iter().cloned().collect());
    assert_eq!(checked, ["c1"], "the check stays");
    assert_eq!(
        h.cx.update(|_, cx| r.read(cx).outcome.clone()),
        Some(SendOutcome::Refused(refused))
    );
    assert!(h.bounds("rv-outcome-w0").is_some(), "the refusal is shown");

    h.keys("ctrl-shift-enter");
    assert_eq!(
        instructions(&mut h, &r),
        "",
        "a delivery clears the instructions"
    );
    assert!(h.cx.update(|_, cx| r.read(cx).checked.is_empty()));
    assert!(h.bounds("rv-outcome-w0").is_none());
    assert!(
        h.bounds("rv-delivery-w0").is_some(),
        "the delivery line is shown"
    );
    assert_eq!(
        toasts(&mut h)[1],
        (
            ToastKind::Info,
            "sample/로그인-수정에 리뷰 전송을 예약했습니다".to_string(),
            "d1 · 의견 1개 · 에이전트 대기 중 전송 예약 (이전 작업 중)".to_string(),
        )
    );

    // A comment podd does not store: one request, the draft is gone, an error toast.
    s.borrow_mut().add_error = Some("disk full".into());
    focus_review(&mut h, &r);
    h.keys("ctrl-shift-c");
    h.type_text("lost");
    let mark = h.backend.calls.len();
    h.keys("enter");
    assert_eq!(requests(&h, mark).len(), 1);
    assert!(h.cx.update(|_, cx| r.read(cx).draft.is_none()));
    assert_eq!(
        toasts(&mut h)[2],
        (
            ToastKind::Error,
            "의견을 저장하지 못했습니다".to_string(),
            "disk full".to_string()
        )
    );
}

/// B: the comment column keeps its width with side-by-side on and off, the
/// Review keeps focus, and a long Korean comment wraps between words inside
/// the column.
#[test]
fn split_keeps_the_column_width_focus_and_word_wrap() {
    let s = Rc::new(RefCell::new(Reviews::default()));
    s.borrow_mut().comments = vec![comment(
        "c1",
        2,
        "이 함수는 입력 문자열을 두 번 읽으므로 큰 파일에서 느려질 수 있습니다 한 번만 읽도록 바꿔 주세요",
        CommentState::Open,
    )];
    let mut h = harness("review-qa-split", s);
    let r = review(&mut h);
    focus_review(&mut h, &r);
    let focus = h.cx.update(|_, cx| r.read(cx).focus_handle());
    let column = h.bounds("review-comments").expect("comment column");
    let columns = h.bounds("review-columns").expect("Review columns");
    assert_eq!(f32::from(column.size.width), super::COMMENTS_W);

    let words = |h: &mut Harness| {
        let col = h.bounds("review-comments").expect("comment column");
        let mut rows = Vec::new();
        for i in 0.. {
            let Some(b) = h.bounds(&format!("rv-body-w{i}")) else {
                break;
            };
            assert!(f32::from(b.size.height) < 24., "word {i} on one line {b:?}");
            assert!(
                b.left() >= col.left() - px(0.5) && b.right() <= col.right() + px(0.5),
                "word {i} inside the column"
            );
            if !rows.contains(&b.top()) {
                rows.push(b.top());
            }
        }
        rows.len()
    };
    assert!(words(&mut h) >= 2, "the comment wraps between words");

    for split in [true, false] {
        h.cx.update(|_, cx| r.update(cx, |p, cx| p.set_split(split, cx)));
        h.settle();
        let (on, wrap) = h.cx.update(|_, cx| (r.read(cx).split, r.read(cx).wrap));
        assert_eq!((on, wrap), (split, split), "side by side wraps lines");
        assert_eq!(h.bounds("review-comments"), Some(column), "split {split}");
        assert_eq!(h.bounds("review-columns"), Some(columns), "split {split}");
        assert!(
            h.is_focused(&focus),
            "split {split}: focus stays in the Review"
        );
        assert!(words(&mut h) >= 2, "split {split}: still wraps");
    }
}
