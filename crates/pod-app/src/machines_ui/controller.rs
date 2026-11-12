//! One control thread per connected machine (blocking pod-remote code never
//! runs on the UI thread).
//!
//! - [`Guard`] wraps every connection the app opens to the machine, control
//!   and attach alike: it dials (system ssh, or a test dialer), reads the
//!   host key fingerprints known_hosts has for the profile, and once the
//!   user confirmed the machine refuses a connection whose `machine_id` or
//!   fingerprints differ (closed before anything is sent; docs/22 TM-13).
//! - The thread owns a [`RemoteControl`]: polls `terminal.list` +
//!   `task.list` (the board and the "last received" time), sends input and
//!   resizes, and on a drop reconnects with backoff, reporting each failed
//!   attempt. A lost input is resolved by asking (`InputResolution`), never
//!   resent.
//! - **No replay**: every mutation carries the connection epoch it was made
//!   in and is dropped unless that is still the live, confirmed connection.
//!   Input typed while the link was down never waits in a queue for the
//!   next connection (docs/17 §2.3, TM-14).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures::channel::mpsc::UnboundedSender;
use pod_remote::session::{ConnectError, Connector, Link};
use pod_remote::{AttachSlots, Machine, ReconnectPolicy, RemoteControl, RemoteError};
use podd::protocol::{
    Empty, HelloResult, ResizeParams, TaskList, TaskListParams, TerminalList, TerminalRef,
    TerminalResize,
};

use super::host_keys::HostKeys;
use super::model::{Failure, HelloInfo, MachineEvent, tasks_of, terminals_of};
use super::trust::Identity;

/// Opens one handshaken connection (`connection.hello` done).
pub trait Dial: Send + Sync {
    fn dial(&self, m: &Machine) -> Result<Link, Box<Failure>>;
}

/// The real transport: the system OpenSSH client (pod-remote R3).
pub struct SystemSsh;

impl Dial for SystemSsh {
    fn dial(&self, m: &Machine) -> Result<Link, Box<Failure>> {
        pod_remote::ssh::connect(m)
            .map(|c| c.into_link())
            .map_err(|e| Box::new(Failure::from_ssh(&e)))
    }
}

/// What a connection must match once the machine was confirmed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expect {
    pub machine_id: Option<String>,
    pub fingerprints: Option<Vec<String>>,
}

/// Shared by every connector of one machine.
pub struct Guard {
    pub machine: Machine,
    dial: Arc<dyn Dial>,
    keys: Arc<dyn HostKeys>,
    expect: Mutex<Expect>,
    last_identity: Mutex<Option<Identity>>,
    last_failure: Mutex<Option<Failure>>,
}

impl Guard {
    pub fn new(machine: Machine, dial: Arc<dyn Dial>, keys: Arc<dyn HostKeys>) -> Self {
        Self {
            machine,
            dial,
            keys,
            expect: Mutex::default(),
            last_identity: Mutex::default(),
            last_failure: Mutex::default(),
        }
    }

    pub fn set_expect(&self, e: Expect) {
        *self.expect.lock().unwrap() = e;
    }

    pub fn expect(&self) -> Expect {
        self.expect.lock().unwrap().clone()
    }

    fn take_identity(&self) -> Option<Identity> {
        self.last_identity.lock().unwrap().take()
    }

    fn take_failure(&self) -> Option<Failure> {
        self.last_failure.lock().unwrap().take()
    }

    /// Dial, read the fingerprints, and unless `show_identity` (the
    /// control connection's first connect, whose identity the board
    /// evaluates) refuse anything that differs from the confirmed one.
    fn connect(&self, show_identity: bool) -> Result<Link, Box<Failure>> {
        let link = self.dial.dial(&self.machine)?;
        let id = Identity {
            target: self.machine.target(),
            known_hosts: self.machine.known_hosts.clone(),
            fingerprints: self.keys.lookup(&self.machine),
            machine_id: link.hello.machine_id.clone(),
        };
        let expect = if show_identity {
            Expect::default()
        } else {
            self.expect()
        };
        let refuse = |summary: &str| {
            (link.closer())();
            Err(Box::new(Failure::other(summary, false)))
        };
        if let Some(mid) = &expect.machine_id
            && *mid != id.machine_id
        {
            return refuse(super::copy::TRUST_MACHINE_TITLE);
        }
        if let Some(fp) = &expect.fingerprints
            && *fp != id.fingerprint_list()
        {
            return refuse(super::copy::TRUST_HOST_KEY_TITLE);
        }
        *self.last_identity.lock().unwrap() = Some(id);
        Ok(link)
    }
}

