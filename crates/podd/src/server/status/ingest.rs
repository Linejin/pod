//! Hook ingest: `pod-hook` lines on the daemon pipe, and the per-terminal
//! hook tokens that authenticate them (`<state dir>\<id>.token`, injected
//! into the terminal as `POD_HOOK_TOKEN`).
//!
//! A line is routed to its terminal's [`TermStatus`](super::TermStatus),
//! whose [`HookGate`] checks the token, the message shape and the flood
//! limit, and counts a rejection. Malformed lines, other message versions
//! and unknown terminals are logged (at most [`DROP_LOGS_PER_SEC`] a second)
//! and dropped without being counted. Hook lines get no reply.
//!
//! Trust (TM-22, docs/07 §8.2): a hook is state evidence only. It never
//! decides a gate, approves a tool or reports a task done. Its token is
//! bound to one terminal and that terminal's life: a fresh one per
//! `terminal.run` (ids are never handed out twice while a record or token
//! file exists), revoked when the terminal's process exits, on `rm`, on a
//! given-up or stale holder record, and swept at start for every file
//! without a listed or unreachable holder. A running agent keeps its token
//! across a daemon restart: its environment cannot change, so rotating
//! the token then would only cut a legitimate agent off. Tokens are never
//! logged, persisted in status files or written to a terminal.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use pod_status::hook::{ENV_HOOK_TOKEN, HOOK_MESSAGE_VERSION, MAX_EVENT_NAME};
use pod_status::{HookMessage, Millis};
use serde_json::Value;

use super::hub::hub;
use crate::protocol::*;
use crate::server::{Daemon, log, sessions};

/// Hooks one terminal may deliver in a burst, and per second after it
/// (TM-22). A real agent sends a few per tool call; beyond this a token
/// holder is flooding, and each extra line is a counted rejection.
pub(super) const HOOK_BURST: f64 = 120.0;
pub(super) const HOOK_RATE_PER_SEC: f64 = 60.0;
/// Rejections per terminal and second that are logged and published at
/// once; the rest are counted (the ticker publishes the counter) and
/// summarized in the next logged one.
pub(super) const REJECT_REPORTS_PER_SEC: u32 = 10;
/// Daemon-wide log lines per second for dropped (uncounted) hook lines.
pub(super) const DROP_LOGS_PER_SEC: u32 = 10;

pub(super) fn token_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.token"))
}

/// A one-second window that lets the first `limit` events through and
/// counts the rest.
#[derive(Debug, Default)]
struct Window {
    start: Option<Millis>,
    passed: u32,
    held: u64,
}

impl Window {
    /// `Some(held before this one)` when this event may be reported now.
    fn pass(&mut self, now: Millis, limit: u32) -> Option<u64> {
        if self.start.is_none_or(|s| now.saturating_sub(s) >= 1000) {
            self.start = Some(now);
            self.passed = 0;
        }
        if self.passed < limit {
            self.passed += 1;
            Some(std::mem::take(&mut self.held))
        } else {
            self.held += 1;
            None
        }
    }
}

/// Why a hook was not accepted, and whether to report it now
/// (`Some(n)`: log it, `n` earlier rejections were only counted).
#[derive(Debug, PartialEq)]
pub(super) struct Rejected {
    pub why: &'static str,
    pub report: Option<u64>,
}

/// One terminal's admission of hook lines (TM-22): its token (read once
/// from the token file), whether the terminal's life ended, the flood
/// limit, and how often rejections are reported.
#[derive(Debug)]
pub(super) struct HookGate {
    token: Option<String>,
    revoked: bool,
    bucket: f64,
    refilled_at: Option<Millis>,
    reports: Window,
}

impl Default for HookGate {
    fn default() -> Self {
        Self {
            token: None,
            revoked: false,
            bucket: HOOK_BURST,
            refilled_at: None,
            reports: Window::default(),
        }
    }
}

impl HookGate {
    /// Accept `msg` for this gate's terminal at `now`, or say why not.
    /// `load` reads the terminal's token file until it is found.
    pub(super) fn admit(
        &mut self,
        msg: &HookMessage,
        now: Millis,
        load: impl FnOnce() -> Option<String>,
    ) -> Result<(), Rejected> {
        let why = self.check(msg, now, load);
        why.map_err(|why| Rejected {
            why,
            report: self.reports.pass(now, REJECT_REPORTS_PER_SEC),
        })
    }

