//! Machine UI without GPUI: profile validation (TM-13), the trust record,
//! known_hosts parsing, the reducer, and the per-machine control thread
//! against a fake remote behind the loopback fault proxy (TM-14: input
//! disabled while down, never replayed, other machine / changed key refused).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use futures::channel::mpsc::{self, UnboundedReceiver};
use pod_remote::{
    AttachOptions, AttachUpdate, Machine, MachineStore, ReconnectPolicy, RemoteAttach, ResyncReason,
};
use podd::protocol::TerminalRef;

use super::controller::{Dial, MachineHandle};
use super::fake_remote::{FakeRemote, ProxyDial, machine};
use super::form::{self, Field, ProfileForm};
use super::host_keys::{self, FixedKeys, HostKeys};
use super::model::{self, LinkState, MachineEvent, MachineVm};
use super::trust::{Confirm, Identity, Pin, Trust, TrustStore, evaluate};

fn temp_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "pod-app-machines-{tag}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn form(name: &str, host: &str) -> ProfileForm {
    ProfileForm {
        name: name.into(),
        host: host.into(),
        ..Default::default()
    }
}

// ───────────────────────────── profiles ─────────────────────────────

#[test]
fn the_form_refuses_what_pod_remote_refuses() {
    let ok = ProfileForm {
        name: " box-1 ".into(),
        host: "build.example.com".into(),
        user: "alice".into(),
        port: "2222".into(),
        identity_file: r"C:\Users\사용자 이름\.ssh\id_ed25519".into(),
        known_hosts: String::new(),
    };
    let m = ok.to_machine().unwrap();
    assert_eq!(m.name, "box-1", "only surrounding blanks are trimmed");
    assert_eq!(m.port, Some(2222));
    assert_eq!(m.known_hosts, None);
    assert_eq!(ProfileForm::from_machine(&m).to_machine().unwrap(), m);

    let field = |f: ProfileForm| f.to_machine().unwrap_err().field;
    // Options, control characters, `%`, newlines, quotes: never into argv/config.
    for host in [
        "-oProxyCommand=calc",
        "host\nx",
        "a b",
        "host;calc",
        "user@host",
        "\"h\"",
        "",
    ] {
        assert_eq!(field(form("x", host)), Field::Host, "{host:?}");
    }
    for user in ["-l", "a b", "a\u{1b}[31m", "DOM\\user", "a\"b"] {
        let mut f = form("x", "h");
        f.user = user.into();
        assert_eq!(field(f), Field::User, "{user:?}");
    }
    for name in ["-x", "a/b", "한글", "a b"] {
        assert_eq!(field(form(name, "h")), Field::Name, "{name:?}");
    }
    for port in ["0", "65536", "-1", "22x", "+22", " 2 2"] {
        let mut f = form("x", "h");
        f.port = port.into();
        assert_eq!(field(f), Field::Port, "{port:?}");
    }
    for path in ["relative\\key", "C:\\keys\\a\"b", "C:\\keys\\a\nb"] {
        let mut f = form("x", "h");
        f.identity_file = path.into();
        assert_eq!(field(f.clone()), Field::IdentityFile, "{path:?}");
        f.identity_file.clear();
        f.known_hosts = path.into();
        assert_eq!(field(f), Field::KnownHosts, "{path:?}");
    }
    // `%` in a path is fine (ssh tokens are escaped when the config is written).
    let mut f = form("x", "h");
    f.identity_file = r"C:\keys\100%\id".into();
    f.to_machine().unwrap();
}

