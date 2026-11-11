//! Unit tests of the source: stream events, lifecycle refreshes, reply
//! filter, presets and error classes (names kept from before the split).

use std::time::{Duration, Instant};

use podd::client::ClientError;
use podd::protocol::{self as p, codes};
use serde_json::{Value, json};

use super::control::{is_method_not_found, is_transport};
use super::events::{DELIVERY_EVENTS, Lifecycle, delivery_events, lifecycle_action, orch_event};
use super::input::{REPLY_MAX_AGE, ReplyFilter};
use super::*;

#[test]
fn lifecycle_events_map_to_refreshes_and_removal() {
    let n = |method: &str, params: Value| p::Notification {
        jsonrpc: "2.0".into(),
        method: method.into(),
        params,
    };
    let refresh = |terminals, tasks, files: Option<&str>| Lifecycle::Refresh {
        terminals,
        tasks,
        files: files.map(str::to_owned),
    };
    assert_eq!(
        lifecycle_action(&n(
            p::NOTIFY_TERMINAL_CHANGED,
            json!({"seq": 1, "id": "t1", "change": "exited"})
        )),
        refresh(true, false, None)
    );
    assert_eq!(
        lifecycle_action(&n(p::NOTIFY_STATUS_LAGGED, json!({"missed": 9}))),
        refresh(true, true, None)
    );
    assert_eq!(
        lifecycle_action(&n(
            p::NOTIFY_TASK_CHANGED,
            json!({"seq": 2, "id": "r/a", "change": "touched", "paths": ["x"]})
        )),
        refresh(false, false, Some("r/a"))
    );
    assert_eq!(
        lifecycle_action(&n(
            p::NOTIFY_TASK_CHANGED,
            json!({"seq": 3, "id": "r/a", "change": "git"})
        )),
        refresh(false, true, Some("r/a"))
    );
    assert_eq!(
        lifecycle_action(&n(
            p::NOTIFY_TASK_CHANGED,
            json!({"seq": 4, "id": "r/a", "change": "terminals", "terminals": ["t2"]})
        )),
        refresh(true, true, None)
    );
    assert_eq!(
        lifecycle_action(&n(
            p::NOTIFY_TASK_CHANGED,
            json!({"seq": 5, "id": "r/a", "change": "status"})
        )),
        refresh(false, true, None)
    );
    assert_eq!(
        lifecycle_action(&n(
            p::NOTIFY_TASK_REMOVING,
            json!({"seq": 6, "id": "r/a", "worktree": "C:/w/a", "force": false, "grace_ms": 300})
        )),
        Lifecycle::Removing("r/a".into(), "C:/w/a".into())
    );
    assert_eq!(
        lifecycle_action(&n(p::NOTIFY_STATUS_CHANGED, json!({}))),
        Lifecycle::None
    );
}

#[test]
fn color_reply_filter_drops_stale_and_duplicate_replies() {
    let reply = b"\x1b]11;rgb:1c1c/1d1d/2121\x1b\\".to_vec();
    let t0 = Instant::now();
    let ms = |n| t0 + Duration::from_millis(n);
    let mut f = ReplyFilter::default();
    // Prompt reply goes out.
    assert!(f.admit("t2", &reply, t0, ms(5)));
    // A second panel on t2 answers the same query: dropped.
    assert!(!f.admit("t2", &reply, ms(3), ms(8)));
    // Same reply for another terminal: its own query.
    assert!(f.admit("t3", &reply, ms(3), ms(9)));
    // A later query of t2 (Codex re-queries on focus/theme change) is answered again.
    assert!(f.admit("t2", &reply, ms(2000), ms(2001)));
    // E2E B4: a reply stuck for longer than the program waits is dropped,
    // not typed into the agent's composer.
    assert!(!f.admit(
        "t4",
        &reply,
        ms(3000),
        ms(3000) + REPLY_MAX_AGE + Duration::from_millis(1)
    ));
    // Different bytes (OSC 10 vs 11) are separate answers.
    let fg = b"\x1b]10;rgb:d6d6/d8d8/dbdb\x1b\\".to_vec();
    assert!(f.admit("t5", &fg, ms(4000), ms(4001)));
    assert!(f.admit("t5", &reply, ms(4000), ms(4001)));
}