/// A [`Connector`] over a [`Guard`]; the control one also reports failed
/// attempts (the board's "다시 연결 중 (n번째)").
pub struct GuardedConnector {
    guard: Arc<Guard>,
    /// Set around the control connection's first connect only.
    first: Arc<AtomicBool>,
    report: Option<(String, UnboundedSender<(String, MachineEvent)>)>,
    attempts: AtomicU32,
}

impl Connector for GuardedConnector {
    fn connect(&self) -> Result<Link, ConnectError> {
        match self.guard.connect(self.first.load(Ordering::SeqCst)) {
            Ok(link) => {
                self.attempts.store(0, Ordering::SeqCst);
                Ok(link)
            }
            Err(f) => {
                let message = if f.hint.is_empty() {
                    f.summary.clone()
                } else {
                    format!("{}. {}", f.summary, f.hint)
                };
                let permanent = !f.retryable;
                *self.guard.last_failure.lock().unwrap() = Some(*f);
                let n = self.attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some((name, tx)) = &self.report {
                    let _ = tx.unbounded_send((
                        name.clone(),
                        MachineEvent::Attempt {
                            attempt: n,
                            reason: message.clone(),
                        },
                    ));
                }
                Err(ConnectError { message, permanent })
            }
        }
    }
}

/// Connection epoch and the machine the user confirmed for it.
#[derive(Debug, Default)]
pub struct Gate {
    pub epoch: u64,
    pub confirmed: Option<String>,
}

pub enum Cmd {
    Connect,
    Disconnect,
    Input {
        terminal: String,
        bytes: Vec<u8>,
        epoch: u64,
    },
    Resize {
        terminal: String,
        cols: u16,
        rows: u16,
        epoch: u64,
    },
    Refresh,
    Shutdown,
}

/// The UI's side of one machine's thread.
pub struct MachineHandle {
    tx: Sender<Cmd>,
    cancel: Arc<AtomicBool>,
    gate: Arc<Mutex<Gate>>,
    guard: Arc<Guard>,
    pub slots: AttachSlots,
    pub policy: ReconnectPolicy,
}

/// Where a remote terminal panel sends what the user types.
#[derive(Clone)]
pub struct InputPort {
    tx: Sender<Cmd>,
    gate: Arc<Mutex<Gate>>,
    terminal: String,
}

impl InputPort {
    /// Stamped with the current epoch; the thread drops it unless that
    /// connection is still live and confirmed when it gets there.
    pub fn send(&self, bytes: Vec<u8>) {
        let epoch = self.gate.lock().unwrap().epoch;
        let _ = self.tx.send(Cmd::Input {
            terminal: self.terminal.clone(),
            bytes,
            epoch,
        });
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let epoch = self.gate.lock().unwrap().epoch;
        let _ = self.tx.send(Cmd::Resize {
            terminal: self.terminal.clone(),
            cols,
            rows,
            epoch,
        });
    }
}

impl MachineHandle {
    pub fn spawn(
        machine: Machine,
        dial: Arc<dyn Dial>,
        keys: Arc<dyn HostKeys>,
        policy: ReconnectPolicy,
        poll: Duration,
        events: UnboundedSender<(String, MachineEvent)>,
    ) -> Self {
        let name = machine.name.clone();
        let guard = Arc::new(Guard::new(machine, dial, keys));
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(Mutex::new(Gate::default()));
        let first = Arc::new(AtomicBool::new(false));
        let connector: Arc<dyn Connector> = Arc::new(GuardedConnector {
            guard: guard.clone(),
            first: first.clone(),
            report: Some((name.clone(), events.clone())),
            attempts: AtomicU32::new(0),
        });
        let mut ctl = Ctl {
            name: name.clone(),
            guard: guard.clone(),
            connector,
            first,
            policy: policy.clone(),
            poll,
            gate: gate.clone(),
            cancel: cancel.clone(),
            events,
            control: None,
        };
        std::thread::Builder::new()
            .name(format!("machine-{name}"))
            .spawn(move || ctl.run(rx))
            .expect("spawn machine thread");
        Self {
            tx,
            cancel,
            gate,
            guard,
            slots: AttachSlots::default(),
            policy,
        }
    }

