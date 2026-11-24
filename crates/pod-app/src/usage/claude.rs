//! Claude Code usage limits from its documented status-line samples
//! (<https://code.claude.com/docs/en/statusline#rate-limit-usage>).
//!
//! Pod never asks Claude for anything: a running Claude Code session passes
//! its status-line JSON to its `statusLine` command after each update. When
//! the user connects `pod-hook statusline` there (the details' "설정 복사"
//! gives the exact value), pod-hook keeps only the `rate_limits` numbers in
//! Pod's cache (`pod_status::usage`), and this module reads that cache.
//!
//! - Read-only: Pod never edits Claude's settings. It reads the user
//!   `settings.json` (bounded) only to see whether `statusLine.command`
//!   already runs Pod's collector, and, when another command is set, to
//!   compose with it (`pod-hook statusline --passthrough | <command>`)
//!   instead of replacing it.
//! - Disconnect: when the collector is gone from the settings and no sample
//!   arrived recently, the old numbers are hidden, not shown as current.

use std::path::{Path, PathBuf};

use pod_status::usage::{ClaudeUsageRecord, LimitSample};
use serde_json::Value;

use super::model::{Bucket, Provider, QuotaWindow, ServiceId, Snapshot, SourceKind, WindowKind};

/// A settings file larger than this is not read.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;
/// A record seen this recently counts as connected even when the user
/// settings do not show the collector (project settings may set it).
pub const RECENT_SAMPLE_MS: u64 = 15 * 60_000;

/// What `statusLine.command` in the user settings is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Collector {
    /// Pod's collector (alone or composed).
    Pod,
    /// Another command; Pod composes with it.
    Other(String),
    /// No status line.
    None,
    /// The settings could not be read or parsed.
    Unreadable,
}

impl Collector {
    pub fn connected(&self) -> bool {
        matches!(self, Self::Pod)
    }
}

fn is_pod_collector(cmd: &str) -> bool {
    let c = cmd.to_ascii_lowercase();
    c.contains("pod-hook") && c.contains("statusline")
}

/// Classify the text of a Claude `settings.json`.
pub fn collector_from_settings(text: Option<&str>) -> Collector {
    let Some(text) = text else {
        return Collector::None;
    };
    let Ok(v) = serde_json::from_str::<Value>(text) else {
        return Collector::Unreadable;
    };
    let cmd = v
        .get("statusLine")
        .filter(|s| s.get("type").and_then(Value::as_str).unwrap_or("command") == "command")
        .and_then(|s| s.get("command"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|c| !c.is_empty());
    match cmd {
        None => Collector::None,
        Some(c) if is_pod_collector(c) => Collector::Pod,
        Some(c) => Collector::Other(c.to_string()),
    }
}

/// The user settings Claude Code reads (`CLAUDE_CONFIG_DIR` or `~/.claude`).
pub fn settings_path() -> Option<PathBuf> {
    let dir = std::env::var_os(pod_status::usage::ENV_CLAUDE_CONFIG_DIR)
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .filter(|h| !h.is_empty())
                .map(|h| PathBuf::from(h).join(".claude"))
        })?;
    Some(dir.join("settings.json"))
}

/// Bounded read of a settings file (`None`: missing or too large).
pub fn read_settings(path: &Path) -> Result<Option<String>, ()> {
    use std::io::Read;
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(()),
    };
    let mut buf = Vec::new();
    f.take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|_| ())?;
    if buf.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(());
    }
    String::from_utf8(buf).map(Some).map_err(|_| ())
}

/// `pod-hook.exe` for the status line: the per-user copy `pod hooks install`
/// keeps (stable across app updates), else the one beside this app.
pub fn hook_exe(local_app_data: Option<&Path>, app_dir: Option<&Path>) -> Option<PathBuf> {
    let installed = local_app_data.map(|d| d.join("Pod").join("bin").join("pod-hook.exe"));
    let beside = app_dir.map(|d| d.join("pod-hook.exe"));
    installed
        .filter(|p| p.is_file())
        .or_else(|| beside.filter(|p| p.is_file()))
}

