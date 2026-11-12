//! Blocking client for the podd pipe, plus "start the daemon if absent".
//!
//! Deliberately free of tokio and the PTY stack so the `pod` CLI stays small.
//! The connection is a [`Transport`] (docs/17 R1): the named pipe locally, any
//! `Read` + `Write` pair (a child's stdio, in-memory pipes) otherwise.

use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocol::*;
use crate::transport::{FrameError, LineReader, Transport};

const ERROR_PIPE_BUSY: i32 = 231;
/// How long `connect` waits while every pipe instance is taken. A stopping
/// daemon keeps its pipe busy until it exits (up to ~8 s with `--kill`).
const BUSY_WAIT: Duration = Duration::from_secs(15);
/// How long an auto-starting client keeps retrying a request the old daemon
/// refused with `SERVER_STOPPING`.
const STOPPING_RETRY: Duration = Duration::from_secs(30);
/// How long auto-start waits for a daemon it started (or another client
/// holding the start lock started) to accept connections.
pub const START_TIMEOUT: Duration = Duration::from_secs(10);

/// Environment override for the daemon executable used by auto-start.
pub const ENV_DAEMON_EXE: &str = "POD_DAEMON";

#[derive(Debug)]
pub enum ClientError {
    /// No daemon listens on the pipe.
    NotRunning,
    Io(std::io::Error),
    /// The daemon answered with a JSON-RPC error.
    Rpc(RpcError),
    Protocol(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning => f.write_str("podd is not running"),
            ClientError::Io(e) => write!(f, "pipe I/O error: {e}"),
            ClientError::Rpc(e) => write!(f, "{}", e.message),
            ClientError::Protocol(m) => write!(f, "protocol error: {m}"),
        }
    }
}

impl std::error::Error for ClientError {}

impl ClientError {
    /// The daemon is shutting down and refused the request (nothing was done).
    pub fn is_stopping(&self) -> bool {
        matches!(self, ClientError::Rpc(e) if e.code == codes::SERVER_STOPPING)
    }

    /// The daemon speaks no protocol version this client can use for a
    /// remote connection (v12 `connection.hello`). Nothing was changed.
    pub fn is_incompatible(&self) -> bool {
        matches!(self, ClientError::Rpc(e) if e.code == remote_codes::PROTOCOL_MISMATCH)
    }
}

fn incompatible(message: String) -> ClientError {
    ClientError::Rpc(RpcError::new(remote_codes::PROTOCOL_MISMATCH, message))
}

impl From<std::io::Error> for ClientError {
    fn from(e: std::io::Error) -> Self {
        ClientError::Io(e)
    }
}

pub struct Client {
    /// The pipe path, or a label for other transports.
    pipe: String,
    conn: Transport,
    next_id: u64,
    /// Set by [`Client::connect_or_start`]: on `SERVER_STOPPING` wait for the
    /// old daemon to go, start / reach the next one and resend.
    autostart: Option<Option<PathBuf>>,
    /// The protocol chosen by [`Client::hello`].
    protocol: Option<u32>,
}

