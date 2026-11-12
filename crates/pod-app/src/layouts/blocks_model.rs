//! The block model of the 블록 터미널 layout (docs/10 §D5.8): terminal output
//! cut into command blocks. Pure (no GPUI): bytes in, blocks out.
//!
//! Boundaries, strongest first:
//! 1. **OSC 133 shell-integration marks** (`A` prompt, `B` input, `C` output,
//!    `D[;exit]` done). Pod's own PowerShell/cmd shells emit them
//!    ([`super::shell_integration`]); other shells may too.
//! 2. **Agent status transitions** (hook / OSC 7501, as podd reports them): a
//!    new block per state (작업 중 → 응답 필요 → 완료).
//! 3. **Prompt detection** for plain shells without marks (`PS C:\…> cmd`,
//!    `C:\…>cmd`, `$ cmd`).
//!
//! Full-screen programs (alternate screen) are never cut: everything between
//! entering and leaving the alternate screen is one live block that shows the
//! current screen, and the raw terminal stays the place to use it.
//!
//! Text comes from a headless VT (the same alacritty engine the terminal view
//! uses) so cursor moves, redraws and ConPTY repaints read as the screen shows
//! them. A closed block keeps its text; only the open block reads the grid.
//!
//! The feed (docs/20-refactor/app.md A-P13) is the pure projection on top:
//! the blocks of a task's terminals as one chronological list with their
//! gates, the header texts, the approval decisions and the keyboard plans.
//! The layout's controller (`blocks/controller.rs`) feeds this model; its view
//! draws what it returns.
use std::collections::HashMap;

use pod_terminal_view::vt::{AlacrittyVt, GridSize, VtEngine};
use podd::protocol::{GateDecision, GateInfo};

use super::blocks::copy as s;
use crate::vm::{AgentState, TaskVm, TerminalVm};

/// Lines of scrollback the headless VT keeps (only the open block needs it).
const VT_SCROLLBACK: usize = 5000;
/// Lines a block keeps (the newest); older ones are counted, not stored.
pub const MAX_BLOCK_LINES: usize = 400;
/// An OSC/CSI longer than this is not a mark: stop carrying it.
const MAX_CARRY: usize = 4096;

/// One OSC 133 mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// `A`: the prompt starts.
    Prompt,
    /// `B`: the prompt ends, the user types the command.
    Input,
    /// `C`: the command runs, output follows.
    Output,
    /// `D[;code]`: the command finished.
    Done(Option<i32>),
}

/// A piece of the output stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    Bytes(Vec<u8>),
    Mark(Mark),
    /// Enter (`true`) or leave the alternate screen; `seq` is the sequence itself.
    Alt {
        on: bool,
        seq: Vec<u8>,
    },
}

/// Finds OSC 133 marks and alternate-screen switches in a byte stream, across
/// chunk boundaries (an unfinished escape sequence is carried to the next chunk).
#[derive(Default)]
pub struct Scanner {
    carry: Vec<u8>,
}

fn parse_mark(body: &[u8]) -> Option<Mark> {
    let rest = body.strip_prefix(b"133;")?;
    let mut parts = rest.split(|&b| b == b';');
    match parts.next()? {
        b"A" => Some(Mark::Prompt),
        b"B" => Some(Mark::Input),
        b"C" => Some(Mark::Output),
        b"D" => Some(Mark::Done(
            parts
                .next()
                .and_then(|c| std::str::from_utf8(c).ok())
                .and_then(|c| c.trim().parse().ok()),
        )),
        _ => None,
    }
}

impl Scanner {
    pub fn split(&mut self, bytes: &[u8]) -> Vec<Piece> {
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut plain = 0; // start of bytes not yet emitted
        let mut i = 0;
        let flush = |out: &mut Vec<Piece>, from: usize, to: usize, buf: &[u8]| {
            if to > from {
                out.push(Piece::Bytes(buf[from..to].to_vec()));
            }
        };
        while i < buf.len() {
            if buf[i] != 0x1b {
                i += 1;
                continue;
            }
            let Some(&kind) = buf.get(i + 1) else {
                // A lone ESC at the end: carry it.
                flush(&mut out, plain, i, &buf);
                self.carry = buf[i..].to_vec();
                return out;
            };
            match kind {
                b']' => {
                    // OSC: up to BEL or ST (ESC \).
                    let mut j = i + 2;
                    let mut end = None;
                    while j < buf.len() {
                        if buf[j] == 0x07 {
                            end = Some((j, j + 1));
                            break;
                        }
                        if buf[j] == 0x1b && buf.get(j + 1) == Some(&b'\\') {
                            end = Some((j, j + 2));
                            break;
                        }
                        if buf[j] == 0x1b && j + 1 == buf.len() {
                            break;
                        }
                        j += 1;
                    }
                    match end {
                        Some((body_end, after)) => {
                            if let Some(m) = parse_mark(&buf[i + 2..body_end]) {
                                flush(&mut out, plain, after, &buf);
                                out.push(Piece::Mark(m));
                                plain = after;
                            }
                            i = after;
                        }
                        None if buf.len() - i < MAX_CARRY => {
                            flush(&mut out, plain, i, &buf);
                            self.carry = buf[i..].to_vec();
                            return out;
                        }
                        None => i = buf.len(),
                    }
                }
                b'[' => {
                    // CSI: parameters, then a final byte 0x40..=0x7e.
                    let mut j = i + 2;
                    while j < buf.len() && !(0x40..=0x7e).contains(&buf[j]) {
                        j += 1;
                    }
                    if j >= buf.len() {
                        if buf.len() - i < MAX_CARRY {
                            flush(&mut out, plain, i, &buf);
                            self.carry = buf[i..].to_vec();
                            return out;
                        }
                        i = buf.len();
                        continue;
                    }
                    let params = &buf[i + 2..j];
                    let fin = buf[j];
                    if (fin == b'h' || fin == b'l')
                        && let Some(p) = params.strip_prefix(b"?")
                        && p.split(|&b| b == b';')
                            .any(|n| matches!(n, b"47" | b"1047" | b"1049"))
                    {
                        flush(&mut out, plain, i, &buf);
                        out.push(Piece::Alt {
                            on: fin == b'h',
                            seq: buf[i..=j].to_vec(),
                        });
                        plain = j + 1;
                    }
                    i = j + 1;
                }
                _ => i += 2,
            }
        }
        flush(&mut out, plain, buf.len(), &buf);
        out
    }
}

