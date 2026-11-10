//! The daemon: terminal registry + JSON-RPC front end.
//!
//! Terminals live in `pod-holder` processes; the daemon starts holders,
//! connects to them (and re-connects to surviving ones on start), mirrors
//! their output for `read`/`wait`, and fans raw output out to attached clients.
//! Repositories and tasks (worktree + terminals) live in [`tasks`].

pub mod agent_launch;
pub mod codex_allowance;
pub mod discovery;
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

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::watch;

use crate::endpoint::{self, ENV_SOCKET, ENV_TERMINAL_ID};
use crate::holder_proto::{Frame, HolderRecord, HolderSpec};
use crate::protocol::*;
use crate::winsec::UserOnly;
use tasks::Registry;
use terminal::{StreamMsg, Terminal};

const ERROR_ACCESS_DENIED: i32 = 5;
/// Requests larger than this are rejected (protects the line reader).
const MAX_LINE: usize = 4 * 1024 * 1024;
// pod-hook caps its lines to this (daemon-05); a larger hook line is dropped.
const _: () = assert!(MAX_LINE >= pod_status::hook::MAX_HOOK_LINE);
const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 30;
const DEFAULT_ATTACH_SCROLLBACK: usize = 1000;

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

/// `POD_WORKTREES_ROOT`, else `%USERPROFILE%\pod\worktrees`.
pub fn default_worktrees_root() -> PathBuf {
    if let Some(p) = std::env::var_os("POD_WORKTREES_ROOT").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("pod")
        .join("worktrees")
}

/// This daemon's folder when the `pod` CLI sits next to it (installed,
/// portable and dev builds ship them together), else `None`.
fn pod_cli_dir() -> Option<String> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    dir.join("pod.exe")
        .is_file()
        .then(|| dir.display().to_string())
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
    /// 0 before it collects the terminals to kill (see [`StartGuard`]).
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
}

/// Create the first pipe instance. `first_pipe_instance` makes creation fail
/// when another process already owns this pipe name: that is the
/// single-daemon lock (atomic, released by the OS when we die). The DACL
/// grants access to the current user only.
pub fn bind(config: &ServerConfig) -> anyhow::Result<(NamedPipeServer, UserOnly)> {
    let sa = UserOnly::new()?;
    // SAFETY: `sa` outlives the call.
    let res = unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(&config.pipe, sa.as_ptr())
    };
    match res {
        Ok(s) => Ok((s, sa)),
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
            anyhow::bail!("another podd already owns {} (lock held)", config.pipe)
        }
        Err(e) => Err(anyhow::anyhow!("cannot create pipe {}: {e}", config.pipe)),
    }
}

/// Serve until `server.stop`.
pub async fn serve(
    config: ServerConfig,
    first: NamedPipeServer,
    sa: UserOnly,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.state_dir)?;
    terminal::set_attach_queue_limit(config.attach_queue_limit);
    query_dedup::set_reply_window_ms(config.reply_window_ms);
    let (shutdown, mut shutdown_rx) = watch::channel(None);
    let registry = Registry::load(&config.state_dir);
    let orch = orch_store::Store::load(&config.state_dir);
    let runs = orch_runs::OrchRuns::load(&config.state_dir);
    let machine_id = remote::load_or_create_machine_id(&config.state_dir);
    let mobile = mobile::Mobile::load(&config.state_dir, mobile::session::system_clock());
    let daemon = Arc::new(Daemon {
        config,
        sa,
        started_ms: crate::now_ms(),
        started: Instant::now(),
        next_id: AtomicU64::new(1),
        terminals: Mutex::new(Vec::new()),
        pending_names: Mutex::new(HashSet::new()),
        unreachable: Mutex::new(Vec::new()),
        shutdown,
        in_flight: AtomicUsize::new(0),
        starting: Mutex::new(0),
        registry: Mutex::new(registry),
        task_ops: tokio::sync::Mutex::new(()),
        orch: Mutex::new(orch),
        runs,
        review_wake: tokio::sync::Notify::new(),
        kept: Mutex::new(HashSet::new()),
        gate_wake: tokio::sync::Notify::new(),
        check_jobs: Mutex::new(Default::default()),
        forge: Arc::from(pod_forge::from_env()),
        pr_ops: tokio::sync::Mutex::new(()),
        early_sessions: Mutex::new(Default::default()),
        early_wake: tokio::sync::Notify::new(),
        machine_id,
        boot_id: remote::new_boot_id(),
        mobile,
    });
    log(&format!(
        "listening on {} (pid {}, state {})",
        daemon.config.pipe,
        std::process::id(),
        daemon.config.state_dir.display()
    ));
    status::set_state_dir(&daemon.config.state_dir);
    daemon.rediscover().await;
    lifecycle::start_holder_retry(&daemon);
    daemon.load_kept();
    daemon.interrupt_stale_check_jobs();
    status::start(&daemon);
    review::start(&daemon);
    gates::start(&daemon);
    retention::start(&daemon);
    lifecycle::start(&daemon);
    sessions::start(&daemon);
    pr::start(&daemon);
    mods::start(&daemon);
    orch_runs::start(&daemon);
    discovery::start(&daemon);
    {
        let reg = daemon.registry.lock().unwrap();
        // Never hand out an id a task still refers to.
        daemon
            .next_id
            .fetch_max(reg.max_terminal_number() + 1, Ordering::Relaxed);
        log(&format!(
            "{} repo(s), {} task(s); worktrees under {}",
            reg.repos.len(),
            reg.tasks.len(),
            daemon.config.worktrees_root.display()
        ));
    }

    let mut server = first;
    loop {
        tokio::select! {
            res = server.connect() => {
                res?;
                // Always keep one instance listening before handing this one off.
                // SAFETY: `daemon.sa` outlives the call.
                let next = unsafe {
                    ServerOptions::new()
                        .reject_remote_clients(true)
                        .create_with_security_attributes_raw(&daemon.config.pipe, daemon.sa.as_ptr())?
                };
                let conn = std::mem::replace(&mut server, next);
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_connection(daemon, conn).await {
                        log(&format!("connection error: {e}"));
                    }
                });
            }
            _ = shutdown_rx.changed() => break,
        }
    }

    // Stop taking connections, but keep the pipe name until this process
    // exits: a client connecting now waits (all instances busy) and then
    // finds no pipe, so it starts / reaches the *next* daemon instead of an
    // instance nobody serves. The name also keeps a new daemon from binding
    // while this one still talks to the holders.
    // Both handles are leaked on purpose: they must outlive `serve` (main
    // exits right after it returns, and the OS closes them then).
    if let Some(plug) = plug_listener(&daemon, server).await {
        std::mem::forget(plug);
    }

    let kill = shutdown_rx.borrow().unwrap_or(false);
    if kill {
        // Terminals still starting are either listed before this returns or
        // end themselves (`run` checks the stop after listing): one rule for
        // every terminal created before the stop.
        let deadline = Instant::now() + START_GRACE;
        while *daemon.starting.lock().unwrap() > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let terms: Vec<Arc<Terminal>> = daemon.terminals.lock().unwrap().clone();
        for t in &terms {
            t.send_to_holder(Frame::Kill);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while terms.iter().any(|t| t.is_running()) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        for t in &terms {
            t.send_to_holder(Frame::Release);
        }
        log(&format!(
            "stopping; killed and released {} terminal(s)",
            terms.len()
        ));
    } else {
        log("stopping; terminals stay alive in their holders");
    }
    // Requests accepted before the stop (including the stop's own reply)
    // finish; long waits are cut off and their clients reconnect.
    let deadline = Instant::now() + IN_FLIGHT_GRACE;
    while daemon.in_flight.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Let the last responses and frames go out.
    tokio::time::sleep(Duration::from_millis(200)).await;
    log("stopped");
    Ok(())
}

