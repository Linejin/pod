//! ssh's stderr as diagnostics: a bounded, sanitized line buffer filled by
//! a reader thread, and the classification of a failed handshake into an
//! [`ErrorKind`] from the client error, those lines and the exit code.

use std::io::Read;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use podd::client::ClientError;

use super::ErrorKind;

/// stderr kept for diagnostics: lines, characters per line.
const DIAG_LINES: usize = 20;
const DIAG_CHARS: usize = 300;

/// Classify a failed handshake from the client error, ssh's stderr and exit code.
pub fn classify(err: Option<&ClientError>, stderr: &str, exit: Option<i32>) -> ErrorKind {
    if err.is_some_and(ClientError::is_incompatible) {
        return ErrorKind::VersionMismatch;
    }
    let has = |needle: &str| stderr.contains(needle);
    let lower = stderr.to_ascii_lowercase();
    if has("REMOTE HOST IDENTIFICATION HAS CHANGED")
        || (lower.contains("host key for") && lower.contains("has changed"))
    {
        return ErrorKind::HostKeyChanged;
    }
    if has("Host key verification failed")
        || lower.contains("host key is known for")
        || lower.contains("no matching host key")
    {
        return ErrorKind::HostKeyUnknown;
    }
    if has("Permission denied (")
        || has("Too many authentication failures")
        || has("UNPROTECTED PRIVATE KEY FILE")
        || lower.contains("bad permissions")
        || lower.contains("no such identity")
    {
        return ErrorKind::AuthFailed;
    }
    if lower.contains("timed out") {
        return ErrorKind::Timeout;
    }
    if has("Could not resolve hostname")
        || has("Connection refused")
        || has("No route to host")
        || has("Network is unreachable")
        || has("kex_exchange_identification")
        || has("Connection reset")
    {
        return ErrorKind::Unreachable;
    }
    if lower.contains("pod: command not found")
        || lower.contains("pod: not found")
        || lower.contains("'pod' is not recognized")
        || lower.contains("term 'pod' is not recognized")
        || exit == Some(127)
    {
        return ErrorKind::RemotePodMissing;
    }
    // An older pod without `remote-stdio` (clap usage error).
    if lower.contains("unrecognized subcommand") && lower.contains("remote-stdio") {
        return ErrorKind::VersionMismatch;
    }
    match err {
        Some(ClientError::Protocol(m)) if !is_closed_message(m) => ErrorKind::StdoutPolluted,
        _ => ErrorKind::Failed,
    }
}

fn is_closed_message(m: &str) -> bool {
    m == "daemon closed the connection"
}

/// The client error means ssh closed the connection (stdout ended).
pub(super) fn is_closed(e: &ClientError) -> bool {
    match e {
        ClientError::Io(_) | ClientError::NotRunning => true,
        ClientError::Protocol(m) => is_closed_message(m),
        ClientError::Rpc(_) => false,
    }
}

/// Bounded stderr collector: the first [`DIAG_LINES`] lines, each cut to
/// [`DIAG_CHARS`] characters with control characters blanked. The rest is
/// read and dropped so ssh never blocks on a full pipe.
#[derive(Default)]
pub(super) struct DiagBuf {
    lines: Vec<String>,
    done: bool,
}

pub(super) type Diag = Arc<(Mutex<DiagBuf>, Condvar)>;

fn push_line(buf: &mut DiagBuf, raw: &[u8]) {
    if buf.lines.len() >= DIAG_LINES {
        return;
    }
    let line: String = String::from_utf8_lossy(raw)
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(DIAG_CHARS)
        .collect();
    let line = line.trim();
    if !line.is_empty() {
        buf.lines.push(line.to_string());
    }
}

