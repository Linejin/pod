//! Protocol v12 connection handshake (docs/17 §2.3, §5): `connection.hello`.
//!
//! A remote client (over `pod remote-stdio`) first reads `server.status` to
//! see the daemon's protocol, then sends `connection.hello` with the range of
//! protocol versions it speaks and the capabilities it cannot work without.
//! The daemon answers with the version both use, its own capabilities, its
//! persistent `machine_id`, the `boot_id` of this daemon start and its frame
//! limits. Local v11 clients never send it and keep working unchanged.
//! Re-exported from [`crate::protocol`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::{Method, PROTOCOL_VERSION, RpcError};

pub struct ConnectionHello;
impl Method for ConnectionHello {
    const NAME: &'static str = "connection.hello";
    type Params = HelloParams;
    type Result = HelloResult;
}

/// The oldest client protocol this daemon still serves. v11 clients never
/// send `connection.hello`; their methods are unchanged in v12.
pub const PROTOCOL_MIN: u32 = 11;
/// The first protocol with `connection.hello`; remote connections need it.
pub const HELLO_PROTOCOL: u32 = 12;

/// Capability names (docs/17 §5). `attach.chunks` since v20, `remote.files`
/// and `remote.diff` since v22; a client that requires `gate.cas` is
/// refused.
pub mod capability {
    pub const REMOTE_FILES: &str = "remote.files";
    pub const REMOTE_DIFF: &str = "remote.diff";
    /// v20: on a connection whose hello chose [`super::CHUNKED_ATTACH_PROTOCOL`]
    /// or newer, attach snapshots come as `terminal.snapshot` chunks and
    /// output notifications carry at most [`super::REMOTE_OUTPUT_CHUNK`] bytes.
    pub const ATTACH_CHUNKS: &str = "attach.chunks";
    pub const GATE_CAS: &str = "gate.cas";
}

/// Capabilities this daemon offers (to a connection that chose v20 or newer;
/// see [`capabilities_for`]).
pub const CAPABILITIES: &[&str] = &[
    capability::ATTACH_CHUNKS,
    capability::REMOTE_FILES,
    capability::REMOTE_DIFF,
];

/// The capabilities a connection that chose `protocol` gets in its hello
/// result: an older client is told nothing it cannot use.
pub fn capabilities_for(protocol: u32) -> Vec<String> {
    CAPABILITIES
        .iter()
        .filter(|c| offered_at(c, protocol))
        .map(|c| c.to_string())
        .collect()
}

/// Is `capability` part of `protocol` (`attach.chunks` from v20,
/// `remote.files` / `remote.diff` from v22)? Unknown names: always.
pub fn offered_at(capability: &str, protocol: u32) -> bool {
    match capability {
        capability::ATTACH_CHUNKS => protocol >= CHUNKED_ATTACH_PROTOCOL,
        capability::REMOTE_FILES | capability::REMOTE_DIFF => {
            protocol >= crate::files_protocol::FILES_PROTOCOL
        }
        _ => true,
    }
}

/// v20 (docs/17 R4): the first protocol with chunked attach snapshots,
/// remote attach framing and `terminal.input_once`.
pub const CHUNKED_ATTACH_PROTOCOL: u32 = 20;
/// v20 remote framing: the largest raw output chunk in one
/// `terminal.output`, and the size of each `terminal.snapshot` chunk.
pub const REMOTE_OUTPUT_CHUNK: usize = 32 * 1024;
/// v20 remote framing: per-attach queue of a remote connection (output not
/// yet written to it); past it the client gets `terminal.lagged`. The
/// daemon's own limit applies when it is smaller.
pub const REMOTE_ATTACH_QUEUE: usize = 2 * 1024 * 1024;
/// v20: a chunked snapshot is at most this large; scrollback is reduced
/// (`AttachResult.scrollback_omitted`) to fit.
pub const MAX_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

/// v20 `terminal.snapshot`: one piece of a chunked attach snapshot.
pub const NOTIFY_SNAPSHOT: &str = "terminal.snapshot";

