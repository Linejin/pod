//! Threat model TM-21 (docs/22, docs/19 §18.6): the worker commit
//! boundary, as an isolated matrix. `orch.commit` runs git outside the
//! worker's sandbox, so no program the repository names may run, nothing
//! outside the worktree may be read into a commit, and config the worker
//! can edit is refused. Every payload writes a marker outside the
//! worktree; each program kind has an active control (plain git runs it).
//! No agents, no daemon. The swap-during-check barrier tests are unit tests
//! (`git::worker_commit`, `git::pin`), the rules-file and executable-pin
//! matrix is in `server::codex_allowance`.

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

/// A control: plain git, whose exit status the test does not need (a
/// failing signer or filter is part of the point).
fn g_any(dir: &Path, args: &[&str]) {
    let _ = Command::new("git").arg("-C").arg(dir).args(args).output();
}

fn u(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

struct Fx {
    root: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
    no_hooks: PathBuf,
    markers: PathBuf,
    outside: PathBuf,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Fx {
    /// Marker files written so far (`kind: lines`), sorted.
    fn marks(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(&self.markers)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let text = std::fs::read_to_string(e.path()).unwrap_or_default();
                format!(
                    "{}: {}",
                    e.file_name().to_string_lossy(),
                    text.lines().count()
                )
            })
            .collect();
        out.sort();
        out
    }

    fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.markers);
        std::fs::create_dir_all(&self.markers).unwrap();
    }

    fn commit(&self, all: bool, paths: &[&str]) -> Result<(String, String), String> {
        let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
        git::commit_for_task_worker(&self.wt, &self.repo, &self.no_hooks, "worker", all, &paths)
    }

    fn head(&self) -> String {
        g(&self.wt, &["rev-parse", "HEAD"])
    }

    /// The outside secret's blob was never written into the repository.
    fn secret_absent(&self) {
        let blob = g(
            &self.repo,
            &["hash-object", "--", &u(&self.outside.join("secret.txt"))],
        );
        let o = Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(["cat-file", "-e", blob.trim()])
            .output()
            .unwrap();
        assert!(!o.status.success(), "the outside secret was hashed in");
    }
}

