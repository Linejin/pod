//! Remote files and review diff (protocol v22, docs/17 §2.4 and R5, docs/22
//! TM-15/TM-16): `fs.stat`, `fs.list`, `fs.read`, `fs.write`, `review.diff`,
//! `review.blob` on a task's worktree.
//!
//! - `path`: the relative path as text (escapes, streams, devices, `.git`).
//! - `held`: the OS side (no link followed, folders held, aliases and hard
//!   links refused, handle-relative replace).
//! - `journal`: `fs.write` operation records (at most once, crash
//!   recovery).
//! - `review`: git for the review diff, no program from the repository.
//!
//! The work is blocking file I/O and runs on the blocking pool; saves of one
//! file run one at a time.

mod held;
mod journal;
mod path;
mod review;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::tasks::blocking;
use super::{Daemon, log};
use crate::protocol::*;
use held::{Access, Fault, Held};
use journal::{Journal, Phase, Record};
use path::Rel;

/// Name prefix of the temporary file a save writes next to its target
/// (never listed).
pub(crate) const TEMP_PREFIX: &str = ".~pod-save-";

/// Daemon state of the file methods.
pub struct Files {
    journal: Mutex<Journal>,
    /// Saves of one file (task + lower-case path) run one at a time.
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

impl Files {
    /// Load the journal under `state_dir` and settle every save a crash
    /// left `prepared` (docs/22 TM-16: before any other save of the file).
    pub fn load(state_dir: &Path) -> Files {
        let mut j = Journal::load(&state_dir.join("files").join("ops"), crate::now_ms());
        for r in j.prepared() {
            match settle(&mut j, &r) {
                Ok(()) => {}
                Err(e) => log(&format!(
                    "fs.write {}: crashed save of {} not settled yet: {e}",
                    r.operation_id, r.path
                )),
            }
        }
        j.prune(crate::now_ms());
        Files {
            journal: Mutex::new(j),
            locks: Mutex::new(HashMap::new()),
        }
    }