/// Where a block's boundaries came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// OSC 133 marks.
    Marks,
    /// Agent status transition.
    Status,
    /// A detected prompt line.
    Prompt,
    /// Output before Pod attached (the first snapshot).
    Earlier,
    /// Output without any boundary yet.
    Plain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A shell command (`title` is the command line).
    Command,
    /// An agent step (`title` is the state label).
    Step,
    /// Output with no command (before the first boundary, earlier output).
    Output,
    /// A full-screen program on the alternate screen (live, never cut).
    Tui,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    /// Unique per terminal, increasing (chronological).
    pub seq: u64,
    pub kind: Kind,
    pub origin: Origin,
    pub title: String,
    /// The newest [`MAX_BLOCK_LINES`] lines; `dropped` older ones were not kept.
    pub lines: Vec<String>,
    pub dropped: usize,
    pub started_ms: Option<u64>,
    pub ended_ms: Option<u64>,
    pub exit: Option<i32>,
    /// Still receiving output.
    pub open: bool,
}

impl Block {
    pub fn duration_ms(&self) -> Option<u64> {
        Some(self.ended_ms?.saturating_sub(self.started_ms?))
    }
    pub fn failed(&self) -> bool {
        self.exit.is_some_and(|c| c != 0)
    }
    /// Title and body as plain text (copy).
    pub fn text(&self) -> String {
        let mut s = String::new();
        if !self.title.is_empty() && self.kind == Kind::Command {
            s.push_str(&self.title);
            s.push('\n');
        }
        for l in &self.lines {
            s.push_str(l);
            s.push('\n');
        }
        s
    }
}

