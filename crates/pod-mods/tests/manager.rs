//! The mod thread end to end with a fake host: discovery, enable/grant
//! persistence, event delivery, permission and scope checks, detectors,
//! crash isolation and the shipped example mods.
//!
//! Needs the mod thread (feature `runtime`); without it only the wire types
//! build and `wire_snapshots` checks them.
#![cfg(feature = "runtime")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pod_mods::*;
use pod_test_support::wait::{stays, until};
use serde_json::json;

/// How long a wait for something that should happen may take.
const READY: Duration = Duration::from_secs(5);
/// Probe interval of the waits.
const POLL: Duration = Duration::from_millis(10);

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
    terminals: Mutex<Vec<TerminalFacts>>,
    tasks: Mutex<Vec<TaskFacts>>,
    groups: Mutex<Vec<GroupFacts>>,
}

impl Fake {
    fn record(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    /// The calls once `pred` holds, or as they are after [`READY`] (the
    /// caller's assert shows them).
    fn wait_for(&self, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
        until(READY, POLL, || {
            let c = self.calls();
            if pred(&c) { Ok(c) } else { Err(c) }
        })
        .unwrap_or_else(|t| t.last)
    }
}

impl Host for Fake {
    fn terminals(&self) -> Vec<TerminalFacts> {
        self.terminals.lock().unwrap().clone()
    }
    fn tasks(&self) -> Vec<TaskFacts> {
        self.tasks.lock().unwrap().clone()
    }
    fn groups(&self) -> Vec<GroupFacts> {
        self.groups.lock().unwrap().clone()
    }
    fn notify(&self, n: Notice) {
        self.record(format!(
            "notify {} {} | {} | {}",
            n.mod_name, n.level, n.title, n.body
        ));
    }
    fn send_text(&self, terminal: &str, text: &str, enter: bool) -> Result<(), String> {
        self.record(format!("send {terminal} {text:?} {enter}"));
        Ok(())
    }
    fn set_status(
        &self,
        owner: &str,
        terminal: &str,
        hint: Option<StatusHint>,
    ) -> Result<(), String> {
        self.record(match hint {
            Some(h) => format!(
                "status {owner} {terminal} {}{}{}",
                h.state,
                h.kind.map(|k| format!(":{k}")).unwrap_or_default(),
                h.msg.map(|m| format!(" {m}")).unwrap_or_default()
            ),
            None => format!("status {owner} {terminal} clear"),
        });
        Ok(())
    }
    fn clear_status(&self, owner: &str) {
        self.record(format!("clear {owner}"));
    }
    fn run_check(
        &self,
        owner: &str,
        task: &str,
        check: &str,
        argv: Vec<String>,
    ) -> Result<(), String> {
        self.record(format!("check {owner} {task} {check} {}", argv.join(" ")));
        Ok(())
    }
    fn request_gate(
        &self,
        owner: &str,
        task: &str,
        title: &str,
        _message: &str,
    ) -> Result<String, String> {
        self.record(format!("gate {owner} {task} {title}"));
        Ok("g7".into())
    }
    fn mod_changed(&self, info: &ModInfo) {
        self.record(format!(
            "changed {} {:?} {:?}",
            info.name, info.state, info.stop_reason
        ));
    }
}

fn write(dir: &Path, name: &str, manifest: serde_json::Value, js: &str) {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("mod.json"), manifest.to_string()).unwrap();
    std::fs::write(d.join("main.js"), js).unwrap();
}

fn start(root: &Path) -> (ModHandle, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    fake.terminals.lock().unwrap().extend([
        TerminalFacts {
            id: "t1".into(),
            task: Some("repo/a".into()),
            state: "working".into(),
            argv0: "aider".into(),
            running: true,
            ..Default::default()
        },
        TerminalFacts {
            id: "t2".into(),
            task: Some("repo/b".into()),
            state: "idle".into(),
            argv0: "cmd".into(),
            running: true,
            ..Default::default()
        },
    ]);
    let cfg = ModConfig {
        root: root.to_path_buf(),
        log_dir: Some(root.join("_logs")),
        limits: Limits {
            load_ms: 500,
            call_ms: 150,
            ..Limits::default()
        },
    };
    (ModHandle::start(cfg, fake.clone()), fake)
}

fn manifest(name: &str, perms: &[&str]) -> serde_json::Value {
    json!({"pod": 1, "name": name, "version": "0.1.0", "entry": "main.js", "permissions": perms, "checks": {"test": ["cargo", "test"]}})
}

fn info<'a>(list: &'a ModList, name: &str) -> &'a ModInfo {
    list.mods.iter().find(|m| m.name == name).unwrap()
}

fn scope(task: &str, terminal: Option<&str>) -> Scope {
    Scope {
        task: Some(task.into()),
        terminal: terminal.map(Into::into),
    }
}

/// Post an event and wait until the mod thread processed it (a list round trip).
fn deliver(h: &ModHandle, name: &str, scope: Scope, payload: serde_json::Value) {
    h.post_event(name, scope, &payload);
    h.list().unwrap();
}

