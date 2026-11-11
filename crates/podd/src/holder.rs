//! `pod-holder`: owns one terminal (ConPTY + Job Object) independently of the
//! daemon, so terminals survive `podd` stopping, crashing or restarting.
//!
//! Started detached by the daemon as `pod-holder --spec <file>`. It keeps an
//! authoritative VT state (to answer terminal queries and to give a
//! reconnecting daemon an exact snapshot) and serves the frame protocol in
//! [`crate::holder_proto`] on its own user-only named pipe. It exits only on
//! `Release` (after the child exited), which closes the Job Object.
//!
//! This file is the facade: the entry point and the public limits.
//!
//! - `process`: the PTY child, VT state, record/log files, output reader
//!   and exit watcher;
//! - `budget`: output queued to the connected daemon ([`OUTPUT_BUDGET`]);
//! - `input`: the child's input queue and writer, input acks, the reply
//!   budget ([`INPUT_HARD_CAP`], [`REPLY_BYTE_CAP`], [`REPLY_COUNT_CAP`]);
//! - `connection`: the pipe server and the frames of one daemon.

mod budget;
mod connection;
mod input;
mod process;

use std::path::PathBuf;

use crate::holder_proto::HolderSpec;

/// Largest raw output chunk read from the PTY at once (one `terminal.output`
/// notification carries at most this much before base64).
pub const OUTPUT_CHUNK: usize = 64 * 1024;

pub use crate::holder_proto::HOLDER_PROTOCOL;

/// Output bytes queued to the connected daemon and not yet written past
/// which the PTY reader waits (so the child blocks on its own output instead
/// of the holder buffering without limit). Control frames never wait.
pub const OUTPUT_BUDGET: usize = 8 * 1024 * 1024;
/// Input bytes waiting for the PTY past which daemon input is dropped
/// (logged, and still acknowledged). Daemons >= holder protocol 3 keep their
/// input in flight far below this through `InputAck`; this guards against
/// older daemons. Replies to terminal queries have their own budget below.
pub const INPUT_HARD_CAP: usize = 16 * 1024 * 1024;
/// Replies to terminal queries (VT/OSC answers the holder writes itself)
/// waiting for the PTY: past this many bytes or items new replies are
/// dropped (logged once per burst). A child that keeps asking without
/// reading its input otherwise grows the holder without limit.
pub const REPLY_BYTE_CAP: usize = 256 * 1024;
pub const REPLY_COUNT_CAP: usize = 4096;

/// Entry point of the `pod-holder` binary. Returns the process exit code.
pub fn main() -> i32 {
    let args: Vec<String> = std::env::args().collect();
    let spec_path = match args
        .iter()
        .position(|a| a == "--spec")
        .and_then(|i| args.get(i + 1))
    {
        Some(p) => PathBuf::from(p),
        None => {
            eprintln!("usage: pod-holder --spec <file>   (started by podd)");
            return 2;
        }
    };
    let spec: HolderSpec = match std::fs::read(&spec_path)
        .map_err(|e| e.to_string())
        .and_then(|b| serde_json::from_slice(&b).map_err(|e| e.to_string()))
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("pod-holder: cannot read spec {}: {e}", spec_path.display());
            return 2;
        }
    };
    let _ = std::fs::remove_file(&spec_path);
    process::run(spec)
}
