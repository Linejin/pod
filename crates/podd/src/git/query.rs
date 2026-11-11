//! Questions about a repository and a task's worktree: the repository root,
//! the default base, refs, the task facts and the review diff. Reading a
//! worker's tree (status, diff) goes through the guarded call: no filter
//! program runs there.

use std::path::{Path, PathBuf};

use crate::protocol::{DiffFile, TaskGit};

use super::command::{guard, output, run, run_guarded};
use super::parse::{
    count_status_entries, merge_diff, parse_commit, parse_left_right, parse_name_status,
    parse_numstat,
};
use super::{native_path, normalize, same_path};

/// Untracked files larger than this are reported without line counts.
const MAX_COUNT_BYTES: u64 = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Repositories and refs
// ---------------------------------------------------------------------------

/// The repository whose root is `dir`. Errors when `dir` is not inside a git
/// work tree, is bare, is a linked worktree, or is a subdirectory.
pub fn repo_root(dir: &Path) -> Result<PathBuf, String> {
    let o = output(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--is-bare-repository",
            "--git-dir",
            "--git-common-dir",
            "--show-toplevel",
        ],
    )?;
    if !o.ok {
        return Err(format!("{} is not a git repository", dir.display()));
    }
    let lines: Vec<&str> = o.stdout.lines().collect();
    if lines.first() == Some(&"true") {
        return Err(format!(
            "{} is a bare repository (a work tree is needed)",
            dir.display()
        ));
    }
    let [_, git_dir, common_dir, top] = lines[..] else {
        return Err(format!("{} is not inside a work tree", dir.display()));
    };
    if !same_path(&native_path(git_dir), &native_path(common_dir)) {
        return Err(format!(
            "{} is a linked worktree; add the main repository instead",
            dir.display()
        ));
    }
    let top = native_path(top);
    if !same_path(&normalize(&top), &normalize(dir)) {
        return Err(format!(
            "{} is inside the repository {}; add its root",
            dir.display(),
            top.display()
        ));
    }
    Ok(top)
}