    fn check(
        &mut self,
        msg: &HookMessage,
        now: Millis,
        load: impl FnOnce() -> Option<String>,
    ) -> Result<(), &'static str> {
        if self.revoked {
            return Err("terminal exited, token revoked");
        }
        if self.token.is_none() {
            self.token = load();
        }
        match (&self.token, &msg.token) {
            (Some(want), Some(got)) if same_secret(want, got) => {}
            (_, None) => return Err("missing token"),
            (None, Some(_)) => return Err("no token on record"),
            (Some(_), Some(_)) => return Err("token mismatch"),
        }
        msg.check()?;
        let since = self.refilled_at.map_or(0, |t| now.saturating_sub(t));
        self.refilled_at = Some(now);
        self.bucket = (self.bucket + since as f64 * HOOK_RATE_PER_SEC / 1000.0).min(HOOK_BURST);
        if self.bucket < 1.0 {
            return Err("rate limited");
        }
        self.bucket -= 1.0;
        Ok(())
    }

    /// The terminal's process exited: no later hook is accepted. True the
    /// first time (the caller removes the token file).
    pub(super) fn revoke(&mut self) -> bool {
        self.token = None;
        !std::mem::replace(&mut self.revoked, true)
    }
}

/// Remove `id`'s token file (the terminal's life ended; its persisted
/// status stays until `rm`).
pub(super) fn revoke_token(id: &str) {
    if let Some(dir) = hub().state_dir.get() {
        let _ = std::fs::remove_file(token_path(dir, id));
    }
}

/// At start, after rediscovery: remove token files whose terminal is
/// neither listed nor an unreachable holder that may still come back (a
/// daemon that died between writing a token and listing its terminal, or
/// a record removed by hand).
pub(super) fn sweep_tokens(daemon: &Daemon) {
    let keep: HashSet<String> = daemon
        .terminals
        .lock()
        .unwrap()
        .iter()
        .map(|t| t.id.clone())
        .chain(
            daemon
                .unreachable
                .lock()
                .unwrap()
                .iter()
                .map(|u| u.id.clone()),
        )
        .collect();
    let Ok(entries) = std::fs::read_dir(&daemon.config.state_dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(id) = name.to_str().and_then(|n| n.strip_suffix(".token")) else {
            continue;
        };
        if is_terminal_id(id) && !keep.contains(id) && std::fs::remove_file(e.path()).is_ok() {
            log(&format!("hook token of {id} revoked (no holder)"));
        }
    }
}

/// Daemon-wide limit for the log lines of dropped hook lines.
fn drop_log(msg: impl FnOnce() -> String) {
    static WINDOW: Mutex<Window> = Mutex::new(Window {
        start: None,
        passed: 0,
        held: 0,
    });
    let pass = WINDOW.lock().unwrap().pass(super::now(), DROP_LOGS_PER_SEC);
    match pass {
        Some(0) => log(&msg()),
        Some(n) => log(&format!("{} ({n} more hook lines dropped before)", msg())),
        None => {}
    }
}

