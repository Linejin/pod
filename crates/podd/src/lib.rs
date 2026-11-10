//! Pod core daemon.
//!
//! Client side (always built):
//! - [`protocol`]: JSON-RPC 2.0 types shared by the daemon, the CLI and future UI.
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
pub mod discovery_protocol;
pub mod endpoint;
pub mod env;
pub mod keys;
pub mod mobile_protocol;
pub mod mods_protocol;
pub mod orch_protocol;
pub mod orch_run_protocol;
pub mod pr_protocol;
pub mod protocol;
pub mod remote_protocol;
pub mod session_protocol;
pub mod status_protocol;
pub mod transport;

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