/// A prompt line: `Some(command)` (possibly empty) when `line` looks like a
/// shell prompt. Used only for terminals without marks or agent status.
pub fn prompt_command(line: &str) -> Option<&str> {
    let line = line.trim_end();
    // PowerShell: `PS C:\path> cmd`
    if let Some(rest) = line.strip_prefix("PS ")
        && let Some(i) = rest.find("> ")
        && rest[..i].contains(['\\', '/', ':'])
    {
        return Some(rest[i + 2..].trim());
    }
    if line.starts_with("PS ") && line.ends_with('>') && line.contains(['\\', ':']) {
        return Some("");
    }
    // cmd: `C:\path>cmd`
    let b = line.as_bytes();
    if b.len() >= 3
        && b[0].is_ascii_alphabetic()
        && b[1] == b':'
        && b[2] == b'\\'
        && let Some(i) = line.find('>')
    {
        return Some(line[i + 1..].trim());
    }
    // POSIX-ish: `$ cmd`
    if let Some(rest) = line.strip_prefix("$ ") {
        return Some(rest.trim());
    }
    if line == "$" {
        return Some("");
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Output (or nothing) since the last boundary.
    Idle,
    /// After `A`: the prompt is being drawn.
    Prompt,
    /// After `B`: the user is typing (abs line, column).
    Input(i64, usize),
    /// After `C`: the command runs.
    Running,
}

/// One terminal's blocks.
pub struct TermBlocks {
    vt: AlacrittyVt,
    scan: Scanner,
    pub blocks: Vec<Block>,
    next_seq: u64,
    /// OSC 133 marks were seen: they alone cut blocks from now on.
    pub marks: bool,
    /// Agent status is reported for this terminal: transitions cut blocks.
    pub agent: bool,
    phase: Phase,
    pub alt: bool,
    /// Absolute line where the open block's body starts.
    open_from: i64,
    /// Prompt detection: the next absolute line to look at.
    scan_from: i64,
    /// The first snapshot was read.
    pub started: bool,
}

impl TermBlocks {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            vt: AlacrittyVt::new(
                GridSize {
                    cols: cols.max(2),
                    rows: rows.max(2),
                },
                VT_SCROLLBACK,
            ),
            scan: Scanner::default(),
            blocks: Vec::new(),
            next_seq: 0,
            marks: false,
            agent: false,
            phase: Phase::Idle,
            alt: false,
            open_from: 0,
            scan_from: 0,
            started: false,
        }
    }

    fn history(&self) -> i64 {
        self.vt.history_size() as i64
    }

    /// Cursor as (absolute line, column).
    fn cursor(&mut self) -> (i64, usize) {
        self.vt.flush_sync();
        let (col, row) = self.vt.snapshot().cursor_point.unwrap_or((0, 0));
        (self.history() + row as i64, col as usize)
    }

    /// Text of absolute line `abs` (soft-wrapped continuation joined by the caller).
    fn row(&self, abs: i64) -> Option<(String, bool)> {
        let r = self.vt.row_text((abs - self.history()) as i32)?;
        let s: String = r.cells.iter().filter(|c| **c != '\0').collect();
        Some((s, r.wrapped))
    }

    /// Logical lines from `from` (inclusive) to `to` (exclusive), wrapped rows
    /// joined, trailing spaces and trailing empty lines dropped.
    fn lines(&self, from: i64, to: i64) -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut cont = false;
        for abs in from.max(-0).max(0)..to {
            let Some((s, wrapped)) = self.row(abs) else {
                continue;
            };
            if cont {
                cur.push_str(&s);
            } else {
                cur = s;
            }
            cont = wrapped;
            if !wrapped {
                out.push(std::mem::take(&mut cur).trim_end().to_string());
            }
        }
        if cont {
            out.push(cur.trim_end().to_string());
        }
        while out.last().is_some_and(|l| l.is_empty()) {
            out.pop();
        }
        while out.first().is_some_and(|l| l.is_empty()) {
            out.remove(0);
        }
        out
    }

    fn push(&mut self, kind: Kind, origin: Origin, title: String, now: Option<u64>) {
        self.blocks.push(Block {
            seq: self.next_seq,
            kind,
            origin,
            title,
            lines: Vec::new(),
            dropped: 0,
            started_ms: now,
            ended_ms: None,
            exit: None,
            open: true,
        });
        self.next_seq += 1;
    }

    fn set_lines(b: &mut Block, mut lines: Vec<String>) {
        let extra = lines.len().saturating_sub(MAX_BLOCK_LINES);
        if extra > 0 {
            lines.drain(..extra);
        }
        b.dropped = extra;
        b.lines = lines;
    }

    /// Close the open block with the text up to (not including) `end`.
    fn close_at(&mut self, end: i64, now: Option<u64>, exit: Option<i32>) {
        let from = self.open_from;
        let lines = if self.alt {
            None
        } else {
            Some(self.lines(from, end))
        };
        if let Some(b) = self.blocks.last_mut().filter(|b| b.open) {
            if let Some(l) = lines {
                Self::set_lines(b, l);
            }
            b.open = false;
            b.ended_ms = b.ended_ms.or(now);
            b.exit = exit.or(b.exit);
        }
        // An untitled output block that stayed empty is not worth a block.
        if self
            .blocks
            .last()
            .is_some_and(|b| !b.open && b.kind == Kind::Output && b.lines.is_empty())
        {
            self.blocks.pop();
        }
        // What follows starts after the closed block (never re-read its lines).
        if !self.alt {
            self.open_from = self.open_from.max(end);
            self.scan_from = self.scan_from.max(end);
        }
    }

    /// The open block's body starts at the cursor's line (or the next one
    /// when the cursor is past the start of its line).
    fn open_here(&mut self) {
        let (line, col) = self.cursor();
        self.open_from = if col == 0 { line } else { line + 1 };
        self.scan_from = self.open_from;
    }

    /// Re-read the open block's lines (after new output).
    fn refresh_open(&mut self) {
        let (line, col) = self.cursor();
        if self.open_from > line + 1 {
            // The screen was cleared: start over at its top.
            self.open_from = self.history().max(0);
        }
        let end = if col == 0 && !self.alt {
            line
        } else {
            line + 1
        };
        let lines = if self.alt {
            self.screen()
        } else {
            self.lines(self.open_from, end)
        };
        if let Some(b) = self.blocks.last_mut().filter(|b| b.open) {
            Self::set_lines(b, lines);
        }
    }

    /// The visible screen (alternate screen preview).
    fn screen(&self) -> Vec<String> {
        let rows = self.vt.size().rows as i64;
        let h = self.history();
        self.lines(h, h + rows)
    }

    /// Start over from a (re)attach snapshot. The first one becomes the
    /// "earlier output" block(s); later ones only reset the VT.
    pub fn reset(&mut self, snapshot: &[u8], cols: u16, rows: u16, now: u64) {
        self.vt = AlacrittyVt::new(
            GridSize {
                cols: cols.max(2),
                rows: rows.max(2),
            },
            VT_SCROLLBACK,
        );
        self.scan = Scanner::default();
        self.alt = false;
        // Pieces, so an alternate screen in the snapshot is seen too.
        for p in self.scan.split(snapshot) {
            match p {
                Piece::Bytes(b) => self.vt.feed(&b),
                Piece::Mark(_) => {}
                Piece::Alt { on, seq } => {
                    self.vt.feed(&seq);
                    self.alt = on;
                }
            }
        }
        let _ = self.vt.take_events();
        let (line, col) = self.cursor();
        let end = if col == 0 { line } else { line + 1 };
        if !self.started {
            self.started = true;
            if !self.alt {
                let lines = self.lines(0, end);
                self.earlier(lines);
            }
        } else if let Some(b) = self.blocks.last_mut().filter(|b| b.open) {
            // Re-attach: the open block keeps the text it had.
            b.open = self.alt && b.kind == Kind::Tui;
        }
        self.phase = Phase::Idle;
        if self.alt {
            if !self
                .blocks
                .last()
                .is_some_and(|b| b.open && b.kind == Kind::Tui)
            {
                self.push(Kind::Tui, Origin::Plain, String::new(), Some(now));
            }
            self.refresh_open();
        } else {
            self.open_from = end;
            // The prompt line under the cursor is complete once Enter comes: look at it then.
            self.scan_from = line;
        }
    }

    /// Earlier output, cut at prompt lines when it has any.
    fn earlier(&mut self, lines: Vec<String>) {
        let mut cur: Option<Block> = None;
        let mut out: Vec<Block> = Vec::new();
        for l in lines {
            if let Some(cmd) = prompt_command(&l).filter(|c| !c.is_empty()) {
                out.extend(cur.take());
                cur = Some(Block {
                    seq: 0,
                    kind: Kind::Command,
                    origin: Origin::Earlier,
                    title: cmd.to_string(),
                    lines: Vec::new(),
                    dropped: 0,
                    started_ms: None,
                    ended_ms: None,
                    exit: None,
                    open: false,
                });
                continue;
            }
            if prompt_command(&l).is_some() {
                continue;
            }
            cur.get_or_insert_with(|| Block {
                seq: 0,
                kind: Kind::Output,
                origin: Origin::Earlier,
                title: String::new(),
                lines: Vec::new(),
                dropped: 0,
                started_ms: None,
                ended_ms: None,
                exit: None,
                open: false,
            })
            .lines
            .push(l);
        }
        out.extend(cur);
        for mut b in out {
            while b.lines.last().is_some_and(|l| l.is_empty()) {
                b.lines.pop();
            }
            if b.kind == Kind::Output && b.lines.is_empty() {
                continue;
            }
            let lines = std::mem::take(&mut b.lines);
            Self::set_lines(&mut b, lines);
            b.seq = self.next_seq;
            self.next_seq += 1;
            self.blocks.push(b);
        }
    }

    /// New output.
    pub fn feed(&mut self, bytes: &[u8], now: u64) {
        for p in self.scan.split(bytes) {
            match p {
                Piece::Bytes(b) => self.vt.feed(&b),
                Piece::Mark(m) => self.mark(m, now),
                Piece::Alt { on, seq } => self.alt_switch(on, &seq, now),
            }
        }
        let _ = self.vt.take_events();
        if !self.marks && !self.agent && !self.alt {
            self.detect_prompts(now);
        }
        if self.blocks.last().is_none_or(|b| !b.open)
            && !self.alt
            && self.phase != Phase::Prompt
            && !matches!(self.phase, Phase::Input(..))
        {
            // Output with no open block: an untitled one (dropped if it stays empty).
            let (line, col) = self.cursor();
            let has_text = (self.open_from..=line)
                .any(|a| self.row(a).is_some_and(|(s, _)| !s.trim().is_empty()))
                && (line > self.open_from || col > 0);
            if has_text {
                self.push(Kind::Output, Origin::Plain, String::new(), Some(now));
            }
        }
        if self.blocks.last().is_some_and(|b| b.open) {
            self.refresh_open();
        }
    }

    fn mark(&mut self, m: Mark, now: u64) {
        if self.alt {
            return;
        }
        self.marks = true;
        match m {
            Mark::Prompt => {
                let (line, _) = self.cursor();
                self.close_at(line, Some(now), None);
                self.phase = Phase::Prompt;
            }
            Mark::Input => {
                let (line, col) = self.cursor();
                self.phase = Phase::Input(line, col);
            }
            Mark::Output => {
                let cmd = match self.phase {
                    Phase::Input(l, c) => self.input_text(l, c),
                    _ => String::new(),
                };
                if self.blocks.last().is_some_and(|b| b.open) {
                    let (line, _) = self.cursor();
                    self.close_at(line, Some(now), None);
                }
                self.push(Kind::Command, Origin::Marks, cmd, Some(now));
                self.open_here();
                self.phase = Phase::Running;
            }
            Mark::Done(code) => match self.phase {
                Phase::Running => {
                    let (line, col) = self.cursor();
                    let end = if col == 0 { line } else { line + 1 };
                    self.close_at(end, Some(now), code);
                    self.phase = Phase::Idle;
                }
                // No `C` (cmd.exe): the command was the input line, its output the lines after it.
                Phase::Input(l, c) => {
                    let cmd = self.input_line(l, c);
                    if !cmd.is_empty() {
                        let (line, col) = self.cursor();
                        let end = if col == 0 { line } else { line + 1 };
                        let lines = self.lines(l + 1, end);
                        let mut b = Block {
                            seq: self.next_seq,
                            kind: Kind::Command,
                            origin: Origin::Marks,
                            title: cmd,
                            lines: Vec::new(),
                            dropped: 0,
                            started_ms: None,
                            ended_ms: Some(now),
                            exit: code,
                            open: false,
                        };
                        Self::set_lines(&mut b, lines);
                        self.next_seq += 1;
                        self.blocks.push(b);
                    }
                    self.phase = Phase::Idle;
                }
                _ => self.phase = Phase::Idle,
            },
        }
    }

    /// The command line typed after `B`: from (line, col) to the cursor.
    fn input_text(&mut self, line: i64, col: usize) -> String {
        let (cl, cc) = self.cursor();
        let end = if cc == 0 && cl > line { cl } else { cl + 1 };
        let mut text = String::new();
        for abs in line..end {
            let Some((s, _)) = self.row(abs) else {
                continue;
            };
            let s: String = if abs == line {
                s.chars().skip(col).collect()
            } else {
                s
            };
            text.push_str(s.trim_end());
        }
        text.trim().to_string()
    }

    /// The input line typed after `B` (and its soft-wrapped continuation).
    fn input_line(&self, line: i64, col: usize) -> String {
        let mut text = String::new();
        let mut abs = line;
        while let Some((s, wrapped)) = self.row(abs) {
            let s: String = if abs == line {
                s.chars().skip(col).collect()
            } else {
                s
            };
            text.push_str(if wrapped { &s } else { s.trim_end() });
            if !wrapped {
                break;
            }
            abs += 1;
        }
        text.trim().to_string()
    }

    fn alt_switch(&mut self, on: bool, seq: &[u8], now: u64) {
        if on == self.alt {
            self.vt.feed(seq);
            return;
        }
        if on {
            // The main screen's output so far closes; the program is one live block.
            let (line, col) = self.cursor();
            let end = if col == 0 { line } else { line + 1 };
            if self.blocks.last().is_some_and(|b| b.open) {
                if self.phase == Phase::Running {
                    // A command that opened a full-screen program: keep its block open
                    // (its end comes with `D`), show the program in a block of its own.
                    let lines = self.lines(self.open_from, end);
                    if let Some(b) = self.blocks.last_mut() {
                        Self::set_lines(b, lines);
                    }
                }
                if self.phase != Phase::Running {
                    self.close_at(end, Some(now), None);
                }
            }
            let title = self
                .blocks
                .last()
                .filter(|b| b.open && b.kind == Kind::Command)
                .map(|b| b.title.clone())
                .unwrap_or_default();
            if let Some(b) = self.blocks.last_mut().filter(|b| b.open) {
                b.open = false;
            }
            self.vt.feed(seq);
            self.alt = true;
            self.push(Kind::Tui, Origin::Plain, title, Some(now));
            self.refresh_open();
        } else {
            // Keep the program's last screen as the block's text.
            if let Some(b) = self
                .blocks
                .last_mut()
                .filter(|b| b.open && b.kind == Kind::Tui)
            {
                b.open = false;
                b.ended_ms = Some(now);
            }
            self.vt.feed(seq);
            self.alt = false;
            self.open_here();
            if self.phase == Phase::Running {
                // The command goes on after the program: its output continues in a new block.
                let title = self
                    .blocks
                    .iter()
                    .rev()
                    .find(|b| b.kind == Kind::Command)
                    .map(|b| b.title.clone())
                    .unwrap_or_default();
                self.push(Kind::Command, Origin::Marks, title, Some(now));
            }
        }
    }

    /// Plain shells: a line that looks like a prompt with a command starts a block.
    fn detect_prompts(&mut self, now: u64) {
        let (line, _) = self.cursor();
        let mut abs = self.scan_from.max(0);
        while abs < line {
            if let Some((s, _)) = self.row(abs) {
                match prompt_command(&s) {
                    Some(cmd) if !cmd.is_empty() => {
                        if self.blocks.last().is_some_and(|b| b.open) {
                            self.close_at(abs, Some(now), None);
                        }
                        self.push(Kind::Command, Origin::Prompt, cmd.to_string(), Some(now));
                        self.open_from = abs + 1;
                    }
                    Some(_) => {
                        // An empty prompt line: what ran before it is done.
                        if self.blocks.last().is_some_and(|b| b.open) {
                            self.close_at(abs, Some(now), None);
                        }
                        self.open_from = abs + 1;
                    }
                    None => {}
                }
            }
            abs += 1;
        }
        self.scan_from = line;
        // The prompt is back on the cursor line: the running command finished.
        if let Some((s, _)) = self.row(line)
            && prompt_command(&s).is_some()
            && self
                .blocks
                .last()
                .is_some_and(|b| b.open && b.kind == Kind::Command)
        {
            self.close_at(line, Some(now), None);
            self.open_from = line;
        }
    }

    /// The agent's reported state changed: the open block ends, a new step starts.
    /// Ignored once OSC 133 marks were seen and on the alternate screen.
    pub fn status(&mut self, label: &str, finished: bool, now: u64) {
        self.agent = true;
        if self.marks || self.alt {
            return;
        }
        let (line, col) = self.cursor();
        let end = if col == 0 { line } else { line + 1 };
        if self.blocks.last().is_some_and(|b| b.open) {
            self.close_at(end, Some(now), None);
        }
        self.push(Kind::Step, Origin::Status, label.to_string(), Some(now));
        self.open_from = end;
        if finished && let Some(b) = self.blocks.last_mut() {
            // A finished state is a point in time; later output opens a new block.
            b.open = false;
            b.ended_ms = Some(now);
        }
    }

    /// The process ended.
    pub fn exited(&mut self, code: Option<i32>, now: u64) {
        let (line, col) = self.cursor();
        let end = if col == 0 { line } else { line + 1 };
        if self.alt {
            if let Some(b) = self.blocks.last_mut().filter(|b| b.open) {
                b.open = false;
                b.ended_ms = Some(now);
            }
            return;
        }
        if self.blocks.last().is_some_and(|b| b.open) {
            let exit = if self.phase == Phase::Running {
                None
            } else {
                code
            };
            self.close_at(end, Some(now), exit);
        }
        self.phase = Phase::Idle;
    }
}

