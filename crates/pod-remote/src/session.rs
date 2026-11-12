//! Remote sessions that survive bad networks (docs/17 R4, docs/22 TM-13/14).
//!
//! The pieces the app's machine view (card rm-r4ui) builds on:
//!
//! - [`Connector`] opens one connection and runs the v12 handshake:
//!   [`SshConnector`] through the system ssh, tests through a loopback proxy.
//!   [`Link`] is that connection plus a [`Closer`] that ends it from another
//!   thread (kill ssh, shut the socket), which is how a call that waits too
//!   long, or a user's "close", unblocks a reader.
//! - [`RemoteControl`] is the control connection: one RPC at a time with a
//!   deadline. **Nothing is ever resent.** A lost answer is
//!   [`RemoteError::Unknown`]; while the connection is down every call is
//!   refused without being sent ([`RemoteError::Disconnected`], input
//!   disabled). [`RemoteControl::reconnect`] retries with backoff, checks the
//!   machine and boot ids, and resolves a lost `terminal.input_once` by
//!   asking the daemon (`check`, which also fences it) instead of typing it
//!   again.
//! - [`RemoteAttach`] is one terminal view: its own connection and thread,
//!   bounded event channel, automatic re-attach after a drop or
//!   `terminal.lagged`. Each (re)attach runs machine/boot check → full
//!   terminal list → attach snapshot, and is reported as a [`Resync`] with a
//!   [`Gap`] when output was never received, or [`ResyncReason::BootChanged`]
//!   when the daemon restarted (old offsets are then not compared).
//! - [`AttachSlots`] caps concurrent attaches per machine (4, docs/17 §2.3).
//!
//! Everything is blocking std code (no tokio): the app runs it on its own
//! threads and maps the events to its views.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use podd::client::{AttachEvent, Client, ClientError};
use podd::protocol::*;

use crate::machine::Machine;

/// Ends a connection from any thread; idempotent.
pub type Closer = Arc<dyn Fn() + Send + Sync>;

/// One connected, handshaken connection.
pub struct Link {
    pub client: Client,
    pub hello: HelloResult,
    closer: Closer,
}

impl Link {
    pub fn new(client: Client, hello: HelloResult, closer: Closer) -> Self {
        Self {
            client,
            hello,
            closer,
        }
    }

    pub fn closer(&self) -> Closer {
        self.closer.clone()
    }
}

/// Why a connection could not be opened.
#[derive(Debug, Clone)]
pub struct ConnectError {
    pub message: String,
    /// Retrying cannot help (host key, authentication, version, profile):
    /// the user has to act. Shown instead of retried.
    pub permanent: bool,
}

/// Opens connections to one machine.
pub trait Connector: Send + Sync {
    /// One new connection after `server.status` + `connection.hello`.
    fn connect(&self) -> Result<Link, ConnectError>;
}

/// The system-ssh connector for a machine profile.
pub struct SshConnector {
    pub machine: Machine,
}

impl Connector for SshConnector {
    fn connect(&self) -> Result<Link, ConnectError> {
        use crate::ssh::ErrorKind as K;
        match crate::ssh::connect(&self.machine) {
            Ok(c) => Ok(c.into_link()),
            Err(e) => Err(ConnectError {
                permanent: !matches!(e.kind, K::Timeout | K::Unreachable | K::Failed),
                message: e.to_string(),
            }),
        }
    }
}

/// Retry timing (docs/17 §2.3: 1 → 2 → 4 … at most 30 s, with jitter, until
/// the user cancels).
#[derive(Debug, Clone)]
pub struct ReconnectPolicy {
    pub first_delay: Duration,
    pub max_delay: Duration,
    /// ± this fraction of each delay, so many views do not retry in step.
    pub jitter: f64,
    /// Stop retrying after this long; `None`: until cancelled.
    pub give_up_after: Option<Duration>,
    /// A call (or attach reply) without an answer for this long ends the
    /// connection: it counts as a drop.
    pub call_timeout: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            first_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            jitter: 0.2,
            give_up_after: None,
            call_timeout: Duration::from_secs(20),
        }
    }
}

