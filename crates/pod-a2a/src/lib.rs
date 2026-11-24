//! Agent2Agent (A2A) protocol 1.0 client for Pod.
//!
//! Talks to one explicitly named peer agent over the standard JSON-RPC 2.0
//! binding (spec v1.0.1 §9): Agent Card discovery at
//! `/.well-known/agent-card.json` or an explicit card URL, interface
//! selection from `supportedInterfaces` (binding `JSONRPC`, protocol version
//! `1.0`), and the `SendMessage`, `GetTask`, `ListTasks` and `CancelTask`
//! methods with canonical ProtoJSON payloads and the `A2A-Version: 1.0`
//! header. Streaming, push notifications, the extended card and the other
//! bindings (gRPC, HTTP+JSON) are not implemented.
//!
//! What the peer returns is data: nothing here executes it, follows its
//! URLs, or treats an Agent Card as permission to reach another host or to
//! use Pod's own credentials. User guide: docs/a2a-client.md.
//!
//! Used by `pod a2a` (pod-cli); the blocking API is meant to be callable
//! from a UI background thread as well.

mod client;
mod error;
pub mod http;
pub mod jsonrpc;
pub mod model;
pub mod target;

pub use client::{A2aClient, CardSource, ClientConfig, Discovery, SelectedInterface, discover};
pub use error::{A2aError, Delivery, RpcError, RpcErrorKind};
pub use http::BearerToken;

/// Peer text made safe for one terminal line: control characters (ANSI
/// escapes, carriage returns, bidi overrides, ...) become `\u{..}` escapes
/// and the result is cut to `max` characters.
pub fn sanitize_line(s: &str, max: usize) -> String {
    sanitize(s, max, false)
}

/// Like [`sanitize_line`] but keeps line feeds and tabs (multi-line text).
pub fn sanitize_text(s: &str, max: usize) -> String {
    sanitize(s, max, true)
}

fn sanitize(s: &str, max: usize, multiline: bool) -> String {
    let mut out = String::new();
    for (n, c) in s.chars().enumerate() {
        if n >= max {
            out.push('…');
            break;
        }
        let keep = multiline && (c == '\n' || c == '\t');
        if !keep && (c.is_control() || is_format_char(c)) {
            out.push_str(&c.escape_unicode().to_string());
        } else {
            out.push(c);
        }
    }
    out
}

/// Invisible formatting characters that can reorder or hide terminal text.
fn is_format_char(c: char) -> bool {
    matches!(c,
        '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}'
        | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{061C}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_escapes_terminal_controls() {
        assert_eq!(sanitize_line("a\x1b[31mb", 100), "a\\u{1b}[31mb");
        assert_eq!(sanitize_line("x\ny", 100), "x\\u{a}y");
        assert_eq!(sanitize_text("x\ny\tz\r", 100), "x\ny\tz\\u{d}");
        assert_eq!(sanitize_line("ab\u{202E}cd", 100), "ab\\u{202e}cd");
        assert_eq!(sanitize_line("abcdef", 3), "abc…");
    }
}
