//! [`VtEngine`] backed by `alacritty_terminal` (parser + grid only, no PTY).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use alacritty_terminal::event::{Event as AlacEvent, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color, CursorShape as AlacShape, NamedColor, Processor, Rgb, StdSyncHandler,
};

use super::{
    CellSnap, CursorShape, GridSize, Modes, MouseTracking, RowText, Snapshot, VtEngine, VtEvent,
};
use crate::palette::{TerminalPalette, dim};

#[derive(Clone, Default)]
struct Listener(Rc<RefCell<Vec<AlacEvent>>>);

impl EventListener for Listener {
    fn send_event(&self, event: AlacEvent) {
        match event {
            // Redraw is driven by `feed`; these carry no information for us.
            AlacEvent::Wakeup | AlacEvent::MouseCursorDirty | AlacEvent::CursorBlinkingChange => {}
            e => self.0.borrow_mut().push(e),
        }
    }
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows as usize
    }
    fn screen_lines(&self) -> usize {
        self.rows as usize
    }
    fn columns(&self) -> usize {
        self.cols as usize
    }
}

pub struct AlacrittyVt {
    term: Term<Listener>,
    parser: Processor<StdSyncHandler>,
    events: Listener,
    cell_px: (u16, u16),
    scrollback: usize,
    palette: TerminalPalette,
}

impl AlacrittyVt {
    pub fn new(size: GridSize, scrollback: usize) -> Self {
        let events = Listener::default();
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        let term = Term::new(config, &size, events.clone());
        Self {
            term,
            parser: Processor::new(),
            events,
            cell_px: (8, 16),
            scrollback,
            palette: TerminalPalette::default(),
        }
    }

    fn color_of(&self, index: usize) -> u32 {
        let colors = self.term.colors();
        if index < 256 {
            return colors[index]
                .map(pack)
                .unwrap_or_else(|| self.palette.indexed(index as u8));
        }
        match index {
            256 => colors[NamedColor::Foreground]
                .map(pack)
                .unwrap_or(self.palette.foreground),
            257 => colors[NamedColor::Background]
                .map(pack)
                .unwrap_or(self.palette.background),
            258 => colors[NamedColor::Cursor]
                .map(pack)
                .unwrap_or(self.palette.cursor),
            _ => self.palette.foreground,
        }
    }
}

impl VtEngine for AlacrittyVt {
    fn set_palette(&mut self, palette: TerminalPalette) {
        self.palette = palette;
    }
    fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    fn take_events(&mut self) -> Vec<VtEvent> {
        let raw = std::mem::take(&mut *self.events.0.borrow_mut());
        let mut out = Vec::with_capacity(raw.len());
        for e in raw {
            match e {
                AlacEvent::PtyWrite(s) => out.push(VtEvent::Reply(s.into_bytes())),
                AlacEvent::Title(t) => out.push(VtEvent::Title(t)),
                AlacEvent::ResetTitle => out.push(VtEvent::ResetTitle),
                AlacEvent::Bell => out.push(VtEvent::Bell),
                AlacEvent::ClipboardStore(_, s) => out.push(VtEvent::ClipboardStore(s)),
                AlacEvent::ColorRequest(index, fmt) => {
                    let c = self.color_of(index);
                    let rgb = Rgb {
                        r: (c >> 16) as u8,
                        g: (c >> 8) as u8,
                        b: c as u8,
                    };
                    out.push(VtEvent::Reply(fmt(rgb).into_bytes()));
                }
                AlacEvent::TextAreaSizeRequest(fmt) => {
                    let s = self.size();
                    let ws = WindowSize {
                        num_lines: s.rows,
                        num_cols: s.cols,
                        cell_width: self.cell_px.0,
                        cell_height: self.cell_px.1,
                    };
                    out.push(VtEvent::Reply(fmt(ws).into_bytes()));
                }
                // Reading the clipboard from the program (OSC 52 load) is refused on purpose.
                _ => {}
            }
        }
        out
    }

    fn sync_deadline(&self) -> Option<Instant> {
        self.parser.sync_timeout().sync_timeout()
    }

