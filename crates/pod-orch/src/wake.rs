//! Wake-up rules (docs/19 §4.3, §5.2, §21.1): when a one-line mail
//! notification may be typed into an agent's terminal (per status source:
//! an agent's own hook/OSC reports outrank a quiet screen), and whether a
//! person has an unsent draft in it.

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
    /// An agent with a status source has not reported its state yet (or
    /// took its report back): `unknown` is not idle for it.
    Unknown,
    /// An idle-looking state that is not the agent's own word: its newest
    /// hook/OSC report is still a turn in progress (`working`, `blocked`),
    /// e.g. the hook went stale and the screen got quiet.
    Stale,
}

/// What the program in a terminal says about itself (docs/19 §21.1, TM-22).
/// Hooks and OSC 7501 reports are the program's own word; screen and title
/// heuristics are Pod's guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reports {
    /// No status source: the program never reported in this run and is not
    /// Claude or Codex (a shell, a script). Its `unknown` counts as idle.
    None,
    /// Claude or Codex that has not reported in this run (starting up, or
    /// no Pod hooks): `unknown` waits; a state the heuristics decided
    /// counts, as for a program without a status source.
    Expected,
    /// Its newest report ended a turn (`idle` `done` `error`).
    TurnEnded,
    /// Its newest report is a turn in progress or waiting (`working`
    /// `blocked`), or `unknown` (taken back).
    InTurn,
}

impl Reports {
    /// From the state name of the program's newest own report (`None`: it
    /// never reported in this run) and whether it is Claude or Codex.
    pub fn of(last: Option<&str>, known_agent: bool) -> Reports {
        match last {
            Some("idle" | "done" | "error") => Reports::TurnEnded,
            Some(_) => Reports::InTurn,
            None if known_agent => Reports::Expected,
            None => Reports::None,
        }
    }
}

/// Whether a terminal counts as idle for mail (wake-ups, `@idle`). `state`
/// is the status engine's display state (`idle` `working` `blocked` `done`
/// `unknown` `error` `exited`). Busy, blocked and exited never are; an
/// agent with a status source is not idle while `unknown` or when the shown
/// state is not its own newest report.
pub fn idle(state: &str, reports: Reports) -> Result<(), Defer> {
    match state {
        "exited" => Err(Defer::Exited),
        "blocked" => Err(Defer::Blocked),
        "working" => Err(Defer::Busy),
        "unknown" if reports != Reports::None => Err(Defer::Unknown),
        _ if reports == Reports::InTurn => Err(Defer::Stale),
        _ => Ok(()),
    }
}

/// Whether a notification may be typed now: [`idle`], no unsent draft, and
/// output quiet for [`QUIET_MS`].
pub fn may_wake(state: &str, reports: Reports, draft: bool, quiet_ms: u64) -> Result<(), Defer> {
    idle(state, reports)?;
    if draft {
        return Err(Defer::Draft);
    }
    if quiet_ms < QUIET_MS {
        return Err(Defer::Noisy);
    }
    Ok(())
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

    /// A program without a status source keeps the rule it always had:
    /// `unknown` counts as idle.
    #[test]
    fn wake_rules_without_a_status_source() {
        let r = Reports::None;
        assert_eq!(may_wake("idle", r, false, 2_000), Ok(()));
        assert_eq!(may_wake("done", r, false, 2_000), Ok(()));
        assert_eq!(may_wake("error", r, false, 2_000), Ok(()));
        assert_eq!(may_wake("unknown", r, false, 2_000), Ok(()));
        assert_eq!(may_wake("working", r, false, 99_000), Err(Defer::Busy));
        assert_eq!(may_wake("blocked", r, false, 99_000), Err(Defer::Blocked));
        assert_eq!(may_wake("exited", r, false, 99_000), Err(Defer::Exited));
        assert_eq!(may_wake("idle", r, true, 99_000), Err(Defer::Draft));
        assert_eq!(may_wake("unknown", r, true, 99_000), Err(Defer::Draft));
        assert_eq!(may_wake("idle", r, false, 1_000), Err(Defer::Noisy));
    }

    /// TM-22: an agent with a status source (hooks/OSC) is woken only on
    /// its own turn end; `unknown` and a stale report wait. Busy, blocked
    /// and draft never wake whatever the source.
    #[test]
    fn wake_rules_with_a_status_source() {
        use Reports::*;
        for (state, reports, want) in [
            ("idle", TurnEnded, Ok(())),
            ("done", TurnEnded, Ok(())),
            ("error", TurnEnded, Ok(())),
            ("unknown", TurnEnded, Err(Defer::Unknown)),
            ("unknown", InTurn, Err(Defer::Unknown)),
            ("unknown", Expected, Err(Defer::Unknown)),
            ("idle", InTurn, Err(Defer::Stale)),
            ("done", InTurn, Err(Defer::Stale)),
            // Claude/Codex that never reported: what the heuristics decided.
            ("idle", Expected, Ok(())),
            ("done", Expected, Ok(())),
            ("working", Expected, Err(Defer::Busy)),
        ] {
            assert_eq!(
                may_wake(state, reports, false, 99_000),
                want,
                "{state} {reports:?}"
            );
            assert_eq!(idle(state, reports), want, "@idle: {state} {reports:?}");
        }
        for r in [None, Expected, TurnEnded, InTurn] {
            assert_eq!(may_wake("working", r, false, 99_000), Err(Defer::Busy));
            assert_eq!(may_wake("blocked", r, false, 99_000), Err(Defer::Blocked));
            assert_eq!(may_wake("exited", r, false, 99_000), Err(Defer::Exited));
            assert!(may_wake("idle", r, true, 99_000).is_err(), "draft {r:?}");
            assert!(may_wake("done", r, false, 1_000).is_err(), "noisy {r:?}");
        }
        assert_eq!(may_wake("done", TurnEnded, true, 99_000), Err(Defer::Draft));
        assert_eq!(may_wake("idle", TurnEnded, false, 1_000), Err(Defer::Noisy));
    }

    #[test]
    fn reports_from_the_newest_report() {
        use Reports::*;
        assert_eq!(Reports::of(Option::None, false), None);
        assert_eq!(Reports::of(Option::None, true), Expected);
        for s in ["idle", "done", "error"] {
            assert_eq!(Reports::of(Some(s), false), TurnEnded);
            assert_eq!(Reports::of(Some(s), true), TurnEnded);
        }
        for s in ["working", "blocked", "unknown"] {
            assert_eq!(Reports::of(Some(s), false), InTurn);
        }
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
