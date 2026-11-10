//! Agent status end-to-end: the real `podd`, `pod-holder`, `pod-hook` and
//! `pod` binaries, with the `fake-agent` example as a scripted program that
//! writes OSC 7501 reports, window titles and runs hooks like an agent would.
//! No real Claude/Codex runs here.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant};

use podd::client::{Client, StatusStreamEvent};
use podd::protocol::*;

// ───────────────────────────── harness ─────────────────────────────

struct Daemon {
    child: Child,
    pipe: String,
    state_dir: PathBuf,
}

fn unique(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("podd-status-{tag}-{}-{nanos}", std::process::id())
}

impl Daemon {
    fn start(tag: &str) -> Self {
        let name = unique(tag);
        let state_dir = std::env::temp_dir().join(&name);
        Self::start_on(format!(r"\\.\pipe\{name}"), state_dir)
    }

    fn start_on(pipe: String, state_dir: PathBuf) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_podd"))
            .args([
                "--pipe",
                &pipe,
                "--holder",
                env!("CARGO_BIN_EXE_pod-holder"),
            ])
            .arg("--state-dir")
            .arg(&state_dir)
            .stdin(Stdio::null())
            .spawn()
            .expect("spawn podd");
        let d = Self {
            child,
            pipe,
            state_dir,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if Client::connect(&d.pipe).is_ok() {
                return d;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("podd did not start listening on {}", d.pipe);
    }

    fn client(&self) -> Client {
        Client::connect(&self.pipe).expect("connect")
    }

    fn stop_keep_terminals(mut self) -> (String, PathBuf) {
        let _ = self
            .client()
            .call::<ServerStop>(&StopParams { kill: false });
        self.wait_exit();
        let pipe = std::mem::take(&mut self.pipe);
        let dir = std::mem::take(&mut self.state_dir);
        std::mem::forget(self);
        (pipe, dir)
    }

    fn wait_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if let Ok(mut c) = Client::connect(&self.pipe) {
            let _ = c.call::<ServerStop>(&StopParams { kill: true });
        }
        self.wait_exit();
        let _ = std::fs::remove_dir_all(&self.state_dir);
    }
}

/// `target/<profile>` of this test binary (`target/<profile>/deps/status-*.exe`).
fn profile_dir() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    exe.parent().unwrap().parent().unwrap().to_path_buf()
}

/// The other workspace binaries the tests drive, built once per test process
/// in ONE cargo invocation (separate invocations resolve podd's features
/// differently and would re-link binaries that running tests are using).
fn workspace_bin(rel: &str) -> PathBuf {
    static BUILT: Mutex<bool> = Mutex::new(false);
    let mut built = BUILT.lock().unwrap();
    let dir = profile_dir();
    if !*built {
        let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
        cmd.args([
            "build",
            "-p",
            "podd",
            "--example",
            "fake-agent",
            "-p",
            "pod-hook",
            "-p",
            "pod-cli",
            // Target flags apply to every -p: name the binaries too, or
            // only the example is built (stale/missing pod.exe, pod-hook.exe).
            "--bin",
            "pod",
            "--bin",
            "pod-hook",
        ]);
        if dir.file_name().and_then(|n| n.to_str()) == Some("release") {
            cmd.arg("--release");
        }
        let st = cmd.status().expect("run cargo build");
        assert!(st.success(), "cargo build of test helpers failed");
        *built = true;
    }
    let p = dir.join(rel);
    assert!(p.exists(), "{} missing", p.display());
    p
}

fn fake_agent() -> PathBuf {
    workspace_bin("examples/fake-agent.exe")
}

fn pod_hook() -> PathBuf {
    workspace_bin("pod-hook.exe")
}

fn pod_cli() -> PathBuf {
    workspace_bin("pod.exe")
}

fn run(c: &mut Client, argv: Vec<String>, env: BTreeMap<String, String>) -> TerminalInfo {
    c.call::<TerminalRun>(&RunParams {
        argv,
        cwd: Some(std::env::temp_dir().display().to_string()),
        env,
        ..Default::default()
    })
    .expect("run")
}

fn argv(exe: &Path, steps: &[&str]) -> Vec<String> {
    std::iter::once(exe.display().to_string())
        .chain(steps.iter().map(|s| s.to_string()))
        .collect()
}

