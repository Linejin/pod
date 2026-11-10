//! Filtering of terminal query replies on attach input.
//!
//! A program writes terminal queries (DA1, DA2, DSR 5, DSR 6 / DECXCPR,
//! OSC 7501, and the color queries OSC 10/11/12/4). Every attached client's
//! terminal (`pod attach` inside Windows Terminal, each pod-app panel) sees the
//! same query bytes and may answer; its answer arrives as attach input
//! (`terminal.input`).
//!
//! The daemon scans the output stream for those queries ([`QueryScanner`]) and
//! records one entry per query with the number of attached clients
//! ([`Expectations`]). [`Expectations::filter_input`] then decides for each
//! reply sequence found in attach input:
//!
//! - **Holder-answered queries** (DA1, DA2, DSR, CPR, OSC 7501): the holder
//!   already answered, so every client reply is a duplicate and is dropped (v7).
//! - **Color queries** (OSC 10/11/12 and OSC 4, v9): the holder does not
//!   answer (it has no palette; the attached UI's theme is the truth). The
//!   first reply per query passes and the replies of the other clients are
//!   dropped as duplicates.
//! - **Late-reply guard (v9)**: a reply that arrives more than the reply
//!   window (default 500 ms, `podd --reply-window`) after its query, or one
//!   with no outstanding query at all, is dropped. The program has stopped
//!   waiting by then and would read it as keystrokes (docs/12 B4: Codex got
//!   `]10;rgb:…\` in its input box). The plain CPR shape `CSI r;c R` is also
//!   a modified F3 key, so it counts as a reply only while a CPR query is
//!   outstanding; otherwise it passes as a key.
//!
//! Only complete sequences inside one input write are recognized (a reply
//! split across two `terminal.input` calls passes through).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// How long a query stays outstanding (its replies are recognized).
pub const EXPECT_MS: u64 = 5_000;
/// Default late-reply window (`podd --reply-window`, `POD_REPLY_WINDOW_MS`).
pub const DEFAULT_REPLY_WINDOW_MS: u64 = 500;
/// Most outstanding queries kept (oldest dropped).
const MAX_EXPECT: usize = 256;
const MAX_SEQ: usize = 64;
/// Longest OSC body the output scanner keeps (a color query is short).
const MAX_OSC_QUERY: usize = 64;

static REPLY_WINDOW_MS: AtomicU64 = AtomicU64::new(DEFAULT_REPLY_WINDOW_MS);

/// Set the late-reply window; 0 turns the guard off.
pub fn set_reply_window_ms(ms: u64) {
    REPLY_WINDOW_MS.store(ms, Ordering::Relaxed);
}

pub fn reply_window_ms() -> u64 {
    REPLY_WINDOW_MS.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// `CSI c` / `CSI 0 c` -> `CSI ? ... c`
    Da1,
    /// `CSI > c` / `CSI > 0 c` -> `CSI > ... c`
    Da2,
    /// `CSI 5 n` -> `CSI 0 n`
    Status,
    /// `CSI 6 n` -> `CSI row ; col R`
    Cpr,
    /// `CSI ? 6 n` -> `CSI ? row ; col (; page) R`
    XCpr,
    /// `OSC 7501;?` -> `OSC 7501;?:...`
    Osc7501,
    /// `OSC 10|11|12;?` -> `OSC 10|11|12;rgb:...` (foreground, background, cursor).
    Color(u8),
    /// `OSC 4;n;?` -> `OSC 4;n;rgb:...` (one per queried index).
    Palette,
}

