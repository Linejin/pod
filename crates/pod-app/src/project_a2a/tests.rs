//! The A2A section without GPUI: peer configuration per project, the
//! draft → review → send states, receipt uncertainty, remote task refresh
//! and project isolation.

use std::time::Duration;

use pod_a2a::model::{
    AgentCard, AgentInterface, AgentSkill, Message, Part, Role, SendMessageResponse, Task,
    TaskState, TaskStatus,
};
use pod_a2a::{A2aError, Delivery};

use super::config::{self, ProjectPeers};
use super::model::{self, CardState, NotReady, ProjectA2a, Resolved, Submit};

const FILE: &str = r#"{
  "version": 1,
  "projects": {
    "alpha": { "peers": [
      { "id": "review", "label": "Reviewer", "peer": "https://agent.example.com" },
      { "id": "local", "card_url": "http://127.0.0.1:9/.well-known/agent-card.json",
        "allow_http_loopback": true, "bearer_token_env": "ALPHA_TOKEN" }
    ] },
    "beta": { "repo_path": "C:\\src\\beta", "peers": [
      { "id": "review", "peer": "https://other.example.com" }
    ] }
  }
}"#;

fn ready(p: &ProjectPeers) -> Vec<(String, Option<String>)> {
    p.slots()
        .iter()
        .map(|s| (s.def.id.clone(), s.problem.clone()))
        .collect()
}

#[test]
fn peers_are_listed_per_project_only() {
    let alpha = config::parse(FILE, "alpha", Some("C:/src/alpha"));
    assert_eq!(
        ready(&alpha),
        [("review".into(), None), ("local".into(), None)]
    );
    assert_eq!(
        alpha.usable("review").unwrap().source_url(),
        "https://agent.example.com"
    );
    let beta = config::parse(FILE, "beta", Some("c:/src/beta/"));
    assert_eq!(ready(&beta), [("review".into(), None)]);
    assert_eq!(
        beta.usable("review").unwrap().source_url(),
        "https://other.example.com",
        "the same peer id in another project is another definition"
    );
    assert!(beta.usable("local").is_none(), "alpha's peers never leak");
    assert_eq!(
        config::parse(FILE, "gamma", None),
        ProjectPeers::NotConfigured
    );
    assert_eq!(
        config::parse(FILE, "alpha-other", None),
        ProjectPeers::NotConfigured,
        "a shared name prefix is another project"
    );
}

#[test]
fn a_bound_repo_path_must_match_the_registration() {
    assert!(matches!(
        config::parse(FILE, "beta", Some("D:/elsewhere/beta")),
        ProjectPeers::OtherPath { .. }
    ));
    assert!(matches!(
        config::parse(FILE, "beta", None),
        ProjectPeers::OtherPath { .. }
    ));
}

#[test]
fn bad_files_and_entries_explain_themselves() {
    assert!(matches!(
        config::parse("{", "alpha", None),
        ProjectPeers::Invalid(_)
    ));
    assert!(matches!(
        config::parse(r#"{"version":2,"projects":{}}"#, "alpha", None),
        ProjectPeers::Invalid(_)
    ));
    // A misspelt switch is refused, not ignored.
    assert!(matches!(
        config::parse(
            r#"{"version":1,"projects":{"a":{"peers":[{"id":"x","peer":"https://a.example","allow_http_loopbak":true}]}}}"#,
            "a",
            None
        ),
        ProjectPeers::Invalid(_)
    ));
    let p = config::parse(
        r#"{"version":1,"projects":{"a":{"peers":[
            {"id":"plain","peer":"http://agent.example.com"},
            {"id":"both","peer":"https://a.example","card_url":"https://a.example/c.json"},
            {"id":"loop","peer":"http://127.0.0.1:8000"},
            {"id":"bad id!","peer":"https://a.example"},
            {"id":"userinfo","peer":"https://u:p@a.example"},
            {"id":"tok","peer":"https://a.example","bearer_token_env":"A","bearer_token_file":"t"},
            {"id":"ok","peer":"https://a.example"},
            {"id":"ok","peer":"https://b.example"}
        ]}}}"#,
        "a",
        None,
    );
    let problems: Vec<(String, bool)> = ready(&p)
        .into_iter()
        .map(|(id, why)| (id, why.is_some()))
        .collect();
    assert_eq!(
        problems,
        [
            ("plain".into(), true),
            ("both".into(), true),
            ("loop".into(), true),
            ("bad id!".into(), true),
            ("userinfo".into(), true),
            ("tok".into(), true),
            ("ok".into(), false),
            ("ok".into(), true),
        ]
    );
    assert!(p.usable("loop").is_none(), "http needs the loopback switch");
    assert_eq!(p.usable("ok").unwrap().source_url(), "https://a.example");
}

