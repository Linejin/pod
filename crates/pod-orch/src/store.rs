//! Files of the orchestration state (docs/19 §4.6):
//!
//! ```text
//! <state>\orch\runs\index.json        id counters + request_id results
//! <state>\orch\runs\<run>\run.json    run, items, attempts, wake state
//! <state>\orch\runs\<run>\mail.jsonl  append-only messages and read receipts
//! ```
//!
//! JSON snapshots are written to a temporary file and renamed (like the task
//! registry). `mail.jsonl` gets one line per message or read receipt, flushed
//! without fsync; a torn last line (crash mid-write) is dropped on load and
//! cut off the file, so the next append starts on a clean line.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::model::MailLine;

/// Write `value` as pretty JSON through a temporary file + rename.
pub fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Read JSON; a file that does not parse is moved aside
/// (`<name>.corrupt-<ms>.json`) and `None` is returned with a note.
pub fn read_json<T: DeserializeOwned>(
    path: &Path,
    now_ms: u64,
    notes: &mut Vec<String>,
) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(e) => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
            let aside = path.with_file_name(format!("{stem}.corrupt-{now_ms}.json"));
            let _ = std::fs::rename(path, &aside);
            notes.push(format!(
                "{} is corrupt ({e}); moved to {}",
                path.display(),
                aside.display()
            ));
            None
        }
    }
}

/// What loading a mail log found.
#[derive(Debug, Default, PartialEq)]
pub struct MailLoad {
    pub lines: Vec<MailLine>,
    /// Bytes of a torn last line that were cut off the file.
    pub truncated_bytes: usize,
    /// Unreadable lines in the middle (kept in the file, skipped).
    pub skipped_lines: usize,
}

/// Load `mail.jsonl`. A last line that does not parse is a torn write: it is
/// dropped and the file is truncated to the last good line.
pub fn load_mail(path: &Path) -> std::io::Result<MailLoad> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(MailLoad::default()),
        Err(e) => return Err(e),
    };
    let mut out = MailLoad::default();
    // (start, end) of each line, end excluding the newline.
    let mut spans = Vec::new();
    let mut start = 0;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            spans.push((start, i));
            start = i + 1;
        }
    }
    if start < bytes.len() {
        spans.push((start, bytes.len()));
    }
    let n = spans.len();
    let mut good_end = 0; // byte offset after the last good line (incl. newline)
    for (k, (s, e)) in spans.into_iter().enumerate() {
        let line = &bytes[s..e];
        let trimmed = line.strip_suffix(b"\r").unwrap_or(line);
        if trimmed.iter().all(|c| c.is_ascii_whitespace()) {
            if e < bytes.len() {
                good_end = e + 1;
            }
            continue;
        }
        let terminated = e < bytes.len();
        match serde_json::from_slice::<MailLine>(trimmed) {
            // Only the last line can lack its newline.
            Ok(l) => {
                out.lines.push(l);
                good_end = if terminated { e + 1 } else { e };
            }
            Err(_) if k + 1 == n => {
                out.truncated_bytes = bytes.len() - good_end;
            }
            Err(_) => {
                out.skipped_lines += 1;
                good_end = e + 1;
            }
        }
    }
    if out.truncated_bytes > 0 {
        let f = std::fs::OpenOptions::new().write(true).open(path)?;
        f.set_len(good_end as u64)?;
    } else if good_end < bytes.len() || (!bytes.is_empty() && !bytes.ends_with(b"\n")) {
        // A good last line without its newline: terminate it so the next
        // append does not run into it.
        let mut f = std::fs::OpenOptions::new().append(true).open(path)?;
        f.write_all(b"\n")?;
    }
    Ok(out)
}

/// Append one line to `mail.jsonl` (created on first use) and flush.
pub fn append_mail(path: &Path, line: &MailLine) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_string(line).map_err(std::io::Error::other)?;
    text.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    // One write call per line: a crash tears at most this line.
    f.write_all(text.as_bytes())?;
    f.flush()
}

pub fn run_dir(root: &Path, run: &str) -> PathBuf {
    root.join(run)
}

pub fn run_json(root: &Path, run: &str) -> PathBuf {
    run_dir(root, run).join("run.json")
}

pub fn mail_jsonl(root: &Path, run: &str) -> PathBuf {
    run_dir(root, run).join("mail.jsonl")
}

pub fn index_json(root: &Path) -> PathBuf {
    root.join("index.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    pub(crate) fn temp(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "pod-orch-{tag}-{}-{nanos}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn mail(id: &str) -> MailLine {
        MailLine::Mail {
            mail: Box::new(Mail {
                id: id.into(),
                run: "r1".into(),
                from: "human".into(),
                to: "run:r1".into(),
                recipients: vec!["run:r1".into()],
                kind: MailType::Status,
                subject: format!("s {id}"),
                body: "b\nline — two".into(),
                thread: None,
                reply_to: None,
                payload: None,
                priority: Priority::Normal,
                created_ms: 1,
                fenced: false,
                text: None,
                body_text: None,
                read_ms: None,
            }),
        }
    }

    #[test]
    fn torn_last_line_is_cut_and_appends_continue() {
        let d = temp("torn");
        let p = d.join("mail.jsonl");
        append_mail(&p, &mail("m1")).unwrap();
        append_mail(&p, &mail("m2")).unwrap();
        let good_len = std::fs::metadata(&p).unwrap().len();
        // A crash in the middle of the third write.
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(br#"{"kind":"mail","mail":{"id":"m3","ru"#)
            .unwrap();
        drop(f);
        let l = load_mail(&p).unwrap();
        assert_eq!(l.lines, vec![mail("m1"), mail("m2")]);
        assert!(l.truncated_bytes > 10);
        assert_eq!(std::fs::metadata(&p).unwrap().len(), good_len);
        append_mail(&p, &mail("m3")).unwrap();
        let l = load_mail(&p).unwrap();
        assert_eq!(l.lines.len(), 3);
        assert_eq!(l.truncated_bytes, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn good_last_line_without_newline_is_kept_and_terminated() {
        let d = temp("nonl");
        let p = d.join("mail.jsonl");
        append_mail(&p, &mail("m1")).unwrap();
        let mut text = std::fs::read(&p).unwrap();
        text.pop(); // drop the newline
        std::fs::write(&p, &text).unwrap();
        let l = load_mail(&p).unwrap();
        assert_eq!(l.lines.len(), 1);
        append_mail(&p, &mail("m2")).unwrap();
        assert_eq!(load_mail(&p).unwrap().lines.len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_middle_line_is_skipped_crlf_ok_missing_file_empty() {
        let d = temp("mid");
        let p = d.join("mail.jsonl");
        assert_eq!(load_mail(&p).unwrap(), MailLoad::default());
        append_mail(&p, &mail("m1")).unwrap();
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"garbage\r\n\r\n").unwrap();
        drop(f);
        append_mail(&p, &mail("m2")).unwrap();
        let l = load_mail(&p).unwrap();
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.skipped_lines, 1);
        assert_eq!(l.truncated_bytes, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn corrupt_json_is_moved_aside() {
        let d = temp("json");
        let p = d.join("run.json");
        std::fs::write(&p, "{nope").unwrap();
        let mut notes = Vec::new();
        assert!(read_json::<RunRecord>(&p, 42, &mut notes).is_none());
        assert!(!p.exists());
        assert!(d.join("run.corrupt-42.json").exists());
        assert_eq!(notes.len(), 1);
        write_json(&p, &serde_json::json!({"a": 1})).unwrap();
        let v: serde_json::Value = read_json(&p, 43, &mut notes).unwrap();
        assert_eq!(v["a"], 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}
