//! Codex CLI's local session store (observed format, docs/18 §2):
//!
//! - `<root>\sessions\YYYY\MM\DD\rollout-<time>-<uuid>.jsonl`: the first
//!   line is `{type: "session_meta", payload: {id, cwd, timestamp,
//!   cli_version, source, originator, git: {branch}}}`; later lines are
//!   `turn_context` (`payload.model`), `event_msg` (`user_message` with
//!   `payload.message`, or `item_completed` with a `UserMessage` item) and
//!   `response_item` lines.
//! - `<root>\session_index.jsonl`: `{id, thread_name, updated_at}` per line
//!   (later lines win).
//!
//! Archived sessions (`archived_sessions`) are left out: the user put them away.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::jsonl::{self, object, str_at};
use crate::time::parse_rfc3339_ms;
use crate::{
    Agent, Budget, FileStamp, Limits, SUMMARY_CHARS, SessionMeta, Source, is_uuid,
    is_wrapper_prompt, summarize,
};

pub const MESSAGE_MARKERS: &[&[u8]] = &[
    b"\"type\":\"user_message\"",
    b"\"type\":\"agent_message\"",
    b"\"type\":\"UserMessage\"",
    b"\"type\":\"AgentMessage\"",
    b"\"type\": \"user_message\"",
    b"\"type\": \"agent_message\"",
    b"\"type\": \"UserMessage\"",
    b"\"type\": \"AgentMessage\"",
];
/// Largest title index read (its newest part).
const MAX_INDEX_BYTES: u64 = 4 * 1024 * 1024;

pub struct CodexSource {
    root: PathBuf,
    titles: HashMap<String, String>,
}

impl CodexSource {
    pub fn new(root: PathBuf) -> Self {
        CodexSource {
            root,
            titles: HashMap::new(),
        }
    }
}

impl Source for CodexSource {
    fn agent(&self) -> Agent {
        Agent::Codex
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn list(&self, limits: &Limits) -> Vec<FileStamp> {
        let mut out = Vec::new();
        let mut visited = 0usize;
        walk(
            &self.root.join("sessions"),
            0,
            &mut visited,
            limits.max_listed,
            &mut out,
        );
        out
    }

    fn prepare(&mut self, budget: &mut Budget) {
        self.titles.clear();
        let Some(stamp) = FileStamp::of(self.root.join("session_index.jsonl")) else {
            return;
        };
        let Some(lines) = jsonl::tail(&stamp.path, stamp.size, MAX_INDEX_BYTES, budget) else {
            return;
        };
        for line in lines {
            let Some(v) = object(&line) else { continue };
            if let (Some(id), Some(name)) = (str_at(&v, "/id"), str_at(&v, "/thread_name")) {
                self.titles
                    .insert(id.to_ascii_lowercase(), name.to_string());
            }
        }
    }

    fn parse(&self, file: &FileStamp, limits: &Limits, budget: &mut Budget) -> Option<SessionMeta> {
        let head = jsonl::head(
            &file.path,
            file.size,
            limits.head_bytes,
            limits.head_lines,
            budget,
        )?;
        let mut acc = Acc::default();
        for line in &head {
            if let Some(v) = object(line) {
                acc.line(&v, true, limits.preview);
            }
        }
        if acc.subagent {
            return None;
        }
        if file.size > limits.head_bytes
            && let Some(tail) = jsonl::tail(&file.path, file.size, limits.tail_bytes, budget)
        {
            for line in &tail {
                if let Some(v) = object(line) {
                    acc.line(&v, false, false);
                }
            }
        }
        let id = acc.id.or_else(|| id_from_file_name(&file.path))?;
        let cwd = acc.cwd?;
        Some(SessionMeta {
            title: self
                .titles
                .get(&id)
                .and_then(|t| summarize(t, SUMMARY_CHARS)),
            id,
            cwd,
            preview: if limits.preview { acc.preview } else { None },
            model: acc.model,
            branch: acc.branch,
            version: acc.version,
            origin: acc.source,
            started_ms: acc.started_ms.or(acc.first_ms),
            last_ms: acc.last_ms,
            messages: None,
        })
    }

    fn message_markers(&self) -> &'static [&'static [u8]] {
        MESSAGE_MARKERS
    }
}

