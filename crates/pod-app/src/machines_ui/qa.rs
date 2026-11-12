//! Headless QA of the machine UI: the real workspace on GPUI's test
//! platform, machines reached through the loopback fault proxy and the
//! in-process fake remote (no ssh, no daemon). Covers the settings page
//! (TM-13 statement, profile validation, edit, remove), the host-key change
//! confirmation (nothing sent before it), and a remote terminal across a
//! cut: banner, disabled input, gap marker, no replay (TM-14).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

// Not `gpui_kit::*`: its `test` macro would shadow `#[test]`.
use gpui_kit::{Bounds, Context, Entity, Focusable, Modifiers, Pixels, Point, Window, point};
use pod_remote::{MachineStore, ReconnectPolicy};
use pod_test_support::fault_proxy::FaultProxy;

use super::board::MachinesBackend;
use super::fake_remote::{FakeRemote, ProxyDial};
use super::host_keys::FixedKeys;
use super::model::{Blocked, RemoteRef};
use super::terminal::{RemoteLink, RemoteTerminalPanel};
use super::trust::{Identity, TrustStore};
use crate::native_qa::{Harness, seed_two_tasks};
use crate::workspace::StartOptions;

const KEY_A: &str = "ED25519 SHA256:AAAA";
const KEY_B: &str = "ED25519 SHA256:BBBB";

fn store_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pod-app-machines-qa-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("machines.json")
}

fn backend(store: PathBuf, proxy: Option<&FaultProxy>, keys: Arc<FixedKeys>) -> MachinesBackend {
    let mut b = MachinesBackend::fixture(None);
    b.fixture = None;
    b.store = store;
    b.keys = keys;
    if let Some(p) = proxy {
        b.dial = Arc::new(ProxyDial { addr: p.addr() });
    }
    b.policy = ReconnectPolicy {
        first_delay: Duration::from_millis(50),
        max_delay: Duration::from_millis(200),
        jitter: 0.0,
        give_up_after: None,
        call_timeout: Duration::from_secs(3),
    };
    b.poll = Duration::from_millis(200);
    b
}

fn harness(name: &'static str, b: MachinesBackend) -> Harness {
    let options = StartOptions {
        machines: Some(b),
        ..Default::default()
    };
    Harness::with_options(name, options, seed_two_tasks)
}

fn center(b: Bounds<Pixels>) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width / 2.,
        b.origin.y + b.size.height / 2.,
    )
}

fn click(h: &mut Harness, id: &str) {
    let b = h
        .bounds(id)
        .unwrap_or_else(|| panic!("{id} is not on screen"));
    h.cx.simulate_click(center(b), Modifiers::none());
    h.settle();
}

/// Settle the window until `cond` holds; the machine threads run for real.
fn until(h: &mut Harness, what: &str, mut cond: impl FnMut(&mut Harness) -> bool) {
    let end = Instant::now() + Duration::from_secs(15);
    loop {
        h.settle();
        if cond(h) {
            return;
        }
        assert!(Instant::now() < end, "timed out: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn command(h: &mut Harness, id: &str) {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.command(id, window, cx)));
    h.settle();
}

fn board_update<R>(
    h: &mut Harness,
    f: impl FnOnce(&mut super::MachineBoard, &mut Window, &mut Context<super::MachineBoard>) -> R,
) -> R {
    let ws = h.ws.clone();
    let board = h.cx.update(|_, cx| ws.read(cx).machines.clone());
    let r =
        h.cx.update(|window, cx| board.update(cx, |b, cx| f(b, window, cx)));
    h.settle();
    r
}

fn open_panel(h: &mut Harness, target: RemoteRef) -> Entity<RemoteTerminalPanel> {
    let ws = h.ws.clone();
    let t = target.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_remote_terminal(t, window, cx)));
    h.settle();
    let board = h.cx.update(|_, cx| ws.read(cx).machines.clone());
    h.cx.update(|_, cx| {
        board
            .read(cx)
            .panels
            .iter()
            .filter_map(|p| p.upgrade())
            .find(|p| p.read(cx).target == target)
            .expect("panel opened")
    })
}

fn focus_panel(h: &mut Harness, p: &Entity<RemoteTerminalPanel>) {
    let p = p.clone();
    h.cx.update(|window, cx| p.read(cx).focus_handle(cx).focus(window, cx));
    h.settle();
}

fn link(h: &mut Harness, p: &Entity<RemoteTerminalPanel>) -> RemoteLink {
    h.cx.update(|_, cx| p.read(cx).link.clone())
}