/// `terminal.snapshot` (v20): piece `index` of `total` of the snapshot whose
/// `terminal.attach` reply had this `seq`. Chunks come in order, right after
/// the reply and before any other notification; their concatenation is the
/// snapshot ANSI.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct SnapshotChunkEvent {
    pub id: String,
    /// The snapshot's output offset (`AttachResult.seq`).
    pub seq: u64,
    /// 0-based position of this chunk.
    pub index: u32,
    /// Number of chunks (`AttachResult.snapshot_chunks`).
    pub total: u32,
    /// Base64, at most [`REMOTE_OUTPUT_CHUNK`] raw bytes.
    pub data: String,
}

method!(
    TerminalInputOnce,
    "terminal.input_once",
    InputOnceParams,
    InputOnceResult
);

/// `terminal.input_once` (v20): `terminal.input` with an identity, applied at
/// most once per `(stream, seq)` and terminal (docs/17 R4, docs/22 TM-14).
/// A client whose answer was lost does **not** resend the input: after
/// reconnecting to the same daemon start it asks with `check: true`, which
/// says whether that input was applied and fences it (a delivery of that
/// `seq` that is still on its way is then never written). The daemon
/// remembers the newest `seq` of the [`INPUT_STREAMS_KEPT`] most recently
/// used streams of each terminal, for this daemon start only: after a new
/// `boot_id` the result of a lost input stays unknown.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct InputOnceParams {
    #[serde(flatten)]
    pub target: crate::protocol::TerminalRef,
    /// Raw input bytes, base64 (standard alphabet, padded). Ignored with
    /// `check`.
    #[serde(default)]
    pub data: String,
    /// The sender's input stream: 1–64 characters `A-Za-z0-9_-`, random per
    /// client session.
    pub stream: String,
    /// Increases with every new input of the stream. A `seq` not above the
    /// newest one seen is never written (`already`).
    pub seq: u64,
    /// Only ask whether `(stream, seq)` was applied; never writes. A `seq`
    /// not applied yet is fenced (`fenced`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub check: bool,
}

/// What `terminal.input_once` did.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputOnceOutcome {
    /// Written by this request.
    Written,
    /// This `seq` (or a newer one of the stream) was seen before: nothing
    /// written now. For this very `seq`, `bytes` is what was written then.
    Already,
    /// A `check` fenced this `seq`: it was never written and never will be
    /// (the answer to the check, and to a late delivery of that input).
    Fenced,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct InputOnceResult {
    pub id: String,
    pub outcome: InputOnceOutcome,
    /// Bytes written to the terminal for this `seq` (0 when `fenced`).
    pub bytes: usize,
    /// Query replies removed from the input (see `terminal.input`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dropped_replies: Option<crate::protocol::DroppedReplies>,
}

/// Input streams remembered per terminal by `terminal.input_once`.
pub const INPUT_STREAMS_KEPT: usize = 64;
/// Longest `InputOnceParams.stream`.
pub const INPUT_STREAM_MAX: usize = 64;

