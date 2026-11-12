//! The `fs.write` operation journal (docs/17 §2.4, docs/22 TM-16): one
//! small JSON record per `operation_id` under `<state>/files/ops/`, written
//! (and flushed) **before** the file is replaced (`prepared`) and again after
//! (`done`). A resend of the same operation answers from it; a record left
//! `prepared` by a crash is settled from the file's content before any
//! other save of that file. Records live [`OPERATION_KEEP_MS`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::protocol::OPERATION_KEEP_MS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    /// About to replace the file (or crashed while doing it).
    Prepared,
    /// Replaced; `revision` is the content's.
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Record {
    pub operation_id: String,
    pub task: String,
    /// The worktree root the path is under, when prepared.
    pub root: String,
    pub path: String,
    /// SHA-256 of the content (the revision after the save).
    pub body: String,
    pub size: u64,
    pub expected: String,
    /// The temporary file's name in the target's folder.
    pub tmp: String,
    pub phase: Phase,
    pub at_ms: u64,
}

/// The journal: records by operation id, mirrored on disk.
pub(crate) struct Journal {
    dir: PathBuf,
    ops: HashMap<String, Record>,
}

fn file_name(op: &str) -> String {
    // Ids differ in case only on a case-insensitive disk; the hash does not.
    let h = Sha256::digest(op.as_bytes());
    format!("{}.json", hex(&h[..16]))
}

pub(crate) fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Journal {
    /// Load `<dir>` (created when missing), dropping records older than
    /// [`OPERATION_KEEP_MS`] and unreadable ones.
    pub fn load(dir: &Path, now_ms: u64) -> Journal {
        let _ = std::fs::create_dir_all(dir);
        let mut ops = HashMap::new();
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|x| x.to_str()) != Some("json") {
                    // A temp file of an interrupted record write.
                    let _ = std::fs::remove_file(&p);
                    continue;
                }
                let rec: Option<Record> = std::fs::read(&p)
                    .ok()
                    .and_then(|b| serde_json::from_slice(&b).ok());
                match rec {
                    // A prepared record is kept whatever its age: it is
                    // settled from the file before anything else.
                    Some(r)
                        if (r.phase == Phase::Prepared
                            || now_ms.saturating_sub(r.at_ms) < OPERATION_KEEP_MS)
                            && p.file_name().and_then(|n| n.to_str())
                                == Some(file_name(&r.operation_id).as_str()) =>
                    {
                        ops.insert(r.operation_id.clone(), r);
                    }
                    _ => {
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }
        Journal {
            dir: dir.to_path_buf(),
            ops,
        }
    }

    pub fn get(&self, op: &str) -> Option<&Record> {
        self.ops.get(op)
    }

    /// Records still `prepared` for `task` + path key (lower case).
    pub fn prepared_for(&self, task: &str, key: &str) -> Vec<Record> {
        self.ops
            .values()
            .filter(|r| {
                r.phase == Phase::Prepared && r.task == task && r.path.to_lowercase() == key
            })
            .cloned()
            .collect()
    }

    /// Every record still `prepared` (after a crash).
    pub fn prepared(&self) -> Vec<Record> {
        self.ops
            .values()
            .filter(|r| r.phase == Phase::Prepared)
            .cloned()
            .collect()
    }

    /// Write `r` to disk (temp file, flush, rename) and keep it.
    pub fn put(&mut self, r: Record) -> std::io::Result<()> {
        let path = self.dir.join(file_name(&r.operation_id));
        let tmp = path.with_extension("tmp");
        let bytes = serde_json::to_vec(&r).map_err(std::io::Error::other)?;
        let res = (|| {
            use std::io::Write as _;
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &path)
        })();
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return res;
        }
        self.ops.insert(r.operation_id.clone(), r);
        Ok(())
    }

    /// Forget `op` (a prepared save that did not happen).
    pub fn remove(&mut self, op: &str) {
        if self.ops.remove(op).is_some() {
            let _ = std::fs::remove_file(self.dir.join(file_name(op)));
        }
    }

    /// Drop records older than [`OPERATION_KEEP_MS`] (never a prepared one:
    /// it is settled first).
    pub fn prune(&mut self, now_ms: u64) {
        let old: Vec<String> = self
            .ops
            .values()
            .filter(|r| {
                r.phase == Phase::Done && now_ms.saturating_sub(r.at_ms) >= OPERATION_KEEP_MS
            })
            .map(|r| r.operation_id.clone())
            .collect();
        for op in old {
            self.remove(&op);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(op: &str, phase: Phase, at_ms: u64) -> Record {
        Record {
            operation_id: op.into(),
            task: "r/t".into(),
            root: "C:\\wt".into(),
            path: "A.txt".into(),
            body: "b".into(),
            size: 1,
            expected: "e".into(),
            tmp: ".~pod-save-x.tmp".into(),
            phase,
            at_ms,
        }
    }

    #[test]
    fn records_survive_a_reload_and_expire() {
        let dir = std::env::temp_dir().join(format!("podd-journal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut j = Journal::load(&dir, 1000);
        j.put(rec("op-aaaaaaaa", Phase::Prepared, 1000)).unwrap();
        j.put(rec("op-AAAAAAAA", Phase::Done, 1000)).unwrap();
        std::fs::write(dir.join("junk.tmp"), "x").unwrap();
        std::fs::write(dir.join("bad.json"), "{").unwrap();
        let j = Journal::load(&dir, 2000);
        assert_eq!(j.get("op-aaaaaaaa").unwrap().phase, Phase::Prepared);
        assert_eq!(j.get("op-AAAAAAAA").unwrap().phase, Phase::Done);
        assert_eq!(j.prepared_for("r/t", "a.txt").len(), 1);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 2, "junk dropped");
        let mut j = Journal::load(&dir, 1000 + OPERATION_KEEP_MS);
        assert!(j.get("op-AAAAAAAA").is_none(), "expired");
        assert!(
            j.get("op-aaaaaaaa").is_some(),
            "prepared: settled, not expired"
        );
        j.put(rec("op-bbbbbbbb", Phase::Done, 5)).unwrap();
        j.prune(5 + OPERATION_KEEP_MS);
        assert!(j.get("op-bbbbbbbb").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
