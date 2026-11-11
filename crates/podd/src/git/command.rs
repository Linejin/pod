//! Running git. Two kinds of call, kept apart on purpose:
//!
//! - the general call ([`output`], [`run`], [`run_os`]) for the user's
//!   repositories and Pod's own bookkeeping;
//! - the guarded call for a tree a worker controls: [`guard`] config that
//!   blanks every configured filter program (and, on the commit path,
//!   hooks, signing and maintenance), read with [`run_guarded`] /
//!   [`output_guarded_os`], or run bounded in time and output with
//!   [`run_bounded`] (docs/19 §18.5).
//!
//! Both start from [`command`]: fixed `-c` options, no prompt, no console
//! window and no inherited variable that points git elsewhere.

use std::path::Path;
use std::process::{Command, Stdio};

use super::parse::parse_filter_drivers;

/// Environment override for the git executable.
pub const ENV_GIT: &str = "POD_GIT";

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct Output {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

fn command(dir: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let exe = std::env::var_os(ENV_GIT).unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(exe);
    cmd.arg("-C")
        .arg(dir)
        .args(["-c", "core.longpaths=true", "-c", "core.quotepath=false"])
        // fsmonitor is only a speed-up, and its program comes from config
        // and may be a script inside a worktree an agent writes (§18.5).
        .args(["-c", "core.fsmonitor=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    // Variables that would point git somewhere other than `-C <dir>`.
    pod_process::git::clear_overrides(&mut cmd);
    cmd
}

/// Run git in `dir`; never fails on a non-zero exit (see [`Output::ok`]).
pub fn output(dir: &Path, args: &[&str]) -> Result<Output, String> {
    let out = command(dir)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    Ok(Output {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_string(),
    })
}

/// Run git in `dir`; a non-zero exit becomes `Err("git <args>: <stderr>")`.
pub fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let o = output(dir, args)?;
    if o.ok {
        Ok(o.stdout)
    } else {
        Err(format!("git {}: {}", args.join(" "), o.stderr))
    }
}

/// Like [`run`] for path arguments.
pub(super) fn run_os(dir: &Path, args: &[&std::ffi::OsStr]) -> Result<String, String> {
    let out = command(dir)
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let shown: Vec<String> = args.iter().map(|a| a.to_string_lossy().into()).collect();
        Err(format!(
            "git {}: {}",
            shown.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

// ---------------------------------------------------------------------------
// Guarded: a tree a worker controls
// ---------------------------------------------------------------------------

/// Whole-call limit for each git child on the worker commit path.
const GUARDED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);
/// Most stdout a git child on that path may print (listings of a large repo).
const GUARDED_STDOUT: usize = 64 * 1024 * 1024;

/// Config (command-line scope, so it beats every config file and include)
/// that keeps repository-configured programs from running in a tree a
/// worker controls (fsmonitor is off for every call, see [`command`]):
/// every configured filter driver emptied (git skips a driver whose command is empty) and not required.
/// With `hooks`: `core.hooksPath` is that empty folder and signing and
/// automatic maintenance are off (the commit path). Also returns the
/// driver names.
pub(super) type Guard = (Vec<(String, String)>, Vec<String>);

pub(super) fn guard(dir: &Path, hooks: Option<&Path>) -> Result<Guard, String> {
    let mut cfg = Vec::new();
    if let Some(h) = hooks {
        let h = h.display().to_string().replace('\\', "/");
        for (k, v) in [
            ("core.hooksPath", h.as_str()),
            ("commit.gpgSign", "false"),
            ("gc.auto", "0"),
            ("maintenance.auto", "false"),
        ] {
            cfg.push((k.into(), v.into()));
        }
    }
    let drivers = filter_drivers(dir)?;
    for d in &drivers {
        for (key, value) in [
            ("clean", ""),
            ("smudge", ""),
            ("process", ""),
            ("required", "false"),
        ] {
            cfg.push((format!("filter.{d}.{key}"), value.into()));
        }
    }
    Ok((cfg, drivers))
}

/// Names of the filter drivers the effective config gives a program
/// (`filter.<name>.clean|smudge|process`). Reading config runs nothing.
fn filter_drivers(dir: &Path) -> Result<Vec<String>, String> {
    let o = output(
        dir,
        &[
            "config",
            "-z",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process)$",
        ],
    )?;
    if !o.ok && !o.stderr.is_empty() {
        return Err(format!("git config: {}", o.stderr));
    }
    Ok(parse_filter_drivers(&o.stdout))
}

/// Pass `cfg` as command-line config through the environment (any key
/// spelling works, unlike `-c k=v`; child gits inherit it).
fn with_config(cmd: &mut Command, cfg: &[(String, String)]) {
    cmd.env("GIT_CONFIG_COUNT", cfg.len().to_string());
    for (i, (k, v)) in cfg.iter().enumerate() {
        cmd.env(format!("GIT_CONFIG_KEY_{i}"), k);
        cmd.env(format!("GIT_CONFIG_VALUE_{i}"), v);
    }
}

/// [`run`] with [`guard`] config.
pub(super) fn run_guarded(
    dir: &Path,
    cfg: &[(String, String)],
    args: &[&str],
) -> Result<String, String> {
    let mut cmd = command(dir);
    with_config(&mut cmd, cfg);
    let out = cmd
        .args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// git with [`guard`] config and path arguments; the caller reads the exit
/// status and words the error.
pub(super) fn output_guarded_os(
    dir: &Path,
    cfg: &[(String, String)],
    args: &[&std::ffi::OsStr],
) -> Result<std::process::Output, String> {
    let mut cmd = command(dir);
    with_config(&mut cmd, cfg);
    cmd.args(args)
        .output()
        .map_err(|e| format!("cannot run git: {e}"))
}

/// One bounded git child (pod_process::bounded: process tree in a job, a
/// whole-call deadline, capped stdout) with `cfg` and optional stdin.
pub(super) fn run_bounded(
    dir: &Path,
    cfg: &[(String, String)],
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> Result<String, String> {
    use pod_process::bounded::{self, End, Keep};
    let mut cmd = command(dir);
    with_config(&mut cmd, cfg);
    cmd.args(args);
    let mut opts = bounded::Options::new(GUARDED_TIMEOUT, Keep::Cap(GUARDED_STDOUT));
    opts.stderr = Keep::Tail(4096);
    opts.stdin = stdin;
    let o = bounded::run(&mut cmd, &opts, None).map_err(|e| format!("cannot run git: {e}"))?;
    let shown = args.iter().take(3).copied().collect::<Vec<_>>().join(" ");
    match o.end {
        End::Exited { success: true, .. } => Ok(o.stdout.text()),
        End::Exited { .. } => Err(format!("git {shown}: {}", o.stderr.text().trim())),
        End::TimedOut => Err(format!(
            "git {shown} took longer than {}s and was stopped",
            GUARDED_TIMEOUT.as_secs()
        )),
        End::TooLarge { .. } => Err(format!(
            "git {shown} printed more than {} MB and was stopped",
            GUARDED_STDOUT / (1024 * 1024)
        )),
        End::Cancelled => Err(format!("git {shown} was cancelled")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The argv and environment edits of every git child, fixed before the
    /// environment cleanup moved to `pod_process::git` (refactor L1).
    #[test]
    fn command_line_and_environment_are_fixed() {
        let cmd = command(Path::new(r"C:\fixture\repo"));
        let exe = std::env::var_os(ENV_GIT).unwrap_or_else(|| "git".into());
        assert_eq!(cmd.get_program(), exe.as_os_str());
        let args: Vec<_> = cmd.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(
            args,
            [
                "-C",
                r"C:\fixture\repo",
                "-c",
                "core.longpaths=true",
                "-c",
                "core.quotepath=false",
                "-c",
                "core.fsmonitor=false",
            ]
        );
        assert_eq!(cmd.get_current_dir(), None);
        let mut env: Vec<(String, Option<String>)> = cmd
            .get_envs()
            .map(|(k, v)| {
                let v = v.map(|v| v.to_string_lossy().into_owned());
                (k.to_string_lossy().into_owned(), v)
            })
            .collect();
        env.sort();
        let gone = |k: &str| -> (String, Option<String>) { (k.into(), None) };
        let set = |k: &str, v: &str| -> (String, Option<String>) { (k.into(), Some(v.into())) };
        assert_eq!(
            env,
            [
                gone("GIT_ALTERNATE_OBJECT_DIRECTORIES"),
                gone("GIT_CEILING_DIRECTORIES"),
                gone("GIT_COMMON_DIR"),
                gone("GIT_DIR"),
                gone("GIT_INDEX_FILE"),
                gone("GIT_NAMESPACE"),
                gone("GIT_OBJECT_DIRECTORY"),
                set("GIT_OPTIONAL_LOCKS", "0"),
                gone("GIT_PREFIX"),
                set("GIT_TERMINAL_PROMPT", "0"),
                gone("GIT_WORK_TREE"),
                set("LC_ALL", "C"),
            ]
        );
    }
}
