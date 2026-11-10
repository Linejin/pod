//! Native-QA flows of the orchestration screens (docs/19 S7): the real
//! workspace, key bindings, palette and panels on GPUI's headless platform
//! ([`crate::native_qa::Harness`]), data from the fixture run served by the
//! in-process test backend. Keyboard only; no mouse event is simulated.
//!
//! Proves: Ctrl+Shift+J / Ctrl+Shift+M and the palette open the screens;
//! the board acts on a worker card by keyboard (↓ then K → `orch.worker.keep`);
//! the next unanswered question's reply box takes Enter (`orch.mail.reply`,
//! box cleared); nothing is re-read without an event (no polling), and each
//! event re-reads only its run; an escalation raises a toast; stop asks first.

use std::cell::RefCell;
use std::rc::Rc;

// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;
use podd::protocol::{MailType, OrchChangedEvent, OrchMailEvent};
use serde_json::{Value, json};

use super::{Page, state};
use crate::native_qa::{Harness, seed_two_tasks};
use crate::source::{SourceEvent, TestBackend};

type Calls = Rc<RefCell<Vec<(String, Value)>>>;

/// The fixture run behind the test backend; mutating calls are recorded and
/// answered like podd would.
fn orch_backend(b: &mut TestBackend) -> Calls {
    seed_two_tasks(b);
    let calls: Calls = Rc::default();
    let log = calls.clone();
    b.rpc = Some(Box::new(move |method, params| {
        if !method.starts_with("orch.")
            && !method.starts_with("task.cleanup")
            && !method.starts_with("task.archive")
        {
            return None;
        }
        if method == "orch.limits.set" {
            log.borrow_mut().push((method.to_string(), params.clone()));
            return super::fixture::answer("orch.limits.get", params);
        }
        log.borrow_mut().push((method.to_string(), params.clone()));
        Some(Ok(match method {
            "orch.mail.reply" => {
                json!({"id": "m9", "run": "r1", "from": "human", "to": "attempt:a2", "recipients": ["attempt:a2"], "type": "answer",
                "subject": "re: 세션 만료", "body": params["body"], "reply_to": params["id"], "thread": params["id"], "created_ms": 1})
            }
            "orch.worker.keep"
            | "orch.worker.release"
            | "orch.worker.stop"
            | "orch.worker.abandon" => {
                let a = params["attempt"].as_str().unwrap_or_default();
                json!({"attempt": {"id": a, "run": "r1", "item": "i2", "generation": 1, "state": "ready", "worker": "kept", "agent": "claude", "place": "new", "depth": 1, "created_ms": 1, "terminal": "fixture-6"},
                    "item_title": "세션 만료 처리 고치기", "next": "사람이 유지"})
            }
            "task.archive" => json!({"task": params["task"], "terminals": []}),
            _ => return super::fixture::answer(method, params),
        }))
    }));
    calls
}

fn count(calls: &Calls, method: &str) -> usize {
    calls.borrow().iter().filter(|(m, _)| m == method).count()
}

fn page_focused(h: &mut Harness, page: Page) -> bool {
    h.cx.update(|window, cx| {
        let p = state(cx)
            .pages
            .iter()
            .find(|(p, w)| *p == page && w.upgrade().is_some())
            .and_then(|(_, w)| w.upgrade());
        p.is_some_and(|p| p.read(cx).focus.contains_focused(window, cx))
    })
}

fn toasts(h: &mut Harness) -> Vec<String> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .toasts
            .iter()
            .map(|t| t.title.to_string())
            .collect()
    })
}

fn palette(h: &mut Harness, query: &str) {
    h.keys("ctrl-k");
    assert!(h.dialog_open(), "Ctrl+K opened the palette");
    h.type_text(query);
    h.keys("enter");
    assert!(!h.dialog_open(), "Enter ran the command");
}

