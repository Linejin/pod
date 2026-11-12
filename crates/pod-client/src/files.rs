//! Remote file calls (protocol v22, docs/17 §2.4 and R5): what a client
//! keeps for a save so that a lost answer is resent safely, and how it
//! reads the daemon's answers. The calls themselves are ordinary
//! [`Client::call`](crate::client::Client::call)s of `fs.*` / `review.*`
//! (or a remote session's `call`).
//!
//! A save is a [`PendingSave`]: its `operation_id` is made once and the
//! **same** params are sent again after a lost answer, so the daemon writes
//! it at most once ([`FsWriteOutcome::Already`]). Its bytes are kept until
//! an answer arrives; a disconnect never drops them. After
//! [`OPERATION_KEEP_MS`] the daemon may have forgotten the id, so a save
//! whose answer is still unknown then is not resent (re-read and compare).

use std::time::{Duration, SystemTime};

use crate::protocol::*;

/// A fresh `fs.write` operation id: 32 hex characters, unique per save
/// (time, process, a counter and the std hasher's random keys; ids need to
/// be unique, not secret).
pub fn new_operation_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let half = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.write_u32(std::process::id());
        h.write_u64(seq);
        h.write_u64(salt);
        h.finish()
    };
    format!("{:016x}{:016x}", half(1), half(2))
}

/// The margin kept from [`OPERATION_KEEP_MS`] before a resend is no longer
/// safe (clock skew, a slow link).
pub const RESEND_MARGIN: Duration = Duration::from_secs(10 * 60);

/// One save as sent: the params (with their operation id) and when it was
/// first sent.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingSave {
    params: FsWriteParams,
    first_sent: SystemTime,
}

impl PendingSave {
    /// A save of `bytes` over `path` of `task`, read at `expected_revision`.
    pub fn new(task: &str, path: &str, bytes: &[u8], expected_revision: &str) -> Self {
        PendingSave {
            params: FsWriteParams {
                task: task.into(),
                path: path.into(),
                data: b64_encode(bytes),
                expected_revision: expected_revision.into(),
                operation_id: new_operation_id(),
            },
            first_sent: SystemTime::now(),
        }
    }

    /// What to send: every resend sends exactly this.
    pub fn params(&self) -> &FsWriteParams {
        &self.params
    }

    pub fn operation_id(&self) -> &str {
        &self.params.operation_id
    }

    /// May this save be sent again after its answer was lost? Only while the
    /// daemon still remembers the operation id.
    pub fn may_resend(&self, now: SystemTime) -> bool {
        let keep = Duration::from_millis(OPERATION_KEEP_MS).saturating_sub(RESEND_MARGIN);
        now.duration_since(self.first_sent)
            .map_or(true, |age| age < keep)
    }
}

/// What became of a save.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveOutcome {
    /// Written (`already`: by an earlier send whose answer was lost).
    Saved { revision: String, already: bool },
    /// The file changed (or went) since it was read: nothing written.
    /// Re-read, merge, save again with a new [`PendingSave`].
    Conflict { current: Option<String> },
    /// Refused for good: path, size, not a file, operation mismatch.
    Refused { code: i64, message: String },
    /// Another program has the file open or a crashed save is being
    /// settled: send the same save again later.
    Busy,
    /// No answer (disconnected, lost): send the same save again once
    /// connected, if [`PendingSave::may_resend`].
    Unknown,
}

/// Classify an `fs.write` answer: `Err(None)` is a transport failure (the
/// outcome is unknown).
pub fn save_outcome(res: Result<FsWriteResult, Option<&RpcError>>) -> SaveOutcome {
    match res {
        Ok(r) => SaveOutcome::Saved {
            already: r.outcome == FsWriteOutcome::Already,
            revision: r.revision,
        },
        Err(None) => SaveOutcome::Unknown,
        Err(Some(e)) => match e.code {
            files_codes::CONFLICT => SaveOutcome::Conflict {
                current: e
                    .data
                    .as_ref()
                    .and_then(|d| d.get("current"))
                    .and_then(|c| c.as_str())
                    .map(str::to_string),
            },
            files_codes::BUSY => SaveOutcome::Busy,
            code => SaveOutcome::Refused {
                code,
                message: e.message.clone(),
            },
        },
    }
}

