//! Daemon-side terminal: a connection to the terminal's holder plus a VT
//! mirror fed from the holder's byte stream.
//!
//! The mirror answers `read`/`wait` and produces attach snapshots. Everything
//! that touches the stream (feeding the mirror, fanning out to attached
//! clients, taking a snapshot + subscribing) happens under one lock, so a
//! subscriber's first output byte is exactly the snapshot's `seq`.

use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio::sync::{Notify, mpsc};

use super::query_dedup::{Expectations, Filtered, QueryScanner};
use crate::holder_proto::{
    DaemonHello, ExitInfo, Frame, HOLDER_PROTOCOL, HolderHello, check_holder_version, read_frame,
};
use crate::protocol::{ReadMode, TerminalInfo, TerminalState};
use crate::vt::Vt;

const ERROR_PIPE_BUSY: i32 = 231;
/// Longest a holder connect may take, busy pipe and handshake included.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Open a holder pipe. The holder may identify but never impersonate us
/// (security-01): `SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION`.
fn open_holder_pipe(pipe: &str) -> io::Result<NamedPipeClient> {
    ClientOptions::new()
        .security_qos_flags(crate::transport::SQOS_IDENTIFICATION)
        .open(pipe)
}
/// How many recently sent texts are remembered for echo filtering.
const RECENT_SENDS: usize = 32;

/// Default per-client attach queue limit (bytes of output not yet written
/// to the client). Overridden by `podd --attach-queue-limit`.
pub const DEFAULT_ATTACH_QUEUE_LIMIT: usize = 16 * 1024 * 1024;
static ATTACH_QUEUE_LIMIT: AtomicUsize = AtomicUsize::new(DEFAULT_ATTACH_QUEUE_LIMIT);

pub fn set_attach_queue_limit(bytes: usize) {
    ATTACH_QUEUE_LIMIT.store(bytes.max(1), Ordering::Relaxed);
}

pub fn attach_queue_limit() -> usize {
    ATTACH_QUEUE_LIMIT.load(Ordering::Relaxed)
}

/// Input bytes sent to a holder and not yet consumed by it (a holder of
/// protocol 3+ acknowledges what it wrote to the PTY; older holders: not
/// yet written to the pipe) past which more input is refused (daemon-01). One
/// input larger than this still goes when nothing is in flight.
pub const INPUT_BUDGET: u64 = 4 * 1024 * 1024;

/// Why input was not sent to the holder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputRefused {
    /// [`INPUT_BUDGET`] bytes wait for the program to read them.
    Backlog { in_flight: u64 },
    /// The holder connection is gone.
    Gone,
}

/// The daemon's side of a holder connection: control frames (resize, kill,
/// release, hello) in their own unbounded queue that the writer always
/// serves first, input in a second queue under [`INPUT_BUDGET`]. Control
/// frames are therefore never stuck behind input (daemon-01).
pub struct HolderTx {
    ctrl: mpsc::UnboundedSender<Frame>,
    input: mpsc::UnboundedSender<Vec<u8>>,
    /// Input bytes accepted for this connection.
    sent: AtomicU64,
    /// Input bytes consumed: acknowledged by the holder (protocol >= 3), or
    /// written to the pipe (older holders).
    done: Arc<AtomicU64>,
}

impl HolderTx {
    /// Start the writer task on `wr`. With `acks` the holder acknowledges
    /// input (`InputAck`); without, input counts as done once written.
    pub fn spawn<W>(mut wr: W, acks: bool) -> HolderTx
    where
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (ctrl, mut ctrl_rx) = mpsc::unbounded_channel::<Frame>();
        let (input, mut input_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let done = Arc::new(AtomicU64::new(0));
        let written = done.clone();
        tokio::spawn(async move {
            loop {
                let (bytes, input_len) = tokio::select! {
                    biased;
                    Some(f) = ctrl_rx.recv() => (f.encode(), 0),
                    Some(b) = input_rx.recv() => {
                        let n = b.len();
                        (Frame::Input(b).encode(), n)
                    }
                    else => break,
                };
                if wr.write_all(&bytes).await.is_err() {
                    break;
                }
                if input_len > 0 && !acks {
                    written.fetch_add(input_len as u64, Ordering::SeqCst);
                }
            }
        });
        HolderTx {
            ctrl,
            input,
            sent: AtomicU64::new(0),
            done,
        }
    }

