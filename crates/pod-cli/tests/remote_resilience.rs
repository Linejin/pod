//! docs/17 R4 / docs/22 TM-13, TM-14 over a bad network, on this PC:
//! `pod_remote::session` → loopback fault proxy (`pod_test_support::
//! fault_proxy`: RTT, throttle, cuts, lost answers) → `pod remote-stdio`
//! (what ssh runs on the far side) → an isolated podd. No SSH server is
//! involved (none may be installed here): the proxy stands for the network,
//! a killed bridge for a dropped ssh.
//!
//! Needs `podd.exe` and `pod-holder.exe` next to `pod.exe` (workspace build).
//! The tests run one at a time (timing and the 60 s drop).

use std::net::{Shutdown, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pod_remote::session::*;
use pod_test_support::fault_proxy::{FaultProxy, Upstream};
use podd::client::{Client, ProcessWatch};
use podd::protocol::*;
use podd::transport::{REMOTE_MAX_LINE, Transport};

const POD: &str = env!("CARGO_BIN_EXE_pod");
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// One isolated machine: its own daemon pipe and state (so its own
/// machine_id), started on demand by the first bridge.
struct Machine {
    root: PathBuf,
    pipe: String,
}

impl Machine {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("pod-r4-{tag}-{}-{nanos}", std::process::id());
        let root = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(root.join("home")).unwrap();
        let podd = Path::new(POD).with_file_name("podd.exe");
        assert!(
            podd.exists() && podd.with_file_name("pod-holder.exe").exists(),
            "{} / pod-holder.exe missing: build the workspace first",
            podd.display()
        );
        Self {
            root,
            pipe: format!(r"\\.\pipe\{name}"),
        }
    }

    fn bridge(&self) -> std::io::Result<Child> {
        let r = &self.root;
        Command::new(POD)
            .args(["--pipe", &self.pipe, "remote-stdio"])
            .env("LOCALAPPDATA", r.join("local"))
            .env("APPDATA", r.join("roaming"))
            .env("USERPROFILE", r.join("home"))
            .env("POD_WORKTREES_ROOT", r.join("worktrees"))
            .env("POD_MODS_DIR", r.join("mods"))
            .env("POD_PR_POLL", "off")
            .env_remove("POD_SOCKET")
            .env_remove("POD_TERMINAL_ID")
            .env_remove("POD_DAEMON")
            .env_remove("POD_HOLDER")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    }

    /// A local client (as the remote side's own tools would talk to it).
    fn local(&self) -> Client {
        Client::connect(&self.pipe).expect("local connect")
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.root.join("local").join("pod").join("podd.log"))
            .unwrap_or_default()
    }

    fn stop_daemon(&self, kill: bool) {
        if let Ok(mut c) = Client::connect(&self.pipe)
            && let Ok(s) = c.call::<ServerStatus>(&Empty {})
        {
            let watch = ProcessWatch::open(s.pid);
            let _ = c.call::<ServerStop>(&StopParams { kill });
            drop(c);
            if let Some(w) = watch {
                assert!(w.wait(Duration::from_secs(20)), "podd did not stop");
            }
        }
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop_daemon(true);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A proxy whose far side is a fresh bridge to the current machine.
fn proxy(target: Arc<Mutex<Arc<Machine>>>) -> FaultProxy {
    FaultProxy::start(move || {
        let m = target.lock().unwrap().clone();
        let mut child = m.bridge()?;
        let reader = child.stdout.take().unwrap();
        let writer = child.stdin.take().unwrap();
        let child = Arc::new(Mutex::new(child));
        Ok(Upstream {
            reader: Box::new(reader),
            writer: Box::new(writer),
            // Like ssh dropping: the bridge dies at once.
            close: Box::new(move || {
                let _ = child.lock().unwrap().kill();
            }),
        })
    })
    .unwrap()
}

/// The connector the app would get from a machine profile, but through the
/// proxy instead of ssh.
struct ProxyConnector {
    addr: SocketAddr,
}

impl Connector for ProxyConnector {
    fn connect(&self) -> Result<Link, ConnectError> {
        let fail = |m: String| ConnectError {
            message: m,
            permanent: false,
        };
        let s = TcpStream::connect(self.addr).map_err(|e| fail(e.to_string()))?;
        let _ = s.set_nodelay(true);
        let closer_sock = s.try_clone().map_err(|e| fail(e.to_string()))?;
        let r = s.try_clone().map_err(|e| fail(e.to_string()))?;
        let mut c = Client::from_transport("proxy", Transport::new(r, s, REMOTE_MAX_LINE));
        // The handshake must not hang a test either.
        let guard = closer_sock.try_clone().map_err(|e| fail(e.to_string()))?;
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if rx.recv_timeout(Duration::from_secs(15)).is_err() {
                let _ = guard.shutdown(Shutdown::Both);
            }
        });
        let hello = c.hello(&[]);
        let _ = tx.send(());
        match hello {
            Ok(h) => Ok(Link::new(
                c,
                h,
                Arc::new(move || {
                    let _ = closer_sock.shutdown(Shutdown::Both);
                }),
            )),
            Err(e) => Err(ConnectError {
                permanent: e.is_incompatible(),
                message: e.to_string(),
            }),
        }
    }
}

