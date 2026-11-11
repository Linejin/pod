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
//! The code is split by responsibility; this file keeps [`TermStatus`]
//! (one engine per terminal: output, titles, ticker samples, the persisted
//! status) and the paths the rest of the server uses (`start`, `handles`,
//! `dispatch`, the `publish_*` functions, `subscribe`, `try_ingest_hook`,
//! `stream_subscribe`):
//! - `hub`: the daemon-wide event sequence and broadcast, the engine clock,
//!   the state dir;
//! - `ingest`: `pod-hook` lines and the per-terminal hook tokens;
//! - `stream`: `status.wait` and the `status.subscribe` stream.
//!
//! Lock order: terminal stream state -> [`TermStatus`] -> hub publish lock.

mod hub;
mod ingest;
mod stream;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pod_status::{
    AgentKind, AgentState, BlockedKind, HookMessage, Millis, Outcome, ProcessInfo, Source,
    TerminalStatus,
};
use serde_json::Value;

use super::terminal::Terminal;
use super::{Daemon, log, params, to_value};
use crate::protocol::*;
use hub::hub;
use ingest::{load_token, same_secret, token_path};
use stream::status_wait;

pub use hub::now;
pub(super) use hub::{
    HubEvent, publish_delivery, publish_gate, publish_group_check, publish_mod,
    publish_orch_changed, publish_orch_mail, publish_sessions, publish_task, publish_task_pr,
    publish_task_removing, publish_terminal, set_state_dir, subscribe, task_event,
};
pub(crate) use ingest::new_token;
pub(super) use ingest::{hook_env, try_ingest_hook};
pub use pod_status::hook::ENV_HOOK_TOKEN;
pub(super) use stream::{lifecycle_subscribers, stream_subscribe};

/// Minimum time between two screen samples of one terminal.
const SCREEN_INTERVAL_MS: Millis = 250;
/// Engine `tick` period (quiet-time idle, hook staleness).
const TICK_INTERVAL_MS: Millis = 1000;
/// Window titles longer than this are ignored.
const TITLE_MAX: usize = 1024;

// ───────────────────────────── ticker ─────────────────────────────

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

// ───────────────────────────── terminal gone ─────────────────────────────

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
}