impl ReconnectPolicy {
    /// The wait before retry number `attempt` (0-based); `unit` in [0, 1)
    /// picks the jitter.
    pub fn delay(&self, attempt: u32, unit: f64) -> Duration {
        let base = self
            .first_delay
            .saturating_mul(1u32 << attempt.min(16))
            .min(self.max_delay);
        let f = 1.0 + self.jitter * (2.0 * unit.clamp(0.0, 1.0) - 1.0);
        base.mul_f64(f.max(0.0))
    }
}

/// A small xorshift for jitter (no dependency, not for secrets).
struct Jitter(u64);

impl Jitter {
    fn new() -> Self {
        let t = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(1);
        Self(t ^ (u64::from(std::process::id()) << 32) | 1)
    }

    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Fires `closer` unless disarmed within `timeout`.
struct Watchdog {
    cancel: mpsc::Sender<()>,
    fired: Arc<AtomicBool>,
}

impl Watchdog {
    fn arm(timeout: Duration, closer: Closer) -> Self {
        let (cancel, rx) = mpsc::channel::<()>();
        let fired = Arc::new(AtomicBool::new(false));
        let f = fired.clone();
        std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = rx.recv_timeout(timeout) {
                f.store(true, Ordering::SeqCst);
                closer();
            }
        });
        Self { cancel, fired }
    }

    /// Disarm; true when it had already fired.
    fn disarm(self) -> bool {
        let _ = self.cancel.send(());
        self.fired.load(Ordering::SeqCst)
    }
}

/// What a remote call can end in, besides its result.
#[derive(Debug, Clone)]
pub enum RemoteError {
    /// The daemon answered with an error (nothing more to know).
    Rpc(RpcError),
    /// Not connected: nothing was sent. Input is disabled until
    /// [`RemoteControl::reconnect`] succeeds.
    Disconnected { since: SystemTime, reason: String },
    /// Sent, but the answer was lost: the result is unknown and the request
    /// is **not** resent. For input, [`RemoteControl::reconnect`] resolves it.
    Unknown(String),
    /// Retrying cannot help: another machine answered, incompatible
    /// versions, a permanent ssh failure, or the policy gave up.
    Refused(String),
    /// The caller cancelled the reconnect.
    Cancelled,
}

impl std::fmt::Display for RemoteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RemoteError::Rpc(e) => write!(f, "{}", e.message),
            RemoteError::Disconnected { reason, .. } => {
                write!(f, "연결이 끊겨 보내지 않았습니다: {reason}")
            }
            RemoteError::Unknown(m) => write!(f, "응답을 받지 못해 결과를 알 수 없습니다: {m}"),
            RemoteError::Refused(m) => write!(f, "{m}"),
            RemoteError::Cancelled => f.write_str("재접속을 취소했습니다"),
        }
    }
}

impl std::error::Error for RemoteError {}

/// "Another machine answered": never talk to it under the old profile
/// (docs/17 §1, docs/22 TM-13).
fn machine_changed(was: &str, now: &str) -> String {
    format!(
        "다른 머신이 응답했습니다(machine_id {was} → {now}). 기존 작업에 자동으로 연결하지 않습니다. \
         호스트 키와 머신을 확인한 뒤 프로필을 다시 연결하세요"
    )
}

/// An input whose answer was lost.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingInput {
    pub target: TerminalRef,
    pub stream: String,
    pub seq: u64,
    /// The daemon start it was sent to; only that start can answer.
    pub boot_id: String,
    /// `false` when the daemon has no `terminal.input_once` (older than
    /// v20): the result cannot be found out.
    pub checkable: bool,
}

/// What became of a lost input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputResolution {
    /// It reached the program (`bytes` written); do not type it again.
    Applied { seq: u64, bytes: usize },
    /// It never reached the program and never will (fenced).
    NotApplied { seq: u64 },
    /// It cannot be known (the daemon restarted, or is older than v20).
    Unknown { seq: u64, reason: String },
}