fn policy() -> ReconnectPolicy {
    ReconnectPolicy {
        first_delay: Duration::from_millis(300),
        max_delay: Duration::from_secs(2),
        jitter: 0.2,
        give_up_after: Some(Duration::from_secs(120)),
        call_timeout: Duration::from_secs(15),
    }
}

fn shell(c: &mut Client, name: &str, cwd: &Path) -> TerminalInfo {
    c.call::<TerminalRun>(&RunParams {
        argv: vec![
            "cmd".into(),
            "/d".into(),
            "/q".into(),
            "/k".into(),
            "prompt $G".into(),
        ],
        cwd: Some(cwd.display().to_string()),
        name: Some(name.into()),
        cols: Some(120),
        rows: Some(30),
        ..Default::default()
    })
    .expect("run cmd")
}

/// Collects a view's events and its reconstructed byte stream.
struct View {
    rx: Receiver<AttachUpdate>,
    /// Snapshot of the last resync, then the output after it.
    screen: Vec<u8>,
    events: Vec<AttachUpdate>,
}

impl View {
    fn new(rx: Receiver<AttachUpdate>) -> Self {
        Self {
            rx,
            screen: Vec::new(),
            events: Vec::new(),
        }
    }

    fn take(&mut self, u: AttachUpdate) {
        match &u {
            AttachUpdate::Resync(r) => self.screen = r.snapshot.clone(),
            AttachUpdate::Output { data, .. } => self.screen.extend_from_slice(data),
            _ => {}
        }
        // Keep the log small: output bytes are in `screen`.
        if !matches!(u, AttachUpdate::Output { .. }) {
            self.events.push(u);
        }
    }

    /// Read events until `pred` holds for the latest one or the reconstructed
    /// screen; panics after `within`.
    fn until(&mut self, within: Duration, what: &str, pred: impl Fn(&View) -> bool) {
        let end = Instant::now() + within;
        while !pred(self) {
            let left = end.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "{what}: not seen; events {:?}",
                self.events
            );
            if let Ok(u) = self.rx.recv_timeout(left.min(Duration::from_millis(200))) {
                self.take(u);
            }
        }
    }

    fn shows(&self, needle: &str) -> bool {
        String::from_utf8_lossy(&self.screen).contains(needle)
    }

    fn resyncs(&self) -> Vec<&Resync> {
        self.events
            .iter()
            .filter_map(|e| match e {
                AttachUpdate::Resync(r) => Some(r.as_ref()),
                _ => None,
            })
            .collect()
    }
}

fn input(c: &mut RemoteControl, t: &str, text: &str) -> Result<InputOnceResult, RemoteError> {
    c.input(&TerminalRef::id(t), text.as_bytes())
}

