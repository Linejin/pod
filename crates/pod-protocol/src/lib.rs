//! Pod wire protocol: newline-delimited JSON-RPC 2.0 between `podd` and its
//! clients (docs/20-refactor/daemon.md §2).
//!
//! It owns the envelope, [`RpcError`](protocol::RpcError) and the error
//! codes, every [`Method`](protocol::Method), the params/result/event types,
//! their serde rules, the schema and the pure wire operations (base64,
//! version negotiation, the downgrade for older clients). Nothing here opens
//! a file, takes a lock or needs tokio or the daemon.
//!
//! The modules keep the names they have under `podd` (`protocol` and the
//! nine `*_protocol` domains, all re-exported from [`protocol`]), and the
//! wire types of pod-orch, pod-forge, pod-mods (without its `runtime`) and
//! pod-discover stay re-exported where they were.

/// One method marker: a unit struct whose [`Method`](protocol::Method) impl
/// ties the method name to its params and result types.
macro_rules! method {
    ($ty:ident, $name:literal, $params:ty, $result:ty) => {
        pub struct $ty;
        impl $crate::protocol::Method for $ty {
            const NAME: &'static str = $name;
            type Params = $params;
            type Result = $result;
        }
    };
}

pub mod discovery_protocol;
pub mod mobile_protocol;
pub mod mods_protocol;
pub mod orch_protocol;
pub mod orch_run_protocol;
pub mod pr_protocol;
pub mod protocol;
pub mod remote_protocol;
pub mod session_protocol;
pub mod status_protocol;

