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

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pod_orch::engine::{BeginAttempt, ErrorKind, NewMail, Orch, OrchError, hash_token};
use pod_orch::model::{attempt_mailbox, run_mailbox};
use pod_orch::{ResourceLimits, wake};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::gates::Decide;
use super::orch_limits::{self, LimitsState};
use super::orch_meter::{self, Meter};
use super::orch_store::GateActor;
use super::terminal::Terminal;
use super::{Daemon, log, params, status, to_value};
use crate::protocol::*;

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

const TICK_MS: u64 = 500;

pub use super::orch_config::OrchConfig;

fn release_ms(c: &OrchConfig) -> Option<u64> {
    c.release_after.map(|d| d.as_millis() as u64)
}

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

/// orch-quality-06: progress of the supervisor, for `orch.queue` (tests
/// wait for passes instead of sleeping before "nothing happened").
#[derive(Default)]
pub struct Passes {
    done: std::sync::atomic::AtomicU64,
    wakes: std::sync::atomic::AtomicU64,
}

impl Passes {
    fn pass_done(&self) {
        self.done.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn wake_started(&self) {
        self.wakes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn wake_settled(&self) {
        self.wakes.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }

    /// (passes finished, wake-ups in flight).
    pub fn get(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::SeqCst;
        (self.done.load(SeqCst), self.wakes.load(SeqCst))
    }
}

/// orch-02: since when each mailbox has a typed but unconfirmed wake-up.
/// The engine's [`Orch::wake_unconfirmed`] count decides whether one is
/// open (a check or a restart clears it); this only remembers the time.
#[derive(Default)]
pub struct Unconfirmed(std::sync::Mutex<HashMap<String, u64>>);

impl Unconfirmed {
    /// A typed wake-up for these mailboxes was not confirmed at `now`
    /// (the first time counts).
    pub fn note(&self, mailboxes: &[String], now: u64) {
        let mut m = self.0.lock().unwrap();
        for mb in mailboxes {
            m.entry(mb.clone()).or_insert(now);
        }
    }

    /// Since when `mailbox` has an open unconfirmed wake-up; forgets
    /// settled ones.
    pub fn since(&self, o: &Orch, mailbox: &str) -> Option<u64> {
        let mut m = self.0.lock().unwrap();
        if o.wake_unconfirmed(mailbox) == 0 {
            m.remove(mailbox);
            return None;
        }
        // Unknown time (cannot happen: noted when counted): say "now".
        Some(*m.entry(mailbox.to_string()).or_insert_with(crate::now_ms))
    }
}

/// What a queued worker needs to start later (stored on the attempt): its
/// start parameters without the caller's token, and the caller's terminal.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct QueuedLaunch {
    params: OrchWorkerStartParams,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal: Option<String>,
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

/// Holds a mailbox's "someone is blocked reading it" mark.
struct Waiting<'a> {
    runs: &'a OrchRuns,
    boxes: Vec<String>,
}

impl<'a> Waiting<'a> {
    fn new(runs: &'a OrchRuns, boxes: Vec<String>) -> Self {
        let mut w = runs.waiters.lock().unwrap();
        for b in &boxes {
            *w.entry(b.clone()).or_default() += 1;
        }
        drop(w);
        Waiting { runs, boxes }
    }
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        let mut w = self.runs.waiters.lock().unwrap();
        for b in &self.boxes {
            if let Some(n) = w.get_mut(b) {
                *n -= 1;
                if *n == 0 {
                    w.remove(b);
                }
            }
        }
        drop(w);
        // A reader left: mail it did not take may need a notification now.
        self.runs.wake.notify_one();
    }
}

/// Marks an attempt as being stopped while it lives.
struct Stopping<'a>(&'a OrchRuns, String);

impl<'a> Stopping<'a> {
    fn new(runs: &'a OrchRuns, id: &str) -> Self {
        runs.stopping.lock().unwrap().insert(id.to_string());
        Stopping(runs, id.to_string())
    }
}

impl Drop for Stopping<'_> {
    fn drop(&mut self) {
        self.0.stopping.lock().unwrap().remove(&self.1);
    }
}

pub(super) fn oe(e: OrchError) -> RpcError {
    use run_codes::*;
    let code = match e.kind {
        ErrorKind::InvalidParams => codes::INVALID_PARAMS,
        ErrorKind::InvalidState => orch_codes::INVALID_STATE,
        ErrorKind::RunNotFound => RUN_NOT_FOUND,
        ErrorKind::ItemNotFound => ITEM_NOT_FOUND,
        ErrorKind::AttemptNotFound => ATTEMPT_NOT_FOUND,
        ErrorKind::MailNotFound => MAIL_NOT_FOUND,
        ErrorKind::AttemptFenced => ATTEMPT_FENCED,
        ErrorKind::DepthExceeded => DEPTH_EXCEEDED,
        ErrorKind::CircuitOpen => CIRCUIT_OPEN,
        ErrorKind::DepsNotDone => DEPS_NOT_DONE,
        ErrorKind::WorkerLimit => WORKER_LIMIT,
        ErrorKind::Io => codes::INTERNAL_ERROR,
    };
    RpcError::new(code, e.message)
}

/// Who is calling.
#[derive(Debug, Clone)]
struct Who {
    /// Sender address: `attempt:<id>`, `run:<id>` or `human`.
    from: String,
    attempt: Option<String>,
    terminal: Option<String>,
    /// The run the caller coordinates (newest), if any.
    coordinates: Option<String>,
    /// The run the caller works in (workers).
    works_in: Option<String>,
}

fn newest(mut runs: Vec<String>) -> Option<String> {
    runs.sort_by_key(|r| r.trim_start_matches('r').parse::<u64>().unwrap_or(0));
    runs.pop()
}

/// Resolve the caller. A worker's token is always checked.
fn who(o: &Orch, c: &OrchCaller) -> Result<Who, RpcError> {
    if let Some(a) = &c.attempt {
        let at = o.verify_token(a, c.token.as_deref()).map_err(oe)?;
        let terminal = c.terminal.clone().or_else(|| at.terminal.clone());
        let mut coord: Vec<String> = o
            .runs()
            .filter(|r| r.rec.run.coordinator_attempt.as_deref() == Some(a.as_str()))
            .map(|r| r.rec.run.id.clone())
            .collect();
        if let Some(t) = &terminal {
            coord.extend(o.runs_of_terminal(t));
        }
        return Ok(Who {
            from: attempt_mailbox(a),
            attempt: Some(a.clone()),
            terminal,
            coordinates: newest(coord),
            works_in: Some(at.run.clone()),
        });
    }
    if let Some(t) = &c.terminal {
        let coordinates = newest(o.runs_of_terminal(t));
        return Ok(Who {
            from: coordinates
                .as_deref()
                .map(run_mailbox)
                .unwrap_or_else(|| "human".into()),
            attempt: None,
            terminal: Some(t.clone()),
            coordinates,
            works_in: None,
        });
    }
    Ok(Who {
        from: "human".into(),
        attempt: None,
        terminal: None,
        coordinates: None,
        works_in: None,
    })
}

impl Who {
    /// The mailboxes this caller reads.
    fn mailboxes(&self, o: &Orch, run: Option<&str>) -> Vec<String> {
        if let Some(r) = run {
            return vec![run_mailbox(r)];
        }
        let mut out = Vec::new();
        if let Some(a) = &self.attempt {
            out.push(attempt_mailbox(a));
        }
        if let Some(t) = &self.terminal {
            for mb in o.terminal_mailboxes(t) {
                if !out.contains(&mb) {
                    out.push(mb);
                }
            }
        }
        out
    }
}

fn cached<T: DeserializeOwned>(
    o: &Orch,
    req: Option<&str>,
    method: &str,
) -> Result<Option<T>, RpcError> {
    let Some(id) = req else { return Ok(None) };
    match o.request(id, method).map_err(oe)? {
        Some(v) => serde_json::from_value(v)
            .map(Some)
            .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("stored result: {e}"))),
        None => Ok(None),
    }
}

fn remember<T: Serialize>(
    o: &mut Orch,
    req: Option<&str>,
    method: &str,
    result: &T,
) -> Result<(), RpcError> {
    if let Some(id) = req {
        let v = serde_json::to_value(result)
            .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, e.to_string()))?;
        o.remember(id, method, v, crate::now_ms()).map_err(oe)?;
    }
    Ok(())
}

fn public(mut a: Attempt) -> Attempt {
    a.token_hash.clear();
    a
}

fn run_info(o: &Orch, run: &str, unconfirmed: &Unconfirmed) -> Result<RunInfo, RpcError> {
    let st = o.run(run).map_err(oe)?;
    Ok(RunInfo {
        wake_unconfirmed_ms: unconfirmed.since(o, &run_mailbox(run)),
        run: st.rec.run.clone(),
        items: st.rec.items.clone(),
        attempts: st.rec.attempts.iter().map(|a| a.public()).collect(),
        unread: o.unread(&run_mailbox(run)).len(),
        mail: st.mail.len(),
    })
}

/// 256 random bits as hex (two 128-bit system RNG draws).
fn new_attempt_token() -> Result<String, RpcError> {
    let a = status::new_token()
        .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("token: {e}")))?;
    let b = status::new_token()
        .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("token: {e}")))?;
    Ok(format!("{a}{b}"))
}

fn agent_label(a: &AgentSpec) -> &'static str {
    match a {
        AgentSpec::Kind(AgentKind::Claude) => "claude",
        AgentSpec::Kind(AgentKind::Codex) => "codex",
        AgentSpec::Kind(AgentKind::Shell) => "shell",
        AgentSpec::Argv { .. } => "argv",
    }
}

/// The brief file: Pod's worker header, then the spec (docs/19 §3.3).
pub fn brief_text(run: &Run, item: &Item, attempt: &Attempt) -> String {
    format!(
        "## Pod worker\n\n\
You are a **worker** started by Pod. Run `{run}`, item `{item}`, attempt `{attempt}`.\n\
Run objective: {objective}\n\n\
Rules:\n\
- Report completion exactly once: `pod orch done --outcome succeeded|failed --body \"what you did / what you found / what is left\"`.\n\
- Ask your coordinator with `pod orch ask --question \"...\"` instead of a local question prompt; it waits for the answer.\n\
- Do not poll. When mail arrives Pod types `pod mail: ...` into this terminal; then run `pod orch check`.\n\
- Do not start workers of your own unless this brief asks for it.\n\
- Never report a failure as a success.\n\n\
### {title}\n\n{spec}\n",
        run = run.id,
        item = item.id,
        attempt = attempt.id,
        objective = run.objective,
        title = item.title,
        spec = item.spec.trim_end(),
    )
}

