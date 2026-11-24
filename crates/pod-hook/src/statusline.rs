//! Status-line mode: `pod-hook statusline [--passthrough]`.
//!
//! Claude Code runs a `statusLine` command with its status-line JSON on
//! stdin and shows what the command prints. In this mode pod-hook keeps the
//! documented `rate_limits` numbers only (`pod_status::usage`), stores them in
//! Pod's per-user cache for the usage monitor, and prints:
//!
//! - default: one short line (`5h 76% 남음 · 7d 59% 남음`, or nothing yet);
//! - `--passthrough`: the input unchanged, so an existing status-line command
//!   keeps working behind a pipe: `pod-hook statusline --passthrough | <old command>`.
//!
//! The input is never logged or stored; nothing but the normalized numbers
//! and timestamps reaches the disk. Stdin is read within a time and size
//! budget; no network, no child process, no window.

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use pod_status::usage::{self, MAX_STATUSLINE_BYTES, Profile};

/// Stdin budget (Claude Code cancels a status-line command that a newer
/// update replaces, and debounces at 300 ms).
const STDIN_BUDGET: Duration = Duration::from_millis(700);

pub(crate) struct Options {
    pub passthrough: bool,
}

impl Options {
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = Options { passthrough: false };
        for a in args {
            match a.as_str() {
                "--passthrough" => o.passthrough = true,
                other => return Err(format!("unknown option {other}")),
            }
        }
        Ok(o)
    }
}

/// What one run read: the bytes (at most the limit + 1) and whether they
/// arrived within the budget.
pub(crate) fn read_stdin(budget: Duration) -> Option<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let r = std::io::stdin()
            .lock()
            .take(MAX_STATUSLINE_BYTES as u64 + 1)
            .read_to_end(&mut buf);
        let _ = tx.send(r.map(|_| buf));
    });
    rx.recv_timeout(budget).ok().and_then(Result::ok)
}

/// Parse and store one sample; returns the line to print (without passthrough).
pub(crate) fn ingest(input: Option<&[u8]>, dir: &std::path::Path, profile: &Profile) -> String {
    let sample = input
        .filter(|b| b.len() <= MAX_STATUSLINE_BYTES)
        // Windows PowerShell pipes text to a native command with a UTF-8 BOM.
        .map(|b| b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b))
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(b).ok())
        .filter(serde_json::Value::is_object);
    // Unreadable input is not a sample: nothing is written.
    let Some(sample) = sample else {
        return String::new();
    };
    match usage::ingest_statusline(dir, profile, Some(&sample), usage::now_ms()) {
        Ok(rec) => rec.summary_line(),
        Err(_) => String::new(),
    }
}

pub(crate) fn run(args: &[String], start: Instant) -> i32 {
    let opts = match Options::parse(args) {
        Ok(o) => o,
        Err(_) => return 2,
    };
    let input = read_stdin(STDIN_BUDGET.saturating_sub(start.elapsed()));
    let mut out = std::io::stdout().lock();
    if opts.passthrough
        && let Some(b) = &input
    {
        let _ = out.write_all(b);
        let _ = out.flush();
    }
    let line = ingest(input.as_deref(), &usage::usage_dir(), &Profile::from_env());
    if !opts.passthrough && !line.is_empty() {
        let _ = writeln!(out, "{line}");
    }
    let _ = out.flush();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        assert!(!Options::parse(&[]).unwrap().passthrough);
        assert!(
            Options::parse(&["--passthrough".into()])
                .unwrap()
                .passthrough
        );
        assert!(Options::parse(&["--bogus".into()]).is_err());
    }

    #[test]
    fn oversized_or_malformed_input_writes_nothing() {
        let d = std::env::temp_dir().join(format!("pod-hook-sl-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = Profile::from_config_dir(None);
        let big = vec![b' '; MAX_STATUSLINE_BYTES + 1];
        assert_eq!(ingest(Some(&big), &d, &p), "");
        assert_eq!(ingest(Some(b"{not json"), &d, &p), "");
        assert_eq!(ingest(Some(b"[1,2]"), &d, &p), "");
        assert_eq!(ingest(None, &d, &p), "");
        assert!(!d.exists(), "no file for unusable input");
    }
}