impl Client {
    /// Connect to a running daemon. Returns [`ClientError::NotRunning`] when the pipe does not exist.
    ///
    /// While every instance is taken (normally a few ms between accepts; a
    /// stopping daemon until it exits) it waits up to 15 s.
    pub fn connect(pipe: &str) -> Result<Self, ClientError> {
        let deadline = Instant::now() + BUSY_WAIT;
        loop {
            match Transport::open_pipe(pipe) {
                Ok(t) => return Ok(Self::from_transport(pipe, t)),
                Err(e) if e.kind() == ErrorKind::NotFound => return Err(ClientError::NotRunning),
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if Instant::now() >= deadline {
                        return Err(ClientError::Io(std::io::Error::new(
                            e.kind(),
                            format!(
                                "every instance of {pipe} stayed busy for {} s \
                                 (podd may still be stopping): {e}",
                                BUSY_WAIT.as_secs()
                            ),
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// A client over any transport (no auto-start, no stop retry). `label`
    /// is what [`Client::pipe`] returns.
    pub fn from_transport(label: impl Into<String>, conn: Transport) -> Self {
        Self {
            pipe: label.into(),
            conn,
            next_id: 1,
            autostart: None,
            protocol: None,
        }
    }

    /// Give up the RPC layer and keep the raw line transport (`pod remote-stdio`).
    pub fn into_transport(self) -> Transport {
        self.conn
    }

    /// Connect, starting the daemon detached first if the pipe is absent.
    ///
    /// A request the daemon refuses because it is stopping is resent to the
    /// next daemon (started here once the old one has exited).
    pub fn connect_or_start(pipe: &str, daemon_exe: Option<&Path>) -> Result<Self, ClientError> {
        let mut c = Self::connect_or_start_once(pipe, daemon_exe)?;
        c.autostart = Some(daemon_exe.map(Path::to_path_buf));
        Ok(c)
    }

    fn connect_or_start_once(pipe: &str, daemon_exe: Option<&Path>) -> Result<Self, ClientError> {
        match Self::connect(pipe) {
            Err(ClientError::NotRunning) => {}
            other => return other,
        }
        let exe = match daemon_exe {
            Some(p) => p.to_path_buf(),
            None => default_daemon_exe()?,
        };
        // One starter at a time (CLI, app, remote bridges): the others wait
        // here and then find the daemon the first one started.
        let _lock = StartLock::acquire(pipe, START_TIMEOUT + Duration::from_secs(5))?;
        match Self::connect(pipe) {
            Err(ClientError::NotRunning) => {}
            other => return other,
        }
        spawn_daemon(&exe, pipe)?;
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            match Self::connect(pipe) {
                Err(ClientError::NotRunning) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(ClientError::NotRunning) => {
                    return Err(ClientError::Io(std::io::Error::new(
                        ErrorKind::TimedOut,
                        format!(
                            "started {} but it did not open {pipe} within {} s (see {})",
                            exe.display(),
                            START_TIMEOUT.as_secs(),
                            default_log_path().display()
                        ),
                    )));
                }
                other => return other,
            }
        }
    }

    pub fn pipe(&self) -> &str {
        &self.pipe
    }

    /// The v12 handshake a remote connection starts with (docs/17 §2.3):
    /// `server.status` for the daemon's protocol, then `connection.hello`
    /// with this build's range and the capabilities the caller needs.
    ///
    /// A v11 daemon (or anything without `connection.hello`), a daemon with
    /// no common version, a missing capability or an answer outside this
    /// build's range all fail with [`ClientError::is_incompatible`]; no other
    /// request is sent, so nothing on the remote side is changed.
    pub fn hello(&mut self, require: &[&str]) -> Result<HelloResult, ClientError> {
        let status = self.call::<ServerStatus>(&Empty {})?;
        if status.protocol < HELLO_PROTOCOL {
            return Err(incompatible(format!(
                "podd {} on {} speaks protocol {}; remote connections need {} or newer. \
                 Install the matching Pod version there (running terminals are not touched)",
                status.version, self.pipe, status.protocol, HELLO_PROTOCOL
            )));
        }
        let params = HelloParams {
            min_protocol: HELLO_PROTOCOL,
            max_protocol: PROTOCOL_VERSION,
            require: require.iter().map(|c| c.to_string()).collect(),
            client: Some(format!("pod {}", env!("CARGO_PKG_VERSION"))),
        };
        let hello = match self.call::<ConnectionHello>(&params) {
            Err(ClientError::Rpc(e)) if e.code == codes::METHOD_NOT_FOUND => {
                return Err(incompatible(format!(
                    "podd {} on {} reports protocol {} but has no connection.hello",
                    status.version, self.pipe, status.protocol
                )));
            }
            other => other?,
        };
        if hello.protocol < HELLO_PROTOCOL || hello.protocol > PROTOCOL_VERSION {
            return Err(incompatible(format!(
                "podd chose protocol {}, outside this client's {}..={}",
                hello.protocol, HELLO_PROTOCOL, PROTOCOL_VERSION
            )));
        }
        let missing: Vec<&str> = require
            .iter()
            .copied()
            .filter(|c| !hello.capabilities.iter().any(|h| h == c))
            .collect();
        if !missing.is_empty() {
            return Err(incompatible(format!(
                "podd lacks required capabilities: {}",
                missing.join(", ")
            )));
        }
        self.protocol = Some(hello.protocol);
        Ok(hello)
    }

    /// The protocol `connection.hello` chose on this connection; `None`
    /// before a hello (local connections never send one).
    pub fn protocol(&self) -> Option<u32> {
        self.protocol
    }

    /// Typed call.
    pub fn call<M: Method>(&mut self, params: &M::Params) -> Result<M::Result, ClientError> {
        let params =
            serde_json::to_value(params).map_err(|e| ClientError::Protocol(e.to_string()))?;
        let result = self.call_raw(M::NAME, params)?;
        serde_json::from_value(result)
            .map_err(|e| ClientError::Protocol(format!("bad {} result: {e}", M::NAME)))
    }

    /// Untyped call; returns the `result` value.
    pub fn call_raw(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let Some(exe) = self.autostart.clone() else {
            return self.call_once(method, params);
        };
        let deadline = Instant::now() + STOPPING_RETRY;
        loop {
            match self.call_once(method, params.clone()) {
                // The old daemon did nothing; it keeps its pipe until it has
                // exited, so reconnecting waits for that, then starts a new one.
                Err(e) if e.is_stopping() && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                    let mut next = Self::connect_or_start_once(&self.pipe, exe.as_deref())?;
                    next.autostart = Some(exe.clone());
                    next.next_id = self.next_id;
                    *self = next;
                }
                other => return other,
            }
        }
    }

    fn call_once(&mut self, method: &str, params: Value) -> Result<Value, ClientError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = Request {
            jsonrpc: JSONRPC_VERSION.into(),
            id: Some(Value::from(id)),
            method: method.into(),
            params,
        };
        let line = serde_json::to_string(&req).map_err(|e| ClientError::Protocol(e.to_string()))?;
        self.conn.writer.write_line(&line)?;

        let Some(buf) = read_frame(&mut self.conn.reader)? else {
            return Err(ClientError::Protocol(CLOSED.into()));
        };
        let resp: Response = parse_line(&buf, "response")?;
        if resp.id != id {
            return Err(ClientError::Protocol(format!(
                "response id {} does not match request id {id}",
                resp.id
            )));
        }
        if let Some(err) = resp.error {
            return Err(ClientError::Rpc(err));
        }
        Ok(resp.result.unwrap_or(Value::Null))
    }
}

/// What every "the daemon went away" error says (callers match on it).
const CLOSED: &str = "daemon closed the connection";

/// The next line, with framing errors mapped to client errors.
fn read_frame(r: &mut LineReader) -> Result<Option<String>, ClientError> {
    match r.read_line() {
        Ok(line) => Ok(line),
        Err(FrameError::Io(e)) => Err(ClientError::Io(e)),
        // The peer went away mid-line: same as going away between lines.
        Err(FrameError::Truncated) => Err(ClientError::Protocol(CLOSED.into())),
        Err(e) => Err(ClientError::Protocol(e.to_string())),
    }
}

/// Parse one protocol line. Anything that is not a JSON object (a login
/// banner, a shell profile's output on a remote stdout) is an error, never
/// skipped (docs/17 §2.1: fail closed).
fn parse_line<T: serde::de::DeserializeOwned>(line: &str, what: &str) -> Result<T, ClientError> {
    if !line.trim_start().starts_with('{') {
        let preview: String = line.chars().take(80).collect();
        return Err(ClientError::Protocol(format!(
            "non-protocol output on the daemon connection: {preview:?}"
        )));
    }
    serde_json::from_str(line).map_err(|e| ClientError::Protocol(format!("bad {what}: {e}")))
}

/// Serializes daemon auto-start per pipe across processes and logon
/// sessions: an exclusive handle on `%LOCALAPPDATA%\pod\start-<pipe key>.lock`.
/// The OS drops it when the holder exits, so a crashed starter never leaves
/// it stuck.
pub struct StartLock {
    _file: std::fs::File,
}

impl StartLock {
    pub fn path(pipe: &str) -> PathBuf {
        default_log_path().with_file_name(format!("start-{}.lock", crate::endpoint::pipe_key(pipe)))
    }

    /// Wait up to `timeout` for the lock.
    pub fn acquire(pipe: &str, timeout: Duration) -> Result<Self, ClientError> {
        use std::os::windows::fs::OpenOptionsExt;
        const ERROR_SHARING_VIOLATION: i32 = 32;
        let path = Self::path(pipe);
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let deadline = Instant::now() + timeout;
        loop {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(&path)
            {
                Ok(f) => return Ok(Self { _file: f }),
                Err(e)
                    if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION)
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(e) => {
                    return Err(ClientError::Io(std::io::Error::new(
                        e.kind(),
                        format!("cannot take the podd start lock {}: {e}", path.display()),
                    )));
                }
            }
        }
    }
}

/// A handle on a process (the daemon) to wait for its exit. Opened before
/// asking it to stop, so a reused pid can never be mistaken for it.
pub struct ProcessWatch(windows_sys::Win32::Foundation::HANDLE);

impl ProcessWatch {
    /// `None` when no such process exists (it has already exited).
    pub fn open(pid: u32) -> Option<Self> {
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE};
        // SAFETY: plain Win32 call; the handle is closed in `drop`.
        let h = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        (!h.is_null()).then_some(Self(h))
    }

    /// `true` once the process has exited, `false` if it still runs after `timeout`.
    pub fn wait(&self, timeout: Duration) -> bool {
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::WaitForSingleObject;
        let ms = timeout.as_millis().min(u32::MAX as u128 - 1) as u32;
        // SAFETY: `self.0` is a valid process handle with SYNCHRONIZE access.
        unsafe { WaitForSingleObject(self.0, ms) == WAIT_OBJECT_0 }
    }
}

impl Drop for ProcessWatch {
    fn drop(&mut self) {
        // SAFETY: we own the handle.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// `POD_DAEMON`, else `podd.exe` next to the current executable.
pub fn default_daemon_exe() -> Result<PathBuf, ClientError> {
    if let Ok(p) = std::env::var(ENV_DAEMON_EXE)
        && !p.is_empty()
    {
        return Ok(PathBuf::from(p));
    }
    let me = std::env::current_exe()?;
    let exe = me.with_file_name(if cfg!(windows) { "podd.exe" } else { "podd" });
    if exe.exists() {
        Ok(exe)
    } else {
        Err(ClientError::Io(std::io::Error::new(
            ErrorKind::NotFound,
            format!(
                "daemon executable not found at {} (set {ENV_DAEMON_EXE})",
                exe.display()
            ),
        )))
    }
}

/// Log file used when the CLI starts the daemon: `%LOCALAPPDATA%\pod\podd.log`.
pub fn default_log_path() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("pod").join("podd.log")
}

/// Start the daemon with no console and no window, outside our job object if
/// allowed, so it outlives the CLI and the terminal the CLI ran in.
pub fn spawn_daemon(exe: &Path, pipe: &str) -> Result<(), ClientError> {
    let log = default_log_path();
    if let Some(dir) = log.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut cmd = Command::new(exe);
    cmd.arg("--pipe").arg(pipe).arg("--log").arg(&log);
    // Start the daemon somewhere that never blocks a directory from being removed.
    if let Some(home) = std::env::var_os("USERPROFILE") {
        cmd.current_dir(home);
    }
    spawn_detached(&mut cmd)?;
    Ok(())
}

/// Spawn with no console, no window, `NUL` stdio and **no other inherited
/// handles**, in a new process group and outside our Job Object when the job
/// allows breakaway (so the process outlives the terminal / test harness that
/// started us). See [`crate::detach`].
pub fn spawn_detached(cmd: &mut Command) -> std::io::Result<crate::detach::DetachedChild> {
    crate::detach::spawn_detached(cmd)
}

/// One event on an attached connection.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachEvent {
    /// Raw output bytes starting at absolute offset `seq`.
    Output {
        seq: u64,
        data: Vec<u8>,
    },
    Resized {
        seq: u64,
        cols: u16,
        rows: u16,
    },
    Exited(ExitedEvent),
    /// v7: this client fell behind and was dropped (last event). Re-attach to
    /// continue from a fresh snapshot.
    Lagged(LaggedEvent),
}

/// The notification stream of a `terminal.attach` connection. Dropping it detaches.
pub struct AttachStream {
    conn: Transport,
}

impl AttachStream {
    /// Next event; `Ok(None)` when the daemon closed the stream.
    pub fn next_event(&mut self) -> Result<Option<AttachEvent>, ClientError> {
        loop {
            let Some(buf) = read_frame(&mut self.conn.reader)? else {
                return Ok(None);
            };
            let n: Notification = parse_line(&buf, "notification")?;
            let bad =
                |e: serde_json::Error| ClientError::Protocol(format!("bad {}: {e}", n.method));
            match n.method.as_str() {
                NOTIFY_OUTPUT => {
                    let o: OutputEvent = serde_json::from_value(n.params.clone()).map_err(bad)?;
                    let data = b64_decode(&o.data).map_err(ClientError::Protocol)?;
                    return Ok(Some(AttachEvent::Output { seq: o.seq, data }));
                }
                NOTIFY_RESIZED => {
                    let r: ResizedEvent = serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(AttachEvent::Resized {
                        seq: r.seq,
                        cols: r.cols,
                        rows: r.rows,
                    }));
                }
                NOTIFY_EXITED => {
                    let e: ExitedEvent = serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(AttachEvent::Exited(e)));
                }
                NOTIFY_LAGGED => {
                    let e: LaggedEvent = serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(AttachEvent::Lagged(e)));
                }
                _ => continue, // unknown notifications are ignored (forward compatible)
            }
        }
    }
}

