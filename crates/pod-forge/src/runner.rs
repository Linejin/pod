//! Process spawning for `git` and `gh`.
//!
//! Windows rules (same as podd's `git.rs`): `CREATE_NO_WINDOW` (podd runs
//! detached, a console child would open a window), stdin from a string or
//! null, no interactive prompts (`GH_PROMPT_DISABLED`, `GIT_TERMINAL_PROMPT=0`),
//! no colors or update notices, inherited `GIT_DIR`-style variables removed.
//! A run longer than its timeout is killed and reported as
//! [`ForgeErrorKind::Timeout`].

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use crate::bounded::{self, End, Keep};

/// After the process exits, how long its output pipes may stay open. A
/// grandchild that inherited them (a credential helper, `start /b`) would
/// otherwise keep the caller (podd's PR lock) waiting until it exits; after
/// the grace the leftovers are killed with the process tree.
const AFTER_EXIT_GRACE: Duration = Duration::from_secs(2);
/// `gh` JSON answers are small; more than this is an error, not a buffer
/// that grows until podd runs out of memory (security-03).
pub const MAX_STDOUT: usize = 16 * 1024 * 1024;
/// Only the end of stderr is needed to classify a failure.
const STDERR_TAIL: usize = 64 * 1024;

use crate::error::{ForgeError, ForgeErrorKind};

/// The result of one finished process.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RunOutput {
    /// `None` when killed by a signal (not on Windows) or unknown.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl RunOutput {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }
}

/// One command to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// `git` or `gh` (resolved by the runner).
    pub program: String,
    pub args: Vec<String>,
    pub stdin: Option<String>,
    pub timeout: Duration,
}

impl Invocation {
    pub fn new(program: &str, args: &[&str], timeout: Duration) -> Self {
        Self {
            program: program.into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            stdin: None,
            timeout,
        }
    }

