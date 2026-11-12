//! Remote file access at the OS level (docs/22 TM-15): the task's worktree
//! root and every folder on the way are opened **without following links**
//! and **held** while the request uses them, and the file is read or
//! replaced through handles checked the same way.
//!
//! - A held folder is opened without `FILE_SHARE_DELETE`: nothing can
//!   rename, delete or replace it (with a junction) until the request ends.
//! - Each handle's final path must be its parent's plus the name asked for
//!   (no 8.3 alias of something else), no handle may be a reparse point
//!   (symbolic link, junction or any other tag), and a file must have one
//!   name (a hard link shares its content with a path outside the tree).
//! - A save writes a new temporary file in the held folder and renames it
//!   over the target, POSIX style (the target may stay open), while every
//!   folder on the way is held (so the name reaches the same folder) and
//!   the target is held against other writers from the revision check on.
//!   (A rename relative to the held folder's own handle is refused by
//!   Windows while that handle holds the folder.)
//!
//! The same rules as `git::pin` (worker commits), with share modes for an
//! editor: reads let other programs keep writing; a save holds the target
//! against writers but lets its own rename replace it.

use std::fs::File;
use std::io::{Read, Write as _};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE,
};

use crate::git::pin::{self, Facts};
use crate::protocol::{FsEntry, FsKind};

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PATH_NOT_FOUND: i32 = 3;
const ERROR_SHARING_VIOLATION: i32 = 32;
const ERROR_INVALID_NAME: i32 = 123;
const ERROR_DIRECTORY: i32 = 267;

/// Why a request could not use a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fault {
    /// Refused: a link, junction, reparse point, hard link or alias.
    Denied(String),
    NotFound,
    /// Another program holds it in a way that blocks this request.
    Busy,
    /// A folder where a file is needed (`"dir"`), or a file where a folder
    /// is (`"file"`).
    Kind(&'static str),
    TooLarge(u64),
    Io(String),
}

fn fault(e: &std::io::Error) -> Fault {
    match e.raw_os_error() {
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => Fault::NotFound,
        Some(ERROR_SHARING_VIOLATION) => Fault::Busy,
        Some(ERROR_DIRECTORY) => Fault::Kind("file"),
        Some(ERROR_INVALID_NAME) => Fault::Denied("a name Windows does not accept".into()),
        _ => Fault::Io(e.to_string()),
    }
}

struct Dir {
    /// Held open for its share mode (no rename or delete meanwhile).
    _hold: File,
    key: String,
}

/// How [`Held::file`] opens the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Read; other programs may keep writing (the revision is what was read).
    Read,
    /// The target of a save: read it, and no other program may open it for
    /// writing until the handle is dropped; deleting (our rename) is shared.
    Replace,
}

/// The worktree root and the folders down to one folder, open and held.
pub(crate) struct Held {
    /// Path of the deepest held folder.
    here: PathBuf,
    dirs: Vec<Dir>,
}

impl Held {
    /// Hold `root` (a real folder, not a link) and every folder of `dirs`
    /// below it.
    pub fn open(root: &Path, dirs: &[String]) -> Result<Held, Fault> {
        let file = pin::open_no_follow(root, true).map_err(|e| fault(&e))?;
        let facts = pin::facts(&file).map_err(|e| Fault::Io(e.to_string()))?;
        if facts.is_link() {
            return Err(Fault::Denied(
                "the task's worktree is a link or junction".into(),
            ));
        }
        if !facts.is_dir() {
            return Err(Fault::NotFound);
        }
        let key = pin::final_key(&file).map_err(|e| Fault::Io(e.to_string()))?;
        let mut held = Held {
            here: root.to_path_buf(),
            dirs: vec![Dir { _hold: file, key }],
        };
        for d in dirs {
            held.enter(d)?;
        }
        Ok(held)
    }

    /// Hold the folder `name` of the deepest held folder.
    pub fn enter(&mut self, name: &str) -> Result<(), Fault> {
        let path = self.here.join(name);
        let file = pin::open_no_follow(&path, true).map_err(|e| fault(&e))?;
        let facts = pin::facts(&file).map_err(|e| Fault::Io(e.to_string()))?;
        if facts.is_link() {
            return Err(Fault::Denied(format!(
                "{name:?} is a symbolic link, junction or other reparse point"
            )));
        }
        if !facts.is_dir() {
            return Err(Fault::Kind("file"));
        }
        let key = pin::final_key(&file).map_err(|e| Fault::Io(e.to_string()))?;
        self.expect_key(name, &key)?;
        self.here = path;
        self.dirs.push(Dir { _hold: file, key });
        Ok(())
    }

    /// The object opened as `name` in the deepest folder really is that
    /// name, not an 8.3 alias or another spelling of something else.
    fn expect_key(&self, name: &str, key: &str) -> Result<(), Fault> {
        let base = &self.dirs.last().expect("root").key;
        if key == format!("{base}\\{}", name.to_lowercase()) {
            Ok(())
        } else {
            Err(Fault::Denied(format!(
                "{name:?} reaches another name (an 8.3 alias or similar); use the real name"
            )))
        }
    }

