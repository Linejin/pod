//! OSC 7501 (`terminal.program-status.v1`) incremental parser.
//!
//! Grammar (docs/04 §1.2, §4.4):
//!
//! ```text
//! Report := ESC "]7501;" Field *( ":" Field ) Term
//! Query  := ESC "]7501;?" Term
//! Term   := BEL / ESC "\"
//! Field  := key "=" value     ; key=[a-z]{1,16}, value=[A-Za-z0-9_.,+/=-]*
//! ```
//!
//! The parser is fed raw PTY output in arbitrary chunks. It never alters or
//! consumes the byte stream (podd passes the bytes through unchanged); it only
//! observes it and yields [`OscEvent`]s.

use base64::Engine as _;
use base64::alphabet;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use serde::{Deserialize, Serialize};

/// The exact bytes a terminal writes back (to the PTY input) when the program
/// sends the `ESC ] 7501 ; ? Term` query. ST-terminated, the common reply form.
pub const QUERY_REPLY: &[u8] =
    b"\x1b]7501;?:states=idle,working,done,blocked,error:kinds=permission,question,auth\x1b\\";

/// Returns [`QUERY_REPLY`]; convenience for callers that prefer a function.
pub fn query_reply() -> &'static [u8] {
    QUERY_REPLY
}

/// Maximum payload (bytes after `7501;`) accepted. Longer sequences are dropped.
pub const MAX_PAYLOAD: usize = 4087;
pub const MAX_KEY_LEN: usize = 16;
pub const MAX_APP_LEN: usize = 32;
pub const MAX_ID_LEN: usize = 128;
pub const MAX_ID_SEGMENTS: usize = 8;
pub const MAX_ID_SEGMENT_LEN: usize = 32;
pub const MAX_TITLE_ENCODED: usize = 256;
pub const MAX_TITLE_DECODED: usize = 192;
pub const MAX_MSG_ENCODED: usize = 2732;
pub const MAX_MSG_DECODED: usize = 2048;

/// `state=` values of a report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    /// Delete the record (`id` given) or release the root status.
    Clear,
}

/// `kind=` values (meaningful only with `state=blocked`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportKind {
    Permission,
    Question,
    Auth,
}

/// A validated OSC 7501 report.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Report {
    pub state: Option<ReportState>,
    pub app: Option<String>,
    /// Sub-record path (`a/b/c`). `None` = root record.
    pub id: Option<String>,
    /// Only kept when `state=blocked`.
    pub kind: Option<ReportKind>,
    /// 0..=100, only kept when `state` is `working` or `blocked`.
    pub progress: Option<u8>,
    /// Decoded from base64.
    pub title: Option<String>,
    /// Decoded from base64.
    pub msg: Option<String>,
}

impl Report {
    /// The (always present after validation) state.
    pub fn state(&self) -> ReportState {
        self.state.unwrap_or(ReportState::Clear)
    }
}

/// Why a sequence was dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvalidReason {
    TooLong,
    NotAscii,
    MissingState,
    BadField(String),
}

/// Something the parser found in the stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OscEvent {
    /// `ESC ] 7501 ; ? Term` — the terminal should write [`QUERY_REPLY`].
    Query,
    /// A valid report.
    Report(Report),
    /// A 7501 sequence that was dropped as a whole (for diagnostics).
    Invalid(InvalidReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Ground,
    /// Saw ESC.
    Esc,
    /// Inside `ESC ]`, matching the `7501;` prefix; `usize` = matched bytes.
    Prefix(usize),
    /// Inside a 7501 payload.
    Payload,
    /// Inside a 7501 payload, saw ESC (expecting `\`).
    PayloadEsc,
}

const PREFIX: &[u8] = b"7501;";

/// Incremental OSC 7501 scanner. One instance per terminal.
#[derive(Debug, Clone)]
pub struct Osc7501Parser {
    st: St,
    buf: Vec<u8>,
    overflow: bool,
}

impl Default for Osc7501Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Osc7501Parser {
    pub fn new() -> Self {
        Self {
            st: St::Ground,
            buf: Vec::new(),
            overflow: false,
        }
    }

    /// True while the parser holds a partial sequence (split across chunks).
    pub fn is_mid_sequence(&self) -> bool {
        self.st != St::Ground
    }