/// Result of [`RemoteControl::reconnect`].
#[derive(Debug, Clone)]
pub struct Reconnected {
    pub hello: HelloResult,
    /// The daemon restarted: event sequences restarted with it. Fetch the
    /// full state again before attaching.
    pub boot_changed: bool,
    pub attempts: u32,
    pub input: Option<InputResolution>,
}

/// The control connection of one machine.
pub struct RemoteControl {
    connector: Arc<dyn Connector>,
    policy: ReconnectPolicy,
    link: Option<Link>,
    hello: HelloResult,
    stream: String,
    next_seq: u64,
    pending: Option<PendingInput>,
    down: Option<(SystemTime, String)>,
    jitter: Jitter,
}

/// A random input stream name.
fn new_stream() -> String {
    let mut j = Jitter::new();
    let a = (j.unit() * (1u64 << 52) as f64) as u64;
    let b = (j.unit() * (1u64 << 52) as f64) as u64;
    format!("r{a:013x}{b:013x}")
}

impl RemoteControl {
    /// Connect once (no retry: the first failure is shown as is).
    pub fn connect(
        connector: Arc<dyn Connector>,
        policy: ReconnectPolicy,
    ) -> Result<Self, RemoteError> {
        let link = connector.connect().map_err(|e| {
            if e.permanent {
                RemoteError::Refused(e.message)
            } else {
                RemoteError::Disconnected {
                    since: SystemTime::now(),
                    reason: e.message,
                }
            }
        })?;
        Ok(Self {
            connector,
            policy,
            hello: link.hello.clone(),
            link: Some(link),
            stream: new_stream(),
            next_seq: 1,
            pending: None,
            down: None,
            jitter: Jitter::new(),
        })
    }

    /// The handshake of the current (or last) connection.
    pub fn hello(&self) -> &HelloResult {
        &self.hello
    }

    pub fn is_connected(&self) -> bool {
        self.link.is_some()
    }

    /// When the connection dropped and why (`None` while connected).
    pub fn disconnected(&self) -> Option<(SystemTime, &str)> {
        self.down.as_ref().map(|(t, r)| (*t, r.as_str()))
    }

    /// The input whose answer was lost, until a reconnect resolves it.
    pub fn pending_input(&self) -> Option<&PendingInput> {
        self.pending.as_ref()
    }

    /// The input stream of this session (for logs and tests).
    pub fn input_stream(&self) -> &str {
        &self.stream
    }

    /// Mark the connection lost (e.g. an attach view saw the machine drop).
    pub fn mark_down(&mut self, reason: impl Into<String>) {
        if let Some(l) = self.link.take() {
            (l.closer)();
        }
        if self.down.is_none() {
            self.down = Some((SystemTime::now(), reason.into()));
        }
    }

    fn refuse_while_down(&self) -> Result<(), RemoteError> {
        match (&self.link, &self.down) {
            (Some(_), _) => Ok(()),
            (None, Some((since, reason))) => Err(RemoteError::Disconnected {
                since: *since,
                reason: reason.clone(),
            }),
            (None, None) => Err(RemoteError::Disconnected {
                since: SystemTime::now(),
                reason: "not connected".into(),
            }),
        }
    }

    /// One call with the policy's deadline. A transport failure (including
    /// the deadline) drops the connection and is [`RemoteError::Unknown`]:
    /// the request may or may not have run; it is not resent.
    pub fn call<M: Method>(&mut self, params: &M::Params) -> Result<M::Result, RemoteError> {
        self.refuse_while_down()?;
        let link = self.link.as_mut().expect("connected");
        let dog = Watchdog::arm(self.policy.call_timeout, link.closer.clone());
        let r = link.client.call::<M>(params);
        let fired = dog.disarm();
        match r {
            Ok(v) => Ok(v),
            Err(ClientError::Rpc(e)) => Err(RemoteError::Rpc(e)),
            Err(e) => {
                let reason = if fired {
                    format!(
                        "{} 안에 응답이 없어 연결을 닫았습니다",
                        secs(self.policy.call_timeout)
                    )
                } else {
                    e.to_string()
                };
                self.mark_down(reason.clone());
                Err(RemoteError::Unknown(format!("{}: {reason}", M::NAME)))
            }
        }
    }

