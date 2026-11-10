//! Claude Code's local session store (observed format, docs/18 §2):
//!
//! - `<root>\projects\<encoded cwd>\<session uuid>.jsonl`: one JSON object
//!   per line. Lines carry `sessionId`, `cwd`, `gitBranch`, `version` and
//!   `timestamp`; `type: "user"` / `"assistant"` lines hold messages
//!   (`message.content`, `message.model`); `type: "ai-title"` (`aiTitle`),
//!   `"custom-title"` (`customTitle`) and the older `"summary"` (`summary`)
//!   lines name the session. Sub-directories (sub-agent transcripts) are
//!   not sessions.
//! - `<root>\sessions\<pid>.json`: `{pid, sessionId, cwd, ...}` for running
//!   interactive processes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::jsonl::{self, object, str_at};
use crate::time::parse_rfc3339_ms;
use crate::{
    Agent, Budget, FileStamp, Limits, SUMMARY_CHARS, SessionMeta, Source, is_uuid,
    is_wrapper_prompt, summarize,
};

/// Lines counted as messages.
/// (Compact JSON as written today, and with a space after `:`.)
pub const MESSAGE_MARKERS: &[&[u8]] = &[
    b"\"type\":\"user\"",
    b"\"type\":\"assistant\"",
    b"\"type\": \"user\"",
    b"\"type\": \"assistant\"",
];
/// Bounds for the running-process records.
const MAX_PID_FILES: usize = 256;
const MAX_PID_FILE_BYTES: u64 = 64 * 1024;

pub struct ClaudeSource {
    root: PathBuf,
}

impl ClaudeSource {
    pub fn new(root: PathBuf) -> Self {
        ClaudeSource { root }
    }
}

impl Source for ClaudeSource {
    fn agent(&self) -> Agent {
        Agent::Claude
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn list(&self, limits: &Limits) -> Vec<FileStamp> {
        let mut out = Vec::new();
        let mut visited = 0usize;
        let Ok(projects) = std::fs::read_dir(self.root.join("projects")) else {
            return out;
        };
        for project in projects.flatten() {
            if !project.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(project.path()) else {
                continue;
            };
            for f in files.flatten() {
                visited += 1;
                if visited > limits.max_listed {
                    return out;
                }
                let path = f.path();
                let ok = path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
                    && path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(is_uuid)
                    && f.file_type().is_ok_and(|t| t.is_file());
                if ok && let Some(stamp) = FileStamp::of(path) {
                    out.push(stamp);
                }
            }
        }
        out
    }

    fn parse(&self, file: &FileStamp, limits: &Limits, budget: &mut Budget) -> Option<SessionMeta> {
        let id = file.path.file_stem()?.to_str()?.to_ascii_lowercase();
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
        if file.size > limits.head_bytes
            && let Some(tail) = jsonl::tail(&file.path, file.size, limits.tail_bytes, budget)
        {
            for line in &tail {
                if let Some(v) = object(line) {
                    acc.line(&v, false, false);
                }
            }
        }
        let cwd = acc.cwd?;
        let usable = acc.has_messages
            || acc.preview.is_some()
            || acc.custom_title.is_some()
            || acc.ai_title.is_some();
        if !usable {
            return None;
        }
        Some(SessionMeta {
            id,
            cwd,
            title: acc
                .custom_title
                .or(acc.ai_title)
                .or(acc.summary)
                .and_then(|t| summarize(&t, SUMMARY_CHARS)),
            preview: if limits.preview { acc.preview } else { None },
            model: acc.model,
            branch: acc.branch,
            version: acc.version,
            origin: acc.entrypoint,
            started_ms: acc.first_ms,
            last_ms: acc.last_ms,
            messages: None,
        })
    }

    fn message_markers(&self) -> &'static [&'static [u8]] {
        MESSAGE_MARKERS
    }

