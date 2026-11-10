//! The mod thread: discovers mods, keeps their enable/grant state, runs
//! their sandboxes one call at a time, checks permissions and scope for
//! every host call, enforces limits and keeps per-mod logs (docs/16 §3–§6).
//!
//! [`ModHandle`] is the cheap, cloneable front end the embedding daemon
//! uses: it filters events and screen samples against what mods registered
//! (without crossing threads) and queues the rest without blocking.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::host::{BLOCKED_KINDS, HINT_STATES, Host, Notice, Scope, StatusHint};
use crate::manifest::{MAX_ENTRY_BYTES, Manifest, Permission, valid_id, valid_name};
use crate::runtime::{Failure, HostFn, Limits, ModRuntime};
use crate::types::*;

/// Events a mod may subscribe to (docs/16 §5).
pub const EVENTS: &[&str] = &[
    "status.changed",
    "terminal.changed",
    "task.changed",
    "review.delivery",
    "gate.changed",
    "group.check",
    "check.finished",
    "orch.changed",
    "orch.mail",
];

/// Events that need the `orch` permission (docs/19 §16.2).
pub const ORCH_EVENTS: &[&str] = &["orch.changed", "orch.mail"];
pub const STATE_FILE: &str = "pod-mods.json";
const QUEUE: usize = 1024;
const LOG_LINES: usize = 500;
const LOG_FILE_MAX: u64 = 256 * 1024;
const MAX_SEND: usize = 4096;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Longest `setTimeout`/`setInterval` delay (24 h).
const MAX_TIMER_MS: f64 = 86_400_000.0;
/// Orders timers that fall due at the same instant across mods (set order).
static TIMER_SEQ: AtomicU64 = AtomicU64::new(0);

// ───────────────────────────── config ─────────────────────────────

#[derive(Debug, Clone)]
pub struct ModConfig {
    /// `%APPDATA%\Pod\mods` (or `POD_MODS_DIR`).
    pub root: PathBuf,
    /// Per-mod log files (`<dir>\<name>.log`); `None` keeps logs in memory only.
    pub log_dir: Option<PathBuf>,
    pub limits: Limits,
}

impl ModConfig {
    /// `POD_MODS_DIR`, else `%APPDATA%\Pod\mods`.
    pub fn default_root() -> PathBuf {
        if let Some(p) = std::env::var_os("POD_MODS_DIR").filter(|p| !p.is_empty()) {
            return PathBuf::from(p);
        }
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("Pod")
            .join("mods")
    }
}

// ───────────────────────────── state file ─────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StateFile {
    #[serde(default)]
    mods: BTreeMap<String, SavedMod>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SavedMod {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    granted: Vec<Permission>,
}

