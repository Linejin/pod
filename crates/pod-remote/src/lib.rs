//! Remote machines (docs/17 §2, R3): SSH machine profiles stored in the Pod
//! config and a transport that runs the **system** OpenSSH client with a
//! Pod-generated minimal config and fixed safety options, then speaks the
//! normal NDJSON JSON-RPC to `pod remote-stdio` on the other side.
//!
//! No UI here: `pod machine …` and `pod --machine <name> …` use it; the app
//! will later. [`session`] (R4) keeps remote terminals alive over bad
//! networks: reconnect with backoff, re-attach with gap markers, input
//! whose lost answer is resolved by asking, never by resending.

pub mod machine;
pub mod session;
pub mod ssh;

pub use machine::{Machine, MachineStore};
pub use session::{
    AttachOptions, AttachSlots, AttachUpdate, Closer, ConnectError, Connector, Gap,
    InputResolution, Link, ReconnectPolicy, Reconnected, RemoteAttach, RemoteControl, RemoteError,
    Resync, ResyncReason, SshConnector,
};
pub use ssh::{ErrorKind, SshConnection, SshError, connect};

/// [`podd::client::Client::pipe`] label of an SSH connection: `ssh:<machine>`.
/// Never a local pipe path, so local-only logic (the caller's own terminal,
/// pipe reconnects) does not apply to it.
pub const LABEL_PREFIX: &str = "ssh:";

/// Is this client label an SSH connection?
pub fn is_remote_label(label: &str) -> bool {
    label.starts_with(LABEL_PREFIX)
}