/// TM-13 statement on the page; pod-remote's rules refuse a bad host
/// before anything is written; a valid profile is saved, edited, removed.
#[test]
fn settings_profiles_validate_edit_and_remove() {
    let store = store_dir("settings");
    let mut h = harness(
        "machines-settings",
        backend(store.clone(), None, Arc::new(FixedKeys::new(&[]))),
    );
    command(&mut h, "machines");
    assert!(
        h.bounds("machines-full-rights").is_some(),
        "full-rights statement shown"
    );
    click(&mut h, "machine-add");
    assert!(h.dialog_open(), "the add dialog opened");
    // Name (focused), then host with an option in it.
    h.type_text("box1");
    h.keys("tab");
    h.type_text("-oProxyCommand=calc");
    h.keys("enter");
    assert!(h.dialog_open(), "not saved");
    assert!(
        h.bounds("machine-form-error-Host").is_some(),
        "host error shown"
    );
    assert!(!store.exists(), "nothing written");
    // Fix the host; Enter saves.
    h.keys("ctrl-a");
    h.type_text("build.example.com");
    h.keys("enter");
    assert!(!h.dialog_open(), "saved and closed");
    let saved = MachineStore::at(store.clone()).load().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(
        (saved[0].name.as_str(), saved[0].host.as_str()),
        ("box1", "build.example.com")
    );
    assert!(h.bounds("machine-profile-0").is_some());
    assert!(
        h.bounds("machine-board").is_some(),
        "the sidebar board appears"
    );

    // Edit: a bad port is refused, a good one saved.
    click(&mut h, "machine-edit-0");
    for _ in 0..3 {
        h.keys("tab");
    }
    h.type_text("70000");
    h.keys("enter");
    assert!(
        h.bounds("machine-form-error-Port").is_some(),
        "port error shown"
    );
    h.keys("ctrl-a");
    h.type_text("2222");
    h.keys("enter");
    assert!(!h.dialog_open());
    assert_eq!(
        MachineStore::at(store.clone()).load().unwrap()[0].port,
        Some(2222)
    );

    // Remove (with its trust record).
    TrustStore::beside(&store)
        .put(
            Identity {
                target: "x".into(),
                known_hosts: None,
                fingerprints: Ok(vec![]),
                machine_id: "m".into(),
            }
            .pin("box1", 1),
        )
        .unwrap();
    click(&mut h, "machine-remove-0");
    click(&mut h, "machine-remove-ok");
    assert!(MachineStore::at(store.clone()).load().unwrap().is_empty());
    assert_eq!(TrustStore::beside(&store).get("box1"), None);
    assert!(h.bounds("machine-board").is_none(), "no profiles: no board");
}

fn add_profile(store: &std::path::Path, name: &str) {
    MachineStore::at(store.to_path_buf())
        .add(super::fake_remote::machine(name))
        .unwrap();
}

fn pin(store: &std::path::Path, name: &str, key: &str, machine_id: &str) {
    let m = super::fake_remote::machine(name);
    TrustStore::beside(store)
        .put(
            Identity {
                target: m.target(),
                known_hosts: None,
                fingerprints: Ok(vec![key.into()]),
                machine_id: machine_id.into(),
            }
            .pin(name, 1),
        )
        .unwrap();
}

/// known_hosts now has another key than the one confirmed: the board asks
/// again, the dialog shows both fingerprints, and nothing (attach, input,
/// resize) reaches the machine until the user confirms.
#[test]
fn a_changed_host_key_is_confirmed_before_anything_is_sent() {
    let remote = FakeRemote::new("m-box", &[("t1", "shell")]);
    let proxy = remote.proxy();
    let store = store_dir("hostkey");
    add_profile(&store, "box");
    pin(&store, "box", KEY_A, "m-box");
    let keys = Arc::new(FixedKeys::new(&[KEY_B]));
    let mut h = harness(
        "machines-hostkey",
        backend(store.clone(), Some(&proxy), keys),
    );
    assert!(h.bounds("machine-board").is_some());
    click(&mut h, "machine-connect-0");
    until(&mut h, "the trust dialog", |h| {
        h.dialog_open() && h.bounds("machine-trust-identity").is_some()
    });
    let vm = board_update(&mut h, |b, _, _| b.vm("box").cloned().unwrap());
    let (title, _, rows) = super::view::trust_rows(&vm);
    assert!(
        title.starts_with(super::copy::TRUST_HOST_KEY_TITLE),
        "{title}"
    );
    let shown: Vec<&str> = rows.iter().map(|(_, v)| v.as_str()).collect();
    assert!(
        shown.contains(&KEY_B) && shown.contains(&KEY_A),
        "new and old keys: {shown:?}"
    );

    // Close the dialog without confirming: read-only.
    h.keys("escape");
    let target = RemoteRef {
        machine: "box".into(),
        machine_id: "m-box".into(),
        id: "t1".into(),
    };
    let panel = open_panel(&mut h, target.clone());
    focus_panel(&mut h, &panel);
    h.type_text("echo pwned");
    std::thread::sleep(Duration::from_millis(300));
    h.settle();
    assert_eq!(
        link(&mut h, &panel),
        RemoteLink::Waiting(super::board::NotReady::Waiting)
    );
    assert!(
        h.cx.update(|_, cx| panel.read(cx).refused.get()) > 0,
        "keys dropped"
    );
    assert_eq!(remote.mutations(), 0, "no input/resize before confirming");
    assert_eq!(
        remote.calls("terminal.attach"),
        0,
        "no attach before confirming"
    );
    assert!(h.bounds("remote-terminal-banner").is_some());

    // Confirm from the board; the record now holds the new key.
    click(&mut h, "machine-confirm-0");
    assert!(h.dialog_open());
    click(&mut h, "machine-trust-ok");
    assert!(!h.dialog_open());
    assert_eq!(
        TrustStore::beside(&store).get("box").unwrap().fingerprints,
        [KEY_B]
    );
    until(&mut h, "attached", |h| {
        link(h, &panel) == RemoteLink::Live && h.cx.update(|_, cx| panel.read(cx).blocked.is_none())
    });
    focus_panel(&mut h, &panel);
    h.type_text("ok");
    until(&mut h, "input after confirming", |_| {
        remote.typed_text("t1") == "ok"
    });
    assert!(
        !remote.typed_text("t1").contains("pwned"),
        "earlier keys never sent"
    );
}