#[test]
fn a_missing_file_is_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(config::load(dir.path(), "a", None), ProjectPeers::NoFile);
    std::fs::write(config::path_in(dir.path()), FILE).unwrap();
    assert_eq!(ready(&config::load(dir.path(), "alpha", None)).len(), 2);
}

// ---------------------------------------------------------------- model

fn card(skills: Vec<AgentSkill>, bearer: bool) -> Resolved {
    let mut c = AgentCard {
        name: "Peer".into(),
        version: "1".into(),
        supported_interfaces: vec![AgentInterface {
            url: "https://agent.example.com/rpc".into(),
            protocol_binding: "JSONRPC".into(),
            protocol_version: "1.0".into(),
            ..Default::default()
        }],
        default_input_modes: vec!["text/plain".into()],
        default_output_modes: vec!["text/plain".into(), "application/json".into()],
        skills,
        ..Default::default()
    };
    if bearer {
        c.security_schemes.insert(
            "bearer".into(),
            serde_json::from_value(serde_json::json!({
                "httpAuthSecurityScheme": {"scheme": "Bearer"}
            }))
            .unwrap(),
        );
    }
    let disc_card = serde_json::to_string(&c).unwrap();
    // Select through the real client code (same-origin JSONRPC 1.0).
    let card: AgentCard = serde_json::from_str(&disc_card).unwrap();
    let interface = test_interface(&card);
    Resolved {
        card,
        card_url: "https://agent.example.com/.well-known/agent-card.json".into(),
        interface,
        bearer_declared: bearer,
    }
}

fn test_interface(card: &AgentCard) -> pod_a2a::SelectedInterface {
    let d = pod_a2a::Discovery {
        card_url: pod_a2a::target::Endpoint::parse(
            "https://agent.example.com/.well-known/agent-card.json",
            Default::default(),
        )
        .unwrap(),
        card: card.clone(),
    };
    d.select(None, Default::default()).unwrap()
}