    /// A connection that goes nowhere (incompatible holders).
    fn closed() -> HolderTx {
        let (ctrl, _) = mpsc::unbounded_channel();
        let (input, _) = mpsc::unbounded_channel();
        HolderTx {
            ctrl,
            input,
            sent: AtomicU64::new(0),
            done: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn control(&self, frame: Frame) -> bool {
        self.ctrl.send(frame).is_ok()
    }

    /// Queue input unless [`INPUT_BUDGET`] bytes are still in flight.
    pub fn try_input(&self, bytes: Vec<u8>) -> Result<(), InputRefused> {
        let len = bytes.len() as u64;
        let in_flight = self
            .sent
            .load(Ordering::SeqCst)
            .saturating_sub(self.done.load(Ordering::SeqCst));
        if in_flight > 0 && in_flight + len > INPUT_BUDGET {
            return Err(InputRefused::Backlog { in_flight });
        }
        self.sent.fetch_add(len, Ordering::SeqCst);
        self.input.send(bytes).map_err(|_| InputRefused::Gone)
    }

    /// The holder consumed `written` input bytes of this connection in total.
    fn acked(&self, written: u64) {
        self.done.fetch_max(written, Ordering::SeqCst);
    }
}

/// Why an attach subscription was dropped: the client fell behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lag {
    /// First output offset the client did not get.
    pub seq: u64,
    pub queued_bytes: u64,
}

/// One attached client's queue accounting (shared with its stream task).
#[derive(Default)]
pub struct SubShared {
    /// Output bytes sent into the channel and not yet taken by the stream task.
    pub queued: AtomicUsize,
    /// Set (once) when the subscription was dropped for lagging.
    pub lag: Mutex<Option<Lag>>,
}

struct Sub {
    tx: mpsc::UnboundedSender<StreamMsg>,
    shared: Arc<SubShared>,
}

/// The receiving side of an attach subscription.
pub struct AttachRx {
    pub rx: mpsc::UnboundedReceiver<StreamMsg>,
    pub shared: Arc<SubShared>,
}

impl AttachRx {
    /// Account a message taken from the queue.
    pub fn taken(&self, msg: &StreamMsg) {
        if let StreamMsg::Output { data, .. } = msg {
            self.shared.queued.fetch_sub(data.len(), Ordering::Relaxed);
        }
    }

