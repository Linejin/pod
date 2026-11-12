//! Threat model TM-21 (docs/19 §18.6), "env overrides": podd's own
//! environment may carry git config overrides (a `pod` started podd from a
//! shell that had them). `GIT_CONFIG_PARAMETERS` is applied after the
//! `GIT_CONFIG_COUNT` entries the worker commit's guard uses, so a
//! `core.hooksPath` there would run hooks again. The worker commit drops
//! inherited `GIT_*` overrides. One test in its own binary: it sets this
//! process's environment.

use std::path::{Path, PathBuf};
use std::process::Command;

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

fn u(p: &Path) -> String {
    p.display().to_string().replace('\\', "/")
}

struct Root(PathBuf);
impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn inherited_git_config_parameters_do_not_reach_a_worker_commit() {
    let root = Root(std::env::temp_dir().join(format!("podd-wce-{}", std::process::id())));
    let _ = std::fs::remove_dir_all(&root.0);
    let (repo, wt, no_hooks, hooks) = (
        root.0.join("repo"),
        root.0.join("wt"),
        root.0.join("no-hooks"),
        root.0.join("evil-hooks"),
    );
    for d in [&repo, &no_hooks, &hooks] {
        std::fs::create_dir_all(d).unwrap();
    }
    let marker = root.0.join("outside.marker");
    for h in ["reference-transaction", "post-index-change", "pre-commit"] {
        std::fs::write(
            hooks.join(h),
            format!("#!/bin/sh\necho {h} >> '{}'\n", u(&marker)),
        )
        .unwrap();
    }
    g(&repo, &["init", "-q", "-b", "main"]);
    for (k, v) in [
        ("user.name", "t"),
        ("user.email", "t@example.invalid"),
        ("commit.gpgsign", "false"),
    ] {
        g(&repo, &["config", k, v]);
    }
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    g(&repo, &["add", "-A"]);
    g(&repo, &["commit", "-q", "-m", "init"]);
    g(&repo, &["worktree", "add", "-q", "-b", "pod/w", &u(&wt)]);

    // podd's environment from here on (git children inherit it).
    // SAFETY: the only test in this binary; no other thread reads the
    // environment while it is set.
    unsafe {
        std::env::set_var(
            "GIT_CONFIG_PARAMETERS",
            format!("'core.hookspath'='{}'", u(&hooks)),
        )
    };
    // Control: a plain git with that environment runs the hooks.
    g(&wt, &["update-ref", "refs/heads/control", "HEAD"]);
    assert!(marker.exists(), "control: the inherited hooksPath is live");
    std::fs::remove_file(&marker).unwrap();

    std::fs::write(wt.join("b.txt"), "b\n").unwrap();
    let (sha, _) =
        podd::git::commit_for_task_worker(&wt, &repo, &no_hooks, "w", true, &[]).unwrap();
    assert!(!marker.exists(), "external marker count 0");
    assert_eq!(g(&wt, &["rev-parse", "HEAD"]).trim(), sha);
    unsafe { std::env::remove_var("GIT_CONFIG_PARAMETERS") };
}
