//! Integration wire snapshots of the library crates (docs/20-refactor/libs.md,
//! card T1). The JSON that podd sends for orchestration runs, workers and
//! mail, pull request status and forge errors, discovered sessions and mods
//! comes from pod-orch, pod-forge, pod-discover and pod-mods. Each test runs
//! a fixed script (fixed times, ids and inputs) through those crates, wraps
//! the results in the protocol types the daemon returns, and compares the
//! serialized JSON with goldens taken on master before the libs refactor
//! (commit 64e9530, before card L-F0) with this same file.
//!
//! Only object key order is normalised (both sides are parsed values);
//! array order, missing against `null`, enum strings, error kinds and
//! messages compare as they are. Three values are replaced because they are
//! not inputs: the mods folder and QuickJS heap size (pod-mods) and the
//! fake forge's merge time (wall clock). The per-crate goldens (L3, L7, L8)
//! pin each crate before its own split; these pin the merged result against
//! one baseline.
//!
//! Where the daemon adds its own state the wrapper is filled like podd fills
//! it for a worker without a terminal (`status` none, no unconfirmed wake);
//! the RPC paths themselves are covered by the orch_runs, pr, discovery and
//! mods tests.
//!
//! A mismatch writes the actual JSON next to the test binary
//! (`CARGO_TARGET_TMPDIR`) and names the first differing path. Do not
//! regenerate the goldens to make a change pass. They were written with
//! `POD_LIBS_WIRE_OUT=<dir> cargo test -p podd --test libs_wire_snapshots`
//! on the baseline commit, which writes `<dir>/<name>.json` instead of
//! comparing.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use pod_discover::claude::ClaudeSource;
use pod_discover::codex::CodexSource;
use pod_discover::matching::{RepoPath, TaskPath, match_cwd};
use pod_discover::procs::{ProcInfo, attach_sessions, find_agents, mark_pod_terminals};
use pod_discover::{Cache, Source, scan};
use pod_forge::fake::{FakeForge, FakeState};
use pod_forge::gh::GhForge;
use pod_forge::runner::{Invocation, RunOutput, Runner};
use pod_forge::{CreatePr, Forge, ForgeError, ForgeErrorKind, ForgeRemote, MergeMethod, RepoSlug};
use pod_mods::{
    GroupFacts, Host, ModConfig, ModHandle, ModInfo, ModState, Notice, Permission, StatusHint,
    TaskFacts, TerminalFacts,
};
use pod_orch::model::{attempt_mailbox, run_mailbox};
use pod_orch::{BeginAttempt, MsgText, NewMail, Orch, OrchError, hash_token, text as tx};
use podd::protocol::{
    Attempt, AttemptState, MailType, ModListResult, OrchMailListResult, PROTOCOL_VERSION, Priority,
    RunInfo, TaskPrInfo, WorkerInfo, WorkerState, schema,
};
use serde::Serialize;
use serde_json::{Map, Value, json};

// ─────────────────────────────── harness ───────────────────────────────

/// A scratch directory removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "pod-libs-wire-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Serialized as the daemon writes it, read back as a value.
fn wire<T: Serialize>(v: &T) -> Value {
    serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap()
}

/// The first path where `actual` differs from `want`.
fn first_diff(path: &str, want: &Value, actual: &Value) -> Option<String> {
    match (want, actual) {
        (Value::Object(w), Value::Object(a)) => {
            for k in w.keys().chain(a.keys()) {
                let p = format!("{path}.{k}");
                match (w.get(k), a.get(k)) {
                    (Some(x), Some(y)) => {
                        if let Some(d) = first_diff(&p, x, y) {
                            return Some(d);
                        }
                    }
                    (x, y) => return Some(format!("{p}: want {x:?}, got {y:?}")),
                }
            }
            None
        }
        (Value::Array(w), Value::Array(a)) => {
            for (i, (x, y)) in w.iter().zip(a).enumerate() {
                if let Some(d) = first_diff(&format!("{path}[{i}]"), x, y) {
                    return Some(d);
                }
            }
            (w.len() != a.len())
                .then(|| format!("{path}: want {} elements, got {}", w.len(), a.len()))
        }
        _ => (want != actual).then(|| format!("{path}: want {want}, got {actual}")),
    }
}

/// Compares `actual` with the golden, or records it (`POD_LIBS_WIRE_OUT`).
fn check(name: &str, golden: &str, actual: Value) {
    if let Some(dir) = std::env::var_os("POD_LIBS_WIRE_OUT") {
        let dir = PathBuf::from(dir);
        std::fs::create_dir_all(&dir).unwrap();
        let text = serde_json::to_string_pretty(&actual).unwrap();
        std::fs::write(dir.join(format!("{name}.json")), text).unwrap();
        return;
    }
    let want: Value = serde_json::from_str(golden)
        .unwrap_or_else(|e| panic!("{name}: the golden is not JSON: {e}"));
    if want == actual {
        return;
    }
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{name}.actual.json"));
    std::fs::write(&out, serde_json::to_string_pretty(&actual).unwrap()).unwrap();
    panic!(
        "{name} differs from the baseline golden at {}; actual JSON: {}",
        first_diff("$", &want, &actual).unwrap_or_default(),
        out.display()
    );
}

