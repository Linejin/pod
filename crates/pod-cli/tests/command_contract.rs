//! `pod` command contract (refactor card D-P3, docs/20-refactor/daemon.md
//! §3): the help of every command, usage errors, text and JSON output and
//! exit codes, taken from the master binary before `main.rs` was split.
//!
//! `fixtures/commands.json` holds each case's argv, exit code and normalized
//! stdout/stderr. Normalized: the temp root, pipe, run id, workspace, version,
//! process ids, uptime and waited milliseconds become placeholders; JSON
//! output compares without key order, and its `*pid` and `*_ms` numbers
//! become placeholders. `pod server schema` must print podd's wire golden
//! (`crates/podd/tests/fixtures/schema-v21.json`).
//!
//! The connection policy is checked on a temp-only pipe: help, guide,
//! schema, machine, usage errors and `server status|stop` never start a
//! daemon; an ordinary command starts one.
//!
//! Needs `podd.exe` and `pod-holder.exe` next to `pod.exe` (`cargo build -p
//! podd`) and git. Everything the daemon writes stays under a temp root,
//! USERNAME is a run id (so the default pipe is never the user's), and the
//! terminals run only `cmd /c exit 3` and `ping`.
//!
//! Rewrite the fixture only for an intended output change, then review the
//! diff: `POD_CONTRACT_BLESS=1 cargo test -p pod-cli --test command_contract`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use podd::client::{Client, ClientError, ProcessWatch};
use podd::protocol::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const POD: &str = env!("CARGO_BIN_EXE_pod");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const BLESS: &str = "POD_CONTRACT_BLESS";
/// `pod server schema` prints this (compared with podd's golden instead).
const SCHEMA_MARK: &str = "<podd/tests/fixtures/schema-v21.json>";

