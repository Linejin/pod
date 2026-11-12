//! The file I/O seam (docs/17 §2.4, R5): where one worktree's files come
//! from. [`FileSource`] reads, stats, lists and saves files by relative
//! path, a save only while the file still has the revision the buffer was
//! read at. [`LocalFiles`] is this machine's disk; a remote task's worktree
//! implements it over the daemon's `fs.*` methods (the app's glue over
//! `pod-client`'s `files`), so a remote file is never opened as a local path.
//!
//! [`Document`] is the GUI-free open/save state of one buffer on any source,
//! with the same rules as a local tab ([`crate::text`]): encoding, BOM,
//! CRLF and mixed endings come back unchanged, large files open read-only,
//! binary ones not at all; a save never overwrites a change it has not
//! seen (a conflict instead), and while the source cannot be reached the
//! unsaved text stays and the same save is sent again later (its
//! [`SaveId`] makes the daemon write it at most once).

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::text::{self, Encoding, LoadError, Loaded};

/// A file's revision: SHA-256 of its bytes, lowercase hex (the daemon's
/// `fs.*` revision; [`LocalFiles`] computes the same).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Revision(pub String);

impl Revision {
    pub fn of(bytes: &[u8]) -> Revision {
        Revision(
            Sha256::digest(bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        )
    }
}

/// Identifies one save across resends (the daemon's `operation_id`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveId(pub String);

impl SaveId {
    /// A fresh id: 32 hex characters, unique (not secret).
    pub fn new() -> SaveId {
        use std::hash::{BuildHasher, Hasher};
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let half = |salt: u64| {
            let mut h = std::collections::hash_map::RandomState::new().build_hasher();
            h.write_u128(nanos);
            h.write_u64(seq ^ salt);
            h.write_u32(std::process::id());
            h.finish()
        };
        SaveId(format!("{:016x}{:016x}", half(1), half(2)))
    }
}

impl Default for SaveId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    /// A link, junction or hard link: shown, never opened.
    Link,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub size: u64,
    pub revision: Option<Revision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    NotFound,
    /// The file changed (or went: `None`) since the buffer read it.
    Conflict {
        current: Option<Revision>,
    },
    /// A path the source refuses (escape, link, stream, device, `.git`).
    Denied(String),
    TooLarge {
        size: u64,
        limit: u64,
    },
    /// Another program has it open; try the same save again later.
    Busy,
    /// Not connected: nothing was sent.
    Unreachable(String),
    /// Sent, but the answer was lost: the outcome is unknown; send the same
    /// save again once connected.
    Unknown(String),
    Other(String),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SourceError::NotFound => f.write_str("파일이 없습니다"),
            SourceError::Conflict { .. } => f.write_str("파일이 그사이 바뀌었습니다"),
            SourceError::Denied(why) => write!(f, "열 수 없는 경로입니다: {why}"),
            SourceError::TooLarge { size, limit } => {
                write!(f, "파일이 너무 큽니다 ({size} > {limit} 바이트)")
            }
            SourceError::Busy => f.write_str("다른 프로그램이 파일을 사용 중입니다"),
            SourceError::Unreachable(m) => write!(f, "연결되지 않았습니다: {m}"),
            SourceError::Unknown(m) => write!(f, "응답을 받지 못했습니다: {m}"),
            SourceError::Other(m) => f.write_str(m),
        }
    }
}

/// One worktree's files, by `/`-separated relative path (`""` = the root).
pub trait FileSource: Send + Sync {
    /// Largest file [`FileSource::read`] returns.
    fn max_read_bytes(&self) -> u64;
    fn read(&self, rel: &str) -> Result<(Vec<u8>, Revision), SourceError>;
    /// `None`: nothing there.
    fn stat(&self, rel: &str) -> Result<Option<Stat>, SourceError>;
    fn list(&self, rel: &str) -> Result<Vec<Entry>, SourceError>;
    /// Replace the existing file `rel` with `bytes` only while it has
    /// `expected`; the new revision. `id` is the same for every resend of
    /// one save.
    fn write(
        &self,
        rel: &str,
        bytes: &[u8],
        expected: &Revision,
        id: &SaveId,
    ) -> Result<Written, SourceError>;
}

/// This machine's disk under `root`.
#[derive(Debug, Clone)]
pub struct LocalFiles {
    root: PathBuf,
}

