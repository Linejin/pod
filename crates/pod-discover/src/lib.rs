//! Session discovery (docs/18): find the coding-agent sessions that already
//! exist on this PC so Pod can list, link and resume them.
//!
//! - [`Source`]: one agent's local session store (v1: [`claude`], [`codex`]).
//!   Sources only **read**: they never modify, move or delete the agent's
//!   files and never touch its configuration.
//! - [`scan`](fn@scan): lists each source's session files (newest first,
//!   bounded), re-parses only files whose size or mtime changed ([`Cache`])
//!   and stays inside a time and byte [`Budget`].
//! - [`procs`]: running agent processes (Windows process snapshot) and the
//!   de-duplication against Pod's own terminals.
//! - [`matching`]: session cwd -> registered repository / task worktree.
//! - [`resume`]: the agent's own resume command for a session id.
//!
//! Parsers are defensive: unknown, missing or garbled lines are skipped and a
//! file that yields nothing useful is left out. Nothing here panics on input.

pub mod claude;
pub mod codex;
pub mod jsonl;
pub mod matching;
pub mod procs;
pub mod resume;
mod scan;
pub mod time;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use procs::{ProcInfo, RunningAgent};
pub use scan::{ScanOutput, scan};

/// The agents Pod can discover sessions of.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Agent {
    Claude,
    Codex,
}

impl Agent {
    pub const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];

    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// Display name (product name).
    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    pub fn parse(s: &str) -> Option<Agent> {
        match s.to_ascii_lowercase().as_str() {
            "claude" => Some(Agent::Claude),
            "codex" => Some(Agent::Codex),
            _ => None,
        }
    }
}

/// `"<agent>:<id>"`, the key Pod stores links and hidden flags under.
pub fn session_key(agent: Agent, id: &str) -> String {
    format!("{}:{id}", agent.as_str())
}

/// Metadata of one past session, read from the agent's store.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionMeta {
    pub id: String,
    pub cwd: String,
    /// Title the agent gave the session (or the user's rename), truncated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// First user prompt, one line, truncated. `None` when previews are off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Agent version that wrote the session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// How the session was started when the store records it (Codex
    /// `source`: `cli`, `exec`, `vscode`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ms: Option<u64>,
    /// Approximate user + assistant message count; `None` for files over
    /// [`Limits::count_max_bytes`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messages: Option<u64>,
}

/// A session file as listed (no content read yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileStamp {
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: u64,
}

impl FileStamp {
    pub fn of(path: PathBuf) -> Option<FileStamp> {
        let md = std::fs::metadata(&path).ok()?;
        if !md.is_file() {
            return None;
        }
        let mtime_ms = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        Some(FileStamp {
            path,
            size: md.len(),
            mtime_ms,
        })
    }
}

/// Scan bounds (docs/18 §5).
#[derive(Debug, Clone)]
pub struct Limits {
    /// Newest files per source.
    pub max_files: usize,
    /// Directory entries a source may visit while listing.
    pub max_listed: usize,
    pub head_bytes: u64,
    pub head_lines: usize,
    pub tail_bytes: u64,
    /// Files larger than this get no message count.
    pub count_max_bytes: u64,
    /// Whole-scan wall clock budget.
    pub time: Duration,
    /// Whole-scan read budget in bytes.
    pub read_bytes: u64,
    /// Extract the first prompt.
    pub preview: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_files: 2000,
            max_listed: 50_000,
            head_bytes: 256 * 1024,
            head_lines: 400,
            tail_bytes: 128 * 1024,
            count_max_bytes: 16 * 1024 * 1024,
            time: Duration::from_secs(10),
            read_bytes: 128 * 1024 * 1024,
            preview: true,
        }
    }
}

/// What a scan may still spend.
#[derive(Debug)]
pub struct Budget {
    deadline: Instant,
    bytes_left: u64,
    /// Set once something was skipped because the budget ran out.
    pub exhausted: bool,
    pub bytes_read: u64,
}

impl Budget {
    pub fn new(limits: &Limits) -> Budget {
        Budget {
            deadline: Instant::now() + limits.time,
            bytes_left: limits.read_bytes,
            exhausted: false,
            bytes_read: 0,
        }
    }

    /// May the scan read `n` more bytes? Marks the budget exhausted if not.
    pub fn take(&mut self, n: u64) -> bool {
        if Instant::now() >= self.deadline || n > self.bytes_left {
            self.exhausted = true;
            return false;
        }
        self.bytes_left -= n;
        self.bytes_read += n;
        true
    }

    pub fn out_of_time(&mut self) -> bool {
        if Instant::now() >= self.deadline {
            self.exhausted = true;
        }
        self.exhausted
    }
}