/// The path as Claude Code's shell needs it: forward slashes (Git Bash eats
/// backslashes), quoted only when it has spaces.
fn shell_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.contains(' ') {
        format!("\"{s}\"")
    } else {
        s
    }
}

/// The `statusLine.command` value that connects Pod, keeping `existing`.
pub fn setup_command(exe: &Path, existing: Option<&str>) -> String {
    let exe = shell_path(exe);
    match existing.map(str::trim).filter(|c| !c.is_empty()) {
        Some(c) if is_pod_collector(c) => c.to_string(),
        Some(c) => format!("{exe} statusline --passthrough | {c}"),
        None => format!("{exe} statusline"),
    }
}

/// The settings snippet to paste: `{"statusLine": {...}}`.
pub fn setup_json(command: &str) -> String {
    let v = serde_json::json!({
        "statusLine": { "type": "command", "command": command }
    });
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// What one read of the Claude side found.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeRead {
    pub records: Vec<ClaudeUsageRecord>,
    pub collector: Collector,
    /// The snippet "설정 복사" copies (`None`: no `pod-hook.exe` found).
    pub setup: Option<String>,
}

fn window(kind: WindowKind, s: &LimitSample) -> QuotaWindow {
    QuotaWindow {
        kind,
        used_percent: s.used_percentage,
        resets_at: s.resets_at,
    }
}

/// One record as a snapshot (`None`: it never carried limits).
pub fn snapshot(rec: &ClaudeUsageRecord) -> Option<Snapshot> {
    let at = rec.limits_at_ms?;
    let mut windows = Vec::new();
    if let Some(s) = &rec.five_hour {
        windows.push(window(WindowKind::FiveHour, s));
    }
    if let Some(s) = &rec.seven_day {
        windows.push(window(WindowKind::Weekly, s));
    }
    if let Some(s) = &rec.spend_limit {
        windows.push(window(WindowKind::Spend(s.period.clone()), s));
    }
    Some(Snapshot {
        service: ServiceId::Claude,
        source: SourceKind::ClaudeStatusLine,
        profile: rec.profile_label.clone(),
        observed_ms: at,
        plan: None,
        buckets: vec![Bucket {
            name: None,
            windows,
            reached: false,
        }],
    })
}

/// The Claude providers to show: one per profile with data, or one state.
pub fn providers(read: &ClaudeRead, now_ms: u64) -> Vec<Provider> {
    let recent = |r: &ClaudeUsageRecord| now_ms.saturating_sub(r.seen_at_ms) < RECENT_SAMPLE_MS;
    let connected = read.collector.connected() || read.records.iter().any(recent);
    if !connected {
        // Disconnected: earlier numbers are not shown as current.
        return vec![Provider::NotConnected];
    }
    if read.records.is_empty() {
        return vec![Provider::AwaitingFirst];
    }
    let mut out: Vec<Provider> = read
        .records
        .iter()
        .filter_map(snapshot)
        .map(Provider::Ready)
        .collect();
    if out.is_empty() {
        out.push(Provider::NoLimits);
    }
    out
}

/// The installed Claude side: Pod's cache and the user settings (read-only).
pub struct RealClaude {
    pub usage_dir: PathBuf,
}

impl super::ClaudeProbe for RealClaude {
    fn read(&self) -> ClaudeRead {
        let records = pod_status::usage::read_records(&self.usage_dir);
        let collector = match settings_path().map(|p| read_settings(&p)) {
            Some(Ok(text)) => collector_from_settings(text.as_deref()),
            Some(Err(())) => Collector::Unreadable,
            None => Collector::None,
        };
        let exe = hook_exe(
            std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .as_deref(),
            std::env::current_exe()
                .ok()
                .as_deref()
                .and_then(Path::parent),
        );
        let existing = match &collector {
            Collector::Other(c) => Some(c.as_str()),
            _ => None,
        };
        let setup = exe.map(|e| setup_json(&setup_command(&e, existing)));
        ClaudeRead {
            records,
            collector,
            setup,
        }
    }

    fn forget(&self, profile: &str) -> Result<(), String> {
        pod_status::usage::remove_record(&self.usage_dir, profile).map_err(|e| e.to_string())
    }
}
