//! `orch.commit`: a commit made in a worktree for a sandboxed worker, through
//! git plumbing in bounded children, with no program the repository
//! configures (docs/19 §18.1, §18.5).

use std::path::Path;

use super::command::{guard, output, run, run_bounded};
use super::{native_path, same_path};

/// `path` for `hash-object --stdin-paths`, which C-unquotes a line that
/// starts with `"`: always quote so every name means itself.
fn c_quote(path: &str) -> String {
    let mut s = String::with_capacity(path.len() + 2);
    s.push('"');
    for c in path.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            '\r' => s.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                s.push_str(&format!("\\{:03o}", c as u32))
            }
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// `commit -m` cleanup ("whitespace"): trailing spaces off each line,
/// blank runs collapsed, no blank lines at either end, one final newline.
fn clean_message(m: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for line in m.lines().map(str::trim_end) {
        if line.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("{}\n", out.join("\n"))
    }
}

/// `rel` (as git printed it: `/`-separated, relative to `top`) and every
/// folder above it are real entries, not links or junctions.
fn no_link_on_the_way(top: &Path, rel: &str) -> Result<(), String> {
    let mut p = top.to_path_buf();
    for part in rel.split('/').filter(|s| !s.is_empty()) {
        if part == ".." {
            return Err(format!("{rel}: the path leaves the worktree"));
        }
        p.push(part);
        if std::fs::symlink_metadata(&p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{rel} is (or is under) a symbolic link or junction; podd does not commit links for workers. {ASK_HUMAN}"
            ));
        }
    }
    Ok(())
}

const ASK_HUMAN: &str = "Commit it yourself with git (with approval) or ask a human";

