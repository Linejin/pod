//! `pod` output across the whole daemon refactor (card D-T3,
//! docs/20-refactor/daemon.md §3; results in
//! docs/20-refactor/daemon-validation.md): the integrated tree prints what
//! the pre-refactor master `64e9530` printed.
//!
//! - `help_tree_matches_the_baseline`: `--version`, the help page of every
//!   command (walked from `pod --help`), `guide` and `server schema`, as exit
//!   code and FNV-64 fingerprints of stdout/stderr. No daemon is involved.
//! - `terminal_flow_matches_the_baseline`: a user's session on an isolated
//!   daemon: status, run a fake agent, wait for idle, send, wait for output,
//!   list, stop the daemon keeping the terminal, start it again, see the
//!   terminal rediscovered, send again, kill, remove, stop. Exit code,
//!   stdout (JSON compared as JSON) and stderr of every command.
//!
//! The file is self-contained on purpose (no dev dependencies, only
//! `podd::client` and `podd::protocol`, which pod-cli reaches on both trees),
//! so the same file builds on `64e9530`, where the golden at the end was
//! recorded: `POD_ACCEPTANCE_OUT=<dir> <test exe>` writes `<dir>/<test>.txt`
//! instead of comparing. A refactor never rewrites the golden. Normalized:
//! temp root, pipe, binary directory, the version, `*_ms`/`*_at`/pid/uptime/
//! `since`/`turn` values and the digits after `pid`/`uptime`/`up` in text.
//!
//! Needs `podd.exe`, `pod-holder.exe` and `examples/fake-agent.exe` next to
//! `pod.exe` (`cargo build -p podd --example fake-agent -p pod-cli --bin
//! pod`). Everything the daemon writes stays under a temp root and USERNAME
//! is a run id, so the default pipe is never the user's.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use podd::client::Client;
use podd::protocol::*;
use serde_json::{Map, Value, json};

const POD: &str = env!("CARGO_BIN_EXE_pod");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const OUT_ENV: &str = "POD_ACCEPTANCE_OUT";

fn bin_dir() -> PathBuf {
    Path::new(POD).parent().unwrap().to_path_buf()
}

fn fixture(rel: &str) -> PathBuf {
    let p = bin_dir().join(rel);
    assert!(
        p.exists(),
        "{} missing: cargo build -p podd --example fake-agent -p pod-cli --bin pod \
         (tests do not run cargo)",
        p.display()
    );
    p
}

// Own copy on purpose, not `pod_test_support::wait::until`: this file must
// also compile on the 64e9530 baseline, where the golden is recorded and that
// crate does not exist (docs/20-refactor/daemon-validation.md, "공용 helper
// 예외").
fn until<T>(what: &str, secs: u64, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn fnv64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn check(name: &str, golden: &str, got: &[String]) {
    let text = got.join("\n") + "\n";
    if let Some(dir) = std::env::var_os(OUT_ENV) {
        let p = PathBuf::from(dir).join(format!("{name}.txt"));
        std::fs::write(&p, &text).unwrap();
        eprintln!("recorded {}", p.display());
        return;
    }
    // The raw string starts with a line break (CRLF in an autocrlf checkout).
    let golden = golden.trim_start_matches(['\r', '\n']);
    let want: Vec<&str> = golden.lines().collect();
    let have: Vec<&str> = text.lines().collect();
    if want == have {
        return;
    }
    let first = want
        .iter()
        .zip(&have)
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(have.len()));
    panic!(
        "{name}: differs from the baseline at line {} ({} vs {} lines)\n\
         baseline: {}\n     got: {}\n\nall got:\n{text}",
        first + 1,
        want.len(),
        have.len(),
        want.get(first).unwrap_or(&"<end>"),
        have.get(first).unwrap_or(&"<end>"),
    );
}

// ───────────────────────────── a temp profile ─────────────────────────────