#[test]
fn discovery_enable_grant_and_persistence() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "hello",
        manifest("hello", &["notify", "terminal.send", "check"]),
        r#"pod.on("status.changed", () => {});"#,
    );
    write(
        root,
        "Bad Name",
        json!({"pod": 1, "name": "x", "version": "1"}),
        "",
    );
    write(
        root,
        "broken",
        json!({"pod": 1, "name": "broken", "version": "1", "permissions": ["fs"]}),
        "",
    );
    let (h, _fake) = start(root);
    let list = h.list().unwrap();
    assert_eq!(list.mods.len(), 3);
    assert_eq!(
        info(&list, "hello").state,
        ModState::Disabled,
        "new mods start disabled"
    );
    assert_eq!(info(&list, "broken").state, ModState::Invalid);
    assert!(
        info(&list, "broken")
            .error
            .as_deref()
            .unwrap()
            .contains("unknown permission")
    );
    assert_eq!(info(&list, "Bad Name").state, ModState::Invalid);
    assert!(h.set_enabled("broken", true).is_err());

    let m = h.set_enabled("hello", true).unwrap();
    assert_eq!(m.state, ModState::Running);
    let granted: Vec<_> = m
        .permissions
        .iter()
        .filter(|p| p.granted)
        .map(|p| p.name.name())
        .collect();
    assert_eq!(
        granted,
        vec!["notify"],
        "send and check need an explicit grant"
    );
    let m = h.grant("hello", Permission::TerminalSend, true).unwrap();
    assert!(
        m.permissions
            .iter()
            .any(|p| p.name == Permission::TerminalSend && p.granted)
    );
    assert!(
        h.grant("hello", Permission::Gate, true).is_err(),
        "not requested"
    );
    h.shutdown();

    // Persisted: a new manager comes up with the same state.
    let (h2, _f) = start(root);
    let m = info(&h2.list().unwrap(), "hello").clone();
    assert!(m.enabled);
    assert_eq!(m.state, ModState::Running);
    assert!(
        m.permissions
            .iter()
            .any(|p| p.name == Permission::TerminalSend && p.granted)
    );
    h2.set_enabled("hello", false).unwrap();
    assert_eq!(info(&h2.list().unwrap(), "hello").state, ModState::Disabled);
    assert!(
        std::fs::read_to_string(root.join(STATE_FILE))
            .unwrap()
            .contains("\"enabled\": false")
    );
}

/// pod-mods.json goes through `pod-mods.json.tmp` and a rename. A failed
/// write is reported as `"<path>: <error>"`, the mods folder is never created
/// for it, and the call's order stays: the change is made in memory, the save
/// fails, the mod is not restarted.
#[test]
fn state_file_write_failures_are_reported() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("mods");
    write(&root, "hello", manifest("hello", &["notify"]), "");
    let (h, _fake) = start(&root);
    let path = root.join(STATE_FILE);
    let blocker = root.join("pod-mods.json.tmp");
    std::fs::create_dir(&blocker).unwrap();
    let err = h.set_enabled("hello", true).unwrap_err();
    assert!(err.starts_with(&format!("{}: ", path.display())), "{err}");
    assert!(!path.exists());
    let m = info(&h.list().unwrap(), "hello").clone();
    assert!(m.enabled, "changed in memory before the save");
    assert_eq!(m.state, ModState::Disabled, "not started after the failure");

    std::fs::remove_dir(&blocker).unwrap();
    let m = h.set_enabled("hello", true).unwrap();
    assert_eq!(m.state, ModState::Running);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\n  \"mods\": {\n    \"hello\": {\n      \"enabled\": true,\n      \"granted\": [\n        \"notify\"\n      ]\n    }\n  }\n}"
    );
    assert!(!blocker.exists(), "the temporary file is renamed away");

    // The mods folder is gone: saving does not create it again.
    std::fs::remove_dir_all(&root).unwrap();
    let err = h.set_enabled("hello", false).unwrap_err();
    assert!(err.starts_with(&format!("{}: ", path.display())), "{err}");
    assert!(!root.exists());
    assert_eq!(info(&h.list().unwrap(), "hello").state, ModState::Running);
}

#[test]
fn events_reach_only_registered_mods_and_logs_are_kept() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "a",
        manifest("a", &["notify"]),
        r#"
        pod.on("status.changed", (ev) => pod.notify("state", `${ev.terminal} ${ev.state}`));
        console.log("loaded a");
    "#,
    );
    write(
        tmp.path(),
        "b",
        manifest("b", &[]),
        r#"pod.on("*", (ev, name) => console.log("any", name));"#,
    );
    let (h, fake) = start(tmp.path());
    h.set_enabled("a", true).unwrap();
    h.set_enabled("b", true).unwrap();
    assert!(h.wants_event("status.changed") && h.wants_event("gate.changed"));
    deliver(
        &h,
        "status.changed",
        scope("repo/a", Some("t1")),
        json!({"terminal": "t1", "state": "done"}),
    );
    deliver(
        &h,
        "gate.changed",
        scope("repo/a", None),
        json!({"gate": "g1"}),
    );
    let calls = fake.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|c| c.starts_with("notify"))
            .collect::<Vec<_>>(),
        vec!["notify a info | state | t1 done"]
    );
    let log_a = h.log("a", 50).unwrap().join("\n");
    assert!(
        log_a.contains("loaded a") && log_a.contains("notify: state"),
        "{log_a}"
    );
    let log_b = h.log("b", 50).unwrap().join("\n");
    assert!(
        log_b.contains("any status.changed") && log_b.contains("any gate.changed"),
        "{log_b}"
    );
    assert!(tmp.path().join("_logs").join("a.log").is_file());
}

