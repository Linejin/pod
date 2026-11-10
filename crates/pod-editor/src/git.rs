//! Working-tree status markers from `git status --porcelain=v1 -z`.
//!
//! The CLI is used instead of a library: it is what the worktree layer
//! (docs/08) already relies on, honours every config/hook, and is fast enough
//! to run after each file-system change batch (it runs off the UI thread).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::uri::path_key;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitMark {
    // Ordered by display priority (highest last) for directory roll-up.
    Untracked,
    Added,
    Renamed,
    Modified,
    Deleted,
    Conflicted,
}

impl GitMark {
    /// One-letter badge shown in the tree.
    pub fn letter(self) -> &'static str {
        match self {
            GitMark::Untracked => "U",
            GitMark::Added => "A",
            GitMark::Renamed => "R",
            GitMark::Modified => "M",
            GitMark::Deleted => "D",
            GitMark::Conflicted => "!",
        }
    }
    /// Korean accessible label / tooltip.
    pub fn label(self) -> &'static str {
        match self {
            GitMark::Untracked => "추적 안 함",
            GitMark::Added => "추가됨",
            GitMark::Renamed => "이름 바뀜",
            GitMark::Modified => "수정됨",
            GitMark::Deleted => "삭제됨",
            GitMark::Conflicted => "충돌",
        }
    }
}

/// Where a worktree's git metadata lives. A linked worktree (`git worktree
/// add`, as every Pod task is) has a `.git` *file* pointing to
/// `<repo>/.git/worktrees/<name>`, which holds its `index` and `HEAD`;
/// branches and the object store are in the common dir (`<repo>/.git`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitDirs {
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
}

/// Reads `<worktree>/.git` (directory, or `gitdir:` file) without running git.
pub fn git_dirs(worktree: &Path) -> Option<GitDirs> {
    let dot = worktree.join(".git");
    let meta = std::fs::metadata(&dot).ok()?;
    if meta.is_dir() {
        let dot = simplify_dir(&dot);
        return Some(GitDirs {
            git_dir: dot.clone(),
            common_dir: dot,
        });
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let target = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let git_dir = simplify_dir(&worktree.join(target));
    let common_dir = match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(c) if !c.trim().is_empty() => simplify_dir(&git_dir.join(c.trim())),
        _ => git_dir.clone(),
    };
    Some(GitDirs {
        git_dir,
        common_dir,
    })
}

