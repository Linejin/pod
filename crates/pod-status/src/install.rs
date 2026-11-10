//! Hook installer: builds the Claude Code `settings.json` hooks block and the
//! Codex `hooks.json` entries that run `pod-hook`, merges them into existing
//! files without touching other entries, and plans/applies writes.
//!
//! Safety: [`apply_plan`] refuses to write the real user configuration
//! (`%USERPROFILE%\.claude`, `%USERPROFILE%\.codex`) unless the caller passes
//! `ApplyOptions { allow_user_config: true }`. The `pod-hook install` CLI only
//! supports `--dry-run`.
//!
//! Env guard: every entry runs `pod-hook`, which prints `{}` and exits 0 at once
//! when `POD_TERMINAL_ID`/`POD_SOCKET` are absent, so hooks installed globally
//! are no-ops outside Pod (global hooks, routed by environment variables).

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::hook::{CLAUDE_EVENTS, CLAUDE_MATCHER_EVENTS, CODEX_EVENTS, HookAgent};

/// Substring that identifies a Pod hook entry (used for idempotent merge/removal).
pub const POD_HOOK_MARKER: &str = "pod-hook";

/// Where hooks go. Which one Pod uses is an open user decision (docs/04 §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum InstallScope {
    /// `<home>/.claude/settings.json` and `<home>/.codex/hooks.json`.
    Global { home: PathBuf },
    /// Separate profile directories, used by launching agents with
    /// `CLAUDE_CONFIG_DIR=<claude_config_dir>` / `CODEX_HOME=<codex_home>`.
    Profile {
        claude_config_dir: PathBuf,
        codex_home: PathBuf,
    },
}

impl InstallScope {
    pub fn config_path(&self, agent: HookAgent) -> PathBuf {
        match (self, agent) {
            (InstallScope::Global { home }, HookAgent::Claude) => {
                home.join(".claude").join("settings.json")
            }
            (InstallScope::Global { home }, HookAgent::Codex) => {
                home.join(".codex").join("hooks.json")
            }
            (
                InstallScope::Profile {
                    claude_config_dir, ..
                },
                HookAgent::Claude,
            ) => claude_config_dir.join("settings.json"),
            (InstallScope::Profile { codex_home, .. }, HookAgent::Codex) => {
                codex_home.join("hooks.json")
            }
        }
    }

    /// Env vars podd must inject when launching agents for this scope.
    pub fn launch_env(&self) -> Vec<(String, String)> {
        match self {
            InstallScope::Global { .. } => Vec::new(),
            InstallScope::Profile {
                claude_config_dir,
                codex_home,
            } => vec![
                (
                    "CLAUDE_CONFIG_DIR".into(),
                    claude_config_dir.display().to_string(),
                ),
                ("CODEX_HOME".into(), codex_home.display().to_string()),
            ],
        }
    }
}

/// How the hook command is spelled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HookCommandSpec {
    /// Absolute path of `pod-hook(.exe)`.
    pub exe: PathBuf,
    /// Per-hook timeout in seconds written into the config.
    pub timeout_secs: u32,
}

impl HookCommandSpec {
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            timeout_secs: 10,
        }
    }

    /// `"C:/path/pod-hook.exe" claude PreToolUse` (forward slashes, quoted if needed).
    pub fn command(&self, agent: HookAgent, event: &str) -> String {
        let p = self.exe.display().to_string().replace('\\', "/");
        let p = if p.contains(' ') {
            format!("\"{p}\"")
        } else {
            p
        };
        format!("{p} {} {event}", agent.as_str())
    }
}

fn entry(
    spec: &HookCommandSpec,
    agent: HookAgent,
    event: &str,
    matcher: Option<&str>,
    timeout: u32,
) -> Value {
    let mut m = Map::new();
    if let Some(mt) = matcher {
        m.insert("matcher".into(), json!(mt));
    }
    m.insert(
        "hooks".into(),
        json!([{ "type": "command", "command": spec.command(agent, event), "timeout": timeout }]),
    );
    Value::Object(m)
}

/// The `{"hooks": {...}}` object for Claude Code settings.json.
pub fn claude_hooks_block(spec: &HookCommandSpec) -> Value {
    let mut hooks = Map::new();
    for ev in CLAUDE_EVENTS {
        let matcher = CLAUDE_MATCHER_EVENTS.contains(ev).then_some("*");
        hooks.insert(
            (*ev).into(),
            json!([entry(
                spec,
                HookAgent::Claude,
                ev,
                matcher,
                spec.timeout_secs
            )]),
        );
    }
    json!({ "hooks": hooks })
}

/// The `{"hooks": {...}}` object for Codex hooks.json.
pub fn codex_hooks_block(spec: &HookCommandSpec) -> Value {
    let mut hooks = Map::new();
    for ev in CODEX_EVENTS {
        // Interrupt must not delay the user's Ctrl-C/Esc (3s budget).
        let timeout = if *ev == "Interrupt" {
            spec.timeout_secs.min(3)
        } else {
            spec.timeout_secs
        };
        hooks.insert(
            (*ev).into(),
            json!([entry(spec, HookAgent::Codex, ev, None, timeout)]),
        );
    }
    json!({ "hooks": hooks })
}

pub fn hooks_block(agent: HookAgent, spec: &HookCommandSpec) -> Value {
    match agent {
        HookAgent::Claude => claude_hooks_block(spec),
        HookAgent::Codex => codex_hooks_block(spec),
    }
}

fn is_pod_group(group: &Value) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|hs| {
            !hs.is_empty()
                && hs.iter().all(|h| {
                    h.get("command")
                        .and_then(Value::as_str)
                        .is_some_and(|c| c.contains(POD_HOOK_MARKER))
                })
        })
}