impl Daemon {
    fn orch(&self) -> std::sync::MutexGuard<'_, Orch> {
        self.runs.engine.lock().unwrap()
    }

    /// The timings and defaults in effect (a copy; never held across an await).
    fn orch_config(&self) -> OrchConfig {
        self.runs
            .config
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn orch_release_ms(&self) -> Option<u64> {
        release_ms(&self.runs.config.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn orch_config_set(&self, s: OrchSettings) -> Result<OrchSettings, RpcError> {
        let mut c = self.runs.config.lock().unwrap_or_else(|e| e.into_inner());
        let out = c
            .set(&self.runs.state_dir, &s)
            .map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
        // The per-run worker cap belongs to `orch.limits`: keep the engine's.
        let mut o = self.orch();
        o.limits = pod_orch::engine::Limits {
            run_worker_cap: o.limits.run_worker_cap,
            ..c.limits
        };
        Ok(out)
    }

    /// `orch.mail.list`: the run's whole thread for the app (nothing acked).
    fn orch_mail_list(&self, p: OrchMailListParams) -> Result<OrchMailListResult, RpcError> {
        let messages = self.orch().mail_of(&p.run).map_err(oe)?;
        Ok(OrchMailListResult { messages })
    }

    /// For mod events: an attempt's state word and its task.
    pub(super) fn orch_attempt_facts(
        &self,
        attempt: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        let Some(id) = attempt else {
            return (None, None);
        };
        let o = self.orch();
        let Ok(a) = o.attempt(id) else {
            return (None, None);
        };
        let state = serde_json::to_value(a.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string));
        (state, a.task.clone())
    }

    fn terminal_by_id(&self, id: &str) -> Option<Arc<Terminal>> {
        self.terminals
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    fn task_of_terminal(&self, terminal: &str) -> Option<super::tasks::TaskRecord> {
        let reg = self.registry.lock().unwrap();
        reg.tasks
            .iter()
            .find(|t| t.terminals.iter().any(|x| x == terminal))
            .cloned()
    }

    fn worker_info(&self, o: &Orch, a: &Attempt) -> WorkerInfo {
        let t = a.terminal.as_deref().and_then(|id| self.terminal_by_id(id));
        let status = t.as_ref().map(|t| t.status.info());
        let waiting = o.has_open_question(&a.id);
        let unread = o.unread(&attempt_mailbox(&a.id)).len();
        use pod_orch::text as tx;
        let next = match (a.state, a.worker) {
            (_, WorkerState::Released) => MsgText::new(tx::NEXT_RELEASED),
            (s, _) if s.is_open() && waiting => MsgText::new(tx::NEXT_WAITING_ANSWER),
            (pod_orch::AttemptState::Starting, _) if a.is_queued() => {
                let q = a.queue.as_ref().unwrap();
                MsgText::new(tx::NEXT_QUEUED)
                    .with("position", q.position)
                    .with("reason", &q.reason)
            }
            (pod_orch::AttemptState::Starting, _) => MsgText::new(tx::NEXT_STARTING),
            (s, _)
                if s.is_open()
                    && !a.waiting_on.is_empty()
                    && status.as_ref().is_none_or(|s| s.state != "working") =>
            {
                MsgText::new(tx::NEXT_WAITING_ITEMS).with("items", a.waiting_on.join(", "))
            }
            (s, _) if s.is_open() => match status.as_ref().map(|s| s.state.as_str()) {
                Some("blocked") => MsgText::new(tx::NEXT_BLOCKED),
                Some("working") => MsgText::new(tx::NEXT_WORKING),
                _ => MsgText::new(tx::NEXT_IDLE),
            },
            (_, WorkerState::Kept) => MsgText::new(tx::NEXT_KEPT),
            (_, WorkerState::Finished) => match a.release_at_ms {
                Some(ms) => MsgText::new(tx::NEXT_RELEASE_IN)
                    .with("secs", ms.saturating_sub(crate::now_ms()) / 1000),
                None => MsgText::new(tx::NEXT_FINISHED),
            },
            _ => MsgText::new(tx::NEXT_FINISHED),
        };
        WorkerInfo {
            attempt: a.public(),
            item_title: o.item(&a.item).map(|i| i.title.clone()).unwrap_or_default(),
            status,
            last_output_ms: t.as_ref().map(|t| t.last_output_ms()).unwrap_or(0),
            waiting_answer: waiting,
            unread,
            next: next.en(),
            next_text: Some(next),
            wake_unconfirmed_ms: self.runs.unconfirmed.since(o, &attempt_mailbox(&a.id)),
        }
    }

    /// Record a message and announce it (event + waiters + supervisor).
    fn post(&self, o: &mut Orch, ctx_run: Option<&str>, m: NewMail) -> Result<Mail, RpcError> {
        let idle = |a: &Attempt| {
            a.terminal
                .as_deref()
                .and_then(|t| self.terminal_by_id(t))
                .is_some_and(|t| {
                    matches!(t.status.info().state.as_str(), "idle" | "done" | "unknown")
                })
        };
        let mail = o.send(ctx_run, m, idle, crate::now_ms()).map_err(oe)?;
        self.announce_mail(&mail);
        Ok(mail)
    }

    fn announce_mail(&self, mail: &Mail) {
        status::publish_orch_mail(mail);
        status::publish_orch_changed(&mail.run, None, None);
        self.runs.mail.notify_waiters();
        self.runs.wake.notify_one();
    }

    // ── runs / items ─────────────────────────────────────────────────

    fn orch_run_create(&self, p: OrchRunCreateParams) -> Result<RunInfo, RpcError> {
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchRunCreate::NAME)? {
            return Ok(r);
        }
        let w = who(&o, &p.caller)?;
        let run = o
            .create_run(
                &p.objective,
                w.terminal.as_deref(),
                w.attempt.as_deref(),
                crate::now_ms(),
            )
            .map_err(oe)?;
        let info = run_info(&o, &run.id, &self.runs.unconfirmed)?;
        remember(&mut o, p.request_id.as_deref(), OrchRunCreate::NAME, &info)?;
        status::publish_orch_changed(&run.id, None, None);
        log(&format!("orch: run {} by {}", run.id, w.from));
        Ok(info)
    }

    fn caller_run(
        &self,
        o: &Orch,
        run: Option<String>,
        caller: &OrchCaller,
    ) -> Result<String, RpcError> {
        if let Some(r) = run {
            return Ok(r);
        }
        let w = who(o, caller)?;
        w.coordinates.or(w.works_in).ok_or_else(|| {
            RpcError::new(
                codes::INVALID_PARAMS,
                "no run: pass --run, or call from a terminal that coordinates or works in one",
            )
        })
    }

    fn orch_run_list(&self, p: OrchRunListParams) -> Result<OrchRunListResult, RpcError> {
        let o = self.orch();
        let mine: Option<Vec<String>> = if p.mine {
            let w = who(&o, &p.caller)?;
            let mut v: Vec<String> = w
                .terminal
                .as_deref()
                .map(|t| o.runs_of_terminal(t))
                .unwrap_or_default();
            v.extend(w.works_in);
            Some(v)
        } else {
            None
        };
        let ids: Vec<String> = o
            .runs()
            .map(|r| r.rec.run.id.clone())
            .filter(|r| mine.as_ref().is_none_or(|m| m.contains(r)))
            .collect();
        Ok(OrchRunListResult {
            runs: ids
                .iter()
                .map(|r| run_info(&o, r, &self.runs.unconfirmed))
                .collect::<Result<_, _>>()?,
        })
    }

    fn orch_run_show(&self, p: OrchRunRef) -> Result<RunInfo, RpcError> {
        let o = self.orch();
        let run = self.caller_run(&o, p.run, &p.caller)?;
        run_info(&o, &run, &self.runs.unconfirmed)
    }

    fn orch_item_create(&self, p: OrchItemCreateParams) -> Result<Item, RpcError> {
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchItemCreate::NAME)? {
            return Ok(r);
        }
        let run = self.caller_run(&o, p.run, &p.caller)?;
        let it = o
            .add_item(
                &run,
                p.title.as_deref(),
                &p.spec,
                &p.deps,
                p.parent.as_deref(),
                crate::now_ms(),
            )
            .map_err(oe)?;
        remember(&mut o, p.request_id.as_deref(), OrchItemCreate::NAME, &it)?;
        status::publish_orch_changed(&run, Some(&it.id), None);
        Ok(it)
    }

    fn orch_item_list(&self, p: OrchItemListParams) -> Result<OrchItemListResult, RpcError> {
        let o = self.orch();
        let run = self.caller_run(&o, p.run, &p.caller)?;
        let st = o.run(&run).map_err(oe)?;
        Ok(OrchItemListResult {
            items: st
                .rec
                .items
                .iter()
                .filter(|i| !p.ready || (i.state == ItemState::Ready && !i.circuit_open))
                .cloned()
                .collect(),
        })
    }

    fn orch_item_update(&self, p: OrchItemUpdateParams) -> Result<Item, RpcError> {
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchItemUpdate::NAME)? {
            return Ok(r);
        }
        who(&o, &p.caller)?;
        let it = if p.reopen {
            o.reopen_item(&p.item, crate::now_ms()).map_err(oe)?
        } else {
            o.item(&p.item).map_err(oe)?.clone()
        };
        remember(&mut o, p.request_id.as_deref(), OrchItemUpdate::NAME, &it)?;
        status::publish_orch_changed(&it.run, Some(&it.id), None);
        Ok(it)
    }

    // ── workers ──────────────────────────────────────────────────────

    async fn orch_worker_start(
        &self,
        mut p: OrchWorkerStartParams,
    ) -> Result<OrchWorkerStartResult, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let req = p.request_id.clone();
        if let Some(r) = cached(&self.orch(), req.as_deref(), OrchWorkerStart::NAME)? {
            return Ok(r);
        }
        let agent = p.agent.clone().unwrap_or_default();
        // The default place in effect now (`orch.config`), also for a queued start.
        if p.place.is_none() {
            p.place = Some(self.orch_config().default_place);
        }
        let place = p.place.clone().unwrap_or_else(|| "new".into());
        // Records first (limits are checked here), then the queue: every
        // start goes through it, so FIFO order holds for everyone.
        let (run, item, attempt) = {
            let mut o = self.orch();
            let w = who(&o, &p.caller)?;
            let now = crate::now_ms();
            let run = match (&p.run, &w.coordinates) {
                (Some(r), _) => {
                    o.run(r).map_err(oe)?;
                    r.clone()
                }
                (None, Some(r)) => r.clone(),
                (None, None) => {
                    // Refuse before creating anything: this run's workers
                    // would be one level below the caller.
                    let depth = match &w.attempt {
                        Some(a) => o.attempt(a).map_err(oe)?.depth,
                        None => 0,
                    } + 1;
                    if depth > o.limits.max_depth {
                        return Err(oe(OrchError::new(
                            ErrorKind::DepthExceeded,
                            format!(
                                "nesting depth {depth} is over the limit {}: do this work yourself instead of starting a worker",
                                o.limits.max_depth
                            ),
                        )));
                    }
                    let objective = p
                        .objective
                        .clone()
                        .or(p.title.clone())
                        .or(p.spec.as_deref().map(pod_orch::spec_title))
                        .unwrap_or_else(|| "work".into());
                    o.create_run(&objective, w.terminal.as_deref(), w.attempt.as_deref(), now)
                        .map_err(oe)?
                        .id
                }
            };
            let item = match (&p.item, &p.spec) {
                (Some(i), None) => o.item(i).map_err(oe)?.clone(),
                (None, Some(spec)) => o
                    .add_item(&run, p.title.as_deref(), spec, &p.deps, None, now)
                    .map_err(oe)?,
                _ => {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        "give exactly one of item or spec",
                    ));
                }
            };
            if item.run != run {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    format!("item {} belongs to run {}, not {run}", item.id, item.run),
                ));
            }
            let attempt = o
                .begin_attempt(
                    BeginAttempt {
                        run: run.clone(),
                        item: item.id.clone(),
                        agent: agent_label(&agent).into(),
                        place: place.clone(),
                        retry_of: p.retry_of.clone(),
                        // Made when it leaves the queue.
                        token_hash: String::new(),
                    },
                    now,
                )
                .map_err(oe)?;
            let mut params = p.clone();
            params.caller = OrchCaller::default();
            params.request_id = None;
            let launch = QueuedLaunch {
                params,
                terminal: w.terminal.clone(),
            };
            let launch = serde_json::to_value(&launch)
                .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, e.to_string()))?;
            // Only a new worktree takes disk space.
            let disk = (place == "new")
                .then(|| self.worker_disk_path(p.repo.as_deref(), w.terminal.as_deref()));
            o.enqueue(&attempt.id, launch, disk, now).map_err(oe)?;
            (run, item, attempt)
        };
        status::publish_orch_changed(&run, Some(&item.id), Some(&attempt.id));
        if let Some(old) = &p.retry_of {
            status::publish_orch_changed(&run, Some(&item.id), Some(old));
        }
        let started = self.orch_admit(Some(&attempt.id)).await;
        let result = match started {
            Some(r) if r.attempt.id == attempt.id => r,
            _ => {
                let a = self.orch().attempt(&attempt.id).map_err(oe)?.clone();
                let q = a.queue.clone();
                OrchWorkerStartResult {
                    run: run.clone(),
                    item: item.id.clone(),
                    state: a.state,
                    attempt: public(a),
                    task: None,
                    terminal: None,
                    stage: "queued_for_resources".into(),
                    next: format!(
                        "Queued for resources (#{}): {} Pod starts queued workers in order when resources free up and mails you if one cannot start. Do not poll; end your turn if you have nothing else to do.",
                        q.as_ref().map_or(0, |q| q.position),
                        q.as_ref().map_or("", |q| q.reason.as_str())
                    ),
                    error: None,
                }
            }
        };
        remember(
            &mut self.orch(),
            req.as_deref(),
            OrchWorkerStart::NAME,
            &result,
        )?;
        status::publish_orch_changed(&run, Some(&item.id), Some(&attempt.id));
        self.runs.wake.notify_one();
        Ok(result)
    }

    /// The drive a new worker's worktree goes to: the repository's
    /// worktrees root (its own setting, else the daemon's).
    fn worker_disk_path(&self, repo: Option<&str>, caller_terminal: Option<&str>) -> String {
        let repo = repo.map(str::to_string).or_else(|| {
            caller_terminal
                .and_then(|t| self.task_of_terminal(t))
                .map(|t| t.repo)
        });
        let rec = repo.and_then(|r| self.repo_record(&r).ok());
        match rec {
            Some(r) => super::sessions::worktrees_root(self, &r),
            None => self.config.worktrees_root.clone(),
        }
        .display()
        .to_string()
    }

    /// One pass of the resource gate (the caller holds `orch_ops`): measure,
    /// store the waiting verdicts, start at most one queued worker. Returns
    /// that worker's start result. `requested` is the attempt of a running
    /// `orch.worker.start` (its caller gets the result; others get mail if
    /// their start fails).
    async fn orch_admit(&self, requested: Option<&str>) -> Option<OrchWorkerStartResult> {
        let sample = self.runs.meter.sample();
        if self.orch().queue().is_empty() {
            return None;
        }
        let token = match new_attempt_token() {
            Ok(t) => t,
            Err(e) => {
                log(&format!("orch: queue: {}", e.message));
                return None;
            }
        };
        let now = crate::now_ms();
        let admitted = {
            let meter = &self.runs.meter;
            let mut disk: HashMap<String, Option<u64>> = HashMap::new();
            let free_disk = |path: &str| {
                *disk
                    .entry(path.to_string())
                    .or_insert_with(|| meter.free_disk(std::path::Path::new(path)))
            };
            self.orch().admit(
                &self.runs.resources(),
                &sample,
                free_disk,
                Some(hash_token(&token)),
                now,
            )
        };
        let admitted = match admitted {
            Ok(a) => a,
            Err(e) => {
                log(&format!("orch: queue: {}", e.message));
                return None;
            }
        };
        for a in &admitted.changed {
            status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
            let Some(q) = &a.queue else { continue };
            if q.kind == pod_orch::WaitKind::Disk {
                // docs/19 §5.1: low disk waits and tells the coordinator.
                let m = NewMail::pod(
                    "pod",
                    run_mailbox(&a.run),
                    MailType::Escalation,
                    MsgText::new(pod_orch::text::LOW_DISK)
                        .with("attempt", &a.id)
                        .with("item", &a.item),
                )
                .body(q.reason.clone())
                .payload(json!({"stop_reason": "low_disk", "attempt": a.id, "item": a.item}));
                let mut o = self.orch();
                if let Err(e) = self.post(&mut o, None, m) {
                    log(&format!("orch: low disk mail for {}: {}", a.id, e.message));
                }
            }
        }
        let (a, launch) = admitted.start?;
        log(&format!("orch: {} leaves the resource queue", a.id));
        let report = requested != Some(a.id.as_str());
        Some(self.launch_admitted(a, launch, token, report).await)
    }

    /// Start a worker that left the queue; record the outcome.
    async fn launch_admitted(
        &self,
        attempt: Attempt,
        launch: Value,
        token: String,
        report: bool,
    ) -> OrchWorkerStartResult {
        let run = attempt.run.clone();
        let item_id = attempt.item.clone();
        let fail_result =
            |a: Attempt, stage: String, error: String, next: String| OrchWorkerStartResult {
                run: run.clone(),
                item: item_id.clone(),
                state: a.state,
                attempt: public(a),
                task: None,
                terminal: None,
                stage,
                next,
                error: Some(error),
            };
        let records = {
            let o = self.orch();
            match (o.run(&attempt.run), o.item(&attempt.item)) {
                (Ok(r), Ok(i)) => Ok((r.rec.run.clone(), i.clone())),
                (Err(e), _) | (_, Err(e)) => Err(e.message),
            }
        };
        let parsed = serde_json::from_value::<QueuedLaunch>(launch)
            .map_err(|e| format!("queued start data: {e}"));
        let launched = match (records, parsed) {
            (Ok((run_rec, item)), Ok(ql)) => {
                let agent = ql.params.agent.clone().unwrap_or_default();
                let place = ql.params.place.clone().unwrap_or_else(|| "new".into());
                self.launch_worker(
                    &ql.params,
                    ql.terminal.as_deref(),
                    &run_rec,
                    &item,
                    &attempt,
                    &agent,
                    &place,
                    &token,
                )
                .await
                .map(|ok| (ok, ql.params.keep, item))
            }
            (Err(e), _) | (_, Err(e)) => Err(Box::new((
                "queue".to_string(),
                RpcError::new(codes::INTERNAL_ERROR, e),
            ))),
        };
        let result = match launched {
            Ok(((task, terminal, brief), keep, item)) => {
                let turn = self
                    .terminal_by_id(&terminal)
                    .map(|t| t.status.info().turn)
                    .unwrap_or(0);
                let mut o = self.orch();
                let attached = o
                    .attach_worker(&attempt.id, task.as_deref(), &terminal, Some(&brief), turn)
                    .and_then(|a| {
                        if keep {
                            o.set_worker(&attempt.id, WorkerState::Kept, None, crate::now_ms())
                        } else {
                            Ok(a)
                        }
                    });
                match attached {
                    Ok(a) => {
                        log(&format!(
                            "orch: worker {} for {} in {} ({})",
                            a.id,
                            item.id,
                            task.as_deref().unwrap_or("-"),
                            terminal
                        ));
                        OrchWorkerStartResult {
                            run: run.clone(),
                            item: item.id.clone(),
                            state: a.state,
                            attempt: public(a),
                            task,
                            terminal: Some(terminal),
                            stage: "started".into(),
                            next: "Do not poll: Pod types `pod mail: ...` into your terminal when the worker asks or reports; then run `pod orch check`. End your turn if you have nothing else to do.".into(),
                            error: None,
                        }
                    }
                    Err(e) => fail_result(
                        attempt.clone(),
                        "record".into(),
                        e.message.clone(),
                        e.message,
                    ),
                }
            }
            Err(err) => {
                let (stage, e) = *err;
                let mut o = self.orch();
                let tripped = o
                    .start_failed(&attempt.id, &e.message, crate::now_ms())
                    .unwrap_or(false);
                let a = o.attempt(&attempt.id).cloned().unwrap_or(attempt.clone());
                log(&format!(
                    "orch: worker {} failed to start at {stage}: {}",
                    a.id, e.message
                ));
                let next_text = if tripped {
                    MsgText::new(pod_orch::text::START_FAILED_TRIPPED)
                        .with("strikes", o.limits.start_strikes)
                } else {
                    MsgText::new(pod_orch::text::START_FAILED_RETRY).with("error", &e.message)
                }
                .with("item", &a.item)
                .with("attempt", &a.id);
                let next = next_text.en();
                if report {
                    // Nobody waits on this start: tell the coordinator.
                    let subject = MsgText::new(pod_orch::text::START_FAILED)
                        .with("attempt", &a.id)
                        .with("item", &a.item);
                    let m = NewMail::pod(attempt_mailbox(&a.id), run_mailbox(&a.run), MailType::Escalation, subject)
                        .body_text(next_text)
                        .payload(json!({"stop_reason": "start_failed", "attempt": a.id, "item": a.item, "stage": stage, "error": e.message}));
                    if let Err(e) = self.post(&mut o, None, m) {
                        log(&format!(
                            "orch: start failure mail for {}: {}",
                            a.id, e.message
                        ));
                    }
                }
                fail_result(a, stage, e.message, next)
            }
        };
        status::publish_orch_changed(&run, Some(&item_id), Some(&attempt.id));
        self.runs.wake.notify_one();
        result
    }

    /// Brief + task/terminal. Errors carry the failed stage.
    #[allow(clippy::too_many_arguments)]
    async fn launch_worker(
        &self,
        p: &OrchWorkerStartParams,
        caller_terminal: Option<&str>,
        run: &Run,
        item: &Item,
        attempt: &Attempt,
        agent: &AgentSpec,
        place: &str,
        token: &str,
    ) -> Result<(Option<String>, String, String), Box<(String, RpcError)>> {
        let fail = |stage: &str| {
            let stage = stage.to_string();
            move |e: RpcError| Box::new((stage.clone(), e))
        };
        let mut env = BTreeMap::new();
        env.insert(ENV_RUN_ID.to_string(), run.id.clone());
        env.insert(ENV_ITEM_ID.to_string(), item.id.clone());
        env.insert(ENV_ATTEMPT_ID.to_string(), attempt.id.clone());
        env.insert(ENV_ATTEMPT_TOKEN.to_string(), token.to_string());
        let caller_task = caller_terminal.and_then(|t| self.task_of_terminal(t));
        // docs/19 §18.5: the worker's own git (and its agent program's, run
        // outside the agent's sandbox) runs no repository filter, hook or
        // fsmonitor. Config is read from where the worker's git will run.
        let git_dir = match place {
            "new" => p
                .repo
                .clone()
                .or(caller_task.as_ref().map(|t| t.repo.clone()))
                .and_then(|r| self.repo_path(&r)),
            "current" => caller_task
                .as_ref()
                .map(|t| PathBuf::from(&t.worktree))
                .or_else(|| {
                    caller_terminal
                        .and_then(|t| self.terminal_by_id(t))
                        .map(|t| PathBuf::from(&t.cwd))
                }),
            task => self
                .task_record(task)
                .ok()
                .map(|t| PathBuf::from(t.worktree)),
        };
        if let Some(dir) = git_dir {
            env.extend(self.worker_git_guard(dir).await);
        }
        match place {
            "new" => {
                // The S1 task-create path: brief file (`POD_BRIEF`) and its safe
                // first line (claude/codex argument, typed for argv agents),
                // the worker env, an open hint (tab without focus).
                let repo = p
                    .repo
                    .clone()
                    .or(caller_task.as_ref().map(|t| t.repo.clone()))
                    .ok_or_else(|| {
                        fail("task")(RpcError::new(
                            codes::INVALID_PARAMS,
                            "no repository: pass --repo, or start workers from a task terminal",
                        ))
                    })?;
                let name = p
                    .name
                    .clone()
                    .unwrap_or_else(|| format!("{}-{}", run.id, attempt.id));
                let info = self
                    .task_create(TaskCreateParams {
                        repo,
                        name,
                        base: p.base.clone(),
                        agent: Some(agent.clone()),
                        parent_terminal: caller_terminal.map(str::to_string),
                        cols: p.cols,
                        rows: p.rows,
                        brief: Some(brief_text(run, item, attempt)),
                        open_hint: Some(OpenHint {
                            focus: false,
                            reason: OpenReason::Worker,
                        }),
                        env,
                        ..Default::default()
                    })
                    .await
                    .map_err(fail("task"))?;
                let term = info
                    .agent_terminal
                    .clone()
                    .or_else(|| info.terminals.first().map(|t| t.id.clone()))
                    .ok_or_else(|| {
                        fail("terminal")(RpcError::new(
                            codes::INTERNAL_ERROR,
                            "the task has no terminal",
                        ))
                    })?;
                let brief = info
                    .brief
                    .as_ref()
                    .map(|b| b.path.clone())
                    .unwrap_or_default();
                Ok((Some(info.id), term, brief))
            }
            _ => {
                // No new task: Pod's own brief file next to the run.
                let dir = self.runs.engine.lock().unwrap().root().join(&run.id);
                let brief = dir.join(format!("{}.brief.md", attempt.id));
                let text = format!(
                    "<!-- pod brief_version {BRIEF_VERSION} -->\n{}",
                    brief_text(run, item, attempt)
                );
                std::fs::create_dir_all(&dir)
                    .and_then(|_| std::fs::write(&brief, text))
                    .map_err(|e| {
                        fail("brief")(RpcError::new(codes::INTERNAL_ERROR, format!("brief: {e}")))
                    })?;
                let brief_s = brief.display().to_string();
                let mut env = env;
                env.insert(ENV_BRIEF.to_string(), brief_s.clone());
                let prompt = matches!(agent, AgentSpec::Kind(AgentKind::Claude | AgentKind::Codex))
                    .then(|| BRIEF_PROMPT.to_string());
                // `current`: the caller's worktree; otherwise an existing task.
                let (task, cwd) = if place == "current" {
                    match &caller_task {
                        Some(t) => (Some(t.id.clone()), None),
                        None => {
                            let cwd = caller_terminal
                                .and_then(|t| self.terminal_by_id(t))
                                .map(|t| t.cwd.clone())
                                .ok_or_else(|| {
                                    fail("terminal")(RpcError::new(
                                        codes::INVALID_PARAMS,
                                        "--place current needs a caller terminal",
                                    ))
                                })?;
                            (None, Some(cwd))
                        }
                    }
                } else {
                    let t = self.task_record(place).map_err(fail("task"))?;
                    (Some(t.id), None)
                };
                let mut argv =
                    super::tasks::agent_argv(agent, prompt.as_deref()).map_err(fail("terminal"))?;
                // In a Pod task's worktree a Codex worker gets that worktree's
                // trust and the narrow allowance, like a new task's worker.
                if let Some(rec) = task.as_deref().and_then(|t| self.task_record(t).ok()) {
                    let wt = PathBuf::from(&rec.worktree);
                    let (a, w, id) = (agent.clone(), wt.clone(), rec.id.clone());
                    let repo = self
                        .registry
                        .lock()
                        .unwrap()
                        .repos
                        .iter()
                        .find(|r| r.name == rec.repo)
                        .cloned();
                    super::tasks::blocking(move || {
                        // The setting comes from the repository root, not this
                        // worktree: an agent here can edit its own copy.
                        let on = repo
                            .map(|r| {
                                super::repo_config::effective(
                                    &super::repo_config::load(std::path::Path::new(&r.path)),
                                    &r,
                                )
                                .codex_allowance
                            })
                            .unwrap_or(true);
                        super::codex_allowance::for_worker(&id, &a, &w, on)
                    })
                    .await
                    .map_err(fail("terminal"))?;
                    argv = super::agent_launch::task_agent_argv(agent, argv, &wt);
                }
                let info = self
                    .run_in_task(RunParams {
                        argv,
                        cwd,
                        name: None,
                        env,
                        cols: p.cols,
                        rows: p.rows,
                        task: task.clone(),
                        parent_terminal: caller_terminal.map(str::to_string),
                    })
                    .await
                    .map_err(fail("terminal"))?;
                Ok((task, info.id, brief_s))
            }
        }
    }

    /// A registered repository's path, by name or path.
    fn repo_path(&self, repo: &str) -> Option<PathBuf> {
        let reg = self.registry.lock().unwrap();
        let want = crate::git::normalize(std::path::Path::new(repo));
        reg.repos
            .iter()
            .find(|r| r.name == repo)
            .or_else(|| {
                reg.repos
                    .iter()
                    .find(|r| crate::git::same_path(std::path::Path::new(&r.path), &want))
            })
            .map(|r| PathBuf::from(&r.path))
    }

    /// docs/19 §18.5: environment overrides that keep a worker's git (and
    /// git its agent program runs) from starting repository programs:
    /// [`crate::env::worker_git_guard`] with the filter drivers configured
    /// for `dir`, appended to the user's own command-line git config.
    async fn worker_git_guard(&self, dir: PathBuf) -> BTreeMap<String, String> {
        let base = self.terminal_env("t0", BTreeMap::new());
        let no_hooks = self.config.state_dir.join("orch").join("no-hooks");
        let nh = no_hooks.clone();
        let read = super::tasks::blocking(move || {
            std::fs::create_dir_all(&nh).map_err(|e| format!("{}: {e}", nh.display()))?;
            crate::git::output(
                &dir,
                &[
                    "config",
                    "-z",
                    "--get-regexp",
                    r"^filter\..*\.(clean|smudge|process)$",
                ],
            )
        })
        .await;
        let drivers = match read {
            Ok(Ok(o)) => crate::env::parse_filter_drivers(&o.stdout),
            Ok(Err(e)) => {
                log(&format!("orch: worker git guard: {e}"));
                Vec::new()
            }
            Err(e) => {
                log(&format!("orch: worker git guard: {}", e.message));
                Vec::new()
            }
        };
        crate::env::worker_git_guard(&base, &drivers, &no_hooks.display().to_string())
    }

    fn orch_worker_list(&self, p: OrchRunRef) -> Result<OrchWorkerListResult, RpcError> {
        let o = self.orch();
        let runs: Vec<String> = match p.run {
            Some(r) => vec![r],
            None if p.caller.is_empty() => o.runs().map(|r| r.rec.run.id.clone()).collect(),
            None => vec![self.caller_run(&o, None, &p.caller)?],
        };
        let mut workers = Vec::new();
        for r in runs {
            let st = o.run(&r).map_err(oe)?;
            for a in &st.rec.attempts {
                workers.push(self.worker_info(&o, a));
            }
        }
        Ok(OrchWorkerListResult { workers })
    }

    fn orch_worker_show(&self, p: OrchAttemptRef) -> Result<WorkerInfo, RpcError> {
        let o = self.orch();
        let a = o.attempt(&p.attempt).map_err(oe)?;
        Ok(self.worker_info(&o, a))
    }

    fn orch_worker_read(&self, p: OrchWorkerReadParams) -> Result<OrchWorkerReadResult, RpcError> {
        let a = self.orch().attempt(&p.attempt).map_err(oe)?.clone();
        let n = p.lines.unwrap_or(200);
        let tail = |mut v: Vec<String>| {
            let skip = v.len().saturating_sub(n);
            v.drain(..skip);
            v
        };
        if let Some(t) = a.terminal.as_deref().and_then(|t| self.terminal_by_id(t)) {
            let (lines, _, suggestion) = t.read_lines_for_agents(ReadMode::History);
            return Ok(OrchWorkerReadResult {
                attempt: a.id,
                source: "terminal".into(),
                lines: tail(lines),
                suggestion,
            });
        }
        if let Some(path) = &a.log_path {
            let text = std::fs::read_to_string(path)
                .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("{path}: {e}")))?;
            return Ok(OrchWorkerReadResult {
                attempt: a.id,
                source: "archive".into(),
                lines: tail(text.lines().map(str::to_string).collect()),
                suggestion: None,
            });
        }
        Err(RpcError::new(
            orch_codes::INVALID_STATE,
            format!("worker {} has no terminal and no archived output", a.id),
        ))
    }

    async fn orch_worker_stop(&self, p: OrchAttemptRef) -> Result<WorkerInfo, RpcError> {
        let _ops = self.runs.ops.lock().await;
        if let Some(r) = cached(&self.orch(), p.request_id.as_deref(), OrchWorkerStop::NAME)? {
            return Ok(r);
        }
        let (a, w) = {
            let o = self.orch();
            (
                o.attempt(&p.attempt).map_err(oe)?.clone(),
                who(&o, &p.caller)?,
            )
        };
        if !a.state.is_open() {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!(
                    "attempt {} already ended ({:?}); use release to close its terminal",
                    a.id, a.state
                ),
            ));
        }
        // Stopped only once the process tree is proven gone.
        let _stopping = Stopping::new(&self.runs, &a.id);
        let gone = match a.terminal.as_deref() {
            Some(tid) => match self.terminal_by_id(tid) {
                Some(t) => {
                    self.kill_and_wait(&t).await;
                    !t.is_running()
                }
                // orch-quality-02: an unreachable holder may still run it.
                None => !self.holder_retrying(tid),
            },
            None => true,
        };
        let (state, why) = if gone {
            (
                pod_orch::AttemptState::Stopped,
                format!("stopped by {}", w.from),
            )
        } else {
            (
                pod_orch::AttemptState::Unknown,
                format!("stop by {} not confirmed: the process still runs", w.from),
            )
        };
        let info = {
            let mut o = self.orch();
            o.end_attempt(
                &a.id,
                state,
                &why,
                None,
                self.orch_release_ms(),
                crate::now_ms(),
            )
            .map_err(oe)?;
            let info = self.worker_info(&o, o.attempt(&a.id).map_err(oe)?);
            remember(&mut o, p.request_id.as_deref(), OrchWorkerStop::NAME, &info)?;
            info
        };
        log(&format!("orch: {} {why}", a.id));
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        Ok(info)
    }

    async fn orch_worker_simple(
        &self,
        method: &'static str,
        p: OrchAttemptRef,
    ) -> Result<WorkerInfo, RpcError> {
        let _ops = self.runs.ops.lock().await;
        if let Some(r) = cached(&self.orch(), p.request_id.as_deref(), method)? {
            return Ok(r);
        }
        let (a, w) = {
            let o = self.orch();
            (
                o.attempt(&p.attempt).map_err(oe)?.clone(),
                who(&o, &p.caller)?,
            )
        };
        let now = crate::now_ms();
        if method == OrchWorkerAbandon::NAME {
            let mut o = self.orch();
            if !o
                .end_attempt(
                    &a.id,
                    pod_orch::AttemptState::Abandoned,
                    &format!("abandoned by {}", w.from),
                    Some(WorkerState::Kept),
                    None,
                    now,
                )
                .map_err(oe)?
            {
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!("attempt {} already ended", a.id),
                ));
            }
        } else if method == OrchWorkerKeep::NAME {
            if a.worker == WorkerState::Released {
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!("worker {} was already released", a.id),
                ));
            }
            self.orch()
                .set_worker(&a.id, WorkerState::Kept, None, now)
                .map_err(oe)?;
        } else {
            // release
            if a.worker == WorkerState::Released {
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!("worker {} was already released", a.id),
                ));
            }
            if a.state.is_open() {
                self.orch()
                    .end_attempt(
                        &a.id,
                        pod_orch::AttemptState::Abandoned,
                        &format!("released by {}", w.from),
                        None,
                        None,
                        now,
                    )
                    .map_err(oe)?;
            }
            self.release_worker(&a.id).await?;
        }
        let mut o = self.orch();
        let info = self.worker_info(&o, o.attempt(&a.id).map_err(oe)?);
        remember(&mut o, p.request_id.as_deref(), method, &info)?;
        log(&format!("orch: {method} {} by {}", a.id, w.from));
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        Ok(info)
    }

    /// Archive the last output, close the terminal (its Job Object ends the
    /// process tree), keep the task and worktree.
    async fn release_worker(&self, attempt: &str) -> Result<Attempt, RpcError> {
        let a = self.orch().attempt(attempt).map_err(oe)?.clone();
        let mut log_path = None;
        if let Some(t) = a.terminal.as_deref().and_then(|t| self.terminal_by_id(t)) {
            let (lines, _) = t.read_lines(ReadMode::History);
            let mut text = lines.join("\n");
            if text.len() > ARCHIVE_BYTES {
                let mut cut = text.len() - ARCHIVE_BYTES;
                while !text.is_char_boundary(cut) {
                    cut += 1;
                }
                text.drain(..cut);
            }
            let path = self
                .runs
                .engine
                .lock()
                .unwrap()
                .root()
                .join(&a.run)
                .join(format!("{}.log", a.id));
            match std::fs::write(&path, text) {
                Ok(()) => log_path = Some(path.display().to_string()),
                Err(e) => log(&format!("orch: cannot archive {}: {e}", a.id)),
            }
            self.rm(RmParams {
                target: TerminalRef::id(t.id.clone()),
                force: true,
            })
            .await?;
        }
        let a = self
            .orch()
            .set_worker(
                attempt,
                WorkerState::Released,
                log_path.as_deref(),
                crate::now_ms(),
            )
            .map_err(oe)?;
        log(&format!("orch: released worker {attempt}"));
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        Ok(a)
    }

    // ── mail ─────────────────────────────────────────────────────────

    async fn orch_mail_send(&self, p: OrchMailSendParams) -> Result<Mail, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchMailSend::NAME)? {
            return Ok(r);
        }
        let w = who(&o, &p.caller)?;
        if p.kind == MailType::Done {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "use orch.done (pod orch done) to report completion",
            ));
        }
        let to = match (&p.to, &w.works_in) {
            (Some(t), _) => t.clone(),
            (None, Some(r)) => run_mailbox(r),
            (None, None) => return Err(RpcError::new(codes::INVALID_PARAMS, "--to is required")),
        };
        let ctx = p
            .run
            .clone()
            .or(w.coordinates.clone())
            .or(w.works_in.clone());
        let mut m =
            NewMail::new(w.from.clone(), to, p.kind, p.subject.clone()).body(p.body.clone());
        m.reply_to = p.reply_to.clone();
        m.payload = p.payload.clone();
        m.priority = p.priority;
        let mail = self.post(&mut o, ctx.as_deref(), m)?;
        remember(&mut o, p.request_id.as_deref(), OrchMailSend::NAME, &mail)?;
        Ok(mail)
    }

    async fn orch_mail_reply(&self, p: OrchMailReplyParams) -> Result<Mail, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchMailReply::NAME)? {
            return Ok(r);
        }
        let w = who(&o, &p.caller)?;
        let q = o.find_mail(&p.id).cloned().ok_or_else(|| {
            RpcError::new(run_codes::MAIL_NOT_FOUND, format!("no message {}", p.id))
        })?;
        if !q.from.starts_with("run:") && !q.from.starts_with("attempt:") {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!(
                    "message {} came from {}, which has no mailbox",
                    q.id, q.from
                ),
            ));
        }
        // A person may answer for the coordinator.
        let from = if w.from == "human" && q.recipients.iter().any(|r| r.starts_with("run:")) {
            "human".to_string()
        } else {
            w.from.clone()
        };
        let kind = if q.kind == MailType::Question {
            MailType::Answer
        } else {
            MailType::Status
        };
        let subject = if q.subject.starts_with("re: ") {
            q.subject.clone()
        } else {
            format!("re: {}", q.subject)
        };
        let mut m = NewMail::new(from, q.from.clone(), kind, subject).body(p.body.clone());
        m.reply_to = Some(q.id.clone());
        let mail = self.post(&mut o, Some(&q.run), m)?;
        remember(&mut o, p.request_id.as_deref(), OrchMailReply::NAME, &mail)?;
        Ok(mail)
    }

    async fn orch_mail_check(
        &self,
        p: OrchMailCheckParams,
    ) -> Result<OrchMailCheckResult, RpcError> {
        let boxes = {
            let o = self.orch();
            let w = who(&o, &p.caller)?;
            let b = w.mailboxes(&o, p.run.as_deref());
            if b.is_empty() {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    "no mailbox: call from a coordinator or worker terminal, or pass --run",
                ));
            }
            b
        };
        let timeout = p.timeout_ms.unwrap_or(ASK_DEFAULT_MS).min(ASK_MAX_MS);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
        let _waiting = p.wait.then(|| Waiting::new(&self.runs, boxes.clone()));
        loop {
            let notified = self.runs.mail.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (msgs, settled) = {
                let mut o = self.orch();
                let open: Vec<&String> =
                    boxes.iter().filter(|b| o.wake_unconfirmed(b) > 0).collect();
                let msgs = o.check(&boxes, p.ack, crate::now_ms()).map_err(oe)?;
                // orch-02: a check settles an unconfirmed wake-up; the app shows those.
                let settled: Vec<String> = open
                    .into_iter()
                    .filter(|b| o.wake_unconfirmed(b) == 0)
                    .filter_map(|b| o.mailbox_run(b))
                    .collect();
                (msgs, settled)
            };
            for run in settled {
                status::publish_orch_changed(&run, None, None);
            }
            if !msgs.is_empty() || !p.wait {
                let next = if msgs.is_empty() {
                    "No mail. Do not poll: end your turn; Pod wakes you when mail arrives."
                        .to_string()
                } else {
                    "Answer questions with `pod orch reply --id <id> --body ...`; workers report with `pod orch done`. Do not poll; Pod wakes you for new mail.".to_string()
                };
                return Ok(OrchMailCheckResult {
                    mailboxes: boxes,
                    messages: msgs,
                    timed_out: false,
                    next,
                });
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(OrchMailCheckResult {
                    mailboxes: boxes,
                    messages: Vec::new(),
                    timed_out: true,
                    next: "No mail yet. End your turn; Pod wakes you when mail arrives.".into(),
                });
            }
        }
    }

    async fn orch_mail_ask(&self, p: OrchMailAskParams) -> Result<OrchMailAskResult, RpcError> {
        const M: &str = OrchMailAsk::NAME;
        let (question, mailbox) = {
            let _ops = self.runs.ops.lock().await;
            let mut o = self.orch();
            let w = who(&o, &p.caller)?;
            let Some(attempt) = w.attempt.clone() else {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    "ask is for workers (POD_ATTEMPT_ID); coordinators use `pod orch send`",
                ));
            };
            let mailbox = attempt_mailbox(&attempt);
            let stored: Option<Value> = cached(&o, p.request_id.as_deref(), M)?;
            let q = match stored.as_ref().and_then(|v| v["question"].as_str()) {
                Some(id) => o.find_mail(id).cloned().ok_or_else(|| {
                    RpcError::new(run_codes::MAIL_NOT_FOUND, format!("no message {id}"))
                })?,
                None => {
                    if p.question.trim().is_empty() {
                        return Err(RpcError::new(
                            codes::INVALID_PARAMS,
                            "the question must not be empty",
                        ));
                    }
                    let subject = p
                        .subject
                        .clone()
                        .unwrap_or_else(|| pod_orch::spec_title(&p.question));
                    let to = run_mailbox(w.works_in.as_deref().unwrap_or_default());
                    let m = NewMail::new(w.from.clone(), to, MailType::Question, subject)
                        .body(p.question.clone());
                    let q = self.post(&mut o, None, m)?;
                    remember(
                        &mut o,
                        p.request_id.as_deref(),
                        M,
                        &json!({"question": q.id}),
                    )?;
                    q
                }
            };
            (q, mailbox)
        };
        let timeout = p.timeout_ms.unwrap_or(ASK_DEFAULT_MS).min(ASK_MAX_MS);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout);
        let _waiting = Waiting::new(&self.runs, vec![mailbox.clone()]);
        loop {
            let notified = self.runs.mail.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let answer = {
                let mut o = self.orch();
                match o.answer_to(&question.id) {
                    Some(mut a) => {
                        let now = crate::now_ms();
                        o.ack(&mailbox, std::slice::from_ref(&a.id), now)
                            .map_err(oe)?;
                        a.read_ms = Some(now);
                        Some(a)
                    }
                    None => None,
                }
            };
            if let Some(a) = answer {
                return Ok(OrchMailAskResult {
                    question,
                    answer: Some(a),
                    timed_out: false,
                    next: "Continue the task with this answer. Report with `pod orch done` when finished.".into(),
                });
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(OrchMailAskResult {
                    question,
                    answer: None,
                    timed_out: true,
                    next: "No answer yet. End your turn now; Pod wakes you (`pod mail: ...`) when the answer arrives, then run `pod orch check`.".into(),
                });
            }
        }
    }

    async fn orch_done(&self, p: OrchDoneParams) -> Result<OrchDoneResult, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let mut o = self.orch();
        if let Some(r) = cached(&o, p.request_id.as_deref(), OrchDone::NAME)? {
            return Ok(r);
        }
        let Some(attempt) = p.caller.attempt.clone() else {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "done is for workers: POD_ATTEMPT_ID and POD_ATTEMPT_TOKEN are not set",
            ));
        };
        let d = o
            .done(
                &attempt,
                p.caller.token.as_deref(),
                p.outcome == DoneOutcome::Succeeded,
                &p.body,
                &p.files,
                p.report.as_deref(),
                self.orch_release_ms(),
                crate::now_ms(),
            )
            .map_err(oe)?;
        if d.repeated {
            // orch-01: the same report again (the first answer or its
            // request record was lost); the mail exists, wake anyway.
            status::publish_orch_changed(&d.mail.run, None, None);
            self.runs.mail.notify_waiters();
            self.runs.wake.notify_one();
        } else {
            self.announce_mail(&d.mail);
        }
        let next = if d.accepted {
            "Reported. Your work here is done: end your turn and stay idle (do not exit); your coordinator may send follow-up mail.".to_string()
        } else {
            "This attempt was replaced by a newer one; your report was recorded but changes nothing. Stop working on it.".to_string()
        };
        let res = OrchDoneResult {
            attempt: public(d.attempt),
            accepted: d.accepted,
            mail: d.mail,
            next,
        };
        remember(&mut o, p.request_id.as_deref(), OrchDone::NAME, &res)?;
        drop(o);
        log(&format!(
            "orch: done {attempt} ({:?}, accepted {})",
            p.outcome, res.accepted
        ));
        status::publish_orch_changed(&res.attempt.run, Some(&res.attempt.item), Some(&attempt));
        Ok(res)
    }

    /// `orch.wait` (docs/19 §18.2): the calling worker waits for other items
    /// of its run. Everything finished already: nothing is recorded.
    async fn orch_wait(&self, p: OrchWaitParams) -> Result<OrchWaitResult, RpcError> {
        let _ops = self.runs.ops.lock().await;
        let mut o = self.orch();
        let Some(attempt) = p.caller.attempt.clone() else {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "wait is for workers (POD_ATTEMPT_ID is not set); a coordinator orders work with item deps",
            ));
        };
        let a = o
            .verify_token(&attempt, p.caller.token.as_deref())
            .map_err(oe)?
            .clone();
        let states = o.wait_for_items(&attempt, &p.items).map_err(oe)?;
        drop(o);
        let waiting_on: Vec<String> = states
            .iter()
            .filter(|(_, s)| !s.is_finished())
            .map(|(i, _)| i.clone())
            .collect();
        let next = if waiting_on.is_empty() {
            "Every item you named has finished already: go on (`pod orch item ls` shows their state).".to_string()
        } else {
            format!(
                "Waiting for {}. End your turn now; that is not a missed done. Pod types `pod mail: ...` into your terminal when they have all finished; then run `pod orch check`.",
                waiting_on.join(", ")
            )
        };
        log(&format!(
            "orch: {attempt} waits for {}",
            if waiting_on.is_empty() {
                "nothing (all finished)".into()
            } else {
                waiting_on.join(", ")
            }
        ));
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&attempt));
        Ok(OrchWaitResult {
            items: states
                .into_iter()
                .map(|(item, state)| OrchWaitItem { item, state })
                .collect(),
            waiting_on,
            next,
        })
    }

    /// `orch.commit` (docs/19 §18.1): stage and commit in the calling
    /// worker's own task worktree. podd runs git itself (plumbing only;
    /// hooks, fsmonitor and repository filter programs never run, §18.5),
    /// so a sandboxed worker can commit without being allowed an
    /// unsandboxed `git`, which would read a config it wrote.
    async fn orch_commit(&self, p: OrchCommitParams) -> Result<OrchCommitResult, RpcError> {
        let Some(attempt) = p.caller.attempt.clone() else {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "commit is for workers (POD_ATTEMPT_ID is not set); use git",
            ));
        };
        let a = self
            .orch()
            .verify_token(&attempt, p.caller.token.as_deref())
            .map_err(oe)?
            .clone();
        if !a.state.is_open() {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!("worker {attempt} has ended"),
            ));
        }
        if p.message.trim().is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "the commit message must not be empty (-m)",
            ));
        }
        if !p.all && p.paths.is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "say what to commit: --all or paths",
            ));
        }
        let Some(task) = a.task.clone() else {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!(
                    "worker {attempt} has no task worktree of its own; commit with git (with approval)"
                ),
            ));
        };
        let wt = PathBuf::from(self.task_record(&task)?.worktree);
        let no_hooks = self.config.state_dir.join("orch").join("no-hooks");
        let (msg, all, paths) = (p.message.clone(), p.all, p.paths.clone());
        let r = super::tasks::blocking(move || {
            std::fs::create_dir_all(&no_hooks)
                .map_err(|e| format!("{}: {e}", no_hooks.display()))?;
            crate::git::commit_for_worker(&wt, &no_hooks, &msg, all, &paths)
        })
        .await?;
        let (commit, summary) = r.map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
        log(&format!("orch: {attempt} committed {commit} in {task}"));
        Ok(OrchCommitResult {
            attempt,
            task,
            next: format!(
                "Committed {} on your branch. Report with `pod orch done` when the work is finished.",
                &commit[..commit.len().min(12)]
            ),
            commit,
            summary,
        })
    }

    /// Workers whose awaited items all finished get a `status` mail (which
    /// wakes them like any mail).
    fn orch_settle_waits(&self) {
        let mut o = self.orch();
        let due = match o.settle_waits() {
            Ok(d) => d,
            Err(e) => {
                log(&format!("orch: settle waits: {}", e.message));
                return;
            }
        };
        for (a, states) in due {
            let items: Vec<String> = states
                .iter()
                .map(|(i, s)| format!("{i} {}", item_state_en(*s)))
                .collect();
            let text = MsgText::new(pod_orch::text::WAIT_FINISHED)
                .with("attempt", &a.id)
                .with("item", &a.item)
                .with("items", items.join(", "));
            let payload: Vec<Value> = states
                .iter()
                .map(|(i, s)| json!({"item": i, "state": s}))
                .collect();
            let m = NewMail::pod("pod", attempt_mailbox(&a.id), MailType::Status, text)
                .payload(json!({"waited": payload}));
            match self.post(&mut o, None, m) {
                Ok(_) => log(&format!(
                    "orch: {} stops waiting ({})",
                    a.id,
                    items.join(", ")
                )),
                Err(e) => log(&format!("orch: wait mail for {}: {}", a.id, e.message)),
            }
            status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        }
    }

    // ── people and terminals ─────────────────────────────────────────

    /// A person typed into `terminal` (attach input): its workers become kept.
    pub(super) fn orch_on_human_input(&self, terminal: &str) {
        let mut o = self.orch();
        let ids: Vec<(String, String, String)> = o
            .attempts()
            .filter(|a| {
                a.terminal.as_deref() == Some(terminal)
                    && matches!(a.worker, WorkerState::Active | WorkerState::Finished)
            })
            .map(|a| (a.id.clone(), a.run.clone(), a.item.clone()))
            .collect();
        for (id, run, item) in ids {
            if o.set_worker(&id, WorkerState::Kept, None, crate::now_ms())
                .is_ok()
            {
                log(&format!(
                    "orch: a person typed into {terminal}: worker {id} is kept"
                ));
                status::publish_orch_changed(&run, Some(&item), Some(&id));
            }
        }
    }
}

