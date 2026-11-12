//! The restricted worker endpoint (docs/19 §20, docs/22 TM-20, card O2):
//! real `podd`, `pod-holder` and `pod`; the coordinator and the worker are
//! `fake-agent orch-loop` TUIs that run `pod` with their terminal's env.
//! No real Claude/Codex. The full pipe stays the OS user's (every method);
//! a worker's `pod` reaches `<pipe>-worker`, where the daemon derives the
//! caller from the attempt token and serves an allowlist.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use common::*;
use pod_test_support::git::git_ok;
use pod_test_support::wait;
use podd::client::{Client, ClientError};
use podd::protocol::*;
use serde_json::{Value, json};

fn fake_agent() -> String {
    fixture_bin("examples/fake-agent.exe").display().to_string()
}

struct Env {
    d: Option<Daemon>,
    root: PathBuf,
    base: PathBuf,
    coord: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        drop(self.d.take());
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn appears(p: &Path) {
    wait::until(Duration::from_secs(90), Duration::from_millis(50), || {
        if p.exists() { Ok(()) } else { Err(()) }
    })
    .unwrap_or_else(|e| panic!("{} did not appear ({e})", p.display()));
}

impl Env {
    fn new(tag: &str) -> Env {
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
        let d = Daemon::start_env(tag, &[]);
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
                    argv: vec![fake_agent(), format!("orch-loop:{}", base.display())],
                }),
                ..Default::default()
            })
            .unwrap();
        let e = Env {
            d: Some(d),
            root,
            base,
            coord: t.terminals[0].id.clone(),
        };
        appears(&e.base.join("coord"));
        e.idle(&e.coord.clone());
        e
    }

    fn d(&self) -> &Daemon {
        self.d.as_ref().unwrap()
    }

    fn c(&self) -> Client {
        self.d().client()
    }

    fn idle(&self, t: &str) {
        let w = self
            .c()
            .call::<StatusWait>(&StatusWaitParams {
                target: TerminalRef::id(t),
                status: Some("idle".into()),
                timeout_ms: Some(30_000),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(w.outcome, WaitOutcome::Matched, "{t} never idle");
    }

    /// One `pod` command in `who`'s terminal (its env); its output.
    fn run(&self, who: &str, args: &[&str]) -> Value {
        let dir = self.base.join(who);
        let mut n = 1;
        while dir.join(format!("out-{n}.json")).exists() {
            n += 1;
        }
        let tmp = dir.join("cmd.tmp");
        std::fs::write(&tmp, serde_json::to_string(&[args]).unwrap()).unwrap();
        std::fs::rename(&tmp, dir.join("cmd.json")).unwrap();
        let p = dir.join(format!("out-{n}.json"));
        appears(&p);
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap()
    }

    fn start_worker(&self) -> OrchWorkerStartResult {
        let r = self
            .c()
            .call::<OrchWorkerStart>(&OrchWorkerStartParams {
                spec: Some("say hi".into()),
                agent: Some(AgentSpec::Argv {
                    argv: vec![fake_agent(), format!("orch-loop:{}", self.base.display())],
                }),
                place: Some("new".into()),
                caller: OrchCaller {
                    terminal: Some(self.coord.clone()),
                    ..Default::default()
                },
                ..Default::default()
            })
            .unwrap();
        assert_eq!(r.stage, "started", "{r:?}");
        appears(&self.base.join(&r.attempt.id));
        self.idle(r.terminal.as_deref().unwrap());
        r
    }
}

fn code<T: std::fmt::Debug>(r: Result<T, ClientError>) -> i64 {
    rpc_code(r)
}

/// A raw call on `c`; the error code, or 0.
fn call(c: &mut Client, method: &str, params: Value) -> i64 {
    match c.call_raw(method, params) {
        Ok(_) => 0,
        Err(ClientError::Rpc(e)) => e.code,
        Err(e) => panic!("{method}: {e}"),
    }
}

/// TM-20 completion test: a worker gets the worker endpoint; there the
/// daemon refuses a missing caller, another terminal, another run, gate
/// decisions, merge, input, limits and config, and a caller override; the
/// worker's own `pod` reaches it; the full pipe still serves the user.
#[test]
fn the_worker_endpoint_is_the_workers_own_run_and_terminal() {
    let e = Env::new("wep");
    let w1 = e.start_worker();
    let a1 = w1.attempt.id.clone();
    let t1 = w1.terminal.clone().unwrap();
    let worker_pipe = podd::endpoint::worker_pipe(&e.d().pipe);

    // The worker's environment: the worker endpoint, never the full pipe.
    let env = e.run(&a1, &["!env"]);
    let env: Value = serde_json::from_str(env["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(env["POD_SOCKET"], json!(worker_pipe), "{env}");
    assert_eq!(env["POD_ATTEMPT_ID"], json!(a1));
    assert_eq!(env["POD_TERMINAL_ID"], json!(t1));
    let token = env["POD_ATTEMPT_TOKEN"].as_str().unwrap().to_string();
    // The coordinator keeps the full pipe.
    let cenv = e.run("coord", &["!env"]);
    let cenv: Value = serde_json::from_str(cenv["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(cenv["POD_SOCKET"], json!(e.d().pipe));

    // Its own `pod` reaches the worker endpoint, and its orch commands work.
    let out = e.run(&a1, &["server", "status", "--json"]);
    assert_eq!(out["code"], 0, "{out}");
    let st: Value = serde_json::from_str(out["stdout"].as_str().unwrap()).unwrap();
    assert_eq!(st["endpoint"], "worker", "{st}");
    let out = e.run(&a1, &["orch", "check", "--json"]);
    assert_eq!(out["code"], 0, "{out}");
    let out = e.run(
        &a1,
        &[
            "orch",
            "gate",
            "request",
            "--title",
            "Pick",
            "--message",
            "a or b",
            "--json",
        ],
    );
    assert_eq!(out["code"], 0, "{out}");
    let gate: Value = serde_json::from_str(out["stdout"].as_str().unwrap()).unwrap();
    let gate = gate["id"].as_str().unwrap().to_string();
    // What a worker cannot do through its own `pod`: list or read other
    // terminals, decide its gate, widen itself with --pipe.
    for args in [
        vec!["ls"],
        vec!["read", e.coord.as_str()],
        vec!["gate", "approve", gate.as_str()],
        vec!["orch", "limits", "--set", "max_workers=64"],
    ] {
        let out = e.run(&a1, &args);
        assert_ne!(out["code"], 0, "{args:?}: {out}");
        assert!(
            out["stderr"].as_str().unwrap().contains("worker endpoint"),
            "{args:?}: {out}"
        );
    }
    let full = e.d().pipe.clone();
    let out = e.run(&a1, &["--pipe", full.as_str(), "orch", "check"]);
    assert_ne!(out["code"], 0, "{out}");
    assert!(
        out["stderr"]
            .as_str()
            .unwrap()
            .contains("not a worker endpoint"),
        "{out}"
    );

    // Straight to the endpoint: no hello, a wrong token, then the worker.
    let mut raw = Client::connect_raw(&worker_pipe).unwrap();
    assert_eq!(
        code(raw.call::<ServerStatus>(&Empty {})),
        worker_codes::NO_WORKER
    );
    assert_eq!(
        call(&mut raw, OrchMailCheck::NAME, json!({})),
        worker_codes::NO_WORKER,
        "a missing caller is no worker, never a person"
    );
    assert_eq!(
        code(raw.worker_hello(&a1, &"0".repeat(64))),
        worker_codes::NO_WORKER
    );
    assert_eq!(
        call(
            &mut raw,
            ConnectionHello::NAME,
            json!({"min_protocol": 12, "max_protocol": 21})
        ),
        worker_codes::NO_WORKER
    );
    let mut w = Client::connect_raw(&worker_pipe).unwrap();
    let h = w.worker_hello(&a1, &token).unwrap();
    assert_eq!(h.protocol, PROTOCOL_VERSION);
    let st = w.call::<ServerStatus>(&Empty {}).unwrap();
    assert_eq!(st.endpoint, Some(Endpoint::Worker));

    // Its own terminal and run: allowed.
    let own = TerminalRef::id(&t1);
    w.call::<TerminalRead>(&ReadParams {
        target: own.clone(),
        mode: ReadMode::Screen,
        lines: None,
        raw: false,
    })
    .unwrap();
    let run = w.call::<OrchRunShow>(&Default::default()).unwrap();
    let r1 = run.run.id.clone();
    assert_eq!(r1, w1.attempt.run);
    // Another run, made on the full pipe.
    let r2 = e
        .c()
        .call::<OrchRunCreate>(&OrchRunCreateParams {
            objective: "other".into(),
            ..Default::default()
        })
        .unwrap()
        .run
        .id;
    assert_ne!(r1, r2);

    let refused: Vec<(&str, Value, i64)> = vec![
        // Another terminal (read, wait, status).
        (
            TerminalRead::NAME,
            json!({"id": e.coord}),
            worker_codes::OUT_OF_SCOPE,
        ),
        (
            TerminalShow::NAME,
            json!({"id": e.coord}),
            worker_codes::OUT_OF_SCOPE,
        ),
        (
            StatusWait::NAME,
            json!({"id": e.coord, "timeout_ms": 1}),
            worker_codes::OUT_OF_SCOPE,
        ),
        // Another run, and its coordinator's mailbox in its own run.
        (
            OrchRunShow::NAME,
            json!({"run": r2}),
            worker_codes::OUT_OF_SCOPE,
        ),
        (
            OrchItemList::NAME,
            json!({"run": r2}),
            worker_codes::OUT_OF_SCOPE,
        ),
        (
            OrchMailCheck::NAME,
            json!({"run": r1}),
            worker_codes::OUT_OF_SCOPE,
        ),
        (
            OrchMailList::NAME,
            json!({"run": r1}),
            worker_codes::OUT_OF_SCOPE,
        ),
        (
            OrchMailSend::NAME,
            json!({"to": format!("run:{r2}"), "subject": "x"}),
            worker_codes::OUT_OF_SCOPE,
        ),
        (
            OrchItemCreate::NAME,
            json!({"run": r1, "spec": "more"}),
            worker_codes::OUT_OF_SCOPE,
        ),
        // Stopping itself is its coordinator's call.
        (
            OrchWorkerStop::NAME,
            json!({"attempt": a1}),
            worker_codes::OUT_OF_SCOPE,
        ),
        // A caller field never raises authority.
        (
            OrchMailCheck::NAME,
            json!({"caller": {"terminal": e.coord}}),
            worker_codes::CALLER_MISMATCH,
        ),
        (
            OrchRunList::NAME,
            json!({"caller": {"attempt": "a99", "token": "x"}}),
            worker_codes::CALLER_MISMATCH,
        ),
        // Never on this endpoint.
        (
            OrchGateDecide::NAME,
            json!({"id": gate}),
            worker_codes::NOT_ALLOWED,
        ),
        (
            OrchGateDecide::NAME,
            json!({"id": gate, "person": true}),
            worker_codes::NOT_ALLOWED,
        ),
        (
            GateResolve::NAME,
            json!({"id": gate, "decision": "approve"}),
            worker_codes::NOT_ALLOWED,
        ),
        (
            TaskPrMerge::NAME,
            json!({"task": "demo/coord"}),
            worker_codes::NOT_ALLOWED,
        ),
        (
            TerminalSend::NAME,
            json!({"id": e.coord, "text": "approve"}),
            worker_codes::NOT_ALLOWED,
        ),
        (
            TerminalInput::NAME,
            json!({"id": t1, "data": "eA=="}),
            worker_codes::NOT_ALLOWED,
        ),
        (
            TerminalKeys::NAME,
            json!({"id": e.coord, "keys": ["enter"]}),
            worker_codes::NOT_ALLOWED,
        ),
        (TerminalList::NAME, json!({}), worker_codes::NOT_ALLOWED),
        (
            TerminalRun::NAME,
            json!({"argv": ["cmd"]}),
            worker_codes::NOT_ALLOWED,
        ),
        (
            OrchLimitsSet::NAME,
            json!({"set": {"max_workers": "64"}}),
            worker_codes::NOT_ALLOWED,
        ),
        (OrchLimitsGet::NAME, json!({}), worker_codes::NOT_ALLOWED),
        (OrchConfigSet::NAME, json!({}), worker_codes::NOT_ALLOWED),
        (
            OrchRunCreate::NAME,
            json!({"objective": "x"}),
            worker_codes::NOT_ALLOWED,
        ),
        (ServerStop::NAME, json!({}), worker_codes::NOT_ALLOWED),
        (RepoList::NAME, json!({}), worker_codes::NOT_ALLOWED),
        ("mobile.test.call", json!({}), worker_codes::NOT_ALLOWED),
        ("orch.nope", json!({}), worker_codes::NOT_ALLOWED),
    ];
    for (method, params, want) in &refused {
        assert_eq!(
            call(&mut w, method, params.clone()),
            *want,
            "{method} {params}"
        );
    }
    // A refusal says the same thing whatever was asked.
    let Err(ClientError::Rpc(err)) = w.call_raw(OrchGateDecide::NAME, json!({"id": gate})) else {
        panic!("gate decide passed");
    };
    assert_eq!(err.data, Some(json!({"method": "orch.gate.decide"})));
    assert!(err.message.contains("not available on the worker endpoint"));
    // Streams are refused like any other unlisted method.
    let id = w.call_raw(TerminalAttach::NAME, json!({"id": t1}));
    assert!(
        matches!(id, Err(ClientError::Rpc(ref e)) if e.code == worker_codes::NOT_ALLOWED),
        "{id:?}"
    );

    // No caller: the daemon's caller is the worker, not `human`.
    w.call_raw(OrchMailSend::NAME, json!({"subject": "from the endpoint"}))
        .unwrap();
    let mail = e
        .c()
        .call::<OrchMailList>(&OrchMailListParams { run: r1.clone() })
        .unwrap();
    let m = mail
        .messages
        .iter()
        .find(|m| m.subject == "from the endpoint")
        .expect("mail");
    assert_eq!(m.from, format!("attempt:{a1}"));
    // Runs it may list: its own only.
    let runs = w.call::<OrchRunList>(&Default::default()).unwrap();
    assert_eq!(
        runs.runs
            .iter()
            .map(|r| r.run.id.as_str())
            .collect::<Vec<_>>(),
        [r1.as_str()]
    );

    // The full pipe: the OS user, every method, said so.
    let mut c = e.c();
    let st = c.call::<ServerStatus>(&Empty {}).unwrap();
    assert_eq!(st.endpoint, Some(Endpoint::Full));
    c.call::<TerminalList>(&Empty {}).unwrap();
    // A caller outside Pod's agents is not a person by itself ...
    let r = c.call::<OrchGateDecide>(&OrchGateDecideParams {
        id: gate.clone(),
        choice: None,
        ..Default::default()
    });
    assert_eq!(rpc_code(r), -32110);
    assert_eq!(
        c.call::<GateShow>(&GateRef { id: gate.clone() })
            .unwrap()
            .state,
        GateState::Pending
    );
    // ... the person's path says so (still same-user).
    let g = c
        .call::<OrchGateDecide>(&OrchGateDecideParams {
            id: gate.clone(),
            person: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(g.state, GateState::Approved);
}

/// docs/19 §20: `pod` in a worker's environment never starts a daemon
/// (and a worker endpoint is never started as a daemon's pipe).
#[test]
fn a_worker_env_never_starts_a_daemon() {
    let pipe = format!(r"\\.\pipe\{}", unique("wep-nostart"));
    let worker = podd::endpoint::worker_pipe(&pipe);
    let home = std::env::temp_dir().join(unique("wep-home"));
    std::fs::create_dir_all(&home).unwrap();
    for (socket, attempt) in [(&pipe, Some("a1")), (&worker, None), (&worker, Some("a1"))] {
        let mut cmd = Command::new(pod_exe());
        cmd.args(["orch", "check"])
            .env("POD_SOCKET", socket)
            .env("POD_DAEMON", env!("CARGO_BIN_EXE_podd"))
            .env("POD_HOLDER", env!("CARGO_BIN_EXE_pod-holder"))
            .env("LOCALAPPDATA", &home)
            .env_remove("POD_TERMINAL_ID")
            .env_remove("POD_ATTEMPT_TOKEN");
        match attempt {
            Some(a) => cmd.env("POD_ATTEMPT_ID", a),
            None => cmd.env_remove("POD_ATTEMPT_ID"),
        };
        let out = cmd.output().unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{socket} {attempt:?}: {err}");
        if attempt.is_some() && socket == &pipe {
            // The CLI refuses a full pipe in a worker env before connecting.
            assert!(err.contains("not a worker endpoint"), "{err}");
        } else {
            assert!(err.contains("does not start a daemon"), "{socket}: {err}");
        }
        assert!(
            matches!(Client::connect(&pipe), Err(ClientError::NotRunning)),
            "a daemon started on {pipe}"
        );
        assert!(matches!(
            Client::connect_raw(&worker),
            Err(ClientError::NotRunning)
        ));
    }
    let _ = std::fs::remove_dir_all(&home);
}