impl Client {
    /// Attach: returns the snapshot reply and turns this connection into the
    /// event stream. Send input/resizes on a different connection.
    ///
    /// A v20 remote connection gets the snapshot in `terminal.snapshot`
    /// chunks (`AttachResult.snapshot_chunks`); they are read and joined
    /// here, so callers always get the whole snapshot.
    pub fn attach(
        mut self,
        params: &AttachParams,
    ) -> Result<(AttachResult, Vec<u8>, AttachStream), ClientError> {
        let result = self.call::<TerminalAttach>(params)?;
        let snapshot = match result.snapshot_chunks {
            None => b64_decode(&result.snapshot).map_err(ClientError::Protocol)?,
            Some(total) => read_snapshot_chunks(&mut self.conn.reader, &result, total)?,
        };
        Ok((result, snapshot, AttachStream { conn: self.conn }))
    }
}

/// The `terminal.snapshot` chunks that follow a chunked attach reply, joined.
/// Every bound is checked before memory is taken (docs/22 TM-14): the
/// declared size against [`MAX_SNAPSHOT_BYTES`], the chunk count against the
/// size, each chunk's base64 length before it is decoded, and the running
/// total against the declared size. Order, `seq`, `id` and `total` must
/// match; anything else on the stream before the last chunk is an error.
fn read_snapshot_chunks(
    r: &mut LineReader,
    result: &AttachResult,
    total: u32,
) -> Result<Vec<u8>, ClientError> {
    let bad = |m: String| ClientError::Protocol(format!("snapshot chunks: {m}"));
    let declared = result.snapshot_bytes.unwrap_or(0);
    if declared > MAX_SNAPSHOT_BYTES as u64 {
        return Err(bad(format!(
            "{declared} bytes declared, the limit is {MAX_SNAPSHOT_BYTES}"
        )));
    }
    let want = declared.div_ceil(REMOTE_OUTPUT_CHUNK as u64);
    if u64::from(total) != want {
        return Err(bad(format!(
            "{total} chunks for {declared} bytes (expected {want})"
        )));
    }
    // Base64 of one full chunk, padded.
    let max_text = REMOTE_OUTPUT_CHUNK.div_ceil(3) * 4;
    let mut out = Vec::with_capacity(declared as usize);
    for index in 0..total {
        let Some(line) = read_frame(r)? else {
            return Err(ClientError::Protocol(CLOSED.into()));
        };
        let n: Notification = parse_line(&line, "notification")?;
        if n.method != NOTIFY_SNAPSHOT {
            return Err(bad(format!("{} before chunk {index} of {total}", n.method)));
        }
        let c: SnapshotChunkEvent =
            serde_json::from_value(n.params).map_err(|e| bad(e.to_string()))?;
        if c.index != index || c.total != total || c.seq != result.seq || c.id != result.terminal.id
        {
            return Err(bad(format!(
                "chunk {}/{} of {} at seq {} where {index}/{total} of {} at seq {} was due",
                c.index, c.total, c.id, c.seq, result.terminal.id, result.seq
            )));
        }
        if c.data.len() > max_text {
            return Err(bad(format!(
                "chunk {index} is larger than {REMOTE_OUTPUT_CHUNK} bytes"
            )));
        }
        let data = b64_decode(&c.data).map_err(bad)?;
        if data.len() > REMOTE_OUTPUT_CHUNK {
            return Err(bad(format!(
                "chunk {index} is larger than {REMOTE_OUTPUT_CHUNK} bytes"
            )));
        }
        if out.len() + data.len() > declared as usize {
            return Err(bad(format!("more than the {declared} bytes declared")));
        }
        out.extend_from_slice(&data);
    }
    if out.len() as u64 != declared {
        return Err(bad(format!("{} bytes of {declared} declared", out.len())));
    }
    Ok(out)
}

