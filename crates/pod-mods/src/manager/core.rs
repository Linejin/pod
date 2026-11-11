//! One mod on the mod thread: [`Core`], the state its host function shares
//! (registrations, the scope of the current call, budgets, log, timers), and
//! [`Slot`], that state plus the running sandbox.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use super::timers::Timer;
use crate::host::Scope;
use crate::manifest::{Manifest, Permission};
use crate::runtime::{Limits, ModRuntime};
use crate::types::{
    CheckInfo, CommandInfo, DetectorInfo, ModInfo, ModState, PermissionInfo, StopReason,
};

const LOG_LINES: usize = 500;
const LOG_FILE_MAX: u64 = 256 * 1024;

pub(super) struct Core {
    pub(super) name: String,
    pub(super) manifest: Result<Manifest, String>,
    pub(super) enabled: bool,
    pub(super) granted: BTreeSet<Permission>,
    pub(super) state: ModState,
    pub(super) stop_reason: Option<StopReason>,
    pub(super) error: Option<String>,
    pub(super) errors_in_row: u32,
    pub(super) log: VecDeque<String>,
    pub(super) log_file: Option<PathBuf>,
    // Registrations (reset on every load).
    pub(super) events: BTreeSet<String>,
    pub(super) commands: Vec<CommandInfo>,
    pub(super) detectors: Vec<Vec<String>>,
    // Per call.
    pub(super) scope: Scope,
    pub(super) loading: bool,
    pub(super) actions_in_call: u32,
    pub(super) recent: VecDeque<Instant>,
    // Pending timers (cleared whenever the runtime goes away).
    pub(super) timers: BTreeMap<u32, Timer>,
    pub(super) next_timer: u32,
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
    pub(super) fn has(&self, p: Permission) -> bool {
        self.granted.contains(&p)
            && self
                .manifest
                .as_ref()
                .is_ok_and(|m| m.permissions.contains(&p))
    }

    pub(super) fn log(&mut self, msg: &str) {
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
    pub(super) fn spend(&mut self, limits: &Limits, per_minute: bool) -> Result<(), String> {
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

pub(super) struct Slot {
    pub(super) core: Rc<RefCell<Core>>,
    pub(super) rt: Option<ModRuntime>,
}

impl Slot {
    pub(super) fn info(&self) -> ModInfo {
        let mem = self
            .rt
            .as_ref()
            .map(|r| r.memory_used() as u64 / 1024)
            .unwrap_or(0);
        self.core.borrow().info(mem)
    }
}
