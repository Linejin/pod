//! Mobile approval M1 (docs/17 §6) end to end: real `podd`, `pod-holder`
//! and `pod`; the phone side is the debug-only test hook `mobile.test.*`
//! (pairing stub + calls through the mobile API router). No network
//! listener exists. Agents are the `fake-agent` example, never a real
//! Claude/Codex.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use std::time::Duration;

use common::*;
use podd::client::{Client, ClientError};
use podd::protocol::*;
use serde_json::{Value, json};

// ───────────────────────────── helpers ─────────────────────────────

const HOOKS: &[(&str, &str)] = &[("POD_MOBILE_TEST_HOOKS", "1")];
const HEADER: &str = "Message from the user, sent from their phone through Pod";

fn fake_agent() -> String {
    fixture_bin("examples/fake-agent.exe").display().to_string()
}

fn pod(pipe: &str, args: &[&str]) -> std::process::Output {
    Command::new(fixture_bin("pod.exe"))
        .arg("--pipe")
        .arg(pipe)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .expect("run pod")
}

fn pod_ok(pipe: &str, args: &[&str]) -> String {
    let o = pod(pipe, args);
    assert!(
        o.status.success(),
        "pod {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

struct TempRepo {
    root: PathBuf,
    path: PathBuf,
}

impl TempRepo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(unique("mobile-repo"));
        let path = root.join("pod repo");
        std::fs::create_dir_all(&path).unwrap();
        git(&path, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Pod Test"),
            ("user.email", "pod-test@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
        ] {
            git(&path, &["config", k, v]);
        }
        std::fs::write(path.join("README.md"), "hello\n").unwrap();
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "-m", "init"]);
        Self { root, path }
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("run git");
    assert!(out.status.success(), "git {args:?}");
}

fn setup(c: &mut Client, repo: &TempRepo) {
    c.call::<RepoAdd>(&RepoAddParams {
        path: repo.path.display().to_string(),
        name: Some("demo".into()),
    })
    .expect("repo.add");
}

fn task(c: &mut Client, name: &str, argv: Vec<String>) -> TaskInfo {
    c.call::<TaskCreate>(&TaskCreateParams {
        repo: "demo".into(),
        name: name.into(),
        agent: Some(AgentSpec::Argv { argv }),
        ..Default::default()
    })
    .expect("task.create")
}

fn wait_state(c: &mut Client, id: &str, state: &str) {
    let r = c
        .call::<StatusWait>(&StatusWaitParams {
            target: TerminalRef::parse(id),
            status: Some(state.into()),
            turn_end: false,
            timeout_ms: Some(30_000),
        })
        .expect("status.wait");
    assert_eq!(r.outcome, WaitOutcome::Matched, "{id} never became {state}");
}

fn pair(c: &mut Client, name: &str) -> String {
    let d = c
        .call_raw("mobile.test.pair", json!({ "name": name }))
        .expect("pair stub");
    d["id"].as_str().unwrap().to_string()
}

fn session(c: &mut Client, device: &str) -> String {
    let s = c
        .call_raw("mobile.test.session", json!({ "device": device }))
        .expect("session");
    s["session"].as_str().unwrap().to_string()
}

/// One request through the mobile API, as a phone channel would make it.
fn phone(c: &mut Client, session: &str, method: &str, params: Value) -> Result<Value, ClientError> {
    c.call_raw(
        "mobile.test.call",
        json!({ "session": session, "method": method, "params": params }),
    )
}

fn code(r: Result<Value, ClientError>) -> i64 {
    rpc_code(r)
}

fn message(
    c: &mut Client,
    s: &str,
    task: &str,
    text: &str,
    op: &str,
) -> Result<Value, ClientError> {
    phone(
        c,
        s,
        mobile_api::MESSAGE,
        json!({ "task": task, "text": text, "operation_id": op }),
    )
}

fn log_text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

fn audit(pipe: &str) -> Vec<MobileAuditEntry> {
    let out = pod_ok(pipe, &["mobile", "audit", "--limit", "1000", "--json"]);
    serde_json::from_str::<MobileAuditResult>(&out)
        .unwrap()
        .entries
}

