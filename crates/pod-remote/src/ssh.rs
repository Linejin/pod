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
//!
//! This file keeps the connection lifecycle ([`connect`], [`SshConnection`])
//! and the errors ([`ErrorKind`], [`SshError`]). `config` builds the config
//! text and argv, `diagnostics` keeps ssh's stderr and classifies failures,
//! `process` finds, runs, times and ends the ssh process. Their public items
//! are re-exported here under the same names.

mod config;
mod diagnostics;
mod process;

use std::process::Child;
use std::time::Duration;

use podd::client::Client;
use podd::protocol::HelloResult;
use podd::transport::Transport;

use crate::machine::Machine;

pub use config::{ENFORCED_OPTIONS, HOST_ALIAS, REMOTE_COMMAND, config_text, ssh_args};
pub use diagnostics::classify;
pub use process::{DEFAULT_TIMEOUT, ENV_SSH, ENV_SSH_TIMEOUT, handshake_timeout, locate_ssh};

use config::TempConfig;
use diagnostics::{Diag, diag_lines, is_closed, spawn_diag};
use process::{finish, hello_within};

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

    /// The connection as a [`crate::session::Link`] whose closer kills ssh
    /// (a reader blocked on its stdout then sees the end at once).
    pub fn into_link(self) -> crate::session::Link {
        let SshConnection {
            client,
            hello,
            child,
            ..
        } = self;
        let child = std::sync::Arc::new(std::sync::Mutex::new(child));
        let closer: crate::session::Closer = std::sync::Arc::new(move || {
            if let Ok(mut c) = child.lock() {
                let _ = c.kill();
            }
        });
        crate::session::Link::new(client, hello, closer)
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
    let mut child = process::command(&ssh, ssh_args(cfg.path()))
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
    let result = match hello_within(client, timeout) {
        Some((client, Ok(hello))) => Ok(SshConnection {
            client,
            hello,
            child,
            diag,
        }),
        Some((client, Err(e))) => {
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
        None => {
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

    /// `--json` `kind` values (`pod machine test`).
    #[test]
    fn error_codes_are_pinned() {
        use ErrorKind::*;
        let codes: Vec<&str> = [
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
        ]
        .into_iter()
        .map(ErrorKind::code)
        .collect();
        assert_eq!(
            codes,
            [
                "invalid_profile",
                "ssh_missing",
                "host_key_unknown",
                "host_key_changed",
                "auth_failed",
                "timeout",
                "unreachable",
                "remote_pod_missing",
                "version_mismatch",
                "stdout_polluted",
                "failed",
            ]
        );
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
