//! Hook trust and provenance (TM-22, docs/22 §4, docs/07 §8.2): a hook is
//! state evidence only. Forged, old, broken, oversized and flooding hook
//! lines are rejected (counted) or dropped; none decides a gate, approves
//! anything, reports work done or wakes a busy or drafting terminal; a
//! terminal's token dies with it; tokens never reach logs, status files or
//! terminal output. Real `podd`, `pod-holder`, `pod`; `fake-agent` stands
//! in for agents (no real Claude/Codex).

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use pod_test_support::git::git_ok;
use pod_test_support::wait;
use podd::client::Client;
use podd::protocol::*;
use serde_json::{Value, json};

// ───────────────────────────── helpers ─────────────────────────────

/// A daemon like [`Daemon::start_env`] whose log (stderr) goes to
/// `<state dir>/podd-test.log`, so a test can read what it logged.
fn start_logged(tag: &str) -> (Daemon, PathBuf) {
    let name = unique(tag);
    let state_dir = std::env::temp_dir().join(&name);
    std::fs::create_dir_all(&state_dir).unwrap();
    let log = state_dir.join("podd-test.log");
    let child = Command::new(env!("CARGO_BIN_EXE_podd"))
        .env(
            "POD_DISCOVER_CLAUDE_DIR",
            state_dir.join("agent-stores").join("claude"),
        )
        .env(
            "POD_DISCOVER_CODEX_DIR",
            state_dir.join("agent-stores").join("codex"),
        )
        .env("POD_DISCOVER_PROCESSES", "off")
        .env("POD_ORCH_CPU_LIMIT", "off")
        .env("POD_ORCH_MIN_FREE_MEM", "off")
        .env("POD_ORCH_MIN_FREE_DISK", "off")
        .env("POD_MODS_DIR", state_dir.join("extensions"))
        .args([
            "--pipe",
            &format!(r"\\.\pipe\{name}"),
            "--holder",
            env!("CARGO_BIN_EXE_pod-holder"),
        ])
        .arg("--state-dir")
        .arg(&state_dir)
        .arg("--worktrees-root")
        .arg(state_dir.join("work trees"))
        .stdin(Stdio::null())
        .stderr(std::fs::File::create(&log).unwrap())
        .spawn()
        .expect("spawn podd");
    let d = Daemon {
        child,
        pipe: format!(r"\\.\pipe\{name}"),
        state_dir,
    };
    wait::until(Duration::from_secs(10), Duration::from_millis(20), || {
        Client::connect(&d.pipe).map(drop)
    })
    .unwrap_or_else(|e| panic!("podd did not start: {e}"));
    (d, log)
}

fn run(c: &mut Client, steps: &[&str]) -> String {
    let mut argv = vec![fake_agent_exe().display().to_string()];
    argv.extend(steps.iter().map(|s| s.to_string()));
    c.call::<TerminalRun>(&RunParams {
        argv,
        cwd: cwd(),
        env: BTreeMap::from([(
            "POD_HOOK_EXE".to_string(),
            pod_hook_exe().display().to_string(),
        )]),
        ..Default::default()
    })
    .expect("run")
    .id
}

fn wait_status(c: &mut Client, id: &str, state: &str) -> StatusInfo {
    let r = c
        .call::<StatusWait>(&StatusWaitParams {
            target: TerminalRef::id(id),
            status: Some(state.into()),
            timeout_ms: Some(30_000),
            ..Default::default()
        })
        .expect("status.wait");
    assert_eq!(r.outcome, WaitOutcome::Matched, "{id}: {:?}", r.status);
    r.status
}

fn token_of(d: &Daemon, id: &str) -> String {
    std::fs::read_to_string(d.state_dir.join(format!("{id}.token"))).expect("token file")
}

/// A hook line as `pod-hook` writes it.
fn hook(id: &str, token: Option<&str>, event: &str, payload: Value) -> String {
    let mut v =
        json!({"v": 1, "terminal_id": id, "agent": "claude", "event": event, "payload": payload});
    if let Some(t) = token {
        v["token"] = json!(t);
    }
    v.to_string()
}