impl Reply {
    /// The holder answers this query itself (so client replies are duplicates).
    pub fn holder_answers(self) -> bool {
        !matches!(self, Reply::Color(_) | Reply::Palette)
    }
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum St {
    #[default]
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

/// Streaming scanner for terminal queries (output side).
#[derive(Default)]
pub struct QueryScanner {
    st: St,
    buf: Vec<u8>,
}

impl QueryScanner {
    pub fn scan(&mut self, bytes: &[u8], mut found: impl FnMut(Reply)) {
        for &b in bytes {
            match self.st {
                St::Ground => {
                    if b == 0x1b {
                        self.st = St::Esc;
                    }
                }
                St::Esc => self.after_esc(b),
                St::Csi => match b {
                    0x20..=0x3f if self.buf.len() < MAX_SEQ => self.buf.push(b),
                    0x40..=0x7e => {
                        if let Some(r) = query_kind(&self.buf, b) {
                            found(r);
                        }
                        self.st = St::Ground;
                    }
                    0x1b => self.st = St::Esc,
                    _ => self.st = St::Ground,
                },
                St::Osc => match b {
                    0x07 => {
                        osc_queries(&self.buf, &mut found);
                        self.st = St::Ground;
                    }
                    0x1b => self.st = St::OscEsc,
                    0x18 | 0x1a => self.st = St::Ground,
                    _ => {
                        // Only short query bodies matter; longer OSCs are skipped.
                        if self.buf.len() < MAX_OSC_QUERY {
                            self.buf.push(b);
                        } else {
                            self.buf.clear();
                            self.buf.extend_from_slice(b"-long-");
                        }
                    }
                },
                St::OscEsc => {
                    if b == b'\\' {
                        osc_queries(&self.buf, &mut found);
                        self.st = St::Ground;
                    } else {
                        // ESC ended the OSC and starts something else.
                        self.after_esc(b);
                    }
                }
            }
        }
    }

    fn after_esc(&mut self, b: u8) {
        self.buf.clear();
        self.st = match b {
            b'[' => St::Csi,
            b']' => St::Osc,
            0x1b => St::Esc,
            _ => St::Ground,
        }
    }
}

/// Queries in one complete OSC body (without `ESC ]` and terminator).
fn osc_queries(body: &[u8], found: &mut impl FnMut(Reply)) {
    if body == b"7501;?" {
        found(Reply::Osc7501);
        return;
    }
    let mut parts = body.split(|&b| b == b';');
    let Some(ps) = parts.next() else { return };
    match ps {
        // `OSC 10;?;?` asks for 10 and then 11 (xterm: successive dynamic colors).
        b"10" | b"11" | b"12" => {
            let first = ps[1] - b'0' + 10;
            for (i, p) in parts.enumerate() {
                let n = first as usize + i;
                if p == b"?" && n <= 12 {
                    found(Reply::Color(n as u8));
                }
            }
        }
        // `OSC 4;c;?;c;?`: one reply per queried index.
        b"4" => {
            let rest: Vec<&[u8]> = parts.collect();
            for pair in rest.chunks(2) {
                if pair.len() == 2 && pair[1] == b"?" && is_digits(pair[0]) {
                    found(Reply::Palette);
                }
            }
        }
        _ => {}
    }
}

fn query_kind(params: &[u8], fin: u8) -> Option<Reply> {
    match (fin, params) {
        (b'c', b"" | b"0") => Some(Reply::Da1),
        (b'c', b">" | b">0") => Some(Reply::Da2),
        (b'n', b"5") => Some(Reply::Status),
        (b'n', b"6") => Some(Reply::Cpr),
        (b'n', b"?6") => Some(Reply::XCpr),
        _ => None,
    }
}

fn is_digits(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(u8::is_ascii_digit)
}

fn digits_list(s: &[u8], min: usize, max: usize) -> bool {
    let parts: Vec<&[u8]> = s.split(|&b| b == b';').collect();
    (min..=max).contains(&parts.len()) && parts.iter().all(|p| is_digits(p))
}

/// What reply a complete CSI sequence (params + final) from input is, if any.
fn reply_kind(params: &[u8], fin: u8) -> Option<Reply> {
    match fin {
        b'c' => match params.first() {
            Some(b'?') => Some(Reply::Da1),
            Some(b'>') => Some(Reply::Da2),
            _ => None,
        },
        b'n' if params == b"0" => Some(Reply::Status),
        b'R' => match params.first() {
            Some(b'?') if digits_list(&params[1..], 2, 3) => Some(Reply::XCpr),
            Some(_) if digits_list(params, 2, 2) => Some(Reply::Cpr),
            _ => None,
        },
        _ => None,
    }
}

/// The reply an OSC body from input is, if any.
fn osc_reply_kind(body: &[u8]) -> Option<Reply> {
    if body.starts_with(b"7501;?:") {
        return Some(Reply::Osc7501);
    }
    let mut parts = body.splitn(3, |&b| b == b';');
    match parts.next()? {
        p @ (b"10" | b"11" | b"12") => {
            let v = parts.next()?;
            (v != b"?" && !v.is_empty()).then(|| Reply::Color(p[1] - b'0' + 10))
        }
        b"4" => {
            let idx = parts.next()?;
            let v = parts.next()?;
            (is_digits(idx) && v != b"?" && !v.is_empty()).then_some(Reply::Palette)
        }
        _ => None,
    }
}

/// Find a complete reply sequence starting at `input[0] == ESC`: its kind and length.
fn reply_at(input: &[u8]) -> Option<(Reply, usize)> {
    match input.get(1)? {
        b'[' => {
            let mut i = 2;
            while i < input.len() && i < MAX_SEQ && (0x20..=0x3f).contains(&input[i]) {
                i += 1;
            }
            let fin = *input.get(i)?;
            if !(0x40..=0x7e).contains(&fin) {
                return None;
            }
            reply_kind(&input[2..i], fin).map(|r| (r, i + 1))
        }
        b']' => {
            let body = input.get(2..)?;
            for (j, &b) in body.iter().enumerate().take(4096) {
                let (end, len) = match b {
                    0x07 => (j, 2 + j + 1),
                    0x1b if body.get(j + 1) == Some(&b'\\') => (j, 2 + j + 2),
                    0x1b | 0x18 | 0x1a => return None,
                    _ => continue,
                };
                return osc_reply_kind(&body[..end]).map(|r| (r, len));
            }
            None
        }
        _ => None,
    }
}

/// One query seen in the output while clients were attached.
#[derive(Debug, Clone, Copy)]
struct Query {
    kind: Reply,
    at: u64,
    /// Clients attached when the query went out (replies to expect).
    clients: usize,
    /// Replies already seen for this query.
    seen: usize,
}

/// Counts from one [`Expectations::filter_input`] call.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Filtered {
    /// Replies to a query already answered (by the holder or another client).
    pub duplicate: usize,
    /// Replies past the reply window, or with no outstanding query.
    pub late: usize,
}

impl Filtered {
    pub fn total(self) -> usize {
        self.duplicate + self.late
    }
}

enum Verdict {
    Pass,
    Duplicate,
    Late,
}

/// Queries whose replies the attached clients may still send.
#[derive(Default)]
pub struct Expectations {
    pending: VecDeque<Query>,
}

impl Expectations {
    /// A query went out to `clients` attached clients at `now`.
    pub fn expect(&mut self, kind: Reply, clients: usize, now: u64) {
        if clients == 0 {
            return;
        }
        if self.pending.len() >= MAX_EXPECT {
            self.pending.pop_front();
        }
        self.pending.push_back(Query {
            kind,
            at: now,
            clients,
            seen: 0,
        });
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    fn expire(&mut self, now: u64) {
        self.pending
            .retain(|q| q.at + EXPECT_MS >= now && q.seen < q.clients);
    }

    /// Account one reply of `kind`; `None` means "not a reply" (pass as input).
    fn judge(&mut self, kind: Reply, now: u64, window: u64) -> Option<Verdict> {
        let pos = self.pending.iter().position(|q| q.kind == kind);
        let Some(i) = pos else {
            // No outstanding query. A CPR-shaped sequence is also a key
            // (modified F3); the other reply shapes are never typed.
            if kind == Reply::Cpr {
                return None;
            }
            return Some(if window == 0 {
                Verdict::Pass
            } else {
                Verdict::Late
            });
        };
        let q = &mut self.pending[i];
        q.seen += 1;
        let first = q.seen == 1;
        let late = window > 0 && now.saturating_sub(q.at) > window;
        let holder = kind.holder_answers();
        if q.seen >= q.clients {
            self.pending.remove(i);
        }
        Some(if holder {
            Verdict::Duplicate
        } else if late {
            Verdict::Late
        } else if first {
            Verdict::Pass
        } else {
            Verdict::Duplicate
        })
    }

    /// Attach input with duplicate and late replies removed, with the window
    /// from [`reply_window_ms`].
    pub fn filter_input(&mut self, input: &[u8], now: u64) -> (Vec<u8>, Filtered) {
        self.filter_input_with(input, now, reply_window_ms())
    }

    /// [`Self::filter_input`] with an explicit late-reply window (0 = off).
    pub fn filter_input_with(
        &mut self,
        input: &[u8],
        now: u64,
        window: u64,
    ) -> (Vec<u8>, Filtered) {
        self.expire(now);
        let mut stats = Filtered::default();
        if !input.contains(&0x1b) || (window == 0 && self.pending.is_empty()) {
            return (input.to_vec(), stats);
        }
        let mut out = Vec::with_capacity(input.len());
        let mut i = 0;
        while i < input.len() {
            if input[i] == 0x1b
                && let Some((r, len)) = reply_at(&input[i..])
                && let Some(v) = self.judge(r, now, window)
            {
                match v {
                    Verdict::Pass => out.extend_from_slice(&input[i..i + len]),
                    Verdict::Duplicate => stats.duplicate += 1,
                    Verdict::Late => stats.late += 1,
                }
                i += len;
                continue;
            }
            out.push(input[i]);
            i += 1;
        }
        (out, stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan_all(chunks: &[&[u8]]) -> Vec<Reply> {
        let mut s = QueryScanner::default();
        let mut v = Vec::new();
        for c in chunks {
            s.scan(c, |r| v.push(r));
        }
        v
    }

    /// Filter with the default 500 ms window; returns (output, dropped total).
    fn f(e: &mut Expectations, input: &[u8], now: u64) -> (Vec<u8>, usize) {
        let (out, s) = e.filter_input_with(input, now, DEFAULT_REPLY_WINDOW_MS);
        (out, s.total())
    }

    #[test]
    fn scanner_finds_queries_across_chunks() {
        let all = b"hi\x1b[c\x1b[0c\x1b[>c\x1b[5n\x1b[6n\x1b[?6n\x1b]7501;?\x07\x1b]7501;?\x1b\\\x1b]10;?\x07\x1b]11;?\x1b\\\x1b]4;1;?;12;?\x07\x1b]10;?;?\x07\x1b[31mred\x1b[2J\x1b]0;title\x07";
        let want = vec![
            Reply::Da1,
            Reply::Da1,
            Reply::Da2,
            Reply::Status,
            Reply::Cpr,
            Reply::XCpr,
            Reply::Osc7501,
            Reply::Osc7501,
            Reply::Color(10),
            Reply::Color(11),
            Reply::Palette,
            Reply::Palette,
            Reply::Color(10),
            Reply::Color(11),
        ];
        assert_eq!(scan_all(&[all]), want);
        // Every two-way split gives the same result.
        for cut in 0..all.len() {
            assert_eq!(scan_all(&[&all[..cut], &all[cut..]]), want, "cut {cut}");
        }
        // Not queries: a status report, color *sets*, other CSI finals, a long OSC.
        assert!(
            scan_all(&[
                b"\x1b]7501;state=idle\x07\x1b]10;#ffffff\x07\x1b]4;1;rgb:ff/00/00\x07\x1b[?1049h\x1b[1;5R\x1b[0n"
            ])
            .is_empty()
        );
    }

    #[test]
    fn holder_answered_replies_are_dropped_once_per_client() {
        let mut e = Expectations::default();
        e.expect(Reply::Da1, 1, 0);
        e.expect(Reply::Cpr, 1, 0);
        e.expect(Reply::Osc7501, 1, 0);
        let input = b"x\x1b[?61;6;7c\x1b[12;40R\x1b]7501;?:states=idle\x1b\\y\x1b[A";
        let (out, s) = e.filter_input_with(input, 10, 500);
        assert_eq!(out, b"xy\x1b[A".to_vec());
        assert_eq!(
            s,
            Filtered {
                duplicate: 3,
                late: 0
            }
        );
        assert!(e.is_empty());

        e.expect(Reply::Da1, 2, 100);
        let (out, n) = f(&mut e, b"\x1b[?6c\x1b[?6c", 101);
        assert_eq!((out.as_slice(), n), (&b""[..], 2));
    }

    #[test]
    fn color_replies_first_passes_others_dropped() {
        let mut e = Expectations::default();
        // Two panels attached; the program asks for fg + bg + palette 1.
        e.expect(Reply::Color(10), 2, 0);
        e.expect(Reply::Color(11), 2, 0);
        e.expect(Reply::Palette, 2, 0);
        let a = b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:0000/0000/0000\x07\x1b]4;1;rgb:cd/00/00\x07";
        let (out, s) = e.filter_input_with(a, 20, 500);
        assert_eq!(out, a.to_vec(), "first client's replies reach the program");
        assert_eq!(s.total(), 0);
        // The second panel's identical answers are duplicates.
        let (out, s) = e.filter_input_with(a, 30, 500);
        assert!(out.is_empty());
        assert_eq!(
            s,
            Filtered {
                duplicate: 3,
                late: 0
            }
        );
        assert!(e.is_empty());
    }

    #[test]
    fn late_and_stray_replies_are_dropped() {
        let mut e = Expectations::default();
        // A reply with no query at all (e.g. a stale queued reply): dropped.
        let (out, s) = e.filter_input_with(b"a\x1b]11;rgb:0/0/0\x1b\\b\x1b[?6c", 0, 500);
        assert_eq!(out, b"ab".to_vec());
        assert_eq!(
            s,
            Filtered {
                duplicate: 0,
                late: 2
            }
        );

        // A color reply 501 ms after its query: late.
        e.expect(Reply::Color(11), 1, 1_000);
        let (out, s) = e.filter_input_with(b"\x1b]11;rgb:0/0/0\x07", 1_501, 500);
        assert!(out.is_empty());
        assert_eq!(s.late, 1);
        // Exactly at the window: still in time.
        e.expect(Reply::Color(11), 1, 2_000);
        let (out, _) = e.filter_input_with(b"\x1b]11;rgb:0/0/0\x07", 2_500, 500);
        assert_eq!(out, b"\x1b]11;rgb:0/0/0\x07".to_vec());

        // Guard off (window 0): late and stray color replies pass (pre-v9
        // behavior); holder duplicates are still dropped.
        e.expect(Reply::Color(10), 1, 0);
        e.expect(Reply::Da1, 1, 0);
        let input = b"\x1b]10;rgb:1/1/1\x07\x1b[?6c\x1b]11;rgb:2/2/2\x07";
        let (out, s) = e.filter_input_with(input, 4_000, 0);
        assert_eq!(out, b"\x1b]10;rgb:1/1/1\x07\x1b]11;rgb:2/2/2\x07".to_vec());
        assert_eq!(
            s,
            Filtered {
                duplicate: 1,
                late: 0
            }
        );
        // Outstanding queries expire after EXPECT_MS; then a reply is stray.
        e.expect(Reply::Color(10), 1, 0);
        let (_, s) = e.filter_input_with(b"\x1b]10;rgb:1/1/1\x07", EXPECT_MS + 1, 500);
        assert_eq!(s.late, 1);
    }

    #[test]
    fn keys_are_not_replies() {
        let mut e = Expectations::default();
        e.expect(Reply::Da1, 1, 0);
        e.expect(Reply::XCpr, 1, 0);
        // Arrows, F-keys, a modified F3 (`CSI 1;5R`, a Cpr shape but none
        // outstanding), Alt+], an incomplete sequence at the end.
        let keys = b"\x1b[A\x1b[15~\x1b[1;5R\x1bOP\x1b]\x1b[?";
        let (out, n) = f(&mut e, keys, 1);
        assert_eq!((out.as_slice(), n), (&keys[..], 0));
        let (out, n) = f(&mut e, b"\x1b[?3;7;1R", 1);
        assert_eq!((out.as_slice(), n), (&b""[..], 1));
        // A modified F3 long after any CPR query is a key even with the guard on.
        let (out, n) = f(&mut e, b"\x1b[1;5R", 10_000);
        assert_eq!((out.as_slice(), n), (&b"\x1b[1;5R"[..], 0));
        // Color *set* sequences typed/pasted are not replies.
        let (out, n) = f(&mut e, b"\x1b]10;?\x07", 10_000);
        assert_eq!((out.as_slice(), n), (&b"\x1b]10;?\x07"[..], 0));
    }
}
