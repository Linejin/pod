//! A task's worktree and branch: add (full or sparse), list, remove, prune.

use std::path::{Path, PathBuf};

use super::command::{guard, output_guarded_os, run, run_os};
use super::parse::parse_worktree_list;

/// `git worktree add -b <branch> <path> <commit>`.
pub fn worktree_add(repo: &Path, path: &Path, branch: &str, commit: &str) -> Result<(), String> {
    run_os(
        repo,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            "-b".as_ref(),
            branch.as_ref(),
            path.as_os_str(),
            commit.as_ref(),
        ],
    )
    .map(|_| ())
}

/// Cone-mode sparse checkout of a worktree added with `--no-checkout`:
/// `git sparse-checkout set --cone <dirs>` then `git read-tree -mu HEAD`
/// (fills only the cone). Git keeps the setting per worktree
/// (`extensions.worktreeConfig`), so the main checkout is untouched.
pub fn sparse_checkout(worktree: &Path, dirs: &[String]) -> Result<(), String> {
    let mut args = vec!["sparse-checkout", "set", "--cone"];
    args.extend(dirs.iter().map(String::as_str));
    run(worktree, &args)?;
    run(worktree, &["read-tree", "-mu", "HEAD"]).map(|_| ())
}

/// `git worktree add --no-checkout` (new branch at `commit`, or an existing
/// branch when `commit` is `None`), for [`sparse_checkout`].
pub fn worktree_add_no_checkout(
    repo: &Path,
    path: &Path,
    branch: &str,
    commit: Option<&str>,
) -> Result<(), String> {
    let mut args: Vec<&std::ffi::OsStr> = vec![
        "worktree".as_ref(),
        "add".as_ref(),
        "--no-checkout".as_ref(),
    ];
    match commit {
        Some(c) => args.extend(["-b".as_ref(), branch.as_ref(), path.as_os_str(), c.as_ref()]),
        None => args.extend([path.as_os_str(), branch.as_ref()]),
    }
    run_os(repo, &args).map(|_| ())
}

/// `git worktree remove [--force] <path>`.
pub fn worktree_remove(repo: &Path, path: &Path, force: bool) -> Result<(), String> {
    let mut args: Vec<&std::ffi::OsStr> = vec!["worktree".as_ref(), "remove".as_ref()];
    if force {
        args.push("--force".as_ref());
    }
    args.push(path.as_os_str());
    // Without --force git first runs `status` in the worktree (an agent's
    // tree): no filter program there (§18.5).
    let (cfg, _) = guard(repo, None)?;
    let out = output_guarded_os(repo, &cfg, &args)?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "git worktree remove {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// `git worktree add <path> <branch>` for an existing branch.
pub fn worktree_add_existing(repo: &Path, path: &Path, branch: &str) -> Result<(), String> {
    run_os(
        repo,
        &[
            "worktree".as_ref(),
            "add".as_ref(),
            path.as_os_str(),
            branch.as_ref(),
        ],
    )
    .map(|_| ())
}

pub fn worktree_prune(repo: &Path) -> Result<(), String> {
    run(repo, &["worktree", "prune"]).map(|_| ())
}

pub fn branch_delete(repo: &Path, branch: &str) -> Result<(), String> {
    run(repo, &["branch", "-D", "--end-of-options", branch]).map(|_| ())
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    pub head: Option<String>,
    /// `refs/heads/...` stripped to the short name.
    pub branch: Option<String>,
    pub bare: bool,
    pub detached: bool,
    pub prunable: bool,
    /// `git worktree lock`ed.
    pub locked: bool,
}

pub fn worktree_list(repo: &Path) -> Result<Vec<WorktreeEntry>, String> {
    run(repo, &["worktree", "list", "--porcelain", "-z"]).map(|s| parse_worktree_list(&s))
}
