//! Protocol v22 remote files and review diff (docs/17 §2.4 and R5, docs/22
//! TM-15/TM-16): the files of a task's worktree, read and saved through the
//! daemon that owns it, so the editor and the review work the same on a
//! remote machine as on this one.
//!
//! - `fs.stat`, `fs.list`, `fs.read`: paths are relative to the task's
//!   worktree (`/`-separated, no `..`, no absolute/UNC/drive/device/stream
//!   names); every folder on the way and the file are opened without
//!   following links and held while used. Links, junctions, other reparse
//!   points and hard links are refused, never followed.
//! - `fs.write`: replaces an existing file in one step, only while it still
//!   has `expected_revision` (else [`files_codes::CONFLICT`] with the
//!   current revision; nothing written). `operation_id` makes a resend of
//!   the same save apply at most once (a daemon journal, kept
//!   [`OPERATION_KEEP_MS`]).
//! - `review.diff` lists a task's changed files against its merge base;
//!   `review.blob` gives one blob of that commit or of the index. git runs
//!   with `--no-ext-diff --no-textconv` and with every configured filter
//!   program blanked; `cat-file` runs no filter or textconv. The client
//!   reads the worktree side with `fs.read` and builds the patch itself
//!   (pod-diff), as it does for a local task.
//!
//! A revision is the lowercase hex SHA-256 of the file's bytes. Contents
//! travel as standard base64. Re-exported from [`crate::protocol`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::Method;

/// The first protocol with the remote file and review diff methods and the
/// `remote.files` / `remote.diff` capabilities.
pub const FILES_PROTOCOL: u32 = 22;

/// Largest file `fs.read` and `review.blob` return (docs/17 §2.4: 4 MiB a
/// file). Bigger: [`files_codes::TOO_LARGE`].
pub const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;
/// Largest content `fs.write` takes: its base64 plus the request envelope
/// fit in the daemon's 4 MiB request line.
pub const MAX_WRITE_BYTES: u64 = 3_000_000;
/// Longest relative path (UTF-8 bytes).
pub const MAX_PATH_BYTES: usize = 4096;
/// Default and largest page of `fs.list`.
pub const LIST_PAGE: u32 = 500;
/// How long the daemon remembers an `fs.write` `operation_id` (24 h). A
/// client never resends a save whose answer it lost after this.
pub const OPERATION_KEEP_MS: u64 = 24 * 60 * 60 * 1000;
/// Longest `operation_id`.
pub const OPERATION_ID_MAX: usize = 64;

