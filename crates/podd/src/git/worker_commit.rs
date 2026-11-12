//! `orch.commit`: a commit made in a worktree for a sandboxed worker, through
//! git plumbing in bounded children, with no program the repository
//! configures (docs/19 §18.1, §18.5, §18.6).

use std::path::{Path, PathBuf};

use super::command::{config_facts, guard, output, run, run_bounded};
use super::pin::{Entry, Held};
use super::{is_inside, native_path, same_path};

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

const ASK_HUMAN: &str = "Commit it yourself with git (with approval) or ask a human";

fn inside_or_same(p: &Path, dir: &Path) -> bool {
    same_path(p, dir) || is_inside(p, dir)
}

/// The worktree's git folders. One inside the worktree other than its own
/// `.git` (a main checkout) is the worker's to write; refused. With
/// `repo` (a task worktree): a linked worktree of that repository.
fn git_dirs(worktree: &Path, repo: Option<&Path>) -> Result<(PathBuf, PathBuf), String> {
    let dirs = |dir: &Path| -> Result<(PathBuf, PathBuf), String> {
        let out = run(
            dir,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--git-dir",
                "--git-common-dir",
            ],
        )?;
        let mut l = out.lines();
        Ok((
            native_path(l.next().unwrap_or_default()),
            native_path(l.next().unwrap_or_default()),
        ))
    };
    let (git_dir, common) = dirs(worktree)?;
    let own = worktree.join(".git");
    for d in [&git_dir, &common] {
        if inside_or_same(d, worktree) && !same_path(d, &own) {
            return Err(format!(
                "the worktree's git folder {} is inside the worktree, where the worker can change its config and hooks. {ASK_HUMAN}",
                d.display()
            ));
        }
    }
    if let Some(repo) = repo {
        let (_, repo_common) = dirs(repo)?;
        if !same_path(&common, &repo_common) || inside_or_same(&git_dir, worktree) {
            return Err(format!(
                "{} is not a linked worktree of {} any more. {ASK_HUMAN}",
                worktree.display(),
                repo.display()
            ));
        }
    }
    Ok((git_dir, common))
}

/// Pod's empty hooks folder: a plain empty folder outside the worktree, or
/// `core.hooksPath` would name hooks after all.
fn check_no_hooks(no_hooks: &Path, worktree: &Path) -> Result<(), String> {
    let bad = |why: &str| {
        Err(format!(
            "Pod's empty hooks folder {} {why}; podd does not commit for workers until it is a plain empty folder",
            no_hooks.display()
        ))
    };
    match super::pin::peek(no_hooks) {
        Ok(Some(f)) if f.is_dir() && !f.is_link() => {}
        Ok(Some(_)) => return bad("is a link or a file"),
        Ok(None) => return bad("is missing"),
        Err(e) => return bad(&e.to_string()),
    }
    if inside_or_same(no_hooks, worktree) {
        return bad("is inside the worktree");
    }
    match std::fs::read_dir(no_hooks) {
        Ok(mut d) => match d.next() {
            None => Ok(()),
            Some(_) => bad("is not empty"),
        },
        Err(e) => bad(&e.to_string()),
    }
}

/// Config the worker could edit: a file read or named as an include
/// (missing ones too: git skips them until they appear) inside the
/// worktree but not in its git folders. A filter driver or hook path added
/// there after podd's guard read would run in podd.
fn check_sources(
    worktree: &Path,
    git_dir: &Path,
    common: &Path,
    sources: &[PathBuf],
) -> Result<(), String> {
    for s in sources {
        if inside_or_same(s, worktree) && !inside_or_same(s, git_dir) && !inside_or_same(s, common)
        {
            return Err(format!(
                "the repository's git config reads {} inside the worktree, where the worker can add a filter or hook program after podd's check. {ASK_HUMAN}",
                s.display()
            ));
        }
    }
    Ok(())
}