#[test]
fn permissions_and_scope_are_enforced() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "p",
        manifest("p", &["notify", "terminal.send", "check", "gate"]),
        r#"
        const tryIt = (label, f) => { try { f(); console.log(label, "ok"); } catch (e) { console.log(label, "denied", e.message); } };
        pod.on("task.changed", (ev) => {
          tryIt("send-own", () => pod.terminal.send("t1", "hi", { enter: true }));
          tryIt("send-other", () => pod.terminal.send("t2", "rm -rf"));
          tryIt("status", () => pod.terminal.setStatus("t1", "idle"));
          tryIt("check-own", () => pod.task.runCheck("repo/a", "test"));
          tryIt("check-undeclared", () => pod.task.runCheck("repo/a", "deploy"));
          tryIt("check-other", () => pod.task.runCheck("repo/b", "test"));
          tryIt("gate-other", () => pod.gate.request("repo/b", "t", "m"));
          tryIt("gate-own", () => console.log("gate id", pod.gate.request("repo/a", "Ship?", "ok?")));
          tryIt("big", () => pod.terminal.send("t1", "x".repeat(5000)));
          tryIt("level", () => pod.notify("a", "b", "loud"));
        });
        pod.on("group.check", () => tryIt("no-scope", () => pod.terminal.send("t1", "x")));
        try { pod.command({ id: "c", title: "C" }, () => {}); } catch (e) { console.log("command denied", e.message); }
        try { pod.detector({ match: "aider", onScreen() { return null; } }); } catch (e) { console.log("detector denied", e.message); }
    "#,
    );
    let (h, fake) = start(tmp.path());
    h.set_enabled("p", true).unwrap();
    deliver(
        &h,
        "task.changed",
        scope("repo/a", None),
        json!({"task": "repo/a"}),
    );
    let log = h.log("p", 200).unwrap().join("\n");
    // Not granted yet: send and check are denied even in scope.
    assert!(log.contains("send-own denied PermissionDenied"), "{log}");
    assert!(log.contains("check-own denied PermissionDenied"), "{log}");
    assert!(log.contains("command denied PermissionDenied"), "{log}");
    assert!(log.contains("detector denied PermissionDenied"), "{log}");
    assert!(
        log.contains("status denied PermissionDenied"),
        "not requested: {log}"
    );
    assert!(
        log.contains("gate-own ok") && log.contains("gate id g7"),
        "{log}"
    );
    assert!(log.contains("gate-other denied PermissionDenied"), "{log}");
    assert!(log.contains("level denied InvalidArgument"), "{log}");

    h.grant("p", Permission::TerminalSend, true).unwrap();
    h.grant("p", Permission::Check, true).unwrap();
    deliver(
        &h,
        "task.changed",
        scope("repo/a", None),
        json!({"task": "repo/a"}),
    );
    deliver(&h, "group.check", Scope::default(), json!({"group": "g"}));
    let log = h.log("p", 200).unwrap().join("\n");
    assert!(log.contains("send-own ok"), "{log}");
    assert!(
        log.contains("send-other denied PermissionDenied: terminal t2 is not in task repo/a"),
        "{log}"
    );
    assert!(log.contains("check-own ok"), "{log}");
    assert!(
        log.contains("check-undeclared denied PermissionDenied"),
        "{log}"
    );
    assert!(log.contains("check-other denied PermissionDenied"), "{log}");
    assert!(log.contains("big denied InvalidArgument"), "{log}");
    assert!(log.contains("no-scope denied PermissionDenied"), "{log}");
    let calls = fake.calls();
    assert!(
        calls.contains(&r#"send t1 "hi" true"#.to_string()),
        "{calls:?}"
    );
    assert!(!calls.iter().any(|c| c.starts_with("send t2")), "{calls:?}");
    assert!(
        calls.contains(&"check p repo/a test cargo test".to_string()),
        "{calls:?}"
    );
}

#[test]
fn rate_limits() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "r",
        manifest("r", &["notify"]),
        r#"
        pod.on("task.changed", () => { let n = 0; try { for (;;) { pod.notify("n" + n); n++; } } catch (e) { console.log("stopped after", n, e.message); } });
    "#,
    );
    let (h, _fake) = start(tmp.path());
    h.set_enabled("r", true).unwrap();
    deliver(&h, "task.changed", scope("repo/a", None), json!({}));
    let log = h.log("r", 100).unwrap().join("\n");
    assert!(log.contains("stopped after 20 RateLimited"), "{log}");
}

