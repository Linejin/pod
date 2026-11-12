//! Wire protocol: newline-delimited JSON-RPC 2.0 over a per-user named pipe.
//!
//! Every request is one JSON object on one line; every response is one line.
//! Each method is described by a [`Method`] impl that ties its name to its
//! params and result types, so the server, the client and the JSON schema
//! (`podd schema`) all come from the same definitions.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use crate::discovery_protocol::*;
pub use crate::files_protocol::*;
pub use crate::mobile_protocol::*;
pub use crate::mods_protocol::*;
pub use crate::orch_protocol::*;
pub use crate::orch_run_protocol::*;
pub use crate::pr_protocol::*;
pub use crate::remote_protocol::*;
pub use crate::session_protocol::*;
pub use crate::status_protocol::*;

pub const JSONRPC_VERSION: &str = "2.0";
/// v5 = v4 + review comments, groups and gates (docs/11).
/// v6 = v5 + background `group.check` jobs (`group.check_cancel`, `group.check`
/// events), gate expiry, `terminal.keep` and exited-terminal retention
/// (`ListResult.retention`).
/// v7 = v6 + terminal parents (`RunParams.parent_terminal`,
/// `TerminalInfo.parent_terminal/parent_task`), `TerminalInfo.incompatible_holder`,
/// the `terminal.lagged` attach notification and attach query-reply
/// de-duplication, review queue expiry (all additive).
/// v8 = v7 + submit confirmation (docs/12 B1): `SendParams.confirm`,
/// `SendResult.submit`, `ReviewDeliveriesResult.submits`,
/// `ReviewDeliveryEvent.submit` (all additive).
/// v9 = v8 + lifecycle events on `status.subscribe` with `lifecycle: true`
/// (`terminal.changed`, `task.changed`, `task.removing`), `task.touch`,
/// color-query reply de-duplication and the late-reply guard on
/// `terminal.input` (`SendResult.dropped_replies`) (all additive).
/// v10 = v9 + task pull requests (docs/15): `task.pr.status`,
/// `task.pr.create`, `task.pr.merge`, `TaskInfo.pr`, the `task.pr.changed`
/// lifecycle notification and the `FORGE_FAILED` error code (all additive).
/// v11 = v10 + extensions (docs/16): `mod.list/set_enabled/grant/log/command/reload`,
/// `mod.notify` / `mod.changed` events, status source `"mod"` (all additive).
/// v12 = v11 + `connection.hello` (docs/17 R2): protocol range, capabilities,
/// persistent `machine_id`, per-start `boot_id`, frame limits, and the
/// `PROTOCOL_MISMATCH` error code (all additive; v11 clients are served as
/// before, see [`PROTOCOL_MIN`]).
/// v13 = v12 + mobile approval M1 (docs/17): `mobile.devices/revoke/policy/
/// audit/settings`, mobile error codes -32060..-32064 (all additive; no
/// network listener).
/// v14 = v13 + worktree sessions (docs/19 S1–S3, `session_protocol.rs`):
/// `task.create {name: "" = suggested, branch, brief, brief_line, setup,
/// setup_mode, open_hint, env}`, `TaskInfo.{lifecycle, agent_terminal, setup,
/// brief, agent_session, branch_existed}`, `task.archive/unarchive/resume/
/// cleanup_candidates/suggest_name`, `repo.config`, `repo.set`, `task.changed`
/// `updated` + `open_hint`, `TaskRemoveResult.branch_kept`, error codes
/// -32070..-32072 (all additive).
/// v15 = v14 + session discovery (docs/18): `sessions.list/scan/settings/
/// register/resume/hide`, the `sessions.changed` lifecycle event and error
/// codes -32040..-32042 (all additive).
/// v16 = v15 + orchestration (docs/19 S4–S5): `orch.run/item/worker/mail.*`,
/// `orch.done`, the `orch.changed` / `orch.mail` events and error codes
/// -32080..-32088 (all additive).
/// v17 = v16 + the worker resource queue (`orch.queue`, docs/19 S6) and
/// decision gates with options (`orch.gate.request/decide`, `options` /
/// `choice` / `run` / `item` on gates and mobile gates; all additive).
/// v18 = v17 + resource limits as daemon settings (`orch.limits.get/set`,
/// docs/19 §14.4; additive).
/// v19 = v18 + `TerminalState::Unreachable` and `TerminalInfo.unreachable`:
/// holders a restarted daemon could not re-attach are listed instead of
/// hidden (daemon-03). Clients that said `connection.hello` with an older
/// protocol get them as `exited` + `lost` (see [`TerminalInfo::for_protocol`]).
/// v20 = v19 + remote attach resilience (docs/17 R4, `remote_protocol.rs`):
/// on a connection whose `connection.hello` chose 20 or newer, the
/// `terminal.attach` snapshot comes as `terminal.snapshot` chunks
/// (`AttachResult.snapshot_chunks`/`snapshot_bytes`/`scrollback_omitted`),
/// output notifications carry at most 32 KiB and the attach queue is 2 MiB;
/// `terminal.input_once` (input applied at most once per `(stream, seq)`)
/// and the `attach.chunks` capability. Nothing changes for connections
/// without hello or with an older one (see [`downgrade_json`]).
/// v21 = v20 + the restricted worker endpoint (docs/19 §20, docs/22
/// TM-20): `connection.hello` `worker` (attempt + token), `server.status`
/// `endpoint`, `orch.gate.decide` `person` and [`worker_codes`]
/// -32115..-32118. An older hello does not get `endpoint`.
/// v22 = v21 + remote files and review diff (docs/17 R5,
/// `files_protocol.rs`): `fs.stat/list/read/write`, `review.diff`,
/// `review.blob`, [`files_codes`] -32120..-32127, the `remote.files` and
/// `remote.diff` capabilities and `FrameLimits.max_read_bytes` /
/// `max_write_bytes` (all additive; an older hello gets neither the
/// capabilities nor the limits).
pub const PROTOCOL_VERSION: u32 = 22;

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    /// Absent for notifications (no response is sent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

/// A JSON-RPC notification line (no `id`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (code {})", self.message, self.code)
    }
}

impl std::error::Error for RpcError {}