/// `1분 12초`, `3.1초`, `42초`.
pub fn duration_text(ms: u64) -> String {
    if ms < 10_000 {
        format!("{:.1}초", ms as f64 / 1000.)
    } else if ms < 60_000 {
        format!("{}초", ms / 1000)
    } else if ms < 3_600_000 {
        format!("{}분 {}초", ms / 60_000, (ms % 60_000) / 1000)
    } else {
        format!("{}시간 {}분", ms / 3_600_000, (ms % 3_600_000) / 60_000)
    }
}

// ───────────────────────────── feed ─────────────────────────────

/// A block's identity in the feed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockKey {
    pub terminal: String,
    pub seq: u64,
}

impl BlockKey {
    /// The block's part of element and QA ids.
    pub(super) fn dom(&self) -> String {
        format!("{}-{}", self.terminal, self.seq)
    }
}

/// A block as the feed shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedBlock {
    pub key: BlockKey,
    /// `Claude · 구현`, `셸 · 빌드`.
    pub who: String,
    /// The terminal's agent (docs/10 §D5.11): the mark before `who`.
    pub kind: crate::agent_kind::AgentKind,
    pub block: Block,
    /// A pending gate of this block's terminal (on its newest block only).
    pub gate: Option<GateInfo>,
    /// The newest block of its terminal.
    pub last: bool,
}

