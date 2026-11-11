//! [`ModHandle`], the daemon's side of the mod thread: the queue into it
//! (never blocking; a full queue counts a drop), the event and detector
//! filters it checks without crossing threads, and request/answer calls.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use serde_json::Value;

use super::{Manager, ModConfig};
use crate::host::{Host, Scope};
use crate::manifest::Permission;
use crate::types::{ModInfo, ModList};

const QUEUE: usize = 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

type Reply<T> = mpsc::Sender<T>;

pub(super) enum Input {
    Event {
        name: String,
        scope: Scope,
        payload: String,
        only: Option<String>,
    },
    Sample {
        terminal: String,
        task: Option<String>,
        argv: Vec<String>,
        lines: Option<Vec<String>>,
        title: Option<String>,
    },
    List(Reply<ModList>),
    SetEnabled(String, bool, Reply<Result<ModInfo, String>>),
    Grant(String, Permission, bool, Reply<Result<ModInfo, String>>),
    Log(String, usize, Reply<Result<Vec<String>, String>>),
    Command(String, String, Scope, Reply<Result<(), String>>),
    Reload(Option<String>, Reply<ModList>),
    Shutdown,
}

/// Lock-free-ish filters the daemon checks before queueing anything.
#[derive(Default)]
pub(super) struct Shared {
    pub(super) events: RwLock<HashSet<String>>,
    /// Normalized program names any running detector matches.
    pub(super) detect: RwLock<HashSet<String>>,
    pub(super) dropped: AtomicU64,
}

/// Front end of the mod thread.
#[derive(Clone)]
pub struct ModHandle {
    tx: SyncSender<Input>,
    shared: Arc<Shared>,
}

/// `C:\x\Aider.EXE` → `aider`.
pub fn program_name(s: &str) -> String {
    let base = s
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(s)
        .to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(stem) = base.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    base
}

pub(super) fn argv_names(argv: &[String]) -> impl Iterator<Item = String> + '_ {
    argv.iter()
        .filter(|a| !a.starts_with('-') && !a.starts_with('/'))
        .map(|a| program_name(a))
}

impl ModHandle {
    /// Start the mod thread: scan `cfg.root`, load enabled mods.
    pub fn start(cfg: ModConfig, host: Arc<dyn Host>) -> ModHandle {
        let (tx, rx) = mpsc::sync_channel(QUEUE);
        let shared = Arc::new(Shared::default());
        let sh = shared.clone();
        std::thread::Builder::new()
            .name("pod-mods".into())
            .spawn(move || Manager::new(cfg, host, sh).run(rx))
            .expect("spawn pod-mods thread");
        ModHandle { tx, shared }
    }

    fn queue(&self, input: Input) {
        match self.tx.try_send(input) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.shared.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// Does any running mod listen for `name`?
    pub fn wants_event(&self, name: &str) -> bool {
        let ev = self.shared.events.read().unwrap();
        ev.contains(name) || ev.contains("*")
    }

    /// Deliver a daemon event to every mod that registered for it.
    pub fn post_event(&self, name: &str, scope: Scope, payload: &Value) {
        if self.wants_event(name) {
            self.queue(Input::Event {
                name: name.into(),
                scope,
                payload: payload.to_string(),
                only: None,
            });
        }
    }

    /// Deliver an event to one mod (e.g. its own `check.finished`).
    pub fn post_to(&self, owner: &str, name: &str, scope: Scope, payload: &Value) {
        self.queue(Input::Event {
            name: name.into(),
            scope,
            payload: payload.to_string(),
            only: Some(owner.into()),
        });
    }

    /// Does a running detector match this terminal's argv?
    pub fn wants_terminal(&self, argv: &[String]) -> bool {
        let d = self.shared.detect.read().unwrap();
        !d.is_empty() && argv_names(argv).any(|n| d.contains(&n))
    }

    /// A screen and/or title sample of a terminal for status detectors.
    pub fn post_sample(
        &self,
        terminal: &str,
        task: Option<&str>,
        argv: &[String],
        lines: Option<Vec<String>>,
        title: Option<String>,
    ) {
        if (lines.is_some() || title.is_some()) && self.wants_terminal(argv) {
            self.queue(Input::Sample {
                terminal: terminal.into(),
                task: task.map(Into::into),
                argv: argv.to_vec(),
                lines,
                title,
            });
        }
    }

    fn ask<T>(&self, make: impl FnOnce(Reply<T>) -> Input) -> Result<T, String> {
        let (tx, rx) = mpsc::channel();
        self.tx
            .send(make(tx))
            .map_err(|_| "mod thread stopped".to_string())?;
        rx.recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| "mod thread did not answer".to_string())
    }

    pub fn list(&self) -> Result<ModList, String> {
        self.ask(Input::List)
    }

    pub fn set_enabled(&self, name: &str, enabled: bool) -> Result<ModInfo, String> {
        self.ask(|r| Input::SetEnabled(name.into(), enabled, r))?
    }

    pub fn grant(
        &self,
        name: &str,
        permission: Permission,
        granted: bool,
    ) -> Result<ModInfo, String> {
        self.ask(|r| Input::Grant(name.into(), permission, granted, r))?
    }

    pub fn log(&self, name: &str, tail: usize) -> Result<Vec<String>, String> {
        self.ask(|r| Input::Log(name.into(), tail, r))?
    }

    /// Run a palette command; `scope` is the task/terminal the user is in.
    pub fn command(&self, name: &str, command: &str, scope: Scope) -> Result<(), String> {
        self.ask(|r| Input::Command(name.into(), command.into(), scope, r))?
    }

    /// Re-read the mods folder (`None`) or one mod.
    pub fn reload(&self, name: Option<&str>) -> Result<ModList, String> {
        self.ask(|r| Input::Reload(name.map(Into::into), r))
    }

    /// Stop the thread (drops every runtime).
    pub fn shutdown(&self) {
        let _ = self.tx.send(Input::Shutdown);
    }
}