/// A remote terminal across a cut: the banner says disconnected with the
/// last received time and that input is blocked; typed keys are dropped;
/// the re-attach shows the gap marker; nothing typed while down is sent.
#[test]
fn a_cut_shows_the_banner_blocks_input_and_marks_the_gap() {
    let remote = FakeRemote::new("m-box", &[("t1", "shell")]);
    remote.emit("t1", b"$ ");
    let proxy = remote.proxy();
    let store = store_dir("cut");
    add_profile(&store, "box");
    pin(&store, "box", KEY_A, "m-box");
    let keys = Arc::new(FixedKeys::new(&[KEY_A]));
    let mut h = harness("machines-cut", backend(store, Some(&proxy), keys));
    click(&mut h, "machine-connect-0");
    until(&mut h, "connected and listed", |h| {
        h.bounds("machine-terminal-0-0").is_some()
    });
    assert!(!h.dialog_open(), "a matching record needs no dialog");
    click(&mut h, "machine-terminal-0-0");
    let panel = board_update(&mut h, |b, _, _| b.panels.iter().find_map(|p| p.upgrade()))
        .expect("the board row opened a panel");
    until(&mut h, "live", |h| {
        link(h, &panel) == RemoteLink::Live && h.cx.update(|_, cx| panel.read(cx).blocked.is_none())
    });
    assert!(
        h.bounds("remote-terminal-banner").is_none(),
        "no banner while live"
    );
    // The layout counts the panel: no "nothing open" hint over it.
    let ws = h.ws.clone();
    let panels =
        h.cx.update(|_, cx| ws.read(cx).layout_frame(&[], cx).panels);
    assert_eq!(panels.remote_terminals, 1);
    assert!(!panels.dock_empty());
    focus_panel(&mut h, &panel);
    h.type_text("a");
    until(&mut h, "first input", |_| remote.typed_text("t1") == "a");

    proxy.cut();
    until(&mut h, "disconnected", |h| {
        matches!(link(h, &panel), RemoteLink::Down { .. })
    });
    let banner =
        h.cx.update(|_, cx| panel.read(cx).banner())
            .expect("banner");
    assert!(banner.1.starts_with(super::copy::STATE_LOST), "{banner:?}");
    assert!(
        banner.1.contains("마지막 수신") && banner.1.contains(super::copy::PANEL_INPUT_BLOCKED),
        "{banner:?}"
    );
    assert!(h.bounds("remote-terminal-banner").is_some(), "banner drawn");
    let refused = h.cx.update(|_, cx| panel.read(cx).refused.get());
    focus_panel(&mut h, &panel);
    h.type_text("b");
    assert!(
        h.cx.update(|_, cx| panel.read(cx).refused.get()) > refused,
        "typing refused"
    );
    assert_eq!(
        h.cx.update(|_, cx| panel.read(cx).blocked),
        Some(Blocked::NotConnected)
    );
    remote.emit("t1", b"output while cut");

    proxy.restore();
    until(&mut h, "live again", |h| {
        link(h, &panel) == RemoteLink::Live && h.cx.update(|_, cx| panel.read(cx).blocked.is_none())
    });
    let marker =
        h.cx.update(|_, cx| panel.read(cx).marker.clone())
            .expect("gap marker");
    assert!(marker.contains("16바이트"), "{marker}");
    assert!(
        h.bounds("remote-terminal-gap").is_some(),
        "marker drawn in the terminal view"
    );
    std::thread::sleep(Duration::from_millis(300));
    h.settle();
    assert_eq!(
        remote.typed_text("t1"),
        "a",
        "input typed while down never sent"
    );
    focus_panel(&mut h, &panel);
    h.type_text("c");
    until(&mut h, "input after reconnect", |_| {
        remote.typed_text("t1") == "ac"
    });
    // The board knows the machine is back.
    let state = board_update(&mut h, |b, _, _| b.vm("box").unwrap().can_mutate());
    assert_eq!(state, Ok(()));
    drop(proxy);
}
