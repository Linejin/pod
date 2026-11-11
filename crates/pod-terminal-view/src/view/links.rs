//! Links on the screen of a [`TerminalView`]: the host filter, the
//! keyboard list, Ctrl+hover underline and the open request.

use gpui_kit::*;

use super::{TerminalEvent, TerminalView, VtRows};
use crate::links::{LinkTarget, TerminalLink};
use crate::selection::SelPoint;

impl TerminalView {
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
    pub(super) fn update_hover(&mut self, ctrl: bool, cx: &mut Context<Self>) {
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
}