/// `origin/HEAD`'s branch when it exists locally (else the remote ref), else
/// the checked-out branch when it has a commit, else `HEAD`.
pub fn default_base(repo: &Path) -> String {
    if let Ok(o) = output(
        repo,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) && o.ok
    {
        let remote = o.stdout.trim().to_string();
        if let Some(local) = remote.strip_prefix("origin/")
            && branch_exists(repo, local)
        {
            return local.to_string();
        }
        if !remote.is_empty() {
            return remote;
        }
    }
    if let Ok(o) = output(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        && o.ok
    {
        let b = o.stdout.trim();
        if !b.is_empty() && resolve_commit(repo, b).is_some() {
            return b.to_string();
        }
    }
    "HEAD".into()
}

/// Full commit id of `rev`, if it names a commit.
pub fn resolve_commit(dir: &Path, rev: &str) -> Option<String> {
    if rev.is_empty() || rev.starts_with('-') {
        return None;
    }
    let spec = format!("{rev}^{{commit}}");
    let o = output(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )
    .ok()?;
    let s = o.stdout.trim();
    (o.ok && !s.is_empty()).then(|| s.to_string())
}

pub fn branch_exists(repo: &Path, branch: &str) -> bool {
    output(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok_and(|o| o.ok)
}

/// `git check-ref-format --branch`.
pub fn valid_branch_name(repo: &Path, branch: &str) -> bool {
    output(repo, &["check-ref-format", "--branch", branch]).is_ok_and(|o| o.ok)
}

/// A full ref (`refs/heads/x`, `refs/tags/x`...) exists.
pub fn ref_exists(repo: &Path, full_ref: &str) -> bool {
    !full_ref.starts_with('-')
        && output(repo, &["show-ref", "--verify", "--quiet", full_ref]).is_ok_and(|o| o.ok)
}

/// `a` is an ancestor of (or equal to) `b`.
pub fn is_ancestor(repo: &Path, a: &str, b: &str) -> bool {
    !a.starts_with('-')
        && !b.starts_with('-')
        && output(repo, &["merge-base", "--is-ancestor", a, b]).is_ok_and(|o| o.ok)
}

// ---------------------------------------------------------------------------
// Task facts and diff
// ---------------------------------------------------------------------------

/// The base to compare against: `base` if it still resolves, else the commit
/// recorded at creation.
pub fn base_rev(dir: &Path, base: &str, base_commit: &str) -> String {
    resolve_commit(dir, base).unwrap_or_else(|| base_commit.to_string())
}

/// `merge-base(base, HEAD)` of a worktree (the base itself when there is none).
pub fn merge_base(worktree: &Path, base: &str, base_commit: &str) -> String {
    let base = base_rev(worktree, base, base_commit);
    match output(worktree, &["merge-base", &base, "HEAD"]) {
        Ok(o) if o.ok && !o.stdout.trim().is_empty() => o.stdout.trim().to_string(),
        _ => base,
    }
}

/// Contents of `path` (`/`-separated, relative to the repository root) at `rev`.
pub fn show_file(dir: &Path, rev: &str, path: &str) -> Result<String, String> {
    if rev.starts_with('-') {
        return Err(format!("invalid revision {rev:?}"));
    }
    run(dir, &["show", &format!("{rev}:{path}")])
}

/// Branch, ahead/behind, dirty count, unpushed count and last commit of a worktree.
pub fn facts(worktree: &Path, base: &str, base_commit: &str) -> Result<TaskGit, String> {
    let base = base_rev(worktree, base, base_commit);
    let mut g = TaskGit {
        head: resolve_commit(worktree, "HEAD"),
        ..Default::default()
    };
    let b = output(worktree, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if b.ok {
        g.branch = Some(b.stdout.trim().to_string());
    }
    let counts = run(
        worktree,
        &[
            "rev-list",
            "--left-right",
            "--count",
            &format!("{base}...HEAD"),
        ],
    )?;
    (g.behind, g.ahead) = parse_left_right(&counts)
        .ok_or_else(|| format!("unexpected rev-list output {counts:?}"))?;
    let unpushed = run(
        worktree,
        &["rev-list", "--count", "HEAD", "--not", &base, "--remotes"],
    )?;
    g.unpushed = unpushed.trim().parse().unwrap_or(0);
    // A worker's tree: no filter program, no status run in a nested repo.
    let (cfg, _) = guard(worktree, None)?;
    let status = run_guarded(
        worktree,
        &cfg,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=dirty",
        ],
    )?;
    g.dirty = count_status_entries(&status);
    let log = output(
        worktree,
        &["log", "-1", "--format=%H%x00%s%x00%an%x00%ct", "HEAD"],
    )?;
    if log.ok {
        g.last_commit = parse_commit(&log.stdout);
    }
    Ok(g)
}

/// Worktree against the merge base of the base and HEAD: tracked changes
/// (committed, staged or not) from `git diff --numstat` + `--name-status`,
/// plus untracked files counted as fully added.
pub fn diff(
    worktree: &Path,
    base: &str,
    base_commit: &str,
) -> Result<(String, Vec<DiffFile>), String> {
    let base = base_rev(worktree, base, base_commit);
    let mb = output(worktree, &["merge-base", &base, "HEAD"])?;
    let merge_base = if mb.ok && !mb.stdout.trim().is_empty() {
        mb.stdout.trim().to_string()
    } else {
        base.clone()
    };
    // A worker's tree: no filter program, no status run in a nested repo.
    let (cfg, _) = guard(worktree, None)?;
    let common = [
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--ignore-submodules=dirty",
        "-M",
        "-z",
    ];
    let numstat = run_guarded(
        worktree,
        &cfg,
        &[&common[..], &["--numstat", &merge_base, "--"]].concat(),
    )?;
    let names = run_guarded(
        worktree,
        &cfg,
        &[&common[..], &["--name-status", &merge_base, "--"]].concat(),
    )?;
    let mut files = merge_diff(&parse_name_status(&names), &parse_numstat(&numstat));
    let untracked = run(
        worktree,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )?;
    for path in untracked.split('\0').filter(|p| !p.is_empty()) {
        let (added, binary) = count_lines(&worktree.join(path));
        files.push(DiffFile {
            path: path.to_string(),
            old_path: None,
            status: "?".into(),
            added,
            deleted: (!binary).then_some(0),
            binary,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((merge_base, files))
}

/// Lines in a new file (`None` + binary for files with NUL bytes, or too big to read).
fn count_lines(path: &Path) -> (Option<u64>, bool) {
    let too_big = std::fs::metadata(path).map_or(true, |m| m.len() > MAX_COUNT_BYTES);
    if too_big {
        return (None, false);
    }
    let Ok(bytes) = std::fs::read(path) else {
        return (None, false);
    };
    if bytes[..bytes.len().min(8000)].contains(&0) {
        return (None, true);
    }
    let mut n = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
    if bytes.last().is_some_and(|&b| b != b'\n') {
        n += 1;
    }
    (Some(n), false)
}