/// What a `status.subscribe` connection delivers.
#[derive(Debug, Clone)]
pub enum StatusStreamEvent {
    Changed(Box<StatusChangedEvent>),
    Lagged(u64),
}

/// The notification stream of a `status.subscribe` connection. Dropping it unsubscribes.
pub struct StatusStream {
    conn: Transport,
}

/// Every kind of event on a `status.subscribe` connection (v5 adds gates,
/// v6 background `group.check` progress).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SubscribeEvent {
    Status(StatusStreamEvent),
    Gate(Box<GateChangedEvent>),
    GroupCheck(Box<GroupCheckEvent>),
    /// A review delivery was recorded or changed state (`review.delivery`).
    Delivery(Box<ReviewDeliveryEvent>),
    /// v9 lifecycle events (subscribe with `lifecycle: true`).
    Terminal(Box<TerminalChangedEvent>),
    Task(Box<TaskChangedEvent>),
    TaskRemoving(Box<TaskRemovingEvent>),
    /// v10 `task.pr.changed` (lifecycle subscribers).
    TaskPr(Box<TaskPrChangedEvent>),
    /// v11 extensions: `mod.notify`, `mod.changed`.
    ModNotify(Box<ModNotifyEvent>),
    ModChanged(Box<ModChangedEvent>),
    /// Session discovery (docs/18): `sessions.changed` (lifecycle subscribers).
    SessionsChanged(Box<SessionsChangedEvent>),
}