impl LocalFiles {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        LocalFiles { root: root.into() }
    }

    fn path(&self, rel: &str) -> Result<PathBuf, SourceError> {
        let rel = rel.replace('\\', "/");
        let bad = rel.starts_with('/')
            || rel.as_bytes().get(1) == Some(&b':')
            || rel.split('/').any(|p| p == "..");
        if bad {
            return Err(SourceError::Denied(format!(
                "{rel}: not under the worktree"
            )));
        }
        Ok(self.root.join(rel.trim_matches('/')))
    }
}

fn io(e: std::io::Error) -> SourceError {
    if e.kind() == std::io::ErrorKind::NotFound {
        SourceError::NotFound
    } else {
        SourceError::Other(e.to_string())
    }
}

impl FileSource for LocalFiles {
    fn max_read_bytes(&self) -> u64 {
        text::MAX_FILE_BYTES
    }

    fn read(&self, rel: &str) -> Result<(Vec<u8>, Revision), SourceError> {
        let p = self.path(rel)?;
        let len = std::fs::metadata(&p).map_err(io)?.len();
        if len > self.max_read_bytes() {
            return Err(SourceError::TooLarge {
                size: len,
                limit: self.max_read_bytes(),
            });
        }
        let b = std::fs::read(&p).map_err(io)?;
        let r = Revision::of(&b);
        Ok((b, r))
    }

    fn stat(&self, rel: &str) -> Result<Option<Stat>, SourceError> {
        let p = self.path(rel)?;
        let m = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(io(e)),
        };
        let kind = if m.file_type().is_symlink() {
            Kind::Link
        } else if m.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        };
        let revision = (kind == Kind::File && m.len() <= self.max_read_bytes())
            .then(|| std::fs::read(&p).ok().map(|b| Revision::of(&b)))
            .flatten();
        Ok(Some(Stat {
            kind,
            size: m.len(),
            revision,
        }))
    }

    fn list(&self, rel: &str) -> Result<Vec<Entry>, SourceError> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(self.path(rel)?).map_err(io)? {
            let e = e.map_err(io)?;
            let Ok(m) = e.metadata() else { continue };
            let kind = if m.file_type().is_symlink() {
                Kind::Link
            } else if m.is_dir() {
                Kind::Dir
            } else {
                Kind::File
            };
            out.push(Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                kind,
                size: m.len(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn write(
        &self,
        rel: &str,
        bytes: &[u8],
        expected: &Revision,
        _: &SaveId,
    ) -> Result<Written, SourceError> {
        let p = self.path(rel)?;
        // The same check as a local tab's save (an unseen change blocks it).
        let now = match std::fs::read(&p) {
            Ok(b) => Some(Revision::of(&b)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(io(e)),
        };
        if now.as_ref() != Some(expected) {
            return Err(SourceError::Conflict { current: now });
        }
        text::write_atomic(&p, bytes).map_err(|e| {
            if text::is_replace_failure(&e) {
                SourceError::Busy
            } else {
                io(e)
            }
        })?;
        Ok(Written {
            revision: Revision::of(bytes),
            already: false,
        })
    }
}

/// A save's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub revision: Revision,
    /// An earlier send of this save (whose answer was lost) wrote it.
    pub already: bool,
}

/// Why a [`Document`] could not open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    Load(LoadError),
    Source(SourceError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Load(e) => e.fmt(f),
            OpenError::Source(e) => e.fmt(f),
        }
    }
}

/// What a [`Document::save`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Saved {
    /// Written (`already`: by an earlier send of this save whose answer was
    /// lost).
    Written { already: bool },
    /// Nothing to save.
    Unchanged,
    /// The file changed since it was read: nothing written, the buffer
    /// kept. Reload (dropping the edits) or merge, then save again.
    Conflict { current: Option<Revision> },
    /// Not saved now; the text and this save are kept and
    /// [`Document::save`] sends the same save again.
    Kept(SourceError),
    /// Refused for good (read-only, cannot be encoded, path refused).
    Refused(String),
}

/// A save sent but not answered yet: resent as it is.
#[derive(Debug, Clone)]
struct Pending {
    bytes: Vec<u8>,
    expected: Revision,
    id: SaveId,
}

/// One open file of a [`FileSource`] (GUI-free).
#[derive(Debug, Clone)]
pub struct Document {
    rel: String,
    /// As loaded or last saved: format (encoding, BOM, endings), flags.
    loaded: Loaded,
    revision: Revision,
    /// The buffer (LF text).
    text: String,
    pending: Option<Pending>,
}

