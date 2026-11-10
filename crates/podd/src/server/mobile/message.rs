//! Short text messages from a phone to an agent (docs/17 §0, 2026-11-10).
//!
//! A message is *refused* (not silently cleaned) when it is empty, too long
//! or carries anything but printable text and newlines: the person on the
//! phone must see exactly what the agent gets, and a stripped escape
//! sequence would change meaning without telling them. Accepted text is
//! typed in through the review delivery path (bracketed paste, Enter, submit
//! confirmation), never as keys. It always goes in under a fixed header
//! line, so its first character can never be taken as an agent command
//! (`/clear`, `!rm ...`) or a shell line.

use sha2::{Digest, Sha256};

use crate::protocol::*;

pub const MAX_CHARS: usize = 1000;
pub const MAX_BYTES: usize = 4 * 1024;
pub const MAX_LINES: usize = 20;

pub const HEADER: &str =
    "Message from the user, sent from their phone through Pod (plain text, not a command):";

fn invalid(m: impl Into<String>) -> RpcError {
    RpcError::new(mobile_codes::MESSAGE_INVALID, m)
}

/// Bidirectional formatting controls: they make text read differently from
/// what it is (`Cf`, not caught by `char::is_control`).
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Validate and normalize (CRLF -> LF, surrounding whitespace trimmed).
/// `allow_empty`: only with `with_open_comments` (the comments are the message).
pub fn validate(text: &str, allow_empty: bool) -> Result<String, RpcError> {
    if text.len() > MAX_BYTES * 2 {
        return Err(invalid(format!(
            "message too long (max {MAX_CHARS} characters)"
        )));
    }
    let t = text.replace("\r\n", "\n");
    let t = t.trim();
    if t.is_empty() && !allow_empty {
        return Err(invalid("message is empty"));
    }
    if let Some(c) = t
        .chars()
        .find(|&c| (c.is_control() && c != '\n') || is_bidi_control(c))
    {
        return Err(invalid(format!(
            "message contains a control character (U+{:04X}); only plain text and newlines are allowed",
            c as u32
        )));
    }
    let chars = t.chars().count();
    if chars > MAX_CHARS || t.len() > MAX_BYTES {
        return Err(invalid(format!(
            "message too long ({chars} characters; max {MAX_CHARS}, {MAX_BYTES} bytes)"
        )));
    }
    if t.lines().count() > MAX_LINES {
        return Err(invalid(format!("message has more than {MAX_LINES} lines")));
    }
    Ok(t.to_string())
}

/// For channels that hand over raw bytes (M2/M4): must be UTF-8.
pub fn validate_bytes(b: &[u8], allow_empty: bool) -> Result<String, RpcError> {
    let s = std::str::from_utf8(b).map_err(|_| invalid("message is not valid UTF-8"))?;
    validate(s, allow_empty)
}

/// The prompt typed into the agent: header line, blank line, the text.
pub fn frame(text: &str) -> String {
    format!("{HEADER}\n\n{text}")
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Request digest for idempotency: same operation id + same digest = replay.
pub fn request_digest(task: &str, text: &str, with_open_comments: bool) -> String {
    let mut b = Vec::with_capacity(task.len() + text.len() + 4);
    b.extend_from_slice(task.as_bytes());
    b.push(0);
    b.push(u8::from(with_open_comments));
    b.push(0);
    b.extend_from_slice(text.as_bytes());
    sha256_hex(&b)
}

/// Operation ids: 1..=64 of `[A-Za-z0-9_-]`.
pub fn check_operation_id(op: &str) -> Result<(), RpcError> {
    if op.is_empty()
        || op.len() > 64
        || !op
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            "operation_id must be 1-64 characters of A-Z a-z 0-9 _ -",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(r: Result<String, RpcError>) -> i64 {
        r.unwrap_err().code
    }

    #[test]
    fn accepts_plain_text_and_normalizes() {
        assert_eq!(
            validate("  please rebase\r\nthen test  ", false).unwrap(),
            "please rebase\nthen test"
        );
        assert_eq!(
            validate("한글 메시지 — ok ✓", false).unwrap(),
            "한글 메시지 — ok ✓"
        );
        assert_eq!(validate("", true).unwrap(), "");
        // 1000 Korean characters (3 bytes each) fit: the byte cap is a backstop.
        assert!(validate(&"가".repeat(MAX_CHARS), false).is_ok());
        assert!(validate(&"가".repeat(MAX_CHARS + 1), false).is_err());
        let max = "a".repeat(MAX_CHARS);
        assert!(validate(&max, false).is_ok());
        assert_eq!(
            code(validate(&format!("{max}a"), false)),
            mobile_codes::MESSAGE_INVALID
        );
        assert!(validate(&"a\n".repeat(MAX_LINES), false).is_ok());
        assert!(validate(&"a\n".repeat(MAX_LINES + 1), false).is_err());
    }

    #[test]
    fn refuses_empty_controls_and_escapes() {
        for bad in [
            "",
            "   \n ",
            "hi\x1b[201~ then", // would end the bracketed paste
            "\x1b]0;title\x07", // OSC
            "ctrl-c \x03",
            "tab\there",
            "cr only\rx",
            "nul\0",
            "del\x7f",
            "c1 \u{9b}31m",
            "bidi \u{202E}evil",
            "isolate \u{2066}x\u{2069}",
        ] {
            assert_eq!(
                code(validate(bad, false)),
                mobile_codes::MESSAGE_INVALID,
                "{bad:?}"
            );
        }
        assert!(validate_bytes(b"ok", false).is_ok());
        assert_eq!(
            code(validate_bytes(b"bad \xff\xfe", false)),
            mobile_codes::MESSAGE_INVALID
        );
        assert_eq!(
            code(validate_bytes(b"\xed\xa0\x80", false)),
            mobile_codes::MESSAGE_INVALID,
            "surrogate"
        );
    }

    #[test]
    fn framing_and_digests() {
        let p = frame("/clear");
        assert!(p.starts_with(HEADER), "never starts with the user's text");
        assert!(p.ends_with("\n\n/clear"));
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_ne!(
            request_digest("a/b", "x", false),
            request_digest("a/b", "x", true)
        );
        assert_ne!(
            request_digest("a/b", "x", false),
            request_digest("a/c", "x", false)
        );
        assert!(check_operation_id("op-1_A").is_ok());
        for bad in ["", "a b", "x/y", &"a".repeat(65)] {
            assert!(check_operation_id(bad).is_err(), "{bad:?}");
        }
    }
}
