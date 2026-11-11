//! [`TerminalView`]: the GPUI entity hosts embed.
//!
//! Input handling lives in child modules of this one: keyboard and mouse in
//! `view/input.rs`, IME / platform text input in `view/ime.rs`, screen links
//! (host filter, keyboard list, Ctrl+hover, open request) in `view/links.rs`.

mod ime;
mod input;
mod links;
#[cfg(test)]
mod tests;

use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::element::{TerminalElement, hsla_of};
use crate::keys::{encode_focus, encode_paste};
use crate::links::{LinkTarget, TerminalLink};
use crate::mouse;
use crate::selection::{RowSource, SelMode, SelPoint, Selection};
use crate::sink::{InputSink, TermSize};
use crate::vt::{AlacrittyVt, GridSize, Modes, RowText, VtEngine, VtEvent};

/// Host check that decides which detected links are real (e.g. the file exists).
pub type LinkFilter = Box<dyn Fn(&LinkTarget) -> bool>;

/// Appearance and buffer settings.
#[derive(Clone, Debug)]
pub struct TerminalConfig {
    pub font_family: SharedString,
    pub font_size: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Scrollback lines kept by the VT.
    pub scrollback: usize,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            font_family: "Cascadia Mono".into(),
            font_size: 13.0,
            line_height: 1.3,
            scrollback: crate::DEFAULT_SCROLLBACK,
        }
    }
}

/// Events for the host (tab titles, notifications).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    TitleChanged(Option<String>),
    Bell,
    /// Ctrl+click on a link (or [`TerminalView::open_link`]): the host opens it.
    OpenLink(LinkTarget),
}

/// Geometry of the last paint, used to map mouse positions and place the IME window.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LayoutInfo {
    pub origin: Point<Pixels>,
    pub cell_w: Pixels,
    pub line_h: Pixels,
    pub size: GridSize,
    pub cursor: Option<(u16, u16)>,
}