/// How long shutdown waits for requests that were already being answered.
const IN_FLIGHT_GRACE: Duration = Duration::from_secs(3);
/// How long `server.stop kill:true` waits for terminals still starting
/// (longer than a holder connect, see [`terminal::CONNECT_TIMEOUT`]).
const START_GRACE: Duration = Duration::from_secs(20);
/// How long the start waits for one surviving holder (they are reached
/// concurrently).
const REDISCOVER_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `terminal.run` waits for its new holder to serve its pipe.
const HOLDER_START_TIMEOUT: Duration = Duration::from_secs(15);

/// Occupy the last listening instance with a client handle of our own so no
/// other client can connect to it. Returns the instance and the handle.
async fn plug_listener(
    daemon: &Arc<Daemon>,
    mut server: NamedPipeServer,
) -> Option<(NamedPipeServer, std::fs::File)> {
    for _ in 0..8 {
        match crate::transport::open_pipe(&daemon.config.pipe) {
            Ok(f) => return Some((server, f)),
            Err(_) => {
                // A client took the instance just before the stop: serve it
                // (every request gets SERVER_STOPPING), then plug a new one.
                if tokio::time::timeout(Duration::from_millis(500), server.connect())
                    .await
                    .is_err()
                {
                    return None;
                }
                // SAFETY: `daemon.sa` outlives the call.
                let next = unsafe {
                    ServerOptions::new()
                        .reject_remote_clients(true)
                        .create_with_security_attributes_raw(
                            &daemon.config.pipe,
                            daemon.sa.as_ptr(),
                        )
                };
                let Ok(next) = next else { return None };
                let conn = std::mem::replace(&mut server, next);
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    let _ = serve_connection(daemon, conn).await;
                });
            }
        }
    }
    None
}

/// One `terminal.run` between its stop check and the end of its start.
struct StartGuard<'a>(&'a Daemon);

impl Drop for StartGuard<'_> {
    fn drop(&mut self) {
        *self.0.starting.lock().unwrap() -= 1;
    }
}

/// Counts a request as in flight until dropped.
struct InFlight<'a>(&'a Daemon);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Daemon {
    fn begin_request(&self) -> InFlight<'_> {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlight(self)
    }

    /// `server.stop` was called.
    fn is_stopping(&self) -> bool {
        self.shutdown.borrow().is_some()
    }

    /// `server.stop kill:true` was called.
    fn is_stopping_with_kill(&self) -> bool {
        *self.shutdown.borrow() == Some(true)
    }

    /// Register a terminal start, or refuse it once the stop was called. The
    /// check and the count share a lock with the shutdown's wait, so a start
    /// is either waited for or sees the stop.
    fn begin_start(&self) -> Result<StartGuard<'_>, RpcError> {
        let mut n = self.starting.lock().unwrap();
        if self.is_stopping() {
            return Err(stopping_err());
        }
        *n += 1;
        Ok(StartGuard(self))
    }
}

fn stopping_err() -> RpcError {
    RpcError::new(
        codes::SERVER_STOPPING,
        "podd is stopping (server.stop); nothing was done, retry once it has exited",
    )
}

tokio::task_local! {
    /// Protocol chosen by `connection.hello` on the connection this task
    /// serves; none = this build's (local clients ship with the daemon and
    /// send no hello). [`write_json`] downgrades every message it writes
    /// there, so responses and stream notifications alike match it.
    static CONN_PROTOCOL: std::cell::Cell<Option<u32>>;
}

async fn serve_connection(daemon: Arc<Daemon>, conn: NamedPipeServer) -> anyhow::Result<()> {
    CONN_PROTOCOL
        .scope(std::cell::Cell::new(None), handle_connection(daemon, conn))
        .await
}

