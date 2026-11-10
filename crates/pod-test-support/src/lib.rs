//! Helpers shared by Pod's tests. A dev-dependency only: nothing shipped
//! links it, and it depends on std alone (no daemon protocol, GUI, network or
//! agent runner). Daemon lifetimes stay in `podd/tests/common`; product
//! fixtures (`pod_editor::fixture`, `pod_forge::fake`) stay in their crates.

/// `git -C <dir> …` in fixture repositories.
pub mod git {
    use std::path::Path;
    use std::process::{Command, Output};

    /// Runs `git -C dir args…` with the inherited environment and git
    /// configuration (nothing added or removed). Panics with the argv, the
    /// exit status and stderr unless git exits 0; otherwise returns stdout
    /// (lossy UTF-8, unchanged: trailing newline kept).
    pub fn git_output(dir: &Path, args: &[&str]) -> String {
        String::from_utf8_lossy(&checked(dir, args).stdout).into_owned()
    }

    /// [`git_output`] for commands whose output the test does not need.
    pub fn git_ok(dir: &Path, args: &[&str]) {
        checked(dir, args);
    }

    fn checked(dir: &Path, args: &[&str]) -> Output {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap_or_else(|e| panic!("run git -C {} {args:?}: {e}", dir.display()));
        assert!(
            out.status.success(),
            "git -C {} {args:?} failed ({}): {}",
            dir.display(),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        out
    }
}

/// Condition waits. [`until`](wait::until) is for "becomes ready" (returns as
/// soon as it holds); [`stays`](wait::stays) is for "does not happen" (watches
/// the whole window).
pub mod wait {
    use std::fmt;
    use std::time::{Duration, Instant};

    /// [`until`] ran out of time.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct TimedOut<O> {
        /// Time from the first probe to the last.
        pub waited: Duration,
        /// Number of probes, at least one.
        pub polls: u32,
        /// What the last probe saw.
        pub last: O,
    }

    impl<O: fmt::Debug> fmt::Display for TimedOut<O> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "not ready after {:?} ({} polls); last seen: {:?}",
                self.waited, self.polls, self.last
            )
        }
    }

    impl<O: fmt::Debug> std::error::Error for TimedOut<O> {}

    /// [`stays`] saw the condition break.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Broke<O> {
        /// Time from the first probe to the one that failed.
        pub after: Duration,
        /// Number of probes, the failing one included.
        pub polls: u32,
        /// What the failing probe saw.
        pub seen: O,
    }

    impl<O: fmt::Debug> fmt::Display for Broke<O> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "condition broke after {:?} (poll {}): {:?}",
                self.after, self.polls, self.seen
            )
        }
    }

    impl<O: fmt::Debug> std::error::Error for Broke<O> {}

    /// Probes until `probe` returns `Ok`, every `every`, for at most
    /// `within`. `Err(observation)` means "not yet". Always probes once, and
    /// once more at the deadline, so a slow machine is not cut short between
    /// polls. Gives the value, or [`TimedOut`] with the last observation.
    pub fn until<T, O>(
        within: Duration,
        every: Duration,
        mut probe: impl FnMut() -> Result<T, O>,
    ) -> Result<T, TimedOut<O>> {
        let start = Instant::now();
        let deadline = start + within;
        let mut polls = 0;
        loop {
            polls += 1;
            let last = match probe() {
                Ok(v) => return Ok(v),
                Err(o) => o,
            };
            let now = Instant::now();
            if now >= deadline {
                return Err(TimedOut {
                    waited: now - start,
                    polls,
                    last,
                });
            }
            std::thread::sleep(every.min(deadline - now));
        }
    }

    /// Probes every `every` for the whole of `during` (first probe at once,
    /// last at or after the end). `Ok` (the number of probes) means the
    /// condition held at every probe; an `Err(observation)` from `probe`
    /// stops at once with [`Broke`].
    pub fn stays<O>(
        during: Duration,
        every: Duration,
        mut probe: impl FnMut() -> Result<(), O>,
    ) -> Result<u32, Broke<O>> {
        let start = Instant::now();
        let end = start + during;
        let mut polls = 0;
        loop {
            polls += 1;
            if let Err(seen) = probe() {
                return Err(Broke {
                    after: start.elapsed(),
                    polls,
                    seen,
                });
            }
            let now = Instant::now();
            if now >= end {
                return Ok(polls);
            }
            std::thread::sleep(every.min(end - now));
        }
    }
}

/// Skipping tests whose external tool is missing (from
/// `tools/test-support/require_tools.rs`, same behaviour).
///
/// Without `POD_REQUIRE_TOOLS=1` a missing tool skips the check with a note on
/// stderr (developer machines). With it (CI) a missing tool fails the test, so
/// a green run never hides an unchecked path (quality-07).
pub mod tools {
    /// Name of the environment variable that turns skips into failures.
    pub const ENV: &str = "POD_REQUIRE_TOOLS";

    /// Call where a test would skip because `what` is unavailable. Returns
    /// normally (caller skips) unless `POD_REQUIRE_TOOLS=1`, then panics.
    pub fn missing(what: &str) {
        missing_with(what, std::env::var(ENV).ok().as_deref());
    }

