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
//! - TM-22: the hooks run an absolute, local `pod-hook.exe`
//!   (`install::check_hook_exe`), and a real install that would change
//!   anything (the binary or a config file) needs the user's confirmation
//!   of exactly that change: a y/N prompt in a console, or `--confirm
//!   <digest>` with the digest a `--dry-run` printed for the same plan. A
//!   plan that changed since (new binary, edited settings) has another
//!   digest and is refused, so it is confirmed again. Codex trust is
//!   written only for Pod's own commands in the hooks.json Pod wrote.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use pod_status::HookAgent;
use pod_status::install::{self, ApplyOptions, HookCommandSpec, InstallPlan, InstallScope};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// `p` in its long, on-disk spelling: absolute, the deepest existing
/// ancestor resolved with `canonicalize` (8.3 short names such as
/// `RUNNER~1` expanded, letter case as on disk) without the `\\?\` prefix,
/// the rest appended as given. Agent configs get paths in this spelling (it
/// keeps working where 8.3 names are off or stripped, and it is the one
/// Codex records in `[hooks.state]` keys).
pub fn long_path(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut base = abs.as_path();
    let mut tail = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(base) {
            let s = c.to_string_lossy();
            let mut out = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
                PathBuf::from(format!(r"\\{rest}"))
            } else {
                PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s))
            };
            out.extend(tail.iter().rev());
            return out;
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                base = parent;
            }
            _ => return abs,
        }
    }
}

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
    /// Install: the digest of exactly this plan (binary, hook path, every
    /// file's new content, Codex trust). `--confirm <it>` installs it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

/// The digest a user confirms: SHA-256 (first 16 hex digits) over the hook
/// executable path, the bytes of the binary that will be copied there, the
/// Codex trust choice and every planned file (path, unchanged or not, full
/// new content). Any change to what would be written changes it.
pub fn plan_digest(
    hook_exe: &Path,
    binary: Option<&[u8]>,
    codex_trust: bool,
    plan: &InstallPlan,
) -> String {
    let mut h = Sha256::new();
    h.update(b"pod-hooks-install 1\n");
    h.update(format!("exe {}\n", hook_exe.display()).as_bytes());
    match binary {
        Some(b) => h.update(format!("binary {:x}\n", Sha256::digest(b)).as_bytes()),
        None => h.update(b"binary missing\n"),
    }
    h.update(format!("codex-trust {codex_trust}\n").as_bytes());
    for f in &plan.files {
        h.update(
            format!(
                "file {} {} {} {}\n",
                f.agent.as_str(),
                f.path.display(),
                f.unchanged,
                f.content.len()
            )
            .as_bytes(),
        );
        h.update(f.content.as_bytes());
    }
    let hex = format!("{:x}", h.finalize());
    hex[..16].to_string()
}

/// What `pod hooks install` shows before asking (stderr, console only).
pub fn prompt_text(o: &Outcome, source: &Path) -> String {
    let mut s = String::from("pod hooks install will:\n");
    s.push_str(&format!("  run hooks from {}", o.hook_exe.display()));
    if o.binary_copied {
        s.push_str(&format!(" (copied from {})", source.display()));
    }
    s.push('\n');
    for f in &o.files {
        s.push_str(&format!(
            "  {}: {} {}\n",
            f.agent,
            f.action,
            f.path.display()
        ));
    }
    for n in &o.notes {
        s.push_str(&format!("  note: {n}\n"));
    }
    s.push_str(
        "Only Pod's hook entries change; other settings and other tools' hooks stay, \
         and every changed file is backed up first.\n",
    );
    s
}

/// `y` / `yes` (any case) confirms; anything else does not.
pub fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
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
    /// `--confirm <digest>`: the plan the user confirmed (see [`plan_digest`]).
    pub confirm: Option<String>,
}

fn codex_home(scope: &InstallScope) -> PathBuf {
    match scope {
        InstallScope::Global { home } => home.join(".codex"),
        InstallScope::Profile { codex_home, .. } => codex_home.clone(),
    }
}

