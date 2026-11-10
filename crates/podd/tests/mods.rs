//! Extensions in the real daemon (docs/16): mods from a temp `POD_MODS_DIR`
//! (set by the harness, never the user's %APPDATA%), `mod.*` RPCs, a status
//! detector feeding the engine below OSC, `mod.notify` / `mod.changed` on the
//! status stream, palette commands, and a looping mod being stopped.

mod common;

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use podd::client::{Client, SubscribeEvent};
use podd::protocol::*;
use serde_json::json;

fn write_mod(root: &Path, name: &str, perms: &[&str], js: &str) {
    let d = root.join(name);
    std::fs::create_dir_all(&d).unwrap();
    let m = json!({"pod": 1, "name": name, "version": "0.1.0", "entry": "main.js", "permissions": perms});
    std::fs::write(d.join("mod.json"), m.to_string()).unwrap();
    std::fs::write(d.join("main.js"), js).unwrap();
}

fn show(c: &mut Client, id: &str) -> StatusInfo {
    c.call::<TerminalShow>(&TerminalRef::id(id))
        .expect("show")
        .status
        .expect("status")
}

fn show_until(
    c: &mut Client,
    id: &str,
    what: &str,
    pred: impl Fn(&StatusInfo) -> bool,
) -> StatusInfo {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let s = show(c, id);
        if pred(&s) {
            return s;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: last {s:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn osc(c: &mut Client, id: &str, state: &str) {
    send(
        c,
        id,
        &format!("[Console]::Write([char]27 + ']7501;state={state}' + [char]7)"),
    );
}

#[test]
fn mods_in_the_daemon() {
    let d = Daemon::start("mods");
    let root = d.state_dir.join("extensions");
    write_mod(
        &root,
        "det",
        &["status"],
        r#"
        pod.detector({
          match: ["powershell"],
          onScreen(ctx) {
            const text = ctx.lines.join("\n");
            if (text.includes("MODTEST-BLOCKED")) return { state: "blocked", kind: "question", msg: "mod asks" };
            if (text.includes("MODTEST-IDLE")) return { state: "idle", msg: "prompt seen" };
            return null;
          },
        });
    "#,
    );
    write_mod(
        &root,
        "note",
        &["notify", "commands"],
        r#"
        pod.on("status.changed", (ev) => { if (ev.source === "mod") pod.notify("mod state", ev.terminal + " " + ev.state); });
        pod.command({ id: "ping", title: "Ping" }, (ctx) => pod.notify("pong", JSON.stringify(ctx)));
    "#,
    );
    write_mod(
        &root,
        "spin",
        &["commands"],
        r#"pod.command({ id: "loop", title: "Loop" }, () => { for (;;) {} });"#,
    );

    let mut c = d.client();
    let list = c.call::<ModListMethod>(&Empty {}).unwrap();
    assert!(list.enabled);
    assert!(
        list.list.mods.is_empty(),
        "the folder was empty when the daemon started"
    );
    let list = c.call::<ModReload>(&ModReloadParams::default()).unwrap();
    assert_eq!(
        list.list.root,
        root.display().to_string(),
        "the harness isolates mods"
    );
    assert_eq!(list.list.mods.len(), 3);
    assert!(list.list.mods.iter().all(|m| m.state == ModState::Disabled));

    let (_, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_any() {
            if tx.send(ev).is_err() {
                break;
            }
        }
    });
    let mut seen: Vec<SubscribeEvent> = Vec::new();
    let mut wait_event = |pred: &dyn Fn(&SubscribeEvent) -> bool, what: &str| {
        if seen.iter().any(pred) {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
                let hit = pred(&ev);
                seen.push(ev);
                if hit {
                    return;
                }
            }
        }
        panic!("no {what} event");
    };

    for m in ["det", "note", "spin"] {
        let info = c
            .call::<ModSetEnabled>(&ModEnableParams {
                name: m.into(),
                enabled: true,
            })
            .unwrap();
        assert_eq!(info.state, ModState::Running, "{m}: {:?}", info.error);
    }

    // Detector: the screen says idle → source "mod".
    let t = powershell(&mut c, "mod-target");
    send(&mut c, &t.id, "Write-Host MODTEST-IDLE");
    let s = show_until(&mut c, &t.id, "mod idle", |s| {
        s.source.as_deref() == Some("mod") && s.state == "idle"
    });
    assert_eq!(s.msg.as_deref(), Some("prompt seen"));
    assert!((s.confidence - 0.75).abs() < 0.01);
    wait_event(
        &|e| matches!(e, SubscribeEvent::ModNotify(n) if n.notice.mod_name == "note" && n.notice.body.contains("idle")),
        "mod.notify",
    );

    // OSC outranks the mod; clearing OSC falls back to the mod's evidence.
    osc(&mut c, &t.id, "working");
    show_until(&mut c, &t.id, "osc working", |s| {
        s.source.as_deref() == Some("osc") && s.state == "working"
    });
    send(&mut c, &t.id, "Write-Host MODTEST-BLOCKED");
    std::thread::sleep(Duration::from_millis(700));
    assert_eq!(
        show(&mut c, &t.id).state,
        "working",
        "the mod cannot override osc"
    );
    osc(&mut c, &t.id, "clear");
    let s = show_until(&mut c, &t.id, "back to mod", |s| {
        s.source.as_deref() == Some("mod")
    });
    assert_eq!(
        (s.state.as_str(), s.kind.as_deref()),
        ("blocked", Some("question"))
    );

    // Palette command with a scope.
    c.call::<ModCommand>(&ModCommandParams {
        name: "note".into(),
        command: "ping".into(),
        task: Some("repo/x".into()),
        terminal: Some(t.id.clone()),
    })
    .unwrap();
    wait_event(
        &|e| matches!(e, SubscribeEvent::ModNotify(n) if n.notice.title == "pong" && n.notice.body.contains("repo/x") && n.notice.task.as_deref() == Some("repo/x")),
        "pong",
    );

    // A looping command is stopped; the daemon keeps answering.
    let started = Instant::now();
    let err = c
        .call::<ModCommand>(&ModCommandParams {
            name: "spin".into(),
            command: "loop".into(),
            task: None,
            terminal: None,
        })
        .unwrap_err();
    assert!(format!("{err:?}").contains("CPU"), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    wait_event(
        &|e| matches!(e, SubscribeEvent::ModChanged(m) if m.info.name == "spin" && m.info.state == ModState::Stopped && m.info.stop_reason == Some(StopReason::Cpu)),
        "mod.changed stopped",
    );
    let _ = c.call::<ServerStatus>(&Empty {}).expect("daemon alive");

    // Logs, then disabling the detector clears its evidence.
    let log = c
        .call::<ModLog>(&ModLogParams {
            name: "det".into(),
            tail: None,
        })
        .unwrap();
    assert!(
        log.lines.iter().any(|l| l.contains("loaded")),
        "{:?}",
        log.lines
    );
    assert!(
        d.state_dir.join("mods").join("det.log").is_file(),
        "per-mod log file in the state dir"
    );
    c.call::<ModSetEnabled>(&ModEnableParams {
        name: "det".into(),
        enabled: false,
    })
    .unwrap();
    show_until(&mut c, &t.id, "mod evidence gone", |s| {
        s.source.as_deref() != Some("mod")
    });

    // Unknown permission name and missing mod are RPC errors.
    assert!(
        c.call::<ModGrant>(&ModGrantParams {
            name: "nope".into(),
            permission: Permission::Notify,
            granted: true
        })
        .is_err()
    );
}
