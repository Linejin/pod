//! Agent status in the daemon: one `pod-status` [`TerminalStatus`] per
//! terminal, fed from the holder stream, hook messages and a ticker
//! (docs/07 §8).
//!
//! Inputs
//! - Output bytes (the same raw bytes attach clients get): OSC 7501 reports,
//!   window titles (OSC 0/2, scanned here because the VT mirror drops them),
//!   and a "screen changed" mark. OSC 7501 *queries* are answered by the
//!   holder (docs/03 §3), never here.
//! - Process facts: the terminal's argv when it is created, its exit.
//! - Ticker (every 250 ms): visible screen text for the screen-pattern
//!   heuristics, only when output arrived since the last sample; engine
//!   `tick` once per second (quiet-time idle, hook staleness).
//! - Hook lines from `pod-hook` on the daemon pipe, authenticated by
//!   `POD_TERMINAL_ID` + the per-terminal `POD_HOOK_TOKEN`.
//!
//! Outputs: `TerminalInfo.status`, `terminal.show`, `terminal.ack`,
//! `status.wait`, `status.subscribe` (`status.changed` events with a
//! daemon-wide sequence number).
//!
//! Lock order: terminal stream state -> [`TermStatus`] -> hub publish lock.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use pod_status::hook::HOOK_MESSAGE_VERSION;
use pod_status::{
    AgentKind, AgentState, BlockedKind, HookMessage, Millis, Outcome, ProcessInfo, Source,
    TerminalStatus,
};
use serde_json::Value;
use tokio::io::BufReader;
use tokio::net::windows::named_pipe::NamedPipeServer;
use tokio::sync::broadcast;

use super::terminal::Terminal;
use super::{Daemon, err_response, log, notification, ok_response, params, to_value, write_json};
use crate::protocol::*;

pub use pod_status::hook::ENV_HOOK_TOKEN;

/// Minimum time between two screen samples of one terminal.
const SCREEN_INTERVAL_MS: Millis = 250;
/// Engine `tick` period (quiet-time idle, hook staleness).
const TICK_INTERVAL_MS: Millis = 1000;
/// Window titles longer than this are ignored.
const TITLE_MAX: usize = 1024;
/// `status.subscribe` buffer; a slower subscriber gets `status.lagged`.
const EVENT_CAPACITY: usize = 4096;

// ───────────────────────────── hub ─────────────────────────────

struct Hub {
    /// Unix ms at `epoch`; engine time is `base_ms + epoch.elapsed()` (monotonic).
    base_ms: u64,
    epoch: Instant,
    /// Last sequence number; held while sending so events go out in order.
    last_seq: Mutex<u64>,
    tx: broadcast::Sender<HubEvent>,
    state_dir: OnceLock<PathBuf>,
}

/// What the hub broadcasts: status changes and (v5) gate changes, in one
/// sequence.
#[derive(Debug, Clone)]
pub(super) enum HubEvent {
    Status(StatusChangedEvent),
    Gate(GateChangedEvent),
    /// v6: background `group.check` progress.
    GroupCheck(GroupCheckEvent),
    /// A review delivery was recorded or changed state (`review.delivery`).
    Delivery(ReviewDeliveryEvent),
    /// v9 lifecycle events (only for `lifecycle` subscribers).
    Terminal(TerminalChangedEvent),
    Task(TaskChangedEvent),
    TaskRemoving(TaskRemovingEvent),
    /// v10 `task.pr.changed` (lifecycle subscribers only).
    TaskPr(TaskPrChangedEvent),
    /// v11 extensions (docs/16): `pod.notify` and mod state changes.
    ModNotify(ModNotifyEvent),
    ModChanged(ModChangedEvent),
    /// v16 orchestration (docs/19 §7): unfiltered subscribers only.
    OrchChanged(OrchChangedEvent),
    OrchMail(OrchMailEvent),
    /// Session discovery (docs/18), lifecycle subscribers only.
    Sessions(SessionsChangedEvent),
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        base_ms: crate::now_ms(),
        epoch: Instant::now(),
        last_seq: Mutex::new(0),
        tx: broadcast::channel(EVENT_CAPACITY).0,
        state_dir: OnceLock::new(),
    })
}

/// Engine clock: monotonic within this daemon, close to unix ms (so `since`
/// can be shown as a wall-clock time).
pub fn now() -> Millis {
    let h = hub();
    h.base_ms + h.epoch.elapsed().as_millis() as u64
}

impl Hub {
    fn publish(&self, id: &str, status: StatusInfo, from: Option<String>, removed: bool) -> u64 {
        let mut seq = self.last_seq.lock().unwrap();
        *seq += 1;
        // No receivers is fine.
        let _ = self.tx.send(HubEvent::Status(StatusChangedEvent {
            seq: *seq,
            id: id.to_string(),
            status,
            from,
            removed,
        }));
        *seq
    }
}

