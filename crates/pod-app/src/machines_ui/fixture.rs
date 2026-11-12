//! `--ui-fixture` data of the machine UI (visual baselines): a fixed board
//! and a remote terminal panel fed with fixed attach updates. No profile
//! file is read or written, no thread starts, nothing is contacted.
//!
//! Scenes: `machines` (settings category + board), `machine-terminal`
//! (a remote panel after a gap, disconnected again and reconnecting) and
//! `machine-trust` (the host key change confirmation). Every other scene
//! gets an empty board, so existing baselines do not change.

use std::time::{Duration, SystemTime};

use gpui_kit::*;
use pod_remote::session::Link;
use pod_remote::ssh::ErrorKind;
use pod_remote::{AttachUpdate, Gap, Machine, Resync, ResyncReason};
use podd::protocol::{TerminalInfo, TerminalState};

use super::controller::Dial;
use super::model::{
    Failure, HelloInfo, LinkState, MachineVm, RemoteRef, RemoteTaskVm, RemoteTerminalVm,
};
use super::terminal::RemoteTerminalPanel;
use super::trust::{Confirm, Identity, Trust};
use crate::workspace::Workspace;

pub const SCENES: [&str; 3] = ["machines", "machine-terminal", "machine-trust"];

/// Fixture runs never dial.
pub struct NoDial;

impl Dial for NoDial {
    fn dial(&self, _: &Machine) -> Result<Link, Box<Failure>> {
        Err(Box::new(Failure::other("fixture", false)))
    }
}

/// The fixture's frozen "now" (the dense layout's instant).
pub fn now() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_millis(crate::layouts::dense::FIXTURE_NOW)
}

fn ago(secs: u64) -> SystemTime {
    now() - Duration::from_secs(secs)
}

const KEY_NOW: &str = "ED25519 SHA256:q3Vb8w0dPz1kTn4Yh2Lm7Rc5Xe9Fa6Gu1Js3Ko8Wi0";
const KEY_OLD: &str = "ED25519 SHA256:Hn2Pc7Yt4Lw9Qa1Zk6Rv3Ue8Sb5Dm0Fj2Gx7Ci4No";

fn machine(name: &str, host: &str, user: &str, port: Option<u16>) -> Machine {
    Machine {
        name: name.into(),
        host: host.into(),
        user: Some(user.into()),
        port,
        identity_file: None,
        known_hosts: None,
    }
}

fn hello(id: &str) -> HelloInfo {
    HelloInfo {
        machine_id: id.into(),
        boot_id: "b-0001".into(),
        protocol: podd::protocol::PROTOCOL_VERSION,
        version: env!("CARGO_PKG_VERSION").into(),
        os: "linux".into(),
    }
}

fn identity(m: &Machine, id: &str, key: &str) -> Identity {
    Identity {
        target: m.target(),
        known_hosts: None,
        fingerprints: Ok(vec![key.into()]),
        machine_id: id.into(),
    }
}

fn term(id: &str, name: &str, task: Option<&str>) -> RemoteTerminalVm {
    RemoteTerminalVm {
        id: id.into(),
        name: Some(name.into()),
        state: TerminalState::Running,
        task: task.map(Into::into),
        created_ms: 1,
    }
}

/// build-box: connected and confirmed, two terminals in one task.
fn build_box(trust: Trust) -> MachineVm {
    let p = machine("build-box", "build.example.com", "alice", None);
    let mut vm = MachineVm::new(p.clone());
    vm.link = LinkState::Connected;
    vm.hello = Some(hello("m-7f3a9c41"));
    vm.identity = Some(identity(&p, "m-7f3a9c41", KEY_NOW));
    vm.trust = Some(trust);
    vm.last_rx = Some(ago(3));
    vm.terminals = vec![
        term("t1", "agent", Some("web/fix-login")),
        term("t2", "shell", None),
    ];
    vm.tasks = vec![RemoteTaskVm {
        id: "web/fix-login".into(),
        name: "fix-login".into(),
        terminals: vec!["t1".into()],
    }];
    vm
}