fn load_state(root: &Path) -> StateFile {
    std::fs::read(root.join(STATE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_state(root: &Path, s: &StateFile) -> Result<(), String> {
    let path = root.join(STATE_FILE);
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text)
        .and_then(|_| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("{}: {e}", path.display()))
}

// ───────────────────────────── handle ─────────────────────────────

type Reply<T> = mpsc::Sender<T>;

enum Input {
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
struct Shared {
    events: RwLock<HashSet<String>>,
    /// Normalized program names any running detector matches.
    detect: RwLock<HashSet<String>>,
    dropped: AtomicU64,
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

fn argv_names(argv: &[String]) -> impl Iterator<Item = String> + '_ {
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

// ───────────────────────────── per mod ─────────────────────────────

struct Core {
    name: String,
    manifest: Result<Manifest, String>,
    enabled: bool,
    granted: BTreeSet<Permission>,
    state: ModState,
    stop_reason: Option<StopReason>,
    error: Option<String>,
    errors_in_row: u32,
    log: VecDeque<String>,
    log_file: Option<PathBuf>,
    // Registrations (reset on every load).
    events: BTreeSet<String>,
    commands: Vec<CommandInfo>,
    detectors: Vec<Vec<String>>,
    // Per call.
    scope: Scope,
    loading: bool,
    actions_in_call: u32,
    recent: VecDeque<Instant>,
    // Pending timers (cleared whenever the runtime goes away).
    timers: BTreeMap<u32, Timer>,
    next_timer: u32,
}

/// A pending `setTimeout` / `setInterval`. It fires with the scope of the
/// callback that created it, so a timer cannot widen what the mod may touch.
#[derive(Debug, Clone)]
struct Timer {
    due: Instant,
    seq: u64,
    interval: Option<Duration>,
    scope: Scope,
}

fn clock() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let s = secs % 86_400;
    format!("{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
}

impl Core {
    fn has(&self, p: Permission) -> bool {
        self.granted.contains(&p)
            && self
                .manifest
                .as_ref()
                .is_ok_and(|m| m.permissions.contains(&p))
    }

    fn log(&mut self, msg: &str) {
        for line in msg.lines() {
            let line = format!("{} {line}", clock());
            if let Some(path) = &self.log_file {
                if std::fs::metadata(path).is_ok_and(|m| m.len() > LOG_FILE_MAX) {
                    let _ = std::fs::rename(path, path.with_extension("log.1"));
                }
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(f, "{line}");
                }
            }
            if self.log.len() >= LOG_LINES {
                self.log.pop_front();
            }
            self.log.push_back(line);
        }
    }

    fn info(&self, memory_kib: u64) -> ModInfo {
        let m = self.manifest.as_ref().ok();
        ModInfo {
            name: self.name.clone(),
            version: m.map(|m| m.version.clone()).unwrap_or_default(),
            title: m.and_then(|m| m.title.clone()),
            description: m.and_then(|m| m.description.clone()),
            enabled: self.enabled,
            state: self.state.clone(),
            stop_reason: self.stop_reason,
            error: self.error.clone(),
            permissions: Permission::ALL
                .into_iter()
                .filter_map(|p| {
                    let requested = m.is_some_and(|m| m.permissions.contains(&p));
                    (requested || self.granted.contains(&p)).then(|| PermissionInfo {
                        name: p,
                        requested,
                        granted: requested && self.granted.contains(&p),
                    })
                })
                .collect(),
            checks: m
                .map(|m| {
                    m.checks
                        .iter()
                        .map(|(n, a)| CheckInfo {
                            name: n.clone(),
                            argv: a.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            commands: if self.state == ModState::Running {
                self.commands.clone()
            } else {
                Vec::new()
            },
            detectors: self
                .detectors
                .iter()
                .map(|d| DetectorInfo { names: d.clone() })
                .collect(),
            layouts: if self.enabled && self.state == ModState::Running {
                m.map(|m| m.layouts.clone()).unwrap_or_default()
            } else {
                Vec::new()
            },
            log_lines: self.log.len(),
            memory_kib,
        }
    }

    /// Count a side effect against the per-call and (for `per_minute`) the
    /// per-minute budget.
    fn spend(&mut self, limits: &Limits, per_minute: bool) -> Result<(), String> {
        if self.actions_in_call >= limits.actions_per_call {
            return Err(format!(
                "RateLimited: more than {} actions in one callback",
                limits.actions_per_call
            ));
        }
        if per_minute {
            let now = Instant::now();
            while self
                .recent
                .front()
                .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
            {
                self.recent.pop_front();
            }
            if self.recent.len() as u32 >= limits.actions_per_minute {
                return Err(format!(
                    "RateLimited: more than {} actions per minute",
                    limits.actions_per_minute
                ));
            }
            self.recent.push_back(now);
        }
        self.actions_in_call += 1;
        Ok(())
    }
}

struct Slot {
    core: Rc<RefCell<Core>>,
    rt: Option<ModRuntime>,
}

impl Slot {
    fn info(&self) -> ModInfo {
        let mem = self
            .rt
            .as_ref()
            .map(|r| r.memory_used() as u64 / 1024)
            .unwrap_or(0);
        self.core.borrow().info(mem)
    }
}

fn denied(core: &mut Core, msg: String) -> String {
    core.log(&format!("denied: {msg}"));
    msg
}

fn arg<T: for<'de> Deserialize<'de>>(args: &str) -> Result<T, String> {
    serde_json::from_str(args).map_err(|e| format!("InvalidArgument: {e}"))
}

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).chain(['…']).collect()
    }
}

/// The host call behind `host(op, json)` for one mod: permission and scope
/// checks, limits, then the [`Host`].
fn host_fn(core: Rc<RefCell<Core>>, host: Arc<dyn Host>, limits: Limits) -> HostFn {
    Rc::new(
        move |op: &str, args: &str| -> Result<Option<String>, String> {
            let mut c = core.borrow_mut();
            let need = |c: &mut Core, p: Permission| -> Result<(), String> {
                if c.has(p) {
                    Ok(())
                } else {
                    Err(denied(
                        c,
                        format!("PermissionDenied: {op} needs the {:?} permission", p.name()),
                    ))
                }
            };
            // Is `terminal` inside the callback's scope?
            let in_scope = |c: &mut Core,
                            host: &Arc<dyn Host>,
                            terminal: &str,
                            allow_scope_terminal: bool|
             -> Result<(), String> {
                if allow_scope_terminal && c.scope.terminal.as_deref() == Some(terminal) {
                    return Ok(());
                }
                let Some(task) = c.scope.task.clone() else {
                    return Err(denied(
                        c,
                        format!("PermissionDenied: {op}: this callback has no task scope"),
                    ));
                };
                let t = host.terminals().into_iter().find(|t| t.id == terminal);
                match t {
                    Some(t) if t.task.as_deref() == Some(task.as_str()) => Ok(()),
                    Some(_) => Err(denied(
                        c,
                        format!("PermissionDenied: terminal {terminal} is not in task {task}"),
                    )),
                    None => Err(denied(
                        c,
                        format!("InvalidArgument: no terminal {terminal}"),
                    )),
                }
            };
            match op {
                "log" => {
                    #[derive(Deserialize)]
                    struct A {
                        msg: String,
                    }
                    let a: A = arg(args)?;
                    c.log(&trunc(&a.msg, 2000));
                    Ok(None)
                }
                "register.event" => {
                    #[derive(Deserialize)]
                    struct A {
                        event: String,
                    }
                    let a: A = arg(args)?;
                    if a.event != "*" && !EVENTS.contains(&a.event.as_str()) {
                        return Err(format!(
                            "InvalidArgument: unknown event {:?} (known: {})",
                            a.event,
                            EVENTS.join(", ")
                        ));
                    }
                    // Registering needs the request in mod.json; delivery needs
                    // the grant (so a mod still loads before the user grants it).
                    if ORCH_EVENTS.contains(&a.event.as_str())
                        && !c
                            .manifest
                            .as_ref()
                            .is_ok_and(|m| m.permissions.contains(&Permission::Orch))
                    {
                        return Err(denied(
                            &mut c,
                            format!(
                                "PermissionDenied: {} needs the \"orch\" permission in mod.json",
                                a.event
                            ),
                        ));
                    }
                    c.events.insert(a.event);
                    Ok(None)
                }
                "register.command" => {
                    #[derive(Deserialize)]
                    struct A {
                        id: String,
                        title: String,
                        detail: Option<String>,
                    }
                    need(&mut c, Permission::Commands)?;
                    if !c.loading {
                        return Err(
                            "InvalidArgument: pod.command may only be called while the mod loads"
                                .into(),
                        );
                    }
                    let a: A = arg(args)?;
                    if !valid_id(&a.id) {
                        return Err(format!(
                            "InvalidArgument: command id {:?} must match [a-z0-9-]{{1,32}}",
                            a.id
                        ));
                    }
                    if a.title.trim().is_empty() {
                        return Err("InvalidArgument: a command needs a title".into());
                    }
                    if c.commands.iter().any(|x| x.id == a.id) || c.commands.len() >= 32 {
                        return Err(format!(
                            "InvalidArgument: command {:?} is defined twice (or more than 32 commands)",
                            a.id
                        ));
                    }
                    c.commands.push(CommandInfo {
                        id: a.id,
                        title: trunc(&a.title, 60),
                        detail: a.detail.map(|d| trunc(&d, 120)),
                    });
                    Ok(None)
                }
                "register.detector" => {
                    #[derive(Deserialize)]
                    struct A {
                        #[serde(rename = "match")]
                        names: Vec<String>,
                    }
                    need(&mut c, Permission::Status)?;
                    if !c.loading {
                        return Err(
                            "InvalidArgument: pod.detector may only be called while the mod loads"
                                .into(),
                        );
                    }
                    let a: A = arg(args)?;
                    let names: Vec<String> = a
                        .names
                        .iter()
                        .map(|n| program_name(n.trim()))
                        .filter(|n| !n.is_empty() && n.len() <= 64)
                        .collect();
                    if names.is_empty() || names.len() > 8 || c.detectors.len() >= 8 {
                        return Err("InvalidArgument: a detector matches 1-8 program names (at most 8 detectors)".into());
                    }
                    c.detectors.push(names);
                    Ok(None)
                }
                "notify" => {
                    #[derive(Deserialize)]
                    struct A {
                        title: String,
                        body: String,
                        level: String,
                    }
                    need(&mut c, Permission::Notify)?;
                    let a: A = arg(args)?;
                    if !["info", "success", "warning", "error"].contains(&a.level.as_str()) {
                        return Err(format!(
                            "InvalidArgument: level {:?} must be info, success, warning or error",
                            a.level
                        ));
                    }
                    c.spend(&limits, true)?;
                    let notice = Notice {
                        mod_name: c.name.clone(),
                        title: trunc(&a.title, 100),
                        body: trunc(&a.body, 500),
                        level: a.level,
                        task: c.scope.task.clone(),
                        terminal: c.scope.terminal.clone(),
                    };
                    c.log(&format!("notify: {} — {}", notice.title, notice.body));
                    drop(c);
                    host.notify(notice);
                    Ok(None)
                }
                "query.terminals" => {
                    drop(c);
                    Ok(Some(
                        serde_json::to_string(&host.terminals()).unwrap_or_else(|_| "[]".into()),
                    ))
                }
                "query.tasks" => {
                    drop(c);
                    Ok(Some(
                        serde_json::to_string(&host.tasks()).unwrap_or_else(|_| "[]".into()),
                    ))
                }
                "query.groups" => {
                    drop(c);
                    Ok(Some(
                        serde_json::to_string(&host.groups()).unwrap_or_else(|_| "[]".into()),
                    ))
                }
                "terminal.send" => {
                    #[derive(Deserialize)]
                    struct A {
                        terminal: String,
                        text: String,
                        enter: bool,
                    }
                    need(&mut c, Permission::TerminalSend)?;
                    let a: A = arg(args)?;
                    if a.text.len() > MAX_SEND {
                        return Err(format!(
                            "InvalidArgument: at most {MAX_SEND} bytes per send"
                        ));
                    }
                    in_scope(&mut c, &host, &a.terminal, false)?;
                    c.spend(&limits, true)?;
                    c.log(&format!(
                        "send to {}: {} bytes{}",
                        a.terminal,
                        a.text.len(),
                        if a.enter { " + Enter" } else { "" }
                    ));
                    drop(c);
                    host.send_text(&a.terminal, &a.text, a.enter)
                        .map_err(|e| format!("HostError: {e}"))?;
                    Ok(None)
                }
                "terminal.status" => {
                    #[derive(Deserialize)]
                    struct A {
                        terminal: String,
                        state: String,
                        kind: Option<String>,
                        msg: Option<String>,
                    }
                    need(&mut c, Permission::Status)?;
                    let a: A = arg(args)?;
                    let hint = parse_hint(&a.state, a.kind, a.msg)?;
                    in_scope(&mut c, &host, &a.terminal, true)?;
                    c.spend(&limits, false)?;
                    let owner = c.name.clone();
                    drop(c);
                    host.set_status(&owner, &a.terminal, hint)
                        .map_err(|e| format!("HostError: {e}"))?;
                    Ok(None)
                }
                "task.check" => {
                    #[derive(Deserialize)]
                    struct A {
                        task: String,
                        check: String,
                    }
                    need(&mut c, Permission::Check)?;
                    let a: A = arg(args)?;
                    let argv = c
                        .manifest
                        .as_ref()
                        .ok()
                        .and_then(|m| m.checks.get(&a.check).cloned());
                    let Some(argv) = argv else {
                        return Err(denied(
                            &mut c,
                            format!(
                                "PermissionDenied: check {:?} is not declared in mod.json",
                                a.check
                            ),
                        ));
                    };
                    if c.scope.task.as_deref() != Some(a.task.as_str()) {
                        return Err(denied(
                            &mut c,
                            format!(
                                "PermissionDenied: task {} is not this callback's task",
                                a.task
                            ),
                        ));
                    }
                    c.spend(&limits, true)?;
                    c.log(&format!(
                        "check {} in {}: {}",
                        a.check,
                        a.task,
                        argv.join(" ")
                    ));
                    let owner = c.name.clone();
                    drop(c);
                    host.run_check(&owner, &a.task, &a.check, argv)
                        .map_err(|e| format!("HostError: {e}"))?;
                    Ok(None)
                }
                "gate.request" => {
                    #[derive(Deserialize)]
                    struct A {
                        task: String,
                        title: String,
                        message: String,
                    }
                    need(&mut c, Permission::Gate)?;
                    let a: A = arg(args)?;
                    if c.scope.task.as_deref() != Some(a.task.as_str()) {
                        return Err(denied(
                            &mut c,
                            format!(
                                "PermissionDenied: task {} is not this callback's task",
                                a.task
                            ),
                        ));
                    }
                    if a.message.trim().is_empty() {
                        return Err("InvalidArgument: a gate needs a message".into());
                    }
                    c.spend(&limits, true)?;
                    let owner = c.name.clone();
                    c.log(&format!(
                        "gate request in {}: {}",
                        a.task,
                        trunc(&a.title, 100)
                    ));
                    drop(c);
                    let id = host
                        .request_gate(
                            &owner,
                            &a.task,
                            &trunc(&a.title, 100),
                            &trunc(&a.message, 2000),
                        )
                        .map_err(|e| format!("HostError: {e}"))?;
                    Ok(Some(serde_json::to_string(&id).unwrap_or_default()))
                }
                "timer.set" => {
                    #[derive(Deserialize)]
                    struct A {
                        delay: f64,
                        repeat: bool,
                    }
                    let a: A = arg(args)?;
                    if !a.delay.is_finite() || a.delay > MAX_TIMER_MS {
                        return Err(format!(
                            "InvalidArgument: a timer delay is at most {MAX_TIMER_MS} ms"
                        ));
                    }
                    if c.timers.len() as u32 >= limits.max_timers {
                        return Err(denied(
                            &mut c,
                            format!(
                                "RateLimited: more than {} pending timers",
                                limits.max_timers
                            ),
                        ));
                    }
                    let min = if a.repeat {
                        limits.min_interval_ms
                    } else {
                        limits.min_timeout_ms
                    };
                    let delay = Duration::from_millis((a.delay.max(0.0) as u64).max(min));
                    let mut id = c.next_timer.max(1);
                    while c.timers.contains_key(&id) {
                        id = id.checked_add(1).unwrap_or(1);
                    }
                    c.next_timer = id.checked_add(1).unwrap_or(1);
                    let timer = Timer {
                        due: Instant::now() + delay,
                        seq: TIMER_SEQ.fetch_add(1, Ordering::Relaxed),
                        interval: a.repeat.then_some(delay),
                        scope: c.scope.clone(),
                    };
                    c.timers.insert(id, timer);
                    Ok(Some(id.to_string()))
                }
                "timer.clear" => {
                    #[derive(Deserialize)]
                    struct A {
                        id: u32,
                    }
                    let a: A = arg(args)?;
                    c.timers.remove(&a.id);
                    Ok(None)
                }
                other => Err(format!("InvalidArgument: unknown host call {other:?}")),
            }
        },
    )
}

/// `None` = clear (`unknown`).
fn parse_hint(
    state: &str,
    kind: Option<String>,
    msg: Option<String>,
) -> Result<Option<StatusHint>, String> {
    if state == "unknown" {
        return Ok(None);
    }
    if !HINT_STATES.contains(&state) {
        return Err(format!(
            "InvalidArgument: state {state:?} must be one of {} or unknown",
            HINT_STATES.join(", ")
        ));
    }
    let kind = match (state, kind) {
        ("blocked", Some(k)) if BLOCKED_KINDS.contains(&k.as_str()) => Some(k),
        ("blocked", Some(k)) => {
            return Err(format!(
                "InvalidArgument: blocked kind {k:?} must be one of {}",
                BLOCKED_KINDS.join(", ")
            ));
        }
        ("blocked", None) => Some("other".to_string()),
        _ => None,
    };
    Ok(Some(StatusHint {
        state: state.to_string(),
        kind,
        msg: msg.map(|m| trunc(&m, 200)),
    }))
}

// ───────────────────────────── manager ─────────────────────────────

struct Manager {
    cfg: ModConfig,
    host: Arc<dyn Host>,
    shared: Arc<Shared>,
    slots: BTreeMap<String, Slot>,
}

impl Manager {
    fn new(cfg: ModConfig, host: Arc<dyn Host>, shared: Arc<Shared>) -> Self {
        Manager {
            cfg,
            host,
            shared,
            slots: BTreeMap::new(),
        }
    }

    fn run(mut self, rx: Receiver<Input>) {
        self.scan(None);
        loop {
            // Timers are driven by this loop: fire what is due, then wait
            // for input until the next timer (never longer).
            self.fire_timers();
            let input = match self.next_due() {
                Some(due) => match rx.recv_timeout(due.saturating_duration_since(Instant::now())) {
                    Ok(i) => i,
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                },
                None => match rx.recv() {
                    Ok(i) => i,
                    Err(_) => break,
                },
            };
            match input {
                Input::Event {
                    name,
                    scope,
                    payload,
                    only,
                } => self.event(&name, scope, &payload, only.as_deref()),
                Input::Sample {
                    terminal,
                    task,
                    argv,
                    lines,
                    title,
                } => self.sample(&terminal, task, &argv, lines, title),
                Input::List(r) => {
                    let _ = r.send(self.list());
                }
                Input::SetEnabled(name, on, r) => {
                    let _ = r.send(self.set_enabled(&name, on));
                }
                Input::Grant(name, p, on, r) => {
                    let _ = r.send(self.grant(&name, p, on));
                }
                Input::Log(name, tail, r) => {
                    let _ = r.send(
                        self.slots
                            .get(&name)
                            .map(|s| {
                                let c = s.core.borrow();
                                let skip = c.log.len().saturating_sub(tail);
                                c.log.iter().skip(skip).cloned().collect()
                            })
                            .ok_or_else(|| format!("no mod {name:?}")),
                    );
                }
                Input::Command(name, cmd, scope, r) => {
                    let _ = r.send(self.command(&name, &cmd, scope));
                }
                Input::Reload(name, r) => {
                    self.scan(name.as_deref());
                    let _ = r.send(self.list());
                }
                Input::Shutdown => break,
            }
        }
        for name in self.slots.keys().cloned().collect::<Vec<_>>() {
            self.unload(&name);
        }
    }

    fn list(&self) -> ModList {
        ModList {
            root: self.cfg.root.display().to_string(),
            mods: self.slots.values().map(Slot::info).collect(),
            dropped_events: self.shared.dropped.load(Ordering::Relaxed),
        }
    }

    /// (Re)discover mods: all of them, or only `only`.
    fn scan(&mut self, only: Option<&str>) {
        let state = load_state(&self.cfg.root);
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.cfg.root) {
            for e in rd.flatten() {
                let path = e.path();
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                if path.is_dir()
                    && path.join(crate::manifest::MANIFEST_FILE).is_file()
                    && only.is_none_or(|o| o == name)
                {
                    found.push((name, path));
                }
            }
        }
        found.sort();
        let names: Vec<String> = match only {
            Some(o) => vec![o.to_string()],
            None => self
                .slots
                .keys()
                .cloned()
                .chain(found.iter().map(|(n, _)| n.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        };
        for name in names {
            let old_log = self.slots.get(&name).map(|s| s.core.borrow().log.clone());
            self.unload(&name);
            self.slots.remove(&name);
            let Some((_, dir)) = found.iter().find(|(n, _)| *n == name) else {
                continue;
            };
            let manifest = if valid_name(&name) {
                Manifest::load(dir)
            } else {
                Err(format!("folder name {name:?} is not a valid mod name"))
            };
            let saved = state.mods.get(&name).cloned().unwrap_or_default();
            let core = Core {
                name: name.clone(),
                state: if manifest.is_err() {
                    ModState::Invalid
                } else {
                    ModState::Disabled
                },
                error: manifest.as_ref().err().cloned(),
                manifest,
                enabled: saved.enabled,
                granted: saved.granted.into_iter().collect(),
                stop_reason: None,
                errors_in_row: 0,
                log: old_log.unwrap_or_default(),
                log_file: self.cfg.log_dir.as_ref().map(|d| {
                    let _ = std::fs::create_dir_all(d);
                    d.join(format!("{name}.log"))
                }),
                events: BTreeSet::new(),
                commands: Vec::new(),
                detectors: Vec::new(),
                scope: Scope::default(),
                loading: false,
                actions_in_call: 0,
                recent: VecDeque::new(),
                timers: BTreeMap::new(),
                next_timer: 1,
            };
            self.slots.insert(
                name.clone(),
                Slot {
                    core: Rc::new(RefCell::new(core)),
                    rt: None,
                },
            );
            if saved.enabled {
                self.load(&name);
            }
        }
        self.refresh_filters();
    }

    /// Start an enabled, valid mod.
    fn load(&mut self, name: &str) {
        let limits = self.cfg.limits;
        let host = self.host.clone();
        let Some(slot) = self.slots.get_mut(name) else {
            return;
        };
        let (entry, version) = {
            let mut c = slot.core.borrow_mut();
            let Ok(m) = &c.manifest else { return };
            let (entry, version) = (m.entry.clone(), m.version.clone());
            c.events.clear();
            c.commands.clear();
            c.detectors.clear();
            c.timers.clear();
            c.error = None;
            c.stop_reason = None;
            c.errors_in_row = 0;
            c.state = ModState::Running;
            (entry, version)
        };
        let Some(entry) = entry else {
            slot.core.borrow_mut().log("loaded (layouts only)");
            return;
        };
        let source = match std::fs::metadata(&entry) {
            Ok(m) if m.len() > MAX_ENTRY_BYTES => Err(format!(
                "{} is larger than {MAX_ENTRY_BYTES} bytes",
                entry.display()
            )),
            Ok(_) => {
                std::fs::read_to_string(&entry).map_err(|e| format!("{}: {e}", entry.display()))
            }
            Err(e) => Err(format!("{}: {e}", entry.display())),
        };
        let filename = entry
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("main.js")
            .to_string();
        let result = source.map_err(Failure::Exception).and_then(|src| {
            {
                let mut c = slot.core.borrow_mut();
                c.loading = true;
                c.scope = Scope::default();
                c.actions_in_call = 0;
            }
            let r = ModRuntime::load(
                name,
                &version,
                &src,
                &filename,
                limits,
                host_fn(slot.core.clone(), host, limits),
            );
            slot.core.borrow_mut().loading = false;
            r
        });
        match result {
            Ok(rt) => {
                slot.rt = Some(rt);
                let mut c = slot.core.borrow_mut();
                let msg = format!(
                    "loaded: {} event(s), {} command(s), {} detector(s)",
                    c.events.len(),
                    c.commands.len(),
                    c.detectors.len()
                );
                c.log(&msg);
            }
            Err(f) => {
                let reason = match &f {
                    Failure::Timeout => StopReason::Cpu,
                    Failure::Memory(_) => StopReason::Memory,
                    Failure::Exception(_) => StopReason::Load,
                };
                self.stop(name, reason, &f.to_string());
            }
        }
    }

    /// Drop the runtime and anything the mod put into the daemon.
    fn unload(&mut self, name: &str) {
        if let Some(slot) = self.slots.get_mut(name) {
            let had = slot.rt.take().is_some() || !slot.core.borrow().detectors.is_empty();
            let mut c = slot.core.borrow_mut();
            c.events.clear();
            c.commands.clear();
            c.detectors.clear();
            c.timers.clear();
            if had || c.state == ModState::Running {
                drop(c);
                self.host.clear_status(name);
            }
        }
    }

    fn stop(&mut self, name: &str, reason: StopReason, detail: &str) {
        self.unload(name);
        if let Some(slot) = self.slots.get(name) {
            let mut c = slot.core.borrow_mut();
            c.state = ModState::Stopped;
            c.stop_reason = Some(reason);
            c.error = Some(trunc(detail, 2000));
            c.log(&format!("stopped ({reason:?}): {detail}"));
        }
        self.refresh_filters();
        if let Some(s) = self.slots.get(name) {
            self.host.mod_changed(&s.info());
        }
    }

    /// Recompute the event and detector filters [`ModHandle`] reads.
    fn refresh_filters(&self) {
        let mut events = HashSet::new();
        let mut detect = HashSet::new();
        for s in self.slots.values().filter(|s| s.rt.is_some()) {
            let c = s.core.borrow();
            events.extend(c.events.iter().cloned());
            for d in &c.detectors {
                detect.extend(d.iter().cloned());
            }
        }
        *self.shared.events.write().unwrap() = events;
        *self.shared.detect.write().unwrap() = detect;
    }

    fn save(&self) -> Result<(), String> {
        let mut st = load_state(&self.cfg.root);
        for s in self.slots.values() {
            let c = s.core.borrow();
            st.mods.insert(
                c.name.clone(),
                SavedMod {
                    enabled: c.enabled,
                    granted: c.granted.iter().copied().collect(),
                },
            );
        }
        save_state(&self.cfg.root, &st)
    }

    fn set_enabled(&mut self, name: &str, on: bool) -> Result<ModInfo, String> {
        let slot = self
            .slots
            .get(name)
            .ok_or_else(|| format!("no mod {name:?}"))?;
        {
            let mut c = slot.core.borrow_mut();
            if on && c.manifest.is_err() {
                return Err(format!(
                    "mod {name:?} is invalid: {}",
                    c.error.clone().unwrap_or_default()
                ));
            }
            if on && !c.enabled && c.granted.is_empty() {
                let defaults: Vec<Permission> = c
                    .manifest
                    .as_ref()
                    .map(|m| {
                        m.permissions
                            .iter()
                            .copied()
                            .filter(|p| p.granted_by_default())
                            .collect()
                    })
                    .unwrap_or_default();
                c.granted.extend(defaults);
            }
            c.enabled = on;
            c.log(if on { "enabled" } else { "disabled" });
        }
        self.save()?;
        self.restart(name, on);
        Ok(self.slots[name].info())
    }

    fn grant(&mut self, name: &str, p: Permission, on: bool) -> Result<ModInfo, String> {
        let slot = self
            .slots
            .get(name)
            .ok_or_else(|| format!("no mod {name:?}"))?;
        let enabled = {
            let mut c = slot.core.borrow_mut();
            if on
                && !c
                    .manifest
                    .as_ref()
                    .is_ok_and(|m| m.permissions.contains(&p))
            {
                return Err(format!("mod {name:?} does not request {:?}", p.name()));
            }
            if on {
                c.granted.insert(p)
            } else {
                c.granted.remove(&p)
            };
            c.log(&format!(
                "permission {} {}",
                p.name(),
                if on { "granted" } else { "revoked" }
            ));
            c.enabled
        };
        self.save()?;
        self.restart(name, enabled);
        Ok(self.slots[name].info())
    }

    fn restart(&mut self, name: &str, on: bool) {
        self.unload(name);
        if let Some(s) = self.slots.get(name) {
            let mut c = s.core.borrow_mut();
            if c.manifest.is_ok() {
                c.state = ModState::Disabled;
                c.stop_reason = None;
                c.error = None;
            }
        }
        if on {
            self.load(name);
        }
        self.refresh_filters();
        if let Some(s) = self.slots.get(name) {
            self.host.mod_changed(&s.info());
        }
    }

    /// Call into a running mod with `scope`; handles failures.
    fn call(
        &mut self,
        name: &str,
        kind: &str,
        key: &str,
        payload: &str,
        scope: &Scope,
    ) -> Result<Option<String>, String> {
        let slot = self
            .slots
            .get(name)
            .ok_or_else(|| format!("no mod {name:?}"))?;
        let Some(rt) = &slot.rt else {
            return Err(format!("mod {name:?} is not running"));
        };
        {
            let mut c = slot.core.borrow_mut();
            c.scope = scope.clone();
            c.actions_in_call = 0;
        }
        let r = rt.call(kind, key, payload);
        let max = self.cfg.limits.max_errors_in_row;
        match r {
            Ok(v) => {
                slot.core.borrow_mut().errors_in_row = 0;
                Ok(v)
            }
            Err(f) => {
                let msg = f.to_string();
                let stop = {
                    let mut c = slot.core.borrow_mut();
                    c.log(&format!("error in {kind} {key}: {msg}"));
                    c.errors_in_row += 1;
                    match &f {
                        Failure::Timeout => Some(StopReason::Cpu),
                        Failure::Memory(_) => Some(StopReason::Memory),
                        Failure::Exception(_) if c.errors_in_row >= max => Some(StopReason::Errors),
                        Failure::Exception(_) => None,
                    }
                };
                if let Some(reason) = stop {
                    self.stop(name, reason, &msg);
                }
                Err(msg)
            }
        }
    }

    /// The earliest pending timer of a running mod.
    fn next_due(&self) -> Option<Instant> {
        self.slots
            .values()
            .filter(|s| s.rt.is_some())
            .filter_map(|s| s.core.borrow().timers.values().map(|t| t.due).min())
            .min()
    }

    /// Run every timer that is due now, earliest first (ties in set order).
    /// Timers that fall due while these run wait for the next turn of the
    /// loop, so queued input is never starved by a burst of timers.
    fn fire_timers(&mut self) {
        let now = Instant::now();
        let mut due: Vec<(Instant, u64, String, u32)> = Vec::new();
        for (name, s) in self.slots.iter().filter(|(_, s)| s.rt.is_some()) {
            for (id, t) in s.core.borrow().timers.iter().filter(|(_, t)| t.due <= now) {
                due.push((t.due, t.seq, name.clone(), *id));
            }
        }
        due.sort();
        for (_, _, name, id) in due {
            // An earlier callback may have cleared it, or the mod stopped.
            let scope = {
                let Some(slot) = self.slots.get(&name).filter(|s| s.rt.is_some()) else {
                    continue;
                };
                let mut c = slot.core.borrow_mut();
                let Some(t) = c.timers.get_mut(&id) else {
                    continue;
                };
                let scope = t.scope.clone();
                match t.interval {
                    // From now, not from the old due time: no catch-up bursts.
                    Some(every) => t.due = Instant::now() + every,
                    None => {
                        c.timers.remove(&id);
                    }
                }
                scope
            };
            let _ = self.call(&name, "timer", &id.to_string(), "{}", &scope);
        }
    }

    fn running_with<F: Fn(&Core) -> bool>(&self, f: F) -> Vec<String> {
        self.slots
            .iter()
            .filter(|(_, s)| s.rt.is_some() && f(&s.core.borrow()))
            .map(|(n, _)| n.clone())
            .collect()
    }

    fn event(&mut self, name: &str, scope: Scope, payload: &str, only: Option<&str>) {
        // `"*"` listeners see orchestration events only with the permission.
        let orch = ORCH_EVENTS.contains(&name);
        let targets = self.running_with(|c| {
            only.is_none_or(|o| o == c.name)
                && (c.events.contains(name) || c.events.contains("*"))
                && (!orch || c.has(Permission::Orch))
        });
        for m in targets {
            let _ = self.call(&m, "event", name, payload, &scope);
        }
    }

    fn sample(
        &mut self,
        terminal: &str,
        task: Option<String>,
        argv: &[String],
        lines: Option<Vec<String>>,
        title: Option<String>,
    ) {
        let names: Vec<String> = argv_names(argv).collect();
        let scope = Scope {
            task: task.clone(),
            terminal: Some(terminal.to_string()),
        };
        let targets: Vec<(String, Vec<usize>)> = self
            .slots
            .iter()
            .filter(|(_, s)| s.rt.is_some())
            .filter_map(|(n, s)| {
                let c = s.core.borrow();
                let idx: Vec<usize> = c
                    .detectors
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| d.iter().any(|x| names.contains(x)))
                    .map(|(i, _)| i)
                    .collect();
                (!idx.is_empty() && c.has(Permission::Status)).then(|| (n.clone(), idx))
            })
            .collect();
        for (m, idx) in targets {
            for i in idx {
                for (kind, payload) in [
                    ("title", title.as_ref().map(|t| json!({"terminal": terminal, "task": task, "title": t}))),
                    ("screen", lines.as_ref().map(|l| json!({"terminal": terminal, "task": task, "lines": l, "title": title}))),
                ] {
                    let Some(payload) = payload else { continue };
                    let Ok(Some(ret)) = self.call(&m, kind, &i.to_string(), &payload.to_string(), &scope) else { continue };
                    let applied = serde_json::from_str::<Value>(&ret).map_err(|e| e.to_string()).and_then(|v| {
                        let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
                        parse_hint(&s("state").unwrap_or_default(), s("kind"), s("msg"))
                    });
                    match applied {
                        Ok(hint) => {
                            if let Err(e) = self.host.set_status(&m, terminal, hint) {
                                self.log(&m, &format!("status for {terminal}: {e}"));
                            }
                        }
                        Err(e) => self.log(&m, &format!("detector {i} returned a bad value: {e}")),
                    }
                }
            }
        }
    }

    fn log(&self, name: &str, msg: &str) {
        if let Some(s) = self.slots.get(name) {
            s.core.borrow_mut().log(msg);
        }
    }

    fn command(&mut self, name: &str, cmd: &str, scope: Scope) -> Result<(), String> {
        let known = self
            .slots
            .get(name)
            .is_some_and(|s| s.core.borrow().commands.iter().any(|c| c.id == cmd));
        if !known {
            return Err(format!("mod {name:?} has no command {cmd:?}"));
        }
        let payload = serde_json::to_string(&scope).unwrap_or_else(|_| "{}".into());
        self.call(name, "command", cmd, &payload, &scope)
            .map(|_| ())
    }
}