/// A repository and a linked worktree `wt` (branch `pod/w`); `markers`
/// and `outside/secret.txt` are outside both. `setup` configures the
/// repository before the first commit; the first commit has signing off.
fn fixture(tag: &str, files: &[(&str, String)], setup: &[(&str, String)]) -> Fx {
    let root = std::env::temp_dir().join(format!("podd-wcb-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (repo, wt, no_hooks, markers, outside) = (
        root.join("repo"),
        root.join("wt"),
        root.join("no-hooks"),
        root.join("markers"),
        root.join("outside"),
    );
    for d in [&repo, &no_hooks, &markers, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(outside.join("secret.txt"), "outside secret\n").unwrap();
    g(&repo, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "t"),
        ("user.email", "t@example.invalid"),
        ("core.autocrlf", "false"),
    ] {
        g(&repo, &["config", k, v]);
    }
    for (rel, text) in files {
        let p = repo.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    std::fs::write(repo.join("keep.txt"), "keep\n").unwrap();
    for (k, v) in setup {
        g(&repo, &["config", k, v]);
    }
    g(&repo, &["add", "-A"]);
    g(
        &repo,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"],
    );
    g(&repo, &["worktree", "add", "-q", "-b", "pod/w", &u(&wt)]);
    let f = Fx {
        root,
        repo,
        wt,
        no_hooks,
        markers,
        outside,
    };
    f.clear();
    f
}

fn script(markers: &Path, kind: &str, then: &str) -> String {
    format!(
        "#!/bin/sh\necho {kind} >> '{}'\n{then}\n",
        u(&markers.join(kind))
    )
}

/// Clean and process filters, textconv, signing, every hook git runs on a
/// commit or a ref/index write, fsmonitor: each named by the repository
/// (scripts tracked in the worker's tree), none runs on a worker commit,
/// paths with a filter are refused, and plain git runs every one of them.
#[test]
fn no_repository_program_runs_on_a_worker_commit() {
    let m = std::env::temp_dir().join(format!("podd-wcb-progs-{}", std::process::id()));
    let markers = m.join("markers");
    let mut files: Vec<(&str, String)> = vec![
        ("tools/clean.sh", script(&markers, "clean", "cat")),
        ("tools/proc.sh", script(&markers, "process", "exit 1")),
        ("tools/tx.sh", script(&markers, "textconv", "cat \"$1\"")),
        ("tools/gpg.sh", script(&markers, "sign", "exit 1")),
        ("tools/fsm.sh", script(&markers, "fsmonitor", "exit 1")),
        (
            ".gitattributes",
            "*.dat filter=demo\n*.prc filter=proc\n*.t diff=tx\n".into(),
        ),
        ("a.dat", "data\n".into()),
        ("x.t", "one\n".into()),
    ];
    let hooks = [
        "pre-commit",
        "commit-msg",
        "post-commit",
        "reference-transaction",
        "post-index-change",
    ];
    let hook_paths: Vec<String> = hooks.iter().map(|h| format!(".githooks/{h}")).collect();
    for (h, p) in hooks.iter().zip(&hook_paths) {
        files.push((p.as_str(), script(&markers, &format!("hook-{h}"), "exit 0")));
    }
    let f = fixture(
        "progs",
        &files,
        &[
            ("filter.demo.clean", "sh tools/clean.sh".into()),
            ("filter.demo.smudge", "cat".into()),
            ("filter.demo.required", "true".into()),
            ("filter.proc.process", "sh tools/proc.sh".into()),
            ("filter.proc.required", "false".into()),
            ("diff.tx.textconv", "sh tools/tx.sh".into()),
            ("gpg.program", "tools/gpg.sh".into()),
        ],
    );
    assert_eq!(f.markers, markers, "fixture root matches the scripts");
    // Hooks and signing from here on (relative: each worktree's own copy,
    // which the worker can rewrite), fsmonitor from the worker's tree.
    g(&f.repo, &["config", "core.hooksPath", ".githooks"]);
    g(&f.repo, &["config", "commit.gpgsign", "true"]);
    g(
        &f.repo,
        &["config", "core.fsmonitor", &u(&f.wt.join("tools/fsm.sh"))],
    );
    f.clear();

    // The worker changes files, including every script, and commits.
    for p in ["tools/clean.sh", "tools/tx.sh", ".githooks/pre-commit"] {
        let path = f.wt.join(p);
        let old = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, old.replace("echo ", "echo pwned-")).unwrap();
    }
    std::fs::write(f.wt.join("x.t"), "two\n").unwrap();
    std::fs::write(f.wt.join("new.txt"), "new\n").unwrap();
    let (sha, _) = f.commit(true, &[]).unwrap();
    assert_eq!(f.marks(), Vec::<String>::new(), "external marker count 0");
    assert_eq!(
        g(&f.wt, &["show", "--name-only", "--format=", &sha]).trim(),
        ".githooks/pre-commit\nnew.txt\ntools/clean.sh\ntools/tx.sh\nx.t"
    );
    assert!(
        g(&f.wt, &["cat-file", "-p", &sha])
            .lines()
            .all(|l| !l.starts_with("gpgsig")),
        "not signed"
    );

    // Paths a filter handles (clean, process): refused, nothing runs.
    let before = f.head();
    std::fs::write(f.wt.join("a.dat"), "changed\n").unwrap();
    let e = f.commit(true, &[]).unwrap_err();
    assert!(e.contains("a.dat (filter=demo)"), "{e}");
    // A new file for the process filter (a failing process filter is fatal
    // for plain git, so the fixture tracks none).
    std::fs::write(f.wt.join("b.prc"), "new\n").unwrap();
    let e = f.commit(false, &["b.prc"]).unwrap_err();
    assert!(e.contains("b.prc (filter=proc)"), "{e}");
    assert_eq!(f.head(), before);
    assert_eq!(f.marks(), Vec::<String>::new(), "external marker count 0");

    // Controls: plain git in the same worktree runs each program.
    g_any(&f.wt, &["add", "a.dat"]);
    g_any(&f.wt, &["add", "b.prc"]);
    std::fs::write(f.wt.join("x.t"), "three\n").unwrap();
    g_any(&f.wt, &["diff", "--", "x.t"]);
    g_any(&f.wt, &["add", "x.t"]);
    g_any(
        &f.wt,
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "c"],
    );
    g_any(&f.wt, &["commit", "-q", "--allow-empty", "-m", "signed"]);
    g_any(&f.wt, &["status", "--porcelain"]);
    let marks = f.marks().join("\n");
    for kind in [
        "clean",
        "process",
        "textconv",
        "sign",
        "fsmonitor",
        "hook-commit-msg",
        "hook-post-commit",
        "hook-post-index-change",
        "hook-reference-transaction",
    ] {
        assert!(
            marks.contains(&format!("{kind}: ")),
            "control: {kind} is live\n{marks}"
        );
    }
    for (kind, payload) in [
        ("hook-pre-commit", "pwned-hook-pre-commit"),
        ("clean", "pwned-clean"),
        ("textconv", "pwned-textconv"),
    ] {
        let text = std::fs::read_to_string(f.markers.join(kind)).unwrap_or_default();
        assert!(
            text.contains(payload),
            "control: the worker's rewritten {kind} is live\n{marks}"
        );
    }
    let _ = std::fs::remove_dir_all(&m);
}

