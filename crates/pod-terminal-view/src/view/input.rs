//! Keyboard and mouse input of [`TerminalView`]: terminal shortcuts, key
//! encoding, mouse reports, selection drags and the wheel.

use gpui_kit::*;

use super::{LayoutInfo, TerminalEvent, TerminalView};
use crate::keys::{KeyInput, KeyModes, encode_key};
use crate::mouse::{self, Action, MouseModes, MouseMods, encode_mouse};
use crate::selection::{SelMode, SelPoint, Selection};
use crate::vt::MouseTracking;

impl TerminalView {
    // ---- keyboard ------------------------------------------------------

    pub(super) fn on_key_down(
        &mut self,
        ev: &KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
    pub(super) fn cell_at(&self, pos: Point<Pixels>) -> Option<(u16, u16, bool)> {
        Some(self.layout?.cell_at(pos))
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

impl LayoutInfo {
    /// (col, row) of a window position in this paint, clamped to the grid,
    /// plus whether it was inside.
    pub(super) fn cell_at(&self, pos: Point<Pixels>) -> (u16, u16, bool) {
        let l = self;
        let x = ((pos.x - l.origin.x) / l.cell_w).floor();
        let y = ((pos.y - l.origin.y) / l.line_h).floor();
        let inside = x >= 0. && y >= 0. && x < l.size.cols as f32 && y < l.size.rows as f32;
        let col = x.clamp(0., (l.size.cols.max(1) - 1) as f32) as u16;
        let row = y.clamp(0., (l.size.rows.max(1) - 1) as f32) as u16;
        (col, row, inside)
    }
}