const SHELLS: [&str; 6] = ["powershell", "pwsh", "cmd", "bash", "sh", "zsh"];

pub fn is_shell(program: &str) -> bool {
    SHELLS.contains(&program.to_ascii_lowercase().trim_end_matches(".exe"))
}

/// `Claude · 이름` for agents, `셸 · 이름` for shells.
pub fn who(t: &TerminalVm, label: &str) -> String {
    // The identified agent (hook/OSC/argv, docs/10 §D5.11) first, then the program name.
    let kind = crate::agent_kind::identify(t, None).kind;
    let agent = if kind.is_agent() {
        kind.name()
    } else if is_shell(&t.program) {
        s::SHELL.to_string()
    } else {
        let mut c = t.program.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    };
    format!("{agent} · {label}")
}

/// Step title for an agent state.
pub fn state_label(st: AgentState) -> (&'static str, bool) {
    match st {
        AgentState::Working => (s::STEP_WORKING, false),
        AgentState::Blocked => (s::NEEDS_YOU, false),
        AgentState::Done => (s::DONE, true),
        AgentState::Idle => (s::STEP_IDLE, true),
        AgentState::Error => (s::STEP_ERROR, true),
        AgentState::Unknown => (s::STEP_UNKNOWN, false),
        AgentState::Exited => (s::STEP_EXITED, true),
    }
}

/// The terminals of `task` in the task's order (then any others that name it).
pub fn task_terminals<'a>(task: &TaskVm, terminals: &'a [TerminalVm]) -> Vec<&'a TerminalVm> {
    let mut out: Vec<&TerminalVm> = task
        .terminals
        .iter()
        .filter_map(|id| terminals.iter().find(|t| &t.id == id))
        .collect();
    for t in terminals
        .iter()
        .filter(|t| t.task_id.as_deref() == Some(task.id.as_str()))
    {
        if !out.iter().any(|x| x.id == t.id) {
            out.push(t);
        }
    }
    out
}

/// The pending gate per terminal: a terminal's own gate, else the task's on its first terminal.
pub fn gates_by_terminal(
    terms: &[&TerminalVm],
    task: &str,
    pending: &[GateInfo],
) -> HashMap<String, GateInfo> {
    let mut out = HashMap::new();
    for g in pending {
        if let Some(t) = g
            .terminal
            .as_deref()
            .filter(|t| terms.iter().any(|x| x.id == *t))
        {
            out.entry(t.to_string()).or_insert_with(|| g.clone());
        } else if g.terminal.is_none()
            && g.task.as_deref() == Some(task)
            && let Some(first) = terms.first()
        {
            out.entry(first.id.clone()).or_insert_with(|| g.clone());
        }
    }
    out
}

