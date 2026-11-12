//! Pod daemon client (docs/20-refactor/daemon.md §2, card D-T1).
//!
//! - [`client`]: blocking pipe client with daemon auto-start and attach streams.
//! - [`transport`]: NDJSON line framing over the named pipe or any
//!   `Read` + `Write` pair (docs/17 R1).
//! - [`endpoint`]: pipe names, state folders and the `POD_*` variables.
//! - `detach` (Windows): starting podd / pod-holder with no inherited handles.
//! - [`env`]: terminal environment hygiene and the worker git guard, pure
//!   map transforms (the git config itself is read by the daemon).
//! - [`files`]: remote file saves that survive a lost answer (v22, docs/17
//!   R5) and the decoding of file answers.
//! - [`protocol`]: the wire types, re-exported from `pod-protocol`.
//!
//! Nothing here depends on the server, the holder, the PTY stack or tokio.
//! `podd` re-exports these modules under their old paths, and the app, CLI
//! and remote crates use this crate under the dependency name `podd`, so
//! `podd::client::Client` is `pod_client::client::Client` in both.

pub mod client;
#[cfg(windows)]
pub mod detach;
pub mod endpoint;
pub mod env;
pub mod files;
pub mod transport;

// Consumers that still spell `podd::protocol` reach the same module.
pub use pod_protocol::protocol;

/// Unix epoch milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