/// Install (or, `req.dry_run`, plan) Pod's hooks. When the install would
/// change anything and `req.confirm` is not this plan's digest, `ask` gets
/// the preview (a dry-run outcome) and must say yes; `ask` returning an
/// error (no console) or `false` writes nothing.
pub fn install(
    req: &Request,
    ask: &mut dyn FnMut(&Outcome) -> anyhow::Result<bool>,
) -> anyhow::Result<Outcome> {
    let stamp = utc_stamp(SystemTime::now());
    let hook_exe = req.bin_dir.join("pod-hook.exe");
    let mut notes = Vec::new();
    if !req.dry_run && !req.source.is_file() {
        bail!(
            "{} not found (build pod-hook or pass --source)",
            req.source.display()
        );
    }
    let spec = HookCommandSpec::new(&hook_exe);
    let plan = install::plan_install(&req.scope, &spec, &req.agents)?;
    let binary = std::fs::read(&req.source).ok();
    let binary_changes = binary.is_some() && std::fs::read(&hook_exe).ok() != binary;
    let codex = req.agents.contains(&HookAgent::Codex);
    let digest = plan_digest(
        &hook_exe,
        binary.as_deref(),
        codex && req.codex_trust,
        &plan,
    );
    let preview = Outcome {
        dry_run: true,
        hook_exe: hook_exe.clone(),
        binary_copied: binary_changes,
        files: apply(&plan, &stamp, true)?,
        codex_trust: None,
        codex_untrust: None,
        notes: plan.notes.clone(),
        confirm: Some(digest.clone()),
    };
    if req.dry_run {
        let mut o = preview;
        o.binary_copied = false;
        if codex {
            o.notes.insert(0, if req.codex_trust {
                "would ask `codex app-server` to trust Pod's Codex hooks ([hooks.state] in config.toml)".into()
            } else {
                "--no-codex-trust: Codex will list Pod's hooks as untrusted until they are approved in Codex".into()
            });
        }
        return Ok(o);
    }
    let changes = binary_changes || plan.files.iter().any(|f| !f.unchanged);
    if changes {
        match req.confirm.as_deref() {
            Some(d) if d == digest => {}
            Some(_) => bail!(
                "the planned change differs from the one confirmed (the binary or a config file changed since); \
                 review it again with `pod hooks install --dry-run`; nothing was written"
            ),
            None => {
                if !ask(&preview)? {
                    bail!("not confirmed; nothing was written");
                }
            }
        }
    }
    let binary_copied = install_binary(&req.source, &req.bin_dir, &stamp)?.1;
    let files = apply(&plan, &stamp, false)?;
    let mut codex_trust = None;
    if codex {
        if req.codex_trust {
            let hooks_json = req.scope.config_path(HookAgent::Codex);
            let want: Vec<String> = pod_status::hook::CODEX_EVENTS
                .iter()
                .map(|ev| spec.command(HookAgent::Codex, ev))
                .collect();
            match trust_codex_hooks(&codex_home(&req.scope), &hooks_json, &want, &stamp) {
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
        dry_run: false,
        hook_exe,
        binary_copied,
        files,
        codex_trust,
        codex_untrust: None,
        notes,
        confirm: Some(digest),
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
        confirm: None,
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

/// The same file however it is spelled: letter case, separators, `\\?\`,
/// and (resolved through [`long_path`]) 8.3 short names.
fn same_file(a: &str, b: &Path) -> bool {
    let norm = |s: &str| {
        s.trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_lowercase()
    };
    let b_str = b.display().to_string();
    norm(a) == norm(&b_str)
        || norm(&long_path(Path::new(a)).display().to_string())
            == norm(&long_path(b).display().to_string())
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

/// Pod's exact hook command shape (another tool's command that merely
/// mentions `pod-hook` is not Pod's).
fn is_pod_command(c: &str) -> bool {
    install::is_pod_command(c)
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
    command: String,
    hash: String,
    trusted: bool,
}

impl PodHook {
    /// One of the commands Pod just wrote (`want`), in the hooks.json Pod
    /// wrote: only these get a trust entry. A Pod-shaped command elsewhere
    /// (another file, another executable) is left for the user to approve.
    fn is_ours(&self, hooks_json: &Path, want: &[String]) -> bool {
        // `<hooks.json path>:<event>:<group>:<hook>`; the path has colons too.
        let path = self.key.rsplitn(4, ':').nth(3).unwrap_or("");
        want.contains(&self.command) && same_file(path, hooks_json)
    }
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
                    command: cmd.to_string(),
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

/// Trust Pod's (and only Pod's) Codex hooks in `<codex_home>/config.toml`:
/// the commands in `want` in `hooks_json` (see [`PodHook::is_ours`]).
pub fn trust_codex_hooks(
    codex_home: &Path,
    hooks_json: &Path,
    want: &[String],
    stamp: &str,
) -> anyhow::Result<CodexTrust> {
    let mut s = CodexAppServer::start(codex_home)?;
    let ours = |s: &mut CodexAppServer| -> anyhow::Result<Vec<PodHook>> {
        Ok(s.pod_hooks()?
            .into_iter()
            .filter(|h| h.is_ours(hooks_json, want))
            .collect())
    };
    let hooks = ours(&mut s)?;
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
    out.untrusted = ours(&mut s)?.iter().filter(|h| !h.trusted).count();
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

    fn temp(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let d = std::env::temp_dir().join(format!(
            "pod-cli-hooks-{tag}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn req(t: &Path, dry_run: bool, confirm: Option<&str>) -> Request {
        Request {
            scope: InstallScope::Profile {
                claude_config_dir: t.join("claude"),
                codex_home: t.join("codex"),
            },
            agents: vec![HookAgent::Claude],
            bin_dir: t.join("bin"),
            source: t.join("src").join("pod-hook.exe"),
            dry_run,
            codex_trust: false,
            confirm: confirm.map(str::to_string),
        }
    }

    fn never(_: &Outcome) -> anyhow::Result<bool> {
        panic!("asked although nothing needed confirming")
    }

    /// TM-22: a change needs the user's yes (or the dry run's digest); a
    /// no, a stale digest or no console writes nothing; once installed, the
    /// same plan needs no confirmation; a new binary asks again.
    #[test]
    fn install_is_confirmed_and_asks_again_when_content_changes() {
        let t = temp("confirm");
        std::fs::create_dir_all(t.join("src")).unwrap();
        std::fs::write(t.join("src").join("pod-hook.exe"), b"build 1").unwrap();
        let settings = t.join("claude").join("settings.json");
        let written = || settings.exists() || t.join("bin").exists();

        let dry = install(&req(&t, true, None), &mut never).unwrap();
        let digest = dry.confirm.clone().expect("a dry run names its digest");
        assert_eq!(digest.len(), 16);
        assert!(!written(), "a dry run writes nothing");

        // No console: refused. Asked and declined: refused.
        let mut no_console = |_: &Outcome| -> anyhow::Result<bool> { bail!("no console") };
        assert!(install(&req(&t, false, None), &mut no_console).is_err());
        let mut asked = 0;
        let mut decline = |o: &Outcome| -> anyhow::Result<bool> {
            asked += 1;
            assert!(o.dry_run && o.files[0].action == "would create", "{o:?}");
            Ok(false)
        };
        let e = install(&req(&t, false, None), &mut decline).unwrap_err();
        assert!(e.to_string().contains("not confirmed"), "{e}");
        assert_eq!(asked, 1);
        assert!(!written());

        // A digest of another plan (the settings changed since the dry run).
        std::fs::create_dir_all(t.join("claude")).unwrap();
        std::fs::write(&settings, "{\"theme\": \"dark\"}\n").unwrap();
        let e = install(&req(&t, false, Some(&digest)), &mut never).unwrap_err();
        assert!(e.to_string().contains("differs"), "{e}");
        assert_eq!(
            std::fs::read_to_string(&settings).unwrap(),
            "{\"theme\": \"dark\"}\n"
        );
        assert!(!t.join("bin").exists());

        // The fresh dry run's digest installs it.
        let digest = install(&req(&t, true, None), &mut never)
            .unwrap()
            .confirm
            .unwrap();
        let o = install(&req(&t, false, Some(&digest)), &mut never).unwrap();
        assert!(o.binary_copied && o.files[0].action == "updated");
        assert_eq!(o.confirm.as_deref(), Some(digest.as_str()));
        let text = std::fs::read_to_string(&settings).unwrap();
        assert!(text.contains("\"theme\": \"dark\""), "{text}");
        let exe = t.join("bin").join("pod-hook.exe");
        assert!(text.contains(&exe.display().to_string().replace('\\', "/")));

        // Nothing to change: no confirmation needed.
        let o = install(&req(&t, false, None), &mut never).unwrap();
        assert!(!o.binary_copied && o.files[0].action == "unchanged");

        // A new pod-hook build changes the plan: asked again, yes installs.
        std::fs::write(t.join("src").join("pod-hook.exe"), b"build 2").unwrap();
        let mut asked = 0;
        let mut accept = |o: &Outcome| -> anyhow::Result<bool> {
            asked += 1;
            assert!(o.binary_copied, "the preview names the new binary");
            Ok(true)
        };
        let o = install(&req(&t, false, None), &mut accept).unwrap();
        assert_eq!(asked, 1);
        assert!(o.binary_copied);
        assert_eq!(std::fs::read(&exe).unwrap(), b"build 2");
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn digest_covers_path_binary_trust_and_content() {
        let t = temp("digest");
        let r = req(&t, true, None);
        let spec = HookCommandSpec::new(r.bin_dir.join("pod-hook.exe"));
        let plan = install::plan_install(&r.scope, &spec, &r.agents).unwrap();
        let exe = r.bin_dir.join("pod-hook.exe");
        let d = plan_digest(&exe, Some(b"x"), false, &plan);
        assert_eq!(d, plan_digest(&exe, Some(b"x"), false, &plan));
        assert_ne!(d, plan_digest(&exe, Some(b"y"), false, &plan));
        assert_ne!(d, plan_digest(&exe, None, false, &plan));
        assert_ne!(d, plan_digest(&exe, Some(b"x"), true, &plan));
        assert_ne!(
            d,
            plan_digest(
                &t.join("other").join("pod-hook.exe"),
                Some(b"x"),
                false,
                &plan
            )
        );
        let mut edited = plan.clone();
        edited.files[0].content.push(' ');
        assert_ne!(d, plan_digest(&exe, Some(b"x"), false, &edited));
        assert!(is_yes(" Y\r\n") && is_yes("yes") && !is_yes("") && !is_yes("n"));
        let _ = std::fs::remove_dir_all(&t);
    }

    /// Codex trust goes only to the commands Pod wrote, in the hooks.json
    /// Pod wrote; a Pod-shaped command in another file or with another
    /// executable is left for the user.
    #[test]
    fn codex_trust_only_for_pods_own_commands() {
        let hooks_json = Path::new(r"C:\Users\x\.codex\hooks.json");
        let want = vec!["C:/p/pod-hook.exe codex Stop".to_string()];
        let h = |key: &str, command: &str| PodHook {
            key: key.into(),
            command: command.into(),
            hash: "sha256:x".into(),
            trusted: false,
        };
        assert!(
            h(
                r"c:/users/x/.codex/hooks.json:stop:1:0",
                "C:/p/pod-hook.exe codex Stop"
            )
            .is_ours(hooks_json, &want)
        );
        for (key, command) in [
            // Another file (a project-level hooks.json).
            (
                r"D:\repo\.codex\hooks.json:stop:0:0",
                "C:/p/pod-hook.exe codex Stop",
            ),
            // Pod-shaped, another executable.
            (
                r"C:\Users\x\.codex\hooks.json:stop:2:0",
                "C:/evil/pod-hook.exe codex Stop",
            ),
            // A command Pod did not write.
            (
                r"C:\Users\x\.codex\hooks.json:stop:3:0",
                "C:/p/pod-hook.exe codex Stop && calc",
            ),
        ] {
            assert!(
                !h(key, command).is_ours(hooks_json, &want),
                "{key} {command}"
            );
        }
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

    /// The 8.3 short spelling of an existing path, `None` when the volume
    /// has no short names.
    fn short_spelling(p: &Path) -> Option<PathBuf> {
        use std::os::windows::ffi::{OsStrExt, OsStringExt};
        use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
        let wide: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
        let mut buf = vec![0u16; 1024];
        // SAFETY: `wide` is NUL-terminated; `buf` is as long as the size passed.
        let n = unsafe { GetShortPathNameW(wide.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        let n = n as usize;
        if n == 0 || n >= buf.len() {
            return None;
        }
        let s = PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]));
        s.to_string_lossy().contains('~').then_some(s)
    }

    /// Codex keys `[hooks.state]` by the long path of hooks.json; Pod may
    /// name the same file through an 8.3 short name (`RUNNER~1`): its
    /// entries are still found and removed, and [`long_path`] gives the
    /// spelling agent configs get.
    #[test]
    fn trust_entries_match_short_spelled_hooks_json() {
        let t = std::env::temp_dir().join(format!("pod-cli-short-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        let codex = t.join("CodexHomeFolder");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("hooks.json"), "{}").unwrap();
        let long = long_path(&codex.join("hooks.json"));
        let Some(short) = short_spelling(&long) else {
            eprintln!("skipped: no 8.3 short names on this volume");
            let _ = std::fs::remove_dir_all(&t);
            return;
        };
        assert_eq!(long_path(&short), long);
        assert_eq!(
            long_path(&short.with_file_name("bin").join("pod-hook.exe")),
            long.with_file_name("bin").join("pod-hook.exe"),
            "a missing tail keeps its name under the long parent"
        );
        assert!(same_file(&long.display().to_string(), &short));
        let config = format!(
            "[hooks.state.'{}:stop:0:0']\ntrusted_hash = \"sha256:pod\"\n",
            long.display()
        );
        let moves = std::collections::HashMap::from([(("stop".to_string(), 0), None)]);
        let (_, removed, moved) = prune_codex_trust(&config, &short, &moves)
            .unwrap()
            .expect("the short spelling names the same hooks.json");
        assert_eq!((removed, moved), (1, 0));
        let _ = std::fs::remove_dir_all(&t);
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