// ───────────────────────────── dispatch ─────────────────────────────

pub(super) fn handles(method: &str) -> bool {
    method.starts_with("orch.")
}

pub(super) async fn dispatch(d: Arc<Daemon>, method: &str, p: Value) -> Result<Value, RpcError> {
    match method {
        OrchRunCreate::NAME => {
            let p: OrchRunCreateParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_run_create(p)?)
        }
        OrchRunList::NAME => to_value(d.orch_run_list(params(p)?)?),
        OrchRunShow::NAME => to_value(d.orch_run_show(params(p)?)?),
        OrchItemCreate::NAME => {
            let p: OrchItemCreateParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_item_create(p)?)
        }
        OrchItemList::NAME => to_value(d.orch_item_list(params(p)?)?),
        OrchItemUpdate::NAME => {
            let p: OrchItemUpdateParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_item_update(p)?)
        }
        OrchWorkerStart::NAME => to_value(d.orch_worker_start(params(p)?).await?),
        OrchWorkerList::NAME => to_value(d.orch_worker_list(params(p)?)?),
        OrchWorkerShow::NAME => to_value(d.orch_worker_show(params(p)?)?),
        OrchWorkerRead::NAME => to_value(d.orch_worker_read(params(p)?)?),
        OrchWorkerStop::NAME => to_value(d.orch_worker_stop(params(p)?).await?),
        OrchWorkerAbandon::NAME | OrchWorkerKeep::NAME | OrchWorkerRelease::NAME => {
            let m = match method {
                OrchWorkerAbandon::NAME => OrchWorkerAbandon::NAME,
                OrchWorkerKeep::NAME => OrchWorkerKeep::NAME,
                _ => OrchWorkerRelease::NAME,
            };
            to_value(d.orch_worker_simple(m, params(p)?).await?)
        }
        OrchMailSend::NAME => to_value(d.orch_mail_send(params(p)?).await?),
        OrchMailCheck::NAME => to_value(d.orch_mail_check(params(p)?).await?),
        OrchMailReply::NAME => to_value(d.orch_mail_reply(params(p)?).await?),
        OrchMailAsk::NAME => to_value(d.orch_mail_ask(params(p)?).await?),
        OrchDone::NAME => to_value(d.orch_done(params(p)?).await?),
        OrchWait::NAME => to_value(d.orch_wait(params(p)?).await?),
        OrchCommit::NAME => to_value(d.orch_commit(params(p)?).await?),
        OrchMailList::NAME => to_value(d.orch_mail_list(params(p)?)?),
        OrchConfigGet::NAME => to_value(d.orch_config().settings()),
        OrchConfigSet::NAME => {
            let p: OrchSettings = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_config_set(p)?)
        }
        OrchQueue::NAME => to_value(d.orch_queue(params(p)?)?),
        OrchGateRequest::NAME => {
            let p: OrchGateRequestParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_gate_request(p)?)
        }
        OrchGateDecide::NAME => to_value(d.orch_gate_decide(params(p)?)?),
        OrchLimitsGet::NAME => {
            let _: OrchLimitsGetParams = params(p)?;
            to_value(d.orch_limits_result())
        }
        OrchLimitsSet::NAME => {
            let p: OrchLimitsSetParams = params(p)?;
            let _ops = d.runs.ops.lock().await;
            to_value(d.orch_limits_set(p)?)
        }
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method {other:?}"),
        )),
    }
}

