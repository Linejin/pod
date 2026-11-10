//! Narrow allowance for Pod-started Codex workers (docs/19 §18.1).
//!
//! Codex runs model commands in its sandbox, which cannot open podd's pipe
//! (`pod orch …` fails with os error 5) and cannot write the worktree's git
//! admin dir (`git add`/`commit` fail on `index.lock`), so every Pod
//! coordination step and commit needed a human approval (docs/19 §17.3-4).
//!
//! What the installed Codex (0.159, Windows) offers, verified with the real
//! binary (`tools/e2e/codex-allowance-probe.ps1`):
//!
//! - Exec-policy rules (`prefix_rule(pattern=[…], decision="allow")`) run a
//!   matching command without approval **and outside the sandbox**. Codex
//!   loads them from `.rules` files only (`$CODEX_HOME/rules`, or a trusted
//!   project's `.codex/rules/`); no `-c` key carries rules inline. So Pod
//!   writes [`RULES_REL`] into the worker's own Pod-created worktree (Codex
//!   already trusts exactly that worktree through the inline `-c projects=…`
//!   override) and hides it from git with one line in the repository's
//!   `info/exclude` (git ignores a linked worktree's own `info/exclude`).
//!   The sandboxed agent cannot change it: Codex keeps `.codex` read-only.
//! - Extra writable roots (`sandbox_workspace_write.writable_roots`) are
//!   refused for anything under a `.git` directory, so the worktree's git
//!   admin dir cannot be made writable.
//! - No `git` command is allowed outside the sandbox. An unsandboxed git
//!   runs hooks, fsmonitor and filters from the repository it finds, and the
//!   sandboxed agent can build one: Codex protects the worktree's own `.git`
//!   file, but not a nested `sub/.git`, and Codex's `workdir` argument runs
//!   an allowed `git commit` there (the probe saw its pre-commit hook and
//!   fsmonitor write outside the worktree; only git's ownership check, gone
//!   with `safe.directory=*`, stood in the way). Commits go through
//!   `pod orch commit` instead: podd commits in the worktree it recorded
//!   with git plumbing only, hooks, fsmonitor and every configured filter
//!   program off, and refuses paths a configured filter handles
//!   ([`crate::git::commit_for_worker`], §18.5).
//!   `git status/diff/log` run in the sandbox (read only) without approval.
//! - Nothing that reads other terminals is allowed (`pod read`, `pod ls`,
//!   `pod orch worker read/show`): outside the sandbox they would show any
//!   Pod terminal, the coordinator's included.
//!
//! Nothing else changes: no writable roots, no network, no sandbox mode, no
//! user file (`~/.codex` is never written). `pod orch` subcommands that start
//! or stop processes (`worker start/stop/…`), change settings (`limits`) or
//! decide gates stay behind approval.

use std::path::{Path, PathBuf};

use super::log;
use crate::protocol::{AgentKind, AgentSpec};

/// The rules file, relative to the worktree root.
pub const RULES_REL: &str = ".codex/rules/pod-worker.rules";
/// Its line in `<common git dir>/info/exclude`.
pub const EXCLUDE_LINE: &str = "/.codex/rules/pod-worker.rules";

/// The rules Pod installs (Starlark, Codex exec-policy format).
pub const RULES: &str = r#"# Written by Pod for a Pod-started Codex worker (docs/19 §18.1).
# Pod coordination (commits too: `pod orch commit`, run by podd) runs
# without approval; everything else stays in Codex's sandbox. Turn off per repository: .pod/config.toml
# [worker] codex_allowance = false
prefix_rule(pattern=["pod", "orch", ["ask", "send", "check", "reply", "done", "wait", "commit", "workers", "queue"]], decision="allow", justification="Pod coordination")
prefix_rule(pattern=["pod", "orch", "run", ["ls", "show"]], decision="allow", justification="Pod coordination")
prefix_rule(pattern=["pod", "orch", "item", "ls"], decision="allow", justification="Pod coordination")
prefix_rule(pattern=["pod", "orch", "gate", "request"], decision="allow", justification="Pod coordination")
prefix_rule(pattern=["pod", "guide", "orchestration"], decision="allow", justification="Pod guide")
prefix_rule(pattern=["pod", "--version"], decision="allow", justification="Pod version")
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Written (or already Pod's current file).
    Installed,
    /// Pod's file was removed (allowance off).
    Removed,
    /// Off and nothing of Pod's was there.
    Absent,
    /// A different file is at [`RULES_REL`] (the repository's own): left alone.
    Foreign,
}

/// Before a Pod orchestration worker's agent starts in `worktree`: a Codex
/// worker gets the rules when `on` (`[worker] codex_allowance`, default on)
/// and loses Pod's earlier rules when off. Other agents: nothing. Problems
/// are logged, never fatal (the worker then just asks for approvals).
pub fn for_worker(task: &str, agent: &AgentSpec, worktree: &Path, on: bool) {
    if !matches!(agent, AgentSpec::Kind(AgentKind::Codex)) {
        return;
    }
    match apply(worktree, on) {
        Ok(Outcome::Installed) => log(&format!(
            "task {task}: codex worker allowance installed ({RULES_REL})"
        )),
        Ok(Outcome::Removed) => log(&format!(
            "task {task}: codex worker allowance off, rules removed"
        )),
        Ok(Outcome::Absent) => log(&format!("task {task}: codex worker allowance off")),
        Ok(Outcome::Foreign) => log(&format!(
            "task {task}: {RULES_REL} is the repository's own file; allowance not applied"
        )),
        Err(e) => log(&format!("task {task}: codex worker allowance: {e}")),
    }
}

