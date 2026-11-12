//! Card D-F2 (docs/20-refactor/daemon.md §3): the wire modules live in
//! `pod-protocol` and `podd` re-exports them under their old paths. Each old
//! path (`podd::protocol` and the nine `podd::*_protocol`) must name the very
//! same items, not copies: a value read through one path goes into code
//! written for the other with no conversion, and a method marker from either
//! path meets the `Method` bound of both.
//!
//! The compiler does most of the checking; the tests run the calls on the
//! samples of the D-F0 wire golden so the values are real ones.

use std::path::PathBuf;

use serde_json::Value;

/// Implemented only when `Self` and `T` are one type.
trait Same<T: ?Sized> {}
impl<T: ?Sized> Same<T> for T {}

/// Compiles only when `A` and `B` are one type.
fn same<A: ?Sized + Same<B>, B: ?Sized>() {}

/// The `Method` bound as the new crate spells it.
fn name_via_new<M: pod_protocol::protocol::Method>() -> &'static str {
    M::NAME
}

/// The `Method` bound as the old path spells it.
fn name_via_old<M: podd::protocol::Method>() -> &'static str {
    M::NAME
}

fn golden() -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wire-v21.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_json::from_str(&text).unwrap()
}

/// The golden samples of one type: (input, expected output).
fn samples(golden: &Value, ty: &str) -> Vec<(Value, Value)> {
    let list = golden["types"][ty]
        .as_array()
        .unwrap_or_else(|| panic!("no golden samples for {ty}"));
    assert!(!list.is_empty(), "no golden samples for {ty}");
    list.iter()
        .map(|s| {
            let out = s.get("out").unwrap_or(&s["in"]);
            (s["in"].clone(), out.clone())
        })
        .collect()
}

/// One test per wire module: a method marker and its params type, both
/// defined in that module.
macro_rules! same_module {
    ($test:ident, $module:ident, $marker:ident, $method:literal, $params:ident) => {
        #[test]
        fn $test() {
            // The old and the new path name the same marker and type.
            same::<podd::$module::$marker, pod_protocol::$module::$marker>();
            same::<podd::$module::$params, pod_protocol::$module::$params>();
            same::<
                <podd::$module::$marker as pod_protocol::protocol::Method>::Params,
                pod_protocol::$module::$params,
            >();

            // Each path's `Method` bound takes the other path's marker.
            assert_eq!(name_via_new::<podd::$module::$marker>(), $method);
            assert_eq!(name_via_old::<pod_protocol::$module::$marker>(), $method);

            // Written against the new crate only.
            fn new_side(p: pod_protocol::$module::$params) -> pod_protocol::$module::$params {
                p
            }
            let golden = golden();
            for (input, want) in samples(&golden, stringify!($params)) {
                let old: podd::$module::$params = serde_json::from_value(input.clone())
                    .unwrap_or_else(|e| panic!("{input}: {e}"));
                // Old value in, old-typed binding out, nothing converted.
                let back: podd::$module::$params = new_side(old);
                assert_eq!(serde_json::to_value(&back).unwrap(), want, "{input}");
            }
        }
    };
}

same_module!(
    protocol,
    protocol,
    TerminalResize,
    "terminal.resize",
    ResizeParams
);
same_module!(
    discovery_protocol,
    discovery_protocol,
    SessionsListMethod,
    "sessions.list",
    SessionsListParams
);
same_module!(
    mobile_protocol,
    mobile_protocol,
    MobileRevoke,
    "mobile.revoke",
    MobileRevokeParams
);
same_module!(
    mods_protocol,
    mods_protocol,
    ModSetEnabled,
    "mod.set_enabled",
    ModEnableParams
);
same_module!(
    orch_protocol,
    orch_protocol,
    ReviewCommentsAdd,
    "review.comments.add",
    ReviewAddParams
);
same_module!(
    orch_run_protocol,
    orch_run_protocol,
    OrchRunCreate,
    "orch.run.create",
    OrchRunCreateParams
);
same_module!(
    pr_protocol,
    pr_protocol,
    TaskPrStatusMethod,
    "task.pr.status",
    TaskPrStatusParams
);
same_module!(
    remote_protocol,
    remote_protocol,
    ConnectionHello,
    "connection.hello",
    HelloParams
);
same_module!(
    session_protocol,
    session_protocol,
    TaskArchive,
    "task.archive",
    TaskArchiveParams
);
same_module!(
    status_protocol,
    status_protocol,
    StatusWait,
    "status.wait",
    StatusWaitParams
);

/// The shared items and the re-exports that consumers reach through
/// `podd::protocol::*` keep pointing at the same definitions, and the old
/// client takes markers and values from either path.
#[test]
fn shared_items_and_reexports_are_the_same() {
    same::<podd::protocol::RpcError, pod_protocol::protocol::RpcError>();
    same::<podd::protocol::Request, pod_protocol::protocol::Request>();
    same::<podd::protocol::TaskInfo, pod_protocol::protocol::TaskInfo>();
    // Domain modules glob-re-exported from `protocol`.
    same::<podd::protocol::OrchRunCreateParams, pod_protocol::orch_run_protocol::OrchRunCreateParams>(
    );
    same::<podd::protocol::HelloParams, pod_protocol::remote_protocol::HelloParams>();
    same::<podd::protocol::StatusInfo, pod_protocol::status_protocol::StatusInfo>();
    // Domain crate types re-exported by the wire modules.
    same::<podd::orch_run_protocol::MsgText, pod_orch::text::MsgText>();
    same::<podd::protocol::DiscoveredAgent, pod_discover::Agent>();

    // v20 (docs/17 R4): chunked attach and `terminal.input_once`.
    // v21 (docs/19 §20): the worker endpoint's hello and `server.status`
    // `endpoint`.
    assert_eq!(podd::protocol::PROTOCOL_VERSION, 21);
    same::<podd::protocol::WorkerAuth, pod_protocol::remote_protocol::WorkerAuth>();
    same::<podd::protocol::Endpoint, pod_protocol::protocol::Endpoint>();
    same::<podd::protocol::InputOnceParams, pod_protocol::remote_protocol::InputOnceParams>();
    same::<podd::protocol::SnapshotChunkEvent, pod_protocol::remote_protocol::SnapshotChunkEvent>();
    same::<podd::protocol::TerminalInputOnce, pod_protocol::protocol::TerminalInputOnce>();
    assert_eq!(podd::protocol::PROTOCOL_MIN, 11);
    assert_eq!(
        podd::protocol::PROTOCOL_VERSION,
        pod_protocol::protocol::PROTOCOL_VERSION
    );

    // `podd::client::Client::call` with a new-path marker, typed with the
    // old-path params and result.
    let _: fn(
        &mut podd::client::Client,
        &podd::protocol::ResizeParams,
    ) -> Result<podd::protocol::TerminalInfo, podd::client::ClientError> =
        podd::client::Client::call::<pod_protocol::protocol::TerminalResize>;
}
