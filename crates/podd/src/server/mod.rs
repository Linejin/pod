//! The daemon: terminal registry + JSON-RPC front end.
//!
//! Terminals live in `pod-holder` processes; the daemon starts holders,
//! connects to them (and re-connects to surviving ones on start), mirrors
//! their output for `read`/`wait`, and fans raw output out to attached clients.
//! Repositories and tasks (worktree + terminals) live in [`tasks`].
//!
//! This file keeps the configuration, the shared [`Daemon`] state and the
//! module list; the rest of the root is split by responsibility:
//! - `runtime`: bind, serve until `server.stop`, shut down;
//! - `connection`: one client connection (framing, stop check, hello,
//!   attach stream);
//! - `rpc`: the method dispatch;
//! - `terminal_ops`: terminal run, input, read/wait, resize, kill, rm;
//! - `rediscover`: re-connecting to holders that survived a previous daemon.

pub mod agent_launch;
pub mod codex_allowance;
pub mod discovery;
pub mod files;
pub mod gates;
pub mod groups;
pub mod lifecycle;
pub mod mobile;
pub mod mods;
pub mod orch_config;
pub mod orch_limits;
pub mod orch_meter;
pub mod orch_runs;
pub mod orch_store;
pub mod pr;
pub mod query_dedup;
pub mod remote;
pub mod repo_config;
pub mod retention;
pub mod review;
pub mod sessions;
pub mod status;
pub mod submit;
pub mod task_archive;
pub mod tasks;
pub mod terminal;

mod connection;
mod rediscover;
mod rpc;
mod runtime;
mod terminal_ops;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::endpoint;
use crate::winsec::UserOnly;
use tasks::Registry;
use terminal::Terminal;

// The root paths the rest of the crate and the domain modules use.
pub(crate) use connection::discard_input;
use connection::{MAX_LINE, err_response, notification, ok_response, write_json};
use rpc::{params, to_value};
pub use runtime::{bind, serve};
use terminal_ops::{exited_err, id_number};

pub struct ServerConfig {
    /// Full pipe path, e.g. `\\.\pipe\podd-alice`.
    pub pipe: String,
    /// Holder records (`<id>.json`), specs and logs.
    pub state_dir: PathBuf,
    /// The `pod-holder` executable.
    pub holder_exe: PathBuf,
    /// Task worktrees go to `<worktrees_root>\<repo>\<task>`.
    pub worktrees_root: PathBuf,
    /// Exited terminals (not kept, not in a task) are removed this long
    /// after they exited; `None` keeps them until `terminal.rm`.
    pub exited_retention: Option<Duration>,
    /// Queued review deliveries older than this expire (`failed`, error
    /// "expired: ..."); `None` keeps them queued until delivered or cancelled.
    pub review_queue_timeout: Option<Duration>,
    /// Per-client attach queue limit in bytes (`terminal.lagged` past it).
    pub attach_queue_limit: usize,
    /// Late-reply window for terminal query replies in ms (0 = off), v9.
    pub reply_window_ms: u64,
}

/// `POD_WORKTREES_ROOT`, else `%USERPROFILE%\pod\worktrees`, normalized
/// ([`crate::git::normalize`]: long names, on-disk case).
pub fn default_worktrees_root() -> PathBuf {
    let root = match std::env::var_os("POD_WORKTREES_ROOT").filter(|p| !p.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => std::env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("pod")
            .join("worktrees"),
    };
    crate::git::normalize(&root)
}

