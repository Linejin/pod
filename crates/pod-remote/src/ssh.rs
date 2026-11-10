//! The system-OpenSSH transport (docs/17 §2.1, §2.3).
//!
//! - The `ssh` executable is located by **absolute path** (`POD_SSH`, else
//!   `%SystemRoot%\System32\OpenSSH\ssh.exe`, `%ProgramFiles%\OpenSSH\ssh.exe`,
//!   else `ssh.exe` in an absolute `PATH` directory) and run with an argv
//!   array, never through a shell.
//! - Each connection gets a Pod-generated minimal config (`-F`): one `Host
//!   pod-remote` block with HostName/User/Port/IdentityFile/UserKnownHostsFile
//!   from the profile. `-F` also means ssh reads neither the user's
//!   `~/.ssh/config` nor the system-wide config, so no ProxyCommand,
//!   LocalCommand or `Match exec` comes in. The file is deleted after the
//!   handshake.
//! - [`ENFORCED_OPTIONS`] go on the command line (`-o`, which wins over any
//!   config) together with `-T`. The destination is the fixed alias, so the
//!   profile's host never appears in argv; the remote command is the fixed
//!   `pod remote-stdio`.
//! - stdout is protocol only: the client refuses anything that is not a JSON
//!   object (a login banner, profile output). stderr is read into a bounded
//!   buffer and used to explain failures ([`ErrorKind`], Korean messages).
//! - The handshake is `server.status` + v12 `connection.hello`
//!   ([`Client::hello`]); a v11 remote gets version guidance only, nothing
//!   else is sent.

use std::ffi::OsString;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use podd::client::{Client, ClientError};
use podd::protocol::HelloResult;
use podd::transport::Transport;

use crate::machine::Machine;

/// Absolute path of the ssh executable to use instead of the system one.
pub const ENV_SSH: &str = "POD_SSH";
/// Handshake time limit in seconds (default [`DEFAULT_TIMEOUT`]).
pub const ENV_SSH_TIMEOUT: &str = "POD_SSH_TIMEOUT";
/// TCP connect + authentication + remote `pod remote-stdio` (which may start
/// the remote podd, up to 10 s) + hello.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// The only `Host` in the generated config; the destination in argv.
pub const HOST_ALIAS: &str = "pod-remote";
/// The fixed remote command. Nothing user-supplied is ever appended.
pub const REMOTE_COMMAND: [&str; 2] = ["pod", "remote-stdio"];

/// Passed as `-o Key=Value` on every connection (docs/17 §2.1).
pub const ENFORCED_OPTIONS: &[(&str, &str)] = &[
    ("BatchMode", "yes"),
    ("StrictHostKeyChecking", "yes"),
    ("PasswordAuthentication", "no"),
    ("KbdInteractiveAuthentication", "no"),
    ("ForwardAgent", "no"),
    ("ForwardX11", "no"),
    ("ClearAllForwardings", "yes"),
    ("RequestTTY", "no"),
    ("PermitLocalCommand", "no"),
    // Never rewrite known_hosts behind the user's back.
    ("UpdateHostKeys", "no"),
    ("ConnectTimeout", "10"),
    ("ServerAliveInterval", "15"),
    ("ServerAliveCountMax", "3"),
];

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// stderr kept for diagnostics: lines, characters per line.
const DIAG_LINES: usize = 20;
const DIAG_CHARS: usize = 300;
/// stderr lines shown in an error message.
const SHOWN_LINES: usize = 8;

/// What went wrong, for messages and `--json` (`code()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    InvalidProfile,
    SshMissing,
    HostKeyUnknown,
    HostKeyChanged,
    AuthFailed,
    Timeout,
    Unreachable,
    RemotePodMissing,
    VersionMismatch,
    StdoutPolluted,
    Failed,
}