fn rules_path(worktree: &Path) -> PathBuf {
    worktree.join(RULES_REL.replace('/', "\\"))
}

/// Install (`on`) or remove Pod's rules file in `worktree`. Never touches a
/// file at that path that Pod did not write.
pub fn apply(worktree: &Path, on: bool) -> Result<Outcome, String> {
    let path = rules_path(worktree);
    let current = std::fs::read_to_string(&path).ok();
    let ours = current.as_deref().map(is_ours);
    if !on {
        return match ours {
            Some(true) => {
                std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                // Drop the folders if Pod's file was all that was in them.
                if let Some(rules) = path.parent() {
                    let _ = std::fs::remove_dir(rules);
                    if let Some(codex) = rules.parent() {
                        let _ = std::fs::remove_dir(codex);
                    }
                }
                Ok(Outcome::Removed)
            }
            Some(false) => Ok(Outcome::Foreign),
            None => Ok(Outcome::Absent),
        };
    }
    if ours == Some(false) {
        return Ok(Outcome::Foreign);
    }
    exclude(worktree)?;
    if current.as_deref() != Some(RULES) {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        std::fs::write(&path, RULES).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(Outcome::Installed)
}

/// A file Pod wrote (any version of it).
fn is_ours(text: &str) -> bool {
    text.starts_with("# Written by Pod for a Pod-started Codex worker")
}

/// Add [`EXCLUDE_LINE`] to the repository's `info/exclude` once.
fn exclude(worktree: &Path) -> Result<(), String> {
    let common = crate::git::run(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let file = crate::git::native_path(&common)
        .join("info")
        .join("exclude");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.lines().any(|l| l.trim() == EXCLUDE_LINE) {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let sep = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    std::fs::write(
        &file,
        format!("{text}{sep}# Pod: Codex worker rules (docs/19 §18.1)\n{EXCLUDE_LINE}\n"),
    )
    .map_err(|e| format!("{}: {e}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) -> String {
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

    struct Tmp(PathBuf);
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A repository with one commit and a linked worktree `wt`.
    fn fixture(name: &str) -> (Tmp, PathBuf, PathBuf) {
        let t =
            Tmp(std::env::temp_dir()
                .join(format!("podd-codex-allow-{name}-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&t.0);
        let repo = t.0.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-qm",
                "init",
            ],
        );
        let wt = t.0.join("wt");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "pod/w1",
                wt.to_str().unwrap(),
            ],
        );
        (t, repo, wt)
    }

    #[test]
    fn rules_cover_only_coordination_and_commits() {
        let allowed: Vec<&str> = RULES
            .lines()
            .filter(|l| l.starts_with("prefix_rule("))
            .collect();
        assert_eq!(allowed.len(), 6);
        assert!(allowed.iter().all(|l| l.contains(r#"decision="allow""#)));
        // Never a bare `pod`/`pod orch` prefix, never process control, no
        // git at all (an unsandboxed git runs hooks/fsmonitor from a repo
        // the agent can build; commits go through `pod orch commit`), no
        // reading other terminals (`pod read`/`ls`, `worker read`).
        for bad in [
            r#"["pod"]"#,
            r#"["pod", "orch"]"#,
            "\"git\"",
            "\"start\"",
            "\"stop\"",
            "\"limits\"",
            "\"decide\"",
            "\"worker\"",
            r#"["pod", ["ls""#,
            r#"["pod", "read""#,
            r#"["pod", "ls""#,
        ] {
            assert!(!RULES.contains(bad), "{bad}");
        }
    }

    #[test]
    fn installs_hidden_from_git_and_removes_only_its_own_file() {
        let (_t, repo, wt) = fixture("install");
        assert_eq!(apply(&wt, true).unwrap(), Outcome::Installed);
        assert_eq!(
            std::fs::read_to_string(wt.join(".codex/rules/pod-worker.rules")).unwrap(),
            RULES
        );
        assert_eq!(
            git(&wt, &["status", "--porcelain", "--untracked-files=all"]),
            "",
            "hidden from git"
        );
        // Idempotent: one exclude line however often it runs.
        assert_eq!(apply(&wt, true).unwrap(), Outcome::Installed);
        let ex = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert_eq!(ex.matches(EXCLUDE_LINE).count(), 1, "{ex}");
        // Off: Pod's file and the folders it made are gone.
        assert_eq!(apply(&wt, false).unwrap(), Outcome::Removed);
        assert!(!wt.join(".codex").exists());
        assert_eq!(apply(&wt, false).unwrap(), Outcome::Absent);
        // The repository's own file at that path is never touched.
        std::fs::create_dir_all(wt.join(".codex/rules")).unwrap();
        std::fs::write(wt.join(".codex/rules/pod-worker.rules"), "# mine\n").unwrap();
        assert_eq!(apply(&wt, true).unwrap(), Outcome::Foreign);
        assert_eq!(apply(&wt, false).unwrap(), Outcome::Foreign);
        assert_eq!(
            std::fs::read_to_string(wt.join(".codex/rules/pod-worker.rules")).unwrap(),
            "# mine\n"
        );
    }

    #[test]
    fn keeps_existing_exclude_lines() {
        let (_t, repo, wt) = fixture("exclude");
        std::fs::write(repo.join(".git/info/exclude"), "*.log").unwrap();
        apply(&wt, true).unwrap();
        let ex = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert!(ex.starts_with("*.log\n# Pod"), "{ex}");
    }
}