#[test]
fn board_and_inbox_by_keyboard_events_only() {
    let mut calls = None;
    let mut h = Harness::new("orch-board", |b| calls = Some(orch_backend(b)));
    let calls = calls.unwrap();
    // The status stream subscribed: one full read, no more.
    let _ = h.backend.events.unbounded_send(SourceEvent::OrchResync);
    h.settle();
    assert_eq!(count(&calls, "orch.run.list"), 1);
    assert_eq!(
        (
            count(&calls, "orch.worker.list"),
            count(&calls, "orch.mail.list")
        ),
        (2, 2),
        "workers and mail of both runs"
    );
    let runs = h.cx.update(|_, cx| state(cx).runs.len());
    assert_eq!(runs, 2);

    // Ctrl+Shift+J: the board, focused.
    h.keys("ctrl-shift-j");
    assert!(
        page_focused(&mut h, Page::Board),
        "board took focus: {}",
        h.focus_name()
    );
    // No events, no reads (no polling), however long we wait.
    let before = calls.borrow().len();
    for _ in 0..3 {
        h.cx.executor()
            .advance_clock(std::time::Duration::from_secs(30));
        h.settle();
    }
    assert_eq!(
        calls.borrow().len(),
        before,
        "nothing re-read without an event"
    );

    // ↓ selects the first card (open workers first: a2), K keeps it.
    h.keys("down");
    assert_eq!(
        h.cx.update(|_, cx| state(cx).selected_card.clone())
            .as_deref(),
        Some("a2")
    );
    h.keys("k");
    assert_eq!(count(&calls, "orch.worker.keep"), 1);
    assert_eq!(
        calls
            .borrow()
            .iter()
            .find(|(m, _)| m == "orch.worker.keep")
            .unwrap()
            .1["attempt"],
        "a2"
    );
    assert!(toasts(&mut h).iter().any(|t| t == super::s::ACTION_DONE));
    // S asks before stopping (Esc cancels, nothing sent).
    h.keys("s");
    assert!(h.dialog_open(), "stop asks first");
    h.keys("escape");
    assert!(!h.dialog_open());
    assert_eq!(count(&calls, "orch.worker.stop"), 0, "Esc sent nothing");

    // An event re-reads only its run.
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::OrchChanged(Box::new(OrchChangedEvent {
            seq: 1,
            run: "r1".into(),
            item: None,
            attempt: Some("a3".into()),
        })));
    h.settle();
    let shows: Vec<Value> = calls
        .borrow()
        .iter()
        .filter(|(m, _)| m == "orch.run.show")
        .map(|(_, p)| p.clone())
        .collect();
    assert_eq!(shows, vec![json!({"run": "r1"})]);
    assert_eq!(
        count(&calls, "orch.run.list"),
        1,
        "no full reload for one change"
    );

    // An escalation: a toast, and that run's mail is re-read.
    let mail_before = count(&calls, "orch.mail.list");
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::OrchMail(Box::new(OrchMailEvent {
            seq: 2,
            run: "r1".into(),
            to: "run:r1".into(),
            id: "m10".into(),
            kind: MailType::Escalation,
            subject: "worker a2 ended its turn without `pod orch done`".into(),
            text: Some(
                podd::protocol::MsgText::new(podd::protocol::msg_text::TURN_ENDED)
                    .with("attempt", "a2"),
            ),
            from: "attempt:a2".into(),
        })));
    h.settle();
    assert!(
        toasts(&mut h)
            .iter()
            .any(|t| t.starts_with(super::s::NOTICE_ESCALATION)),
        "{:?}",
        toasts(&mut h)
    );
    // docs/19 §16.3: the daemon's structured text is shown in Korean.
    let ws = h.ws.clone();
    let bodies: Vec<String> = h.cx.update(|_, cx| {
        ws.read(cx)
            .toasts
            .iter()
            .map(|t| t.body.to_string())
            .collect()
    });
    assert!(
        bodies
            .iter()
            .any(|b| b.starts_with("워커 a2: `pod orch done` 없이 턴을 끝냈습니다")),
        "{bodies:?}"
    );
    assert!(
        !bodies.iter().any(|b| b.contains("ended its turn")),
        "{bodies:?}"
    );
    assert_eq!(count(&calls, "orch.mail.list"), mail_before + 1);
    // A worker's ordinary report is not a person's notification.
    let n = toasts(&mut h).len();
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::OrchMail(Box::new(OrchMailEvent {
            seq: 3,
            run: "r1".into(),
            to: "run:r1".into(),
            id: "m11".into(),
            kind: MailType::Done,
            subject: "done".into(),
            text: None,
            from: "attempt:a1".into(),
        })));
    h.settle();
    assert_eq!(toasts(&mut h).len(), n);

    // Palette → next question: the inbox opens with the oldest open
    // question's reply box focused; Enter sends it.
    palette(&mut h, "다음 답할 질문");
    assert!(
        h.focus_name().contains("text input"),
        "reply box focused: {}",
        h.focus_name()
    );
    h.type_text("401 keep");
    h.keys("enter");
    let replies: Vec<Value> = calls
        .borrow()
        .iter()
        .filter(|(m, _)| m == "orch.mail.reply")
        .map(|(_, p)| p.clone())
        .collect();
    assert_eq!(replies.len(), 1);
    assert_eq!(
        (replies[0]["id"].as_str(), replies[0]["body"].as_str()),
        (Some("m4"), Some("401 keep"))
    );
    assert!(
        replies[0].get("caller").is_none(),
        "the app answers as a person"
    );
    assert!(
        h.focus_name().contains("value \"\""),
        "box cleared: {}",
        h.focus_name()
    );
    // The answer shows at once; the question is no longer open.
    assert_eq!(h.cx.update(|_, cx| state(cx).open_questions()), 0);

    // Ctrl+Shift+M also lands in the inbox.
    h.keys("ctrl-shift-j");
    h.keys("ctrl-shift-m");
    let inbox_focused = h.cx.update(|window, cx| {
        state(cx)
            .inbox
            .as_ref()
            .and_then(|p| p.upgrade())
            .is_some_and(|p| p.read(cx).focus.contains_focused(window, cx))
    });
    assert!(inbox_focused, "{}", h.focus_name());
}

