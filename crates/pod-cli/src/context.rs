//! Who `pod` talks to, and when it may start a daemon (docs/20-refactor/
//! daemon.md §2). The policy, by command:
//!
//! - help, guide, `server schema`, machine and hooks: no connection.
//! - local `server status|stop`: only a daemon that already runs
//!   ([`CliContext::connect_running`]).
//! - attach and remote-stdio: their own connection to [`CliContext::pipe`].
//! - every other command: [`CliContext::connect`], the local daemon
//!   (started when it does not run) or, with `--machine`, the remote one
//!   over SSH.

use anyhow::{Context, bail};
use podd::client::{Client, ClientError};
use podd::endpoint;

use crate::args::{Cmd, ServerCmd};
use crate::machine;

/// The global flags of one `pod` run.
pub(crate) struct CliContext {
    /// The local pipe, or `ssh:<name>` with `--machine`.
    pub(crate) pipe: String,
    /// `--machine`: the SSH machine profile.
    pub(crate) remote: Option<String>,
    /// `--json`.
    pub(crate) json: bool,
}

impl CliContext {
    /// The context for `cmd`; refuses what `cmd` cannot do with `--machine`.
    pub(crate) fn new(
        json: bool,
        pipe: Option<&str>,
        machine: Option<String>,
        cmd: &Cmd,
    ) -> anyhow::Result<Self> {
        let remote = machine;
        if let Some(name) = &remote
            && let Some(why) = remote_unsupported(cmd)
        {
            bail!("--machine {name}: {why}");
        }
        // Over SSH the label is `ssh:<name>`: never equal to a local pipe, so
        // the caller's own terminal is not sent as parent and waits do not
        // reconnect to a local pipe.
        let pipe = match &remote {
            Some(name) => format!("{}{name}", pod_remote::LABEL_PREFIX),
            None => pipe
                .map(endpoint::pipe_path)
                .unwrap_or_else(endpoint::default_pipe),
        };
        Ok(Self { pipe, remote, json })
    }

    pub(crate) fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// Ordinary commands: the remote daemon over SSH with `--machine`,
    /// else the local one, started when it does not run.
    pub(crate) fn connect(&self) -> anyhow::Result<Client> {
        match &self.remote {
            Some(name) => machine::connect(name),
            None => Client::connect_or_start(&self.pipe, None)
                .with_context(|| format!("cannot reach podd on {}", self.pipe)),
        }
    }

    /// Local `server status|stop`: never starts podd.
    pub(crate) fn connect_running(&self) -> Result<Client, ClientError> {
        Client::connect(&self.pipe)
    }

    /// [`caller_terminal`] for this pipe.
    pub(crate) fn caller_terminal(&self) -> Option<String> {
        caller_terminal(&self.pipe)
    }
}

/// Why `cmd` cannot run with `--machine` (None: it can).
fn remote_unsupported(cmd: &Cmd) -> Option<&'static str> {
    match cmd {
        Cmd::Attach { .. } => Some("attach는 아직 원격을 지원하지 않습니다 (docs/17 R4)"),
        Cmd::RemoteStdio => Some("remote-stdio는 원격 쪽에서 ssh가 실행하는 명령입니다"),
        Cmd::Hooks { .. } => Some("hooks는 이 PC의 에이전트 설정만 다룹니다"),
        Cmd::Machine { .. } => Some("machine은 이 PC의 프로필만 다룹니다"),
        Cmd::Server {
            cmd: ServerCmd::Stop { .. },
        } => Some("원격 데몬 중지는 지원하지 않습니다"),
        Cmd::Run {
            inherit_env: true, ..
        } => Some("--inherit-env는 이 PC의 환경 변수를 원격에 보내므로 쓸 수 없습니다"),
        _ => None,
    }
}

/// The caller's own terminal id when this `pod` runs inside a terminal of
/// the daemon it talks to.
pub(crate) fn caller_terminal(pipe: &str) -> Option<String> {
    let sock = std::env::var(endpoint::ENV_SOCKET).ok()?;
    let id = std::env::var(endpoint::ENV_TERMINAL_ID).ok()?;
    (endpoint::pipe_path(&sock).eq_ignore_ascii_case(pipe) && !id.is_empty()).then_some(id)
}
