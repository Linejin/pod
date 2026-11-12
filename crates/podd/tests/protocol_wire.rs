//! Wire contract golden (refactor card D-F0, docs/20-refactor/daemon.md
//! §3, completion condition W). Taken from master before the protocol
//! types move to their own crate; later cards keep these tests green
//! without touching the golden files. Rewritten for v20 (docs/17 R4:
//! `terminal.input_once`, `terminal.snapshot`, the `AttachResult` chunk
//! fields, `attach.chunks` and the remote framing constants).
//!
//! - `fixtures/schema-v20.json`: the whole `podd schema` output.
//! - `fixtures/wire-v20.json`: the params and result type of each of the
//!   105 methods, the event type of each of the 19 notifications, and
//!   samples of every such type: one per enum variant position (`full#k`,
//!   every optional field set, the k-th variant of every enum), only the
//!   required fields (`min`) and every nullable field null (`null`). A
//!   sample's `in` goes through the real Rust type of the method
//!   (deserialize, then serialize) and must give `out` (`in` when absent).
//!   Also: error codes and error data, `connection.hello` negotiation,
//!   the v18 and v19 downgrades of every result and event sample, envelopes,
//!   base64 and the protocol constants.
//!
//! Objects compare without key order; everything else (absent vs null,
//! defaults, enum spelling, array order, numbers, text) compares exactly.
//!
//! The golden is written by hand, only for a new protocol version:
//! `POD_WIRE_GOLDEN_OUT=<dir> cargo test -p podd --test protocol_wire --
//! --ignored write_golden` writes both files into `<dir>`; review the diff
//! before copying them here. A refactor never rewrites them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use podd::holder_proto::{HOLDER_PROTOCOL, HOLDER_PROTOCOL_MIN};
use podd::protocol::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

const WIRE: &str = "wire-v20.json";
const SCHEMA: &str = "schema-v20.json";
/// The newest protocol without the v19 `unreachable` terminal state.
const DOWNGRADE_TO: u32 = 18;
/// The newest protocol without the v20 attach chunk fields.
const DOWNGRADE_TO_V19: u32 = 19;

// ───────────────────────────── the Rust types ─────────────────────────────

/// Deserialize into `T`, serialize again.
type RoundTrip = fn(&Value) -> Result<Value, String>;

fn roundtrip<T: Serialize + DeserializeOwned>(v: &Value) -> Result<Value, String> {
    let t: T = serde_json::from_value(v.clone()).map_err(|e| format!("deserialize: {e}"))?;
    serde_json::to_value(&t).map_err(|e| format!("serialize: {e}"))
}

struct MethodTypes {
    name: &'static str,
    params: RoundTrip,
    result: RoundTrip,
}

macro_rules! method_types {
    ($($m:ident),* $(,)?) => {
        fn method_types() -> Vec<MethodTypes> {
            vec![$(MethodTypes {
                name: <$m as Method>::NAME,
                params: roundtrip::<<$m as Method>::Params>,
                result: roundtrip::<<$m as Method>::Result>,
            }),*]
        }
    };
}

// The order of `schema()`; the tests compare the set with the schema's.
method_types!(
    ServerStatus,
    ServerStop,
    TerminalRun,
    TerminalList,
    TerminalSend,
    TerminalKeys,
    TerminalInput,
    TerminalRead,
    TerminalWait,
    TerminalResize,
    TerminalKill,
    TerminalRm,
    TerminalKeep,
    TerminalAttach,
    TerminalShow,
    TerminalAck,
    StatusWait,
    StatusSubscribe,
    RepoAdd,
    RepoList,
    RepoRemove,
    TaskCreate,
    TaskList,
    TaskShow,
    TaskDiff,
    TaskRemove,
    TaskTouch,
    TaskPrStatusMethod,
    TaskPrCreate,
    TaskPrMerge,
    ReviewCommentsAdd,
    ReviewCommentsList,
    ReviewCommentsResolve,
    ReviewCommentsRemove,
    ReviewSend,
    ReviewDeliveries,
    ReviewCancel,
    GroupCreate,
    GroupList,
    GroupShow,
    GroupCompare,
    GroupCheck,
    GroupCheckCancel,
    GroupPick,
    GroupRemove,
    GateCreate,
    GateResolve,
    GateList,
    GateShow,
    GateWait,
    ModListMethod,
    ModSetEnabled,
    ModGrant,
    ModLog,
    ModCommand,
    ModReload,
    OrchRunCreate,
    OrchRunList,
    OrchRunShow,
    OrchItemCreate,
    OrchItemList,
    OrchItemUpdate,
    OrchWorkerStart,
    OrchWorkerList,
    OrchWorkerShow,
    OrchWorkerRead,
    OrchWorkerStop,
    OrchWorkerAbandon,
    OrchWorkerKeep,
    OrchWorkerRelease,
    OrchMailSend,
    OrchMailCheck,
    OrchMailReply,
    OrchMailAsk,
    OrchDone,
    OrchMailList,
    OrchConfigGet,
    OrchConfigSet,
    OrchQueue,
    OrchGateRequest,
    OrchGateDecide,
    OrchLimitsGet,
    OrchLimitsSet,
    OrchWait,
    OrchCommit,
    SessionsListMethod,
    SessionsScan,
    SessionsSettings,
    SessionsRegister,
    SessionsResume,
    SessionsHide,
    ConnectionHello,
    TerminalInputOnce,
    MobileDevices,
    MobileRevoke,
    MobilePolicy,
    MobileAudit,
    MobileSettingsMethod,
    TaskArchive,
    TaskUnarchive,
    TaskResume,
    TaskCleanupCandidates,
    TaskSuggestName,
    RepoConfigMethod,
    RepoSet,
);

