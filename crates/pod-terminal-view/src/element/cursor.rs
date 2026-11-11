//! The terminal cursor: block (glyph redrawn in the background color),
//! beam, underline, or an outline while unfocused.

use gpui_kit::*;

use super::hsla_of;
use super::text::{shape_cells, text_run};
use crate::boxdraw;
use crate::vt::{CursorShape, Snapshot};

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_cursor(
    focused: bool,
    col: u16,
    row: u16,
    shape: CursorShape,
    origin: Point<Pixels>,
    cell_w: Pixels,
    line_h: Pixels,
    font_size: Pixels,
    font: &Font,
    snapshot: &Snapshot,
    window: &mut Window,
    cx: &mut App,
) {
    let cols = snapshot.size.cols as usize;
    let cell = snapshot
        .cells
        .get(row as usize * cols + col as usize)
        .copied();
    let width = if cell.is_some_and(|c| c.wide) {
        cell_w * 2.
    } else {
        cell_w
    };
    let at = point(
        origin.x + cell_w * col as f32,
        origin.y + line_h * row as f32,
    );
    let color = hsla_of(snapshot.cursor_color);
    let b = Bounds::new(at, size(width, line_h));
    match (focused, shape) {
        (true, CursorShape::Block) => {
            window.paint_quad(fill(b, color));
            // Re-draw the glyph under the cursor in the background color.
            if let Some(c) = cell
                && !c.c.is_whitespace()
                && c.c != '\0'
                && !boxdraw::is_custom(c.c)
            {
                let ix = row as usize * cols + col as usize;
                let marks = snapshot
                    .zerowidth
                    .binary_search_by_key(&ix, |(i, _)| *i)
                    .ok()
                    .map(|i| snapshot.zerowidth[i].1.as_str());
                let text = SharedString::from(format!("{}{}", c.c, marks.unwrap_or("")));
                let run = text_run(text.len(), font, &c, snapshot.default_bg);
                let fit = if c.wide {
                    2
                } else if marks.is_some() {
                    1
                } else {
                    0
                };
                let (line, dx) = shape_cells(text, &[run], font_size, cell_w, fit, window);
                let _ = line.paint(
                    point(at.x + dx, at.y),
                    line_h,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
        }
        (true, CursorShape::Beam) => {
            window.paint_quad(fill(Bounds::new(at, size(px(2.), line_h)), color))
        }
        (true, CursorShape::Underline) => window.paint_quad(fill(
            Bounds::new(point(at.x, at.y + line_h - px(2.)), size(width, px(2.))),
            color,
        )),
        _ => window.paint_quad(outline(b, color, BorderStyle::Solid)),
    }
}