/// Lines the shell actually appended to `file` (what it ran), once settled.
fn ran(file: &Path) -> Vec<String> {
    std::thread::sleep(Duration::from_millis(1500));
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

fn append(tag: &str, file: &Path) -> String {
    format!("echo {tag}>>\"{}\"\r", file.display())
}

fn p95(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[((v.len() * 95).div_ceil(100)).saturating_sub(1)]
}

/// RTT 20/100/300 ms with four throttled views on flooding terminals:
/// control RPC p95 within RTT + 250 ms, output pieces within 32 KiB, the
/// proxy's buffers bounded, a lagged view comes back with a gap marker,
/// and the daemon never queued more than 2 MiB for a remote view.
#[test]
fn control_stays_responsive_while_four_throttled_views_flood() {
    let _s = serial();
    let m = Arc::new(Machine::new("flood"));
    let px = proxy(Arc::new(Mutex::new(m.clone())));
    px.set_throttle(Some(256 * 1024));
    let conn: Arc<dyn Connector> = Arc::new(ProxyConnector { addr: px.addr() });
    let mut ctl = RemoteControl::connect(conn.clone(), policy()).expect("control");
    assert_eq!(ctl.hello().protocol, PROTOCOL_VERSION);
    let mut local = m.local();
    let slots = AttachSlots::default();
    let mut views = Vec::new();
    let mut ids = Vec::new();
    for i in 0..4 {
        let t = shell(&mut local, &format!("flood{i}"), &m.root);
        let (h, rx) = RemoteAttach::spawn(
            conn.clone(),
            policy(),
            AttachOptions::new(TerminalRef::id(&t.id)),
            &slots,
        )
        .expect("slot");
        views.push((h, View::new(rx)));
        ids.push(t.id);
    }
    assert!(
        RemoteAttach::spawn(
            conn.clone(),
            policy(),
            AttachOptions::new(TerminalRef::id(&ids[0])),
            &slots
        )
        .is_none(),
        "a fifth view is refused"
    );
    for (_, v) in &mut views {
        v.until(Duration::from_secs(30), "first resync", |v| {
            !v.resyncs().is_empty()
        });
    }
    // Every terminal floods (~4 MB of text each, as fast as cmd prints).
    for id in &ids {
        input(
            &mut ctl,
            id,
            &format!(
                "for /L %i in (1,1,40000) do @echo flood-%i-{}\r",
                "z".repeat(90)
            ),
        )
        .expect("start flood");
    }
    // Drain the views in the background so they keep flowing.
    let stop = Arc::new(AtomicBool::new(false));
    let drains: Vec<_> = views
        .into_iter()
        .map(|(h, mut v)| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut biggest = 0usize;
                while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                    if let Ok(u) = v.rx.recv_timeout(Duration::from_millis(100)) {
                        if let AttachUpdate::Output { data, .. } = &u {
                            biggest = biggest.max(data.len());
                        }
                        v.take(u);
                    }
                }
                (h, v, biggest)
            })
        })
        .collect();

    for rtt in [20u64, 100, 300] {
        px.set_rtt(Duration::from_millis(rtt));
        std::thread::sleep(Duration::from_millis(300));
        let mut times = Vec::new();
        for _ in 0..20 {
            let t = Instant::now();
            ctl.call::<TerminalList>(&Empty {}).expect("terminal.list");
            times.push(t.elapsed());
        }
        let p = p95(times.clone());
        eprintln!("rtt {rtt} ms: control p95 {p:?} (all {times:?})");
        assert!(
            p <= Duration::from_millis(rtt + 250),
            "rtt {rtt} ms: p95 {p:?} over RTT + 250 ms ({times:?})"
        );
    }

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    for d in drains {
        let (h, v, biggest) = d.join().unwrap();
        assert!(biggest <= REMOTE_OUTPUT_CHUNK, "output piece of {biggest}");
        for r in v.resyncs() {
            if r.reason == ResyncReason::Lagged {
                assert!(r.gap.is_some(), "a lagged view marks the gap: {r:?}");
            }
        }
        assert!(
            !v.events
                .iter()
                .any(|e| matches!(e, AttachUpdate::Failed { .. })),
            "{:?}",
            v.events
        );
        h.close();
    }
    assert_eq!(slots.in_use(), 0);
    let st = px.stats();
    assert!(
        st.max_buffered <= px.buffer_limit() + 16 * 1024,
        "proxy buffers bounded: {st:?}"
    );
    // Whatever lagged, the daemon had queued at most the remote limit.
    for line in m.log().lines().filter(|l| l.contains("lagged at seq")) {
        let queued: u64 = line
            .split('(')
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("log line {line}"));
        assert!(queued <= REMOTE_ATTACH_QUEUE as u64, "{line}");
    }
}