/// Announce a created or resolved gate on every `status.subscribe` stream.
pub(super) fn publish_gate(gate: GateInfo) -> u64 {
    let h = hub();
    let mut seq = h.last_seq.lock().unwrap();
    *seq += 1;
    let _ =
        h.tx.send(HubEvent::Gate(GateChangedEvent { seq: *seq, gate }));
    *seq
}

/// Announce `group.check` job progress on every unfiltered `status.subscribe` stream.
pub(super) fn publish_group_check(group: &str, job: CheckJob, member: Option<MemberCheck>) -> u64 {
    let h = hub();
    let mut seq = h.last_seq.lock().unwrap();
    *seq += 1;
    let _ = h.tx.send(HubEvent::GroupCheck(GroupCheckEvent {
        seq: *seq,
        group: group.to_string(),
        job,
        member,
    }));
    *seq
}

/// Announce a recorded or changed review delivery (`review.delivery`) on
/// every `status.subscribe` stream that wants its terminal.
pub(super) fn publish_delivery(delivery: DeliveryInfo, submit: Option<SubmitCheck>) -> u64 {
    let h = hub();
    let mut seq = h.last_seq.lock().unwrap();
    *seq += 1;
    let _ = h.tx.send(HubEvent::Delivery(ReviewDeliveryEvent {
        seq: *seq,
        delivery,
        submit,
    }));
    *seq
}

/// Publish with the next sequence number (the closure gets it).
fn publish_with(make: impl FnOnce(u64) -> HubEvent) -> u64 {
    let h = hub();
    let mut seq = h.last_seq.lock().unwrap();
    *seq += 1;
    let _ = h.tx.send(make(*seq));
    *seq
}

/// v9 `terminal.changed`.
pub(super) fn publish_terminal(
    id: &str,
    change: TerminalChange,
    terminal: Option<TerminalInfo>,
) -> u64 {
    publish_with(|seq| {
        HubEvent::Terminal(TerminalChangedEvent {
            seq,
            id: id.to_string(),
            change,
            terminal: terminal.map(Box::new),
        })
    })
}

/// v9 `task.changed`; `seq` in `ev` is replaced.
pub(super) fn publish_task(mut ev: TaskChangedEvent) -> u64 {
    publish_with(|seq| {
        ev.seq = seq;
        HubEvent::Task(ev)
    })
}

/// A `task.changed` event of `change` with no payload (fill fields after).
pub(super) fn task_event(id: &str, change: TaskChange) -> TaskChangedEvent {
    TaskChangedEvent {
        seq: 0,
        id: id.to_string(),
        change,
        task: None,
        status: None,
        git: None,
        terminals: Vec::new(),
        paths: Vec::new(),
        open_hint: None,
    }
}

/// v9 `task.removing`.
pub(super) fn publish_task_removing(id: &str, worktree: &str, force: bool, grace_ms: u64) -> u64 {
    publish_with(|seq| {
        HubEvent::TaskRemoving(TaskRemovingEvent {
            seq,
            id: id.to_string(),
            worktree: worktree.to_string(),
            force,
            grace_ms,
        })
    })
}

/// v10 `task.pr.changed`.
pub(super) fn publish_task_pr(id: &str, pr: Option<TaskPr>) -> u64 {
    publish_with(|seq| {
        HubEvent::TaskPr(TaskPrChangedEvent {
            seq,
            id: id.to_string(),
            pr,
        })
    })
}

/// v11 `mod.notify` / `mod.changed`; the `seq` in `ev` is replaced.
/// v13 `orch.changed`.
pub(super) fn publish_orch_changed(run: &str, item: Option<&str>, attempt: Option<&str>) -> u64 {
    publish_with(|seq| {
        HubEvent::OrchChanged(OrchChangedEvent {
            seq,
            run: run.to_string(),
            item: item.map(str::to_string),
            attempt: attempt.map(str::to_string),
        })
    })
}

/// v13 `orch.mail` (subject only).
pub(super) fn publish_orch_mail(m: &Mail) -> u64 {
    publish_with(|seq| {
        HubEvent::OrchMail(OrchMailEvent {
            seq,
            run: m.run.clone(),
            to: m.to.clone(),
            id: m.id.clone(),
            kind: m.kind,
            subject: m.subject.clone(),
            text: m.text.clone(),
            from: m.from.clone(),
        })
    })
}

pub(super) fn publish_mod(ev: HubEvent) -> u64 {
    publish_with(|seq| match ev {
        HubEvent::ModNotify(mut e) => {
            e.seq = seq;
            HubEvent::ModNotify(e)
        }
        HubEvent::ModChanged(mut e) => {
            e.seq = seq;
            HubEvent::ModChanged(e)
        }
        other => other,
    })
}

pub(super) fn publish_sessions(mut ev: SessionsChangedEvent) -> u64 {
    publish_with(|seq| {
        ev.seq = seq;
        HubEvent::Sessions(ev)
    })
}

/// Connected `status.subscribe` streams that asked for lifecycle events.
static LIFECYCLE_SUBSCRIBERS: AtomicUsize = AtomicUsize::new(0);

