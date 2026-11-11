//! Hook ingest: `pod-hook` lines on the daemon pipe, and the per-terminal
//! hook tokens that authenticate them (`<state dir>\<id>.token`, injected
//! into the terminal as `POD_HOOK_TOKEN`).
//!
//! A line is routed to its terminal's [`TermStatus`](super::TermStatus),
//! which checks the token and counts a rejection. Malformed lines, other
//! message versions and unknown terminals are logged and dropped without
//! being counted. Hook lines get no reply.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pod_status::HookMessage;
use pod_status::hook::{ENV_HOOK_TOKEN, HOOK_MESSAGE_VERSION};
use serde_json::Value;

use super::hub::hub;
use crate::protocol::*;
use crate::server::{Daemon, log, sessions};

pub(super) fn token_path(dir: &Path, id: &str) -> PathBuf {
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
            sessions::note_hook_session(daemon, &msg.terminal_id, msg.agent.as_str(), &msg.payload);
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
            Ok(()) => sessions::note_hook_session(daemon, &t.id, msg.agent.as_str(), &msg.payload),
        },
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

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