impl ErrorKind {
    pub fn code(self) -> &'static str {
        match self {
            ErrorKind::InvalidProfile => "invalid_profile",
            ErrorKind::SshMissing => "ssh_missing",
            ErrorKind::HostKeyUnknown => "host_key_unknown",
            ErrorKind::HostKeyChanged => "host_key_changed",
            ErrorKind::AuthFailed => "auth_failed",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Unreachable => "unreachable",
            ErrorKind::RemotePodMissing => "remote_pod_missing",
            ErrorKind::VersionMismatch => "version_mismatch",
            ErrorKind::StdoutPolluted => "stdout_polluted",
            ErrorKind::Failed => "failed",
        }
    }

    /// One Korean sentence: what happened.
    pub fn summary(self) -> &'static str {
        match self {
            ErrorKind::InvalidProfile => "머신 프로필이 올바르지 않습니다",
            ErrorKind::SshMissing => "ssh 실행 파일을 찾을 수 없습니다",
            ErrorKind::HostKeyUnknown => "호스트 키를 알 수 없어 연결을 막았습니다",
            ErrorKind::HostKeyChanged => "호스트 키가 바뀌어 연결을 막았습니다",
            ErrorKind::AuthFailed => "SSH 인증에 실패했습니다",
            ErrorKind::Timeout => "시간 안에 응답이 없습니다",
            ErrorKind::Unreachable => "호스트에 연결할 수 없습니다",
            ErrorKind::RemotePodMissing => "원격에 pod가 없습니다",
            ErrorKind::VersionMismatch => "원격 Pod 버전이 맞지 않습니다",
            ErrorKind::StdoutPolluted => "원격 출력에 프로토콜이 아닌 내용이 섞였습니다",
            ErrorKind::Failed => "SSH 연결에 실패했습니다",
        }
    }

    /// What the user can do (Pod never changes SSH setup itself).
    pub fn hint(self) -> &'static str {
        match self {
            ErrorKind::InvalidProfile => "`pod machine remove` 후 다시 추가하세요.",
            ErrorKind::SshMissing => {
                "Windows 설정 > 시스템 > 선택적 기능에서 'OpenSSH 클라이언트'를 설치하세요. \
                 Pod는 설치하지 않습니다."
            }
            ErrorKind::HostKeyUnknown => {
                "서버 관리자에게 받은 fingerprint를 확인한 뒤 known_hosts에 직접 등록하세요. \
                 Pod는 키를 자동으로 받아들이지 않습니다."
            }
            ErrorKind::HostKeyChanged => {
                "서버가 바뀌었거나 중간자 공격일 수 있습니다. 관리자에게 새 fingerprint를 \
                 확인하기 전에는 known_hosts를 고치지 마세요."
            }
            ErrorKind::AuthFailed => {
                "키를 ssh-agent에 올리거나(암호가 걸린 키는 직접 해제) 프로필의 키 경로와 \
                 사용자를 확인하세요. 비밀번호 로그인은 쓰지 않습니다."
            }
            ErrorKind::Timeout => "네트워크와 원격 Pod 상태를 확인한 뒤 다시 시도하세요.",
            ErrorKind::Unreachable => "호스트 이름·포트·방화벽·SSH 서버 실행 여부를 확인하세요.",
            ErrorKind::RemotePodMissing => {
                "원격 사용자의 PATH에 Pod를 설치하세요(`pod remote-stdio`가 실행돼야 합니다)."
            }
            ErrorKind::VersionMismatch => {
                "원격에 같은 버전의 Pod를 설치하세요. 실행 중인 원격 데몬·터미널은 건드리지 않았습니다."
            }
            ErrorKind::StdoutPolluted => {
                "로그인 배너나 셸 profile이 비대화형 SSH 세션에서 stdout에 쓰지 않게 하세요."
            }
            ErrorKind::Failed => "아래 ssh 진단을 확인하세요.",
        }
    }
}

/// A failed connection: the kind, the technical cause and bounded ssh stderr.
#[derive(Debug, Clone)]
pub struct SshError {
    pub kind: ErrorKind,
    pub machine: String,
    pub detail: String,
    pub stderr: Vec<String>,
    pub exit_code: Option<i32>,
}

impl SshError {
    fn new(kind: ErrorKind, machine: &str, detail: impl Into<String>) -> Self {
        Self {
            kind,
            machine: machine.into(),
            detail: detail.into(),
            stderr: Vec::new(),
            exit_code: None,
        }
    }
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "머신 {}: {}. {}",
            self.machine,
            self.kind.summary(),
            self.kind.hint()
        )?;
        if !self.detail.is_empty() {
            write!(f, "\n  원인: {}", self.detail)?;
        }
        for l in self.stderr.iter().take(SHOWN_LINES) {
            write!(f, "\n  ssh: {l}")?;
        }
        if self.stderr.len() > SHOWN_LINES {
            write!(f, "\n  ssh: (… {}줄 더)", self.stderr.len() - SHOWN_LINES)?;
        }
        Ok(())
    }
}