#[test]
fn looping_throwing_and_memory_hungry_mods_are_stopped() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "loop-load",
        manifest("loop-load", &[]),
        "while (true) {}",
    );
    write(
        tmp.path(),
        "loop-later",
        manifest("loop-later", &[]),
        r#"pod.on("task.changed", () => { for (;;) {} });"#,
    );
    write(
        tmp.path(),
        "thrower",
        manifest("thrower", &[]),
        r#"pod.on("task.changed", () => { throw new Error("boom"); });"#,
    );
    write(
        tmp.path(),
        "hog",
        manifest("hog", &[]),
        r#"const keep = []; pod.on("task.changed", () => { for (;;) keep.push(new Array(4096).fill(keep.length)); });"#,
    );
    write(
        tmp.path(),
        "good",
        manifest("good", &["notify"]),
        r#"pod.on("task.changed", () => pod.notify("still here"));"#,
    );
    let (h, fake) = start(tmp.path());
    for m in ["loop-load", "loop-later", "thrower", "hog", "good"] {
        h.set_enabled(m, true).unwrap();
    }
    let list = h.list().unwrap();
    assert_eq!(info(&list, "loop-load").state, ModState::Stopped);
    assert_eq!(info(&list, "loop-load").stop_reason, Some(StopReason::Cpu));
    for _ in 0..3 {
        deliver(&h, "task.changed", scope("repo/a", None), json!({}));
    }
    let list = h.list().unwrap();
    assert_eq!(info(&list, "loop-later").stop_reason, Some(StopReason::Cpu));
    assert_eq!(info(&list, "hog").stop_reason, Some(StopReason::Memory));
    assert_eq!(
        info(&list, "thrower").stop_reason,
        Some(StopReason::Errors),
        "three errors in a row"
    );
    assert!(
        info(&list, "thrower")
            .error
            .as_deref()
            .unwrap()
            .contains("boom")
    );
    assert_eq!(info(&list, "good").state, ModState::Running);
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| c.contains("still here"))
            .count(),
        3,
        "the healthy mod kept working"
    );
    assert!(
        fake.calls()
            .contains(&"changed hog Stopped Some(Memory)".to_string())
    );
    assert!(
        fake.calls().contains(&"clear loop-later".to_string()),
        "a stopped mod's status hints are cleared"
    );
    // Re-enabling starts it fresh.
    h.set_enabled("thrower", false).unwrap();
    assert_eq!(
        h.set_enabled("thrower", true).unwrap().state,
        ModState::Running
    );
}

#[test]
fn detectors_see_matching_terminals_only() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "det",
        manifest("det", &["status"]),
        r#"
        pod.detector({
          match: ["aider"],
          onScreen(ctx) {
            const last = ctx.lines.filter((l) => l.trim()).pop() || "";
            if (last.endsWith("> ")) return { state: "idle" };
            if (last.includes("(Y)es")) return { state: "blocked", kind: "permission", msg: last };
            if (last === "bad") return { state: "sleeping" };
            if (last === "forget") return { state: "unknown" };
            return null;
          },
          onTitle(ctx) { return ctx.title === "busy" ? { state: "working" } : null; },
        });
    "#,
    );
    let (h, fake) = start(tmp.path());
    h.set_enabled("det", true).unwrap();
    let aider = vec![
        "C:/tools/Aider.EXE".to_string(),
        "--model".into(),
        "x".into(),
    ];
    let wrapped = vec!["cmd.exe".to_string(), "/c".into(), "aider".into()];
    assert!(h.wants_terminal(&aider) && h.wants_terminal(&wrapped));
    assert!(!h.wants_terminal(&["powershell.exe".to_string()]));
    let sample = |lines: &[&str], title: Option<&str>| {
        h.post_sample(
            "t1",
            Some("repo/a"),
            &aider,
            Some(lines.iter().map(|s| s.to_string()).collect()),
            title.map(Into::into),
        );
        h.list().unwrap();
    };
    sample(&["Aider v0", "> "], None);
    sample(&["Allow edit? (Y)es/(N)o"], None);
    sample(&["thinking"], Some("busy"));
    sample(&["bad"], None);
    sample(&["forget"], None);
    h.post_sample(
        "t9",
        None,
        &["powershell.exe".to_string()],
        Some(vec!["> ".into()]),
        None,
    );
    h.list().unwrap();
    let status: Vec<String> = fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("status"))
        .collect();
    assert_eq!(
        status,
        vec![
            "status det t1 idle",
            "status det t1 blocked:permission Allow edit? (Y)es/(N)o",
            "status det t1 working",
            "status det t1 clear",
        ]
    );
    assert!(h.log("det", 50).unwrap().join("\n").contains("bad value"));
    // Revoking the permission stops the detector and clears its hints.
    h.grant("det", Permission::Status, false).unwrap();
    assert!(!h.wants_terminal(&aider));
    assert!(fake.calls().contains(&"clear det".to_string()));
}