    /// Feed a chunk, calling `out` for each event found.
    pub fn feed_with(&mut self, bytes: &[u8], mut out: impl FnMut(OscEvent)) {
        for &b in bytes {
            self.step(b, &mut out);
        }
    }

    /// Feed a chunk and collect events.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<OscEvent> {
        let mut v = Vec::new();
        self.feed_with(bytes, |e| v.push(e));
        v
    }

    fn step(&mut self, b: u8, out: &mut impl FnMut(OscEvent)) {
        match self.st {
            St::Ground => {
                if b == 0x1b {
                    self.st = St::Esc;
                }
            }
            St::Esc => {
                self.st = match b {
                    b']' => St::Prefix(0),
                    0x1b => St::Esc,
                    _ => St::Ground,
                };
            }
            St::Prefix(n) => {
                if b == PREFIX[n] {
                    if n + 1 == PREFIX.len() {
                        self.buf.clear();
                        self.overflow = false;
                        self.st = St::Payload;
                    } else {
                        self.st = St::Prefix(n + 1);
                    }
                } else if b == 0x1b {
                    // Some other OSC / aborted; ESC may start the next sequence.
                    self.st = St::Esc;
                } else {
                    // Another OSC number: not ours. Skip to the next ESC.
                    self.st = St::Ground;
                }
            }
            St::Payload => match b {
                0x07 => self.finish(out),
                0x1b => self.st = St::PayloadEsc,
                0x18 | 0x1a => self.abort(), // CAN / SUB cancel the sequence
                _ => self.push(b),
            },
            St::PayloadEsc => {
                if b == b'\\' {
                    self.finish(out);
                } else {
                    // ESC not followed by `\`: the OSC was cut off. The ESC may
                    // start a new sequence; re-process this byte from Esc.
                    self.abort();
                    self.st = St::Esc;
                    self.step(b, out);
                }
            }
        }
    }

    fn push(&mut self, b: u8) {
        if self.buf.len() >= MAX_PAYLOAD {
            self.overflow = true;
        } else {
            self.buf.push(b);
        }
    }

    fn abort(&mut self) {
        self.buf.clear();
        self.overflow = false;
        self.st = St::Ground;
    }

    fn finish(&mut self, out: &mut impl FnMut(OscEvent)) {
        self.st = St::Ground;
        let ev = if self.overflow {
            OscEvent::Invalid(InvalidReason::TooLong)
        } else {
            parse_payload(&self.buf)
        };
        self.buf.clear();
        self.overflow = false;
        out(ev);
    }
}

/// Parse a complete payload (the bytes between `7501;` and the terminator).
pub fn parse_payload(payload: &[u8]) -> OscEvent {
    if payload.len() > MAX_PAYLOAD {
        return OscEvent::Invalid(InvalidReason::TooLong);
    }
    if !payload.iter().all(|&c| (0x20..0x7f).contains(&c)) {
        return OscEvent::Invalid(InvalidReason::NotAscii);
    }
    // Safe: all printable ASCII.
    let s = std::str::from_utf8(payload).unwrap_or_default();
    if s == "?" {
        return OscEvent::Query;
    }
    if s.starts_with('?') {
        // A capability reply echoed back (e.g. by a nested terminal). Not a report.
        return OscEvent::Invalid(InvalidReason::BadField("reply".into()));
    }
    let mut r = Report::default();
    for field in s.split(':') {
        let Some((key, value)) = field.split_once('=') else {
            return bad(field);
        };
        if key.is_empty() || key.len() > MAX_KEY_LEN || !key.bytes().all(|c| c.is_ascii_lowercase())
        {
            return bad(key);
        }
        if !value.bytes().all(is_value_byte) {
            return bad(key);
        }
        let ok = match key {
            "state" => parse_state(value).map(|v| r.state = Some(v)),
            "app" => valid_token(value, MAX_APP_LEN).then(|| r.app = Some(value.to_string())),
            "id" => valid_id(value).then(|| r.id = Some(value.to_string())),
            "kind" => parse_kind(value).map(|v| r.kind = Some(v)),
            "progress" => parse_progress(value).map(|v| r.progress = Some(v)),
            "title" => {
                decode_text(value, MAX_TITLE_ENCODED, MAX_TITLE_DECODED).map(|v| r.title = v)
            }
            "msg" => decode_text(value, MAX_MSG_ENCODED, MAX_MSG_DECODED).map(|v| r.msg = v),
            _ => Some(()), // unknown keys are ignored (forward compatible, incl. `x*` Pod keys)
        };
        if ok.is_none() {
            return bad(key);
        }
    }
    let Some(state) = r.state else {
        return OscEvent::Invalid(InvalidReason::MissingState);
    };
    if state != ReportState::Blocked {
        r.kind = None;
    }
    if !matches!(state, ReportState::Working | ReportState::Blocked) {
        r.progress = None;
    }
    OscEvent::Report(r)
}

