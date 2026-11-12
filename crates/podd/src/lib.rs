//! Pod core daemon.
//!
//! Client side (always built), from `pod-client` and `pod-protocol` under the
//! old paths, as the same items (docs/20-refactor/daemon.md §2, cards D-F2
//! and D-T1): `podd::client::Client` is `pod_client::client::Client`.
//! - [`protocol`]: JSON-RPC 2.0 types shared by the daemon, the CLI and future UI,
//!   re-exported from `pod-protocol` with the nine `*_protocol` domain modules.
//! - [`client`]: blocking pipe client with daemon auto-start and attach streams.
//! - [`transport`], [`endpoint`], `detach`: line framing, pipe names, detached spawn.
//! - [`env`](mod@env), [`keys`]: terminal environment hygiene, named keys -> bytes.
//!
//! Server side (feature `server`):
//! - [`server`]: the `podd` daemon (registry, RPC, read/wait/attach).
//! - [`holder`] + [`holder_proto`]: the `pod-holder` process that owns one
//!   terminal (ConPTY + Job Object) and survives daemon restarts.
//! - [`git`]: the `git` CLI wrapper behind repositories and tasks.
//! - [`vt`], [`pty`], [`winsec`]: VT state + ANSI snapshots, ConPTY spawn,
//!   user-only pipe DACL and the clean user environment.

// The blocking client lives in `pod-client` (card D-T1). These are the
// modules themselves, so every type keeps one identity across both paths.
#[cfg(windows)]
pub use pod_client::detach;
pub use pod_client::{client, endpoint, now_ms, transport};
pub mod env;
pub mod keys;

// The wire modules live in `pod-protocol` (docs/20-refactor/daemon.md §2,
// card D-F2). They stay reachable under their old paths, as the same modules:
// `podd::protocol::TaskInfo` is `pod_protocol::protocol::TaskInfo`.
pub use pod_protocol::{
    discovery_protocol, files_protocol, mobile_protocol, mods_protocol, orch_protocol,
    orch_run_protocol, pr_protocol, protocol, remote_protocol, session_protocol, status_protocol,
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

#[cfg(test)]
mod client_shim_tests {
    use std::any::TypeId;

    /// D-T1: the old client paths name the pod-client items themselves, not
    /// copies (a copy would be a second, incompatible type).
    #[test]
    fn old_client_paths_are_the_pod_client_types() {
        assert_eq!(
            TypeId::of::<crate::client::Client>(),
            TypeId::of::<pod_client::client::Client>()
        );
        assert_eq!(
            TypeId::of::<crate::transport::Transport>(),
            TypeId::of::<pod_client::transport::Transport>()
        );
        #[cfg(windows)]
        assert_eq!(
            TypeId::of::<crate::detach::DetachedChild>(),
            TypeId::of::<pod_client::detach::DetachedChild>()
        );
        assert_eq!(
            TypeId::of::<crate::protocol::TaskInfo>(),
            TypeId::of::<pod_client::protocol::TaskInfo>()
        );
    }
}
