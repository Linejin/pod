//! Orchestration in the daemon (docs/19 S4–S6): the `orch.*` RPCs over the
//! `pod-orch` engine, worker start (task + agent terminal + brief) through
//! the resource queue (S6: worker counts, CPU, memory, disk; FIFO), mail
//! wake-ups typed into idle agents, supervision (ready, turn ended without
//! `done`, stalled, lost, auto-release, a person typing keeps a worker) and
//! decision gates with options on items (§4.5).
//!
//! State lives in `<state>\orch\runs` (see `pod_orch::store`). The engine is
//! behind a std mutex that is never held across an await; mutating RPCs are
//! serialized by `orch_ops` so a `request_id` retry sees the first result.
//!
//! The code is split by responsibility; this file keeps the public
//! constants, [`OrchRuns`] and the paths the rest of the server uses
//! (`handles`, `dispatch`, `start`):
//! - `state`: the `ops` lock and the waiter / stopping guards, the
//!   `request_id` cache, supervisor passes, unconfirmed wake-ups, config;
//! - `identity`: who is calling (attempt token, terminal), the caller's run;
//! - `records`: runs and items;
//! - `launch`: worker start through the resource queue, its limits;
//! - `workers`: list, show, read, stop, keep / abandon / release, commit;
//! - `mail`: send, reply, check, ask, done, wait;
//! - `supervisor`: the supervisor loop (ready, turn ended, stalled, lost,
//!   auto-release) and the links to review / group checks;
//! - `wake`: typing wake-ups into idle agents;
//! - `gates`: decision gates with options on items;
//! - `rpc`: the `orch.*` dispatch and the engine's error codes.

mod gates;
mod identity;
mod launch;
mod mail;
mod records;
mod rpc;
mod state;
mod supervisor;
mod wake;
mod workers;

use std::collections::HashMap;
use std::path::PathBuf;

use pod_orch::ResourceLimits;
use pod_orch::engine::Orch;

use super::log;
use super::orch_limits::{self, LimitsState};
use super::orch_meter::{self, Meter};

pub use launch::brief_text;
pub(super) use rpc::{dispatch, handles};
pub use state::{Passes, Unconfirmed};
pub(super) use supervisor::start;

/// Environment of worker agent terminals (docs/19 §3.4).
pub const ENV_RUN_ID: &str = "POD_RUN_ID";
pub const ENV_ITEM_ID: &str = "POD_ITEM_ID";
pub const ENV_ATTEMPT_ID: &str = "POD_ATTEMPT_ID";
pub const ENV_ATTEMPT_TOKEN: &str = "POD_ATTEMPT_TOKEN";
pub use super::sessions::ENV_BRIEF;

/// The first prompt of claude/codex workers: one safe line (no quotes, `%`
/// or line breaks, so `.cmd` shims pass it unchanged).
pub use super::sessions::DEFAULT_FIRST_LINE as BRIEF_PROMPT;
/// Version of the brief header text.
pub const BRIEF_VERSION: u32 = 1;

/// `ask` waits this long by default, and at most `ASK_MAX_MS`.
pub const ASK_DEFAULT_MS: u64 = 10 * 60_000;
pub const ASK_MAX_MS: u64 = 30 * 60_000;
/// Output kept from a released worker.
pub const ARCHIVE_BYTES: usize = 1024 * 1024;

pub use super::orch_config::OrchConfig;

/// `4G`, `512M`, `1.5GiB`, plain bytes; `off`/`0` = no check.
pub fn parse_bytes(v: &str) -> Option<Option<u64>> {
    let v = v.trim().to_ascii_lowercase();
    if v == "off" || v == "0" {
        return Some(None);
    }
    let v = v.trim_end_matches("ib").trim_end_matches('b');
    let (num, mul) = match v.char_indices().last()? {
        (i, 'k') => (&v[..i], 1u64 << 10),
        (i, 'm') => (&v[..i], 1u64 << 20),
        (i, 'g') => (&v[..i], 1u64 << 30),
        (i, 't') => (&v[..i], 1u64 << 40),
        _ => (v, 1),
    };
    let n: f64 = num
        .trim()
        .parse()
        .ok()
        .filter(|n: &f64| n.is_finite() && *n >= 0.0)?;
    Some(Some((n * mul as f64) as u64))
}

