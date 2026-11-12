//! L1 hook adapter: the wire message `pod-hook` sends to podd, and the mapping
//! of Claude Code / Codex hook events (stdin JSON payloads) to Pod states.
//!
//! Payload field names follow docs/04 §3.3 and the public
//! Claude Code hook schema. Fields marked "unverified" in [`PAYLOAD_FIELDS`]
//! were never seen in a real payload during research.
//!
//! Layout: this file holds the wire message, the effect types and the
//! shared payload helpers; `hook/claude.rs` and `hook/codex.rs` map each
//! agent's events and list the events Pod installs for it.

use crate::types::{AgentState, Outcome};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod claude;
mod codex;

pub use claude::{CLAUDE_EVENTS, CLAUDE_MATCHER_EVENTS};
pub use codex::CODEX_EVENTS;

/// Wire version of [`HookMessage`].
pub const HOOK_MESSAGE_VERSION: u32 = 1;

/// Longest hook line podd accepts, in bytes without the newline (podd's
/// request line limit). `pod-hook` sends [`HookMessage::to_wire_line`] with this.
pub const MAX_HOOK_LINE: usize = 4 * 1024 * 1024;
/// Payload key that marks a payload cut to fit the line:
/// `{"pod_truncated": {"payload_bytes": <original JSON size>}}`.
pub const TRUNCATED_KEY: &str = "pod_truncated";
/// Scalar payload fields up to this size survive a cut (ids, names, kinds).
const KEEP_FIELD_BYTES: usize = 4096;
/// Longest event name a hook message may carry (`PostToolUseFailure` is 18).
pub const MAX_EVENT_NAME: usize = 64;
/// Longest `terminal_id` a hook message may carry.
pub const MAX_TERMINAL_ID: usize = 64;
/// Hook text shown as status (tool name, prompt message, error, subagent
/// type) is cut to this many characters (TM-22: hook input is evidence,
/// not a channel for arbitrary text).
pub const MAX_HOOK_TEXT: usize = 256;

/// Env vars `pod-hook` reads (injected into each PTY by podd).
pub const ENV_TERMINAL_ID: &str = "POD_TERMINAL_ID";
pub const ENV_SOCKET: &str = "POD_SOCKET";
pub const ENV_HOOK_TOKEN: &str = "POD_HOOK_TOKEN";

/// Which agent CLI invoked the hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HookAgent {
    Claude,
    Codex,
}

impl HookAgent {
    pub fn parse(s: &str) -> Option<HookAgent> {
        match s.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(HookAgent::Claude),
            "codex" => Some(HookAgent::Codex),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            HookAgent::Claude => "claude",
            HookAgent::Codex => "codex",
        }
    }
}

/// One JSON line sent by `pod-hook` over the podd named pipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HookMessage {
    /// Always [`HOOK_MESSAGE_VERSION`].
    pub v: u32,
    /// `POD_TERMINAL_ID` of the PTY the agent runs in.
    pub terminal_id: String,
    /// `POD_HOOK_TOKEN`, if podd injected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    pub agent: HookAgent,
    /// Event name from argv (falls back to `payload.hook_event_name`).
    pub event: String,
    /// The hook's stdin JSON as-is (`null` if stdin was empty or not JSON).
    #[serde(default)]
    pub payload: Value,
    /// Wall clock (unix ms) when `pod-hook` ran; informational only.
    #[serde(default)]
    pub sent_at_unix_ms: u64,
}

impl HookMessage {
    pub fn new(
        terminal_id: impl Into<String>,
        agent: HookAgent,
        event: impl Into<String>,
        payload: Value,
    ) -> Self {
        Self {
            v: HOOK_MESSAGE_VERSION,
            terminal_id: terminal_id.into(),
            token: None,
            agent,
            event: event.into(),
            payload,
            sent_at_unix_ms: 0,
        }
    }