// ───────────────────────────── messages ─────────────────────────────

/// Short messages go through the review delivery path: typed in once
/// (bracketed paste + Enter + submit confirmation) when the agent is idle,
/// queued while it works, refused while it is blocked; validated, rate
/// limited, per-task permission, idempotent, audited without the text.
#[test]
fn mobile_messages_use_the_review_delivery_path() {
    let repo = TempRepo::new();
    let d = Daemon::start_env("mobile-msg", HOOKS);
    let pipe = d.pipe.clone();
    let mut c = d.client();
    setup(&mut c, &repo);
    let logs = d.state_dir.join("agent logs");
    std::fs::create_dir_all(&logs).unwrap();
    let idle_log = logs.join("idle.log");
    let busy_log = logs.join("busy.log");
    let signal = logs.join("go");

    let idle = task(
        &mut c,
        "idle",
        vec![fake_agent(), format!("paste-loop:{}", idle_log.display())],
    );
    let busy = task(
        &mut c,
        "busy",
        vec![
            fake_agent(),
            "osc:state=working".into(),
            format!("wait-file:{}", signal.display()),
            format!("paste-loop:{}", busy_log.display()),
        ],
    );
    let blk = task(
        &mut c,
        "blk",
        vec![
            fake_agent(),
            "osc:state=blocked:kind=question:msg=UHJvY2VlZD8=".into(),
            "hold".into(),
        ],
    );
    wait_state(&mut c, &idle.terminals[0].id, "idle");
    wait_state(&mut c, &busy.terminals[0].id, "working");
    wait_state(&mut c, &blk.terminals[0].id, "blocked");

    // ── pairing stub + policy through the CLI ──
    let m1 = pair(&mut c, "Pixel");
    assert_eq!(m1, "m1");
    for t in ["demo/idle", "demo/busy", "demo/blk"] {
        pod_ok(
            &pipe,
            &["mobile", "policy", &m1, t, "--allow", "status,message"],
        );
    }
    let devices = pod_ok(&pipe, &["mobile", "devices"]);
    assert!(devices.contains("network listener: off"), "{devices}");
    assert!(devices.contains("demo/idle: status,message"), "{devices}");
    let s1 = session(&mut c, &m1);

    // ── the router refuses everything but the mobile API ──
    for m in [
        "terminal.send",
        "terminal.keys",
        "review.send",
        "gate.resolve",
        "mobile.policy",
        "server.stop",
    ] {
        assert_eq!(
            code(phone(&mut c, &s1, m, json!({}))),
            mobile_codes::MOBILE_DENIED,
            "{m}"
        );
    }
    let tasks = phone(&mut c, &s1, mobile_api::TASKS, json!({})).unwrap();
    let ids: Vec<&str> = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["demo/blk", "demo/busy", "demo/idle"]);
    let shown = serde_json::to_string(&tasks).unwrap();
    for secret in ["paste-loop", "fake-agent", "worktree", "work trees"] {
        assert!(
            !shown.contains(secret),
            "{secret} leaked to the phone: {shown}"
        );
    }

    // ── idle agent: typed in now, submit confirmed ──
    let text = "Please rerun the tests — 한글도 OK";
    let r = message(&mut c, &s1, "demo/idle", text, "op-1").unwrap();
    assert_eq!(r["state"], "delivered", "{r}");
    assert_eq!(r["replayed"], false);
    let d1 = r["delivery"].as_str().unwrap().to_string();
    until("the agent got the message", 20, || {
        let l = log_text(&idle_log);
        (l.contains(HEADER) && l.contains(text)).then_some(())
    });
    let submit = until("submit confirmation", 20, || {
        std::thread::sleep(Duration::from_millis(400)); // reads are rate limited too
        let v = phone(&mut c, &s1, mobile_api::DELIVERY, json!({ "delivery": d1 })).unwrap();
        (v["submit"]["state"] != "pending" && !v["submit"].is_null()).then_some(v)
    });
    assert_eq!(submit["submit"]["state"], "confirmed", "{submit}");
    // Retry of the same operation: its result, nothing typed again.
    let again = message(&mut c, &s1, "demo/idle", text, "op-1").unwrap();
    assert_eq!(again["delivery"], d1.as_str());
    assert_eq!(again["replayed"], true);
    let conflict = message(&mut c, &s1, "demo/idle", "something else", "op-1");
    assert_eq!(code(conflict), mobile_codes::OPERATION_CONFLICT);
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        log_text(&idle_log).matches(HEADER).count(),
        1,
        "typed exactly once"
    );
    // Same record as review deliveries; the PC sees where it came from only by id.
    let listed = c
        .call::<ReviewDeliveries>(&ReviewDeliveriesParams::default())
        .unwrap();
    let rec = listed.deliveries.iter().find(|x| x.id == d1).unwrap();
    assert!(rec.comment_ids.is_empty());
    assert!(rec.prompt.starts_with(HEADER) && rec.prompt.ends_with(text));

    // ── working agent: queued, delivered when it is idle ──
    let r = message(
        &mut c,
        &s1,
        "demo/busy",
        "after you finish, summarize",
        "op-2",
    )
    .unwrap();
    assert_eq!(r["state"], "queued", "{r}");
    std::thread::sleep(Duration::from_millis(1000));
    assert!(!log_text(&busy_log).contains("summarize"));
    std::fs::write(&signal, b"go").unwrap();
    until("queued message delivered", 30, || {
        log_text(&busy_log)
            .contains("after you finish, summarize")
            .then_some(())
    });

    // ── blocked agent: refused (docs/11), nothing recorded ──
    let before = c
        .call::<ReviewDeliveries>(&ReviewDeliveriesParams::default())
        .unwrap()
        .deliveries
        .len();
    let r = message(&mut c, &s1, "demo/blk", "yes", "op-3");
    assert_eq!(code(r), orch_codes::AGENT_BLOCKED);
    let after = c
        .call::<ReviewDeliveries>(&ReviewDeliveriesParams::default())
        .unwrap()
        .deliveries
        .len();
    assert_eq!(before, after);

    // ── rate limit: 5 message attempts per minute per device ──
    let e = message(&mut c, &s1, "demo/idle", "one more", "op-4");
    match e {
        Err(ClientError::Rpc(e)) => {
            assert_eq!(e.code, mobile_codes::RATE_LIMITED);
            assert!(e.data.unwrap()["retry_after_ms"].as_u64().unwrap() > 0);
        }
        other => panic!("expected RATE_LIMITED, got {other:?}"),
    }
    // Reads have their own budget.
    phone(&mut c, &s1, mobile_api::TASKS, json!({})).unwrap();

    // ── a second device: validation, permission, task gone, review comments ──
    let m2 = pair(&mut c, "Tablet");
    pod_ok(
        &pipe,
        &[
            "mobile",
            "policy",
            &m2,
            "demo/idle",
            "--allow",
            "status,message",
        ],
    );
    let gone = task(&mut c, "gone", vec![fake_agent(), "hold".into()]);
    pod_ok(
        &pipe,
        &["mobile", "policy", &m2, &gone.id, "--allow", "message"],
    );
    let s2 = session(&mut c, &m2);
    assert_eq!(
        code(message(&mut c, &s2, "demo/busy", "hi", "a1")),
        mobile_codes::MOBILE_DENIED,
        "no permission for this task"
    );
    assert_eq!(
        code(message(&mut c, &s2, "demo/idle", &"x".repeat(1001), "a2")),
        mobile_codes::MESSAGE_INVALID,
        "too long"
    );
    assert_eq!(
        code(message(
            &mut c,
            &s2,
            "demo/idle",
            "stop\u{1b}[201~\r rm -rf",
            "a3"
        )),
        mobile_codes::MESSAGE_INVALID,
        "escape sequence"
    );
    c.call::<TaskRemove>(&TaskRemoveParams {
        task: gone.id.clone(),
        force: true,
        delete_branch: true,
    })
    .unwrap();
    assert_eq!(
        code(message(&mut c, &s2, &gone.id, "hello?", "a4")),
        codes::TASK_NOT_FOUND
    );
    c.call::<ReviewCommentsAdd>(&ReviewAddParams {
        task: "demo/idle".into(),
        comments: vec![NewComment {
            file: "README.md".into(),
            line: 1,
            body: "Say more here".into(),
            ..Default::default()
        }],
    })
    .unwrap();
    let r = phone(
        &mut c,
        &s2,
        mobile_api::MESSAGE,
        json!({ "task": "demo/idle", "text": "fix this one", "operation_id": "a5", "with_open_comments": true }),
    )
    .unwrap();
    assert_eq!(r["replayed"], false);
    until("review comments delivered from the phone", 30, || {
        let l = log_text(&idle_log);
        (l.contains("Say more here") && l.contains("fix this one")).then_some(())
    });
    let comments = c
        .call::<ReviewCommentsList>(&ReviewListParams {
            task: Some("demo/idle".into()),
            state: None,
        })
        .unwrap()
        .comments;
    assert_eq!(comments[0].state, CommentState::Sent);

    // ── audit: who/what/result, hash + length, never the text ──
    let entries = audit(&pipe);
    let ok = entries
        .iter()
        .find(|e| {
            e.action == "message" && e.result == "ok" && e.operation_id.as_deref() == Some("op-1")
        })
        .expect("message audited");
    assert_eq!(ok.actor, m1);
    assert_eq!(ok.msg_len, Some(text.chars().count()));
    assert_eq!(ok.msg_sha256.as_deref().map(str::len), Some(64));
    assert!(ok.msg_body.is_none());
    for (result, op) in [("queued", "op-2"), ("replayed", "op-1")] {
        assert!(
            entries
                .iter()
                .any(|e| e.result == result && e.operation_id.as_deref() == Some(op)),
            "{result} {op}"
        );
    }
    for c in [
        mobile_codes::MOBILE_DENIED,
        mobile_codes::RATE_LIMITED,
        mobile_codes::MESSAGE_INVALID,
        mobile_codes::OPERATION_CONFLICT,
        orch_codes::AGENT_BLOCKED,
        codes::TASK_NOT_FOUND,
    ] {
        assert!(
            entries.iter().any(|e| e.code == Some(c)),
            "code {c} audited"
        );
    }
    assert!(
        entries
            .iter()
            .any(|e| e.action == "terminal.send" && e.result == "denied")
    );
    assert!(
        entries
            .iter()
            .any(|e| e.action == "policy" && e.actor == "local")
    );
    let raw = std::fs::read_to_string(d.state_dir.join("mobile").join("audit.jsonl")).unwrap();
    assert!(
        !raw.contains("rerun the tests"),
        "message text in the audit log"
    );
    let human = pod_ok(&pipe, &["mobile", "audit", "--device", &m1]);
    assert!(
        human.contains("message ok") && human.contains("sha256="),
        "{human}"
    );

    // With bodies enabled, the next message keeps its text (third device:
    // the other two used their message budget for this minute).
    pod_ok(&pipe, &["mobile", "settings", "--audit-bodies", "on"]);
    let m3 = pair(&mut c, "Laptop");
    pod_ok(
        &pipe,
        &["mobile", "policy", &m3, "demo/idle", "--allow", "message"],
    );
    let s3 = session(&mut c, &m3);
    message(&mut c, &s3, "demo/idle", "kept text", "a6").unwrap();
    let last = audit(&pipe)
        .into_iter()
        .rev()
        .find(|e| e.operation_id.as_deref() == Some("a6"))
        .unwrap();
    assert_eq!(last.msg_body.as_deref(), Some("kept text"));

    // ── restart: devices, policy and audit stay; sessions do not ──
    drop(c);
    let (pipe, dir) = d.stop(false);
    let d = Daemon::start_full(pipe.clone(), dir, &[], HOOKS);
    let mut c = d.client();
    assert_eq!(
        code(phone(&mut c, &s3, mobile_api::TASKS, json!({}))),
        mobile_codes::MOBILE_AUTH
    );
    let s3 = session(&mut c, &m3);
    let r = message(&mut c, &s3, "demo/idle", "kept text", "a6").unwrap();
    assert_eq!(r["replayed"], true, "operation ids survive a restart");
    assert!(
        audit(&pipe)
            .iter()
            .any(|e| e.operation_id.as_deref() == Some("op-1"))
    );
    let devs = c.call::<MobileDevices>(&MobileDevicesParams {}).unwrap();
    assert_eq!(devs.devices.len(), 3);
    // A fourth device needs a revoke first.
    assert_eq!(
        rpc_code(c.call_raw("mobile.test.pair", json!({ "name": "Watch" }))),
        mobile_codes::MOBILE_DENIED
    );
    assert!(devs.settings.audit_message_bodies);
}

