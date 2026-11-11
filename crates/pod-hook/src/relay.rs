//! Hook mode: read the hook JSON from stdin within its budget, wrap it in a
//! v1 [`HookMessage`] and write one line to podd's pipe before the deadline.
//!
//! Write-only: no reply is read, no daemon is started, nothing reconnects.
//! The caller has already printed `{}` and exits right after [`run`]
//! returns, whatever happened here.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pod_status::hook::{
    ENV_HOOK_TOKEN, ENV_SOCKET, ENV_TERMINAL_ID, HookAgent, HookMessage, MAX_HOOK_LINE,
    TRUNCATED_KEY,
};
use serde_json::Value;

/// Total time budget for reading stdin + delivering the message.
const DEADLINE: Duration = Duration::from_millis(1000);
/// Part of the budget for reading stdin.
const STDIN_BUDGET: Duration = Duration::from_millis(500);
/// Larger payloads are forwarded as a `pod_truncated` marker (event name
/// still delivered). The whole line is capped separately (`to_wire_line`).
const STDIN_LIMIT: u64 = 4 * 1024 * 1024;

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// Relay one hook event: `args` is `claude|codex <Event>`, `start` is when
/// the process started (the deadline counts from there).
pub(crate) fn run(args: &[String], start: Instant) {
    let (Some(terminal_id), Some(socket)) =
        (env_nonempty(ENV_TERMINAL_ID), env_nonempty(ENV_SOCKET))
    else {
        return;
    };
    let Some(agent) = args.first().and_then(|a| HookAgent::parse(a)) else {
        return;
    };
    let event = args.get(1).cloned().unwrap_or_default();

    let payload = read_stdin(STDIN_BUDGET);
    let mut msg = HookMessage::new(terminal_id, agent, event, payload);
    msg.token = env_nonempty(ENV_HOOK_TOKEN);
    msg.sent_at_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // podd drops a line over its limit, and the event with it: cut the
    // payload, never the event (daemon-05).
    let mut line = msg.to_wire_line(MAX_HOOK_LINE);
    line.push('\n');

    let remaining = DEADLINE.saturating_sub(start.elapsed());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(send_line(
            &socket,
            line.as_bytes(),
            Instant::now() + remaining,
        ));
    });
    let _ = rx.recv_timeout(remaining);
}

/// Read stdin with a time budget (the agent might not close it).
fn read_stdin(budget: Duration) -> Value {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let r = std::io::stdin()
            .lock()
            .take(STDIN_LIMIT + 1)
            .read_to_end(&mut buf);
        let _ = tx.send(r.map(|_| buf));
    });
    match rx.recv_timeout(budget) {
        Ok(Ok(buf)) if (buf.len() as u64) <= STDIN_LIMIT => {
            serde_json::from_slice(&buf).unwrap_or(Value::Null)
        }
        Ok(Ok(_)) => serde_json::json!({ TRUNCATED_KEY: { "payload_bytes_over": STDIN_LIMIT } }),
        _ => Value::Null,
    }
}

#[cfg(windows)]
fn send_line(pipe: &str, line: &[u8], deadline: Instant) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT};
    const ERROR_PIPE_BUSY: i32 = 231;
    loop {
        // Whoever serves this pipe name (podd, or a process that took the
        // name while podd was down) may identify us, never impersonate us.
        match std::fs::OpenOptions::new()
            .write(true)
            .security_qos_flags(SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION)
            .open(pipe)
        {
            Ok(mut f) => {
                f.write_all(line)?;
                return f.flush();
            }
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => return Err(e),
        }
    }
}

#[cfg(unix)]
fn send_line(path: &str, line: &[u8], deadline: Instant) -> std::io::Result<()> {
    let mut s = std::os::unix::net::UnixStream::connect(path)?;
    let left = deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_millis(1));
    s.set_write_timeout(Some(left))?;
    s.write_all(line)
}
