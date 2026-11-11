//! Repository configuration (`.pod/config.toml`) and its approval
//! (docs/19 §3.2).
//!
//! The file is committed by whoever writes the repository, so its commands
//! are someone else's code: nothing from it runs until the user approved
//! that exact content. The approval is the SHA-256 of the file's commands
//! (setup script, tabs, archive script), stored per repository in the
//! registry; any change to a command asks again. Personal settings
//! (`repo.set {personal}`) replace the file's sections one by one and need no
//! approval.
//!
//! Commands run in Windows PowerShell (`-NoProfile -EncodedCommand`, so no
//! quoting of the user's text is involved) in the task's worktree.

use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::tasks::{RepoRecord, blocking};
use super::{Daemon, log};
use crate::protocol::*;

/// `<repo>\.pod\config.toml`.
pub fn config_path(root: &Path) -> PathBuf {
    root.join(".pod").join("config.toml")
}

/// What was read from a worktree or the repository root.
#[derive(Debug, Clone, Default)]
pub struct Loaded {
    pub path: PathBuf,
    pub exists: bool,
    pub file: Option<RepoConfigFile>,
    pub error: Option<String>,
    pub hash: Option<String>,
}

/// Larger files are refused (a config file is a few lines).
const MAX_CONFIG_BYTES: u64 = 256 * 1024;

pub fn load(root: &Path) -> Loaded {
    let path = config_path(root);
    let mut out = Loaded {
        path: path.clone(),
        ..Default::default()
    };
    let meta = match std::fs::metadata(&path) {
        Ok(m) if m.is_file() => m,
        _ => return out,
    };
    out.exists = true;
    if meta.len() > MAX_CONFIG_BYTES {
        out.error = Some(format!("{} is larger than 256 KiB", path.display()));
        return out;
    }
    match std::fs::read_to_string(&path) {
        Ok(text) => match parse(&text) {
            Ok(f) => {
                out.hash = commands_hash(&f);
                out.file = Some(f);
            }
            Err(e) => out.error = Some(e),
        },
        Err(e) => out.error = Some(format!("cannot read {}: {e}", path.display())),
    }
    out
}

pub fn parse(text: &str) -> Result<RepoConfigFile, String> {
    let mut f: RepoConfigFile =
        toml::from_str(text).map_err(|e| format!("invalid .pod/config.toml: {e}"))?;
    for t in &f.tabs {
        if t.title.trim().is_empty() || t.command.trim().is_empty() {
            return Err(
                "invalid .pod/config.toml: every [[tabs]] needs a title and a command".into(),
            );
        }
    }
    if let Some(w) = f.worktree.as_mut() {
        w.sparse = sparse_dirs(&w.sparse)
            .map_err(|e| format!("invalid .pod/config.toml: [worktree] sparse: {e}"))?;
    }
    Ok(f)
}

/// Most `[worktree] sparse` entries.
const MAX_SPARSE: usize = 64;

/// Checks and normalizes cone-mode directories: relative, `/`-separated,
/// no `..`, no glob characters, no option-like names. `.pod` is always
/// added (the task reads its config from the worktree).
pub fn sparse_dirs(dirs: &[String]) -> Result<Vec<String>, String> {
    if dirs.is_empty() {
        return Ok(Vec::new());
    }
    if dirs.len() > MAX_SPARSE {
        return Err(format!("at most {MAX_SPARSE} directories"));
    }
    let mut out: Vec<String> = Vec::new();
    for d in dirs {
        let n = d.trim().replace('\\', "/");
        let n = n.trim_matches('/');
        let bad = n.is_empty()
            || n.starts_with('-')
            || n.contains(':')
            || n.chars()
                .any(|c| matches!(c, '*' | '?' | '[' | ']' | '!' | '\n' | '\r' | '\0'))
            || n.split('/').any(|p| p.is_empty() || p == "." || p == "..");
        if bad {
            return Err(format!("{d:?} is not a relative directory"));
        }
        if !out.iter().any(|x| x == n) {
            out.push(n.to_string());
        }
    }
    if !out.iter().any(|x| x == ".pod") {
        out.push(".pod".into());
    }
    Ok(out)
}

