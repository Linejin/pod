//! Routing of every JSON-RPC method (docs/20-refactor/daemon.md, card D-P1),
//! pinned from the dispatch match before it became a registry: each public
//! method answers one probe exactly as recorded in [`PROBE`]; unknown names,
//! notifications, a wrong `jsonrpc`, id-less stream requests, a params-free
//! method and the debug-only `mobile.test.*` methods keep their answers.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::Daemon;
use podd::protocol::*;
use podd::transport::{LineWriter, Transport};
use serde_json::{Value, json};

/// The answer of each public method to `"params": "route-probe"` (a string,
/// which no params struct accepts): `method code message`, or `method ok`.
const PROBE: &str = r#"connection.hello -32602 invalid params: invalid type: string "route-probe", expected struct HelloParams
gate.create -32602 invalid params: invalid type: string "route-probe", expected struct GateCreateParams
gate.list -32602 invalid params: invalid type: string "route-probe", expected struct GateListParams
gate.resolve -32602 invalid params: invalid type: string "route-probe", expected struct GateResolveParams
gate.show -32602 invalid params: invalid type: string "route-probe", expected struct GateRef
gate.wait -32602 invalid params: invalid type: string "route-probe", expected struct GateWaitParams
group.check -32602 invalid params: invalid type: string "route-probe", expected struct GroupCheckParams
group.check_cancel -32602 invalid params: invalid type: string "route-probe", expected struct GroupRef
group.compare -32602 invalid params: invalid type: string "route-probe", expected struct GroupRef
group.create -32602 invalid params: invalid type: string "route-probe", expected struct GroupCreateParams
group.list -32602 invalid params: invalid type: string "route-probe", expected struct GroupListParams
group.pick -32602 invalid params: invalid type: string "route-probe", expected struct GroupPickParams
group.remove -32602 invalid params: invalid type: string "route-probe", expected struct GroupRef
group.show -32602 invalid params: invalid type: string "route-probe", expected struct GroupRef
mobile.audit -32602 invalid params: invalid type: string "route-probe", expected struct MobileAuditParams
mobile.devices -32602 invalid params: invalid type: string "route-probe", expected struct MobileDevicesParams
mobile.policy -32602 invalid params: invalid type: string "route-probe", expected struct MobilePolicyParams
mobile.revoke -32602 invalid params: invalid type: string "route-probe", expected struct MobileRevokeParams
mobile.settings -32602 invalid params: invalid type: string "route-probe", expected struct MobileSettingsParams
mod.command -32602 invalid params: invalid type: string "route-probe", expected struct ModCommandParams
mod.grant -32602 invalid params: invalid type: string "route-probe", expected struct ModGrantParams
mod.list -32602 invalid params: invalid type: string "route-probe", expected struct Empty
mod.log -32602 invalid params: invalid type: string "route-probe", expected struct ModLogParams
mod.reload -32602 invalid params: invalid type: string "route-probe", expected struct ModReloadParams
mod.set_enabled -32602 invalid params: invalid type: string "route-probe", expected struct ModEnableParams
orch.commit -32602 invalid params: invalid type: string "route-probe", expected struct OrchCommitParams
orch.config.get ok
orch.config.set -32602 invalid params: invalid type: string "route-probe", expected struct OrchSettings
orch.done -32602 invalid params: invalid type: string "route-probe", expected struct OrchDoneParams
orch.gate.decide -32602 invalid params: invalid type: string "route-probe", expected struct OrchGateDecideParams
orch.gate.request -32602 invalid params: invalid type: string "route-probe", expected struct OrchGateRequestParams
orch.item.create -32602 invalid params: invalid type: string "route-probe", expected struct OrchItemCreateParams
orch.item.list -32602 invalid params: invalid type: string "route-probe", expected struct OrchItemListParams
orch.item.update -32602 invalid params: invalid type: string "route-probe", expected struct OrchItemUpdateParams
orch.limits.get -32602 invalid params: invalid type: string "route-probe", expected struct OrchLimitsGetParams
orch.limits.set -32602 invalid params: invalid type: string "route-probe", expected struct OrchLimitsSetParams
orch.mail.ask -32602 invalid params: invalid type: string "route-probe", expected struct OrchMailAskParams
orch.mail.check -32602 invalid params: invalid type: string "route-probe", expected struct OrchMailCheckParams
orch.mail.list -32602 invalid params: invalid type: string "route-probe", expected struct OrchMailListParams
orch.mail.reply -32602 invalid params: invalid type: string "route-probe", expected struct OrchMailReplyParams
orch.mail.send -32602 invalid params: invalid type: string "route-probe", expected struct OrchMailSendParams
orch.queue -32602 invalid params: invalid type: string "route-probe", expected struct OrchRunRef
orch.run.create -32602 invalid params: invalid type: string "route-probe", expected struct OrchRunCreateParams
orch.run.list -32602 invalid params: invalid type: string "route-probe", expected struct OrchRunListParams
orch.run.show -32602 invalid params: invalid type: string "route-probe", expected struct OrchRunRef
orch.wait -32602 invalid params: invalid type: string "route-probe", expected struct OrchWaitParams
orch.worker.abandon -32602 invalid params: invalid type: string "route-probe", expected struct OrchAttemptRef
orch.worker.keep -32602 invalid params: invalid type: string "route-probe", expected struct OrchAttemptRef
orch.worker.list -32602 invalid params: invalid type: string "route-probe", expected struct OrchRunRef
orch.worker.read -32602 invalid params: invalid type: string "route-probe", expected struct OrchWorkerReadParams
orch.worker.release -32602 invalid params: invalid type: string "route-probe", expected struct OrchAttemptRef
orch.worker.show -32602 invalid params: invalid type: string "route-probe", expected struct OrchAttemptRef
orch.worker.start -32602 invalid params: invalid type: string "route-probe", expected struct OrchWorkerStartParams
orch.worker.stop -32602 invalid params: invalid type: string "route-probe", expected struct OrchAttemptRef
repo.add -32602 invalid params: invalid type: string "route-probe", expected struct RepoAddParams
repo.config -32602 invalid params: invalid type: string "route-probe", expected struct RepoConfigParams
repo.list -32602 invalid params: invalid type: string "route-probe", expected struct Empty
repo.remove -32602 invalid params: invalid type: string "route-probe", expected struct RepoRef
repo.set -32602 invalid params: invalid type: string "route-probe", expected struct RepoSetParams
review.cancel -32602 invalid params: invalid type: string "route-probe", expected struct DeliveryRef
review.comments.add -32602 invalid params: invalid type: string "route-probe", expected struct ReviewAddParams
review.comments.list -32602 invalid params: invalid type: string "route-probe", expected struct ReviewListParams
review.comments.remove -32602 invalid params: invalid type: string "route-probe", expected struct ReviewIdsParams
review.comments.resolve -32602 invalid params: invalid type: string "route-probe", expected struct ReviewResolveParams
review.deliveries -32602 invalid params: invalid type: string "route-probe", expected struct ReviewDeliveriesParams
review.send -32602 invalid params: invalid type: string "route-probe", expected struct ReviewSendParams
server.status -32602 invalid params: invalid type: string "route-probe", expected struct Empty
server.stop -32602 invalid params: invalid type: string "route-probe", expected struct StopParams
sessions.hide -32042 session discovery is off (turn it on with `pod sessions scan --enable`)
sessions.list -32602 invalid params: invalid type: string "route-probe", expected struct SessionsListParams
sessions.register -32042 session discovery is off (turn it on with `pod sessions scan --enable`)
sessions.resume -32042 session discovery is off (turn it on with `pod sessions scan --enable`)
sessions.scan -32602 invalid params: invalid type: string "route-probe", expected struct SessionsScanParams
sessions.settings -32602 invalid params: invalid type: string "route-probe", expected struct SessionsSettingsParams
status.subscribe -32602 invalid params: invalid type: string "route-probe", expected struct StatusSubscribeParams
status.wait -32602 invalid params: invalid type: string "route-probe", expected struct StatusWaitParams
task.archive -32602 invalid params: invalid type: string "route-probe", expected struct TaskArchiveParams
task.cleanup_candidates -32602 invalid params: invalid type: string "route-probe", expected struct CleanupParams
task.create -32602 invalid params: invalid type: string "route-probe", expected struct TaskCreateParams
task.diff -32602 invalid params: invalid type: string "route-probe", expected struct TaskRef
task.list -32602 invalid params: invalid type: string "route-probe", expected struct TaskListParams
task.pr.create -32602 invalid params: invalid type: string "route-probe", expected struct TaskPrCreateParams
task.pr.merge -32602 invalid params: invalid type: string "route-probe", expected struct TaskPrMergeParams
task.pr.status -32602 invalid params: invalid type: string "route-probe", expected struct TaskPrStatusParams
task.remove -32602 invalid params: invalid type: string "route-probe", expected struct TaskRemoveParams
task.resume -32602 invalid params: invalid type: string "route-probe", expected struct TaskResumeParams
task.show -32602 invalid params: invalid type: string "route-probe", expected struct TaskRef
task.suggest_name -32602 invalid params: invalid type: string "route-probe", expected struct SuggestNameParams
task.touch -32602 invalid params: invalid type: string "route-probe", expected struct TaskTouchParams
task.unarchive -32602 invalid params: invalid type: string "route-probe", expected struct TaskUnarchiveParams
terminal.ack -32602 invalid params: invalid type: string "route-probe", expected struct TerminalRef
terminal.attach -32602 invalid params: invalid type: string "route-probe", expected struct AttachParams
terminal.input -32602 invalid params: invalid type: string "route-probe", expected struct InputParams
terminal.keep -32602 invalid params: invalid type: string "route-probe", expected struct KeepParams
terminal.keys -32602 invalid params: invalid type: string "route-probe", expected struct KeysParams
terminal.kill -32602 invalid params: invalid type: string "route-probe", expected struct TerminalRef
terminal.list -32602 invalid params: invalid type: string "route-probe", expected struct Empty
terminal.read -32602 invalid params: invalid type: string "route-probe", expected struct ReadParams
terminal.resize -32602 invalid params: invalid type: string "route-probe", expected struct ResizeParams
terminal.rm -32602 invalid params: invalid type: string "route-probe", expected struct RmParams
terminal.run -32602 invalid params: invalid type: string "route-probe", expected struct RunParams
terminal.send -32602 invalid params: invalid type: string "route-probe", expected struct SendParams
terminal.show -32602 invalid params: invalid type: string "route-probe", expected struct TerminalRef
terminal.wait -32602 invalid params: invalid type: string "route-probe", expected struct WaitParams"#;

