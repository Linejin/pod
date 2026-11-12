//! Agent launch adjustments for task terminals.
//!
//! Codex folder trust (docs/08 §8): a Pod-created worktree is new to Codex, so
//! Codex would open its "Trust this folder?" dialog. Pod passes the trust for
//! exactly that worktree as a per-run config override instead of editing the
//! user's `config.toml`:
//!
//! ```text
//! codex --no-daemon -c "projects={'C:\…\worktree'={trust_level='trusted'}}" [prompt]
//! ```
//!
//! Observed with Codex 0.159.0 on Windows: the dotted form
//! `-c projects.'<path>'.trust_level="trusted"` does NOT suppress the dialog
//! (in any case/slash spelling), the inline-table form does (original case
//! or lower case). The override replaces the `projects` table only for this
//! process, so nothing has to be cleaned up when the task is removed, and
//! only Pod-created worktree paths are ever trusted this way.
//!
//! The trust is Pod vouching for the folder, so it is only given to a
//! worktree whose `.codex` holds nothing Pod did not write (docs/19 §21.3,
//! TM-21 follow-up): a trusted project's `.codex` config, hooks and exec
//! policies load (and can run code at start), even with Pod's worker
//! allowance off. Otherwise Pod passes `trust_level='untrusted'` for it:
//! Codex 0.159 then loads none of them and asks the person once ("Open
//! restricted" or quit), never saving a trust decision. (A Codex
//! orchestration worker is not started there at all,
//! [`super::codex_allowance::for_worker`].)
//!
//! `--no-daemon` keeps the agent's work inside the PTY (docs/04 §4.3); the
//! daemon mode also refuses to start from an elevated parent.
//! `check_for_update_on_startup=false`: the "Update available" prompt would
//! otherwise block the start of an unattended task agent, like the trust
//! dialog does (updating stays the user's business outside Pod tasks).

use std::collections::BTreeMap;
use std::path::Path;

use crate::protocol::{AgentKind, AgentSpec};

/// docs/19 §20 (TM-20): a Pod worker's terminal (its env has
/// `POD_ATTEMPT_ID`) talks to the restricted worker endpoint, not the full
/// pipe: `POD_SOCKET` is `<pipe>-worker`, whatever the overrides said. So
/// `pod orch ...`, the Codex allowance's commands and the hook relay run
/// there under the worker's own identity. Other terminals keep the pipe.
pub fn worker_endpoint_env(env: &mut BTreeMap<String, String>, pipe: &str) {
    let worker =
        crate::env::get_var(env, crate::env::ENV_ATTEMPT_ID).is_some_and(|a| !a.is_empty());
    if worker {
        crate::env::set_var(
            env,
            crate::endpoint::ENV_SOCKET,
            crate::endpoint::worker_pipe(pipe),
        );
    }
}

/// TOML string for a Windows path: a literal string (no escapes, no `"`, so
/// it survives a `cmd /c` shim) unless the path contains `'`.
fn toml_path(p: &str) -> String {
    if p.contains('\'') {
        format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{p}'")
    }
}

/// The `-c` value setting `worktree`'s trust for one Codex run:
/// `trusted` when its `.codex` holds nothing Pod did not write, else
/// `untrusted` ([`super::codex_allowance::foreign_codex`]).
pub fn codex_trust_override(worktree: &Path) -> String {
    let level = match super::codex_allowance::foreign_codex(worktree) {
        None => "trusted",
        Some(files) => {
            super::log(&format!(
                "codex in {}: no folder trust, .codex holds files Pod did not write ({files})",
                worktree.display()
            ));
            "untrusted"
        }
    };
    trust_override(worktree, level)
}

fn trust_override(worktree: &Path, level: &str) -> String {
    let p = worktree.display().to_string().replace('/', "\\");
    format!("projects={{{}={{trust_level='{level}'}}}}", toml_path(&p))
}

