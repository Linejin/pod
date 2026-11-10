//! `pod hooks install|uninstall|status`: Pod's agent hooks in the user's
//! global agent configuration (user decision 2026-11-08, docs/07 §9).
//!
//! - `pod-hook.exe` is copied to a stable per-user place
//!   (`%LOCALAPPDATA%\Pod\bin`), and the hooks point there, so moving or
//!   rebuilding the repository never breaks the user's agents.
//! - Claude `~/.claude/settings.json` and Codex `~/.codex/hooks.json` are
//!   merged with `pod_status::install` (only groups whose command contains
//!   `pod-hook` are added/replaced/removed; other keys and other tools'
//!   hooks, e.g. another IDE's, stay). Every file is backed up before it is written
//!   (`<file>.pod-backup-<UTC timestamp>`).
//! - Codex only runs hooks the user trusted (`[hooks.state]` in
//!   `config.toml`, keyed by file/event/position, pinned to a hash Codex
//!   computes). Pod asks Codex itself: `codex app-server` `hooks/list` gives
//!   each hook's `currentHash`, `config/batchWrite` stores it for Pod's hooks
//!   only. `CODEX_HOME` is set explicitly so a
//!   `--home` install never touches the real configuration.
//! - The hooks are no-ops outside Pod: `pod-hook` exits at once when
//!   `POD_TERMINAL_ID`/`POD_SOCKET` are unset.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use pod_status::HookAgent;
use pod_status::install::{
    self, ApplyOptions, HookCommandSpec, InstallPlan, InstallScope, POD_HOOK_MARKER,
};
use serde::Serialize;
use serde_json::{Value, json};

/// Where `pod-hook.exe` lives for installed hooks.
pub fn default_bin_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Pod")
        .join("bin")
}

/// `YYYYMMDD-HHMMSS` (UTC).
pub fn utc_stamp(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Copy `path` to `<path>.pod-backup-<stamp>` (adds `-N` if that exists).
pub fn backup(path: &Path, stamp: &str) -> std::io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let mut n = 0;
    loop {
        let suffix = if n == 0 {
            String::new()
        } else {
            format!("-{n}")
        };
        let b = path.with_file_name(format!("{name}.pod-backup-{stamp}{suffix}"));
        if !b.exists() {
            std::fs::copy(path, &b)?;
            return Ok(Some(b));
        }
        n += 1;
    }
}

/// Copy `src` to `<bin_dir>/pod-hook.exe` unless identical. A running copy
/// (an agent executing a hook right now) cannot be overwritten on Windows but
/// can be renamed, so the old file is moved aside first.
pub fn install_binary(src: &Path, bin_dir: &Path, stamp: &str) -> anyhow::Result<(PathBuf, bool)> {
    let dst = bin_dir.join("pod-hook.exe");
    let new = std::fs::read(src).with_context(|| format!("read {}", src.display()))?;
    if std::fs::read(&dst).is_ok_and(|old| old == new) {
        return Ok((dst, false));
    }
    std::fs::create_dir_all(bin_dir).with_context(|| format!("create {}", bin_dir.display()))?;
    let tmp = bin_dir.join("pod-hook.exe.new");
    std::fs::write(&tmp, &new).with_context(|| format!("write {}", tmp.display()))?;
    if dst.exists() && std::fs::remove_file(&dst).is_err() {
        let aside = bin_dir.join(format!("pod-hook.exe.old-{stamp}"));
        std::fs::rename(&dst, &aside)
            .with_context(|| format!("move the running {} aside", dst.display()))?;
    }
    std::fs::rename(&tmp, &dst).with_context(|| format!("install {}", dst.display()))?;
    Ok((dst, true))
}