    fn running_sessions(&self, budget: &mut Budget) -> HashMap<u32, String> {
        let mut out = HashMap::new();
        let Ok(dir) = std::fs::read_dir(self.root.join("sessions")) else {
            return out;
        };
        for f in dir.flatten().take(MAX_PID_FILES) {
            let path = f.path();
            if !path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            let Some(stamp) = FileStamp::of(path) else {
                continue;
            };
            if stamp.size > MAX_PID_FILE_BYTES || !budget.take(stamp.size) {
                continue;
            }
            let Some(v) = jsonl::open_shared(&stamp.path)
                .ok()
                .and_then(|f| serde_json::from_reader::<_, Value>(std::io::BufReader::new(f)).ok())
            else {
                continue;
            };
            if let Some((pid, id)) = parse_pid_record(&v) {
                out.insert(pid, id);
            }
        }
        out
    }
}

/// `{pid, sessionId}` of a running-process record.
pub fn parse_pid_record(v: &Value) -> Option<(u32, String)> {
    let pid = u32::try_from(v.get("pid")?.as_u64()?).ok()?;
    let id = str_at(v, "/sessionId")?.to_ascii_lowercase();
    is_uuid(&id).then_some((pid, id))
}

#[derive(Default)]
struct Acc {
    cwd: Option<String>,
    branch: Option<String>,
    version: Option<String>,
    entrypoint: Option<String>,
    model: Option<String>,
    first_ms: Option<u64>,
    last_ms: Option<u64>,
    preview: Option<String>,
    ai_title: Option<String>,
    custom_title: Option<String>,
    summary: Option<String>,
    has_messages: bool,
}

impl Acc {
    /// Fold one line. Head lines fill first-seen facts; later lines update
    /// the latest ones (time, model, branch, titles).
    fn line(&mut self, v: &Value, head: bool, want_preview: bool) {
        if let Some(ms) = str_at(v, "/timestamp").and_then(parse_rfc3339_ms) {
            if head && self.first_ms.is_none() {
                self.first_ms = Some(ms);
            }
            self.last_ms = Some(self.last_ms.map_or(ms, |l| l.max(ms)));
        }
        if self.cwd.is_none()
            && let Some(c) = str_at(v, "/cwd")
        {
            self.cwd = Some(c.to_string());
        }
        if let Some(b) = str_at(v, "/gitBranch") {
            self.branch = Some(b.to_string());
        }
        if let Some(s) = str_at(v, "/version") {
            self.version = Some(s.to_string());
        }
        if self.entrypoint.is_none()
            && let Some(s) = str_at(v, "/entrypoint")
        {
            self.entrypoint = Some(s.to_string());
        }
        match v.get("type").and_then(Value::as_str) {
            Some("user") => {
                if v.get("isMeta").and_then(Value::as_bool) == Some(true)
                    || v.get("isSidechain").and_then(Value::as_bool) == Some(true)
                {
                    return;
                }
                self.has_messages = true;
                if want_preview
                    && self.preview.is_none()
                    && let Some(text) = user_text(v.pointer("/message/content"))
                    && !is_wrapper_prompt(&text)
                {
                    self.preview = summarize(&text, SUMMARY_CHARS);
                }
            }
            Some("assistant") => {
                self.has_messages = true;
                if let Some(m) = str_at(v, "/message/model").filter(|m| !m.starts_with('<')) {
                    self.model = Some(m.to_string());
                }
            }
            Some("ai-title") => {
                if let Some(t) = str_at(v, "/aiTitle") {
                    self.ai_title = Some(t.to_string());
                }
            }
            Some("custom-title") => {
                if let Some(t) = str_at(v, "/customTitle") {
                    self.custom_title = Some(t.to_string());
                }
            }
            Some("summary") => {
                if let Some(t) = str_at(v, "/summary") {
                    self.summary = Some(t.to_string());
                }
            }
            _ => {}
        }
    }
}

/// Text the user typed: a string, or the text blocks of a block list (a list
/// holding a `tool_result` is a tool answer, not a prompt).
fn user_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(s) => Some(s.clone()),
        Value::Array(blocks) => {
            if blocks
                .iter()
                .any(|b| b.get("type").and_then(Value::as_str) == Some("tool_result"))
            {
                return None;
            }
            let text: Vec<&str> = blocks
                .iter()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str))
                .collect();
            (!text.is_empty()).then(|| text.join(" "))
        }
        _ => None,
    }
}