/// Standard JSON-RPC codes plus podd application codes (-32000..-32099).
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// No terminal matches the given id / name.
    pub const TERMINAL_NOT_FOUND: i64 = -32001;
    /// A live terminal already uses the requested name.
    pub const NAME_TAKEN: i64 = -32002;
    /// The holder, the PTY or the child process could not be started.
    pub const SPAWN_FAILED: i64 = -32003;
    /// The operation needs a running terminal but it has exited.
    pub const TERMINAL_EXITED: i64 = -32004;
    /// The operation needs an exited terminal but it is still running.
    pub const TERMINAL_RUNNING: i64 = -32005;
    /// `repo.add`: the path is not the root of a (non-bare) git work tree.
    pub const NOT_A_REPO: i64 = -32006;
    /// No registered repository matches the given name / path.
    pub const REPO_NOT_FOUND: i64 = -32007;
    /// The repository (path or name) is already registered.
    pub const REPO_EXISTS: i64 = -32008;
    /// `repo.remove`: the repository still has tasks.
    pub const REPO_IN_USE: i64 = -32009;
    /// No task matches the given reference (or a bare name is ambiguous).
    pub const TASK_NOT_FOUND: i64 = -32010;
    /// `task.create`: the task, its branch or its worktree directory already exists.
    pub const TASK_EXISTS: i64 = -32011;
    /// `task.remove`: uncommitted changes or unpushed commits (use `force`).
    /// `data` = `{dirty, unpushed}`.
    pub const TASK_DIRTY: i64 = -32012;
    /// A git command failed; the message carries git's stderr.
    pub const GIT_FAILED: i64 = -32013;
    /// The daemon got `server.stop` and no longer takes requests. Nothing was
    /// done; retry once it has exited (the CLI and the app do this for you).
    pub const SERVER_STOPPING: i64 = -32050;
    /// Input refused: the program has not read the input already sent
    /// (4 MiB in flight). Nothing of this request was typed; retry later.
    pub const INPUT_BACKLOG: i64 = -32052;
}

// ---------------------------------------------------------------------------
// Method trait
// ---------------------------------------------------------------------------

pub trait Method {
    const NAME: &'static str;
    type Params: Serialize + DeserializeOwned + JsonSchema;
    type Result: Serialize + DeserializeOwned + JsonSchema;
}

method!(ServerStatus, "server.status", Empty, ServerStatusResult);
method!(ServerStop, "server.stop", StopParams, ServerStopResult);
method!(TerminalRun, "terminal.run", RunParams, TerminalInfo);
method!(TerminalList, "terminal.list", Empty, ListResult);
method!(TerminalSend, "terminal.send", SendParams, SendResult);
method!(TerminalKeys, "terminal.keys", KeysParams, SendResult);
method!(TerminalInput, "terminal.input", InputParams, SendResult);
method!(TerminalRead, "terminal.read", ReadParams, ReadResult);
method!(TerminalWait, "terminal.wait", WaitParams, WaitResult);
method!(
    TerminalResize,
    "terminal.resize",
    ResizeParams,
    TerminalInfo
);
method!(TerminalKill, "terminal.kill", TerminalRef, TerminalInfo);
method!(TerminalRm, "terminal.rm", RmParams, RmResult);
method!(TerminalKeep, "terminal.keep", KeepParams, Retention);
method!(
    TerminalAttach,
    "terminal.attach",
    AttachParams,
    AttachResult
);
method!(RepoAdd, "repo.add", RepoAddParams, RepoInfo);
method!(RepoList, "repo.list", Empty, RepoListResult);
method!(RepoRemove, "repo.remove", RepoRef, RepoRemoveResult);
method!(TaskCreate, "task.create", TaskCreateParams, TaskInfo);
method!(TaskList, "task.list", TaskListParams, TaskListResult);
method!(TaskShow, "task.show", TaskRef, TaskInfo);
method!(TaskDiff, "task.diff", TaskRef, TaskDiffResult);
method!(
    TaskRemove,
    "task.remove",
    TaskRemoveParams,
    TaskRemoveResult
);
method!(TaskTouch, "task.touch", TaskTouchParams, TaskTouchResult);

/// Server -> client notifications on an attached connection.
pub const NOTIFY_OUTPUT: &str = "terminal.output";
pub const NOTIFY_RESIZED: &str = "terminal.resized";
pub const NOTIFY_EXITED: &str = "terminal.exited";
/// v7: the attach client fell too far behind (its queue passed the daemon's
/// limit). Last notification; the daemon closes the connection. Re-attach to
/// continue from a fresh snapshot.
pub const NOTIFY_LAGGED: &str = "terminal.lagged";

/// All method names, in documentation order.
pub const METHODS: &[&str] = &[
    ServerStatus::NAME,
    ServerStop::NAME,
    TerminalRun::NAME,
    TerminalList::NAME,
    TerminalSend::NAME,
    TerminalKeys::NAME,
    TerminalInput::NAME,
    TerminalRead::NAME,
    TerminalWait::NAME,
    TerminalResize::NAME,
    TerminalKill::NAME,
    TerminalRm::NAME,
    TerminalKeep::NAME,
    TerminalAttach::NAME,
    RepoAdd::NAME,
    RepoList::NAME,
    RepoRemove::NAME,
    TaskCreate::NAME,
    TaskList::NAME,
    TaskShow::NAME,
    TaskDiff::NAME,
    TaskRemove::NAME,
    TaskTouch::NAME,
];

// ---------------------------------------------------------------------------
// Params / results
// ---------------------------------------------------------------------------

/// Params for methods that take none. Accepts `{}`, `null` or an absent field.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Empty {}

/// Addresses a terminal by `id` (e.g. `"t3"`) or by `name`. Exactly one is required.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TerminalRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl TerminalRef {
    pub fn id(id: impl Into<String>) -> Self {
        Self {
            id: Some(id.into()),
            name: None,
        }
    }
    pub fn name(name: impl Into<String>) -> Self {
        Self {
            id: None,
            name: Some(name.into()),
        }
    }
    /// CLI convenience: strings shaped like an id (`t<digits>`) are ids, anything else is a name.
    pub fn parse(s: &str) -> Self {
        if is_terminal_id(s) {
            Self::id(s)
        } else {
            Self::name(s)
        }
    }
}

impl std::fmt::Display for TerminalRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (&self.id, &self.name) {
            (Some(id), _) => f.write_str(id),
            (None, Some(n)) => f.write_str(n),
            (None, None) => f.write_str("<none>"),
        }
    }
}

