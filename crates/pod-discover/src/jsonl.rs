//! Bounded, read-only access to JSON Lines files: the head, the tail and a
//! line count by marker. Files are opened for reading with full sharing so a
//! running agent can keep appending (and even rename or delete) them.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;

use crate::Budget;

/// Open for reading, sharing read/write/delete with the agent that owns it.
pub fn open_shared(path: &Path) -> std::io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
        o.share_mode(0x1 | 0x2 | 0x4);
    }
    o.open(path)
}

/// Split bytes into lines (LF or CRLF), lossily decoded. `drop_first` /
/// `drop_last` remove a partial line cut by a byte window.
fn split_lines(buf: &[u8], drop_first: bool, drop_last: bool) -> Vec<String> {
    let mut parts: Vec<&[u8]> = buf.split(|b| *b == b'\n').collect();
    if drop_last || buf.last() == Some(&b'\n') {
        parts.pop();
    }
    if drop_first && !parts.is_empty() {
        parts.remove(0);
    }
    parts
        .into_iter()
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .filter(|l| !l.is_empty())
        .map(|l| String::from_utf8_lossy(l).into_owned())
        .collect()
}

/// The first `max_lines` complete lines within the first `max_bytes`.
pub fn head(
    path: &Path,
    size: u64,
    max_bytes: u64,
    max_lines: usize,
    budget: &mut Budget,
) -> Option<Vec<String>> {
    let n = size.min(max_bytes);
    if !budget.take(n) {
        return None;
    }
    let mut buf = Vec::with_capacity(n as usize);
    open_shared(path).ok()?.take(n).read_to_end(&mut buf).ok()?;
    // A line cut by the window is incomplete unless the whole file fit.
    let cut = (buf.len() as u64) < size;
    let mut lines = split_lines(&buf, false, cut);
    lines.truncate(max_lines);
    Some(lines)
}

/// Complete lines within the last `max_bytes` of the file.
pub fn tail(path: &Path, size: u64, max_bytes: u64, budget: &mut Budget) -> Option<Vec<String>> {
    let n = size.min(max_bytes);
    if !budget.take(n) {
        return None;
    }
    let mut f = open_shared(path).ok()?;
    let start = size - n;
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity(n as usize);
    f.take(n).read_to_end(&mut buf).ok()?;
    // The file may have grown since `size` was taken: the last line can be partial.
    let partial_last = buf.last() != Some(&b'\n');
    Some(split_lines(
        &buf,
        start > 0,
        partial_last && start + (buf.len() as u64) < size,
    ))
}

/// Lines containing any of `markers`, counted over the whole file in
/// chunks. `None` if the file is larger than `max_bytes` or the budget is
/// spent.
pub fn count_lines_with(
    path: &Path,
    size: u64,
    max_bytes: u64,
    markers: &[&[u8]],
    budget: &mut Budget,
) -> Option<u64> {
    if size > max_bytes || !budget.take(size) {
        return None;
    }
    let mut f = open_shared(path).ok()?.take(size);
    let mut count = 0u64;
    let mut carry: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; 256 * 1024];
    loop {
        let k = f.read(&mut chunk).ok()?;
        if k == 0 {
            break;
        }
        carry.extend_from_slice(&chunk[..k]);
        let last_nl = carry.iter().rposition(|b| *b == b'\n');
        if let Some(end) = last_nl {
            count += carry[..end]
                .split(|b| *b == b'\n')
                .filter(|l| contains_any(l, markers))
                .count() as u64;
            carry.drain(..=end);
        }
        if budget.out_of_time() {
            return None;
        }
    }
    if !carry.is_empty() && contains_any(&carry, markers) {
        count += 1;
    }
    Some(count)
}

fn contains_any(line: &[u8], markers: &[&[u8]]) -> bool {
    markers
        .iter()
        .any(|m| !m.is_empty() && line.windows(m.len()).any(|w| w == *m))
}

/// Parse a line as a JSON object; anything else is `None`.
pub fn object(line: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(line.trim_start_matches('\u{feff}')).ok()?;
    v.is_object().then_some(v)
}

/// A non-empty string at a JSON pointer.
pub fn str_at<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Limits;

    fn tmp(name: &str, body: &[u8]) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pod-discover-jsonl-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.jsonl");
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn head_drops_cut_line_and_tail_drops_partial_first() {
        let p = tmp("ht", b"{\"a\":1}\r\n{\"b\":2}\n{\"c\":3}\n");
        let size = std::fs::metadata(&p).unwrap().len();
        let mut b = Budget::new(&Limits::default());
        assert_eq!(
            head(&p, size, 1000, 10, &mut b).unwrap(),
            ["{\"a\":1}", "{\"b\":2}", "{\"c\":3}"]
        );
        assert_eq!(head(&p, size, 12, 10, &mut b).unwrap(), ["{\"a\":1}"]);
        assert_eq!(head(&p, size, 1000, 2, &mut b).unwrap().len(), 2);
        assert_eq!(tail(&p, size, 12, &mut b).unwrap(), ["{\"c\":3}"]);
        assert_eq!(tail(&p, size, 1000, &mut b).unwrap().len(), 3);
    }

    #[test]
    fn count_and_budget() {
        let mut body = Vec::new();
        for i in 0..5000 {
            body.extend_from_slice(if i % 2 == 0 {
                b"{\"type\":\"user\"}\n"
            } else {
                b"{\"type\":\"x\"}\n"
            });
        }
        body.extend_from_slice(b"{\"type\":\"user\"}");
        let p = tmp("count", &body);
        let size = body.len() as u64;
        let mut b = Budget::new(&Limits::default());
        assert_eq!(
            count_lines_with(&p, size, u64::MAX, &[b"\"user\""], &mut b),
            Some(2501)
        );
        assert_eq!(count_lines_with(&p, size, 10, &[b"\"user\""], &mut b), None);
        let mut small = Budget::new(&Limits {
            read_bytes: 10,
            ..Limits::default()
        });
        assert_eq!(head(&p, size, 100, 10, &mut small), None);
        assert!(small.exhausted);
    }

    #[test]
    fn invalid_utf8_and_missing_files_do_not_panic() {
        let p = tmp("utf8", b"\xff\xfe{\"a\n\x00\x00\n");
        let mut b = Budget::new(&Limits::default());
        let lines = head(&p, 100, 100, 10, &mut b).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| object(l).is_none()));
        assert!(
            head(
                Path::new(r"C:\definitely\not\here.jsonl"),
                10,
                10,
                10,
                &mut b
            )
            .is_none()
        );
    }
}