    /// `program sub sub` (the first two arguments, no values) for messages.
    pub fn short_line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(
                self.args
                    .iter()
                    .take(2)
                    .map(String::as_str)
                    .filter(|a| !a.starts_with('-')),
            )
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `program arg arg…` for logs and recorded fixtures.
    pub fn line(&self) -> String {
        std::iter::once(self.program.as_str())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Runs invocations. [`ProcessRunner`] spawns real processes; tests use a
/// recorded runner that answers from fixtures.
pub trait Runner: Send + Sync {
    /// Runs to completion. `Err` only when the process could not run at all
    /// (missing tool) or timed out; a non-zero exit is an `Ok` output.
    fn run(&self, dir: &Path, inv: &Invocation) -> Result<RunOutput, ForgeError>;
}

/// Environment overrides for the executables.
pub const ENV_GH: &str = "POD_GH";
pub const ENV_GIT: &str = "POD_GIT";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Variables that would point git somewhere other than the worktree.
const GIT_ENV_OVERRIDES: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_PREFIX",
];

#[derive(Debug, Clone, Default)]
pub struct ProcessRunner;

impl ProcessRunner {
    fn command(&self, dir: &Path, inv: &Invocation) -> Command {
        let exe = match inv.program.as_str() {
            "gh" => std::env::var_os(ENV_GH).unwrap_or_else(|| "gh".into()),
            "git" => std::env::var_os(ENV_GIT).unwrap_or_else(|| "git".into()),
            other => other.into(),
        };
        let mut cmd = Command::new(exe);
        if inv.program == "git" {
            cmd.arg("-C").arg(dir).args([
                "-c",
                "core.longpaths=true",
                "-c",
                "core.quotepath=false",
            ]);
        }
        cmd.args(&inv.args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_NO_UPDATE_NOTIFIER", "1")
            .env("GH_SPINNER_DISABLED", "1")
            .env("NO_COLOR", "1")
            .env("CLICOLOR", "0")
            .env("LC_ALL", "C");
        for v in GIT_ENV_OVERRIDES {
            cmd.env_remove(v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        #[cfg(not(windows))]
        let _ = CREATE_NO_WINDOW;
        cmd
    }
}

impl ProcessRunner {
    /// [`Runner::run`] with an explicit stdout cap (tests use a small one).
    pub fn run_capped(
        &self,
        dir: &Path,
        inv: &Invocation,
        max_stdout: usize,
    ) -> Result<RunOutput, ForgeError> {
        let mut opts = bounded::Options::new(inv.timeout, Keep::Cap(max_stdout));
        opts.stderr = Keep::Tail(STDERR_TAIL);
        opts.stdin = inv.stdin.clone().map(String::into_bytes);
        opts.after_exit_grace = AFTER_EXIT_GRACE;
        let o = bounded::run(&mut self.command(dir, inv), &opts, None).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ForgeError::new(
                    ForgeErrorKind::ToolMissing,
                    format!("{} is not installed or not on PATH", inv.program),
                )
            } else {
                ForgeError::failed(format!("cannot run {}: {e}", inv.program))
            }
        })?;
        match o.end {
            End::Exited { code, .. } => Ok(RunOutput {
                code,
                stdout: o.stdout.text(),
                stderr: o.stderr.text(),
            }),
            End::TimedOut | End::Cancelled => Err(ForgeError::new(
                ForgeErrorKind::Timeout,
                format!(
                    "{} did not finish within {} s",
                    inv.short_line(),
                    inv.timeout.as_secs()
                ),
            )),
            End::TooLarge { stream, limit } => Err(ForgeError::failed(format!(
                "{} output too large: {stream} went over {limit} bytes; stopped",
                inv.short_line()
            ))),
        }
    }
}

impl Runner for ProcessRunner {
    fn run(&self, dir: &Path, inv: &Invocation) -> Result<RunOutput, ForgeError> {
        self.run_capped(dir, inv, MAX_STDOUT)
    }
}

/// Runs `inv` and turns a non-zero exit into a classified [`ForgeError`].
pub fn run_ok(runner: &dyn Runner, dir: &Path, inv: &Invocation) -> Result<RunOutput, ForgeError> {
    let out = runner.run(dir, inv)?;
    if out.ok() {
        Ok(out)
    } else {
        Err(ForgeError::classify(&inv.program, out.code, &out.stderr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn here() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn runs_and_captures_stdin_stdout_stderr() {
        // `git hash-object --stdin` echoes a hash of stdin: stdin and stdout work.
        let mut inv = Invocation::new("git", &["hash-object", "--stdin"], Duration::from_secs(20));
        inv.stdin = Some("hello\n".into());
        let out = ProcessRunner.run(&here(), &inv).unwrap();
        assert!(out.ok(), "{out:?}");
        assert_eq!(
            out.stdout.trim(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
        let bad = ProcessRunner
            .run(
                &here(),
                &Invocation::new("git", &["no-such-subcommand-xyz"], Duration::from_secs(20)),
            )
            .unwrap();
        assert!(!bad.ok());
        assert!(bad.stderr.contains("no-such-subcommand-xyz"), "{bad:?}");
    }

    /// A grandchild holding the output pipes does not keep the run waiting
    /// after the process itself exited; what was written is kept.
    #[cfg(windows)]
    #[test]
    fn inherited_pipes_do_not_block_after_exit() {
        let t = Instant::now();
        let out = ProcessRunner
            .run(
                &here(),
                &Invocation::new(
                    "cmd",
                    &["/c", "start /b ping -n 8 127.0.0.1 >NUL & echo done"],
                    Duration::from_secs(20),
                ),
            )
            .unwrap();
        assert!(out.ok(), "{out:?}");
        assert!(out.stdout.contains("done"), "{out:?}");
        assert!(
            t.elapsed() < Duration::from_secs(5),
            "returned after the grace, not when ping ends: {:?}",
            t.elapsed()
        );
    }

    /// A grandchild holding the pipes past the deadline: the call returns at
    /// the deadline and the grandchild dies with the tree (security-02).
    #[cfg(windows)]
    #[test]
    fn timeout_covers_a_grandchild_holding_the_pipes() {
        let t = Instant::now();
        let e = ProcessRunner
            .run(
                &here(),
                &Invocation::new(
                    "cmd",
                    &[
                        "/c",
                        "start /b ping -n 60 127.0.0.1 >NUL & ping -n 60 127.0.0.1 >NUL",
                    ],
                    Duration::from_millis(800),
                ),
            )
            .unwrap_err();
        assert_eq!(e.kind, ForgeErrorKind::Timeout, "{e:?}");
        assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    }

    /// Output over the cap is an explicit error, not an unbounded buffer
    /// (security-03).
    #[cfg(windows)]
    #[test]
    fn output_over_the_cap_is_an_error() {
        let inv = Invocation::new(
            "cmd",
            &["/c", "for /L %i in (1,1,100000) do @echo line-%i"],
            Duration::from_secs(60),
        );
        let e = ProcessRunner.run_capped(&here(), &inv, 2048).unwrap_err();
        assert_eq!(e.kind, ForgeErrorKind::Failed);
        assert!(e.message.contains("output too large"), "{e:?}");
        let small = Invocation::new("cmd", &["/c", "echo fits"], Duration::from_secs(20));
        assert!(
            ProcessRunner
                .run_capped(&here(), &small, 2048)
                .unwrap()
                .stdout
                .contains("fits")
        );
    }

    #[test]
    fn missing_tool_and_timeout() {
        let e = ProcessRunner
            .run(
                &here(),
                &Invocation::new("pod-no-such-tool-xyz", &[], Duration::from_secs(5)),
            )
            .unwrap_err();
        assert_eq!(e.kind, ForgeErrorKind::ToolMissing);
        // `ping -n 30` runs ~29 s; the 300 ms limit kills it.
        let t = Instant::now();
        let e = ProcessRunner
            .run(
                &here(),
                &Invocation::new(
                    "ping",
                    &["-n", "30", "127.0.0.1"],
                    Duration::from_millis(300),
                ),
            )
            .unwrap_err();
        assert_eq!(e.kind, ForgeErrorKind::Timeout);
        assert!(e.message.contains("did not finish"), "{e:?}");
        let view = Invocation::new(
            "gh",
            &["pr", "view", "3", "--repo", "o/r"],
            Duration::from_secs(30),
        );
        assert_eq!(
            view.short_line(),
            "gh pr view",
            "timeouts name the subcommand"
        );
        assert_eq!(
            Invocation::new("git", &["push", "--porcelain"], Duration::from_secs(1)).short_line(),
            "git push"
        );
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "killed promptly: {:?}",
            t.elapsed()
        );
    }
}