/// Terminal ids are `t` followed by decimal digits. Names may not look like ids.
pub fn is_terminal_id(s: &str) -> bool {
    s.len() > 1 && s.starts_with('t') && s[1..].bytes().all(|b| b.is_ascii_digit())
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ServerStatusResult {
    pub version: String,
    pub protocol: u32,
    pub pid: u32,
    /// Full pipe path, e.g. `\\.\pipe\podd-alice`.
    pub pipe: String,
    pub started_ms: u64,
    pub uptime_ms: u64,
    pub terminals: usize,
    pub running: usize,
    /// Directory holding this daemon's holder records.
    pub state_dir: String,
    /// v21 (docs/19 §20, docs/22 TM-20): the endpoint this connection
    /// reached. Absent from older daemons and for older hellos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<Endpoint>,
}

/// Which daemon endpoint a connection reached (v21, docs/19 §20).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    /// The full pipe: every method, for any process of this OS user. A
    /// process that can open it is **not** isolated by attempt tokens or
    /// caller fields.
    Full,
    /// The restricted worker pipe: one Pod worker (its attempt token, said
    /// in `connection.hello`), its own run and terminal, a method allowlist.
    Worker,
}

/// The first protocol with the worker endpoint (`connection.hello`
/// `worker`, `server.status` `endpoint`, `orch.gate.decide` `person`,
/// [`worker_codes`]).
pub const WORKER_ENDPOINT_PROTOCOL: u32 = 21;

/// Error codes of the restricted worker endpoint (v21, docs/19 §20). The
/// message of each is fixed; `data.method` names the refused method.
pub mod worker_codes {
    /// The method is not on the worker endpoint's allowlist (gate
    /// decisions, merge, input, limits, config, other terminals...).
    pub const NOT_ALLOWED: i64 = -32115;
    /// No worker identity: the connection did not say `connection.hello`
    /// with this worker's attempt and token, or the token is not the
    /// attempt's.
    pub const NO_WORKER: i64 = -32116;
    /// The target belongs to another run, attempt, item, mail or terminal.
    pub const OUT_OF_SCOPE: i64 = -32117;
    /// `caller` names another attempt or terminal than the connection's
    /// worker (a caller field never raises authority there).
    pub const CALLER_MISMATCH: i64 = -32118;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StopParams {
    /// Also kill every terminal and end its holder. By default terminals keep
    /// running in their holders and the next daemon re-attaches to them.
    #[serde(default)]
    pub kill: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ServerStopResult {
    pub stopping: bool,
    /// Running terminals killed because of `kill: true`.
    pub killed: usize,
    /// Running terminals left alive in their holders.
    pub detached: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RunParams {
    /// Program and arguments. `argv[0]` is resolved through `PATH`/`PATHEXT`
    /// (`.cmd`/`.bat` shims run through `cmd.exe /d /s /c`).
    pub argv: Vec<String>,
    /// Working directory (absolute). Defaults to the user's home directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Optional name, unique among running terminals; must not look like an id (`t<digits>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Environment overrides, applied last (may set anything). The base is the
    /// user's registry environment with launcher markers removed (see `env.rs`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// Grid size; defaults to 120x30.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    /// Add the terminal to this task (id `<repo>/<name>` or unique name):
    /// `cwd` defaults to the task's worktree and `POD_TASK_ID` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    /// v7: the caller's `POD_TERMINAL_ID` when it runs inside a pod terminal of
    /// this daemon (`pod run` sends it). Recorded as the new terminal's parent,
    /// together with the task that terminal belongs to. Unknown ids are ignored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_terminal: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerminalState {
    #[default]
    Running,
    Exited,
    /// v19: a holder record found at daemon start whose holder could not be
    /// reached (busy, timed out, broken handshake). The daemon retries in the
    /// background; `terminal.kill`/`terminal.rm` forget it. The reason is in
    /// [`TerminalInfo::unreachable`]. Nothing can be sent to it.
    Unreachable,
}

/// The first protocol that knows [`TerminalState::Unreachable`].
pub const UNREACHABLE_PROTOCOL: u32 = 19;

/// Fields added after v6 are optional with `#[serde(default)]`; Rust code that
/// builds this struct should use [`TerminalInfo::new`] or `..Default::default()`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TerminalInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub argv: Vec<String>,
    pub cwd: String,
    pub state: TerminalState,
    /// Exit code of the root process, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    /// Root process id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Holder process id (owns the ConPTY and the Job Object).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holder_pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    /// Unix epoch milliseconds.
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exited_ms: Option<u64>,
    /// True when the terminal ended because of `terminal.kill` / `server.stop {kill}`.
    #[serde(default)]
    pub killed: bool,
    /// True when the holder vanished without reporting an exit (crash).
    #[serde(default)]
    pub lost: bool,
    /// Clients currently attached with `terminal.attach`.
    #[serde(default)]
    pub attached: usize,
    /// ConPTY implementation hosting the terminal: `bundled:<conpty.dll path>` or `inbox`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conpty: Option<String>,
    /// Agent status (P1 status engine, see `status_protocol.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusInfo>,
    /// v7: the pod terminal that started this one (`pod run` from inside a pod
    /// terminal, or the caller of `task.create`/`group.create`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_terminal: Option<String>,
    /// v7: the task the parent terminal belonged to at start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task: Option<String>,
    /// v7: set when the terminal's holder speaks a holder protocol this daemon
    /// does not support (e.g. a holder left running by an older/newer podd).
    /// The terminal is then reported as exited (`state: exited`) and cannot be
    /// driven; `terminal.rm --force` terminates the holder (and its process tree).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incompatible_holder: Option<String>,
    /// v19: why the holder of an `unreachable` terminal could not be reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreachable: Option<String>,
}

/// Rewrite one outgoing JSON message (response or notification, any
/// method) for a client that chose `protocol` in `connection.hello`: every
/// v19 `unreachable` terminal state anywhere in it becomes what that client
/// can read. Objects shaped like [`TerminalInfo`] go through
/// [`TerminalInfo::for_protocol`]; any other `state: "unreachable"` (e.g.
/// `terminal.read`) becomes `exited`. Before v20 an [`AttachResult`] loses
/// its chunk fields (the daemon never chunks for such a connection; this is
/// the safety net). Before v21 a [`ServerStatusResult`] loses `endpoint`.
/// Before v22 a hello's [`FrameLimits`] loses `max_read_bytes` and
/// `max_write_bytes`. No-op for the current version.
pub fn downgrade_json(v: &mut Value, protocol: u32) {
    if protocol >= PROTOCOL_VERSION {
        return;
    }
    match v {
        Value::Array(items) => items.iter_mut().for_each(|x| downgrade_json(x, protocol)),
        Value::Object(map) => {
            if protocol < FILES_PROTOCOL
                && map.contains_key("max_line_bytes")
                && map.contains_key("attach_queue_bytes")
            {
                map.remove("max_read_bytes");
                map.remove("max_write_bytes");
            }
            if protocol < WORKER_ENDPOINT_PROTOCOL
                && map.contains_key("state_dir")
                && map.contains_key("uptime_ms")
                && map.contains_key("pipe")
            {
                map.remove("endpoint");
            }
            if protocol < CHUNKED_ATTACH_PROTOCOL
                && map.contains_key("snapshot")
                && map.contains_key("terminal")
                && map.contains_key("seq")
            {
                for k in ["snapshot_chunks", "snapshot_bytes", "scrollback_omitted"] {
                    map.remove(k);
                }
            }
            if protocol < UNREACHABLE_PROTOCOL
                && map.get("state").and_then(Value::as_str) == Some("unreachable")
            {
                if map.contains_key("argv")
                    && let Ok(info) =
                        serde_json::from_value::<TerminalInfo>(Value::Object(map.clone()))
                    && let Ok(Value::Object(down)) =
                        serde_json::to_value(info.for_protocol(protocol))
                {
                    *map = down;
                    return;
                }
                map.insert("state".into(), Value::from("exited"));
            }
            map.values_mut().for_each(|x| downgrade_json(x, protocol));
        }
        _ => {}
    }
}

impl TerminalInfo {
    /// This info as a client speaking `protocol` can read it: before v19
    /// there is no `unreachable` state, so such a terminal is reported as
    /// `exited` with `lost` (the closest state an old client will not try
    /// to drive; its `rm` still forgets it).
    pub fn for_protocol(mut self, protocol: u32) -> Self {
        if protocol < UNREACHABLE_PROTOCOL && self.state == TerminalState::Unreachable {
            self.state = TerminalState::Exited;
            self.lost = true;
        }
        self
    }

    /// The required fields; everything optional starts empty.
    pub fn new(
        id: impl Into<String>,
        argv: Vec<String>,
        cwd: impl Into<String>,
        state: TerminalState,
        cols: u16,
        rows: u16,
        created_ms: u64,
    ) -> Self {
        Self {
            id: id.into(),
            argv,
            cwd: cwd.into(),
            state,
            cols,
            rows,
            created_ms,
            ..Default::default()
        }
    }
}

/// `terminal.keep` (v6): exempt a terminal from (or return it to) the
/// exited-terminal retention policy. Stored in the state dir.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct KeepParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    /// Default true; false returns it to the policy.
    #[serde(default = "default_true")]
    pub keep: bool,
}

/// Retention facts of one terminal (v6). Kept out of [`TerminalInfo`] so
/// clients that build that struct literally keep compiling.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Retention {
    pub id: String,
    /// Exempt from the exited-terminal retention (`terminal.keep`).
    #[serde(default)]
    pub kept: bool,
    /// Unix ms when the daemon will remove this exited terminal; absent while
    /// it runs, when kept, when it belongs to a task, or when retention is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ListResult {
    pub terminals: Vec<TerminalInfo>,
    /// v6: exited-terminal retention in ms (absent = off).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_ms: Option<u64>,
    /// v6: retention facts for the terminals that are kept or will expire.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retention: Vec<Retention>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SendParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    /// Bytes to type, sent verbatim (UTF-8).
    pub text: String,
    /// Press Enter (`\r`) after the text. Sent as a separate write once the
    /// program has taken the text in (its output went quiet), so TUIs that
    /// detect pastes by timing (Codex on Windows) treat it as a key press.
    #[serde(default)]
    pub enter: bool,
    /// v8: with `enter`, check that an agent started a turn after Enter and
    /// press Enter once more if it did not (see [`SubmitCheck`]). Default on;
    /// terminals without an agent status are never retried.
    #[serde(default = "yes")]
    pub confirm: bool,
}