#[test]
fn palette_commands_and_layout_only_mods() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "cmds",
        manifest("cmds", &["commands", "notify"]),
        r#"
        pod.command({ id: "hello", title: "인사", detail: "알림을 띄운다" }, (ctx) => pod.notify("hi", ctx.task || "none"));
        pod.command({ id: "fail", title: "Fail" }, () => { throw new Error("nope"); });
    "#,
    );
    let lay = tmp.path().join("lay");
    std::fs::create_dir_all(lay.join("layouts")).unwrap();
    std::fs::write(
        lay.join("mod.json"),
        r#"{"pod":1,"name":"lay","version":"1","layouts":["layouts/q.json"]}"#,
    )
    .unwrap();
    std::fs::write(lay.join("layouts/q.json"), r#"{"id":"quad","title":"4분할","center":{"split":"v","children":[{"split":"h","children":[{"panel":"terminal","index":0},{"panel":"terminal","index":1}]},{"split":"h","children":[{"panel":"terminal","index":2},{"panel":"terminal","index":3}]}]}}"#).unwrap();
    let (h, fake) = start(tmp.path());
    let m = h.set_enabled("cmds", true).unwrap();
    assert_eq!(
        m.commands.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
        vec!["hello", "fail"]
    );
    h.command("cmds", "hello", scope("repo/a", Some("t1")))
        .unwrap();
    assert!(
        fake.calls()
            .contains(&"notify cmds info | hi | repo/a".to_string())
    );
    assert!(
        h.command("cmds", "fail", Scope::default())
            .unwrap_err()
            .contains("nope")
    );
    assert!(h.command("cmds", "missing", Scope::default()).is_err());
    let l = h.set_enabled("lay", true).unwrap();
    assert_eq!(l.state, ModState::Running);
    assert_eq!(l.layouts[0].id, "quad");
    assert!(
        h.log("lay", 10)
            .unwrap()
            .join("\n")
            .contains("layouts only")
    );
}

#[test]
fn queue_overflow_is_counted_not_blocking() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "slow",
        manifest("slow", &[]),
        r#"pod.on("task.changed", () => { const t = Date.now(); while (Date.now() - t < 5) {} });"#,
    );
    let (h, _fake) = start(tmp.path());
    h.set_enabled("slow", true).unwrap();
    let start = Instant::now();
    for _ in 0..3000 {
        h.post_event("task.changed", Scope::default(), &json!({}));
    }
    assert!(
        start.elapsed() < Duration::from_millis(500),
        "posting never blocks"
    );
    let dropped = until(Duration::from_secs(30), Duration::ZERO, || {
        match h.list().unwrap().dropped_events {
            0 => Err(0),
            n => Ok(n),
        }
    });
    assert!(dropped.is_ok(), "{dropped:?}");
}

// ───────────────────────────── shipped examples ─────────────────────────────

fn examples() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/mods")
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

#[test]
fn example_mods_load_and_work() {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&examples(), tmp.path());
    let (h, fake) = start(tmp.path());
    let list = h.list().unwrap();
    for m in &list.mods {
        assert_ne!(m.state, ModState::Invalid, "{}: {:?}", m.name, m.error);
        h.set_enabled(&m.name, true).unwrap();
    }
    let list = h.list().unwrap();
    for m in &list.mods {
        assert_eq!(m.state, ModState::Running, "{}: {:?}", m.name, m.error);
    }
    assert!(info(&list, "pod-layouts").layouts.len() >= 2);

    // aider-status: Aider-like screens.
    let argv = vec!["aider".to_string(), "--model".into(), "sonnet".into()];
    for screen in [
        vec![
            "Aider v0.0 (fixture)",
            "Repo-map: using 1024 tokens",
            "",
            "> ",
        ],
        vec!["> fix the bug", "Waiting for sonnet", "⠋ Updating repo map"],
        vec!["Add src/main.rs to the chat? (Y)es/(N)o/(D)on't ask again [Yes]: "],
        vec![
            "Applied edit to src/main.rs",
            "Commit 1a2b3c4 fix: off by one",
            "",
            "architect> ",
        ],
    ] {
        h.post_sample(
            "t1",
            Some("repo/a"),
            &argv,
            Some(screen.iter().map(|s| s.to_string()).collect()),
            None,
        );
        h.list().unwrap();
    }
    let status: Vec<String> = fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("status aider-status"))
        .collect();
    assert_eq!(status.len(), 4, "{status:?}");
    assert!(status[0].ends_with("t1 idle"), "{status:?}");
    assert!(status[1].contains("t1 working"), "{status:?}");
    assert!(status[2].contains("t1 blocked:question"), "{status:?}");
    assert!(status[3].contains("t1 done"), "{status:?}");

    // group-done-notify: all members of a group done → one notification.
    *fake.groups.lock().unwrap() = vec![GroupFacts {
        id: "repo/cmp".into(),
        name: "cmp".into(),
        members: vec![
            GroupMemberFacts {
                task: "repo/a".into(),
                state: Some("done".into()),
            },
            GroupMemberFacts {
                task: "repo/b".into(),
                state: Some("working".into()),
            },
        ],
    }];
    let ev = |task: &str, state: &str| {
        deliver(
            &h,
            "status.changed",
            scope(task, Some("t1")),
            json!({"terminal": "t1", "task": task, "state": state, "from": "working"}),
        );
    };
    ev("repo/a", "done");
    assert!(
        !fake
            .calls()
            .iter()
            .any(|c| c.starts_with("notify group-done-notify")),
        "one member still working"
    );
    fake.groups.lock().unwrap()[0].members[1].state = Some("done".into());
    ev("repo/b", "done");
    ev("repo/b", "done");
    let n: Vec<String> = fake
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("notify group-done-notify"))
        .collect();
    assert_eq!(n.len(), 1, "notified once: {n:?}");
    assert!(n[0].contains("success") && n[0].contains("cmp"), "{n:?}");
    // A new round after someone works again.
    fake.groups.lock().unwrap()[0].members[0].state = Some("working".into());
    ev("repo/a", "working");
    fake.groups.lock().unwrap()[0].members[0].state = Some("done".into());
    ev("repo/a", "done");
    assert_eq!(
        fake.calls()
            .iter()
            .filter(|c| c.starts_with("notify group-done-notify"))
            .count(),
        2
    );
    // Its palette command.
    h.command("group-done-notify", "summary", Scope::default())
        .unwrap();
    assert!(
        fake.wait_for(|c| c.iter().any(|x| x.contains("그룹 상태")))
            .iter()
            .any(|x| x.contains("cmp"))
    );
}