    fn lock_for(&self, key: String) -> Arc<Mutex<()>> {
        let mut locks = self.locks.lock().unwrap();
        locks.retain(|_, l| Arc::strong_count(l) > 1);
        locks.entry(key).or_default().clone()
    }
}

/// Settle a `prepared` record from the file: its content is the save's →
/// done; else the save did not happen (or was overwritten since) → the
/// record goes. A leftover temporary file is removed either way. `Err`:
/// the file cannot be read now; the record stays and blocks saves of it.
fn settle(j: &mut Journal, r: &Record) -> Result<(), String> {
    let rel = path::parse(&r.path)?;
    let Some((dirs, leaf)) = rel.split_leaf() else {
        j.remove(&r.operation_id);
        return Ok(());
    };
    let held = match Held::open(Path::new(&r.root), dirs) {
        Ok(h) => h,
        Err(Fault::Busy) => return Err("busy".into()),
        Err(_) => {
            // The worktree or folder is gone: nothing left to settle.
            j.remove(&r.operation_id);
            return Ok(());
        }
    };
    held.remove_temp(&r.tmp);
    let now = match held.file(leaf, Access::Read) {
        Ok((mut f, facts)) => match held::read_all(&mut f, &facts, u64::MAX) {
            Ok(b) => Some(review::revision(&b)),
            Err(Fault::Busy) => return Err("busy".into()),
            Err(_) => None,
        },
        Err(Fault::Busy) => return Err("busy".into()),
        Err(_) => None,
    };
    if now.as_deref() == Some(r.body.as_str()) {
        let mut done = r.clone();
        done.phase = Phase::Done;
        j.put(done).map_err(|e| e.to_string())?;
        log(&format!(
            "fs.write {}: settled a crashed save of {} as written",
            r.operation_id, r.path
        ));
    } else {
        j.remove(&r.operation_id);
        log(&format!(
            "fs.write {}: settled a crashed save of {} as not written",
            r.operation_id, r.path
        ));
    }
    Ok(())
}

// ───────────────────────────── errors ─────────────────────────────

fn denied(rel: &str, reason: impl Into<String>) -> RpcError {
    let reason = reason.into();
    let mut e = RpcError::new(
        files_codes::PATH_DENIED,
        format!("{rel:?}: refused, {reason}"),
    );
    e.data = Some(json!({ "path": rel, "reason": reason }));
    e
}

fn with_path(code: i64, msg: String, rel: &str) -> RpcError {
    let mut e = RpcError::new(code, msg);
    e.data = Some(json!({ "path": rel }));
    e
}

fn fault_err(f: Fault, rel: &str, limit: u64) -> RpcError {
    match f {
        Fault::Denied(why) => denied(rel, why),
        Fault::NotFound => with_path(files_codes::NOT_FOUND, format!("{rel:?}: not found"), rel),
        Fault::Busy => with_path(
            files_codes::BUSY,
            format!(
                "{rel:?}: another program has it open, or a save of it is being recovered; try again"
            ),
            rel,
        ),
        Fault::Kind(kind) => {
            let mut e = RpcError::new(
                files_codes::NOT_A_FILE,
                format!(
                    "{rel:?}: a {} is in the way",
                    if kind == "dir" { "folder" } else { "file" }
                ),
            );
            e.data = Some(json!({ "path": rel, "kind": kind }));
            e
        }
        Fault::TooLarge(size) => {
            let mut e = RpcError::new(
                files_codes::TOO_LARGE,
                format!("{rel:?}: {size} bytes, more than {limit}"),
            );
            e.data = Some(json!({ "path": rel, "size": size, "limit": limit }));
            e
        }
        Fault::Io(m) => RpcError::new(codes::INTERNAL_ERROR, format!("{rel:?}: {m}")),
    }
}

fn parse_rel(p: &str) -> Result<Rel, RpcError> {
    path::parse(p).map_err(|why| denied(p, why))
}

fn invalid(m: impl Into<String>) -> RpcError {
    RpcError::new(codes::INVALID_PARAMS, m)
}

// ───────────────────────────── the operations ─────────────────────────────

/// What a file read found, with the facts the client shows.
fn read_result(task: &str, rel: &Rel, bytes: Vec<u8>) -> FsReadResult {
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    let body = if bom { &bytes[3..] } else { &bytes[..] };
    let crlf = body.windows(2).filter(|w| w == b"\r\n").count();
    let lf = body.iter().filter(|b| **b == b'\n').count() - crlf;
    FsReadResult {
        task: task.into(),
        path: rel.text(),
        size: bytes.len() as u64,
        revision: review::revision(&bytes),
        bom,
        eol: match (crlf > 0, lf > 0) {
            (false, false) => FsEol::None,
            (false, true) => FsEol::Lf,
            (true, false) => FsEol::Crlf,
            (true, true) => FsEol::Mixed,
        },
        binary: body.iter().take(8192).any(|b| *b == 0),
        utf8: std::str::from_utf8(body).is_ok(),
        data: b64_encode(&bytes),
    }
}

/// The bytes of the plain file `rel` under `root` (held, no link followed).
pub(crate) fn read_file(root: &Path, rel: &Rel, limit: u64) -> Result<Vec<u8>, Fault> {
    let Some((dirs, leaf)) = rel.split_leaf() else {
        return Err(Fault::Kind("dir"));
    };
    let held = Held::open(root, dirs)?;
    let (mut f, facts) = held.file(leaf, Access::Read)?;
    held::read_all(&mut f, &facts, limit)
}

fn stat(task: &str, root: &Path, rel: &Rel) -> Result<FsStatResult, RpcError> {
    let text = rel.text();
    let mut out = FsStatResult {
        task: task.into(),
        path: text.clone(),
        ..Default::default()
    };
    let Some((dirs, leaf)) = rel.split_leaf() else {
        Held::open(root, &[]).map_err(|f| fault_err(f, &text, 0))?;
        out.entry = Some(FsEntry {
            name: String::new(),
            kind: FsKind::Dir,
            ..Default::default()
        });
        return Ok(out);
    };
    let held = match Held::open(root, dirs) {
        Ok(h) => h,
        Err(Fault::NotFound | Fault::Kind(_)) => return Ok(out),
        Err(f) => return Err(fault_err(f, &text, 0)),
    };
    let Some(facts) = held.peek(leaf).map_err(|f| fault_err(f, &text, 0))? else {
        return Ok(out);
    };
    let kind = if facts.is_link() || (!facts.is_dir() && facts.links != 1) {
        FsKind::Link
    } else if facts.is_dir() {
        FsKind::Dir
    } else {
        FsKind::File
    };
    if kind == FsKind::File {
        // Same checks as a read (alias, hard link); facts from the handle.
        match held.file(leaf, Access::Read) {
            Ok((mut f, facts)) => {
                if facts.size <= MAX_READ_BYTES
                    && let Ok(b) = held::read_all(&mut f, &facts, MAX_READ_BYTES)
                {
                    out.revision = Some(review::revision(&b));
                }
            }
            Err(Fault::Busy) => {}
            Err(f) => return Err(fault_err(f, &text, 0)),
        }
    }
    if kind == FsKind::Dir {
        // An alias of a folder (`GIT~1`) is refused like when entering it.
        let mut held = held;
        held.enter(leaf).map_err(|f| fault_err(f, &text, 0))?;
    }
    out.entry = Some(FsEntry {
        name: text,
        kind,
        size: if kind == FsKind::File { facts.size } else { 0 },
        modified_ms: filetime_ms(facts.written),
    });
    Ok(out)
}

/// Windows FILETIME (100 ns since 1601) → Unix ms.
fn filetime_ms(ft: u64) -> u64 {
    (ft / 10_000).saturating_sub(11_644_473_600_000)
}

fn list(task: &str, root: &Path, rel: &Rel, p: &FsListParams) -> Result<FsListResult, RpcError> {
    let text = rel.text();
    let held = Held::open(root, &rel.parts).map_err(|f| fault_err(f, &text, 0))?;
    let mut entries = held.list().map_err(|f| fault_err(f, &text, 0))?;
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    if let Some(c) = &p.cursor {
        entries.retain(|e| e.name.as_str() > c.as_str());
    }
    let limit = p.limit.unwrap_or(LIST_PAGE).clamp(1, LIST_PAGE) as usize;
    let next = (entries.len() > limit).then(|| entries[limit - 1].name.clone());
    entries.truncate(limit);
    Ok(FsListResult {
        task: task.into(),
        path: text,
        entries,
        next,
    })
}

/// Test points of a save (debug builds): `POD_TEST_FILES_CRASH=prepared`
/// ends the daemon right after the journal says `prepared`, `=renamed`
/// right after the file was replaced (before `done`).
fn crash_point(stage: &str) {
    #[cfg(debug_assertions)]
    if std::env::var("POD_TEST_FILES_CRASH").as_deref() == Ok(stage) {
        log(&format!("POD_TEST_FILES_CRASH: exiting at {stage}"));
        std::process::exit(86);
    }
    let _ = stage;
}

struct Save<'a> {
    task: &'a str,
    root: &'a Path,
    rel: &'a Rel,
    data: &'a [u8],
    body: String,
    expected: &'a str,
    op: &'a str,
}