impl StatusStream {
    /// Next status event; `Ok(None)` when the daemon closed the stream.
    /// Gate events are skipped (see [`StatusStream::next_any`]).
    pub fn next_event(&mut self) -> Result<Option<StatusStreamEvent>, ClientError> {
        loop {
            match self.next_any()? {
                None => return Ok(None),
                Some(SubscribeEvent::Status(e)) => return Ok(Some(e)),
                Some(_) => continue,
            }
        }
    }

    /// Next status or gate event; `Ok(None)` when the daemon closed the stream.
    pub fn next_any(&mut self) -> Result<Option<SubscribeEvent>, ClientError> {
        loop {
            let Some(buf) = read_frame(&mut self.conn.reader)? else {
                return Ok(None);
            };
            let n: Notification = parse_line(&buf, "notification")?;
            let bad =
                |e: serde_json::Error| ClientError::Protocol(format!("bad {}: {e}", n.method));
            match n.method.as_str() {
                NOTIFY_STATUS_CHANGED => {
                    let e: StatusChangedEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::Status(StatusStreamEvent::Changed(
                        Box::new(e),
                    ))));
                }
                NOTIFY_STATUS_LAGGED => {
                    let e: StatusLaggedEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::Status(StatusStreamEvent::Lagged(
                        e.missed,
                    ))));
                }
                NOTIFY_GATE_CHANGED => {
                    let e: GateChangedEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::Gate(Box::new(e))));
                }
                NOTIFY_GROUP_CHECK => {
                    let e: GroupCheckEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::GroupCheck(Box::new(e))));
                }
                NOTIFY_REVIEW_DELIVERY => {
                    let e: ReviewDeliveryEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::Delivery(Box::new(e))));
                }
                NOTIFY_TERMINAL_CHANGED => {
                    let e: TerminalChangedEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::Terminal(Box::new(e))));
                }
                NOTIFY_TASK_CHANGED => {
                    let e: TaskChangedEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::Task(Box::new(e))));
                }
                NOTIFY_TASK_REMOVING => {
                    let e: TaskRemovingEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::TaskRemoving(Box::new(e))));
                }
                NOTIFY_TASK_PR_CHANGED => {
                    let e: TaskPrChangedEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::TaskPr(Box::new(e))));
                }
                NOTIFY_MOD_NOTIFY => {
                    let e: ModNotifyEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::ModNotify(Box::new(e))));
                }
                NOTIFY_MOD_CHANGED => {
                    let e: ModChangedEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::ModChanged(Box::new(e))));
                }
                NOTIFY_SESSIONS_CHANGED => {
                    let e: SessionsChangedEvent =
                        serde_json::from_value(n.params.clone()).map_err(bad)?;
                    return Ok(Some(SubscribeEvent::SessionsChanged(Box::new(e))));
                }
                _ => continue,
            }
        }
    }
}