/// `sessions\YYYY\MM\DD\rollout-*.jsonl`, at most four levels deep.
fn walk(dir: &Path, depth: usize, visited: &mut usize, max: usize, out: &mut Vec<FileStamp>) {
    if depth > 3 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        *visited += 1;
        if *visited > max {
            return;
        }
        let Ok(t) = e.file_type() else { continue };
        let path = e.path();
        if t.is_dir() {
            walk(&path, depth + 1, visited, max, out);
        } else if t.is_file()
            && path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("jsonl"))
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("rollout-"))
            && let Some(s) = FileStamp::of(path)
        {
            out.push(s);
        }
    }
}

/// `rollout-2026-11-08T21-57-37-<uuid>.jsonl` -> the uuid.
pub fn id_from_file_name(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let tail = stem.get(stem.len().checked_sub(36)?..)?;
    is_uuid(tail).then(|| tail.to_ascii_lowercase())
}

#[derive(Default)]
struct Acc {
    id: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
    version: Option<String>,
    source: Option<String>,
    model: Option<String>,
    started_ms: Option<u64>,
    first_ms: Option<u64>,
    last_ms: Option<u64>,
    preview: Option<String>,
    subagent: bool,
}

impl Acc {
    fn line(&mut self, v: &Value, head: bool, want_preview: bool) {
        if let Some(ms) = str_at(v, "/timestamp").and_then(parse_rfc3339_ms) {
            if head && self.first_ms.is_none() {
                self.first_ms = Some(ms);
            }
            self.last_ms = Some(self.last_ms.map_or(ms, |l| l.max(ms)));
        }
        let Some(p) = v.get("payload").filter(|p| p.is_object()) else {
            return;
        };
        match v.get("type").and_then(Value::as_str) {
            Some("session_meta") if head && self.id.is_none() => {
                self.id = str_at(p, "/id")
                    .or_else(|| str_at(p, "/session_id"))
                    .map(str::to_ascii_lowercase)
                    .filter(|i| is_uuid(i));
                self.cwd = str_at(p, "/cwd").map(str::to_string);
                self.started_ms = str_at(p, "/timestamp").and_then(parse_rfc3339_ms);
                self.version = str_at(p, "/cli_version").map(str::to_string);
                self.branch = str_at(p, "/git/branch").map(str::to_string);
                match p.get("source") {
                    Some(Value::String(s)) => self.source = Some(s.clone()),
                    // Sub-agent threads record an object naming their parent.
                    Some(other @ Value::Object(_)) if other.to_string().contains("subagent") => {
                        self.subagent = true
                    }
                    _ => {}
                }
            }
            Some("turn_context") => {
                if let Some(m) = str_at(p, "/model") {
                    self.model = Some(m.to_string());
                }
                if self.cwd.is_none() {
                    self.cwd = str_at(p, "/cwd").map(str::to_string);
                }
            }
            Some("event_msg") => {
                let text = match p.get("type").and_then(Value::as_str) {
                    Some("user_message") => str_at(p, "/message").map(str::to_string),
                    Some("item_completed")
                        if p.pointer("/item/type").and_then(Value::as_str)
                            == Some("UserMessage") =>
                    {
                        let parts: Vec<&str> = p
                            .pointer("/item/content")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|c| c.get("text").and_then(Value::as_str))
                                    .collect()
                            })
                            .unwrap_or_default();
                        (!parts.is_empty()).then(|| parts.join(" "))
                    }
                    Some("thread_settings_applied") => {
                        if let Some(m) = str_at(p, "/thread_settings/model") {
                            self.model = Some(m.to_string());
                        }
                        None
                    }
                    _ => None,
                };
                if want_preview
                    && self.preview.is_none()
                    && let Some(t) = text
                    && !is_wrapper_prompt(&t)
                {
                    self.preview = summarize(&t, SUMMARY_CHARS);
                }
            }
            _ => {}
        }
    }
}
