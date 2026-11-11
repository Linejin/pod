//! Thin wrapper over the `git` CLI for worktree-based tasks.
//!
//! Everything goes through the `git` executable (not libgit2) so behaviour
//! matches what the user and the agents see in their shells. Output is read
//! in porcelain / `-z` form only. Every invocation adds
//! `-c core.longpaths=true -c core.quotepath=false -c core.fsmonitor=false`,
//! clears inherited
//! `GIT_DIR`-style variables, never prompts and never opens a console window
//! (the daemon runs detached, so a console child would get a fresh window).
//!
//! Layout (refactor D-P5): `command` runs git, the general call and the
//! guarded or bounded call of the worker paths, kept apart; `parse` reads
//! git's output and runs nothing; `worktree` adds, lists and removes task
//! worktrees; `query` answers questions about a repository or a worktree;
//! `worker_commit` commits for a sandboxed worker. Pod's path-identity rule
//! lives here. The public names are the ones this module always had.

use std::path::{Path, PathBuf};

mod command;
mod parse;
mod query;
mod worker_commit;
mod worktree;

pub use command::{ENV_GIT, Output, output, run};
pub use parse::{count_status_entries, parse_name_status, parse_numstat, parse_worktree_list};
pub use query::{
    base_rev, branch_exists, default_base, diff, facts, is_ancestor, merge_base, ref_exists,
    repo_root, resolve_commit, show_file, valid_branch_name,
};
pub use worker_commit::commit_for_worker;
pub use worktree::{
    WorktreeEntry, branch_delete, sparse_checkout, worktree_add, worktree_add_existing,
    worktree_add_no_checkout, worktree_list, worktree_prune, worktree_remove,
};

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

// Pod's one path-identity rule. Windows spells one folder many ways: 8.3
// short names (`C:\Users\RUNNER~1`), any letter case, `/` or `\`, the `\\?\`
// prefix. git prints the long, on-disk spelling. So:
// - paths Pod registers or creates (worktrees roots, repository roots, task
//   worktrees) are stored [`normalize`]d;
// - paths from outside (git output, stored state written by older versions,
//   client arguments, session cwds) are compared with [`same_path`] /
//   [`is_inside`], which fall back to [`normalize`] on both sides when the
//   spellings differ.

/// Absolute path without the `\\?\` prefix that `canonicalize` adds (git and
/// most tools do not accept it): the deepest existing ancestor is resolved
/// with `canonicalize` (long names, on-disk letter case, links), the rest
/// is appended as given. A path with no existing ancestor stays lexical.
pub fn normalize(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut base = abs.as_path();
    let mut tail = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(base) {
            let mut out = strip_verbatim(&c);
            out.extend(tail.iter().rev());
            return out;
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                base = parent;
            }
            _ => return abs,
        }
    }
}

fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

/// Lexical comparison key (Windows semantics): no `\\?\`, `\` separators,
/// no trailing separator, lowercase.
pub fn path_key(p: &Path) -> String {
    let s = strip_verbatim(p).to_string_lossy().replace('/', "\\");
    s.trim_end_matches('\\').to_lowercase()
}

/// The same folder or file however it is spelled: equal [`path_key`]s, else
/// equal keys after [`normalize`] (8.3 short names, links).
pub fn same_path(a: &Path, b: &Path) -> bool {
    path_key(a) == path_key(b) || path_key(&normalize(a)) == path_key(&normalize(b))
}

/// `child` is strictly inside `parent` (on a component boundary), however
/// either is spelled.
pub fn is_inside(child: &Path, parent: &Path) -> bool {
    fn lexical(c: &str, p: &str) -> bool {
        c.len() > p.len() && c.starts_with(p) && c.as_bytes()[p.len()] == b'\\'
    }
    lexical(&path_key(child), &path_key(parent))
        || lexical(&path_key(&normalize(child)), &path_key(&normalize(parent)))
}

/// git prints `C:/x/y`; make it `C:\x\y`.
pub fn native_path(git_path: &str) -> PathBuf {
    PathBuf::from(git_path.trim().replace('/', "\\"))
}

/// The 8.3 short spelling of an existing path (`C:\Users\RUNNER~1`),
/// `None` when the volume has no short names. Tests only.
#[cfg(test)]
pub(crate) fn short_spelling(p: &Path) -> Option<PathBuf> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: `wide` is NUL-terminated; `buf` is as long as the size passed.
    let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
    let n = n as usize;
    if n == 0 || n >= buf.len() {
        return None;
    }
    let s = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
    s.to_string_lossy().contains('~').then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pod's one path-identity rule: a folder named through its 8.3 short
    /// spelling (GitHub runners: `C:\Users\RUNNER~1`) is the folder git
    /// prints in the long one, for comparisons and for the worker commit's
    /// top-level check.
    #[test]
    fn short_names_are_the_same_path() {
        let t = std::env::temp_dir().join(format!("podd-git-short-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let dir = t.join("LongFolderName");
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        let long = normalize(&dir);
        let Some(short) = short_spelling(&long) else {
            eprintln!("skipped: no 8.3 short names on this volume");
            let _ = std::fs::remove_dir_all(&t);
            return;
        };
        assert_ne!(path_key(&short), path_key(&long));
        assert_eq!(normalize(&short), long);
        // Missing tail: the deepest existing ancestor is resolved.
        assert_eq!(
            normalize(&short.join("gone").join("x")),
            long.join("gone").join("x")
        );
        assert!(same_path(&short, &long));
        assert!(same_path(&short.join("gone"), &long.join("GONE")));
        assert!(!same_path(&short, &long.join("inner")));
        assert!(is_inside(&short.join("inner"), &long));
        assert!(is_inside(&long.join("inner"), &short));
        assert!(!is_inside(&long, &short));

        let (repo, no_hooks) = (long.join("repo"), long.join("no-hooks"));
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&no_hooks).unwrap();
        let g = |args: &[&str]| run(&repo, args).unwrap();
        g(&["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "t"),
            ("user.email", "t@example.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            g(&["config", k, v]);
        }
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        g(&["add", "a.txt"]);
        g(&["commit", "-q", "-m", "init"]);
        std::fs::write(repo.join("b.txt"), "b").unwrap();
        let short_repo = short.join("repo");
        let (sha, _) = commit_for_worker(&short_repo, &no_hooks, "short", true, &[]).unwrap();
        assert_eq!(sha.len(), 40);
        assert_eq!(path_key(&repo_root(&short_repo).unwrap()), path_key(&repo));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn path_helpers() {
        assert!(same_path(Path::new(r"C:\A\b\"), Path::new("c:/a/B")));
        assert!(!same_path(Path::new(r"C:\a\b"), Path::new(r"C:\a\bc")));
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\C:\x y\z")),
            PathBuf::from(r"C:\x y\z")
        );
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\srv\share\x")),
            PathBuf::from(r"\\srv\share\x")
        );
        assert_eq!(native_path("C:/a b/c\n"), PathBuf::from(r"C:\a b\c"));
    }
}