fn yes() -> bool {
    true
}

/// v8: whether text typed into an agent terminal and followed by Enter was
/// accepted as a new message (docs/12 B1).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubmitState {
    /// Enter not pressed yet, or pressed and waiting for the agent's status
    /// to react.
    Pending,
    /// The agent started (or finished) a turn after Enter.
    Confirmed,
    /// No reaction, even after a second Enter: the text may still sit in the
    /// agent's input box.
    Unconfirmed,
    /// Nothing reports an agent status here (a shell), or the agent was
    /// already busy: no confirmation possible, no retry.
    Unverifiable,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SubmitCheck {
    pub state: SubmitState,
    /// Enter presses sent: 0 (not yet), 1, or 2 when the first one was not
    /// confirmed.
    pub enters: u32,
    /// What confirmed it: `working`, `blocked`, or `turn` (a whole turn
    /// already ended).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<String>,
    /// Unix ms of the first Enter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entered_ms: Option<u64>,
    /// Unix ms when the state became final.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub settled_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SendResult {
    pub id: String,
    pub bytes: usize,
    /// v8: outcome of the Enter (only with `enter: true`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<SubmitCheck>,
    /// v9, `terminal.input` only: terminal query replies removed from the
    /// input (duplicates of a reply already given, or late/stray replies).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_replies: Option<DroppedReplies>,
}