fn go(c: &mut Client, id: &str) {
    c.call::<TerminalSend>(&SendParams {
        target: TerminalRef::id(id),
        text: "go".into(),
        enter: true,
        confirm: false, // a stepping key: exactly one Enter
    })
    .expect("send");
}

fn wait_status(c: &mut Client, id: &str, status: &str) -> StatusInfo {
    let r = c
        .call::<StatusWait>(&StatusWaitParams {
            target: TerminalRef::id(id),
            status: Some(status.into()),
            turn_end: false,
            timeout_ms: Some(30_000),
        })
        .expect("status.wait");
    assert_eq!(
        r.outcome,
        WaitOutcome::Matched,
        "{id} never became {status}: {:?}",
        r.status
    );
    r.status
}

fn show(c: &mut Client, id: &str) -> StatusInfo {
    c.call::<TerminalShow>(&TerminalRef::id(id))
        .expect("show")
        .status
        .expect("status present")
}

/// Poll `terminal.show` until `pred` holds.
fn show_until(c: &mut Client, id: &str, pred: impl Fn(&StatusInfo) -> bool) -> StatusInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let s = show(c, id);
        if pred(&s) {
            return s;
        }
        assert!(Instant::now() < deadline, "condition not reached: {s:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run the real pod-hook from the test process (not from inside the terminal).
fn external_hook(pipe: &str, id: &str, token: Option<&str>, event: &str, json: &str) {
    let mut cmd = Command::new(pod_hook());
    cmd.args(["claude", event])
        .env("POD_TERMINAL_ID", id)
        .env("POD_SOCKET", pipe)
        .env_remove("POD_HOOK_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    if let Some(t) = token {
        cmd.env("POD_HOOK_TOKEN", t);
    }
    let mut child = cmd.spawn().expect("spawn pod-hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(json.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "{}");
}

// ───────────────────────────── tests ─────────────────────────────

/// OSC 7501 working -> blocked(question) -> done drives `terminal.show` and
/// the `status.subscribe` stream in order; `terminal.ack` turns done into idle.
#[test]
fn osc_reports_drive_show_and_subscribe() {
    let d = Daemon::start("osc");
    let (snap, mut stream) = d
        .client()
        .subscribe_status(&StatusSubscribeParams::default())
        .expect("subscribe");
    assert!(snap.terminals.is_empty());
    assert!(snap.epoch > 0);
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(Some(ev)) = stream.next_event() {
            if tx.send(ev).is_err() {
                break;
            }
        }
    });

    let mut c = d.client();
    // base64("Proceed?") = UHJvY2VlZD8=
    let t = run(
        &mut c,
        argv(
            &fake_agent(),
            &[
                "osc:state=working:app=fake",
                "read",
                "osc:state=blocked:app=fake:kind=question:msg=UHJvY2VlZD8=",
                "read",
                "osc:state=done:app=fake",
                "hold",
            ],
        ),
        BTreeMap::new(),
    );
    let id = t.id.clone();

    let s = wait_status(&mut c, &id, "working");
    assert_eq!(s.source.as_deref(), Some("osc"));
    assert_eq!(s.agent.as_deref(), Some("fake"));
    assert_eq!(show(&mut c, &id).state, "working");

    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "blocked");
    assert_eq!(s.kind.as_deref(), Some("question"));
    assert_eq!(s.msg.as_deref(), Some("Proceed?"));
    assert_eq!(s.badge, "blocked:question");
    assert_eq!(s.turn, 1, "leaving working advances the turn");

    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "done");
    assert_eq!(s.source.as_deref(), Some("osc"));
    // terminal.list carries the same status.
    let list = c.call::<TerminalList>(&Empty {}).unwrap();
    assert_eq!(list.terminals[0].status.as_ref().unwrap().state, "done");

    let acked = c.call::<TerminalAck>(&TerminalRef::id(&id)).unwrap();
    assert_eq!(acked.status.unwrap().state, "idle");

    // The stream saw exactly this sequence, with increasing seq numbers.
    let mut labels: Vec<String> = Vec::new();
    let mut last_seq = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    while labels.last().map(String::as_str) != Some("idle") {
        let left = deadline.saturating_duration_since(Instant::now());
        let ev = rx.recv_timeout(left).expect("status event");
        let StatusStreamEvent::Changed(ev) = ev else {
            panic!("lagged")
        };
        assert!(ev.seq > last_seq, "seq not increasing");
        last_seq = ev.seq;
        if ev.id != id {
            continue;
        }
        let l = ev.status.state_label();
        if labels.last() != Some(&l) {
            labels.push(l);
        }
    }
    // The creation event is `unknown`, or already `working` when the report
    // was written before the daemon connected (replayed from the snapshot).
    if labels.first().map(String::as_str) == Some("unknown") {
        labels.remove(0);
    }
    assert_eq!(labels, ["working", "blocked:question", "done", "idle"]);
}

