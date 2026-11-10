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

/// Capability names (docs/17 §5). None is implemented yet, so the daemon
/// advertises none of them; a client that requires one is refused.
pub mod capability {
    pub const REMOTE_FILES: &str = "remote.files";
    pub const REMOTE_DIFF: &str = "remote.diff";
    pub const ATTACH_CHUNKS: &str = "attach.chunks";
    pub const GATE_CAS: &str = "gate.cas";
}

/// Capabilities this daemon offers.
pub const CAPABILITIES: &[&str] = &[];

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
}

/// Pick the newest version in both ranges and check required capabilities.
/// `server` is the daemon's `(min, max)`.
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
        .filter(|c| !capabilities.contains(c))
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
        let mut e = RpcError::new(
            remote_codes::PROTOCOL_MISMATCH,
            format!(
                "this podd lacks required capabilities: {}",
                missing.join(", ")
            ),
        );
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
    fn inverted_client_range_is_invalid_params() {
        let e = negotiate(&hello(13, 12, &[]), (11, 12), &[]).unwrap_err();
        assert_eq!(e.code, crate::protocol::codes::INVALID_PARAMS);
    }
}