// ───────────────────────────── supervisor ─────────────────────────────

/// Called once by `serve` after terminals were re-attached.
pub(super) fn start(daemon: &Arc<Daemon>) {
    daemon.orch_reconcile_after_start();
    let daemon = daemon.clone();
    tokio::spawn(async move {
        let mut rx = status::subscribe();
        loop {
            daemon.orch_supervise().await;
            tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(ev) => daemon.orch_link(ev),
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        tokio::time::sleep(Duration::from_millis(TICK_MS)).await;
                    }
                },
                _ = daemon.runs.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(TICK_MS)) => {}
            }
        }
    });
}

impl Daemon {
    /// Links to the older orchestration features (docs/19 §9, §16.2): review
    /// comments delivered to a worker's task, and a group check over worker
    /// tasks, are mailed (`status`) to the run's coordinator so it sees them
    /// in `pod orch check`. Each delivery / check job is mailed once.
    fn orch_link(&self, ev: status::HubEvent) {
        use pod_orch::text as tx;
        let (key, tasks, text, payload) = match ev {
            status::HubEvent::Delivery(e) if e.delivery.state == DeliveryState::Delivered => {
                let dl = e.delivery;
                let text = MsgText::new(tx::REVIEW_DELIVERED)
                    .with("task", &dl.task)
                    .with("delivery", &dl.id)
                    .with("comments", dl.comment_ids.len());
                let payload = json!({"review": dl.id, "task": dl.task, "comments": dl.comment_ids});
                (format!("review:{}", dl.id), vec![dl.task], text, payload)
            }
            status::HubEvent::GroupCheck(e)
                if e.member.is_none() && e.job.state != CheckJobState::Running =>
            {
                let state = serde_json::to_value(e.job.state)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_string))
                    .unwrap_or_default();
                let mut text = MsgText::new(tx::GROUP_CHECKED)
                    .with("group", &e.group)
                    .with("state", &state);
                if let Some(all) = e.job.all_passed {
                    text = text.with("all_passed", all);
                }
                let payload = json!({"group": e.group, "job": e.job.id, "state": state, "all_passed": e.job.all_passed});
                (
                    format!("check:{}:{}", e.group, e.job.id),
                    e.job.tasks,
                    text,
                    payload,
                )
            }
            _ => return,
        };
        let workers: Vec<Attempt> = {
            let o = self.orch();
            let mut by_run: BTreeMap<String, Attempt> = BTreeMap::new();
            // The newest attempt of each run that worked in one of these tasks.
            for a in o
                .attempts()
                .filter(|a| a.task.as_ref().is_some_and(|t| tasks.contains(t)))
            {
                by_run.insert(a.run.clone(), a.clone());
            }
            by_run.into_values().collect()
        };
        if workers.is_empty() || !self.runs.linked.lock().unwrap().insert(key) {
            return;
        }
        let mut o = self.orch();
        for a in workers {
            let t = text.clone().with("attempt", &a.id).with("item", &a.item);
            let mut p = payload.clone();
            p["attempt"] = json!(a.id);
            let m = NewMail::pod("pod", run_mailbox(&a.run), MailType::Status, t).payload(p);
            if let Err(e) = self.post(&mut o, None, m) {
                log(&format!("orch: link mail for {}: {}", a.id, e.message));
            }
        }
    }

    /// Open attempts whose terminal did not survive (host restart) are lost;
    /// their coordinators get an escalation.
    fn orch_reconcile_after_start(&self) {
        let open: Vec<Attempt> = self
            .orch()
            .attempts()
            .filter(|a| a.state.is_open())
            .cloned()
            .collect();
        for a in open {
            let term = a.terminal.as_deref().and_then(|t| self.terminal_by_id(t));
            if let Some(t) = &term {
                // The status engine starts a new turn count after a restart.
                let turn = t.status.info().turn;
                let _ = self.orch().update_attempt(&a.id, |x| x.seen_turn = turn);
            }
            let alive = term.is_some();
            if let Some(tid) = a.terminal.as_deref().filter(|_| !alive) {
                if self.holder_retrying(tid) {
                    // orch-quality-02: podd still retries its holder; the
                    // supervisor re-binds or loses it later.
                    self.runs
                        .rebind
                        .lock()
                        .unwrap()
                        .insert(tid.to_string(), false);
                    log(&format!(
                        "orch: attempt {} waits for the holder of {tid} (unreachable, retrying)",
                        a.id
                    ));
                } else {
                    self.orch_lose(&a, "host_restart", "host_restart");
                }
            } else if a.terminal.is_none() && !a.is_queued() {
                // Queued workers keep their place (docs/19 §5.1).
                self.orch_lose(&a, "host_restart", "host_restart_starting");
            }
        }
        // Released workers whose terminal is gone anyway, and notifications
        // that were never typed, are handled by the supervisor loop.
        self.orch_reconcile_gates();
    }

    /// orch-quality-02: `terminal` is a holder podd could not reach at start
    /// and still retries (`lifecycle.rs`).
    fn holder_retrying(&self, terminal: &str) -> bool {
        self.unreachable
            .lock()
            .unwrap()
            .iter()
            .any(|u| u.id == terminal && !u.gave_up)
    }

    /// orch-quality-02: an open attempt's terminal is not attached. While
    /// its holder is retried the attempt stays open; otherwise it
    /// is lost: retries used up, the holder gone, or `pod kill` gave up
    /// on it (after a restart), or the terminal was removed.
    fn orch_terminal_missing(&self, a: &Attempt, tid: &str) {
        if self.holder_retrying(tid) {
            return;
        }
        let waited = {
            let mut rb = self.runs.rebind.lock().unwrap();
            match rb.get_mut(tid) {
                // Not seen in either list once: maybe between the two.
                Some(seen @ false) => {
                    *seen = true;
                    return;
                }
                Some(true) => rb.remove(tid).is_some(),
                None => false,
            }
        };
        if waited {
            self.orch_lose(a, "holder_unreachable", "host_restart");
        } else {
            self.orch_lose(a, "terminal_gone", "terminal_gone");
        }
    }

    /// `cause` is a [`pod_orch::text::LOST`] cause; `reason` the stop_reason.
    fn orch_lose(&self, a: &Attempt, reason: &str, cause: &str) {
        let text = MsgText::new(pod_orch::text::LOST)
            .with("attempt", &a.id)
            .with("item", &a.item)
            .with("cause", cause);
        let full = text.en();
        let msg = full.split_once(": ").map(|(_, m)| m).unwrap_or(&full);
        let mut o = self.orch();
        let now = crate::now_ms();
        if !o
            .end_attempt(
                &a.id,
                pod_orch::AttemptState::Lost,
                msg,
                None,
                self.orch_release_ms(),
                now,
            )
            .unwrap_or(false)
        {
            return;
        }
        log(&format!("orch: attempt {} lost ({reason})", a.id));
        let m = NewMail::pod(
            attempt_mailbox(&a.id),
            run_mailbox(&a.run),
            MailType::Escalation,
            text,
        )
        .payload(json!({"stop_reason": reason, "attempt": a.id, "item": a.item}));
        if let Err(e) = self.post(&mut o, None, m) {
            log(&format!("orch: escalation for {}: {}", a.id, e.message));
        }
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
    }

    fn orch_escalate(
        &self,
        a: &Attempt,
        reason: &str,
        subject: MsgText,
        mark: impl FnOnce(&mut Attempt),
    ) {
        let subject = subject.with("attempt", &a.id).with("item", &a.item);
        let mut o = self.orch();
        if o.update_attempt(&a.id, mark).is_err() {
            return;
        }
        log(&format!("orch: {} {reason}", a.id));
        let m = NewMail::pod(
            attempt_mailbox(&a.id),
            run_mailbox(&a.run),
            MailType::Escalation,
            subject,
        )
        .payload(json!({"stop_reason": reason, "attempt": a.id, "item": a.item}));
        if let Err(e) = self.post(&mut o, None, m) {
            log(&format!("orch: escalation for {}: {}", a.id, e.message));
        }
    }

    async fn orch_supervise(self: &Arc<Self>) {
        let now = crate::now_ms();
        let attempts: Vec<Attempt> = self.orch().attempts().cloned().collect();
        let stopping = self.runs.stopping.lock().unwrap().clone();
        for a in attempts
            .iter()
            .filter(|a| a.state.is_open() && !stopping.contains(&a.id))
        {
            let Some(tid) = a.terminal.as_deref() else {
                continue;
            };
            let Some(t) = self.terminal_by_id(tid) else {
                self.orch_terminal_missing(a, tid);
                continue;
            };
            if self.runs.rebind.lock().unwrap().remove(tid).is_some() {
                // orch-quality-02: reached after a restart; its status
                // engine counts turns from here.
                let turn = t.status.info().turn;
                let _ = self.orch().update_attempt(&a.id, |x| x.seen_turn = turn);
                log(&format!("orch: attempt {} re-bound to {tid}", a.id));
                status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
                continue;
            }
            if !t.is_running() {
                self.orch_lose(a, "agent_exited", "agent_exited");
                continue;
            }
            let s = t.status.info();
            let busy = matches!(s.state.as_str(), "working" | "blocked");
            if a.state == pod_orch::AttemptState::Starting
                && (s.state == "working" || s.turn > a.seen_turn)
            {
                let _ = self.orch().mark_ready(&a.id, now);
                status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
            }
            // A turn ended and the worker neither reported nor waits for an answer.
            if s.turn > a.seen_turn && !busy {
                let waiting = self
                    .runs
                    .waiters
                    .lock()
                    .unwrap()
                    .contains_key(&attempt_mailbox(&a.id));
                let open_q = self.orch().has_open_question(&a.id);
                let open_q = open_q || self.orch_waits_on_gate(&a.id);
                // Waiting for other items (`pod orch wait`, §18.2).
                let open_q = open_q || !a.waiting_on.is_empty();
                let turn = s.turn;
                if !waiting && !open_q && a.escalated_turn != Some(turn) {
                    self.orch_escalate(
                        a,
                        "turn_ended_without_done",
                        MsgText::new(pod_orch::text::TURN_ENDED),
                        move |x| {
                            x.seen_turn = turn;
                            x.escalated_turn = Some(turn);
                        },
                    );
                } else {
                    let _ = self
                        .orch()
                        .update_attempt(&a.id, move |x| x.seen_turn = turn);
                }
            }
            // Stalled: working without output for too long (never killed).
            if let Some(stall) = self.orch_config().stall_after {
                let last = t.last_output_ms();
                if s.state == "working"
                    && last > 0
                    && now.saturating_sub(last) >= stall.as_millis() as u64
                    && a.stalled_at_output_ms != Some(last)
                {
                    self.orch_escalate(
                        a,
                        "stalled",
                        MsgText::new(pod_orch::text::STALLED)
                            .with("secs", now.saturating_sub(last) / 1000),
                        move |x| x.stalled_at_output_ms = Some(last),
                    );
                }
            }
        }
        // Finished workers past their release time.
        let due = self.orch().due_releases(now);
        for id in due {
            if let Err(e) = self.release_worker(&id).await {
                log(&format!("orch: release {id}: {}", e.message));
            }
        }
        self.orch_settle_waits();
        // The resource queue: skipped while a start or another mutation runs
        // (that call admits, or the next tick does).
        if let Ok(_ops) = self.runs.ops.try_lock() {
            self.orch_admit(None).await;
        }
        self.orch_wake_ups();
        self.runs.passes.pass_done();
    }

    /// orch-02: settle a typed notification. Confirmed is persisted as
    /// delivered; unconfirmed (or not checkable, orch-quality-04) or not
    /// typed (input refused, holder gone)
    /// wants a wake again: right away when nothing was typed, after a check
    /// or a restart when the line was typed but not taken
    /// ([`pod_orch::engine::MAX_UNCONFIRMED_WAKES`]).
    fn orch_wake_settled(&self, terminal: &str, mark: &[String], typed: WakeTyped) {
        let mut o = self.orch();
        let r = match typed {
            WakeTyped::Taken => o.wake_delivered(mark, crate::now_ms()),
            WakeTyped::Unconfirmed => {
                // Not typed again: the line may still sit in the input box
                // (a second one would be appended). A check or a restart
                // allows the next one.
                log(&format!(
                    "orch: wake-up in {terminal} was not confirmed as submitted; the line may sit in the input box (press Enter there, or run `pod orch check`)"
                ));
                self.runs.unconfirmed.note(mark, crate::now_ms());
                o.wake_failed(mark, true)
            }
            WakeTyped::NotTyped => o.wake_failed(mark, false),
        };
        drop(o);
        if let Err(e) = r {
            log(&format!("orch: wake-up bookkeeping: {}", e.message));
        }
        if let Some(run) = mark.first().and_then(|mb| self.orch().mailbox_run(mb)) {
            status::publish_orch_changed(&run, None, None);
        }
        self.runs.wake.notify_one();
    }

    /// Type one notification line into idle agents with unread mail.
    fn orch_wake_ups(self: &Arc<Self>) {
        // terminal -> its mailboxes that want a wake-up
        let mut by_term: BTreeMap<String, Vec<String>> = BTreeMap::new();
        {
            let o = self.orch();
            let waiters = self.runs.waiters.lock().unwrap();
            let mut boxes: Vec<String> = o.runs().map(|r| run_mailbox(&r.rec.run.id)).collect();
            boxes.extend(o.attempts().map(|a| attempt_mailbox(&a.id)));
            for mb in boxes {
                if waiters.contains_key(&mb) || !o.wants_wake(&mb) {
                    continue;
                }
                if let Some(t) = o.mailbox_terminal(&mb) {
                    by_term.entry(t).or_default().push(mb);
                }
            }
        }
        for (tid, boxes) in by_term {
            let Some(t) = self.terminal_by_id(&tid) else {
                continue;
            };
            if !t.is_running() {
                continue;
            }
            let s = t.status.info();
            let quiet = crate::now_ms().saturating_sub(t.last_output_ms());
            if wake::may_wake(&s.state, t.human_draft(), quiet).is_err() {
                continue;
            }
            let (count, mark) = {
                let mut o = self.orch();
                // Every mailbox of the terminal counts; all are marked.
                let all = o.terminal_mailboxes(&tid);
                let n: usize = all.iter().map(|mb| o.unread_waking(mb)).sum();
                let mut mark = boxes.clone();
                for mb in all {
                    if !mark.contains(&mb)
                        && o.unread_waking(&mb) > 0
                        && o.notified(&mb).is_none()
                        && !o.wake_pending(&mb)
                    {
                        mark.push(mb);
                    }
                }
                // orch-02: scheduled only; marked delivered once the agent
                // took the line (below).
                if let Err(e) = o.wake_scheduled(&mark, crate::now_ms()) {
                    log(&format!("orch: {}", e.message));
                    continue;
                }
                (n, mark)
            };
            let line = wake::notification_line(count);
            log(&format!("orch: wake {tid} ({count} unread)"));
            let t2 = t.clone();
            let d = self.clone();
            self.runs.passes.wake_started();
            tokio::spawn(async move {
                let written = crate::now_ms();
                let chars = line.chars().count();
                // orch-quality-04: a refused line (input backlog, holder
                // gone) is not typed; no Enter, a wake again later.
                t2.remember_sent(&line);
                let body = t2.try_input(line.clone().into_bytes());
                let s = match body {
                    Ok(()) => super::submit::press_enter(&t2, written, chars, true, |_| {})
                        .await
                        .map(|s| s.state),
                    Err(e) => {
                        log(&format!("orch: wake-up for {} not typed: {e:?}", t2.id));
                        None
                    }
                };
                d.orch_wake_settled(&t2.id, &mark, wake_outcome(body, s));
                d.runs.passes.wake_settled();
            });
        }
    }
}