#[derive(Debug, Serialize)]
pub struct FileResult {
    pub agent: &'static str,
    pub path: PathBuf,
    /// `unchanged`, `created`, `updated`, or (dry run) `would create|update`.
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
    /// Dry run: the full content that would be written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct CodexTrust {
    /// Pod hooks Codex reports for this CODEX_HOME.
    pub pod_hooks: usize,
    /// Trust entries written now.
    pub newly_trusted: usize,
    /// Pod hooks still not trusted after the write (should be 0).
    pub untrusted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config_backup: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct Outcome {
    pub dry_run: bool,
    pub hook_exe: PathBuf,
    pub binary_copied: bool,
    pub files: Vec<FileResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_trust: Option<CodexTrust>,
    /// Uninstall: Pod's Codex trust entries removed from `config.toml`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_untrust: Option<CodexUntrust>,
    pub notes: Vec<String>,
}

fn agent_name(a: HookAgent) -> &'static str {
    a.as_str()
}

/// Write a plan: backup + write each changed file.
fn apply(plan: &InstallPlan, stamp: &str, dry_run: bool) -> anyhow::Result<Vec<FileResult>> {
    let mut out = Vec::new();
    for f in &plan.files {
        let action = match (f.unchanged, f.existed, dry_run) {
            (true, _, _) => "unchanged",
            (false, true, true) => "would update",
            (false, false, true) => "would create",
            (false, true, false) => "updated",
            (false, false, false) => "created",
        };
        let backup = if !dry_run && !f.unchanged {
            backup(&f.path, stamp).with_context(|| format!("backup {}", f.path.display()))?
        } else {
            None
        };
        out.push(FileResult {
            agent: agent_name(f.agent),
            path: f.path.clone(),
            action: action.into(),
            backup,
            content: (dry_run && !f.unchanged).then(|| f.content.clone()),
        });
    }
    if !dry_run {
        // The user decided on a global install (docs/07 §9): writing the
        // real agent config is intended here.
        install::apply_plan(
            plan,
            ApplyOptions {
                allow_user_config: true,
            },
        )?;
    }
    Ok(out)
}

pub struct Request {
    pub scope: InstallScope,
    pub agents: Vec<HookAgent>,
    pub bin_dir: PathBuf,
    pub source: PathBuf,
    pub dry_run: bool,
    pub codex_trust: bool,
}

fn codex_home(scope: &InstallScope) -> PathBuf {
    match scope {
        InstallScope::Global { home } => home.join(".codex"),
        InstallScope::Profile { codex_home, .. } => codex_home.clone(),
    }
}

pub fn install(req: &Request) -> anyhow::Result<Outcome> {
    let stamp = utc_stamp(SystemTime::now());
    let hook_exe = req.bin_dir.join("pod-hook.exe");
    let mut notes = Vec::new();
    let binary_copied = if req.dry_run {
        false
    } else {
        if !req.source.is_file() {
            bail!(
                "{} not found (build pod-hook or pass --source)",
                req.source.display()
            );
        }
        install_binary(&req.source, &req.bin_dir, &stamp)?.1
    };
    let spec = HookCommandSpec::new(&hook_exe);
    let plan = install::plan_install(&req.scope, &spec, &req.agents)?;
    let files = apply(&plan, &stamp, req.dry_run)?;
    let mut codex_trust = None;
    if req.agents.contains(&HookAgent::Codex) {
        if req.dry_run {
            notes.push("would ask `codex app-server` to trust Pod's Codex hooks ([hooks.state] in config.toml)".into());
        } else if req.codex_trust {
            match trust_codex_hooks(&codex_home(&req.scope), &stamp) {
                Ok(t) => codex_trust = Some(t),
                Err(e) => notes.push(format!(
                    "Codex hook trust not written ({e:#}); Codex will list Pod's hooks as untrusted until they are approved in Codex"
                )),
            }
        } else {
            notes.push("--no-codex-trust: Codex will list Pod's hooks as untrusted until they are approved in Codex".into());
        }
    }
    notes.extend(plan.notes);
    Ok(Outcome {
        dry_run: req.dry_run,
        hook_exe,
        binary_copied,
        files,
        codex_trust,
        codex_untrust: None,
        notes,
    })
}