fn skill(id: &str, inputs: &[&str], outputs: &[&str]) -> AgentSkill {
    AgentSkill {
        id: id.into(),
        name: id.to_uppercase(),
        description: format!("{id} skill"),
        input_modes: inputs.iter().map(|s| s.to_string()).collect(),
        output_modes: outputs.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

/// `alpha` with its card for `review` fetched.
fn alpha(bearer: bool) -> ProjectA2a {
    let mut p = ProjectA2a::default();
    p.set_peers(config::parse(FILE, "alpha", None));
    assert!(p.select("review"), "the first choice fetches the card");
    p.cards.insert(
        "review".into(),
        CardState::Ready(Box::new(card(
            vec![
                skill("summarize", &["text/plain"], &["text/markdown"]),
                skill("image", &["image/png"], &["image/png"]),
            ],
            bearer,
        ))),
    );
    p
}

fn task(id: &str, ctx: &str, state: TaskState) -> Task {
    Task {
        id: id.into(),
        context_id: ctx.into(),
        status: TaskStatus {
            state,
            message: None,
            timestamp: None,
        },
        ..Default::default()
    }
}

fn unknown() -> A2aError {
    A2aError::Timeout {
        delivery: Delivery::Unknown,
        after: Duration::from_secs(1),
    }
}

#[test]
fn nothing_is_preselected_and_review_freezes_exactly_what_is_sent() {
    let mut p = ProjectA2a::default();
    p.set_peers(config::parse(FILE, "alpha", None));
    assert_eq!(p.selected, None, "no peer until the user picks one");
    assert_eq!(
        p.review("alpha", "hi", "m1".into()).unwrap_err(),
        NotReady::NoPeer
    );
    p.select("review");
    assert_eq!(
        p.review("alpha", "hi", "m1".into()).unwrap_err(),
        NotReady::CardMissing
    );

    let mut p = alpha(false);
    assert_eq!(
        p.review("alpha", "  \n", "m1".into()).unwrap_err(),
        NotReady::EmptyText
    );
    let r = p.review("alpha", "요약해 줘", "m1".into()).unwrap();
    assert_eq!(
        (
            r.repo.as_str(),
            r.peer_id.as_str(),
            r.text.as_str(),
            r.message_id.as_str()
        ),
        ("alpha", "review", "요약해 줘", "m1")
    );
    assert_eq!(r.interface_url, "https://agent.example.com/rpc");
    assert_eq!(r.skill, None);
    assert!(
        r.accepted_output_modes.is_empty(),
        "no skill: no preference"
    );
    assert!(!r.bearer);
    assert!(r.task_id.is_empty() && r.context_id.is_empty());

    // A skill sends its output modes; one that takes no text is refused.
    p.skill.insert("review".into(), "summarize".into());
    let r = p.review("alpha", "x", "m2".into()).unwrap();
    assert_eq!(r.skill, Some(("summarize".into(), "SUMMARIZE".into())));
    assert_eq!(r.accepted_output_modes, ["text/markdown"]);
    p.skill.insert("review".into(), "image".into());
    assert!(matches!(
        p.review("alpha", "x", "m3".into()),
        Err(NotReady::TextNotAccepted(_))
    ));
    // A skill the card no longer has is no choice at all.
    p.skill.insert("review".into(), "gone".into());
    assert_eq!(p.review("alpha", "x", "m4".into()).unwrap().skill, None);
}

#[test]
fn a_configured_token_needs_a_declared_bearer_scheme() {
    let mut p = alpha(false);
    p.select("local");
    p.cards.insert(
        "local".into(),
        CardState::Ready(Box::new(card(vec![], false))),
    );
    assert_eq!(
        p.review("alpha", "x", "m".into()).unwrap_err(),
        NotReady::TokenNotDeclared
    );
    p.cards.insert(
        "local".into(),
        CardState::Ready(Box::new(card(vec![], true))),
    );
    assert!(p.review("alpha", "x", "m".into()).unwrap().bearer);
    // The review never carries the token itself.
    let shown = format!("{:?}", p.review("alpha", "x", "m".into()).unwrap());
    assert!(!shown.contains("ALPHA_TOKEN"));
}

#[test]
fn a_task_answer_is_recorded_and_clears_the_draft() {
    let mut p = alpha(false);
    let r = p.review("alpha", "hello", "m1".into()).unwrap();
    p.submit = Submit::Sending(Box::new(r.clone()));
    assert!(p.submit.busy());
    assert!(!p.select("local"), "the target cannot change while sending");
    let delivered = p.apply_send(
        r,
        Ok(SendMessageResponse::Task(task(
            "t1",
            "c1",
            TaskState::Working,
        ))),
    );
    assert!(delivered);
    assert!(matches!(p.submit, Submit::Compose));
    let t = &p.tasks[0];
    assert_eq!(
        (
            t.task_id.as_str(),
            t.context_id.as_str(),
            t.message_id.as_str()
        ),
        ("t1", "c1", "m1")
    );
    assert!(t.active() && t.wants_poll());
}

#[test]
fn an_unknown_outcome_keeps_the_draft_and_is_never_resent_silently() {
    let mut p = alpha(false);
    let r = p.review("alpha", "hello", "m1".into()).unwrap();
    assert!(!p.apply_send(r.clone(), Err(unknown())));
    let u = p.unconfirmed.clone().expect("recorded as unconfirmed");
    assert_eq!(u.review.message_id, "m1");
    assert!(p.tasks.is_empty());
    // No other message until the user decides.
    assert_eq!(
        p.review("alpha", "other", "m9".into()).unwrap_err(),
        NotReady::Unconfirmed
    );
    // Only an explicit resend, reviewed again, with the same id and text.
    let again = p.resend_review().unwrap();
    assert!(again.resend);
    assert_eq!(
        (again.message_id.as_str(), again.text.as_str()),
        ("m1", "hello")
    );
    // Leaving that review, or a resend that fails before sending, keeps it.
    p.submit = Submit::Reviewing(Box::new(again.clone()));
    p.leave_review();
    assert!(p.unconfirmed.is_some());
    let refused = A2aError::Transport {
        delivery: Delivery::NotSent,
        detail: "connection refused".into(),
    };
    assert!(!p.apply_send(again.clone(), Err(refused)));
    assert!(
        p.unconfirmed.is_some(),
        "still unconfirmed after a known failure"
    );
    // An answer to the same message id settles it.
    assert!(p.apply_send(
        again,
        Ok(SendMessageResponse::Task(task(
            "t1",
            "c1",
            TaskState::Completed
        )))
    ));
    assert!(p.unconfirmed.is_none());
    // Or the user lets it go explicitly.
    let r = p.review("alpha", "again", "m2".into()).unwrap();
    p.apply_send(r, Err(unknown()));
    p.forget_unconfirmed();
    assert!(p.unconfirmed.is_none());
    assert!(p.review("alpha", "next", "m3".into()).is_ok());

    // A known failure (nothing sent) keeps the draft too, without a resend.
    let mut p = alpha(false);
    let r = p.review("alpha", "hello", "m2".into()).unwrap();
    let refused = A2aError::Transport {
        delivery: Delivery::NotSent,
        detail: "connection refused".into(),
    };
    assert!(!p.apply_send(r, Err(refused)));
    assert!(matches!(p.submit, Submit::Failed { .. }));
    assert!(p.resend_review().is_none());
    // Auth: kept as reported.
    let mut p = alpha(false);
    let r = p.review("alpha", "hello", "m3".into()).unwrap();
    p.apply_send(
        r,
        Err(A2aError::AuthRequired {
            status: 401,
            challenges: vec!["Bearer".into()],
        }),
    );
    let Submit::Failed { error, .. } = &p.submit else {
        panic!()
    };
    assert!(error.contains("401"), "{error}");
}

#[test]
fn a_direct_message_reply_is_kept_apart_from_tasks() {
    let mut p = alpha(false);
    let r = p.review("alpha", "message hi", "m1".into()).unwrap();
    let mut reply = Message::user_text("a1", "echo");
    reply.role = Role::Agent;
    reply.context_id = "c9".into();
    assert!(p.apply_send(r, Ok(SendMessageResponse::Message(reply))));
    assert!(p.tasks.is_empty());
    assert_eq!(p.replies[0].sent_message_id, "m1");
    assert!(p.continue_context(0));
    let r = p.review("alpha", "more", "m2".into()).unwrap();
    assert_eq!((r.task_id.as_str(), r.context_id.as_str()), ("", "c9"));
}

#[test]
fn input_required_continues_the_same_task_and_context() {
    let mut p = alpha(false);
    let r = p.review("alpha", "input x", "m1".into()).unwrap();
    p.apply_send(
        r,
        Ok(SendMessageResponse::Task(task(
            "t1",
            "c1",
            TaskState::InputRequired,
        ))),
    );
    assert!(
        !p.tasks[0].wants_poll(),
        "interrupted: the peer waits for us"
    );
    assert!(p.continue_task("review", "t1"));
    let r = p.review("alpha", "more", "m2".into()).unwrap();
    assert_eq!((r.task_id.as_str(), r.context_id.as_str()), ("t1", "c1"));
    p.apply_send(
        r,
        Ok(SendMessageResponse::Task(task(
            "t1",
            "c1",
            TaskState::Completed,
        ))),
    );
    assert_eq!(p.tasks.len(), 1, "the same task, updated");
    assert_eq!(p.tasks[0].message_id, "m2");
    assert!(p.continuing.is_none());
    assert!(
        !p.continue_task("review", "t1"),
        "terminal tasks take no more"
    );
}

#[test]
fn refresh_is_bounded_and_keeps_the_last_good_snapshot_on_errors() {
    assert_eq!(model::poll_delay(0), Some(Duration::from_secs(2)));
    assert_eq!(model::poll_delay(7), Some(Duration::from_secs(30)));
    assert_eq!(model::poll_delay(model::MAX_AUTO_POLLS), None);

    let mut p = alpha(false);
    let r = p.review("alpha", "slow", "m1".into()).unwrap();
    p.apply_send(
        r,
        Ok(SendMessageResponse::Task(task(
            "t1",
            "c1",
            TaskState::Working,
        ))),
    );
    assert_eq!(p.next_poll(), Some(Duration::from_secs(2)));
    let due = p.take_refreshes(None);
    assert_eq!(due, [("review".to_string(), "t1".to_string())]);
    assert!(
        p.take_refreshes(None).is_empty(),
        "one request in flight per task"
    );
    // Offline: the error is shown next to the last answer, not instead of it.
    p.apply_task(
        "review",
        "t1",
        Err(A2aError::Transport {
            delivery: Delivery::NotSent,
            detail: "connection refused".into(),
        }),
    );
    let t = &p.tasks[0];
    assert_eq!(*t.state(), TaskState::Working);
    assert!(t.error.as_deref().unwrap().contains("connection refused"));
    // Auto rounds run out; a manual refresh starts over.
    for _ in 0..model::MAX_AUTO_POLLS {
        let _ = p.take_refreshes(None);
        p.apply_task("review", "t1", Err(unknown()));
    }
    assert_eq!(p.next_poll(), None);
    assert!(p.take_refreshes(None).is_empty());
    assert_eq!(p.take_refreshes(Some(("review", "t1"))).len(), 1);
    let mut done = task("t1", "c1", TaskState::Completed);
    done.artifacts = Some(vec![pod_a2a::model::Artifact {
        artifact_id: "a1".into(),
        parts: vec![Part::text("result")],
        ..Default::default()
    }]);
    p.apply_task("review", "t1", Ok(done));
    let t = &p.tasks[0];
    assert_eq!(*t.state(), TaskState::Completed);
    assert!(t.error.is_none());
    assert_eq!(t.fetched, 1);
    assert_eq!(p.next_poll(), None, "terminal: nothing left to refresh");
}

#[test]
fn a_peer_that_cannot_get_tasks_is_not_polled() {
    let mut p = alpha(false);
    let r = p.review("alpha", "slow", "m1".into()).unwrap();
    p.apply_send(
        r,
        Ok(SendMessageResponse::Task(task(
            "t1",
            "c1",
            TaskState::Submitted,
        ))),
    );
    let _ = p.take_refreshes(None);
    p.apply_task(
        "review",
        "t1",
        Err(A2aError::Rpc(pod_a2a::RpcError {
            code: -32601,
            kind: pod_a2a::RpcErrorKind::MethodNotFound,
            message: "no GetTask".into(),
            data: None,
        })),
    );
    assert!(p.tasks[0].no_refresh.is_some());
    assert_eq!(p.next_poll(), None);
}

#[test]
fn results_for_one_project_never_reach_another() {
    let mut a = alpha(false);
    let mut b = ProjectA2a::default();
    b.set_peers(config::parse(FILE, "beta", Some("C:/src/beta")));
    let r = a.review("alpha", "secret alpha text", "m1".into()).unwrap();
    a.apply_send(
        r,
        Ok(SendMessageResponse::Task(task(
            "t1",
            "c1",
            TaskState::Working,
        ))),
    );
    // A GetTask answer addressed to beta's same-named peer finds nothing there.
    b.apply_task("review", "t1", Ok(task("t1", "c1", TaskState::Completed)));
    assert!(b.tasks.is_empty());
    assert!(b.take_refreshes(None).is_empty());
    assert_eq!(*a.tasks[0].state(), TaskState::Working);
    // Peer state of a reloaded, changed list is dropped.
    let mut a2 = a.clone();
    a2.set_peers(config::parse(
        r#"{"version":1,"projects":{"alpha":{"peers":[{"id":"local","peer":"https://x.example"}]}}}"#,
        "alpha",
        None,
    ));
    assert_eq!(a2.selected, None, "the chosen peer is gone");
    assert!(a2.cards.is_empty());
}

#[test]
fn peer_content_is_escaped_and_links_are_not_opened() {
    let text = Part::text("ok\x1b[31m red\u{202E}");
    assert_eq!(
        model::part_view(&text),
        model::PartView::Text("ok\\u{1b}[31m red\\u{202e}".into())
    );
    let url: Part = serde_json::from_value(
        serde_json::json!({"url": "https://files.invalid/r.pdf", "filename": "r.pdf"}),
    )
    .unwrap();
    assert_eq!(
        model::part_view(&url),
        model::PartView::Url {
            url: "https://files.invalid/r.pdf".into(),
            name: "r.pdf".into()
        }
    );
    let raw: Part = serde_json::from_value(serde_json::json!({"raw": "AAEC"})).unwrap();
    assert_eq!(
        model::part_view(&raw),
        model::PartView::Raw {
            bytes: Some(3),
            name: String::new()
        }
    );
    assert!(model::accepts_text("text/plain; charset=utf-8"));
    assert!(!model::accepts_text("application/json"));
}
