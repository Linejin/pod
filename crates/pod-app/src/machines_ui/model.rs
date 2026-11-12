//! What the machine board shows, and how controller events change it.
//! Pure data: no GPUI, no threads (unit-tested in `tests.rs`).

use std::time::SystemTime;

use pod_remote::Machine;
use pod_remote::ssh::ErrorKind;
use podd::protocol::{TaskInfo, TerminalInfo, TerminalState};

use super::copy;
use super::trust::{Identity, Trust};

/// A remote resource: profile, the machine that owns it and its id there
/// (docs/17 §1). Never a joined string; `t1` on one machine is not `t1` on
/// another, even under the same profile name.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RemoteRef {
    pub machine: String,
    pub machine_id: String,
    pub id: String,
}

/// A failed connection, from pod-remote's diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub kind: Option<ErrorKind>,
    pub summary: String,
    pub hint: String,
    pub detail: String,
    pub stderr: Vec<String>,
    /// The key the remote host sent, when ssh refused a changed one.
    pub sent_fingerprint: Option<String>,
    /// Retrying may help (timeouts, unreachable): the board offers it.
    pub retryable: bool,
}

impl Failure {
    pub fn from_ssh(e: &pod_remote::SshError) -> Self {
        use ErrorKind as K;
        Self {
            kind: Some(e.kind),
            summary: e.kind.summary().to_string(),
            hint: e.kind.hint().to_string(),
            detail: e.detail.clone(),
            stderr: e.stderr.clone(),
            sent_fingerprint: super::host_keys::sent_fingerprint(&e.stderr),
            retryable: matches!(e.kind, K::Timeout | K::Unreachable | K::Failed),
        }
    }

    /// A refusal that is not an ssh failure (another machine, a changed key
    /// under a confirmed pin, the policy gave up).
    pub fn other(summary: impl Into<String>, retryable: bool) -> Self {
        Self {
            kind: None,
            summary: summary.into(),
            hint: String::new(),
            detail: String::new(),
            stderr: Vec::new(),
            sent_fingerprint: None,
            retryable,
        }
    }
}

/// The control connection of a machine.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkState {
    Off,
    Connecting,
    Connected,
    /// Dropped; reconnecting with backoff. Input is disabled.
    Lost {
        since: SystemTime,
        reason: String,
        attempt: u32,
    },
    Failed(Failure),
}

/// The remote Pod that answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelloInfo {
    pub machine_id: String,
    pub boot_id: String,
    pub protocol: u32,
    pub version: String,
    pub os: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTerminalVm {
    pub id: String,
    pub name: Option<String>,
    pub state: TerminalState,
    pub task: Option<String>,
    pub created_ms: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteTaskVm {
    pub id: String,
    pub name: String,
    pub terminals: Vec<String>,
}

/// One machine on the board.
#[derive(Clone, Debug, PartialEq)]
pub struct MachineVm {
    pub profile: Machine,
    pub link: LinkState,
    pub hello: Option<HelloInfo>,
    pub identity: Option<Identity>,
    /// `None` until a connection showed who answered.
    pub trust: Option<Trust>,
    pub last_rx: Option<SystemTime>,
    pub terminals: Vec<RemoteTerminalVm>,
    pub tasks: Vec<RemoteTaskVm>,
    /// One-line notice (a lost input's fate, a daemon restart).
    pub notice: Option<String>,
}

/// Why nothing may be sent right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    NotConnected,
    NeedsConfirm,
}

impl Blocked {
    pub fn message(self) -> &'static str {
        match self {
            Blocked::NotConnected => copy::BLOCKED_DISCONNECTED,
            Blocked::NeedsConfirm => copy::BLOCKED_NOT_CONFIRMED,
        }
    }
}