struct Env {
    root: PathBuf,
    run_id: String,
    pipe: String,
    /// (text, placeholder), longest first.
    replace: Vec<(String, String)>,
    daemon: Option<Child>,
    got: Vec<String>,
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(mut d) = self.daemon.take() {
            if let Ok(mut c) = Client::connect(&self.pipe) {
                let _ = c.call::<ServerStop>(&StopParams { kill: true });
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline && !matches!(d.try_wait(), Ok(Some(_))) {
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = d.kill();
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Env {
    fn new(tag: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let run_id = format!("pod-accept-{tag}-{}-{nanos}", std::process::id());
        let root = std::env::temp_dir().join(&run_id);
        for d in ["home", "roaming", "local", "tmp", "state", "agents"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        let pipe = format!(r"\\.\pipe\{run_id}");
        let mut replace: Vec<(String, String)> = Vec::new();
        let mut add = |p: &Path, mark: &str| {
            let mut spellings = vec![p.display().to_string()];
            if let Ok(long) = std::fs::canonicalize(p) {
                let s = long.display().to_string();
                spellings.push(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string());
            }
            for s in spellings {
                let s = s.trim_end_matches('\\').to_string();
                for v in [s.clone(), s.replace('\\', "/")] {
                    replace.push((v.to_lowercase(), mark.into()));
                    replace.push((v, mark.into()));
                }
            }
        };
        add(&root, "<ROOT>");
        add(&bin_dir(), "<BIN>");
        add(&std::env::temp_dir(), "<TMP>");
        replace.push((pipe.clone(), "<PIPE>".into()));
        replace.push((run_id.clone(), "<RUN>".into()));
        replace.push((VERSION.into(), "<VERSION>".into()));
        replace.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        replace.dedup();
        Self {
            root,
            run_id,
            pipe,
            replace,
            daemon: None,
            got: Vec::new(),
        }
    }

    fn text(&self, s: &str) -> String {
        let mut s = s.replace("\r\n", "\n");
        for (from, to) in &self.replace {
            s = s.replace(from.as_str(), to);
        }
        mask_numbers_after(
            &s,
            &["pid ", "pid=", "pid: ", "uptime ", "uptime: ", " up "],
        )
    }

    fn value(&self, key: Option<&str>, v: &Value) -> Value {
        if let Some(k) = key
            && volatile(k)
            && !v.is_null()
        {
            return json!("<n>");
        }
        match v {
            Value::String(s) => json!(self.text(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.value(None, x)).collect()),
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().collect();
                keys.sort();
                let mut out = Map::new();
                for k in keys {
                    out.insert(k.clone(), self.value(Some(k), &m[k]));
                }
                Value::Object(out)
            }
            other => other.clone(),
        }
    }

    /// `pod` with this profile: temp-only home, app data and default pipe.
    fn pod(&self, args: &[&str]) -> (i32, String, String) {
        let out = Command::new(POD)
            .args(args)
            // `pod run` starts terminals in the caller's directory.
            .current_dir(self.root.join("tmp"))
            .env("USERNAME", &self.run_id)
            .env("USERPROFILE", self.root.join("home"))
            .env("HOME", self.root.join("home"))
            .env("APPDATA", self.root.join("roaming"))
            .env("LOCALAPPDATA", self.root.join("local"))
            .env("TEMP", self.root.join("tmp"))
            .env("TMP", self.root.join("tmp"))
            .env_remove("POD_SOCKET")
            .env_remove("POD_TERMINAL_ID")
            .env_remove("POD_DAEMON")
            .stdin(Stdio::null())
            .output()
            .expect("run pod");
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// `pod --pipe <pipe> <args>`, recorded.
    fn step(&mut self, args: &[&str]) -> (i32, String) {
        let mut full = vec!["--pipe", self.pipe.as_str()];
        full.extend_from_slice(args);
        let full: Vec<String> = full.iter().map(|s| s.to_string()).collect();
        let full: Vec<&str> = full.iter().map(String::as_str).collect();
        let (code, stdout, stderr) = self.pod(&full);
        let shown = match serde_json::from_str::<Value>(&stdout) {
            Ok(v) if args.contains(&"--json") => {
                serde_json::to_string(&self.value(None, &v)).unwrap()
            }
            _ => format!("{:?}", self.text(&stdout)),
        };
        let line = format!(
            "pod {} -> exit {code} stdout {shown} stderr {:?}",
            self.text(&args.join(" ")),
            self.text(&stderr)
        );
        self.got.push(line);
        (code, stdout)
    }

    fn start_daemon(&mut self) {
        let podd = fixture("podd.exe");
        let state = self.root.join("state");
        let child = Command::new(podd)
            .env(
                "POD_DISCOVER_CLAUDE_DIR",
                state.join("agent-stores").join("claude"),
            )
            .env(
                "POD_DISCOVER_CODEX_DIR",
                state.join("agent-stores").join("codex"),
            )
            .env("POD_DISCOVER_PROCESSES", "off")
            .env("POD_MODS_DIR", state.join("extensions"))
            .env("USERNAME", &self.run_id)
            .env_remove("POD_SOCKET")
            .env_remove("POD_TERMINAL_ID")
            .args(["--pipe", &self.pipe, "--holder"])
            .arg(fixture("pod-holder.exe"))
            .arg("--state-dir")
            .arg(&state)
            .arg("--worktrees-root")
            .arg(self.root.join("worktrees"))
            .stdin(Stdio::null())
            .spawn()
            .expect("spawn podd");
        let pipe = self.pipe.clone();
        until("podd listening", 15, || Client::connect(&pipe).ok());
        self.daemon = Some(child);
    }

    fn wait_daemon_exit(&mut self) {
        let mut d = self.daemon.take().expect("a daemon");
        until("podd exit", 15, || {
            matches!(d.try_wait(), Ok(Some(_))).then_some(())
        });
    }
}

/// Values that differ from run to run by nature: times, process ids and the
/// status engine's turn counter.
fn volatile(k: &str) -> bool {
    k.ends_with("_ms")
        || k.ends_with("_at")
        || k.ends_with("_pid")
        || k.starts_with("uptime")
        || ["ms", "pid", "since", "turn"].contains(&k)
}

/// Digits right after one of `marks` become `<n>`.
fn mask_numbers_after(s: &str, marks: &[&str]) -> String {
    let mut out = s.to_string();
    for m in marks {
        let mut res = String::new();
        let mut rest = out.as_str();
        while let Some(i) = rest.find(m) {
            res.push_str(&rest[..i + m.len()]);
            rest = &rest[i + m.len()..];
            let n = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            if n > 0 {
                res.push_str("<n>");
                rest = &rest[n..];
            }
        }
        res.push_str(rest);
        out = res;
    }
    out
}

/// Names under `Commands:` of a help page (not `help`).
fn subcommands(help: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in help.lines() {
        if line == "Commands:" {
            inside = true;
            continue;
        }
        if inside {
            if !line.starts_with("  ") {
                break;
            }
            if line.starts_with("   ") {
                continue;
            }
            let name = line.split_whitespace().next().unwrap_or_default();
            if !name.is_empty() && name != "help" {
                out.push(name.to_string());
            }
        }
    }
    out
}

// ───────────────────────────── tests ─────────────────────────────

#[test]
fn help_tree_matches_the_baseline() {
    let mut e = Env::new("help");
    let (code, out, err) = e.pod(&["--version"]);
    let line = format!(
        "version -> exit {code} stdout {:?} stderr {:?}",
        e.text(&out),
        e.text(&err)
    );
    e.got.push(line);
    let mut queue: Vec<Vec<String>> = vec![Vec::new()];
    let mut pages = 0;
    while let Some(path) = queue.pop() {
        let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
        args.push("--help");
        let (code, out, err) = e.pod(&args);
        pages += 1;
        let line = format!(
            "help [{}] -> exit {code} stdout {:016x} ({} lines) stderr {:016x}",
            path.join(" "),
            fnv64(&e.text(&out)),
            out.lines().count(),
            fnv64(&e.text(&err))
        );
        e.got.push(line);
        for sub in subcommands(&out).into_iter().rev() {
            let mut p = path.clone();
            p.push(sub);
            queue.push(p);
        }
    }
    let pipe = e.run_id.clone();
    for args in [vec!["guide"], vec!["--pipe", &pipe, "server", "schema"]] {
        let (code, out, err) = e.pod(&args);
        let line = format!(
            "{} -> exit {code} stdout {:016x} ({} lines) stderr {:?}",
            e.text(&args.join(" ")),
            fnv64(&e.text(&out)),
            out.lines().count(),
            e.text(&err)
        );
        e.got.push(line);
    }
    assert!(pages > 40, "only {pages} help pages");
    let got = std::mem::take(&mut e.got);
    check("cli-help", HELP_GOLDEN, &got);
}

#[test]
fn terminal_flow_matches_the_baseline() {
    let fake = fixture("examples/fake-agent.exe").display().to_string();
    let mut e = Env::new("flow");
    let log = e
        .root
        .join("agents")
        .join("pastes.log")
        .display()
        .to_string();
    e.step(&["server", "status"]);
    e.start_daemon();
    e.step(&["server", "status"]);
    e.step(&["--json", "server", "status"]);
    e.step(&["--json", "ls"]);
    let loop_arg = format!("paste-loop:{log}");
    e.step(&[
        "run", "--name", "agent", "--cols", "80", "--rows", "24", "--", &fake, &loop_arg,
    ]);
    e.step(&["wait", "agent", "--status", "idle", "--timeout", "30"]);
    e.step(&["send", "agent", "hello acceptance"]);
    e.step(&[
        "wait",
        "agent",
        "--output",
        "> hello acceptance",
        "--timeout",
        "30",
    ]);
    e.step(&[
        "--json",
        "wait",
        "agent",
        "--status",
        "idle",
        "--timeout",
        "30",
    ]);
    e.step(&["--json", "ls"]);
    e.step(&["--json", "status", "agent"]);

    // Stop the daemon, keep the terminal; a new daemon rediscovers it.
    e.step(&["server", "stop"]);
    e.wait_daemon_exit();
    e.step(&["server", "status"]);
    e.start_daemon();
    e.step(&["--json", "ls"]);
    e.step(&["wait", "agent", "--status", "idle", "--timeout", "30"]);
    e.step(&["send", "agent", "after restart"]);
    e.step(&[
        "wait",
        "agent",
        "--output",
        "> after restart",
        "--timeout",
        "30",
    ]);
    e.step(&["wait", "agent", "--status", "idle", "--timeout", "30"]);
    let pastes = std::fs::read_to_string(&log).unwrap_or_default();
    let line = format!("agent submissions {:?}", e.text(&pastes));
    e.got.push(line);

    e.step(&["kill", "agent"]);
    e.step(&["wait", "agent", "--exit", "--timeout", "30"]);
    e.step(&["rm", "agent"]);
    e.step(&["--json", "ls", "--all"]);
    e.step(&["server", "stop", "--kill"]);
    e.wait_daemon_exit();
    let got = std::mem::take(&mut e.got);
    check("cli-flow", FLOW_GOLDEN, &got);
}

// ───────────────────────────── golden (recorded on 64e9530) ─────────────────────────────
// Protocol v20 (docs/17 R4) changed exactly three lines, re-recorded and
// reviewed: `guide` (it prints the protocol number), `server schema` (two
// methods' worth of schema more) and the `server status` protocol.
// Protocol v21 (rm-o2, docs/19 §20) changed the same kinds of lines:
// `guide`, `server schema` (hello `worker`, status `endpoint`, decide
// `person`) and `server status` (protocol 21, `endpoint: full`).
// Protocol v22 (rm-r5, docs/17 R5): `guide`, `server schema` (fs.* and
// review.diff/blob, the hello file limits) and `server status` (22).

const HELP_GOLDEN: &str = r##"
version -> exit 0 stdout "pod <VERSION>\n" stderr ""
help [] -> exit 0 stdout 83ab65e2168bba92 (40 lines) stderr cbf29ce484222325
help [run] -> exit 0 stdout 61186194de40fbe4 (19 lines) stderr cbf29ce484222325
help [ls] -> exit 0 stdout a27ea58240460f5b (10 lines) stderr cbf29ce484222325
help [keep] -> exit 0 stdout bfefc92556f540c6 (13 lines) stderr cbf29ce484222325
help [send] -> exit 0 stdout 492506372d7da6b0 (15 lines) stderr cbf29ce484222325
help [keys] -> exit 0 stdout b387718ef426ec9a (13 lines) stderr cbf29ce484222325
help [read] -> exit 0 stdout 056a364d5f403c47 (15 lines) stderr cbf29ce484222325
help [wait] -> exit 0 stdout 27e6905942fbc8f0 (18 lines) stderr cbf29ce484222325
help [resize] -> exit 0 stdout d50313b6ad371b67 (14 lines) stderr cbf29ce484222325
help [attach] -> exit 0 stdout 20082ae38008b1a9 (14 lines) stderr cbf29ce484222325
help [status] -> exit 0 stdout 191b589ca0f84469 (12 lines) stderr cbf29ce484222325
help [ack] -> exit 0 stdout d6686865ab8e0709 (12 lines) stderr cbf29ce484222325
help [hooks] -> exit 0 stdout 3259e6391fe345ff (15 lines) stderr cbf29ce484222325
help [hooks install] -> exit 0 stdout 646aea70ad2ac905 (55 lines) stderr cbf29ce484222325
help [hooks uninstall] -> exit 0 stdout 6c8084a2a7ca8414 (47 lines) stderr cbf29ce484222325
help [hooks status] -> exit 0 stdout 655100d8ae44cc8b (47 lines) stderr cbf29ce484222325
help [kill] -> exit 0 stdout 4452c906ad83d431 (12 lines) stderr cbf29ce484222325
help [rm] -> exit 0 stdout 14aeb33f0b3823dd (13 lines) stderr cbf29ce484222325
help [repo] -> exit 0 stdout 9462660f3c687aee (18 lines) stderr cbf29ce484222325
help [repo add] -> exit 0 stdout ce3e26f218c2e251 (13 lines) stderr cbf29ce484222325
help [repo ls] -> exit 0 stdout 2e1491c235c60a13 (9 lines) stderr cbf29ce484222325
help [repo rm] -> exit 0 stdout b4596bcd688011ed (12 lines) stderr cbf29ce484222325
help [repo config] -> exit 0 stdout 0dbda5b51be6face (13 lines) stderr cbf29ce484222325
help [repo trust] -> exit 0 stdout b175d9b88105f59e (15 lines) stderr cbf29ce484222325
help [repo set] -> exit 0 stdout ffc514b74ace3c7f (16 lines) stderr cbf29ce484222325
help [task] -> exit 0 stdout 80f4f814cb6313ef (23 lines) stderr cbf29ce484222325
help [task new] -> exit 0 stdout 6f522773a4977f02 (25 lines) stderr cbf29ce484222325
help [task ls] -> exit 0 stdout 7d3de676fe2598a3 (10 lines) stderr cbf29ce484222325
help [task show] -> exit 0 stdout c3cb88d845d86a92 (12 lines) stderr cbf29ce484222325
help [task diff] -> exit 0 stdout 247ccc35ad57b3b8 (12 lines) stderr cbf29ce484222325
help [task touch] -> exit 0 stdout 7eb8c9d8d596c504 (13 lines) stderr cbf29ce484222325
help [task rm] -> exit 0 stdout 7d4dcb3c37e28b4f (14 lines) stderr cbf29ce484222325
help [task archive] -> exit 0 stdout 1326a3cec4492f01 (14 lines) stderr cbf29ce484222325
help [task unarchive] -> exit 0 stdout 3493af1956b00aa9 (13 lines) stderr cbf29ce484222325
help [task resume] -> exit 0 stdout 87e2800b622f1236 (14 lines) stderr cbf29ce484222325
help [task cleanup] -> exit 0 stdout 61a8e20a936d2b1e (11 lines) stderr cbf29ce484222325
help [task suggest-name] -> exit 0 stdout 6bf384c42851a787 (12 lines) stderr cbf29ce484222325
help [review] -> exit 0 stdout 4f4c5dd241fceddc (19 lines) stderr cbf29ce484222325
help [review add] -> exit 0 stdout 0c5b5cdd9ddb2a73 (18 lines) stderr cbf29ce484222325
help [review ls] -> exit 0 stdout b1ebc99ca054d9c5 (13 lines) stderr cbf29ce484222325
help [review send] -> exit 0 stdout ebb0cb91c9f5bc8e (17 lines) stderr cbf29ce484222325
help [review resolve] -> exit 0 stdout a59bfbafdf3ebbd8 (14 lines) stderr cbf29ce484222325
help [review rm] -> exit 0 stdout bc85cdcd94fbc252 (12 lines) stderr cbf29ce484222325
help [review deliveries] -> exit 0 stdout 2335d4af5bc06950 (12 lines) stderr cbf29ce484222325
help [review cancel] -> exit 0 stdout 7e9f7932e6a170f8 (12 lines) stderr cbf29ce484222325
help [group] -> exit 0 stdout 843188c285a7dfea (19 lines) stderr cbf29ce484222325
help [group new] -> exit 0 stdout 54b3af70d822eb32 (17 lines) stderr cbf29ce484222325
help [group ls] -> exit 0 stdout 5e19936f8757da68 (10 lines) stderr cbf29ce484222325
help [group show] -> exit 0 stdout e6dd83ad903fe765 (12 lines) stderr cbf29ce484222325
help [group compare] -> exit 0 stdout f152a9c3a7c35fda (12 lines) stderr cbf29ce484222325
help [group check] -> exit 0 stdout c681dcdd97496564 (16 lines) stderr cbf29ce484222325
help [group pick] -> exit 0 stdout 7b81df19ef836985 (16 lines) stderr cbf29ce484222325
help [group rm] -> exit 0 stdout 1319f3a1192a776f (12 lines) stderr cbf29ce484222325
help [orch] -> exit 0 stdout 7e6b6541f8199c1d (26 lines) stderr cbf29ce484222325
help [orch run] -> exit 0 stdout d440f3482ee48bcf (15 lines) stderr cbf29ce484222325
help [orch run new] -> exit 0 stdout d37672912dfd348a (10 lines) stderr cbf29ce484222325
help [orch run ls] -> exit 0 stdout dce2f3dc768f2a73 (8 lines) stderr cbf29ce484222325
help [orch run show] -> exit 0 stdout 02b4e40a835fdffa (10 lines) stderr cbf29ce484222325
help [orch item] -> exit 0 stdout ebf097c5271c981f (15 lines) stderr cbf29ce484222325
help [orch item add] -> exit 0 stdout bbb964423720dea7 (13 lines) stderr cbf29ce484222325
help [orch item ls] -> exit 0 stdout 634509157c4ac58b (9 lines) stderr cbf29ce484222325
help [orch item reopen] -> exit 0 stdout 8d542ca7b52e587a (12 lines) stderr cbf29ce484222325
help [orch worker] -> exit 0 stdout 11728049d4616ff4 (20 lines) stderr cbf29ce484222325
help [orch worker start] -> exit 0 stdout 20f4f00eae29c353 (23 lines) stderr cbf29ce484222325
help [orch worker show] -> exit 0 stdout f4c58af6013d8835 (10 lines) stderr cbf29ce484222325
help [orch worker read] -> exit 0 stdout c63aba7084376f0f (13 lines) stderr cbf29ce484222325
help [orch worker stop] -> exit 0 stdout e62c3b1b3d6a1681 (12 lines) stderr cbf29ce484222325
help [orch worker abandon] -> exit 0 stdout 6959542508d34648 (12 lines) stderr cbf29ce484222325
help [orch worker keep] -> exit 0 stdout f49ecbaa7c1addf6 (12 lines) stderr cbf29ce484222325
help [orch worker release] -> exit 0 stdout 9fdb5e5926ac1211 (12 lines) stderr cbf29ce484222325
help [orch worker pr] -> exit 0 stdout d5fc5b2697075e4e (18 lines) stderr cbf29ce484222325
help [orch workers] -> exit 0 stdout 7e13a0431443c50a (10 lines) stderr cbf29ce484222325
help [orch queue] -> exit 0 stdout 3489487a1ef07891 (10 lines) stderr cbf29ce484222325
help [orch limits] -> exit 0 stdout 93e45fda9faeb1bb (11 lines) stderr cbf29ce484222325
help [orch gate] -> exit 0 stdout 08cd1af5231c4532 (14 lines) stderr cbf29ce484222325
help [orch gate request] -> exit 0 stdout 4365abe1a9f5bf85 (17 lines) stderr cbf29ce484222325
help [orch gate decide] -> exit 0 stdout 5a3033d375663a70 (15 lines) stderr cbf29ce484222325
help [orch send] -> exit 0 stdout 5ff298f825dcfebe (17 lines) stderr cbf29ce484222325
help [orch check] -> exit 0 stdout 42575648297a6c85 (13 lines) stderr cbf29ce484222325
help [orch reply] -> exit 0 stdout 59a0bf38ff55efae (12 lines) stderr cbf29ce484222325
help [orch ask] -> exit 0 stdout 63acc28082f412ac (12 lines) stderr cbf29ce484222325
help [orch done] -> exit 0 stdout 7db22e223f56c690 (14 lines) stderr cbf29ce484222325
help [orch wait] -> exit 0 stdout a96af065373710fb (10 lines) stderr cbf29ce484222325
help [orch commit] -> exit 0 stdout 23dad66e372544eb (15 lines) stderr cbf29ce484222325
help [gate] -> exit 0 stdout 14a753d611251771 (17 lines) stderr cbf29ce484222325
help [gate request] -> exit 0 stdout fed882c940e9d78b (18 lines) stderr cbf29ce484222325
help [gate ls] -> exit 0 stdout 44c9308460a859c8 (12 lines) stderr cbf29ce484222325
help [gate show] -> exit 0 stdout 1fccd44babcd5a4c (10 lines) stderr cbf29ce484222325
help [gate approve] -> exit 0 stdout 301be2c746def647 (12 lines) stderr cbf29ce484222325
help [gate reject] -> exit 0 stdout 58bb1210a0ccaadc (11 lines) stderr cbf29ce484222325
help [pr] -> exit 0 stdout 66464e00d44538f5 (16 lines) stderr cbf29ce484222325
help [pr status] -> exit 0 stdout edbef64b0f8905bb (13 lines) stderr cbf29ce484222325
help [pr create] -> exit 0 stdout e6097f4bad455f01 (18 lines) stderr cbf29ce484222325
help [pr merge] -> exit 0 stdout cddbc618a3203f2f (14 lines) stderr cbf29ce484222325
help [pr open] -> exit 0 stdout 1f40845d2e8872dd (13 lines) stderr cbf29ce484222325
help [sessions] -> exit 0 stdout e0e3f6a77e86eff1 (18 lines) stderr cbf29ce484222325
help [sessions list] -> exit 0 stdout d9988d932bad51fb (10 lines) stderr cbf29ce484222325
help [sessions scan] -> exit 0 stdout cdee58d30c150d98 (10 lines) stderr cbf29ce484222325
help [sessions settings] -> exit 0 stdout 114ae1ff9ba9f3bc (11 lines) stderr cbf29ce484222325
help [sessions register] -> exit 0 stdout 1b16b35b5dceea54 (14 lines) stderr cbf29ce484222325
help [sessions resume] -> exit 0 stdout 20922ac9f8ea2771 (13 lines) stderr cbf29ce484222325
help [sessions hide] -> exit 0 stdout 3aaa320b6aea5639 (13 lines) stderr cbf29ce484222325
help [mobile] -> exit 0 stdout 0fd3756906670c2e (17 lines) stderr cbf29ce484222325
help [mobile devices] -> exit 0 stdout d920473123dde1d6 (9 lines) stderr cbf29ce484222325
help [mobile revoke] -> exit 0 stdout 664468e82e70063f (13 lines) stderr cbf29ce484222325
help [mobile policy] -> exit 0 stdout 7cf3c40ff66adf0a (16 lines) stderr cbf29ce484222325
help [mobile audit] -> exit 0 stdout 1c739508d85cf02f (11 lines) stderr cbf29ce484222325
help [mobile settings] -> exit 0 stdout 6bad70371c4e5dbc (10 lines) stderr cbf29ce484222325
help [server] -> exit 0 stdout 5fb4d53d5e948c6a (15 lines) stderr cbf29ce484222325
help [server status] -> exit 0 stdout 3b108a7c32d68e68 (9 lines) stderr cbf29ce484222325
help [server stop] -> exit 0 stdout d6b6c4ee91b1e56b (10 lines) stderr cbf29ce484222325
help [server schema] -> exit 0 stdout 1b57e5dbc490dbcf (9 lines) stderr cbf29ce484222325
help [remote-stdio] -> exit 0 stdout f3ec7939cd771694 (9 lines) stderr cbf29ce484222325
help [machine] -> exit 0 stdout 4ba4eae5cbef7a7b (16 lines) stderr cbf29ce484222325
help [machine add] -> exit 0 stdout ccf35c57c59929dd (17 lines) stderr cbf29ce484222325
help [machine list] -> exit 0 stdout fe988d6f01d44d82 (9 lines) stderr cbf29ce484222325
help [machine remove] -> exit 0 stdout e80f5c33a14c0168 (12 lines) stderr cbf29ce484222325
help [machine test] -> exit 0 stdout b036010d9a8e4ee7 (12 lines) stderr cbf29ce484222325
help [guide] -> exit 0 stdout 62db4f75842bdc90 (14 lines) stderr cbf29ce484222325
help [guide orchestration] -> exit 0 stdout a2fcba0bcb8181d2 (10 lines) stderr cbf29ce484222325
help [guide install] -> exit 0 stdout 907ba8bb94101b30 (27 lines) stderr cbf29ce484222325
guide -> exit 0 stdout c52b43dc98440dd9 (8 lines) stderr ""
--pipe <RUN> server schema -> exit 0 stdout 76c1047ba82fe46d (38094 lines) stderr ""
"##;

const FLOW_GOLDEN: &str = r##"
pod server status -> exit 3 stdout "podd is not running (<PIPE>)\n" stderr ""
pod server status -> exit 0 stdout "podd <VERSION> running: pid <n>, pipe <PIPE>, up <n>s, terminals 0 (0 running)\n" stderr ""
pod --json server status -> exit 0 stdout {"endpoint":"full","pid":"<n>","pipe":"<PIPE>","protocol":22,"running":0,"started_ms":"<n>","state_dir":"<ROOT>\\state","terminals":0,"uptime_ms":"<n>","version":"<VERSION>"} stderr ""
pod --json ls -> exit 0 stdout {"retention_ms":"<n>","terminals":[]} stderr ""
pod run --name agent --cols 80 --rows 24 -- <BIN>\examples/fake-agent.exe paste-loop:<ROOT>\agents\pastes.log -> exit 0 stdout "t1\n" stderr ""
pod wait agent --status idle --timeout 30 -> exit 0 stdout "t1 idle (turn 0)\n" stderr ""
pod send agent hello acceptance -> exit 0 stdout "" stderr ""
pod wait agent --output > hello acceptance --timeout 30 -> exit 0 stdout "t1 matched\n" stderr ""
pod --json wait agent --status idle --timeout 30 -> exit 0 stdout {"outcome":"matched","status":{"badge":"idle","blocked_children":0,"confidence":0.9,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":"<n>"},"terminal":{"argv":["<BIN>\\examples/fake-agent.exe","paste-loop:<ROOT>\\agents\\pastes.log"],"attached":0,"cols":80,"conpty":"bundled:<BIN>\\conpty.dll","created_ms":"<n>","cwd":"<ROOT>\\tmp","holder_pid":"<n>","id":"t1","killed":false,"lost":false,"name":"agent","pid":"<n>","rows":24,"state":"running","status":{"badge":"idle","blocked_children":0,"confidence":0.9,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":"<n>"}},"waited_ms":"<n>"} stderr ""
pod --json ls -> exit 0 stdout {"retention_ms":"<n>","terminals":[{"argv":["<BIN>\\examples/fake-agent.exe","paste-loop:<ROOT>\\agents\\pastes.log"],"attached":0,"cols":80,"conpty":"bundled:<BIN>\\conpty.dll","created_ms":"<n>","cwd":"<ROOT>\\tmp","holder_pid":"<n>","id":"t1","killed":false,"lost":false,"name":"agent","pid":"<n>","rows":24,"state":"running","status":{"badge":"idle","blocked_children":0,"confidence":0.9,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":"<n>"}}]} stderr ""
pod --json status agent -> exit 0 stdout {"badge":"idle","blocked_children":0,"confidence":0.9,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":"<n>"} stderr ""
pod server stop -> exit 0 stdout "podd stopped (1 running terminal(s) stay alive in their holders)\n" stderr ""
pod server status -> exit 3 stdout "podd is not running (<PIPE>)\n" stderr ""
pod --json ls -> exit 0 stdout {"retention_ms":"<n>","terminals":[{"argv":["<BIN>\\examples/fake-agent.exe","paste-loop:<ROOT>\\agents\\pastes.log"],"attached":0,"cols":80,"conpty":"bundled:<BIN>\\conpty.dll","created_ms":"<n>","cwd":"<ROOT>\\tmp","holder_pid":"<n>","id":"t1","killed":false,"lost":false,"name":"agent","pid":"<n>","rows":24,"state":"running","status":{"badge":"idle","blocked_children":0,"confidence":0.9,"hooks_accepted":0,"hooks_rejected":0,"records":0,"running_children":0,"since":"<n>","source":"osc","state":"idle","turn":"<n>"}}]} stderr ""
pod wait agent --status idle --timeout 30 -> exit 0 stdout "t1 idle (turn 1)\n" stderr ""
pod send agent after restart -> exit 0 stdout "" stderr ""
pod wait agent --output > after restart --timeout 30 -> exit 0 stdout "t1 matched\n" stderr ""
pod wait agent --status idle --timeout 30 -> exit 0 stdout "t1 idle (turn 2)\n" stderr ""
agent submissions "hello acceptance\n<<<END pastes=0>>>\nafter restart\n<<<END pastes=0>>>\n"
pod kill agent -> exit 0 stdout "t1 killed (code 1)\n" stderr ""
pod wait agent --exit --timeout 30 -> exit 0 stdout "t1 exited (code 1)\n" stderr ""
pod rm agent -> exit 0 stdout "t1 removed\n" stderr ""
pod --json ls --all -> exit 0 stdout {"retention_ms":"<n>","terminals":[]} stderr ""
pod server stop --kill -> exit 0 stdout "podd stopped (killed 0 running terminal(s))\n" stderr ""
"##;