#[test]
fn command_line_quotes() {
    assert_eq!(
        split_command_line(r#"vim -c "set mouse=a" f.txt"#),
        ["vim", "-c", "set mouse=a", "f.txt"]
    );
    assert_eq!(
        split_command_line("  cmd.exe   /c dir "),
        ["cmd.exe", "/c", "dir"]
    );
    assert!(split_command_line("   ").is_empty());
}

#[test]
fn presets() {
    assert_eq!(Program::Shell.argv("")[0], "powershell.exe");
    assert_eq!(Program::Claude.argv("ignored"), ["claude"]);
    assert_eq!(
        Program::Custom.argv("git log -n 3"),
        ["git", "log", "-n", "3"]
    );
}

#[test]
fn run_and_mail_stream_events() {
    let n = |method: &str, params: Value| p::Notification {
        jsonrpc: "2.0".into(),
        method: method.into(),
        params,
    };
    match orch_event(&n(
        p::NOTIFY_ORCH_CHANGED,
        json!({"seq": 3, "run": "r1", "attempt": "a2"}),
    )) {
        Some(SourceEvent::OrchChanged(e)) => assert_eq!(
            (e.run.as_str(), e.attempt.as_deref(), e.item),
            ("r1", Some("a2"), None)
        ),
        other => panic!("{other:?}"),
    }
    match orch_event(&n(
        p::NOTIFY_ORCH_MAIL,
        json!({"seq": 4, "run": "r1", "to": "run:r1", "id": "m7", "type": "escalation", "subject": "stalled"}),
    )) {
        Some(SourceEvent::OrchMail(e)) => assert_eq!(
            (e.id.as_str(), e.kind, e.subject.as_str()),
            ("m7", p::MailType::Escalation, "stalled")
        ),
        other => panic!("{other:?}"),
    }
    assert!(
        orch_event(&n(p::NOTIFY_ORCH_MAIL, json!({"run": "r1"}))).is_none(),
        "malformed events are dropped"
    );
}

#[test]
fn orchestration_stream_events() {
    let n = |method: &str, params: Value| p::Notification {
        jsonrpc: "2.0".into(),
        method: method.into(),
        params,
    };
    let gate = json!({"seq": 4, "gate": {"id": "g1", "title": "Deploy", "message": "ok?", "state": "pending", "created_ms": 1, "terminal": "t2"}});
    match orch_event(&n(p::NOTIFY_GATE_CHANGED, gate)) {
        Some(SourceEvent::Gate(g)) => {
            assert_eq!((g.id.as_str(), g.terminal.as_deref()), ("g1", Some("t2")))
        }
        other => panic!("{other:?}"),
    }
    let delivery = json!({"seq": 9, "delivery": {"id": "d1", "task": "demo/a", "terminal": "t1", "comment_ids": ["c1"],
        "state": "queued", "prompt": "p", "created_ms": 1, "agent_state": "working"}});
    DELIVERY_EVENTS.store(false, std::sync::atomic::Ordering::Relaxed);
    match orch_event(&n(p::NOTIFY_REVIEW_DELIVERY, delivery)) {
        Some(SourceEvent::Delivery(d, submit)) => {
            assert_eq!((d.state, submit), (p::DeliveryState::Queued, None))
        }
        other => panic!("{other:?}"),
    }
    assert!(
        delivery_events(),
        "a delivery event switches the review panel off polling"
    );
    let check = json!({"seq": 5, "group": "demo/fx", "job": {"id": "k1", "state": "running", "argv": ["cmd"], "tasks": ["demo/fx-shell-1"],
        "timeout_ms": 1000, "started_ms": 1}});
    assert!(
        matches!(orch_event(&n(p::NOTIFY_GROUP_CHECK, check)), Some(SourceEvent::GroupCheck(e)) if e.group == "demo/fx")
    );
    assert!(orch_event(&n(p::NOTIFY_STATUS_CHANGED, json!({}))).is_none());
    assert!(orch_event(&n(p::NOTIFY_GATE_CHANGED, json!({"bad": 1}))).is_none());
}

#[test]
fn transport_vs_rpc_errors() {
    let rpc = ClientError::Rpc(p::RpcError::new(codes::METHOD_NOT_FOUND, "nope"));
    assert!(is_method_not_found(&rpc));
    assert!(!is_transport(&rpc));
    assert!(is_transport(&ClientError::Protocol("closed".into())));
    // A stopping daemon: reconnect (to the next one), not an ordinary error.
    assert!(is_transport(&ClientError::Rpc(p::RpcError::new(
        codes::SERVER_STOPPING,
        "podd is stopping"
    ))));
}