/// v9: query replies `terminal.input` removed (docs/03 §4.1-5).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct DroppedReplies {
    /// Already answered by the holder or by another attached client.
    pub duplicate: usize,
    /// Arrived more than the reply window after the query, or no query outstanding.
    pub late: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct KeysParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    /// Key names such as `enter`, `ctrl+c`, `esc`, `up`, `f5`, `alt+x`, or single characters.
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct InputParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    /// Raw input bytes, base64 (standard alphabet, padded).
    pub data: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadMode {
    /// The visible grid.
    #[default]
    Screen,
    /// Scrollback followed by the visible grid.
    History,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReadParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    #[serde(default)]
    pub mode: ReadMode,
    /// Return only the last N lines (after trailing blank lines are dropped).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<usize>,
    /// Keep a grey prompt suggestion at the caret in the text (by default it
    /// is left out and returned as `suggestion`; docs/19 §18.3).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub raw: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReadResult {
    pub id: String,
    pub mode: ReadMode,
    /// Text lines with trailing spaces trimmed. Soft-wrapped rows are joined
    /// into one logical line.
    pub lines: Vec<String>,
    /// Cursor position on the visible grid (column, row), 0-based.
    pub cursor: (u16, u16),
    pub state: TerminalState,
    /// Grey text at the input caret that the program shows but nobody typed
    /// (Claude's suggested prompt, PSReadLine's prediction). Left out of
    /// `lines` unless `raw` (docs/19 §18.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WaitUntil {
    /// Until the root process exits.
    Exit,
    /// Until the terminal text (scrollback + screen) contains this string.
    /// Occurrences inside the echo of text sent with `terminal.send` are
    /// ignored, so `send "echo X"` followed by `wait X` waits for the output.
    OutputContains(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WaitParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    pub until: WaitUntil,
    /// Give up after this many milliseconds; absent means wait forever.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitOutcome {
    /// The condition was met.
    Matched,
    /// The timeout elapsed first.
    TimedOut,
    /// Waiting for output, but the terminal exited without producing it.
    Exited,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WaitResult {
    pub outcome: WaitOutcome,
    pub terminal: TerminalInfo,
    pub waited_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ResizeParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RmParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    /// Kill first if still running.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RmResult {
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AttachParams {
    #[serde(flatten)]
    pub target: TerminalRef,
    /// Scrollback lines to include in the snapshot (default 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollback: Option<usize>,
    /// Resize the terminal to this size before the snapshot is taken
    /// (size policy: the most recent resize wins).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
}

/// Reply to `terminal.attach`. After it the connection carries only
/// notifications (`terminal.output`, `terminal.resized`, `terminal.exited`);
/// the client detaches by closing it and sends input on another connection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AttachResult {
    pub terminal: TerminalInfo,
    /// Output offset the snapshot corresponds to: the first `terminal.output`
    /// notification starts exactly here.
    pub seq: u64,
    /// ANSI bytes (base64) that reproduce the screen and some scrollback on a
    /// terminal of `terminal.cols` x `terminal.rows`. Empty when the snapshot
    /// comes in chunks (`snapshot_chunks`).
    pub snapshot: String,
    /// v20, connections whose hello chose 20 or newer: the snapshot follows
    /// in exactly this many `terminal.snapshot` notifications, before any
    /// other notification of the stream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_chunks: Option<u32>,
    /// v20: total raw (decoded) bytes of those chunks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_bytes: Option<u64>,
    /// v20: scrollback lines left out of the requested ones so the snapshot
    /// stays within [`MAX_SNAPSHOT_BYTES`] (only with chunks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scrollback_omitted: Option<usize>,
}

/// `terminal.output`: raw PTY output bytes `[seq, seq + len)`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OutputEvent {
    pub id: String,
    pub seq: u64,
    /// Base64.
    pub data: String,
}

/// `terminal.resized`: the terminal has this size from output offset `seq` on.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ResizedEvent {
    pub id: String,
    pub seq: u64,
    pub cols: u16,
    pub rows: u16,
}

/// `terminal.exited`: the last notification on an attached connection.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ExitedEvent {
    pub id: String,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default)]
    pub killed: bool,
}

/// `terminal.lagged` (v7).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct LaggedEvent {
    pub id: String,
    /// Offset of the first output byte this client did not get.
    pub seq: u64,
    /// Bytes queued for this client when it was dropped.
    pub queued_bytes: u64,
    /// The daemon's per-client limit (`podd --attach-queue-limit`).
    pub limit_bytes: u64,
}

// ---------------------------------------------------------------------------
// Repositories and tasks (P3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RepoAddParams {
    /// Root of a git work tree (the main worktree, not a linked one).
    pub path: String,
    /// Registry name; defaults to the directory name. Used in task ids and
    /// worktree paths, so it is restricted to `[A-Za-z0-9._-]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RepoInfo {
    pub name: String,
    /// Absolute path of the repository root.
    pub path: String,
    pub added_ms: u64,
    /// Ref used when `task.create` gets no `base` (`origin/HEAD`'s branch if it
    /// exists locally, else the checked-out branch, else `HEAD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_base: Option<String>,
    /// Number of tasks in this repository.
    pub tasks: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RepoListResult {
    pub repos: Vec<RepoInfo>,
}

/// A repository by registry name or by path.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RepoRef {
    pub repo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct RepoRemoveResult {
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    /// `claude [prompt]` (Claude Code).
    Claude,
    /// `codex [prompt]`.
    Codex,
    /// `powershell -NoLogo`; a prompt is not allowed.
    Shell,
}

/// What runs in a task's first terminal: a known agent or an explicit argv.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(untagged)]
pub enum AgentSpec {
    Kind(AgentKind),
    Argv { argv: Vec<String> },
}

impl Default for AgentSpec {
    fn default() -> Self {
        AgentSpec::Kind(AgentKind::Shell)
    }
}