pub struct TerminalView {
    vt: Box<dyn VtEngine>,
    sink: Box<dyn InputSink>,
    config: TerminalConfig,
    pub(crate) palette: crate::TerminalPalette,
    focus: FocusHandle,
    title: Option<String>,
    selection: Option<Selection>,
    selecting: bool,
    /// The drag moved off the press cell (a plain click selects nothing).
    sel_moved: bool,
    /// IME composition string (preedit), drawn at the cursor.
    marked: Option<String>,
    pub(crate) layout: Option<LayoutInfo>,
    /// Button held while the program receives mouse reports.
    reported_button: Option<mouse::Button>,
    last_report_cell: Option<(u16, u16)>,
    /// Sub-line remainder of precise (touchpad) scrolling.
    scroll_remainder: f32,
    sync_flush: Option<Task<()>>,
    last_size: Option<TermSize>,
    /// Host check for links (e.g. "the file exists"); unchecked when `None`.
    link_filter: Option<LinkFilter>,
    /// Link under the mouse while Ctrl is held (underlined, hand cursor).
    hover_link: Option<TerminalLink>,
    last_mouse: Option<Point<Pixels>>,
    _focus_subs: [Subscription; 2],
}

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    /// A view over a fresh VT. `sink` receives keyboard/mouse/paste bytes,
    /// query replies and resizes.
    pub fn new(
        sink: Box<dyn InputSink>,
        config: TerminalConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let vt = AlacrittyVt::new(GridSize { cols: 80, rows: 24 }, config.scrollback);
        Self::with_engine(Box::new(vt), sink, config, window, cx)
    }

    /// Same as [`TerminalView::new`] with a caller-supplied VT engine.
    pub fn with_engine(
        vt: Box<dyn VtEngine>,
        sink: Box<dyn InputSink>,
        config: TerminalConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        let subs = [
            cx.on_focus(&focus, window, |v, _, cx| v.focus_changed(true, cx)),
            cx.on_blur(&focus, window, |v, _, cx| v.focus_changed(false, cx)),
        ];
        Self {
            vt,
            sink,
            config,
            palette: crate::TerminalPalette::default(),
            focus,
            title: None,
            selection: None,
            selecting: false,
            sel_moved: false,
            marked: None,
            layout: None,
            reported_button: None,
            last_report_cell: None,
            scroll_remainder: 0.,
            sync_flush: None,
            last_size: None,
            link_filter: None,
            hover_link: None,
            last_mouse: None,
            _focus_subs: subs,
        }
    }

    /// Change defaults without touching program-owned OSC/SGR colors.
    pub fn set_palette(&mut self, palette: crate::TerminalPalette, cx: &mut Context<Self>) {
        self.vt.set_palette(palette.clone());
        self.palette = palette;
        cx.notify();
    }

    /// Repaint with new font metrics; the next layout reports the resized grid.
    pub fn set_font(&mut self, family: SharedString, size: f32, cx: &mut Context<Self>) {
        self.config.font_family = family;
        self.config.font_size = size.clamp(10., 28.);
        cx.notify();
    }

    // ---- data in -------------------------------------------------------

    /// Feed raw program output (PTY bytes / podd stream) into the VT.
    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        self.vt.feed(bytes);
        // Absolute selection lines drift once scrollback is full and old
        // lines are dropped; drop the selection rather than show wrong text.
        if self.selection.is_some() && self.vt.history_size() >= self.vt.max_history() {
            self.selection = None;
            self.selecting = false;
        }
        self.after_vt_change(cx);
    }

    fn after_vt_change(&mut self, cx: &mut Context<Self>) {
        for event in self.vt.take_events() {
            match event {
                VtEvent::Reply(bytes) => self.sink.write(&bytes),
                VtEvent::Title(t) => {
                    self.title = Some(t.clone());
                    cx.emit(TerminalEvent::TitleChanged(Some(t)));
                }
                VtEvent::ResetTitle => {
                    self.title = None;
                    cx.emit(TerminalEvent::TitleChanged(None));
                }
                VtEvent::Bell => cx.emit(TerminalEvent::Bell),
                VtEvent::ClipboardStore(s) => cx.write_to_clipboard(ClipboardItem::new_string(s)),
            }
        }
        self.schedule_sync_flush(cx);
        cx.notify();
    }

    /// DEC 2026 synchronized updates are buffered by the parser; if the
    /// program never sends the end marker, flush at the parser's deadline.
    fn schedule_sync_flush(&mut self, cx: &mut Context<Self>) {
        let Some(deadline) = self.vt.sync_deadline() else {
            self.sync_flush = None;
            return;
        };
        let wait = deadline.saturating_duration_since(Instant::now()) + Duration::from_millis(1);
        self.sync_flush = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |v, cx| {
                if v.vt.sync_deadline().is_some_and(|d| d <= Instant::now()) {
                    v.vt.flush_sync();
                    v.after_vt_change(cx);
                }
            });
        }));
    }

    // ---- accessors -----------------------------------------------------

    pub fn config(&self) -> &TerminalConfig {
        &self.config
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn modes(&self) -> Modes {
        self.vt.modes()
    }

    pub fn grid_size(&self) -> GridSize {
        self.vt.size()
    }

    pub fn vt(&self) -> &dyn VtEngine {
        &*self.vt
    }

    pub fn marked_text(&self) -> Option<&str> {
        self.marked.as_deref()
    }

    /// Visible screen as text (rows joined by '\n'), for tests and debugging.
    pub fn screen_text(&self) -> String {
        let rows = self.vt.size().rows as i32;
        let off = self.vt.display_offset() as i32;
        (0..rows)
            .filter_map(|r| self.vt.row_text(r - off))
            .map(|t| {
                t.cells
                    .iter()
                    .filter(|c| **c != '\0')
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ---- data out ------------------------------------------------------

    /// Send user input to the program: jumps back to the live screen first.
    pub fn write_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if self.vt.display_offset() > 0 {
            self.vt.scroll_to_bottom();
            cx.notify();
        }
        self.sink.write(bytes);
    }

    /// Paste text (bracketed when the program enabled it).
    pub fn paste(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        let bytes = encode_paste(text, self.vt.modes().bracketed_paste);
        self.write_input(&bytes, cx);
    }

    pub fn paste_from_clipboard(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.paste(&text, cx);
        }
    }

    /// Copy the selection to the clipboard. Returns false when nothing is selected.
    pub fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        match self.selection_text() {
            Some(text) if !text.is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            _ => false,
        }
    }

    pub(crate) fn resize_grid(&mut self, size: GridSize, cell_px: (u16, u16)) {
        let ts = TermSize {
            cols: size.cols,
            rows: size.rows,
            cell_width: cell_px.0,
            cell_height: cell_px.1,
        };
        if self.last_size == Some(ts) {
            return;
        }
        self.last_size = Some(ts);
        self.vt.resize(size, cell_px);
        self.sink.resize(ts);
    }

    fn focus_changed(&mut self, focused: bool, cx: &mut Context<Self>) {
        if self.vt.modes().focus_events {
            self.sink.write(encode_focus(focused));
        }
        cx.notify();
    }

    // ---- selection -----------------------------------------------------

    pub fn selection(&self) -> Option<Selection> {
        self.selection
    }

    pub fn set_selection(&mut self, selection: Option<Selection>, cx: &mut Context<Self>) {
        self.selection = selection;
        cx.notify();
    }

    /// Absolute line (see [`crate::selection`]) of visible row `row`.
    pub fn abs_line_of_row(&self, row: u16) -> i64 {
        self.vt.history_size() as i64 - self.vt.display_offset() as i64 + row as i64
    }

    pub fn selection_range(&self) -> Option<(SelPoint, SelPoint)> {
        let sel = self.selection?;
        if sel.mode == SelMode::Simple && sel.anchor == sel.head && !self.sel_moved {
            return None;
        }
        Some(sel.range(&VtRows(&*self.vt)))
    }

    pub fn selection_text(&self) -> Option<String> {
        self.selection_range()?;
        Some(self.selection?.to_text(&VtRows(&*self.vt)))
    }

    // ---- scrolling -----------------------------------------------------

    /// Scroll the history view; positive = older lines.
    pub fn scroll_lines(&mut self, delta: i32, cx: &mut Context<Self>) {
        if self.vt.modes().alt_screen {
            return;
        }
        self.vt.scroll_display(delta);
        cx.notify();
    }
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        div()
            .id("pod-terminal-view")
            .role(Role::TextInput)
            .aria_label("터미널 입력 및 출력")
            .track_focus(&self.focus)
            .focus_visible(|s| {
                s.shadow(vec![BoxShadow {
                    inset: true,
                    color: hsla_of(self.palette.cursor),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(2.),
                }])
            })
            .key_context("Terminal")
            .on_key_down(cx.listener(Self::on_key_down))
            // Pressing/releasing Ctrl over a link shows/hides its underline.
            .on_modifiers_changed(cx.listener(|v, e: &ModifiersChangedEvent, _, cx| {
                v.modifiers_changed(&e.modifiers, cx)
            }))
            .size_full()
            .bg(hsla_of(self.palette.background))
            .child(TerminalElement::new(
                cx.entity(),
                self.focus.clone(),
                focused,
            ))
    }
}

/// Absolute-line row access over a VT engine.
struct VtRows<'a>(&'a dyn VtEngine);

impl RowSource for VtRows<'_> {
    fn cols(&self) -> u16 {
        self.0.size().cols
    }
    fn row(&self, line: i64) -> Option<RowText> {
        let vt_line = line - self.0.history_size() as i64;
        self.0.row_text(i32::try_from(vt_line).ok()?)
    }
}