    pub fn connect(&self) {
        self.cancel.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Connect);
    }

    /// Stop now, also a reconnect in progress (its backoff is cancelled).
    pub fn disconnect(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Disconnect);
    }

    pub fn refresh(&self) {
        let _ = self.tx.send(Cmd::Refresh);
    }

    /// The user confirmed this identity: mutations may go to it, and every
    /// later connection (reconnects, attaches) must match it.
    pub fn confirm(&self, id: &Identity) {
        self.guard.set_expect(Expect {
            machine_id: Some(id.machine_id.clone()),
            fingerprints: Some(id.fingerprint_list()),
        });
        self.gate.lock().unwrap().confirmed = Some(id.machine_id.clone());
    }

    pub fn port(&self, terminal: &str) -> InputPort {
        InputPort {
            tx: self.tx.clone(),
            gate: self.gate.clone(),
            terminal: terminal.to_string(),
        }
    }

    /// A connector for one attach view (same guard, no attempt reports).
    pub fn attach_connector(&self) -> Arc<dyn Connector> {
        Arc::new(GuardedConnector {
            guard: self.guard.clone(),
            first: Arc::new(AtomicBool::new(false)),
            report: None,
            attempts: AtomicU32::new(0),
        })
    }

    #[cfg(test)]
    pub fn epoch(&self) -> u64 {
        self.gate.lock().unwrap().epoch
    }
}

