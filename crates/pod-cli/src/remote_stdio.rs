//! `pod remote-stdio`: a short-lived bridge from stdin/stdout to the local
//! daemon (docs/17 §2.2, R2).
//!
//! A remote client runs `ssh -T host pod remote-stdio` (or `wsl.exe … --exec
//! pod remote-stdio`) and speaks the normal NDJSON JSON-RPC over the child's
//! stdio. The bridge only moves lines: it connects to this user's podd
//! endpoint, starting the installed `podd` next to `pod` if none runs (one
//! starter at a time, bounded wait — [`podd::client::Client::connect_or_start`]),
//! then copies stdin lines to the daemon and daemon lines to stdout.
//!
//! - **stdout carries protocol lines only.** Nothing else in this path prints
//!   to stdout; the auto-started daemon gets `NUL` stdio and inherits no
//!   handle. A daemon line that is not a JSON object stops the bridge (fail
//!   closed) instead of being forwarded or skipped.
//! - **stderr** gets a few short diagnostics (bounded count and length).
//! - Lines over 8 MiB, non-UTF-8 lines and lines cut by EOF are refused in
//!   both directions before they are buffered.
//! - When the daemon side closes, the bridge exits. When stdin closes, it
//!   still forwards the answers to requests already sent (by JSON-RPC id, at
//!   most [`DRAIN_MAX`]) and then exits. The daemon, its holders and their
//!   programs keep running; the bridge is never their parent.

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use podd::client::Client;
use podd::transport::{FrameError, LineReader, LineWriter, REMOTE_MAX_LINE};

/// Exit codes.
const EXIT_UNREACHABLE: u8 = 3;
const EXIT_PROTOCOL: u8 = 4;
/// After stdin closes, answers to requests already sent go out for at most this long.
pub const DRAIN_MAX: Duration = Duration::from_secs(30);

const DIAG_MAX_LINES: usize = 16;
const DIAG_MAX_CHARS: usize = 300;
static DIAG_COUNT: AtomicUsize = AtomicUsize::new(0);

/// One short line on stderr; at most [`DIAG_MAX_LINES`] per run.
fn diag(msg: &str) {
    let n = DIAG_COUNT.fetch_add(1, Ordering::Relaxed);
    if n >= DIAG_MAX_LINES {
        return;
    }
    let mut line: String = msg
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(DIAG_MAX_CHARS)
        .collect();
    if n + 1 == DIAG_MAX_LINES {
        line.push_str(" (further diagnostics suppressed)");
    }
    let _ = writeln!(std::io::stderr(), "pod remote-stdio: {line}");
}

/// Why one direction stopped.
#[derive(Debug, PartialEq)]
pub enum Stop {
    /// Clean EOF between lines.
    Closed,
    /// A framing or content violation (already described on stderr).
    Protocol(String),
    /// Writing to the other side failed (it went away).
    WriteFailed(String),
}

/// A daemon line must be one JSON object (a response or a notification).
/// Cheap shape check; the remote client parses it fully.
pub fn is_protocol_line(line: &[u8]) -> bool {
    let t = line.trim_ascii();
    t.first() == Some(&b'{') && t.last() == Some(&b'}')
}

/// Requests sent to the daemon whose response has not come back yet.
pub type Outstanding = Arc<AtomicI64>;

/// Does this JSON-RPC line carry a non-null `id` (a request expecting an
/// answer, or that answer)? Notifications and id-less error replies do not.
fn has_id(line: &[u8]) -> bool {
    #[derive(serde::Deserialize)]
    struct Probe {
        #[serde(default)]
        id: Option<serde::de::IgnoredAny>,
    }
    serde_json::from_slice::<Probe>(line).is_ok_and(|p| p.id.is_some())
}

fn frame_stop(what: &str, e: FrameError) -> Stop {
    match e {
        FrameError::Truncated => Stop::Protocol(format!("{what} closed in the middle of a line")),
        FrameError::Io(e) => Stop::WriteFailed(format!("{what}: {e}")),
        e => Stop::Protocol(format!("{what}: {e}")),
    }
}