/// A 60 s network drop: the view says so and keeps retrying, input is
/// refused while down (the one in flight is resolved by asking: it never
/// arrived), and after the network returns the view re-attaches with the
/// output made meanwhile and a visible gap marker; new input works.
#[test]
fn a_sixty_second_drop_reattaches_and_converges() {
    let _s = serial();
    let m = Arc::new(Machine::new("drop"));
    let px = proxy(Arc::new(Mutex::new(m.clone())));
    px.set_rtt(Duration::from_millis(100));
    let conn: Arc<dyn Connector> = Arc::new(ProxyConnector { addr: px.addr() });
    let mut ctl = RemoteControl::connect(conn.clone(), policy()).unwrap();
    let mut local = m.local();
    let t = shell(&mut local, "work", &m.root);
    let file = m.root.join("ran.txt");
    let (h, rx) = RemoteAttach::spawn(
        conn.clone(),
        policy(),
        AttachOptions::new(TerminalRef::id(&t.id)),
        &AttachSlots::default(),
    )
    .unwrap();
    let mut v = View::new(rx);
    v.until(Duration::from_secs(30), "attach", |v| {
        !v.resyncs().is_empty()
    });
    input(&mut ctl, &t.id, "echo before-drop\r").unwrap();
    v.until(Duration::from_secs(30), "before-drop", |v| {
        v.shows("before-drop")
    });

    let cut_at = Instant::now();
    px.cut();
    v.until(Duration::from_secs(30), "disconnected", |v| {
        v.events
            .iter()
            .any(|e| matches!(e, AttachUpdate::Disconnected { .. }))
    });
    // The first input finds the connection gone: unknown, kept to resolve.
    let lost = input(&mut ctl, &t.id, &append("lost-input", &file));
    assert!(matches!(lost, Err(RemoteError::Unknown(_))), "{lost:?}");
    assert!(ctl.pending_input().is_some());
    // Later input is refused without being sent (input disabled).
    let refused = input(&mut ctl, &t.id, &append("refused-input", &file));
    assert!(
        matches!(refused, Err(RemoteError::Disconnected { .. })),
        "{refused:?}"
    );
    // The program goes on printing while the network is down.
    local
        .call::<TerminalSend>(&SendParams {
            target: TerminalRef::id(&t.id),
            text: "echo during-outage".into(),
            enter: true,
            confirm: false,
        })
        .unwrap();
    while cut_at.elapsed() < Duration::from_secs(60) {
        if let Ok(u) = v.rx.recv_timeout(Duration::from_millis(500)) {
            v.take(u);
        }
    }
    let retries = v
        .events
        .iter()
        .filter(|e| matches!(e, AttachUpdate::Reconnecting { .. }))
        .count();
    assert!(retries >= 10, "kept retrying through the drop: {retries}");
    assert!(
        !v.events
            .iter()
            .any(|e| matches!(e, AttachUpdate::Failed { .. }))
    );
    let last_seq = v
        .events
        .iter()
        .find_map(|e| match e {
            AttachUpdate::Disconnected { last_seq, .. } => Some(*last_seq),
            _ => None,
        })
        .unwrap();

    px.restore();
    v.until(Duration::from_secs(30), "re-attach", |v| {
        v.resyncs().len() >= 2
    });
    let r = v.resyncs()[1].clone();
    assert_eq!(r.reason, ResyncReason::Reconnected);
    let gap = r.gap.expect("output was made while down: a gap marker");
    assert_eq!(gap.from_seq, last_seq);
    assert_eq!(gap.to_seq, r.seq);
    assert!(gap.label().contains("바이트"));
    assert!(v.shows("during-outage"), "the snapshot has it");

    let lost_seq = ctl.pending_input().expect("still pending").seq;
    let back = ctl.reconnect(&AtomicBool::new(false)).expect("reconnect");
    assert!(!back.boot_changed);
    assert_eq!(
        back.input,
        Some(InputResolution::NotApplied { seq: lost_seq }),
        "the lost input never arrived; the check fenced it"
    );
    input(&mut ctl, &t.id, "echo after-reconnect\r").unwrap();
    v.until(Duration::from_secs(30), "after-reconnect", |v| {
        v.shows("after-reconnect")
    });
    assert!(ran(&file).is_empty(), "neither input ran: {:?}", ran(&file));
    h.close();
}