/// docs/07 §8.5: the turn counter and the last hook evidence (here a
/// permission prompt reported by a hook) survive a daemon restart; `rm`
/// deletes the persisted status.
#[test]
fn turn_and_hook_evidence_survive_daemon_restart() {
    let d = Daemon::start("restart-persist");
    let mut c = d.client();
    let env = BTreeMap::from([("POD_HOOK_EXE".to_string(), pod_hook().display().to_string())]);
    let t = run(
        &mut c,
        argv(
            &fake_agent(),
            &[
                "osc:state=working:app=fake",
                "read",
                "osc:state=done:app=fake",
                "read",
                r#"hook:claude:PermissionRequest:{"session_id":"s-restart","hook_event_name":"PermissionRequest","tool_name":"Bash"}"#,
                "hold",
            ],
        ),
        env,
    );
    let id = t.id.clone();
    wait_status(&mut c, &id, "working");
    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "done");
    assert_eq!(s.turn, 1);
    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "blocked");
    assert_eq!(
        (s.kind.as_deref(), s.source.as_deref()),
        (Some("permission"), Some("hook"))
    );
    let persisted = d.state_dir.join(format!("{id}.status.json"));
    assert!(persisted.exists(), "status persisted");
    drop(c);

    let (pipe, dir) = d.stop_keep_terminals();
    let d = Daemon::start_on(pipe, dir);
    let mut c = d.client();
    let s = show(&mut c, &id);
    assert_eq!(s.turn, 1, "turn counter restored");
    assert_eq!(
        (s.state.as_str(), s.kind.as_deref(), s.source.as_deref()),
        ("blocked", Some("permission"), Some("hook")),
        "hook evidence restored over the replayed OSC done: {s:?}"
    );
    assert_eq!(s.msg.as_deref(), Some("Bash"));

    c.call::<TerminalRm>(&RmParams {
        target: TerminalRef::id(&id),
        force: true,
    })
    .unwrap();
    assert!(!persisted.exists(), "rm deletes the persisted status");
}

