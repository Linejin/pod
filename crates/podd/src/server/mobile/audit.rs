//! Append-only mobile audit log: `<state_dir>\mobile\audit.jsonl`, one JSON
//! object per line. Bounded: past [`MAX_FILE_BYTES`] the file becomes
//! `audit.1.jsonl` (replacing the previous one), so at most two files.
//!
//! Message text is recorded as length + SHA-256 unless the user turns on
//! `audit_message_bodies` (`pod mobile settings`): the text already lives in
//! the delivery record (`review.deliveries`, which the user can delete with
//! the task's history), and an audit log is kept longer and copied around
//! more often, so it should not become a second copy of what was said. The
//! hash still proves which text a device sent when compared with the
//! delivery. Gate decisions are additionally committed with the gate itself
//! (`orch` store `gate_ops`); this file is the readable, derived log.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::protocol::MobileAuditEntry;

pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
pub const MAX_READ: usize = 1000;

pub struct AuditLog {
    dir: PathBuf,
    max_bytes: u64,
    lock: Mutex<()>,
}

impl AuditLog {
    pub fn new(state_dir: &Path) -> Self {
        Self::with_limit(state_dir, MAX_FILE_BYTES)
    }

    pub fn with_limit(state_dir: &Path, max_bytes: u64) -> Self {
        Self {
            dir: state_dir.join("mobile"),
            max_bytes,
            lock: Mutex::new(()),
        }
    }

    fn current(&self) -> PathBuf {
        self.dir.join("audit.jsonl")
    }

    fn rotated(&self) -> PathBuf {
        self.dir.join("audit.1.jsonl")
    }

    /// Append one entry (flushed to disk before returning).
    pub fn append(&self, e: &MobileAuditEntry) -> std::io::Result<()> {
        let _g = self.lock.lock().unwrap();
        std::fs::create_dir_all(&self.dir)?;
        let path = self.current();
        if std::fs::metadata(&path).is_ok_and(|m| m.len() >= self.max_bytes) {
            std::fs::rename(&path, self.rotated())?;
        }
        let mut line = serde_json::to_vec(e)?;
        line.push(b'\n');
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&path)?;
        // A line torn by a crash must not swallow this one.
        let len = f.metadata()?.len();
        if len > 0 {
            use std::io::{Read, Seek, SeekFrom};
            let mut last = [0u8];
            f.seek(SeekFrom::Start(len - 1))?;
            f.read_exact(&mut last)?;
            if last[0] != b'\n' {
                line.insert(0, b'\n');
            }
        }
        f.write_all(&line)?;
        f.sync_data()
    }

    /// Newest `limit` entries (oldest first), optionally of one actor.
    /// Unreadable lines are skipped.
    pub fn read(&self, limit: usize, actor: Option<&str>) -> Vec<MobileAuditEntry> {
        let _g = self.lock.lock().unwrap();
        let mut out: Vec<MobileAuditEntry> = Vec::new();
        for p in [self.rotated(), self.current()] {
            let Ok(f) = std::fs::File::open(&p) else {
                continue;
            };
            for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
                if let Ok(e) = serde_json::from_str::<MobileAuditEntry>(&line)
                    && actor.is_none_or(|a| e.actor == a)
                {
                    out.push(e);
                }
            }
        }
        let skip = out.len().saturating_sub(limit.min(MAX_READ));
        out.drain(..skip);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(i: u64, actor: &str) -> MobileAuditEntry {
        MobileAuditEntry {
            ts_ms: i,
            actor: actor.into(),
            action: "message".into(),
            result: "ok".into(),
            ..Default::default()
        }
    }

    #[test]
    fn appends_rotates_and_survives_reopen() {
        let dir = std::env::temp_dir().join(format!(
            "pod-mobile-audit-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let log = AuditLog::with_limit(&dir, 2_000);
        for i in 0..100 {
            log.append(&entry(i, if i % 2 == 0 { "m1" } else { "m2" }))
                .unwrap();
        }
        let size = |p: PathBuf| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        assert!(size(log.current()) <= 2_000 + 200);
        assert!(size(log.rotated()) > 0, "rotated");
        let total = size(log.current()) + size(log.rotated());
        assert!(total < 4_500, "bounded: {total}");
        // A new instance (daemon restart) reads what is on disk.
        let again = AuditLog::with_limit(&dir, 2_000);
        let all = again.read(1_000, None);
        assert_eq!(all.last().unwrap().ts_ms, 99, "newest kept");
        assert!(all.windows(2).all(|w| w[0].ts_ms < w[1].ts_ms), "in order");
        let m2 = again.read(3, Some("m2"));
        assert_eq!(
            m2.iter().map(|e| e.ts_ms).collect::<Vec<_>>(),
            vec![95, 97, 99]
        );
        // A torn last line does not hide the rest.
        std::fs::OpenOptions::new()
            .append(true)
            .open(again.current())
            .unwrap()
            .write_all(b"{\"ts_ms\":")
            .unwrap();
        again.append(&entry(100, "m1")).unwrap();
        assert_eq!(again.read(1, None)[0].ts_ms, 100);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