/// Is `s` a valid `terminal.input_once` stream name?
pub fn valid_input_stream(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= INPUT_STREAM_MAX
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Error codes added with protocol v12.
pub mod remote_codes {
    /// `connection.hello`: no protocol version both sides speak, or a
    /// required capability is missing. `data` = `{min_protocol,
    /// max_protocol, capabilities, missing}`. Nothing was changed.
    pub const PROTOCOL_MISMATCH: i64 = -32051;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct HelloParams {
    /// Oldest protocol version the client speaks.
    pub min_protocol: u32,
    /// Newest protocol version the client speaks.
    pub max_protocol: u32,
    /// Capabilities the client cannot work without.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub require: Vec<String>,
    /// Free-form client name/version, for the daemon log only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// v21, worker endpoint only (docs/19 §20): the Pod worker this
    /// connection speaks for. The daemon checks the token and derives the
    /// connection's identity from it; the full pipe ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<WorkerAuth>,
}

/// A worker's attempt and its token (`POD_ATTEMPT_ID`,
/// `POD_ATTEMPT_TOKEN`), said once per worker endpoint connection.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WorkerAuth {
    pub attempt: String,
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct HelloResult {
    /// The version both sides use from now on (the newest common one).
    pub protocol: u32,
    /// The daemon's supported range.
    pub min_protocol: u32,
    pub max_protocol: u32,
    pub capabilities: Vec<String>,
    /// Random id persisted in the daemon's state directory: the same across
    /// restarts of this daemon, different for another machine or user.
    pub machine_id: String,
    /// Random id of this daemon start (event sequences restart with it).
    pub boot_id: String,
    /// Daemon version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// `"windows"` or `"linux"`; paths in requests follow this OS.
    pub os: String,
    pub limits: FrameLimits,
}

/// Size limits of a connection, in bytes.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FrameLimits {
    /// Longest line on the remote stdio transport (both directions).
    pub max_line_bytes: u64,
    /// Longest request line the daemon reads (longer: connection closed).
    pub max_request_bytes: u64,
    /// Largest raw output chunk in one `terminal.output` (before base64).
    pub max_output_chunk_bytes: u64,
    /// Per-client attach queue; past it the client gets `terminal.lagged`.
    pub attach_queue_bytes: u64,
    /// v22: largest file `fs.read` / `review.blob` return.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_read_bytes: Option<u64>,
    /// v22: largest content `fs.write` takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_write_bytes: Option<u64>,
}

/// Pick the newest version in both ranges and check required capabilities
/// (offered by the daemon and part of the chosen version, see
/// [`offered_at`]). `server` is the daemon's `(min, max)`.
pub fn negotiate(
    p: &HelloParams,
    server: (u32, u32),
    capabilities: &[&str],
) -> Result<u32, RpcError> {
    if p.min_protocol > p.max_protocol {
        return Err(RpcError::new(
            crate::protocol::codes::INVALID_PARAMS,
            format!(
                "min_protocol {} is greater than max_protocol {}",
                p.min_protocol, p.max_protocol
            ),
        ));
    }
    let lo = p.min_protocol.max(server.0);
    let hi = p.max_protocol.min(server.1);
    let missing: Vec<&str> = p
        .require
        .iter()
        .map(String::as_str)
        .filter(|c| !capabilities.contains(c) || !offered_at(c, hi))
        .collect();
    let data = || {
        serde_json::json!({
            "min_protocol": server.0,
            "max_protocol": server.1,
            "capabilities": capabilities,
            "missing": missing,
        })
    };
    if lo > hi {
        let mut e = RpcError::new(
            remote_codes::PROTOCOL_MISMATCH,
            format!(
                "no common protocol: client speaks {}..={}, this podd {}..={}; install matching Pod versions",
                p.min_protocol, p.max_protocol, server.0, server.1
            ),
        );
        e.data = Some(data());
        return Err(e);
    }
    if !missing.is_empty() {
        // Offered, but only by a newer protocol than the one both speak.
        let message = if missing.iter().all(|c| capabilities.contains(c)) {
            format!(
                "protocol {hi} has no {}; the client must speak a newer protocol",
                missing.join(", ")
            )
        } else {
            format!(
                "this podd lacks required capabilities: {}",
                missing.join(", ")
            )
        };
        let mut e = RpcError::new(remote_codes::PROTOCOL_MISMATCH, message);
        e.data = Some(data());
        return Err(e);
    }
    Ok(hi)
}

/// The daemon's range.
pub fn server_range() -> (u32, u32) {
    (PROTOCOL_MIN, PROTOCOL_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello(min: u32, max: u32, require: &[&str]) -> HelloParams {
        HelloParams {
            min_protocol: min,
            max_protocol: max,
            require: require.iter().map(|s| s.to_string()).collect(),
            client: None,
            worker: None,
        }
    }

    #[test]
    fn picks_the_newest_common_version() {
        assert_eq!(negotiate(&hello(12, 12, &[]), (11, 12), &[]), Ok(12));
        // A newer client that still speaks 12.
        assert_eq!(negotiate(&hello(12, 14, &[]), (11, 12), &[]), Ok(12));
        // A newer daemon that still serves 12.
        assert_eq!(negotiate(&hello(12, 12, &[]), (11, 15), &[]), Ok(12));
        assert_eq!(negotiate(&hello(12, 20, &[]), (11, 15), &[]), Ok(15));
    }

    #[test]
    fn disjoint_ranges_are_refused_with_the_daemon_range() {
        let e = negotiate(&hello(13, 14, &[]), (11, 12), &[]).unwrap_err();
        assert_eq!(e.code, remote_codes::PROTOCOL_MISMATCH);
        let d = e.data.unwrap();
        assert_eq!(d["min_protocol"], 11);
        assert_eq!(d["max_protocol"], 12);
        // A future daemon that dropped 12.
        let e = negotiate(&hello(12, 12, &[]), (13, 14), &[]).unwrap_err();
        assert_eq!(e.code, remote_codes::PROTOCOL_MISMATCH);
    }

    #[test]
    fn missing_required_capability_is_refused() {
        let e = negotiate(&hello(12, 12, &[capability::REMOTE_FILES]), (11, 12), &[]).unwrap_err();
        assert_eq!(e.code, remote_codes::PROTOCOL_MISMATCH);
        assert_eq!(e.data.unwrap()["missing"][0], "remote.files");
        assert_eq!(
            negotiate(&hello(12, 12, &["x"]), (11, 12), &["x", "y"]),
            Ok(12)
        );
    }

    #[test]
    fn attach_chunks_is_offered_from_v20_only() {
        assert!(capabilities_for(CHUNKED_ATTACH_PROTOCOL - 1).is_empty());
        assert_eq!(
            capabilities_for(CHUNKED_ATTACH_PROTOCOL),
            [capability::ATTACH_CHUNKS]
        );
        // v22: the remote file and diff methods.
        let f = crate::files_protocol::FILES_PROTOCOL;
        assert_eq!(capabilities_for(f - 1), [capability::ATTACH_CHUNKS]);
        assert_eq!(
            capabilities_for(f),
            [
                capability::ATTACH_CHUNKS,
                capability::REMOTE_FILES,
                capability::REMOTE_DIFF
            ]
        );
        assert_eq!(
            negotiate(
                &hello(12, 20, &[capability::ATTACH_CHUNKS]),
                (11, 20),
                CAPABILITIES
            ),
            Ok(20)
        );
        // A client that speaks 21 at most cannot require a v22 capability.
        let e = negotiate(
            &hello(12, 21, &[capability::REMOTE_FILES]),
            (11, 22),
            CAPABILITIES,
        )
        .unwrap_err();
        assert_eq!(e.data.unwrap()["missing"][0], "remote.files");
        assert_eq!(
            negotiate(
                &hello(12, 22, &[capability::REMOTE_DIFF]),
                (11, 22),
                CAPABILITIES
            ),
            Ok(22)
        );
    }

    #[test]
    fn input_stream_names() {
        assert!(valid_input_stream("a"));
        assert!(valid_input_stream(&"f".repeat(64)));
        assert!(valid_input_stream("pod-remote_0123"));
        assert!(!valid_input_stream(""));
        assert!(!valid_input_stream(&"f".repeat(65)));
        assert!(!valid_input_stream("a b"));
        assert!(!valid_input_stream("a/b"));
        assert!(!valid_input_stream("한"));
    }

    #[test]
    fn inverted_client_range_is_invalid_params() {
        let e = negotiate(&hello(13, 12, &[]), (11, 12), &[]).unwrap_err();
        assert_eq!(e.code, crate::protocol::codes::INVALID_PARAMS);
    }
}