impl std::error::Error for SshError {}

/// Classify a failed handshake from the client error, ssh's stderr and exit code.
pub fn classify(err: Option<&ClientError>, stderr: &str, exit: Option<i32>) -> ErrorKind {
    if err.is_some_and(ClientError::is_incompatible) {
        return ErrorKind::VersionMismatch;
    }
    let has = |needle: &str| stderr.contains(needle);
    let lower = stderr.to_ascii_lowercase();
    if has("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || (lower.contains("host key for") && lower.contains("has changed"))
    {
        return ErrorKind::HostKeyChanged;
    }
    if has("Host key verification failed")
        || lower.contains("host key is known for")
        || lower.contains("no matching host key")
    {
        return ErrorKind::HostKeyUnknown;
    }
    if has("Permission denied (")
        || has("Too many authentication failures")
        || has("UNPROTECTED PRIVATE KEY FILE")
        || lower.contains("bad permissions")
        || lower.contains("no such identity")
    {
        return ErrorKind::AuthFailed;
    }
    if lower.contains("timed out") {
        return ErrorKind::Timeout;
    }
    if has("Could not resolve hostname")
        || has("Connection refused")
        || has("No route to host")
        || has("Network is unreachable")
        || has("kex_exchange_identification")
        || has("Connection reset")
    {
        return ErrorKind::Unreachable;
    }
    if lower.contains("pod: command not found")
        || lower.contains("pod: not found")
        || lower.contains("'pod' is not recognized")
        || lower.contains("term 'pod' is not recognized")
        || exit == Some(127)
    {
        return ErrorKind::RemotePodMissing;
    }
    // An older pod without `remote-stdio` (clap usage error).
    if lower.contains("unrecognized subcommand") && lower.contains("remote-stdio") {
        return ErrorKind::VersionMismatch;
    }
    match err {
        Some(ClientError::Protocol(m)) if !is_closed_message(m) => ErrorKind::StdoutPolluted,
        _ => ErrorKind::Failed,
    }
}

fn is_closed_message(m: &str) -> bool {
    m == "daemon closed the connection"
}

fn is_closed(e: &ClientError) -> bool {
    match e {
        ClientError::Io(_) | ClientError::NotRunning => true,
        ClientError::Protocol(m) => is_closed_message(m),
        ClientError::Rpc(_) => false,
    }
}

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

/// A path as written into the ssh config: forward slashes (every Windows
/// OpenSSH version reads them; backslash escapes differ between versions),
/// `%` doubled (ssh expands `%` tokens), quoted by the caller.
fn config_path(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").replace('%', "%%")
}

/// The generated ssh config for `m` (validated beforehand).
pub fn config_text(m: &Machine) -> String {
    let mut s =
        String::from("# Generated by Pod for one connection (docs/17 2.1); deleted after use.\n");
    s.push_str(&format!("Host {HOST_ALIAS}\n"));
    s.push_str(&format!("  HostName {}\n", m.host.replace('%', "%%")));
    if let Some(u) = &m.user {
        s.push_str(&format!("  User {u}\n"));
    }
    if let Some(p) = m.port {
        s.push_str(&format!("  Port {p}\n"));
    }
    if let Some(k) = &m.identity_file {
        s.push_str(&format!("  IdentityFile \"{}\"\n", config_path(k)));
        s.push_str("  IdentitiesOnly yes\n");
    }
    if let Some(k) = &m.known_hosts {
        s.push_str(&format!("  UserKnownHostsFile \"{}\"\n", config_path(k)));
    }
    s
}

/// argv after the executable.
pub fn ssh_args(config: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = vec!["-F".into(), config.into(), "-T".into()];
    for (k, v) in ENFORCED_OPTIONS {
        a.push("-o".into());
        a.push(format!("{k}={v}").into());
    }
    a.push("--".into());
    a.push(HOST_ALIAS.into());
    a.extend(REMOTE_COMMAND.iter().map(OsString::from));
    a
}

/// The generated config file; removed on drop.
struct TempConfig(PathBuf);

impl TempConfig {
    fn write(m: &Machine) -> std::io::Result<Self> {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("pod")
            .join("ssh");
        std::fs::create_dir_all(&dir)?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let path = dir.join(format!(
            "{}-{}-{}-{nanos}.conf",
            m.name,
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        f.write_all(config_text(m).as_bytes())?;
        Ok(Self(path))
    }
}

impl Drop for TempConfig {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Bounded stderr collector: the first [`DIAG_LINES`] lines, each cut to
/// [`DIAG_CHARS`] characters with control characters blanked. The rest is
/// read and dropped so ssh never blocks on a full pipe.
#[derive(Default)]
struct DiagBuf {
    lines: Vec<String>,
    done: bool,
}

type Diag = Arc<(Mutex<DiagBuf>, Condvar)>;

fn push_line(buf: &mut DiagBuf, raw: &[u8]) {
    if buf.lines.len() >= DIAG_LINES {
        return;
    }
    let line: String = String::from_utf8_lossy(raw)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(DIAG_CHARS)
        .collect();
    let line = line.trim();
    if !line.is_empty() {
        buf.lines.push(line.to_string());
    }
}

fn spawn_diag(mut r: impl Read + Send + 'static) -> Diag {
    let diag: Diag = Arc::default();
    let d = diag.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        let mut cur: Vec<u8> = Vec::new();
        loop {
            let n = match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &chunk[..n] {
                if b == b'\n' {
                    push_line(&mut d.0.lock().unwrap(), &cur);
                    cur.clear();
                } else if cur.len() < DIAG_CHARS * 4 {
                    cur.push(b);
                }
            }
        }
        let mut g = d.0.lock().unwrap();
        push_line(&mut g, &cur);
        g.done = true;
        d.1.notify_all();
    });
    diag
}

/// The lines so far, after waiting up to `wait` for stderr to close.
fn diag_lines(d: &Diag, wait: Duration) -> Vec<String> {
    let g = d.0.lock().unwrap();
    let (g, _) = d.1.wait_timeout_while(g, wait, |b| !b.done).unwrap();
    g.lines.clone()
}

/// Wait up to `grace` for the child to exit by itself, then kill it.
fn finish(child: &mut Child, grace: Duration) -> Option<i32> {
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

/// `POD_SSH_TIMEOUT` seconds, else [`DEFAULT_TIMEOUT`].
pub fn handshake_timeout() -> Duration {
    std::env::var(ENV_SSH_TIMEOUT)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(DEFAULT_TIMEOUT)
}

/// A connected remote podd: the client (after the v12 handshake) and the
/// ssh child. Dropping the client closes ssh's stdin; ssh and the remote
/// bridge then end on their own (the remote daemon keeps running).
pub struct SshConnection {
    pub client: Client,
    pub hello: HelloResult,
    child: Child,
    diag: Diag,
}

impl SshConnection {
    /// Keep only the client (the ssh process ends when it is dropped).
    pub fn into_client(self) -> Client {
        self.client
    }

    /// Close the connection and wait (bounded) for ssh to exit; returns
    /// ssh's exit code and its stderr diagnostics.
    pub fn close(self) -> (Option<i32>, Vec<String>) {
        let SshConnection {
            client,
            mut child,
            diag,
            ..
        } = self;
        drop(client);
        let code = finish(&mut child, Duration::from_secs(5));
        (code, diag_lines(&diag, Duration::from_secs(1)))
    }
}

/// Connect to `m` with the system ssh and run the v12 handshake.
pub fn connect(m: &Machine) -> Result<SshConnection, SshError> {
    m.validate()
        .map_err(|e| SshError::new(ErrorKind::InvalidProfile, &m.name, e))?;
    let ssh = locate_ssh().map_err(|e| SshError::new(ErrorKind::SshMissing, &m.name, e))?;
    let cfg = TempConfig::write(m).map_err(|e| {
        SshError::new(
            ErrorKind::Failed,
            &m.name,
            format!("ssh 설정 파일을 쓸 수 없습니다: {e}"),
        )
    })?;
    let mut child = Command::new(&ssh)
        .args(ssh_args(&cfg.0))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // BatchMode already forbids prompts; also never call a GUI askpass.
        .env_remove("SSH_ASKPASS")
        .env_remove("SSH_ASKPASS_REQUIRE")
        .env_remove("DISPLAY")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| {
            let kind = if e.kind() == std::io::ErrorKind::NotFound {
                ErrorKind::SshMissing
            } else {
                ErrorKind::Failed
            };
            SshError::new(kind, &m.name, format!("{} 실행 실패: {e}", ssh.display()))
        })?;
    let diag = spawn_diag(child.stderr.take().expect("piped stderr"));
    let transport = Transport::from_child(&mut child).expect("piped stdio");
    let client = Client::from_transport(format!("{}{}", crate::LABEL_PREFIX, m.name), transport);

    let timeout = handshake_timeout();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut c = client;
        let r = c.hello(&[]);
        let _ = tx.send((c, r));
    });
    let result = match rx.recv_timeout(timeout) {
        Ok((client, Ok(hello))) => Ok(SshConnection {
            client,
            hello,
            child,
            diag,
        }),
        Ok((client, Err(e))) => {
            drop(client);
            // ssh is exiting (stdout closed): let it finish to get its
            // stderr and exit code. Otherwise the remote is alive but wrong.
            let grace = if is_closed(&e) {
                Duration::from_secs(3)
            } else {
                Duration::from_millis(500)
            };
            let exit_code = finish(&mut child, grace);
            let stderr = diag_lines(&diag, Duration::from_secs(1));
            let detail = match (is_closed(&e), exit_code) {
                (true, Some(c)) => format!("ssh가 연결을 닫았습니다 (종료 코드 {c})"),
                (true, None) => "ssh가 연결을 닫았습니다".into(),
                (false, _) => e.to_string(),
            };
            Err(SshError {
                kind: classify(Some(&e), &stderr.join("\n"), exit_code),
                machine: m.name.clone(),
                detail,
                stderr,
                exit_code,
            })
        }
        Err(_) => {
            let exit_code = finish(&mut child, Duration::ZERO);
            Err(SshError {
                kind: ErrorKind::Timeout,
                machine: m.name.clone(),
                detail: format!(
                    "{:.1}초 안에 핸드셰이크가 끝나지 않아 ssh를 종료했습니다",
                    timeout.as_secs_f64()
                ),
                stderr: diag_lines(&diag, Duration::from_secs(1)),
                exit_code,
            })
        }
    };
    // ssh read its config at start; the handshake is past that point.
    drop(cfg);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn machine() -> Machine {
        Machine {
            name: "dev".into(),
            host: "fe80::1%eth0".into(),
            user: Some("alice".into()),
            port: Some(2222),
            identity_file: Some(r"C:\Users\A B\.ssh\id_ed25519".into()),
            known_hosts: Some(r"\\server\share\known%hosts".into()),
        }
    }

    #[test]
    fn config_has_only_profile_fields() {
        let t = config_text(&machine());
        let directives: Vec<&str> = t
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .map(|l| l.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(
            directives,
            [
                "Host",
                "HostName",
                "User",
                "Port",
                "IdentityFile",
                "IdentitiesOnly",
                "UserKnownHostsFile"
            ]
        );
        assert!(t.contains("Host pod-remote\n"));
        assert!(t.contains("HostName fe80::1%%eth0\n"));
        assert!(t.contains("IdentityFile \"C:/Users/A B/.ssh/id_ed25519\"\n"));
        assert!(t.contains("UserKnownHostsFile \"//server/share/known%%hosts\"\n"));
        let mut bare = machine();
        bare.user = None;
        bare.port = None;
        bare.identity_file = None;
        bare.known_hosts = None;
        assert_eq!(
            config_text(&bare)
                .lines()
                .filter(|l| !l.starts_with('#'))
                .count(),
            2
        );
    }

    #[test]
    fn argv_is_fixed_and_ends_with_the_remote_command() {
        let a: Vec<String> = ssh_args(Path::new(r"C:\x y\c.conf"))
            .into_iter()
            .map(|s| s.into_string().unwrap())
            .collect();
        assert_eq!(&a[..3], ["-F", r"C:\x y\c.conf", "-T"]);
        assert_eq!(
            &a[a.len() - 4..],
            ["--", "pod-remote", "pod", "remote-stdio"]
        );
        for (k, v) in ENFORCED_OPTIONS {
            let kv = format!("{k}={v}");
            let i = a
                .iter()
                .position(|x| *x == kv)
                .unwrap_or_else(|| panic!("{kv}"));
            assert_eq!(a[i - 1], "-o");
        }
        // The profile's host never reaches argv.
        assert!(!a.iter().any(|x| x.contains("fe80")));
    }

    #[test]
    fn classification() {
        use ErrorKind::*;
        let closed = ClientError::Protocol("daemon closed the connection".into());
        let c = |s: &str, exit| classify(Some(&closed), s, exit);
        assert_eq!(
            c(
                "No ED25519 host key is known for h and you have requested strict checking.\r\nHost key verification failed.",
                Some(255)
            ),
            HostKeyUnknown
        );
        assert_eq!(
            c(
                "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.",
                Some(255)
            ),
            HostKeyChanged
        );
        assert_eq!(
            c("alice@h: Permission denied (publickey).", Some(255)),
            AuthFailed
        );
        assert_eq!(c("Load key \"k\": bad permissions", Some(255)), AuthFailed);
        assert_eq!(
            c(
                "ssh: connect to host h port 22: Connection timed out",
                Some(255)
            ),
            Timeout
        );
        assert_eq!(
            c(
                "ssh: connect to host h port 22: Connection refused",
                Some(255)
            ),
            Unreachable
        );
        assert_eq!(
            c(
                "ssh: Could not resolve hostname h: No such host is known.",
                Some(255)
            ),
            Unreachable
        );
        assert_eq!(
            c("bash: line 1: pod: command not found", Some(127)),
            RemotePodMissing
        );
        assert_eq!(
            c(
                "'pod' is not recognized as an internal or external command,\r\noperable program or batch file.",
                Some(1)
            ),
            RemotePodMissing
        );
        assert_eq!(
            c(
                "pod : The term 'pod' is not recognized as the name of a cmdlet",
                Some(1)
            ),
            RemotePodMissing
        );
        assert_eq!(c("", Some(127)), RemotePodMissing);
        assert_eq!(
            c("error: unrecognized subcommand 'remote-stdio'", Some(2)),
            VersionMismatch
        );
        assert_eq!(c("", Some(1)), Failed);
        let banner = ClientError::Protocol(
            "non-protocol output on the daemon connection: \"Welcome\"".into(),
        );
        assert_eq!(classify(Some(&banner), "", None), StdoutPolluted);
        let v11 = ClientError::Rpc(podd::protocol::RpcError::new(
            podd::protocol::remote_codes::PROTOCOL_MISMATCH,
            "v11",
        ));
        assert_eq!(classify(Some(&v11), "", Some(0)), VersionMismatch);
        assert_eq!(classify(None, "", None), Failed);
    }

    #[test]
    fn stderr_is_bounded_and_sanitized() {
        let mut data = Vec::new();
        data.extend_from_slice(b"first\x1b[31m red\r\n");
        data.extend_from_slice(&vec![b'x'; 100_000]);
        data.push(b'\n');
        for i in 0..100 {
            data.extend_from_slice(format!("line {i}\n").as_bytes());
        }
        let d = spawn_diag(std::io::Cursor::new(data));
        let lines = diag_lines(&d, Duration::from_secs(5));
        assert_eq!(lines.len(), DIAG_LINES);
        assert_eq!(lines[0], "first [31m red");
        assert_eq!(lines[1].chars().count(), DIAG_CHARS);
        assert_eq!(lines[2], "line 0");
    }

    #[test]
    fn display_is_korean_and_bounded() {
        let e = SshError {
            kind: ErrorKind::AuthFailed,
            machine: "dev".into(),
            detail: "daemon closed the connection".into(),
            stderr: (0..12).map(|i| format!("l{i}")).collect(),
            exit_code: Some(255),
        };
        let s = e.to_string();
        assert!(s.starts_with("머신 dev: SSH 인증에 실패했습니다."), "{s}");
        assert!(s.contains("ssh: l7") && !s.contains("ssh: l8") && s.contains("4줄 더"));
    }
}