/// Copy daemon lines to `out` until EOF or a violation. A non-protocol line
/// is not written.
pub fn pump_daemon_to_client(
    from: &mut LineReader,
    out: &mut LineWriter,
    outstanding: &AtomicI64,
) -> Stop {
    loop {
        let line = match from.read_line_bytes() {
            Ok(Some(l)) => l,
            Ok(None) => return Stop::Closed,
            Err(e) => return frame_stop("daemon connection", e),
        };
        if std::str::from_utf8(&line).is_err() {
            return Stop::Protocol("daemon sent a line that is not UTF-8".into());
        }
        if !is_protocol_line(&line) {
            return Stop::Protocol("daemon sent a line that is not a JSON object".into());
        }
        if let Err(e) = out.write_line_bytes(&line) {
            return Stop::WriteFailed(format!("stdout: {e}"));
        }
        if has_id(&line) {
            outstanding.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// Copy client lines (stdin) to the daemon until EOF or a violation.
pub fn pump_client_to_daemon(
    from: &mut LineReader,
    to: &mut LineWriter,
    outstanding: &AtomicI64,
) -> Stop {
    loop {
        let line = match from.read_line() {
            Ok(Some(l)) => l,
            Ok(None) => return Stop::Closed,
            Err(e) => return frame_stop("stdin", e),
        };
        // Counted before it is sent, so the answer can never come first.
        if has_id(line.as_bytes()) {
            outstanding.fetch_add(1, Ordering::AcqRel);
        }
        if let Err(e) = to.write_line(&line) {
            return Stop::WriteFailed(format!("daemon connection: {e}"));
        }
    }
}

pub fn run(pipe: &str) -> ExitCode {
    let client = match Client::connect_or_start(pipe, None) {
        Ok(c) => c,
        Err(e) => {
            diag(&format!("cannot reach podd on {pipe}: {e}"));
            return ExitCode::from(EXIT_UNREACHABLE);
        }
    };
    let (mut d_read, mut d_write) = client.into_transport().into_parts();
    d_read.set_max_line(REMOTE_MAX_LINE);

    let outstanding: Outstanding = Arc::default();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<Stop>();
    let n = outstanding.clone();
    std::thread::spawn(move || {
        let mut out = LineWriter::new(std::io::stdout());
        let stop = pump_daemon_to_client(&mut d_read, &mut out, &n);
        let _ = done_tx.send(stop);
    });
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Stop>();
    let n = outstanding.clone();
    std::thread::spawn(move || {
        let mut stdin = LineReader::new(std::io::stdin(), REMOTE_MAX_LINE);
        let stop = pump_client_to_daemon(&mut stdin, &mut d_write, &n);
        let _ = in_tx.send(stop);
    });

    // The daemon side ending decides at once; stdin ending first leaves time
    // for the answers to what was already sent.
    let mut drain_until: Option<Instant> = None;
    let stop = loop {
        if let Ok(s) = done_rx.recv_timeout(Duration::from_millis(20)) {
            break s;
        }
        if let Some(t) = drain_until {
            if outstanding.load(Ordering::Acquire) <= 0 || Instant::now() >= t {
                break Stop::Closed;
            }
            continue;
        }
        match in_rx.try_recv() {
            Ok(Stop::Closed) => drain_until = Some(Instant::now() + DRAIN_MAX),
            Ok(other) => break other,
            Err(_) => {}
        }
    };
    let _ = std::io::stdout().flush();
    match stop {
        Stop::Closed | Stop::WriteFailed(_) => ExitCode::SUCCESS,
        Stop::Protocol(m) => {
            diag(&m);
            ExitCode::from(EXIT_PROTOCOL)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pipes() -> (LineReader, std::io::PipeWriter) {
        let (r, w) = std::io::pipe().unwrap();
        (LineReader::new(r, 1024), w)
    }

    fn collect(r: std::io::PipeReader) -> Vec<u8> {
        let mut v = Vec::new();
        let mut r = r;
        std::io::Read::read_to_end(&mut r, &mut v).unwrap();
        v
    }

    #[test]
    fn protocol_line_shape() {
        assert!(is_protocol_line(b"{\"jsonrpc\":\"2.0\"}"));
        assert!(is_protocol_line(b" {} \r"));
        assert!(!is_protocol_line(b"Welcome to host"));
        assert!(!is_protocol_line(b"[1,2]"));
        assert!(!is_protocol_line(b""));
    }

    #[test]
    fn daemon_lines_are_forwarded_until_a_non_protocol_line() {
        let (mut from, mut w) = pipes();
        w.write_all(b"{\"a\":1}\n{\"b\":\"\xed\x95\x9c\"}\nbanner text\n{\"c\":3}\n")
            .unwrap();
        drop(w);
        let (out_r, out_w) = std::io::pipe().unwrap();
        let mut out = LineWriter::new(out_w);
        let n = AtomicI64::new(5);
        let stop = pump_daemon_to_client(&mut from, &mut out, &n);
        drop(out);
        assert_eq!(
            n.load(Ordering::Relaxed),
            5,
            "lines without an id are not answers"
        );
        assert!(matches!(stop, Stop::Protocol(m) if m.contains("not a JSON object")));
        // Only the lines before the bad one went out; the bad one did not.
        assert_eq!(collect(out_r), "{\"a\":1}\n{\"b\":\"한\"}\n".as_bytes());
    }

    #[test]
    fn oversized_or_cut_daemon_line_stops_the_bridge() {
        let (mut from, mut w) = pipes();
        std::thread::spawn(move || {
            let _ = w.write_all(&vec![b'{'; 4096]);
        });
        let (_out_r, out_w) = std::io::pipe().unwrap();
        let stop =
            pump_daemon_to_client(&mut from, &mut LineWriter::new(out_w), &AtomicI64::new(0));
        assert!(matches!(stop, Stop::Protocol(m) if m.contains("limit")));

        let (mut from, mut w) = pipes();
        w.write_all(b"{\"a\":").unwrap();
        drop(w);
        let (_out_r, out_w) = std::io::pipe().unwrap();
        let stop =
            pump_daemon_to_client(&mut from, &mut LineWriter::new(out_w), &AtomicI64::new(0));
        assert!(matches!(stop, Stop::Protocol(m) if m.contains("middle of a line")));
    }

    #[test]
    fn client_lines_go_to_the_daemon_and_bad_utf8_stops() {
        let (mut from, mut w) = pipes();
        w.write_all(b"{\"id\":1}\n\xff\n{\"id\":2}\n").unwrap();
        drop(w);
        let (d_r, d_w) = std::io::pipe().unwrap();
        let stop = pump_client_to_daemon(&mut from, &mut LineWriter::new(d_w), &AtomicI64::new(0));
        assert!(matches!(stop, Stop::Protocol(m) if m.contains("UTF-8")));
        assert_eq!(collect(d_r), b"{\"id\":1}\n");

        let (mut from, mut w) = pipes();
        w.write_all(b"{\"id\":1}\n").unwrap();
        drop(w);
        let (_d_r, d_w) = std::io::pipe().unwrap();
        assert_eq!(
            pump_client_to_daemon(&mut from, &mut LineWriter::new(d_w), &AtomicI64::new(0)),
            Stop::Closed
        );
    }

    #[test]
    fn requests_and_answers_are_counted_by_id() {
        let n = AtomicI64::new(0);
        let (mut from, mut w) = pipes();
        w.write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"a\"}\n\
              {\"jsonrpc\":\"2.0\",\"method\":\"note\"}\n\
              {\"id\":null,\"method\":\"b\"}\n\
              {\"method\":\"c\",\"params\":{\"id\":5},\"id\":\"x\"}\n",
        )
        .unwrap();
        drop(w);
        let (_d_r, d_w) = std::io::pipe().unwrap();
        let mut to = LineWriter::new(d_w);
        // The reader would block on a full pipe; 4 short lines fit.
        assert_eq!(pump_client_to_daemon(&mut from, &mut to, &n), Stop::Closed);
        assert_eq!(n.load(Ordering::Relaxed), 2);

        let (mut from, mut w) = pipes();
        w.write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\
              {\"jsonrpc\":\"2.0\",\"method\":\"terminal.output\",\"params\":{\"id\":\"t1\"}}\n\
              {\"jsonrpc\":\"2.0\",\"id\":\"x\",\"error\":{\"code\":1,\"message\":\"m\"}}\n",
        )
        .unwrap();
        drop(w);
        let (_o_r, o_w) = std::io::pipe().unwrap();
        assert_eq!(
            pump_daemon_to_client(&mut from, &mut LineWriter::new(o_w), &n),
            Stop::Closed
        );
        assert_eq!(n.load(Ordering::Relaxed), 0);
    }
}