    /// The lag that ended this subscription, if any.
    pub fn lag(&self) -> Option<Lag> {
        *self.shared.lag.lock().unwrap()
    }
}

/// What attached clients receive, in stream order.
#[derive(Clone, Debug)]
pub enum StreamMsg {
    Output { seq: u64, data: Arc<[u8]> },
    Resized { seq: u64, cols: u16, rows: u16 },
    Exited(ExitInfo),
}

struct TState {
    vt: Vt,
    exit: Option<ExitInfo>,
    lost: bool,
    subs: Vec<Sub>,
    /// Terminal queries seen in the output (for attach reply de-duplication).
    queries: QueryScanner,
}

impl TState {
    /// Send to every attached client. A client whose queue would pass the
    /// limit is dropped (its stream task reports `terminal.lagged`).
    fn broadcast(&mut self, msg: StreamMsg) {
        let limit = attach_queue_limit();
        self.subs.retain(|s| {
            if let StreamMsg::Output { seq, data } = &msg {
                let queued = s.shared.queued.load(Ordering::Relaxed);
                if queued + data.len() > limit {
                    *s.shared.lag.lock().unwrap() = Some(Lag {
                        seq: *seq,
                        queued_bytes: queued as u64,
                    });
                    return false; // dropping the sender ends the stream
                }
                s.shared.queued.fetch_add(data.len(), Ordering::Relaxed);
            }
            s.tx.send(msg.clone()).is_ok()
        });
    }
}

/// The JSON of a parsed hello (for the incompatible path).
fn foreign_hello(h: &HolderHello) -> serde_json::Value {
    serde_json::to_value(h).unwrap_or_default()
}

pub struct Terminal {
    pub id: String,
    pub name: Option<String>,
    pub argv: Vec<String>,
    pub cwd: String,
    pub created_ms: u64,
    pub pid: u32,
    pub holder_pid: u32,
    pub pipe: String,
    pub conpty: String,
    /// Parent terminal / task (v7), see `TerminalInfo.parent_terminal`.
    pub parent_terminal: Option<String>,
    pub parent_task: Option<String>,
    /// Holder protocol of this terminal's holder.
    pub holder_protocol: u32,
    /// Set when the holder speaks an unsupported protocol (v7).
    pub incompatible: Option<String>,
    /// Fired on every output batch, resize and exit.
    pub changed: Notify,
    /// Agent status engine (fed from the same stream, see `status.rs`).
    pub status: super::status::TermStatus,
    state: Mutex<TState>,
    to_holder: HolderTx,
    recent_sends: Mutex<VecDeque<String>>,
    /// Unix ms of the last output batch (0 = none since connect).
    last_output_ms: AtomicU64,
    /// Replies to terminal queries the attached clients are expected to send.
    expect: Mutex<Expectations>,
    /// What a person typed (attach input): unsent drafts (docs/19 짠4.3).
    human: Mutex<pod_orch::wake::DraftTracker>,
}

impl Terminal {
    /// Connect to a holder pipe and start mirroring it. `NotFound` means no
    /// holder serves that pipe (not started yet, or gone). Gives up with
    /// `TimedOut` after [`CONNECT_TIMEOUT`].
    pub async fn connect(pipe: &str) -> io::Result<Arc<Terminal>> {
        Self::connect_within(pipe, CONNECT_TIMEOUT).await
    }