    /// Serialize as one line (no trailing newline).
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    /// [`HookMessage::to_line`] that is never longer than `max` bytes when it
    /// can be helped: an oversized payload keeps its small scalar fields
    /// (`session_id`, `hook_event_name`, `tool_name`, ...) plus a
    /// [`TRUNCATED_KEY`] marker, or only the marker. The event itself is
    /// always kept, so a big tool output never costs a status change.
    pub fn to_wire_line(&self, max: usize) -> String {
        let line = self.to_line();
        if line.len() <= max {
            return line;
        }
        let payload_bytes = serde_json::to_string(&self.payload).map_or(0, |p| p.len());
        let marker = serde_json::json!({ "payload_bytes": payload_bytes });
        let mut kept = serde_json::Map::new();
        if let Value::Object(fields) = &self.payload {
            for (k, v) in fields {
                let small = !matches!(v, Value::Object(_) | Value::Array(_))
                    && serde_json::to_string(v).is_ok_and(|j| j.len() <= KEEP_FIELD_BYTES);
                if small && k != TRUNCATED_KEY {
                    kept.insert(k.clone(), v.clone());
                }
            }
        }
        kept.insert(TRUNCATED_KEY.into(), marker.clone());
        let mut cut = self.clone();
        cut.payload = Value::Object(kept);
        let line = cut.to_line();
        if line.len() <= max {
            return line;
        }
        cut.payload = serde_json::json!({ TRUNCATED_KEY: marker });
        cut.to_line()
    }

    pub fn from_line(line: &str) -> serde_json::Result<HookMessage> {
        serde_json::from_str(line.trim_end())
    }

    /// The shape podd accepts from an authenticated sender (TM-22): a short
    /// printable `terminal_id`, an event name of letters and digits (or
    /// none), and a payload that is an object or `null`. Anything else is
    /// rejected before it reaches the engine.
    pub fn check(&self) -> Result<(), &'static str> {
        if self.terminal_id.is_empty()
            || self.terminal_id.len() > MAX_TERMINAL_ID
            || !self.terminal_id.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err("bad terminal id");
        }
        if !valid_event_name(&self.event) {
            return Err("bad event name");
        }
        if !matches!(self.payload, Value::Object(_) | Value::Null) {
            return Err("payload is not an object");
        }
        if let Some(name) = self.payload.get("hook_event_name")
            && !name.as_str().is_some_and(valid_event_name)
        {
            return Err("bad hook_event_name");
        }
        Ok(())
    }

    /// The effective event name.
    pub fn event_name(&self) -> &str {
        if !self.event.is_empty() {
            return &self.event;
        }
        self.payload
            .get("hook_event_name")
            .and_then(Value::as_str)
            .unwrap_or("")
    }
}

/// What a hook event does to the root (lead agent) state.
#[derive(Debug, Clone, PartialEq)]
pub enum RootEffect {
    /// No change to the root state.
    None,
    Set {
        state: AgentState,
        msg: Option<String>,
    },
    /// Return to the state held before the last wait (permission/question).
    RestoreFromWait,
    /// The session ended (the process will exit soon); hook evidence is dropped.
    EndSession,
}

/// What a hook event does to a sub-record (subagent).
#[derive(Debug, Clone, PartialEq)]
pub enum ChildEffect {
    Set {
        id: String,
        state: AgentState,
        title: Option<String>,
    },
    Remove {
        id: String,
    },
}

/// The interpretation of one hook event.
#[derive(Debug, Clone, PartialEq)]
pub struct HookEffect {
    pub root: RootEffect,
    pub child: Option<ChildEffect>,
    /// New session: clear sub-records.
    pub session_boundary: bool,
    pub outcome: Option<Outcome>,
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
}

impl HookEffect {
    fn none() -> Self {
        Self {
            root: RootEffect::None,
            child: None,
            session_boundary: false,
            outcome: None,
            session_id: None,
            tool_name: None,
        }
    }
    fn set(state: AgentState, msg: Option<String>) -> Self {
        Self {
            root: RootEffect::Set { state, msg },
            ..Self::none()
        }
    }
}

/// Payload field reference: `(agent, event, field, verified)`. `verified` =
/// seen in a real payload or only in documentation (docs/04 §3.3). Unverified
/// fields are read defensively (absence is fine).
pub static PAYLOAD_FIELDS: &[(&str, &str, &str, bool)] = &[
    ("claude", "*", "session_id", true),
    ("claude", "*", "hook_event_name", true),
    ("claude", "*", "transcript_path", true),
    ("claude", "*", "cwd", true),
    ("claude", "*", "agent_id", true),
    ("claude", "SessionStart", "source", true),
    ("claude", "UserPromptSubmit", "prompt", true),
    ("claude", "PreToolUse", "tool_name", true),
    ("claude", "PreToolUse", "tool_use_id", true),
    ("claude", "PermissionRequest", "tool_name", true),
    ("claude", "Elicitation", "message", false),
    ("claude", "Elicitation", "mcp_server_name", false),
    ("claude", "Stop", "is_interrupt", false),
    ("claude", "StopFailure", "error", false),
    ("claude", "SubagentStart", "agent_type", true),
    ("claude", "SessionEnd", "reason", false),
    ("codex", "*", "session_id", false),
    ("codex", "*", "turn_id", false),
    ("codex", "*", "transcript_path", false),
    ("codex", "*", "agent_id", false),
    ("codex", "PreToolUse", "tool_name", false),
    ("codex", "PermissionRequest", "tool_name", false),
];