// ───────────────────────────── resource queue, gates ─────────────────────────────

impl Daemon {
    fn orch_limits_result(&self) -> OrchLimitsResult {
        let l = self.runs.limits.lock().unwrap();
        OrchLimitsResult {
            limits: orch_limits::info(&l.effective),
            entries: l.entries(),
            path: l.path().display().to_string(),
        }
    }

    /// `orch.limits.set`: check, save, apply; the queue is judged again.
    fn orch_limits_set(&self, p: OrchLimitsSetParams) -> Result<OrchLimitsResult, RpcError> {
        if p.caller.attempt.is_some() {
            // Verify first so a wrong token reads as such.
            who(&self.orch(), &p.caller)?;
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                "워커는 자원 한도를 바꿀 수 없어요. 코디네이터나 사람에게 부탁하세요.",
            ));
        }
        let effective = {
            let mut l = self.runs.limits.lock().unwrap();
            l.set(&p.set, &p.reset)
                .map_err(|m| RpcError::new(codes::INVALID_PARAMS, m))?;
            l.effective
        };
        self.orch().limits.run_worker_cap = effective.run_workers;
        if !p.set.is_empty() || !p.reset.is_empty() {
            log(&format!("orch limits: set {:?} reset {:?}", p.set, p.reset));
        }
        self.runs.wake.notify_one();
        Ok(self.orch_limits_result())
    }

    /// `orch.queue`: limits, measurements and the waiting workers with a
    /// fresh verdict each (nothing is stored).
    fn orch_queue(&self, p: OrchRunRef) -> Result<OrchQueueResult, RpcError> {
        let limits = self.runs.resources();
        let sample = self.runs.meter.sample();
        let free_disk = self.runs.meter.free_disk(&self.config.worktrees_root);
        let o = self.orch();
        let meter = &self.runs.meter;
        let mut disk: HashMap<String, Option<u64>> = HashMap::new();
        let all: Vec<&Attempt> = o.attempts().collect();
        let plan = pod_orch::resource::plan(
            &all,
            &limits,
            &sample,
            |path| {
                *disk
                    .entry(path.to_string())
                    .or_insert_with(|| meter.free_disk(std::path::Path::new(path)))
            },
            crate::now_ms(),
        );
        let mut queue = Vec::new();
        for a in o.queue() {
            if p.run.as_ref().is_some_and(|r| *r != a.run) {
                continue;
            }
            let mut info = self.worker_info(&o, a);
            if let Some(q) = info.attempt.queue.as_mut() {
                match plan.waiting.iter().find(|v| v.attempt == a.id) {
                    Some(v) => {
                        q.kind = v.kind;
                        q.reason = v.reason.clone();
                        q.position = v.position;
                    }
                    None => {
                        q.kind = pod_orch::WaitKind::Behind;
                        q.reason = "자원이 있어 곧 시작해요.".into();
                    }
                }
                let t = MsgText::new(pod_orch::text::NEXT_QUEUED)
                    .with("position", q.position)
                    .with("reason", &q.reason);
                info.next = t.en();
                info.next_text = Some(t);
            }
            queue.push(info);
        }
        Ok(OrchQueueResult {
            limits: orch_limits::info(&limits),
            cpu_pct: sample.cpu_pct,
            free_mem: sample.free_mem,
            free_disk,
            active: o.active_workers(),
            queue,
            supervisor_passes: self.runs.passes.get().0,
            wakes_in_flight: self.runs.passes.get().1,
        })
    }

    /// The run and item a gate is about (checked against the engine).
    pub(super) fn orch_gate_target(
        &self,
        run: Option<&str>,
        item: Option<&str>,
    ) -> Result<(Option<String>, Option<String>), RpcError> {
        let o = self.orch();
        match (run, item) {
            (_, Some(i)) => {
                let it = o.item(i).map_err(oe)?;
                if let Some(r) = run
                    && r != it.run
                {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        format!("item {i} belongs to run {}, not {r}", it.run),
                    ));
                }
                if matches!(it.state, ItemState::Completed | ItemState::Failed) {
                    return Err(RpcError::new(
                        orch_codes::INVALID_STATE,
                        format!("item {i} is already finished; a gate cannot block it"),
                    ));
                }
                Ok((Some(it.run.clone()), Some(i.to_string())))
            }
            (Some(r), None) => {
                o.run(r).map_err(oe)?;
                Ok((Some(r.to_string()), None))
            }
            (None, None) => Ok((None, None)),
        }
    }

    /// A new gate: block its item; a worker's gate goes to its coordinator.
    pub(super) fn orch_gate_opened(&self, g: &GateInfo) {
        let Some(run) = g.run.as_deref() else { return };
        let mut o = self.orch();
        if let Some(item) = &g.item {
            match o.block_item(item, &g.id, crate::now_ms()) {
                Ok(_) => {
                    status::publish_orch_changed(run, Some(item), None);
                }
                Err(e) => log(&format!(
                    "orch: gate {} cannot block {item}: {}",
                    g.id, e.message
                )),
            }
        }
        if g.requested_by
            .as_deref()
            .is_some_and(|r| r.starts_with("attempt:"))
        {
            let subject = MsgText::new(pod_orch::text::GATE_REQUESTED)
                .with("gate", &g.id)
                .with("title", &g.title);
            let mut body = MsgText::new(pod_orch::text::GATE_REQUESTED_BODY)
                .with("gate", &g.id)
                .with("message", &g.message);
            if !g.options.is_empty() {
                body = body.with("options", g.options.join(" | "));
            }
            let m = NewMail::pod(
                g.requested_by.clone().unwrap_or_default(),
                run_mailbox(run),
                MailType::Gate,
                subject,
            )
            .body_text(body)
            .payload(
                json!({"gate": g.id, "options": g.options, "item": g.item, "state": "pending"}),
            );
            if let Err(e) = self.post(&mut o, None, m) {
                log(&format!("orch: gate {} mail: {}", g.id, e.message));
            }
        }
    }

    /// A gate was decided or expired: unblock its item and mail the
    /// requester (and the coordinator, when someone else decided a worker's
    /// gate). Called after the single decision path committed.
    pub(super) fn orch_gate_settled(&self, g: &GateInfo) {
        let Some(run) = g.run.as_deref() else { return };
        let mut o = self.orch();
        if let Some(item) = &g.item {
            match o.unblock_item(item, &g.id, crate::now_ms()) {
                Ok(Some(_)) => {
                    status::publish_orch_changed(run, Some(item), None);
                }
                Ok(None) => {}
                Err(e) => log(&format!(
                    "orch: gate {} unblock {item}: {}",
                    g.id, e.message
                )),
            }
        }
        let state = match g.state {
            GateState::Approved => "approved",
            GateState::Rejected => "rejected",
            GateState::Expired => "expired",
            GateState::Pending => return,
        };
        let outcome = match (&g.choice, g.state) {
            (Some(_), _) => "decided",
            _ => state,
        };
        let settled = |kind: &str| {
            let mut t = MsgText::new(kind)
                .with("gate", &g.id)
                .with("title", &g.title)
                .with("outcome", outcome);
            if let Some(c) = &g.choice {
                t = t.with("choice", c);
            }
            if let Some(n) = g.note.as_deref().filter(|n| !n.is_empty()) {
                t = t.with("note", n);
            }
            t
        };
        let from = match g.decided_by.as_deref() {
            Some(d) if d.starts_with("run:") => d.to_string(),
            Some(_) => "human".to_string(),
            None => "pod".to_string(),
        };
        let mut to: Vec<String> = g.requested_by.iter().cloned().collect();
        if g.requested_by
            .as_deref()
            .is_some_and(|r| r.starts_with("attempt:"))
            && !from.starts_with("run:")
        {
            to.push(run_mailbox(run));
        }
        for t in to {
            let m = NewMail::pod(from.clone(), t, MailType::Gate, settled(pod_orch::text::GATE_SETTLED))
                .body_text(settled(pod_orch::text::GATE_SETTLED_BODY))
                .payload(json!({"gate": g.id, "state": state, "choice": g.choice, "decided_by": g.decided_by, "item": g.item}));
            if let Err(e) = self.post(&mut o, None, m) {
                log(&format!("orch: gate {} decision mail: {}", g.id, e.message));
            }
        }
    }

    /// After a restart: items still blocked by gates that were decided (or
    /// lost) are freed; pending item gates that the item misses block it.
    fn orch_reconcile_gates(&self) {
        let gates: Vec<GateInfo> = self
            .orch
            .lock()
            .unwrap()
            .gates
            .iter()
            .filter(|g| g.item.is_some())
            .cloned()
            .collect();
        let blocked: Vec<(String, String)> = self
            .orch()
            .runs()
            .flat_map(|r| r.rec.items.iter())
            .flat_map(|i| i.gates.iter().map(|g| (i.id.clone(), g.clone())))
            .collect();
        let now = crate::now_ms();
        for (item, gid) in blocked {
            let pending = gates
                .iter()
                .any(|g| g.id == gid && g.state == GateState::Pending);
            if !pending && let Ok(Some(it)) = self.orch().unblock_item(&item, &gid, now) {
                log(&format!("orch: item {item} freed from settled gate {gid}"));
                status::publish_orch_changed(&it.run, Some(&item), None);
            }
        }
        for g in gates.iter().filter(|g| g.state == GateState::Pending) {
            let item = g.item.as_deref().unwrap_or_default();
            let missing = self
                .orch()
                .item(item)
                .is_ok_and(|i| !i.gates.contains(&g.id));
            if missing {
                let _ = self.orch().block_item(item, &g.id, now);
            }
        }
    }

    /// The worker asked for a decision that is still pending.
    fn orch_waits_on_gate(&self, attempt: &str) -> bool {
        let mb = attempt_mailbox(attempt);
        self.orch.lock().unwrap().gates.iter().any(|g| {
            g.state == GateState::Pending && g.requested_by.as_deref() == Some(mb.as_str())
        })
    }

    /// `orch.gate.request`.
    fn orch_gate_request(&self, p: OrchGateRequestParams) -> Result<GateInfo, RpcError> {
        if let Some(r) = cached(&self.orch(), p.request_id.as_deref(), OrchGateRequest::NAME)? {
            return Ok(r);
        }
        let (w, item, run) = {
            let o = self.orch();
            let w = who(&o, &p.caller)?;
            // A worker's gate is about its own item unless it names another.
            let item = p.item.clone().or_else(|| {
                (p.run.is_none())
                    .then(|| {
                        w.attempt
                            .as_deref()
                            .and_then(|a| o.attempt(a).ok())
                            .map(|a| a.item.clone())
                    })
                    .flatten()
            });
            let run = p.run.clone().or_else(|| {
                if item.is_none() {
                    w.coordinates.clone().or(w.works_in.clone())
                } else {
                    None
                }
            });
            (w, item, run)
        };
        if item.is_none() && run.is_none() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "no run: pass --run or --item, or call from a coordinator or worker terminal",
            ));
        }
        let requested_by = (w.from != "human").then(|| w.from.clone());
        let g = self.gate_create_as(
            GateCreateParams {
                task: None,
                group: None,
                terminal: w.terminal.clone(),
                title: p.title.clone(),
                message: p.message.clone(),
                expires_in_s: p.expires_in_s,
                item,
                run,
                options: p.options.clone(),
            },
            requested_by,
        )?;
        remember(
            &mut self.orch(),
            p.request_id.as_deref(),
            OrchGateRequest::NAME,
            &g,
        )?;
        Ok(g)
    }

    /// `orch.gate.decide`: a person, or the coordinator of the gate's run
    /// (never for a gate it asked itself). Through the single decision path.
    fn orch_gate_decide(&self, p: OrchGateDecideParams) -> Result<GateInfo, RpcError> {
        let g = self.gate_show(GateRef { id: p.id.clone() })?;
        let actor = {
            let o = self.orch();
            let w = who(&o, &p.caller)?;
            if w.from == "human" {
                GateActor::Local
            } else {
                let run = g.run.clone().ok_or_else(|| {
                    RpcError::new(
                        codes::INVALID_PARAMS,
                        format!("gate {} belongs to no run: a person decides it (Gates panel, `pod gate`)", g.id),
                    )
                })?;
                let coordinates = w
                    .terminal
                    .as_deref()
                    .is_some_and(|t| o.runs_of_terminal(t).contains(&run))
                    || o.run(&run).is_ok_and(|r| {
                        r.rec.run.coordinator_attempt.is_some()
                            && r.rec.run.coordinator_attempt == w.attempt
                    });
                if !coordinates {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        format!(
                            "only the coordinator of run {run} or a person can decide gate {}",
                            g.id
                        ),
                    ));
                }
                if g.requested_by.as_deref() == Some(run_mailbox(&run).as_str()) {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        format!(
                            "gate {} was asked by this coordinator: a person decides it",
                            g.id
                        ),
                    ));
                }
                GateActor::Agent {
                    from: run_mailbox(&run),
                }
            }
        };
        let req = Decide {
            id: p.id.clone(),
            decision: if p.reject {
                GateDecision::Reject
            } else {
                GateDecision::Approve
            },
            choice: p.choice.clone(),
            note: p.note.clone(),
            actor,
            operation_id: p.request_id.clone(),
            task_scope: None,
        };
        match self.gate_decide(&req, crate::now_ms())? {
            super::gates::Decided::Now(g) | super::gates::Decided::Replayed(g) => Ok(g),
        }
    }
}