/// A test hook run between the steps (`listed`, `held`, `hashed`).
#[cfg(test)]
type Hook = Option<Box<dyn FnMut(&'static str)>>;

#[cfg(test)]
thread_local! {
    pub(super) static AT: std::cell::RefCell<Hook> = const { std::cell::RefCell::new(None) };
}

fn at(_stage: &'static str) {
    #[cfg(test)]
    AT.with(|h| {
        if let Some(f) = h.borrow_mut().as_mut() {
            f(_stage)
        }
    });
}

/// Stage and commit in a worktree on behalf of a sandboxed worker
/// (`orch.commit`, docs/19 §18.1, §18.5, §18.6). git runs here, outside
/// any sandbox, so nothing the repository names as a program may run:
///
/// - plumbing only (`ls-files`, `diff-files`, `check-attr`, `hash-object`,
///   `update-index`, `write-tree`, `commit-tree`, `update-ref`), each child
///   bounded (time, output), a fixed number of calls;
/// - a fresh guard from this commit's config: hooks from `no_hooks` (a
///   plain empty folder, checked), fsmonitor, signing and automatic
///   maintenance off, every configured filter driver emptied; inherited
///   `GIT_*` overrides dropped;
/// - refused: a path whose `filter` attribute names any driver (LFS, say),
///   config read from inside the worktree, git folders inside it, links,
///   junctions and other reparse points, hard links, names Windows reads
///   as streams or devices, nested repositories (submodule pointers are
///   left alone);
/// - the files git hashes are held open from the check to the end of
///   `hash-object` (`Held`: no rename, delete, write or swap), then
///   re-read, and the config is read again: any change, nothing commits.
///
/// The worktree's top level must be `worktree` itself. Returns (full hash,
/// subject).
pub fn commit_for_worker(
    worktree: &Path,
    no_hooks: &Path,
    message: &str,
    all: bool,
    paths: &[String],
) -> Result<(String, String), String> {
    commit(worktree, None, no_hooks, message, all, paths)
}

/// [`commit_for_worker`] in a task's worktree: also a linked worktree of
/// `repo` (same common git folder, its own git folder outside the tree).
pub fn commit_for_task_worker(
    worktree: &Path,
    repo: &Path,
    no_hooks: &Path,
    message: &str,
    all: bool,
    paths: &[String],
) -> Result<(String, String), String> {
    commit(worktree, Some(repo), no_hooks, message, all, paths)
}

fn commit(
    worktree: &Path,
    repo: Option<&Path>,
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
    let (git_dir, common) = git_dirs(worktree, repo)?;
    check_no_hooks(no_hooks, worktree)?;
    let config = config_facts(worktree)?;
    check_sources(worktree, &git_dir, &common, &config.sources)?;
    let (cfg, _drivers) = guard(worktree, Some(no_hooks))?;
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
    at("listed");
    // From here to the end of `hash-object`, every file to stage and every
    // folder above it is held (TM-21): what was checked is what git reads.
    let mut held = Held::root(worktree).map_err(|e| format!("{e}. {ASK_HUMAN}"))?;
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
        match held
            .entry(&p)
            .map_err(|e| format!("{e}; podd does not commit it for workers. {ASK_HUMAN}"))?
        {
            Entry::File => stage.push(p),
            // Deleted, or a folder now (its files are listed as new).
            Entry::Missing | Entry::Dir => {
                if modes.contains_key(&p) {
                    remove.push(p);
                }
            }
        }
    }
    at("held");
    if !stage.is_empty() {
        // Any driver name, configured or not: one configured later (by
        // anyone) must not find a path podd is about to hash.
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
                [path, _, value] if !matches!(*value, "unspecified" | "unset" | "set") => {
                    Some(format!("{path} (filter={value})"))
                }
                _ => None,
            })
            .collect();
        if !filtered.is_empty() {
            return Err(format!(
                "these paths go through a git filter (LFS or a program the repository configures), and podd does not run repository programs for workers: {}. {ASK_HUMAN}",
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
    at("hashed");
    held.recheck()
        .map_err(|e| format!("{e}; nothing was committed"))?;
    drop(held);
    if config_facts(worktree)? != config {
        return Err(
            "the repository's git config (filter drivers or config files) changed while podd was committing; nothing was committed"
                .into(),
        );
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
        commit_for_task_worker(&wt, &repo, &no_hooks, "rest", true, &[]).unwrap();
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
        // The main checkout is not a task worktree of the repository.
        std::fs::write(repo.join("m.txt"), "m").unwrap();
        let e = commit_for_task_worker(&repo, &repo, &no_hooks, "x", true, &[]).unwrap_err();
        assert!(e.contains("not a linked worktree"), "{e}");
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

    struct Fx {
        root: PathBuf,
        repo: PathBuf,
        wt: PathBuf,
        no_hooks: PathBuf,
        outside: PathBuf,
    }
    impl Drop for Fx {
        fn drop(&mut self) {
            AT.with(|h| h.borrow_mut().take());
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// A repository with `d/f.txt` and a linked worktree; `outside` holds a
    /// `secret.txt` no commit may contain.
    fn race_fixture(tag: &str) -> Fx {
        let root = std::env::temp_dir().join(format!("podd-wc-race-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (repo, wt, no_hooks, outside) = (
            root.join("repo"),
            root.join("wt"),
            root.join("no-hooks"),
            root.join("outside"),
        );
        for d in [repo.join("d"), no_hooks.clone(), outside.clone()] {
            std::fs::create_dir_all(d).unwrap();
        }
        std::fs::write(outside.join("f.txt"), "secret\n").unwrap();
        let g = |dir: &Path, args: &[&str]| run(dir, args).unwrap();
        g(&repo, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "t"),
            ("user.email", "t@example.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            g(&repo, &["config", k, v]);
        }
        std::fs::write(repo.join("d").join("f.txt"), "one\n").unwrap();
        g(&repo, &["add", "-A"]);
        g(&repo, &["commit", "-q", "-m", "init"]);
        g(
            &repo,
            &["worktree", "add", "-q", "-b", "pod/w", wt.to_str().unwrap()],
        );
        Fx {
            root,
            repo,
            wt,
            no_hooks,
            outside,
        }
    }

    fn on(stage: &'static str, mut f: impl FnMut() + 'static) {
        AT.with(|h| {
            *h.borrow_mut() = Some(Box::new(move |s| {
                if s == stage {
                    f()
                }
            }))
        });
    }

    fn head(wt: &Path) -> String {
        run(wt, &["rev-parse", "HEAD"]).unwrap()
    }

    /// TM-21 "swap during check": after podd listed `d/f.txt` but before it
    /// holds it, the worker moves `d` away and puts a junction to a folder
    /// outside in its place. The hold sees the junction: refused, nothing
    /// from outside is hashed or committed.
    #[test]
    fn a_swap_before_the_hold_is_refused() {
        let f = race_fixture("before");
        std::fs::write(f.wt.join("d").join("f.txt"), "two\n").unwrap();
        let (wt, outside) = (f.wt.clone(), f.outside.clone());
        on("listed", move || {
            std::fs::rename(wt.join("d"), wt.join("d-moved")).unwrap();
            assert!(super::super::pin::junction(&wt.join("d"), &outside));
        });
        let before = head(&f.wt);
        let e = commit_for_worker(&f.wt, &f.no_hooks, "x", true, &[]).unwrap_err();
        assert!(e.contains("reparse point"), "{e}");
        assert_eq!(head(&f.wt), before, "no commit");
        let objects = run(&f.repo, &["count-objects", "-v"]).unwrap();
        let blob = run(
            &f.repo,
            &[
                "hash-object",
                "--",
                &f.outside.join("f.txt").display().to_string(),
            ],
        )
        .unwrap();
        assert!(
            run(&f.repo, &["cat-file", "-e", blob.trim()]).is_err(),
            "the outside file was never hashed into the repository: {objects}"
        );
    }

    /// TM-21: once held, the same swap and any write fail (sharing
    /// violation) until git has read the file; the commit has the content
    /// podd checked.
    #[test]
    fn a_swap_after_the_hold_cannot_happen() {
        let f = race_fixture("after");
        std::fs::write(f.wt.join("d").join("f.txt"), "two\n").unwrap();
        let (wt, outside) = (f.wt.clone(), f.outside.clone());
        let tried = std::rc::Rc::new(std::cell::Cell::new(false));
        let t2 = tried.clone();
        on("held", move || {
            assert!(std::fs::rename(wt.join("d"), wt.join("d-moved")).is_err());
            assert!(std::fs::write(wt.join("d").join("f.txt"), "swapped\n").is_err());
            assert!(std::fs::remove_file(wt.join("d").join("f.txt")).is_err());
            assert!(!super::super::pin::junction(&wt.join("d"), &outside));
            t2.set(true);
        });
        commit_for_worker(&f.wt, &f.no_hooks, "x", true, &[]).unwrap();
        assert!(tried.get());
        assert_eq!(
            run(&f.wt, &["cat-file", "blob", "HEAD:d/f.txt"]).unwrap(),
            "two\n"
        );
    }

    /// TM-21 "new driver during the commit": a filter driver configured
    /// while podd commits (by anyone) stops the commit before the ref moves.
    #[test]
    fn a_driver_configured_during_the_commit_stops_it() {
        let f = race_fixture("driver");
        std::fs::write(f.wt.join("d").join("f.txt"), "two\n").unwrap();
        let repo = f.repo.clone();
        on("hashed", move || {
            run(&repo, &["config", "filter.late.clean", "sh late.sh"]).unwrap();
        });
        let before = head(&f.wt);
        let e = commit_for_worker(&f.wt, &f.no_hooks, "x", true, &[]).unwrap_err();
        assert!(
            e.contains("config") && e.contains("nothing was committed"),
            "{e}"
        );
        assert_eq!(head(&f.wt), before);
    }

    /// Pod's hooks folder must be a plain empty folder outside the tree.
    #[test]
    fn the_no_hooks_folder_is_checked() {
        let f = race_fixture("nohooks");
        std::fs::write(f.wt.join("d").join("f.txt"), "two\n").unwrap();
        std::fs::write(f.no_hooks.join("reference-transaction"), "#!/bin/sh\n").unwrap();
        let e = commit_for_worker(&f.wt, &f.no_hooks, "x", true, &[]).unwrap_err();
        assert!(e.contains("not empty"), "{e}");
        let j = f.root.join("no-hooks-link");
        assert!(super::super::pin::junction(&j, &f.outside));
        let e = commit_for_worker(&f.wt, &j, "x", true, &[]).unwrap_err();
        assert!(e.contains("link"), "{e}");
        let inside = f.wt.join("nh");
        std::fs::create_dir_all(&inside).unwrap();
        let e = commit_for_worker(&f.wt, &inside, "x", true, &[]).unwrap_err();
        assert!(e.contains("inside the worktree"), "{e}");
        std::fs::remove_file(f.no_hooks.join("reference-transaction")).unwrap();
        std::fs::remove_dir(&inside).unwrap();
        commit_for_worker(&f.wt, &f.no_hooks, "x", true, &[]).unwrap();
    }
}