/// `70`, `70%`; `off` = no check.
pub fn parse_pct(v: &str) -> Option<Option<f64>> {
    let v = v.trim().trim_end_matches('%');
    if v.eq_ignore_ascii_case("off") {
        return Some(None);
    }
    v.parse::<f64>()
        .ok()
        .filter(|p| (0.0..=100.0).contains(p))
        .map(Some)
}

/// The daemon's orchestration state.
pub struct OrchRuns {
    pub engine: std::sync::Mutex<Orch>,
    /// Serializes mutating `orch.*` calls (request_id idempotency).
    pub ops: tokio::sync::Mutex<()>,
    /// Fired on every new message (blocked `check --wait` / `ask`).
    pub mail: tokio::sync::Notify,
    /// Mailboxes with a blocked reader: no notification is typed for them.
    pub waiters: std::sync::Mutex<HashMap<String, usize>>,
    /// Wakes the supervisor.
    pub wake: tokio::sync::Notify,
    /// Attempts a `stop` is ending: the supervisor must not call them lost.
    pub stopping: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Timings and defaults (`orch.config.*`, `orch_config.rs`).
    pub config: std::sync::Mutex<OrchConfig>,
    state_dir: PathBuf,
    /// The resource gate's limits (docs/19 §5.1): saved settings with
    /// environment overrides (`orch_limits.rs`, `orch.limits.*`).
    pub limits: std::sync::Mutex<LimitsState>,
    /// Measures CPU, memory and disk for the resource gate (injectable).
    pub meter: Box<dyn Meter>,
    /// Review deliveries and group checks already mailed to coordinators
    /// (docs/19 §16.2; hub events can repeat).
    linked: std::sync::Mutex<std::collections::HashSet<String>>,
    /// orch-02: when a typed wake-up was first left unconfirmed, by mailbox
    /// (memory only, like the engine's count).
    pub unconfirmed: Unconfirmed,
    /// orch-quality-06: supervisor passes and wake-ups in flight.
    pub passes: Passes,
    /// orch-quality-02: terminals of open attempts whose holder was
    /// unreachable at start; their turn count is re-based once reached.
    /// `true` once a pass found it neither attached nor retried (a retry
    /// that reached it moves it from one list to the other).
    rebind: std::sync::Mutex<HashMap<String, bool>>,
}

impl OrchRuns {
    pub fn load(state_dir: &std::path::Path) -> Self {
        let (mut config, note) = OrchConfig::load(state_dir);
        if let Some(n) = note {
            log(&n);
        }
        let mut notes = Vec::new();
        let limits = LimitsState::load(
            &orch_limits::default_path(state_dir),
            |k| std::env::var(k).ok(),
            &mut notes,
        );
        for n in &notes {
            log(&format!("orch limits: {n}"));
        }
        config.limits.run_worker_cap = limits.effective.run_workers;
        let engine = Orch::load(
            &state_dir.join("orch").join("runs"),
            config.limits,
            crate::now_ms(),
        );
        for n in &engine.notes {
            log(&format!("orch: {n}"));
        }
        OrchRuns {
            engine: std::sync::Mutex::new(engine),
            ops: tokio::sync::Mutex::new(()),
            mail: tokio::sync::Notify::new(),
            waiters: std::sync::Mutex::new(HashMap::new()),
            wake: tokio::sync::Notify::new(),
            stopping: std::sync::Mutex::new(Default::default()),
            linked: std::sync::Mutex::new(Default::default()),
            unconfirmed: Unconfirmed::default(),
            passes: Passes::default(),
            rebind: std::sync::Mutex::new(Default::default()),
            config: std::sync::Mutex::new(config),
            state_dir: state_dir.to_path_buf(),
            limits: std::sync::Mutex::new(limits),
            meter: orch_meter::from_env(),
        }
    }

    /// The resource gate's limits in effect now.
    pub fn resources(&self) -> ResourceLimits {
        self.limits.lock().unwrap().effective
    }
}

/// `<state>\orch\runs` (tests and docs).
pub fn runs_dir(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("orch").join("runs")
}