/// Write `lines` on one daemon connection, then `terminal.show` of `id`:
/// lines are taken in order, so the answer shows their effect. Lines that
/// are not JSON get a JSON-RPC error reply; those are skipped.
fn ingest(pipe: &str, lines: &[String], id: &str) -> StatusInfo {
    let mut f = wait::until(Duration::from_secs(10), Duration::from_millis(20), || {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(pipe)
    })
    .expect("open pipe");
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
    let show = json!({"jsonrpc": "2.0", "id": 4242, "method": TerminalShow::NAME, "params": TerminalRef::id(id)});
    writeln!(f, "{show}").unwrap();
    let mut r = std::io::BufReader::new(&f);
    loop {
        let mut line = String::new();
        assert!(r.read_line(&mut line).unwrap() > 0, "no reply");
        let v: Value = serde_json::from_str(&line).unwrap();
        if v["id"] == 4242 {
            let info: TerminalInfo = serde_json::from_value(v["result"].clone()).unwrap();
            return info.status.unwrap();
        }
    }
}

/// Every file under `dir` (recursively) except token files, as bytes.
fn files_without_tokens(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().and_then(|x| x.to_str()) != Some("token")
                && let Ok(b) = std::fs::read(&p)
            {
                out.push((p, b));
            }
        }
    }
    out
}

fn assert_no_secret(where_: &str, files: &[(PathBuf, Vec<u8>)], tokens: &[&str]) {
    for (p, bytes) in files {
        for t in tokens {
            assert!(
                !bytes.windows(t.len()).any(|w| w == t.as_bytes()),
                "{where_}: token found in {}",
                p.display()
            );
        }
    }
}

// ───────────────────────────── status level ─────────────────────────────

