//! docs/19 §18.5 (re-score daemon-security-01 / orch-quality-01 /
//! ui-editor-01): `orch.commit` and the task git facts run git outside the
//! worker's sandbox, so no program the repository configures may run on a
//! worker's tree. Fixture: `filter.demo.clean = sh clean.sh`, with
//! `clean.sh` tracked in the worktree (the worker can rewrite it) and
//! writing a marker outside the worktree. No agents, no daemon.

use std::path::{Path, PathBuf};
use std::process::Command;

use podd::git;

fn g(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
    no_hooks: PathBuf,
    marker: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn u(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

/// A repository whose config routes `*.dat` through `sh clean.sh`; the
/// script is tracked and appends to `marker` (outside every worktree).
fn fixture(tag: &str) -> Fx {
    let root = std::env::temp_dir().join(format!("podd-wcf-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (repo, wt, no_hooks) = (root.join("repo"), root.join("wt"), root.join("no-hooks"));
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&no_hooks).unwrap();
    let marker = root.join("outside-worker.marker");
    g(&repo, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "t"),
        ("user.email", "t@example.invalid"),
        ("commit.gpgsign", "false"),
        ("core.autocrlf", "true"),
        ("filter.demo.clean", "sh clean.sh"),
        ("filter.demo.smudge", "cat"),
        ("filter.demo.required", "true"),
    ] {
        g(&repo, &["config", k, v]);
    }
    std::fs::write(
        repo.join("clean.sh"),
        format!("echo clean >> '{}'\ncat\n", u(&marker)),
    )
    .unwrap();
    std::fs::write(repo.join(".gitattributes"), "*.dat filter=demo\n").unwrap();
    std::fs::write(repo.join("a.dat"), "data\n").unwrap();
    std::fs::write(repo.join("keep.txt"), "keep\n").unwrap();
    std::fs::write(repo.join("gone.txt"), "gone\n").unwrap();
    g(&repo, &["add", "-A"]);
    g(&repo, &["commit", "-q", "-m", "init"]);
    assert!(
        marker.exists(),
        "control: the filter is live for a plain `git add`"
    );
    std::fs::remove_file(&marker).unwrap();
    g(&repo, &["worktree", "add", "-q", "-b", "pod/w", &u(&wt)]);
    let _ = std::fs::remove_file(&marker);
    Fx {
        root,
        repo,
        wt,
        no_hooks,
        marker,
    }
}

#[test]
fn worker_commit_runs_no_repository_filter() {
    let f = fixture("commit");
    let wt = &f.wt;
    // The worker rewrites the filter's script and adds files.
    std::fs::write(
        wt.join("clean.sh"),
        format!("echo pwned >> '{}'\ncat\n", u(&f.marker)),
    )
    .unwrap();
    std::fs::write(wt.join("new.txt"), "line\r\n").unwrap();
    let (sha, subject) = git::commit_for_worker(
        wt,
        &f.no_hooks,
        "worker change  \n\n\n body \n",
        false,
        &["clean.sh".into(), "new.txt".into()],
    )
    .unwrap();
    assert_eq!((sha.len(), subject.as_str()), (40, "worker change"));
    assert!(!f.marker.exists(), "no filter program ran");
    assert_eq!(
        g(wt, &["log", "-1", "--format=%B"]),
        "worker change\n\n body\n\n"
    );
    assert_eq!(
        g(wt, &["show", "--name-only", "--format=", "HEAD"]).trim(),
        "clean.sh\nnew.txt"
    );
    // Built-in conversions still apply (core.autocrlf: CRLF → LF), and the
    // script went in as the worker wrote it.
    assert_eq!(g(wt, &["cat-file", "blob", "HEAD:new.txt"]), "line\n");
    assert!(g(wt, &["cat-file", "blob", "HEAD:clean.sh"]).contains("pwned"));
    assert!(g(&f.repo, &["reflog", "-1", "pod/w"]).contains("commit: worker change"));

    // A path the configured filter would handle: refused, nothing runs.
    let checked_out = std::fs::read(wt.join("a.dat")).unwrap();
    std::fs::write(wt.join("a.dat"), "changed\n").unwrap();
    let e = git::commit_for_worker(wt, &f.no_hooks, "data", true, &[]).unwrap_err();
    assert!(e.contains("a.dat (filter=demo)"), "{e}");
    assert!(e.contains("ask a human"), "{e}");
    assert!(!f.marker.exists(), "refused before any filter could run");
    let e = git::commit_for_worker(wt, &f.no_hooks, "data", false, &["a.dat".into()]).unwrap_err();
    assert!(e.contains("filter=demo"), "{e}");
    // The checked-out bytes again (same size, new mtime): comparing them
    // needs the content, which is where git would run the clean filter.
    std::fs::write(wt.join("a.dat"), &checked_out).unwrap();

    // Task facts and the review diff read the same tree: no filter either.
    let base = g(&f.repo, &["rev-parse", "main"]).trim().to_string();
    let facts = git::facts(wt, "main", &base).unwrap();
    assert_eq!(facts.ahead, 1);
    git::diff(wt, "main", &base).unwrap();
    assert!(!f.marker.exists(), "facts/diff ran no filter");

    // A deletion and a change, named.
    std::fs::remove_file(wt.join("gone.txt")).unwrap();
    std::fs::write(wt.join("keep.txt"), "kept\n").unwrap();
    let rest = ["gone.txt".to_string(), "keep.txt".into()];
    git::commit_for_worker(wt, &f.no_hooks, "rest", false, &rest).unwrap();
    let files = g(wt, &["show", "--name-status", "--format=", "HEAD"]);
    assert_eq!(files.trim(), "D\tgone.txt\nM\tkeep.txt", "{files}");
    assert!(!f.marker.exists());
    assert!(
        git::commit_for_worker(wt, &f.no_hooks, "again", false, &rest[1..])
            .unwrap_err()
            .contains("nothing to commit")
    );

    // The control: a plain `git status` there runs the worker's script
    // (touched again: `git diff` above refreshed the index entry).
    std::fs::write(wt.join("a.dat"), &checked_out).unwrap();
    g(wt, &["status", "--porcelain"]);
    assert!(f.marker.exists(), "control: the worker's script is live");
}

#[test]
fn worker_commit_refuses_nested_repos_links_and_bad_paths() {
    let f = fixture("refuse");
    let wt = &f.wt;
    std::fs::create_dir_all(wt.join("sub")).unwrap();
    g(&wt.join("sub"), &["init", "-q"]);
    let e = git::commit_for_worker(wt, &f.no_hooks, "x", true, &[]).unwrap_err();
    assert!(e.contains("sub/ is a nested git repository"), "{e}");
    std::fs::remove_dir_all(wt.join("sub")).unwrap();
    // Unknown, ignored-or-missing and outside paths: refused like git add.
    for bad in ["missing.txt", "../outside.txt"] {
        assert!(git::commit_for_worker(wt, &f.no_hooks, "x", false, &[bad.into()]).is_err());
    }
    assert!(
        git::commit_for_worker(wt, &f.no_hooks, " \n ", true, &[])
            .unwrap_err()
            .contains("empty")
    );
    // A symbolic link (only where this machine lets us make one).
    let target = f.root.join("secret.txt");
    std::fs::write(&target, "secret").unwrap();
    if std::os::windows::fs::symlink_file(&target, wt.join("link.txt")).is_ok() {
        let e = git::commit_for_worker(wt, &f.no_hooks, "x", true, &[]).unwrap_err();
        assert!(e.contains("symbolic link"), "{e}");
    }
    assert!(!f.marker.exists());
}

/// Refactor D-P5 (docs/20-refactor/daemon.md): what the git layer returns
/// for one fixture, written down before `git.rs` was split into modules and
/// unchanged by the split. Names with spaces, Korean and a leading dash, a
/// rename, `-z` status and diff output, a staged rename, binary and text
/// untracked files, refs that look like options, the worktree listing and
/// the refusal to remove a dirty worktree without force. No filter runs.
#[test]
fn git_results_are_pinned() {
    let f = fixture("pin");
    let (repo, wt) = (&f.repo, &f.wt);
    let mut got: Vec<String> = Vec::new();
    let mut rec = |s: String| got.push(s);

    // Refs and queries; option-like revisions are refused before git runs.
    let base = g(repo, &["rev-parse", "main"]).trim().to_string();
    rec(format!("repo_root(repo) = {:?}", git::repo_root(repo)));
    rec(format!("repo_root(wt) = {:?}", git::repo_root(wt)));
    rec(format!("default_base = {}", git::default_base(repo)));
    rec(format!(
        "resolve_commit main = base: {}, -x: {:?}",
        git::resolve_commit(repo, "main").as_deref() == Some(base.as_str()),
        git::resolve_commit(repo, "-x"),
    ));
    rec(format!(
        "branch_exists pod/w {}, nope {}; valid_branch_name bad..name {}, ok/x {}",
        git::branch_exists(repo, "pod/w"),
        git::branch_exists(repo, "nope"),
        git::valid_branch_name(repo, "bad..name"),
        git::valid_branch_name(repo, "ok/x"),
    ));
    rec(format!(
        "ref_exists refs/heads/main {}, --all {}; is_ancestor main pod/w {}, -x main {}",
        git::ref_exists(repo, "refs/heads/main"),
        git::ref_exists(repo, "--all"),
        git::is_ancestor(repo, "main", "pod/w"),
        git::is_ancestor(repo, "-x", "main"),
    ));
    rec(format!(
        "show_file main:keep.txt {:?}; --output=x {:?}",
        git::show_file(repo, "main", "keep.txt"),
        git::show_file(repo, "--output=x", "keep.txt"),
    ));
    rec(format!(
        "base_rev gone = base: {}; merge_base = base: {}",
        git::base_rev(wt, "gone", &base) == base,
        git::merge_base(wt, "main", &base) == base,
    ));

    // Worker commits: odd names (pathspecs after `--`), then a rename.
    std::fs::create_dir_all(wt.join("dir with space")).unwrap();
    std::fs::write(wt.join("dir with space").join("a b.txt"), "a b\n").unwrap();
    std::fs::write(wt.join("한글 é.txt"), "k\n").unwrap();
    std::fs::write(wt.join("-dash.txt"), "d\n").unwrap();
    let names = [
        "dir with space/a b.txt".to_string(),
        "한글 é.txt".into(),
        "-dash.txt".into(),
    ];
    let r = git::commit_for_worker(wt, &f.no_hooks, "names", false, &names);
    rec(format!("commit names = {:?}", r.map(|(_, s)| s)));
    let shown = || {
        g(
            wt,
            &["show", "--name-status", "-M", "-z", "--format=", "HEAD"],
        )
        .replace('\0', "|")
    };
    rec(format!("  HEAD files {}", shown()));
    std::fs::rename(wt.join("keep.txt"), wt.join("kept name.txt")).unwrap();
    let r = git::commit_for_worker(wt, &f.no_hooks, "rename\n\nbody", true, &[]);
    rec(format!("commit rename = {:?}", r.map(|(_, s)| s)));
    rec(format!("  HEAD files {}", shown()));

    // A dirty tree: unstaged change, staged rename, untracked text and binary.
    std::fs::write(wt.join("kept name.txt"), "keep\nmore\n").unwrap();
    g(wt, &["mv", "--", "-dash.txt", "dash renamed.txt"]);
    std::fs::write(wt.join("new file.bin"), [0u8, 1, 2, 0, 3]).unwrap();
    std::fs::write(wt.join("notes 2.txt"), "1\n2\n3").unwrap();
    let t = git::facts(wt, "main", &base).unwrap();
    rec(format!(
        "facts ahead {} behind {} dirty {} unpushed {} branch {:?} head {} last {:?}",
        t.ahead,
        t.behind,
        t.dirty,
        t.unpushed,
        t.branch,
        t.head == git::resolve_commit(wt, "HEAD"),
        t.last_commit.map(|c| c.summary),
    ));
    let (mb, files) = git::diff(wt, "main", &base).unwrap();
    rec(format!("diff merge base = base: {}", mb == base));
    for d in files {
        rec(format!(
            "  {} {:?} {:?} +{:?} -{:?} binary {}",
            d.status, d.path, d.old_path, d.added, d.deleted, d.binary
        ));
    }

    // Worktrees: listing, refusal on a dirty tree, forced removal.
    for w in git::worktree_list(repo).unwrap() {
        rec(format!(
            "worktree {} branch {:?} head {} bare {} detached {} prunable {} locked {}",
            w.path.display(),
            w.branch,
            w.head.is_some(),
            w.bare,
            w.detached,
            w.prunable,
            w.locked,
        ));
    }
    let e = git::worktree_remove(repo, wt, false).unwrap_err();
    let (head, _git_says) = e.split_once(": ").unwrap_or((&e, ""));
    rec(format!(
        "remove dirty = Err({head}: …), kept {}",
        wt.exists()
    ));
    rec(format!(
        "remove --force = {:?}, kept {}",
        git::worktree_remove(repo, wt, true),
        wt.exists()
    ));
    rec(format!(
        "prune {:?}, branch_delete {:?}, pod/w exists {}",
        git::worktree_prune(repo),
        git::branch_delete(repo, "pod/w"),
        git::branch_exists(repo, "pod/w"),
    ));
    rec(format!("marker {}", f.marker.exists()));

    // The temp root as written, as git prints it, and inside `{:?}` output.
    let mut got = got.join("\n");
    for r in [git::normalize(&f.root), f.root.clone()] {
        let plain = r.display().to_string();
        for spelling in [plain.replace('\\', r"\\"), plain, u(&r)] {
            got = got.replace(&spelling, "<root>");
        }
    }
    assert_eq!(got, PINNED, "\n--- got ---\n{got}\n");
}

const PINNED: &str = r#"repo_root(repo) = Ok("<root>\\repo")
repo_root(wt) = Err("<root>\\wt is a linked worktree; add the main repository instead")
default_base = main
resolve_commit main = base: true, -x: None
branch_exists pod/w true, nope false; valid_branch_name bad..name false, ok/x true
ref_exists refs/heads/main true, --all false; is_ancestor main pod/w true, -x main false
show_file main:keep.txt Ok("keep\n"); --output=x Err("invalid revision \"--output=x\"")
base_rev gone = base: true; merge_base = base: true
commit names = Ok("names")
  HEAD files A|-dash.txt|A|dir with space/a b.txt|A|한글 é.txt|
commit rename = Ok("rename")
  HEAD files R100|keep.txt|kept name.txt|
facts ahead 2 behind 0 dirty 4 unpushed 2 branch Some("pod/w") head true last Some("rename")
diff merge base = base: true
  A "dash renamed.txt" None +Some(1) -Some(0) binary false
  A "dir with space/a b.txt" None +Some(1) -Some(0) binary false
  R "kept name.txt" Some("keep.txt") +Some(1) -Some(0) binary false
  ? "new file.bin" None +None -None binary true
  ? "notes 2.txt" None +Some(3) -Some(0) binary false
  A "한글 é.txt" None +Some(1) -Some(0) binary false
worktree <root>\repo branch Some("main") head true bare false detached false prunable false locked false
worktree <root>\wt branch Some("pod/w") head true bare false detached false prunable false locked false
remove dirty = Err(git worktree remove <root>\wt: …), kept true
remove --force = Ok(()), kept false
prune Ok(()), branch_delete Ok(()), pod/w exists false
marker false"#;