impl ServerConfig {
    /// Defaults: state under `%LOCALAPPDATA%\pod\state\<pipe>`, holder next to this exe.
    pub fn new(pipe: String) -> Self {
        let state_dir = endpoint::default_state_dir(&pipe);
        let holder_exe = std::env::var_os("POD_HOLDER")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::current_exe()
                    .unwrap_or_default()
                    .with_file_name("pod-holder.exe")
            });
        // `POD_RETAIN_EXITED` (e.g. `12h`, `off`), else 24 h.
        let exited_retention = std::env::var("POD_RETAIN_EXITED")
            .ok()
            .and_then(|v| retention::parse_duration(&v))
            .unwrap_or(Some(retention::DEFAULT_RETENTION));
        // `POD_REVIEW_QUEUE_TIMEOUT` (e.g. `10m`, `off`), else 30 min.
        let review_queue_timeout = std::env::var("POD_REVIEW_QUEUE_TIMEOUT")
            .ok()
            .and_then(|v| retention::parse_duration(&v))
            .unwrap_or(Some(review::DEFAULT_QUEUE_TIMEOUT));
        // `POD_ATTACH_QUEUE_LIMIT` in bytes, else 16 MiB.
        let attach_queue_limit = std::env::var("POD_ATTACH_QUEUE_LIMIT")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .filter(|&n| n > 0)
            .unwrap_or(terminal::DEFAULT_ATTACH_QUEUE_LIMIT);
        // `POD_REPLY_WINDOW_MS` (0 = off), else 500 ms.
        let reply_window_ms = std::env::var("POD_REPLY_WINDOW_MS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(query_dedup::DEFAULT_REPLY_WINDOW_MS);
        Self {
            pipe,
            state_dir,
            holder_exe,
            worktrees_root: default_worktrees_root(),
            exited_retention,
            review_queue_timeout,
            attach_queue_limit,
            reply_window_ms,
        }
    }
}

pub struct Daemon {
    config: ServerConfig,
    sa: UserOnly,
    started_ms: u64,
    started: Instant,
    next_id: AtomicU64,
    /// Ordered by id; exited terminals stay listed until `terminal.rm`.
    terminals: Mutex<Vec<Arc<Terminal>>>,
    /// Names reserved by `terminal.run` calls still starting their holder.
    pending_names: Mutex<HashSet<String>>,
    /// Holder records found at start whose holder did not answer: listed as
    /// `unreachable`, retried in the background (`lifecycle.rs`, daemon-03).
    unreachable: Mutex<Vec<lifecycle::Unreachable>>,
    /// `Some(kill_terminals)` once `server.stop` was called.
    shutdown: watch::Sender<Option<bool>>,
    /// Requests being answered right now; shutdown waits for them (bounded).
    in_flight: AtomicUsize,
    /// `terminal.run` calls between their stop check and the terminal being
    /// listed (or given up). `server.stop kill:true` waits for this to reach
    /// 0 before it collects the terminals to kill (see [`runtime::StartGuard`]).
    starting: Mutex<usize>,
    /// Repositories and tasks (persisted in the state dir).
    registry: Mutex<Registry>,
    /// Serializes task create/remove (they span git commands and terminal starts).
    task_ops: tokio::sync::Mutex<()>,
    /// Review comments, deliveries, groups and gates (persisted in the state dir).
    orch: Mutex<orch_store::Store>,
    /// Runs, items, attempts and mail (docs/19, `orch_runs.rs`).
    runs: orch_runs::OrchRuns,
    /// Wakes the review delivery worker (a prompt was queued).
    review_wake: tokio::sync::Notify,
    /// Terminals exempt from the exited-terminal retention (`terminal.keep`).
    kept: Mutex<HashSet<String>>,
    /// Wakes the gate expiry worker (a gate with an expiry was created).
    gate_wake: tokio::sync::Notify,
    /// Running `group.check` jobs: group id -> cancel flag.
    check_jobs: Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
    /// GitHub through `gh`, or the fake forge (`POD_FORGE=fake:<file>`), docs/15.
    forge: Arc<dyn pod_forge::Forge>,
    /// v12: persistent id of this daemon's machine (state dir) and the id of
    /// this start (docs/17 R2).
    machine_id: String,
    boot_id: String,
    /// Serializes `task.pr.create` / `task.pr.merge` (pushes and PR writes).
    pr_ops: tokio::sync::Mutex<()>,
    /// Hook session ids of terminals no task owns yet (`sessions.rs`).
    /// Lock order: `registry` before this.
    early_sessions: Mutex<sessions::EarlySessions>,
    /// Fired when a session id waits in `early_sessions`.
    early_wake: tokio::sync::Notify,
    /// Paired phones, their sessions, policy and audit (docs/17 M1, no listener).
    mobile: mobile::Mobile,
    /// v22 remote files (docs/17 R5): the save journal and per-file locks.
    files: files::Files,
}

/// Timestamped line on stderr (redirected to the log file by `podd --log`).
pub fn log(msg: &str) {
    eprintln!("[podd {}] {msg}", crate::now_ms());
}
