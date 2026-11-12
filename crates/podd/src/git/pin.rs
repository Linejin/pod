//! Paths in a worker's tree, opened without following links and **held**
//! while git reads them (threat model TM-21, docs/19 §18.6).
//!
//! A check on a path name (`symlink_metadata`) and git opening that name
//! later are two moments; in between a worker can swap a folder for a
//! junction or rewrite a file. Here every folder from the root down and the
//! file itself are opened by handle (`FILE_FLAG_OPEN_REPARSE_POINT`: the
//! link itself, never its target) and kept open with share modes that make
//! the name stable until [`Held`] is dropped:
//!
//! - folders: no `FILE_SHARE_DELETE`, so none can be renamed, deleted or
//!   replaced (a junction needs the old entry gone first); the handle asks
//!   for `FILE_LIST_DIRECTORY` so the share mode counts at all;
//! - files: only `FILE_SHARE_READ`, so nothing can write, rename or delete
//!   them, and opening fails if something has them open for writing.
//!
//! Each handle's final path must be its parent's plus the name (no 8.3
//! alias, no other spelling), no handle may be a reparse point (symbolic
//! link, junction or any other tag), a file must have one name (a hard link
//! shares its content with a path outside the tree), and names that Windows
//! reads as a stream (`a:b`), a device (`NUL`, `com1.txt`) or with trailing
//! dots/spaces are refused before anything opens. [`Held::recheck`] reads
//! every handle again after git ran.

use std::collections::HashMap;
use std::fs::File;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};

use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
    FILE_LIST_DIRECTORY, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle, GetFinalPathNameByHandleW,
    SYNCHRONIZE, VOLUME_NAME_DOS,
};

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PATH_NOT_FOUND: i32 = 3;
const ERROR_SHARING_VIOLATION: i32 = 32;

/// What one handle says about its object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Facts {
    pub attrs: u32,
    pub links: u32,
    pub size: u64,
    pub written: u64,
    /// Volume serial and file index: the object's identity.
    pub id: (u32, u64),
}

impl Facts {
    pub fn is_link(&self) -> bool {
        self.attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    pub fn is_dir(&self) -> bool {
        self.attrs & FILE_ATTRIBUTE_DIRECTORY != 0
    }
}

/// `GetFileInformationByHandle`.
pub(crate) fn facts(f: &File) -> std::io::Result<Facts> {
    let mut i: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: `f` is an open handle; `i` is a valid out pointer.
    if unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut i) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(Facts {
        attrs: i.dwFileAttributes,
        links: i.nNumberOfLinks,
        size: (u64::from(i.nFileSizeHigh) << 32) | u64::from(i.nFileSizeLow),
        written: (u64::from(i.ftLastWriteTime.dwHighDateTime) << 32)
            | u64::from(i.ftLastWriteTime.dwLowDateTime),
        id: (
            i.dwVolumeSerialNumber,
            (u64::from(i.nFileIndexHigh) << 32) | u64::from(i.nFileIndexLow),
        ),
    })
}

/// The handle's normalized final path as a comparison key (no `\\?\`,
/// lowercase): what the object really is, whatever name opened it.
pub(crate) fn final_key(f: &File) -> std::io::Result<String> {
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: `buf` holds `buf.len()` u16s; the handle is open.
        let n = unsafe {
            GetFinalPathNameByHandleW(
                f.as_raw_handle() as _,
                buf.as_mut_ptr(),
                buf.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        } as usize;
        if n == 0 {
            return Err(std::io::Error::last_os_error());
        }
        if n < buf.len() {
            let s = String::from_utf16_lossy(&buf[..n]);
            return Ok(super::path_key(Path::new(&s)));
        }
        buf.resize(n + 1, 0);
    }
}

/// Open `p` itself (a link is opened as the link, never followed). A
/// folder handle blocks rename and delete; a file handle also blocks
/// writers.
pub(crate) fn open_no_follow(p: &Path, dir: bool) -> std::io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    if dir {
        o.access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    } else {
        o.access_mode(FILE_GENERIC_READ).share_mode(FILE_SHARE_READ);
    }
    o.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS)
        .open(p)
}