/// The feed: every block of the task's terminals (or one terminal), oldest first.
/// A terminal with a pending gate and no block yet gets an approval block.
pub fn build_feed(
    terms: &[&TerminalVm],
    blocks: &HashMap<String, Vec<Block>>,
    gates: &HashMap<String, GateInfo>,
    filter: Option<&str>,
    label: impl Fn(&TerminalVm) -> String,
) -> Vec<FeedBlock> {
    let mut out: Vec<(u64, usize, FeedBlock)> = Vec::new();
    for (ti, t) in terms
        .iter()
        .enumerate()
        .filter(|(_, t)| filter.is_none_or(|f| f == t.id))
    {
        let who = who(t, &label(t));
        let kind = crate::agent_kind::identify(t, None).kind;
        let empty = Vec::new();
        let list = blocks.get(&t.id).unwrap_or(&empty);
        let gate = gates.get(&t.id).cloned();
        let n = list.len();
        for (i, b) in list.iter().enumerate() {
            let last = i + 1 == n;
            out.push((
                b.started_ms.or(b.ended_ms).unwrap_or(0),
                ti,
                FeedBlock {
                    key: BlockKey {
                        terminal: t.id.clone(),
                        seq: b.seq,
                    },
                    who: who.clone(),
                    kind: kind.clone(),
                    block: b.clone(),
                    gate: gate.clone().filter(|_| last),
                    last,
                },
            ));
        }
        if n == 0
            && let Some(g) = gate
        {
            let b = Block {
                seq: u64::MAX,
                kind: Kind::Step,
                origin: Origin::Status,
                title: s::APPROVAL.into(),
                lines: Vec::new(),
                dropped: 0,
                started_ms: Some(g.created_ms),
                ended_ms: None,
                exit: None,
                open: true,
            };
            out.push((
                g.created_ms,
                ti,
                FeedBlock {
                    key: BlockKey {
                        terminal: t.id.clone(),
                        seq: u64::MAX,
                    },
                    who: who.clone(),
                    kind: kind.clone(),
                    block: b,
                    gate: Some(g),
                    last: true,
                },
            ));
        }
    }
    out.sort_by_key(|(at, ti, f)| (*at, *ti, f.block.seq));
    out.into_iter().map(|(_, _, f)| f).collect()
}

/// Expanded unless the user folded it: running, waiting for approval, failed,
/// or the newest block of its terminal. Other finished blocks start folded.
pub fn default_expanded(f: &FeedBlock) -> bool {
    f.block.open || f.gate.is_some() || f.block.failed() || f.last
}

/// The header's status text.
pub fn status_text(f: &FeedBlock) -> String {
    let b = &f.block;
    if f.gate.is_some() {
        return s::WAITING.into();
    }
    if b.kind == Kind::Tui {
        return if b.open { s::TUI_LIVE } else { s::TUI_DONE }.into();
    }
    if b.open {
        return s::RUNNING.into();
    }
    if b.origin == Origin::Earlier {
        return s::EARLIER.into();
    }
    let tail = match (b.kind, b.exit) {
        (_, Some(c)) => s::exit_code(c),
        (Kind::Command, None) => s::ENDED.into(),
        _ => s::DONE.into(),
    };
    match b.duration_ms() {
        Some(ms) => format!("{} · {tail}", duration_text(ms)),
        None => tail,
    }
}

/// What the header names the block by.
pub fn title_of(f: &FeedBlock) -> String {
    match f.block.kind {
        _ if !f.block.title.is_empty() => f.block.title.clone(),
        Kind::Tui => s::TUI_LIVE.into(),
        _ if f.block.origin == Origin::Earlier => s::EARLIER.into(),
        _ => s::OUTPUT.into(),
    }
}

/// The block's accessible name.
pub fn block_name(f: &FeedBlock, expanded: bool) -> String {
    let mut name = format!("{}, {}, {}", f.who, title_of(f), status_text(f));
    if let Some(g) = &f.gate {
        name.push_str(&format!(", {}: {}", s::APPROVAL, g.message));
    }
    name.push_str(if expanded { s::EXPANDED } else { s::COLLAPSED });
    name
}

/// The three approval buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    Once,
    Always,
    Reject,
}

fn is_always(o: &str) -> bool {
    let o = o.to_lowercase();
    o.contains("always") || o.contains("항상") || o.contains("don't ask") || o.contains("다시 묻지")
}

/// The `gate.resolve` decision (and option) for a button; `None`: the gate
/// does not offer it (항상 허용 without an "always" option).
pub fn decision(g: &GateInfo, a: Approval) -> Option<(GateDecision, Option<String>)> {
    match a {
        Approval::Reject => Some((GateDecision::Reject, None)),
        Approval::Once if g.options.is_empty() => Some((GateDecision::Approve, None)),
        Approval::Once => {
            let once = g.options.iter().find(|o| {
                let l = o.to_lowercase();
                !is_always(o)
                    && (l.contains("once")
                        || l.contains("이번")
                        || l.contains("allow")
                        || l.contains("허용")
                        || l.contains("yes"))
            });
            once.or_else(|| g.options.iter().find(|o| !is_always(o)))
                .map(|o| (GateDecision::Approve, Some(o.clone())))
        }
        Approval::Always => g
            .options
            .iter()
            .find(|o| is_always(o))
            .map(|o| (GateDecision::Approve, Some(o.clone()))),
    }
}

/// Keyboard movement in the feed.
pub fn nav(feed: &[FeedBlock], from: Option<&BlockKey>, key: &str) -> Option<BlockKey> {
    let i = from.and_then(|k| feed.iter().position(|f| &f.key == k));
    let j = match (key, i) {
        (_, None) => feed.len().checked_sub(1),
        ("up", Some(i)) => i.checked_sub(1),
        ("down", Some(i)) => (i + 1 < feed.len()).then_some(i + 1),
        ("home", _) => (!feed.is_empty()).then_some(0),
        ("end", _) => feed.len().checked_sub(1),
        _ => None,
    }?;
    Some(feed[j].key.clone())
}

/// Next input target among the running terminals (Tab / Shift+Tab).
pub fn cycle_target(terms: &[&TerminalVm], current: Option<&str>, back: bool) -> Option<String> {
    let live: Vec<&str> = terms
        .iter()
        .filter(|t| t.is_running())
        .map(|t| t.id.as_str())
        .collect();
    if live.is_empty() {
        return None;
    }
    let i = current.and_then(|c| live.iter().position(|x| *x == c));
    let j = match (i, back) {
        (None, _) => 0,
        (Some(i), false) => (i + 1) % live.len(),
        (Some(i), true) => (i + live.len() - 1) % live.len(),
    };
    Some(live[j].to_string())
}