/// The wire golden that card D-F0 took from master with the `podd` types
/// (`crates/podd/tests/fixtures`, checked there by `tests/protocol_wire.rs`),
/// checked here against the types of this crate, which `podd` now re-exports
/// under its old paths (card D-F2). The holder and hook
/// versions in the golden belong to `podd` and `pod-status`, not here.
#[cfg(test)]
mod wire_golden {
    use std::collections::BTreeSet;

    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Map, Value, json};

    use crate::protocol::*;

    const WIRE: &str = include_str!("../../podd/tests/fixtures/wire-v20.json");
    const SCHEMA: &str = include_str!("../../podd/tests/fixtures/schema-v20.json");
    /// The newest protocol without the v19 `unreachable` terminal state.
    const DOWNGRADE_TO: u32 = 18;
    /// The newest protocol without the v20 attach chunk fields.
    const DOWNGRADE_TO_V19: u32 = 19;
    /// Golden constants owned by other crates (holder binary, hook relay).
    const NOT_OURS: [&str; 3] = [
        "HOLDER_PROTOCOL",
        "HOLDER_PROTOCOL_MIN",
        "HOOK_MESSAGE_VERSION",
    ];

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
    /// names, error codes and capability names; without [`NOT_OURS`].
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

    fn load(text: &str) -> Value {
        serde_json::from_str(text).expect("golden JSON")
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

    fn names(v: &Value) -> BTreeSet<&str> {
        v.as_object().unwrap().keys().map(String::as_str).collect()
    }

    /// `schema()` is the master schema, key order aside.
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
        assert_eq!(now["methods"].as_object().unwrap().len(), 105);
        assert_eq!(now["notifications"].as_object().unwrap().len(), 19);
    }

    /// The same method and notification names, and every golden sample of
    /// their types survives the types of this crate unchanged.
    #[test]
    fn every_method_and_notification_roundtrips_its_golden_samples() {
        let golden = load(WIRE);
        let s = schema();
        let mut d = Diffs::default();

        let methods = method_types();
        let table: BTreeSet<&str> = methods.iter().map(|m| m.name).collect();
        assert_eq!(methods.len(), 105, "method table");
        assert_eq!(table, names(&golden["methods"]), "methods: table vs golden");
        assert_eq!(table, names(&s["methods"]), "methods: table vs schema()");
        for m in &methods {
            let types = &golden["methods"][m.name];
            for (role, rt) in [("params", m.params), ("result", m.result)] {
                let key = types[role].as_str().unwrap();
                check_samples(&mut d, &golden, &format!("{} {role}", m.name), key, rt);
            }
        }

        let events = notification_types();
        let table: BTreeSet<&str> = events.iter().map(|(n, _)| *n).collect();
        assert_eq!(events.len(), 19, "notification table");
        assert_eq!(
            table,
            names(&golden["notifications"]),
            "events: table vs golden"
        );
        assert_eq!(
            table,
            names(&s["notifications"]),
            "events: table vs schema()"
        );
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
        let mut want = golden["constants"].as_object().unwrap().clone();
        for k in NOT_OURS {
            assert!(want.remove(k).is_some(), "golden constant {k}");
        }
        let now = constants();
        for (k, v) in &want {
            d.check(format!("constants.{k}"), &now[k], v);
        }
        d.check("constants (whole)", &now, &Value::Object(want));
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
            let got = match negotiate(&p, server_range(), CAPABILITIES) {
                Ok(v) => json!({ "ok": v }),
                Err(e) => json!({ "error": e }),
            };
            d.check(format!("negotiate[{i}]"), &got, &case["outcome"]);
        }
        d.assert_none();
    }

    /// Every result and event sample, rewritten for a v18 client.
    #[test]
    fn v18_downgrade_matches_master_golden() {
        let golden = load(WIRE);
        let mut outgoing: BTreeSet<String> = golden["methods"]
            .as_object()
            .unwrap()
            .values()
            .map(|t| t["result"].as_str().unwrap().to_string())
            .collect();
        outgoing.extend(
            golden["notifications"]
                .as_object()
                .unwrap()
                .values()
                .map(|k| k.as_str().unwrap().to_string()),
        );
        let mut now = Vec::new();
        for key in outgoing {
            for s in golden["types"][&key].as_array().unwrap() {
                let v = sample_out(s);
                let mut down = v.clone();
                downgrade_json(&mut down, DOWNGRADE_TO);
                if &down != v {
                    now.push(json!({ "type": key, "case": s["case"], "v18": down }));
                }
            }
        }
        let want = golden["downgrade_v18"].as_array().unwrap();
        assert!(!want.is_empty());
        let mut d = Diffs::default();
        d.check("downgrade_v18 count", &json!(now.len()), &json!(want.len()));
        for (i, (got, want)) in now.iter().zip(want).enumerate() {
            d.check(format!("downgrade_v18[{i}]"), got, want);
        }
        d.assert_none();
        // Older clients get the same rewrite.
        for case in want.iter().take(3) {
            let samples = golden["types"][case["type"].as_str().unwrap()]
                .as_array()
                .unwrap();
            let s = samples.iter().find(|s| s["case"] == case["case"]).unwrap();
            let mut v = sample_out(s).clone();
            downgrade_json(&mut v, PROTOCOL_MIN);
            assert_eq!(v, case["v18"]);
        }
    }

    /// Every result and event sample, rewritten for a v19 client (only the
    /// v20 attach chunk fields go).
    #[test]
    fn v19_downgrade_matches_golden() {
        let golden = load(WIRE);
        let mut outgoing: BTreeSet<String> = golden["methods"]
            .as_object()
            .unwrap()
            .values()
            .map(|t| t["result"].as_str().unwrap().to_string())
            .collect();
        outgoing.extend(
            golden["notifications"]
                .as_object()
                .unwrap()
                .values()
                .map(|k| k.as_str().unwrap().to_string()),
        );
        let mut now = Vec::new();
        for key in outgoing {
            for s in golden["types"][&key].as_array().unwrap() {
                let v = sample_out(s);
                let mut down = v.clone();
                downgrade_json(&mut down, DOWNGRADE_TO_V19);
                if &down != v {
                    now.push(json!({ "type": key, "case": s["case"], "v19": down }));
                }
            }
        }
        let want = golden["downgrade_v19"].as_array().unwrap();
        assert!(!want.is_empty());
        let mut d = Diffs::default();
        d.check("downgrade_v19 count", &json!(now.len()), &json!(want.len()));
        for (i, (got, want)) in now.iter().zip(want).enumerate() {
            d.check(format!("downgrade_v19[{i}]"), got, want);
        }
        d.assert_none();
    }
}