#[test]
fn cleanup_and_settings_pages_load_on_open() {
    let mut calls = None;
    let mut h = Harness::new("orch-cleanup", |b| calls = Some(orch_backend(b)));
    let calls = calls.unwrap();
    palette(&mut h, "끝난 워커 정리");
    assert!(page_focused(&mut h, Page::Cleanup));
    assert_eq!(count(&calls, "task.cleanup_candidates"), 1);
    assert!(
        count(&calls, "orch.run.list") >= 1,
        "first open loads the runs"
    );
    let (rows, sel) = h.cx.update(|_, cx| {
        let st = state(cx);
        let rows = super::model::cleanup_rows(st.runs.values(), &st.candidates);
        (
            rows.iter().map(|r| r.attempt.clone()).collect::<Vec<_>>(),
            super::model::default_selection(&rows),
        )
    });
    assert_eq!(
        rows,
        ["a1", "a4", "a6"],
        "finished workers only, ready tier first"
    );
    assert_eq!(
        sel.into_iter().collect::<Vec<_>>(),
        ["a1"],
        "ready (and not archived) selected by default"
    );
    // Tab stays among the page's controls (keyboard reachable).
    for _ in 0..4 {
        h.keys("tab");
    }
    assert!(h.focus_name().contains("control"), "{}", h.focus_name());

    palette(&mut h, "오케스트레이션 설정");
    assert!(page_focused(&mut h, Page::Settings));
    assert_eq!(count(&calls, "orch.config.get"), 1);
    let locked =
        h.cx.update(|_, cx| state(cx).settings.as_ref().map(|s| s.locked.clone()));
    assert_eq!(locked, Some(vec!["max_depth".to_string()]));
    // A choice sends the whole settings with that field changed (never `locked`).
    let ws = h.ws.downgrade();
    let next = h.cx.update(|_, cx| {
        super::model::with_change(
            state(cx).settings.as_ref().unwrap(),
            "release_after_min",
            "30",
        )
    });
    h.cx.update(|_, cx| super::set_settings(&ws, next, cx));
    h.settle();
    let set = calls
        .borrow()
        .iter()
        .find(|(m, _)| m == "orch.config.set")
        .map(|(_, p)| p.clone())
        .expect("orch.config.set");
    assert_eq!(set["release_after_min"], 30);
    assert!(set.get("locked").is_none());
    // Resource limits (v18): read with the page, one key per change.
    assert_eq!(count(&calls, "orch.limits.get"), 1);
    let env_locked = h.cx.update(|_, cx| {
        state(cx).limits.as_ref().map(|l| {
            l.entries
                .iter()
                .filter(|e| e.source == "env")
                .map(|e| e.key.clone())
                .collect::<Vec<_>>()
        })
    });
    assert_eq!(env_locked, Some(vec!["min_free_disk".to_string()]));
    h.cx.update(|_, cx| super::set_limit(&ws, "max_workers".into(), "2".into(), cx));
    h.settle();
    let lim = calls
        .borrow()
        .iter()
        .find(|(m, _)| m == "orch.limits.set")
        .map(|(_, p)| p.clone())
        .expect("orch.limits.set");
    assert_eq!(lim, json!({"set": {"max_workers": "2"}}));
}

