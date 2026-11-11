//! The one git environment rule podd and pod-forge share: clear the
//! variables that would point git at another repository than the folder the
//! caller names (`-C <dir>` or the working folder). Only this cleanup is
//! shared; each caller keeps its own executable, argv, config, prompts,
//! output handling and time limit (docs/20-refactor/libs.md §2).

use std::process::Command;

/// Variables that would point git somewhere other than the caller's folder.
pub const OVERRIDES: [&str; 9] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_CEILING_DIRECTORIES",
    "GIT_PREFIX",
];

/// Removes every [`OVERRIDES`] variable from `cmd`'s environment, inherited
/// or set on `cmd` before, and returns `cmd`. Nothing else on `cmd` changes.
pub fn clear_overrides(cmd: &mut Command) -> &mut Command {
    for v in OVERRIDES {
        cmd.env_remove(v);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(cmd: &Command) -> Vec<(String, Option<String>)> {
        let mut env: Vec<_> = cmd
            .get_envs()
            .map(|(k, v)| {
                let v = v.map(|v| v.to_string_lossy().into_owned());
                (k.to_string_lossy().into_owned(), v)
            })
            .collect();
        env.sort();
        env
    }

    #[test]
    fn removes_exactly_the_nine_overrides() {
        let mut cmd = Command::new("git");
        cmd.args(["-C", "repo", "status"])
            .current_dir("elsewhere")
            .env("GIT_DIR", "other.git")
            .env("GIT_TERMINAL_PROMPT", "0");
        clear_overrides(&mut cmd);
        assert_eq!(cmd.get_program(), "git");
        assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["-C", "repo", "status"]);
        assert_eq!(
            cmd.get_current_dir(),
            Some(std::path::Path::new("elsewhere"))
        );
        let mut want: Vec<(String, Option<String>)> =
            OVERRIDES.iter().map(|k| (k.to_string(), None)).collect();
        want.push(("GIT_TERMINAL_PROMPT".into(), Some("0".into())));
        want.sort();
        assert_eq!(env(&cmd), want);
    }

    /// A child started from the command sees none of them, even when they
    /// were set on the command first.
    #[cfg(windows)]
    #[test]
    fn the_child_does_not_see_them() {
        let mut cmd = Command::new("cmd");
        cmd.args(["/D", "/C", "set"]);
        for v in OVERRIDES {
            cmd.env(v, "pod-process-test");
        }
        cmd.env("POD_PROCESS_GIT_KEPT", "1");
        let out = clear_overrides(&mut cmd).output().unwrap();
        assert!(out.status.success(), "{out:?}");
        let text = String::from_utf8_lossy(&out.stdout).to_uppercase();
        assert!(text.contains("POD_PROCESS_GIT_KEPT=1"), "{text}");
        for v in OVERRIDES {
            assert!(!text.contains(&format!("\n{v}=")), "{v} reached the child");
            assert!(!text.starts_with(&format!("{v}=")), "{v} reached the child");
        }
    }
}