pub fn uninstall(req: &Request) -> anyhow::Result<Outcome> {
    let stamp = utc_stamp(SystemTime::now());
    let hook_exe = req.bin_dir.join("pod-hook.exe");
    let spec = HookCommandSpec::new(&hook_exe);
    let plan = install::plan_uninstall(&req.scope, &spec, &req.agents)?;
    // Do not create files that did not exist.
    let plan = InstallPlan {
        files: plan.files.into_iter().filter(|f| f.existed).collect(),
        ..plan
    };
    // Codex trust entries are keyed by hook position: read the positions
    // before Pod's groups are removed.
    let codex_hooks = req.scope.config_path(HookAgent::Codex);
    let codex_moves = if req.agents.contains(&HookAgent::Codex) {
        std::fs::read_to_string(&codex_hooks)
            .ok()
            .filter(|t| !t.trim().is_empty())
            .map(|t| serde_json::from_str::<Value>(&t))
            .transpose()
            .with_context(|| format!("{} is not valid JSON", codex_hooks.display()))?
            .map(|v| codex_group_moves(&v))
    } else {
        None
    };
    let files = apply(&plan, &stamp, req.dry_run)?;
    let mut notes = vec![format!(
        "{} is left in place (an agent may be running it)",
        req.bin_dir.join("pod-hook.exe").display()
    )];
    let mut codex_untrust = None;
    if let Some(moves) = codex_moves {
        let config = codex_home(&req.scope).join("config.toml");
        match untrust_codex_hooks(&config, &codex_hooks, &moves, &stamp, req.dry_run) {
            Ok(u) => codex_untrust = Some(u),
            Err(e) => notes.push(format!(
                "Codex trust entries in {} not cleaned ({e:#}); they are inert (pinned to the old hook hash)",
                config.display()
            )),
        }
    }
    Ok(Outcome {
        dry_run: req.dry_run,
        hook_exe,
        binary_copied: false,
        files,
        codex_trust: None,
        codex_untrust,
        notes,
    })
}

// ─────────────────────────── codex trust cleanup ───────────────────────────

/// Where each hook group of a Codex `hooks.json` ends up once Pod's groups
/// are removed: `(event_snake, old group index) -> Some(new index)`, or
/// `None` for Pod's own groups.
pub fn codex_group_moves(
    hooks_json: &Value,
) -> std::collections::HashMap<(String, usize), Option<usize>> {
    let mut out = std::collections::HashMap::new();
    let Some(events) = hooks_json.get("hooks").and_then(Value::as_object) else {
        return out;
    };
    for (event, groups) in events {
        let mut kept = 0;
        for (g, group) in groups.as_array().into_iter().flatten().enumerate() {
            let pod = group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|h| h.get("command").and_then(Value::as_str))
                .any(is_pod_command);
            let to = (!pod).then(|| {
                kept += 1;
                kept - 1
            });
            out.insert((snake_case(event), g), to);
        }
    }
    out
}

/// `PreToolUse` -> `pre_tool_use` (Codex's event names in trust keys).
pub fn snake_case(event: &str) -> String {
    let mut s = String::new();
    for (i, c) in event.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                s.push('_');
            }
            s.push(c.to_ascii_lowercase());
        } else {
            s.push(c);
        }
    }
    s
}

fn same_file(a: &str, b: &Path) -> bool {
    let norm = |s: &str| {
        s.trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_ascii_lowercase()
    };
    norm(a) == norm(&b.display().to_string())
}

#[derive(Debug, Default, Serialize, PartialEq)]
pub struct CodexUntrust {
    pub config: PathBuf,
    /// Trust entries of Pod's removed hooks.
    pub removed: usize,
    /// Other tools' entries re-keyed to their hook's new position.
    pub moved: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backup: Option<PathBuf>,
}

