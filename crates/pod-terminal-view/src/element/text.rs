//! Grid text: split a row into shaping segments (narrow runs on the cell
//! grid, wide chars, zero-width marks and emoji sequences as one cluster),
//! shape them and fit wide glyphs into their cells.

use gpui_kit::*;

use super::hsla_of;
use crate::boxdraw;
use crate::vt::CellSnap;

pub(super) fn text_run(len: usize, font: &Font, c: &CellSnap, color: u32) -> TextRun {
    TextRun {
        len,
        font: styled_font(font, c),
        color: hsla_of(color),
        background_color: None,
        underline: c.underline.then(|| UnderlineStyle {
            thickness: px(1.),
            color: Some(hsla_of(color)),
            wavy: false,
        }),
        strikethrough: c.strikeout.then(|| StrikethroughStyle {
            thickness: px(1.),
            color: Some(hsla_of(color)),
        }),
    }
}

/// Shape grid text. Narrow cells get a fixed `cell_w` advance per glyph. A
/// wide (2-cell) char is shaped at its natural advance and placed by
/// [`wide_fit`]; returns the line and the x offset inside its first cell.
/// Shapes a segment. `fit_cells` 0: a run of narrow chars on the cell grid
/// (fixed advance); 1 or 2: one glyph cluster (a wide char, or a char with
/// zero-width marks, which must not get a cell advance each) fitted into
/// that many cells; more: an emoji sequence joined across cells (ZWJ, skin
/// tone, flag), see [`cluster_fit`].
pub(super) fn shape_cells(
    text: SharedString,
    runs: &[TextRun],
    font_size: Pixels,
    cell_w: Pixels,
    fit_cells: usize,
    window: &Window,
) -> (ShapedLine, Pixels) {
    let ts = window.text_system();
    if fit_cells == 0 {
        return (ts.shape_line(text, font_size, runs, Some(cell_w)), px(0.));
    }
    let natural = ts.shape_line(text.clone(), font_size, runs, None);
    // Ink box of the glyph in the font that actually drew it (fallback included).
    let ink = natural
        .runs
        .first()
        .zip(text.chars().next())
        .and_then(|(run, ch)| ts.typographic_bounds(run.font_id, font_size, ch).ok());
    let ink = ink.map(|b| (b.origin.x.as_f32(), b.size.width.as_f32()));
    let slot = cell_w.as_f32() * fit_cells as f32;
    let (dx, k) = if fit_cells > 2 {
        cluster_fit(natural.width.as_f32(), slot, window.scale_factor())
    } else {
        wide_fit(natural.width.as_f32(), ink, slot, window.scale_factor())
    };
    if (k - 1.).abs() > 0.001 {
        (ts.shape_line(text, font_size * k, runs, None), px(dx))
    } else {
        (natural, px(dx))
    }
}

/// Largest enlargement of a wide glyph (keeps it inside the line height:
/// line height is 1.3 em, a 1.35× Hangul syllable is about 1.25 em tall).
pub(super) const WIDE_MAX_GROW: f32 = 1.35;

/// Placement of a wide glyph in a 2-cell `slot` (logical px): `(x offset of the
/// glyph origin, font scale)`, the offset snapped to device pixels. `advance`
/// is the natural advance, `ink` the natural ink box `(left bearing, width)`.
///
/// - Advance fits the slot (Hangul, kana, kanji: 1 em of advance against two
///   Cascadia Mono cells ≈ 1.17 em): scale so the advance fills the slot (at
///   most [`WIDE_MAX_GROW`]), centered by advance. Every glyph of a font gets
///   the same scale, so kana stay smaller than kanji as designed, and at
///   natural size each syllable left a visible gap ("문 서").
/// - Advance overflows the slot (color emoji: ≈ 1.37 em of advance but only
///   ≈ 1.0–1.1 em of ink): fit by ink instead — natural size unless the ink
///   itself is wider than the slot, ink centered. Fitting the advance shrank
///   emoji to ≈ 0.85× with empty sides.
pub(super) fn wide_fit(advance: f32, ink: Option<(f32, f32)>, slot: f32, scale: f32) -> (f32, f32) {
    if !(advance > 0. && slot > 0.) {
        return (0., 1.);
    }
    let (dx, k) = match ink {
        Some((left, width)) if advance > slot && width > 0. => {
            let k = (slot / width).min(1.);
            ((slot - width * k) / 2. - left * k, k)
        }
        _ => {
            let k = (slot / advance).min(WIDE_MAX_GROW);
            (((slot - advance * k) / 2.).max(0.), k)
        }
    };
    ((dx * scale).round() / scale.max(0.01), k)
}

