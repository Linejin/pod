//! IME preedit (composition) drawn at the cursor.

use gpui_kit::*;
use unicode_width::UnicodeWidthChar;

use super::hsla_of;
use super::text::{shape_cells, text_run};
use crate::vt::{CellSnap, Snapshot};

/// IME composition text drawn over the grid at the cursor, underlined.
#[allow(clippy::too_many_arguments)]
pub(super) fn paint_preedit(
    text: &str,
    at: (u16, u16),
    origin: Point<Pixels>,
    cell_w: Pixels,
    line_h: Pixels,
    font_size: Pixels,
    font: &Font,
    snapshot: &Snapshot,
    window: &mut Window,
    cx: &mut App,
) {
    let x0 = origin.x + cell_w * at.0 as f32;
    let y = origin.y + line_h * at.1 as f32;
    let widths: Vec<usize> = text.chars().map(|c| c.width().unwrap_or(0)).collect();
    let total: usize = widths.iter().sum::<usize>().max(1);
    let fg = hsla_of(snapshot.default_fg);
    window.paint_quad(fill(
        Bounds::new(point(x0, y), size(cell_w * total as f32, line_h)),
        hsla_of(snapshot.default_bg),
    ));
    let style = CellSnap {
        c: ' ',
        fg: snapshot.default_fg,
        bg: snapshot.default_bg,
        bold: false,
        italic: false,
        underline: false,
        strikeout: false,
        wide: false,
    };
    let mut col = 0usize;
    for (c, w) in text.chars().zip(widths) {
        if w == 0 {
            continue;
        }
        let s = SharedString::from(c.to_string());
        let run = text_run(s.len(), font, &style, snapshot.default_fg);
        let (line, dx) = shape_cells(
            s,
            &[run],
            font_size,
            cell_w,
            if w > 1 { 2 } else { 0 },
            window,
        );
        let _ = line.paint(
            point(x0 + cell_w * col as f32 + dx, y),
            line_h,
            TextAlign::Left,
            None,
            window,
            cx,
        );
        col += w;
    }
    // Composition underline + caret after the preedit.
    window.paint_quad(fill(
        Bounds::new(
            point(x0, y + line_h - px(2.)),
            size(cell_w * total as f32, px(2.)),
        ),
        fg,
    ));
    window.paint_quad(fill(
        Bounds::new(point(x0 + cell_w * col as f32, y), size(px(2.), line_h)),
        hsla_of(snapshot.cursor_color),
    ));
}