/// Nested repositories, junctions, symbolic and hard links, NTFS streams
/// and device names: refused one by one; nothing from outside is hashed,
/// nothing the nested repository configures runs.
#[test]
fn links_streams_devices_and_nested_repositories_are_refused() {
    let f = fixture("refuse", &[("a.txt", "a\n".into())], &[]);
    let wt = &f.wt;
    let before = f.head();
    let refused = |what: &str| {
        let e = f.commit(true, &[]).unwrap_err();
        assert!(e.contains(what), "{what}: {e}");
        assert_eq!(f.head(), before, "{what}: no commit");
    };

    // A nested repository whose own config names a hook and a filter.
    let sub = wt.join("sub");
    std::fs::create_dir_all(sub.join("h")).unwrap();
    g(&sub, &["init", "-q"]);
    std::fs::write(
        sub.join("h").join("pre-commit"),
        script(&f.markers, "nested", "exit 0"),
    )
    .unwrap();
    g(&sub, &["config", "core.hooksPath", "h"]);
    g(&sub, &["config", "filter.n.clean", "sh h/pre-commit"]);
    std::fs::write(sub.join(".gitattributes"), "* filter=n\n").unwrap();
    std::fs::write(sub.join("s.txt"), "s\n").unwrap();
    refused("sub/ is a nested git repository");
    std::fs::remove_dir_all(&sub).unwrap();

    // A junction to a folder outside.
    let st = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(wt.join("jd"))
        .arg(&f.outside)
        .output()
        .unwrap();
    assert!(st.status.success(), "mklink /J");
    refused("reparse point");
    std::fs::remove_dir(wt.join("jd")).unwrap();

    // A hard link to a file outside.
    std::fs::hard_link(f.outside.join("secret.txt"), wt.join("hl.txt")).unwrap();
    refused("hard link");
    std::fs::remove_file(wt.join("hl.txt")).unwrap();

    // A file symbolic link (only where this machine lets us make one).
    if std::os::windows::fs::symlink_file(f.outside.join("secret.txt"), wt.join("sl.txt")).is_ok() {
        refused("symbolic link");
        std::fs::remove_file(wt.join("sl.txt")).unwrap();
    }

    // A device name git lists as an untracked file.
    let aux = PathBuf::from(format!(r"\\?\{}\aux.txt", wt.display()));
    std::fs::write(&aux, "x\n").unwrap();
    refused("a device name");
    std::fs::remove_file(&aux).unwrap();

    // An NTFS stream: not a path podd (or git) commits; the file's own
    // content goes in without it.
    std::fs::write(wt.join("a.txt"), "a2\n").unwrap();
    std::fs::write(wt.join("a.txt:hidden"), "outside secret\n").unwrap();
    assert!(f.commit(false, &["a.txt:hidden"]).is_err());
    let (sha, _) = f.commit(false, &["a.txt"]).unwrap();
    assert_eq!(
        g(wt, &["cat-file", "blob", &format!("{sha}:a.txt")]),
        "a2\n"
    );

    f.secret_absent();
    assert_eq!(f.marks(), Vec::<String>::new(), "external marker count 0");
}