/// Every object key in `v` that names a token (none may reach the wire).
fn token_keys(v: &Value, path: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                let p = format!("{path}.{k}");
                if k.contains("token") {
                    out.push(p.clone());
                }
                token_keys(x, &p, out);
            }
        }
        Value::Array(a) => {
            for (i, x) in a.iter().enumerate() {
                token_keys(x, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

fn assert_no_token(what: &str, v: &Value) {
    let mut keys = Vec::new();
    token_keys(v, "$", &mut keys);
    assert!(keys.is_empty(), "{what} carries token fields: {keys:?}");
}

// ───────────────────────────── orchestration ─────────────────────────────

fn err(e: OrchError) -> Value {
    json!({ "kind": format!("{:?}", e.kind), "message": e.message })
}

fn begin(o: &mut Orch, item: &str, token: &str, now: u64) -> Result<Attempt, OrchError> {
    o.begin_attempt(
        BeginAttempt {
            run: "r1".into(),
            item: item.into(),
            agent: "codex".into(),
            place: "new".into(),
            retry_of: None,
            token_hash: hash_token(token),
        },
        now,
    )
}

/// `orch.run.show` as podd builds it (no unconfirmed wake).
fn run_info(o: &Orch, run: &str) -> RunInfo {
    let st = o.run(run).unwrap();
    RunInfo {
        wake_unconfirmed_ms: None,
        run: st.rec.run.clone(),
        items: st.rec.items.clone(),
        attempts: st.rec.attempts.iter().map(|a| a.public()).collect(),
        unread: o.unread(&run_mailbox(run)).len(),
        mail: st.mail.len(),
    }
}

/// `orch.worker.list` for a worker whose terminal is gone, as podd builds it.
fn worker_info(o: &Orch, a: &Attempt, now: u64) -> WorkerInfo {
    let waiting = o.has_open_question(&a.id);
    let next = match (a.state, a.worker) {
        (_, WorkerState::Released) => MsgText::new(tx::NEXT_RELEASED),
        (s, _) if s.is_open() && waiting => MsgText::new(tx::NEXT_WAITING_ANSWER),
        (AttemptState::Starting, _) if a.is_queued() => {
            let q = a.queue.as_ref().unwrap();
            MsgText::new(tx::NEXT_QUEUED)
                .with("position", q.position)
                .with("reason", &q.reason)
        }
        (AttemptState::Starting, _) => MsgText::new(tx::NEXT_STARTING),
        (s, _) if s.is_open() && !a.waiting_on.is_empty() => {
            MsgText::new(tx::NEXT_WAITING_ITEMS).with("items", a.waiting_on.join(", "))
        }
        (s, _) if s.is_open() => MsgText::new(tx::NEXT_IDLE),
        (_, WorkerState::Kept) => MsgText::new(tx::NEXT_KEPT),
        (_, WorkerState::Finished) => match a.release_at_ms {
            Some(ms) => {
                MsgText::new(tx::NEXT_RELEASE_IN).with("secs", ms.saturating_sub(now) / 1000)
            }
            None => MsgText::new(tx::NEXT_FINISHED),
        },
        _ => MsgText::new(tx::NEXT_FINISHED),
    };
    WorkerInfo {
        attempt: a.public(),
        item_title: o.item(&a.item).map(|i| i.title.clone()).unwrap_or_default(),
        status: None,
        last_output_ms: 0,
        waiting_answer: waiting,
        unread: o.unread(&attempt_mailbox(&a.id)).len(),
        next: next.en(),
        next_text: Some(next),
        wake_unconfirmed_ms: None,
    }
}

fn workers(o: &Orch, run: &str, now: u64) -> Value {
    let st = o.run(run).unwrap();
    Value::Array(
        st.rec
            .attempts
            .iter()
            .map(|a| wire(&worker_info(o, a, now)))
            .collect(),
    )
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn read_jsonl(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap();
    Value::Array(
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect(),
    )
}

/// One run: items with a dependency and a child, three attempts, a question
/// and its answer, group mail, a read check, an accepted done, a failed done,
/// a lost worker, then a reload from disk.
#[test]
fn orch_runs_workers_and_mail_match_the_baseline() {
    let dir = Scratch::new("orch");
    let root = dir.0.as_path();
    let mut o = Orch::load(root, pod_orch::Limits::default(), 1);
    let mut s = Map::new();
    let mut e = Map::new();

    e.insert(
        "create_run_blank".into(),
        err(o.create_run("  ", None, None, 2).unwrap_err()),
    );
    let run = o
        .create_run("  ship the parser  ", Some("term_c"), None, 5)
        .unwrap();
    assert_eq!(run.id, "r1");
    s.insert("created".into(), wire(&run_info(&o, "r1")));
    o.add_item(
        "r1",
        None,
        "\n# Parse the header\n\nmore detail",
        &[],
        None,
        6,
    )
    .unwrap();
    o.add_item(
        "r1",
        Some("  wire it  "),
        "connect",
        &["i1".into()],
        None,
        7,
    )
    .unwrap();
    o.add_item("r1", Some("child"), "a sub task", &[], Some("i1"), 8)
        .unwrap();
    e.insert(
        "add_item_missing_dep".into(),
        err(o
            .add_item("r1", None, "x", &["i9".into()], None, 9)
            .unwrap_err()),
    );
    e.insert(
        "begin_pending".into(),
        err(begin(&mut o, "i2", "k", 9).unwrap_err()),
    );

    let a1 = begin(&mut o, "i1", "k1", 10).unwrap();
    assert_eq!(a1.id, "a1");
    o.attach_worker("a1", Some("task_1"), "term_w1", Some("brief-a1.md"), 3)
        .unwrap();
    o.mark_ready("a1", 12).unwrap();
    e.insert(
        "verify_wrong_token".into(),
        err(o.verify_token("a1", Some("wrong")).unwrap_err()),
    );
    let a2 = begin(&mut o, "i3", "k2", 13).unwrap();
    assert_eq!(a2.id, "a2");

    let q = o
        .send(
            Some("r1"),
            NewMail::new("attempt:a1", "run:r1", MailType::Question, "which format?")
                .body("json or toml"),
            |_| false,
            15,
        )
        .unwrap();
    s.insert("workers_asking".into(), workers(&o, "r1", 15));
    let mut ans = NewMail::new("run:r1", "attempt:a1", MailType::Answer, "json").body("use json");
    ans.reply_to = Some(q.id.clone());
    ans.priority = Priority::High;
    o.send(Some("r1"), ans, |_| false, 16).unwrap();
    o.send(
        Some("r1"),
        NewMail::pod(
            "run:r1",
            "@all",
            MailType::Status,
            MsgText::new(tx::GROUP_CHECKED).with("n", 2),
        )
        .body_text(MsgText::new(tx::NEXT_WORKING))
        .payload(json!({"note": null, "list": [3, 1, 2]})),
        |_| false,
        17,
    )
    .unwrap();
    e.insert(
        "send_bad_address".into(),
        err(o
            .send(
                Some("r1"),
                NewMail::new("run:r1", "bogus", MailType::Status, "s"),
                |_| false,
                17,
            )
            .unwrap_err()),
    );
    s.insert(
        "check_run".into(),
        wire(&o.check(&[run_mailbox("r1")], true, 20).unwrap()),
    );
    s.insert(
        "check_a1".into(),
        wire(&o.check(&[attempt_mailbox("a1")], false, 21).unwrap()),
    );

    let d = o
        .done(
            "a1",
            Some("k1"),
            true,
            "parsed",
            &["src/parse.rs".into()],
            Some("report.md"),
            Some(60_000),
            30,
        )
        .unwrap();
    s.insert(
        "done_a1".into(),
        json!({
            "accepted": d.accepted,
            "repeated": d.repeated,
            "attempt": wire(&d.attempt.public()),
            "mail": wire(&d.mail),
        }),
    );
    e.insert(
        "done_wrong_token".into(),
        err(o
            .done("a2", Some("k1"), true, "x", &[], None, None, 31)
            .unwrap_err()),
    );
    let d = o
        .done("a2", Some("k2"), false, "could not", &[], None, None, 31)
        .unwrap();
    s.insert(
        "done_a2".into(),
        json!({
            "accepted": d.accepted,
            "repeated": d.repeated,
            "attempt": wire(&d.attempt.public()),
            "mail": wire(&d.mail),
        }),
    );
    let a3 = begin(&mut o, "i2", "k3", 32).unwrap();
    assert_eq!(a3.id, "a3");
    s.insert(
        "end_a3".into(),
        json!(
            o.end_attempt(
                "a3",
                AttemptState::Lost,
                "terminal gone",
                None,
                Some(500),
                33
            )
            .unwrap()
        ),
    );

    let info = run_info(&o, "r1");
    s.insert("final_run".into(), wire(&info));
    s.insert("final_workers".into(), workers(&o, "r1", 40));
    s.insert(
        "final_mail".into(),
        wire(&OrchMailListResult {
            messages: o.mail_of("r1").unwrap(),
        }),
    );
    s.insert("errors".into(), Value::Object(e));

    // What a restarted daemon reads back (pod-storage writes these files).
    let back = Orch::load(root, pod_orch::Limits::default(), 50);
    assert_eq!(run_info(&back, "r1"), info, "reloaded RunInfo");
    s.insert(
        "files".into(),
        json!({
            "run.json": read_json(&pod_orch::store::run_json(root, "r1")),
            "mail.jsonl": read_jsonl(&pod_orch::store::mail_jsonl(root, "r1")),
            "index.json": read_json(&pod_orch::store::index_json(root)),
        }),
    );

    let actual = Value::Object(s);
    for k in [
        "created",
        "workers_asking",
        "done_a1",
        "done_a2",
        "final_run",
        "final_workers",
    ] {
        assert_no_token(k, &actual[k]);
    }
    check("orch", GOLDEN_ORCH, actual);
}

// ──────────────────────────────── forge ────────────────────────────────

/// Answers invocations from a script (first matching prefix of the command
/// line) and records them.
struct Recorded {
    script: Vec<(&'static str, Result<RunOutput, ForgeError>)>,
    seen: Mutex<Vec<Invocation>>,
}

impl Recorded {
    fn new(script: Vec<(&'static str, Result<RunOutput, ForgeError>)>) -> Self {
        Recorded {
            script,
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl Runner for Recorded {
    fn run(&self, _dir: &Path, inv: &Invocation) -> Result<RunOutput, ForgeError> {
        self.seen.lock().unwrap().push(inv.clone());
        let line = inv.line();
        self.script
            .iter()
            .find(|(prefix, _)| line.starts_with(prefix))
            .map(|(_, o)| o.clone())
            .unwrap_or_else(|| Err(ForgeError::failed(format!("unscripted: {line}"))))
    }
}

fn ok(stdout: &str) -> Result<RunOutput, ForgeError> {
    Ok(RunOutput {
        code: Some(0),
        stdout: stdout.into(),
        stderr: String::new(),
    })
}

fn fail(code: i32, stderr: &str) -> Result<RunOutput, ForgeError> {
    Ok(RunOutput {
        code: Some(code),
        stdout: String::new(),
        stderr: stderr.into(),
    })
}

const REMOTE_V: &str = "upstream\thttps://gitlab.com/x/y.git (fetch)\n\
origin\thttps://user:secret@github.com/cli/cli.git (fetch)\n\
origin\thttps://user:secret@github.com/cli/cli.git (push)\n";

/// The commands a forge sent: program, arguments, stdin and timeout.
fn invocations(r: &Recorded) -> Value {
    Value::Array(
        r.seen
            .lock()
            .unwrap()
            .iter()
            .map(|i| {
                json!({
                    "line": i.line(),
                    "stdin": i.stdin,
                    "timeout_ms": i.timeout.as_millis() as u64,
                })
            })
            .collect(),
    )
}

/// `task.pr.status` around a collected status, as podd fills it.
fn pr_info(status: pod_forge::PrStatus, branch: &str) -> TaskPrInfo {
    TaskPrInfo {
        task: "demo/t1".into(),
        branch: branch.into(),
        default_base: podd::server::pr::default_base("origin/main", Some("origin")),
        ahead: 2,
        unpushed: 0,
        dirty: 1,
        status,
        cached: false,
        refreshed_ms: 1_000,
    }
}

fn gh_status(
    script: Vec<(&'static str, Result<RunOutput, ForgeError>)>,
    known: Option<u64>,
) -> Value {
    let gh = GhForge::new(Recorded::new(script));
    let head = pod_forge::gh::parse_view(pod_forge::fixtures::VIEW_OPEN)
        .unwrap()
        .head;
    let st = pod_forge::status::collect(&gh, Path::new("."), &head, known);
    json!({
        "info": wire(&pr_info(st, &head)),
        "commands": invocations(gh.runner()),
    })
}

/// Pull request status through the `gh` adapter with recorded command
/// output: open, merged, failing checks, missing remote, gh errors; create
/// and merge; the error classification; the fake forge podd runs in tests.
#[test]
fn pull_request_results_and_errors_match_the_baseline() {
    use pod_forge::fixtures as fx;
    let mut s = Map::new();
    let list_open = format!("[{}]", fx::VIEW_OPEN.trim());
    let open_number = pod_forge::gh::parse_view(fx::VIEW_OPEN).unwrap().number;

    s.insert(
        "open_known".into(),
        gh_status(
            vec![
                ("git remote -v", ok(REMOTE_V)),
                ("gh pr view", ok(fx::VIEW_OPEN)),
                ("gh pr checks", ok(fx::CHECKS_OPEN)),
                ("gh api", ok(fx::REVIEW_COMMENTS)),
            ],
            Some(open_number),
        ),
    );
    s.insert(
        "open_by_branch".into(),
        gh_status(
            vec![
                ("git remote -v", ok(REMOTE_V)),
                ("gh pr list", ok(&list_open)),
                ("gh pr checks", fail(8, "")),
                (
                    "gh api",
                    fail(1, "HTTP 403: Resource not accessible by integration"),
                ),
            ],
            None,
        ),
    );
    s.insert(
        "merged".into(),
        gh_status(
            vec![
                ("git remote -v", ok(REMOTE_V)),
                ("gh pr view", ok(fx::VIEW_MERGED)),
                ("gh pr list", ok(&format!("[{}]", fx::VIEW_MERGED.trim()))),
                ("gh pr checks", ok(fx::CHECKS_MERGED)),
            ],
            Some(open_number),
        ),
    );
    s.insert(
        "checks_failed".into(),
        gh_status(
            vec![
                ("git remote -v", ok(REMOTE_V)),
                ("gh pr list", ok(&list_open)),
                ("gh pr checks", fail(1, fx::CHECKS_FAILED)),
                ("gh api", ok("[]")),
            ],
            None,
        ),
    );
    s.insert(
        "no_pr".into(),
        gh_status(
            vec![("git remote -v", ok(REMOTE_V)), ("gh pr list", ok("[]"))],
            None,
        ),
    );
    s.insert(
        "no_remote".into(),
        gh_status(vec![("git remote -v", ok(""))], None),
    );
    s.insert(
        "unsupported_host".into(),
        gh_status(
            vec![(
                "git remote -v",
                ok("origin\thttps://gitlab.com/x/y.git (fetch)\n"),
            )],
            None,
        ),
    );
    s.insert(
        "not_logged_in".into(),
        gh_status(
            vec![
                ("git remote -v", ok(REMOTE_V)),
                (
                    "gh pr list",
                    fail(
                        4,
                        "To get started with GitHub CLI, please run:  gh auth login",
                    ),
                ),
            ],
            None,
        ),
    );
    s.insert(
        "gh_missing".into(),
        gh_status(
            vec![
                ("git remote -v", ok(REMOTE_V)),
                (
                    "gh pr list",
                    Err(ForgeError::new(
                        ForgeErrorKind::ToolMissing,
                        "gh is not installed",
                    )),
                ),
            ],
            None,
        ),
    );

    // Create (body on stdin, then a view) and merge, through the adapter.
    let remote = ForgeRemote {
        name: "origin".into(),
        url: "https://github.com/cli/cli.git".into(),
        slug: RepoSlug {
            host: "github.com".into(),
            owner: "cli".into(),
            name: "cli".into(),
        },
    };
    let gh = GhForge::new(Recorded::new(vec![
        ("git push", ok("")),
        (
            "gh pr create",
            ok("https://github.com/cli/cli/pull/14964\n"),
        ),
        ("gh pr view 14964", ok(fx::VIEW_OPEN)),
        ("gh pr merge", ok("")),
        ("gh pr view 7", ok(fx::VIEW_MERGED)),
        (
            "gh pr close",
            fail(1, "GraphQL: Could not resolve to a PullRequest"),
        ),
    ]));
    let req = CreatePr {
        head: "pod/x".into(),
        base: "main".into(),
        title: "제목 \"quoted\"".into(),
        body: "본문\n- a\n".into(),
        draft: true,
    };
    let dot = Path::new(".");
    s.insert(
        "gh_writes".into(),
        json!({
            "push": wire(&gh.push(dot, &remote, "pod/x").map_err(|e| wire(&e))),
            "create": wire(&gh.create(dot, &remote, &req).map_err(|e| wire(&e))),
            "merge": wire(&gh.merge(dot, &remote, 7, MergeMethod::Squash).map_err(|e| wire(&e))),
            "close": wire(&gh.close(dot, &remote, 9).map_err(|e| wire(&e))),
            "commands": invocations(gh.runner()),
        }),
    );

    // How tool failures are classified (kind and message on the wire).
    let samples: [(&str, Option<i32>, &str); 9] = [
        (
            "gh",
            Some(4),
            "To get started with GitHub CLI, please run:  gh auth login",
        ),
        (
            "gh",
            Some(1),
            "HTTP 404: Not Found (https://api.github.com/repos/o/r/pulls/9)",
        ),
        ("gh", Some(1), "API rate limit exceeded for user ID 1."),
        (
            "gh",
            Some(1),
            "error connecting to api.github.com\ncheck your internet connection",
        ),
        (
            "gh",
            Some(1),
            "a pull request for branch \"pod/x\" into branch \"main\" already exists:\nhttps://github.com/o/r/pull/3",
        ),
        (
            "gh",
            Some(1),
            "HTTP 403: Must have admin rights to Repository.",
        ),
        (
            "git",
            Some(1),
            " ! [rejected]        pod/x -> pod/x (non-fast-forward)\nerror: failed to push some refs to 'https://user:secret@github.com/o/r.git'",
        ),
        (
            "git",
            Some(128),
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com",
        ),
        ("gh", None, ""),
    ];
    s.insert(
        "classify".into(),
        Value::Array(
            samples
                .iter()
                .map(|(tool, code, stderr)| wire(&ForgeError::classify(tool, *code, stderr)))
                .collect(),
        ),
    );

    // The fake forge (POD_FORGE=fake:<file>): its state file is the wire
    // between the tests and podd.
    let dir = Scratch::new("forge");
    let file = dir.0.join("forge").join("state.json");
    let seed = FakeForge::with_file(file.clone());
    seed.edit(|st: &mut FakeState| {
        st.remote = Some(remote.clone());
        st.fail.insert(
            "close".into(),
            ForgeError::new(ForgeErrorKind::NoPermission, "no permission"),
        );
    });
    let fake = FakeForge::with_file(file.clone());
    let wd = dir.0.as_path();
    let not_pushed = fake.create(wd, &remote, &req).map_err(|e| wire(&e));
    fake.push(wd, &remote, "pod/x").unwrap();
    let created = fake.create(wd, &remote, &req).unwrap();
    let draft_merge = fake.merge(wd, &remote, created.number, MergeMethod::Merge);
    fake.edit(|st: &mut FakeState| {
        st.pr_mut(created.number).unwrap().view.draft = false;
        st.new_checks = Some(pod_forge::fixtures::checks_passed());
    });
    let merged = fake.merge(wd, &remote, created.number, MergeMethod::Squash);
    let again = fake.merge(wd, &remote, created.number, MergeMethod::Squash);
    let closed = fake.close(wd, &remote, created.number);
    let status = pod_forge::status::collect(&fake, wd, "pod/x", Some(created.number));
    let mut state: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
    let mut merged = wire(&merged.map_err(|e| wire(&e)));
    let mut status = wire(&status);
    for v in [&mut state, &mut merged, &mut status] {
        wall_clock_out(v);
    }
    s.insert(
        "fake".into(),
        json!({
            "create_unpushed": wire(&not_pushed),
            "created": wire(&created),
            "merge_draft": wire(&draft_merge.map_err(|e| wire(&e))),
            "merged": merged,
            "merge_again": wire(&again.map_err(|e| wire(&e))),
            "close_injected": wire(&closed.map_err(|e| wire(&e))),
            "status": status,
            "state_file": state,
        }),
    );

    check("pr", GOLDEN_PR, Value::Object(s));
}

/// `merged_at` / `closed_at` set from the wall clock become `<now>`.
fn wall_clock_out(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if (k == "merged_at" || k == "closed_at") && x.is_string() {
                    *x = json!("<now>");
                } else {
                    wall_clock_out(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(wall_clock_out),
        _ => {}
    }
}

// ────────────────────────────── discovery ──────────────────────────────

const MTIME_MS: u64 = 1_767_225_600_000;

/// pod-discover's fixtures copied with every file's mtime set to [`MTIME_MS`].
fn fixed_fixtures(into: &Path) {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let t = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &t);
            } else {
                std::fs::copy(e.path(), &t).unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(&t)
                    .unwrap()
                    .set_modified(UNIX_EPOCH + Duration::from_millis(MTIME_MS))
                    .unwrap();
            }
        }
    }
    let from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("pod-discover")
        .join("tests")
        .join("fixtures");
    copy(&from, into);
}

fn proc(
    pid: u32,
    ppid: u32,
    name: &str,
    start_ms: u64,
    cmd: Option<&str>,
    cwd: Option<&str>,
) -> ProcInfo {
    ProcInfo {
        pid,
        ppid,
        name: name.into(),
        start_ms,
        cmdline: cmd.map(Into::into),
        cwd: cwd.map(Into::into),
    }
}

/// A scan of fixed session files, the running agents matched to them in the
/// daemon's order, and the repository/task match of each session's cwd.
#[test]
fn discovered_sessions_match_the_baseline() {
    let dir = Scratch::new("discover");
    let root = dir.0.as_path();
    fixed_fixtures(root);
    let rel = |p: &Path| {
        p.strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut sources: Vec<Box<dyn Source>> = vec![
        Box::new(ClaudeSource::new(root.join("claude"))),
        Box::new(CodexSource::new(root.join("codex"))),
    ];
    let mut cache = Cache::default();
    let out = scan(&mut sources, &mut cache, &pod_discover::Limits::default());
    let repos = [
        RepoPath {
            name: "demo",
            path: r"C:\src\demo",
        },
        RepoPath {
            name: "other",
            path: r"c:\SRC\other\",
        },
    ];
    let tasks = [TaskPath {
        id: "demo/t1",
        repo: "demo",
        worktree: r"C:\src\demo\.pod\t1",
    }];
    let sessions: Vec<Value> = out
        .sessions
        .iter()
        .map(|f| {
            json!({
                "agent": wire(&f.agent),
                "meta": wire(&f.meta),
                "path": rel(&f.path),
                "mtime_ms": f.mtime_ms,
                "match": wire(&match_cwd(&f.meta.cwd, &repos, &tasks)),
            })
        })
        .collect();

    let resume = "claude --resume 33333333-2222-4333-8444-555555555555";
    let procs = vec![
        proc(4, 0, "System", 0, None, None),
        proc(100, 4, "pod-holder.exe", 10, None, None),
        proc(101, 100, "cmd.exe", 11, None, None),
        proc(4242, 101, "claude.exe", 0, None, None),
        proc(
            300,
            4,
            "node.exe",
            1,
            Some(r#"node "C:\npm\node_modules\@openai\codex\bin\codex.js""#),
            Some(r"C:\src\demo"),
        ),
        proc(301, 300, "codex.exe", 2, Some("codex.exe"), None),
        proc(500, 4, "claude.exe", 3, Some(resume), None),
        proc(600, 4, "claude.exe", 0, None, Some(r"C:\src\other")),
        proc(700, 4, "node.exe", 5, None, Some(r"C:\src\demo")),
        proc(
            800,
            4,
            "codex.exe",
            6,
            Some("codex app-server --listen stdio"),
            None,
        ),
    ];
    let mut agents = find_agents(&procs);
    mark_pod_terminals(&mut agents, &procs, &[("t1".to_string(), vec![100, 101])]);
    attach_sessions(&mut agents, &out.running_ids, &out.sessions);

    let cwd_cases: Vec<Value> = [
        r"C:\src\demo\.pod\t1\crates",
        r"c:\src\DEMO",
        r"C:\src\other\x",
        r"C:\src\demo2",
        "",
    ]
    .iter()
    .map(|c| json!([c, wire(&match_cwd(c, &repos, &tasks))]))
    .collect();

    check(
        "discovery",
        GOLDEN_DISCOVERY,
        json!({
            "sessions": sessions,
            "running_ids": wire(&out.running_ids),
            "truncated": out.truncated,
            "running": wire(&agents),
            "cwd_match": cwd_cases,
        }),
    );
}

// ──────────────────────────────── mods ────────────────────────────────

struct NoHost;

impl Host for NoHost {
    fn terminals(&self) -> Vec<TerminalFacts> {
        Vec::new()
    }
    fn tasks(&self) -> Vec<TaskFacts> {
        Vec::new()
    }
    fn groups(&self) -> Vec<GroupFacts> {
        Vec::new()
    }
    fn notify(&self, _: Notice) {}
    fn send_text(&self, _: &str, _: &str, _: bool) -> Result<(), String> {
        Ok(())
    }
    fn set_status(&self, _: &str, _: &str, _: Option<StatusHint>) -> Result<(), String> {
        Ok(())
    }
    fn clear_status(&self, _: &str) {}
    fn run_check(&self, _: &str, _: &str, _: &str, _: Vec<String>) -> Result<(), String> {
        Ok(())
    }
    fn request_gate(&self, _: &str, _: &str, _: &str, _: &str) -> Result<String, String> {
        Ok("g1".into())
    }
    fn mod_changed(&self, _: &ModInfo) {}
}

fn write_mod(root: &Path, folder: &str, manifest: Value, js: &str) {
    let d = root.join(folder);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("mod.json"), manifest.to_string()).unwrap();
    std::fs::write(d.join("main.js"), js).unwrap();
}

/// The heap size is not an input; running mods must report one.
fn mod_out(mut m: ModInfo) -> Value {
    if m.state == ModState::Running {
        assert!(m.memory_kib > 0, "{}: no heap while running", m.name);
    }
    m.memory_kib = 0;
    wire(&m)
}

/// `mod.list` / `mod.set_enabled` / `mod.grant` results from the mod thread
/// over a fixed mods folder, and the state file it keeps.
#[test]
fn mod_results_match_the_baseline() {
    let dir = Scratch::new("mods");
    let root = dir.0.as_path();
    write_mod(
        root,
        "alpha",
        json!({"pod": 1, "name": "alpha", "version": "1.2.0", "title": "Alpha",
            "description": "Golden fixture", "entry": "main.js",
            "permissions": ["notify", "commands", "status", "terminal.send", "check"],
            "checks": {"test": ["cargo", "test"], "lint": ["cargo", "clippy", "-q"]},
            "layouts": ["layouts/two.json"]}),
        r#"
        pod.on("task.changed", () => {});
        pod.command({ id: "hello", title: "인사", detail: "알림을 띄운다" }, () => {});
        pod.detector({ match: ["C:\\Tools\\Aider.EXE", "claude"], onScreen: () => null });
        pod.log("line one\nline two");
        "#,
    );
    let lay = root.join("alpha").join("layouts");
    std::fs::create_dir_all(&lay).unwrap();
    std::fs::write(
        lay.join("two.json"),
        r#"{"id":"two","title":"둘","center":{"split":"h","children":[{"panel":"terminal","index":0,"size":0.5},{"tabs":[{"panel":"review"},{"panel":"editor"}]}]}}"#,
    )
    .unwrap();
    write_mod(
        root,
        "boom",
        json!({"pod": 1, "name": "boom", "version": "0.1.0", "entry": "main.js"}),
        r#"throw new Error("boom at load");"#,
    );
    write_mod(
        root,
        "quiet",
        json!({"pod": 1, "name": "quiet", "version": "3", "entry": "main.js",
            "permissions": ["gate", "orch"]}),
        "",
    );
    write_mod(
        root,
        "Bad Name",
        json!({"pod": 1, "name": "x", "version": "1"}),
        "",
    );

    let h = ModHandle::start(
        ModConfig {
            root: root.to_path_buf(),
            log_dir: None,
            limits: pod_mods::Limits {
                load_ms: 2_000,
                call_ms: 1_000,
                ..pod_mods::Limits::default()
            },
        },
        Arc::new(NoHost),
    );
    let list = |h: &ModHandle| {
        let mut l = h.list().unwrap();
        l.root = "<root>".into();
        let mods: Vec<ModInfo> = std::mem::take(&mut l.mods);
        let mut v = wire(&ModListResult {
            enabled: true,
            list: l,
        });
        v["mods"] = Value::Array(mods.into_iter().map(mod_out).collect());
        v
    };
    let discovered = list(&h);
    let enable: Vec<Value> = ["alpha", "boom", "quiet", "missing"]
        .iter()
        .map(|m| match h.set_enabled(m, true) {
            Ok(i) => json!({ "ok": mod_out(i) }),
            Err(e) => json!({ "err": e }),
        })
        .collect();
    let grant: Vec<Value> = [
        ("alpha", Permission::TerminalSend, true),
        ("quiet", Permission::Orch, true),
        ("quiet", Permission::Gate, false),
        ("alpha", Permission::Orch, true),
    ]
    .iter()
    .map(|(m, p, on)| match h.grant(m, *p, *on) {
        Ok(i) => json!({ "ok": mod_out(i) }),
        Err(e) => json!({ "err": e }),
    })
    .collect();
    let enabled = list(&h);
    let disable = mod_out(h.set_enabled("alpha", false).unwrap());
    let state_file: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(pod_mods::STATE_FILE)).unwrap())
            .unwrap();
    h.shutdown();
    check(
        "mods",
        GOLDEN_MODS,
        json!({
            "discovered": discovered,
            "set_enabled": enable,
            "grant": grant,
            "enabled": enabled,
            "disable_alpha": disable,
            "state_file": state_file,
        }),
    );
}

// ─────────────────────────────── schema ───────────────────────────────

/// `v` as compact JSON with object keys sorted at every level (whatever
/// order the map keeps).
fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write_sorted(v, &mut out);
    out
}

fn write_sorted(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String((*k).clone()).to_string());
                out.push(':');
                write_sorted(&m[*k], out);
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_sorted(x, out);
            }
            out.push(']');
        }
        x => out.push_str(&x.to_string()),
    }
}

/// FNV-1a, 64 bit: a fingerprint, not a security hash.
fn fnv64(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// The schema of every method and notification whose types come from the
/// library crates (orch.*, task.pr.*, sessions.*, mod.*) as a fingerprint of
/// its key-sorted JSON; protocol_wire compares the full text.
#[test]
fn library_wire_schema_matches_the_baseline() {
    assert_eq!(PROTOCOL_VERSION, 19, "protocol version");
    assert_eq!(pod_orch::engine::RUN_VERSION, 1, "orch run.json version");
    let sch = schema();
    let lib = |name: &str| {
        ["orch.", "task.pr.", "sessions.", "mod."]
            .iter()
            .any(|p| name.starts_with(p))
    };
    let mut out = Map::new();
    for part in ["methods", "notifications"] {
        let mut m = Map::new();
        for (name, v) in sch[part].as_object().unwrap() {
            if lib(name) {
                m.insert(name.clone(), json!(fnv64(&canonical(v))));
            }
        }
        out.insert(part.into(), Value::Object(m));
    }
    out.insert("protocol".into(), sch["protocol"].clone());
    check("schema", GOLDEN_SCHEMA, Value::Object(out));
}

// ─────────────────────────────── goldens ───────────────────────────────
// Taken on the baseline commit (see the top of the file).

const GOLDEN_ORCH: &str = r##"{
  "created": {
    "run": {
      "id": "r1",
      "objective": "ship the parser",
      "coordinator_terminal": "term_c",
      "depth": 0,
      "created_ms": 5
    },
    "items": [],
    "attempts": [],
    "unread": 0,
    "mail": 0
  },
  "workers_asking": [
    {
      "attempt": {
        "id": "a1",
        "run": "r1",
        "item": "i1",
        "generation": 1,
        "state": "ready",
        "worker": "active",
        "agent": "codex",
        "place": "new",
        "depth": 1,
        "task": "task_1",
        "terminal": "term_w1",
        "brief_path": "brief-a1.md",
        "created_ms": 10,
        "ready_ms": 12,
        "seen_turn": 3,
        "reopens": 0
      },
      "item_title": "Parse the header",
      "last_output_ms": 0,
      "waiting_answer": true,
      "unread": 0,
      "next": "waiting for an answer",
      "next_text": {
        "kind": "next.waiting_answer"
      }
    },
    {
      "attempt": {
        "id": "a2",
        "run": "r1",
        "item": "i3",
        "generation": 1,
        "state": "starting",
        "worker": "active",
        "agent": "codex",
        "place": "new",
        "depth": 1,
        "created_ms": 13,
        "seen_turn": 0,
        "reopens": 0
      },
      "item_title": "child",
      "last_output_ms": 0,
      "waiting_answer": false,
      "unread": 0,
      "next": "starting",
      "next_text": {
        "kind": "next.starting"
      }
    }
  ],
  "check_run": [
    {
      "id": "m1",
      "run": "r1",
      "from": "attempt:a1",
      "to": "run:r1",
      "recipients": [
        "run:r1"
      ],
      "type": "question",
      "subject": "which format?",
      "body": "json or toml",
      "priority": "normal",
      "created_ms": 15,
      "read_ms": 20
    }
  ],
  "check_a1": [
    {
      "id": "m2",
      "run": "r1",
      "from": "run:r1",
      "to": "attempt:a1",
      "recipients": [
        "attempt:a1"
      ],
      "type": "answer",
      "subject": "json",
      "body": "use json",
      "thread": "m1",
      "reply_to": "m1",
      "priority": "high",
      "created_ms": 16
    },
    {
      "id": "m3",
      "run": "r1",
      "from": "run:r1",
      "to": "@all",
      "recipients": [
        "attempt:a1",
        "attempt:a2"
      ],
      "type": "status",
      "subject": "group  check  (no verdict)",
      "body": "working",
      "payload": {
        "note": null,
        "list": [
          3,
          1,
          2
        ]
      },
      "priority": "normal",
      "created_ms": 17,
      "text": {
        "kind": "group.checked",
        "params": {
          "n": "2"
        }
      },
      "body_text": {
        "kind": "next.working"
      }
    }
  ],
  "done_a1": {
    "accepted": true,
    "repeated": false,
    "attempt": {
      "id": "a1",
      "run": "r1",
      "item": "i1",
      "generation": 1,
      "state": "succeeded",
      "worker": "finished",
      "agent": "codex",
      "place": "new",
      "depth": 1,
      "task": "task_1",
      "terminal": "term_w1",
      "brief_path": "brief-a1.md",
      "created_ms": 10,
      "ready_ms": 12,
      "ended_ms": 30,
      "release_at_ms": 60030,
      "body": "parsed",
      "files": [
        "src/parse.rs"
      ],
      "report": "report.md",
      "seen_turn": 3,
      "reopens": 0
    },
    "mail": {
      "id": "m4",
      "run": "r1",
      "from": "attempt:a1",
      "to": "run:r1",
      "recipients": [
        "run:r1"
      ],
      "type": "done",
      "subject": "done (succeeded): Parse the header",
      "body": "parsed",
      "payload": {
        "outcome": "succeeded",
        "item": "i1",
        "attempt": "a1",
        "files": [
          "src/parse.rs"
        ],
        "report": "report.md",
        "accepted": true,
        "generation": 1,
        "reopens": 0,
        "release_at_ms": 60030
      },
      "priority": "normal",
      "created_ms": 30,
      "text": {
        "kind": "done.report",
        "params": {
          "attempt": "a1",
          "item": "i1",
          "outcome": "succeeded",
          "title": "Parse the header"
        }
      }
    }
  },
  "done_a2": {
    "accepted": true,
    "repeated": false,
    "attempt": {
      "id": "a2",
      "run": "r1",
      "item": "i3",
      "generation": 1,
      "state": "failed",
      "worker": "finished",
      "agent": "codex",
      "place": "new",
      "depth": 1,
      "created_ms": 13,
      "ended_ms": 31,
      "body": "could not",
      "seen_turn": 0,
      "reopens": 0
    },
    "mail": {
      "id": "m5",
      "run": "r1",
      "from": "attempt:a2",
      "to": "run:r1",
      "recipients": [
        "run:r1"
      ],
      "type": "done",
      "subject": "done (failed): child",
      "body": "could not",
      "payload": {
        "outcome": "failed",
        "item": "i3",
        "attempt": "a2",
        "files": [],
        "report": null,
        "accepted": true,
        "generation": 1,
        "reopens": 0,
        "release_at_ms": null
      },
      "priority": "normal",
      "created_ms": 31,
      "text": {
        "kind": "done.report",
        "params": {
          "attempt": "a2",
          "item": "i3",
          "outcome": "failed",
          "title": "child"
        }
      }
    }
  },
  "end_a3": true,
  "final_run": {
    "run": {
      "id": "r1",
      "objective": "ship the parser",
      "coordinator_terminal": "term_c",
      "depth": 0,
      "created_ms": 5
    },
    "items": [
      {
        "id": "i1",
        "run": "r1",
        "title": "Parse the header",
        "spec": "\n# Parse the header\n\nmore detail",
        "state": "completed",
        "generation": 1,
        "start_failures": 0,
        "circuit_open": false,
        "attempt": "a1",
        "created_ms": 6,
        "updated_ms": 30
      },
      {
        "id": "i2",
        "run": "r1",
        "title": "wire it",
        "spec": "connect",
        "deps": [
          "i1"
        ],
        "state": "ready",
        "generation": 1,
        "start_failures": 0,
        "circuit_open": false,
        "attempt": "a3",
        "created_ms": 7,
        "updated_ms": 33
      },
      {
        "id": "i3",
        "run": "r1",
        "title": "child",
        "spec": "a sub task",
        "parent": "i1",
        "state": "failed",
        "generation": 1,
        "start_failures": 0,
        "circuit_open": false,
        "attempt": "a2",
        "created_ms": 8,
        "updated_ms": 31
      }
    ],
    "attempts": [
      {
        "id": "a1",
        "run": "r1",
        "item": "i1",
        "generation": 1,
        "state": "succeeded",
        "worker": "finished",
        "agent": "codex",
        "place": "new",
        "depth": 1,
        "task": "task_1",
        "terminal": "term_w1",
        "brief_path": "brief-a1.md",
        "created_ms": 10,
        "ready_ms": 12,
        "ended_ms": 30,
        "release_at_ms": 60030,
        "body": "parsed",
        "files": [
          "src/parse.rs"
        ],
        "report": "report.md",
        "seen_turn": 3,
        "reopens": 0
      },
      {
        "id": "a2",
        "run": "r1",
        "item": "i3",
        "generation": 1,
        "state": "failed",
        "worker": "finished",
        "agent": "codex",
        "place": "new",
        "depth": 1,
        "created_ms": 13,
        "ended_ms": 31,
        "body": "could not",
        "seen_turn": 0,
        "reopens": 0
      },
      {
        "id": "a3",
        "run": "r1",
        "item": "i2",
        "generation": 1,
        "state": "lost",
        "worker": "finished",
        "agent": "codex",
        "place": "new",
        "depth": 1,
        "created_ms": 32,
        "ended_ms": 33,
        "release_at_ms": 533,
        "error": "terminal gone",
        "seen_turn": 0,
        "reopens": 0
      }
    ],
    "unread": 2,
    "mail": 5
  },
  "final_workers": [
    {
      "attempt": {
        "id": "a1",
        "run": "r1",
        "item": "i1",
        "generation": 1,
        "state": "succeeded",
        "worker": "finished",
        "agent": "codex",
        "place": "new",
        "depth": 1,
        "task": "task_1",
        "terminal": "term_w1",
        "brief_path": "brief-a1.md",
        "created_ms": 10,
        "ready_ms": 12,
        "ended_ms": 30,
        "release_at_ms": 60030,
        "body": "parsed",
        "files": [
          "src/parse.rs"
        ],
        "report": "report.md",
        "seen_turn": 3,
        "reopens": 0
      },
      "item_title": "Parse the header",
      "last_output_ms": 0,
      "waiting_answer": false,
      "unread": 2,
      "next": "finished: release in 59 s",
      "next_text": {
        "kind": "next.release_in",
        "params": {
          "secs": "59"
        }
      }
    },
    {
      "attempt": {
        "id": "a2",
        "run": "r1",
        "item": "i3",
        "generation": 1,
        "state": "failed",
        "worker": "finished",
        "agent": "codex",
        "place": "new",
        "depth": 1,
        "created_ms": 13,
        "ended_ms": 31,
        "body": "could not",
        "seen_turn": 0,
        "reopens": 0
      },
      "item_title": "child",
      "last_output_ms": 0,
      "waiting_answer": false,
      "unread": 1,
      "next": "finished",
      "next_text": {
        "kind": "next.finished"
      }
    },
    {
      "attempt": {
        "id": "a3",
        "run": "r1",
        "item": "i2",
        "generation": 1,
        "state": "lost",
        "worker": "finished",
        "agent": "codex",
        "place": "new",
        "depth": 1,
        "created_ms": 32,
        "ended_ms": 33,
        "release_at_ms": 533,
        "error": "terminal gone",
        "seen_turn": 0,
        "reopens": 0
      },
      "item_title": "wire it",
      "last_output_ms": 0,
      "waiting_answer": false,
      "unread": 0,
      "next": "finished: release in 0 s",
      "next_text": {
        "kind": "next.release_in",
        "params": {
          "secs": "0"
        }
      }
    }
  ],
  "final_mail": {
    "messages": [
      {
        "id": "m1",
        "run": "r1",
        "from": "attempt:a1",
        "to": "run:r1",
        "recipients": [
          "run:r1"
        ],
        "type": "question",
        "subject": "which format?",
        "body": "json or toml",
        "priority": "normal",
        "created_ms": 15,
        "read_ms": 20
      },
      {
        "id": "m2",
        "run": "r1",
        "from": "run:r1",
        "to": "attempt:a1",
        "recipients": [
          "attempt:a1"
        ],
        "type": "answer",
        "subject": "json",
        "body": "use json",
        "thread": "m1",
        "reply_to": "m1",
        "priority": "high",
        "created_ms": 16
      },
      {
        "id": "m3",
        "run": "r1",
        "from": "run:r1",
        "to": "@all",
        "recipients": [
          "attempt:a1",
          "attempt:a2"
        ],
        "type": "status",
        "subject": "group  check  (no verdict)",
        "body": "working",
        "payload": {
          "note": null,
          "list": [
            3,
            1,
            2
          ]
        },
        "priority": "normal",
        "created_ms": 17,
        "text": {
          "kind": "group.checked",
          "params": {
            "n": "2"
          }
        },
        "body_text": {
          "kind": "next.working"
        }
      },
      {
        "id": "m4",
        "run": "r1",
        "from": "attempt:a1",
        "to": "run:r1",
        "recipients": [
          "run:r1"
        ],
        "type": "done",
        "subject": "done (succeeded): Parse the header",
        "body": "parsed",
        "payload": {
          "outcome": "succeeded",
          "item": "i1",
          "attempt": "a1",
          "files": [
            "src/parse.rs"
          ],
          "report": "report.md",
          "accepted": true,
          "generation": 1,
          "reopens": 0,
          "release_at_ms": 60030
        },
        "priority": "normal",
        "created_ms": 30,
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a1",
            "item": "i1",
            "outcome": "succeeded",
            "title": "Parse the header"
          }
        }
      },
      {
        "id": "m5",
        "run": "r1",
        "from": "attempt:a2",
        "to": "run:r1",
        "recipients": [
          "run:r1"
        ],
        "type": "done",
        "subject": "done (failed): child",
        "body": "could not",
        "payload": {
          "outcome": "failed",
          "item": "i3",
          "attempt": "a2",
          "files": [],
          "report": null,
          "accepted": true,
          "generation": 1,
          "reopens": 0,
          "release_at_ms": null
        },
        "priority": "normal",
        "created_ms": 31,
        "text": {
          "kind": "done.report",
          "params": {
            "attempt": "a2",
            "item": "i3",
            "outcome": "failed",
            "title": "child"
          }
        }
      }
    ]
  },
  "errors": {
    "create_run_blank": {
      "kind": "InvalidParams",
      "message": "the objective must not be empty"
    },
    "add_item_missing_dep": {
      "kind": "ItemNotFound",
      "message": "no item i9 in run r1"
    },
    "begin_pending": {
      "kind": "DepsNotDone",
      "message": "item i2 waits for its dependencies"
    },
    "verify_wrong_token": {
      "kind": "AttemptFenced",
      "message": "the token does not belong to attempt a1 (POD_ATTEMPT_TOKEN)"
    },
    "send_bad_address": {
      "kind": "InvalidParams",
      "message": "bad address \"bogus\": use run:<id>, attempt:<id>, @all, @idle, @claude or @codex"
    },
    "done_wrong_token": {
      "kind": "AttemptFenced",
      "message": "the token does not belong to attempt a2 (POD_ATTEMPT_TOKEN)"
    }
  },
  "files": {
    "run.json": {
      "version": 1,
      "run": {
        "id": "r1",
        "objective": "ship the parser",
        "coordinator_terminal": "term_c",
        "depth": 0,
        "created_ms": 5
      },
      "items": [
        {
          "id": "i1",
          "run": "r1",
          "title": "Parse the header",
          "spec": "\n# Parse the header\n\nmore detail",
          "state": "completed",
          "generation": 1,
          "start_failures": 0,
          "circuit_open": false,
          "attempt": "a1",
          "created_ms": 6,
          "updated_ms": 30
        },
        {
          "id": "i2",
          "run": "r1",
          "title": "wire it",
          "spec": "connect",
          "deps": [
            "i1"
          ],
          "state": "ready",
          "generation": 1,
          "start_failures": 0,
          "circuit_open": false,
          "attempt": "a3",
          "created_ms": 7,
          "updated_ms": 33
        },
        {
          "id": "i3",
          "run": "r1",
          "title": "child",
          "spec": "a sub task",
          "parent": "i1",
          "state": "failed",
          "generation": 1,
          "start_failures": 0,
          "circuit_open": false,
          "attempt": "a2",
          "created_ms": 8,
          "updated_ms": 31
        }
      ],
      "attempts": [
        {
          "id": "a1",
          "run": "r1",
          "item": "i1",
          "generation": 1,
          "state": "succeeded",
          "worker": "finished",
          "agent": "codex",
          "place": "new",
          "depth": 1,
          "token_hash": "6ab9f1eb8f7d3388f4f9d586f66e99fd54080df2c446f0e58668b09c08a16dd0",
          "task": "task_1",
          "terminal": "term_w1",
          "brief_path": "brief-a1.md",
          "created_ms": 10,
          "ready_ms": 12,
          "ended_ms": 30,
          "release_at_ms": 60030,
          "body": "parsed",
          "files": [
            "src/parse.rs"
          ],
          "report": "report.md",
          "seen_turn": 3,
          "reopens": 0
        },
        {
          "id": "a2",
          "run": "r1",
          "item": "i3",
          "generation": 1,
          "state": "failed",
          "worker": "finished",
          "agent": "codex",
          "place": "new",
          "depth": 1,
          "token_hash": "015f7e6bc5aeaf483724089e9252cc13b50951a6b69412522765cff4d780306e",
          "created_ms": 13,
          "ended_ms": 31,
          "body": "could not",
          "seen_turn": 0,
          "reopens": 0
        },
        {
          "id": "a3",
          "run": "r1",
          "item": "i2",
          "generation": 1,
          "state": "lost",
          "worker": "finished",
          "agent": "codex",
          "place": "new",
          "depth": 1,
          "token_hash": "2f5052c9fd15b19a18c584d01363568198613f0c34e84409ef7938709a159ec2",
          "created_ms": 32,
          "ended_ms": 33,
          "release_at_ms": 533,
          "error": "terminal gone",
          "seen_turn": 0,
          "reopens": 0
        }
      ],
      "wakes": {}
    },
    "mail.jsonl": [
      {
        "kind": "mail",
        "mail": {
          "id": "m1",
          "run": "r1",
          "from": "attempt:a1",
          "to": "run:r1",
          "recipients": [
            "run:r1"
          ],
          "type": "question",
          "subject": "which format?",
          "body": "json or toml",
          "priority": "normal",
          "created_ms": 15
        }
      },
      {
        "kind": "mail",
        "mail": {
          "id": "m2",
          "run": "r1",
          "from": "run:r1",
          "to": "attempt:a1",
          "recipients": [
            "attempt:a1"
          ],
          "type": "answer",
          "subject": "json",
          "body": "use json",
          "thread": "m1",
          "reply_to": "m1",
          "priority": "high",
          "created_ms": 16
        }
      },
      {
        "kind": "mail",
        "mail": {
          "id": "m3",
          "run": "r1",
          "from": "run:r1",
          "to": "@all",
          "recipients": [
            "attempt:a1",
            "attempt:a2"
          ],
          "type": "status",
          "subject": "group  check  (no verdict)",
          "body": "working",
          "payload": {
            "note": null,
            "list": [
              3,
              1,
              2
            ]
          },
          "priority": "normal",
          "created_ms": 17,
          "text": {
            "kind": "group.checked",
            "params": {
              "n": "2"
            }
          },
          "body_text": {
            "kind": "next.working"
          }
        }
      },
      {
        "kind": "read",
        "mailbox": "run:r1",
        "ids": [
          "m1"
        ],
        "ms": 20
      },
      {
        "kind": "mail",
        "mail": {
          "id": "m4",
          "run": "r1",
          "from": "attempt:a1",
          "to": "run:r1",
          "recipients": [
            "run:r1"
          ],
          "type": "done",
          "subject": "done (succeeded): Parse the header",
          "body": "parsed",
          "payload": {
            "outcome": "succeeded",
            "item": "i1",
            "attempt": "a1",
            "files": [
              "src/parse.rs"
            ],
            "report": "report.md",
            "accepted": true,
            "generation": 1,
            "reopens": 0,
            "release_at_ms": 60030
          },
          "priority": "normal",
          "created_ms": 30,
          "text": {
            "kind": "done.report",
            "params": {
              "attempt": "a1",
              "item": "i1",
              "outcome": "succeeded",
              "title": "Parse the header"
            }
          }
        }
      },
      {
        "kind": "mail",
        "mail": {
          "id": "m5",
          "run": "r1",
          "from": "attempt:a2",
          "to": "run:r1",
          "recipients": [
            "run:r1"
          ],
          "type": "done",
          "subject": "done (failed): child",
          "body": "could not",
          "payload": {
            "outcome": "failed",
            "item": "i3",
            "attempt": "a2",
            "files": [],
            "report": null,
            "accepted": true,
            "generation": 1,
            "reopens": 0,
            "release_at_ms": null
          },
          "priority": "normal",
          "created_ms": 31,
          "text": {
            "kind": "done.report",
            "params": {
              "attempt": "a2",
              "item": "i3",
              "outcome": "failed",
              "title": "child"
            }
          }
        }
      }
    ],
    "index.json": {
      "counters": {
        "a": 3,
        "i": 3,
        "m": 5,
        "r": 1
      },
      "requests": []
    }
  }
}"##;