/// Tool names that mean "the agent asks the user a question".
pub static QUESTION_TOOLS: &[&str] = &["AskUserQuestion", "request_user_input"];

/// An event name `podd` accepts: ASCII letters, digits and `_`, at most
/// [`MAX_EVENT_NAME`] bytes (empty: the payload's `hook_event_name` decides).
pub fn valid_event_name(e: &str) -> bool {
    e.len() <= MAX_EVENT_NAME && e.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn s(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_string)
}

/// Text from the payload that becomes visible status (message, title,
/// tool name): control and bidi-override characters removed, at most
/// [`MAX_HOOK_TEXT`] characters.
fn text(v: &Value, key: &str) -> Option<String> {
    let raw = v.get(key).and_then(Value::as_str)?;
    let clean: String = raw
        .chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .take(MAX_HOOK_TEXT)
        .collect();
    let clean = clean.trim();
    (!clean.is_empty()).then(|| clean.to_string())
}

fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

fn child_id(agent_id: &str) -> String {
    let clean: String = agent_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "_.+-".contains(c) {
                c
            } else {
                '-'
            }
        })
        .take(32)
        .collect();
    format!("hook/{clean}")
}

/// Interpret a hook event. Unknown events map to "no effect".
pub fn map_hook(agent: HookAgent, event: &str, payload: &Value) -> HookEffect {
    let mut eff = match agent {
        HookAgent::Claude => claude::map_claude(event, payload),
        HookAgent::Codex => codex::map_codex(event, payload),
    };
    eff.session_id = s(payload, "session_id");
    if eff.tool_name.is_none() {
        eff.tool_name = text(payload, "tool_name");
    }
    eff
}