/// Edit `[hooks.state]` of a Codex `config.toml` text: drop the entries of
/// Pod's hook groups in `hooks_json` and re-key other entries of that file
/// whose group index shifts. Returns `None` when nothing changes.
pub fn prune_codex_trust(
    config: &str,
    hooks_json: &Path,
    moves: &std::collections::HashMap<(String, usize), Option<usize>>,
) -> anyhow::Result<Option<(String, usize, usize)>> {
    let mut doc: toml_edit::DocumentMut =
        config.parse().context("config.toml is not valid TOML")?;
    let Some(state) = doc
        .get_mut("hooks")
        .and_then(|h| h.get_mut("state"))
        .and_then(|s| s.as_table_like_mut())
    else {
        return Ok(None);
    };
    let keys: Vec<String> = state.iter().map(|(k, _)| k.to_string()).collect();
    let (mut removed, mut renames) = (0, Vec::new());
    for key in keys {
        // `<hooks.json path>:<event>:<group>:<hook>`; the path has colons too.
        let parts: Vec<&str> = key.rsplitn(4, ':').collect();
        let [hook, group, event, path] = parts[..] else {
            continue;
        };
        let Ok(g) = group.parse::<usize>() else {
            continue;
        };
        if !same_file(path, hooks_json) {
            continue;
        }
        match moves.get(&(event.to_string(), g)) {
            Some(None) => {
                state.remove(&key);
                removed += 1;
            }
            Some(Some(to)) if *to != g => {
                renames.push((*to, key.clone(), format!("{path}:{event}:{to}:{hook}")));
            }
            _ => {}
        }
    }
    // Lowest new index first, so a chain of shifts never overwrites.
    renames.sort_by_key(|r| r.0);
    let mut moved = 0;
    for (_, from, to) in renames {
        if state.get(&to).is_some() {
            continue;
        }
        if let Some(item) = state.remove(&from) {
            state.insert(&to, item);
            moved += 1;
        }
    }
    if removed == 0 && moved == 0 {
        return Ok(None);
    }
    Ok(Some((doc.to_string(), removed, moved)))
}

/// Remove Pod's Codex trust entries from `config` (backed up first).
fn untrust_codex_hooks(
    config: &Path,
    hooks_json: &Path,
    moves: &std::collections::HashMap<(String, usize), Option<usize>>,
    stamp: &str,
    dry_run: bool,
) -> anyhow::Result<CodexUntrust> {
    let mut out = CodexUntrust {
        config: config.to_path_buf(),
        ..Default::default()
    };
    let Ok(text) = std::fs::read_to_string(config) else {
        return Ok(out);
    };
    let Some((new, removed, moved)) = prune_codex_trust(&text, hooks_json, moves)? else {
        return Ok(out);
    };
    out.removed = removed;
    out.moved = moved;
    if !dry_run {
        out.backup = backup(config, stamp)?;
        let tmp = config.with_extension("toml.pod-new");
        std::fs::write(&tmp, new).with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, config).with_context(|| format!("replace {}", config.display()))?;
    }
    Ok(out)
}

// ─────────────────────────── status ───────────────────────────

#[derive(Debug, Serialize)]
pub struct AgentStatus {
    pub agent: &'static str,
    pub path: PathBuf,
    pub exists: bool,
    /// Events with a Pod hook group.
    pub pod_events: Vec<String>,
    /// Hook groups of other tools (kept untouched).
    pub foreign_groups: usize,
    /// Executables the Pod hooks run, and whether they exist.
    pub hook_exes: Vec<(String, bool)>,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub bin: PathBuf,
    pub bin_exists: bool,
    /// `pod-hook --version` of the installed copy (`None`: missing, or a
    /// build from before `--version`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_version: Option<String>,
    /// The `pod-hook.exe` this `pod` would install, and its version.
    pub source: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_version: Option<String>,
    /// The installed binary equals the `pod-hook.exe` next to this `pod.exe`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bin_current: Option<bool>,
    pub agents: Vec<AgentStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_trust: Option<CodexTrust>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_trust_error: Option<String>,
}

/// `pod-hook --version` -> `0.1.0` (2 s budget; `None` for anything else).
pub fn hook_version(exe: &Path) -> Option<String> {
    if !exe.is_file() {
        return None;
    }
    let mut cmd = Command::new(exe);
    cmd.arg("--version")
        // Outside Pod a hook is a no-op even if it misreads the argument.
        .env_remove("POD_TERMINAL_ID")
        .env_remove("POD_SOCKET")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut stdout, &mut s);
        let _ = tx.send(s);
    });
    let out = rx.recv_timeout(Duration::from_secs(2)).ok();
    let _ = child.kill();
    let _ = child.wait();
    parse_hook_version(&out?)
}

