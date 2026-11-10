//! The VT model boundary.
//!
//! Everything above this module (renderer, input, selection) talks to a
//! [`VtEngine`] and the plain data types below — never to alacritty types.
//! Swapping in libghostty-vt means writing one more `VtEngine` impl.

mod alacritty;

pub use alacritty::AlacrittyVt;

/// Grid size in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
}

/// Cursor shape as requested by the program (DECSCUSR).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorShape {
    Block,
    Underline,
    Beam,
    HollowBlock,
}

/// One renderable cell, colors already resolved to 0xRRGGBB.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellSnap {
    /// `'\0'` for the spacer cell behind a wide char.
    pub c: char,
    pub fg: u32,
    pub bg: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    pub wide: bool,
}

/// The visible grid at the current scroll position, copied out of the VT.
pub struct Snapshot {
    pub size: GridSize,
    /// Row-major, `rows * cols` cells.
    pub cells: Vec<CellSnap>,
    /// Zero-width characters drawn with a cell's char (combining marks,
    /// ZWJ sequences, VS16 emoji presentation): `(cell index, chars)`,
    /// sorted by index. Rare, so kept out of the `Copy` cell.
    pub zerowidth: Vec<(usize, String)>,
    /// (col, screen row, shape) when the cursor is visible on screen.
    pub cursor: Option<(u16, u16, CursorShape)>,
    /// Cursor cell even when hidden (TUIs that draw their own cursor still
    /// park the real one at the input point): IME preedit goes here.
    pub cursor_point: Option<(u16, u16)>,
    pub default_bg: u32,
    pub default_fg: u32,
    pub cursor_color: u32,
    /// Lines scrolled back from the live screen (0 = bottom).
    pub display_offset: usize,
    /// Lines available in scrollback.
    pub history_size: usize,
}

/// Mouse tracking requested by the program.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MouseTracking {
    #[default]
    Off,
    /// 1000: press/release.
    Click,
    /// 1002: press/release + motion while a button is held.
    Drag,
    /// 1003: press/release + all motion.
    Motion,
}

/// The program-controlled modes the input layer cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Modes {
    pub app_cursor: bool,
    pub app_keypad: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub mouse: MouseTracking,
    pub mouse_sgr: bool,
    pub mouse_utf8: bool,
    pub alt_screen: bool,
    /// 1007: wheel sends arrow keys on the alternate screen.
    pub alternate_scroll: bool,
    pub show_cursor: bool,
}

/// Text of one grid line, for selection and copy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowText {
    /// One entry per column; `'\0'` marks the spacer behind a wide char.
    pub cells: Vec<char>,
    /// The line continues on the next one (soft wrap): no newline when copied.
    pub wrapped: bool,
    /// Zero-width characters after a column's char: `(column, chars)`, sorted.
    pub zerowidth: Vec<(u16, String)>,
}

/// Something the program asked for that the view must act on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VtEvent {
    /// Reply bytes for the program (DA, DSR, OSC color queries …): send to the sink.
    Reply(Vec<u8>),
    Title(String),
    ResetTitle,
    Bell,
    /// OSC 52 store.
    ClipboardStore(String),
}

/// The VT engine interface used by the view. Lines are numbered like
/// alacritty: `0..rows` is the live screen, negative lines are scrollback.
pub trait VtEngine {
    /// Set host color defaults; adapters should forward this to their engine.
    fn set_palette(&mut self, _palette: crate::TerminalPalette) {}
    /// Parse program output.
    fn feed(&mut self, bytes: &[u8]);
    /// Drain events produced by `feed`.
    fn take_events(&mut self) -> Vec<VtEvent>;
    /// A synchronized update (DEC 2026) is buffered; call [`VtEngine::flush_sync`]
    /// after this instant if no more bytes arrive.
    fn sync_deadline(&self) -> Option<std::time::Instant>;
    fn flush_sync(&mut self);

    fn size(&self) -> GridSize;
    fn resize(&mut self, size: GridSize, cell_px: (u16, u16));
    fn modes(&self) -> Modes;

    /// Copy the visible grid (at the current scroll position).
    fn snapshot(&self) -> Snapshot;

    /// Scroll the view into history by `delta` lines (positive = up / older).
    fn scroll_display(&mut self, delta: i32);
    fn scroll_to_bottom(&mut self);
    fn scroll_to_top(&mut self);
    fn display_offset(&self) -> usize;
    fn history_size(&self) -> usize;
    fn max_history(&self) -> usize;

    /// Text of grid line `line` (see trait docs for numbering).
    fn row_text(&self, line: i32) -> Option<RowText>;
}