    fn flush_sync(&mut self) {
        self.parser.stop_sync(&mut self.term);
    }

    fn size(&self) -> GridSize {
        GridSize {
            cols: self.term.columns() as u16,
            rows: self.term.screen_lines() as u16,
        }
    }

    fn resize(&mut self, size: GridSize, cell_px: (u16, u16)) {
        self.cell_px = cell_px;
        if size != self.size() {
            self.term.resize(size);
        }
    }

    fn modes(&self) -> Modes {
        let m = *self.term.mode();
        let mouse = if m.contains(TermMode::MOUSE_MOTION) {
            MouseTracking::Motion
        } else if m.contains(TermMode::MOUSE_DRAG) {
            MouseTracking::Drag
        } else if m.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseTracking::Click
        } else {
            MouseTracking::Off
        };
        Modes {
            app_cursor: m.contains(TermMode::APP_CURSOR),
            app_keypad: m.contains(TermMode::APP_KEYPAD),
            bracketed_paste: m.contains(TermMode::BRACKETED_PASTE),
            focus_events: m.contains(TermMode::FOCUS_IN_OUT),
            mouse,
            mouse_sgr: m.contains(TermMode::SGR_MOUSE),
            mouse_utf8: m.contains(TermMode::UTF8_MOUSE),
            alt_screen: m.contains(TermMode::ALT_SCREEN),
            alternate_scroll: m.contains(TermMode::ALTERNATE_SCROLL),
            show_cursor: m.contains(TermMode::SHOW_CURSOR),
        }
    }

    fn snapshot(&self) -> Snapshot {
        let term = &self.term;
        let content = term.renderable_content();
        let colors = content.colors;
        let cols = term.columns();
        let rows = term.screen_lines();
        let default_fg = resolve(Color::Named(NamedColor::Foreground), colors, &self.palette);
        let default_bg = resolve(Color::Named(NamedColor::Background), colors, &self.palette);
        let blank = CellSnap {
            c: ' ',
            fg: default_fg,
            bg: default_bg,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            wide: false,
        };
        let mut cells = vec![blank; cols * rows];
        let mut zerowidth: Vec<(usize, String)> = Vec::new();
        let offset = content.display_offset as i32;

        for indexed in content.display_iter {
            let row = indexed.point.line.0 + offset;
            if row < 0 || row as usize >= rows {
                continue;
            }
            let Column(col) = indexed.point.column;
            if col >= cols {
                continue;
            }
            let cell = indexed.cell;
            let flags = cell.flags;
            let (mut fg, mut bg) = (
                resolve(cell.fg, colors, &self.palette),
                resolve(cell.bg, colors, &self.palette),
            );
            if flags.contains(Flags::BOLD) {
                // Bold + named dark color → bright variant, like xterm's boldColors.
                if let Color::Named(n) = cell.fg
                    && (n as usize) < 8
                {
                    fg = resolve(Color::Indexed(n as u8 + 8), colors, &self.palette);
                }
            }
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::DIM) {
                fg = dim(fg);
            }
            if flags.contains(Flags::HIDDEN) {
                fg = bg;
            }
            let spacer =
                flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
            if !spacer && let Some(zw) = cell.zerowidth().filter(|z| !z.is_empty()) {
                zerowidth.push((row as usize * cols + col, zw.iter().collect()));
            }
            cells[row as usize * cols + col] = CellSnap {
                c: if spacer { '\0' } else { cell.c },
                fg,
                bg,
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                strikeout: flags.contains(Flags::STRIKEOUT),
                wide: flags.contains(Flags::WIDE_CHAR),
            };
        }

        let cursor_point = {
            let p = content.cursor.point;
            let row = p.line.0 + offset;
            (row >= 0 && (row as usize) < rows).then_some((p.column.0 as u16, row as u16))
        };
        let shape = content.cursor.shape;
        let cursor = if content.mode.contains(TermMode::SHOW_CURSOR) && shape != AlacShape::Hidden {
            let p = content.cursor.point;
            let row = p.line.0 + offset;
            let shape = match shape {
                AlacShape::Underline => CursorShape::Underline,
                AlacShape::Beam => CursorShape::Beam,
                AlacShape::HollowBlock => CursorShape::HollowBlock,
                _ => CursorShape::Block,
            };
            (row >= 0 && (row as usize) < rows).then_some((p.column.0 as u16, row as u16, shape))
        } else {
            None
        };

        zerowidth.sort_unstable_by_key(|(i, _)| *i);
        Snapshot {
            size: GridSize {
                cols: cols as u16,
                rows: rows as u16,
            },
            cells,
            zerowidth,
            cursor,
            cursor_point,
            default_bg,
            default_fg,
            cursor_color: colors[NamedColor::Cursor]
                .map(pack)
                .unwrap_or(self.palette.cursor),
            display_offset: content.display_offset,
            history_size: term.grid().history_size(),
        }
    }

    fn scroll_display(&mut self, delta: i32) {
        self.term.scroll_display(Scroll::Delta(delta));
    }

    fn scroll_to_bottom(&mut self) {
        self.term.scroll_display(Scroll::Bottom);
    }

    fn scroll_to_top(&mut self) {
        self.term.scroll_display(Scroll::Top);
    }

    fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    fn max_history(&self) -> usize {
        self.scrollback
    }

    fn row_text(&self, line: i32) -> Option<RowText> {
        let grid = self.term.grid();
        let rows = grid.screen_lines() as i32;
        let history = grid.history_size() as i32;
        if line >= rows || line < -history {
            return None;
        }
        let row = &grid[Line(line)];
        let cols = grid.columns();
        let mut cells = Vec::with_capacity(cols);
        let mut zerowidth = Vec::new();
        for c in 0..cols {
            let cell = &row[Column(c)];
            let spacer = cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER);
            cells.push(if spacer { '\0' } else { cell.c });
            if !spacer && let Some(zw) = cell.zerowidth().filter(|z| !z.is_empty()) {
                zerowidth.push((c as u16, zw.iter().collect()));
            }
        }
        let wrapped = cols > 0 && row[Column(cols - 1)].flags.contains(Flags::WRAPLINE);
        Some(RowText {
            cells,
            wrapped,
            zerowidth,
        })
    }
}