/// TM-22 completion test (status level): wrong, missing and old tokens,
/// broken lines, bad shapes and big payloads are rejected or dropped and
/// change no status; an accepted big payload is persisted slim; a flood is
/// limited with counted rejections; an exited terminal's token is revoked
/// (a leftover child cannot bring it back with SessionStart); no token
/// appears in the daemon log, the state files or the terminal output.
#[test]
fn forged_old_broken_and_flooding_hooks_are_limited_and_leak_nothing() {
    let (d, log) = start_logged("hook-trust");
    let mut c = d.client();
    let a = run(&mut c, &["osc:state=working:app=claude-code", "hold"]);
    wait_status(&mut c, &a, "working");
    let ta = token_of(&d, &a);

    // An old token: from a terminal that was removed.
    let b = run(&mut c, &["hold"]);
    let tb = token_of(&d, &b);
    c.call::<TerminalRm>(&RmParams {
        target: TerminalRef::id(&b),
        force: true,
    })
    .unwrap();
    assert!(!d.state_dir.join(format!("{b}.token")).exists());

    let big = "x".repeat(3 * 1024 * 1024);
    let forged = [
        // Rejected, counted: wrong, old, missing token.
        hook(
            &a,
            Some("0123456789abcdef0123456789abcdef"),
            "Stop",
            json!({}),
        ),
        hook(
            &a,
            Some(&tb),
            "PermissionRequest",
            json!({"tool_name": "Bash"}),
        ),
        hook(&a, None, "Stop", json!({})),
        // Rejected, counted: right token, bad shape.
        hook(&a, Some(&ta), "Stop\r\nPermissionRequest", json!({})),
        hook(&a, Some(&ta), "Stop", json!(["not", "an", "object"])),
        hook(
            &a,
            Some(&ta),
            "",
            json!({"hook_event_name": "Stop\u{1b}[2J"}),
        ),
        // Rejected, counted: a big payload with a wrong token.
        hook(
            &a,
            Some(&tb),
            "PermissionRequest",
            json!({"tool_input": big}),
        ),
        // Dropped, not counted: broken JSON, a malformed message that
        // carries the token in another field, the old terminal's id.
        format!(r#"{{"v":1,"terminal_id":"{a}","token":"{ta}","agent":"#),
        json!({"v": ta, "terminal_id": a, "agent": "claude", "event": "Stop"}).to_string(),
        json!({"v": 1, "terminal_id": a, "token": ta, "agent": ta, "event": "Stop"}).to_string(),
        hook(&b, Some(&tb), "Stop", json!({})),
    ];
    let s = ingest(&d.pipe, &forged, &a);
    assert_eq!((s.hooks_accepted, s.hooks_rejected), (0, 7), "{s:?}");
    assert_eq!(
        (s.state.as_str(), s.source.as_deref()),
        ("working", Some("osc"))
    );

    // Accepted: a big payload from the real token is persisted slim.
    let s = ingest(
        &d.pipe,
        &[hook(
            &a,
            Some(&ta),
            "PostToolUse",
            json!({"tool_name": "Bash", "tool_response": big}),
        )],
        &a,
    );
    assert_eq!((s.hooks_accepted, s.hooks_rejected), (1, 7), "{s:?}");
    let persisted = std::fs::read(d.state_dir.join(format!("{a}.status.json"))).unwrap();
    assert!(persisted.len() < 64 * 1024, "{} bytes", persisted.len());

    // A flood from the token holder: at most a burst plus the rate.
    const N: u64 = 400;
    let lines: Vec<String> = (0..N)
        .map(|_| hook(&a, Some(&ta), "PostToolUse", json!({"tool_name": "Bash"})))
        .collect();
    let started = Instant::now();
    ingest(&d.pipe, &lines, &a);
    let secs = started.elapsed().as_secs() + 1;
    // Flood rejections are published by the next ticker sample (250 ms).
    let s = until("every flood line counted", 10, || {
        let s = c
            .call::<TerminalShow>(&TerminalRef::id(&a))
            .unwrap()
            .status
            .unwrap();
        (s.hooks_accepted + s.hooks_rejected == 1 + 7 + N).then_some(s)
    });
    let accepted = s.hooks_accepted - 1;
    let rejected = s.hooks_rejected - 7;
    assert!(
        accepted <= 120 + 60 * secs && rejected >= N.saturating_sub(120 + 60 * secs),
        "accepted {accepted}, rejected {rejected} in {secs} s"
    );
    assert!(rejected > 0, "the flood was limited");
    // Reported (logged) at most 10 a second; the rest only counted.
    let logged = || {
        std::fs::read_to_string(&log)
            .unwrap_or_default()
            .matches("rejected (rate limited)")
            .count() as u64
    };
    assert!(logged() <= 10 * (secs + 1), "{} log lines", logged());

    // A terminal's token dies with it.
    let e = run(
        &mut c,
        &[
            "read",
            r#"hook:claude:UserPromptSubmit:{"hook_event_name":"UserPromptSubmit"}"#,
            "read",
            "exit:0",
        ],
    );
    let te = token_of(&d, &e);
    send(&mut c, &e, "go");
    let s = wait_status(&mut c, &e, "working");
    assert_eq!((s.source.as_deref(), s.hooks_accepted), (Some("hook"), 1));
    send(&mut c, &e, "go");
    wait_status(&mut c, &e, "exited");
    until_true("token revoked at exit", 10, || {
        !d.state_dir.join(format!("{e}.token")).exists()
    });
    let s = ingest(
        &d.pipe,
        &[hook(
            &e,
            Some(&te),
            "SessionStart",
            json!({"session_id": "s-left-over", "source": "startup"}),
        )],
        &e,
    );
    assert_eq!(
        (s.state.as_str(), s.hooks_accepted, s.hooks_rejected),
        ("exited", 1, 1),
        "a leftover child cannot revive an exited terminal: {s:?}"
    );

    // No token in the log, the state files or the terminal output.
    let screen = read(&mut c, &a, ReadMode::Screen).join("\n");
    for t in [&ta, &tb, &te] {
        assert!(!screen.contains(t.as_str()), "token on screen");
    }
    drop(c);
    let files = files_without_tokens(&d.state_dir);
    assert!(
        files.iter().any(|(p, _)| p == &log),
        "the log is among the scanned files"
    );
    assert_no_secret("state dir", &files, &[&ta, &tb, &te]);
}

/// Rediscovery: a token file without a holder (a daemon that died between
/// writing it and listing its terminal) is swept at start; a re-attached
/// running agent keeps its token, because its environment cannot change.
#[test]
fn restart_sweeps_orphan_tokens_and_keeps_live_ones() {
    let (d, _log) = start_logged("hook-sweep");
    let mut c = d.client();
    let a = run(&mut c, &["osc:state=working:app=claude-code", "hold"]);
    wait_status(&mut c, &a, "working");
    let ta = token_of(&d, &a);
    drop(c);
    let (pipe, dir) = d.stop(false);
    let orphan = dir.join("t999.token");
    std::fs::write(&orphan, "0123456789abcdef0123456789abcdef").unwrap();

    let d = Daemon::start_on(pipe, dir);
    assert!(!orphan.exists(), "orphan token swept at start");
    assert_eq!(token_of(&d, &a), ta, "the live terminal keeps its token");
    let s = ingest(
        &d.pipe,
        &[hook(
            &a,
            Some(&ta),
            "Stop",
            json!({"hook_event_name": "Stop"}),
        )],
        &a,
    );
    assert_eq!((s.hooks_accepted, s.state.as_str()), (1, "done"), "{s:?}");
}

// ───────────────────────────── orchestration level ─────────────────────────────

/// A coordinator and workers as `fake-agent orch-loop` TUIs (see
/// tests/orch_runs.rs): they run `pod orch ...` with their terminal's env
/// and log what is typed into them.
struct Orch {
    d: Daemon,
    root: PathBuf,
    base: PathBuf,
    coord: String,
}

impl Drop for Orch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Orch {
    fn new(tag: &str) -> Orch {
        let root = std::env::temp_dir().join(unique(tag));
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_ok(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            git_ok(&repo, &["config", k, v]);
        }
        std::fs::write(repo.join("README.md"), "hello\n").unwrap();
        git_ok(&repo, &["add", "-A"]);
        git_ok(&repo, &["commit", "-q", "-m", "init"]);
        let base = root.join("agents");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("pod-exe.txt"), pod_exe().display().to_string()).unwrap();
        let d = Daemon::start_env(tag, &[("POD_ORCH_RELEASE_AFTER", "off")]);
        let mut c = d.client();
        c.call::<RepoAdd>(&RepoAddParams {
            path: repo.display().to_string(),
            name: Some("demo".into()),
        })
        .unwrap();
        let t = c
            .call::<TaskCreate>(&TaskCreateParams {
                repo: "demo".into(),
                name: "coord".into(),
                agent: Some(AgentSpec::Argv {
                    argv: Self::agent_argv(&base),
                }),
                ..Default::default()
            })
            .unwrap();
        let o = Orch {
            d,
            root,
            base,
            coord: t.terminals[0].id.clone(),
        };
        appear(&o.base.join("coord"));
        wait_status(&mut o.c(), &o.coord, "idle");
        o
    }

    fn agent_argv(base: &Path) -> Vec<String> {
        vec![
            fake_agent_exe().display().to_string(),
            format!("orch-loop:{}", base.display()),
        ]
    }

    fn c(&self) -> Client {
        self.d.client()
    }

    fn dir(&self, who: &str) -> PathBuf {
        self.base.join(who)
    }

    /// Make an agent run `pod` commands in one turn; their outputs.
    fn run(&self, who: &str, cmds: &[&[&str]]) -> Vec<Value> {
        let dir = self.dir(who);
        let mut first = 1;
        while dir.join(format!("out-{first}.json")).exists() {
            first += 1;
        }
        let tmp = dir.join("cmd.tmp");
        std::fs::write(&tmp, serde_json::to_string(cmds).unwrap()).unwrap();
        std::fs::rename(&tmp, dir.join("cmd.json")).unwrap();
        (first..first + cmds.len())
            .map(|n| {
                let p = dir.join(format!("out-{n}.json"));
                wait::until(Duration::from_secs(90), Duration::from_millis(50), || {
                    std::fs::read_to_string(&p)
                        .ok()
                        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                        .ok_or(())
                })
                .unwrap_or_else(|_| panic!("{} did not appear", p.display()))
            })
            .collect()
    }

    fn osc(&self, who: &str, payload: &str) {
        let tmp = self.dir(who).join("osc.tmp");
        std::fs::write(&tmp, payload).unwrap();
        std::fs::rename(&tmp, self.dir(who).join("osc")).unwrap();
    }

    /// What was typed (and submitted) into an agent.
    fn submits(&self, who: &str) -> String {
        std::fs::read_to_string(self.dir(who).join("submits.log")).unwrap_or_default()
    }

    fn wakes(&self, who: &str) -> usize {
        self.submits(who).matches("pod mail: ").count()
    }

    fn mail(&self) -> Vec<Mail> {
        self.c()
            .call::<OrchMailCheck>(&OrchMailCheckParams {
                run: Some("r1".into()),
                ack: false,
                ..Default::default()
            })
            .unwrap()
            .messages
    }

    /// Two more supervisor passes, no wake-up settling.
    fn passes(&self) {
        let mut c = self.c();
        let mut q = || c.call::<OrchQueue>(&OrchRunRef::default()).unwrap();
        let start = q().supervisor_passes;
        wait::until(Duration::from_secs(30), Duration::from_millis(20), || {
            let r = q();
            (r.supervisor_passes >= start + 2 && r.wakes_in_flight == 0)
                .then_some(())
                .ok_or(())
        })
        .expect("supervisor passes");
    }
}