// ───────────────────────────── gates ─────────────────────────────

/// Gate decisions from a phone: capability checks, idempotent operation
/// ids, PC/phone races decided once, stale and expired gates refused,
/// revoke ends sessions; output only when allowed.
#[test]
fn mobile_gate_decisions_and_revoke() {
    let repo = TempRepo::new();
    let d = Daemon::start_env("mobile-gate", HOOKS);
    let pipe = d.pipe.clone();
    let mut c = d.client();
    setup(&mut c, &repo);
    let t = task(
        &mut c,
        "g",
        vec![
            fake_agent(),
            "print:secret-free build log line".into(),
            "hold".into(),
        ],
    );
    let hidden = task(&mut c, "h", vec![fake_agent(), "hold".into()]);
    let gate = |c: &mut Client, task: &str, expires: Option<u64>| {
        c.call::<GateCreate>(&GateCreateParams {
            task: Some(task.into()),
            message: "Deploy to staging?".into(),
            expires_in_s: expires,
            ..Default::default()
        })
        .unwrap()
        .id
    };
    let m1 = pair(&mut c, "Pixel");
    pod_ok(
        &pipe,
        &["mobile", "policy", &m1, &t.id, "--allow", "status"],
    );
    let s = session(&mut c, &m1);
    let decide = |c: &mut Client, s: &str, g: &str, d: &str, op: &str| {
        phone(
            c,
            s,
            mobile_api::GATE_DECIDE,
            json!({ "gate": g, "decision": d, "operation_id": op }),
        )
    };

    // Visible but not approvable without the capability; other tasks' gates look absent.
    let g1 = gate(&mut c, &t.id, None);
    let gh = gate(&mut c, &hidden.id, None);
    let view = phone(&mut c, &s, mobile_api::TASK, json!({ "task": t.id })).unwrap();
    assert_eq!(view["gates"][0]["id"], g1.as_str());
    assert_eq!(
        code(decide(&mut c, &s, &g1, "approve", "o1")),
        mobile_codes::MOBILE_DENIED
    );
    assert_eq!(
        code(decide(&mut c, &s, &gh, "approve", "o1")),
        orch_codes::GATE_NOT_FOUND
    );
    assert_eq!(
        code(phone(
            &mut c,
            &s,
            mobile_api::TASK,
            json!({ "task": hidden.id })
        )),
        mobile_codes::MOBILE_DENIED
    );
    assert_eq!(
        code(phone(
            &mut c,
            &s,
            mobile_api::OUTPUT,
            json!({ "task": t.id })
        )),
        mobile_codes::MOBILE_DENIED,
        "output is off by default"
    );

    pod_ok(
        &pipe,
        &["mobile", "policy", &m1, &t.id, "--allow", "approve,output"],
    );
    let out = phone(&mut c, &s, mobile_api::OUTPUT, json!({ "task": t.id })).unwrap();
    let lines: Vec<&str> = out["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("secret-free build log line")),
        "{lines:?}"
    );

    // Approve, then the retry replays, the PC is told it is resolved.
    let r = decide(&mut c, &s, &g1, "approve", "o1").unwrap();
    assert_eq!(r["gate"]["state"], "approved");
    assert_eq!(r["replayed"], false);
    let r = decide(&mut c, &s, &g1, "approve", "o1").unwrap();
    assert_eq!(r["replayed"], true);
    assert_eq!(
        code(decide(&mut c, &s, &g1, "reject", "o1")),
        mobile_codes::OPERATION_CONFLICT
    );
    let pc = c.call::<GateResolve>(&GateResolveParams {
        id: g1.clone(),
        decision: GateDecision::Reject,
        note: None,
        choice: None,
    });
    assert_eq!(rpc_code(pc), orch_codes::GATE_RESOLVED);
    let shown = c.call::<GateShow>(&GateRef { id: g1.clone() }).unwrap();
    assert_eq!(shown.state, GateState::Approved);
    assert!(shown.note.unwrap().contains(&m1));

    // Stale: the PC decided first.
    let g2 = gate(&mut c, &t.id, None);
    c.call::<GateResolve>(&GateResolveParams {
        id: g2.clone(),
        decision: GateDecision::Approve,
        note: None,
        choice: None,
    })
    .unwrap();
    match decide(&mut c, &s, &g2, "reject", "o2") {
        Err(ClientError::Rpc(e)) => {
            assert_eq!(e.code, orch_codes::GATE_RESOLVED);
            assert_eq!(e.data.unwrap()["state"], "approved");
        }
        other => panic!("expected GATE_RESOLVED, got {other:?}"),
    }

    // Expired: refused even if the expiry worker has not run yet.
    let g3 = gate(&mut c, &t.id, Some(1));
    std::thread::sleep(Duration::from_millis(1100));
    assert_eq!(
        code(decide(&mut c, &s, &g3, "approve", "o3")),
        orch_codes::GATE_RESOLVED
    );
    until("expiry recorded", 10, || {
        (c.call::<GateShow>(&GateRef { id: g3.clone() })
            .unwrap()
            .state
            == GateState::Expired)
            .then_some(())
    });

    // PC and phone at the same time: exactly one decision.
    for round in 0..3 {
        let g = gate(&mut c, &t.id, None);
        let (p1, p2) = (pipe.clone(), pipe.clone());
        let (g_pc, g_phone, s_phone) = (g.clone(), g.clone(), s.clone());
        let op = format!("race-{round}");
        let a = std::thread::spawn(move || {
            Client::connect(&p1)
                .unwrap()
                .call::<GateResolve>(&GateResolveParams {
                    id: g_pc,
                    decision: GateDecision::Reject,
                    note: None,
                    choice: None,
                })
        });
        let b = std::thread::spawn(move || {
            let mut c = Client::connect(&p2).unwrap();
            phone(
                &mut c,
                &s_phone,
                mobile_api::GATE_DECIDE,
                json!({
                    "gate": g_phone, "decision": "approve", "operation_id": op
                }),
            )
        });
        let (a, b) = (a.join().unwrap(), b.join().unwrap());
        assert!(a.is_ok() != b.is_ok(), "round {round}: {a:?} / {b:?}");
        let state = c.call::<GateShow>(&GateRef { id: g }).unwrap().state;
        assert_eq!(
            state,
            if a.is_ok() {
                GateState::Rejected
            } else {
                GateState::Approved
            }
        );
    }

    // Decisions are audited (actor, gate, operation, result).
    let entries = audit(&pipe);
    let e = entries
        .iter()
        .find(|e| {
            e.action == "gate.decide" && e.operation_id.as_deref() == Some("o1") && e.result == "ok"
        })
        .unwrap();
    assert_eq!(
        (e.actor.as_str(), e.gate.as_deref()),
        (m1.as_str(), Some(g1.as_str()))
    );
    assert_eq!(e.detail.as_deref(), Some("approve"));

    // Revoke: sessions end now, the device stays listed as revoked.
    let g5 = gate(&mut c, &t.id, None);
    let out = pod_ok(&pipe, &["mobile", "revoke", &m1]);
    assert!(out.contains("revoked m1"), "{out}");
    assert_eq!(
        code(decide(&mut c, &s, &g5, "approve", "o5")),
        mobile_codes::MOBILE_AUTH
    );
    assert!(
        rpc_code(c.call_raw("mobile.test.session", json!({ "device": m1 })))
            == mobile_codes::MOBILE_AUTH
    );
    let devs = c.call::<MobileDevices>(&MobileDevicesParams {}).unwrap();
    assert!(devs.devices[0].revoked_ms.is_some());
    assert!(devs.devices[0].policy.is_empty());
    assert_eq!(
        c.call::<GateShow>(&GateRef { id: g5 }).unwrap().state,
        GateState::Pending
    );
    assert!(audit(&pipe).iter().any(|e| e.action == "revoke"));
}

