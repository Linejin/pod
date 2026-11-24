//! Status-line mode: `pod-hook statusline [--passthrough [--ascii]]`.
//!
//! Claude Code runs a `statusLine` command with its status-line JSON on
//! stdin and shows what the command prints. In this mode pod-hook keeps the
//! documented `rate_limits` numbers only (`pod_status::usage`), stores them in
//! Pod's per-user cache for the usage monitor, and prints:
//!
//! - default: one short line (`5h 76% 남음 · 7d 59% 남음`, or nothing yet);
//! - `--passthrough`: the input unchanged, so an existing status-line command
//!   keeps working behind a pipe: `pod-hook statusline --passthrough | <old command>`;
//! - `--passthrough --ascii`: the same JSON with every non-ASCII character
//!   written as a `\uXXXX` escape (an equal JSON value; key order and spacing
//!   kept). Windows PowerShell re-encodes text piped between native commands
//!   as ASCII (`$OutputEncoding`), which turns `C:/작업` into `C:/??`; Pod's
//!   PowerShell setup uses this form. Input that is not a JSON object is
//!   passed through unchanged.
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
    pub ascii: bool,
}

impl Options {
    pub(crate) fn parse(args: &[String]) -> Result<Self, String> {
        let mut o = Options {
            passthrough: false,
            ascii: false,
        };
        for a in args {
            match a.as_str() {
                "--passthrough" => o.passthrough = true,
                "--ascii" => o.ascii = true,
                other => return Err(format!("unknown option {other}")),
            }
        }
        if o.ascii && !o.passthrough {
            return Err("--ascii needs --passthrough".into());
        }
        Ok(o)
    }
}

/// The passthrough bytes: unchanged, or (`ascii`) the same JSON with each
/// non-ASCII character escaped. JSON tokens and whitespace are ASCII, so in
/// valid JSON text a non-ASCII character can only be inside a string, where
/// `\uXXXX` (a surrogate pair beyond the BMP) denotes the same character.
/// A leading UTF-8 BOM is dropped then; anything that is not a JSON object
/// is returned unchanged.
pub(crate) fn passthrough_bytes(input: &[u8], ascii: bool) -> std::borrow::Cow<'_, [u8]> {
    use std::borrow::Cow;
    use std::fmt::Write as _;
    if !ascii {
        return Cow::Borrowed(input);
    }
    let body = input.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(input);
    let Ok(text) = std::str::from_utf8(body) else {
        return Cow::Borrowed(input);
    };
    if !serde_json::from_str::<serde_json::Value>(text).is_ok_and(|v| v.is_object()) {
        return Cow::Borrowed(input);
    }
    if text.is_ascii() {
        return Cow::Borrowed(body);
    }
    let mut out = String::with_capacity(text.len() + text.len() / 2);
    for c in text.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                let _ = write!(out, "\\u{u:04x}");
            }
        }
    }
    Cow::Owned(out.into_bytes())
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
        let _ = out.write_all(&passthrough_bytes(b, opts.ascii));
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
        let both = Options::parse(&["--passthrough".into(), "--ascii".into()]).unwrap();
        assert!(both.passthrough && both.ascii);
        assert!(
            Options::parse(&["--ascii".into()]).is_err(),
            "--ascii only changes the passthrough"
        );
        assert!(Options::parse(&["--bogus".into()]).is_err());
    }

    #[test]
    fn ascii_passthrough_is_the_same_json_in_ascii() {
        let input = r#"{"cwd":"C:/작업/저장소",  "z":1,"a":"😀 · \"q\""}"#.to_string() + "\n";
        let out = passthrough_bytes(input.as_bytes(), true);
        let out = std::str::from_utf8(&out).unwrap();
        assert!(out.is_ascii(), "{out}");
        // The same value, key order and spacing.
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(out).unwrap(),
            serde_json::from_str::<serde_json::Value>(&input).unwrap()
        );
        assert!(out.starts_with(r#"{"cwd":"C:/\uc791\uc5c5/"#), "{out}");
        assert!(out.contains(r#"  "z":1"#), "spacing kept: {out}");
        assert!(out.contains(r"\ud83d\ude00"), "surrogate pair: {out}");
        assert!(out.find(r#""z""#).unwrap() < out.find(r#""a""#).unwrap());
        assert!(out.ends_with("}\n"));
        // A BOM is dropped; ASCII JSON and anything not JSON stay unchanged.
        let bom = [b"\xEF\xBB\xBF".as_slice(), input.as_bytes()].concat();
        assert_eq!(&*passthrough_bytes(&bom, true), out.as_bytes());
        assert_eq!(&*passthrough_bytes(br#"{"a":1}"#, true), br#"{"a":1}"#);
        let junk = "not json: 작업".as_bytes();
        assert_eq!(&*passthrough_bytes(junk, true), junk);
        assert_eq!(
            &*passthrough_bytes(input.as_bytes(), false),
            input.as_bytes()
        );
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