    /// Raw input with an identity (`terminal.input_once`). Refused without
    /// sending while disconnected. When the answer is lost the input is kept
    /// as [`RemoteControl::pending_input`] and **not** resent; the next
    /// [`RemoteControl::reconnect`] finds out whether it arrived.
    pub fn input(
        &mut self,
        target: &TerminalRef,
        bytes: &[u8],
    ) -> Result<InputOnceResult, RemoteError> {
        self.refuse_while_down()?;
        let seq = self.next_seq;
        self.next_seq += 1;
        let checkable = self.hello.protocol >= CHUNKED_ATTACH_PROTOCOL;
        let pending = PendingInput {
            target: target.clone(),
            stream: self.stream.clone(),
            seq,
            boot_id: self.hello.boot_id.clone(),
            checkable,
        };
        let r = if checkable {
            self.call::<TerminalInputOnce>(&InputOnceParams {
                target: target.clone(),
                data: b64_encode(bytes),
                stream: self.stream.clone(),
                seq,
                check: false,
            })
        } else {
            // An older daemon: plain input, no identity to ask about later.
            self.call::<TerminalInput>(&InputParams {
                target: target.clone(),
                data: b64_encode(bytes),
            })
            .map(|r| InputOnceResult {
                id: r.id,
                outcome: InputOnceOutcome::Written,
                bytes: r.bytes,
                dropped_replies: r.dropped_replies,
            })
        };
        if let Err(RemoteError::Unknown(_)) = &r {
            self.pending = Some(pending);
        }
        r
    }

    /// Reconnect with backoff until it works, `cancel` is set, the policy
    /// gives up, or a permanent error. Order (docs/22 TM-14): machine id
    /// (another machine: refused, nothing sent), boot id, then the lost
    /// input is resolved by `check` on the same daemon start. The caller
    /// then fetches the full state before re-attaching views.
    pub fn reconnect(&mut self, cancel: &AtomicBool) -> Result<Reconnected, RemoteError> {
        if self.link.is_some() {
            self.mark_down("reconnect requested");
        }
        let started = Instant::now();
        let mut attempt = 0u32;
        let link = loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(RemoteError::Cancelled);
            }
            match self.connector.connect() {
                Ok(l) => break l,
                Err(e) if e.permanent => return Err(RemoteError::Refused(e.message)),
                Err(e) => {
                    if let Some(down) = &mut self.down {
                        down.1 = e.message;
                    }
                }
            }
            let wait = self.policy.delay(attempt, self.jitter.unit());
            attempt += 1;
            if let Some(limit) = self.policy.give_up_after
                && started.elapsed() + wait > limit
            {
                return Err(RemoteError::Refused(format!(
                    "{} 동안 다시 연결하지 못했습니다",
                    secs(limit)
                )));
            }
            if !sleep_unless(wait, cancel) {
                return Err(RemoteError::Cancelled);
            }
        };
        if link.hello.machine_id != self.hello.machine_id {
            (link.closer)();
            return Err(RemoteError::Refused(machine_changed(
                &self.hello.machine_id,
                &link.hello.machine_id,
            )));
        }
        let boot_changed = link.hello.boot_id != self.hello.boot_id;
        self.hello = link.hello.clone();
        self.link = Some(link);
        self.down = None;
        let input = match self.pending.take() {
            None => None,
            Some(p) => Some(self.resolve(p)?),
        };
        Ok(Reconnected {
            hello: self.hello.clone(),
            boot_changed,
            attempts: attempt + 1,
            input,
        })
    }

    fn resolve(&mut self, p: PendingInput) -> Result<InputResolution, RemoteError> {
        if !p.checkable {
            return Ok(InputResolution::Unknown {
                seq: p.seq,
                reason: "원격 Pod가 v20보다 오래되어 입력 결과를 확인할 수 없습니다".into(),
            });
        }
        if p.boot_id != self.hello.boot_id {
            return Ok(InputResolution::Unknown {
                seq: p.seq,
                reason: "원격 데몬이 다시 시작되어 입력 결과를 확인할 수 없습니다".into(),
            });
        }
        let r = self.call::<TerminalInputOnce>(&InputOnceParams {
            target: p.target.clone(),
            data: String::new(),
            stream: p.stream.clone(),
            seq: p.seq,
            check: true,
        });
        match r {
            Ok(r) if r.outcome == InputOnceOutcome::Fenced => {
                Ok(InputResolution::NotApplied { seq: p.seq })
            }
            Ok(r) => Ok(InputResolution::Applied {
                seq: p.seq,
                bytes: r.bytes,
            }),
            Err(RemoteError::Rpc(e)) => Ok(InputResolution::Unknown {
                seq: p.seq,
                reason: e.message,
            }),
            Err(e) => {
                // Lost again: keep it for the next reconnect.
                self.pending = Some(p);
                Err(e)
            }
        }
    }
}