// ───────────────────────────── panel drawer ─────────────────────────────

/// A panel was opened while the layout shows (settings, gates, a Review …):
/// the count of the dock's panels other than terminals rose since the last
/// frame (`usize::MAX`: no frame yet, nothing to compare). The drawer opens.
pub fn panels_rose(last: usize, now: usize) -> bool {
    now > last && last != usize::MAX
}

/// Width of the panel drawer at the right, in logical px: about 42 % of the
/// window, 360–720 px, but the task list and at least 360 px of feed stay
/// beside it (280 px of drawer at the very least).
///
/// `side` is what the dock's open side docks keep for themselves, plus the
/// drawer's border (the gates inbox: 380 px at the right). The dock gives
/// them that width and its centre only the rest, so a narrower drawer cut
/// them off on the right; the drawer grows to hold them while the feed keeps
/// [`FEED_BESIDE_DOCKS`]. A window too narrow even for that scrolls the dock
/// sideways in the drawer.
pub fn drawer_width(window: f32, sidebar: f32, side: f32) -> f32 {
    let room = (window - sidebar - 360.).max(280.);
    let base = (window * 0.42).clamp(360., 720.).min(room);
    base.max(side.min(window - sidebar - FEED_BESIDE_DOCKS))
}

/// Feed width kept beside a drawer that grew for the dock's side docks.
pub const FEED_BESIDE_DOCKS: f32 = 280.;

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    const A: &str = "\x1b]133;A\x07";
    const B: &str = "\x1b]133;B\x07";
    const C: &str = "\x1b]133;C\x07";
    fn d(code: i32) -> String {
        format!("\x1b]133;D;{code}\x07")
    }

    fn titles(t: &TermBlocks) -> Vec<(Kind, &str, bool)> {
        t.blocks
            .iter()
            .map(|b| (b.kind, b.title.as_str(), b.open))
            .collect()
    }

    #[test]
    fn scanner_finds_marks_and_alt_screen_across_chunks() {
        let mut s = Scanner::default();
        let mut pieces = s.split(b"ab\x1b]13");
        assert_eq!(
            pieces,
            vec![Piece::Bytes(b"ab".to_vec())],
            "unfinished OSC is carried"
        );
        pieces = s.split(b"3;D;2\x1b\\cd\x1b[?1049h\x1b[2J\x1b]0;title\x07");
        assert_eq!(pieces[0], Piece::Bytes(b"\x1b]133;D;2\x1b\\".to_vec()));
        assert_eq!(pieces[1], Piece::Mark(Mark::Done(Some(2))));
        assert_eq!(pieces[2], Piece::Bytes(b"cd".to_vec()));
        assert_eq!(
            pieces[3],
            Piece::Alt {
                on: true,
                seq: b"\x1b[?1049h".to_vec()
            }
        );
        assert_eq!(
            pieces[4],
            Piece::Bytes(b"\x1b[2J\x1b]0;title\x07".to_vec()),
            "other sequences pass through"
        );
        assert_eq!(
            s.split(b"\x1b[?25l\x1b[?1049;1l"),
            vec![
                Piece::Bytes(b"\x1b[?25l".to_vec()),
                Piece::Alt {
                    on: false,
                    seq: b"\x1b[?1049;1l".to_vec()
                }
            ]
        );
        assert_eq!(s.split(b"x\x1b"), vec![Piece::Bytes(b"x".to_vec())]);
        assert_eq!(
            s.split(b"]133;A\x07"),
            vec![
                Piece::Bytes(b"\x1b]133;A\x07".to_vec()),
                Piece::Mark(Mark::Prompt)
            ]
        );
    }

    #[test]
    fn osc_133_marks_cut_command_blocks_with_exit_codes() {
        let mut t = TermBlocks::new(80, 24);
        t.reset(b"", 80, 24, 0);
        let first = format!(
            "{A}PS C:\\src> {B}git status --short\r\n{C} M src/a.rs\r\n?? tests/b.rs\r\n{}{A}PS C:\\src> {B}",
            d(0)
        );
        t.feed(first.as_bytes(), 1000);
        assert_eq!(
            titles(&t),
            vec![(Kind::Command, "git status --short", false)]
        );
        assert_eq!(t.blocks[0].lines, [" M src/a.rs", "?? tests/b.rs"]);
        assert_eq!(t.blocks[0].exit, Some(0));
        // A failing command, output arriving in pieces.
        t.feed(b"cargo test\r\n", 2000);
        t.feed(C.as_bytes(), 2000);
        t.feed("error[E0599]: 한글 오류\r\n".as_bytes(), 2500);
        assert_eq!(
            titles(&t)[1],
            (Kind::Command, "cargo test", true),
            "running"
        );
        assert_eq!(
            t.blocks[1].lines,
            ["error[E0599]: 한글 오류"],
            "live text while running"
        );
        t.feed(format!("{}{A}PS C:\\src> {B}", d(101)).as_bytes(), 5100);
        let b = &t.blocks[1];
        assert_eq!(
            (b.open, b.exit, b.duration_ms(), b.failed()),
            (false, Some(101), Some(3100), true)
        );
        assert!(t.marks);
        // Regression: after `D` with no new prompt yet, the closed output is not read again as a new block.
        t.feed(
            format!("{A}PS C:\\src> {B}git log -1\r\n{C}commit abc\r\n{}", d(0)).as_bytes(),
            5200,
        );
        assert_eq!(t.blocks.len(), 3, "{:?}", titles(&t));
        assert_eq!(t.blocks[2].lines, ["commit abc"]);
        t.feed(format!("{A}PS C:\\src> {B}").as_bytes(), 5300);
        assert_eq!(t.blocks.len(), 3, "{:?}", titles(&t));
        // An empty Enter (no C) makes no block.
        t.feed(format!("\r\n{}{A}PS C:\\src> {B}", d(0)).as_bytes(), 6000);
        assert_eq!(t.blocks.len(), 3);
    }

    #[test]
    fn cmd_prompt_without_c_mark_still_makes_blocks() {
        let mut t = TermBlocks::new(80, 24);
        t.reset(b"", 80, 24, 0);
        t.feed(
            format!("{A}C:\\src>{B}dir /b\r\na.txt\r\nb.txt\r\n\x1b]133;D\x1b\\{A}C:\\src>{B}")
                .as_bytes(),
            10,
        );
        assert_eq!(titles(&t), vec![(Kind::Command, "dir /b", false)]);
        assert_eq!(t.blocks[0].lines, ["a.txt", "b.txt"]);
    }

    #[test]
    fn full_screen_programs_are_one_live_block_and_never_cut() {
        let mut t = TermBlocks::new(40, 6);
        t.reset(b"", 40, 6, 0);
        t.feed(format!("{A}$ {B}vim notes.md\r\n{C}").as_bytes(), 0);
        t.feed(b"\x1b[?1049h\x1b[H\x1b[2Jline one\r\n~\r\n~", 100);
        assert!(t.alt);
        let tui = t.blocks.last().unwrap();
        assert_eq!(
            (tui.kind, tui.title.as_str(), tui.open),
            (Kind::Tui, "vim notes.md", true)
        );
        assert_eq!(
            tui.lines[0], "line one",
            "the live screen is the block body"
        );
        // Marks inside the program (a nested shell) do not cut it.
        t.feed(format!("{A}{B}{C}").as_bytes(), 150);
        assert_eq!(t.blocks.iter().filter(|b| b.kind == Kind::Tui).count(), 1);
        t.feed(b"\x1b[H\x1b[2Jsecond screen", 200);
        assert_eq!(t.blocks.last().unwrap().lines, ["second screen"]);
        // Leaving: the block keeps the last screen, the command ends with D.
        t.feed(format!("\x1b[?1049l{}{A}$ {B}", d(0)).as_bytes(), 300);
        assert!(!t.alt);
        let tui = t.blocks.iter().find(|b| b.kind == Kind::Tui).unwrap();
        assert_eq!(
            (tui.open, tui.lines.clone()),
            (false, vec!["second screen".to_string()])
        );
        assert!(t.blocks.iter().all(|b| !b.open));
    }

    #[test]
    fn agent_status_transitions_cut_steps_without_marks() {
        let mut t = TermBlocks::new(60, 10);
        t.reset("이전 출력 한 줄\r\n".as_bytes(), 60, 10, 0);
        assert_eq!(
            titles(&t),
            vec![(Kind::Output, "", false)],
            "the snapshot is earlier output"
        );
        assert_eq!(t.blocks[0].origin, Origin::Earlier);
        t.status("작업 중", false, 1000);
        t.feed(
            "• 수정 crates/pod-ui/tests/wide_glyph.rs\r\n  + #[test] fn hangul()\r\n".as_bytes(),
            1500,
        );
        assert_eq!(
            t.blocks[1].lines,
            [
                "• 수정 crates/pod-ui/tests/wide_glyph.rs",
                "  + #[test] fn hangul()"
            ]
        );
        t.status("응답 필요", false, 4000);
        t.feed("Allow command? (y/n)".as_bytes(), 4100);
        assert_eq!(
            titles(&t)[1..],
            [
                (Kind::Step, "작업 중", false),
                (Kind::Step, "응답 필요", true)
            ]
        );
        assert_eq!(t.blocks[1].duration_ms(), Some(3000));
        assert_eq!(
            t.blocks[2].lines,
            ["Allow command? (y/n)"],
            "the current line counts while open"
        );
        t.status("완료", true, 9000);
        assert!(t.blocks.iter().all(|b| !b.open));
        assert_eq!(t.blocks[2].lines, ["Allow command? (y/n)"]);
    }

    #[test]
    fn prompt_detection_for_plain_shells_and_earlier_output() {
        assert_eq!(
            prompt_command("PS C:\\src\\pod> git status"),
            Some("git status")
        );
        assert_eq!(prompt_command("PS C:\\src\\pod>"), Some(""));
        assert_eq!(prompt_command("C:\\Users\\me>dir"), Some("dir"));
        assert_eq!(prompt_command("$ ls -la"), Some("ls -la"));
        assert_eq!(prompt_command("warning: PS is not a prompt> x"), None);
        assert_eq!(prompt_command("결과 3개"), None);

        let mut t = TermBlocks::new(60, 10);
        t.reset(b"PS C:\\src> echo hi\r\nhi\r\nPS C:\\src> ", 60, 10, 0);
        assert_eq!(
            titles(&t),
            vec![(Kind::Command, "echo hi", false)],
            "earlier output is cut at prompts"
        );
        assert_eq!(t.blocks[0].lines, ["hi"]);
        t.feed(b"git log -1\r\n", 100);
        t.feed(b"commit abc\r\n", 200);
        assert_eq!(titles(&t)[1], (Kind::Command, "git log -1", true));
        assert_eq!(t.blocks[1].lines, ["commit abc"]);
        t.feed(b"PS C:\\src> ", 900);
        assert_eq!(
            titles(&t)[1],
            (Kind::Command, "git log -1", false),
            "the prompt is back: done"
        );
        assert_eq!(t.blocks[1].duration_ms(), Some(800));
    }

    #[test]
    fn long_output_keeps_the_newest_lines_and_exit_closes() {
        let mut t = TermBlocks::new(40, 5);
        t.reset(b"", 40, 5, 0);
        t.feed(format!("{A}> {B}seq\r\n{C}").as_bytes(), 0);
        let body: String = (0..MAX_BLOCK_LINES + 50)
            .map(|n| format!("{n}\r\n"))
            .collect();
        t.feed(body.as_bytes(), 10);
        let b = t.blocks.last().unwrap();
        assert_eq!((b.lines.len(), b.dropped), (MAX_BLOCK_LINES, 50));
        assert_eq!(
            b.lines.last().map(String::as_str),
            Some(&*format!("{}", MAX_BLOCK_LINES + 49))
        );
        t.exited(Some(3), 20);
        assert!(!t.blocks.last().unwrap().open);
        assert_eq!(duration_text(3100), "3.1초");
        assert_eq!(duration_text(42_000), "42초");
        assert_eq!(duration_text(72_000), "1분 12초");
    }
}