fn conflict(rel: &str, expected: &str, current: Option<&str>, why: &str) -> RpcError {
    let mut e = RpcError::new(
        files_codes::CONFLICT,
        format!("{rel:?}: {why}; not saved (re-read it and save again)"),
    );
    e.data = Some(json!({ "path": rel, "expected": expected, "current": current }));
    e
}

impl Files {
    fn write(&self, s: Save<'_>, hook: &dyn Fn(&str)) -> Result<FsWriteResult, RpcError> {
        let text = s.rel.text();
        let Some((dirs, leaf)) = s.rel.split_leaf() else {
            return Err(fault_err(Fault::Kind("dir"), &text, 0));
        };
        let lock = self.lock_for(format!("{}\n{}", s.task, s.rel.key()));
        let _one = lock.lock().unwrap_or_else(|e| e.into_inner());
        let answer = |outcome| FsWriteResult {
            task: s.task.into(),
            path: text.clone(),
            outcome,
            revision: s.body.clone(),
            size: s.data.len() as u64,
        };
        // 1. The same operation again: answer from the journal.
        {
            let mut j = self.journal.lock().unwrap();
            if let Some(r) = j.get(s.op).cloned() {
                if r.task != s.task || r.path.to_lowercase() != s.rel.key() || r.body != s.body {
                    let mut e = RpcError::new(
                        files_codes::OPERATION_MISMATCH,
                        format!(
                            "operation {} was a save of other content or another file; use a new id",
                            s.op
                        ),
                    );
                    e.data = Some(json!({ "operation_id": s.op }));
                    return Err(e);
                }
                if r.phase == Phase::Prepared {
                    settle(&mut j, &r).map_err(|_| fault_err(Fault::Busy, &text, 0))?;
                }
                if j.get(s.op).is_some_and(|r| r.phase == Phase::Done) {
                    return Ok(answer(FsWriteOutcome::Already));
                }
            }
            // 2. A crashed save of this file is settled before this one.
            for r in j.prepared_for(s.task, &s.rel.key()) {
                settle(&mut j, &r).map_err(|_| fault_err(Fault::Busy, &text, 0))?;
            }
        }
        // 3. Hold the way and the target (no other writer from here on),
        // compare revisions.
        let held = Held::open(s.root, dirs).map_err(|f| fault_err(f, &text, 0))?;
        let (mut target, facts) = match held.file(leaf, Access::Replace) {
            Ok(t) => t,
            Err(Fault::NotFound) => {
                return Err(conflict(&text, s.expected, None, "the file is gone"));
            }
            Err(f) => return Err(fault_err(f, &text, 0)),
        };
        let current = match held::read_all(&mut target, &facts, MAX_READ_BYTES) {
            Ok(b) => review::revision(&b),
            Err(Fault::TooLarge(_)) => {
                return Err(conflict(&text, s.expected, None, "the file grew too large"));
            }
            Err(f) => return Err(fault_err(f, &text, 0)),
        };
        if current != s.expected {
            return Err(conflict(
                &text,
                s.expected,
                Some(&current),
                "changed since it was read",
            ));
        }
        hook("checked");
        // 4. Journal first, then replace, then done.
        let tmp = format!(
            "{TEMP_PREFIX}{}.tmp",
            &review::revision(s.op.as_bytes())[..16]
        );
        let mut rec = Record {
            operation_id: s.op.into(),
            task: s.task.into(),
            root: s.root.display().to_string(),
            path: text.clone(),
            body: s.body.clone(),
            size: s.data.len() as u64,
            expected: s.expected.into(),
            tmp: tmp.clone(),
            phase: Phase::Prepared,
            at_ms: crate::now_ms(),
        };
        self.journal
            .lock()
            .unwrap()
            .put(rec.clone())
            .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("save journal: {e}")))?;
        crash_point("prepared");
        if let Err(f) = held.replace(leaf, &tmp, s.data) {
            self.journal.lock().unwrap().remove(s.op);
            return Err(fault_err(f, &text, 0));
        }
        crash_point("renamed");
        drop(target);
        rec.phase = Phase::Done;
        let mut j = self.journal.lock().unwrap();
        if let Err(e) = j.put(rec) {
            // The file is saved; a resend settles from its content.
            log(&format!("fs.write {}: journal not updated: {e}", s.op));
        }
        j.prune(crate::now_ms());
        Ok(answer(FsWriteOutcome::Written))
    }
}