fn secs(d: Duration) -> String {
    format!("{:.0}초", d.as_secs_f64())
}

/// Sleep `d` in short steps; false when `cancel` was set.
fn sleep_unless(d: Duration, cancel: &AtomicBool) -> bool {
    let end = Instant::now() + d;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return false;
        }
        let now = Instant::now();
        if now >= end {
            return true;
        }
        std::thread::sleep((end - now).min(Duration::from_millis(20)));
    }
}

// ───────────────────────────── attach ─────────────────────────────

/// Caps concurrent attaches to one machine (docs/17 §2.3: 4; the rest of
/// the terminals show status only).
#[derive(Clone)]
pub struct AttachSlots {
    used: Arc<AtomicUsize>,
    max: usize,
}

impl AttachSlots {
    pub const DEFAULT_MAX: usize = 4;

    pub fn new(max: usize) -> Self {
        Self {
            used: Arc::default(),
            max,
        }
    }

    pub fn in_use(&self) -> usize {
        self.used.load(Ordering::SeqCst)
    }

    fn take(&self) -> Option<SlotGuard> {
        let mut n = self.used.load(Ordering::SeqCst);
        loop {
            if n >= self.max {
                return None;
            }
            match self
                .used
                .compare_exchange(n, n + 1, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => return Some(SlotGuard(self.used.clone())),
                Err(now) => n = now,
            }
        }
    }
}

impl Default for AttachSlots {
    fn default() -> Self {
        Self::new(Self::DEFAULT_MAX)
    }
}

struct SlotGuard(Arc<AtomicUsize>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Output offsets `[from_seq, to_seq)` that were never received: the screen
/// is restored from the snapshot, but those bytes (and the scrollback they
/// made) are not shown. A UI must mark the spot ([`Gap::label`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gap {
    pub from_seq: u64,
    pub to_seq: u64,
}

impl Gap {
    pub fn bytes(&self) -> u64 {
        self.to_seq - self.from_seq
    }

    /// The marker text (Korean product copy).
    pub fn label(&self) -> String {
        format!(
            "── 연결이 끊긴 동안 출력 {}바이트를 받지 못했습니다. 화면은 현재 상태로 복원했습니다 ──",
            self.bytes()
        )
    }
}

/// Why a [`Resync`] happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResyncReason {
    /// The first attach.
    First,
    /// After a dropped connection, same daemon start.
    Reconnected,
    /// The daemon dropped this view for falling behind (`terminal.lagged`).
    Lagged,
    /// The daemon restarted: offsets are not compared with the old ones.
    BootChanged,
}