    /// [`Terminal::connect`] with one deadline over everything: waiting for
    /// a free pipe instance (`ERROR_PIPE_BUSY`) and the handshake. A holder
    /// that never frees an instance or never says hello cannot hold up the
    /// caller (daemon-04).
    pub async fn connect_within(pipe: &str, limit: Duration) -> io::Result<Arc<Terminal>> {
        let deadline = tokio::time::Instant::now() + limit;
        let timed_out = || io::Error::new(io::ErrorKind::TimedOut, "holder connect timed out");
        let client = loop {
            match open_holder_pipe(pipe) {
                Ok(c) => break c,
                Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(timed_out());
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(e) => return Err(e),
            }
        };
        let (mut rd, wr) = tokio::io::split(client);
        let handshake = async {
            let hello = match read_frame(&mut rd).await? {
                Some(Frame::Hello(h)) => match check_holder_version(h.version) {
                    Ok(()) => h,
                    Err(why) => return Ok(Err((foreign_hello(&h), why))),
                },
                Some(Frame::ForeignHello(v)) => {
                    let version = v.get("version").and_then(|x| x.as_u64());
                    let why = match version {
                        Some(n) => check_holder_version(n as u32)
                            .err()
                            .unwrap_or_else(|| format!("unreadable hello for holder protocol {n}")),
                        None => "holder hello without a version".to_string(),
                    };
                    return Ok(Err((v, why)));
                }
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("expected hello, got {other:?}"),
                    ));
                }
            };
            let (seq, snapshot) = match read_frame(&mut rd).await? {
                Some(Frame::Snapshot { seq, data }) => (seq, data),
                other => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("expected snapshot, got {other:?}"),
                    ));
                }
            };
            Ok(Ok((hello, seq, snapshot)))
        };
        let handshake = tokio::time::timeout_at(deadline, handshake)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "holder handshake timed out"))??;
        let (hello, seq, snapshot) = match handshake {
            Ok(x) => x,
            // Dropping the connection leaves the holder (and its child) running.
            Err((hello, why)) => return Terminal::incompatible(pipe, &hello, why),
        };

        // Holders >= 3 acknowledge input once they get our hello.
        let tx = HolderTx::spawn(wr, hello.version >= 3);
        if hello.version >= 2 {
            // Version exchange; holders < 2 would reject the unknown frame kind.
            tx.control(Frame::DaemonHello(DaemonHello {
                version: HOLDER_PROTOCOL,
            }));
        }
        let status =
            super::status::TermStatus::new(&hello.id, &hello.argv, hello.child_pid, &snapshot);
        let term = Arc::new(Terminal {
            status,
            id: hello.id,
            name: hello.name,
            argv: hello.argv,
            cwd: hello.cwd,
            created_ms: hello.created_ms,
            pid: hello.child_pid,
            holder_pid: hello.holder_pid,
            pipe: pipe.to_string(),
            conpty: hello.conpty,
            parent_terminal: hello.parent_terminal,
            parent_task: hello.parent_task,
            holder_protocol: hello.version,
            incompatible: None,
            changed: Notify::new(),
            state: Mutex::new(TState {
                vt: Vt::from_snapshot(hello.cols, hello.rows, seq, &snapshot),
                exit: None,
                lost: false,
                subs: Vec::new(),
                queries: QueryScanner::default(),
            }),
            to_holder: tx,
            recent_sends: Mutex::new(VecDeque::new()),
            last_output_ms: AtomicU64::new(0),
            expect: Mutex::new(Expectations::default()),
            human: Mutex::new(Default::default()),
        });
        let t = term.clone();
        tokio::spawn(async move {
            loop {
                match read_frame(&mut rd).await {
                    Ok(Some(f)) => t.on_frame(f),
                    Ok(None) => break,
                    Err(e) => {
                        super::log(&format!("{}: holder stream error: {e}", t.id));
                        break;
                    }
                }
            }
            t.on_holder_gone();
        });
        Ok(term)
    }

    /// A terminal whose holder speaks a protocol this daemon cannot drive:
    /// listed as exited with `incompatible_holder`; nothing is sent to it.
    fn incompatible(
        pipe: &str,
        hello: &serde_json::Value,
        why: String,
    ) -> io::Result<Arc<Terminal>> {
        let str_of = |k: &str| hello.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let u64_of = |k: &str| hello.get(k).and_then(|v| v.as_u64());
        let Some(id) = str_of("id") else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("incompatible holder without an id ({why})"),
            ));
        };
        let version = u64_of("version").unwrap_or(0) as u32;
        let argv: Vec<String> = hello
            .get("argv")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let cols = u64_of("cols").unwrap_or(120).clamp(2, 1000) as u16;
        let rows = u64_of("rows").unwrap_or(30).clamp(2, 500) as u16;
        let child_pid = u64_of("child_pid").unwrap_or(0) as u32;
        let reason = format!("incompatible holder: {why}");
        super::log(&format!("{id}: {reason} (pipe {pipe})"));
        let mut vt = Vt::new(cols, rows, None);
        vt.feed(
            format!(
                "[pod: {reason}. The terminal keeps running in its holder, but this podd \
                 cannot show or drive it. Use the matching podd, or `pod rm --force {id}` \
                 to end it.]\r\n"
            )
            .as_bytes(),
        );
        let tx = HolderTx::closed();
        let status = super::status::TermStatus::new(&id, &argv, child_pid, b"");
        status.on_exit();
        Ok(Arc::new(Terminal {
            status,
            name: str_of("name"),
            argv,
            cwd: str_of("cwd").unwrap_or_default(),
            created_ms: u64_of("created_ms").unwrap_or(0),
            pid: child_pid,
            holder_pid: u64_of("holder_pid").unwrap_or(0) as u32,
            pipe: pipe.to_string(),
            conpty: str_of("conpty").unwrap_or_default(),
            parent_terminal: str_of("parent_terminal"),
            parent_task: str_of("parent_task"),
            holder_protocol: version,
            incompatible: Some(reason),
            changed: Notify::new(),
            state: Mutex::new(TState {
                vt,
                exit: Some(ExitInfo {
                    seq: 0,
                    exit_code: None,
                    killed: false,
                    exited_ms: crate::now_ms(),
                }),
                lost: false,
                subs: Vec::new(),
                queries: QueryScanner::default(),
            }),
            to_holder: tx,
            recent_sends: Mutex::new(VecDeque::new()),
            last_output_ms: AtomicU64::new(0),
            expect: Mutex::new(Expectations::default()),
            human: Mutex::new(Default::default()),
            id,
        }))
    }

    fn on_frame(&self, frame: Frame) {
        let mut exited = false;
        let mut st = self.state.lock().unwrap();
        match frame {
            Frame::Output { seq, data } => {
                if seq != st.vt.seq() {
                    super::log(&format!(
                        "{}: output gap: expected seq {}, got {seq}",
                        self.id,
                        st.vt.seq()
                    ));
                }
                st.vt.feed(&data);
                // Terminal queries: each attached client may answer; record
                // them so duplicate and late replies are dropped.
                let clients = st.subs.len();
                if clients > 0 {
                    let now = crate::now_ms();
                    let mut expect = self.expect.lock().unwrap();
                    st.queries.scan(&data, |r| expect.expect(r, clients, now));
                } else {
                    st.queries.scan(&data, |_| {});
                }
                self.last_output_ms
                    .store(crate::now_ms(), Ordering::Relaxed);
                self.status.on_output(&data);
                st.broadcast(StreamMsg::Output {
                    seq,
                    data: data.into(),
                });
            }
            Frame::Resized { seq, cols, rows } => {
                st.vt.resize(cols, rows);
                st.broadcast(StreamMsg::Resized { seq, cols, rows });
            }
            Frame::InputAck { written } => {
                self.to_holder.acked(written);
                return;
            }
            Frame::Exited(info) => {
                st.exit = Some(info.clone());
                st.broadcast(StreamMsg::Exited(info));
                self.status.on_exit();
                exited = true;
            }
            other => super::log(&format!(
                "{}: unexpected frame from holder: {other:?}",
                self.id
            )),
        }
        drop(st);
        self.changed.notify_waiters();
        if exited {
            self.publish_exited();
        }
    }

    /// v9 `terminal.changed exited` (no stream lock held).
    fn publish_exited(&self) {
        super::status::publish_terminal(
            &self.id,
            crate::protocol::TerminalChange::Exited,
            Some(self.info()),
        );
    }

    /// The holder connection ended. If it never reported an exit, it died.
    fn on_holder_gone(&self) {
        let mut st = self.state.lock().unwrap();
        let lost = st.exit.is_none();
        if lost {
            let info = ExitInfo {
                seq: st.vt.seq(),
                exit_code: None,
                killed: false,
                exited_ms: crate::now_ms(),
            };
            st.exit = Some(info.clone());
            st.lost = true;
            st.broadcast(StreamMsg::Exited(info));
            self.status.on_exit();
        }
        st.subs.clear();
        drop(st);
        self.changed.notify_waiters();
        if lost {
            self.publish_exited();
        }
    }

    /// Unix ms of the last output (0 = none since the daemon connected).
    pub fn last_output_ms(&self) -> u64 {
        self.last_output_ms.load(Ordering::Relaxed)
    }

    pub fn is_running(&self) -> bool {
        self.state.lock().unwrap().exit.is_none()
    }

    pub fn size(&self) -> (u16, u16) {
        self.state.lock().unwrap().vt.size()
    }

    pub fn info(&self) -> TerminalInfo {
        let mut st = self.state.lock().unwrap();
        st.subs.retain(|s| !s.tx.is_closed());
        let (cols, rows) = st.vt.size();
        TerminalInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            argv: self.argv.clone(),
            cwd: self.cwd.clone(),
            state: if st.exit.is_some() {
                TerminalState::Exited
            } else {
                TerminalState::Running
            },
            exit_code: st.exit.as_ref().and_then(|e| e.exit_code),
            pid: Some(self.pid),
            holder_pid: Some(self.holder_pid),
            cols,
            rows,
            created_ms: self.created_ms,
            exited_ms: st.exit.as_ref().map(|e| e.exited_ms),
            killed: st.exit.as_ref().is_some_and(|e| e.killed),
            lost: st.lost,
            attached: st.subs.len(),
            conpty: Some(self.conpty.clone()),
            status: Some(self.status.info()),
            parent_terminal: self.parent_terminal.clone(),
            parent_task: self.parent_task.clone(),
            incompatible_holder: self.incompatible.clone(),
            unreachable: None,
        }
    }

    /// Send a control frame (resize, kill, release): never behind input.
    pub fn send_to_holder(&self, frame: Frame) -> bool {
        self.to_holder.control(frame)
    }

    /// Type into the terminal. The status engine sees it too: answering a
    /// hook permission prompt ends `blocked` (docs/12 B2).
    /// False when the input was refused (see [`Terminal::try_input`]).
    pub fn input(&self, bytes: Vec<u8>) -> bool {
        self.try_input(bytes).is_ok()
    }

    /// [`Terminal::input`] that says why input was refused: the program has
    /// not read [`INPUT_BUDGET`] bytes yet, or the holder is gone.
    pub fn try_input(&self, bytes: Vec<u8>) -> Result<(), InputRefused> {
        self.status.on_input(&bytes);
        self.to_holder.try_input(bytes)
    }

    /// Raw input from an attach client (`terminal.input`): duplicate and late
    /// replies to terminal queries are removed first (see `query_dedup`).
    /// Returns (bytes forwarded, replies dropped).
    pub fn attach_input(&self, bytes: &[u8]) -> Result<(usize, Filtered), InputRefused> {
        let (bytes, dropped) = self
            .expect
            .lock()
            .unwrap()
            .filter_input(bytes, crate::now_ms());
        let n = bytes.len();
        if n > 0 {
            self.try_input(bytes)?;
        }
        Ok((n, dropped))
    }

    /// Record input a person sent from an attach client. True when they typed
    /// (not just focus reports or query replies).
    pub fn note_human_input(&self, bytes: &[u8]) -> bool {
        self.human.lock().unwrap().feed(bytes, crate::now_ms())
    }

    /// A person typed into the input box and has not sent it.
    pub fn human_draft(&self) -> bool {
        self.human.lock().unwrap().draft()
    }

    /// Remember sent text so its echo does not satisfy `output_contains`.
    pub fn remember_sent(&self, text: &str) {
        let mut recent = self.recent_sends.lock().unwrap();
        for line in text
            .split(['\r', '\n'])
            .map(str::trim)
            .filter(|l| !l.is_empty())
        {
            recent.push_back(line.to_string());
            while recent.len() > RECENT_SENDS {
                recent.pop_front();
            }
        }
    }

    pub fn app_cursor(&self) -> bool {
        self.state.lock().unwrap().vt.app_cursor()
    }

    pub fn bracketed_paste(&self) -> bool {
        self.state.lock().unwrap().vt.bracketed_paste()
    }

    pub fn read_lines(&self, mode: ReadMode) -> (Vec<String>, (u16, u16)) {
        self.state.lock().unwrap().vt.read_lines(mode)
    }

    /// Lines without a grey prompt suggestion at the caret, and the
    /// suggestion (docs/19 §18.3; [`crate::vt::Vt::read_lines_with`]).
    pub fn read_lines_for_agents(
        &self,
        mode: ReadMode,
    ) -> (Vec<String>, (u16, u16), Option<String>) {
        self.state.lock().unwrap().vt.read_lines_with(mode, true)
    }

    /// Whether scrollback + screen contains `needle` outside the echo of text
    /// sent with `terminal.send`: occurrences of the needle that lie inside a
    /// recently sent line (e.g. `PS> echo pod-ok`) are blanked out first.
    pub fn output_contains(&self, needle: &str) -> bool {
        let echoes: Vec<String> = self
            .recent_sends
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.contains(needle))
            .cloned()
            .collect();
        let (lines, _) = self.read_lines(ReadMode::History);
        let strip = |l: &str| {
            echoes
                .iter()
                .fold(l.to_string(), |acc, t| acc.replace(t.as_str(), "\u{0}"))
        };
        if needle.contains('\n') {
            return strip(&lines.join("\n")).contains(needle);
        }
        lines
            .iter()
            .any(|l| l.contains(needle) && strip(l).contains(needle))
    }

    /// Snapshot + subscription, atomically. Returns (snapshot ANSI, seq, receiver).
    pub fn attach(&self, scrollback: usize) -> (Vec<u8>, u64, AttachRx) {
        let (tx, rx) = mpsc::unbounded_channel();
        let shared = Arc::new(SubShared::default());
        let mut st = self.state.lock().unwrap();
        let snapshot = st.vt.snapshot_ansi(scrollback);
        let seq = st.vt.seq();
        match &st.exit {
            Some(e) => {
                let _ = tx.send(StreamMsg::Exited(e.clone()));
            }
            None => st.subs.push(Sub {
                tx,
                shared: shared.clone(),
            }),
        }
        (snapshot, seq, AttachRx { rx, shared })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqos_probe::{IDENTIFICATION, pipe_name, serve_once};
    use std::time::Instant;
    use tokio::net::windows::named_pipe::ServerOptions;

    fn timed_out(r: io::Result<Arc<Terminal>>) -> bool {
        matches!(r, Err(e) if e.kind() == io::ErrorKind::TimedOut)
    }

    /// Every frame in `bytes` (encoded), in order.
    async fn frames_of(mut bytes: &[u8]) -> Vec<Frame> {
        let mut out = Vec::new();
        while let Some(f) = read_frame(&mut bytes).await.unwrap() {
            out.push(f);
        }
        out
    }

    /// daemon-01: a kill queued behind input that the holder is not reading
    /// is written before the rest of that input.
    #[tokio::test]
    async fn control_frames_overtake_queued_input() {
        use tokio::io::AsyncReadExt;
        let (near, mut far) = tokio::io::duplex(256);
        let tx = HolderTx::spawn(near, true);
        for _ in 0..4 {
            tx.try_input(vec![b'a'; 1000]).unwrap();
        }
        assert!(tx.control(Frame::Kill));
        drop(tx);
        let mut all = Vec::new();
        far.read_to_end(&mut all).await.unwrap();
        let frames = frames_of(&all).await;
        assert_eq!(frames.len(), 5);
        let kill = frames.iter().position(|f| *f == Frame::Kill).unwrap();
        assert!(kill < 4, "kill waited for all input: {kill}");
    }

    /// daemon-01: input in flight is capped at INPUT_BUDGET until the
    /// holder acknowledges it; a refusal says how much is waiting.
    #[tokio::test]
    async fn input_waits_for_acknowledgement_past_the_budget() {
        let (near, _far) = tokio::io::duplex(1 << 20);
        let tx = HolderTx::spawn(near, true);
        let chunk = (INPUT_BUDGET / 4) as usize;
        for _ in 0..4 {
            tx.try_input(vec![0; chunk]).unwrap();
        }
        assert_eq!(
            tx.try_input(vec![0; 1]),
            Err(InputRefused::Backlog {
                in_flight: INPUT_BUDGET
            })
        );
        tx.acked(chunk as u64);
        tx.try_input(vec![0; chunk]).unwrap();
        tx.acked(5 * chunk as u64);
        // Nothing in flight: one input larger than the budget still goes.
        tx.try_input(vec![0; INPUT_BUDGET as usize + 1]).unwrap();
    }

    /// Without acknowledgements (holder protocol < 3) input counts as done
    /// once it is written to the pipe.
    #[tokio::test]
    async fn old_holders_count_written_input_as_done() {
        use tokio::io::AsyncReadExt;
        let (near, mut far) = tokio::io::duplex(1 << 16);
        let tx = HolderTx::spawn(near, false);
        let chunk = INPUT_BUDGET as usize;
        tx.try_input(vec![0; chunk]).unwrap();
        let reader = tokio::spawn(async move {
            let mut sink = Vec::new();
            let _ = far.read_to_end(&mut sink).await;
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while tx.try_input(vec![0; 1]).is_err() {
            assert!(
                Instant::now() < deadline,
                "written input never counted as done"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        drop(tx);
        reader.await.unwrap();
    }

    /// security-01: a holder (or whoever serves its pipe name) may identify
    /// the daemon, never impersonate it.
    #[test]
    fn holder_pipe_client_allows_identification_only() {
        let name = pipe_name("holder");
        let server = serve_once(&name);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut c = open_holder_pipe(&name).expect("open");
            c.write_all(b"x").await.unwrap();
        });
        assert_eq!(server.join().unwrap(), IDENTIFICATION);
    }

    /// daemon-04: a holder whose only pipe instance stays taken
    /// (`ERROR_PIPE_BUSY` forever) does not hold the caller past the deadline.
    #[tokio::test]
    async fn busy_holder_pipe_gives_up_at_the_deadline() {
        let name = pipe_name("busy");
        let _server = ServerOptions::new()
            .first_pipe_instance(true)
            .max_instances(1)
            .create(&name)
            .unwrap();
        let _occupant = ClientOptions::new().open(&name).expect("first client");
        let t0 = Instant::now();
        let r = Terminal::connect_within(&name, Duration::from_millis(300)).await;
        assert!(timed_out(r), "a busy pipe must time out");
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    }

    /// daemon-04: the same deadline covers the handshake (a holder that
    /// accepts but never says hello).
    #[tokio::test]
    async fn silent_holder_gives_up_at_the_deadline() {
        let name = pipe_name("silent");
        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)
            .unwrap();
        let hold = tokio::spawn(async move {
            let _ = server.connect().await;
            tokio::time::sleep(Duration::from_secs(30)).await;
            drop(server);
        });
        let t0 = Instant::now();
        let r = Terminal::connect_within(&name, Duration::from_millis(300)).await;
        assert!(timed_out(r), "a silent holder must time out");
        assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
        hold.abort();
    }

    /// Test support for security-01: a fake pipe server that reports how far
    /// its client lets it impersonate (the same probe as pod-client's own
    /// transport tests).
    mod sqos_probe {
        use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Security::{
            GetTokenInformation, RevertToSelf, TOKEN_QUERY, TokenImpersonationLevel,
        };
        use windows_sys::Win32::Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile};
        use windows_sys::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, ImpersonateNamedPipeClient, PIPE_READMODE_BYTE,
            PIPE_TYPE_BYTE, PIPE_WAIT,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

        /// `SecurityIdentification` (`SECURITY_IMPERSONATION_LEVEL`).
        pub const IDENTIFICATION: i32 = 1;

        /// Serve `name` once: wait for a client, read one byte, impersonate it and
        /// return the impersonation level of the token we got (`-1` if none).
        pub fn serve_once(name: &str) -> std::thread::JoinHandle<i32> {
            let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: `wide` is NUL-terminated; default security.
            let h = unsafe {
                CreateNamedPipeW(
                    wide.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    1,
                    4096,
                    4096,
                    0,
                    std::ptr::null(),
                )
            };
            assert_ne!(h, INVALID_HANDLE_VALUE, "CreateNamedPipeW");
            let h = h as usize;
            std::thread::spawn(move || {
                let h = h as HANDLE;
                let mut level = -1i32;
                // SAFETY: plain Win32 calls on handles owned by this thread.
                unsafe {
                    ConnectNamedPipe(h, std::ptr::null_mut());
                    let mut b = [0u8; 1];
                    let mut n = 0u32;
                    ReadFile(h, b.as_mut_ptr(), 1, &mut n, std::ptr::null_mut());
                    if ImpersonateNamedPipeClient(h) != 0 {
                        let mut tok: HANDLE = std::ptr::null_mut();
                        if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut tok) != 0 {
                            let mut len = 0u32;
                            GetTokenInformation(
                                tok,
                                TokenImpersonationLevel,
                                (&mut level as *mut i32).cast(),
                                4,
                                &mut len,
                            );
                            CloseHandle(tok);
                        }
                        RevertToSelf();
                    }
                    CloseHandle(h);
                }
                level
            })
        }

        pub fn pipe_name(tag: &str) -> String {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            format!(r"\\.\pipe\pod-sqos-{tag}-{}-{nanos}", std::process::id())
        }
    }
}