pub(super) fn lifecycle_subscribers() -> usize {
    LIFECYCLE_SUBSCRIBERS.load(AtomicOrdering::Relaxed)
}

struct LifecycleGuard;
impl LifecycleGuard {
    fn new() -> Self {
        LIFECYCLE_SUBSCRIBERS.fetch_add(1, AtomicOrdering::Relaxed);
        LifecycleGuard
    }
}
impl Drop for LifecycleGuard {
    fn drop(&mut self) {
        LIFECYCLE_SUBSCRIBERS.fetch_sub(1, AtomicOrdering::Relaxed);
    }
}

/// Receive every later hub event (status and gates).
pub(super) fn subscribe() -> broadcast::Receiver<HubEvent> {
    hub().tx.subscribe()
}

/// Remember the state dir (hook tokens, persisted status). Called by `serve`
/// before terminals are re-attached.
pub(super) fn set_state_dir(dir: &Path) {
    let _ = hub().state_dir.set(dir.to_path_buf());
}

/// Called once by `serve`: start the ticker.
pub(super) fn start(daemon: &Arc<Daemon>) {
    set_state_dir(&daemon.config.state_dir);
    let daemon = daemon.clone();
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(SCREEN_INTERVAL_MS));
        iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            iv.tick().await;
            let terms: Vec<Arc<Terminal>> = daemon.terminals.lock().unwrap().clone();
            for t in &terms {
                t.status.sample(&daemon, t);
            }
        }
    });
}

// ───────────────────────────── hook tokens ─────────────────────────────

fn token_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.token"))
}

/// 128 random bits as hex, from the system RNG.
pub(crate) fn new_token() -> std::io::Result<String> {
    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom,
    };
    let mut b = [0u8; 16];
    // SAFETY: valid buffer of the given length; no algorithm handle with the
    // system-preferred-RNG flag.
    let st = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            b.as_mut_ptr(),
            b.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if st != 0 {
        return Err(std::io::Error::other(format!(
            "BCryptGenRandom failed: {st:#x}"
        )));
    }
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Create the terminal's hook token (`<state dir>/<id>.token`, user-only
/// like the rest of the state dir) and add `POD_HOOK_TOKEN` to its env. Set
/// last, so `terminal.run` env overrides cannot replace it.
pub(super) fn hook_env(
    mut env: BTreeMap<String, String>,
    dir: &Path,
    id: &str,
) -> Result<BTreeMap<String, String>, RpcError> {
    let token = new_token()
        .and_then(|t| std::fs::write(token_path(dir, id), &t).map(|_| t))
        .map_err(|e| RpcError::new(codes::SPAWN_FAILED, format!("hook token: {e}")))?;
    crate::env::set_var(&mut env, ENV_HOOK_TOKEN, token);
    Ok(env)
}

fn load_token(id: &str) -> Option<String> {
    let dir = hub().state_dir.get()?;
    let t = std::fs::read_to_string(token_path(dir, id)).ok()?;
    let t = t.trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// A hook for a terminal `terminal.run` is still starting: its token file
/// exists already (written before the holder starts).
fn early_hook_token_ok(msg: &HookMessage) -> bool {
    if !is_terminal_id(&msg.terminal_id) {
        return false;
    }
    match (load_token(&msg.terminal_id), &msg.token) {
        (Some(want), Some(got)) => same_secret(&want, got),
        _ => false,
    }
}

/// Remove the token (and the persisted status) of a terminal that is gone.
pub(super) fn remove_token(dir: &Path, id: &str) {
    let _ = std::fs::remove_file(token_path(dir, id));
    let _ = std::fs::remove_file(persist_path(dir, id));
}

// ─────────────────────── persisted status (restarts) ───────────────────────

/// What survives a daemon restart besides the OSC/title replay in the holder
/// snapshot: the turn counter and the last accepted hook message (replayed
/// into the new engine at its original time, so staleness rules still
/// apply), plus when the user last acknowledged. `<state dir>\<id>.status.json`.
#[derive(Debug, Default, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Persisted {
    #[serde(default)]
    turn: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hook: Option<HookMessage>,
    /// Engine time (≈ unix ms) the hook was accepted.
    #[serde(default)]
    hook_ms: Millis,
    /// Engine time of the last `terminal.ack`.
    #[serde(default)]
    acked_ms: Millis,
    /// Engine time the user answered the persisted (blocked) hook's prompt:
    /// `working` was inferred (docs/12 B2). 0 = not answered.
    #[serde(default, skip_serializing_if = "is_zero")]
    answered_ms: Millis,
}

fn is_zero(v: &Millis) -> bool {
    *v == 0
}

/// Hook payload fields larger than this are not persisted (tool input and
/// output blobs); the engine only needs the small ones.
const PERSIST_FIELD_MAX: usize = 4096;

fn persist_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.status.json"))
}