impl Document {
    pub fn open(src: &dyn FileSource, rel: &str) -> Result<Document, OpenError> {
        Self::open_as(src, rel, text::utf8())
    }

    /// Open reading the bytes in `encoding` (a non-UTF-8 file).
    pub fn open_as(
        src: &dyn FileSource,
        rel: &str,
        encoding: &'static Encoding,
    ) -> Result<Document, OpenError> {
        let (bytes, revision) = src.read(rel).map_err(|e| match e {
            SourceError::TooLarge { size, .. } => OpenError::Load(LoadError::TooLarge(size)),
            e => OpenError::Source(e),
        })?;
        let len = bytes.len() as u64;
        let loaded = text::decode_as(bytes, len, encoding).map_err(OpenError::Load)?;
        Ok(Document {
            rel: rel.into(),
            text: loaded.text.clone(),
            loaded,
            revision,
            pending: None,
        })
    }

    pub fn rel(&self) -> &str {
        &self.rel
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn loaded(&self) -> &Loaded {
        &self.loaded
    }

    pub fn revision(&self) -> &Revision {
        &self.revision
    }

    /// Read-only: undecodable bytes or a large file (as a local tab).
    pub fn read_only(&self) -> bool {
        !self.loaded.lossless || self.loaded.large
    }

    pub fn is_dirty(&self) -> bool {
        self.text != self.loaded.text || self.pending.is_some()
    }

    /// A save is waiting to be sent again.
    pub fn has_pending_save(&self) -> bool {
        self.pending.is_some()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = text.into();
    }

    /// Save the buffer: encoded as the file was read, sent with the
    /// revision it was read at. A save kept by an earlier
    /// [`Saved::Kept`] is sent again unchanged (same id) as long as the
    /// buffer did not change since; otherwise a new save replaces it.
    pub fn save(&mut self, src: &dyn FileSource) -> Saved {
        if self.read_only() {
            return Saved::Refused("읽기 전용으로 연 파일은 저장하지 않습니다".into());
        }
        let bytes = match text::encode(&self.text, &self.loaded.format) {
            Ok(b) => b,
            Err(e) => return Saved::Refused(e.to_string()),
        };
        let pending = match self.pending.take() {
            Some(p) if p.bytes == bytes => p,
            _ => {
                if self.text == self.loaded.text {
                    return Saved::Unchanged;
                }
                Pending {
                    bytes,
                    expected: self.revision.clone(),
                    id: SaveId::new(),
                }
            }
        };
        match src.write(&self.rel, &pending.bytes, &pending.expected, &pending.id) {
            Ok(w) => {
                self.saved(pending.bytes, w.revision);
                Saved::Written { already: w.already }
            }
            Err(SourceError::Conflict { current }) => {
                // The earlier send may have been the one that wrote it.
                if current.as_ref() == Some(&Revision::of(&pending.bytes)) {
                    let rev = Revision::of(&pending.bytes);
                    self.saved(pending.bytes, rev);
                    return Saved::Written { already: true };
                }
                Saved::Conflict { current }
            }
            Err(
                e @ (SourceError::Busy | SourceError::Unreachable(_) | SourceError::Unknown(_)),
            ) => {
                self.pending = Some(pending);
                Saved::Kept(e)
            }
            Err(e) => Saved::Refused(e.to_string()),
        }
    }

    fn saved(&mut self, bytes: Vec<u8>, rev: Revision) {
        let len = bytes.len() as u64;
        // The written bytes are the new base (mixed line endings), as a
        // local save does.
        if let Ok(l) = text::decode_as(bytes, len, self.loaded.format.encoding) {
            self.loaded = l;
        }
        self.text = self.loaded.text.clone();
        self.revision = rev;
        self.pending = None;
    }

    /// Read the file again, dropping the buffer's edits (after a conflict).
    pub fn reload(&mut self, src: &dyn FileSource) -> Result<(), OpenError> {
        let d = Self::open_as(src, &self.rel, self.loaded.format.encoding)?;
        *self = d;
        Ok(())
    }
}

/// Open `rel` of the worktree at `root` on this disk (a [`LocalFiles`]).
pub fn open_local(root: &Path, rel: &str) -> Result<Document, OpenError> {
    Document::open(&LocalFiles::new(root), rel)
}

#[cfg(test)]
mod tests;