impl std::fmt::Display for AgentSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentSpec::Kind(AgentKind::Claude) => f.write_str("claude"),
            AgentSpec::Kind(AgentKind::Codex) => f.write_str("codex"),
            AgentSpec::Kind(AgentKind::Shell) => f.write_str("shell"),
            AgentSpec::Argv { argv } => write!(f, "{}", argv.join(" ")),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskCreateParams {
    /// Repository name or path (must be registered with `repo.add`).
    pub repo: String,
    /// Task name: `[A-Za-z0-9._-]`, at most 64 chars. The branch is
    /// `pod/<name>` and the worktree `<worktrees_root>\<repo>\<name>`.
    /// Empty: a name is suggested (never one of a live or removed task).
    #[serde(default)]
    pub name: String,
    /// Starting point (any commit-ish). Defaults to the repository's default base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Defaults to `shell`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentSpec>,
    /// Initial prompt, passed as the agent CLI's prompt argument (claude/codex only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// The caller's `POD_TERMINAL_ID` when it runs inside a pod terminal;
    /// recorded as the parent (and its task as the parent task).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cols: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rows: Option<u16>,
    /// Branch to use instead of `pod/<name>`. An existing branch is checked
    /// out (then `base` only labels the comparison base).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Task brief (any length, any characters). Written to a file
    /// (`POD_BRIEF`); the agent gets a short safe first line instead.
    /// Not together with `prompt`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<String>,
    /// First line for the agent instead of the default one; `{brief_path}`
    /// is replaced by the brief file path. Must not contain `"`, `%`, CR or LF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief_line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupPolicy>,
    /// Overrides the repository's `setup.mode`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_mode: Option<SetupMode>,
    /// Default: `{focus: true, reason: user}`, or `{focus: false, reason:
    /// worker}` when `parent_terminal` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_hint: Option<OpenHint>,
    /// Extra environment of the task's terminals (agent, setup, tabs, resume).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskListParams {
    /// Only tasks of this repository (name or path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// A task by id (`<repo>/<name>`) or by bare name when that is unique.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskRef {
    pub task: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CommitInfo {
    pub sha: String,
    pub summary: String,
    pub author: String,
    /// Committer time, Unix epoch milliseconds.
    pub time_ms: u64,
}

/// Git facts of a task's worktree, computed on every `task.list` / `task.show`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskGit {
    /// Checked-out branch (normally `pod/<name>`); `None` when detached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    /// Commits on HEAD that are not on the base.
    pub ahead: u32,
    /// Commits on the base that are not on HEAD.
    pub behind: u32,
    /// Entries of `git status --porcelain` (modified, staged, untracked...).
    pub dirty: u32,
    /// Commits not reachable from the base or any remote-tracking branch.
    pub unpushed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_commit: Option<CommitInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskTerminal {
    pub id: String,
    /// `None` when the daemon no longer knows the terminal (its holder vanished).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<TerminalInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskInfo {
    /// `<repo>/<name>`; also the name of the task's first terminal.
    pub id: String,
    pub repo: String,
    pub name: String,
    pub branch: String,
    /// Base ref as given (or defaulted) at creation.
    pub base: String,
    /// Commit the base resolved to at creation.
    pub base_commit: String,
    /// Absolute worktree path.
    pub worktree: String,
    pub agent: AgentSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_terminal: Option<String>,
    pub terminals: Vec<TaskTerminal>,
    /// False when the worktree directory has disappeared.
    pub worktree_exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<TaskGit>,
    /// Why `git` is missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_error: Option<String>,
    /// Aggregate agent status of the task's terminals (status_protocol.rs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<TaskStatus>,
    /// Review comment counts (v5; absent when the task has none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<ReviewCounts>,
    /// Gates about this task (v5), newest last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gates: Vec<GateInfo>,
    /// Group this task is a member of (v5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Last known pull request of the task's branch (v10, docs/15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<TaskPr>,
    /// `active` or `archived` (docs/19 §3.6).
    #[serde(default)]
    pub lifecycle: Lifecycle,
    /// The agent terminal (normally the first one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<SetupInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brief: Option<BriefInfo>,
    /// The agent's conversation id, for `task.resume`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<AgentSession>,
    /// The branch existed before the task (`branch` param); never deleted by Pod.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub branch_existed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskListResult {
    pub tasks: Vec<TaskInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DiffFile {
    /// Path relative to the worktree root, `/`-separated.
    pub path: String,
    /// Previous path of a rename or copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// `A` added, `M` modified, `D` deleted, `R` renamed, `C` copied, `T` type
    /// change, `?` untracked (not yet added).
    pub status: String,
    /// Lines added / deleted; `None` for binary files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<u64>,
    #[serde(default)]
    pub binary: bool,
}

/// The worktree (committed + uncommitted + untracked) against the merge base
/// of the task's base and HEAD.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskDiffResult {
    pub task: String,
    pub base: String,
    pub merge_base: String,
    pub files: Vec<DiffFile>,
    pub added: u64,
    pub deleted: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskRemoveParams {
    pub task: String,
    /// Remove even with uncommitted changes or unpushed commits.
    #[serde(default)]
    pub force: bool,
    /// Also delete the branch `pod/<name>`.
    #[serde(default)]
    pub delete_branch: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskRemoveResult {
    pub id: String,
    /// Terminals killed and removed.
    pub terminals: Vec<String>,
    pub worktree_removed: bool,
    pub branch_deleted: bool,
    /// `delete_branch` was asked but the branch was kept: why (not merged
    /// into the base, existed before the task...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_kept: Option<String>,
}

/// v9 `task.touch`: a client saved files in the task's worktree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskTouchParams {
    pub task: String,
    /// Saved paths (relative to the worktree or absolute), passed through to
    /// the `task.changed` event; informational.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskTouchResult {
    pub task: String,
    /// Fresh git facts of the worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<TaskGit>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_error: Option<String>,
    /// The facts differ from the last ones the daemon announced (a
    /// `task.changed` `git` event went out as well).
    pub git_changed: bool,
}

pub fn b64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())
}