fn notification_types() -> Vec<(&'static str, RoundTrip)> {
    macro_rules! n {
        ($($name:ident => $t:ty),* $(,)?) => {
            vec![$(($name, roundtrip::<$t> as RoundTrip)),*]
        };
    }
    n!(
        NOTIFY_OUTPUT => OutputEvent,
        NOTIFY_RESIZED => ResizedEvent,
        NOTIFY_EXITED => ExitedEvent,
        NOTIFY_LAGGED => LaggedEvent,
        NOTIFY_SNAPSHOT => SnapshotChunkEvent,
        NOTIFY_STATUS_CHANGED => StatusChangedEvent,
        NOTIFY_STATUS_LAGGED => StatusLaggedEvent,
        NOTIFY_GATE_CHANGED => GateChangedEvent,
        NOTIFY_GROUP_CHECK => GroupCheckEvent,
        NOTIFY_REVIEW_DELIVERY => ReviewDeliveryEvent,
        NOTIFY_TERMINAL_CHANGED => TerminalChangedEvent,
        NOTIFY_TASK_CHANGED => TaskChangedEvent,
        NOTIFY_TASK_REMOVING => TaskRemovingEvent,
        NOTIFY_TASK_PR_CHANGED => TaskPrChangedEvent,
        NOTIFY_MOD_NOTIFY => ModNotifyEvent,
        NOTIFY_MOD_CHANGED => ModChangedEvent,
        NOTIFY_ORCH_CHANGED => OrchChangedEvent,
        NOTIFY_ORCH_MAIL => OrchMailEvent,
        NOTIFY_SESSIONS_CHANGED => SessionsChangedEvent,
    )
}

/// Every error code constant, by its path.
fn error_codes() -> Value {
    macro_rules! c {
        ($($path:path),* $(,)?) => {{
            let mut m = Map::new();
            $(m.insert(stringify!($path).to_string(), json!($path));)*
            Value::Object(m)
        }};
    }
    c!(
        codes::PARSE_ERROR,
        codes::INVALID_REQUEST,
        codes::METHOD_NOT_FOUND,
        codes::INVALID_PARAMS,
        codes::INTERNAL_ERROR,
        codes::TERMINAL_NOT_FOUND,
        codes::NAME_TAKEN,
        codes::SPAWN_FAILED,
        codes::TERMINAL_EXITED,
        codes::TERMINAL_RUNNING,
        codes::NOT_A_REPO,
        codes::REPO_NOT_FOUND,
        codes::REPO_EXISTS,
        codes::REPO_IN_USE,
        codes::TASK_NOT_FOUND,
        codes::TASK_EXISTS,
        codes::TASK_DIRTY,
        codes::GIT_FAILED,
        codes::SERVER_STOPPING,
        codes::INPUT_BACKLOG,
        orch_codes::AGENT_BLOCKED,
        orch_codes::REVIEW_NOT_FOUND,
        orch_codes::GROUP_NOT_FOUND,
        orch_codes::GROUP_EXISTS,
        orch_codes::GATE_NOT_FOUND,
        orch_codes::GATE_RESOLVED,
        orch_codes::INVALID_STATE,
        pr_codes::FORGE_FAILED,
        sessions_codes::SESSION_NOT_FOUND,
        sessions_codes::SESSION_RUNNING,
        sessions_codes::DISCOVERY_OFF,
        sessions_codes::PERSIST_FAILED,
        remote_codes::PROTOCOL_MISMATCH,
        mobile_codes::MOBILE_DENIED,
        mobile_codes::MOBILE_AUTH,
        mobile_codes::RATE_LIMITED,
        mobile_codes::OPERATION_CONFLICT,
        mobile_codes::MESSAGE_INVALID,
        session_codes::TASK_BUSY,
        session_codes::ARCHIVE_FAILED,
        session_codes::WORKTREE_LOCKED,
        run_codes::RUN_NOT_FOUND,
        run_codes::ITEM_NOT_FOUND,
        run_codes::ATTEMPT_NOT_FOUND,
        run_codes::ATTEMPT_FENCED,
        run_codes::DEPTH_EXCEEDED,
        run_codes::CIRCUIT_OPEN,
        run_codes::DEPS_NOT_DONE,
        run_codes::WORKER_LIMIT,
        run_codes::MAIL_NOT_FOUND,
    )
}