const GOLDEN_PR: &str = r##"{
  "open_known": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "remote": {
        "name": "origin",
        "url": "https://***@github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "pr": {
        "number": 14964,
        "url": "https://github.com/cli/cli/pull/14964",
        "title": "chore(deps): Bump the codeql-actions group with 3 updates",
        "state": "open",
        "draft": false,
        "head": "dependabot/github_actions/codeql-actions-92276e7fb0",
        "base": "trunk",
        "review_decision": "review_required",
        "merge_state": "BLOCKED",
        "mergeable": "MERGEABLE",
        "additions": 4,
        "deletions": 4,
        "changed_files": 2,
        "author": "app/dependabot"
      },
      "checks": [
        {
          "name": "CodeQL",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/runs/127935203732",
          "started_at": "2026-11-06T14:03:55Z",
          "completed_at": "2026-11-06T14:03:59Z"
        },
        {
          "name": "label-external / label_issues",
          "workflow": "PR Triaging",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230516639/job/127934910476",
          "started_at": "2026-11-06T14:03:32Z",
          "completed_at": "2026-11-06T14:03:35Z"
        },
        {
          "name": "ready-for-review / ready-for-review",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230521101/job/127935001240",
          "started_at": "2026-11-06T14:03:31Z",
          "completed_at": "2026-11-06T14:03:21Z"
        },
        {
          "name": "close-unmet-requirements",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230522203/job/127935005940",
          "started_at": "2026-11-06T14:03:31Z",
          "completed_at": "2026-11-06T14:03:21Z"
        },
        {
          "name": "close-from-default-branch",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230521101/job/127935002225",
          "started_at": "2026-11-06T14:03:31Z",
          "completed_at": "2026-11-06T14:03:21Z"
        },
        {
          "name": "close-no-help-wanted",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230516639/job/127934987630",
          "started_at": "2026-11-06T14:03:29Z",
          "completed_at": "2026-11-06T14:03:19Z"
        },
        {
          "name": "check-requirements",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230522203/job/127934979869",
          "started_at": "2026-11-06T14:03:28Z",
          "completed_at": "2026-11-06T14:03:21Z"
        },
        {
          "name": "close-no-help-wanted / close-no-help-wanted",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230522203/job/127934985119",
          "started_at": "2026-11-06T14:03:28Z",
          "completed_at": "2026-11-06T14:03:21Z"
        },
        {
          "name": "ready-for-review",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230516639/job/127934972277",
          "started_at": "2026-11-06T14:03:27Z",
          "completed_at": "2026-11-06T14:03:19Z"
        },
        {
          "name": "build (macos-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230519488/job/127934916710",
          "started_at": "2026-11-06T14:03:23Z",
          "completed_at": "2026-11-06T14:07:26Z"
        },
        {
          "name": "integration-tests (ubuntu-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230519488/job/127934916736",
          "started_at": "2026-11-06T14:03:23Z",
          "completed_at": "2026-11-06T14:04:17Z"
        },
        {
          "name": "integration-tests (windows-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230519488/job/127934916681",
          "started_at": "2026-11-06T14:03:23Z",
          "completed_at": "2026-11-06T14:05:26Z"
        },
        {
          "name": "integration-tests (macos-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230519488/job/127934916582",
          "started_at": "2026-11-06T14:03:23Z",
          "completed_at": "2026-11-06T14:04:44Z"
        },
        {
          "name": "CodeQL-Build (go, manual, ./.github/codeql/codeql-config.yml)",
          "workflow": "Code Scanning",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230519580/job/127934916796",
          "started_at": "2026-11-06T14:03:23Z",
          "completed_at": "2026-11-06T14:06:40Z"
        },
        {
          "name": "CodeQL-Build (actions, none, security-and-quality)",
          "workflow": "Code Scanning",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230519580/job/127934917064",
          "started_at": "2026-11-06T14:03:23Z",
          "completed_at": "2026-11-06T14:04:05Z"
        },
        {
          "name": "build (windows-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230519488/job/127934916392",
          "started_at": "2026-11-06T14:03:22Z",
          "completed_at": "2026-11-06T14:09:31Z"
        },
        {
          "name": "build (ubuntu-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42230519488/job/127934916165",
          "started_at": "2026-11-06T14:03:22Z",
          "completed_at": "2026-11-06T14:06:11Z"
        },
        {
          "name": "label-external",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230522203/job/127934930325",
          "started_at": "2026-11-06T14:03:22Z",
          "completed_at": "2026-11-06T14:03:21Z"
        },
        {
          "name": "check-requirements / close-unmet-requirements",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230516639/job/127934913403",
          "started_at": "2026-11-06T14:03:19Z",
          "completed_at": "2026-11-06T14:03:19Z"
        },
        {
          "name": "check-requirements / check-requirements",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230516639/job/127934912723",
          "started_at": "2026-11-06T14:03:19Z",
          "completed_at": "2026-11-06T14:03:19Z"
        },
        {
          "name": "close-from-default-branch / close-from-default-branch",
          "workflow": "PR Triaging",
          "bucket": "skipping",
          "state": "SKIPPED",
          "link": "https://github.com/cli/cli/actions/runs/42230516639/job/127934912634",
          "started_at": "2026-11-06T14:03:19Z",
          "completed_at": "2026-11-06T14:03:19Z"
        }
      ],
      "summary": {
        "total": 21,
        "passed": 10,
        "failed": 0,
        "pending": 0,
        "skipped": 11
      },
      "comments": [
        {
          "id": 4499039797,
          "path": ".github/scripts/issue-triage-report.cjs",
          "line": 314,
          "body": "🛑 Requirement: Validate each fixture URL independently\n\nThe whitespace-only boundary includes unrelated content in the fixture pathname, so valid captured output such as `[\"https://github.com/example.invalid/hello/pull/1\",\"feature%2Fbranch\"]` blocks publication even though the URL itself passes validation. Adjacent Markdown links can trigger the same false rejection when the second link contains `%2F`. Extract individual URL spans before validation, preserving the owner-escape checks and captured output, and add regression cases for compact JSON and adjacent Markdown links.",
          "author": "Copilot",
          "url": "https://github.com/cli/cli/pull/14976#discussion_r4499039797",
          "created_at": "2026-11-07T00:23:10Z"
        },
        {
          "id": 4499417847,
          "path": ".github/scripts/issue-triage-report.cjs",
          "line": 314,
          "body": "Fixed in [6ceb78a](https://github.com/cli/cli/commit/6ceb78a575d98dbe06ac366e42501c67aee2b1c4). Both examples are covered in the existing publication table, with owner-escape checks retained.",
          "author": "BagToad",
          "url": "https://github.com/cli/cli/pull/14976#discussion_r4499417847",
          "created_at": "2026-11-07T01:42:20Z"
        }
      ],
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      },
      {
        "line": "gh pr view 14964 --repo cli/cli --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh pr checks 14964 --repo cli/cli --json bucket,completedAt,description,event,link,name,startedAt,state,workflow",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh api repos/cli/cli/pulls/14964/comments --paginate",
        "stdin": null,
        "timeout_ms": 30000
      }
    ]
  },
  "open_by_branch": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "remote": {
        "name": "origin",
        "url": "https://***@github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "pr": {
        "number": 14964,
        "url": "https://github.com/cli/cli/pull/14964",
        "title": "chore(deps): Bump the codeql-actions group with 3 updates",
        "state": "open",
        "draft": false,
        "head": "dependabot/github_actions/codeql-actions-92276e7fb0",
        "base": "trunk",
        "review_decision": "review_required",
        "merge_state": "BLOCKED",
        "mergeable": "MERGEABLE",
        "additions": 4,
        "deletions": 4,
        "changed_files": 2,
        "author": "app/dependabot"
      },
      "summary": {
        "total": 0,
        "passed": 0,
        "failed": 0,
        "pending": 0,
        "skipped": 0
      },
      "error": {
        "kind": "failed",
        "message": "unexpected pr checks output from gh: EOF while parsing a value at line 1 column 0"
      },
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      },
      {
        "line": "gh pr list --repo cli/cli --head dependabot/github_actions/codeql-actions-92276e7fb0 --state all --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt --limit 20",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh pr checks 14964 --repo cli/cli --json bucket,completedAt,description,event,link,name,startedAt,state,workflow",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh api repos/cli/cli/pulls/14964/comments --paginate",
        "stdin": null,
        "timeout_ms": 30000
      }
    ]
  },
  "merged": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "remote": {
        "name": "origin",
        "url": "https://***@github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "pr": {
        "number": 14976,
        "url": "https://github.com/cli/cli/pull/14976",
        "title": "Use a synthetic namespace for triage fixtures",
        "state": "merged",
        "draft": false,
        "head": "bagtoad/triage-ghmock-skill",
        "base": "trunk",
        "review_decision": "approved",
        "merge_state": "UNKNOWN",
        "mergeable": "UNKNOWN",
        "additions": 261,
        "deletions": 18,
        "changed_files": 10,
        "author": "BagToad",
        "merged_at": "2026-11-07T01:48:47Z",
        "closed_at": "2026-11-07T01:48:47Z"
      },
      "checks": [
        {
          "name": "CodeQL",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/runs/128259773225",
          "started_at": "2026-11-07T01:42:41Z",
          "completed_at": "2026-11-07T01:42:44Z"
        },
        {
          "name": "build (windows-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599299",
          "started_at": "2026-11-07T01:42:08Z",
          "completed_at": "2026-11-07T01:48:05Z"
        },
        {
          "name": "build (macos-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599597",
          "started_at": "2026-11-07T01:42:08Z",
          "completed_at": "2026-11-07T01:46:31Z"
        },
        {
          "name": "integration-tests (macos-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599542",
          "started_at": "2026-11-07T01:42:08Z",
          "completed_at": "2026-11-07T01:43:46Z"
        },
        {
          "name": "integration-tests (windows-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599554",
          "started_at": "2026-11-07T01:42:07Z",
          "completed_at": "2026-11-07T01:43:49Z"
        },
        {
          "name": "CodeQL-Build (go, manual, ./.github/codeql/codeql-config.yml)",
          "workflow": "Code Scanning",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466453/job/128259599242",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:07Z"
        },
        {
          "name": "lint",
          "workflow": "Lint",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466445/job/128259599765",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:36Z"
        },
        {
          "name": "build (ubuntu-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599513",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:49Z"
        },
        {
          "name": "CodeQL-Build (actions, none, security-and-quality)",
          "workflow": "Code Scanning",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466453/job/128259599567",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:42:51Z"
        },
        {
          "name": "govulncheck",
          "workflow": "Lint",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466445/job/128259599355",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:42:34Z"
        },
        {
          "name": "integration-tests (ubuntu-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599389",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:42:53Z"
        }
      ],
      "summary": {
        "total": 11,
        "passed": 11,
        "failed": 0,
        "pending": 0,
        "skipped": 0
      },
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      },
      {
        "line": "gh pr view 14964 --repo cli/cli --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh pr list --repo cli/cli --head dependabot/github_actions/codeql-actions-92276e7fb0 --state all --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt --limit 20",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh pr checks 14976 --repo cli/cli --json bucket,completedAt,description,event,link,name,startedAt,state,workflow",
        "stdin": null,
        "timeout_ms": 30000
      }
    ]
  },
  "checks_failed": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "remote": {
        "name": "origin",
        "url": "https://***@github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "pr": {
        "number": 14964,
        "url": "https://github.com/cli/cli/pull/14964",
        "title": "chore(deps): Bump the codeql-actions group with 3 updates",
        "state": "open",
        "draft": false,
        "head": "dependabot/github_actions/codeql-actions-92276e7fb0",
        "base": "trunk",
        "review_decision": "review_required",
        "merge_state": "BLOCKED",
        "mergeable": "MERGEABLE",
        "additions": 4,
        "deletions": 4,
        "changed_files": 2,
        "author": "app/dependabot"
      },
      "summary": {
        "total": 0,
        "passed": 0,
        "failed": 0,
        "pending": 0,
        "skipped": 0
      },
      "error": {
        "kind": "failed",
        "message": "[{\"bucket\":\"skipping\",\"completedAt\":\"2026-11-03T21:29:08Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41722208901/job/126217723786\",\"name\":\"check-requirements / close-unmet-requirements\",\"startedAt\":\"2026-11-03T21:29:17Z\",\"state\":\"SKIPPED\",\"workflow\":\"PR Triaging\"},{\"bucket\":\"skipping\",\"completedAt\":\"2026-11-03T21:29:08Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41722208901/job/126217716143\",\"name\":\"ready-for-review\",\"startedAt\":\"2026-11-03T21:29:16Z\",\"state\":\"SKIPPED\",\"workflow\":\"PR Triaging\"},{\"bucket\":\"skipping\",\"completedAt\":\"2026-11-03T21:29:08Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41722208901/job/126217709553\",\"name\":\"check-requirements / check-requirements\",\"startedAt\":\"2026-11-03T21:29:15Z\",\"state\":\"SKIPPED\",\"workflow\":\"PR Triaging\"},{\"bucket\":\"skipping\",\"completedAt\":\"2026-11-03T21:29:08Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41722208901/job/126217704899\",\"name\":\"close-unmet-requirements\",\"startedAt\":\"2026-11-03T21:29:14Z\",\"state\":\"SKIPPED\",\"workflow\":\"PR Triaging\"},{\"bucket\":\"skipping\",\"completedAt\":\"2026-11-03T21:29:08Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41722208901/job/126217657403\",\"name\":\"label-external\",\"startedAt\":\"2026-11-03T21:29:09Z\",\"state\":\"SKIPPED\",\"workflow\":\"PR Triaging\"},{\"bucket\":\"skipping\",\"completedAt\":\"2026-11-03T21:29:08Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41722208901/job/126217658515\",\"name\":\"close-from-default-branch\",\"startedAt\":\"2026-11-03T21:29:09Z\",\"state\":\"SKIPPED\",\"workflow\":\"PR Triaging\"},{\"bucket\":\"skipping\",\"completedAt\":\"2026-11-03T21:29:08Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41722208901/job/126217658068\",\"name\":\"close-no-help-wanted\",\"startedAt\":\"2026-11-03T21:29:09Z\",\"state\":\"SKIPPED\",\"workflow\":\"PR Triaging\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:22:11Z\",\"description\":\"\",\"event\":\"\",\"link\":\"https://github.com/cli/cli/runs/126213975170\",\"name\":\"CodeQL\",\"startedAt\":\"2026-11-03T21:22:08Z\",\"state\":\"SUCCESS\",\"workflow\":\"\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:23:31Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41721000240/job/126213408501\",\"name\":\"CodeQL-Build (go, manual, ./.github/codeql/codeql-config.yml)\",\"startedAt\":\"2026-11-03T21:21:08Z\",\"state\":\"SUCCESS\",\"workflow\":\"Code Scanning\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:22:18Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41721000240/job/126213408141\",\"name\":\"CodeQL-Build (actions, none, security-and-quality)\",\"startedAt\":\"2026-11-03T21:21:07Z\",\"state\":\"SUCCESS\",\"workflow\":\"Code Scanning\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:28:23Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41720844033/job/126212877913\",\"name\":\"build (macos-latest)\",\"startedAt\":\"2026-11-03T21:20:12Z\",\"state\":\"SUCCESS\",\"workflow\":\"Unit and Integration Tests\"},{\"bucket\":\"fail\",\"completedAt\":\"2026-11-03T21:27:04Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41720844033/job/126212878432\",\"name\":\"build (windows-latest)\",\"startedAt\":\"2026-11-03T21:20:11Z\",\"state\":\"FAILURE\",\"workflow\":\"Unit and Integration Tests\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:24:48Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41720844033/job/126212878347\",\"name\":\"build (ubuntu-latest)\",\"startedAt\":\"2026-11-03T21:20:09Z\",\"state\":\"SUCCESS\",\"workflow\":\"Unit and Integration Tests\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:21:16Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41720844033/job/126212878283\",\"name\":\"integration-tests (ubuntu-latest)\",\"startedAt\":\"2026-11-03T21:20:09Z\",\"state\":\"SUCCESS\",\"workflow\":\"Unit and Integration Tests\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:20:13Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41720845088/job/126212885818\",\"name\":\"label-external / label_issues\",\"startedAt\":\"2026-11-03T21:20:09Z\",\"state\":\"SUCCESS\",\"workflow\":\"PR Triaging\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:22:59Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41720843855/job/126212878043\",\"name\":\"lint\",\"startedAt\":\"2026-11-03T21:20:09Z\",\"state\":\"SUCCESS\",\"workflow\":\"Lint\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:22:44Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41720844033/job/126212878343\",\"name\":\"integration-tests (windows-latest)\",\"startedAt\":\"2026-11-03T21:20:08Z\",\"state\":\"SUCCESS\",\"workflow\":\"Unit and Integration Tests\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:21:31Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41720844033/job/126212878267\",\"name\":\"integration-tests (macos-latest)\",\"startedAt\":\"2026-11-03T21:20:08Z\",\"state\":\"SUCCESS\",\"workflow\":\"Unit and Integration Tests\"},{\"bucket\":\"pass\",\"completedAt\":\"2026-11-03T21:21:05Z\",\"description\":\"\",\"event\":\"pull_request\",\"link\":\"https://github.com/cli/cli/actions/runs/41720843855/job/126212877473\",\"name\":\"govulncheck\",\"startedAt\":\"2026-11-03T21:20:08Z\",\"state\":\"SUCCESS\",\"workflow\":\"Lint\"},{\"bucket\":\"skipping\",\"completedAt\":\"2026-11-03T21:20:06Z\",\"description\":\"\",\"event\":\"pull_request_target\",\"link\":\"https://github.com/cli/cli/actions/runs/41720845088/job/126212888648\",\"name\":\"close-from-default-branch / close-from-default-branch\",\"startedAt\":\"2026-11-03T21:20:07Z\",\"state\":\"SKIPPED\",\"workflow\":\"PR Triaging\"}] …"
      },
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      },
      {
        "line": "gh pr list --repo cli/cli --head dependabot/github_actions/codeql-actions-92276e7fb0 --state all --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt --limit 20",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh pr checks 14964 --repo cli/cli --json bucket,completedAt,description,event,link,name,startedAt,state,workflow",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh api repos/cli/cli/pulls/14964/comments --paginate",
        "stdin": null,
        "timeout_ms": 30000
      }
    ]
  },
  "no_pr": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "remote": {
        "name": "origin",
        "url": "https://***@github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "summary": {
        "total": 0,
        "passed": 0,
        "failed": 0,
        "pending": 0,
        "skipped": 0
      },
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      },
      {
        "line": "gh pr list --repo cli/cli --head dependabot/github_actions/codeql-actions-92276e7fb0 --state all --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt --limit 20",
        "stdin": null,
        "timeout_ms": 30000
      }
    ]
  },
  "no_remote": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "summary": {
        "total": 0,
        "passed": 0,
        "failed": 0,
        "pending": 0,
        "skipped": 0
      },
      "error": {
        "kind": "no_remote",
        "message": "this repository has no git remote"
      },
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      }
    ]
  },
  "unsupported_host": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "summary": {
        "total": 0,
        "passed": 0,
        "failed": 0,
        "pending": 0,
        "skipped": 0
      },
      "error": {
        "kind": "unsupported_host",
        "message": "no remote points to a GitHub repository: origin (https://gitlab.com/x/y.git)"
      },
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      }
    ]
  },
  "not_logged_in": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "remote": {
        "name": "origin",
        "url": "https://***@github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "summary": {
        "total": 0,
        "passed": 0,
        "failed": 0,
        "pending": 0,
        "skipped": 0
      },
      "error": {
        "kind": "not_logged_in",
        "message": "To get started with GitHub CLI, please run:  gh auth login"
      },
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      },
      {
        "line": "gh pr list --repo cli/cli --head dependabot/github_actions/codeql-actions-92276e7fb0 --state all --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt --limit 20",
        "stdin": null,
        "timeout_ms": 30000
      }
    ]
  },
  "gh_missing": {
    "info": {
      "task": "demo/t1",
      "branch": "dependabot/github_actions/codeql-actions-92276e7fb0",
      "default_base": "main",
      "ahead": 2,
      "unpushed": 0,
      "dirty": 1,
      "remote": {
        "name": "origin",
        "url": "https://***@github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "summary": {
        "total": 0,
        "passed": 0,
        "failed": 0,
        "pending": 0,
        "skipped": 0
      },
      "error": {
        "kind": "tool_missing",
        "message": "gh is not installed"
      },
      "cached": false,
      "refreshed_ms": 1000
    },
    "commands": [
      {
        "line": "git remote -v",
        "stdin": null,
        "timeout_ms": 20000
      },
      {
        "line": "gh pr list --repo cli/cli --head dependabot/github_actions/codeql-actions-92276e7fb0 --state all --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt --limit 20",
        "stdin": null,
        "timeout_ms": 30000
      }
    ]
  },
  "gh_writes": {
    "push": {
      "Ok": null
    },
    "create": {
      "Ok": {
        "number": 14964,
        "url": "https://github.com/cli/cli/pull/14964",
        "title": "chore(deps): Bump the codeql-actions group with 3 updates",
        "state": "open",
        "draft": false,
        "head": "dependabot/github_actions/codeql-actions-92276e7fb0",
        "base": "trunk",
        "review_decision": "review_required",
        "merge_state": "BLOCKED",
        "mergeable": "MERGEABLE",
        "additions": 4,
        "deletions": 4,
        "changed_files": 2,
        "author": "app/dependabot"
      }
    },
    "merge": {
      "Ok": {
        "number": 14976,
        "url": "https://github.com/cli/cli/pull/14976",
        "title": "Use a synthetic namespace for triage fixtures",
        "state": "merged",
        "draft": false,
        "head": "bagtoad/triage-ghmock-skill",
        "base": "trunk",
        "review_decision": "approved",
        "merge_state": "UNKNOWN",
        "mergeable": "UNKNOWN",
        "additions": 261,
        "deletions": 18,
        "changed_files": 10,
        "author": "BagToad",
        "merged_at": "2026-11-07T01:48:47Z",
        "closed_at": "2026-11-07T01:48:47Z"
      }
    },
    "close": {
      "Err": {
        "kind": "not_found",
        "message": "GraphQL: Could not resolve to a PullRequest"
      }
    },
    "commands": [
      {
        "line": "git push --porcelain --set-upstream origin HEAD:refs/heads/pod/x",
        "stdin": null,
        "timeout_ms": 180000
      },
      {
        "line": "git branch --set-upstream-to origin/pod/x pod/x",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh pr create --repo cli/cli --head pod/x --base main --title 제목 \"quoted\" --body-file - --draft",
        "stdin": "본문\n- a\n",
        "timeout_ms": 60000
      },
      {
        "line": "gh pr view 14964 --repo cli/cli --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh pr merge 7 --repo cli/cli --squash",
        "stdin": null,
        "timeout_ms": 60000
      },
      {
        "line": "gh pr view 7 --repo cli/cli --json number,url,title,state,isDraft,headRefName,headRefOid,baseRefName,reviewDecision,mergeStateStatus,mergeable,additions,deletions,changedFiles,author,mergedAt,closedAt",
        "stdin": null,
        "timeout_ms": 30000
      },
      {
        "line": "gh pr close 9 --repo cli/cli",
        "stdin": null,
        "timeout_ms": 60000
      }
    ]
  },
  "classify": [
    {
      "kind": "not_logged_in",
      "message": "To get started with GitHub CLI, please run:  gh auth login"
    },
    {
      "kind": "not_found",
      "message": "HTTP 404: Not Found (https://api.github.com/repos/o/r/pulls/9)"
    },
    {
      "kind": "rate_limited",
      "message": "API rate limit exceeded for user ID 1."
    },
    {
      "kind": "network",
      "message": "error connecting to api.github.com\ncheck your internet connection"
    },
    {
      "kind": "already_exists",
      "message": "a pull request for branch \"pod/x\" into branch \"main\" already exists:\nhttps://github.com/o/r/pull/3"
    },
    {
      "kind": "no_permission",
      "message": "HTTP 403: Must have admin rights to Repository."
    },
    {
      "kind": "rejected",
      "message": "! [rejected]        pod/x -> pod/x (non-fast-forward)\nerror: failed to push some refs to 'https://***@github.com/o/r.git'"
    },
    {
      "kind": "network",
      "message": "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com"
    },
    {
      "kind": "failed",
      "message": "gh exited with code ?"
    }
  ],
  "fake": {
    "create_unpushed": {
      "Err": {
        "kind": "failed",
        "message": "pull request create failed: GraphQL: Head sha can't be blank, Base sha can't be blank, No commits between main and pod/x, Head ref must be a branch (createPullRequest)"
      }
    },
    "created": {
      "number": 1,
      "url": "https://github.com/cli/cli/pull/1",
      "title": "제목 \"quoted\"",
      "state": "open",
      "draft": true,
      "head": "pod/x",
      "base": "main",
      "review_decision": "review_required",
      "merge_state": "BLOCKED",
      "mergeable": "MERGEABLE",
      "additions": 0,
      "deletions": 0,
      "changed_files": 0,
      "author": "pod-fixture"
    },
    "merge_draft": {
      "Err": {
        "kind": "failed",
        "message": "Pull request #1 is still a draft"
      }
    },
    "merged": {
      "Ok": {
        "number": 1,
        "url": "https://github.com/cli/cli/pull/1",
        "title": "제목 \"quoted\"",
        "state": "merged",
        "draft": false,
        "head": "pod/x",
        "base": "main",
        "review_decision": "review_required",
        "merge_state": "UNKNOWN",
        "mergeable": "UNKNOWN",
        "additions": 0,
        "deletions": 0,
        "changed_files": 0,
        "author": "pod-fixture",
        "merged_at": "<now>",
        "closed_at": "<now>"
      }
    },
    "merge_again": {
      "Err": {
        "kind": "failed",
        "message": "Pull request #1 was already merged"
      }
    },
    "close_injected": {
      "Err": {
        "kind": "no_permission",
        "message": "no permission"
      }
    },
    "status": {
      "remote": {
        "name": "origin",
        "url": "https://github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "pr": {
        "number": 1,
        "url": "https://github.com/cli/cli/pull/1",
        "title": "제목 \"quoted\"",
        "state": "merged",
        "draft": false,
        "head": "pod/x",
        "base": "main",
        "review_decision": "review_required",
        "merge_state": "UNKNOWN",
        "mergeable": "UNKNOWN",
        "additions": 0,
        "deletions": 0,
        "changed_files": 0,
        "author": "pod-fixture",
        "merged_at": "<now>",
        "closed_at": "<now>"
      },
      "checks": [
        {
          "name": "CodeQL",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/runs/128259773225",
          "started_at": "2026-11-07T01:42:41Z",
          "completed_at": "2026-11-07T01:42:44Z"
        },
        {
          "name": "build (windows-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599299",
          "started_at": "2026-11-07T01:42:08Z",
          "completed_at": "2026-11-07T01:48:05Z"
        },
        {
          "name": "build (macos-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599597",
          "started_at": "2026-11-07T01:42:08Z",
          "completed_at": "2026-11-07T01:46:31Z"
        },
        {
          "name": "integration-tests (macos-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pending",
          "state": "IN_PROGRESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599542",
          "started_at": "2026-11-07T01:42:08Z"
        },
        {
          "name": "integration-tests (windows-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pending",
          "state": "IN_PROGRESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599554",
          "started_at": "2026-11-07T01:42:07Z"
        },
        {
          "name": "CodeQL-Build (go, manual, ./.github/codeql/codeql-config.yml)",
          "workflow": "Code Scanning",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466453/job/128259599242",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:07Z"
        },
        {
          "name": "lint",
          "workflow": "Lint",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466445/job/128259599765",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:36Z"
        },
        {
          "name": "build (ubuntu-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599513",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:49Z"
        },
        {
          "name": "CodeQL-Build (actions, none, security-and-quality)",
          "workflow": "Code Scanning",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466453/job/128259599567",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:42:51Z"
        },
        {
          "name": "govulncheck",
          "workflow": "Lint",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466445/job/128259599355",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:42:34Z"
        },
        {
          "name": "integration-tests (ubuntu-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pending",
          "state": "IN_PROGRESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599389",
          "started_at": "2026-11-07T01:42:06Z"
        }
      ],
      "summary": {
        "total": 11,
        "passed": 8,
        "failed": 0,
        "pending": 3,
        "skipped": 0
      }
    },
    "state_file": {
      "remote": {
        "name": "origin",
        "url": "https://github.com/cli/cli.git",
        "slug": {
          "host": "github.com",
          "owner": "cli",
          "name": "cli"
        }
      },
      "no_remote": false,
      "next_number": 1,
      "prs": [
        {
          "view": {
            "number": 1,
            "url": "https://github.com/cli/cli/pull/1",
            "title": "제목 \"quoted\"",
            "state": "merged",
            "draft": false,
            "head": "pod/x",
            "base": "main",
            "review_decision": "review_required",
            "merge_state": "UNKNOWN",
            "mergeable": "UNKNOWN",
            "additions": 0,
            "deletions": 0,
            "changed_files": 0,
            "author": "pod-fixture",
            "merged_at": "<now>",
            "closed_at": "<now>"
          },
          "body": "본문\n- a\n",
          "checks": [
            {
              "name": "CodeQL",
              "bucket": "pass",
              "state": "SUCCESS",
              "link": "https://github.com/cli/cli/runs/128259773225",
              "started_at": "2026-11-07T01:42:41Z",
              "completed_at": "2026-11-07T01:42:44Z"
            },
            {
              "name": "build (windows-latest)",
              "workflow": "Unit and Integration Tests",
              "bucket": "pass",
              "state": "SUCCESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599299",
              "started_at": "2026-11-07T01:42:08Z",
              "completed_at": "2026-11-07T01:48:05Z"
            },
            {
              "name": "build (macos-latest)",
              "workflow": "Unit and Integration Tests",
              "bucket": "pass",
              "state": "SUCCESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599597",
              "started_at": "2026-11-07T01:42:08Z",
              "completed_at": "2026-11-07T01:46:31Z"
            },
            {
              "name": "integration-tests (macos-latest)",
              "workflow": "Unit and Integration Tests",
              "bucket": "pending",
              "state": "IN_PROGRESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599542",
              "started_at": "2026-11-07T01:42:08Z"
            },
            {
              "name": "integration-tests (windows-latest)",
              "workflow": "Unit and Integration Tests",
              "bucket": "pending",
              "state": "IN_PROGRESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599554",
              "started_at": "2026-11-07T01:42:07Z"
            },
            {
              "name": "CodeQL-Build (go, manual, ./.github/codeql/codeql-config.yml)",
              "workflow": "Code Scanning",
              "bucket": "pass",
              "state": "SUCCESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466453/job/128259599242",
              "started_at": "2026-11-07T01:42:06Z",
              "completed_at": "2026-11-07T01:44:07Z"
            },
            {
              "name": "lint",
              "workflow": "Lint",
              "bucket": "pass",
              "state": "SUCCESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466445/job/128259599765",
              "started_at": "2026-11-07T01:42:06Z",
              "completed_at": "2026-11-07T01:44:36Z"
            },
            {
              "name": "build (ubuntu-latest)",
              "workflow": "Unit and Integration Tests",
              "bucket": "pass",
              "state": "SUCCESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599513",
              "started_at": "2026-11-07T01:42:06Z",
              "completed_at": "2026-11-07T01:44:49Z"
            },
            {
              "name": "CodeQL-Build (actions, none, security-and-quality)",
              "workflow": "Code Scanning",
              "bucket": "pass",
              "state": "SUCCESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466453/job/128259599567",
              "started_at": "2026-11-07T01:42:06Z",
              "completed_at": "2026-11-07T01:42:51Z"
            },
            {
              "name": "govulncheck",
              "workflow": "Lint",
              "bucket": "pass",
              "state": "SUCCESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466445/job/128259599355",
              "started_at": "2026-11-07T01:42:06Z",
              "completed_at": "2026-11-07T01:42:34Z"
            },
            {
              "name": "integration-tests (ubuntu-latest)",
              "workflow": "Unit and Integration Tests",
              "bucket": "pending",
              "state": "IN_PROGRESS",
              "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599389",
              "started_at": "2026-11-07T01:42:06Z"
            }
          ],
          "comments": [],
          "merged_with": "squash"
        }
      ],
      "pushes": [
        {
          "remote": "origin",
          "branch": "pod/x"
        }
      ],
      "new_checks": [
        {
          "name": "CodeQL",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/runs/128259773225",
          "started_at": "2026-11-07T01:42:41Z",
          "completed_at": "2026-11-07T01:42:44Z"
        },
        {
          "name": "build (windows-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599299",
          "started_at": "2026-11-07T01:42:08Z",
          "completed_at": "2026-11-07T01:48:05Z"
        },
        {
          "name": "build (macos-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599597",
          "started_at": "2026-11-07T01:42:08Z",
          "completed_at": "2026-11-07T01:46:31Z"
        },
        {
          "name": "integration-tests (macos-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599542",
          "started_at": "2026-11-07T01:42:08Z",
          "completed_at": "2026-11-07T01:43:46Z"
        },
        {
          "name": "integration-tests (windows-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599554",
          "started_at": "2026-11-07T01:42:07Z",
          "completed_at": "2026-11-07T01:43:49Z"
        },
        {
          "name": "CodeQL-Build (go, manual, ./.github/codeql/codeql-config.yml)",
          "workflow": "Code Scanning",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466453/job/128259599242",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:07Z"
        },
        {
          "name": "lint",
          "workflow": "Lint",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466445/job/128259599765",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:36Z"
        },
        {
          "name": "build (ubuntu-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599513",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:44:49Z"
        },
        {
          "name": "CodeQL-Build (actions, none, security-and-quality)",
          "workflow": "Code Scanning",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466453/job/128259599567",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:42:51Z"
        },
        {
          "name": "govulncheck",
          "workflow": "Lint",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466445/job/128259599355",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:42:34Z"
        },
        {
          "name": "integration-tests (ubuntu-latest)",
          "workflow": "Unit and Integration Tests",
          "bucket": "pass",
          "state": "SUCCESS",
          "link": "https://github.com/cli/cli/actions/runs/42325466444/job/128259599389",
          "started_at": "2026-11-07T01:42:06Z",
          "completed_at": "2026-11-07T01:42:53Z"
        }
      ],
      "fail": {
        "close": {
          "kind": "no_permission",
          "message": "no permission"
        }
      },
      "calls": [
        "create pod/x -> main draft",
        "push origin pod/x",
        "create pod/x -> main draft",
        "merge 1 --merge",
        "merge 1 --squash",
        "merge 1 --squash",
        "close 1",
        "detect",
        "view 1",
        "checks 1"
      ]
    }
  }
}"##;