/// A sender-chosen string for a log line: escaped, at most
/// [`MAX_EVENT_NAME`] characters.
fn shown(s: &str) -> String {
    let cut: String = s.chars().take(MAX_EVENT_NAME).collect();
    format!("{cut:?}")
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
pub(in crate::server) fn hook_env(
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

pub(super) fn load_token(id: &str) -> Option<String> {
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

pub(super) fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// If `line` is a `pod-hook` message (not JSON-RPC), route it and return true.
/// Hook lines get no reply: `pod-hook` writes one line and closes.
pub(in crate::server) fn try_ingest_hook(daemon: &Daemon, line: &str) -> bool {
    let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    if obj.contains_key("jsonrpc") || !obj.contains_key("terminal_id") || !obj.contains_key("v") {
        return false;
    }
    let msg: HookMessage = match serde_json::from_value(Value::Object(obj)) {
        Ok(m) => m,
        Err(e) => {
            // Not the serde text: it can quote a field's value (a token in
            // the wrong place, say). The kind and position are enough.
            drop_log(|| {
                format!(
                    "hook: malformed message dropped ({:?} error at column {})",
                    e.classify(),
                    e.column()
                )
            });
            return true;
        }
    };
    if msg.v != HOOK_MESSAGE_VERSION {
        drop_log(|| format!("hook: unsupported version {} dropped", msg.v));
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
        None if msg.check().is_ok() && early_hook_token_ok(&msg) => {
            log(&format!(
                "hook: {} {} for {} before its terminal was registered: session id kept, status dropped",
                msg.agent.as_str(),
                shown(&msg.event),
                msg.terminal_id
            ));
            sessions::note_hook_session(daemon, &msg.terminal_id, msg.agent.as_str(), &msg.payload);
        }
        None => drop_log(|| {
            format!(
                "hook: {} {} for unknown terminal {} dropped",
                msg.agent.as_str(),
                shown(&msg.event),
                shown(&msg.terminal_id)
            )
        }),
        Some(t) => match t.status.on_hook(&msg) {
            Err(Rejected {
                why,
                report: Some(held),
            }) => log(&format!(
                "hook: {} {} for {} rejected ({why}){}",
                msg.agent.as_str(),
                shown(&msg.event),
                t.id,
                if held > 0 {
                    format!("; {held} more rejected before this one")
                } else {
                    String::new()
                }
            )),
            Err(Rejected { report: None, .. }) => {}
            // docs/19 §3.5: the task remembers the agent's session id.
            Ok(()) => sessions::note_hook_session(daemon, &t.id, msg.agent.as_str(), &msg.payload),
        },
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use pod_status::HookAgent;

    #[test]
    fn secrets_compare() {
        assert!(same_secret("abc", "abc"));
        assert!(!same_secret("abc", "abd"));
        assert!(!same_secret("abc", "abcd"));
    }

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn hook(token: Option<&str>, event: &str) -> HookMessage {
        let mut m = HookMessage::new("t1", HookAgent::Claude, event, serde_json::json!({}));
        m.token = token.map(str::to_string);
        m
    }

    fn admit(g: &mut HookGate, m: &HookMessage, now: Millis) -> Result<(), &'static str> {
        g.admit(m, now, || Some(TOKEN.to_string()))
            .map_err(|r| r.why)
    }

    /// TM-22: the token decides first (missing, mismatch, none on record),
    /// then the message shape; a revoked gate takes nothing, not even the
    /// right token.
    #[test]
    fn gate_checks_token_shape_and_lifetime() {
        let mut g = HookGate::default();
        assert_eq!(admit(&mut g, &hook(None, "Stop"), 0), Err("missing token"));
        assert_eq!(
            admit(
                &mut g,
                &hook(Some("ffffffffffffffffffffffffffffffff"), "Stop"),
                0
            ),
            Err("token mismatch")
        );
        assert_eq!(
            admit(&mut g, &hook(Some(TOKEN), "Stop\r\nforged"), 0),
            Err("bad event name")
        );
        assert_eq!(admit(&mut g, &hook(Some(TOKEN), "Stop"), 0), Ok(()));
        let mut none = HookGate::default();
        assert_eq!(
            none.admit(&hook(Some(TOKEN), "Stop"), 0, || None)
                .map_err(|r| r.why),
            Err("no token on record")
        );
        assert!(g.revoke(), "first revoke removes the file");
        assert!(!g.revoke());
        assert_eq!(
            admit(&mut g, &hook(Some(TOKEN), "Stop"), 1),
            Err("terminal exited, token revoked")
        );
    }

    /// TM-22: a burst of [`HOOK_BURST`] is accepted, then the limit holds
    /// at [`HOOK_RATE_PER_SEC`]; every refused line is a counted rejection.
    #[test]
    fn gate_limits_floods() {
        let mut g = HookGate::default();
        let m = hook(Some(TOKEN), "PostToolUse");
        let burst = HOOK_BURST as usize;
        let ok = (0..burst * 3)
            .filter(|_| admit(&mut g, &m, 1_000).is_ok())
            .count();
        assert_eq!(ok, burst);
        assert_eq!(admit(&mut g, &m, 1_000), Err("rate limited"));
        // One second later: one second's worth.
        let ok = (0..burst * 3)
            .filter(|_| admit(&mut g, &m, 2_000).is_ok())
            .count();
        assert_eq!(ok, HOOK_RATE_PER_SEC as usize);
        // A quiet minute refills the burst, never more.
        let ok = (0..burst * 3)
            .filter(|_| admit(&mut g, &m, 62_000).is_ok())
            .count();
        assert_eq!(ok, burst);
    }

    /// Rejections are reported (logged, published) at most
    /// [`REJECT_REPORTS_PER_SEC`] a second; the next report says how many
    /// were only counted.
    #[test]
    fn rejection_reports_are_limited() {
        let mut g = HookGate::default();
        let bad = hook(None, "Stop");
        let reports: Vec<Option<u64>> = (0..25)
            .map(|_| g.admit(&bad, 5_000, || None).unwrap_err().report)
            .collect();
        let n = REJECT_REPORTS_PER_SEC as usize;
        assert!(reports[..n].iter().all(|r| *r == Some(0)), "{reports:?}");
        assert!(reports[n..].iter().all(Option::is_none), "{reports:?}");
        let next = g.admit(&bad, 6_000, || None).unwrap_err().report;
        assert_eq!(next, Some((25 - n) as u64));
    }

    #[test]
    fn shown_strings_are_escaped_and_short() {
        let s = shown(&format!("Stop\n{}", "x".repeat(500)));
        assert!(s.starts_with("\"Stop\\n") && !s.contains('\n'), "{s}");
        assert!(s.chars().count() <= MAX_EVENT_NAME + 8, "{s}");
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
