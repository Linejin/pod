//! Named keys -> the bytes a VT terminal sends for them.

/// Translate one key name. `app_cursor` selects SS3 (`ESC O A`) instead of CSI
/// (`ESC [ A`) for arrows/Home/End, as DECCKM requires.
///
/// Accepted: `enter`/`return`, `tab`, `shift+tab`/`backtab`, `esc`/`escape`,
/// `backspace`/`bs`, `space`, `up`/`down`/`left`/`right`, `home`, `end`,
/// `pageup`/`pgup`, `pagedown`/`pgdn`, `insert`/`ins`, `delete`/`del`,
/// `f1`..`f12`, `ctrl+<a-z @ [ \ ] ^ _ space>`, `alt+<any key>`, or a single
/// literal character.
pub fn key_bytes(name: &str, app_cursor: bool) -> Option<Vec<u8>> {
    let lower = name.trim().to_ascii_lowercase();
    if let Some(rest) = lower
        .strip_prefix("alt+")
        .or_else(|| lower.strip_prefix("meta+"))
    {
        // Keep the case of a literal character after alt+ (alt+A != alt+a).
        let original = &name.trim()[name.trim().len() - rest.len()..];
        let mut out = vec![0x1b];
        out.extend(key_bytes(original, app_cursor)?);
        return Some(out);
    }
    if let Some(rest) = lower
        .strip_prefix("ctrl+")
        .or_else(|| lower.strip_prefix("c-"))
    {
        let b = match rest {
            "space" | "@" | "2" => 0x00,
            "[" | "3" => 0x1b,
            "\\" | "4" => 0x1c,
            "]" | "5" => 0x1d,
            "^" | "6" => 0x1e,
            "_" | "-" | "7" => 0x1f,
            "?" | "8" => 0x7f,
            s if s.len() == 1 && s.as_bytes()[0].is_ascii_lowercase() => s.as_bytes()[0] & 0x1f,
            _ => return None,
        };
        return Some(vec![b]);
    }
    let csi_or_ss3 = |c: u8| {
        if app_cursor {
            vec![0x1b, b'O', c]
        } else {
            vec![0x1b, b'[', c]
        }
    };
    let tilde = |n: &str| format!("\x1b[{n}~").into_bytes();
    Some(match lower.as_str() {
        "enter" | "return" | "cr" => b"\r".to_vec(),
        "tab" => b"\t".to_vec(),
        "shift+tab" | "backtab" => b"\x1b[Z".to_vec(),
        "esc" | "escape" => b"\x1b".to_vec(),
        "backspace" | "bs" => b"\x7f".to_vec(),
        "space" => b" ".to_vec(),
        "up" => csi_or_ss3(b'A'),
        "down" => csi_or_ss3(b'B'),
        "right" => csi_or_ss3(b'C'),
        "left" => csi_or_ss3(b'D'),
        "home" => csi_or_ss3(b'H'),
        "end" => csi_or_ss3(b'F'),
        "insert" | "ins" => tilde("2"),
        "delete" | "del" => tilde("3"),
        "pageup" | "pgup" => tilde("5"),
        "pagedown" | "pgdn" => tilde("6"),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde("15"),
        "f6" => tilde("17"),
        "f7" => tilde("18"),
        "f8" => tilde("19"),
        "f9" => tilde("20"),
        "f10" => tilde("21"),
        "f11" => tilde("23"),
        "f12" => tilde("24"),
        _ => {
            // A single literal character, case preserved.
            let t = name.trim();
            let mut chars = t.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.to_string().into_bytes(),
                _ => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::key_bytes;

    #[test]
    fn named_keys() {
        assert_eq!(key_bytes("Enter", false).unwrap(), b"\r");
        assert_eq!(key_bytes("ctrl+c", false).unwrap(), [0x03]);
        assert_eq!(key_bytes("CTRL+]", false).unwrap(), [0x1d]);
        assert_eq!(key_bytes("esc", false).unwrap(), [0x1b]);
        assert_eq!(key_bytes("up", false).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("up", true).unwrap(), b"\x1bOA");
        assert_eq!(key_bytes("f5", false).unwrap(), b"\x1b[15~");
        assert_eq!(key_bytes("shift+tab", false).unwrap(), b"\x1b[Z");
        assert_eq!(key_bytes("alt+x", false).unwrap(), b"\x1bx");
        assert_eq!(key_bytes("alt+X", false).unwrap(), b"\x1bX");
        assert_eq!(key_bytes("y", false).unwrap(), b"y");
        assert!(key_bytes("ctrl+", false).is_none());
        assert!(key_bytes("hyper+q", false).is_none());
    }
}
