//! Prebuilt fixture binaries and bounded waits for the podd integration
//! tests (refactor card D-F0, docs/20-refactor/daemon.md).
//!
//! Tests never run cargo. The binaries a test drives besides `podd` and
//! `pod-holder` (which cargo builds for every podd integration test) are
//! built beforehand, once, outside the test run:
//!
//! - `cargo test --workspace` builds them anyway (CI).
//! - `tools/test-daemon-refactor.ps1` builds them, then runs the chosen
//!   test target and filter in an isolated temp profile.
//! - by hand: [`BUILD_FIXTURES`], then `cargo test -p podd --test <name>`.
//!
//! A missing binary fails the test with that command. A stale one is not
//! detected: rebuild after changing pod-cli, pod-hook or the fake agent.
//!
//! The API is small on purpose: [`fixture_bin`] (+ three named shortcuts),
//! [`until`] / [`until_every`] / [`until_true`]. Later cards add helpers
//! next to these rather than new copies in test files.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Builds every fixture binary in the profile of a plain `cargo test`
/// (add `--release` for release test runs).
pub const BUILD_FIXTURES: &str =
    "cargo build -p podd --example fake-agent -p pod-hook -p pod-cli --bin pod --bin pod-hook";

/// `target/<profile>`: where cargo put the `podd` this test was built with.
pub fn profile_dir() -> PathBuf {
    Path::new(env!("CARGO_BIN_EXE_podd"))
        .parent()
        .expect("podd.exe has a directory")
        .to_path_buf()
}

/// A prebuilt binary under [`profile_dir`], e.g. `pod.exe` or
/// `examples/fake-agent.exe`. Panics with the build command if it is missing.
pub fn fixture_bin(rel: &str) -> PathBuf {
    let p = profile_dir().join(rel);
    assert!(
        p.exists(),
        "{} missing: build the test fixtures first ({BUILD_FIXTURES}{}, \
         or tools/test-daemon-refactor.ps1); tests do not run cargo",
        p.display(),
        if is_release() { " --release" } else { "" },
    );
    p
}

fn is_release() -> bool {
    profile_dir().file_name().and_then(|n| n.to_str()) == Some("release")
}

/// The `pod` CLI.
pub fn pod_exe() -> PathBuf {
    fixture_bin("pod.exe")
}

/// The `pod-hook` relay the status hooks run.
pub fn pod_hook_exe() -> PathBuf {
    fixture_bin("pod-hook.exe")
}

/// `examples/fake-agent.rs`: a scripted stand-in for an agent CLI.
pub fn fake_agent_exe() -> PathBuf {
    fixture_bin("examples/fake-agent.exe")
}

/// How often [`until`] polls.
pub const POLL: Duration = Duration::from_millis(100);

/// Polls `f` every [`POLL`] until it returns `Some`; fails the test after
/// `secs` seconds. Each round: call `f`, check the deadline, sleep.
pub fn until<T>(what: &str, secs: u64, f: impl FnMut() -> Option<T>) -> T {
    until_every(what, secs, POLL, f)
}

/// [`until`] with an explicit poll interval.
pub fn until_every<T>(
    what: &str,
    secs: u64,
    every: Duration,
    mut f: impl FnMut() -> Option<T>,
) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(every);
    }
}

/// [`until`] for a plain condition.
pub fn until_true(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    until(what, secs, || f().then_some(()))
}