/// Facts about `p` itself without following a link, `None` when it does
/// not exist. Opens and closes at once (nothing is held).
pub(crate) fn peek(p: &Path) -> std::io::Result<Option<Facts>> {
    let mut o = std::fs::OpenOptions::new();
    o.access_mode(FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS);
    match o.open(p) {
        Ok(f) => facts(&f).map(Some),
        Err(e) if missing(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

fn missing(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND)
    )
}

/// Why Windows would not read `name` as a plain file name, if it would not:
/// a stream (`a.txt:s`), a device (`NUL`, `com1.txt`, `CONIN$`), a trailing
/// dot or space (dropped by Windows, so two names meet), `.`/`..`, or a
/// character Windows does not allow.
pub(crate) fn bad_name(name: &str) -> Option<&'static str> {
    if name.is_empty() || name == "." || name == ".." {
        return Some("an empty, `.` or `..` part");
    }
    if name.contains(':') {
        return Some("a `:` (an NTFS stream)");
    }
    if name
        .chars()
        .any(|c| (c as u32) < 0x20 || matches!(c, '\\' | '<' | '>' | '"' | '|' | '?' | '*'))
    {
        return Some("a character Windows does not allow in names");
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Some("a trailing dot or space");
    }
    let stem: String = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_uppercase();
    let numbered = |p: &str| {
        stem.strip_prefix(p).is_some_and(|d| {
            let mut c = d.chars();
            matches!(
                (c.next(), c.next()),
                (Some('0'..='9' | '¹' | '²' | '³'), None)
            )
        })
    };
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || numbered("COM")
        || numbered("LPT")
    {
        return Some("a device name");
    }
    None
}

/// What [`Held::entry`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Entry {
    /// A plain file with one name, now held.
    File,
    /// Nothing there (a folder on the way is missing or is a file).
    Missing,
    /// A folder (not held).
    Dir,
}

struct Handle {
    file: File,
    key: String,
    facts: Facts,
    what: String,
}

/// The root and the paths [`Held::entry`] found, open and held.
pub(crate) struct Held {
    root: PathBuf,
    dirs: HashMap<String, Handle>,
    files: Vec<Handle>,
}

impl Held {
    /// Hold `root` (a real folder, not a link).
    pub fn root(root: &Path) -> Result<Held, String> {
        let file = open_no_follow(root, true).map_err(|e| format!("{}: {e}", root.display()))?;
        let facts = facts(&file).map_err(|e| format!("{}: {e}", root.display()))?;
        if facts.is_link() || !facts.is_dir() {
            return Err(format!(
                "{} is not a plain folder (a link, junction or file)",
                root.display()
            ));
        }
        let key = final_key(&file).map_err(|e| format!("{}: {e}", root.display()))?;
        let mut dirs = HashMap::new();
        dirs.insert(
            String::new(),
            Handle {
                file,
                key,
                facts,
                what: root.display().to_string(),
            },
        );
        Ok(Held {
            root: root.to_path_buf(),
            dirs,
            files: Vec::new(),
        })
    }