impl Client {
    /// Subscribe to status changes: returns the current statuses and turns
    /// this connection into the event stream.
    pub fn subscribe_status(
        mut self,
        params: &StatusSubscribeParams,
    ) -> Result<(StatusSubscribeResult, StatusStream), ClientError> {
        let result = self.call::<StatusSubscribe>(params)?;
        Ok((result, StatusStream { conn: self.conn }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn reply(chunks: u32, bytes: u64) -> AttachResult {
        AttachResult {
            terminal: TerminalInfo::new("t1", vec![], "", TerminalState::Running, 80, 24, 1),
            seq: 9,
            snapshot: String::new(),
            snapshot_chunks: Some(chunks),
            snapshot_bytes: Some(bytes),
            scrollback_omitted: None,
        }
    }

    fn chunk(index: u32, total: u32, data: &[u8]) -> String {
        let n = Notification {
            jsonrpc: JSONRPC_VERSION.into(),
            method: NOTIFY_SNAPSHOT.into(),
            params: serde_json::to_value(SnapshotChunkEvent {
                id: "t1".into(),
                seq: 9,
                index,
                total,
                data: b64_encode(data),
            })
            .unwrap(),
        };
        serde_json::to_string(&n).unwrap() + "\n"
    }

    fn reader(lines: &[String]) -> LineReader {
        LineReader::new(Cursor::new(lines.concat().into_bytes()), 1 << 20)
    }

    /// v20 chunked snapshots: joined in order; every bound refused.
    #[test]
    fn snapshot_chunks_join_and_refuse_bad_streams() {
        let data: Vec<u8> = (0..70_000u32).map(|i| (i % 251) as u8).collect();
        let parts: Vec<&[u8]> = data.chunks(REMOTE_OUTPUT_CHUNK).collect();
        let lines: Vec<String> = (0..3).map(|i| chunk(i, 3, parts[i as usize])).collect();
        let r = reply(3, data.len() as u64);
        assert_eq!(
            read_snapshot_chunks(&mut reader(&lines), &r, 3).unwrap(),
            data
        );
        // An empty snapshot is zero chunks.
        assert!(
            read_snapshot_chunks(&mut reader(&[]), &reply(0, 0), 0)
                .unwrap()
                .is_empty()
        );

        let err = |lines: &[String], r: &AttachResult, total: u32| {
            read_snapshot_chunks(&mut reader(lines), r, total)
                .unwrap_err()
                .to_string()
        };
        // Out of order.
        let swapped = [lines[1].clone(), lines[0].clone(), lines[2].clone()];
        assert!(err(&swapped, &r, 3).contains("chunk 1/3"));
        // Output before the last chunk.
        let out = serde_json::to_string(&Notification {
            jsonrpc: JSONRPC_VERSION.into(),
            method: NOTIFY_OUTPUT.into(),
            params: serde_json::json!({"id": "t1", "seq": 9, "data": ""}),
        })
        .unwrap()
            + "\n";
        assert!(err(&[lines[0].clone(), out], &r, 3).contains("terminal.output before chunk 1"));
        // Over the 16 MiB limit or a count that does not fit the size: refused
        // before reading anything.
        let huge = reply(513, MAX_SNAPSHOT_BYTES as u64 + 1);
        assert!(err(&[], &huge, 513).contains("limit"));
        assert!(err(&lines, &reply(4, data.len() as u64), 4).contains("expected 3"));
        // More bytes than declared, and fewer.
        assert!(err(&lines, &reply(3, 65_537), 3).contains("more than"));
        assert!(err(&lines, &reply(3, 69_999), 3).contains("more than"));
        let short: Vec<String> = vec![chunk(0, 1, b"abc")];
        assert!(err(&short, &reply(1, 4), 1).contains("3 bytes of 4"));
        // A chunk larger than 32 KiB is refused before it is decoded.
        let big = vec![b'x'; REMOTE_OUTPUT_CHUNK + 3];
        assert!(err(&[chunk(0, 1, &big)], &reply(1, 1), 1).contains("larger than"));
        // One byte over still has the same base64 length: caught after decoding.
        let big = vec![b'x'; REMOTE_OUTPUT_CHUNK + 1];
        assert!(err(&[chunk(0, 2, &big)], &reply(2, 40_000), 2).contains("larger than"));
        // EOF in the middle.
        assert!(err(&lines[..1], &r, 3).contains(CLOSED));
    }
}