/// Versions, method lists (in their documented order), notification
/// names, error codes and capability names.
fn constants() -> Value {
    json!({
        "PROTOCOL_VERSION": PROTOCOL_VERSION,
        "PROTOCOL_MIN": PROTOCOL_MIN,
        "HELLO_PROTOCOL": HELLO_PROTOCOL,
        "UNREACHABLE_PROTOCOL": UNREACHABLE_PROTOCOL,
        "CHUNKED_ATTACH_PROTOCOL": CHUNKED_ATTACH_PROTOCOL,
        "remote_framing": {
            "REMOTE_OUTPUT_CHUNK": REMOTE_OUTPUT_CHUNK,
            "REMOTE_ATTACH_QUEUE": REMOTE_ATTACH_QUEUE,
            "MAX_SNAPSHOT_BYTES": MAX_SNAPSHOT_BYTES,
            "INPUT_STREAMS_KEPT": INPUT_STREAMS_KEPT,
            "INPUT_STREAM_MAX": INPUT_STREAM_MAX,
        },
        "JSONRPC_VERSION": JSONRPC_VERSION,
        "HOLDER_PROTOCOL": HOLDER_PROTOCOL,
        "HOLDER_PROTOCOL_MIN": HOLDER_PROTOCOL_MIN,
        "HOOK_MESSAGE_VERSION": pod_status::hook::HOOK_MESSAGE_VERSION,
        "server_range": server_range(),
        "method_lists": {
            "METHODS": METHODS,
            "STATUS_METHODS": STATUS_METHODS,
            "ORCH_METHODS": ORCH_METHODS,
            "PR_METHODS": PR_METHODS,
            "ORCH_RUN_METHODS": ORCH_RUN_METHODS,
            "MOBILE_METHODS": MOBILE_METHODS,
            "SESSION_METHODS": SESSION_METHODS,
            "SESSIONS_METHODS": SESSIONS_METHODS,
        },
        "notifications": notification_types().iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        "codes": error_codes(),
        "capability": [
            capability::REMOTE_FILES,
            capability::REMOTE_DIFF,
            capability::ATTACH_CHUNKS,
            capability::GATE_CAS,
        ],
        "CAPABILITIES": CAPABILITIES,
        "mobile_api::ALL": mobile_api::ALL,
    })
}

// ───────────────────────────── golden files ─────────────────────────────

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