impl MachineVm {
    pub fn new(profile: Machine) -> Self {
        Self {
            profile,
            link: LinkState::Off,
            hello: None,
            identity: None,
            trust: None,
            last_rx: None,
            terminals: Vec::new(),
            tasks: Vec::new(),
            notice: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.profile.name
    }

    /// Mutations (input, resize) only on a live connection to a confirmed
    /// machine (docs/22 TM-13/14).
    pub fn can_mutate(&self) -> Result<(), Blocked> {
        if self.link != LinkState::Connected {
            return Err(Blocked::NotConnected);
        }
        match self.trust {
            Some(Trust::Confirmed) => Ok(()),
            _ => Err(Blocked::NeedsConfirm),
        }
    }

    /// The confirmed machine id of the live connection (what panels and
    /// attaches may target).
    pub fn confirmed_machine_id(&self) -> Option<&str> {
        match (&self.trust, &self.hello) {
            (Some(Trust::Confirmed), Some(h)) => Some(h.machine_id.as_str()),
            _ => None,
        }
    }

    pub fn needs_confirm(&self) -> bool {
        matches!(self.trust, Some(Trust::Needs(_)))
            && matches!(self.link, LinkState::Connected | LinkState::Lost { .. })
    }

    /// The board's one-word state.
    pub fn state_label(&self) -> String {
        match &self.link {
            LinkState::Off => copy::STATE_OFF.into(),
            LinkState::Connecting => copy::STATE_CONNECTING.into(),
            LinkState::Connected if self.needs_confirm() => copy::STATE_NEEDS_CONFIRM.into(),
            LinkState::Connected => copy::STATE_CONNECTED.into(),
            LinkState::Lost { attempt, .. } if *attempt > 0 => copy::reconnecting(*attempt, None),
            LinkState::Lost { .. } => copy::STATE_LOST.into(),
            LinkState::Failed(_) => copy::STATE_FAILED.into(),
        }
    }

    pub fn remote_ref(&self, terminal: &str) -> Option<RemoteRef> {
        Some(RemoteRef {
            machine: self.profile.name.clone(),
            machine_id: self.hello.as_ref()?.machine_id.clone(),
            id: terminal.to_string(),
        })
    }
}

pub fn terminals_of(list: &[TerminalInfo], tasks: &[TaskInfo]) -> Vec<RemoteTerminalVm> {
    list.iter()
        .map(|t| RemoteTerminalVm {
            id: t.id.clone(),
            name: t.name.clone(),
            state: t.state,
            task: tasks
                .iter()
                .find(|k| k.terminals.iter().any(|x| x.id == t.id))
                .map(|k| k.id.clone()),
            created_ms: t.created_ms,
        })
        .collect()
}

pub fn tasks_of(tasks: &[TaskInfo]) -> Vec<RemoteTaskVm> {
    tasks
        .iter()
        .map(|t| RemoteTaskVm {
            id: t.id.clone(),
            name: t.name.clone(),
            terminals: t.terminals.iter().map(|x| x.id.clone()).collect(),
        })
        .collect()
}

/// What a machine's control thread reports (`controller.rs`).
#[derive(Clone, Debug)]
pub enum MachineEvent {
    Connecting,
    /// First connection up. The board evaluates trust from `identity`.
    Connected {
        hello: HelloInfo,
        identity: Identity,
    },
    Lists {
        terminals: Vec<RemoteTerminalVm>,
        tasks: Vec<RemoteTaskVm>,
        at: SystemTime,
    },
    Lost {
        since: SystemTime,
        reason: String,
    },
    /// A reconnect attempt failed (it keeps trying).
    Attempt {
        attempt: u32,
        reason: String,
    },
    Reconnected {
        hello: HelloInfo,
        boot_changed: bool,
        input: Option<pod_remote::InputResolution>,
    },
    Failed(Failure),
    Off,
}

/// Apply one event. Trust is evaluated by the caller (it owns the record)
/// and passed in for `Connected`.
pub fn apply(vm: &mut MachineVm, ev: MachineEvent, trust: impl FnOnce(&Identity) -> Trust) {
    match ev {
        MachineEvent::Connecting => {
            vm.link = LinkState::Connecting;
            vm.notice = None;
        }
        MachineEvent::Connected { hello, identity } => {
            vm.trust = Some(trust(&identity));
            vm.identity = Some(identity);
            vm.hello = Some(hello);
            vm.link = LinkState::Connected;
            vm.last_rx = Some(SystemTime::now());
        }
        MachineEvent::Lists {
            terminals,
            tasks,
            at,
        } => {
            vm.terminals = terminals;
            vm.tasks = tasks;
            vm.last_rx = Some(at);
        }
        MachineEvent::Lost { since, reason } => {
            vm.link = LinkState::Lost {
                since,
                reason,
                attempt: 0,
            };
        }
        MachineEvent::Attempt { attempt, reason } => {
            if let LinkState::Lost {
                attempt: a,
                reason: r,
                ..
            } = &mut vm.link
            {
                *a = attempt;
                *r = reason;
            }
        }
        MachineEvent::Reconnected {
            hello,
            boot_changed,
            input,
        } => {
            vm.link = LinkState::Connected;
            vm.last_rx = Some(SystemTime::now());
            vm.hello = Some(hello);
            vm.notice = match input {
                Some(pod_remote::InputResolution::Applied { .. }) => {
                    Some(copy::RESOLUTION_APPLIED.into())
                }
                Some(pod_remote::InputResolution::NotApplied { .. }) => {
                    Some(copy::RESOLUTION_NOT_APPLIED.into())
                }
                Some(pod_remote::InputResolution::Unknown { reason, .. }) => {
                    Some(copy::unknown_resolution(&reason))
                }
                None if boot_changed => Some(copy::BOOT_CHANGED.into()),
                None => None,
            };
        }
        MachineEvent::Failed(f) => {
            vm.link = LinkState::Failed(f);
        }
        MachineEvent::Off => {
            vm.link = LinkState::Off;
        }
    }
}