    /// Facts of `name` in the deepest folder without following a link;
    /// `None` when nothing is there.
    pub fn peek(&self, name: &str) -> Result<Option<Facts>, Fault> {
        pin::peek(&self.here.join(name)).map_err(|e| fault(&e))
    }

    /// Open the plain file `name` of the deepest folder (see [`Access`]).
    pub fn file(&self, name: &str, access: Access) -> Result<(File, Facts), Fault> {
        let share = match access {
            Access::Read => FILE_SHARE_READ | FILE_SHARE_WRITE,
            Access::Replace => FILE_SHARE_READ | FILE_SHARE_DELETE,
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(FILE_GENERIC_READ)
            .share_mode(share)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
            .open(self.here.join(name))
            .map_err(|e| fault(&e))?;
        let facts = pin::facts(&file).map_err(|e| Fault::Io(e.to_string()))?;
        if facts.is_link() {
            return Err(Fault::Denied(format!(
                "{name:?} is a symbolic link, junction or other reparse point"
            )));
        }
        if facts.is_dir() {
            return Err(Fault::Kind("dir"));
        }
        if facts.links != 1 {
            return Err(Fault::Denied(format!(
                "{name:?} is a hard link ({} names share its content)",
                facts.links
            )));
        }
        let key = pin::final_key(&file).map_err(|e| Fault::Io(e.to_string()))?;
        self.expect_key(name, &key)?;
        Ok((file, facts))
    }

    /// The entries of the deepest folder, unsorted. Links are listed as
    /// [`FsKind::Link`] (never entered); `.git` and names Windows reads
    /// specially are left out.
    pub fn list(&self) -> Result<Vec<FsEntry>, Fault> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(&self.here).map_err(|e| fault(&e))? {
            let e = e.map_err(|e| fault(&e))?;
            let Some(name) = e.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if name.eq_ignore_ascii_case(".git")
                || pin::bad_name(&name).is_some()
                || name.starts_with(super::TEMP_PREFIX)
            {
                continue;
            }
            // From the directory listing itself: no link is followed.
            let Ok(m) = e.metadata() else { continue };
            let attrs = m.file_attributes();
            let kind = if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                FsKind::Link
            } else if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
                FsKind::Dir
            } else {
                FsKind::File
            };
            out.push(FsEntry {
                name,
                kind,
                size: if kind == FsKind::File { m.len() } else { 0 },
                modified_ms: m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as u64),
            });
        }
        Ok(out)
    }

    /// Replace the file `name` of the deepest folder with `bytes`: a new
    /// file `tmp` (created, never reused) in the same held folder, flushed,
    /// then renamed over `name`. The temp file is removed on every
    /// failure.
    pub fn replace(&self, name: &str, tmp: &str, bytes: &[u8]) -> Result<(), Fault> {
        let tmp_path = self.here.join(tmp);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(FILE_GENERIC_WRITE | DELETE)
            // Kept open until renamed: nobody else may write it meanwhile;
            // the rename itself needs delete sharing.
            .share_mode(FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(&tmp_path)
            .map_err(|e| fault(&e))?;
        let res = f
            .write_all(bytes)
            .and_then(|()| f.sync_all())
            .map_err(|e| fault(&e))
            .and_then(|()| self.rename_onto(&tmp_path, name));
        drop(f);
        if res.is_err() {
            let _ = std::fs::remove_file(&tmp_path);
        }
        res
    }

    /// Rename by name: every folder on the way is held, so the name
    /// reaches the folder that was checked; std's rename replaces the
    /// target POSIX style (our held target shares delete).
    fn rename_onto(&self, tmp_path: &Path, name: &str) -> Result<(), Fault> {
        std::fs::rename(tmp_path, self.here.join(name)).map_err(|e| fault(&e))
    }

    /// Remove a leftover temporary file `tmp` of the deepest folder (only a
    /// plain file; a link of that name is left alone).
    pub fn remove_temp(&self, tmp: &str) {
        if let Ok(Some(f)) = self.peek(tmp)
            && !f.is_link()
            && !f.is_dir()
        {
            let _ = std::fs::remove_file(self.here.join(tmp));
        }
    }
}

/// Read all of `f` (at most `limit` bytes; larger: [`Fault::TooLarge`]).
pub(crate) fn read_all(f: &mut File, facts: &Facts, limit: u64) -> Result<Vec<u8>, Fault> {
    if facts.size > limit {
        return Err(Fault::TooLarge(facts.size));
    }
    let mut bytes = Vec::with_capacity(facts.size as usize);
    f.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|e| fault(&e))?;
    if bytes.len() as u64 > limit {
        return Err(Fault::TooLarge(bytes.len() as u64));
    }
    Ok(bytes)
}