impl ResyncReason {
    /// A marker for the restarted-daemon case (none otherwise).
    pub fn label(self) -> Option<&'static str> {
        (self == ResyncReason::BootChanged).then_some(
            "── 원격 데몬이 다시 시작되었습니다. 끊긴 동안의 출력은 확인할 수 없어 화면을 새로 받았습니다 ──",
        )
    }
}

/// A (re)attach: replace the view's screen with `snapshot`.
#[derive(Debug, Clone)]
pub struct Resync {
    pub reason: ResyncReason,
    /// From the full terminal list taken just before the attach.
    pub terminal: TerminalInfo,
    pub seq: u64,
    pub snapshot: Vec<u8>,
    pub gap: Option<Gap>,
    pub scrollback_omitted: Option<usize>,
    pub attempts: u32,
}

/// Events of a [`RemoteAttach`], in order.
#[derive(Debug, Clone)]
pub enum AttachUpdate {
    Resync(Box<Resync>),
    Output {
        seq: u64,
        data: Vec<u8>,
    },
    Resized {
        seq: u64,
        cols: u16,
        rows: u16,
    },
    /// The connection dropped: disable input, show "disconnected since"
    /// with the last received offset. A [`AttachUpdate::Resync`] follows when
    /// it is back.
    Disconnected {
        at: SystemTime,
        reason: String,
        last_seq: u64,
    },
    /// Waiting `delay` before reconnect attempt `attempt`.
    Reconnecting {
        attempt: u32,
        delay: Duration,
    },
    /// The program exited (last event).
    Exited(ExitedEvent),
    /// Given up (last event): another machine, the terminal is gone or
    /// replaced, a permanent connection error, or the policy's limit.
    Failed {
        reason: String,
    },
}

/// What one view attaches to.
#[derive(Debug, Clone)]
pub struct AttachOptions {
    pub target: TerminalRef,
    pub scrollback: Option<usize>,
    /// Sent with the first attach only (a re-attach does not resize: the
    /// most recent resize of any client wins, docs/03 §4.1).
    pub size: Option<(u16, u16)>,
    /// Events buffered for a slow consumer; when full the reader stops, the
    /// daemon's queue fills and the view is re-attached (`Lagged`).
    pub channel: usize,
}

impl AttachOptions {
    pub fn new(target: TerminalRef) -> Self {
        Self {
            target,
            scrollback: None,
            size: None,
            channel: 64,
        }
    }
}

struct AttachShared {
    stop: AtomicBool,
    closer: Mutex<Option<Closer>>,
}

/// One remote terminal view (see the module docs). Dropping it detaches.
pub struct RemoteAttach {
    shared: Arc<AttachShared>,
    thread: Option<JoinHandle<()>>,
}

impl RemoteAttach {
    /// Start the view's thread. `None` when all of `slots` are in use.
    pub fn spawn(
        connector: Arc<dyn Connector>,
        policy: ReconnectPolicy,
        opts: AttachOptions,
        slots: &AttachSlots,
    ) -> Option<(RemoteAttach, Receiver<AttachUpdate>)> {
        let slot = slots.take()?;
        let (tx, rx) = mpsc::sync_channel(opts.channel.max(1));
        let shared = Arc::new(AttachShared {
            stop: AtomicBool::new(false),
            closer: Mutex::new(None),
        });
        let s = shared.clone();
        let thread = std::thread::spawn(move || {
            let _slot = slot;
            let mut w = Worker {
                connector,
                policy,
                opts,
                tx,
                shared: s,
                jitter: Jitter::new(),
            };
            w.run();
        });
        Some((
            RemoteAttach {
                shared,
                thread: Some(thread),
            },
            rx,
        ))
    }