/// For `agent: codex` tasks: insert `--no-daemon -c <no update prompt>
/// -c <trust>` after the program name (before the prompt). Other agents are
/// unchanged.
pub fn task_agent_argv(agent: &AgentSpec, mut argv: Vec<String>, worktree: &Path) -> Vec<String> {
    if matches!(agent, AgentSpec::Kind(AgentKind::Codex)) && !argv.is_empty() {
        argv.splice(
            1..1,
            [
                "--no-daemon".to_string(),
                "-c".to_string(),
                "check_for_update_on_startup=false".to_string(),
                "-c".to_string(),
                codex_trust_override(worktree),
            ],
        );
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_gets_trust_for_its_worktree_only() {
        let wt = Path::new(r"C:\Users\me\AppData\Local\pod\worktrees\demo\feat-1");
        let argv = task_agent_argv(
            &AgentSpec::Kind(AgentKind::Codex),
            vec!["codex".into(), "fix it".into()],
            wt,
        );
        assert_eq!(
            argv,
            [
                "codex",
                "--no-daemon",
                "-c",
                "check_for_update_on_startup=false",
                "-c",
                r"projects={'C:\Users\me\AppData\Local\pod\worktrees\demo\feat-1'={trust_level='trusted'}}",
                "fix it"
            ]
        );
        let claude = task_agent_argv(
            &AgentSpec::Kind(AgentKind::Claude),
            vec!["claude".into()],
            wt,
        );
        assert_eq!(claude, ["claude"]);
    }

    /// docs/19 §21.3: Pod's trust only for a worktree whose `.codex` holds
    /// nothing Pod did not write; otherwise `untrusted` (Codex then loads
    /// none of it and asks the person).
    #[test]
    fn no_trust_next_to_codex_files_pod_did_not_write() {
        let wt = std::env::temp_dir().join(format!("podd-trust-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&wt);
        std::fs::create_dir_all(wt.join(".codex").join("rules")).unwrap();
        let level = |wt: &Path| {
            let argv =
                task_agent_argv(&AgentSpec::Kind(AgentKind::Codex), vec!["codex".into()], wt);
            let v = argv.last().unwrap().clone();
            assert!(v.starts_with("projects={'"), "{v}");
            if v.ends_with("={trust_level='trusted'}}") {
                "trusted"
            } else if v.ends_with("={trust_level='untrusted'}}") {
                "untrusted"
            } else {
                panic!("{v}")
            }
        };
        let rules = wt.join(super::super::codex_allowance::RULES_REL);
        assert_eq!(level(&wt), "trusted", "empty .codex");
        std::fs::write(&rules, super::super::codex_allowance::RULES).unwrap();
        std::fs::write(wt.join(".codex/rules/pod-worker.rules.pod-tmp-12-3"), "").unwrap();
        assert_eq!(level(&wt), "trusted", "Pod's own file and temp file");
        let allow_cmd = "prefix_rule(pattern=[\"cmd\"], decision=\"allow\")\n";
        for (name, text) in [
            (
                ".codex/config.toml",
                "sandbox_mode = 'danger-full-access'\n",
            ),
            (".codex/hooks.json", "{}"),
            (".codex/rules/repo.rules", allow_cmd),
            (".codex/rules/x.pod-tmp-.rules", allow_cmd),
        ] {
            std::fs::write(wt.join(name), text).unwrap();
            assert_eq!(level(&wt), "untrusted", "{name}");
            std::fs::remove_file(wt.join(name)).unwrap();
        }
        // A foreign file at Pod's path.
        std::fs::write(&rules, allow_cmd).unwrap();
        assert_eq!(level(&wt), "untrusted", "not Pod's rules file");
        std::fs::remove_dir_all(wt.join(".codex")).unwrap();
        assert_eq!(level(&wt), "trusted", "no .codex");
        // Claude gets no Codex arguments at all.
        std::fs::create_dir_all(wt.join(".codex")).unwrap();
        std::fs::write(wt.join(".codex/config.toml"), "x = 1\n").unwrap();
        assert_eq!(
            task_agent_argv(
                &AgentSpec::Kind(AgentKind::Claude),
                vec!["claude".into()],
                &wt
            ),
            ["claude"]
        );
        let _ = std::fs::remove_dir_all(&wt);
    }

    /// docs/19 §20: workers get the worker endpoint even when an override
    /// names the full pipe; other terminals keep the daemon's pipe.
    #[test]
    fn workers_get_the_worker_endpoint() {
        let pipe = r"\\.\pipe\podd-x";
        let mut env: BTreeMap<String, String> = [
            ("POD_SOCKET".to_string(), pipe.to_string()),
            ("pod_attempt_id".to_string(), "a1".to_string()),
        ]
        .into();
        worker_endpoint_env(&mut env, pipe);
        assert_eq!(
            crate::env::get_var(&env, "POD_SOCKET"),
            Some(r"\\.\pipe\podd-x-worker")
        );
        assert_eq!(env.len(), 2, "replaced, not added: {env:?}");
        let mut plain: BTreeMap<String, String> =
            [("POD_SOCKET".to_string(), pipe.to_string())].into();
        worker_endpoint_env(&mut plain, pipe);
        assert_eq!(crate::env::get_var(&plain, "POD_SOCKET"), Some(pipe));
        let mut empty: BTreeMap<String, String> = [
            ("POD_SOCKET".to_string(), pipe.to_string()),
            ("POD_ATTEMPT_ID".to_string(), String::new()),
        ]
        .into();
        worker_endpoint_env(&mut empty, pipe);
        assert_eq!(crate::env::get_var(&empty, "POD_SOCKET"), Some(pipe));
    }

    #[test]
    fn quotes_and_slashes() {
        assert_eq!(
            codex_trust_override(Path::new("C:/a/b's")),
            r#"projects={"C:\\a\\b's"={trust_level='trusted'}}"#
        );
    }
}