pub fn machines(scene: &str) -> Vec<MachineVm> {
    if !SCENES.contains(&scene) {
        return Vec::new();
    }
    if scene == "machine-trust" {
        return vec![build_box(Trust::Needs(Confirm::HostKey {
            old: vec![KEY_OLD.into()],
        }))];
    }
    let gpu = {
        let p = machine("gpu-lab", "10.0.0.7", "bob", Some(2222));
        let mut vm = MachineVm::new(p.clone());
        vm.link = LinkState::Lost {
            since: ago(42),
            reason: "ssh: connect to host 10.0.0.7 port 2222: Connection timed out".into(),
            attempt: 3,
        };
        vm.hello = Some(hello("m-19d0e2aa"));
        vm.identity = Some(identity(&p, "m-19d0e2aa", KEY_OLD));
        vm.trust = Some(Trust::Confirmed);
        vm.last_rx = Some(ago(45));
        vm.terminals = vec![term("t4", "train", None)];
        vm
    };
    let staging = {
        let mut vm = MachineVm::new(machine("staging", "staging.example.com", "deploy", None));
        let kind = ErrorKind::HostKeyChanged;
        vm.link = LinkState::Failed(Failure {
            kind: Some(kind),
            summary: kind.summary().into(),
            hint: kind.hint().into(),
            detail: "ssh가 연결을 닫았습니다 (종료 코드 255)".into(),
            stderr: vec![
                "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @".into(),
                "Host key verification failed.".into(),
            ],
            sent_fingerprint: Some("SHA256:Zr4Tq8Nw1Bv6Kc3Hy9Pa2Le7Mx5Ud0Gs4Fo1Ji8Wk".into()),
            retryable: false,
        });
        vm
    };
    vec![build_box(Trust::Confirmed), gpu, staging]
}

/// The attach updates of the `machine-terminal` panel, in order.
fn terminal_updates() -> Vec<AttachUpdate> {
    let info = TerminalInfo {
        id: "t1".into(),
        name: Some("agent".into()),
        argv: vec!["bash".into()],
        cwd: "/home/alice/web".into(),
        state: TerminalState::Running,
        cols: 100,
        rows: 30,
        created_ms: 1,
        ..Default::default()
    };
    // Leading blanks matter (cargo's right-aligned verbs): no `\` continuations.
    let screen = [
        "\x1b[0m\x1b[H\x1b[2J\x1b[3J",
        "alice@build-box:~/web$ cargo test -p login\r\n",
        "   Compiling login v0.3.1 (/home/alice/web/login)\r\n",
        "    Finished `test` profile in 4.21s\r\n",
        "     Running unittests src/lib.rs\r\n",
        "test session::refresh ... ok\r\n",
        "test session::expiry ... ok\r\n",
        "test result: ok. 2 passed; 0 failed\r\n",
        "alice@build-box:~/web$ ",
    ]
    .concat();
    let resync = |reason, seq, gap| {
        AttachUpdate::Resync(Box::new(Resync {
            reason,
            terminal: info.clone(),
            seq,
            snapshot: screen.as_bytes().to_vec(),
            gap,
            scrollback_omitted: None,
            attempts: 1,
        }))
    };
    vec![
        resync(ResyncReason::First, 1_840, None),
        AttachUpdate::Disconnected {
            at: ago(60),
            reason: "원격 연결이 닫혔습니다".into(),
            last_seq: 1_840,
        },
        resync(
            ResyncReason::Reconnected,
            6_448,
            Some(Gap {
                from_seq: 1_840,
                to_seq: 6_448,
            }),
        ),
        AttachUpdate::Disconnected {
            at: ago(8),
            reason: "원격 연결이 닫혔습니다".into(),
            last_seq: 6_448,
        },
        AttachUpdate::Reconnecting {
            attempt: 2,
            delay: Duration::from_secs(4),
        },
    ]
}

/// Run a machine scene once the fixture window is up.
pub fn run(scene: &str, w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    match scene {
        "machines" => w.open_machine_settings(window, cx),
        "machine-trust" => {
            let board = w.machines.downgrade();
            super::view::open_trust_dialog(board, "build-box".into(), window, cx);
        }
        "machine-terminal" => {
            let target = RemoteRef {
                machine: "build-box".into(),
                machine_id: "m-7f3a9c41".into(),
                id: "t1".into(),
            };
            let panel = cx.new(|cx| RemoteTerminalPanel::fixture(target, now(), window, cx));
            panel.update(cx, |p, cx| {
                for u in terminal_updates() {
                    p.apply(u, cx);
                }
            });
            w.add_remote_panel(panel, window, cx);
        }
        _ => {}
    }
}