/// Gates with options from a phone (v17, docs/19 §14.2): the phone sees
/// the options and approves with one of them through the single decision
/// path; an off-list or missing choice and a rejection with a choice are
/// refused without deciding; a retry replays, another choice under the same
/// operation id conflicts, and the PC sees the choice and the device.
#[test]
fn mobile_gate_choice_on_a_gate_with_options() {
    let repo = TempRepo::new();
    let d = Daemon::start_env("mobile-choice", HOOKS);
    let pipe = d.pipe.clone();
    let mut c = d.client();
    setup(&mut c, &repo);
    let t = task(&mut c, "c", vec![fake_agent(), "hold".into()]);
    let m1 = pair(&mut c, "Pixel");
    pod_ok(
        &pipe,
        &["mobile", "policy", &m1, &t.id, "--allow", "status,approve"],
    );
    let s = session(&mut c, &m1);
    let g = c
        .call::<GateCreate>(&GateCreateParams {
            task: Some(t.id.clone()),
            message: "Which database?".into(),
            options: vec!["sqlite".into(), "postgres".into()],
            ..Default::default()
        })
        .unwrap()
        .id;
    let view = phone(&mut c, &s, mobile_api::TASK, json!({ "task": t.id })).unwrap();
    assert_eq!(view["gates"][0]["id"], g.as_str());
    assert_eq!(view["gates"][0]["options"], json!(["sqlite", "postgres"]));
    let decide = |c: &mut Client, decision: &str, choice: Option<&str>, op: &str| {
        let mut p = json!({ "gate": g, "decision": decision, "operation_id": op });
        if let Some(ch) = choice {
            p["choice"] = json!(ch);
        }
        phone(c, &s, mobile_api::GATE_DECIDE, p)
    };

    // Refused, and nothing is decided.
    assert_eq!(
        code(decide(&mut c, "approve", Some("mysql"), "x1")),
        codes::INVALID_PARAMS,
        "off-list choice"
    );
    assert_eq!(
        code(decide(&mut c, "approve", None, "x2")),
        codes::INVALID_PARAMS,
        "no choice"
    );
    assert_eq!(
        code(decide(&mut c, "reject", Some("sqlite"), "x3")),
        codes::INVALID_PARAMS,
        "reject with a choice"
    );
    assert_eq!(
        c.call::<GateShow>(&GateRef { id: g.clone() })
            .unwrap()
            .state,
        GateState::Pending
    );

    // Approve with a listed option; the retry replays the same result.
    let r = decide(&mut c, "approve", Some("postgres"), "c1").unwrap();
    assert_eq!(
        (r["gate"]["state"].as_str(), r["gate"]["choice"].as_str()),
        (Some("approved"), Some("postgres"))
    );
    assert_eq!(r["replayed"], false);
    let r = decide(&mut c, "approve", Some("postgres"), "c1").unwrap();
    assert_eq!(
        (r["replayed"].as_bool(), r["gate"]["choice"].as_str()),
        (Some(true), Some("postgres"))
    );
    assert_eq!(
        code(decide(&mut c, "approve", Some("sqlite"), "c1")),
        mobile_codes::OPERATION_CONFLICT,
        "another choice under the same operation id"
    );
    assert_eq!(
        code(decide(&mut c, "approve", Some("sqlite"), "c2")),
        orch_codes::GATE_RESOLVED
    );

    // The PC sees the choice and who made it; the PC cannot change it.
    let shown = c.call::<GateShow>(&GateRef { id: g.clone() }).unwrap();
    assert_eq!(shown.state, GateState::Approved);
    assert_eq!(shown.choice.as_deref(), Some("postgres"));
    assert_eq!(shown.decided_by.as_deref(), Some(m1.as_str()));
    let pc = c.call::<GateResolve>(&GateResolveParams {
        id: g.clone(),
        decision: GateDecision::Approve,
        note: None,
        choice: Some("sqlite".into()),
    });
    assert_eq!(rpc_code(pc), orch_codes::GATE_RESOLVED);
    assert!(
        audit(&pipe).iter().any(|e| e.action == "gate.decide"
            && e.operation_id.as_deref() == Some("c1")
            && e.result == "ok"),
        "the decision is audited"
    );
}