fn load(name: &str) -> Value {
    let p = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Objects with sorted keys (whatever map order serde_json was built with).
fn sorted(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let b: BTreeMap<&String, Value> = m.iter().map(|(k, v)| (k, sorted(v))).collect();
            Value::Object(b.into_iter().map(|(k, v)| (k.clone(), v)).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// Mismatches of one test, reported together.
#[derive(Default)]
struct Diffs(Vec<String>);

impl Diffs {
    fn check(&mut self, what: impl std::fmt::Display, got: &Value, want: &Value) {
        if got != want {
            self.0
                .push(format!("{what}\n   got: {got}\n  want: {want}"));
        }
    }
    fn fail(&mut self, what: impl std::fmt::Display) {
        self.0.push(what.to_string());
    }
    fn assert_none(self) {
        if !self.0.is_empty() {
            let shown: Vec<&str> = self.0.iter().take(25).map(String::as_str).collect();
            panic!(
                "{} wire difference(s) from the master golden:\n{}",
                self.0.len(),
                shown.join("\n")
            );
        }
    }
}

fn sample_out(s: &Value) -> &Value {
    s.get("out").unwrap_or(&s["in"])
}

/// Round-trip every golden sample of the type `key` through `rt`.
fn check_samples(d: &mut Diffs, golden: &Value, owner: &str, key: &str, rt: RoundTrip) {
    let Some(samples) = golden["types"][key].as_array() else {
        d.fail(format!("{owner}: no samples for type {key}"));
        return;
    };
    for s in samples {
        let case = s["case"].as_str().unwrap_or("?");
        let want = sample_out(s);
        for (from, input) in [("in", &s["in"]), ("out", want)] {
            match rt(input) {
                Ok(got) => d.check(format!("{owner} {key} {case} ({from})"), &got, want),
                Err(e) => d.fail(format!(
                    "{owner} {key} {case} ({from}): {e}\n  json: {input}"
                )),
            }
        }
    }
}

// ───────────────────────────── tests ─────────────────────────────

/// `podd::protocol::schema()` is the master schema, key order aside.
#[test]
fn schema_matches_master_golden() {
    let golden = load(SCHEMA);
    let mut d = Diffs::default();
    let now = schema();
    for part in ["protocol", "min_protocol", "transport", "error"] {
        d.check(format!("schema.{part}"), &now[part], &golden[part]);
    }
    for part in ["methods", "notifications"] {
        let (a, b) = (
            now[part].as_object().unwrap(),
            golden[part].as_object().unwrap(),
        );
        let keys: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
        for k in keys {
            d.check(
                format!("schema.{part}.{k}"),
                a.get(k).unwrap_or(&Value::Null),
                b.get(k).unwrap_or(&Value::Null),
            );
        }
    }
    d.check("schema (whole)", &now, &golden);
    d.assert_none();
    assert_eq!(golden["methods"].as_object().unwrap().len(), 105);
    assert_eq!(golden["notifications"].as_object().unwrap().len(), 19);
}

/// `podd schema` prints the same schema.
#[test]
fn podd_schema_command_prints_the_golden() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_podd"))
        .arg("schema")
        .output()
        .expect("run podd schema");
    assert!(out.status.success(), "{out:?}");
    let printed: Value = serde_json::from_slice(&out.stdout).expect("schema JSON");
    assert!(printed == load(SCHEMA), "podd schema differs from {SCHEMA}");
}

/// Methods and notifications: the same names, and every golden sample of
/// their types survives the real Rust types unchanged.
#[test]
fn every_method_and_notification_roundtrips_its_golden_samples() {
    let golden = load(WIRE);
    let mut d = Diffs::default();

    let methods = method_types();
    let names: BTreeSet<&str> = methods.iter().map(|m| m.name).collect();
    let in_golden: BTreeSet<&str> = golden["methods"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let s = schema();
    let in_schema: BTreeSet<&str> = s["methods"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(methods.len(), 105, "method table");
    assert_eq!(names, in_golden, "methods: Rust table vs golden");
    assert_eq!(names, in_schema, "methods: Rust table vs schema()");
    for m in &methods {
        let types = &golden["methods"][m.name];
        for (role, rt) in [("params", m.params), ("result", m.result)] {
            let key = types[role].as_str().unwrap();
            check_samples(&mut d, &golden, &format!("{} {role}", m.name), key, rt);
        }
    }

    let events = notification_types();
    let names: BTreeSet<&str> = events.iter().map(|(n, _)| *n).collect();
    let in_golden: BTreeSet<&str> = golden["notifications"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let in_schema: BTreeSet<&str> = s["notifications"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(names, in_golden, "notifications: Rust table vs golden");
    assert_eq!(names, in_schema, "notifications: Rust table vs schema()");
    for (name, rt) in events {
        let key = golden["notifications"][name].as_str().unwrap();
        check_samples(&mut d, &golden, name, key, rt);
    }
    d.assert_none();
}

/// Versions, method lists, notification names, error codes, capabilities.
#[test]
fn constants_match_master_golden() {
    let golden = load(WIRE);
    let mut d = Diffs::default();
    let now = constants();
    for (k, v) in golden["constants"].as_object().unwrap() {
        d.check(format!("constants.{k}"), &now[k], v);
    }
    d.check("constants (whole)", &now, &golden["constants"]);
    d.assert_none();
}

/// Error objects (with the `data` shapes the daemon sends), envelopes and
/// base64 keep their JSON.
#[test]
fn errors_envelopes_and_base64_match_master_golden() {
    let golden = load(WIRE);
    let mut d = Diffs::default();
    for (section, rt) in [
        ("errors", roundtrip::<RpcError> as RoundTrip),
        ("requests", roundtrip::<Request>),
        ("responses", roundtrip::<Response>),
        ("notification_envelopes", roundtrip::<Notification>),
    ] {
        for (i, s) in golden[section].as_array().unwrap().iter().enumerate() {
            let want = sample_out(s);
            match rt(&s["in"]) {
                Ok(got) => d.check(format!("{section}[{i}]"), &got, want),
                Err(e) => d.fail(format!("{section}[{i}]: {e}")),
            }
        }
    }
    for (i, s) in golden["base64"].as_array().unwrap().iter().enumerate() {
        let bytes: Vec<u8> = serde_json::from_value(s["bytes"].clone()).unwrap();
        d.check(
            format!("base64[{i}] encode"),
            &json!(b64_encode(&bytes)),
            &s["text"],
        );
        let back = b64_decode(s["text"].as_str().unwrap()).map(|b| json!(b));
        d.check(
            format!("base64[{i}] decode"),
            &back.unwrap_or(Value::Null),
            &s["bytes"],
        );
    }
    d.assert_none();
}

/// `connection.hello` version and capability negotiation.
#[test]
fn hello_negotiation_matches_master_golden() {
    let golden = load(WIRE);
    let mut d = Diffs::default();
    for (i, case) in golden["negotiate"].as_array().unwrap().iter().enumerate() {
        let p: HelloParams = serde_json::from_value(case["params"].clone()).unwrap();
        d.check(
            format!("negotiate[{i}]"),
            &negotiate_json(&p),
            &case["outcome"],
        );
    }
    d.assert_none();
}

/// Every result and event sample, rewritten for a v18 client.
#[test]
fn v18_downgrade_matches_master_golden() {
    let golden = load(WIRE);
    let now = downgrades(&golden);
    let mut d = Diffs::default();
    d.check(
        "downgrade_v18 count",
        &json!(now.len()),
        &json!(golden["downgrade_v18"].as_array().unwrap().len()),
    );
    for (i, (got, want)) in now
        .iter()
        .zip(golden["downgrade_v18"].as_array().unwrap())
        .enumerate()
    {
        d.check(format!("downgrade_v18[{i}]"), got, want);
    }
    d.assert_none();
    // Older clients get the same rewrite.
    for case in golden["downgrade_v18"].as_array().unwrap().iter().take(3) {
        let samples = golden["types"][case["type"].as_str().unwrap()]
            .as_array()
            .unwrap();
        let s = samples.iter().find(|s| s["case"] == case["case"]).unwrap();
        let mut v = sample_out(s).clone();
        downgrade_json(&mut v, PROTOCOL_MIN);
        assert_eq!(v, case["v18"]);
    }
}

/// Every result and event sample, rewritten for a v19 client: only the v20
/// attach chunk fields go (the `unreachable` state stays).
#[test]
fn v19_downgrade_matches_golden() {
    let golden = load(WIRE);
    let now = downgrades_to(&golden, DOWNGRADE_TO_V19, "v19");
    let want = golden["downgrade_v19"].as_array().unwrap();
    assert!(!want.is_empty());
    let mut d = Diffs::default();
    d.check("downgrade_v19 count", &json!(now.len()), &json!(want.len()));
    for (i, (got, want)) in now.iter().zip(want).enumerate() {
        d.check(format!("downgrade_v19[{i}]"), got, want);
    }
    d.assert_none();
    for case in want {
        let v19 = case["v19"].to_string();
        assert!(!v19.contains("snapshot_chunks"), "{v19}");
        assert!(!v19.contains("scrollback_omitted"), "{v19}");
    }
}

// ───────────────────────────── shared by tests and writer ─────────────────────────────

fn negotiate_json(p: &HelloParams) -> Value {
    match negotiate(p, server_range(), CAPABILITIES) {
        Ok(v) => json!({ "ok": v }),
        Err(e) => json!({ "error": e }),
    }
}

/// Type keys of method results and notification events, in golden order.
fn outgoing_keys(golden: &Value) -> Vec<String> {
    let mut keys: BTreeSet<String> = golden["methods"]
        .as_object()
        .unwrap()
        .values()
        .map(|t| t["result"].as_str().unwrap().to_string())
        .collect();
    keys.extend(
        golden["notifications"]
            .as_object()
            .unwrap()
            .values()
            .map(|k| k.as_str().unwrap().to_string()),
    );
    keys.into_iter().collect()
}

/// `{type, case, v18}` for every outgoing sample the downgrade changes.
fn downgrades(golden: &Value) -> Vec<Value> {
    downgrades_to(golden, DOWNGRADE_TO, "v18")
}

/// `{type, case, <label>}` for every outgoing sample the downgrade to
/// `protocol` changes.
fn downgrades_to(golden: &Value, protocol: u32, label: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for key in outgoing_keys(golden) {
        for s in golden["types"][&key].as_array().unwrap() {
            let v = sample_out(s);
            let mut down = v.clone();
            downgrade_json(&mut down, protocol);
            if &down != v {
                out.push(json!({ "type": key, "case": s["case"], label: down }));
            }
        }
    }
    out
}

// ───────────────────────────── the writer ─────────────────────────────

/// Writes a new golden into `$POD_WIRE_GOLDEN_OUT` (see the module docs).
#[test]
#[ignore = "writes a new golden for a protocol version bump; see the module docs"]
fn write_golden() {
    let Some(dir) = std::env::var_os("POD_WIRE_GOLDEN_OUT") else {
        eprintln!("POD_WIRE_GOLDEN_OUT is not set: nothing written");
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let schema = schema();

    // Type key per method role / notification: the schema title, unless
    // two different schemas share it.
    let mut types: BTreeMap<String, (Value, RoundTrip)> = BTreeMap::new();
    let mut key_for = |root: &Value, fallback: String, rt: RoundTrip| -> String {
        let title = root["title"].as_str().map(str::to_string);
        let key = match title {
            Some(t) if types.get(&t).is_none_or(|(s, _)| s == root) => t,
            _ => fallback,
        };
        types.entry(key.clone()).or_insert((root.clone(), rt));
        key
    };
    let mut methods = Map::new();
    for m in method_types() {
        let entry = &schema["methods"][m.name];
        let p = key_for(&entry["params"], format!("{}.params", m.name), m.params);
        let r = key_for(&entry["result"], format!("{}.result", m.name), m.result);
        methods.insert(m.name.into(), json!({ "params": p, "result": r }));
    }
    let mut notifications = Map::new();
    for (name, rt) in notification_types() {
        let k = key_for(&schema["notifications"][name], format!("{name}.event"), rt);
        notifications.insert(name.into(), json!(k));
    }

    let mut failures = Vec::new();
    let mut samples = Map::new();
    for (key, (root, rt)) in &types {
        let mut list: Vec<Value> = Vec::new();
        for (case, input) in generate(root) {
            if list.iter().any(|s| s["in"] == input) {
                continue;
            }
            match rt(&input) {
                Ok(out) => {
                    let mut s = json!({ "case": case, "in": input });
                    if out != input {
                        s["out"] = out;
                    }
                    list.push(s);
                }
                Err(e) => failures.push(format!("{key} {case}: {e}\n  json: {input}")),
            }
        }
        samples.insert(key.clone(), Value::Array(list));
    }
    assert!(
        failures.is_empty(),
        "samples the types refuse:\n{}",
        failures.join("\n")
    );

    let mut golden = json!({
        "about": "Wire golden of protocol v20 (refactor card D-F0, rewritten for docs/17 R4); see crates/podd/tests/protocol_wire.rs",
        "constants": constants(),
        "methods": methods,
        "notifications": notifications,
        "types": samples,
        "errors": error_samples(),
        "negotiate": negotiate_cases(),
        "requests": [
            { "in": { "jsonrpc": "2.0", "id": 1, "method": "terminal.list" } },
            { "in": { "jsonrpc": "2.0", "id": "a", "method": "terminal.run", "params": { "argv": ["cmd"] } } },
            { "in": { "jsonrpc": "2.0", "method": "status.subscribe", "params": {} } },
            { "in": { "jsonrpc": "2.0", "id": null, "method": "server.status", "params": null } },
        ],
        "responses": [
            { "in": { "jsonrpc": "2.0", "id": 1, "result": { "terminals": [] } } },
            { "in": { "jsonrpc": "2.0", "id": 2, "error": { "code": -32601, "message": "method not found: x" } } },
            { "in": { "jsonrpc": "2.0", "id": "a", "result": null } },
        ],
        "notification_envelopes": [
            { "in": { "jsonrpc": "2.0", "method": "terminal.output", "params": { "id": "t1", "seq": 3, "data": "aGk=" } } },
        ],
        "base64": [
            b64_case(b""),
            b64_case(b"f"),
            b64_case(b"fo"),
            b64_case(b"foo"),
            b64_case(b"\x1b[31mhi\r\n\xff"),
        ],
    });
    for section in ["requests", "responses", "notification_envelopes"] {
        let rt: RoundTrip = match section {
            "requests" => roundtrip::<Request>,
            "responses" => roundtrip::<Response>,
            _ => roundtrip::<Notification>,
        };
        for s in golden[section].as_array_mut().unwrap() {
            let out = rt(&s["in"]).unwrap();
            if out != s["in"] {
                s["out"] = out;
            }
        }
    }
    golden["downgrade_v18"] = Value::Array(downgrades(&golden));
    golden["downgrade_v19"] = Value::Array(downgrades_to(&golden, DOWNGRADE_TO_V19, "v19"));

    for (name, v, levels) in [(WIRE, &golden, 3), (SCHEMA, &schema, 2)] {
        let mut text = String::new();
        layout(&sorted(v), levels, 0, &mut text);
        text.push('\n');
        std::fs::write(dir.join(name), text).unwrap();
    }
    eprintln!("wrote {WIRE} and {SCHEMA} into {}", dir.display());
}

fn b64_case(bytes: &[u8]) -> Value {
    json!({ "bytes": bytes, "text": b64_encode(bytes) })
}

/// The error `data` shapes the daemon sends (crates/podd/src/server), plus
/// errors without data. Messages are examples; codes and keys are the
/// contract.
fn error_samples() -> Vec<Value> {
    let mismatch = negotiate(
        &HelloParams {
            min_protocol: 1,
            max_protocol: 10,
            ..Default::default()
        },
        server_range(),
        CAPABILITIES,
    )
    .unwrap_err();
    let errors = [
        json!({ "code": codes::METHOD_NOT_FOUND, "message": "method not found: x" }),
        json!({ "code": codes::INVALID_PARAMS, "message": "invalid params", "data": null }),
        json!({ "code": codes::TASK_DIRTY, "message": "task has changes",
                "data": { "dirty": 2, "unpushed": 1 } }),
        json!({ "code": orch_codes::INVALID_STATE, "message": "no commits ahead",
                "data": { "ahead": 0, "dirty": 3 } }),
        json!({ "code": orch_codes::AGENT_BLOCKED, "message": "the agent is waiting for you",
                "data": { "state": "blocked", "msg": null } }),
        json!({ "code": sessions_codes::PERSIST_FAILED, "message": "the disk could not be updated",
                "data": { "paths": ["C:\\state\\sessions.json"], "error": "denied" } }),
        json!({ "code": sessions_codes::SESSION_RUNNING, "message": "running in Pod terminal t3",
                "data": { "pid": 42, "pod_terminal": "t3" } }),
        json!({ "code": mobile_codes::RATE_LIMITED, "message": "slow down",
                "data": { "retry_after_ms": 1500 } }),
        json!({ "code": session_codes::TASK_BUSY, "message": "task is busy",
                "data": { "terminal": "t2", "state": "working" } }),
        json!({ "code": session_codes::ARCHIVE_FAILED, "message": "archive script changed",
                "data": { "hash": "abc", "output": "" } }),
        json!({ "code": session_codes::ARCHIVE_FAILED, "message": "archive script failed",
                "data": { "exit_code": 1, "output": "boom" } }),
        serde_json::to_value(mismatch).unwrap(),
    ];
    errors
        .into_iter()
        .map(|e| {
            let out = roundtrip::<RpcError>(&e).unwrap();
            if out == e {
                json!({ "in": e })
            } else {
                json!({ "in": e, "out": out })
            }
        })
        .collect()
}

fn negotiate_cases() -> Vec<Value> {
    let cases = [
        json!({ "min_protocol": 11, "max_protocol": 19 }),
        json!({ "min_protocol": 12, "max_protocol": 25 }),
        json!({ "min_protocol": 1, "max_protocol": 11 }),
        json!({ "min_protocol": 1, "max_protocol": 10 }),
        json!({ "min_protocol": 20, "max_protocol": 30 }),
        json!({ "min_protocol": 12, "max_protocol": 19, "require": ["remote.files"] }),
        json!({ "min_protocol": 19, "max_protocol": 12 }),
        json!({ "min_protocol": 12, "max_protocol": 19, "client": "pod-app 1.0" }),
        // v20 (docs/17 R4): a client newer than this podd only, an older
        // remote client (keeps v19), and one that needs chunked attach.
        json!({ "min_protocol": 21, "max_protocol": 30 }),
        json!({ "min_protocol": 12, "max_protocol": 19, "client": "pod 0.19" }),
        json!({ "min_protocol": 12, "max_protocol": 20, "require": ["attach.chunks"] }),
    ];
    cases
        .into_iter()
        .map(|params| {
            let p: HelloParams = serde_json::from_value(params.clone()).unwrap();
            json!({ "params": params, "outcome": negotiate_json(&p) })
        })
        .collect()
}

// ───────────────────────────── samples from a schema ─────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Every property; at each choice the option that covers the most
    /// not-yet-sampled choices (enum values, oneOf/anyOf variants).
    Full,
    /// Required properties only, first options.
    Min,
    /// Nullable properties null, other optional ones left out.
    Null,
}

/// Below this depth samples stop growing (recursive types).
const GROW_DEPTH: usize = 24;
/// More `full#k` samples than any type needs; a safety stop.
const MAX_FULL: usize = 64;

/// One option of a choice: a sub-schema, or one value of an `enum`
/// (an option that is itself a plain enum counts value by value).
#[derive(Clone, Copy)]
enum Opt<'a> {
    Schema(&'a Value),
    Literal(&'a Value),
}

struct Gen<'a> {
    defs: Option<&'a Map<String, Value>>,
    /// `(choice node address, option index)` already in a sample.
    covered: std::cell::RefCell<BTreeSet<(usize, usize)>>,
}

/// `full#0..n` until every reachable option is in some sample, `min`, `null`.
fn generate(root: &Value) -> Vec<(String, Value)> {
    let g = Gen {
        defs: root.get("$defs").and_then(Value::as_object),
        covered: Default::default(),
    };
    let mut out = Vec::new();
    for k in 0..MAX_FULL {
        let before = g.covered.borrow().len();
        let v = g.value(root, Mode::Full, 0);
        if k > 0 && g.covered.borrow().len() == before {
            break;
        }
        out.push((format!("full#{k}"), v));
    }
    assert!(out.len() < MAX_FULL, "too many samples: {root}");
    out.push(("min".into(), g.value(root, Mode::Min, 0)));
    out.push(("null".into(), g.value(root, Mode::Null, 0)));
    out
}

impl<'a> Gen<'a> {
    fn deref(&self, s: &'a Value) -> &'a Value {
        match s.get("$ref").and_then(Value::as_str) {
            Some(r) => {
                let name = r
                    .strip_prefix("#/$defs/")
                    .unwrap_or_else(|| panic!("ref {r}"));
                self.deref(&self.defs.unwrap()[name])
            }
            None => s,
        }
    }

    /// The options of a choice node (`enum`, `oneOf`, `anyOf`), null left out.
    fn options(&self, s: &'a Value) -> Option<Vec<Opt<'a>>> {
        if let Some(e) = s.get("enum").and_then(Value::as_array) {
            return Some(
                e.iter()
                    .filter(|v| !v.is_null())
                    .map(Opt::Literal)
                    .collect(),
            );
        }
        let opts = s.get("oneOf").or_else(|| s.get("anyOf"))?.as_array()?;
        let mut out = Vec::new();
        for o in opts.iter().filter(|o| !is_null_schema(o)) {
            let d = self.deref(o);
            match d.get("enum").and_then(Value::as_array) {
                Some(e) if d.get("properties").is_none() => {
                    out.extend(e.iter().filter(|v| !v.is_null()).map(Opt::Literal))
                }
                _ => out.push(Opt::Schema(o)),
            }
        }
        Some(out)
    }

    fn nullable(&self, s: &'a Value) -> bool {
        let s = self.deref(s);
        if let Some(Value::Array(t)) = s.get("type")
            && t.iter().any(|t| t == "null")
        {
            return true;
        }
        if let Some(e) = s.get("enum").and_then(Value::as_array)
            && e.iter().any(Value::is_null)
        {
            return true;
        }
        s.get("oneOf")
            .or_else(|| s.get("anyOf"))
            .and_then(Value::as_array)
            .is_some_and(|o| o.iter().any(is_null_schema))
    }

    fn is_covered(&self, node: &Value, i: usize) -> bool {
        self.covered
            .borrow()
            .contains(&(node as *const Value as usize, i))
    }

    /// Options reachable from `s` that no sample has used yet.
    fn uncovered(&self, s: &'a Value, seen: &mut BTreeSet<usize>) -> usize {
        let s = self.deref(s);
        if !seen.insert(s as *const Value as usize) {
            return 0;
        }
        let mut n = 0;
        if let Some(opts) = self.options(s) {
            for (i, o) in opts.iter().enumerate() {
                n += usize::from(!self.is_covered(s, i));
                if let Opt::Schema(o) = o {
                    n += self.uncovered(o, seen);
                }
            }
            return n;
        }
        for key in [
            "properties",
            "additionalProperties",
            "items",
            "prefixItems",
            "allOf",
        ] {
            match s.get(key) {
                Some(Value::Object(m)) if key == "properties" => {
                    for v in m.values() {
                        n += self.uncovered(v, seen);
                    }
                }
                Some(v @ Value::Object(_)) => n += self.uncovered(v, seen),
                Some(Value::Array(a)) => {
                    for v in a {
                        n += self.uncovered(v, seen);
                    }
                }
                _ => {}
            }
        }
        n
    }

    /// Full mode: the option covering the most unsampled choices.
    fn choose(&self, node: &'a Value, opts: &[Opt<'a>], mode: Mode) -> usize {
        if mode != Mode::Full {
            return 0;
        }
        // A recursive option (a layout node holding layout nodes) must not
        // score through `node` itself, or it would always win.
        let score = |i: usize| {
            let own = usize::from(!self.is_covered(node, i)) * 1_000_000;
            let mut seen = BTreeSet::from([node as *const Value as usize]);
            match opts[i] {
                Opt::Schema(o) => own + self.uncovered(o, &mut seen),
                Opt::Literal(_) => own,
            }
        };
        let best = (0..opts.len())
            .max_by_key(|&i| (score(i), std::cmp::Reverse(i)))
            .unwrap();
        self.covered
            .borrow_mut()
            .insert((node as *const Value as usize, best));
        best
    }

    fn value(&self, s: &'a Value, mode: Mode, depth: usize) -> Value {
        assert!(depth < 64, "schema nests too deep: {s}");
        let mode = if depth >= GROW_DEPTH { Mode::Min } else { mode };
        let s = self.deref(s);
        let o = match s {
            Value::Bool(true) => return any_value(mode),
            Value::Object(o) if o.is_empty() => return any_value(mode),
            Value::Object(o) => o,
            other => panic!("unexpected schema {other}"),
        };
        if let Some(c) = o.get("const") {
            return c.clone();
        }
        if let Some(opts) = self.options(s) {
            return match opts[self.choose(s, &opts, mode)] {
                Opt::Literal(v) => v.clone(),
                Opt::Schema(o) => self.value(o, mode, depth + 1),
            };
        }
        if let Some(all) = o.get("allOf").and_then(Value::as_array) {
            let mut merged = Map::new();
            for part in all {
                match self.value(part, mode, depth + 1) {
                    Value::Object(m) => merged.extend(m),
                    other => return other,
                }
            }
            if o.contains_key("properties")
                && let Value::Object(m) = self.object(o, mode, depth)
            {
                merged.extend(m);
            }
            return Value::Object(merged);
        }
        let ty = match o.get("type") {
            Some(Value::String(t)) => t.as_str(),
            Some(Value::Array(ts)) => ts
                .iter()
                .filter_map(Value::as_str)
                .find(|t| *t != "null")
                .unwrap_or("null"),
            _ if o.contains_key("properties") => "object",
            _ => return any_value(mode),
        };
        match ty {
            "object" => self.object(o, mode, depth),
            "array" => self.array(o, mode, depth),
            "string" => json!("x"),
            "integer" => {
                let lo = o.get("minimum").and_then(Value::as_i64).unwrap_or(i64::MIN);
                let hi = o.get("maximum").and_then(Value::as_i64).unwrap_or(i64::MAX);
                json!(7i64.clamp(lo, hi))
            }
            "number" => json!(1.5),
            "boolean" => json!(mode == Mode::Full),
            "null" => Value::Null,
            other => panic!("unknown schema type {other}"),
        }
    }

    fn object(&self, o: &'a Map<String, Value>, mode: Mode, depth: usize) -> Value {
        let required: BTreeSet<&str> = o
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut m = Map::new();
        if let Some(props) = o.get("properties").and_then(Value::as_object) {
            for (name, ps) in props {
                let req = required.contains(name.as_str());
                let v = match mode {
                    Mode::Full => Some(self.value(ps, mode, depth + 1)),
                    Mode::Min => req.then(|| self.value(ps, mode, depth + 1)),
                    Mode::Null if self.nullable(ps) => Some(Value::Null),
                    Mode::Null => req.then(|| self.value(ps, mode, depth + 1)),
                };
                if let Some(v) = v {
                    m.insert(name.clone(), v);
                }
            }
        }
        if let (Mode::Full, Some(ap @ Value::Object(_))) = (mode, o.get("additionalProperties")) {
            m.insert("key".into(), self.value(ap, mode, depth + 1));
        }
        Value::Object(m)
    }

    fn array(&self, o: &'a Map<String, Value>, mode: Mode, depth: usize) -> Value {
        if let Some(prefix) = o.get("prefixItems").and_then(Value::as_array) {
            return prefix
                .iter()
                .map(|p| self.value(p, mode, depth + 1))
                .collect();
        }
        let Some(items) = o.get("items") else {
            return json!([]);
        };
        let min = o.get("minItems").and_then(Value::as_u64).unwrap_or(0) as usize;
        let n = match mode {
            Mode::Full => min.max(1),
            Mode::Min | Mode::Null => min,
        };
        (0..n).map(|_| self.value(items, mode, depth + 1)).collect()
    }
}

fn is_null_schema(s: &Value) -> bool {
    s.get("type").and_then(Value::as_str) == Some("null")
}

/// A free-form `serde_json::Value` field.
fn any_value(mode: Mode) -> Value {
    match mode {
        Mode::Full => json!({ "any": [1, "two", true] }),
        Mode::Min | Mode::Null => json!({}),
    }
}

/// Pretty JSON down to `levels` nesting levels, one compact line below:
/// small files whose diffs still show which entry changed.
fn layout(v: &Value, levels: usize, indent: usize, out: &mut String) {
    let pad = |n: usize| "  ".repeat(n);
    match v {
        Value::Object(m) if levels > 0 && !m.is_empty() => {
            out.push_str("{\n");
            for (i, (k, x)) in m.iter().enumerate() {
                out.push_str(&pad(indent + 1));
                out.push_str(&serde_json::to_string(k).unwrap());
                out.push_str(": ");
                layout(x, levels - 1, indent + 1, out);
                out.push_str(if i + 1 < m.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(indent));
            out.push('}');
        }
        Value::Array(a) if levels > 0 && !a.is_empty() => {
            out.push_str("[\n");
            for (i, x) in a.iter().enumerate() {
                out.push_str(&pad(indent + 1));
                layout(x, levels - 1, indent + 1, out);
                out.push_str(if i + 1 < a.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(indent));
            out.push(']');
        }
        other => out.push_str(&serde_json::to_string(other).unwrap()),
    }
}