/// `[worktree] sparse` of the config at `commit` (empty when there is no
/// file or no section; an invalid file is an error).
pub fn sparse_at(repo: &Path, commit: &str) -> Result<Vec<String>, String> {
    let Ok(text) = crate::git::show_file(repo, commit, ".pod/config.toml") else {
        return Ok(Vec::new());
    };
    if text.len() as u64 > MAX_CONFIG_BYTES {
        return Err(".pod/config.toml is larger than 256 KiB".into());
    }
    Ok(parse(&text)?.worktree.map(|w| w.sparse).unwrap_or_default())
}

fn nonempty(s: &Option<String>) -> Option<&str> {
    s.as_deref().map(str::trim).filter(|s| !s.is_empty())
}

/// SHA-256 (hex) over the file's commands, `None` when it has none.
pub fn commands_hash(f: &RepoConfigFile) -> Option<String> {
    let mut parts: Vec<(&str, &str, &str)> = Vec::new();
    if let Some(s) = f.setup.as_ref().and_then(|s| nonempty(&s.script)) {
        parts.push(("setup", "", s));
    }
    for t in &f.tabs {
        parts.push(("tab", t.title.as_str(), t.command.as_str()));
    }
    if let Some(s) = f.archive.as_ref().and_then(|a| nonempty(&a.script)) {
        parts.push(("archive", "", s));
    }
    if parts.is_empty() {
        return None;
    }
    let mut h = Sha256::new();
    h.update(b"pod-config-commands-v1\0");
    for (kind, title, cmd) in parts {
        for x in [kind, title, cmd] {
            h.update((x.len() as u64).to_le_bytes());
            h.update(x.as_bytes());
        }
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// One command with where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Cmd {
    pub command: String,
    /// The repository file (needs approval) vs the user's own settings.
    pub personal: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Effective {
    pub setup: Option<Cmd>,
    pub mode: SetupMode,
    pub tabs: Vec<(String, Cmd)>,
    pub archive: Option<Cmd>,
    pub worker_agent: Option<String>,
    /// `[worker] codex_allowance` (docs/19 §18.1); on unless the file says
    /// `false`.
    pub codex_allowance: bool,
    /// Hash of the file's commands and whether it is approved.
    pub hash: Option<String>,
    pub trusted: bool,
}

impl Effective {
    /// A repository (not personal) command is used and not approved.
    pub fn uses_untrusted(&self, c: &Cmd) -> bool {
        !c.personal && !self.trusted
    }

    pub fn commands(&self) -> Vec<ConfigCommand> {
        let src = |c: &Cmd| if c.personal { "personal" } else { "repo" }.to_string();
        let mut v = Vec::new();
        if let Some(c) = &self.setup {
            v.push(ConfigCommand {
                kind: "setup".into(),
                command: c.command.clone(),
                source: src(c),
            });
        }
        for (title, c) in &self.tabs {
            v.push(ConfigCommand {
                kind: format!("tab:{title}"),
                command: c.command.clone(),
                source: src(c),
            });
        }
        if let Some(c) = &self.archive {
            v.push(ConfigCommand {
                kind: "archive".into(),
                command: c.command.clone(),
                source: src(c),
            });
        }
        v
    }
}

/// Personal sections replace the file's; the file counts only when it parsed.
pub fn effective(loaded: &Loaded, repo: &RepoRecord) -> Effective {
    let file = loaded.file.clone().unwrap_or_default();
    let personal = repo.personal.clone().unwrap_or_default();
    let mut e = Effective {
        hash: loaded.hash.clone(),
        trusted: loaded.hash.is_some() && loaded.hash == repo.approved_hash,
        ..Default::default()
    };
    let (setup, setup_personal) = match &personal.setup {
        Some(s) => (Some(s.clone()), true),
        None => (file.setup.clone(), false),
    };
    if let Some(s) = &setup {
        e.mode = s.mode.unwrap_or_default();
        e.setup = nonempty(&s.script).map(|c| Cmd {
            command: c.to_string(),
            personal: setup_personal,
        });
    }
    e.tabs = match &personal.tabs {
        Some(tabs) => tabs
            .iter()
            .map(|t| {
                (
                    t.title.clone(),
                    Cmd {
                        command: t.command.clone(),
                        personal: true,
                    },
                )
            })
            .collect(),
        None => file
            .tabs
            .iter()
            .map(|t| {
                (
                    t.title.clone(),
                    Cmd {
                        command: t.command.clone(),
                        personal: false,
                    },
                )
            })
            .collect(),
    };
    e.archive = match &personal.archive {
        Some(a) => nonempty(&a.script).map(|c| Cmd {
            command: c.into(),
            personal: true,
        }),
        None => file
            .archive
            .as_ref()
            .and_then(|a| nonempty(&a.script))
            .map(|c| Cmd {
                command: c.into(),
                personal: false,
            }),
    };
    e.codex_allowance = file
        .worker
        .as_ref()
        .and_then(|w| w.codex_allowance)
        .unwrap_or(true);
    e.worker_agent = file.worker.and_then(|w| w.agent);
    e
}

// ───────────────────────────── PowerShell commands ─────────────────────────────

/// The script wrapped so the exit code reflects failure: an explicit
/// `exit N` wins, else the last native exit code, else `$?` of the last
/// statement.
fn wrapped(script: &str) -> String {
    format!(
        "$ErrorActionPreference = 'Continue'\r\n{script}\r\n$podOk = $?\r\n\
         if ($LASTEXITCODE) {{ exit $LASTEXITCODE }}\r\nif (-not $podOk) {{ exit 1 }}\r\nexit 0\r\n"
    )
}

fn encoded(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    b64_encode(&utf16)
}

/// argv running `script` once (setup, archive).
pub fn script_argv(script: &str) -> Vec<String> {
    vec![
        "powershell".into(),
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-EncodedCommand".into(),
        encoded(&wrapped(script)),
    ]
}

/// argv for a default tab: runs `command`, then stays an interactive shell.
pub fn tab_argv(command: &str) -> Vec<String> {
    vec![
        "powershell".into(),
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-NoExit".into(),
        "-ExecutionPolicy".into(),
        "Bypass".into(),
        "-EncodedCommand".into(),
        encoded(command),
    ]
}

const OUTPUT_TAIL: usize = 4096;
/// Archive scripts get this long (`POD_ARCHIVE_TIMEOUT_SECS`).
const DEFAULT_SCRIPT_TIMEOUT: Duration = Duration::from_secs(600);

pub fn script_timeout() -> Duration {
    std::env::var("POD_ARCHIVE_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_SCRIPT_TIMEOUT)
}

/// Run `script` without a terminal (no window), in `cwd` with `env`;
/// returns success, exit code and the output tail. The script and
/// everything it starts run in one process tree (Job Object): `timeout`
/// covers the whole call including output pipes that a leftover child holds,
/// and output is kept as a bounded tail while it arrives (orch-03).
pub fn run_script(
    script: &str,
    cwd: &Path,
    env: &std::collections::BTreeMap<String, String>,
    timeout: Duration,
) -> ScriptRun {
    use pod_process::bounded::{self, End, Keep};
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let argv = script_argv(script);
    let mut cmd = std::process::Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(cwd)
        .env_clear()
        .envs(env)
        .creation_flags(CREATE_NO_WINDOW);
    let failed = |output: String| ScriptRun {
        ok: false,
        exit_code: None,
        output,
        skipped: None,
    };
    let mut opts = bounded::Options::new(timeout, Keep::Tail(OUTPUT_TAIL));
    opts.merge = true;
    let o = match bounded::run(&mut cmd, &opts, None) {
        Ok(o) => o,
        Err(e) => return failed(format!("cannot start powershell: {e}")),
    };
    let tail = bounded::tail_str(o.stdout.text().trim_end(), OUTPUT_TAIL);
    match o.end {
        End::Exited { code, success } => ScriptRun {
            ok: success,
            exit_code: code.map(i64::from),
            output: tail,
            skipped: None,
        },
        End::TimedOut | End::Cancelled => {
            failed(format!("timed out after {} s\n{tail}", timeout.as_secs()))
        }
        End::TooLarge { .. } => failed(tail),
    }
}

// ───────────────────────────── RPC ─────────────────────────────

impl Daemon {
    /// Where `.pod/config.toml` is read: a task's worktree or the repo root.
    fn config_root(&self, repo: &RepoRecord, task: Option<&str>) -> Result<PathBuf, RpcError> {
        match task {
            Some(t) => {
                let rec = self.task_record(t)?;
                if rec.repo != repo.name {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        format!("task {} is not in repository {}", rec.id, repo.name),
                    ));
                }
                Ok(PathBuf::from(rec.worktree))
            }
            None => Ok(PathBuf::from(&repo.path)),
        }
    }

    pub(super) async fn repo_config(
        &self,
        p: RepoConfigParams,
    ) -> Result<RepoConfigResult, RpcError> {
        let repo = self.repo_record(&p.repo)?;
        let root = self.config_root(&repo, p.task.as_deref())?;
        let loaded = blocking(move || load(&root)).await?;
        Ok(config_result(&repo, &loaded))
    }

    pub(super) async fn repo_set(&self, p: RepoSetParams) -> Result<RepoConfigResult, RpcError> {
        let repo = self.repo_record(&p.repo)?;
        let root = self.config_root(&repo, p.task.as_deref())?;
        let loaded = blocking(move || load(&root)).await?;
        if let Some(t) = &p.trust
            && t.approve
        {
            match &loaded.hash {
                Some(h) if h.eq_ignore_ascii_case(t.hash.trim()) => {}
                Some(h) => {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        format!(
                            "{} changed: its commands now hash to {h}, not {}; review them again (repo.config)",
                            loaded.path.display(),
                            t.hash
                        ),
                    ));
                }
                None => {
                    return Err(RpcError::new(
                        codes::INVALID_PARAMS,
                        format!("{} has no commands to approve", loaded.path.display()),
                    ));
                }
            }
        }
        if let Some(root) = p.worktrees_root.as_deref().filter(|r| !r.is_empty())
            && !Path::new(root).is_absolute()
        {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("worktrees_root {root:?} must be an absolute path"),
            ));
        }
        let rec = {
            let mut reg = self.registry.lock().unwrap();
            let Some(r) = reg.repos.iter_mut().find(|r| r.name == repo.name) else {
                return Err(RpcError::new(
                    codes::REPO_NOT_FOUND,
                    format!("no repository {:?}", repo.name),
                ));
            };
            if let Some(root) = &p.worktrees_root {
                r.worktrees_root = (!root.is_empty())
                    .then(|| crate::git::normalize(Path::new(root)).display().to_string());
            }
            if p.clear_personal {
                r.personal = None;
            }
            if let Some(personal) = &p.personal {
                r.personal = Some(personal.clone());
            }
            if let Some(t) = &p.trust {
                r.approved_hash = if t.approve { loaded.hash.clone() } else { None };
                log(&format!(
                    "repo {}: commands {} ({})",
                    r.name,
                    if t.approve {
                        "approved"
                    } else {
                        "approval withdrawn"
                    },
                    loaded.hash.as_deref().unwrap_or("-")
                ));
            }
            let rec = r.clone();
            self.save_registry(&mut reg)?;
            rec
        };
        Ok(config_result(&rec, &loaded))
    }
}