/// JSON schema of every method's params and result, keyed by method name.
pub fn schema() -> Value {
    fn entry<M: Method>() -> (String, Value) {
        (
            M::NAME.to_string(),
            serde_json::json!({
                "params": schemars::schema_for!(M::Params),
                "result": schemars::schema_for!(M::Result),
            }),
        )
    }
    let methods: serde_json::Map<String, Value> = [
        entry::<ServerStatus>(),
        entry::<ServerStop>(),
        entry::<TerminalRun>(),
        entry::<TerminalList>(),
        entry::<TerminalSend>(),
        entry::<TerminalKeys>(),
        entry::<TerminalInput>(),
        entry::<TerminalRead>(),
        entry::<TerminalWait>(),
        entry::<TerminalResize>(),
        entry::<TerminalKill>(),
        entry::<TerminalRm>(),
        entry::<TerminalKeep>(),
        entry::<TerminalAttach>(),
        entry::<TerminalShow>(),
        entry::<TerminalAck>(),
        entry::<StatusWait>(),
        entry::<StatusSubscribe>(),
        entry::<RepoAdd>(),
        entry::<RepoList>(),
        entry::<RepoRemove>(),
        entry::<TaskCreate>(),
        entry::<TaskList>(),
        entry::<TaskShow>(),
        entry::<TaskDiff>(),
        entry::<TaskRemove>(),
        entry::<TaskTouch>(),
        entry::<TaskPrStatusMethod>(),
        entry::<TaskPrCreate>(),
        entry::<TaskPrMerge>(),
        entry::<ReviewCommentsAdd>(),
        entry::<ReviewCommentsList>(),
        entry::<ReviewCommentsResolve>(),
        entry::<ReviewCommentsRemove>(),
        entry::<ReviewSend>(),
        entry::<ReviewDeliveries>(),
        entry::<ReviewCancel>(),
        entry::<GroupCreate>(),
        entry::<GroupList>(),
        entry::<GroupShow>(),
        entry::<GroupCompare>(),
        entry::<GroupCheck>(),
        entry::<GroupCheckCancel>(),
        entry::<GroupPick>(),
        entry::<GroupRemove>(),
        entry::<GateCreate>(),
        entry::<GateResolve>(),
        entry::<GateList>(),
        entry::<GateShow>(),
        entry::<GateWait>(),
        entry::<ModListMethod>(),
        entry::<ModSetEnabled>(),
        entry::<ModGrant>(),
        entry::<ModLog>(),
        entry::<ModCommand>(),
        entry::<ModReload>(),
        entry::<OrchRunCreate>(),
        entry::<OrchRunList>(),
        entry::<OrchRunShow>(),
        entry::<OrchItemCreate>(),
        entry::<OrchItemList>(),
        entry::<OrchItemUpdate>(),
        entry::<OrchWorkerStart>(),
        entry::<OrchWorkerList>(),
        entry::<OrchWorkerShow>(),
        entry::<OrchWorkerRead>(),
        entry::<OrchWorkerStop>(),
        entry::<OrchWorkerAbandon>(),
        entry::<OrchWorkerKeep>(),
        entry::<OrchWorkerRelease>(),
        entry::<OrchMailSend>(),
        entry::<OrchMailCheck>(),
        entry::<OrchMailReply>(),
        entry::<OrchMailAsk>(),
        entry::<OrchDone>(),
        entry::<OrchMailList>(),
        entry::<OrchConfigGet>(),
        entry::<OrchConfigSet>(),
        entry::<OrchQueue>(),
        entry::<OrchGateRequest>(),
        entry::<OrchGateDecide>(),
        entry::<OrchLimitsGet>(),
        entry::<OrchLimitsSet>(),
        entry::<OrchWait>(),
        entry::<OrchCommit>(),
        entry::<SessionsListMethod>(),
        entry::<SessionsScan>(),
        entry::<SessionsSettings>(),
        entry::<SessionsRegister>(),
        entry::<SessionsResume>(),
        entry::<SessionsHide>(),
        entry::<ConnectionHello>(),
        entry::<TerminalInputOnce>(),
        entry::<FsStat>(),
        entry::<FsList>(),
        entry::<FsRead>(),
        entry::<FsWrite>(),
        entry::<ReviewDiff>(),
        entry::<ReviewBlob>(),
        entry::<MobileDevices>(),
        entry::<MobileRevoke>(),
        entry::<MobilePolicy>(),
        entry::<MobileAudit>(),
        entry::<MobileSettingsMethod>(),
        entry::<TaskArchive>(),
        entry::<TaskUnarchive>(),
        entry::<TaskResume>(),
        entry::<TaskCleanupCandidates>(),
        entry::<TaskSuggestName>(),
        entry::<RepoConfigMethod>(),
        entry::<RepoSet>(),
    ]
    .into_iter()
    .collect();
    serde_json::json!({
        "protocol": PROTOCOL_VERSION,
        "transport": "newline-delimited JSON-RPC 2.0 over a Windows named pipe, or over the stdio of `pod remote-stdio`",
        "min_protocol": PROTOCOL_MIN,
        "methods": methods,
        "notifications": {
            NOTIFY_OUTPUT: schemars::schema_for!(OutputEvent),
            NOTIFY_RESIZED: schemars::schema_for!(ResizedEvent),
            NOTIFY_EXITED: schemars::schema_for!(ExitedEvent),
            NOTIFY_LAGGED: schemars::schema_for!(LaggedEvent),
            NOTIFY_SNAPSHOT: schemars::schema_for!(SnapshotChunkEvent),
            NOTIFY_STATUS_CHANGED: schemars::schema_for!(StatusChangedEvent),
            NOTIFY_STATUS_LAGGED: schemars::schema_for!(StatusLaggedEvent),
            NOTIFY_GATE_CHANGED: schemars::schema_for!(GateChangedEvent),
            NOTIFY_GROUP_CHECK: schemars::schema_for!(GroupCheckEvent),
            NOTIFY_REVIEW_DELIVERY: schemars::schema_for!(ReviewDeliveryEvent),
            NOTIFY_TERMINAL_CHANGED: schemars::schema_for!(TerminalChangedEvent),
            NOTIFY_TASK_CHANGED: schemars::schema_for!(TaskChangedEvent),
            NOTIFY_TASK_REMOVING: schemars::schema_for!(TaskRemovingEvent),
            NOTIFY_TASK_PR_CHANGED: schemars::schema_for!(TaskPrChangedEvent),
            NOTIFY_MOD_NOTIFY: schemars::schema_for!(ModNotifyEvent),
            NOTIFY_MOD_CHANGED: schemars::schema_for!(ModChangedEvent),
            NOTIFY_ORCH_CHANGED: schemars::schema_for!(OrchChangedEvent),
            NOTIFY_ORCH_MAIL: schemars::schema_for!(OrchMailEvent),
            NOTIFY_SESSIONS_CHANGED: schemars::schema_for!(SessionsChangedEvent),
        },
        "error": schemars::schema_for!(RpcError),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// ui-editor-03: every message shape that carries an unreachable
    /// terminal is downgraded for a v18 connection, nothing for v19.
    #[test]
    fn downgrade_json_covers_every_shape() {
        let mut u = TerminalInfo::new("t5", vec![], "", TerminalState::Unreachable, 0, 0, 1);
        u.unreachable = Some("busy".into());
        let u = serde_json::to_value(&u).unwrap();
        let messages = [
            // terminal.list
            json!({"jsonrpc": "2.0", "id": 1, "result": {"terminals": [u.clone()]}}),
            // terminal.kill / run / resize
            json!({"jsonrpc": "2.0", "id": 2, "result": u.clone()}),
            // terminal.wait / attach
            json!({"id": 3, "result": {"outcome": "exited", "terminal": u.clone(), "waited_ms": 0}}),
            // task.* (TaskTerminal)
            json!({"id": 4, "result": {"terminals": [{"id": "t5", "info": u.clone()}]}}),
            // terminal.changed notification
            json!({"method": NOTIFY_TERMINAL_CHANGED,
                   "params": {"seq": 1, "id": "t5", "change": "created", "terminal": u.clone()}}),
            // terminal.read
            json!({"id": 5, "result": {"id": "t5", "mode": "screen", "lines": [],
                   "cursor": [0, 0], "state": "unreachable"}}),
        ];
        for m in messages {
            let mut new = m.clone();
            downgrade_json(&mut new, UNREACHABLE_PROTOCOL);
            assert_eq!(new, m, "v19 untouched");
            let mut old = m.clone();
            downgrade_json(&mut old, UNREACHABLE_PROTOCOL - 1);
            let text = old.to_string();
            assert!(!text.contains("\"state\":\"unreachable\""), "{text}");
            assert!(text.contains("\"state\":\"exited\""), "{text}");
            if m.to_string().contains("argv") {
                assert!(text.contains("\"lost\":true"), "{text}");
            }
        }
    }

    /// v20 chunk fields of `terminal.attach` exist only for v20 clients;
    /// the unreachable rewrite still reaches the nested terminal.
    #[test]
    fn v20_attach_fields_are_dropped_for_older_clients() {
        let mut u = TerminalInfo::new("t5", vec![], "", TerminalState::Unreachable, 0, 0, 1);
        u.unreachable = Some("busy".into());
        let chunked = json!({"jsonrpc": "2.0", "id": 1, "result": {
            "terminal": u, "seq": 7, "snapshot": "",
            "snapshot_chunks": 3, "snapshot_bytes": 70000, "scrollback_omitted": 500}});
        let mut same = chunked.clone();
        downgrade_json(&mut same, PROTOCOL_VERSION);
        assert_eq!(same, chunked, "v20 untouched");
        let mut v19 = chunked.clone();
        downgrade_json(&mut v19, CHUNKED_ATTACH_PROTOCOL - 1);
        let r = &v19["result"];
        for k in ["snapshot_chunks", "snapshot_bytes", "scrollback_omitted"] {
            assert!(r.get(k).is_none(), "{k} left for v19: {r}");
        }
        assert_eq!(
            r["terminal"]["state"], "unreachable",
            "v19 knows unreachable"
        );
        let mut v18 = chunked.clone();
        downgrade_json(&mut v18, UNREACHABLE_PROTOCOL - 1);
        assert_eq!(v18["result"]["terminal"]["state"], "exited");
        assert!(v18["result"].get("snapshot_chunks").is_none());
        // Other objects with the same key names are left alone.
        let other = json!({"snapshot_chunks": 1, "seq": 2});
        let mut o = other.clone();
        downgrade_json(&mut o, PROTOCOL_MIN);
        assert_eq!(o, other);
    }

    /// v21 (docs/19 §20): `server.status` `endpoint` is not sent to a
    /// connection whose hello chose 20 or older; v21 keeps it.
    #[test]
    fn v21_endpoint_is_dropped_for_older_clients() {
        let status = ServerStatusResult {
            version: "x".into(),
            protocol: PROTOCOL_VERSION,
            pid: 1,
            pipe: r"\\.\pipe\podd-x".into(),
            started_ms: 1,
            uptime_ms: 2,
            terminals: 0,
            running: 0,
            state_dir: "s".into(),
            endpoint: Some(Endpoint::Worker),
        };
        let v = json!({"jsonrpc": "2.0", "id": 1, "result": status});
        assert_eq!(v["result"]["endpoint"], "worker");
        let mut same = v.clone();
        downgrade_json(&mut same, WORKER_ENDPOINT_PROTOCOL);
        assert_eq!(same, v);
        for p in [
            WORKER_ENDPOINT_PROTOCOL - 1,
            UNREACHABLE_PROTOCOL,
            PROTOCOL_MIN,
        ] {
            let mut old = v.clone();
            downgrade_json(&mut old, p);
            assert!(old["result"].get("endpoint").is_none(), "v{p}: {old}");
            assert_eq!(old["result"]["state_dir"], "s");
        }
        // Another object with an `endpoint` key is left alone.
        let other = json!({"endpoint": "x", "pipe": "p"});
        let mut o = other.clone();
        downgrade_json(&mut o, PROTOCOL_MIN);
        assert_eq!(o, other);
    }

    #[test]
    fn wait_until_wire_shape() {
        assert_eq!(
            serde_json::to_value(WaitUntil::Exit).unwrap(),
            json!("exit")
        );
        assert_eq!(
            serde_json::to_value(WaitUntil::OutputContains("x".into())).unwrap(),
            json!({"output_contains": "x"})
        );
        let p: WaitParams = serde_json::from_value(
            json!({"name": "a", "until": {"output_contains": "ok"}, "timeout_ms": 5}),
        )
        .unwrap();
        assert_eq!(p.target, TerminalRef::name("a"));
        assert_eq!(p.until, WaitUntil::OutputContains("ok".into()));
    }

    #[test]
    fn terminal_ref_parse() {
        assert_eq!(TerminalRef::parse("t12"), TerminalRef::id("t12"));
        assert_eq!(TerminalRef::parse("t"), TerminalRef::name("t"));
        assert_eq!(TerminalRef::parse("tests"), TerminalRef::name("tests"));
    }

    #[test]
    fn empty_params_accept_null_and_object() {
        let _: Empty = serde_json::from_value(json!({})).unwrap();
        let req: Request =
            serde_json::from_str(r#"{"jsonrpc":"2.0","id":1,"method":"terminal.list"}"#).unwrap();
        assert!(req.params.is_null());
        let stop: StopParams = serde_json::from_value(json!({})).unwrap();
        assert!(!stop.kill);
    }

    #[test]
    fn base64_roundtrip() {
        let data = b"\x1b[31mhi\r\n\xff";
        assert_eq!(b64_decode(&b64_encode(data)).unwrap(), data);
    }

    #[test]
    fn schema_lists_all_methods() {
        let s = schema();
        for m in METHODS
            .iter()
            .chain(STATUS_METHODS)
            .chain(ORCH_METHODS)
            .chain(PR_METHODS)
            .chain(ORCH_RUN_METHODS)
            .chain(MOBILE_METHODS)
            .chain(SESSION_METHODS)
            .chain(SESSIONS_METHODS)
            .chain(FILES_METHODS)
            .chain([&ConnectionHello::NAME, &TerminalInputOnce::NAME])
        {
            assert!(s["methods"].get(*m).is_some(), "{m} missing from schema");
        }
        assert!(s["notifications"].get(NOTIFY_OUTPUT).is_some());
    }

    #[test]
    fn agent_spec_wire_shape() {
        let a: AgentSpec = serde_json::from_value(json!("claude")).unwrap();
        assert_eq!(a, AgentSpec::Kind(AgentKind::Claude));
        let a: AgentSpec = serde_json::from_value(json!({"argv": ["cmd", "/k"]})).unwrap();
        assert_eq!(
            a,
            AgentSpec::Argv {
                argv: vec!["cmd".into(), "/k".into()]
            }
        );
        assert!(serde_json::from_value::<AgentSpec>(json!("vim")).is_err());
        assert_eq!(
            serde_json::to_value(AgentSpec::default()).unwrap(),
            json!("shell")
        );
    }
}
