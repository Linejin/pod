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
//! `--no-daemon` keeps the agent's work inside the PTY (docs/04 §4.3); the
//! daemon mode also refuses to start from an elevated parent.
//! `check_for_update_on_startup=false`: the "Update available" prompt would
//! otherwise block the start of an unattended task agent, like the trust
//! dialog does (updating stays the user's business outside Pod tasks).

use std::path::Path;

use crate::protocol::{AgentKind, AgentSpec};

/// TOML string for a Windows path: a literal string (no escapes, no `"`, so
/// it survives a `cmd /c` shim) unless the path contains `'`.
fn toml_path(p: &str) -> String {
    if p.contains('\'') {
        format!("\"{}\"", p.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("'{p}'")
    }
}

/// The `-c` value trusting `worktree` for one Codex run.
pub fn codex_trust_override(worktree: &Path) -> String {
    let p = worktree.display().to_string().replace('/', "\\");
    format!("projects={{{}={{trust_level='trusted'}}}}", toml_path(&p))
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

    #[test]
    fn quotes_and_slashes() {
        assert_eq!(
            codex_trust_override(Path::new("C:/a/b's")),
            r#"projects={"C:\\a\\b's"={trust_level='trusted'}}"#
        );
    }
}