    /// `rel` (`/`-separated, relative to the root): every folder on the way
    /// is held; a plain file is held too. Links, junctions, other reparse
    /// points, hard links, aliases and names Windows reads specially are
    /// refused (`Err` with the reason).
    pub fn entry(&mut self, rel: &str) -> Result<Entry, String> {
        let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
        let Some((leaf, dirs)) = parts.split_last() else {
            return Err(format!("{rel:?}: an empty path"));
        };
        for p in &parts {
            if let Some(why) = bad_name(p) {
                return Err(format!("{rel}: the name has {why}"));
            }
        }
        let mut parent = String::new();
        for d in dirs {
            let here = if parent.is_empty() {
                (*d).to_string()
            } else {
                format!("{parent}/{d}")
            };
            if !self.dirs.contains_key(&here) {
                let path = self.root.join(here.replace('/', "\\"));
                let file = match open_no_follow(&path, true) {
                    Ok(f) => f,
                    Err(e) if missing(&e) => return Ok(Entry::Missing),
                    Err(e) => return Err(format!("{here}: {}", busy(&e))),
                };
                let facts = facts(&file).map_err(|e| format!("{here}: {e}"))?;
                if facts.is_link() {
                    return Err(format!(
                        "{rel} is under a symbolic link, junction or other reparse point ({here})"
                    ));
                }
                if !facts.is_dir() {
                    return Ok(Entry::Missing);
                }
                let key = final_key(&file).map_err(|e| format!("{here}: {e}"))?;
                self.expect_key(&parent, d, &key, rel)?;
                self.dirs.insert(
                    here.clone(),
                    Handle {
                        file,
                        key,
                        facts,
                        what: here.clone(),
                    },
                );
            }
            parent = here;
        }
        let path = self.root.join(parts.join("\\"));
        let file = match open_no_follow(&path, false) {
            Ok(f) => f,
            Err(e) if missing(&e) => return Ok(Entry::Missing),
            Err(e) => return Err(format!("{rel}: {}", busy(&e))),
        };
        let facts = facts(&file).map_err(|e| format!("{rel}: {e}"))?;
        if facts.is_link() {
            return Err(format!(
                "{rel} is a symbolic link, junction or other reparse point"
            ));
        }
        if facts.is_dir() {
            return Ok(Entry::Dir);
        }
        if facts.links != 1 {
            return Err(format!(
                "{rel} is a hard link ({} names share its content)",
                facts.links
            ));
        }
        let key = final_key(&file).map_err(|e| format!("{rel}: {e}"))?;
        self.expect_key(&parent, leaf, &key, rel)?;
        self.files.push(Handle {
            file,
            key,
            facts,
            what: rel.to_string(),
        });
        Ok(Entry::File)
    }

    /// The object opened as `parent/name` really is that name (not an 8.3
    /// alias or any other spelling of something else).
    fn expect_key(&self, parent: &str, name: &str, key: &str, rel: &str) -> Result<(), String> {
        let base = &self.dirs[parent].key;
        let want = format!("{base}\\{}", name.to_lowercase());
        if key == want {
            Ok(())
        } else {
            Err(format!(
                "{rel}: opening {name:?} reached a different name ({key}); podd only commits plain names"
            ))
        }
    }

    /// Every held handle still shows the object it showed when opened.
    pub fn recheck(&self) -> Result<(), String> {
        for h in self.dirs.values().chain(&self.files) {
            let now = facts(&h.file).map_err(|e| format!("{}: {e}", h.what))?;
            let key = final_key(&h.file).map_err(|e| format!("{}: {e}", h.what))?;
            let same = if h.facts.is_dir() {
                now.attrs == h.facts.attrs && now.id == h.facts.id
            } else {
                now == h.facts
            };
            if !same || key != h.key {
                return Err(format!("{} changed while podd was committing it", h.what));
            }
        }
        Ok(())
    }
}

fn busy(e: &std::io::Error) -> String {
    if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
        "another program has it open for writing or deleting; close it and commit again".into()
    } else {
        e.to_string()
    }
}