// ───────────────────────────── timers ─────────────────────────────

/// `pod.log` lines of a mod with the clock prefix cut off.
fn log_msgs(h: &ModHandle, name: &str) -> Vec<String> {
    h.log(name, 500)
        .unwrap()
        .iter()
        .map(|l| {
            l.split_once(' ')
                .map(|(_, m)| m.to_string())
                .unwrap_or_default()
        })
        .collect()
}

/// [`log_msgs`] once `pred` holds, or as they are after [`READY`].
fn wait_log(h: &ModHandle, name: &str, pred: impl Fn(&[String]) -> bool) -> Vec<String> {
    until(READY, POLL, || {
        let l = log_msgs(h, name);
        if pred(&l) { Ok(l) } else { Err(l) }
    })
    .unwrap_or_else(|t| t.last)
}

/// Number of `msg` lines in a mod's log.
fn count_log(h: &ModHandle, name: &str, msg: &str) -> usize {
    log_msgs(h, name).iter().filter(|m| *m == msg).count()
}

#[test]
fn timers_fire_in_order_with_args_and_the_creating_scope() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tm",
        manifest("tm", &["notify"]),
        r#"
        setTimeout((x) => pod.log("late", x), 60, "L");
        setTimeout(() => pod.log("early-1"), 15);
        setTimeout(() => pod.log("early-2"), 15);
        setTimeout(() => pod.log("zero"), 0);
        let n = 0;
        const iv = setInterval(() => { n++; pod.log("tick", n); if (n === 3) { clearInterval(iv); pod.log("cleared"); } }, 1);
        pod.on("task.changed", (ev) => { setTimeout(() => pod.notify("later", ev.task), 20); });
    "#,
    );
    let (h, fake) = start(tmp.path());
    let t0 = Instant::now();
    h.set_enabled("tm", true).unwrap();
    let log = wait_log(&h, "tm", |l| {
        l.iter().any(|m| m == "cleared") && l.iter().any(|m| m == "late L")
    });
    let order: Vec<&str> = log
        .iter()
        .map(String::as_str)
        .filter(|m| ["zero", "early-1", "early-2", "late L"].contains(m))
        .collect();
    assert_eq!(order, ["zero", "early-1", "early-2", "late L"], "{log:?}");
    let ticks: Vec<&String> = log.iter().filter(|m| m.starts_with("tick")).collect();
    assert_eq!(ticks, ["tick 1", "tick 2", "tick 3"], "{log:?}");
    // setInterval(…, 1) is raised to the 100 ms minimum: three ticks take ≥ 300 ms.
    assert!(
        t0.elapsed() >= Duration::from_millis(290),
        "{:?}",
        t0.elapsed()
    );
    let held = stays(Duration::from_millis(250), POLL, || {
        match log_msgs(&h, "tm")
            .iter()
            .filter(|m| m.starts_with("tick"))
            .count()
        {
            3 => Ok(()),
            n => Err(n),
        }
    });
    assert!(held.is_ok(), "cleared interval stays cleared: {held:?}");

    // A timer created in an event callback keeps that callback's task scope.
    deliver(
        &h,
        "task.changed",
        scope("repo/a", None),
        json!({"task": "repo/a"}),
    );
    let calls = fake.wait_for(|c| c.iter().any(|x| x.contains("| later |")));
    assert!(
        calls.iter().any(|c| c == "notify tm info | later | repo/a"),
        "{calls:?}"
    );
}