async fn handle_connection(daemon: Arc<Daemon>, conn: NamedPipeServer) -> anyhow::Result<()> {
    let (read, mut write) = tokio::io::split(conn);
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        let n = read_line_limited(&mut reader, &mut line, MAX_LINE).await?;
        if n == 0 {
            return Ok(());
        }
        if line.trim().is_empty() {
            continue;
        }
        if status::try_ingest_hook(&daemon, &line) {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = err_response(
                    Value::Null,
                    RpcError::new(codes::PARSE_ERROR, format!("parse error: {e}")),
                );
                write_json(&mut write, &resp).await?;
                continue;
            }
        };
        // Counted before the stop check: either the stop sees this request
        // and waits for it, or this request sees the stop and is refused.
        let guard = daemon.begin_request();
        if daemon.is_stopping() {
            if let Some(id) = req.id {
                write_json(&mut write, &err_response(id, stopping_err())).await?;
            }
            continue;
        }
        if req.method == TerminalAttach::NAME
            && let Some(id) = req.id.clone()
        {
            drop(guard);
            // The connection becomes an output stream until either side closes it.
            return daemon.stream_attach(id, req.params, reader, write).await;
        }
        if req.method == StatusSubscribe::NAME
            && let Some(id) = req.id.clone()
        {
            drop(guard);
            return status::stream_subscribe(daemon, id, req.params, reader, write).await;
        }
        let method = req.method.clone();
        let Some(resp) = daemon.clone().handle_request(req).await else {
            continue;
        };
        if method == ConnectionHello::NAME
            && let Some(v) = resp
                .result
                .as_ref()
                .and_then(|r| r.get("protocol"))
                .and_then(Value::as_u64)
        {
            let _ = CONN_PROTOCOL.try_with(|c| c.set(Some(v as u32)));
        }
        write_json(&mut write, &resp).await?;
        drop(guard);
    }
}

/// `read_line` that refuses a line longer than `max` bytes (without the
/// `\n`) **before** buffering it, so a peer that never sends a newline cannot
/// make the daemon allocate more than `max`. `buf` is cleared first and gets
/// the line with its `\n`; returns the bytes read (0 at EOF). A line cut by
/// EOF is returned as is (the JSON parse then fails), like `read_line`.
pub(crate) async fn read_line_limited<R: AsyncBufRead + Unpin>(
    r: &mut R,
    buf: &mut String,
    max: usize,
) -> anyhow::Result<usize> {
    buf.clear();
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let avail = r.fill_buf().await?;
        if avail.is_empty() {
            break;
        }
        let (take, done) = match avail.iter().position(|&b| b == b'\n') {
            Some(i) => (i + 1, true),
            None => (avail.len(), false),
        };
        if bytes.len() + take - usize::from(done) > max {
            anyhow::bail!("request line longer than {max} bytes");
        }
        bytes.extend_from_slice(&avail[..take]);
        r.consume(take);
        if done {
            break;
        }
    }
    let n = bytes.len();
    *buf = String::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "request line is not UTF-8")
    })?;
    Ok(n)
}

/// Read and drop whatever the client sends on a streaming connection
/// (attach, subscribe) without buffering it. Returns the bytes dropped, 0 at
/// EOF. Cancel-safe (used in `select!`).
pub(crate) async fn discard_input<R: AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<usize> {
    let n = r.fill_buf().await?.len();
    r.consume(n);
    Ok(n)
}