/// One agent's local session store.
pub trait Source: Send + Sync {
    fn agent(&self) -> Agent;
    /// Store root (for messages and tests).
    fn root(&self) -> &Path;
    /// Session files, unordered; at most `limits.max_listed` entries visited.
    fn list(&self, limits: &Limits) -> Vec<FileStamp>;
    /// Read once per scan before parsing (e.g. a title index).
    fn prepare(&mut self, _budget: &mut Budget) {}
    /// Metadata of one session file, or `None` when it is not a usable session.
    fn parse(&self, file: &FileStamp, limits: &Limits, budget: &mut Budget) -> Option<SessionMeta>;
    /// Byte strings marking a message line (counted in phase 2 of [`scan`](fn@scan)).
    fn message_markers(&self) -> &'static [&'static [u8]];
    /// Session ids of running processes this store records (`pid -> id`).
    fn running_sessions(&self, _budget: &mut Budget) -> HashMap<u32, String> {
        HashMap::new()
    }
}

/// The sources for the current user: `POD_DISCOVER_CLAUDE_DIR` /
/// `POD_DISCOVER_CODEX_DIR` (tests), else the agents' own `CLAUDE_CONFIG_DIR`
/// / `CODEX_HOME`, else `%USERPROFILE%\.claude` / `\.codex`.
pub fn default_sources() -> Vec<Box<dyn Source>> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_default();
    let dir = |pod: &str, agent: &str, default: &str| {
        std::env::var_os(pod)
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var_os(agent).filter(|v| !v.is_empty()))
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(default))
    };
    vec![
        Box::new(claude::ClaudeSource::new(dir(
            "POD_DISCOVER_CLAUDE_DIR",
            "CLAUDE_CONFIG_DIR",
            ".claude",
        ))),
        Box::new(codex::CodexSource::new(dir(
            "POD_DISCOVER_CODEX_DIR",
            "CODEX_HOME",
            ".codex",
        ))),
    ]
}

/// A parsed file remembered between scans.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedFile {
    pub agent: Agent,
    pub size: u64,
    pub mtime_ms: u64,
    /// Was the preview extracted (a cache made with previews off is re-read
    /// when they are turned on)?
    #[serde(default)]
    pub with_preview: bool,
    /// Was the message count attempted (it is `None` for large files)?
    #[serde(default)]
    pub counted: bool,
    /// `None`: the file is not a usable session (remembered so it is not re-read).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<SessionMeta>,
}

/// Parsed files by path.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cache {
    #[serde(default)]
    pub files: HashMap<String, CachedFile>,
}

impl Cache {
    /// Drop every stored preview (previews were turned off).
    pub fn strip_previews(&mut self) {
        for f in self.files.values_mut() {
            f.with_preview = false;
            if let Some(m) = f.meta.as_mut() {
                m.preview = None;
            }
        }
    }
}

/// A session found by [`scan`](fn@scan).
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub agent: Agent,
    pub meta: SessionMeta,
    pub path: PathBuf,
    pub mtime_ms: u64,
}

/// Last activity of a found session (recorded time, else file mtime).
pub fn last_activity(f: &Found) -> u64 {
    f.meta.last_ms.unwrap_or(f.mtime_ms)
}

/// Characters kept in titles and previews.
pub const SUMMARY_CHARS: usize = 120;

/// One line, control characters removed, whitespace collapsed, at most
/// `max` characters (with `…`). Empty input gives `None`.
pub fn summarize(s: &str, max: usize) -> Option<String> {
    // Bounded work for huge inputs.
    let head: String = s.chars().take(max.saturating_mul(16).max(64)).collect();
    let norm = head
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if norm.is_empty() {
        return None;
    }
    if norm.chars().count() <= max {
        return Some(norm);
    }
    let mut out = norm
        .chars()
        .take(max)
        .collect::<String>()
        .trim_end()
        .to_string();
    out.push('…');
    Some(out)
}

/// Is `s` a UUID (8-4-4-4-12 hex digits)? Session ids of both agents are.
pub fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// Does a prompt look like an agent-injected wrapper rather than something
/// the user typed (`<command-name>…`, `<environment_context>…`)?
pub(crate) fn is_wrapper_prompt(s: &str) -> bool {
    let t = s.trim_start();
    t.starts_with('<') || t.starts_with("[Request interrupted") || t.starts_with("Caveat:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarize_collapses_and_truncates() {
        assert_eq!(summarize("  a\n\tb  c ", 50).as_deref(), Some("a b c"));
        assert_eq!(summarize("\x1b[31m", 50).as_deref(), Some("[31m"));
        assert_eq!(summarize("", 5), None);
        assert_eq!(summarize(" \n ", 5), None);
        assert_eq!(summarize("abcdefgh", 5).as_deref(), Some("abcde…"));
        assert_eq!(summarize("abcde", 5).as_deref(), Some("abcde"));
        assert_eq!(
            summarize("한국어 문장입니다", 4).as_deref(),
            Some("한국어…")
        );
    }

    #[test]
    fn uuid_check() {
        assert!(is_uuid("0f8b6a3e-1111-4222-8333-944445555666"));
        assert!(!is_uuid("0f8b6a3e-1111-4222-8333-94444555566"));
        assert!(!is_uuid("0f8b6a3e-1111-4222-8333-94444555566g"));
        assert!(!is_uuid("x; rm -rf"));
    }

    #[test]
    fn agent_names() {
        for a in Agent::ALL {
            assert_eq!(Agent::parse(a.as_str()), Some(a));
        }
        assert_eq!(session_key(Agent::Codex, "x"), "codex:x");
    }
}