fn load_persisted(id: &str) -> Persisted {
    hub()
        .state_dir
        .get()
        .and_then(|d| std::fs::read(persist_path(d, id)).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_persisted(id: &str, p: &Persisted) {
    let Some(dir) = hub().state_dir.get() else {
        return;
    };
    let path = persist_path(dir, id);
    let tmp = path.with_extension("json.tmp");
    let r = serde_json::to_vec(p)
        .map_err(std::io::Error::other)
        .and_then(|b| std::fs::write(&tmp, b))
        .and_then(|_| std::fs::rename(&tmp, &path));
    if let Err(e) = r {
        log(&format!("{id}: cannot save status: {e}"));
    }
}

/// The hook message as persisted: no token, no large payload fields.
fn slim_hook(msg: &HookMessage) -> HookMessage {
    let mut m = msg.clone();
    m.token = None;
    if let Value::Object(map) = &mut m.payload {
        map.retain(|_, v| serde_json::to_string(v).is_ok_and(|s| s.len() <= PERSIST_FIELD_MAX));
    }
    m
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

// ───────────────────────────── per terminal ─────────────────────────────

pub struct TermStatus {
    id: String,
    inner: Mutex<Inner>,
}

struct Inner {
    engine: TerminalStatus,
    titles: TitleScanner,
    screen_dirty: bool,
    last_screen: String,
    last_screen_at: Millis,
    last_tick: Millis,
    token: Option<String>,
    hooks_accepted: u64,
    hooks_rejected: u64,
    /// Last published status and its sequence number.
    published: Option<StatusInfo>,
    seq: u64,
    /// Saved to the state dir when it changes.
    persisted: Persisted,
    /// Mod that set the current `Source::Mod` evidence (docs/16 §7.1).
    mod_owner: Option<String>,
    /// Last window title, and whether detectors have not seen it yet.
    last_title: Option<String>,
    title_unsent: bool,
}

impl TermStatus {
    /// A terminal appeared (new, or re-attached after a daemon restart): the
    /// engine starts `unknown`, learns the agent from argv, then replays the
    /// holder snapshot, which ends with the last OSC 7501 reports and window
    /// title (osc_replay.rs), and re-derives the rest from new output, the
    /// screen and hooks. The turn counter, the last hook message and the
    /// last ack come back from `<id>.status.json` (see [`Persisted`]).
    pub fn new(id: &str, argv: &[String], pid: u32, snapshot: &[u8]) -> Self {
        let now = now();
        let mut engine = TerminalStatus::new(now);
        if let Some(first) = argv.first() {
            let name = first
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(first)
                .to_string();
            engine.on_foreground(
                Some(&ProcessInfo {
                    pid: Some(pid),
                    name,
                    argv: argv.to_vec(),
                }),
                now,
            );
        }
        let mut titles = TitleScanner::default();
        let _ = engine.feed_output(snapshot, now);
        if let Some(title) = titles.feed(snapshot).pop() {
            engine.on_title(&title, now);
        }
        let persisted = load_persisted(id);
        if let Some(h) = &persisted.hook {
            engine.on_hook(h, persisted.hook_ms.min(now));
            if persisted.acked_ms > persisted.hook_ms {
                engine.ack(now);
            }
            if persisted.answered_ms >= persisted.hook_ms && persisted.answered_ms > 0 {
                engine.restore_answer(persisted.answered_ms.min(now), now);
            }
        }
        engine.restore_turn(persisted.turn);
        let s = Self {
            id: id.to_string(),
            inner: Mutex::new(Inner {
                engine,
                titles,
                screen_dirty: true, // sample the restored screen once
                last_screen: String::new(),
                last_screen_at: 0,
                last_tick: now,
                token: None,
                hooks_accepted: 0,
                hooks_rejected: 0,
                published: None,
                seq: 0,
                persisted,
                mod_owner: None,
                last_title: None,
                title_unsent: false,
            }),
        };
        s.publish(&mut s.inner.lock().unwrap());
        s
    }

    /// Raw output bytes, in stream order.
    pub fn on_output(&self, data: &[u8]) {
        let now = now();
        let mut g = self.inner.lock().unwrap();
        // `queries` are answered by the holder; reports drive the engine.
        let _ = g.engine.feed_output(data, now);
        for title in g.titles.feed(data) {
            g.engine.on_title(&title, now);
            g.last_title = Some(title);
            g.title_unsent = true;
        }
        if let Some(at) = g.engine.answer_inferred_at()
            && at != g.persisted.answered_ms
            && g.persisted.hook.is_some()
        {
            g.persisted.answered_ms = at;
            save_persisted(&self.id, &g.persisted);
        }
        g.screen_dirty = true;
        self.publish(&mut g);
    }

    /// Input typed into the terminal (see `TerminalStatus::on_input`).
    pub fn on_input(&self, data: &[u8]) {
        let mut g = self.inner.lock().unwrap();
        g.engine.on_input(data, now());
    }

    /// The terminal's root process exited (or its holder vanished).
    pub fn on_exit(&self) {
        let mut g = self.inner.lock().unwrap();
        g.engine.on_process_exit(now());
        self.publish(&mut g);
    }

    /// Evidence from extension `owner` (`None` clears it), docs/16 §7.1.
    pub fn on_mod(&self, owner: &str, hint: Option<(AgentState, Option<String>)>) {
        let mut g = self.inner.lock().unwrap();
        let now = now();
        match hint {
            Some((state, msg)) => {
                g.engine.on_mod(Some(state), msg, now);
                g.mod_owner = Some(owner.to_string());
            }
            None => {
                g.engine.on_mod(None, None, now);
                g.mod_owner = None;
            }
        }
        self.publish(&mut g);
    }

    /// Drop extension `owner`'s evidence (the mod stopped or was disabled).
    pub fn clear_mod(&self, owner: &str) {
        let mut g = self.inner.lock().unwrap();
        if g.mod_owner.as_deref() == Some(owner) {
            g.mod_owner = None;
            g.engine.on_mod(None, None, now());
            self.publish(&mut g);
        }
    }

    /// Ticker: throttled screen sample + periodic engine tick.
    fn sample(&self, daemon: &Daemon, t: &Terminal) {
        let now = now();
        let want_screen = {
            let g = self.inner.lock().unwrap();
            g.screen_dirty && now.saturating_sub(g.last_screen_at) >= SCREEN_INTERVAL_MS
        };
        // Read the VT mirror without holding our lock (lock order).
        let screen =
            (want_screen && t.is_running()).then(|| t.read_lines(ReadMode::Screen).0.join("\n"));
        let mut g = self.inner.lock().unwrap();
        if want_screen {
            g.screen_dirty = false;
            g.last_screen_at = now;
        }
        let mut changed_screen = None;
        if let Some(text) = screen
            && text != g.last_screen
        {
            g.engine.on_screen(&text, now);
            g.last_screen = text.clone();
            changed_screen = Some(text);
        }
        let title = if std::mem::take(&mut g.title_unsent) {
            g.last_title.clone()
        } else {
            None
        };
        if now.saturating_sub(g.last_tick) >= TICK_INTERVAL_MS {
            g.last_tick = now;
            g.engine.tick(now);
        }
        self.publish(&mut g);
        drop(g);
        // Extension status detectors see the same samples (no lock held).
        super::mods::sample(daemon, t, changed_screen.as_deref(), title);
    }

    /// A hook message addressed to this terminal. Rejected unless its token
    /// matches the one injected into this terminal's environment.
    fn on_hook(&self, msg: &HookMessage) -> Result<(), &'static str> {
        let mut g = self.inner.lock().unwrap();
        if g.token.is_none() {
            g.token = load_token(&self.id);
        }
        let ok = match (&g.token, &msg.token) {
            (Some(want), Some(got)) => same_secret(want, got),
            _ => false,
        };
        if !ok {
            g.hooks_rejected += 1;
            self.publish(&mut g);
            return Err(if msg.token.is_none() {
                "missing token"
            } else if g.token.is_none() {
                "no token on record"
            } else {
                "token mismatch"
            });
        }
        g.hooks_accepted += 1;
        let now = now();
        g.engine.on_hook(msg, now);
        g.persisted.hook = Some(slim_hook(msg));
        g.persisted.hook_ms = now;
        g.persisted.answered_ms = 0;
        g.persisted.turn = g.engine.turn();
        save_persisted(&self.id, &g.persisted);
        self.publish(&mut g);
        Ok(())
    }

    fn ack(&self) {
        let mut g = self.inner.lock().unwrap();
        let now = now();
        g.engine.ack(now);
        if g.persisted.hook.is_some() {
            g.persisted.acked_ms = now;
            save_persisted(&self.id, &g.persisted);
        }
        self.publish(&mut g);
    }

    pub fn info(&self) -> StatusInfo {
        let g = self.inner.lock().unwrap();
        g.published.clone().unwrap_or_else(|| make_info(&g))
    }

    fn entry(&self) -> StatusEntry {
        let g = self.inner.lock().unwrap();
        StatusEntry {
            id: self.id.clone(),
            seq: g.seq,
            status: g.published.clone().unwrap_or_else(|| make_info(&g)),
        }
    }

    /// Final event for a removed terminal.
    fn removed(&self) {
        let g = self.inner.lock().unwrap();
        let info = g.published.clone().unwrap_or_else(|| make_info(&g));
        hub().publish(&self.id, info, None, true);
    }

    /// Publish if anything visible changed.
    fn publish(&self, g: &mut Inner) {
        let info = make_info(g);
        if g.published.as_ref() == Some(&info) {
            return;
        }
        let from = g
            .published
            .as_ref()
            .map(StatusInfo::state_label)
            .filter(|f| *f != info.state_label());
        if info.turn != g.persisted.turn {
            g.persisted.turn = info.turn;
            save_persisted(&self.id, &g.persisted);
        }
        g.seq = hub().publish(&self.id, info.clone(), from, false);
        g.published = Some(info);
    }
}

fn state_parts(s: AgentState) -> (String, Option<String>) {
    match s {
        AgentState::Blocked(k) => (
            "blocked".into(),
            Some(
                match k {
                    BlockedKind::Permission => "permission",
                    BlockedKind::Question => "question",
                    BlockedKind::Auth => "auth",
                    BlockedKind::Other => "other",
                }
                .into(),
            ),
        ),
        other => (other.name().into(), None),
    }
}

fn state_label(s: AgentState) -> String {
    match state_parts(s) {
        (st, Some(k)) => format!("{st}:{k}"),
        (st, None) => st,
    }
}

fn make_info(g: &Inner) -> StatusInfo {
    let s = g.engine.snapshot();
    let (state, kind) = state_parts(s.state);
    StatusInfo {
        state,
        kind,
        source: s.source.map(|x| {
            match x {
                Source::Hook => "hook",
                Source::Osc => "osc",
                Source::Heuristic => "heuristic",
                Source::Mod => "mod",
                Source::Process => "process",
            }
            .into()
        }),
        confidence: s.confidence,
        since: s.since,
        turn: s.turn,
        agent: s.agent.as_ref().map(|a| match a {
            AgentKind::Claude => "claude".into(),
            AgentKind::Codex => "codex".into(),
            AgentKind::Other(x) => x.clone(),
        }),
        app: s.app.clone(),
        title: s.title.clone(),
        msg: s.msg.clone(),
        progress: s.progress,
        outcome: s.outcome.map(|o| {
            match o {
                Outcome::Completed => "completed",
                Outcome::Cancelled => "cancelled",
                Outcome::Failed => "failed",
            }
            .into()
        }),
        final_state: s.final_state.map(state_label),
        badge: s.badge(),
        records: s.records.len(),
        running_children: s.running_children,
        blocked_children: s.blocked_children,
        hooks_accepted: g.hooks_accepted,
        hooks_rejected: g.hooks_rejected,
    }
}

// ───────────────────────────── window titles ─────────────────────────────

/// Finds OSC 0 / OSC 2 (window title) sequences in a chunked byte stream.
#[derive(Default)]
struct TitleScanner {
    state: TitleState,
    num: u32,
    buf: Vec<u8>,
    overflow: bool,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum TitleState {
    #[default]
    Ground,
    Esc,
    /// After `ESC ]`, reading the OSC number.
    Num,
    /// Collecting the title text.
    Body,
    /// `ESC` inside the body: `\` completes ST.
    BodyEsc,
}

impl TitleScanner {
    fn feed(&mut self, bytes: &[u8]) -> Vec<String> {
        let mut out = Vec::new();
        for &b in bytes {
            self.step(b, &mut out);
        }
        out
    }

    fn finish(&mut self, out: &mut Vec<String>) {
        if !self.overflow {
            out.push(String::from_utf8_lossy(&self.buf).into_owned());
        }
        self.buf.clear();
        self.overflow = false;
        self.state = TitleState::Ground;
    }

    fn step(&mut self, b: u8, out: &mut Vec<String>) {
        use TitleState::*;
        match self.state {
            Ground => {
                if b == 0x1b {
                    self.state = Esc;
                }
            }
            Esc => {
                self.state = match b {
                    b']' => {
                        self.num = 0;
                        Num
                    }
                    0x1b => Esc,
                    _ => Ground,
                }
            }
            Num => match b {
                b'0'..=b'9' if self.num < 100_000 => {
                    self.num = self.num * 10 + u32::from(b - b'0');
                }
                b';' if self.num == 0 || self.num == 2 => {
                    self.buf.clear();
                    self.overflow = false;
                    self.state = Body;
                }
                0x1b => self.state = Esc,
                _ => self.state = Ground, // another OSC: ignored
            },
            Body => match b {
                0x07 => self.finish(out),
                0x1b => self.state = BodyEsc,
                0x18 | 0x1a => {
                    self.buf.clear();
                    self.state = Ground;
                }
                _ => {
                    if self.buf.len() < TITLE_MAX {
                        self.buf.push(b);
                    } else {
                        self.overflow = true;
                    }
                }
            },
            BodyEsc => {
                if b == b'\\' {
                    self.finish(out);
                } else {
                    // Unterminated: drop it, and let this byte start over.
                    self.buf.clear();
                    self.state = Ground;
                    self.step(b, out);
                }
            }
        }
    }
}

// ───────────────────────────── hook ingestion ─────────────────────────────

/// If `line` is a `pod-hook` message (not JSON-RPC), route it and return true.
/// Hook lines get no reply: `pod-hook` writes one line and closes.
pub(super) fn try_ingest_hook(daemon: &Daemon, line: &str) -> bool {
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    if obj.contains_key("jsonrpc") || !obj.contains_key("terminal_id") || !obj.contains_key("v") {
        return false;
    }
    let msg: HookMessage = match serde_json::from_value(Value::Object(obj)) {
        Ok(m) => m,
        Err(e) => {
            log(&format!("hook: malformed message dropped: {e}"));
            return true;
        }
    };
    if msg.v != HOOK_MESSAGE_VERSION {
        log(&format!("hook: unsupported version {} dropped", msg.v));
        return true;
    }
    let term = daemon
        .terminals
        .lock()
        .unwrap()
        .iter()
        .find(|t| t.id == msg.terminal_id)
        .cloned();
    match term {
        // The holder may run the agent before `terminal.run` has connected
        // to it: a hook with this terminal's token still gives its session
        // id (kept until a task registers the terminal, sessions.rs).
        None if early_hook_token_ok(&msg) => {
            log(&format!(
                "hook: {} {} for {} before its terminal was registered: session id kept, status dropped",
                msg.agent.as_str(),
                msg.event,
                msg.terminal_id
            ));
            super::sessions::note_hook_session(
                daemon,
                &msg.terminal_id,
                msg.agent.as_str(),
                &msg.payload,
            );
        }
        None => log(&format!(
            "hook: {} {} for unknown terminal {:?} dropped",
            msg.agent.as_str(),
            msg.event,
            msg.terminal_id
        )),
        Some(t) => match t.status.on_hook(&msg) {
            Err(why) => log(&format!(
                "hook: {} {} for {} rejected ({why})",
                msg.agent.as_str(),
                msg.event,
                t.id
            )),
            // docs/19 §3.5: the task remembers the agent's session id.
            Ok(()) => {
                super::sessions::note_hook_session(daemon, &t.id, msg.agent.as_str(), &msg.payload)
            }
        },
    }
    true
}

// ───────────────────────────── RPC ─────────────────────────────

/// Methods served by this module (besides `status.subscribe`, a stream).
pub(super) fn handles(method: &str) -> bool {
    matches!(
        method,
        TerminalShow::NAME | TerminalAck::NAME | StatusWait::NAME
    )
}

pub(super) async fn dispatch(
    daemon: Arc<Daemon>,
    method: &str,
    p: Value,
) -> Result<Value, RpcError> {
    match method {
        TerminalShow::NAME => {
            let r: TerminalRef = params(p)?;
            to_value(daemon.resolve(&r)?.info())
        }
        TerminalAck::NAME => {
            let r: TerminalRef = params(p)?;
            let t = daemon.resolve(&r)?;
            t.status.ack();
            to_value(t.info())
        }
        StatusWait::NAME => to_value(status_wait(&daemon, params(p)?).await?),
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method {other:?}"),
        )),
    }
}

/// `terminal.rm`: drop the token and tell subscribers.
pub(super) fn on_removed(daemon: &Daemon, t: &Terminal) {
    remove_token(&daemon.config.state_dir, &t.id);
    t.status.removed();
}

const STATE_NAMES: &[&str] = &[
    "unknown", "idle", "working", "blocked", "done", "error", "exited",
];

enum Want {
    Any,
    State(String),
    AnyChange,
}

async fn status_wait(daemon: &Daemon, p: StatusWaitParams) -> Result<StatusWaitResult, RpcError> {
    let want = match p.status.as_deref() {
        None => Want::Any,
        Some("any_change" | "any-change") => Want::AnyChange,
        Some(s) if STATE_NAMES.contains(&s) => Want::State(s.to_string()),
        Some(s) => {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("unknown status {s:?} (expected one of {STATE_NAMES:?} or any_change)"),
            ));
        }
    };
    if matches!(want, Want::Any) && !p.turn_end {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            "status or turn_end is required",
        ));
    }
    let t = daemon.resolve(&p.target)?;
    // Subscribe before the first check so no change is missed.
    let mut rx = hub().tx.subscribe();
    let start = Instant::now();
    let initial = t.status.info();
    let deadline = p
        .timeout_ms
        .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
    let outcome = loop {
        let cur = t.status.info();
        let state_ok = match &want {
            Want::Any => true,
            Want::State(s) => cur.state == *s,
            Want::AnyChange => cur.state_label() != initial.state_label(),
        };
        let turn_ok = !p.turn_end || (cur.turn > initial.turn && cur.state != "working");
        let running = t.is_running();
        if state_ok && turn_ok && (running || matches!(&want, Want::State(s) if s == "exited")) {
            break WaitOutcome::Matched;
        }
        if !running {
            break WaitOutcome::Exited;
        }
        // Wait for an event of this terminal.
        let next = async {
            loop {
                match rx.recv().await {
                    Ok(HubEvent::Status(ev)) if ev.id == t.id => return,
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => return,
                    Err(broadcast::error::RecvError::Closed) => std::future::pending::<()>().await,
                }
            }
        };
        match deadline {
            Some(d) => {
                if tokio::time::timeout_at(d, next).await.is_err() {
                    break WaitOutcome::TimedOut;
                }
            }
            None => next.await,
        }
    };
    Ok(StatusWaitResult {
        outcome,
        status: t.status.info(),
        terminal: t.info(),
        waited_ms: start.elapsed().as_millis() as u64,
    })
}

