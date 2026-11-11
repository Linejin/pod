//! IME / platform text input of [`TerminalView`] (preedit and commit).

use std::ops::Range;

use gpui_kit::*;

use super::TerminalView;

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