#[test]
fn gates_panel_and_inbox_decide_option_gates_with_a_choice() {
    let mut calls = None;
    let mut h = Harness::new("orch-gate-options", |b| calls = Some(orch_backend(b)));
    let calls = calls.unwrap();
    let mut gates = super::fixture::gates_for_test();
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::Gates(std::mem::take(&mut gates)));
    h.settle();
    // The Gates panel: its approve buttons are the options (shared rule).
    h.keys("ctrl-shift-i");
    let ws = h.ws.clone();
    let panel =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .orch
                .gates_panel
                .as_ref()
                .and_then(|p| p.upgrade())
        })
        .expect("gates panel");
    h.cx.update(|_, cx| {
        panel.update(cx, |p, cx| {
            p.resolve(
                "g3".into(),
                podd::protocol::GateDecision::Approve,
                Some("바로 운영".into()),
                cx,
            )
        })
    });
    h.settle();
    let sent: Vec<String> = h
        .backend
        .calls
        .iter()
        .filter(|c| c.starts_with("gate.resolve"))
        .cloned()
        .collect();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].contains("\"choice\":\"바로 운영\""), "{sent:?}");
    let _ = calls;
}

/// Worker cards name their agent with a mark (docs/10 §D5.11).
#[test]
fn worker_cards_show_agent_marks() {
    let mut h = Harness::new("orch-agent-marks", |b| {
        orch_backend(b);
    });
    let _ = h.backend.events.unbounded_send(SourceEvent::OrchResync);
    h.settle();
    h.keys("ctrl-shift-j");
    assert!(
        page_focused(&mut h, Page::Board),
        "board took focus: {}",
        h.focus_name()
    );
    assert!(
        h.bounds("agent-mark:orch-a1:Codex 에이전트").is_some(),
        "codex worker card"
    );
    assert!(
        h.bounds("agent-mark:orch-a5:Claude 에이전트").is_some(),
        "claude worker card"
    );
}