/// Config the worker can edit (an include inside the worktree, present or
/// not yet), a driver configured after the worker started, and a filter
/// name no config defines: refused before anything runs.
#[test]
fn worktree_config_and_new_drivers_are_refused() {
    let f = fixture("config", &[("a.txt", "a\n".into())], &[]);
    let (wt, repo) = (&f.wt, &f.repo);
    let before = f.head();
    std::fs::write(wt.join("ok.txt"), "ok\n").unwrap();

    // An include of a file in the worker's tree, not there yet.
    let inc = wt.join("evil.gitconfig");
    g(repo, &["config", "include.path", &u(&inc)]);
    let e = f.commit(false, &["ok.txt"]).unwrap_err();
    assert!(e.contains("inside the worktree"), "{e}");
    // The worker writes it: a driver and an attribute.
    std::fs::write(
        &inc,
        format!(
            "[filter \"inc\"]\n\tclean = sh '{}'\n",
            u(&f.root.join("inc.sh"))
        ),
    )
    .unwrap();
    std::fs::write(f.root.join("inc.sh"), script(&f.markers, "include", "cat")).unwrap();
    std::fs::write(wt.join(".gitattributes"), "*.inc filter=inc\n").unwrap();
    std::fs::write(wt.join("x.inc"), "x\n").unwrap();
    let e = f.commit(true, &[]).unwrap_err();
    assert!(e.contains("inside the worktree"), "{e}");
    assert_eq!(f.marks(), Vec::<String>::new());
    g_any(wt, &["add", "x.inc"]);
    assert_eq!(
        f.marks(),
        ["include: 1"],
        "control: the included driver is live"
    );
    g(wt, &["reset", "-q"]);
    g(repo, &["config", "--unset", "include.path"]);
    f.clear();

    // A driver configured after the worker started: the commit's own
    // guard reads it, and the path naming it is refused.
    g(
        repo,
        &[
            "config",
            "filter.late.clean",
            &format!("sh '{}'", u(&f.root.join("inc.sh"))),
        ],
    );
    std::fs::write(
        wt.join(".gitattributes"),
        "*.inc filter=late\n*.ghost filter=ghost\n",
    )
    .unwrap();
    let e = f.commit(true, &[]).unwrap_err();
    assert!(e.contains("x.inc (filter=late)"), "{e}");
    // A driver name nothing configures (yet): refused as well.
    std::fs::remove_file(wt.join("x.inc")).unwrap();
    std::fs::write(wt.join("y.ghost"), "y\n").unwrap();
    let e = f.commit(true, &[]).unwrap_err();
    assert!(e.contains("y.ghost (filter=ghost)"), "{e}");
    assert_eq!(f.head(), before);
    // Paths without a filter still commit.
    f.commit(false, &["ok.txt", ".gitattributes"]).unwrap();
    assert_eq!(f.marks(), Vec::<String>::new(), "external marker count 0");
}
