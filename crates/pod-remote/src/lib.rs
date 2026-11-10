//! Remote machines (docs/17 §2, R3): SSH machine profiles stored in the Pod
//! config and a transport that runs the **system** OpenSSH client with a
//! Pod-generated minimal config and fixed safety options, then speaks the
//! normal NDJSON JSON-RPC to `pod remote-stdio` on the other side.
//!
//! No UI here: `pod machine …` and `pod --machine <name> …` use it; the app
//! will later.

pub mod machine;
pub mod ssh;

pub use machine::{Machine, MachineStore};
pub use ssh::{ErrorKind, SshConnection, SshError, connect};

/// [`podd::client::Client::pipe`] label of an SSH connection: `ssh:<machine>`.
/// Never a local pipe path, so local-only logic (the caller's own terminal,
/// pipe reconnects) does not apply to it.
pub const LABEL_PREFIX: &str = "ssh:";

/// Is this client label an SSH connection?
pub fn is_remote_label(label: &str) -> bool {
    label.starts_with(LABEL_PREFIX)
}