    /// Detach now and wait for the thread.
    pub fn close(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(c) = self.shared.closer.lock().unwrap().as_ref() {
            c();
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for RemoteAttach {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What the last attach left.
struct Seen {
    machine_id: String,
    boot_id: String,
    terminal_id: String,
    created_ms: u64,
    next_seq: u64,
}

enum Leave {
    /// Reconnect after a backoff.
    Dropped(String),
    /// Re-attach at once.
    Lagged,
    /// Done (exited, failed or stopped).
    End,
}

struct Worker {
    connector: Arc<dyn Connector>,
    policy: ReconnectPolicy,
    opts: AttachOptions,
    tx: SyncSender<AttachUpdate>,
    shared: Arc<AttachShared>,
    jitter: Jitter,
}

impl Worker {
    fn stopped(&self) -> bool {
        self.shared.stop.load(Ordering::SeqCst)
    }

    /// Bounded send that still notices a stop; false when the view is gone.
    fn send(&self, mut u: AttachUpdate) -> bool {
        loop {
            if self.stopped() {
                return false;
            }
            match self.tx.try_send(u) {
                Ok(()) => return true,
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(back)) => {
                    u = back;
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
    }

    fn fail(&self, reason: String) {
        self.send(AttachUpdate::Failed { reason });
    }

    fn run(&mut self) {
        let mut seen: Option<Seen> = None;
        let mut reason = ResyncReason::First;
        let mut down_since: Option<Instant> = None;
        let mut attempt = 0u32;
        loop {
            if self.stopped() {
                return;
            }
            let link = match self.connector.connect() {
                Ok(l) => l,
                Err(e) if e.permanent => return self.fail(e.message),
                Err(e) => {
                    let started = *down_since.get_or_insert_with(Instant::now);
                    let delay = self.policy.delay(attempt, self.jitter.unit());
                    attempt += 1;
                    if let Some(limit) = self.policy.give_up_after
                        && started.elapsed() + delay > limit
                    {
                        return self.fail(format!(
                            "{} 동안 다시 연결하지 못했습니다: {}",
                            secs(limit),
                            e.message
                        ));
                    }
                    if !self.send(AttachUpdate::Reconnecting { attempt, delay })
                        || !sleep_unless(delay, &self.shared.stop)
                    {
                        return;
                    }
                    continue;
                }
            };
            *self.shared.closer.lock().unwrap() = Some(link.closer());
            if self.stopped() {
                (link.closer)();
                return;
            }
            match self.session(link, &mut seen, reason, attempt + 1) {
                Leave::End => return,
                Leave::Lagged => {
                    reason = ResyncReason::Lagged;
                    attempt = 0;
                    down_since = None;
                }
                Leave::Dropped(why) => {
                    let last_seq = seen.as_ref().map_or(0, |s| s.next_seq);
                    if !self.send(AttachUpdate::Disconnected {
                        at: SystemTime::now(),
                        reason: why,
                        last_seq,
                    }) {
                        return;
                    }
                    reason = ResyncReason::Reconnected;
                    down_since = Some(Instant::now());
                    attempt = 0;
                    let delay = self.policy.delay(0, self.jitter.unit());
                    attempt += 1;
                    if !self.send(AttachUpdate::Reconnecting { attempt, delay })
                        || !sleep_unless(delay, &self.shared.stop)
                    {
                        return;
                    }
                }
            }
        }
    }

    /// One connection: identity → full list → attach → stream.
    fn session(
        &mut self,
        mut link: Link,
        seen: &mut Option<Seen>,
        reason: ResyncReason,
        attempts: u32,
    ) -> Leave {
        let hello = link.hello.clone();
        let mut reason = reason;
        if let Some(s) = seen.as_ref() {
            if hello.machine_id != s.machine_id {
                (link.closer)();
                self.fail(machine_changed(&s.machine_id, &hello.machine_id));
                return Leave::End;
            }
            if hello.boot_id != s.boot_id {
                reason = ResyncReason::BootChanged;
            }
        }
        // Full state first: the terminal must still be the same one.
        let dog = Watchdog::arm(self.policy.call_timeout, link.closer());
        let list = link.client.call::<TerminalList>(&Empty {});
        dog.disarm();
        let list = match list {
            Ok(l) => l,
            Err(ClientError::Rpc(e)) => {
                self.fail(e.message);
                return Leave::End;
            }
            Err(e) => return Leave::Dropped(e.to_string()),
        };
        let found = list.terminals.into_iter().find(|t| match seen.as_ref() {
            Some(s) => t.id == s.terminal_id,
            None => {
                self.opts.target.id.as_deref() == Some(t.id.as_str())
                    || (self.opts.target.id.is_none()
                        && self.opts.target.name.is_some()
                        && t.name == self.opts.target.name)
            }
        });
        let Some(info) = found else {
            self.fail("터미널이 원격에서 사라졌습니다".into());
            return Leave::End;
        };
        if let Some(s) = seen.as_ref()
            && info.created_ms != s.created_ms
        {
            self.fail(format!(
                "터미널 {}이(가) 다른 터미널로 바뀌었습니다. 자동으로 연결하지 않습니다",
                info.id
            ));
            return Leave::End;
        }
        let first = seen.is_none();
        let params = AttachParams {
            target: TerminalRef::id(&info.id),
            scrollback: self.opts.scrollback,
            cols: if first {
                self.opts.size.map(|s| s.0)
            } else {
                None
            },
            rows: if first {
                self.opts.size.map(|s| s.1)
            } else {
                None
            },
        };
        let closer = link.closer();
        let dog = Watchdog::arm(self.policy.call_timeout, closer);
        let attached = link.client.attach(&params);
        dog.disarm();
        let (result, snapshot, mut stream) = match attached {
            Ok(x) => x,
            Err(ClientError::Rpc(e)) => {
                self.fail(e.message);
                return Leave::End;
            }
            Err(e) => return Leave::Dropped(e.to_string()),
        };
        let gap = match seen.as_ref() {
            Some(s) if reason != ResyncReason::BootChanged => {
                if result.seq < s.next_seq {
                    self.fail(format!(
                        "터미널 {}의 출력 위치가 거꾸로 갔습니다({} → {}). 자동으로 연결하지 않습니다",
                        info.id, s.next_seq, result.seq
                    ));
                    return Leave::End;
                }
                (result.seq > s.next_seq).then_some(Gap {
                    from_seq: s.next_seq,
                    to_seq: result.seq,
                })
            }
            _ => None,
        };
        *seen = Some(Seen {
            machine_id: hello.machine_id.clone(),
            boot_id: hello.boot_id.clone(),
            terminal_id: info.id.clone(),
            created_ms: info.created_ms,
            next_seq: result.seq,
        });
        let resync = Resync {
            reason,
            terminal: info,
            seq: result.seq,
            snapshot,
            gap,
            scrollback_omitted: result.scrollback_omitted,
            attempts,
        };
        if !self.send(AttachUpdate::Resync(Box::new(resync))) {
            return Leave::End;
        }
        loop {
            let ev = stream.next_event();
            if self.stopped() {
                return Leave::End;
            }
            let s = seen.as_mut().expect("set above");
            match ev {
                Ok(Some(AttachEvent::Output { seq, data })) => {
                    if seq != s.next_seq {
                        // Never expected from the daemon; resync rather than
                        // splice bytes at the wrong place.
                        return Leave::Lagged;
                    }
                    s.next_seq = seq + data.len() as u64;
                    if !self.send(AttachUpdate::Output { seq, data }) {
                        return Leave::End;
                    }
                }
                Ok(Some(AttachEvent::Resized { seq, cols, rows })) => {
                    if !self.send(AttachUpdate::Resized { seq, cols, rows }) {
                        return Leave::End;
                    }
                }
                Ok(Some(AttachEvent::Exited(e))) => {
                    self.send(AttachUpdate::Exited(e));
                    return Leave::End;
                }
                Ok(Some(AttachEvent::Lagged(_))) => return Leave::Lagged,
                Ok(None) => return Leave::Dropped("원격 연결이 닫혔습니다".into()),
                Err(e) => return Leave::Dropped(e.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests;