/// orch-quality-04: how a typed wake-up ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WakeTyped {
    /// The agent started a turn for it.
    Taken,
    /// Typed and Enter pressed, but nothing showed the agent took it
    /// (including agents that report no status): not a delivery.
    Unconfirmed,
    /// The line was refused (input backlog, holder gone) or Enter could not
    /// be sent: nothing to take, a wake-up again.
    NotTyped,
}

/// The line's input result and the Enter check's state, as [`WakeTyped`].
/// A refused line never had its Enter pressed.
pub(super) fn wake_outcome(
    body: Result<(), super::terminal::InputRefused>,
    enter: Option<SubmitState>,
) -> WakeTyped {
    match (body, enter) {
        (Err(_), _) | (Ok(()), None) => WakeTyped::NotTyped,
        (Ok(()), Some(SubmitState::Confirmed)) => WakeTyped::Taken,
        (
            Ok(()),
            Some(SubmitState::Unverifiable | SubmitState::Unconfirmed | SubmitState::Pending),
        ) => WakeTyped::Unconfirmed,
    }
}

/// `<state>\orch\runs` (tests and docs).
pub fn runs_dir(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("orch").join("runs")
}

fn item_state_en(s: ItemState) -> &'static str {
    match s {
        ItemState::Pending => "pending",
        ItemState::Ready => "ready",
        ItemState::Assigned => "assigned",
        ItemState::Completed => "completed",
        ItemState::Failed => "failed",
        ItemState::Blocked => "blocked",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brief_has_header_and_spec() {
        let run = Run {
            id: "r1".into(),
            objective: "ship".into(),
            coordinator_terminal: None,
            coordinator_attempt: None,
            depth: 0,
            created_ms: 0,
        };
        let item = Item {
            id: "i1".into(),
            run: "r1".into(),
            title: "Fix it".into(),
            spec: "line one\n\"quoted\" 100%\n".into(),
            deps: vec![],
            parent: None,
            state: ItemState::Assigned,
            generation: 1,
            start_failures: 0,
            circuit_open: false,
            attempt: Some("a1".into()),
            gates: vec![],
            created_ms: 0,
            updated_ms: 0,
        };
        let a: Attempt = serde_json::from_value(json!({
            "id": "a1", "run": "r1", "item": "i1", "generation": 1, "state": "starting",
            "worker": "active", "agent": "codex", "place": "new", "depth": 1, "created_ms": 0
        }))
        .unwrap();
        let b = brief_text(&run, &item, &a);
        assert!(b.starts_with("## Pod worker"));
        assert!(
            b.contains("attempt `a1`") && b.contains("pod orch done") && b.contains("pod orch ask")
        );
        assert!(b.ends_with("### Fix it\n\nline one\n\"quoted\" 100%\n"));
        assert!(!BRIEF_PROMPT.contains(['"', '%', '\r', '\n']));
    }

    /// orch-02 (app part): the time of an unconfirmed wake-up is reported
    /// while the engine counts it open, kept from the first miss, and gone
    /// after a check (which settles it).
    #[test]
    fn unconfirmed_wake_since_follows_the_engine() {
        let dir = std::env::temp_dir().join(format!(
            "podd-unconfirmed-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let mut o = Orch::load(&dir, Default::default(), 1);
        o.create_run("x", Some("t1"), None, 1).unwrap();
        o.send(
            None,
            NewMail::new("human", "run:r1", MailType::Status, "hi"),
            |_| false,
            2,
        )
        .unwrap();
        let rb = vec!["run:r1".to_string()];
        let u = Unconfirmed::default();
        assert_eq!(u.since(&o, "run:r1"), None);
        // Typed but not taken (what `orch_wake_settled` does).
        o.wake_scheduled(&rb, 3).unwrap();
        u.note(&rb, 4);
        o.wake_failed(&rb, true).unwrap();
        assert_eq!(u.since(&o, "run:r1"), Some(4));
        u.note(&rb, 9);
        assert_eq!(u.since(&o, "run:r1"), Some(4), "the first miss counts");
        let info = run_info(&o, "r1", &u).unwrap();
        assert_eq!(info.wake_unconfirmed_ms, Some(4));
        // A check settles it; a later miss starts a new time.
        o.check(&rb, true, 10).unwrap();
        assert_eq!(u.since(&o, "run:r1"), None);
        assert_eq!(run_info(&o, "r1", &u).unwrap().wake_unconfirmed_ms, None);
        u.note(&rb, 20);
        o.wake_failed(&rb, false).unwrap();
        assert_eq!(
            u.since(&o, "run:r1"),
            None,
            "not typed at all is not unconfirmed"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// orch-quality-04: a refused line is not typed (its Enter is never
    /// pressed, so the Enter state is `None`), and only a confirmed Enter
    /// is a delivery; not checkable counts as unconfirmed.
    #[test]
    fn wake_outcome_needs_the_line_and_a_confirmed_enter() {
        use super::super::terminal::InputRefused;
        use SubmitState::*;
        let backlog = Err(InputRefused::Backlog { in_flight: 9 });
        for enter in [None, Some(Confirmed), Some(Unverifiable)] {
            assert_eq!(wake_outcome(backlog, enter), WakeTyped::NotTyped);
            assert_eq!(
                wake_outcome(Err(InputRefused::Gone), enter),
                WakeTyped::NotTyped
            );
        }
        assert_eq!(wake_outcome(Ok(()), None), WakeTyped::NotTyped);
        assert_eq!(wake_outcome(Ok(()), Some(Confirmed)), WakeTyped::Taken);
        for s in [Unverifiable, Unconfirmed, Pending] {
            assert_eq!(wake_outcome(Ok(()), Some(s)), WakeTyped::Unconfirmed);
        }
    }

    #[test]
    fn error_codes_map() {
        let e = oe(OrchError::new(ErrorKind::AttemptFenced, "x"));
        assert_eq!(e.code, run_codes::ATTEMPT_FENCED);
        assert_eq!(
            oe(OrchError::new(ErrorKind::InvalidState, "x")).code,
            orch_codes::INVALID_STATE
        );
        assert_eq!(
            oe(OrchError::new(ErrorKind::DepthExceeded, "x")).code,
            run_codes::DEPTH_EXCEEDED
        );
    }
}