const GOLDEN_DISCOVERY: &str = r##"{
  "sessions": [
    {
      "agent": "codex",
      "meta": {
        "id": "dddddddd-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "cwd": "D:\\work",
        "model": "m",
        "started_ms": 1793750400000,
        "last_ms": 1793750401000,
        "messages": 0
      },
      "path": "codex/sessions/2026/11/04/rollout-2026-11-04T10-00-00-dddddddd-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl",
      "mtime_ms": 1767225600000,
      "match": {}
    },
    {
      "agent": "codex",
      "meta": {
        "id": "bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "cwd": "C:\\src\\other",
        "preview": "Refactor the parser",
        "model": "gpt-new",
        "branch": "feat/x",
        "version": "0.167.0",
        "origin": "vscode",
        "started_ms": 1793664000000,
        "last_ms": 1793664009000,
        "messages": 2
      },
      "path": "codex/sessions/2026/11/03/rollout-2026-11-03T09-00-00-bbbbbbbb-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl",
      "mtime_ms": 1767225600000,
      "match": {
        "repo": "other"
      }
    },
    {
      "agent": "codex",
      "meta": {
        "id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        "cwd": "C:\\src\\demo",
        "title": "Dark mode",
        "preview": "Add a dark mode toggle",
        "model": "gpt-test",
        "branch": "main",
        "version": "0.159.0",
        "origin": "cli",
        "started_ms": 1793577600000,
        "last_ms": 1793578200000,
        "messages": 3
      },
      "path": "codex/sessions/2026/11/02/rollout-2026-11-02T09-00-00-aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.jsonl",
      "mtime_ms": 1767225600000,
      "match": {
        "repo": "demo"
      }
    },
    {
      "agent": "claude",
      "meta": {
        "id": "11111111-2222-4333-8444-555555555555",
        "cwd": "C:\\src\\demo",
        "title": "Fix login bug",
        "preview": "Fix the login bug in auth.rs please",
        "model": "claude-test-model",
        "branch": "feature/login",
        "version": "2.1.0",
        "origin": "cli",
        "started_ms": 1793527199000,
        "last_ms": 1793527500000,
        "messages": 6
      },
      "path": "claude/projects/C--src-demo/11111111-2222-4333-8444-555555555555.jsonl",
      "mtime_ms": 1767225600000,
      "match": {
        "repo": "demo"
      }
    },
    {
      "agent": "claude",
      "meta": {
        "id": "33333333-2222-4333-8444-555555555555",
        "cwd": "C:\\src\\other",
        "title": "My renamed session",
        "preview": "Write the docs",
        "branch": "main",
        "version": "2.1.0",
        "origin": "cli",
        "started_ms": 1790841600000,
        "last_ms": 1790841600000,
        "messages": 1
      },
      "path": "claude/projects/C--src-other/33333333-2222-4333-8444-555555555555.jsonl",
      "mtime_ms": 1767225600000,
      "match": {
        "repo": "other"
      }
    }
  ],
  "running_ids": {
    "claude": {
      "4242": "11111111-2222-4333-8444-555555555555"
    },
    "codex": {}
  },
  "truncated": false,
  "running": [
    {
      "agent": "codex",
      "pid": 300,
      "pids": [
        300,
        301
      ],
      "start_ms": 1,
      "cwd": "C:\\src\\demo",
      "session_id": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
      "guessed": true
    },
    {
      "agent": "claude",
      "pid": 500,
      "pids": [
        500
      ],
      "start_ms": 3,
      "session_id": "33333333-2222-4333-8444-555555555555",
      "guessed": false
    },
    {
      "agent": "claude",
      "pid": 600,
      "pids": [
        600
      ],
      "start_ms": 0,
      "cwd": "C:\\src\\other",
      "guessed": false
    },
    {
      "agent": "claude",
      "pid": 4242,
      "pids": [
        4242
      ],
      "start_ms": 0,
      "session_id": "11111111-2222-4333-8444-555555555555",
      "guessed": false,
      "pod_terminal": "t1"
    }
  ],
  "cwd_match": [
    [
      "C:\\src\\demo\\.pod\\t1\\crates",
      {
        "repo": "demo",
        "task": "demo/t1"
      }
    ],
    [
      "c:\\src\\DEMO",
      {
        "repo": "demo"
      }
    ],
    [
      "C:\\src\\other\\x",
      {
        "repo": "other"
      }
    ],
    [
      "C:\\src\\demo2",
      {}
    ],
    [
      "",
      {}
    ]
  ]
}"##;