#[test]
fn cleared_timers_never_fire() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tc",
        manifest("tc", &[]),
        r#"
        const a = setTimeout(() => pod.log("a fired"), 30);
        const b = setInterval(() => pod.log("b fired"), 100);
        setTimeout(() => pod.log("c fired"), 10);
        clearTimeout(a);
        clearInterval(b);
        clearTimeout(12345); clearTimeout("nonsense"); clearTimeout();
        // Clearing a timer from inside an earlier one.
        const d = setTimeout(() => pod.log("d fired"), 80);
        setTimeout(() => { clearTimeout(d); pod.log("cleared d"); }, 40);
        setTimeout(() => pod.log("done"), 200);
    "#,
    );
    let (h, _fake) = start(tmp.path());
    h.set_enabled("tc", true).unwrap();
    let log = wait_log(&h, "tc", |l| l.iter().any(|m| m == "done"));
    assert!(log.iter().any(|m| m == "c fired"), "{log:?}");
    assert!(log.iter().any(|m| m == "cleared d"), "{log:?}");
    for never in ["a fired", "b fired", "d fired"] {
        assert!(!log.iter().any(|m| m == never), "{never}: {log:?}");
    }
    assert_eq!(info(&h.list().unwrap(), "tc").state, ModState::Running);
}

#[test]
fn timer_limits() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tl",
        manifest("tl", &[]),
        r#"
        let n = 0;
        try { for (;;) { setTimeout(() => {}, 60000); n++; } } catch (e) { pod.log("refused after", n, e.message); }
        try { setTimeout(() => {}, 1e12); } catch (e) { pod.log("too long:", e.message); }
        try { setInterval(42, 100); } catch (e) { pod.log("not a function:", e.message); }
    "#,
    );
    let (h, _fake) = start(tmp.path());
    let m = h.set_enabled("tl", true).unwrap();
    assert_eq!(
        m.state,
        ModState::Running,
        "a refused timer is a catchable error, not a crash"
    );
    let log = log_msgs(&h, "tl").join("\n");
    assert!(
        log.contains("refused after 32 RateLimited: more than 32 pending timers"),
        "{log}"
    );
    assert!(log.contains("too long: InvalidArgument"), "{log}");
    assert!(
        log.contains("not a function: InvalidArgument: setInterval needs a function"),
        "{log}"
    );
}

#[test]
fn looping_timer_callbacks_are_stopped_and_timers_die_with_the_mod() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "tloop",
        manifest("tloop", &["notify"]),
        r#"
        setInterval(() => pod.notify("tick"), 100);
        setTimeout(() => { for (;;) {} }, 250);
    "#,
    );
    write(
        tmp.path(),
        "tthrow",
        manifest("tthrow", &[]),
        r#"setInterval(() => { throw new Error("tick failed"); }, 100);"#,
    );
    write(
        tmp.path(),
        "tok",
        manifest("tok", &[]),
        r#"setInterval(() => pod.log("ok tick"), 100);"#,
    );
    let (h, fake) = start(tmp.path());
    for m in ["tloop", "tthrow", "tok"] {
        h.set_enabled(m, true).unwrap();
    }
    let list = until(READY, Duration::from_millis(20), || {
        let l = h.list().unwrap();
        if info(&l, "tloop").state == ModState::Stopped
            && info(&l, "tthrow").state == ModState::Stopped
        {
            Ok(l)
        } else {
            Err(l)
        }
    })
    .unwrap_or_else(|t| t.last);
    assert_eq!(
        info(&list, "tloop").stop_reason,
        Some(StopReason::Cpu),
        "the CPU limit applies to timer callbacks"
    );
    assert_eq!(
        info(&list, "tthrow").stop_reason,
        Some(StopReason::Errors),
        "three failing ticks in a row"
    );
    let notified = || {
        fake.calls()
            .iter()
            .filter(|c| c.contains("| tick |"))
            .count()
    };
    let ticks = notified();
    assert!(ticks >= 1, "the interval ran before the loop");
    let held = stays(Duration::from_millis(350), POLL, || match notified() {
        n if n == ticks => Ok(()),
        n => Err(n),
    });
    assert!(
        held.is_ok(),
        "a stopped mod's interval no longer fires: {held:?}"
    );
    assert_eq!(
        info(&h.list().unwrap(), "tok").state,
        ModState::Running,
        "other mods' timers keep running"
    );
    assert!(count_log(&h, "tok", "ok tick") >= 3);

    // Disabling clears timers too; re-enabling starts from scratch.
    h.set_enabled("tok", false).unwrap();
    let before = count_log(&h, "tok", "ok tick");
    let held = stays(Duration::from_millis(300), POLL, || {
        match count_log(&h, "tok", "ok tick") {
            n if n == before => Ok(()),
            n => Err(n),
        }
    });
    assert!(held.is_ok(), "disabled mods' timers are gone: {held:?}");
    h.set_enabled("tok", true).unwrap();
    wait_log(&h, "tok", |l| {
        l.iter().filter(|m| *m == "ok tick").count() > before
    });
    // Reload (re-scan) also starts timers fresh rather than doubling them:
    // the whole 550 ms are watched (never more than 7 ticks), then counted.
    h.reload(Some("tok")).unwrap();
    let base = count_log(&h, "tok", "ok tick");
    let held = stays(
        Duration::from_millis(550),
        Duration::from_millis(50),
        || match count_log(&h, "tok", "ok tick") - base {
            n if n <= 7 => Ok(()),
            n => Err(n),
        },
    );
    assert!(held.is_ok(), "one interval after reload, not two: {held:?}");
    let after = count_log(&h, "tok", "ok tick") - base;
    assert!(
        (3..=7).contains(&after),
        "one interval after reload, not two: {after} ticks in 550 ms"
    );
}