    fn missing_with(what: &str, require: Option<&str>) {
        if require.is_some_and(|v| v.trim() == "1") {
            panic!("{what} is unavailable and {ENV}=1: refusing to skip");
        }
        eprintln!("{what} is unavailable: skipped (set {ENV}=1 to fail instead)");
    }

    #[cfg(test)]
    mod tests {
        use super::missing_with;

        #[test]
        fn require_tools_skips_without_env() {
            missing_with("x", None);
            missing_with("x", Some("0"));
            missing_with("x", Some(""));
            missing_with("x", Some(" 0 "));
        }

        #[test]
        #[should_panic(expected = "POD_REQUIRE_TOOLS=1")]
        fn require_tools_fails_with_env() {
            missing_with("x", Some("1"));
        }

        #[test]
        #[should_panic(expected = "x is unavailable and POD_REQUIRE_TOOLS=1")]
        fn require_tools_fails_with_padded_env() {
            missing_with("x", Some(" 1\n"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::git::{git_ok, git_output};
    use super::wait::{Broke, TimedOut, stays, until};
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    fn panic_text(f: impl FnOnce() + std::panic::UnwindSafe) -> String {
        let p = std::panic::catch_unwind(f).expect_err("should panic");
        p.downcast_ref::<String>()
            .cloned()
            .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default()
    }

    fn have_git() -> bool {
        let ok = std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !ok {
            crate::tools::missing("git on PATH");
        }
        ok
    }

    #[test]
    fn git_output_returns_stdout_unchanged() {
        if !have_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        git_ok(dir.path(), &["init", "-q"]);
        let out = git_output(dir.path(), &["rev-parse", "--is-inside-work-tree"]);
        assert_eq!(out, "true\n");
    }

    #[test]
    fn git_failure_names_argv_status_and_stderr() {
        if !have_git() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        git_ok(dir.path(), &["init", "-q"]);
        let p = dir.path().to_path_buf();
        let msg = panic_text(move || {
            git_output(&p, &["rev-parse", "--verify", "no-such-ref"]);
        });
        assert!(
            msg.contains(r#"["rev-parse", "--verify", "no-such-ref"] failed ("#),
            "{msg}"
        );
        assert!(msg.contains(&dir.path().display().to_string()), "{msg}");
        assert!(msg.contains("fatal"), "stderr missing: {msg}");
        let p = dir.path().to_path_buf();
        let msg = panic_text(move || git_ok(&p, &["no-such-command"]));
        assert!(msg.contains(r#"["no-such-command"] failed ("#), "{msg}");
    }

    #[test]
    fn until_returns_the_first_success() {
        let n = Cell::new(0);
        let v = until(Duration::from_secs(5), Duration::from_millis(1), || {
            n.set(n.get() + 1);
            if n.get() == 3 {
                Ok("ready")
            } else {
                Err(n.get())
            }
        });
        assert_eq!(v, Ok("ready"));
        assert_eq!(n.get(), 3);
        // Already true: exactly one probe, no sleep.
        let n = Cell::new(0);
        let v: Result<u8, TimedOut<()>> = until(Duration::ZERO, Duration::from_secs(60), || {
            n.set(n.get() + 1);
            Ok(1)
        });
        assert_eq!((v, n.get()), (Ok(1), 1));
    }

    #[test]
    fn until_timeout_reports_the_wait_and_the_last_observation() {
        let n = Cell::new(0u32);
        let e = until(Duration::from_millis(60), Duration::from_millis(5), || {
            n.set(n.get() + 1);
            Err::<(), _>(format!("pid file has {} lines", n.get()))
        })
        .unwrap_err();
        assert!(e.waited >= Duration::from_millis(60), "{e:?}");
        assert!(e.polls >= 2 && e.polls == n.get(), "{e:?}");
        assert_eq!(e.last, format!("pid file has {} lines", n.get()));
        let text = e.to_string();
        assert!(text.starts_with("not ready after "), "{text}");
        assert!(text.contains(&format!("({} polls)", e.polls)), "{text}");
        let last = format!("last seen: \"pid file has {} lines\"", n.get());
        assert!(text.ends_with(&last), "{text}");
        // A zero window still probes once.
        let e = until(Duration::ZERO, Duration::from_secs(60), || Err::<(), _>(7)).unwrap_err();
        assert_eq!((e.polls, e.last), (1, 7));
    }

    #[test]
    fn stays_watches_the_whole_window() {
        let n = Cell::new(0u32);
        let start = Instant::now();
        let polls = stays(Duration::from_millis(60), Duration::from_millis(5), || {
            n.set(n.get() + 1);
            Ok::<(), ()>(())
        })
        .unwrap();
        assert!(start.elapsed() >= Duration::from_millis(60));
        assert!(polls >= 2 && polls == n.get(), "{polls}");
    }

    #[test]
    fn stays_stops_at_the_first_break() {
        let n = Cell::new(0u32);
        let start = Instant::now();
        let e = stays(Duration::from_secs(60), Duration::from_millis(1), || {
            n.set(n.get() + 1);
            if n.get() == 3 {
                Err("tick fired")
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(30));
        assert_eq!((e.polls, e.seen), (3, "tick fired"));
        let b = Broke {
            after: Duration::from_millis(5),
            polls: 2,
            seen: "x",
        };
        assert_eq!(b.to_string(), "condition broke after 5ms (poll 2): \"x\"");
    }
}
