//! The ssh process: the executable by absolute path, an argv-only command
//! without a console window or askpass, the handshake deadline and the
//! bounded wait-then-kill at the end.

use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use podd::client::{Client, ClientError};
use podd::protocol::HelloResult;

/// Absolute path of the ssh executable to use instead of the system one.
pub const ENV_SSH: &str = "POD_SSH";
/// Handshake time limit in seconds (default [`DEFAULT_TIMEOUT`]).
pub const ENV_SSH_TIMEOUT: &str = "POD_SSH_TIMEOUT";
/// TCP connect + authentication + remote `pod remote-stdio` (which may start
/// the remote podd, up to 10 s) + hello.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The ssh executable: `POD_SSH` (must be absolute), else the Windows
/// OpenSSH client, else `ssh.exe` in an absolute `PATH` directory.
pub fn locate_ssh() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os(ENV_SSH).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        if !p.is_absolute() {
            return Err(format!("{ENV_SSH}는 절대 경로여야 합니다: {}", p.display()));
        }
        if !p.is_file() {
            return Err(format!("{ENV_SSH}의 파일이 없습니다: {}", p.display()));
        }
        return Ok(p);
    }
    let mut candidates = Vec::new();
    if let Some(root) = std::env::var_os("SystemRoot") {
        candidates.push(PathBuf::from(root).join(r"System32\OpenSSH\ssh.exe"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        candidates.push(PathBuf::from(pf).join(r"OpenSSH\ssh.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        candidates.extend(
            std::env::split_paths(&path)
                .filter(|d| d.is_absolute())
                .map(|d| d.join("ssh.exe")),
        );
    }
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        "System32\\OpenSSH, Program Files\\OpenSSH, PATH 어디에도 ssh.exe가 없습니다".into()
    })
}

/// `ssh <args>` with piped stdio, never through a shell.
pub(super) fn command(ssh: &Path, args: Vec<OsString>) -> Command {
    let mut c = Command::new(ssh);
    c.args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // BatchMode already forbids prompts; also never call a GUI askpass.
        .env_remove("SSH_ASKPASS")
        .env_remove("SSH_ASKPASS_REQUIRE")
        .env_remove("DISPLAY")
        .creation_flags(CREATE_NO_WINDOW);
    c
}

/// `POD_SSH_TIMEOUT` seconds, else [`DEFAULT_TIMEOUT`].
pub fn handshake_timeout() -> Duration {
    timeout_from(std::env::var(ENV_SSH_TIMEOUT).ok().as_deref())
}

/// A positive, finite number of seconds, else [`DEFAULT_TIMEOUT`].
fn timeout_from(v: Option<&str>) -> Duration {
    v.and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// Run the v12 handshake on its own thread and wait up to `timeout` for it.
/// `None`: no answer in time (the thread keeps the client until ssh ends).
pub(super) fn hello_within(
    client: Client,
    timeout: Duration,
) -> Option<(Client, Result<HelloResult, ClientError>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut c = client;
        let r = c.hello(&[]);
        let _ = tx.send((c, r));
    });
    rx.recv_timeout(timeout).ok()
}

/// Wait up to `grace` for the child to exit by itself, then kill it.
pub(super) fn finish(child: &mut Child, grace: Duration) -> Option<i32> {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(s)) => return s.code(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                return child.wait().ok().and_then(|s| s.code());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_is_argv_only_without_askpass() {
        let c = command(
            Path::new(r"C:\x y\ssh.exe"),
            vec!["-T".into(), "a b".into()],
        );
        assert_eq!(c.get_program(), r"C:\x y\ssh.exe");
        assert_eq!(c.get_args().collect::<Vec<_>>(), ["-T", "a b"]);
        let mut envs: Vec<(String, Option<String>)> = c
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        envs.sort();
        assert_eq!(
            envs,
            [
                ("DISPLAY".to_string(), None),
                ("SSH_ASKPASS".to_string(), None),
                ("SSH_ASKPASS_REQUIRE".to_string(), None),
            ]
        );
    }

    #[test]
    fn timeout_is_positive_finite_seconds_else_default() {
        assert_eq!(timeout_from(None), DEFAULT_TIMEOUT);
        assert_eq!(timeout_from(Some("1.5")), Duration::from_millis(1500));
        assert_eq!(timeout_from(Some(" 2 ")), Duration::from_secs(2));
        for bad in ["", "0", "-1", "NaN", "inf", "abc", "1s"] {
            assert_eq!(timeout_from(Some(bad)), DEFAULT_TIMEOUT, "{bad:?}");
        }
    }

    fn cmd(args: &[&str]) -> Child {
        Command::new(std::env::var_os("ComSpec").unwrap_or_else(|| "cmd.exe".into()))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap()
    }

    #[test]
    fn finish_waits_for_the_exit_code_then_kills() {
        // Exits by itself within the grace period: its own code.
        let mut done = cmd(&["/d", "/c", "exit 3"]);
        assert_eq!(finish(&mut done, Duration::from_secs(10)), Some(3));
        // Blocked on stdin (kept open): killed after the grace period,
        // TerminateProcess exit code 1.
        let mut stuck = cmd(&["/d", "/c", "set /p x="]);
        let start = Instant::now();
        assert_eq!(finish(&mut stuck, Duration::from_millis(200)), Some(1));
        assert!(start.elapsed() >= Duration::from_millis(200));
        drop(stuck.stdin.take());
    }
}
