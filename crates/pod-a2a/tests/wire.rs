//! Wire and model tests: exact request shapes from the A2A v1.0.1
//! specification (§9.4), responses captured from the official a2a-python SDK
//! 1.3.1 server (tests/fixtures/sdk-*.json), and ProtoJSON edge cases.

use pod_a2a::jsonrpc::{self, decode_response, encode_request};
use pod_a2a::model::*;
use pod_a2a::target::{Endpoint, TargetPolicy};
use pod_a2a::{A2aError, Discovery, RpcErrorKind};
use serde_json::{Value, json};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn id_of(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn as_value(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

// ------------------------------------------------------------ requests

#[test]
fn send_message_request_matches_spec_9_4_1_shape() {
    let mut msg = Message::user_text("msg-uuid", "What is the weather today?");
    msg.context_id = "ctx-1".into();
    msg.task_id = "task-1".into();
    msg.reference_task_ids = vec!["task-0".into()];
    let req = SendMessageRequest {
        tenant: String::new(),
        message: msg,
        configuration: Some(SendMessageConfiguration {
            accepted_output_modes: vec!["text/plain".into()],
            history_length: Some(0),
            return_immediately: true,
        }),
        metadata: None,
    };
    let body = encode_request("pod-1", jsonrpc::SEND_MESSAGE, &req);
    assert_eq!(
        as_value(&body),
        json!({
            "jsonrpc": "2.0",
            "id": "pod-1",
            "method": "SendMessage",
            "params": {
                "message": {
                    "messageId": "msg-uuid",
                    "contextId": "ctx-1",
                    "taskId": "task-1",
                    "role": "ROLE_USER",
                    "parts": [{"text": "What is the weather today?"}],
                    "referenceTaskIds": ["task-0"]
                },
                "configuration": {
                    "acceptedOutputModes": ["text/plain"],
                    "historyLength": 0,
                    "returnImmediately": true
                }
            }
        })
    );
}

#[test]
fn proto3_defaults_are_omitted_and_tenant_is_sent_when_set() {
    let req = SendMessageRequest {
        tenant: "acme".into(),
        message: Message::user_text("m", "hi"),
        configuration: Some(SendMessageConfiguration::default()),
        metadata: None,
    };
    assert_eq!(
        serde_json::to_value(&req).unwrap(),
        json!({
            "tenant": "acme",
            "message": {"messageId": "m", "role": "ROLE_USER", "parts": [{"text": "hi"}]},
            "configuration": {}
        })
    );
}

#[test]
fn get_list_cancel_requests_match_spec_9_4_3_to_9_4_5() {
    let get = GetTaskRequest {
        tenant: String::new(),
        id: "task-uuid".into(),
        history_length: Some(10),
    };
    assert_eq!(
        as_value(&encode_request("2", jsonrpc::GET_TASK, &get)),
        json!({"jsonrpc": "2.0", "id": "2", "method": "GetTask",
               "params": {"id": "task-uuid", "historyLength": 10}})
    );
    let list = ListTasksRequest {
        context_id: "context-uuid".into(),
        status: Some(TaskState::Working),
        page_size: Some(50),
        page_token: "cursor-token".into(),
        ..ListTasksRequest::default()
    };
    assert_eq!(
        as_value(&encode_request("3", jsonrpc::LIST_TASKS, &list)),
        json!({"jsonrpc": "2.0", "id": "3", "method": "ListTasks",
               "params": {"contextId": "context-uuid", "status": "TASK_STATE_WORKING",
                          "pageSize": 50, "pageToken": "cursor-token"}})
    );
    // An unset filter is not sent (proto3 enum without presence).
    let all = ListTasksRequest {
        status: Some(TaskState::Unspecified),
        include_artifacts: Some(true),
        ..ListTasksRequest::default()
    };
    assert_eq!(
        serde_json::to_value(&all).unwrap(),
        json!({"includeArtifacts": true})
    );
    let cancel = CancelTaskRequest {
        tenant: String::new(),
        id: "task-uuid".into(),
        metadata: None,
    };
    assert_eq!(
        as_value(&encode_request("4", jsonrpc::CANCEL_TASK, &cancel)),
        json!({"jsonrpc": "2.0", "id": "4", "method": "CancelTask", "params": {"id": "task-uuid"}})
    );
}

// ------------------------------------------------------------ official SDK responses

#[test]
fn sdk_agent_card_parses_and_selects_jsonrpc_1_0() {
    let card: AgentCard = serde_json::from_slice(&fixture("sdk-agent-card.json")).unwrap();
    validate::card(&card).unwrap();
    assert_eq!(card.name, "Pod interop echo agent");
    assert_eq!(card.capabilities.streaming, Some(false));
    assert_eq!(card.skills[0].id, "echo");
    let iface = &card.supported_interfaces[0];
    assert_eq!(
        (
            iface.protocol_binding.as_str(),
            iface.protocol_version.as_str()
        ),
        ("JSONRPC", "1.0")
    );
    // The fixture was captured on some port; select against that origin.
    let ep = Endpoint::parse(
        &iface.url,
        TargetPolicy {
            allow_http_loopback: true,
        },
    )
    .unwrap();
    let card_url = Endpoint {
        path_and_query: "/.well-known/agent-card.json".into(),
        ..ep.clone()
    };
    let d = Discovery { card_url, card };
    let sel = d
        .select(
            None,
            TargetPolicy {
                allow_http_loopback: true,
            },
        )
        .unwrap();
    assert_eq!(sel.index, 0);
    assert_eq!(sel.endpoint(), &ep);
    assert!(sel.tenant.is_empty());
}

#[test]
fn sdk_completed_task_with_text_and_data_artifact() {
    let body = fixture("sdk-send-task-completed.json");
    let r: SendMessageResponse = decode_response(&id_of(&body), &body).unwrap();
    validate::send_response(&r).unwrap();
    let SendMessageResponse::Task(t) = r else {
        panic!("expected a Task")
    };
    assert_eq!(t.status.state, TaskState::Completed);
    assert!(t.status.state.is_terminal());
    let arts = t.artifacts.as_ref().unwrap();
    assert_eq!(arts[0].name, "echo");
    assert_eq!(
        arts[0].parts[0].content,
        PartContent::Text("echo: hello world".into())
    );
    assert_eq!(arts[0].parts[0].media_type, "text/plain");
    assert_eq!(
        arts[0].parts[1].content,
        PartContent::Data(json!({"length": 11.0, "words": 2.0}))
    );
    assert_eq!(t.history[0].message_id, "fixture-m1");
    assert_eq!(t.history[0].role, Role::User);
}

#[test]
fn sdk_direct_message_reply_is_a_message_not_a_task() {
    let body = fixture("sdk-send-message.json");
    let r: SendMessageResponse = decode_response(&id_of(&body), &body).unwrap();
    let SendMessageResponse::Message(m) = r else {
        panic!("expected a Message")
    };
    assert_eq!(m.role, Role::Agent);
    assert!(m.task_id.is_empty());
    assert!(!m.context_id.is_empty());
    assert_eq!(
        m.parts[0].content,
        PartContent::Text("echo-message: message hi".into())
    );
}

#[test]
fn sdk_input_required_is_interrupted_with_agent_message() {
    let body = fixture("sdk-send-input-required.json");
    let r: SendMessageResponse = decode_response(&id_of(&body), &body).unwrap();
    let SendMessageResponse::Task(t) = r else {
        panic!("expected a Task")
    };
    assert_eq!(t.status.state, TaskState::InputRequired);
    assert_eq!(t.status.state.class(), StateClass::Interrupted);
    assert_eq!(t.status.message.as_ref().unwrap().role, Role::Agent);
    assert!(t.artifacts.is_none(), "no artifacts field at all");
}

#[test]
fn sdk_file_parts_are_kept_as_data_not_fetched() {
    let body = fixture("sdk-send-files.json");
    let r: SendMessageResponse = decode_response(&id_of(&body), &body).unwrap();
    let SendMessageResponse::Task(t) = r else {
        panic!("expected a Task")
    };
    let parts = &t.artifacts.as_ref().unwrap()[0].parts;
    assert_eq!(
        parts[0].content,
        PartContent::Url("https://files.invalid/report.pdf".into())
    );
    assert_eq!(parts[0].filename, "report.pdf");
    let PartContent::Raw(raw) = &parts[1].content else {
        panic!("raw part")
    };
    assert_eq!(Part::raw_len(raw), Some(11));
    assert!(matches!(parts[2].content, PartContent::Data(_)));
    // Round trip keeps the ProtoJSON shape.
    let again = serde_json::to_value(&parts[0]).unwrap();
    assert_eq!(
        again,
        json!({"url": "https://files.invalid/report.pdf", "filename": "report.pdf", "mediaType": "application/pdf"})
    );
}

#[test]
fn sdk_list_tasks_without_artifacts_and_with_empty_default_fields() {
    let body = fixture("sdk-list-tasks.json");
    let r: ListTasksResponse = decode_response(&id_of(&body), &body).unwrap();
    assert_eq!(r.tasks.len(), 2);
    assert_eq!(r.page_size, 2);
    assert!(r.total_size >= 2);
    for t in &r.tasks {
        validate::task("t", t).unwrap();
        assert!(t.artifacts.is_none());
    }
}

#[test]
fn sdk_errors_map_to_a2a_error_kinds() {
    for (file, kind, code, reason) in [
        (
            "sdk-error-task-not-found.json",
            RpcErrorKind::TaskNotFound,
            -32001,
            Some("TASK_NOT_FOUND"),
        ),
        (
            "sdk-error-version.json",
            RpcErrorKind::VersionNotSupported,
            -32009,
            Some("VERSION_NOT_SUPPORTED"),
        ),
        (
            "sdk-error-method.json",
            RpcErrorKind::MethodNotFound,
            -32601,
            None,
        ),
    ] {
        let body = fixture(file);
        let e = decode_response::<Task>(&id_of(&body), &body).unwrap_err();
        let A2aError::Rpc(rpc) = &e else {
            panic!("{file}: {e:?}")
        };
        assert_eq!((rpc.kind, rpc.code), (kind, code), "{file}");
        assert_eq!(rpc.reasons().first().map(String::as_str), reason, "{file}");
        assert!(
            !e.outcome_unknown(),
            "{file}: a JSON-RPC error is a definite answer"
        );
    }
}

// ------------------------------------------------------------ spec examples

#[test]
fn spec_6_1_and_6_3_payloads_parse() {
    let completed = json!({"task": {
        "id": "task-uuid", "contextId": "context-uuid",
        "status": {"state": "TASK_STATE_COMPLETED"},
        "artifacts": [{"artifactId": "artifact-uuid", "name": "Weather Report",
                       "parts": [{"text": "Today will be sunny with a high of 75°F"}]}]}});
    let r: SendMessageResponse = serde_json::from_value(completed).unwrap();
    validate::send_response(&r).unwrap();
    let input = json!({"task": {"id": "task-uuid", "status": {
        "state": "TASK_STATE_INPUT_REQUIRED",
        "message": {"role": "ROLE_AGENT", "parts": [{"text": "I need more details."}]}}}});
    let r: SendMessageResponse = serde_json::from_value(input).unwrap();
    let SendMessageResponse::Task(t) = &r else {
        panic!()
    };
    assert_eq!(t.status.state, TaskState::InputRequired);
    // The spec example omits the status message's messageId (REQUIRED):
    // parsing keeps it, validation names the missing field.
    assert_eq!(
        validate::send_response(&r).unwrap_err(),
        "result.task.status.message.messageId is missing (REQUIRED)"
    );
}

// ------------------------------------------------------------ ProtoJSON rules

#[test]
fn part_oneof_needs_exactly_one_member() {
    let two = serde_json::from_value::<Part>(json!({"text": "a", "url": "https://x"}));
    assert!(two.unwrap_err().to_string().contains("more than one"));
    let none = serde_json::from_value::<Part>(json!({"mediaType": "text/plain"}));
    assert!(none.unwrap_err().to_string().contains("none of"));
    // `data: null` is the JSON null value, a set member.
    let p: Part = serde_json::from_value(json!({"data": null})).unwrap();
    assert_eq!(p.content, PartContent::Data(Value::Null));
    // `text: null` is unset, so `url` is the only member.
    let p: Part = serde_json::from_value(json!({"text": null, "url": "https://x"})).unwrap();
    assert_eq!(p.content, PartContent::Url("https://x".into()));
    assert!(serde_json::from_value::<Part>(json!({"raw": "%%%"})).is_err());
    let p: Part = serde_json::from_value(json!({"raw": "cmF3LWJ5dGVz"})).unwrap();
    assert!(matches!(p.content, PartContent::Raw(_)));
}

#[test]
fn enums_accept_names_and_numbers_and_keep_unknown_values() {
    let t: TaskStatus = serde_json::from_value(json!({"state": 6})).unwrap();
    assert_eq!(t.state, TaskState::InputRequired);
    let t: TaskStatus = serde_json::from_value(json!({"state": "TASK_STATE_PAUSED"})).unwrap();
    assert_eq!(t.state, TaskState::Unrecognized("TASK_STATE_PAUSED".into()));
    assert_eq!(t.state.class(), StateClass::Unknown);
    assert_eq!(
        serde_json::to_value(&TaskState::AuthRequired).unwrap(),
        json!("TASK_STATE_AUTH_REQUIRED")
    );
    let m: Message =
        serde_json::from_value(json!({"messageId": "m", "role": 2, "parts": [{"text": "x"}]}))
            .unwrap();
    assert_eq!(m.role, Role::Agent);
}

#[test]
fn unknown_fields_are_ignored_nulls_are_defaults_ints_may_be_strings() {
    let r: ListTasksResponse = serde_json::from_value(json!({
        "tasks": [{"id": "t", "contextId": null, "status": {"state": "TASK_STATE_WORKING"},
                   "futureField": {"x": 1}}],
        "nextPageToken": "", "pageSize": "50", "totalSize": 1, "somethingNew": true
    }))
    .unwrap();
    assert_eq!(r.page_size, 50);
    assert_eq!(r.tasks[0].context_id, "");
    let card: AgentCard = serde_json::from_value(json!({
        "name": "a", "supportedInterfaces": [{"url": "https://a.example/rpc",
            "protocolBinding": "JSONRPC", "protocolVersion": "1.0", "extra": 1}],
        "capabilities": null, "skills": null, "newCardField": []
    }))
    .unwrap();
    validate::card(&card).unwrap();
    assert!(card.skills.is_empty());
}

#[test]
fn send_response_oneof_is_exclusive() {
    let both = serde_json::from_value::<SendMessageResponse>(json!({
        "task": {"id": "t", "status": {"state": "TASK_STATE_WORKING"}},
        "message": {"messageId": "m", "role": "ROLE_AGENT", "parts": [{"text": "x"}]}}));
    assert!(both.unwrap_err().to_string().contains("both"));
    let neither = serde_json::from_value::<SendMessageResponse>(json!({}));
    assert!(neither.unwrap_err().to_string().contains("neither"));
}

#[test]
fn jsonrpc_envelope_is_checked() {
    let ok = br#"{"jsonrpc":"2.0","id":"a","result":{"id":"t","status":{"state":"TASK_STATE_WORKING"}}}"#;
    assert!(decode_response::<Task>("a", ok).is_ok());
    let wrong_id = decode_response::<Task>("b", ok).unwrap_err();
    assert!(matches!(wrong_id, A2aError::Protocol(ref s) if s.contains("does not match")));
    let no_version = br#"{"id":"a","result":{}}"#;
    assert!(matches!(
        decode_response::<Task>("a", no_version),
        Err(A2aError::Protocol(_))
    ));
    let both = br#"{"jsonrpc":"2.0","id":"a","result":{},"error":{"code":1,"message":"x"}}"#;
    assert!(matches!(
        decode_response::<Task>("a", both),
        Err(A2aError::Protocol(_))
    ));
    // An error the server could not tie to a request id.
    let null_id =
        br#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Invalid JSON payload"}}"#;
    let A2aError::Rpc(e) = decode_response::<Task>("a", null_id).unwrap_err() else {
        panic!()
    };
    assert_eq!(e.kind, RpcErrorKind::JsonParse);
    let custom = br#"{"jsonrpc":"2.0","id":"a","error":{"code":-31000,"message":"Unauthorized"}}"#;
    let A2aError::Rpc(e) = decode_response::<Task>("a", custom).unwrap_err() else {
        panic!()
    };
    assert_eq!(e.kind, RpcErrorKind::Other);
    assert!(matches!(
        decode_response::<Task>("a", b"<html>"),
        Err(A2aError::Protocol(_))
    ));
}

#[test]
fn versions_compare_major_minor_only() {
    assert_eq!(major_minor("1.0"), Some((1, 0)));
    assert_eq!(major_minor("1.0.1"), Some((1, 0)));
    assert_eq!(major_minor("0.3.0"), Some((0, 3)));
    assert_eq!(major_minor("1"), None);
    assert_eq!(major_minor("v1.0"), None);
    assert!(Discovery::is_compatible("JSONRPC", "1.0.1"));
    assert!(!Discovery::is_compatible("JSONRPC", "0.3"));
    assert!(!Discovery::is_compatible("jsonrpc", "1.0"));
    assert!(!Discovery::is_compatible("HTTP+JSON", "1.0"));
}