/// A Codex-like program (an exe named codex.exe) with a Braille spinner in its
/// title is `working` by heuristic; when the spinner goes and output stops it
/// is `done`; a Codex approval prompt on screen is `blocked`.
#[test]
fn codex_title_spinner_heuristic() {
    let d = Daemon::start("codex");
    let dir = std::env::temp_dir().join(unique("codex-bin"));
    std::fs::create_dir_all(&dir).unwrap();
    let codex = dir.join("codex.exe");
    std::fs::copy(fake_agent(), &codex).unwrap();

    let mut c = d.client();
    let t = run(
        &mut c,
        argv(
            &codex,
            &[
                "title:\u{280b} Reply ok | work",
                "print:thinking...",
                "read",
                "title:Reply ok | work",
                "read",
                "print:Would you like to run the following command?",
                "hold",
            ],
        ),
        BTreeMap::new(),
    );
    let id = t.id.clone();
    let s = show(&mut c, &id);
    assert_eq!(s.agent.as_deref(), Some("codex"), "agent from argv");

    let s = wait_status(&mut c, &id, "working");
    assert_eq!(s.source.as_deref(), Some("heuristic"));

    go(&mut c, &id);
    // No spinner + 3 s without output -> done (a working phase was seen).
    let s = wait_status(&mut c, &id, "done");
    assert_eq!(s.source.as_deref(), Some("heuristic"));
    assert_eq!(s.turn, 1);

    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "blocked");
    assert_eq!(s.kind.as_deref(), Some("permission"));
    assert_eq!(s.source.as_deref(), Some("heuristic"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// A hook sent by the real pod-hook binary from inside the terminal (token from
/// the terminal env) changes the status and outranks OSC; forged messages
/// (wrong or missing token) are rejected. A hook `blocked` ends as soon as the
/// agent's OSC report changes to the end of a turn (Esc on the prompt sends
/// no hook, docs/07 §3.1).
#[test]
fn hooks_authenticated_and_outrank_osc() {
    let d = Daemon::start("hook");
    let mut c = d.client();
    let env = BTreeMap::from([("POD_HOOK_EXE".to_string(), pod_hook().display().to_string())]);
    // While the hook says blocked, the fixture is stepped with files, not
    // keys: an Enter typed at a permission prompt answers it (docs/07 §3.1-6).
    let flags = std::env::temp_dir().join(unique("hook-flags"));
    std::fs::create_dir_all(&flags).unwrap();
    let (step1, step2) = (flags.join("1"), flags.join("2"));
    let wait1 = format!("wait-file:{}", step1.display());
    let wait2 = format!("wait-file:{}", step2.display());
    let t = run(
        &mut c,
        argv(
            &fake_agent(),
            &[
                "osc:state=working:app=claude-code",
                "read",
                r#"hook:claude:PermissionRequest:{"hook_event_name":"PermissionRequest","tool_name":"Bash"}"#,
                &wait1,
                "osc:state=working:app=claude-code",
                "print:osc-working-sent",
                &wait2,
                "osc:state=idle:app=claude-code",
                "print:osc-idle-sent",
                "read",
                r#"hook:claude:Stop:{"hook_event_name":"Stop"}"#,
                "hold",
            ],
        ),
        env,
    );
    let id = t.id.clone();
    let s = wait_status(&mut c, &id, "working");
    assert_eq!(s.source.as_deref(), Some("osc"));
    assert_eq!(s.agent.as_deref(), Some("claude"));

    // Forged: right terminal id, wrong token / no token.
    let forged = r#"{"hook_event_name":"PermissionRequest","tool_name":"Evil"}"#;
    external_hook(
        &d.pipe,
        &id,
        Some("0123456789abcdef0123456789abcdef"),
        "PermissionRequest",
        forged,
    );
    external_hook(&d.pipe, &id, None, "PermissionRequest", forged);
    let s = show_until(&mut c, &id, |s| s.hooks_rejected == 2);
    assert_eq!(s.state, "working", "forged hooks must not change status");
    assert_eq!(s.hooks_accepted, 0);

    // Real hook from inside the terminal.
    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "blocked");
    assert_eq!(s.kind.as_deref(), Some("permission"));
    assert_eq!(s.source.as_deref(), Some("hook"));
    assert_eq!(s.msg.as_deref(), Some("Bash"));
    assert_eq!(s.hooks_accepted, 1);

    // A fresh (lagging) OSC `working` does not override the hook evidence.
    std::fs::write(&step1, "").unwrap();
    let w = c
        .call::<TerminalWait>(&WaitParams {
            target: TerminalRef::id(&id),
            until: WaitUntil::OutputContains("osc-working-sent".into()),
            timeout_ms: Some(20_000),
        })
        .unwrap();
    assert_eq!(w.outcome, WaitOutcome::Matched);
    std::thread::sleep(Duration::from_millis(300));
    let s = show(&mut c, &id);
    assert_eq!(s.state_label(), "blocked:permission");
    assert_eq!(s.source.as_deref(), Some("hook"));

    // The OSC report turning idle (user pressed Esc, no hook) clears it at once.
    std::fs::write(&step2, "").unwrap();
    let s = wait_status(&mut c, &id, "idle");
    assert_eq!(s.source.as_deref(), Some("osc"));

    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "done");
    assert_eq!(s.source.as_deref(), Some("hook"));
    assert_eq!(s.outcome.as_deref(), Some("completed"));
    assert_eq!(s.hooks_accepted, 2);
    assert_eq!(s.hooks_rejected, 2);
    let _ = std::fs::remove_dir_all(&flags);
}