/// docs/22 TM-14 lost ACK: the input reaches the program but its answer is
/// lost. It is not resent; after reconnecting, a check finds it applied, and
/// the program ran it exactly once.
#[test]
fn a_lost_answer_is_resolved_by_asking_not_by_resending() {
    let _s = serial();
    let m = Arc::new(Machine::new("ack"));
    let px = proxy(Arc::new(Mutex::new(m.clone())));
    let conn: Arc<dyn Connector> = Arc::new(ProxyConnector { addr: px.addr() });
    let mut ctl = RemoteControl::connect(conn, policy()).unwrap();
    let mut local = m.local();
    let t = shell(&mut local, "ack", &m.root);
    let file = m.root.join("ran.txt");

    px.drop_replies_then_cut(Duration::from_millis(1500));
    let r = input(&mut ctl, &t.id, &append("once", &file));
    assert!(matches!(r, Err(RemoteError::Unknown(_))), "{r:?}");
    let pending = ctl.pending_input().cloned().expect("kept");
    assert!(px.stats().dropped_bytes > 0, "the answer was swallowed");
    while !px.is_cut() {
        std::thread::sleep(Duration::from_millis(50));
    }
    px.restore();
    let back = ctl.reconnect(&AtomicBool::new(false)).unwrap();
    match back.input {
        Some(InputResolution::Applied { seq, bytes }) => {
            assert_eq!(seq, pending.seq);
            assert!(bytes > 0);
        }
        other => panic!("expected applied, got {other:?}"),
    }
    assert!(ctl.pending_input().is_none());
    assert_eq!(ran(&file), ["once"], "ran exactly once");
    // The session goes on.
    input(&mut ctl, &t.id, &append("next", &file)).unwrap();
    assert_eq!(ran(&file), ["once", "next"]);
}

/// The remote daemon restarts while the network is down (holders keep the
/// terminal): the view re-attaches as `BootChanged` without comparing old
/// offsets, and a lost input stays unknown instead of being guessed.
#[test]
fn a_daemon_restart_is_a_boot_change() {
    let _s = serial();
    let m = Arc::new(Machine::new("boot"));
    let px = proxy(Arc::new(Mutex::new(m.clone())));
    let conn: Arc<dyn Connector> = Arc::new(ProxyConnector { addr: px.addr() });
    let mut ctl = RemoteControl::connect(conn.clone(), policy()).unwrap();
    let boot0 = ctl.hello().boot_id.clone();
    let mut local = m.local();
    let t = shell(&mut local, "boot", &m.root);
    let (h, rx) = RemoteAttach::spawn(
        conn,
        policy(),
        AttachOptions::new(TerminalRef::id(&t.id)),
        &AttachSlots::default(),
    )
    .unwrap();
    let mut v = View::new(rx);
    v.until(Duration::from_secs(30), "attach", |v| {
        !v.resyncs().is_empty()
    });

    px.drop_replies_then_cut(Duration::from_millis(1000));
    let r = input(&mut ctl, &t.id, "echo before-restart\r");
    assert!(matches!(r, Err(RemoteError::Unknown(_))), "{r:?}");
    while !px.is_cut() {
        std::thread::sleep(Duration::from_millis(50));
    }
    drop(local);
    m.stop_daemon(false); // terminals stay with their holders
    px.restore();
    v.until(Duration::from_secs(60), "re-attach after restart", |v| {
        v.resyncs().len() >= 2
    });
    let r = v.resyncs()[1].clone();
    assert_eq!(r.reason, ResyncReason::BootChanged);
    assert_eq!(r.gap, None, "offsets of another boot are not compared");
    assert!(r.reason.label().is_some());
    assert_eq!(r.terminal.id, t.id);

    let back = ctl.reconnect(&AtomicBool::new(false)).unwrap();
    assert!(back.boot_changed);
    assert_ne!(back.hello.boot_id, boot0);
    assert!(
        matches!(back.input, Some(InputResolution::Unknown { .. })),
        "{:?}",
        back.input
    );
    input(&mut ctl, &t.id, "echo after-restart\r").unwrap();
    v.until(Duration::from_secs(30), "after-restart", |v| {
        v.shows("after-restart")
    });
    h.close();
}