/// Remove every Pod hook group from a config object (other entries untouched;
/// events left empty are removed).
pub fn remove_pod_hooks(mut config: Value) -> Value {
    if let Some(hooks) = config.get_mut("hooks").and_then(Value::as_object_mut) {
        for groups in hooks.values_mut() {
            if let Some(arr) = groups.as_array_mut() {
                arr.retain(|g| !is_pod_group(g));
            }
        }
        hooks.retain(|_, g| g.as_array().is_none_or(|a| !a.is_empty()));
    }
    config
}

/// Merge a hooks block into an existing config (idempotent: earlier Pod
/// entries are replaced, foreign entries from other tools are kept).
pub fn merge_hooks(existing: Value, block: &Value) -> Value {
    let mut config = remove_pod_hooks(existing);
    if !config.is_object() {
        config = json!({});
    }
    let obj = config.as_object_mut().expect("object");
    let hooks = obj.entry("hooks").or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let hooks = hooks.as_object_mut().expect("object");
    if let Some(new) = block.get("hooks").and_then(Value::as_object) {
        for (ev, groups) in new {
            let slot = hooks.entry(ev.clone()).or_insert_with(|| json!([]));
            if !slot.is_array() {
                *slot = json!([]);
            }
            if let (Some(dst), Some(src)) = (slot.as_array_mut(), groups.as_array()) {
                dst.extend(src.iter().cloned());
            }
        }
    }
    config
}

/// One file the installer would write.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedFile {
    pub agent: HookAgent,
    pub path: PathBuf,
    pub existed: bool,
    /// Full new file content (pretty JSON + newline).
    pub content: String,
    /// True when `content` equals the current file.
    pub unchanged: bool,
}

/// Install plan for a scope.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InstallPlan {
    pub scope: InstallScope,
    pub files: Vec<PlannedFile>,
    /// Env vars podd must set on agent PTYs (profile scope), besides POD_*.
    pub launch_env: Vec<(String, String)>,
    pub notes: Vec<String>,
}

fn read_json(path: &Path) -> io::Result<(bool, Option<String>, Value)> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let v = if text.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&text).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: {e}", path.display()),
                    )
                })?
            };
            Ok((true, Some(text), v))
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok((false, None, json!({}))),
        Err(e) => Err(e),
    }
}

fn pretty(v: &Value) -> String {
    let mut s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into());
    s.push('\n');
    s
}

/// Build an install (or uninstall, `remove = true`) plan. Only reads files.
pub fn plan(
    scope: &InstallScope,
    spec: &HookCommandSpec,
    agents: &[HookAgent],
    remove: bool,
) -> io::Result<InstallPlan> {
    let mut files = Vec::new();
    for &agent in agents {
        let path = scope.config_path(agent);
        let (existed, before, current) = read_json(&path)?;
        let next = if remove {
            remove_pod_hooks(current)
        } else {
            merge_hooks(current, &hooks_block(agent, spec))
        };
        let content = pretty(&next);
        let unchanged = before.as_deref() == Some(content.as_str());
        files.push(PlannedFile {
            agent,
            path,
            existed,
            content,
            unchanged,
        });
    }
    let mut notes = vec![
        "pod-hook prints {} and exits 0 when POD_TERMINAL_ID/POD_SOCKET are unset, so these hooks are no-ops outside Pod.".to_string(),
    ];
    if agents.contains(&HookAgent::Codex) {
        notes.push(
            "Codex may ask to trust new hooks (config.toml [hooks.state] entries); the installer does not edit config.toml.".into(),
        );
    }
    Ok(InstallPlan {
        scope: scope.clone(),
        files,
        launch_env: scope.launch_env(),
        notes,
    })
}

/// Install plan (merge Pod hooks).
pub fn plan_install(
    scope: &InstallScope,
    spec: &HookCommandSpec,
    agents: &[HookAgent],
) -> io::Result<InstallPlan> {
    plan(scope, spec, agents, false)
}

/// Uninstall plan (remove Pod hooks, keep everything else).
pub fn plan_uninstall(
    scope: &InstallScope,
    spec: &HookCommandSpec,
    agents: &[HookAgent],
) -> io::Result<InstallPlan> {
    plan(scope, spec, agents, true)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ApplyOptions {
    /// Allow writing under the real `%USERPROFILE%\.claude` / `.codex`.
    pub allow_user_config: bool,
}

/// The real user's home, from `USERPROFILE` (Windows) or `HOME`.
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn norm(p: &Path) -> String {
    let s = p
        .display()
        .to_string()
        .replace('\\', "/")
        .to_ascii_lowercase();
    s.trim_end_matches('/').to_string()
}

/// True if `path` lies inside the real user's `.claude` or `.codex` directory.
pub fn is_user_agent_config(path: &Path) -> bool {
    let Some(home) = user_home() else {
        return false;
    };
    let p = norm(path);
    [".claude", ".codex"].iter().any(|d| {
        let base = norm(&home.join(d));
        p == base || p.starts_with(&format!("{base}/"))
    })
}

/// Write a plan's files (creating parent dirs). Refuses user config unless allowed.
pub fn apply_plan(plan: &InstallPlan, opts: ApplyOptions) -> io::Result<()> {
    if !opts.allow_user_config
        && let Some(f) = plan.files.iter().find(|f| is_user_agent_config(&f.path))
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("refusing to write user agent config {}", f.path.display()),
        ));
    }
    for f in &plan.files {
        if f.unchanged {
            continue;
        }
        if let Some(dir) = f.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&f.path, &f.content)?;
    }
    Ok(())
}
