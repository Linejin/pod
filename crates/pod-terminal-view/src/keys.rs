//! xterm key encoding.
//!
//! Plain printable text (no Ctrl/Alt) is *not* encoded here: it reaches the
//! view through the platform text-input path (WM_CHAR / IME commit), so IME
//! composition and keyboard layouts work. This module handles everything a
//! text-input path cannot express: control keys, function keys, modifiers.
//!
//! Modifier parameter (xterm `modifyCursorKeys`/`modifyFunctionKeys` = 2):
//! `1 + shift(1) + alt(2) + ctrl(4)`, e.g. Ctrl+Up = `CSI 1;5A`.

use crate::vt::Modes;

/// Key input independent of GPUI so it can be unit-tested and reused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyInput<'a> {
    /// GPUI key name: "a", "enter", "f5", "pageup", "^" …
    pub key: &'a str,
    /// Text the key would type with the current layout, if any.
    pub key_char: Option<&'a str>,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Windows/Command key: never sent to the program.
    pub platform: bool,
    /// The platform says this key produces layout text with these modifiers
    /// (AltGr on European layouts): let the text path handle it.
    pub prefer_text: bool,
}

/// Modes that change key encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModes {
    pub app_cursor: bool,
    pub app_keypad: bool,
}

impl From<&Modes> for KeyModes {
    fn from(m: &Modes) -> Self {
        Self {
            app_cursor: m.app_cursor,
            app_keypad: m.app_keypad,
        }
    }
}

fn modifier_param(k: &KeyInput) -> u8 {
    1 + k.shift as u8 + 2 * k.alt as u8 + 4 * k.ctrl as u8
}