/// The CLI: `pod wait --status done` and `--turn-end` return when done
/// arrives; `pod status --json`, `pod ls` STATUS column; hooks dry run.
#[test]
fn pod_cli_wait_status_and_ls() {
    let d = Daemon::start("cli");
    let pod = pod_cli();
    let mut c = d.client();
    let t = run(
        &mut c,
        argv(
            &fake_agent(),
            &["osc:state=working", "read", "osc:state=done", "hold"],
        ),
        BTreeMap::new(),
    );
    let id = t.id.clone();
    wait_status(&mut c, &id, "working");

    let spawn_wait = |extra: &[&str]| {
        Command::new(&pod)
            .args(["--pipe", &d.pipe, "wait", &id, "--timeout", "30"])
            .args(extra)
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn pod wait")
    };
    let mut w_done = spawn_wait(&["--status", "done"]);
    let mut w_turn = spawn_wait(&["--turn-end"]);
    std::thread::sleep(Duration::from_millis(700));
    assert!(w_done.try_wait().unwrap().is_none(), "returned too early");
    assert!(w_turn.try_wait().unwrap().is_none(), "returned too early");
    go(&mut c, &id);
    for (w, what) in [(w_done, "--status done"), (w_turn, "--turn-end")] {
        let out = w.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "pod wait {what}: {text}");
        assert!(text.contains("done"), "pod wait {what}: {text}");
    }

    let out = Command::new(&pod)
        .args(["--pipe", &d.pipe, "status", &id, "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let st: StatusInfo = serde_json::from_slice(&out.stdout).expect("status json");
    assert_eq!(st.state, "done");
    assert_eq!(st.source.as_deref(), Some("osc"));

    let out = Command::new(&pod)
        .args(["--pipe", &d.pipe, "ls"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().next().unwrap().contains("STATUS"), "{text}");
    assert!(text.contains("done (osc)"), "{text}");

    // Timeout -> exit code 1.
    let out = Command::new(&pod)
        .args([
            "--pipe",
            &d.pipe,
            "wait",
            &id,
            "--status",
            "error",
            "--timeout",
            "0.3",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));

    // Hooks dry run: prints the plan, writes nothing; real install refused.
    let tmp = std::env::temp_dir().join(unique("hooks"));
    let (cl, cx) = (tmp.join("claude"), tmp.join("codex"));
    let out = Command::new(&pod)
        .args([
            "hooks",
            "install",
            "--dry-run",
            "--scope",
            "profile",
            "--json",
        ])
        .arg("--claude-config-dir")
        .arg(&cl)
        .arg("--codex-home")
        .arg(&cx)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(plan["files"].as_array().unwrap().len(), 2);
    assert!(
        plan["files"][0]["content"]
            .as_str()
            .unwrap()
            .contains("pod-hook")
    );
    assert!(!tmp.exists(), "dry run must not create anything");
    // A real install, into the temporary profile dirs only.
    let out = Command::new(&pod)
        .args(["hooks", "install", "--scope", "profile", "--no-codex-trust"])
        .arg("--claude-config-dir")
        .arg(&cl)
        .arg("--codex-home")
        .arg(&cx)
        .arg("--bin-dir")
        .arg(tmp.join("bin"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(cl.join("settings.json").exists() && cx.join("hooks.json").exists());
    assert!(tmp.join("bin").join("pod-hook.exe").exists());
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Daemon restart: the holder snapshot replays the last OSC 7501 report, so
/// a re-attached terminal comes back `working` (re-derived). Hook tokens
/// survive (token file in the state dir). The turn counter and the last hook
/// are restored too (see `turn_and_hook_evidence_survive_daemon_restart`).
#[test]
fn status_rederived_after_daemon_restart() {
    let d = Daemon::start("restart");
    let mut c = d.client();
    let env = BTreeMap::from([("POD_HOOK_EXE".to_string(), pod_hook().display().to_string())]);
    let t = run(
        &mut c,
        argv(
            &fake_agent(),
            &[
                "osc:state=working:app=fake",
                "read",
                "osc:state=done:app=fake",
                "read",
                r#"hook:claude:UserPromptSubmit:{"hook_event_name":"UserPromptSubmit"}"#,
                "hold",
            ],
        ),
        env,
    );
    let id = t.id.clone();
    wait_status(&mut c, &id, "working");
    drop(c);

    let (pipe, dir) = d.stop_keep_terminals();
    let d = Daemon::start_on(pipe, dir);
    let mut c = d.client();
    let s = show(&mut c, &id);
    assert_eq!(s.state, "working", "re-derived from the replayed report");
    assert_eq!(s.source.as_deref(), Some("osc"));
    assert_eq!(s.turn, 0, "never left working: still turn 0");

    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "done");
    assert_eq!(s.source.as_deref(), Some("osc"));

    go(&mut c, &id);
    let s = wait_status(&mut c, &id, "working");
    assert_eq!(s.source.as_deref(), Some("hook"));
    assert_eq!(s.hooks_accepted, 1, "token still valid after restart");

    // rm sends a final `removed` event and deletes the token.
    c.call::<TerminalRm>(&RmParams {
        target: TerminalRef::id(&id),
        force: true,
    })
    .unwrap();
    assert!(!d.state_dir.join(format!("{id}.token")).exists());
}