fn appear(p: &Path) {
    wait::until(Duration::from_secs(20), Duration::from_millis(50), || {
        p.exists().then_some(()).ok_or(())
    })
    .unwrap_or_else(|_| panic!("{} did not appear", p.display()));
}

/// TM-22 completion test (orchestration level): with a pending gate, forged
/// PermissionRequest/Stop hooks to the worker, from a wrong token and even
/// from the real one, decide no gate, type nothing into the worker (no
/// approval), and never make a `done` report or a finished attempt; a busy
/// coordinator is not woken by a forged idle, nor a drafting one by a
/// hook-reported `done`.
#[test]
fn forged_hooks_decide_no_gate_approve_nothing_and_wake_no_busy_terminal() {
    let o = Orch::new("hook-trust-orch");
    let w = o
        .c()
        .call::<OrchWorkerStart>(&OrchWorkerStartParams {
            spec: Some("Choose a database".into()),
            agent: Some(AgentSpec::Argv {
                argv: Orch::agent_argv(&o.base),
            }),
            place: Some("new".into()),
            caller: OrchCaller {
                terminal: Some(o.coord.clone()),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    assert_eq!(w.stage, "started", "{w:?}");
    let a1 = w.attempt.id.clone();
    let wt = w.terminal.clone().unwrap();
    appear(&o.dir(&a1));
    wait_status(&mut o.c(), &wt, "idle");
    let out = &o.run(
        &a1,
        &[&[
            "orch",
            "gate",
            "request",
            "--title",
            "DB",
            "--message",
            "Which db?",
            "--option",
            "sqlite",
            "--option",
            "postgres",
            "--json",
        ]],
    )[0];
    assert_eq!(out["code"], 0, "{out}");
    let gates = || {
        o.c()
            .call::<GateList>(&GateListParams::default())
            .unwrap()
            .gates
    };
    let before = gates();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].state, GateState::Pending);
    // The coordinator reads its gate mail, so new mail arms a wake-up again.
    o.passes();
    let out = &o.run("coord", &[&["orch", "check", "--json"]])[0];
    assert_eq!(out["code"], 0, "{out}");
    o.passes();
    let typed_before = o.submits(&a1);

    // Forged from outside (wrong token), then from the token holder itself.
    let tw = std::fs::read_to_string(o.d.state_dir.join(format!("{wt}.token"))).unwrap();
    let wrong = "0123456789abcdef0123456789abcdef";
    let perm = json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash"});
    let stop = json!({"hook_event_name": "Stop"});
    let s = ingest(
        &o.d.pipe,
        &[
            hook(&wt, Some(wrong), "PermissionRequest", perm.clone()),
            hook(&wt, Some(wrong), "Stop", stop.clone()),
        ],
        &wt,
    );
    assert_eq!((s.hooks_rejected, s.state.as_str()), (2, "idle"), "{s:?}");
    let s = ingest(
        &o.d.pipe,
        &[hook(&wt, Some(&tw), "PermissionRequest", perm.clone())],
        &wt,
    );
    assert_eq!(s.badge, "blocked:permission", "hooks are evidence: {s:?}");
    o.passes();
    let s = ingest(
        &o.d.pipe,
        &[hook(&wt, Some(&tw), "Stop", stop.clone())],
        &wt,
    );
    assert_eq!(
        (s.state.as_str(), s.source.as_deref()),
        ("done", Some("hook"))
    );
    o.passes();
    o.passes();

    // No gate transition, nothing typed (no approval), no `done` report.
    assert_eq!(gates(), before, "a hook decided nothing");
    assert_eq!(
        o.submits(&a1),
        typed_before,
        "nothing typed into the worker"
    );
    let mail = o.mail();
    assert!(
        mail.iter().all(|m| m.kind != MailType::Done),
        "a hook is never a done report: {mail:?}"
    );
    let att = o
        .c()
        .call::<OrchWorkerShow>(&OrchAttemptRef {
            attempt: a1.clone(),
            ..Default::default()
        })
        .unwrap()
        .attempt;
    assert!(
        !matches!(att.state, AttemptState::Succeeded | AttemptState::Failed),
        "{att:?}"
    );
    let item = o
        .c()
        .call::<OrchRunShow>(&OrchRunRef {
            run: Some("r1".into()),
            ..Default::default()
        })
        .unwrap()
        .items[0]
        .clone();
    assert_eq!(item.state, ItemState::Blocked, "still waiting on the gate");

    // A busy coordinator with unread mail: a forged (wrong-token) idle does
    // not make it wakeable.
    o.osc("coord", "state=working");
    wait_status(&mut o.c(), &o.coord, "working");
    let wakes = o.wakes("coord");
    let out = &o.run(&a1, &[&["orch", "send", "--subject", "progress", "--json"]])[0];
    assert_eq!(out["code"], 0, "{out}");
    let s = ingest(
        &o.d.pipe,
        &[hook(
            &o.coord,
            Some(wrong),
            "Stop",
            json!({"hook_event_name": "Stop"}),
        )],
        &o.coord,
    );
    assert_eq!((s.state.as_str(), s.hooks_rejected), ("working", 1));
    o.passes();
    o.passes();
    assert_eq!(o.wakes("coord"), wakes, "a busy coordinator was woken");

    // A person's draft in the coordinator: even the real token's `done`
    // does not get a wake-up typed over it.
    let tc = std::fs::read_to_string(o.d.state_dir.join(format!("{}.token", o.coord))).unwrap();
    o.c()
        .call::<TerminalInput>(&InputParams {
            target: TerminalRef::id(&o.coord),
            data: b64_encode(b"hel"),
        })
        .unwrap();
    let s = ingest(
        &o.d.pipe,
        &[hook(
            &o.coord,
            Some(&tc),
            "Stop",
            json!({"hook_event_name": "Stop"}),
        )],
        &o.coord,
    );
    assert_eq!(
        (s.state.as_str(), s.source.as_deref()),
        ("done", Some("hook"))
    );
    o.passes();
    o.passes();
    assert_eq!(o.wakes("coord"), wakes, "typed over a draft");
    // Control: once the person sends the draft, the waiting mail does wake
    // the coordinator (the wake-up was armed all along).
    o.c()
        .call::<TerminalInput>(&InputParams {
            target: TerminalRef::id(&o.coord),
            data: b64_encode(b"\r"),
        })
        .unwrap();
    until_true("woken after the draft", 30, || o.wakes("coord") > wakes);

    // Tokens stay out of every file Pod wrote (state, worktrees, briefs,
    // mail) and the agents' logs.
    let mut files = files_without_tokens(&o.d.state_dir);
    files.extend(files_without_tokens(&o.base));
    assert_no_secret("orchestration", &files, &[&tw, &tc]);
}

/// Start a worker of `o`'s run with `argv` as its agent (a new task).
fn start_argv_worker(o: &Orch, spec: &str, argv: Vec<String>) -> OrchWorkerStartResult {
    let w = o
        .c()
        .call::<OrchWorkerStart>(&OrchWorkerStartParams {
            spec: Some(spec.into()),
            agent: Some(AgentSpec::Argv { argv }),
            place: Some("new".into()),
            caller: OrchCaller {
                terminal: Some(o.coord.clone()),
                ..Default::default()
            },
            ..Default::default()
        })
        .unwrap();
    assert_eq!(w.stage, "started", "{w:?}");
    w
}

fn status_of(o: &Orch, id: &str) -> StatusInfo {
    o.c()
        .call::<TerminalShow>(&TerminalRef::id(id))
        .unwrap()
        .status
        .unwrap()
}

/// docs/19 §21.1 (TM-22 wake rule), both kinds of agent:
/// - with a status source (hooks/OSC; or Claude/Codex by its command
///   line): `unknown` and a stale or contradicted state wait, and its own
///   turn end wakes it;
/// - without one (a program that never reports): `unknown` is woken as
///   before.
///
/// `@idle` follows the same rule.
#[test]
fn wake_follows_the_agents_own_report_and_keeps_the_rule_without_one() {
    let o = Orch::new("hook-trust-wake");
    let w1 = start_argv_worker(&o, "Report", Orch::agent_argv(&o.base));
    let a1 = w1.attempt.id.clone();
    appear(&o.dir(&a1));
    wait_status(&mut o.c(), w1.terminal.as_deref().unwrap(), "idle");
    let send = |subject: &str| {
        let out = &o.run(&a1, &[&["orch", "send", "--subject", subject, "--json"]])[0];
        assert_eq!(out["code"], 0, "{out}");
    };
    let check = || {
        let out = &o.run("coord", &[&["orch", "check", "--json"]])[0];
        assert_eq!(out["code"], 0, "{out}");
    };
    o.passes();
    check();
    o.passes();

    // 1. The coordinator reports through OSC, then takes the report back:
    // `unknown` from an agent with a status source does not wake.
    o.osc("coord", "state=clear");
    wait_status(&mut o.c(), &o.coord, "unknown");
    let wakes = o.wakes("coord");
    send("while unknown");
    o.passes();
    o.passes();
    assert_eq!(o.wakes("coord"), wakes, "woken while unknown");
    // Control: its own idle wakes it (the wake-up was armed all along).
    o.osc("coord", "state=idle");
    until_true("woken on its own idle", 30, || o.wakes("coord") > wakes);
    o.passes();
    check();
    o.passes();

    // 2. A hook Stop, then the agent's own OSC says `working`: the fresh
    // hook still shows `done`, but the agent's newest word is a turn in
    // progress (the same as a hook gone stale under a quiet screen).
    let tc = std::fs::read_to_string(o.d.state_dir.join(format!("{}.token", o.coord))).unwrap();
    let s = ingest(
        &o.d.pipe,
        &[hook(
            &o.coord,
            Some(&tc),
            "Stop",
            json!({"hook_event_name": "Stop"}),
        )],
        &o.coord,
    );
    assert_eq!(
        (s.state.as_str(), s.source.as_deref()),
        ("done", Some("hook"))
    );
    o.osc("coord", "state=working");
    wait_output(&mut o.c(), &o.coord, "osc state=working");
    let s = status_of(&o, &o.coord);
    assert_eq!(
        (s.state.as_str(), s.source.as_deref()),
        ("done", Some("hook")),
        "{s:?}"
    );
    let wakes = o.wakes("coord");
    send("while stale");
    o.passes();
    o.passes();
    assert_eq!(o.wakes("coord"), wakes, "woken on a contradicted done");
    o.osc("coord", "state=idle");
    until_true("woken on its own turn end", 30, || o.wakes("coord") > wakes);
    o.passes();
    check();
    o.passes();

    // 3. Without a status source: a program that never reports (not an
    // agent Pod knows) is `unknown` and is woken as before.
    let silent_log = o.root.join("silent.log");
    let w2 = start_argv_worker(
        &o,
        "Silent",
        vec![
            fake_agent_exe().display().to_string(),
            format!("paste-loop-silent:{}", silent_log.display()),
        ],
    );
    let (a2, t2) = (w2.attempt.id.clone(), w2.terminal.clone().unwrap());
    // 4. Claude by its command line that has not reported (starting up, or
    // no Pod hooks): `unknown` waits.
    let bin = o.root.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude.exe");
    std::fs::copy(fake_agent_exe(), &claude).unwrap();
    let claude_log = o.root.join("claude.log");
    let w3 = start_argv_worker(
        &o,
        "Quiet Claude",
        vec![
            claude.display().to_string(),
            format!("paste-loop-silent:{}", claude_log.display()),
        ],
    );
    let (a3, t3) = (w3.attempt.id.clone(), w3.terminal.clone().unwrap());
    let typed = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
    // Both took the typed brief line and say nothing about themselves.
    until_true("the briefs were typed", 30, || {
        typed(&silent_log).contains("<<<END") && typed(&claude_log).contains("<<<END")
    });
    let (s2, s3) = (status_of(&o, &t2), status_of(&o, &t3));
    assert_eq!(
        (s2.state.as_str(), s2.agent.as_deref()),
        ("unknown", None),
        "{s2:?}"
    );
    assert_eq!(
        (s3.state.as_str(), s3.agent.as_deref()),
        ("unknown", Some("claude")),
        "{s3:?}"
    );
    // `@idle`: the reporting idle worker and the silent one, not the quiet
    // Claude.
    let out = &o.run(
        "coord",
        &[&[
            "orch",
            "send",
            "--to",
            "@idle",
            "--subject",
            "all idle",
            "--json",
        ]],
    )[0];
    assert_eq!(out["code"], 0, "{out}");
    let mut to: Vec<String> = serde_json::from_str::<Value>(out["stdout"].as_str().unwrap())
        .unwrap()["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    to.sort();
    let mut want = vec![format!("attempt:{a1}"), format!("attempt:{a2}")];
    want.sort();
    assert_eq!(to, want, "@idle");
    // Mail to both: the silent program is woken, the quiet Claude is not.
    for a in [&a2, &a3] {
        let to = format!("attempt:{a}");
        let out = &o.run(
            "coord",
            &[&["orch", "send", "--to", &to, "--subject", "hello", "--json"]],
        )[0];
        assert_eq!(out["code"], 0, "{out}");
    }
    until_true("the silent program is woken", 30, || {
        typed(&silent_log).contains("pod mail: ")
    });
    o.passes();
    o.passes();
    assert!(
        !typed(&claude_log).contains("pod mail: "),
        "a Claude that never reported was woken: {}",
        typed(&claude_log)
    );
    assert_eq!(status_of(&o, &t3).state, "unknown");
}