/// Convenience: interpret a [`HookMessage`].
pub fn map_message(msg: &HookMessage) -> HookEffect {
    map_hook(msg.agent, msg.event_name(), &msg.payload)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(payload: Value) -> HookMessage {
        let mut m = HookMessage::new("t12", HookAgent::Claude, "PostToolUse", payload);
        m.token = Some("0123456789abcdef0123456789abcdef".into());
        m.sent_at_unix_ms = 1_794_000_000_000;
        m
    }

    /// A payload with `session_id` and one big field, whose full line is
    /// exactly `len` bytes.
    fn msg_with_line_len(len: usize) -> HookMessage {
        let mut m = msg(json!({ "session_id": "s1", "tool_name": "Bash", "tool_response": "" }));
        let base = m.to_line().len();
        m.payload["tool_response"] = Value::String("x".repeat(len - base));
        assert_eq!(m.to_line().len(), len);
        m
    }

    /// daemon-05: a line at the limit goes as is; one byte over, the payload
    /// is cut (small fields + marker), never the event.
    #[test]
    fn wire_line_boundaries() {
        let max = 10_000;
        let at = msg_with_line_len(max);
        assert_eq!(at.to_wire_line(max), at.to_line());

        let over = msg_with_line_len(max + 1);
        let line = over.to_wire_line(max);
        assert!(line.len() <= max, "{}", line.len());
        let back = HookMessage::from_line(&line).unwrap();
        assert_eq!(back.event, "PostToolUse");
        assert_eq!(back.terminal_id, "t12");
        assert_eq!(back.token, over.token);
        assert_eq!(back.payload["session_id"], "s1");
        assert_eq!(back.payload["tool_name"], "Bash");
        assert!(back.payload.get("tool_response").is_none());
        let full = serde_json::to_string(&over.payload).unwrap().len();
        assert_eq!(back.payload[TRUNCATED_KEY]["payload_bytes"], full);
        assert_eq!(map_message(&back).session_id.as_deref(), Some("s1"));
    }

    /// The review's case: a payload of exactly the stdin limit (4 MiB) plus
    /// the envelope was over podd's line limit and the whole event was lost.
    #[test]
    fn a_four_mib_payload_still_delivers_its_event() {
        let mut m = msg(json!({ "session_id": "s9", "hook_event_name": "Stop", "big": "" }));
        let pad = MAX_HOOK_LINE - serde_json::to_string(&m.payload).unwrap().len();
        m.payload["big"] = Value::String("y".repeat(pad));
        assert_eq!(
            serde_json::to_string(&m.payload).unwrap().len(),
            MAX_HOOK_LINE
        );
        assert!(
            m.to_line().len() > MAX_HOOK_LINE,
            "the envelope pushes it over"
        );
        let line = m.to_wire_line(MAX_HOOK_LINE);
        assert!(line.len() <= MAX_HOOK_LINE);
        let back = HookMessage::from_line(&line).unwrap();
        assert_eq!(back.payload["session_id"], "s9");
        assert_eq!(back.payload["hook_event_name"], "Stop");
        assert!(back.payload[TRUNCATED_KEY].is_object());
    }

    /// TM-22: podd takes only short, plain event names, a short printable
    /// terminal id and an object (or null) payload.
    #[test]
    fn message_shape_checked() {
        assert_eq!(msg(json!({"session_id": "s"})).check(), Ok(()));
        assert_eq!(msg(Value::Null).check(), Ok(()));
        let mut m = msg(json!({}));
        m.event = String::new();
        assert_eq!(m.check(), Ok(()), "event from the payload");
        for (event, why) in [
            ("Stop\nforged", "bad event name"),
            ("Stop Stop", "bad event name"),
            (&"E".repeat(MAX_EVENT_NAME + 1)[..], "bad event name"),
        ] {
            let mut m = msg(json!({}));
            m.event = event.into();
            assert_eq!(m.check(), Err(why), "{event:?}");
        }
        for payload in [json!([1, 2]), json!("text"), json!(7)] {
            assert_eq!(
                msg(payload.clone()).check(),
                Err("payload is not an object"),
                "{payload}"
            );
        }
        assert_eq!(
            msg(json!({"hook_event_name": "Stop\u{1b}[2J"})).check(),
            Err("bad hook_event_name")
        );
        for id in ["", "t 1", "t\u{7}", &"t".repeat(MAX_TERMINAL_ID + 1)[..]] {
            let mut m = msg(json!({}));
            m.terminal_id = id.into();
            assert_eq!(m.check(), Err("bad terminal id"), "{id:?}");
        }
        assert!(valid_event_name("PostToolUseFailure"));
        assert!(!valid_event_name("Pre-Tool"));
    }

    /// TM-22: payload text that becomes status (message, tool name, error,
    /// subagent title) loses control and bidi characters and is capped.
    #[test]
    fn hook_text_is_cleaned_and_capped() {
        let long = "q".repeat(MAX_HOOK_TEXT * 4);
        let m = HookMessage::new(
            "t1",
            HookAgent::Claude,
            "Elicitation",
            json!({"message": format!("\u{202E}Approve\u{1b}[31m\r\n{long}")}),
        );
        let RootEffect::Set {
            msg: Some(text), ..
        } = map_message(&m).root
        else {
            panic!("blocked with a message");
        };
        assert!(text.starts_with("Approve[31m"), "{text:?}");
        assert!(!text.chars().any(|c| c.is_control() || is_bidi_control(c)));
        assert_eq!(text.chars().count(), MAX_HOOK_TEXT);

        let m = HookMessage::new(
            "t1",
            HookAgent::Codex,
            "PermissionRequest",
            json!({"tool_name": format!("Bash\u{7}{long}")}),
        );
        let eff = map_message(&m);
        let tool = eff.tool_name.unwrap();
        assert!(tool.starts_with("Bash") && tool.chars().count() == MAX_HOOK_TEXT);
        let only_controls = HookMessage::new(
            "t1",
            HookAgent::Claude,
            "StopFailure",
            json!({"error": "\u{1b}\u{202E}\r\n"}),
        );
        let RootEffect::Set { msg, .. } = map_message(&only_controls).root else {
            panic!("error state");
        };
        assert_eq!(msg, None);
    }

    /// Many small fields that do not fit either: only the marker is left.
    #[test]
    fn marker_only_when_the_small_fields_do_not_fit() {
        let fields: serde_json::Map<String, Value> = (0..100)
            .map(|i| (format!("f{i}"), Value::String("z".repeat(100))))
            .collect();
        let m = msg(Value::Object(fields));
        let line = m.to_wire_line(2_000);
        assert!(line.len() <= 2_000, "{}", line.len());
        let back = HookMessage::from_line(&line).unwrap();
        let obj = back.payload.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert!(obj.contains_key(TRUNCATED_KEY));
    }
}