fn parse_hook_version(out: &str) -> Option<String> {
    out.lines()
        .find_map(|l| l.trim().strip_prefix("pod-hook "))
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn is_pod_command(c: &str) -> bool {
    c.contains(POD_HOOK_MARKER)
}

/// The executable of a hook command line (`"C:/a b/x.exe" args` or `C:/x.exe args`).
fn command_exe(c: &str) -> String {
    let c = c.trim();
    if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next().unwrap_or("").to_string()
    } else {
        c.split_whitespace().next().unwrap_or("").to_string()
    }
}

pub fn status(req: &Request, check_codex: bool) -> anyhow::Result<Status> {
    let bin = req.bin_dir.join("pod-hook.exe");
    let installed = std::fs::read(&bin).ok();
    let bin_current = match (&installed, std::fs::read(&req.source).ok()) {
        (Some(a), Some(b)) => Some(*a == b),
        _ => None,
    };
    let mut agents = Vec::new();
    for &a in &req.agents {
        let path = req.scope.config_path(a);
        let text = std::fs::read_to_string(&path).ok();
        let v: Value = match &text {
            Some(t) if !t.trim().is_empty() => serde_json::from_str(t)
                .with_context(|| format!("{} is not valid JSON", path.display()))?,
            _ => json!({}),
        };
        let mut pod_events = Vec::new();
        let mut foreign_groups = 0;
        let mut hook_exes: Vec<(String, bool)> = Vec::new();
        if let Some(h) = v.get("hooks").and_then(Value::as_object) {
            for (ev, groups) in h {
                for g in groups.as_array().into_iter().flatten() {
                    let cmds: Vec<&str> = g
                        .get("hooks")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|x| x.get("command").and_then(Value::as_str))
                        .collect();
                    if cmds.iter().any(|c| is_pod_command(c)) {
                        pod_events.push(ev.clone());
                        for c in cmds {
                            let exe = command_exe(c);
                            if !hook_exes.iter().any(|(e, _)| *e == exe) {
                                let ok = Path::new(&exe).is_file();
                                hook_exes.push((exe, ok));
                            }
                        }
                    } else {
                        foreign_groups += 1;
                    }
                }
            }
        }
        agents.push(AgentStatus {
            agent: agent_name(a),
            path,
            exists: text.is_some(),
            pod_events,
            foreign_groups,
            hook_exes,
        });
    }
    let (mut codex_trust, mut codex_trust_error) = (None, None);
    if check_codex && req.agents.contains(&HookAgent::Codex) {
        match CodexAppServer::start(&codex_home(&req.scope)).and_then(|mut s| s.pod_hooks()) {
            Ok(hooks) => {
                codex_trust = Some(CodexTrust {
                    pod_hooks: hooks.len(),
                    newly_trusted: 0,
                    untrusted: hooks.iter().filter(|h| !h.trusted).count(),
                    config_backup: None,
                })
            }
            Err(e) => codex_trust_error = Some(format!("{e:#}")),
        }
    }
    Ok(Status {
        bin_exists: installed.is_some(),
        bin_version: installed.as_ref().and_then(|_| hook_version(&bin)),
        source_version: hook_version(&req.source),
        source: req.source.clone(),
        bin,
        bin_current,
        agents,
        codex_trust,
        codex_trust_error,
    })
}

// ─────────────────────────── codex app-server ───────────────────────────

struct PodHook {
    key: String,
    hash: String,
    trusted: bool,
}

/// Minimal client of `codex app-server` (newline-delimited JSON-RPC on stdio).
struct CodexAppServer {
    child: Child,
    stdin: std::process::ChildStdin,
    lines: mpsc::Receiver<String>,
    next_id: u64,
}

const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(30);

