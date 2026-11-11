//! The mod thread: discovers mods, keeps their enable/grant state, runs
//! their sandboxes one call at a time, checks permissions and scope for
//! every host call, enforces limits and keeps per-mod logs (docs/16 §3–§6).
//!
//! [`ModHandle`] is the cheap, cloneable front end the embedding daemon
//! uses: it filters events and screen samples against what mods registered
//! (without crossing threads) and queues the rest without blocking.
//!
//! Layout: this file owns the mod thread (`Manager`: queue loop, scan,
//! load/unload, calls into mods). `handle` is the daemon's side, `core` the
//! per-mod state, `host_api` every host call's checks, `timers` the
//! `setTimeout` machinery and `state` the persisted enable/grant file.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use serde_json::{Value, json};

use crate::host::{Host, Scope};
use crate::manifest::{MAX_ENTRY_BYTES, Manifest, Permission, valid_name};
use crate::runtime::{Failure, Limits, ModRuntime};
use crate::types::{ModInfo, ModList, ModState, StopReason};

mod core;
mod handle;
mod host_api;
mod state;
mod timers;

use self::core::{Core, Slot};
use self::handle::{Input, Shared, argv_names};
pub use self::handle::{ModHandle, program_name};
use self::host_api::{host_fn, parse_hint};
use self::state::load_state;

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

fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max.saturating_sub(1)).chain(['…']).collect()
    }
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