/// Placement of an emoji sequence joined across cells in its `slot` (all
/// its cells): natural size and centered when it fits (one combined glyph,
/// e.g. a family), otherwise shrunk to the slot (a font without the
/// sequence draws the parts side by side). Never enlarged.
pub(super) fn cluster_fit(advance: f32, slot: f32, scale: f32) -> (f32, f32) {
    if !(advance > 0. && slot > 0.) {
        return (0., 1.);
    }
    let k = (slot / advance).min(1.);
    let dx = (slot - advance * k) / 2.;
    ((dx * scale).round() / scale.max(0.01), k)
}

/// The cell at a column continues the emoji sequence of the previous
/// cluster `prev` (alacritty gives each emoji of a sequence its own cells
/// and hangs the ZWJ on the cell before): after a ZWJ, a skin tone
/// modifier, or the second regional indicator of a flag.
fn continues_cluster(prev: &str, c: char) -> bool {
    let ri = |c: char| ('\u{1F1E6}'..='\u{1F1FF}').contains(&c);
    prev.ends_with('\u{200D}')
        || ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
        || (ri(c) && prev.chars().count() == 1 && prev.chars().all(ri))
}

pub(super) struct Segment {
    pub(super) start_col: usize,
    /// 0: narrow chars on the grid; 1/2: one cluster fitted into that many
    /// cells (a wide char, or a char with zero-width marks).
    pub(super) fit_cells: usize,
    pub(super) text: String,
    /// (byte length, style source cell)
    pub(super) runs: Vec<(usize, CellSnap)>,
}

fn same_style(a: &CellSnap, b: &CellSnap) -> bool {
    a.fg == b.fg
        && a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.strikeout == b.strikeout
}

/// The zero-width chars of one row (`start..start+cols` of the grid), by column.
pub(super) fn row_zerowidth(
    all: &[(usize, String)],
    start: usize,
    cols: usize,
) -> Vec<(usize, &str)> {
    let from = all.partition_point(|(i, _)| *i < start);
    all[from..]
        .iter()
        .take_while(|(i, _)| *i < start + cols)
        .map(|(i, z)| (i - start, z.as_str()))
        .collect()
}

/// Split a row into segments; a wide char, or a char carrying zero-width
/// chars (`zw`: column, chars), ends a segment so that every other segment
/// can be shaped with a fixed per-glyph advance.
pub(super) fn segments(cells: &[CellSnap], zw: &[(usize, &str)]) -> Vec<Segment> {
    let blank = |col| Segment {
        start_col: col,
        fit_cells: 0,
        text: String::new(),
        runs: Vec::new(),
    };
    let mut out: Vec<Segment> = Vec::new();
    let mut cur = blank(0);
    for (col, cell) in cells.iter().enumerate() {
        if cell.c == '\0' {
            if cur.text.is_empty() {
                cur.start_col = col + 1;
            }
            continue;
        }
        let marks = zw
            .binary_search_by_key(&col, |(c, _)| *c)
            .ok()
            .map(|i| zw[i].1);
        // One emoji sequence spread over several cells stays one cluster,
        // so the font can draw it as one glyph (ui-editor-08).
        if cur.text.is_empty()
            && let Some(prev) = out.last_mut()
            && prev.fit_cells > 0
            && prev.start_col + prev.fit_cells == col
            && continues_cluster(&prev.text, cell.c)
        {
            let width = if cell.wide { 2 } else { 1 };
            push_char(prev, cell);
            if let Some(m) = marks {
                prev.text.push_str(m);
                if let Some((l, _)) = prev.runs.last_mut() {
                    *l += m.len();
                }
            }
            prev.fit_cells += width;
            cur = blank(col + width);
            continue;
        }
        if cell.wide || marks.is_some() {
            if !cur.text.is_empty() {
                out.push(std::mem::replace(&mut cur, blank(col)));
            }
            let width = if cell.wide { 2 } else { 1 };
            cur.start_col = col;
            cur.fit_cells = width;
            push_char(&mut cur, cell);
            if let Some(m) = marks {
                cur.text.push_str(m);
                if let Some((l, _)) = cur.runs.last_mut() {
                    *l += m.len();
                }
            }
            out.push(std::mem::replace(&mut cur, blank(col + width)));
            continue;
        }
        push_char(&mut cur, cell);
    }
    if !cur.text.is_empty() {
        out.push(cur);
    }
    out
}

fn push_char(seg: &mut Segment, cell: &CellSnap) {
    // Custom-drawn cells become spaces: same fixed advance, so the grid stays aligned.
    let c = if cell.c.is_control() || boxdraw::is_custom(cell.c) {
        ' '
    } else {
        cell.c
    };
    seg.text.push(c);
    let len = c.len_utf8();
    match seg.runs.last_mut() {
        Some((l, s)) if same_style(s, cell) => *l += len,
        _ => seg.runs.push((len, *cell)),
    }
}

fn styled_font(base: &Font, c: &CellSnap) -> Font {
    let mut f = base.clone();
    if c.bold {
        f.weight = FontWeight::BOLD;
    }
    if c.italic {
        f.style = FontStyle::Italic;
    }
    f
}