/// Stage and commit in a worktree on behalf of a sandboxed worker
/// (`orch.commit`, docs/19 §18.1, §18.5). git runs here, outside any
/// sandbox, in the worktree Pod recorded, so nothing the repository names
/// as a program may run: plumbing only (`ls-files`, `diff-files`,
/// `hash-object`, `update-index`, `write-tree`, `commit-tree`,
/// `update-ref`), hooks from `no_hooks` (an empty folder), fsmonitor,
/// signing and automatic maintenance off and every configured filter
/// driver emptied. A path whose attributes name a configured filter driver
/// (LFS, say) is refused: committing it unfiltered would be wrong, and
/// running the filter is what this guards against. Links, junctions and
/// nested repositories are refused too; submodule pointers are left alone.
/// Each git child is bounded (time, output). The worktree's top level must
/// be `worktree` itself. Returns (full hash, subject).
pub fn commit_for_worker(
    worktree: &Path,
    no_hooks: &Path,
    message: &str,
    all: bool,
    paths: &[String],
) -> Result<(String, String), String> {
    let top = run(worktree, &["rev-parse", "--show-toplevel"])?;
    if !same_path(&native_path(&top), worktree) {
        return Err(format!(
            "{} is not the top of its git worktree ({})",
            worktree.display(),
            top.trim()
        ));
    }
    let message = clean_message(message);
    if message.is_empty() {
        return Err("the commit message must not be empty".into());
    }
    if !all && paths.is_empty() {
        return Err("say what to commit: --all or paths".into());
    }
    let (cfg, drivers) = guard(worktree, Some(no_hooks))?;
    let git = |args: &[&str]| run_bounded(worktree, &cfg, args, None);
    let git_in = |args: &[&str], stdin: Vec<u8>| run_bounded(worktree, &cfg, args, Some(stdin));
    // `--all`: the whole tree; otherwise the paths (git pathspecs).
    let spec: Vec<&str> = if all {
        Vec::new()
    } else {
        paths.iter().map(String::as_str).collect()
    };
    let with_spec = |head: &[&'static str]| -> Vec<&str> {
        let mut a: Vec<&str> = head.to_vec();
        if !spec.is_empty() {
            a.push("--");
            a.extend(spec.iter().copied());
        }
        a
    };
    if !spec.is_empty() {
        // Every path names something tracked, or new and not ignored,
        // inside the worktree (like `git add`).
        git(&with_spec(&[
            "ls-files",
            "-z",
            "--error-unmatch",
            "--cached",
            "--others",
            "--exclude-standard",
        ]))?;
    }
    // Tracked paths whose file differs from the index (no filter program
    // can run while comparing), then new files.
    let mut changed: Vec<String> = Vec::new();
    for list in [
        git(&with_spec(&[
            "diff-files",
            "-z",
            "--name-only",
            "--ignore-submodules=all",
            "--no-ext-diff",
            "--no-textconv",
        ]))?,
        git(&with_spec(&[
            "ls-files",
            "-z",
            "--others",
            "--exclude-standard",
        ]))?,
    ] {
        for p in list.split('\0').filter(|p| !p.is_empty()) {
            if !changed.iter().any(|c| c == p) {
                changed.push(p.to_string());
            }
        }
    }
    // Index modes: kept for tracked files, 100644 for new ones.
    let mut modes = std::collections::HashMap::new();
    for e in git(&with_spec(&["ls-files", "-z", "--stage"]))?.split('\0') {
        if let Some((meta, path)) = e.split_once('\t') {
            let mode = meta.split(' ').next().unwrap_or_default();
            modes.insert(path.to_string(), mode.to_string());
        }
    }
    let (mut stage, mut remove) = (Vec::new(), Vec::new());
    for p in changed {
        if p.ends_with('/') {
            return Err(format!(
                "{p} is a nested git repository; podd does not commit repositories for workers. {ASK_HUMAN}"
            ));
        }
        if modes.get(&p).is_some_and(|m| m == "160000") {
            continue;
        }
        no_link_on_the_way(worktree, &p)?;
        let is_file = std::fs::symlink_metadata(worktree.join(&p)).is_ok_and(|m| m.is_file());
        if is_file {
            stage.push(p);
        } else if modes.contains_key(&p) {
            // Deleted, or a folder now (its files are listed as new).
            remove.push(p);
        }
    }
    if !stage.is_empty() && !drivers.is_empty() {
        let mut input = Vec::new();
        for p in &stage {
            input.extend_from_slice(p.as_bytes());
            input.push(0);
        }
        let attrs = git_in(&["check-attr", "-z", "--stdin", "filter"], input)?;
        let fields: Vec<&str> = attrs.split('\0').collect();
        let filtered: Vec<String> = fields
            .chunks(3)
            .filter_map(|t| match t {
                [path, _, value] if drivers.iter().any(|d| d == value) => {
                    Some(format!("{path} (filter={value})"))
                }
                _ => None,
            })
            .collect();
        if !filtered.is_empty() {
            return Err(format!(
                "these paths go through a git filter program the repository configures, and podd does not run repository programs for workers: {}. {ASK_HUMAN}",
                filtered.join(", ")
            ));
        }
    }
    let mut info = String::new();
    if !stage.is_empty() {
        let input: String = stage.iter().map(|p| format!("{}\n", c_quote(p))).collect();
        let hashes = git_in(&["hash-object", "-w", "--stdin-paths"], input.into_bytes())?;
        let hashes: Vec<&str> = hashes.lines().collect();
        if hashes.len() != stage.len() {
            return Err(format!(
                "git hash-object: {} hashes for {} files",
                hashes.len(),
                stage.len()
            ));
        }
        for (p, h) in stage.iter().zip(hashes) {
            let mode = modes.get(p).map(String::as_str).unwrap_or("100644");
            info.push_str(&format!("{mode} {h}\t{p}\0"));
        }
    }
    if !remove.is_empty() {
        let sha256 = git(&["rev-parse", "--show-object-format"])?.trim() == "sha256";
        let zero = "0".repeat(if sha256 { 64 } else { 40 });
        for p in &remove {
            info.push_str(&format!("0 {zero}\t{p}\0"));
        }
    }
    if !info.is_empty() {
        git_in(&["update-index", "-z", "--index-info"], info.into_bytes())?;
    }
    let tree = git(&["write-tree"])?.trim().to_string();
    let parent = output(worktree, &["rev-parse", "-q", "--verify", "HEAD^{commit}"])?;
    let parent = parent.ok.then(|| parent.stdout.trim().to_string());
    if let Some(p) = &parent
        && git(&["rev-parse", &format!("{p}^{{tree}}")])?.trim() == tree
    {
        return Err("nothing to commit".into());
    }
    let mut args = vec!["commit-tree", tree.as_str()];
    if let Some(p) = &parent {
        args.extend(["-p", p.as_str()]);
    }
    args.extend(["-F", "-"]);
    let sha = git_in(&args, message.clone().into_bytes())?
        .trim()
        .to_string();
    let subject = message.lines().next().unwrap_or_default().to_string();
    let initial = if parent.is_none() { " (initial)" } else { "" };
    let reflog = format!("commit{initial}: {subject}");
    let old = parent.unwrap_or_else(|| "0".repeat(sha.len()));
    git(&["update-ref", "-m", &reflog, "HEAD", &sha, &old])?;
    Ok((sha, subject))
}