#[test]
fn saving_adds_edits_renames_and_never_overwrites_a_bad_file() {
    let dir = temp_dir("save");
    let store = MachineStore::at(dir.join("machines.json"));
    let a = form("a", "host-a").to_machine().unwrap();
    form::save(&store, None, a.clone()).unwrap();
    let e = form::save(&store, None, form("a", "other").to_machine().unwrap()).unwrap_err();
    assert_eq!(e.field, Field::Name, "duplicate name");
    // Edit in place.
    let mut a2 = a.clone();
    a2.port = Some(2200);
    form::save(&store, Some("a"), a2.clone()).unwrap();
    assert_eq!(store.load().unwrap(), vec![a2.clone()]);
    // Rename onto another existing profile is refused, nothing lost.
    form::save(&store, None, form("b", "host-b").to_machine().unwrap()).unwrap();
    let mut onto_b = a2.clone();
    onto_b.name = "b".into();
    assert!(form::save(&store, Some("a"), onto_b).is_err());
    assert_eq!(store.load().unwrap().len(), 2);
    // Rename.
    let mut c = a2.clone();
    c.name = "c".into();
    form::save(&store, Some("a"), c.clone()).unwrap();
    let names: Vec<_> = store.load().unwrap().into_iter().map(|m| m.name).collect();
    assert_eq!(names, ["b", "c"]);
    // A hand-added SSH option is refused, and the file is left alone.
    std::fs::write(
        store.path(),
        r#"{"version":1,"machines":[{"name":"x","host":"h","proxy_command":"calc"}]}"#,
    )
    .unwrap();
    assert!(form::save(&store, None, form("y", "h").to_machine().unwrap()).is_err());
    assert!(
        std::fs::read_to_string(store.path())
            .unwrap()
            .contains("proxy_command")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ───────────────────────────── trust ─────────────────────────────

fn identity(target: &str, keys: &[&str], id: &str) -> Identity {
    Identity {
        target: target.into(),
        known_hosts: None,
        fingerprints: Ok(keys.iter().map(|k| k.to_string()).collect()),
        machine_id: id.into(),
    }
}

#[test]
fn trust_asks_again_for_target_key_and_machine_changes() {
    let now = identity("u@h", &["ED25519 SHA256:A"], "m1");
    assert_eq!(evaluate(None, &now), Trust::Needs(Confirm::First));
    let pin = now.pin("box", 1);
    assert_eq!(evaluate(Some(&pin), &now), Trust::Confirmed);
    assert_eq!(
        evaluate(
            Some(&pin),
            &identity("u@other", &["ED25519 SHA256:A"], "m1")
        ),
        Trust::Needs(Confirm::Profile {
            old_target: "u@h".into()
        })
    );
    assert_eq!(
        evaluate(Some(&pin), &identity("u@h", &["ED25519 SHA256:B"], "m1")),
        Trust::Needs(Confirm::HostKey {
            old: vec!["ED25519 SHA256:A".into()]
        })
    );
    assert_eq!(
        evaluate(Some(&pin), &identity("u@h", &["ED25519 SHA256:A"], "m2")),
        Trust::Needs(Confirm::Machine { old: "m1".into() })
    );
    let mut kh = now.clone();
    kh.known_hosts = Some(r"C:\other\known_hosts".into());
    assert!(matches!(
        evaluate(Some(&pin), &kh),
        Trust::Needs(Confirm::Profile { .. })
    ));
}

#[test]
fn trust_record_round_trips_and_refuses_unknown_fields() {
    let dir = temp_dir("trust");
    let t = TrustStore::beside(&dir.join("machines.json"));
    assert_eq!(t.path(), dir.join("machine-trust.json"));
    assert!(t.load().unwrap().is_empty());
    let pin = identity("u@h", &["ED25519 SHA256:A"], "m1").pin("box", 7);
    t.put(pin.clone()).unwrap();
    t.put(Pin {
        name: "other".into(),
        ..pin.clone()
    })
    .unwrap();
    assert_eq!(t.get("box"), Some(pin.clone()));
    let mut changed = pin.clone();
    changed.machine_id = "m2".into();
    t.put(changed.clone()).unwrap();
    assert_eq!(t.get("box"), Some(changed));
    t.remove("box").unwrap();
    assert_eq!(t.get("box"), None);
    assert!(t.get("other").is_some());
    std::fs::write(
        t.path(),
        r#"{"version":1,"pins":[{"name":"x","auto_accept":true}]}"#,
    )
    .unwrap();
    assert!(t.load().is_err(), "unknown field refused");
    assert_eq!(t.get("x"), None, "a damaged record confirms nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

// ───────────────────────────── host keys ─────────────────────────────

#[test]
fn keygen_output_and_refusal_fingerprints_parse() {
    let out = "# Host example.com found: line 1 \r\n\
               example.com ED25519 SHA256:dXV0dvounE7eXAnn8F1z32hA+O7D40MWtYRn1A1i3u8\r\n\
               |1|abc=|def= RSA SHA256:zzz\r\n\
               @revoked example.com ECDSA SHA256:revoked\r\n";
    assert_eq!(
        host_keys::parse_keygen(out),
        [
            "ED25519 SHA256:dXV0dvounE7eXAnn8F1z32hA+O7D40MWtYRn1A1i3u8",
            "RSA SHA256:zzz"
        ]
    );
    assert!(host_keys::parse_keygen("").is_empty());
    let stderr: Vec<String> = [
        "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@",
        "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @",
        "The fingerprint for the ED25519 key sent by the remote host is",
        "SHA256:Zr4Tq8Nw1Bv6Kc3Hy9Pa2Le7Mx5Ud0Gs4Fo1Ji8Wk.",
        "Host key verification failed.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(
        host_keys::sent_fingerprint(&stderr).as_deref(),
        Some("SHA256:Zr4Tq8Nw1Bv6Kc3Hy9Pa2Le7Mx5Ud0Gs4Fo1Ji8Wk")
    );
    let mut m = machine("x");
    assert_eq!(host_keys::search_name(&m), "fake.example.com");
    m.port = Some(22);
    assert_eq!(host_keys::search_name(&m), "fake.example.com");
    m.port = Some(2222);
    assert_eq!(host_keys::search_name(&m), "[fake.example.com]:2222");
    m.known_hosts = Some(r"C:\kh\known_hosts".into());
    assert_eq!(
        host_keys::known_hosts_files(&m),
        [PathBuf::from(r"C:\kh\known_hosts")]
    );
}

/// The real lookup reads a known_hosts file with the system ssh-keygen
/// (read-only, offline). Skipped where OpenSSH is not installed.
#[test]
fn ssh_keygen_reads_the_profile_known_hosts() {
    let Ok(ssh) = pod_remote::ssh::locate_ssh() else {
        eprintln!("skipped: no ssh");
        return;
    };
    if !ssh.with_file_name("ssh-keygen.exe").is_file() {
        eprintln!("skipped: no ssh-keygen next to {}", ssh.display());
        return;
    }
    let dir = temp_dir("keygen");
    let kh = dir.join("known_hosts");
    let key = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBG6L4TQPmooMXvAOTF6b/tQTycz0bsgFqiFbAeRER+v";
    std::fs::write(
        &kh,
        format!("fake.example.com {key}\n[fake.example.com]:2222 {key}\nother.example.com {key}\n"),
    )
    .unwrap();
    let before = std::fs::read(&kh).unwrap();
    let mut m = machine("x");
    m.known_hosts = Some(kh.clone());
    let want = ["ED25519 SHA256:dXV0dvounE7eXAnn8F1z32hA+O7D40MWtYRn1A1i3u8".to_string()];
    assert_eq!(host_keys::SshKeygen.lookup(&m).unwrap(), want);
    m.port = Some(2222);
    assert_eq!(host_keys::SshKeygen.lookup(&m).unwrap(), want);
    m.host = "missing.example.com".into();
    assert!(host_keys::SshKeygen.lookup(&m).unwrap().is_empty());
    assert_eq!(std::fs::read(&kh).unwrap(), before, "known_hosts untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

// ───────────────────────────── reducer ─────────────────────────────

#[test]
fn the_board_state_follows_the_events() {
    let mut vm = MachineVm::new(machine("box"));
    assert_eq!(vm.can_mutate(), Err(model::Blocked::NotConnected));
    model::apply(&mut vm, MachineEvent::Connecting, |_| Trust::Confirmed);
    assert_eq!(vm.link, LinkState::Connecting);
    let hello = super::model::HelloInfo {
        machine_id: "m1".into(),
        boot_id: "b1".into(),
        protocol: 21,
        version: "t".into(),
        os: "linux".into(),
    };
    model::apply(
        &mut vm,
        MachineEvent::Connected {
            hello: hello.clone(),
            identity: identity("u@h", &[], "m1"),
        },
        |_| Trust::Needs(Confirm::First),
    );
    assert!(vm.needs_confirm());
    assert_eq!(vm.can_mutate(), Err(model::Blocked::NeedsConfirm));
    assert_eq!(vm.confirmed_machine_id(), None);
    vm.trust = Some(Trust::Confirmed);
    assert_eq!(vm.can_mutate(), Ok(()));
    assert_eq!(vm.confirmed_machine_id(), Some("m1"));
    let since = SystemTime::now();
    model::apply(
        &mut vm,
        MachineEvent::Lost {
            since,
            reason: "cut".into(),
        },
        |_| unreachable!(),
    );
    assert_eq!(vm.can_mutate(), Err(model::Blocked::NotConnected));
    model::apply(
        &mut vm,
        MachineEvent::Attempt {
            attempt: 3,
            reason: "timed out".into(),
        },
        |_| unreachable!(),
    );
    assert_eq!(vm.state_label(), "다시 연결 중 (3번째)");
    model::apply(
        &mut vm,
        MachineEvent::Reconnected {
            hello,
            boot_changed: false,
            input: Some(pod_remote::InputResolution::NotApplied { seq: 4 }),
        },
        |_| unreachable!(),
    );
    assert_eq!(vm.link, LinkState::Connected);
    assert_eq!(
        vm.notice.as_deref(),
        Some(super::copy::RESOLUTION_NOT_APPLIED)
    );
}

// ───────────────────────────── control thread ─────────────────────────────

fn fast_policy() -> ReconnectPolicy {
    ReconnectPolicy {
        first_delay: Duration::from_millis(50),
        max_delay: Duration::from_millis(200),
        jitter: 0.0,
        give_up_after: None,
        call_timeout: Duration::from_secs(3),
    }
}

struct Rig {
    remote: FakeRemote,
    proxy: pod_test_support::fault_proxy::FaultProxy,
    keys: Arc<FixedKeys>,
    handle: MachineHandle,
    events: UnboundedReceiver<(String, MachineEvent)>,
}

fn rig() -> Rig {
    let remote = FakeRemote::new("m-one", &[("t1", "shell")]);
    let proxy = remote.proxy();
    let keys = Arc::new(FixedKeys::new(&["ED25519 SHA256:A"]));
    let (tx, events) = mpsc::unbounded();
    let dial: Arc<dyn Dial> = Arc::new(ProxyDial { addr: proxy.addr() });
    let handle = MachineHandle::spawn(
        machine("box"),
        dial,
        keys.clone(),
        fast_policy(),
        Duration::from_millis(100),
        tx,
    );
    Rig {
        remote,
        proxy,
        keys,
        handle,
        events,
    }
}

impl Rig {
    /// The next event matching `want` (others are skipped), within 10 s.
    fn wait(&mut self, what: &str, want: impl Fn(&MachineEvent) -> bool) -> MachineEvent {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            match self.events.try_recv() {
                Ok((_, ev)) if want(&ev) => return ev,
                Ok(_) => {}
                Err(_) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        panic!("no {what} event");
    }

    fn connect_confirmed(&mut self) -> Identity {
        self.handle.connect();
        let MachineEvent::Connected { identity, .. } =
            self.wait("connected", |e| matches!(e, MachineEvent::Connected { .. }))
        else {
            unreachable!()
        };
        assert_eq!(identity.machine_id, "m-one");
        assert_eq!(identity.fingerprint_list(), ["ED25519 SHA256:A"]);
        self.handle.confirm(&identity);
        identity
    }
}

fn eventually(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < end, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Nothing changes on a machine the user has not confirmed (TM-13).
#[test]
fn an_unconfirmed_machine_gets_no_mutation() {
    let mut r = rig();
    r.handle.connect();
    r.wait("connected", |e| matches!(e, MachineEvent::Connected { .. }));
    r.wait("lists", |e| matches!(e, MachineEvent::Lists { .. }));
    let port = r.handle.port("t1");
    port.send(b"rm -rf /\r".to_vec());
    port.resize(100, 40);
    r.handle.refresh();
    r.wait("lists", |e| matches!(e, MachineEvent::Lists { .. }));
    assert_eq!(r.remote.mutations(), 0, "no input or resize sent");
    assert!(r.remote.typed().is_empty());
}

/// Input typed while the link is down is dropped, not queued: after the
/// reconnect nothing of it reaches the program; new input does (TM-14).
#[test]
fn input_while_down_is_never_replayed() {
    let mut r = rig();
    r.connect_confirmed();
    let port = r.handle.port("t1");
    port.send(b"a".to_vec());
    eventually("first input", || r.remote.typed_text("t1") == "a");
    let epoch_live = r.handle.epoch();

    r.proxy.cut();
    r.wait("lost", |e| matches!(e, MachineEvent::Lost { .. }));
    r.wait("attempt", |e| matches!(e, MachineEvent::Attempt { .. }));
    assert!(
        r.handle.epoch() > epoch_live,
        "a new epoch when the link dropped"
    );
    port.send(b"b".to_vec());
    port.resize(120, 40);
    r.proxy.restore();
    r.wait("reconnected", |e| {
        matches!(e, MachineEvent::Reconnected { .. })
    });
    r.wait("lists", |e| matches!(e, MachineEvent::Lists { .. }));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        r.remote.typed_text("t1"),
        "a",
        "typed while down: never sent"
    );
    assert_eq!(r.remote.calls("terminal.resize"), 0);
    port.send(b"c".to_vec());
    eventually("input after reconnect", || {
        r.remote.typed_text("t1") == "ac"
    });
}

/// After a confirmation, a reconnect that finds another known_hosts key is
/// refused before anything is sent (the board then asks again).
#[test]
fn a_changed_host_key_is_refused_on_reconnect() {
    let mut r = rig();
    r.connect_confirmed();
    r.keys.set(&["ED25519 SHA256:B"]);
    r.proxy.cut();
    r.wait("lost", |e| matches!(e, MachineEvent::Lost { .. }));
    r.proxy.restore();
    let MachineEvent::Failed(f) = r.wait("failed", |e| matches!(e, MachineEvent::Failed(_))) else {
        unreachable!()
    };
    assert_eq!(f.summary, super::copy::TRUST_HOST_KEY_TITLE);
    r.handle.port("t1").send(b"x".to_vec());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.remote.mutations(), 0);
}

/// Another machine behind the same profile: refused, and a fresh connect
/// reports its identity for a new confirmation instead of attaching.
#[test]
fn another_machine_is_refused_then_reported_for_confirmation() {
    let mut r = rig();
    let first = r.connect_confirmed();
    r.remote.set_machine_id("m-two");
    r.proxy.cut();
    r.wait("lost", |e| matches!(e, MachineEvent::Lost { .. }));
    r.proxy.restore();
    r.wait("failed", |e| matches!(e, MachineEvent::Failed(_)));
    // The attach connector refuses it too.
    let slots = pod_remote::AttachSlots::default();
    let (_attach, rx) = RemoteAttach::spawn(
        r.handle.attach_connector(),
        fast_policy(),
        AttachOptions::new(TerminalRef::id("t1")),
        &slots,
    )
    .unwrap();
    match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
        AttachUpdate::Failed { reason } => {
            assert!(
                reason.contains(super::copy::TRUST_MACHINE_TITLE),
                "{reason}"
            )
        }
        other => panic!("attach not refused: {other:?}"),
    }
    r.handle.connect();
    let MachineEvent::Connected { identity, .. } =
        r.wait("connected", |e| matches!(e, MachineEvent::Connected { .. }))
    else {
        unreachable!()
    };
    assert_eq!(identity.machine_id, "m-two");
    let pin = first.pin("box", 1);
    assert_eq!(
        evaluate(Some(&pin), &identity),
        Trust::Needs(Confirm::Machine {
            old: "m-one".into()
        })
    );
    // Showing the new identity does not unpin the attaches: until the
    // user confirms it, a view still refuses the other machine.
    let (_again, rx) = RemoteAttach::spawn(
        r.handle.attach_connector(),
        fast_policy(),
        AttachOptions::new(TerminalRef::id("t1")),
        &slots,
    )
    .unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        AttachUpdate::Failed { .. }
    ));
    r.handle.port("t1").send(b"x".to_vec());
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.remote.mutations(), 0, "nothing sent to the other machine");
    assert_eq!(
        r.remote.calls("terminal.attach"),
        0,
        "no view attached there"
    );
}

/// The attach view through the app's guarded connector: snapshot, live
/// output, a drop, and a re-attach whose skipped output is a gap.
#[test]
fn remote_attach_reports_the_gap_after_a_cut() {
    let mut r = rig();
    r.connect_confirmed();
    r.remote.emit("t1", b"hello\r\n");
    let slots = pod_remote::AttachSlots::default();
    let mut opts = AttachOptions::new(TerminalRef::id("t1"));
    opts.size = Some((80, 24));
    let (_attach, rx) =
        RemoteAttach::spawn(r.handle.attach_connector(), fast_policy(), opts, &slots).unwrap();
    let next = || rx.recv_timeout(Duration::from_secs(10)).expect("update");
    let AttachUpdate::Resync(first) = next() else {
        panic!("first update is the snapshot")
    };
    assert_eq!(
        (first.reason, first.seq, first.gap),
        (ResyncReason::First, 7, None)
    );
    r.remote.emit("t1", b"live");
    assert!(matches!(next(), AttachUpdate::Output { seq: 7, .. }));
    r.proxy.cut();
    loop {
        if let AttachUpdate::Disconnected { last_seq, .. } = next() {
            assert_eq!(last_seq, 11);
            break;
        }
    }
    r.remote.emit("t1", b" missed while cut");
    r.proxy.restore();
    loop {
        if let AttachUpdate::Resync(again) = next() {
            assert_eq!(again.reason, ResyncReason::Reconnected);
            let gap = again.gap.expect("gap");
            assert_eq!((gap.from_seq, gap.to_seq), (11, 28));
            assert!(gap.label().contains("17바이트"), "{}", gap.label());
            break;
        }
    }
    assert_eq!(r.remote.mutations(), 0, "attaching changes nothing");
    let t = Instant::now();
    drop(_attach);
    assert!(t.elapsed() < Duration::from_secs(5), "detach is prompt");
}

/// Fixture scenes never read or write profiles.
#[test]
fn the_fixture_backend_reads_nothing() {
    let b = super::board::MachinesBackend::fixture(Some("machines"));
    assert!(!b.store.exists());
    assert_eq!(
        super::fixture::machines("workspace").len(),
        0,
        "other scenes: no board"
    );
    assert_eq!(super::fixture::machines("machines").len(), 3);
    let m: Machine = super::fixture::machines("machine-trust")[0].profile.clone();
    assert!(m.validate().is_ok());
    // The system backend uses the CLI's profiles file (not read here).
    let sys = super::board::MachinesBackend::system();
    assert_eq!(sys.store, MachineStore::open_default().path());
}
