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
