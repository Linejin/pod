//! Wake-up rules (docs/19 §4.3, §5.2): when a one-line mail notification
//! may be typed into an agent's terminal, and whether a person has an
//! unsent draft in it.

/// The line typed into an idle agent. Only safe characters: no quotes,
/// `%`, control characters or line breaks.
pub fn notification_line(unread: usize) -> String {
    format!("pod mail: {unread} new message(s) - run: pod orch check")
}

/// Output must have been quiet this long before a notification is typed.
pub const QUIET_MS: u64 = 1_500;

/// Why a notification waits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Defer {
    /// The agent works; it is told when its turn ends.
    Busy,
    /// Waiting on a person (permission, question).
    Blocked,
    /// A person typed into the input box and did not send it.
    Draft,
    /// The screen is still changing.
    Noisy,
    /// The terminal exited.
    Exited,
}

/// Whether a notification may be typed now. `state` is the status
/// engine's display state (`idle` `working` `blocked` `done` `unknown`
/// `error` `exited`).
pub fn may_wake(state: &str, draft: bool, quiet_ms: u64) -> Result<(), Defer> {
    match state {
        "exited" => Err(Defer::Exited),
        "blocked" => Err(Defer::Blocked),
        "working" => Err(Defer::Busy),
        _ if draft => Err(Defer::Draft),
        _ if quiet_ms < QUIET_MS => Err(Defer::Noisy),
        _ => Ok(()),
    }
}

/// Tracks what a person typed into a terminal (attach-client input) to
/// tell whether an unsent draft is in the agent's input box.
///
/// Printable characters (and pastes) start a draft; Enter, Ctrl+C and
/// Ctrl+U end it; Backspace removes one character. Escape sequences (arrow
/// keys, focus reports) are skipped. It cannot see what the program does
/// with the keys, so it errs towards "draft": typing then clearing the box
/// with the mouse still counts until Enter/Ctrl+C/Ctrl+U.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DraftTracker {
    chars: usize,
    esc: EscState,
    /// Inside a bracketed paste.
    paste: bool,
    /// Parameters of the CSI sequence being read.
    csi: Vec<u8>,
    /// Unix ms of the last typed (non-escape) key.
    pub last_typed_ms: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum EscState {
    #[default]
    None,
    Esc,
    /// CSI parameters; the bytes collected decide paste start/end.
    Csi,
    /// OSC/DCS string until BEL or ST.
    Str,
    StrEsc,
}

impl DraftTracker {
    /// Feed bytes a person sent. Returns true when they typed anything
    /// (printable, Enter, Backspace, Ctrl keys), as opposed to only escape
    /// sequences such as focus reports.
    pub fn feed(&mut self, bytes: &[u8], now_ms: u64) -> bool {
        let mut typed = false;
        for &b in bytes {
            match self.esc {
                EscState::Esc => {
                    self.esc = match b {
                        b'[' => {
                            self.csi.clear();
                            EscState::Csi
                        }
                        b']' | b'P' | b'_' | b'^' => EscState::Str,
                        _ => EscState::None, // Alt+key / SS3: not text
                    };
                    continue;
                }
                EscState::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.esc = EscState::None;
                        if b == b'~' && self.csi == b"200" {
                            self.paste = true;
                        } else if b == b'~' && self.csi == b"201" {
                            self.paste = false;
                        }
                    } else {
                        self.csi.push(b);
                    }
                    continue;
                }
                EscState::Str => {
                    if b == 0x07 {
                        self.esc = EscState::None;
                    } else if b == 0x1b {
                        self.esc = EscState::StrEsc;
                    }
                    continue;
                }
                EscState::StrEsc => {
                    self.esc = if b == b'\\' {
                        EscState::None
                    } else {
                        EscState::Str
                    };
                    continue;
                }
                EscState::None => {}
            }
            match b {
                0x1b => self.esc = EscState::Esc,
                b'\r' | b'\n' if self.paste => {
                    self.chars += 1;
                    typed = true;
                }
                b'\r' | 0x03 | 0x15 => {
                    self.chars = 0;
                    typed = true;
                }
                0x08 | 0x7f => {
                    self.chars = self.chars.saturating_sub(1);
                    typed = true;
                }
                b if b >= 0x20 => {
                    // UTF-8 continuation bytes are part of one character.
                    if b & 0xc0 != 0x80 {
                        self.chars += 1;
                    }
                    typed = true;
                }
                _ => {}
            }
        }
        if typed {
            self.last_typed_ms = now_ms;
        }
        typed
    }

    /// An unsent draft is (probably) in the input box.
    pub fn draft(&self) -> bool {
        self.chars > 0
    }

    /// The agent took the input (a new turn started): forget the draft.
    pub fn clear(&mut self) {
        self.chars = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_is_safe() {
        let l = notification_line(2);
        assert_eq!(l, "pod mail: 2 new message(s) - run: pod orch check");
        assert!(!l.contains(['"', '%', '\r', '\n', '\'']));
    }

    #[test]
    fn wake_rules() {
        assert_eq!(may_wake("idle", false, 2_000), Ok(()));
        assert_eq!(may_wake("done", false, 2_000), Ok(()));
        assert_eq!(may_wake("unknown", false, 2_000), Ok(()));
        assert_eq!(may_wake("working", false, 99_000), Err(Defer::Busy));
        assert_eq!(may_wake("blocked", false, 99_000), Err(Defer::Blocked));
        assert_eq!(may_wake("exited", false, 99_000), Err(Defer::Exited));
        assert_eq!(may_wake("idle", true, 99_000), Err(Defer::Draft));
        assert_eq!(may_wake("idle", false, 1_000), Err(Defer::Noisy));
    }

    #[test]
    fn drafts() {
        let mut d = DraftTracker::default();
        assert!(!d.feed(b"\x1b[I\x1b[O", 1), "focus reports are not typing");
        assert!(!d.draft());
        assert!(d.feed("hé".as_bytes(), 2));
        assert!(d.draft());
        assert_eq!(d.last_typed_ms, 2);
        d.feed(b"\x7f", 3);
        assert!(d.draft());
        d.feed(b"\x7f", 4);
        assert!(!d.draft(), "erased");
        d.feed(b"abc\x1b[D\x1b[D", 5);
        assert!(d.draft(), "arrows keep it");
        d.feed(b"\r", 6);
        assert!(!d.draft(), "sent");
        d.feed(b"x\x03", 7);
        assert!(!d.draft(), "Ctrl+C");
        d.feed(b"y\x15", 8);
        assert!(!d.draft(), "Ctrl+U");
        d.feed(b"\x1b[200~two\rlines\x1b[201~", 9);
        assert!(d.draft(), "a pasted newline does not send");
        d.clear();
        assert!(!d.draft());
        // Split escape sequences and OSC strings.
        d.feed(b"\x1b", 10);
        d.feed(b"[I", 11);
        assert!(!d.draft());
        d.feed(b"\x1b]11;rgb:0000/0000/0000\x1b\\", 12);
        assert!(!d.draft());
        assert_eq!(d.last_typed_ms, 9);
    }
}
