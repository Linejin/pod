//! Shared harness: the real `podd` and `pod-holder` binaries on a test-only
//! pipe, state directory and worktrees root. Prebuilt fixture binaries and
//! bounded waits are in [`fixtures`].

#![allow(dead_code)]

pub mod fixtures;
#[allow(unused_imports)]
pub use fixtures::*;

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use podd::client::{Client, ClientError};
use podd::protocol::*;

pub struct Daemon {
    pub child: Child,
    pub pipe: String,
    pub state_dir: PathBuf,
}

pub fn unique(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // Tests starting in the same clock tick (seen under CPU load) would
    // otherwise share a directory.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("podd-test-{tag}-{}-{nanos}-{seq}", std::process::id())
}

impl Daemon {
    /// Where this daemon puts task worktrees.
    pub fn worktrees_root(&self) -> PathBuf {
        self.state_dir.join("work trees")
    }

    pub fn start(tag: &str) -> Self {
        let name = unique(tag);
        let state_dir = std::env::temp_dir().join(&name);
        Self::start_on(format!(r"\\.\pipe\{name}"), state_dir)
    }

    pub fn start_on(pipe: String, state_dir: PathBuf) -> Self {
        Self::start_with(pipe, state_dir, &[])
    }

    /// Like [`Daemon::start_on`] with extra `podd` arguments.
    pub fn start_with(pipe: String, state_dir: PathBuf, extra: &[&str]) -> Self {
        Self::start_full(pipe, state_dir, extra, &[])
    }

    /// A fresh daemon with extra environment variables (e.g. `POD_FORGE`).
    pub fn start_env(tag: &str, env: &[(&str, &str)]) -> Self {
        let name = unique(tag);
        let state_dir = std::env::temp_dir().join(&name);
        Self::start_full(format!(r"\\.\pipe\{name}"), state_dir, &[], env)
    }

    pub fn start_full(
        pipe: String,
        state_dir: PathBuf,
        extra: &[&str],
        env: &[(&str, &str)],
    ) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_podd"))
            // Session discovery (docs/18): never the user's real agent stores
            // or processes; tests that need stores pass their own dirs.
            .env(
                "POD_DISCOVER_CLAUDE_DIR",
                state_dir.join("agent-stores").join("claude"),
            )
            .env(
                "POD_DISCOVER_CODEX_DIR",
                state_dir.join("agent-stores").join("codex"),
            )
            .env("POD_DISCOVER_PROCESSES", "off")
            // Worker resource gate (docs/19 §5.1): never this machine's real
            // load in tests; S6 tests inject a meter and limits.
            .env("POD_MAX_WORKERS", "64")
            .env("POD_ORCH_CPU_LIMIT", "off")
            .env("POD_ORCH_MIN_FREE_MEM", "off")
            .env("POD_ORCH_MIN_FREE_DISK", "off")
            .envs(env.iter().copied())
            .args(extra)
            .args([
                "--pipe",
                &pipe,
                "--holder",
                env!("CARGO_BIN_EXE_pod-holder"),
            ])
            .arg("--state-dir")
            .arg(&state_dir)
            // A space on purpose: paths with spaces must work end to end.
            .arg("--worktrees-root")
            .arg(state_dir.join("work trees"))
            // Launcher markers that must not reach terminals (env sanitizing).
            .env("NO_COLOR", "1")
            .env("CLAUDECODE", "1")
            .env("ORCA_TERMINAL_HANDLE", "term_test")
            // Never the user's real %APPDATA%\Pod\mods.
            .env("POD_MODS_DIR", state_dir.join("extensions"))
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

    pub fn client(&self) -> Client {
        Client::connect(&self.pipe).expect("connect")
    }

    /// Stop the daemon process, optionally killing the terminals.
    pub fn stop(mut self, kill: bool) -> (String, PathBuf) {
        let _ = self.client().call::<ServerStop>(&StopParams { kill });
        self.wait_exit();
        let pipe = std::mem::take(&mut self.pipe);
        let dir = std::mem::take(&mut self.state_dir);
        std::mem::forget(self);
        (pipe, dir)
    }

    pub fn wait_exit(&mut self) {
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

pub fn cwd() -> Option<String> {
    Some(std::env::temp_dir().display().to_string())
}

pub fn powershell(c: &mut Client, name: &str) -> TerminalInfo {
    c.call::<TerminalRun>(&RunParams {
        argv: vec!["powershell".into(), "-NoLogo".into(), "-NoProfile".into()],
        cwd: cwd(),
        name: Some(name.into()),
        ..Default::default()
    })
    .expect("run powershell")
}

pub fn send(c: &mut Client, t: &str, text: &str) {
    c.call::<TerminalSend>(&SendParams {
        target: TerminalRef::parse(t),
        text: text.into(),
        enter: true,
        confirm: false, // exactly one Enter (tests step fixtures with it)
    })
    .expect("send");
}

pub fn wait_output(c: &mut Client, t: &str, needle: &str) -> WaitResult {
    let w = c
        .call::<TerminalWait>(&WaitParams {
            target: TerminalRef::parse(t),
            until: WaitUntil::OutputContains(needle.into()),
            timeout_ms: Some(30_000),
        })
        .expect("wait");
    assert_eq!(
        w.outcome,
        WaitOutcome::Matched,
        "{needle:?} not seen in {t}"
    );
    w
}

pub fn read(c: &mut Client, t: &str, mode: ReadMode) -> Vec<String> {
    c.call::<TerminalRead>(&ReadParams {
        target: TerminalRef::parse(t),
        mode,
        lines: None,
        // As the screen is (tests assert on what programs print).
        raw: true,
    })
    .expect("read")
    .lines
}

pub fn process_alive(pid: u32) -> bool {
    let out = Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

pub fn rpc_code<T: std::fmt::Debug>(r: Result<T, ClientError>) -> i64 {
    match r {
        Err(ClientError::Rpc(e)) => e.code,
        other => panic!("expected an RPC error, got {other:?}"),
    }
}