/// orch-02 (app part): unconfirmed wake-ups show on the board (who, since)
/// with a retry that presses Enter in that terminal (`terminal.send`, no
/// new text); the row goes once the agent checked its mail (`orch.changed`).
/// ui-10: the run picker and the settings are radio options whose checked
/// state is the current value.
#[test]
fn unconfirmed_wakeups_show_with_a_retry_and_choices_are_radios() {
    let stuck = Rc::new(std::cell::Cell::new(true));
    let mut calls: Option<Calls> = None;
    let mut h = Harness::new("orch-wake", |b| {
        seed_two_tasks(b);
        let log: Calls = Rc::default();
        calls = Some(log.clone());
        let stuck = stuck.clone();
        b.rpc = Some(Box::new(move |method, params| {
            if method == "terminal.send" {
                log.borrow_mut().push((method.to_string(), params.clone()));
                return Some(Ok(
                    json!({"id": params["id"], "bytes": 1, "submit": {"state": "confirmed", "enters": 1}}),
                ));
            }
            if !method.starts_with("orch.") {
                return None;
            }
            log.borrow_mut().push((method.to_string(), params.clone()));
            super::fixture::answer_with(method, params, stuck.get())
        }));
    });
    let calls = calls.unwrap();
    let _ = h.backend.events.unbounded_send(SourceEvent::OrchResync);
    h.settle();
    h.keys("ctrl-shift-j");
    assert!(page_focused(&mut h, Page::Board), "{}", h.focus_name());
    assert!(
        h.bounds("orch-wake:r1:coordinator").is_some(),
        "coordinator row"
    );
    assert!(h.bounds("orch-wake:r1:a2").is_some(), "worker a2 row");
    assert!(
        h.bounds("orch-wake:r1:a3").is_none(),
        "only unconfirmed ones"
    );
    // ui-10: the shown run is the checked radio, the other is not.
    assert!(
        h.bounds(&format!(
            "radio:{}: r1 · 로그인 버그 수정과 테스트 보강 · {}:true",
            super::s::RUNS,
            super::s::ACTIVE
        ))
        .is_some()
    );
    assert!(
        h.bounds(&format!(
            "radio:{}: r2 · 의존성 정리 · {}:false",
            super::s::RUNS,
            super::s::FINISHED_RUN
        ))
        .is_some()
    );

    // Retry a2: Enter in its terminal, nothing typed, confirmed → toast.
    let st = h.cx.update(|_, cx| {
        let run = state(cx).runs.get("r1").cloned().unwrap();
        super::model::stuck_wakes(&run)
            .into_iter()
            .find(|w| w.attempt.as_deref() == Some("a2"))
            .unwrap()
    });
    let ws = h.ws.downgrade();
    h.cx.update(|_, cx| super::rewake(&ws, st.clone(), cx));
    h.cx.update(|_, cx| super::rewake(&ws, st, cx)); // a double click sends once
    h.settle();
    let sends: Vec<Value> = calls
        .borrow()
        .iter()
        .filter(|(m, _)| m == "terminal.send")
        .map(|(_, p)| p.clone())
        .collect();
    assert_eq!(
        sends,
        vec![json!({"id": "fixture-6", "text": "", "enter": true})]
    );
    assert!(
        toasts(&mut h).iter().any(|t| t == super::s::WAKE_RETRIED),
        "{:?}",
        toasts(&mut h)
    );

    // The agents checked their mail: the daemon reports no unconfirmed wake.
    stuck.set(false);
    let _ = h
        .backend
        .events
        .unbounded_send(SourceEvent::OrchChanged(Box::new(OrchChangedEvent {
            seq: 1,
            run: "r1".into(),
            item: None,
            attempt: None,
        })));
    h.settle();
    assert!(
        h.bounds("orch-wake:r1:a2").is_none() && h.bounds("orch-wake:r1:coordinator").is_none(),
        "rows gone after the check"
    );

    // ui-10: settings rows are radio groups; the current value is checked.
    palette(&mut h, "오케스트레이션 설정");
    assert!(page_focused(&mut h, Page::Settings));
    assert!(
        h.bounds(&format!("radio:{}: 10분:true", super::s::SET_RELEASE))
            .is_some()
    );
    assert!(
        h.bounds(&format!("radio:{}: 30분:false", super::s::SET_RELEASE))
            .is_some()
    );
    assert!(
        h.bounds(&format!("radio:{}: 3:true", super::s::SET_DEPTH))
            .is_some(),
        "locked rows still show the checked value"
    );
    assert!(
        h.bounds(&format!("radio:{}: 70%:true", super::s::LIM_CPU))
            .is_some()
    );
}