/// `status.subscribe`: reply with every terminal's current status, then
/// stream `status.changed` until the client disconnects.
pub(super) async fn stream_subscribe(
    daemon: Arc<Daemon>,
    id: Value,
    p: Value,
    mut reader: BufReader<tokio::io::ReadHalf<NamedPipeServer>>,
    mut write: tokio::io::WriteHalf<NamedPipeServer>,
) -> anyhow::Result<()> {
    let p: StatusSubscribeParams = match params(p) {
        Ok(p) => p,
        Err(e) => return write_json(&mut write, &err_response(id, e)).await,
    };
    let wanted = |tid: &str| p.ids.is_empty() || p.ids.iter().any(|x| x == tid);
    let mut rx = hub().tx.subscribe();
    let _lifecycle = p.lifecycle.then(LifecycleGuard::new);
    let terms: Vec<Arc<Terminal>> = daemon.terminals.lock().unwrap().clone();
    let entries: Vec<StatusEntry> = terms
        .iter()
        .filter(|t| wanted(&t.id))
        .map(|t| t.status.entry())
        .collect();
    // Events already reflected in the snapshot are skipped per terminal.
    let mut seen: HashMap<String, u64> = entries.iter().map(|e| (e.id.clone(), e.seq)).collect();
    let gate_wanted = |g: &GateInfo| p.ids.is_empty() || g.terminal.as_deref().is_some_and(wanted);
    let gates: Vec<GateInfo> = daemon
        .orch
        .lock()
        .unwrap()
        .gates
        .iter()
        .filter(|g| g.state == GateState::Pending && gate_wanted(g))
        .cloned()
        .collect();
    let result = StatusSubscribeResult {
        epoch: hub().base_ms,
        seq: entries.iter().map(|e| e.seq).max().unwrap_or(0),
        terminals: entries,
        gates,
    };
    write_json(&mut write, &ok_response(id, serde_json::to_value(result)?)).await?;

    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(HubEvent::Gate(ev)) => {
                    if !gate_wanted(&ev.gate) {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_GATE_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::GroupCheck(ev)) => {
                    // Not about a terminal: only for unfiltered subscribers.
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_GROUP_CHECK, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Delivery(ev)) => {
                    if !wanted(&ev.delivery.terminal) {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_REVIEW_DELIVERY, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Terminal(ev)) => {
                    if !p.lifecycle || !wanted(&ev.id) {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_TERMINAL_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Task(ev)) => {
                    if !p.lifecycle || !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_TASK_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::TaskRemoving(ev)) => {
                    if !p.lifecycle || !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_TASK_REMOVING, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::TaskPr(ev)) => {
                    if !p.lifecycle || !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_TASK_PR_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::ModNotify(ev)) => {
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_MOD_NOTIFY, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Sessions(ev)) => {
                    if !p.lifecycle || !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_SESSIONS_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::ModChanged(ev)) => {
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_MOD_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::OrchChanged(ev)) => {
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_ORCH_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::OrchMail(ev)) => {
                    if !p.ids.is_empty() {
                        continue;
                    }
                    if write_json(&mut write, &notification(NOTIFY_ORCH_MAIL, ev)).await.is_err() {
                        break;
                    }
                }
                Ok(HubEvent::Status(ev)) => {
                    if !wanted(&ev.id) || ev.seq <= seen.get(&ev.id).copied().unwrap_or(0) {
                        continue;
                    }
                    seen.insert(ev.id.clone(), ev.seq);
                    if write_json(&mut write, &notification(NOTIFY_STATUS_CHANGED, ev)).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    let n = notification(NOTIFY_STATUS_LAGGED, StatusLaggedEvent { missed });
                    if write_json(&mut write, &n).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            // Anything the client sends is ignored; EOF ends the subscription.
            r = super::discard_input(&mut reader) => {
                if !matches!(r, Ok(n) if n > 0) {
                    break;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_bel_st_and_chunks() {
        let mut s = TitleScanner::default();
        let data =
            "a\x1b]0;\u{280b} work\x07b\x1b]2;plain\x1b\\c\x1b]7501;state=idle\x07\x1b]1;icon\x07";
        assert_eq!(s.feed(data.as_bytes()), vec!["\u{280b} work", "plain"]);
        // Every split point gives the same titles.
        let bytes = data.as_bytes();
        for cut in 0..bytes.len() {
            let mut s = TitleScanner::default();
            let mut got = s.feed(&bytes[..cut]);
            got.extend(s.feed(&bytes[cut..]));
            assert_eq!(got, vec!["\u{280b} work", "plain"], "cut at {cut}");
        }
    }

    #[test]
    fn titles_unterminated_and_overlong_dropped() {
        let mut s = TitleScanner::default();
        assert!(s.feed(b"\x1b]0;abc\x1b[31m").is_empty());
        assert_eq!(s.feed(b"\x1b]0;ok\x07"), vec!["ok"]);
        let long = format!("\x1b]0;{}\x07", "x".repeat(TITLE_MAX + 5));
        assert!(s.feed(long.as_bytes()).is_empty());
        assert_eq!(s.feed(b"\x1b]2;next\x07"), vec!["next"]);
    }

    #[test]
    fn secrets_compare() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "abcd"));
    }

    #[test]
    fn token_is_random_hex() {
        let a = new_token().unwrap();
        let b = new_token().unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