/// `mklink /J link target` (no privilege needed). Tests only.
#[cfg(test)]
pub(crate) fn junction(link: &Path, target: &Path) -> bool {
    std::process::Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn tmp(tag: &str) -> Tmp {
        let t = std::env::temp_dir().join(format!("podd-pin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(&t).unwrap();
        Tmp(t)
    }

    #[test]
    fn names_windows_reads_specially_are_refused() {
        for bad in [
            "a.txt:secret",
            "a:b",
            "NUL",
            "nul.txt",
            "Com1.log",
            "LPT9",
            "COM¹",
            "CONIN$",
            "x.",
            "x ",
            ".",
            "..",
            "",
            "a*b",
            "a\\b",
        ] {
            assert!(bad_name(bad).is_some(), "{bad:?}");
        }
        for ok in [
            "a.txt",
            "com10",
            "console.txt",
            "nul_",
            "한글 é.txt",
            "-dash",
            ".gitattributes",
        ] {
            assert_eq!(bad_name(ok), None, "{ok:?}");
        }
    }

    /// TM-21: while held, nothing on the way can be renamed, deleted,
    /// replaced by a junction or written; after drop it can.
    #[test]
    fn held_paths_cannot_be_swapped_or_written() {
        let t = tmp("swap");
        let root = t.0.join("wt");
        std::fs::create_dir_all(root.join("d")).unwrap();
        std::fs::write(root.join("d").join("f.txt"), "mine").unwrap();
        let outside = t.0.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("f.txt"), "secret").unwrap();

        let mut h = Held::root(&root).unwrap();
        assert_eq!(h.entry("d/f.txt").unwrap(), Entry::File);
        // The swap a worker would try: move the folder away, put a
        // junction to `outside` in its place. Every step is refused.
        assert!(std::fs::rename(root.join("d"), root.join("d2")).is_err());
        assert!(std::fs::remove_file(root.join("d").join("f.txt")).is_err());
        assert!(std::fs::rename(root.join("d").join("f.txt"), root.join("g.txt")).is_err());
        assert!(std::fs::write(root.join("d").join("f.txt"), "changed").is_err());
        assert!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(root.join("d").join("f.txt"))
                .is_err()
        );
        assert!(std::fs::rename(&root, t.0.join("wt-moved")).is_err());
        h.recheck().unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("d").join("f.txt")).unwrap(),
            "mine",
            "readers (git) still read it"
        );
        drop(h);
        std::fs::rename(root.join("d"), root.join("d2")).unwrap();
        // Now a junction stands where the folder was: refused, not followed.
        assert!(junction(&root.join("d"), &outside), "mklink /J");
        let mut h = Held::root(&root).unwrap();
        let e = h.entry("d/f.txt").unwrap_err();
        assert!(e.contains("reparse point"), "{e}");
        assert_eq!(h.entry("d2/f.txt").unwrap(), Entry::File);
        // The root itself as a junction: refused.
        let jroot = t.0.join("jroot");
        assert!(junction(&jroot, &root));
        assert!(Held::root(&jroot).is_err());
    }

    #[test]
    fn hard_links_aliases_and_open_writers_are_refused() {
        let t = tmp("alias");
        let root = t.0.join("wt");
        std::fs::create_dir_all(root.join("Long Folder Name")).unwrap();
        std::fs::write(root.join("Long Folder Name").join("a.txt"), "a").unwrap();
        let outside = t.0.join("secret.txt");
        std::fs::write(&outside, "secret").unwrap();
        std::fs::hard_link(&outside, root.join("hl.txt")).unwrap();
        let mut h = Held::root(&root).unwrap();
        assert!(h.entry("hl.txt").unwrap_err().contains("hard link"));
        assert_eq!(h.entry("missing.txt").unwrap(), Entry::Missing);
        assert_eq!(h.entry("nope/missing.txt").unwrap(), Entry::Missing);
        assert_eq!(h.entry("Long Folder Name").unwrap(), Entry::Dir);
        assert_eq!(h.entry("long folder name/a.txt").unwrap(), Entry::File);
        if let Some(short) = crate::git::short_spelling(&root.join("Long Folder Name")) {
            let alias = short.file_name().unwrap().to_string_lossy().into_owned();
            let e = h.entry(&format!("{alias}/a.txt")).unwrap_err();
            assert!(e.contains("different name"), "{e}");
        }
        // Held, a.txt has no writer; released, one opens it and podd
        // cannot hold it until it is closed.
        assert!(
            std::fs::OpenOptions::new()
                .append(true)
                .open(root.join("Long Folder Name").join("a.txt"))
                .is_err()
        );
        drop(h);
        let _writer = std::fs::OpenOptions::new()
            .append(true)
            .open(root.join("Long Folder Name").join("a.txt"))
            .unwrap();
        let mut h2 = Held::root(&root).unwrap();
        let e = h2.entry("Long Folder Name/a.txt").unwrap_err();
        assert!(e.contains("open for writing"), "{e}");
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "secret");
    }
}