/// A raw pipe connection: JSON lines out, JSON lines back.
struct Raw {
    lines: mpsc::Receiver<Option<String>>,
    writer: LineWriter,
}

impl Raw {
    fn open(pipe: &str) -> Self {
        // Busy until the daemon has its next pipe instance up.
        let deadline = Instant::now() + Duration::from_secs(10);
        let transport = loop {
            match Transport::open_pipe(pipe) {
                Ok(t) => break t,
                Err(e) => {
                    assert!(Instant::now() < deadline, "open {pipe}: {e}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        };
        let (mut reader, writer) = transport.into_parts();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let line = reader.read_line().ok().flatten();
                let end = line.is_none();
                if tx.send(line).is_err() || end {
                    break;
                }
            }
        });
        Self { lines, writer }
    }

    fn send(&mut self, v: Value) {
        self.writer.write_line(&v.to_string()).expect("write");
    }

    /// The next line, `None` once the daemon closed the connection.
    fn next(&self) -> Option<Value> {
        self.lines
            .recv_timeout(Duration::from_secs(10))
            .expect("no answer within 10 s")
            .map(|l| serde_json::from_str(&l).expect("a JSON line"))
    }

    fn call(&mut self, v: Value) -> Value {
        self.send(v);
        self.next().expect("the daemon closed the connection")
    }
}

/// `code message` of an error answer, `ok` of a result.
fn outcome(resp: &Value) -> String {
    match resp.get("error") {
        Some(e) => format!("{} {}", e["code"], e["message"].as_str().unwrap()),
        None => {
            assert!(
                resp.get("result").is_some(),
                "neither result nor error: {resp}"
            );
            "ok".into()
        }
    }
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn unknown(method: &str) -> String {
    format!("{} unknown method {method:?}", codes::METHOD_NOT_FOUND)
}

#[test]
fn every_public_method_answers_the_probe_as_before() {
    let schema = schema();
    let methods: BTreeSet<&str> = schema["methods"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(methods.len(), 104, "the public method set");

    let d = Daemon::start("routes");
    let mut got = BTreeMap::new();
    for (i, m) in methods.iter().enumerate() {
        // One connection each: a stream method ends its connection.
        let mut c = Raw::open(&d.pipe);
        let resp = c.call(request(i as u64, m, json!("route-probe")));
        assert_eq!(resp["id"], i, "{m}: {resp}");
        got.insert(*m, outcome(&resp));
    }
    let table: Vec<String> = got.iter().map(|(m, o)| format!("{m} {o}")).collect();
    let expected: Vec<&str> = PROBE.lines().collect();
    assert_eq!(
        table,
        expected,
        "answers to the probe (`method outcome`):\n{}",
        table.join("\n")
    );
    // Every method reached a handler: none answers as an unknown name.
    for (m, o) in &got {
        assert_ne!(*o, unknown(m), "{m} is not routed");
    }
}

#[test]
fn unknown_names_answer_method_not_found() {
    let d = Daemon::start("routes-unknown");
    let mut c = Raw::open(&d.pipe);
    // Names in a domain's namespace too: the domain dispatch answers them
    // with the same error as the central one.
    let names = [
        "nope.method",
        "",
        "orch",
        "orch.",
        "orch.nope",
        "mod.nope",
        "sessions.nope",
        "mobile.nope",
        "mobile.test.nope",
        "status.nope",
        "status.subscribe.nope",
        "terminal.nope",
        "repo.nope",
        "task.nope",
        "Server.Status",
    ];
    for (i, m) in names.into_iter().enumerate() {
        let resp = c.call(request(i as u64, m, json!({})));
        assert_eq!(resp["id"], i, "{m}: {resp}");
        assert_eq!(outcome(&resp), unknown(m), "{m}");
    }
}

#[test]
fn streams_notifications_and_the_version_check_keep_their_answers() {
    let d = Daemon::start("routes-streams");
    let mut c = Raw::open(&d.pipe);

    // A stream method as a notification goes to the dispatch, which answers
    // nothing to a notification; the connection stays a request connection.
    for m in [TerminalAttach::NAME, StatusSubscribe::NAME] {
        c.send(json!({"jsonrpc": "2.0", "method": m, "params": {"id": "t1"}}));
        c.send(json!({"jsonrpc": "2.0", "method": m}));
    }
    let resp = c.call(request(1, ServerStatus::NAME, json!({})));
    assert_eq!(
        (resp["id"].clone(), outcome(&resp)),
        (json!(1), "ok".into())
    );

    // The version check of a request and of a notification.
    let resp = c.call(json!({"jsonrpc": "1.0", "id": 2, "method": ServerStatus::NAME}));
    assert_eq!(
        (resp["id"].clone(), outcome(&resp)),
        (
            json!(2),
            format!("{} jsonrpc must be \"2.0\"", codes::INVALID_REQUEST)
        )
    );
    let resp = c.call(json!({"jsonrpc": "1.0", "method": ServerStatus::NAME}));
    assert_eq!(
        (resp["id"].clone(), outcome(&resp)),
        (
            Value::Null,
            format!("{} jsonrpc must be \"2.0\"", codes::INVALID_REQUEST)
        )
    );
    // An unknown notification gets no answer either.
    c.send(json!({"jsonrpc": "2.0", "method": "nope.method"}));
    let resp = c.call(request(3, ServerStatus::NAME, json!({})));
    assert_eq!(resp["id"], 3, "{resp}");

    // A stream request switches before the version check.
    let mut s = Raw::open(&d.pipe);
    let resp = s.call(json!({"jsonrpc": "1.0", "id": 4, "method": StatusSubscribe::NAME}));
    assert_eq!(
        (resp["id"].clone(), outcome(&resp)),
        (json!(4), "ok".into())
    );
    let mut a = Raw::open(&d.pipe);
    let resp = a.call(json!({
        "jsonrpc": "1.0", "id": 5, "method": TerminalAttach::NAME, "params": {"id": "t999"},
    }));
    assert_eq!(resp["id"], 5, "{resp}");
    assert_eq!(outcome(&resp), "-32001 no terminal t999", "{resp}");
}

#[test]
fn a_method_without_params_ignores_them() {
    let d = Daemon::start("routes-params");
    let mut c = Raw::open(&d.pipe);
    let plain = c.call(request(1, OrchConfigGet::NAME, Value::Null));
    for (i, junk) in [json!("route-probe"), json!([1, 2]), json!({"x": 1})]
        .into_iter()
        .enumerate()
    {
        let resp = c.call(request(10 + i as u64, OrchConfigGet::NAME, junk));
        assert_eq!(resp["result"], plain["result"], "{resp}");
    }
    // A method with params rejects the same junk.
    let resp = c.call(request(2, ServerStatus::NAME, json!([1, 2])));
    assert_eq!(resp["error"]["code"], codes::INVALID_PARAMS, "{resp}");
}

#[test]
fn mobile_test_methods_stay_behind_their_gate() {
    let hooks = [
        "mobile.test.pair",
        "mobile.test.session",
        "mobile.test.call",
    ];
    let off = Daemon::start("routes-hooks-off");
    let mut c = Raw::open(&off.pipe);
    for (i, m) in hooks.into_iter().enumerate() {
        let resp = c.call(request(i as u64, m, json!("route-probe")));
        assert_eq!(outcome(&resp), unknown(m), "{m} without the gate");
    }
    drop(c);
    drop(off);

    let on = Daemon::start_env("routes-hooks-on", &[("POD_MOBILE_TEST_HOOKS", "1")]);
    let mut c = Raw::open(&on.pipe);
    for (i, m) in hooks.into_iter().enumerate() {
        let resp = c.call(request(i as u64, m, json!("route-probe")));
        let o = outcome(&resp);
        if cfg!(debug_assertions) {
            assert!(
                o.starts_with(&format!("{} invalid params", codes::INVALID_PARAMS)),
                "{m} with the gate in a debug build: {o}"
            );
        } else {
            assert_eq!(o, unknown(m), "{m} in a release build");
        }
    }
}