fn pack(rgb: Rgb) -> u32 {
    ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
}

fn resolve(color: Color, colors: &Colors, palette: &TerminalPalette) -> u32 {
    match color {
        Color::Spec(rgb) => pack(rgb),
        Color::Indexed(i) => colors[i as usize]
            .map(pack)
            .unwrap_or_else(|| palette.indexed(i)),
        Color::Named(n) => colors[n]
            .map(pack)
            .unwrap_or_else(|| named_default(n, palette)),
    }
}

fn named_default(n: NamedColor, palette: &TerminalPalette) -> u32 {
    let idx = n as usize;
    if idx < 16 {
        return palette.ansi[idx];
    }
    match n {
        NamedColor::Background => palette.background,
        NamedColor::Cursor => palette.cursor,
        NamedColor::DimForeground => dim(palette.foreground),
        NamedColor::BrightForeground => 0xffffff,
        NamedColor::Foreground => palette.foreground,
        _ if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&idx) => {
            dim(palette.ansi[idx - NamedColor::DimBlack as usize])
        }
        _ => palette.foreground,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_defaults_and_program_precedence() {
        use crate::palette::{ANSI16, CURSOR, DEFAULT_BG, DEFAULT_FG, SELECTION_BG};
        let default = TerminalPalette::default();
        assert_eq!(
            default,
            TerminalPalette {
                foreground: DEFAULT_FG,
                background: DEFAULT_BG,
                cursor: CURSOR,
                selection_bg: SELECTION_BG,
                ansi: ANSI16
            }
        );
        let mut vt = AlacrittyVt::new(GridSize { cols: 12, rows: 2 }, 100);
        assert_eq!(vt.snapshot().default_bg, default.background);
        let mut palette = default.clone();
        palette.foreground = 0x112233;
        palette.background = 0x445566;
        palette.cursor = 0x778899;
        palette.ansi[1] = 0x123456;
        vt.set_palette(palette.clone());
        vt.feed(b"\x1b[31mA\x1b[38;5;1mB\x1b[38;2;7;8;9mC");
        let snap = vt.snapshot();
        assert_eq!(snap.cells[0].fg, 0x123456);
        assert_eq!(snap.cells[1].fg, 0x123456);
        assert_eq!(snap.cells[2].fg, 0x070809);
        assert_eq!(snap.default_bg, 0x445566);
        vt.feed(b"\x1b]4;1;rgb:aa/bb/cc\x07\x1b]10;rgb:11/22/44\x07\x1b]11;rgb:22/33/55\x07\x1b]12;rgb:33/44/66\x07");
        vt.set_palette(default);
        let snap = vt.snapshot();
        assert_eq!(snap.cells[0].fg, 0xaabbcc);
        assert_eq!(snap.cells[1].fg, 0xaabbcc);
        assert_eq!(snap.cells[2].fg, 0x070809);
        assert_eq!(snap.default_fg, 0x112244);
        assert_eq!(snap.default_bg, 0x223355);
        assert_eq!(snap.cursor_color, 0x334466);
        for index in 16..=255 {
            assert_eq!(palette.indexed(index), crate::palette::xterm_256(index));
        }
    }
    #[test]
    fn feed_and_read_rows() {
        let mut vt = AlacrittyVt::new(GridSize { cols: 10, rows: 3 }, 100);
        vt.feed(b"hello\r\nworld");
        assert_eq!(
            vt.row_text(0)
                .unwrap()
                .cells
                .iter()
                .collect::<String>()
                .trim_end(),
            "hello"
        );
        assert_eq!(
            vt.row_text(1)
                .unwrap()
                .cells
                .iter()
                .collect::<String>()
                .trim_end(),
            "world"
        );
        vt.feed(b"\r\n1\r\n2\r\n3");
        assert_eq!(vt.history_size(), 2);
        assert_eq!(vt.row_text(-2).unwrap().cells[0], 'h');
    }

    #[test]
    fn modes_and_replies() {
        let mut vt = AlacrittyVt::new(GridSize { cols: 10, rows: 3 }, 100);
        vt.feed(b"\x1b[?1002h\x1b[?1006h\x1b[?2004h\x1b[?1h\x1b[?1004h");
        let m = vt.modes();
        assert_eq!(m.mouse, MouseTracking::Drag);
        assert!(m.mouse_sgr && m.bracketed_paste && m.app_cursor && m.focus_events);
        vt.feed(b"\x1b[6n");
        assert_eq!(
            vt.take_events(),
            vec![VtEvent::Reply(b"\x1b[1;1R".to_vec())]
        );
    }

    /// editor-12: combining marks, VS16 and ZWJ sequences are kept with
    /// their cell (drawn and copied), not dropped.
    #[test]
    fn zero_width_chars_are_kept() {
        let mut vt = AlacrittyVt::new(GridSize { cols: 12, rows: 2 }, 100);
        // e + combining acute, ⚠ + VS16, then a ZWJ family.
        vt.feed("e\u{301}x\u{26A0}\u{FE0F}y\u{1F468}\u{200D}\u{1F469}".as_bytes());
        let s = vt.snapshot();
        let zw: Vec<(usize, &str)> = s.zerowidth.iter().map(|(i, z)| (*i, z.as_str())).collect();
        assert_eq!(zw[0], (0, "\u{301}"));
        assert_eq!(zw[1], (2, "\u{FE0F}"));
        assert!(zw[2].1.starts_with('\u{200D}'), "{zw:?}");
        let row = vt.row_text(0).unwrap();
        assert_eq!(row.zerowidth[0], (0, "\u{301}".to_string()));
    }

    #[test]
    fn soft_wrap_flag() {
        let mut vt = AlacrittyVt::new(GridSize { cols: 4, rows: 3 }, 100);
        vt.feed(b"abcdef");
        assert!(vt.row_text(0).unwrap().wrapped);
        assert!(!vt.row_text(1).unwrap().wrapped);
    }
}