async fn write_json<W: AsyncWriteExt + Unpin, T: Serialize>(
    w: &mut W,
    v: &T,
) -> anyhow::Result<()> {
    // ui-editor-03: one place for every message (any response, attach and
    // subscribe notifications), not a per-method list.
    let protocol = CONN_PROTOCOL.try_with(|c| c.get()).ok().flatten();
    let mut out = match protocol {
        Some(p) if p < UNREACHABLE_PROTOCOL => {
            let mut value = serde_json::to_value(v)?;
            downgrade_json(&mut value, p);
            serde_json::to_string(&value)?
        }
        _ => serde_json::to_string(v)?,
    };
    out.push('\n');
    w.write_all(out.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

fn ok_response(id: Value, result: Value) -> Response {
    Response {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        result: Some(result),
        error: None,
    }
}

fn err_response(id: Value, error: RpcError) -> Response {
    Response {
        jsonrpc: JSONRPC_VERSION.into(),
        id,
        result: None,
        error: Some(error),
    }
}

fn notification<T: Serialize>(method: &str, params: T) -> Notification {
    Notification {
        jsonrpc: JSONRPC_VERSION.into(),
        method: method.into(),
        params: serde_json::to_value(params).unwrap_or(Value::Null),
    }
}

fn params<P: DeserializeOwned>(v: Value) -> Result<P, RpcError> {
    let v = if v.is_null() {
        Value::Object(Default::default())
    } else {
        v
    };
    serde_json::from_value(v)
        .map_err(|e| RpcError::new(codes::INVALID_PARAMS, format!("invalid params: {e}")))
}

fn to_value<T: Serialize>(t: T) -> Result<Value, RpcError> {
    serde_json::to_value(t).map_err(|e| RpcError::new(codes::INTERNAL_ERROR, e.to_string()))
}

/// Input that did not reach the holder (daemon-01).
fn input_err(t: &Terminal, e: terminal::InputRefused) -> RpcError {
    match e {
        terminal::InputRefused::Backlog { in_flight } => RpcError::new(
            codes::INPUT_BACKLOG,
            format!(
                "terminal {} has not read {in_flight} bytes of earlier input yet; nothing was sent, retry later",
                t.id
            ),
        ),
        terminal::InputRefused::Gone => exited_err(t),
    }
}

fn exited_err(t: &Terminal) -> RpcError {
    RpcError::new(
        codes::TERMINAL_EXITED,
        format!("terminal {} has exited", t.id),
    )
}

impl Daemon {
    /// Returns `None` for notifications.
    async fn handle_request(self: Arc<Self>, req: Request) -> Option<Response> {
        let id = req.id.clone();
        if req.jsonrpc != JSONRPC_VERSION {
            return Some(err_response(
                id.unwrap_or(Value::Null),
                RpcError::new(codes::INVALID_REQUEST, "jsonrpc must be \"2.0\""),
            ));
        }
        let result = self.dispatch(&req.method, req.params).await;
        let id = id?;
        Some(match result {
            Ok(v) => ok_response(id, v),
            Err(e) => err_response(id, e),
        })
    }

    async fn dispatch(self: Arc<Self>, method: &str, p: Value) -> Result<Value, RpcError> {
        match method {
            ServerStatus::NAME => {
                let _: Empty = params(p)?;
                to_value(self.status())
            }
            ServerStop::NAME => to_value(self.stop(params(p)?)),
            ConnectionHello::NAME => self.hello(params(p)?),
            TerminalRun::NAME => to_value(self.run_in_task(params(p)?).await?),
            TerminalList::NAME => {
                let _: Empty = params(p)?;
                let mut terminals = self.list();
                terminals.extend(self.unreachable_infos());
                terminals.sort_by_key(|t| id_number(&t.id));
                let retention = terminals
                    .iter()
                    .map(|t| self.retention_of(t))
                    .filter(|r| r.kept || r.expires_ms.is_some())
                    .collect();
                to_value(ListResult {
                    terminals,
                    retention_ms: self.config.exited_retention.map(|d| d.as_millis() as u64),
                    retention,
                })
            }
            TerminalSend::NAME => to_value(self.send(params(p)?).await?),
            TerminalKeys::NAME => to_value(self.keys(params(p)?)?),
            TerminalInput::NAME => to_value(self.input(params(p)?)?),
            TerminalRead::NAME => to_value(self.read(params(p)?)?),
            TerminalWait::NAME => to_value(self.wait(params(p)?).await?),
            TerminalResize::NAME => to_value(self.resize(params(p)?).await?),
            TerminalKill::NAME => to_value(self.kill(params(p)?).await?),
            TerminalRm::NAME => to_value(self.rm(params(p)?).await?),
            TerminalKeep::NAME => to_value(self.keep(params(p)?)?),
            TerminalAttach::NAME => Err(RpcError::new(
                codes::INVALID_REQUEST,
                "terminal.attach must be a request with an id",
            )),
            m if status::handles(m) => status::dispatch(self, m, p).await,
            m if mods::handles(m) => mods::dispatch(m, p).await,
            m if orch_runs::handles(m) => orch_runs::dispatch(self, m, p).await,
            m if discovery::handles(m) => discovery::dispatch(self.clone(), m, p).await,
            m if mobile::handles(m) => mobile::dispatch(self, m, p).await,
            RepoAdd::NAME => to_value(self.repo_add(params(p)?).await?),
            RepoList::NAME => {
                let _: Empty = params(p)?;
                to_value(self.repo_list().await)
            }
            RepoRemove::NAME => to_value(self.repo_remove(params(p)?)?),
            TaskCreate::NAME => to_value(self.task_create(params(p)?).await?),
            TaskList::NAME => to_value(self.task_list(params(p)?).await?),
            TaskShow::NAME => to_value(self.task_show(params(p)?).await?),
            TaskDiff::NAME => to_value(self.task_diff(params(p)?).await?),
            TaskRemove::NAME => to_value(self.task_remove(params(p)?).await?),
            TaskTouch::NAME => to_value(self.task_touch(params(p)?).await?),
            TaskArchive::NAME => to_value(self.task_archive(params(p)?).await?),
            TaskUnarchive::NAME => to_value(self.task_unarchive(params(p)?).await?),
            TaskResume::NAME => to_value(self.task_resume(params(p)?).await?),
            TaskCleanupCandidates::NAME => {
                to_value(self.task_cleanup_candidates(params(p)?).await?)
            }
            TaskSuggestName::NAME => to_value(self.task_suggest_name(params(p)?).await?),
            RepoConfigMethod::NAME => to_value(self.repo_config(params(p)?).await?),
            RepoSet::NAME => to_value(self.repo_set(params(p)?).await?),
            TaskPrStatusMethod::NAME => to_value(self.task_pr_status(params(p)?).await?),
            TaskPrCreate::NAME => to_value(self.task_pr_create(params(p)?).await?),
            TaskPrMerge::NAME => to_value(self.task_pr_merge(params(p)?).await?),
            ReviewCommentsAdd::NAME => to_value(self.review_add(params(p)?).await?),
            ReviewCommentsList::NAME => to_value(self.review_list(params(p)?)?),
            ReviewCommentsResolve::NAME => to_value(self.review_resolve(params(p)?)?),
            ReviewCommentsRemove::NAME => to_value(self.review_remove(params(p)?)?),
            ReviewSend::NAME => to_value(self.review_send(params(p)?).await?),
            ReviewDeliveries::NAME => to_value(self.review_deliveries(params(p)?)?),
            ReviewCancel::NAME => to_value(self.review_cancel(params(p)?)?),
            GroupCreate::NAME => to_value(self.group_create(params(p)?).await?),
            GroupList::NAME => to_value(self.group_list(params(p)?).await?),
            GroupShow::NAME => to_value(self.group_show(params(p)?).await?),
            GroupCompare::NAME => to_value(self.group_compare(params(p)?).await?),
            GroupCheck::NAME => to_value(self.group_check(params(p)?).await?),
            GroupCheckCancel::NAME => to_value(self.group_check_cancel(params(p)?).await?),
            GroupPick::NAME => to_value(self.group_pick(params(p)?).await?),
            GroupRemove::NAME => to_value(self.group_remove(params(p)?)?),
            GateCreate::NAME => to_value(self.gate_create(params(p)?)?),
            GateResolve::NAME => to_value(self.gate_resolve(params(p)?)?),
            GateList::NAME => to_value(self.gate_list(params(p)?)?),
            GateShow::NAME => to_value(self.gate_show(params(p)?)?),
            GateWait::NAME => to_value(self.gate_wait(params(p)?).await?),
            other => Err(RpcError::new(
                codes::METHOD_NOT_FOUND,
                format!("unknown method {other:?}"),
            )),
        }
    }

    /// Re-connect to holders that survived a previous daemon.
    ///
    /// Every `t<n>.json` record reserves its id, whether or not its holder
    /// answers: a holder that is alive but unreachable right now (busy,
    /// slow, access error) keeps its id, pipe name, token and state files,
    /// and no new terminal is given that id (daemon-03). Holders are reached
    /// concurrently, each within [`REDISCOVER_TIMEOUT`], so one stuck holder
    /// cannot hold up the start (daemon-04).
    async fn rediscover(&self) {
        let Ok(entries) = std::fs::read_dir(&self.config.state_dir) else {
            return;
        };
        let mut reserved = 0;
        let mut pending = Vec::new();
        let mut lost = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some("json")
                || name.ends_with(".spec.json")
            {
                continue;
            }
            if let Some(stem) = name.strip_suffix(".json")
                && is_terminal_id(stem)
            {
                reserved = reserved.max(id_number(stem));
            }
            let rec = match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| serde_json::from_slice::<HolderRecord>(&b).map_err(|e| e.to_string()))
            {
                Ok(rec) => rec,
                Err(e) => {
                    // Listed (and re-read on retry) so `pod kill` can forget it.
                    if let Some(stem) = name.strip_suffix(".json")
                        && is_terminal_id(stem)
                    {
                        lost.push(lifecycle::Unreachable::new(
                            stem.to_string(),
                            path.clone(),
                            None,
                            format!("record unreadable: {e}"),
                        ));
                    }
                    continue;
                }
            };
            if is_terminal_id(&rec.id) {
                reserved = reserved.max(id_number(&rec.id));
            }
            let pipe = rec.pipe.clone();
            let connect =
                tokio::spawn(
                    async move { Terminal::connect_within(&pipe, REDISCOVER_TIMEOUT).await },
                );
            pending.push((rec, path, connect));
        }
        let mut found = Vec::new();
        for (rec, path, connect) in pending {
            let res = connect
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e)));
            match res {
                Ok(t) => {
                    log(&format!(
                        "re-attached {} (holder pid {}, {})",
                        t.id,
                        t.holder_pid,
                        state_word(&t.info())
                    ));
                    found.push(t);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    log(&format!(
                        "removing stale record {} ({})",
                        rec.id,
                        path.display()
                    ));
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_file(path.with_extension("log"));
                    status::remove_token(&self.config.state_dir, &rec.id);
                }
                Err(e) => {
                    log(&format!(
                        "cannot re-attach {} (holder pid {}): {e}; listed as unreachable, \
                         its id stays reserved",
                        rec.id, rec.holder_pid
                    ));
                    let id = rec.id.clone();
                    lost.push(lifecycle::Unreachable::new(
                        id,
                        path,
                        Some(rec),
                        e.to_string(),
                    ));
                }
            }
        }
        // One entry per id (a holder that answered wins).
        lost.retain(|u| !found.iter().any(|t| t.id == u.id));
        lost.sort_by_key(|u| id_number(&u.id));
        lost.dedup_by(|a, b| a.id == b.id);
        *self.unreachable.lock().unwrap() = lost;
        found.sort_by_key(|t| id_number(&t.id));
        let max = found
            .iter()
            .map(|t| id_number(&t.id))
            .max()
            .unwrap_or(0)
            .max(reserved);
        self.next_id.fetch_max(max + 1, Ordering::Relaxed);
        *self.terminals.lock().unwrap() = found;
    }

    fn status(&self) -> ServerStatusResult {
        let terms = self.terminals.lock().unwrap();
        ServerStatusResult {
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
            pipe: self.config.pipe.clone(),
            started_ms: self.started_ms,
            uptime_ms: self.started.elapsed().as_millis() as u64,
            terminals: terms.len(),
            running: terms.iter().filter(|t| t.is_running()).count(),
            state_dir: self.config.state_dir.display().to_string(),
        }
    }

    fn stop(&self, p: StopParams) -> ServerStopResult {
        let running = self
            .terminals
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.is_running())
            .count();
        // Reply first; the accept loop stops right after.
        let _ = self.shutdown.send(Some(p.kill));
        ServerStopResult {
            stopping: true,
            killed: if p.kill { running } else { 0 },
            detached: if p.kill { 0 } else { running },
        }
    }

    fn list(&self) -> Vec<TerminalInfo> {
        self.terminals
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.info())
            .collect()
    }

    /// By id: exact. By name: the running terminal with that name, else the
    /// most recently created exited one.
    fn resolve(&self, r: &TerminalRef) -> Result<Arc<Terminal>, RpcError> {
        let terms = self.terminals.lock().unwrap();
        let found = match (&r.id, &r.name) {
            (Some(id), _) => terms.iter().find(|t| &t.id == id).cloned(),
            (None, Some(name)) => terms
                .iter()
                // max_by_key keeps the last maximum: the newest running one, else the newest.
                .filter(|t| t.name.as_deref() == Some(name))
                .max_by_key(|t| t.is_running())
                .cloned(),
            (None, None) => {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    "either id or name is required",
                ));
            }
        };
        drop(terms);
        found.ok_or_else(|| {
            let msg = match self.unreachable_reason(r) {
                Some((id, why)) => format!(
                    "the holder of terminal {id} is unreachable ({why}); podd keeps retrying, \
                     `pod kill {id}` forgets it"
                ),
                None => format!("no terminal {r}"),
            };
            RpcError::new(codes::TERMINAL_NOT_FOUND, msg)
        })
    }

    fn running(&self, r: &TerminalRef) -> Result<Arc<Terminal>, RpcError> {
        let t = self.resolve(r)?;
        if !t.is_running() {
            return Err(exited_err(&t));
        }
        Ok(t)
    }

    /// Clean base environment + our variables + per-run overrides (see `env.rs`).
    fn terminal_env(
        &self,
        id: &str,
        overrides: BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let base = crate::winsec::user_environment().unwrap_or_else(|e| {
            log(&format!(
                "user environment unavailable ({e}); using the daemon's"
            ));
            std::env::vars().collect()
        });
        let mut env = crate::env::sanitize(base);
        crate::env::set_var(&mut env, "TERM", "xterm-256color");
        crate::env::set_var(&mut env, "COLORTERM", "truecolor");
        crate::env::set_var(&mut env, ENV_TERMINAL_ID, id);
        crate::env::set_var(&mut env, ENV_SOCKET, self.config.pipe.as_str());
        if let Some(dir) = pod_cli_dir() {
            crate::env::prepend_path(&mut env, &dir);
        }
        for (k, v) in overrides {
            crate::env::set_var(&mut env, &k, v);
        }
        env
    }

    async fn run(&self, p: RunParams) -> Result<TerminalInfo, RpcError> {
        if p.argv.is_empty() || p.argv[0].is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "argv must not be empty",
            ));
        }
        if let Some(name) = &p.name
            && (name.is_empty() || is_terminal_id(name))
        {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("invalid name {name:?} (must not look like an id)"),
            ));
        }
        let cwd = match &p.cwd {
            Some(c) => PathBuf::from(c),
            None => std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
        };
        if !cwd.is_dir() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("cwd {} is not a directory", cwd.display()),
            ));
        }
        let cols = p.cols.unwrap_or(DEFAULT_COLS).clamp(2, 1000);
        let rows = p.rows.unwrap_or(DEFAULT_ROWS).clamp(2, 500);

        // Reserve the name while the holder starts.
        let _reservation = match &p.name {
            Some(name) => {
                let terms = self.terminals.lock().unwrap();
                let mut pending = self.pending_names.lock().unwrap();
                if let Some(t) = terms
                    .iter()
                    .find(|t| t.name.as_ref() == Some(name) && t.is_running())
                {
                    return Err(RpcError::new(
                        codes::NAME_TAKEN,
                        format!("name {name:?} is used by running terminal {}", t.id),
                    ));
                }
                if !pending.insert(name.clone()) {
                    return Err(RpcError::new(
                        codes::NAME_TAKEN,
                        format!("name {name:?} is being started"),
                    ));
                }
                Some(NameReservation {
                    daemon: self,
                    name: name.clone(),
                })
            }
            None => None,
        };

        // From here until the terminal is listed, `server.stop kill:true`
        // waits for this start (daemon-02).
        let _start = self.begin_start()?;
        let (parent_terminal, parent_task) = self.parent_of(p.parent_terminal.as_deref());
        let id = format!("t{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let spawn_err = |m: String| RpcError::new(codes::SPAWN_FAILED, m);
        let dir = &self.config.state_dir;
        std::fs::create_dir_all(dir).map_err(|e| spawn_err(format!("state dir: {e}")))?;
        let record = dir.join(format!("{id}.json"));
        let spec_path = dir.join(format!("{id}.spec.json"));
        let err_path = dir.join(format!("{id}.err"));
        let _ = std::fs::remove_file(&err_path);
        let pipe = endpoint::holder_pipe(&self.config.pipe, &id);
        let spec = HolderSpec {
            id: id.clone(),
            name: p.name.clone(),
            argv: p.argv.clone(),
            cwd: cwd.display().to_string(),
            env: status::hook_env(self.terminal_env(&id, p.env), dir, &id)?,
            cols,
            rows,
            pipe: pipe.clone(),
            record_path: record.display().to_string(),
            created_ms: crate::now_ms(),
            parent_terminal,
            parent_task,
        };
        std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap())
            .map_err(|e| spawn_err(format!("spec: {e}")))?;

        let mut cmd = Command::new(&self.config.holder_exe);
        cmd.arg("--spec").arg(&spec_path).current_dir(dir);
        let child = crate::client::spawn_detached(&mut cmd).map_err(|e| {
            let _ = std::fs::remove_file(&spec_path);
            spawn_err(format!(
                "cannot start holder {}: {e}",
                self.config.holder_exe.display()
            ))
        })?;

        let deadline = Instant::now() + HOLDER_START_TIMEOUT;
        let term = loop {
            if self.is_stopping_with_kill() {
                // Not connected yet: end the holder process; its Job Object
                // (kill on close) takes the child with it.
                let _ = child.kill();
                let _ = std::fs::remove_file(&spec_path);
                let _ = std::fs::remove_file(&record);
                log(&format!("run {id}: stopped while starting; holder killed"));
                return Err(stopping_err());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match Terminal::connect_within(&pipe, left.max(Duration::from_millis(100))).await {
                Ok(t) => break t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline => {
                    if let Ok(Some(status)) = child.try_wait() {
                        let msg = std::fs::read_to_string(&err_path)
                            .unwrap_or_else(|_| format!("holder exited with {status}"));
                        let _ = std::fs::remove_file(&err_path);
                        let _ = std::fs::remove_file(&spec_path);
                        return Err(spawn_err(format!("failed to start terminal: {msg}")));
                    }
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(spawn_err(format!("cannot connect to holder: {e}")));
                }
            }
        };
        let info = term.info();
        log(&format!(
            "run {} {:?} in {} (holder pid {})",
            info.id, info.argv, info.cwd, term.holder_pid
        ));
        self.terminals.lock().unwrap().push(term.clone());
        // Listed after `server.stop kill:true` took its kill set (it waited
        // at most START_GRACE): end it here, as the stop would have.
        if self.is_stopping_with_kill() {
            self.kill_and_wait(&term).await;
            if term.is_running() {
                let _ = child.kill();
            }
            term.send_to_holder(Frame::Release);
            log(&format!("run {}: stopped while starting; killed", info.id));
            return Err(stopping_err());
        }
        status::publish_terminal(&info.id, TerminalChange::Created, Some(info.clone()));
        Ok(info)
    }

    /// The caller's terminal as a parent: (terminal, task it belongs to) when
    /// this daemon knows the terminal, else nothing.
    pub(crate) fn parent_of(&self, caller: Option<&str>) -> (Option<String>, Option<String>) {
        let Some(tid) = caller.filter(|t| !t.is_empty()) else {
            return (None, None);
        };
        if !self.terminals.lock().unwrap().iter().any(|t| t.id == tid) {
            log(&format!("unknown parent terminal {tid} ignored"));
            return (None, None);
        }
        let task = self
            .registry
            .lock()
            .unwrap()
            .tasks
            .iter()
            .find(|t| t.terminals.iter().any(|x| x == tid))
            .map(|t| t.id.clone());
        (Some(tid.to_string()), task)
    }

    async fn send(&self, p: SendParams) -> Result<SendResult, RpcError> {
        let t = self.running(&p.target)?;
        let mut bytes = p.text.len();
        let chars = p.text.chars().count();
        t.remember_sent(&p.text);
        let written = crate::now_ms();
        if !p.text.is_empty() {
            t.try_input(p.text.into_bytes())
                .map_err(|e| input_err(&t, e))?;
        }
        let mut submit = None;
        if p.enter {
            // A separate write once the program has taken the text in: TUIs
            // that detect pastes by timing (Codex on Windows) turn an early
            // Enter into a newline (docs/12 B1, `submit.rs`).
            let since = if chars > 0 { written } else { 0 };
            submit = submit::press_enter(&t, since, chars, p.confirm, |_| {}).await;
            bytes += submit.as_ref().map_or(1, |s| s.enters as usize);
            if let Some(s) = &submit
                && s.state == SubmitState::Unconfirmed
            {
                log(&format!(
                    "{}: sent text was not confirmed as submitted (Enter pressed {} times)",
                    t.id, s.enters
                ));
            }
        }
        Ok(SendResult {
            id: t.id.clone(),
            bytes,
            submit,
            dropped_replies: None,
        })
    }

    fn keys(&self, p: KeysParams) -> Result<SendResult, RpcError> {
        let t = self.running(&p.target)?;
        let app_cursor = t.app_cursor();
        let mut bytes = Vec::new();
        for k in &p.keys {
            let b = crate::keys::key_bytes(k, app_cursor).ok_or_else(|| {
                RpcError::new(codes::INVALID_PARAMS, format!("unknown key {k:?}"))
            })?;
            bytes.extend(b);
        }
        let n = bytes.len();
        t.try_input(bytes).map_err(|e| input_err(&t, e))?;
        Ok(SendResult {
            id: t.id.clone(),
            bytes: n,
            submit: None,
            dropped_replies: None,
        })
    }

    fn input(&self, p: InputParams) -> Result<SendResult, RpcError> {
        let t = self.running(&p.target)?;
        let bytes = b64_decode(&p.data)
            .map_err(|e| RpcError::new(codes::INVALID_PARAMS, format!("data: {e}")))?;
        // Attach clients answer terminal queries: drop duplicates and late replies.
        let (n, dropped) = t.attach_input(&bytes).map_err(|e| input_err(&t, e))?;
        // A person typed: drafts block wake-ups; workers become kept (docs/19 짠5.2).
        if t.note_human_input(&bytes) {
            self.orch_on_human_input(&t.id);
        }
        if dropped.total() > 0 {
            log(&format!(
                "{}: dropped query replies from attach input ({} duplicate, {} late)",
                t.id, dropped.duplicate, dropped.late
            ));
        }
        Ok(SendResult {
            id: t.id.clone(),
            bytes: n,
            submit: None,
            dropped_replies: (dropped.total() > 0).then_some(crate::protocol::DroppedReplies {
                duplicate: dropped.duplicate,
                late: dropped.late,
            }),
        })
    }

    fn read(&self, p: ReadParams) -> Result<ReadResult, RpcError> {
        let t = self.resolve(&p.target)?;
        let (mut lines, cursor, suggestion) = if p.raw {
            let (l, c) = t.read_lines(p.mode);
            (l, c, None)
        } else {
            t.read_lines_for_agents(p.mode)
        };
        if let Some(n) = p.lines {
            let skip = lines.len().saturating_sub(n);
            lines.drain(..skip);
        }
        Ok(ReadResult {
            id: t.id.clone(),
            mode: p.mode,
            lines,
            cursor,
            state: t.info().state,
            suggestion,
        })
    }

    async fn wait(&self, p: WaitParams) -> Result<WaitResult, RpcError> {
        let t = self.resolve(&p.target)?;
        let start = Instant::now();
        let deadline = p
            .timeout_ms
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        let outcome = loop {
            // Register for the next change *before* checking, so none is missed.
            let notified = t.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let running = t.is_running();
            match &p.until {
                WaitUntil::Exit if !running => break WaitOutcome::Matched,
                WaitUntil::OutputContains(s) if t.output_contains(s) => break WaitOutcome::Matched,
                WaitUntil::OutputContains(_) if !running => break WaitOutcome::Exited,
                _ => {}
            }
            match deadline {
                Some(d) => {
                    if tokio::time::timeout_at(d, notified).await.is_err() {
                        break WaitOutcome::TimedOut;
                    }
                }
                None => notified.await,
            }
        };
        Ok(WaitResult {
            outcome,
            terminal: t.info(),
            waited_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Resize and wait until the holder confirmed it (the `Resized` frame).
    async fn resize_to(&self, t: &Terminal, cols: u16, rows: u16) -> Result<(), RpcError> {
        if cols < 2 || rows < 2 || cols > 1000 || rows > 500 {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "size out of range (2..=1000 x 2..=500)",
            ));
        }
        if t.size() == (cols, rows) {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let notified = t.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if t.size() == (cols, rows) {
                return Ok(());
            }
            if !t.is_running() {
                return Err(exited_err(t));
            }
            // (Re)send inside the loop: cheap, and robust against a reconnect.
            t.send_to_holder(Frame::Resize { cols, rows });
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(RpcError::new(
                    codes::INTERNAL_ERROR,
                    "holder did not confirm the resize",
                ));
            }
        }
    }

    async fn resize(&self, p: ResizeParams) -> Result<TerminalInfo, RpcError> {
        let t = self.running(&p.target)?;
        self.resize_to(&t, p.cols, p.rows).await?;
        Ok(t.info())
    }

    async fn kill_and_wait(&self, t: &Terminal) {
        if !t.is_running() {
            return;
        }
        t.send_to_holder(Frame::Kill);
        let deadline = Instant::now() + Duration::from_secs(5);
        while t.is_running() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn kill(&self, r: TerminalRef) -> Result<TerminalInfo, RpcError> {
        let t = match self.resolve(&r) {
            Ok(t) => t,
            // An unreachable holder: give up on it (daemon-03).
            Err(e) => return self.give_up_unreachable(&r).ok_or(e),
        };
        self.kill_and_wait(&t).await;
        log(&format!("kill {}", t.id));
        Ok(t.info())
    }

    async fn rm(&self, p: RmParams) -> Result<RmResult, RpcError> {
        let t = match self.resolve(&p.target) {
            Ok(t) => t,
            Err(e) => {
                let info = self.give_up_unreachable(&p.target).ok_or(e)?;
                return Ok(RmResult { id: info.id });
            }
        };
        if let Some(why) = &t.incompatible {
            // We cannot speak to this holder: only `force` ends it, by
            // terminating the holder process (its Job Object then ends the tree).
            if !p.force {
                return Err(RpcError::new(
                    codes::TERMINAL_RUNNING,
                    format!(
                        "terminal {} has an {why}; use force to terminate its holder (pid {})",
                        t.id, t.holder_pid
                    ),
                ));
            }
            if let Err(e) = crate::winsec::terminate_process(t.holder_pid) {
                log(&format!(
                    "{}: cannot terminate holder {}: {e}",
                    t.id, t.holder_pid
                ));
            }
            let record = self.config.state_dir.join(format!("{}.json", t.id));
            let _ = std::fs::remove_file(&record);
            let _ = std::fs::remove_file(record.with_extension("log"));
            self.terminals.lock().unwrap().retain(|x| x.id != t.id);
            status::on_removed(self, &t);
            self.forget_task_terminal(&t.id);
            self.forget_kept(&t.id);
            status::publish_terminal(&t.id, TerminalChange::Removed, None);
            log(&format!("rm {} (incompatible holder terminated)", t.id));
            return Ok(RmResult { id: t.id.clone() });
        }
        if t.is_running() {
            if !p.force {
                return Err(RpcError::new(
                    codes::TERMINAL_RUNNING,
                    format!("terminal {} is running (kill it first or use force)", t.id),
                ));
            }
            self.kill_and_wait(&t).await;
        }
        t.send_to_holder(Frame::Release);
        let record = self.config.state_dir.join(format!("{}.json", t.id));
        // The holder removes its record on release; a lost holder cannot.
        if t.info().lost {
            let _ = std::fs::remove_file(&record);
            let _ = std::fs::remove_file(record.with_extension("log"));
        }
        self.terminals.lock().unwrap().retain(|x| x.id != t.id);
        status::on_removed(self, &t);
        self.forget_task_terminal(&t.id);
        self.forget_kept(&t.id);
        status::publish_terminal(&t.id, TerminalChange::Removed, None);
        log(&format!("rm {}", t.id));
        Ok(RmResult { id: t.id.clone() })
    }

    /// `terminal.attach`: reply with a snapshot, then stream notifications
    /// until the client disconnects or the terminal exits.
    async fn stream_attach(
        &self,
        id: Value,
        p: Value,
        mut reader: BufReader<tokio::io::ReadHalf<NamedPipeServer>>,
        mut write: tokio::io::WriteHalf<NamedPipeServer>,
    ) -> anyhow::Result<()> {
        let prepared = async {
            let p: AttachParams = params(p)?;
            let t = self.resolve(&p.target)?;
            if let (Some(c), Some(r)) = (p.cols, p.rows)
                && t.is_running()
            {
                self.resize_to(&t, c, r).await?;
            }
            Ok::<_, RpcError>((t, p.scrollback.unwrap_or(DEFAULT_ATTACH_SCROLLBACK)))
        }
        .await;
        let (t, scrollback) = match prepared {
            Ok(x) => x,
            Err(e) => return write_json(&mut write, &err_response(id, e)).await,
        };
        let (snapshot, seq, mut sub) = t.attach(scrollback);
        let result = AttachResult {
            terminal: t.info(),
            seq,
            snapshot: b64_encode(&snapshot),
        };
        write_json(&mut write, &ok_response(id, serde_json::to_value(result)?)).await?;
        log(&format!("attach {} at seq {seq}", t.id));

        loop {
            tokio::select! {
                msg = sub.rx.recv() => {
                    // Dropped for falling behind: say so instead of writing the
                    // rest of the queue, then close (the client re-attaches).
                    if let Some(lag) = sub.lag() {
                        let n = notification(NOTIFY_LAGGED, LaggedEvent {
                            id: t.id.clone(),
                            seq: lag.seq,
                            queued_bytes: lag.queued_bytes,
                            limit_bytes: terminal::attach_queue_limit() as u64,
                        });
                        log(&format!(
                            "attach {} lagged at seq {} ({} bytes queued); dropped",
                            t.id, lag.seq, lag.queued_bytes
                        ));
                        let _ = write_json(&mut write, &n).await;
                        break;
                    }
                    let Some(msg) = msg else { break };
                    sub.taken(&msg);
                    let (n, last) = match msg {
                        StreamMsg::Output { seq, data } => (notification(NOTIFY_OUTPUT, OutputEvent {
                            id: t.id.clone(), seq, data: b64_encode(&data),
                        }), false),
                        StreamMsg::Resized { seq, cols, rows } => (notification(NOTIFY_RESIZED, ResizedEvent {
                            id: t.id.clone(), seq, cols, rows,
                        }), false),
                        StreamMsg::Exited(e) => (notification(NOTIFY_EXITED, ExitedEvent {
                            id: t.id.clone(), seq: e.seq, exit_code: e.exit_code, killed: e.killed,
                        }), true),
                    };
                    if write_json(&mut write, &n).await.is_err() || last {
                        break;
                    }
                }
                // Anything the client sends is ignored; EOF means detach.
                r = discard_input(&mut reader) => {
                    if !matches!(r, Ok(n) if n > 0) {
                        break;
                    }
                }
            }
        }
        log(&format!("detach {}", t.id));
        Ok(())
    }
}

struct NameReservation<'a> {
    daemon: &'a Daemon,
    name: String,
}

impl Drop for NameReservation<'_> {
    fn drop(&mut self) {
        self.daemon.pending_names.lock().unwrap().remove(&self.name);
    }
}

fn id_number(id: &str) -> u64 {
    id.trim_start_matches('t').parse().unwrap_or(0)
}

fn state_word(i: &TerminalInfo) -> &'static str {
    match i.state {
        TerminalState::Exited if i.incompatible_holder.is_some() => "incompatible holder",
        TerminalState::Running => "running",
        TerminalState::Exited => "exited",
        TerminalState::Unreachable => "unreachable",
    }
}

/// Timestamped line on stderr (redirected to the log file by `podd --log`).
pub fn log(msg: &str) {
    eprintln!("[podd {}] {msg}", crate::now_ms());
}