#[cfg(test)]
mod tests {
    use super::super::parse::parse_filter_drivers;
    use super::*;

    /// docs/19 §18.1: the repository's own config names a pre-commit hook
    /// and an fsmonitor that each write a marker; a worker's commit through
    /// podd runs neither, in a linked worktree too.
    #[test]
    fn commit_for_worker_runs_no_hooks_or_fsmonitor() {
        let t = std::env::temp_dir().join(format!("podd-git-commit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let (repo, wt, no_hooks) = (t.join("repo"), t.join("wt"), t.join("no-hooks"));
        std::fs::create_dir_all(repo.join("h")).unwrap();
        std::fs::create_dir_all(&no_hooks).unwrap();
        let g = |dir: &Path, args: &[&str]| run(dir, args).unwrap();
        g(&repo, &["init", "-q", "-b", "main"]);
        let u = |p: &Path| p.display().to_string().replace('\\', "/");
        let marker = t.join("marker");
        std::fs::write(
            repo.join("h").join("pre-commit"),
            format!("#!/bin/sh\necho hook >> '{}'\n", u(&marker)),
        )
        .unwrap();
        std::fs::write(
            repo.join("h").join("post-commit"),
            format!("#!/bin/sh\necho post >> '{}'\n", u(&marker)),
        )
        .unwrap();
        std::fs::write(
            repo.join("fsm"),
            format!("#!/bin/sh\necho fsm >> '{}'\nexit 1\n", u(&marker)),
        )
        .unwrap();
        for (k, v) in [
            ("user.name", "t".to_string()),
            ("user.email", "t@example.invalid".into()),
            ("commit.gpgsign", "false".into()),
            ("core.hooksPath", u(&repo.join("h"))),
            ("core.fsmonitor", u(&repo.join("fsm"))),
        ] {
            g(&repo, &["config", k, &v]);
        }
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        // The payloads are live: a plain commit fires them.
        g(&repo, &["add", "a.txt"]);
        g(&repo, &["commit", "-q", "-m", "init"]);
        assert!(
            marker.exists(),
            "control: the repository's hooks run on a plain commit"
        );
        std::fs::remove_file(&marker).unwrap();
        g(
            &repo,
            &["worktree", "add", "-q", "-b", "pod/w", wt.to_str().unwrap()],
        );
        std::fs::remove_file(&marker).ok();
        std::fs::write(wt.join("b.txt"), "b").unwrap();
        std::fs::write(wt.join("c.txt"), "c").unwrap();
        let (sha, subject) =
            commit_for_worker(&wt, &no_hooks, "worker change", false, &["b.txt".into()]).unwrap();
        assert_eq!((sha.len(), subject.as_str()), (40, "worker change"));
        assert!(!marker.exists(), "no hook or fsmonitor ran");
        assert_eq!(
            g(&wt, &["show", "--name-only", "--format=", "HEAD"]).trim(),
            "b.txt",
            "only the named path"
        );
        commit_for_worker(&wt, &no_hooks, "rest", true, &[]).unwrap();
        assert!(!marker.exists());
        assert_eq!(g(&wt, &["status", "--porcelain"]).trim(), "");
        // Nothing to commit, a path outside, or a folder that is not the
        // worktree's top level: refused.
        assert!(commit_for_worker(&wt, &no_hooks, "empty", true, &[]).is_err());
        assert!(
            commit_for_worker(
                &wt,
                &no_hooks,
                "x",
                false,
                &[t.join("marker2").display().to_string()]
            )
            .is_err()
        );
        std::fs::create_dir_all(wt.join("sub")).unwrap();
        assert!(
            commit_for_worker(&wt.join("sub"), &no_hooks, "x", true, &[])
                .unwrap_err()
                .contains("not the top")
        );
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn guard_parsers() {
        let cfg = "filter.lfs.clean\ngit-lfs clean -- %f\0filter.lfs.process\ngit-lfs filter-process\0\
                   filter.a.b.smudge\nsh x.sh\0filter.off.clean\n\0";
        assert_eq!(parse_filter_drivers(cfg), ["lfs", "a.b"]);
        assert_eq!(c_quote("a\"b\\c\nd\u{1}é"), "\"a\\\"b\\\\c\\nd\\001é\"");
        assert_eq!(clean_message("  \n a  \n\n\n b\t\n\n"), " a\n\n b\n");
        assert_eq!(clean_message(" \n\t\n"), "");
    }
}