impl Drop for MachineHandle {
    fn drop(&mut self) {
        // The thread ends after its current call (bounded by the policy's
        // call timeout); it is not joined so the UI never waits on it.
        self.cancel.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

pub fn hello_info(h: &HelloResult) -> HelloInfo {
    HelloInfo {
        machine_id: h.machine_id.clone(),
        boot_id: h.boot_id.clone(),
        protocol: h.protocol,
        version: h.version.clone(),
        os: h.os.clone(),
    }
}

/// Reconnects in a row before the thread stops polling and waits for the
/// next command (a link that drops right after every reconnect).
const MAX_RECOVERIES: u32 = 8;

struct Ctl {
    name: String,
    guard: Arc<Guard>,
    connector: Arc<dyn Connector>,
    /// The control connector's "first connect" flag.
    first: Arc<AtomicBool>,
    policy: ReconnectPolicy,
    poll: Duration,
    gate: Arc<Mutex<Gate>>,
    cancel: Arc<AtomicBool>,
    events: UnboundedSender<(String, MachineEvent)>,
    control: Option<RemoteControl>,
}

impl Ctl {
    fn emit(&self, ev: MachineEvent) {
        let _ = self.events.unbounded_send((self.name.clone(), ev));
    }

    fn connected(&self) -> bool {
        self.control
            .as_ref()
            .is_some_and(RemoteControl::is_connected)
    }

    fn bump_epoch(&self) {
        self.gate.lock().unwrap().epoch += 1;
    }

    fn run(&mut self, rx: mpsc::Receiver<Cmd>) {
        loop {
            let wait = if self.connected() {
                self.poll
            } else {
                Duration::from_secs(3600)
            };
            match rx.recv_timeout(wait) {
                Ok(Cmd::Connect) => self.connect(),
                Ok(Cmd::Disconnect) => self.off(),
                Ok(Cmd::Input {
                    terminal,
                    bytes,
                    epoch,
                }) => self.input(&terminal, &bytes, epoch),
                Ok(Cmd::Resize {
                    terminal,
                    cols,
                    rows,
                    epoch,
                }) => self.resize(&terminal, cols, rows, epoch),
                Ok(Cmd::Refresh) | Err(RecvTimeoutError::Timeout) => self.poll_lists(),
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn off(&mut self) {
        if self.control.take().is_some() || self.connected() {
            self.bump_epoch();
        }
        self.emit(MachineEvent::Off);
    }

    /// A fresh connection: trust is evaluated again by the board, so the
    /// confirmation is cleared first (no mutation until the board confirms
    /// again). Only this first connect skips the confirmed identity, so the
    /// board can show what answered; attach connections keep enforcing it.
    fn connect(&mut self) {
        if self.connected() {
            self.poll_lists();
            return;
        }
        self.control = None;
        {
            let mut g = self.gate.lock().unwrap();
            g.confirmed = None;
            g.epoch += 1;
        }
        self.emit(MachineEvent::Connecting);
        self.first.store(true, Ordering::SeqCst);
        let connected = RemoteControl::connect(self.connector.clone(), self.policy.clone());
        self.first.store(false, Ordering::SeqCst);
        match connected {
            Ok(c) => {
                let hello = hello_info(c.hello());
                let identity = self.guard.take_identity().unwrap_or(Identity {
                    target: self.guard.machine.target(),
                    known_hosts: self.guard.machine.known_hosts.clone(),
                    fingerprints: Err(String::new()),
                    machine_id: hello.machine_id.clone(),
                });
                self.control = Some(c);
                self.bump_epoch();
                self.emit(MachineEvent::Connected { hello, identity });
                self.poll_lists();
            }
            Err(e) => {
                let f = self.failure_of(&e);
                self.emit(MachineEvent::Failed(f));
            }
        }
    }

    fn failure_of(&self, e: &RemoteError) -> Failure {
        if let Some(f) = self.guard.take_failure()
            && !matches!(e, RemoteError::Cancelled)
        {
            return f;
        }
        let retryable = matches!(e, RemoteError::Disconnected { .. });
        Failure::other(e.to_string(), retryable)
    }

    /// One poll; on a drop, reconnect and poll again (bounded).
    fn poll_lists(&mut self) {
        for _ in 0..MAX_RECOVERIES {
            let Some(c) = self.control.as_mut() else {
                return;
            };
            if !c.is_connected() {
                return;
            }
            let terminals = match c.call::<TerminalList>(&Empty {}) {
                Ok(l) => l.terminals,
                Err(RemoteError::Rpc(_)) => return,
                Err(e) => {
                    if self.recover(e) {
                        continue;
                    }
                    return;
                }
            };
            let tasks = match c.call::<TaskList>(&TaskListParams::default()) {
                Ok(t) => t.tasks,
                // An answer: tasks are just not available there.
                Err(RemoteError::Rpc(_)) => Vec::new(),
                Err(e) => {
                    if self.recover(e) {
                        continue;
                    }
                    return;
                }
            };
            self.emit(MachineEvent::Lists {
                terminals: terminals_of(&terminals, &tasks),
                tasks: tasks_of(&tasks),
                at: SystemTime::now(),
            });
            return;
        }
    }

    /// The link dropped (`e` from a call): reconnect until it works, the
    /// user disconnects, or a permanent refusal. True when connected again.
    fn recover(&mut self, e: RemoteError) -> bool {
        let Some(since) = self
            .control
            .as_ref()
            .map(|c| c.disconnected().map_or_else(SystemTime::now, |(t, _)| t))
        else {
            return false;
        };
        self.bump_epoch();
        self.emit(MachineEvent::Lost {
            since,
            reason: e.to_string(),
        });
        let cancel = self.cancel.clone();
        let reconnected = match self.control.as_mut() {
            Some(c) => c.reconnect(&cancel),
            None => return false,
        };
        match reconnected {
            Ok(r) => {
                let hello = hello_info(&r.hello);
                self.bump_epoch();
                self.emit(MachineEvent::Reconnected {
                    hello,
                    boot_changed: r.boot_changed,
                    input: r.input,
                });
                true
            }
            Err(RemoteError::Cancelled) => {
                self.control = None;
                self.emit(MachineEvent::Off);
                false
            }
            Err(e) => {
                self.control = None;
                let f = self.failure_of(&e);
                self.emit(MachineEvent::Failed(f));
                false
            }
        }
    }

    /// May a mutation stamped with `epoch` go out now?
    fn allowed(&self, epoch: u64) -> bool {
        let Some(c) = self.control.as_ref() else {
            return false;
        };
        let g = self.gate.lock().unwrap();
        c.is_connected()
            && g.epoch == epoch
            && g.confirmed.as_deref() == Some(c.hello().machine_id.as_str())
    }

    fn input(&mut self, terminal: &str, bytes: &[u8], epoch: u64) {
        if !self.allowed(epoch) {
            return;
        }
        let c = self.control.as_mut().expect("allowed");
        // Refused unsent (down) or answered: nothing more to do. A lost
        // answer is resolved by the reconnect (`check`), never resent.
        if let Err(e @ RemoteError::Unknown(_)) = c.input(&TerminalRef::id(terminal), bytes)
            && self.recover(e)
        {
            self.poll_lists();
        }
    }

    fn resize(&mut self, terminal: &str, cols: u16, rows: u16, epoch: u64) {
        if !self.allowed(epoch) {
            return;
        }
        let c = self.control.as_mut().expect("allowed");
        let r = c.call::<TerminalResize>(&ResizeParams {
            target: TerminalRef::id(terminal),
            cols,
            rows,
        });
        if let Err(e @ RemoteError::Unknown(_)) = r
            && self.recover(e)
        {
            self.poll_lists();
        }
    }
}