impl Daemon {
    fn task_root(&self, task: &str) -> Result<(String, PathBuf), RpcError> {
        let rec = self.task_record(task)?;
        Ok((rec.id, PathBuf::from(rec.worktree)))
    }

    pub(super) async fn fs_stat(&self, p: FsPathParams) -> Result<FsStatResult, RpcError> {
        let rel = parse_rel(&p.path)?;
        let (task, root) = self.task_root(&p.task)?;
        blocking(move || stat(&task, &root, &rel)).await?
    }

    pub(super) async fn fs_list(&self, p: FsListParams) -> Result<FsListResult, RpcError> {
        let rel = parse_rel(&p.path)?;
        let (task, root) = self.task_root(&p.task)?;
        blocking(move || list(&task, &root, &rel, &p)).await?
    }

    pub(super) async fn fs_read(&self, p: FsPathParams) -> Result<FsReadResult, RpcError> {
        let rel = parse_rel(&p.path)?;
        let (task, root) = self.task_root(&p.task)?;
        blocking(move || {
            read_file(&root, &rel, MAX_READ_BYTES)
                .map(|b| read_result(&task, &rel, b))
                .map_err(|f| fault_err(f, &rel.text(), MAX_READ_BYTES))
        })
        .await?
    }

    pub(super) async fn fs_write(
        self: Arc<Self>,
        p: FsWriteParams,
    ) -> Result<FsWriteResult, RpcError> {
        if !valid_operation_id(&p.operation_id) {
            return Err(invalid(
                "operation_id: 8-64 characters A-Z, a-z, 0-9, _ or -",
            ));
        }
        if p.expected_revision.len() != 64
            || !p
                .expected_revision
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        {
            return Err(invalid("expected_revision: a revision from fs.read"));
        }
        let rel = parse_rel(&p.path)?;
        let data = b64_decode(&p.data).map_err(|e| invalid(format!("data: {e}")))?;
        if data.len() as u64 > MAX_WRITE_BYTES {
            return Err(fault_err(
                Fault::TooLarge(data.len() as u64),
                &rel.text(),
                MAX_WRITE_BYTES,
            ));
        }
        let (task, root) = self.task_root(&p.task)?;
        let d = self.clone();
        let (t, r) = (task.clone(), rel.clone());
        let res = blocking(move || {
            let body = review::revision(&data);
            d.files.write(
                Save {
                    task: &t,
                    root: &root,
                    rel: &r,
                    data: &data,
                    body,
                    expected: &p.expected_revision,
                    op: &p.operation_id,
                },
                &|_| {},
            )
        })
        .await??;
        if res.outcome == FsWriteOutcome::Written {
            // docs/17 §2.4: a save is a task touch (git facts, `task.changed`).
            let touch = TaskTouchParams {
                task: task.clone(),
                paths: vec![rel.text()],
            };
            if let Err(e) = self.task_touch(touch).await {
                log(&format!("fs.write: task.touch {task}: {}", e.message));
            }
        }
        Ok(res)
    }