fn manifest_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn fixture_path() -> PathBuf {
    manifest_dir()
        .join("tests")
        .join("fixtures")
        .join("commands.json")
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
struct Case {
    name: String,
    args: Vec<String>,
    exit: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stdout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stdout_json: Option<Value>,
    stderr: String,
}

#[derive(Serialize, Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

struct Env {
    root: PathBuf,
    run_id: String,
    pipe: String,
    /// (text, placeholder), longest first.
    replace: Vec<(String, &'static str)>,
    pids: BTreeSet<u32>,
    cases: Vec<Case>,
}

impl Env {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let run_id = format!("pod-contract-{}-{nanos}", std::process::id());
        let root = std::env::temp_dir().join(&run_id);
        for d in [
            "home",
            "roaming",
            "local",
            "tmp",
            "worktrees",
            "mods",
            "agents/claude",
            "agents/codex",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(
            root.join("home").join(".gitconfig"),
            "[user]\n\tname = Pod Test\n\temail = pod-test@example.invalid\n",
        )
        .unwrap();
        let podd = Path::new(POD).with_file_name("podd.exe");
        assert!(
            podd.exists() && podd.with_file_name("pod-holder.exe").exists(),
            "{} / pod-holder.exe missing: build them first (cargo build -p podd)",
            podd.display()
        );
        let pipe = format!(r"\\.\pipe\{run_id}");
        let mut replace: Vec<(String, &'static str)> = Vec::new();
        let mut paths = vec![(root.display().to_string(), "<ROOT>")];
        if let Ok(long) = std::fs::canonicalize(&root) {
            paths.push((plain(&long), "<ROOT>"));
        }
        let bin = Path::new(POD).parent().unwrap();
        paths.push((bin.display().to_string(), "<BIN>"));
        let workspace = manifest_dir().join("..").join("..");
        for (dir, mark) in [(bin, "<BIN>"), (workspace.as_path(), "<WORKSPACE>")] {
            if let Ok(long) = std::fs::canonicalize(dir) {
                paths.push((plain(&long), mark));
            }
        }
        for (p, mark) in paths {
            replace.push((p.replace('\\', "/"), mark));
            replace.push((p, mark));
        }
        replace.push((format!(r"\\.\pipe\podd-{run_id}"), "<DEFAULT_PIPE>"));
        replace.push((pipe.clone(), "<PIPE>"));
        replace.push((run_id.clone(), "<RUN>"));
        replace.sort_by_key(|(s, _)| std::cmp::Reverse(s.len()));
        replace.dedup();
        Self {
            root,
            run_id,
            pipe,
            replace,
            pids: BTreeSet::new(),
            cases: Vec::new(),
        }
    }

    fn command(&self, program: &str) -> Command {
        let mut c = Command::new(program);
        let r = &self.root;
        c.env("LOCALAPPDATA", r.join("local"))
            .env("APPDATA", r.join("roaming"))
            .env("USERPROFILE", r.join("home"))
            .env("HOME", r.join("home"))
            .env("TEMP", r.join("tmp"))
            .env("TMP", r.join("tmp"))
            .env("USERNAME", &self.run_id)
            .env("POD_WORKTREES_ROOT", r.join("worktrees"))
            .env("POD_MODS_DIR", r.join("mods"))
            .env("POD_DISCOVER_CLAUDE_DIR", r.join("agents").join("claude"))
            .env("POD_DISCOVER_CODEX_DIR", r.join("agents").join("codex"))
            .env("POD_DISCOVER_PROCESSES", "off")
            .env("POD_PR_POLL", "off")
            .env("NO_COLOR", "1")
            .current_dir(r.join("tmp"))
            .stdin(Stdio::null());
        for k in [
            "POD_SOCKET",
            "POD_TERMINAL_ID",
            "POD_DAEMON",
            "POD_HOLDER",
            "POD_CONFIG_DIR",
            "POD_STOP_TIMEOUT",
            "CLICOLOR_FORCE",
            "CLICOLOR",
        ] {
            c.env_remove(k);
        }
        c
    }

    /// `--pipe <this run's pipe>` + args.
    fn p(&self, args: &[&str]) -> Vec<String> {
        let mut v = vec!["--pipe".to_string(), self.run_id.clone()];
        v.extend(args.iter().map(|a| a.to_string()));
        v
    }

    fn running(&self) -> Option<Client> {
        match Client::connect(&self.pipe) {
            Ok(c) => Some(c),
            Err(ClientError::NotRunning) => None,
            Err(e) => panic!("probe {}: {e}", self.pipe),
        }
    }

    /// Process ids the daemon reports (its own, terminals', holders').
    fn learn_pids(&mut self) {
        let Some(mut c) = self.running() else { return };
        if let Ok(s) = c.call::<ServerStatus>(&Empty {}) {
            self.pids.insert(s.pid);
        }
        if let Ok(l) = c.call::<TerminalList>(&Empty {}) {
            for t in l.terminals {
                self.pids.extend(t.pid);
                self.pids.extend(t.holder_pid);
            }
        }
    }

    fn text(&self, s: &str) -> String {
        let mut s = s.to_string();
        for (from, to) in &self.replace {
            s = s.replace(from.as_str(), to);
        }
        let before = s.clone();
        for pid in &self.pids {
            s = replace_number(&s, &pid.to_string(), "<PID>");
        }
        if s != before {
            // Column widths follow the length of the process ids.
            s = s
                .lines()
                .map(collapse_spaces)
                .collect::<Vec<_>>()
                .join("\n")
                + if before.ends_with('\n') { "\n" } else { "" };
        }
        let duration = |m: &str| m.chars().all(|c| c.is_ascii_digit() || "hms".contains(c));
        s = mask_between(&s, ", up ", ",", "<UP>", duration);
        s = mask_between(&s, "since=", " ago", "<AGO>", duration);
        mask_between(&s, " after ", " ms", "<N>", |m| {
            m.chars().all(|c| c.is_ascii_digit())
        })
        .replace(VERSION, "<VERSION>")
    }

    fn json(&self, v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, v) in m.iter_mut() {
                    let time = k.ends_with("_ms") || k == "since";
                    if (k.ends_with("pid") || time) && v.is_number() {
                        *v = Value::String("<N>".into());
                    } else if k == "conpty" && v.is_string() {
                        // bundled:<dll next to podd> or the system one.
                        *v = Value::String("<CONPTY>".into());
                    } else {
                        self.json(v);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|v| self.json(v)),
            Value::String(s) => *s = self.text(s),
            _ => {}
        }
    }

    fn case<S: AsRef<str>>(&mut self, name: &str, args: &[S]) {
        assert!(
            !self.cases.iter().any(|c| c.name == name),
            "duplicate case {name}"
        );
        self.learn_pids();
        let out = self
            .command(POD)
            .args(args.iter().map(AsRef::as_ref))
            .output()
            .expect("run pod");
        self.learn_pids();
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let (stdout, stdout_json) = if name == "server schema" && out.status.success() {
            let printed: Value = serde_json::from_str(&stdout).expect("schema JSON");
            let golden: Value = serde_json::from_slice(
                &std::fs::read(manifest_dir().join("../podd/tests/fixtures/schema-v21.json"))
                    .unwrap(),
            )
            .unwrap();
            assert!(
                printed == golden,
                "pod server schema differs from podd's golden"
            );
            (Some(SCHEMA_MARK.to_string()), None)
        } else {
            let t = stdout.trim_start();
            match serde_json::from_str::<Value>(&stdout) {
                Ok(mut v) if t.starts_with('{') || t.starts_with('[') => {
                    self.json(&mut v);
                    (None, Some(v))
                }
                _ => (Some(self.text(&stdout)), None),
            }
        };
        self.cases.push(Case {
            name: name.to_string(),
            args: args.iter().map(|a| self.text(a.as_ref())).collect(),
            exit: out.status.code().unwrap_or(-1),
            stdout,
            stdout_json,
            stderr: self.text(&String::from_utf8_lossy(&out.stderr)),
        });
    }

    /// `pod <path> --help` for every command and subcommand, found through
    /// the `Commands:` lists themselves.
    fn help_tree(&mut self) {
        let mut queue: Vec<Vec<String>> = vec![Vec::new()];
        while let Some(path) = queue.pop() {
            let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
            args.push("--help");
            let name = format!("help {}", path.join(" ")).trim_end().to_string();
            self.case(&name, &args);
            let help = self
                .cases
                .last()
                .unwrap()
                .stdout
                .clone()
                .unwrap_or_default();
            for sub in subcommands(&help).into_iter().rev() {
                let mut p = path.clone();
                p.push(sub);
                queue.push(p);
            }
        }
    }

    fn git(&self, dir: &Path, args: &[&str]) {
        let out = self
            .command("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .output()
            .expect("run git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn stop_daemon(&self) {
        if let Some(mut c) = self.running()
            && let Ok(s) = c.call::<ServerStatus>(&Empty {})
        {
            let watch = ProcessWatch::open(s.pid);
            let _ = c.call::<ServerStop>(&StopParams { kill: true });
            drop(c);
            if let Some(w) = watch {
                w.wait(Duration::from_secs(20));
            }
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        self.stop_daemon();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A path without the `\\?\` prefix of `canonicalize`.
fn plain(p: &Path) -> String {
    let s = p.display().to_string();
    s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
}

/// `num` replaced where it stands alone (no letter or digit next to it).
fn replace_number(s: &str, num: &str, with: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(num) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + num.len()..].chars().next();
        let alone = !before.is_some_and(|c| c.is_ascii_alphanumeric())
            && !after.is_some_and(|c| c.is_ascii_alphanumeric());
        out.push_str(&rest[..i]);
        out.push_str(if alone { with } else { num });
        rest = &rest[i + num.len()..];
    }
    out.push_str(rest);
    out
}

/// Runs of two or more spaces as two spaces.
fn collapse_spaces(line: &str) -> String {
    let mut out = String::new();
    let mut spaces = 0;
    for c in line.chars() {
        if c == ' ' {
            spaces += 1;
            continue;
        }
        out.push_str(match spaces {
            0 => "",
            1 => " ",
            _ => "  ",
        });
        spaces = 0;
        out.push(c);
    }
    out.push_str(match spaces {
        0 => "",
        1 => " ",
        _ => "  ",
    });
    out
}

/// The text between `start` and the next `end` replaced when `ok` says so.
fn mask_between(s: &str, start: &str, end: &str, with: &str, ok: impl Fn(&str) -> bool) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(start) {
        let from = i + start.len();
        out.push_str(&rest[..from]);
        rest = &rest[from..];
        if let Some(j) = rest.find(end)
            && j > 0
            && ok(&rest[..j])
        {
            out.push_str(with);
            rest = &rest[j..];
        }
    }
    out.push_str(rest);
    out
}

/// Names in the `Commands:` list of a clap help page (without `help`).
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
            // Continuation lines of a long description are indented further.
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

/// The cases, in order. Each depends on the daemon state the ones before it
/// left behind.
fn run_cases(t: &mut Env) {
    // ── no daemon: help, usage errors, remote limits, local-only commands ──
    t.help_tree();
    t.case("version", &["--version"]);
    for (name, args) in [
        ("usage: no command", &[][..]),
        ("usage: unknown command", &["nope"]),
        ("usage: keys without keys", &["keys", "t1"]),
        ("usage: wait without target", &["wait"]),
        (
            "usage: wait exit and output",
            &["wait", "t1", "--exit", "--output", "x"],
        ),
        (
            "usage: wait terminal and gate",
            &["wait", "t1", "--gate", "g1"],
        ),
        ("usage: run without argv", &["run"]),
        (
            "usage: task new bad agent",
            &["task", "new", "a", "b", "--agent", "vim"],
        ),
        (
            "usage: pipe and machine",
            &["--pipe", "x", "--machine", "m", "ls"],
        ),
        ("usage: server without command", &["server"]),
        ("usage: repo without command", &["repo"]),
        ("usage: task without command", &["task"]),
        ("usage: resize without rows", &["resize", "t1", "80"]),
        (
            "usage: setup value",
            &["task", "new", "a", "--setup", "maybe"],
        ),
    ] {
        t.case(name, args);
    }
    for (name, args) in [
        ("remote: attach", &["--machine", "m", "attach", "t1"][..]),
        ("remote: remote-stdio", &["--machine", "m", "remote-stdio"]),
        ("remote: hooks", &["--machine", "m", "hooks", "status"]),
        ("remote: machine", &["--machine", "m", "machine", "ls"]),
        ("remote: server stop", &["--machine", "m", "server", "stop"]),
        (
            "remote: inherit-env",
            &["--machine", "m", "run", "--inherit-env", "--", "cmd"],
        ),
        ("remote: unknown machine ls", &["--machine", "m", "ls"]),
        (
            "remote: unknown machine server status",
            &["--machine", "m", "server", "status"],
        ),
        (
            "remote: unknown machine repo add",
            &["--machine", "m", "repo", "add", "x"],
        ),
    ] {
        t.case(name, args);
    }
    t.case("guide", &t.p(&["guide"]));
    t.case("server schema", &t.p(&["server", "schema"]));
    t.case("machine ls", &t.p(&["machine", "ls"]));
    t.case("machine ls json", &t.p(&["--json", "machine", "ls"]));
    t.case("server status, none", &t.p(&["server", "status"]));
    t.case(
        "server status json, none",
        &t.p(&["--json", "server", "status"]),
    );
    t.case("server stop, none", &t.p(&["server", "stop"]));
    t.case(
        "server stop kill, none",
        &t.p(&["server", "stop", "--kill"]),
    );
    t.case("server status, default pipe", &["server", "status"]);
    t.case("run bad env", &t.p(&["run", "--env", "=x", "--", "cmd"]));
    t.case("run bad env 2", &t.p(&["run", "--env", "x", "--", "cmd"]));
    assert!(
        t.running().is_none(),
        "a command without a connection started podd on {}",
        t.pipe
    );
    assert!(
        matches!(
            Client::connect(&format!(r"\\.\pipe\podd-{}", t.run_id)),
            Err(ClientError::NotRunning)
        ),
        "server status started podd on the default pipe"
    );

    // ── an ordinary command starts the daemon ──
    t.case("ls, empty", &t.p(&["ls"]));
    assert!(t.running().is_some(), "pod ls did not start podd");
    t.case("ls json, empty", &t.p(&["--json", "ls"]));
    t.case("server status", &t.p(&["server", "status"]));
    t.case("server status json", &t.p(&["--json", "server", "status"]));

    // ── terminals ──
    for (name, args) in [
        ("read unknown", &["read", "nope"][..]),
        ("status unknown", &["status", "nope"]),
        ("ack unknown", &["ack", "nope"]),
        ("kill unknown", &["kill", "nope"]),
        ("rm unknown", &["rm", "nope"]),
        ("keep unknown", &["keep", "nope"]),
        ("resize unknown", &["resize", "nope", "80", "24"]),
        ("send unknown", &["send", "nope", "x"]),
        ("keys unknown", &["keys", "nope", "enter"]),
        ("wait unknown", &["wait", "nope", "--timeout", "1"]),
        (
            "wait status unknown",
            &["wait", "nope", "--status", "idle", "--timeout", "1"],
        ),
        (
            "wait gate unknown",
            &["wait", "--gate", "g9", "--timeout", "1"],
        ),
        (
            "run bad cwd",
            &["run", "--cwd", "no-such-dir-here", "--", "cmd"],
        ),
    ] {
        t.case(name, &t.p(args));
    }
    let cwd = t.root.join("tmp").display().to_string();
    t.case(
        "run a",
        &t.p(&[
            "run", "--name", "a", "--cols", "80", "--rows", "24", "--", "cmd", "/c", "exit 3",
        ]),
    );
    t.case("wait a", &t.p(&["wait", "a", "--timeout", "60"]));
    t.case(
        "wait a json",
        &t.p(&["--json", "wait", "a", "--timeout", "60"]),
    );
    t.case(
        "wait a output, exited",
        &t.p(&["wait", "a", "--output", "zzz", "--timeout", "5"]),
    );
    t.case("ls, hidden", &t.p(&["ls"]));
    t.case("ls all", &t.p(&["ls", "--all"]));
    t.case("ls all json", &t.p(&["--json", "ls", "-a"]));
    t.case("keep a", &t.p(&["keep", "a"]));
    t.case("keep a json", &t.p(&["--json", "keep", "a"]));
    t.case("unkeep a", &t.p(&["keep", "a", "--unkeep"]));
    t.case("status a", &t.p(&["status", "a"]));
    t.case("status a json", &t.p(&["--json", "status", "a"]));
    t.case("ack a", &t.p(&["ack", "a"]));
    t.case("ack a json", &t.p(&["--json", "ack", "a"]));
    t.case("resize a, exited", &t.p(&["resize", "a", "100", "30"]));
    t.case("send a, exited", &t.p(&["send", "a", "hi"]));
    t.case("keys a, exited", &t.p(&["keys", "a", "enter"]));
    t.case("kill a, exited", &t.p(&["kill", "a"]));
    t.case("rm a", &t.p(&["rm", "a"]));
    t.case("ls all, after rm", &t.p(&["ls", "--all"]));

    t.case(
        "run b",
        &t.p(&[
            "--json",
            "run",
            "--name",
            "b",
            "--cwd",
            &cwd,
            "--",
            "ping",
            "-n",
            "120",
            "127.0.0.1",
        ]),
    );
    t.case(
        "wait b output timeout",
        &t.p(&["wait", "b", "--output", "zzz-never", "--timeout", "0.3"]),
    );
    t.case("ls, running", &t.p(&["ls"]));
    t.case("send b no-enter", &t.p(&["send", "b", "x", "--no-enter"]));
    t.case("keys b json", &t.p(&["--json", "keys", "b", "esc"]));
    t.case("keys b", &t.p(&["keys", "b", "backspace"]));
    t.case("resize b", &t.p(&["resize", "b", "100", "30"]));
    t.case(
        "resize b json",
        &t.p(&["--json", "resize", "b", "90", "30"]),
    );
    t.case("rm b, running", &t.p(&["rm", "b"]));
    t.case("kill b", &t.p(&["kill", "b"]));
    t.case("wait b", &t.p(&["wait", "b", "--exit", "--timeout", "60"]));
    t.case("rm b json", &t.p(&["--json", "rm", "b"]));
    t.case(
        "run c",
        &t.p(&[
            "run",
            "--name",
            "c",
            "--cwd",
            &cwd,
            "--",
            "ping",
            "-n",
            "120",
            "127.0.0.1",
        ]),
    );
    t.case("rm c force", &t.p(&["rm", "c", "--force"]));

    // ── repositories and tasks ──
    let repo = t.root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    t.git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("README.md"), "fixture\n").unwrap();
    t.git(&repo, &["add", "README.md"]);
    t.git(&repo, &["commit", "-q", "-m", "init"]);
    let repo_s = repo.display().to_string();
    t.case("repo ls, empty", &t.p(&["repo", "ls"]));
    t.case("repo ls json, empty", &t.p(&["--json", "repo", "ls"]));
    t.case(
        "repo add missing",
        &t.p(&["repo", "add", &format!("{repo_s}-missing")]),
    );
    t.case(
        "repo add",
        &t.p(&["repo", "add", &repo_s, "--name", "demo"]),
    );
    t.case("repo add again", &t.p(&["repo", "add", &repo_s]));
    t.case("repo ls", &t.p(&["repo", "ls"]));
    t.case("repo ls json", &t.p(&["--json", "repo", "ls"]));
    t.case("task ls, empty", &t.p(&["task", "ls"]));
    t.case(
        "task ls json, empty",
        &t.p(&["--json", "task", "ls", "--repo", "demo"]),
    );
    t.case("task show unknown", &t.p(&["task", "show", "demo/nope"]));
    t.case("task new unknown repo", &t.p(&["task", "new", "nope", "x"]));
    t.case(
        "task new brief and brief-file",
        &t.p(&[
            "task",
            "new",
            "demo",
            "x",
            "--brief",
            "a",
            "--brief-file",
            "b",
        ]),
    );
    t.case(
        "task new",
        &t.p(&[
            "task",
            "new",
            "demo",
            "t1",
            "--no-focus",
            "--",
            "ping",
            "-n",
            "120",
            "127.0.0.1",
        ]),
    );
    t.case(
        "task new json",
        &t.p(&[
            "--json",
            "task",
            "new",
            "demo",
            "t2",
            "--no-focus",
            "--base",
            "main",
            "--",
            "ping",
            "-n",
            "120",
            "127.0.0.1",
        ]),
    );
    t.case("task diff", &t.p(&["task", "diff", "demo/t1"]));
    t.case(
        "task diff json",
        &t.p(&["--json", "task", "diff", "demo/t1"]),
    );
    t.case(
        "task touch",
        &t.p(&["task", "touch", "demo/t1", "README.md"]),
    );
    t.case(
        "task touch json",
        &t.p(&["--json", "task", "touch", "demo/t1"]),
    );
    t.case("task ls", &t.p(&["task", "ls"]));
    t.case("task show", &t.p(&["task", "show", "demo/t1"]));
    t.case(
        "task show json",
        &t.p(&["--json", "task", "show", "demo/t1"]),
    );
    t.case("repo rm, has tasks", &t.p(&["repo", "rm", "demo"]));
    t.case(
        "task rm",
        &t.p(&["task", "rm", "demo/t1", "--force", "--delete-branch"]),
    );
    t.case(
        "task rm json",
        &t.p(&["--json", "task", "rm", "demo/t2", "--force"]),
    );
    t.case("task rm unknown", &t.p(&["task", "rm", "demo/t1"]));
    t.case("repo rm", &t.p(&["repo", "rm", "demo"]));
    t.case(
        "repo rm json, unknown",
        &t.p(&["--json", "repo", "rm", "demo"]),
    );

    // ── stopping ──
    t.case("server stop json", &t.p(&["--json", "server", "stop"]));
    assert!(
        t.running().is_none(),
        "podd still runs after pod server stop"
    );
    t.case("ls, restarted", &t.p(&["ls", "--all"]));
    t.case("server stop kill", &t.p(&["server", "stop", "--kill"]));
    t.case("server status, stopped", &t.p(&["server", "status"]));
}

#[test]
fn pod_commands_match_the_master_contract() {
    let mut t = Env::new();
    run_cases(&mut t);
    let now = std::mem::take(&mut t.cases);
    let path = fixture_path();
    if std::env::var(BLESS).is_ok_and(|v| v == "1") {
        let text = serde_json::to_string_pretty(&Fixture { cases: now }).unwrap() + "\n";
        std::fs::write(&path, text).unwrap();
        eprintln!("wrote {}", path.display());
        return;
    }
    let golden: Fixture = serde_json::from_slice(
        &std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display())),
    )
    .unwrap();
    let mut diffs = Vec::new();
    let names = |cs: &[Case]| cs.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    if names(&now) != names(&golden.cases) {
        diffs.push(format!(
            "case list differs:\n  now:    {:?}\n  golden: {:?}",
            names(&now),
            names(&golden.cases)
        ));
    }
    for g in &golden.cases {
        match now.iter().find(|c| c.name == g.name) {
            Some(c) if c == g => {}
            Some(c) => diffs.push(format!(
                "case {:?} differs:\n  now:    {}\n  golden: {}",
                g.name,
                serde_json::to_string(c).unwrap(),
                serde_json::to_string(g).unwrap()
            )),
            None => {}
        }
    }
    assert!(
        diffs.is_empty(),
        "{} difference(s) from {}:\n{}",
        diffs.len(),
        path.display(),
        diffs.join("\n")
    );
}