const GOLDEN_MODS: &str = r##"{
  "discovered": {
    "enabled": true,
    "root": "<root>",
    "mods": [
      {
        "name": "Bad Name",
        "version": "",
        "enabled": false,
        "state": "invalid",
        "error": "folder name \"Bad Name\" is not a valid mod name",
        "permissions": [],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 0,
        "memory_kib": 0
      },
      {
        "name": "alpha",
        "version": "1.2.0",
        "title": "Alpha",
        "description": "Golden fixture",
        "enabled": false,
        "state": "disabled",
        "permissions": [
          {
            "name": "notify",
            "requested": true,
            "granted": false
          },
          {
            "name": "commands",
            "requested": true,
            "granted": false
          },
          {
            "name": "status",
            "requested": true,
            "granted": false
          },
          {
            "name": "terminal.send",
            "requested": true,
            "granted": false
          },
          {
            "name": "check",
            "requested": true,
            "granted": false
          }
        ],
        "checks": [
          {
            "name": "lint",
            "argv": [
              "cargo",
              "clippy",
              "-q"
            ]
          },
          {
            "name": "test",
            "argv": [
              "cargo",
              "test"
            ]
          }
        ],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 0,
        "memory_kib": 0
      },
      {
        "name": "boom",
        "version": "0.1.0",
        "enabled": false,
        "state": "disabled",
        "permissions": [],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 0,
        "memory_kib": 0
      },
      {
        "name": "quiet",
        "version": "3",
        "enabled": false,
        "state": "disabled",
        "permissions": [
          {
            "name": "gate",
            "requested": true,
            "granted": false
          },
          {
            "name": "orch",
            "requested": true,
            "granted": false
          }
        ],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 0,
        "memory_kib": 0
      }
    ],
    "dropped_events": 0
  },
  "set_enabled": [
    {
      "ok": {
        "name": "alpha",
        "version": "1.2.0",
        "title": "Alpha",
        "description": "Golden fixture",
        "enabled": true,
        "state": "running",
        "permissions": [
          {
            "name": "notify",
            "requested": true,
            "granted": true
          },
          {
            "name": "commands",
            "requested": true,
            "granted": true
          },
          {
            "name": "status",
            "requested": true,
            "granted": true
          },
          {
            "name": "terminal.send",
            "requested": true,
            "granted": false
          },
          {
            "name": "check",
            "requested": true,
            "granted": false
          }
        ],
        "checks": [
          {
            "name": "lint",
            "argv": [
              "cargo",
              "clippy",
              "-q"
            ]
          },
          {
            "name": "test",
            "argv": [
              "cargo",
              "test"
            ]
          }
        ],
        "commands": [
          {
            "id": "hello",
            "title": "인사",
            "detail": "알림을 띄운다"
          }
        ],
        "detectors": [
          {
            "match": [
              "aider",
              "claude"
            ]
          }
        ],
        "layouts": [
          {
            "id": "two",
            "title": "둘",
            "center": {
              "split": "h",
              "children": [
                {
                  "panel": "terminal",
                  "index": 0,
                  "size": 0.5
                },
                {
                  "tabs": [
                    {
                      "panel": "review"
                    },
                    {
                      "panel": "editor"
                    }
                  ]
                }
              ]
            }
          }
        ],
        "log_lines": 4,
        "memory_kib": 0
      }
    },
    {
      "ok": {
        "name": "boom",
        "version": "0.1.0",
        "enabled": true,
        "state": "stopped",
        "stop_reason": "load",
        "error": "Error: boom at load\n    at <eval> (main.js:1:11)",
        "permissions": [],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 3,
        "memory_kib": 0
      }
    },
    {
      "ok": {
        "name": "quiet",
        "version": "3",
        "enabled": true,
        "state": "running",
        "permissions": [
          {
            "name": "gate",
            "requested": true,
            "granted": true
          },
          {
            "name": "orch",
            "requested": true,
            "granted": false
          }
        ],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 2,
        "memory_kib": 0
      }
    },
    {
      "err": "no mod \"missing\""
    }
  ],
  "grant": [
    {
      "ok": {
        "name": "alpha",
        "version": "1.2.0",
        "title": "Alpha",
        "description": "Golden fixture",
        "enabled": true,
        "state": "running",
        "permissions": [
          {
            "name": "notify",
            "requested": true,
            "granted": true
          },
          {
            "name": "commands",
            "requested": true,
            "granted": true
          },
          {
            "name": "status",
            "requested": true,
            "granted": true
          },
          {
            "name": "terminal.send",
            "requested": true,
            "granted": true
          },
          {
            "name": "check",
            "requested": true,
            "granted": false
          }
        ],
        "checks": [
          {
            "name": "lint",
            "argv": [
              "cargo",
              "clippy",
              "-q"
            ]
          },
          {
            "name": "test",
            "argv": [
              "cargo",
              "test"
            ]
          }
        ],
        "commands": [
          {
            "id": "hello",
            "title": "인사",
            "detail": "알림을 띄운다"
          }
        ],
        "detectors": [
          {
            "match": [
              "aider",
              "claude"
            ]
          }
        ],
        "layouts": [
          {
            "id": "two",
            "title": "둘",
            "center": {
              "split": "h",
              "children": [
                {
                  "panel": "terminal",
                  "index": 0,
                  "size": 0.5
                },
                {
                  "tabs": [
                    {
                      "panel": "review"
                    },
                    {
                      "panel": "editor"
                    }
                  ]
                }
              ]
            }
          }
        ],
        "log_lines": 8,
        "memory_kib": 0
      }
    },
    {
      "ok": {
        "name": "quiet",
        "version": "3",
        "enabled": true,
        "state": "running",
        "permissions": [
          {
            "name": "gate",
            "requested": true,
            "granted": true
          },
          {
            "name": "orch",
            "requested": true,
            "granted": true
          }
        ],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 4,
        "memory_kib": 0
      }
    },
    {
      "ok": {
        "name": "quiet",
        "version": "3",
        "enabled": true,
        "state": "running",
        "permissions": [
          {
            "name": "gate",
            "requested": true,
            "granted": false
          },
          {
            "name": "orch",
            "requested": true,
            "granted": true
          }
        ],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 6,
        "memory_kib": 0
      }
    },
    {
      "err": "mod \"alpha\" does not request \"orch\""
    }
  ],
  "enabled": {
    "enabled": true,
    "root": "<root>",
    "mods": [
      {
        "name": "Bad Name",
        "version": "",
        "enabled": false,
        "state": "invalid",
        "error": "folder name \"Bad Name\" is not a valid mod name",
        "permissions": [],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 0,
        "memory_kib": 0
      },
      {
        "name": "alpha",
        "version": "1.2.0",
        "title": "Alpha",
        "description": "Golden fixture",
        "enabled": true,
        "state": "running",
        "permissions": [
          {
            "name": "notify",
            "requested": true,
            "granted": true
          },
          {
            "name": "commands",
            "requested": true,
            "granted": true
          },
          {
            "name": "status",
            "requested": true,
            "granted": true
          },
          {
            "name": "terminal.send",
            "requested": true,
            "granted": true
          },
          {
            "name": "check",
            "requested": true,
            "granted": false
          }
        ],
        "checks": [
          {
            "name": "lint",
            "argv": [
              "cargo",
              "clippy",
              "-q"
            ]
          },
          {
            "name": "test",
            "argv": [
              "cargo",
              "test"
            ]
          }
        ],
        "commands": [
          {
            "id": "hello",
            "title": "인사",
            "detail": "알림을 띄운다"
          }
        ],
        "detectors": [
          {
            "match": [
              "aider",
              "claude"
            ]
          }
        ],
        "layouts": [
          {
            "id": "two",
            "title": "둘",
            "center": {
              "split": "h",
              "children": [
                {
                  "panel": "terminal",
                  "index": 0,
                  "size": 0.5
                },
                {
                  "tabs": [
                    {
                      "panel": "review"
                    },
                    {
                      "panel": "editor"
                    }
                  ]
                }
              ]
            }
          }
        ],
        "log_lines": 8,
        "memory_kib": 0
      },
      {
        "name": "boom",
        "version": "0.1.0",
        "enabled": true,
        "state": "stopped",
        "stop_reason": "load",
        "error": "Error: boom at load\n    at <eval> (main.js:1:11)",
        "permissions": [],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 3,
        "memory_kib": 0
      },
      {
        "name": "quiet",
        "version": "3",
        "enabled": true,
        "state": "running",
        "permissions": [
          {
            "name": "gate",
            "requested": true,
            "granted": false
          },
          {
            "name": "orch",
            "requested": true,
            "granted": true
          }
        ],
        "checks": [],
        "commands": [],
        "detectors": [],
        "layouts": [],
        "log_lines": 6,
        "memory_kib": 0
      }
    ],
    "dropped_events": 0
  },
  "disable_alpha": {
    "name": "alpha",
    "version": "1.2.0",
    "title": "Alpha",
    "description": "Golden fixture",
    "enabled": false,
    "state": "disabled",
    "permissions": [
      {
        "name": "notify",
        "requested": true,
        "granted": true
      },
      {
        "name": "commands",
        "requested": true,
        "granted": true
      },
      {
        "name": "status",
        "requested": true,
        "granted": true
      },
      {
        "name": "terminal.send",
        "requested": true,
        "granted": true
      },
      {
        "name": "check",
        "requested": true,
        "granted": false
      }
    ],
    "checks": [
      {
        "name": "lint",
        "argv": [
          "cargo",
          "clippy",
          "-q"
        ]
      },
      {
        "name": "test",
        "argv": [
          "cargo",
          "test"
        ]
      }
    ],
    "commands": [],
    "detectors": [],
    "layouts": [],
    "log_lines": 9,
    "memory_kib": 0
  },
  "state_file": {
    "mods": {
      "Bad Name": {
        "enabled": false,
        "granted": []
      },
      "alpha": {
        "enabled": false,
        "granted": [
          "notify",
          "commands",
          "status",
          "terminal.send"
        ]
      },
      "boom": {
        "enabled": true,
        "granted": []
      },
      "quiet": {
        "enabled": true,
        "granted": [
          "orch"
        ]
      }
    }
  }
}"##;