pub fn config_result(repo: &RepoRecord, loaded: &Loaded) -> RepoConfigResult {
    let eff = effective(loaded, repo);
    RepoConfigResult {
        repo: repo.name.clone(),
        path: loaded.path.display().to_string(),
        exists: loaded.exists,
        file: loaded.file.clone(),
        error: loaded.error.clone(),
        hash: loaded.hash.clone(),
        trusted: eff.trusted,
        approved_hash: repo.approved_hash.clone(),
        personal: repo.personal.clone(),
        worktrees_root: repo.worktrees_root.clone(),
        commands: eff.commands(),
        setup_mode: eff.mode,
        worker_agent: eff.worker_agent.clone(),
        codex_allowance: Some(eff.codex_allowance),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoRecord {
        RepoRecord {
            name: "r".into(),
            path: "C:\\r".into(),
            added_ms: 0,
            worktrees_root: None,
            personal: None,
            approved_hash: None,
        }
    }

    const FILE: &str = r#"
[setup]
script = "pnpm install --frozen-lockfile"
mode = "wait"

[[tabs]]
title = "dev"
command = "pnpm dev"

[archive]
script = "cargo clean"

[worker]
agent = "claude"
"#;

    #[test]
    fn parses_the_documented_schema() {
        let f = parse(FILE).unwrap();
        assert_eq!(f.setup.as_ref().unwrap().mode, Some(SetupMode::Wait));
        assert_eq!(f.tabs[0].title, "dev");
        assert_eq!(
            f.archive.as_ref().unwrap().script.as_deref(),
            Some("cargo clean")
        );
        assert_eq!(f.worker.as_ref().unwrap().agent.as_deref(), Some("claude"));
        assert!(
            parse("[setup]\nscrpt = \"x\"").is_err(),
            "unknown keys are errors"
        );
        assert!(parse("[[tabs]]\ntitle = \"\"\ncommand = \"x\"").is_err());
        assert!(commands_hash(&parse("[worker]\nagent = \"codex\"").unwrap()).is_none());
    }

    #[test]
    fn codex_allowance_is_on_unless_the_file_turns_it_off() {
        let eff = |text: &str| {
            let loaded = Loaded {
                file: Some(parse(text).unwrap()),
                exists: true,
                ..Default::default()
            };
            effective(&loaded, &repo()).codex_allowance
        };
        assert!(
            effective(&Loaded::default(), &repo()).codex_allowance,
            "no file"
        );
        assert!(eff("[worker]\nagent = \"codex\""));
        assert!(eff("[worker]\ncodex_allowance = true"));
        assert!(!eff("[worker]\ncodex_allowance = false"));
        assert!(parse("[worker]\ncodex_allowance = \"yes\"").is_err());
        // Data, not a command: no approval needed.
        assert!(commands_hash(&parse("[worker]\ncodex_allowance = false").unwrap()).is_none());
        let r = config_result(
            &repo(),
            &Loaded {
                file: Some(parse("[worker]\ncodex_allowance = false").unwrap()),
                ..Default::default()
            },
        );
        assert_eq!(r.codex_allowance, Some(false));
    }

    #[test]
    fn sparse_dirs_are_checked_and_normalized() {
        let f = parse(
            r#"[worktree]
sparse = ['crates\podd/', " docs ", "docs"]
"#,
        )
        .unwrap();
        assert_eq!(
            f.worktree.unwrap().sparse,
            vec!["crates/podd", "docs", ".pod"]
        );
        assert_eq!(
            sparse_dirs(&[]).unwrap(),
            Vec::<String>::new(),
            "no list: a full checkout"
        );
        assert_eq!(
            sparse_dirs(&[".pod".into(), "a".into()]).unwrap(),
            vec![".pod", "a"]
        );
        for bad in [
            "../x", "a/../b", "C:/x", "/", "-x", "a/*", "a//b", "./a", "",
        ] {
            assert!(sparse_dirs(&[bad.to_string()]).is_err(), "{bad:?}");
        }
        assert!(sparse_dirs(&vec!["a".to_string(); 65]).is_err());
        assert!(
            parse("[worktree]\nsparse = [\"..\"]")
                .unwrap_err()
                .contains("[worktree] sparse")
        );
        assert!(
            parse("[worktree]\ncone = false").is_err(),
            "unknown keys are errors"
        );
        // Not a command: the approval hash ignores it.
        assert!(commands_hash(&parse("[worktree]\nsparse = [\"a\"]").unwrap()).is_none());
    }

    #[test]
    fn hash_changes_with_any_command_only() {
        let a = commands_hash(&parse(FILE).unwrap()).unwrap();
        assert_eq!(a.len(), 64);
        let same = FILE
            .replace("agent = \"claude\"", "agent = \"codex\"")
            .replace("mode = \"wait\"", "");
        assert_eq!(
            commands_hash(&parse(&same).unwrap()).unwrap(),
            a,
            "non-command fields do not ask again"
        );
        for changed in [
            FILE.replace("--frozen-lockfile", ""),
            FILE.replace("pnpm dev", "pnpm dev --host"),
            FILE.replace("cargo clean", "cargo clean -p x"),
            FILE.replace("title = \"dev\"", "title = \"web\""),
        ] {
            assert_ne!(
                commands_hash(&parse(&changed).unwrap()).unwrap(),
                a,
                "{changed}"
            );
        }
    }

    #[test]
    fn approval_and_personal_overrides() {
        let mut r = repo();
        let loaded = Loaded {
            exists: true,
            file: Some(parse(FILE).unwrap()),
            hash: commands_hash(&parse(FILE).unwrap()),
            ..Default::default()
        };
        let e = effective(&loaded, &r);
        assert!(!e.trusted);
        assert!(e.uses_untrusted(e.setup.as_ref().unwrap()));
        assert_eq!(e.mode, SetupMode::Wait);
        assert_eq!(e.commands().len(), 3);
        r.approved_hash = loaded.hash.clone();
        let e = effective(&loaded, &r);
        assert!(e.trusted && !e.uses_untrusted(e.setup.as_ref().unwrap()));
        // Personal setup wins and needs no approval; other sections stay the file's.
        r.approved_hash = None;
        r.personal = Some(PersonalConfig {
            setup: Some(SetupSection {
                script: Some("npm ci".into()),
                mode: None,
            }),
            tabs: Some(vec![]),
            archive: None,
        });
        let e = effective(&loaded, &r);
        let setup = e.setup.as_ref().unwrap();
        assert_eq!((setup.command.as_str(), setup.personal), ("npm ci", true));
        assert!(!e.uses_untrusted(setup));
        assert_eq!(e.mode, SetupMode::Parallel);
        assert!(e.tabs.is_empty());
        assert!(e.uses_untrusted(e.archive.as_ref().unwrap()));
    }

    #[test]
    fn script_exit_codes_and_output() {
        let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
        let dir = std::env::temp_dir();
        let t = Duration::from_secs(60);
        let ok = run_script("Write-Output 'hi \"there\" 100%'", &dir, &env, t);
        assert!(ok.ok, "{ok:?}");
        assert!(ok.output.contains("hi \"there\" 100%"), "{ok:?}");
        let bad = run_script("Write-Output before\r\nexit 3", &dir, &env, t);
        assert_eq!((bad.ok, bad.exit_code), (false, Some(3)), "{bad:?}");
        let native = run_script("cmd /c exit 5", &dir, &env, t);
        assert_eq!(native.exit_code, Some(5), "{native:?}");
        let cmdlet = run_script(
            "Get-Item C:\\definitely\\not\\here -ErrorAction Stop",
            &dir,
            &env,
            t,
        );
        assert!(!cmdlet.ok, "{cmdlet:?}");
    }

    /// orch-03: the script exits at once but leaves a child holding the
    /// output pipes for two minutes; the call ends after the short
    /// after-exit grace (never past the timeout) and the child is killed.
    /// Large output keeps only a bounded tail.
    #[test]
    fn script_timeout_covers_leftover_children_and_output_is_bounded() {
        let env: std::collections::BTreeMap<String, String> = std::env::vars().collect();
        let dir = std::env::temp_dir();
        let marker = "127.0.0.51";
        let t = std::time::Instant::now();
        let run = run_script(
            &format!("cmd /c \"start /b ping -n 120 {marker} & echo started\""),
            &dir,
            &env,
            Duration::from_secs(30),
        );
        assert!(t.elapsed() < Duration::from_secs(15), "{:?}", t.elapsed());
        assert!(run.output.contains("started"), "{run:?}");
        let left = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!("@(Get-CimInstance Win32_Process | Where-Object {{ $_.CommandLine -like '*{marker}*' -and $_.Name -eq 'ping.exe' }}).Count")])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&left.stdout).trim(),
            "0",
            "leftover child killed"
        );

        let big = run_script(
            "1..50000 | ForEach-Object { \"line-$_\" }",
            &dir,
            &env,
            Duration::from_secs(120),
        );
        assert!(big.ok, "{big:?}");
        assert!(big.output.len() <= OUTPUT_TAIL + 3, "{}", big.output.len());
        assert!(
            big.output.ends_with("line-50000"),
            "{:?}",
            &big.output[big.output.len() - 40..]
        );
    }
}
