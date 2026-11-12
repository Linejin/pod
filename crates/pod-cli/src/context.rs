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
        if let Some(why) = worker_refusal(podd::env::in_worker_env(), remote.is_some(), &pipe, cmd)
        {
            bail!("{why}");
        }
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

/// docs/19 §20: in a Pod worker's environment (`POD_ATTEMPT_ID` or
/// `POD_ATTEMPT_TOKEN`) `pod` talks to a worker endpoint only, so `--pipe`
/// to the full pipe or `--machine` cannot widen what the worker reaches
/// through commands its sandbox lets run outside (the Codex allowance).
/// Commands that never connect are not refused. Same-user code that also
/// drops the worker variables is outside this check (docs/22 TM-20).
fn worker_refusal(worker: bool, remote: bool, pipe: &str, cmd: &Cmd) -> Option<String> {
    if !worker
        || matches!(
            cmd,
            Cmd::Guide { .. } | Cmd::Machine { .. } | Cmd::Hooks { .. }
        )
    {
        return None;
    }
    if remote {
        return Some(
            "--machine is refused in a Pod worker's environment (POD_ATTEMPT_ID): a worker reaches its own run through its worker endpoint only".into(),
        );
    }
    (!endpoint::is_worker_pipe(pipe)).then(|| {
        format!(
            "{pipe} is not a worker endpoint: in a Pod worker's environment (POD_ATTEMPT_ID) pod talks to the worker endpoint in POD_SOCKET only; the full pipe is the OS user's"
        )
    })
}

/// The caller's own terminal id when this `pod` runs inside a terminal of
/// the daemon it talks to.
pub(crate) fn caller_terminal(pipe: &str) -> Option<String> {
    let sock = std::env::var(endpoint::ENV_SOCKET).ok()?;
    let id = std::env::var(endpoint::ENV_TERMINAL_ID).ok()?;
    (endpoint::pipe_path(&sock).eq_ignore_ascii_case(pipe) && !id.is_empty()).then_some(id)
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::args::Cli;

    fn cmd(args: &[&str]) -> Cmd {
        let mut all = vec!["pod"];
        all.extend_from_slice(args);
        Cli::try_parse_from(all).unwrap().cmd
    }

    /// docs/19 §20: a worker's `pod` reaches its worker endpoint only.
    #[test]
    fn a_worker_env_talks_to_its_worker_endpoint_only() {
        let full = r"\\.\pipe\podd-me";
        let worker = r"\\.\pipe\podd-me-worker";
        let orch = cmd(&["orch", "check"]);
        assert_eq!(worker_refusal(true, false, worker, &orch), None);
        assert!(
            worker_refusal(true, false, full, &orch)
                .is_some_and(|w| w.contains("not a worker endpoint"))
        );
        assert!(
            worker_refusal(true, true, "ssh:box", &orch).is_some_and(|w| w.contains("--machine"))
        );
        assert!(worker_refusal(true, false, full, &cmd(&["ls"])).is_some());
        // Not a worker, or a command that never connects: no refusal.
        assert_eq!(worker_refusal(false, false, full, &orch), None);
        assert_eq!(worker_refusal(true, false, full, &cmd(&["guide"])), None);
    }
}