/// docs/22 TM-13: after a drop another machine answers under the same
/// profile (same terminal name there). The control connection and the view
/// refuse it and send it nothing that changes anything.
#[test]
fn another_machine_behind_the_same_profile_is_refused() {
    let _s = serial();
    let a = Arc::new(Machine::new("ma"));
    let b = Arc::new(Machine::new("mb"));
    let target = Arc::new(Mutex::new(a.clone()));
    let px = proxy(target.clone());
    let conn: Arc<dyn Connector> = Arc::new(ProxyConnector { addr: px.addr() });
    let mut ctl = RemoteControl::connect(conn.clone(), policy()).unwrap();
    let mut la = a.local();
    let ta = shell(&mut la, "work", &a.root);
    // The other machine has a terminal of the same name (started through a
    // bridge, which also starts its daemon).
    let mut cb = {
        let mut child = b.bridge().unwrap();
        let t = Transport::from_child(&mut child).unwrap();
        std::mem::forget(child);
        Client::from_transport("b", t)
    };
    cb.hello(&[]).unwrap();
    let tb = shell(&mut cb, "work", &b.root);
    let (h, rx) = RemoteAttach::spawn(
        conn,
        policy(),
        AttachOptions::new(TerminalRef::name("work")),
        &AttachSlots::default(),
    )
    .unwrap();
    let mut v = View::new(rx);
    v.until(Duration::from_secs(30), "attach", |v| {
        !v.resyncs().is_empty()
    });
    assert_eq!(v.resyncs()[0].terminal.id, ta.id);

    px.cut();
    *target.lock().unwrap() = b.clone();
    px.restore();
    v.until(Duration::from_secs(30), "refused", |v| {
        v.events
            .iter()
            .any(|e| matches!(e, AttachUpdate::Failed { reason } if reason.contains("다른 머신")))
    });
    let r = ctl.reconnect(&AtomicBool::new(false));
    assert!(
        matches!(&r, Err(RemoteError::Refused(m)) if m.contains("다른 머신")),
        "{r:?}"
    );
    let after = input(&mut ctl, &ta.id, "echo should-not-run\r");
    assert!(matches!(after, Err(RemoteError::Disconnected { .. })));
    // Machine B's "work" saw no attach and no input.
    std::thread::sleep(Duration::from_millis(500));
    let list = cb.call::<TerminalList>(&Empty {}).unwrap();
    let work = list.terminals.iter().find(|t| t.id == tb.id).unwrap();
    assert_eq!(work.attached, 0);
    let screen = cb
        .call::<TerminalRead>(&ReadParams {
            target: TerminalRef::id(&tb.id),
            mode: ReadMode::History,
            lines: None,
            raw: true,
        })
        .unwrap();
    assert!(!screen.lines.join("\n").contains("should-not-run"));
    h.close();
}