/// docs/19 §16.2: orchestration events need the `orch` permission — to
/// register (requested in mod.json) and to receive (granted by the user,
/// never by default); `"*"` listeners do not see them without it.
#[test]
fn orchestration_events_are_permission_checked() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let js = r#"
        pod.on("orch.mail", (ev) => pod.notify("mail", `${ev.type} ${ev.subject} ${ev.attempt ?? "-"}`));
        pod.on("orch.changed", (ev) => {
            pod.notify("changed", `${ev.run} ${ev.attempt ?? "-"} ${ev.state ?? "-"}`);
            if (ev.state === "succeeded") pod.task.runCheck(ev.task, "test");
        });
    "#;
    write(
        root,
        "watch",
        manifest("watch", &["notify", "orch", "check"]),
        js,
    );
    write(
        root,
        "star",
        manifest("star", &["notify"]),
        r#"pod.on("*", (ev, name) => pod.notify("star", name));"#,
    );
    write(
        root,
        "sneaky",
        manifest("sneaky", &["notify"]),
        r#"pod.on("orch.mail", () => {});"#,
    );
    let (h, fake) = start(root);
    // Enabling grants notify but not orch (nor check).
    let m = h.set_enabled("watch", true).unwrap();
    assert_eq!(
        m.state,
        ModState::Running,
        "registering is allowed when mod.json asks for orch: {:?}",
        m.error
    );
    assert!(
        m.permissions
            .iter()
            .any(|p| p.name == Permission::Orch && p.requested && !p.granted)
    );
    h.set_enabled("star", true).unwrap();
    // Registering without asking in mod.json is refused (the load fails).
    let s = h.set_enabled("sneaky", true).unwrap();
    assert_ne!(s.state, ModState::Running);
    assert!(
        s.error.as_deref().unwrap_or_default().contains("orch"),
        "{:?}",
        s.error
    );

    let mail = json!({"run": "r1", "id": "m3", "from": "attempt:a2", "to": "run:r1", "type": "escalation",
        "subject": "worker a2 lost", "text": {"kind": "escalation.lost"}, "attempt": "a2", "task": "repo/b"});
    deliver(&h, "orch.mail", scope("repo/b", None), mail.clone());
    let held = stays(Duration::from_millis(100), POLL, || {
        let calls = fake.calls();
        if calls.iter().any(|c| c.contains("| mail |")) {
            Err(format!("not granted yet: {calls:?}"))
        } else if calls.iter().any(|c| c.contains("| star | orch.")) {
            Err(format!("\"*\" without orch sees nothing: {calls:?}"))
        } else {
            Ok(())
        }
    });
    assert!(held.is_ok(), "{held:?}");

    h.grant("watch", Permission::Orch, true).unwrap();
    h.grant("watch", Permission::Check, true).unwrap();
    deliver(&h, "orch.mail", scope("repo/b", None), mail);
    let calls = fake.wait_for(|c| c.iter().any(|c| c.contains("| mail |")));
    assert!(
        calls
            .iter()
            .any(|c| c == "notify watch info | mail | escalation worker a2 lost a2"),
        "{calls:?}"
    );
    // The event scope is the worker's task: a check there is allowed.
    deliver(
        &h,
        "orch.changed",
        scope("repo/b", None),
        json!({"run": "r1", "item": "i1", "attempt": "a2", "state": "succeeded", "task": "repo/b"}),
    );
    let calls = fake.wait_for(|c| c.iter().any(|c| c.starts_with("check watch")));
    assert!(
        calls
            .iter()
            .any(|c| c == "notify watch info | changed | r1 a2 succeeded"),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|c| c == "check watch repo/b test cargo test"),
        "{calls:?}"
    );
    assert!(
        !calls.iter().any(|c| c.contains("| star | orch.")),
        "{calls:?}"
    );
    // Revoking stops delivery again.
    h.grant("watch", Permission::Orch, false).unwrap();
    let before = fake.calls().len();
    deliver(
        &h,
        "orch.changed",
        scope("repo/b", None),
        json!({"run": "r1"}),
    );
    let held = stays(Duration::from_millis(100), POLL, || {
        let calls = fake.calls();
        if calls[before..].iter().any(|c| c.contains("| changed |")) {
            Err(calls)
        } else {
            Ok(())
        }
    });
    assert!(held.is_ok(), "{held:?}");
}