const GOLDEN_SCHEMA: &str = r##"{
  "methods": {
    "task.pr.status": "d91c344ddb9254d1",
    "task.pr.create": "e014cdcce3c12e67",
    "task.pr.merge": "744cb430565dc989",
    "mod.list": "b3660cbb66dfad2f",
    "mod.set_enabled": "098a1a78a8418442",
    "mod.grant": "6879844970eea347",
    "mod.log": "201051e76726032c",
    "mod.command": "77d93793cfddca96",
    "mod.reload": "cecc4e82c2099889",
    "orch.run.create": "37e21f66f621f1b6",
    "orch.run.list": "d4a69e6e4ccf415d",
    "orch.run.show": "cfb7c07508eed14f",
    "orch.item.create": "b815c404345d324c",
    "orch.item.list": "bf93b119fe546edc",
    "orch.item.update": "fe46cc0199df9d73",
    "orch.worker.start": "4aa2cfea4b2e4fe1",
    "orch.worker.list": "adb3eff17fe450d0",
    "orch.worker.show": "e5ca4c819c06793d",
    "orch.worker.read": "639fb8e82b9811d3",
    "orch.worker.stop": "e5ca4c819c06793d",
    "orch.worker.abandon": "e5ca4c819c06793d",
    "orch.worker.keep": "e5ca4c819c06793d",
    "orch.worker.release": "e5ca4c819c06793d",
    "orch.mail.send": "9e7d15c76f7d6c02",
    "orch.mail.check": "9ae6b5f124fcf5dd",
    "orch.mail.reply": "eb353eea030bdc6e",
    "orch.mail.ask": "171eabae3fede703",
    "orch.done": "24699fdc1d4092b7",
    "orch.mail.list": "66465af7b72d71d6",
    "orch.config.get": "4e4357d359a4f665",
    "orch.config.set": "1caea688c733aff8",
    "orch.queue": "97d9d72b35526215",
    "orch.gate.request": "cd89208010e6be64",
    "orch.gate.decide": "4fd9adaa11e964a7",
    "orch.limits.get": "b519846ea67f759f",
    "orch.limits.set": "430b2164a22fe8e6",
    "orch.wait": "0b918337e39e23c4",
    "orch.commit": "8ff70de39fa185cf",
    "sessions.list": "6495fe6e774e1ee9",
    "sessions.scan": "d47a751c74ef7992",
    "sessions.settings": "eca8346ea7aee7a0",
    "sessions.register": "d5ea89c561e57154",
    "sessions.resume": "c269399d3ee85ad6",
    "sessions.hide": "b8cf53878ba15c1b"
  },
  "notifications": {
    "task.pr.changed": "5013c88a15cb9576",
    "mod.notify": "38686cb081af178f",
    "mod.changed": "04a9acf37427af82",
    "orch.changed": "b6734b30328b8d8f",
    "orch.mail": "aa20f9654e8dccd6",
    "sessions.changed": "6607bb4076acf9c3"
  },
  "protocol": 19
}"##;