fn simplify_dir(p: &Path) -> PathBuf {
    crate::uri::simplify(&p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
}

/// Parses `XY` status codes.
pub fn mark_of(x: u8, y: u8) -> Option<GitMark> {
    Some(match (x, y) {
        (b'?', b'?') => GitMark::Untracked,
        (b'!', b'!') => return None,
        (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => GitMark::Conflicted,
        (b'R', _) | (b'C', _) => GitMark::Renamed,
        (b'A', _) => GitMark::Added,
        (_, b'D') | (b'D', _) => GitMark::Deleted,
        (b'M', _) | (_, b'M') | (b'T', _) | (_, b'T') => GitMark::Modified,
        _ => return None,
    })
}

/// Parses porcelain v1 `-z` output into (relative path, mark).
pub fn parse_porcelain_z(out: &[u8]) -> Vec<(String, GitMark)> {
    let mut res = Vec::new();
    let mut parts = out.split(|&b| b == 0).peekable();
    while let Some(rec) = parts.next() {
        if rec.len() < 4 {
            continue;
        }
        let (x, y) = (rec[0], rec[1]);
        let path = String::from_utf8_lossy(&rec[3..]).into_owned();
        if x == b'R' || x == b'C' {
            // The source path follows as its own NUL-terminated field.
            parts.next();
        }
        if let Some(m) = mark_of(x, y) {
            res.push((path, m));
        }
    }
    res
}

/// Status of every changed file plus a roll-up for their ancestor dirs.
#[derive(Clone, Debug, Default)]
pub struct GitStatus {
    files: HashMap<String, GitMark>,
    dirs: HashMap<String, GitMark>,
    pub branch: Option<String>,
    pub available: bool,
}

impl GitStatus {
    pub fn from_entries(top: &Path, entries: &[(String, GitMark)]) -> Self {
        let mut s = GitStatus {
            available: true,
            ..Default::default()
        };
        for (rel, m) in entries {
            let rel = rel.trim_end_matches('/');
            let abs = top.join(rel);
            s.files.insert(path_key(&abs), *m);
            let mut p = abs.parent();
            while let Some(dir) = p {
                if !dir.starts_with(top) {
                    break;
                }
                let k = path_key(dir);
                let e = s.dirs.entry(k).or_insert(*m);
                if *m > *e {
                    *e = *m;
                }
                if dir == top {
                    break;
                }
                p = dir.parent();
            }
        }
        s
    }

    pub fn file(&self, path: &Path) -> Option<GitMark> {
        self.files.get(&path_key(path)).copied()
    }
    pub fn dir(&self, path: &Path) -> Option<GitMark> {
        self.dirs.get(&path_key(path)).copied()
    }
    pub fn mark(&self, path: &Path, is_dir: bool) -> Option<GitMark> {
        if is_dir {
            self.dir(path)
        } else {
            self.file(path)
        }
    }
    pub fn changed_count(&self) -> usize {
        self.files.len()
    }
}

fn git(root: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C").arg(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

pub fn toplevel(root: &Path) -> Option<PathBuf> {
    let out = git(root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(PathBuf::from(if cfg!(windows) {
        s.replace('/', "\\")
    } else {
        s
    }))
}

/// Runs git (blocking; call from a background thread). Not a repository or
/// no git on PATH yields an unavailable, empty status.
pub fn read_status(root: &Path) -> GitStatus {
    let Some(top) = toplevel(root) else {
        return GitStatus::default();
    };
    let Ok(out) = git(root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    else {
        return GitStatus::default();
    };
    if !out.status.success() {
        return GitStatus::default();
    }
    let mut s = GitStatus::from_entries(&top, &parse_porcelain_z(&out.stdout));
    s.branch = git(root)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_records_and_renames() {
        let raw = b" M src/a.rs\0?? new dir/x.txt\0A  added.rs\0R  new.rs\0old.rs\0 D gone.rs\0UU conflict.rs\0";
        let v = parse_porcelain_z(raw);
        assert_eq!(
            v,
            vec![
                ("src/a.rs".into(), GitMark::Modified),
                ("new dir/x.txt".into(), GitMark::Untracked),
                ("added.rs".into(), GitMark::Added),
                ("new.rs".into(), GitMark::Renamed),
                ("gone.rs".into(), GitMark::Deleted),
                ("conflict.rs".into(), GitMark::Conflicted),
            ]
        );
    }

    #[test]
    fn dirs_roll_up_highest_priority() {
        let top = Path::new(if cfg!(windows) { r"C:\repo" } else { "/repo" });
        let s = GitStatus::from_entries(
            top,
            &[
                ("a/b/new.txt".into(), GitMark::Untracked),
                ("a/c.rs".into(), GitMark::Modified),
            ],
        );
        assert_eq!(s.file(&top.join("a").join("c.rs")), Some(GitMark::Modified));
        assert_eq!(s.dir(&top.join("a")), Some(GitMark::Modified));
        assert_eq!(s.dir(&top.join("a").join("b")), Some(GitMark::Untracked));
        assert_eq!(s.dir(top), Some(GitMark::Modified));
        assert_eq!(s.file(&top.join("x")), None);
    }

    #[test]
    fn real_repository_status() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let run = |args: &[&str]| {
            let ok = git(root)
                .args(args)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            assert!(ok, "git {args:?}");
        };
        if git(root).arg("--version").output().is_err() {
            eprintln!("git not on PATH; skipping");
            return;
        }
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.invalid"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(root.join("tracked.txt"), "1\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "init"]);
        std::fs::write(root.join("tracked.txt"), "2\n").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("u.txt"), "u").unwrap();
        let s = read_status(root);
        assert!(s.available);
        let canon = crate::uri::simplify(&root.canonicalize().unwrap());
        assert_eq!(s.file(&canon.join("tracked.txt")), Some(GitMark::Modified));
        assert_eq!(
            s.file(&canon.join("sub").join("u.txt")),
            Some(GitMark::Untracked)
        );
        assert_eq!(s.dir(&canon.join("sub")), Some(GitMark::Untracked));
    }
}