fn bad(what: &str) -> OscEvent {
    OscEvent::Invalid(InvalidReason::BadField(what.chars().take(32).collect()))
}

fn is_value_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-')
}

fn is_token_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'+' | b'-')
}

fn valid_token(v: &str, max: usize) -> bool {
    !v.is_empty() && v.len() <= max && v.bytes().all(is_token_byte)
}

fn valid_id(v: &str) -> bool {
    if v.is_empty() || v.len() > MAX_ID_LEN {
        return false;
    }
    let segs: Vec<&str> = v.split('/').collect();
    segs.len() <= MAX_ID_SEGMENTS && segs.iter().all(|s| valid_token(s, MAX_ID_SEGMENT_LEN))
}

fn parse_state(v: &str) -> Option<ReportState> {
    Some(match v {
        "idle" => ReportState::Idle,
        "working" => ReportState::Working,
        "done" => ReportState::Done,
        "blocked" => ReportState::Blocked,
        "error" => ReportState::Error,
        "clear" => ReportState::Clear,
        _ => return None,
    })
}

fn parse_kind(v: &str) -> Option<ReportKind> {
    Some(match v {
        "permission" => ReportKind::Permission,
        "question" => ReportKind::Question,
        "auth" => ReportKind::Auth,
        _ => return None,
    })
}

/// `0*\d{1,3}`, value ≤ 100.
fn parse_progress(v: &str) -> Option<u8> {
    if v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let trimmed = v.trim_start_matches('0');
    if trimmed.len() > 3 {
        return None;
    }
    let n: u32 = if trimmed.is_empty() {
        0
    } else {
        trimmed.parse().ok()?
    };
    (n <= 100).then_some(n as u8)
}

const B64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// Decode a base64 UTF-8 text field. `Some(None)` = empty value.
fn decode_text(v: &str, max_enc: usize, max_dec: usize) -> Option<Option<String>> {
    if v.len() > max_enc {
        return None;
    }
    if v.is_empty() {
        return Some(None);
    }
    let bytes = B64.decode(v).ok()?;
    if bytes.len() > max_dec {
        return None;
    }
    let s = String::from_utf8(bytes).ok()?;
    if s.chars().any(char::is_control) {
        return None;
    }
    Some(Some(s))
}

/// Encode a report as an OSC 7501 sequence (BEL-terminated). Handy for tests
/// and for Pod's own tools that want to self-report.
pub fn encode_report(r: &Report) -> Vec<u8> {
    let state = match r.state() {
        ReportState::Idle => "idle",
        ReportState::Working => "working",
        ReportState::Done => "done",
        ReportState::Blocked => "blocked",
        ReportState::Error => "error",
        ReportState::Clear => "clear",
    };
    let mut fields = vec![format!("state={state}")];
    if let Some(app) = &r.app {
        fields.push(format!("app={app}"));
    }
    if let Some(id) = &r.id {
        fields.push(format!("id={id}"));
    }
    if let Some(k) = r.kind {
        let k = match k {
            ReportKind::Permission => "permission",
            ReportKind::Question => "question",
            ReportKind::Auth => "auth",
        };
        fields.push(format!("kind={k}"));
    }
    if let Some(p) = r.progress {
        fields.push(format!("progress={p}"));
    }
    if let Some(t) = &r.title {
        fields.push(format!(
            "title={}",
            base64::engine::general_purpose::STANDARD.encode(t)
        ));
    }
    if let Some(m) = &r.msg {
        fields.push(format!(
            "msg={}",
            base64::engine::general_purpose::STANDARD.encode(m)
        ));
    }
    let mut out = b"\x1b]7501;".to_vec();
    out.extend_from_slice(fields.join(":").as_bytes());
    out.push(0x07);
    out
}
