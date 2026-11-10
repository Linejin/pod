//! Pod core daemon.
//!
//! Client side (always built):
//! - [`protocol`]: JSON-RPC 2.0 types shared by the daemon, the CLI and future UI,
//!   re-exported from `pod-protocol` with the nine `*_protocol` domain modules.
//! - [`client`]: blocking pipe client with daemon auto-start and attach streams.
//! - [`env`], [`keys`]: terminal environment hygiene, named keys -> bytes.
//!
//! Server side (feature `server`):
//! - [`server`]: the `podd` daemon (registry, RPC, read/wait/attach).
//! - [`holder`] + [`holder_proto`]: the `pod-holder` process that owns one
//!   terminal (ConPTY + Job Object) and survives daemon restarts.
//! - [`git`]: the `git` CLI wrapper behind repositories and tasks.
//! - [`vt`], [`pty`], [`winsec`]: VT state + ANSI snapshots, ConPTY spawn,
//!   user-only pipe DACL and the clean user environment.

pub mod client;
#[cfg(windows)]
pub mod detach;
pub mod endpoint;
pub mod env;
pub mod keys;
pub mod transport;

// The wire modules live in `pod-protocol` (docs/20-refactor/daemon.md §2,
// card D-F2). They stay reachable under their old paths, as the same modules:
// `podd::protocol::TaskInfo` is `pod_protocol::protocol::TaskInfo`.
pub use pod_protocol::{
    discovery_protocol, mobile_protocol, mods_protocol, orch_protocol, orch_run_protocol,
    pr_protocol, protocol, remote_protocol, session_protocol, status_protocol,
};

#[cfg(feature = "server")]
pub mod git;
#[cfg(feature = "server")]
pub mod holder;
#[cfg(feature = "server")]
pub mod holder_proto;
#[cfg(feature = "server")]
pub mod osc_replay;
#[cfg(feature = "server")]
pub mod pty;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
pub mod vt;
#[cfg(feature = "server")]
pub mod winsec;

/// Unix epoch milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
