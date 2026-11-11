//! Printing shared by the commands: JSON, aligned columns, short commit ids,
//! terminal state words. What only one command prints stays with it.

use podd::protocol::{TerminalInfo, TerminalState};

pub(crate) fn short_sha(s: &str) -> &str {
    &s[..s.len().min(10)]
}

/// Left-aligned columns; the last one is not padded.
pub(crate) fn print_rows(header: &[&str], rows: Vec<Vec<String>>) {
    let mut w: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let mut s = String::new();
        let last = cells.len() - 1;
        for (i, c) in cells.iter().enumerate() {
            if i == last {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<width$}  ", width = w[i]));
            }
        }
        println!("{}", s.trim_end());
    };
    line(header.to_vec());
    for r in &rows {
        line(r.iter().map(String::as_str).collect());
    }
}

/// A grey prompt suggestion that `read` left out of the lines (docs/19
/// §18.3): named on its own line so a reading agent does not take it for
/// typed input.
pub(crate) fn print_suggestion(s: Option<&str>) {
    if let Some(s) = s {
        println!("[grey prompt suggestion at the caret, not typed: {s:?}]");
    }
}

pub(crate) fn print_json<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).expect("serialize"));
}

pub(crate) fn code_suffix(code: Option<i32>) -> String {
    code.map(|c| format!(" (code {c})")).unwrap_or_default()
}

pub(crate) fn state_word(t: &TerminalInfo) -> String {
    match t.state {
        TerminalState::Running => "running".into(),
        TerminalState::Exited if t.incompatible_holder.is_some() => "incompatible holder".into(),
        TerminalState::Exited if t.lost => "lost".into(),
        TerminalState::Exited if t.killed => format!("killed{}", code_suffix(t.exit_code)),
        TerminalState::Exited => format!("exited{}", code_suffix(t.exit_code)),
        TerminalState::Unreachable => "unreachable".into(),
    }
}
