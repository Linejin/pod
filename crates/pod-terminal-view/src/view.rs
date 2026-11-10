//! [`TerminalView`]: the GPUI entity hosts embed.

use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::element::{TerminalElement, hsla_of};
use crate::keys::{KeyInput, KeyModes, encode_focus, encode_key, encode_paste};
use crate::links::{LinkTarget, TerminalLink};
use crate::mouse::{self, Action, MouseModes, MouseMods, encode_mouse};
use crate::selection::{RowSource, SelMode, SelPoint, Selection};
use crate::sink::{InputSink, TermSize};
use crate::vt::{AlacrittyVt, GridSize, Modes, MouseTracking, RowText, VtEngine, VtEvent};

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

    // ---- links ---------------------------------------------------------

    /// Only links accepted by `filter` are underlined, clickable and listed.
    pub fn set_link_filter(&mut self, filter: impl Fn(&LinkTarget) -> bool + 'static) {
        self.link_filter = Some(Box::new(filter));
    }

    fn link_ok(&self, l: &TerminalLink) -> bool {
        self.link_filter.as_ref().is_none_or(|f| f(&l.target))
    }

    /// Links on the visible screen, top to bottom (keyboard alternative to Ctrl+click).
    pub fn visible_links(&self) -> Vec<TerminalLink> {
        let top = self.abs_line_of_row(0);
        let rows = self.vt.size().rows as usize;
        let mut seen = std::collections::HashSet::new();
        crate::links::links_in(&VtRows(&*self.vt), top, rows)
            .into_iter()
            .filter(|l| self.link_ok(l) && seen.insert(l.target.clone()))
            .collect()
    }

    /// The link covering visible cell (`col`, `row`).
    pub fn link_at(&self, col: u16, row: u16) -> Option<TerminalLink> {
        let p = SelPoint::new(self.abs_line_of_row(row), col);
        crate::links::links_in(&VtRows(&*self.vt), p.line, 1)
            .into_iter()
            .find(|l| l.contains(p) && self.link_ok(l))
    }

    /// Window bounds of visible cell (`col`, `row`) in the last paint.
    pub fn cell_bounds(&self, col: u16, row: u16) -> Option<Bounds<Pixels>> {
        let l = self.layout?;
        Some(Bounds::new(
            point(
                l.origin.x + l.cell_w * col as f32,
                l.origin.y + l.line_h * row as f32,
            ),
            size(l.cell_w, l.line_h),
        ))
    }

    /// Shows the Ctrl-hover underline on the link at a visible cell
    /// (fixture screenshots; the mouse path is [`Self::modifiers_changed`]).
    #[doc(hidden)]
    pub fn hover_link_at(&mut self, col: u16, row: u16, cx: &mut Context<Self>) {
        self.hover_link = self.link_at(col, row);
        cx.notify();
    }

    pub fn hovered_link(&self) -> Option<&TerminalLink> {
        self.hover_link.as_ref()
    }

    /// Ask the host to open `target` (same event as Ctrl+click).
    pub fn open_link(&mut self, target: LinkTarget, cx: &mut Context<Self>) {
        cx.emit(TerminalEvent::OpenLink(target));
    }

    /// Recompute the Ctrl-hover link at the last mouse position.
    fn update_hover(&mut self, ctrl: bool, cx: &mut Context<Self>) {
        let link = match (ctrl, self.last_mouse.and_then(|p| self.cell_at(p))) {
            (true, Some((col, row, true))) => self.link_at(col, row),
            _ => None,
        };
        if link != self.hover_link {
            self.hover_link = link;
            cx.notify();
        }
    }

    pub(crate) fn modifiers_changed(&mut self, m: &Modifiers, cx: &mut Context<Self>) {
        if m.control || self.hover_link.is_some() {
            self.update_hover(m.control && !m.shift && !m.alt, cx);
        }
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

    // ---- keyboard ------------------------------------------------------

    fn on_key_down(&mut self, ev: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let ks = &ev.keystroke;
        let m = &ks.modifiers;
        let modes = self.vt.modes();
        crate::debug!(
            "key {:?} char={:?} mods={:?} prefer_text={} marked={:?}",
            ks.key,
            ks.key_char,
            m,
            ev.prefer_character_input,
            self.marked
        );

        // Terminal shortcuts (not sent to the program).
        if m.control && m.shift && !m.alt && !m.platform {
            match ks.key.as_str() {
                "c" => {
                    self.copy_selection(cx);
                    cx.stop_propagation();
                    return;
                }
                "v" => {
                    self.paste_from_clipboard(cx);
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }
        if m.shift && !m.control && !m.alt && !modes.alt_screen {
            let page = self.vt.size().rows.saturating_sub(1).max(1) as i32;
            let handled = match ks.key.as_str() {
                "pageup" => Some(page),
                "pagedown" => Some(-page),
                // Only while scrolled back, so Shift+Home/End still reach the shell.
                "home" if self.vt.display_offset() > 0 => {
                    self.vt.scroll_to_top();
                    Some(0)
                }
                "end" if self.vt.display_offset() > 0 => {
                    self.vt.scroll_to_bottom();
                    Some(0)
                }
                _ => None,
            };
            if let Some(delta) = handled {
                if delta != 0 {
                    self.vt.scroll_display(delta);
                }
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }

        // While the IME composes, keys belong to it.
        if self.marked.is_some() {
            return;
        }

        let input = KeyInput {
            key: &ks.key,
            key_char: ks.key_char.as_deref(),
            ctrl: m.control,
            alt: m.alt,
            shift: m.shift,
            platform: m.platform,
            prefer_text: ev.prefer_character_input,
        };
        if let Some(bytes) = encode_key(&input, KeyModes::from(&modes)) {
            if self.selection.is_some() {
                self.selection = None;
                cx.notify();
            }
            self.write_input(&bytes, cx);
            cx.stop_propagation();
        }
        // Otherwise: printable text arrives via the input handler (WM_CHAR / IME).
    }

    // ---- mouse ---------------------------------------------------------

    /// (col, row) of a window position, clamped to the grid, plus whether it was inside.
    fn cell_at(&self, pos: Point<Pixels>) -> Option<(u16, u16, bool)> {
        let l = self.layout?;
        let x = ((pos.x - l.origin.x) / l.cell_w).floor();
        let y = ((pos.y - l.origin.y) / l.line_h).floor();
        let inside = x >= 0. && y >= 0. && x < l.size.cols as f32 && y < l.size.rows as f32;
        let col = x.clamp(0., (l.size.cols.max(1) - 1) as f32) as u16;
        let row = y.clamp(0., (l.size.rows.max(1) - 1) as f32) as u16;
        Some((col, row, inside))
    }

    fn report(
        &mut self,
        action: Action,
        button: mouse::Button,
        col: u16,
        row: u16,
        m: &Modifiers,
    ) -> bool {
        let mods = MouseMods {
            shift: m.shift,
            alt: m.alt,
            ctrl: m.control,
        };
        match encode_mouse(
            action,
            button,
            col,
            row,
            mods,
            MouseModes::from(&self.vt.modes()),
        ) {
            Some(bytes) => {
                self.sink.write(&bytes);
                true
            }
            None => false,
        }
    }

    fn program_wants_mouse(&self, m: &Modifiers) -> bool {
        // Shift bypasses mouse reporting so the user can still select (xterm convention).
        self.vt.modes().mouse != MouseTracking::Off && !m.shift
    }

    pub(crate) fn mouse_down(
        &mut self,
        e: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::debug!(
            "mouse_down {:?} at {:?} clicks={} layout={:?}",
            e.button,
            e.position,
            e.click_count,
            self.layout.map(|l| l.origin)
        );
        window.focus(&self.focus, cx);
        let Some((col, row, _)) = self.cell_at(e.position) else {
            return;
        };

        // Ctrl+click opens a link, even when the program reports the mouse.
        let m = &e.modifiers;
        if e.button == MouseButton::Left
            && m.control
            && !m.shift
            && !m.alt
            && let Some(link) = self.link_at(col, row)
        {
            self.hover_link = None;
            cx.emit(TerminalEvent::OpenLink(link.target));
            cx.notify();
            return;
        }

        if self.program_wants_mouse(&e.modifiers) {
            let button = match e.button {
                MouseButton::Left => mouse::Button::Left,
                MouseButton::Middle => mouse::Button::Middle,
                MouseButton::Right => mouse::Button::Right,
                _ => return,
            };
            if self.report(Action::Press, button, col, row, &e.modifiers) {
                self.reported_button = Some(button);
                self.last_report_cell = Some((col, row));
            }
            return;
        }

        match e.button {
            MouseButton::Left => {
                let p = SelPoint::new(self.abs_line_of_row(row), col);
                let mode = match e.click_count {
                    0 | 1 => SelMode::Simple,
                    2 => SelMode::Word,
                    _ => SelMode::Line,
                };
                match (&mut self.selection, e.modifiers.shift && e.click_count <= 1) {
                    // Shift+click extends the current selection.
                    (Some(sel), true) => {
                        sel.head = p;
                        self.sel_moved = true;
                    }
                    _ => {
                        self.selection = Some(Selection::new(mode, p));
                        self.sel_moved = mode != SelMode::Simple;
                    }
                }
                self.selecting = true;
                cx.notify();
            }
            MouseButton::Right => {
                // Windows Terminal convention: copy if there is a selection, else paste.
                if self.copy_selection(cx) {
                    self.selection = None;
                    cx.notify();
                } else {
                    self.paste_from_clipboard(cx);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn mouse_move(
        &mut self,
        e: &MouseMoveEvent,
        hovered: bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        crate::debug!(
            "mouse_move {:?} pressed={:?} hovered={hovered}",
            e.position,
            e.pressed_button
        );
        let Some((col, row, inside)) = self.cell_at(e.position) else {
            return;
        };
        self.last_mouse = Some(e.position);
        let m = &e.modifiers;
        let want_hover = hovered && m.control && !m.shift && !m.alt && e.pressed_button.is_none();
        if want_hover || self.hover_link.is_some() {
            self.update_hover(want_hover, cx);
        }

        if let Some(button) = self.reported_button {
            if self.last_report_cell != Some((col, row)) {
                self.last_report_cell = Some((col, row));
                self.report(Action::Motion, button, col, row, &e.modifiers);
            }
            return;
        }
        if hovered
            && inside
            && e.pressed_button.is_none()
            && self.vt.modes().mouse == MouseTracking::Motion
            && !e.modifiers.shift
        {
            if self.last_report_cell != Some((col, row)) {
                self.last_report_cell = Some((col, row));
                self.report(Action::Motion, mouse::Button::None, col, row, &e.modifiers);
            }
            return;
        }

        if self.selecting && e.pressed_button == Some(MouseButton::Left) {
            // Dragging past the top/bottom edge scrolls the history.
            if let Some(l) = self.layout {
                if e.position.y < l.origin.y {
                    self.vt.scroll_display(1);
                } else if e.position.y > l.origin.y + l.line_h * l.size.rows as f32 {
                    self.vt.scroll_display(-1);
                }
            }
            let p = SelPoint::new(self.abs_line_of_row(row), col);
            if let Some(sel) = &mut self.selection
                && sel.head != p
            {
                sel.head = p;
                self.sel_moved = true;
                cx.notify();
            }
        }
    }

    pub(crate) fn mouse_up(&mut self, e: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        crate::debug!(
            "mouse_up {:?} selecting={} moved={} sel={:?}",
            e.button,
            self.selecting,
            self.sel_moved,
            self.selection
        );
        if let Some(button) = self.reported_button.take() {
            let (col, row) = self
                .cell_at(e.position)
                .map(|(c, r, _)| (c, r))
                .unwrap_or((0, 0));
            self.report(Action::Release, button, col, row, &e.modifiers);
            self.last_report_cell = None;
            return;
        }
        if self.selecting && e.button == MouseButton::Left {
            self.selecting = false;
            if !self.sel_moved {
                self.selection = None;
            }
            cx.notify();
        }
    }

    pub(crate) fn scroll_wheel(&mut self, e: &ScrollEvent, _: &mut Window, cx: &mut Context<Self>) {
        crate::debug!(
            "scroll {:?} mods={:?} history={} offset={}",
            e.delta,
            e.modifiers,
            self.vt.history_size(),
            self.vt.display_offset()
        );
        let Some(l) = self.layout else { return };
        let dy = e.delta.pixel_delta(l.line_h).y / l.line_h + self.scroll_remainder;
        let lines = dy.trunc() as i32;
        self.scroll_remainder = dy - lines as f32;
        if lines == 0 {
            return;
        }
        let modes = self.vt.modes();
        if self.program_wants_mouse(&e.modifiers) {
            let (col, row, _) = self.cell_at(e.position).unwrap_or((0, 0, false));
            let button = if lines > 0 {
                mouse::Button::WheelUp
            } else {
                mouse::Button::WheelDown
            };
            for _ in 0..lines.unsigned_abs().min(10) {
                self.report(Action::Press, button, col, row, &e.modifiers);
            }
        } else if modes.alt_screen {
            if modes.alternate_scroll {
                // 1007: the wheel acts as arrow keys for pagers/editors.
                let key = match (lines > 0, modes.app_cursor) {
                    (true, true) => &b"\x1bOA"[..],
                    (true, false) => b"\x1b[A",
                    (false, true) => b"\x1bOB",
                    (false, false) => b"\x1b[B",
                };
                let bytes: Vec<u8> = key.repeat(lines.unsigned_abs().min(10) as usize);
                self.sink.write(&bytes);
            }
        } else {
            self.vt.scroll_display(lines);
            cx.notify();
        }
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

/// IME / platform text input. The terminal has no editable document: the
/// only "text" is the current composition, and committed text goes straight
/// to the program.
impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let marked: Vec<u16> = self.marked.as_ref()?.encode_utf16().collect();
        let r = range.start.min(marked.len())..range.end.min(marked.len());
        *adjusted = Some(r.clone());
        Some(String::from_utf16_lossy(&marked[r]))
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let n = self.marked.as_ref().map_or(0, |m| m.encode_utf16().count());
        Some(UTF16Selection {
            range: n..n,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| 0..m.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Commit: typed character (WM_CHAR) or finished IME composition.
        self.marked = None;
        if !text.is_empty() {
            if self.selection.is_some() {
                self.selection = None;
            }
            self.write_input(text.as_bytes(), cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked = (!new_text.is_empty()).then(|| new_text.to_string());
        if self.vt.display_offset() > 0 {
            self.vt.scroll_to_bottom();
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Candidate window goes below the cursor cell.
        let l = self.layout?;
        let (col, row) = l.cursor.unwrap_or((0, 0));
        Some(Bounds::new(
            point(
                l.origin.x + l.cell_w * col as f32,
                l.origin.y + l.line_h * row as f32,
            ),
            size(l.cell_w, l.line_h),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
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