/// The bytes of an `fs.read` answer, checked against its size.
pub fn read_bytes(r: &FsReadResult) -> Result<Vec<u8>, String> {
    let b = b64_decode(&r.data)?;
    if b.len() as u64 != r.size {
        return Err(format!(
            "{}: {} bytes, the daemon said {}",
            r.path,
            b.len(),
            r.size
        ));
    }
    Ok(b)
}

/// The bytes of a `review.blob` answer (`None`: no such path at `rev`).
pub fn blob_bytes(r: &ReviewBlobResult) -> Result<Option<Vec<u8>>, String> {
    r.data.as_deref().map(b64_decode).transpose()
}

/// Does this hello allow the file and review diff methods?
pub fn has_files(h: &HelloResult) -> bool {
    h.protocol >= FILES_PROTOCOL
        && [capability::REMOTE_FILES, capability::REMOTE_DIFF]
            .iter()
            .all(|c| h.capabilities.iter().any(|x| x == c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_are_valid_and_distinct() {
        let a = new_operation_id();
        let b = new_operation_id();
        assert!(valid_operation_id(&a), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn a_pending_save_resends_the_same_params_within_the_window() {
        let s = PendingSave::new(
            "r/t",
            "한글 a.txt",
            "\u{feff}x\r\n".as_bytes(),
            &"0".repeat(64),
        );
        let again = s.clone();
        assert_eq!(s.params(), again.params(), "a resend is the same request");
        assert_eq!(
            b64_decode(&s.params().data).unwrap(),
            "\u{feff}x\r\n".as_bytes()
        );
        let now = SystemTime::now();
        assert!(s.may_resend(now));
        let late = now + Duration::from_millis(OPERATION_KEEP_MS);
        assert!(!s.may_resend(late), "the daemon may have forgotten it");
        assert_ne!(
            PendingSave::new("r/t", "a", b"x", "e").operation_id(),
            s.operation_id()
        );
    }

    #[test]
    fn answers_classify() {
        let ok = FsWriteResult {
            outcome: FsWriteOutcome::Already,
            revision: "r".into(),
            ..Default::default()
        };
        assert_eq!(
            save_outcome(Ok(ok)),
            SaveOutcome::Saved {
                revision: "r".into(),
                already: true
            }
        );
        assert_eq!(save_outcome(Err(None)), SaveOutcome::Unknown);
        let mut e = RpcError::new(files_codes::CONFLICT, "x");
        e.data = Some(serde_json::json!({"current": "abc", "expected": "e"}));
        assert_eq!(
            save_outcome(Err(Some(&e))),
            SaveOutcome::Conflict {
                current: Some("abc".into())
            }
        );
        e.data = Some(serde_json::json!({"current": null}));
        assert_eq!(
            save_outcome(Err(Some(&e))),
            SaveOutcome::Conflict { current: None }
        );
        let busy = RpcError::new(files_codes::BUSY, "b");
        assert_eq!(save_outcome(Err(Some(&busy))), SaveOutcome::Busy);
        let denied = RpcError::new(files_codes::PATH_DENIED, "d");
        assert!(matches!(
            save_outcome(Err(Some(&denied))),
            SaveOutcome::Refused {
                code: files_codes::PATH_DENIED,
                ..
            }
        ));
    }

    #[test]
    fn read_answers_decode_and_check_their_size() {
        let r = FsReadResult {
            data: b64_encode(b"ab"),
            size: 2,
            ..Default::default()
        };
        assert_eq!(read_bytes(&r).unwrap(), b"ab");
        let bad = FsReadResult { size: 3, ..r };
        assert!(read_bytes(&bad).is_err());
        let none = ReviewBlobResult::default();
        assert_eq!(blob_bytes(&none).unwrap(), None);
    }
}