/// Without the test hook variable the stub does not exist.
#[test]
fn pairing_stub_is_off_by_default() {
    let d = Daemon::start("mobile-nohooks");
    let mut c = d.client();
    assert_eq!(
        rpc_code(c.call_raw("mobile.test.pair", json!({ "name": "x" }))),
        codes::METHOD_NOT_FOUND
    );
    let devs = c.call::<MobileDevices>(&MobileDevicesParams {}).unwrap();
    assert_eq!(devs.listener, "off");
    assert!(devs.devices.is_empty());
}

/// security-05: every failed mobile call is audited, parameter errors
/// included (`{}` and wrong field types), without the parameters' content.
#[test]
fn failed_mobile_calls_are_audited_without_their_params() {
    let d = Daemon::start_env("mobile-badparams", HOOKS);
    let mut c = d.client();
    let dev = pair(&mut c, "phone");
    let s = session(&mut c, &dev);
    let secret = "secret-body-text-5309";

    let calls = [
        (mobile_api::TASK, json!({})),
        (mobile_api::TASK, json!({ "task": 5 })),
        (mobile_api::OUTPUT, json!({ "task": [secret] })),
        (mobile_api::GATE_DECIDE, json!({})),
        (
            mobile_api::MESSAGE,
            json!({ "task": "x", "text": "hi", "operation_id": "op-1", "with_open_comments": secret }),
        ),
        (mobile_api::MESSAGE, json!({ "text": secret })),
        (mobile_api::DELIVERY, json!({ "delivery": 7 })),
    ];
    for (method, params) in &calls {
        assert_eq!(
            code(phone(&mut c, &s, method, params.clone())),
            codes::INVALID_PARAMS,
            "{method} {params}"
        );
    }
    // A well-formed call that fails after parsing is audited too.
    assert_eq!(
        code(phone(
            &mut c,
            &s,
            mobile_api::DELIVERY,
            json!({ "delivery": "d404" })
        )),
        orch_codes::REVIEW_NOT_FOUND
    );

    let entries = c
        .call::<MobileAudit>(&MobileAuditParams {
            limit: Some(1000),
            device: None,
        })
        .unwrap()
        .entries;
    let bad: Vec<&MobileAuditEntry> = entries
        .iter()
        .filter(|e| e.code == Some(codes::INVALID_PARAMS))
        .collect();
    assert_eq!(bad.len(), calls.len(), "{entries:#?}");
    let mut want: Vec<&str> = calls.iter().map(|(m, _)| *m).collect();
    let mut got: Vec<&str> = bad.iter().map(|e| e.action.as_str()).collect();
    want.sort_unstable();
    got.sort_unstable();
    assert_eq!(got, want);
    for e in &bad {
        assert_eq!(e.actor, dev);
        assert_eq!(e.result, "error");
        assert_eq!(e.detail.as_deref(), Some("invalid params (not recorded)"));
        assert!(e.msg_body.is_none() && e.msg_len.is_none());
    }
    assert!(
        entries
            .iter()
            .any(|e| e.action == mobile_api::DELIVERY
                && e.code == Some(orch_codes::REVIEW_NOT_FOUND)),
        "{entries:#?}"
    );
    let raw = serde_json::to_string(&entries).unwrap();
    assert!(
        !raw.contains(secret),
        "parameters leaked into the audit: {raw}"
    );
}