/// Is `s` a valid `fs.write` operation id: 8–64 characters `A-Za-z0-9_-`?
pub fn valid_operation_id(s: &str) -> bool {
    (8..=OPERATION_ID_MAX).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Error codes added with protocol v22 (-32120..-32129). Each `data` is
/// given at the code; nothing was changed when one of them is returned.
pub mod files_codes {
    /// The path is not a plain name under the task's worktree: absolute,
    /// UNC, drive-relative, `..`, a stream (`a:b`), a device name, a
    /// trailing dot or space, an 8.3 alias, or on the way through a
    /// symbolic link, junction, other reparse point or hard link, or a
    /// path Pod does not serve (`.git`). `data` = `{path, reason}`.
    pub const PATH_DENIED: i64 = -32120;
    /// Nothing at the path (or a folder on the way is missing).
    /// `data` = `{path}`.
    pub const NOT_FOUND: i64 = -32121;
    /// The file is bigger than the method serves.
    /// `data` = `{path, size, limit}`.
    pub const TOO_LARGE: i64 = -32122;
    /// `fs.write`: the file no longer has `expected_revision` (changed or
    /// deleted since the client read it). `data` = `{path, expected,
    /// current}` (`current` null when the file is gone). Re-read, merge,
    /// save again; never overwritten.
    pub const CONFLICT: i64 = -32123;
    /// Another program has the file open for writing, or a save of it is
    /// still being recovered. `data` = `{path}`. Retry later.
    pub const BUSY: i64 = -32124;
    /// `fs.write`: this `operation_id` was used for another task, path or
    /// content. `data` = `{operation_id}`.
    pub const OPERATION_MISMATCH: i64 = -32125;
    /// The path is a folder where a file is needed (or a file where a
    /// folder is), or `fs.write` names a file that does not exist (v1
    /// saves existing files only). `data` = `{path, kind}`.
    pub const NOT_A_FILE: i64 = -32126;
    /// `review.blob`: `rev` is neither the task's current merge base nor
    /// the index; list again with `review.diff`. `data` = `{rev,
    /// merge_base}`.
    pub const STALE_DIFF: i64 = -32127;
}

method!(FsStat, "fs.stat", FsPathParams, FsStatResult);
method!(FsList, "fs.list", FsListParams, FsListResult);
method!(FsRead, "fs.read", FsPathParams, FsReadResult);
method!(FsWrite, "fs.write", FsWriteParams, FsWriteResult);
method!(
    ReviewDiff,
    "review.diff",
    ReviewDiffParams,
    ReviewDiffResult
);
method!(
    ReviewBlob,
    "review.blob",
    ReviewBlobParams,
    ReviewBlobResult
);

/// The v22 method names.
pub const FILES_METHODS: &[&str] = &[
    FsStat::NAME,
    FsList::NAME,
    FsRead::NAME,
    FsWrite::NAME,
    ReviewDiff::NAME,
    ReviewBlob::NAME,
];

/// A path in a task's worktree.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FsPathParams {
    /// Task id (`<repo>/<name>`) or unique name.
    pub task: String,
    /// Relative to the worktree, `/`-separated (`\` is read as `/`); `""`
    /// is the worktree itself (`fs.stat`, `fs.list` only).
    pub path: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FsKind {
    #[default]
    File,
    Dir,
    /// A symbolic link, junction or other reparse point, or a hard link:
    /// listed so the tree can show it, never read, written or entered.
    Link,
}

/// One entry of a folder, or the answer of `fs.stat`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FsEntry {
    /// The name in its folder (`fs.list`) or the normalized path (`fs.stat`).
    pub name: String,
    pub kind: FsKind,
    /// Bytes (files only).
    #[serde(default)]
    pub size: u64,
    /// Last write, Unix epoch milliseconds.
    #[serde(default)]
    pub modified_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FsStatResult {
    pub task: String,
    /// The path as the daemon reads it (`/`-separated, no empty parts).
    pub path: String,
    /// `None`: nothing there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<FsEntry>,
    /// The file's revision, when it is a file of at most
    /// [`MAX_READ_BYTES`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FsListParams {
    pub task: String,
    /// The folder, `""` for the worktree.
    #[serde(default)]
    pub path: String,
    /// The `next` of the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Page size, at most [`LIST_PAGE`] (the default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FsListResult {
    pub task: String,
    pub path: String,
    /// Sorted by name (ordinal); `.git` is never listed.
    pub entries: Vec<FsEntry>,
    /// More entries: pass as `cursor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}

/// Line endings found in a file.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FsEol {
    /// No line break at all.
    #[default]
    None,
    Lf,
    Crlf,
    /// Both CRLF and LF.
    Mixed,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FsReadResult {
    pub task: String,
    pub path: String,
    /// The bytes, base64. The client decodes them (encoding, BOM, line
    /// endings) exactly as it decodes a local file.
    pub data: String,
    pub size: u64,
    /// SHA-256 of the bytes, lowercase hex: pass it as `expected_revision`.
    pub revision: String,
    /// Starts with the UTF-8 BOM.
    #[serde(default)]
    pub bom: bool,
    pub eol: FsEol,
    /// Has a NUL byte in its first 8 KiB (the editor will not open it).
    #[serde(default)]
    pub binary: bool,
    /// Valid UTF-8 (after a BOM).
    #[serde(default)]
    pub utf8: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FsWriteParams {
    pub task: String,
    pub path: String,
    /// The new bytes, base64, at most [`MAX_WRITE_BYTES`].
    pub data: String,
    /// The revision the client's buffer was read at. The file is replaced
    /// only while it still has it.
    pub expected_revision: String,
    /// Random per save (8–64 characters `A-Za-z0-9_-`). Sending the same
    /// save again with the same id never writes twice: the answer is the
    /// first one's (`already`). The id is bound to the task, path and
    /// content.
    pub operation_id: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FsWriteOutcome {
    /// Written by this request.
    #[default]
    Written,
    /// This `operation_id` was written before (by a request whose answer
    /// was lost); nothing written now.
    Already,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FsWriteResult {
    pub task: String,
    pub path: String,
    pub outcome: FsWriteOutcome,
    /// The file's revision after the save (the content's SHA-256).
    pub revision: String,
    pub size: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewDiffParams {
    pub task: String,
}

/// git's `check-attr` values of a path (`text`, `crlf`, `eol`: `set`,
/// `unset`, `unspecified` or a value), for cleaning CRLF the way `git diff`
/// does.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EolAttrs {
    pub text: String,
    pub crlf: String,
    pub eol: String,
}

/// One changed file of `review.diff`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewFile {
    pub path: String,
    /// Renames: the old path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    /// git status letter: `A`, `M`, `D`, `R`, `T`, ..., or `?` (untracked).
    pub status: String,
    /// git counts it as binary.
    #[serde(default)]
    pub binary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted: Option<u64>,
    /// Revision of the worktree file when listed (`None`: deleted, not a
    /// plain file, or above [`MAX_READ_BYTES`]). An `fs.read` that returns
    /// another revision means the diff is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    pub attrs: EolAttrs,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewDiffResult {
    pub task: String,
    pub base: String,
    /// The commit the old side comes from (`review.blob` `rev`).
    pub merge_base: String,
    /// The task's HEAD commit when listed.
    pub head: String,
    /// Changes when the merge base, HEAD or any listed file changes.
    pub snapshot: String,
    /// The worktree's `core.autocrlf` (`None`: unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autocrlf: Option<String>,
    pub files: Vec<ReviewFile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewBlobParams {
    pub task: String,
    /// `review.diff`'s `merge_base`, or `""` for the index.
    pub rev: String,
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReviewBlobResult {
    pub task: String,
    pub rev: String,
    pub path: String,
    /// `None`: no such path at `rev`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default)]
    pub size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids() {
        assert!(valid_operation_id("abcdefgh"));
        assert!(valid_operation_id(&"a".repeat(64)));
        assert!(valid_operation_id("save-0123_x"));
        assert!(!valid_operation_id("short"));
        assert!(!valid_operation_id(&"a".repeat(65)));
        assert!(!valid_operation_id("has space"));
        assert!(!valid_operation_id("한글한글한글한글"));
    }

    #[test]
    fn write_limit_fits_the_request_line() {
        // base64 of the largest write plus a generous envelope (path and
        // ids) stays under the daemon's 4 MiB request line.
        let b64 = MAX_WRITE_BYTES.div_ceil(3) * 4;
        assert!(b64 + MAX_PATH_BYTES as u64 + 1024 < 4 * 1024 * 1024);
    }
}