    pub(super) async fn review_diff(
        &self,
        p: ReviewDiffParams,
    ) -> Result<ReviewDiffResult, RpcError> {
        let rec = self.task_record(&p.task)?;
        let root = PathBuf::from(&rec.worktree);
        let (base, commit) = (rec.base.clone(), rec.base_commit.clone());
        let l = blocking(move || {
            review::list(&root, &base, &commit, |p| {
                let rel = path::parse(p).ok()?;
                read_file(&root, &rel, MAX_READ_BYTES).ok()
            })
        })
        .await?
        .map_err(|m| RpcError::new(codes::GIT_FAILED, m))?;
        Ok(ReviewDiffResult {
            task: rec.id,
            base: rec.base,
            snapshot: review::snapshot(&l),
            merge_base: l.merge_base,
            head: l.head,
            autocrlf: l.autocrlf,
            files: l.files,
        })
    }

    pub(super) async fn review_blob(
        &self,
        p: ReviewBlobParams,
    ) -> Result<ReviewBlobResult, RpcError> {
        let rel = parse_rel(&p.path)?;
        if rel.parts.is_empty() {
            return Err(fault_err(Fault::Kind("dir"), "", 0));
        }
        let rec = self.task_record(&p.task)?;
        let root = PathBuf::from(&rec.worktree);
        let (base, commit, rev) = (rec.base.clone(), rec.base_commit.clone(), p.rev.clone());
        let r = rel.clone();
        let blob = blocking(move || {
            if !rev.is_empty() {
                let mb = crate::git::merge_base(&root, &base, &commit);
                if mb != rev {
                    return Ok(Err(mb));
                }
            }
            review::blob(&root, &format!("{rev}:{}", r.text()), MAX_READ_BYTES).map(Ok)
        })
        .await?
        .map_err(|m| RpcError::new(codes::GIT_FAILED, m))?;
        let text = rel.text();
        let mut out = ReviewBlobResult {
            task: rec.id,
            rev: p.rev.clone(),
            path: text.clone(),
            ..Default::default()
        };
        match blob {
            Err(merge_base) => {
                let mut e = RpcError::new(
                    files_codes::STALE_DIFF,
                    format!(
                        "{:?} is not the task's merge base any more ({merge_base}); list again",
                        p.rev
                    ),
                );
                e.data = Some(json!({ "rev": p.rev, "merge_base": merge_base }));
                Err(e)
            }
            Ok(review::Blob::Missing) => Ok(out),
            Ok(review::Blob::TooLarge(n)) => {
                Err(fault_err(Fault::TooLarge(n), &text, MAX_READ_BYTES))
            }
            Ok(review::Blob::Bytes(b)) => {
                out.size = b.len() as u64;
                out.data = Some(b64_encode(&b));
                Ok(out)
            }
        }
    }
}

#[cfg(test)]
mod tests;
