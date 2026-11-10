//! The agents' own resume commands (docs/18 §4). Verified against the
//! installed CLIs' `--help`: Claude Code 2.1.319 (`-r, --resume [value]
//! Resume a conversation by session ID`) and codex-cli 0.159.0 (`codex
//! resume [OPTIONS] [SESSION_ID]`, `--no-daemon`).

use crate::{Agent, is_uuid};

/// argv that resumes session `id` of `agent`, run in the session's cwd.
/// `exe` replaces the program (tests use a fake executable). Ids must be
/// UUIDs, so nothing else can reach the command line.
pub fn resume_argv(agent: Agent, id: &str, exe: Option<&str>) -> Result<Vec<String>, String> {
    if !is_uuid(id) {
        return Err(format!("{id:?} is not a session id (UUID)"));
    }
    let program = exe
        .filter(|e| !e.trim().is_empty())
        .unwrap_or(agent.as_str())
        .to_string();
    Ok(match agent {
        Agent::Claude => vec![program, "--resume".into(), id.into()],
        // Same flags as Pod's task agents (podd `agent_launch.rs`) minus the
        // folder trust, which Pod grants only to its own worktrees:
        // `--no-daemon` keeps the work inside the PTY (docs/04 §4.3), no
        // update prompt blocking the start.
        Agent::Codex => vec![
            program,
            "--no-daemon".into(),
            "-c".into(),
            "check_for_update_on_startup=false".into(),
            "resume".into(),
            id.into(),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0f8b6a3e-1111-4222-8333-944445555666";

    #[test]
    fn per_agent_commands() {
        assert_eq!(
            resume_argv(Agent::Claude, ID, None).unwrap(),
            ["claude", "--resume", ID]
        );
        assert_eq!(
            resume_argv(Agent::Codex, ID, None).unwrap(),
            [
                "codex",
                "--no-daemon",
                "-c",
                "check_for_update_on_startup=false",
                "resume",
                ID
            ]
        );
        assert_eq!(
            resume_argv(Agent::Codex, ID, Some(r"C:\fake\fake-agent.exe")).unwrap()[0],
            r"C:\fake\fake-agent.exe"
        );
        assert_eq!(
            resume_argv(Agent::Claude, ID, Some(" ")).unwrap()[0],
            "claude"
        );
    }

    #[test]
    fn rejects_non_uuid_ids() {
        for bad in [
            "",
            "--dangerously-skip-permissions",
            "x & calc",
            "0f8b6a3e-1111-4222-8333-94444555566\"",
        ] {
            assert!(resume_argv(Agent::Claude, bad, None).is_err(), "{bad}");
        }
    }
}