impl CodexAppServer {
    fn start(codex_home: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(codex_home).ok();
        let mut cmd = Command::new(std::env::var_os("POD_CODEX").unwrap_or_else(|| "codex".into()));
        cmd.args([
            "-c",
            "approval_policy=never",
            "-s",
            "read-only",
            "-a",
            "never",
            "app-server",
        ])
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = cmd
            .spawn()
            .context("cannot start `codex app-server` (is codex on PATH?)")?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for l in BufReader::new(stdout).lines() {
                let Ok(l) = l else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        let mut s = Self {
            child,
            stdin,
            lines,
            next_id: 1,
        };
        s.request(
            "initialize",
            json!({"clientInfo": {"name": "pod", "title": "Pod", "version": env!("CARGO_PKG_VERSION")}}),
        )?;
        s.send(&json!({"method": "initialized"}))?;
        Ok(s)
    }

    fn send(&mut self, v: &Value) -> anyhow::Result<()> {
        let mut line = serde_json::to_string(v)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.flush()?;
        Ok(())
    }

    fn request(&mut self, method: &str, params: Value) -> anyhow::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id": id, "method": method, "params": params}))?;
        let deadline = Instant::now() + APP_SERVER_TIMEOUT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self
                .lines
                .recv_timeout(left)
                .with_context(|| format!("codex app-server: no reply to {method}"))?;
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id") != Some(&json!(id)) {
                continue; // notifications, other replies
            }
            if let Some(e) = v.get("error") {
                bail!("codex app-server {method}: {e}");
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Pod hooks Codex sees in this CODEX_HOME.
    fn pod_hooks(&mut self) -> anyhow::Result<Vec<PodHook>> {
        let cwd = std::env::temp_dir().display().to_string();
        let r = self.request("hooks/list", json!({"cwds": [cwd]}))?;
        let mut out = Vec::new();
        for entry in r
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            for h in entry
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let cmd = h.get("command").and_then(Value::as_str).unwrap_or("");
                if !is_pod_command(cmd) {
                    continue;
                }
                let (Some(key), Some(hash)) = (
                    h.get("key").and_then(Value::as_str),
                    h.get("currentHash").and_then(Value::as_str),
                ) else {
                    continue;
                };
                if out.iter().any(|x: &PodHook| x.key == key) {
                    continue;
                }
                out.push(PodHook {
                    key: key.to_string(),
                    hash: hash.to_string(),
                    trusted: h.get("trustStatus").and_then(Value::as_str) == Some("trusted"),
                });
            }
        }
        Ok(out)
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Trust Pod's (and only Pod's) Codex hooks in `<codex_home>/config.toml`.
pub fn trust_codex_hooks(codex_home: &Path, stamp: &str) -> anyhow::Result<CodexTrust> {
    let mut s = CodexAppServer::start(codex_home)?;
    let hooks = s.pod_hooks()?;
    let todo: Vec<&PodHook> = hooks.iter().filter(|h| !h.trusted).collect();
    let mut out = CodexTrust {
        pod_hooks: hooks.len(),
        ..Default::default()
    };
    if !todo.is_empty() {
        out.config_backup = backup(&codex_home.join("config.toml"), stamp)?;
        let mut value = serde_json::Map::new();
        for h in &todo {
            value.insert(h.key.clone(), json!({"trusted_hash": h.hash}));
        }
        s.request(
            "config/batchWrite",
            json!({
                "edits": [{"keyPath": "hooks.state", "value": value, "mergeStrategy": "upsert"}],
                "reloadUserConfig": true,
            }),
        )?;
        out.newly_trusted = todo.len();
    }
    out.untrusted = s.pod_hooks()?.iter().filter(|h| !h.trusted).count();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101-000000");
        // 2026-11-08 12:34:56 UTC
        let t = UNIX_EPOCH + Duration::from_secs(1_794_141_296);
        assert_eq!(utc_stamp(t), "20261108-123456");
        let leap = UNIX_EPOCH + Duration::from_secs(951_782_400); // 2000-02-29
        assert_eq!(utc_stamp(leap), "20000229-000000");
    }

    #[test]
    fn command_exes() {
        assert_eq!(
            command_exe(r#""C:/a b/pod-hook.exe" claude Stop"#),
            "C:/a b/pod-hook.exe"
        );
        assert_eq!(
            command_exe("C:/x/pod-hook.exe codex Stop"),
            "C:/x/pod-hook.exe"
        );
    }

    #[test]
    fn hook_versions_and_event_names() {
        assert_eq!(
            parse_hook_version("pod-hook 0.1.0\n").as_deref(),
            Some("0.1.0")
        );
        // A pre-`--version` build answers like a hook: `{}`.
        assert_eq!(parse_hook_version("{}\n"), None);
        assert_eq!(snake_case("PreToolUse"), "pre_tool_use");
        assert_eq!(snake_case("Stop"), "stop");
    }

    /// Pod's group sits between another tool's groups (one installed after Pod):
    /// Pod's entries go, the later foreign entry follows its hook to index 1,
    /// other files and unrelated keys stay byte-identical.
    #[test]
    fn codex_trust_entries_pruned_and_rekeyed() {
        let hooks_path = Path::new(r"C:\Users\x\.codex\hooks.json");
        let hooks = json!({"hooks": {
            "Stop": [
                {"hooks": [{"command": "C:/o/codex-hook.cmd"}]},
                {"hooks": [{"command": "C:/p/pod-hook.exe codex Stop"}]},
                {"hooks": [{"command": "C:/later/tool.cmd"}]}
            ],
            "SessionStart": [
                {"hooks": [{"command": "C:/p/pod-hook.exe codex SessionStart"}]}
            ]
        }});
        let moves = codex_group_moves(&hooks);
        assert_eq!(moves[&("stop".to_string(), 0)], Some(0));
        assert_eq!(moves[&("stop".to_string(), 1)], None);
        assert_eq!(moves[&("stop".to_string(), 2)], Some(1));
        assert_eq!(moves[&("session_start".to_string(), 0)], None);

        let config = r#"# my settings
model = "o4"

[hooks.state.'C:\Users\x\.codex\hooks.json:stop:0:0']
trusted_hash = "sha256:other-tool"

[hooks.state.'C:\Users\x\.codex\hooks.json:stop:1:0']
trusted_hash = "sha256:pod-stop"

[hooks.state.'c:/users/x/.codex/hooks.json:stop:2:0']
trusted_hash = "sha256:later"

[hooks.state.'C:\Users\x\.codex\hooks.json:session_start:0:0']
trusted_hash = "sha256:pod-start"

[hooks.state.'D:\other\hooks.json:stop:1:0']
trusted_hash = "sha256:other-file"

[projects.'C:\work']
trust_level = "trusted"
"#;
        let (out, removed, moved) = prune_codex_trust(config, hooks_path, &moves)
            .unwrap()
            .expect("changes");
        assert_eq!((removed, moved), (2, 1), "{out}");
        assert!(out.starts_with("# my settings\nmodel = \"o4\"\n"), "{out}");
        assert!(
            !out.contains("pod-stop") && !out.contains("pod-start"),
            "{out}"
        );
        assert!(out.contains("sha256:other-tool") && out.contains("sha256:other-file"));
        let doc: toml_edit::DocumentMut = out.parse().unwrap();
        let state = doc["hooks"]["state"].as_table_like().unwrap();
        assert_eq!(
            state
                .get("c:/users/x/.codex/hooks.json:stop:1:0")
                .and_then(|t| t.get("trusted_hash"))
                .and_then(|v| v.as_str()),
            Some("sha256:later"),
            "{out}"
        );
        assert_eq!(
            doc["projects"]["C:\\work"]["trust_level"].as_str(),
            Some("trusted")
        );
        // A second uninstall sees hooks.json without Pod's groups: no change.
        let after = json!({"hooks": {"Stop": [
            {"hooks": [{"command": "C:/o/codex-hook.cmd"}]},
            {"hooks": [{"command": "C:/later/tool.cmd"}]}
        ]}});
        let moves2 = codex_group_moves(&after);
        assert!(
            prune_codex_trust(&out, hooks_path, &moves2)
                .unwrap()
                .is_none()
        );
        assert!(
            prune_codex_trust("model = \"x\"\n", hooks_path, &moves)
                .unwrap()
                .is_none()
        );
    }
}