pub(super) fn spawn_diag(mut r: impl Read + Send + 'static) -> Diag {
    let diag: Diag = Arc::default();
    let d = diag.clone();
    std::thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        let mut cur: Vec<u8> = Vec::new();
        loop {
            let n = match r.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &chunk[..n] {
                if b == b'\n' {
                    push_line(&mut d.0.lock().unwrap(), &cur);
                    cur.clear();
                } else if cur.len() < DIAG_CHARS * 4 {
                    cur.push(b);
                }
            }
        }
        let mut g = d.0.lock().unwrap();
        push_line(&mut g, &cur);
        g.done = true;
        d.1.notify_all();
    });
    diag
}

/// The lines so far, after waiting up to `wait` for stderr to close.
pub(super) fn diag_lines(d: &Diag, wait: Duration) -> Vec<String> {
    let g = d.0.lock().unwrap();
    let (g, _) = d.1.wait_timeout_while(g, wait, |b| !b.done).unwrap();
    g.lines.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        use ErrorKind::*;
        let closed = ClientError::Protocol("daemon closed the connection".into());
        let c = |s: &str, exit| classify(Some(&closed), s, exit);
        assert_eq!(
            c(
                "No ED25519 host key is known for h and you have requested strict checking.\r\nHost key verification failed.",
                Some(255)
            ),
            HostKeyUnknown
        );
        assert_eq!(
            c(
                "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.",
                Some(255)
            ),
            HostKeyChanged
        );
        assert_eq!(
            c("alice@h: Permission denied (publickey).", Some(255)),
            AuthFailed
        );
        assert_eq!(c("Load key \"k\": bad permissions", Some(255)), AuthFailed);
        assert_eq!(
            c(
                "ssh: connect to host h port 22: Connection timed out",
                Some(255)
            ),
            Timeout
        );
        assert_eq!(
            c(
                "ssh: connect to host h port 22: Connection refused",
                Some(255)
            ),
            Unreachable
        );
        assert_eq!(
            c(
                "ssh: Could not resolve hostname h: No such host is known.",
                Some(255)
            ),
            Unreachable
        );
        assert_eq!(
            c("bash: line 1: pod: command not found", Some(127)),
            RemotePodMissing
        );
        assert_eq!(
            c(
                "'pod' is not recognized as an internal or external command,\r\noperable program or batch file.",
                Some(1)
            ),
            RemotePodMissing
        );
        assert_eq!(
            c(
                "pod : The term 'pod' is not recognized as the name of a cmdlet",
                Some(1)
            ),
            RemotePodMissing
        );
        assert_eq!(c("", Some(127)), RemotePodMissing);
        assert_eq!(
            c("error: unrecognized subcommand 'remote-stdio'", Some(2)),
            VersionMismatch
        );
        assert_eq!(c("", Some(1)), Failed);
        let banner = ClientError::Protocol(
            "non-protocol output on the daemon connection: \"Welcome\"".into(),
        );
        assert_eq!(classify(Some(&banner), "", None), StdoutPolluted);
        let v11 = ClientError::Rpc(podd::protocol::RpcError::new(
            podd::protocol::remote_codes::PROTOCOL_MISMATCH,
            "v11",
        ));
        assert_eq!(classify(Some(&v11), "", Some(0)), VersionMismatch);
        assert_eq!(classify(None, "", None), Failed);
    }

    #[test]
    fn stderr_is_bounded_and_sanitized() {
        let mut data = Vec::new();
        data.extend_from_slice(b"first\x1b[31m red\r\n");
        data.extend_from_slice(&vec![b'x'; 100_000]);
        data.push(b'\n');
        for i in 0..100 {
            data.extend_from_slice(format!("line {i}\n").as_bytes());
        }
        let d = spawn_diag(std::io::Cursor::new(data));
        let lines = diag_lines(&d, Duration::from_secs(5));
        assert_eq!(lines.len(), DIAG_LINES);
        assert_eq!(lines[0], "first [31m red");
        assert_eq!(lines[1].chars().count(), DIAG_CHARS);
        assert_eq!(lines[2], "line 0");
    }
}