/// `CSI 1;m X` / `SS3 X` / `CSI X` for cursor-style keys (arrows, Home, End, F1–F4).
fn cursor_key(k: &KeyInput, final_byte: char, ss3_when_plain: bool) -> Vec<u8> {
    let m = modifier_param(k);
    if m > 1 {
        format!("\x1b[1;{m}{final_byte}").into_bytes()
    } else if ss3_when_plain {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

/// `CSI n ~` / `CSI n;m ~` for editing and F5+ keys.
fn tilde_key(k: &KeyInput, n: u8) -> Vec<u8> {
    let m = modifier_param(k);
    if m > 1 {
        format!("\x1b[{n};{m}~").into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn with_alt(alt: bool, mut bytes: Vec<u8>) -> Vec<u8> {
    if alt {
        bytes.insert(0, 0x1b);
    }
    bytes
}

/// Ctrl+key → C0 control byte (xterm/VT220 conventions).
fn ctrl_byte(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    let (Some(c), None) = (chars.next(), chars.next()) else {
        return match key {
            "space" => Some(0x00),
            _ => None,
        };
    };
    let c = c.to_ascii_lowercase();
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' | ' ' | '`' => 0x00,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' | '~' => 0x1e,
        '_' | '7' | '-' | '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

/// Encode a key press for the program, or `None` when it is plain text that
/// the text-input path will deliver (or a key we do not send at all).
pub fn encode_key(k: &KeyInput, modes: KeyModes) -> Option<Vec<u8>> {
    if k.platform {
        return None;
    }
    let plain = !k.ctrl && !k.alt && !k.shift;
    let app = modes.app_cursor;
    let seq = match k.key {
        "enter" => with_alt(k.alt, b"\r".to_vec()),
        "tab" if k.shift => b"\x1b[Z".to_vec(),
        "tab" => with_alt(k.alt, b"\t".to_vec()),
        "backspace" if k.ctrl => with_alt(k.alt, b"\x08".to_vec()),
        "backspace" => with_alt(k.alt, b"\x7f".to_vec()),
        "escape" => with_alt(k.alt, b"\x1b".to_vec()),
        "up" => cursor_key(k, 'A', app),
        "down" => cursor_key(k, 'B', app),
        "right" => cursor_key(k, 'C', app),
        "left" => cursor_key(k, 'D', app),
        "home" => cursor_key(k, 'H', app),
        "end" => cursor_key(k, 'F', app),
        "insert" => tilde_key(k, 2),
        "delete" => tilde_key(k, 3),
        "pageup" => tilde_key(k, 5),
        "pagedown" => tilde_key(k, 6),
        "f1" => cursor_key(k, 'P', true),
        "f2" => cursor_key(k, 'Q', true),
        "f3" => cursor_key(k, 'R', true),
        "f4" => cursor_key(k, 'S', true),
        "f5" => tilde_key(k, 15),
        "f6" => tilde_key(k, 17),
        "f7" => tilde_key(k, 18),
        "f8" => tilde_key(k, 19),
        "f9" => tilde_key(k, 20),
        "f10" => tilde_key(k, 21),
        "f11" => tilde_key(k, 23),
        "f12" => tilde_key(k, 24),
        "space" if k.ctrl => with_alt(k.alt, vec![0]),
        "space" if k.alt => b"\x1b ".to_vec(),
        // Plain/shift space is text.
        "space" => return None,
        key => {
            if k.prefer_text {
                return None;
            }
            if k.ctrl {
                return ctrl_byte(key).map(|b| with_alt(k.alt, vec![b]));
            }
            if k.alt {
                // Alt+x → ESC x. Use the layout's character when it has one.
                let text = k.key_char.map(str::to_string).unwrap_or_else(|| {
                    if k.shift {
                        key.to_uppercase()
                    } else {
                        key.to_string()
                    }
                });
                if text.chars().count() != 1 {
                    return None;
                }
                return Some(with_alt(true, text.into_bytes()));
            }
            // Plain or shifted printable key: text path.
            let _ = plain;
            return None;
        }
    };
    Some(seq)
}

/// Prepare clipboard text for the program: normalize newlines to CR and wrap
/// in bracketed-paste markers when the program asked for them. Any embedded
/// end marker is stripped so pasted text cannot break out of the bracket.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let body = normalized.replace("\x1b[201~", "").replace("\x1b[200~", "");
        let mut out = b"\x1b[200~".to_vec();
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalized.into_bytes()
    }
}

/// Focus in/out reports (mode 1004).
pub fn encode_focus(focused: bool) -> &'static [u8] {
    if focused { b"\x1b[I" } else { b"\x1b[O" }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(key: &str) -> KeyInput<'_> {
        KeyInput {
            key,
            ..Default::default()
        }
    }
    fn enc(k: KeyInput) -> Option<String> {
        encode_key(&k, KeyModes::default()).map(|b| String::from_utf8(b).unwrap())
    }
    fn enc_app(k: KeyInput) -> Option<String> {
        encode_key(
            &k,
            KeyModes {
                app_cursor: true,
                app_keypad: true,
            },
        )
        .map(|b| String::from_utf8(b).unwrap())
    }

    #[test]
    fn plain_text_goes_to_text_path() {
        assert_eq!(
            enc(KeyInput {
                key_char: Some("a"),
                ..k("a")
            }),
            None
        );
        assert_eq!(
            enc(KeyInput {
                key_char: Some("A"),
                shift: true,
                ..k("a")
            }),
            None
        );
        assert_eq!(enc(k("space")), None);
        // AltGr+q on a German layout types '@'.
        assert_eq!(
            enc(KeyInput {
                key_char: Some("@"),
                ctrl: true,
                alt: true,
                prefer_text: true,
                ..k("q")
            }),
            None
        );
    }

    #[test]
    fn control_keys() {
        assert_eq!(enc(k("enter")).unwrap(), "\r");
        assert_eq!(
            enc(KeyInput {
                alt: true,
                ..k("enter")
            })
            .unwrap(),
            "\x1b\r"
        );
        assert_eq!(enc(k("tab")).unwrap(), "\t");
        assert_eq!(
            enc(KeyInput {
                shift: true,
                ..k("tab")
            })
            .unwrap(),
            "\x1b[Z"
        );
        assert_eq!(enc(k("backspace")).unwrap(), "\x7f");
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("backspace")
            })
            .unwrap(),
            "\x08"
        );
        assert_eq!(
            enc(KeyInput {
                alt: true,
                ..k("backspace")
            })
            .unwrap(),
            "\x1b\x7f"
        );
        assert_eq!(enc(k("escape")).unwrap(), "\x1b");
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("space")
            })
            .unwrap(),
            "\0"
        );
        assert_eq!(
            enc(KeyInput {
                alt: true,
                ..k("space")
            })
            .unwrap(),
            "\x1b "
        );
    }

    #[test]
    fn ctrl_combos() {
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("c")
            })
            .unwrap(),
            "\x03"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                shift: true,
                ..k("a")
            })
            .unwrap(),
            "\x01"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("[")
            })
            .unwrap(),
            "\x1b"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("\\")
            })
            .unwrap(),
            "\x1c"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("]")
            })
            .unwrap(),
            "\x1d"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("^")
            })
            .unwrap(),
            "\x1e"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("_")
            })
            .unwrap(),
            "\x1f"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("@")
            })
            .unwrap(),
            "\0"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("2")
            })
            .unwrap(),
            "\0"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("/")
            })
            .unwrap(),
            "\x1f"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                alt: true,
                ..k("x")
            })
            .unwrap(),
            "\x1b\x18"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("1")
            }),
            None
        );
    }

    #[test]
    fn alt_combos() {
        assert_eq!(
            enc(KeyInput {
                alt: true,
                key_char: Some("b"),
                ..k("b")
            })
            .unwrap(),
            "\x1bb"
        );
        assert_eq!(
            enc(KeyInput {
                alt: true,
                shift: true,
                ..k("b")
            })
            .unwrap(),
            "\x1bB"
        );
        assert_eq!(
            enc(KeyInput {
                alt: true,
                ..k(".")
            })
            .unwrap(),
            "\x1b."
        );
    }

    #[test]
    fn cursor_keys_and_modes() {
        assert_eq!(enc(k("up")).unwrap(), "\x1b[A");
        assert_eq!(enc_app(k("up")).unwrap(), "\x1bOA");
        assert_eq!(enc(k("home")).unwrap(), "\x1b[H");
        assert_eq!(enc_app(k("end")).unwrap(), "\x1bOF");
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("left")
            })
            .unwrap(),
            "\x1b[1;5D"
        );
        assert_eq!(
            enc_app(KeyInput {
                ctrl: true,
                ..k("left")
            })
            .unwrap(),
            "\x1b[1;5D"
        );
        assert_eq!(
            enc(KeyInput {
                shift: true,
                ..k("right")
            })
            .unwrap(),
            "\x1b[1;2C"
        );
        assert_eq!(
            enc(KeyInput {
                alt: true,
                ..k("up")
            })
            .unwrap(),
            "\x1b[1;3A"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                shift: true,
                alt: true,
                ..k("down")
            })
            .unwrap(),
            "\x1b[1;8B"
        );
        assert_eq!(
            enc(KeyInput {
                shift: true,
                ..k("home")
            })
            .unwrap(),
            "\x1b[1;2H"
        );
    }

    #[test]
    fn editing_keys() {
        assert_eq!(enc(k("insert")).unwrap(), "\x1b[2~");
        assert_eq!(enc(k("delete")).unwrap(), "\x1b[3~");
        assert_eq!(enc(k("pageup")).unwrap(), "\x1b[5~");
        assert_eq!(enc(k("pagedown")).unwrap(), "\x1b[6~");
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("delete")
            })
            .unwrap(),
            "\x1b[3;5~"
        );
        assert_eq!(
            enc(KeyInput {
                alt: true,
                ..k("pageup")
            })
            .unwrap(),
            "\x1b[5;3~"
        );
    }

    #[test]
    fn function_keys() {
        let plain = [
            "\x1bOP", "\x1bOQ", "\x1bOR", "\x1bOS", "\x1b[15~", "\x1b[17~", "\x1b[18~", "\x1b[19~",
            "\x1b[20~", "\x1b[21~", "\x1b[23~", "\x1b[24~",
        ];
        for (i, want) in plain.iter().enumerate() {
            let name = format!("f{}", i + 1);
            assert_eq!(enc(k(&name)).unwrap(), *want, "{name}");
        }
        assert_eq!(
            enc(KeyInput {
                shift: true,
                ..k("f1")
            })
            .unwrap(),
            "\x1b[1;2P"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                ..k("f4")
            })
            .unwrap(),
            "\x1b[1;5S"
        );
        assert_eq!(
            enc(KeyInput {
                shift: true,
                ..k("f5")
            })
            .unwrap(),
            "\x1b[15;2~"
        );
        assert_eq!(
            enc(KeyInput {
                ctrl: true,
                shift: true,
                ..k("f12")
            })
            .unwrap(),
            "\x1b[24;6~"
        );
    }

    #[test]
    fn windows_key_is_not_sent() {
        assert_eq!(
            enc(KeyInput {
                platform: true,
                ..k("enter")
            }),
            None
        );
    }

    #[test]
    fn paste() {
        assert_eq!(encode_paste("a\r\nb\nc", false), b"a\rb\rc");
        assert_eq!(encode_paste("x", true), b"\x1b[200~x\x1b[201~");
        assert_eq!(
            encode_paste("a\x1b[201~rm -rf", true),
            b"\x1b[200~arm -rf\x1b[201~"
        );
    }

    #[test]
    fn focus() {
        assert_eq!(encode_focus(true), b"\x1b[I");
        assert_eq!(encode_focus(false), b"\x1b[O");
    }
}
