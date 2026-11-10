//! Custom GPUI element that paints a [`TerminalView`]'s grid and wires mouse
//! and IME input back to it.
//!
//! Strategy (validated by pod-spike):
//! - snapshot the VT grid once per render, then paint from the copy
//! - background: merge horizontal runs of equal bg into one quad
//! - text: one shaped line per (row, segment); segments break only at wide
//!   chars, so the monospace grid stays aligned via `force_width`
//! - box-drawing / block glyphs (U+2500–U+259F) are painted as cell-sized
//!   quads (`boxdraw`) and appear as spaces in the shaped text
//! - GPUI's line layout cache makes unchanged rows cheap frame-to-frame
//! - the element only runs when its view re-renders: hosts embed the view
//!   with `Entity::cached` so idle terminals reuse last frame's paint

use std::time::Instant;

use gpui_kit::*;
use unicode_width::UnicodeWidthChar;

use crate::boxdraw;
use crate::palette;
use crate::selection::{SelPoint, row_span};
use crate::view::{LayoutInfo, TerminalView};
use crate::vt::{CellSnap, CursorShape, GridSize, Snapshot};

pub struct TerminalElement {
    view: Entity<TerminalView>,
    focus: FocusHandle,
    focused: bool,
}

impl TerminalElement {
    pub fn new(view: Entity<TerminalView>, focus: FocusHandle, focused: bool) -> Self {
        Self {
            view,
            focus,
            focused,
        }
    }
}

#[derive(Clone)]
pub struct Metrics {
    pub cell_w: Pixels,
    pub line_h: Pixels,
    pub font: Font,
    pub font_size: Pixels,
}

pub fn metrics(
    family: &SharedString,
    font_size: f32,
    line_height: f32,
    window: &Window,
) -> Metrics {
    let font = font(family.clone());
    let ts = window.text_system();
    let font_id = ts.resolve_font(&font);
    let size = px(font_size);
    let cell_w = ts
        .advance(font_id, size, 'm')
        .map(|s| s.width)
        .unwrap_or(px(font_size * 0.6));
    let line_h = px((font_size * line_height).round());
    Metrics {
        cell_w,
        line_h,
        font,
        font_size: size,
    }
}

pub struct Prepared {
    snapshot: Snapshot,
    metrics: Metrics,
    /// Selection as an absolute-line range plus the absolute line of screen row 0.
    selection: Option<(SelPoint, SelPoint)>,
    top_abs: i64,
    marked: Option<String>,
    /// Ctrl-hovered link: underlined, hand cursor.
    link: Option<(SelPoint, SelPoint)>,
    hitbox: Hitbox,
}

impl IntoElement for TerminalElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepared;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepared {
        let config = self.view.read(cx).config().clone();
        let metrics = metrics(
            &config.font_family,
            config.font_size,
            config.line_height,
            window,
        );
        let cols = ((bounds.size.width / metrics.cell_w).floor() as u16).max(2);
        let rows = ((bounds.size.height / metrics.line_h).floor() as u16).max(1);
        let cell_px = (
            metrics.cell_w.as_f32().round() as u16,
            metrics.line_h.as_f32().round() as u16,
        );
        let hitbox = window.insert_hitbox(bounds, HitboxBehavior::Normal);
        let (snapshot, selection, top_abs, marked, link) = self.view.update(cx, |view, _| {
            view.resize_grid(GridSize { cols, rows }, cell_px);
            let snapshot = view.vt().snapshot();
            view.layout = Some(LayoutInfo {
                origin: bounds.origin,
                cell_w: metrics.cell_w,
                line_h: metrics.line_h,
                size: snapshot.size,
                cursor: snapshot.cursor_point,
            });
            let top_abs = snapshot.history_size as i64 - snapshot.display_offset as i64;
            let link = view.hovered_link().map(|l| (l.start, l.end));
            (
                snapshot,
                view.selection_range(),
                top_abs,
                view.marked_text().map(str::to_string),
                link,
            )
        });
        Prepared {
            snapshot,
            metrics,
            selection,
            top_abs,
            marked,
            link,
            hitbox,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepared: &mut Prepared,
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        let Prepared {
            snapshot,
            metrics,
            selection,
            top_abs,
            marked,
            link,
            hitbox,
        } = prepared;
        let Metrics {
            cell_w,
            line_h,
            font,
            font_size,
        } = metrics.clone();
        let cols = snapshot.size.cols as usize;
        let origin = bounds.origin;
        let scale = window.scale_factor();

        window.set_cursor_style(
            if link.is_some() {
                CursorStyle::PointingHand
            } else {
                CursorStyle::IBeam
            },
            hitbox,
        );
        window.paint_quad(fill(bounds, hsla_of(snapshot.default_bg)));

        let snap_rect = |c0: usize, c1: usize, row: usize| {
            let a = boxdraw::cell_rect(origin, cell_w, line_h, c0, row, scale);
            let z = boxdraw::cell_rect(origin, cell_w, line_h, c1, row, scale);
            Bounds::from_corners(
                point(px(a.x0 as f32 / scale), px(a.y0 as f32 / scale)),
                point(px(z.x1 as f32 / scale), px(z.y1 as f32 / scale)),
            )
        };

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (row, cells) in snapshot.cells.chunks(cols).enumerate() {
                let zw = row_zerowidth(&snapshot.zerowidth, row * cols, cols);
                let y = origin.y + line_h * row as f32;

                // Background runs (device-pixel snapped so no sub-pixel slivers).
                let mut col = 0;
                while col < cells.len() {
                    let bg = cells[col].bg;
                    let start = col;
                    while col < cells.len() && cells[col].bg == bg {
                        col += 1;
                    }
                    if bg != snapshot.default_bg {
                        window.paint_quad(fill(snap_rect(start, col - 1, row), hsla_of(bg)));
                    }
                }

                // Selection highlight, under the text.
                if let Some(range) = selection
                    && let Some((c0, c1)) = row_span(*range, *top_abs + row as i64, cols as u16)
                {
                    window.paint_quad(fill(
                        snap_rect(c0 as usize, c1 as usize, row),
                        hsla_of(self.view.read(cx).palette.selection_bg),
                    ));
                }

                // Box-drawing / block cells: full-cell quads, not font glyphs.
                for (col, cell) in cells.iter().enumerate() {
                    if boxdraw::is_custom(cell.c) {
                        let r = boxdraw::cell_rect(origin, cell_w, line_h, col, row, scale);
                        boxdraw::paint(
                            cell.c,
                            r,
                            hsla_of(cell.fg),
                            scale,
                            font_size.as_f32(),
                            window,
                        );
                    }
                }

                // Ctrl-hovered link: underline in the text color of its first cell.
                if let Some(range) = link
                    && let Some((c0, c1)) = row_span(*range, *top_abs + row as i64, cols as u16)
                {
                    let r = snap_rect(c0 as usize, c1 as usize, row);
                    let color = cells.get(c0 as usize).map_or(snapshot.default_fg, |c| c.fg);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(r.origin.x, r.origin.y + r.size.height - px(1.5)),
                            size(r.size.width, px(1.)),
                        ),
                        hsla_of(color),
                    ));
                }

                // Text segments.
                for seg in segments(cells, &zw) {
                    if seg.text.trim().is_empty() {
                        continue;
                    }
                    let runs: Vec<TextRun> = seg
                        .runs
                        .iter()
                        .map(|(len, c)| text_run(*len, &font, c, c.fg))
                        .collect();
                    let (line, dx) = shape_cells(
                        SharedString::from(seg.text),
                        &runs,
                        font_size,
                        cell_w,
                        seg.fit_cells,
                        window,
                    );
                    let x = origin.x + cell_w * seg.start_col as f32 + dx;
                    let _ = line.paint(point(x, y), line_h, TextAlign::Left, None, window, cx);
                }
            }

            if let Some(text) = marked.as_deref() {
                let at = snapshot.cursor_point.unwrap_or((0, 0));
                paint_preedit(
                    text, at, origin, cell_w, line_h, font_size, &font, snapshot, window, cx,
                );
            } else if let Some((ccol, crow, shape)) = snapshot.cursor {
                paint_cursor(
                    self.focused,
                    ccol,
                    crow,
                    shape,
                    origin,
                    cell_w,
                    line_h,
                    font_size,
                    &font,
                    snapshot,
                    window,
                    cx,
                );
            }

            paint_scrollbar(bounds, snapshot, window);
        });

        // IME / text input: only registered while focused (GPUI checks).
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        self.register_mouse(hitbox.clone(), window);

        crate::perf::record_paint(started.elapsed());
    }
}

impl TerminalElement {
    fn register_mouse(&self, hitbox: Hitbox, window: &mut Window) {
        let view = self.view.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |e: &MouseDownEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble && hb.is_hovered(window) {
                view.update(cx, |v, cx| v.mouse_down(e, window, cx));
            }
        });
        let view = self.view.clone();
        let hb = hitbox.clone();
        window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                let hovered = hb.is_hovered(window);
                view.update(cx, |v, cx| v.mouse_move(e, hovered, window, cx));
            }
        });
        let view = self.view.clone();
        window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
            if phase == DispatchPhase::Bubble {
                view.update(cx, |v, cx| v.mouse_up(e, window, cx));
            }
        });
        let view = self.view.clone();
        let hb = hitbox;
        window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
            // Not `is_hovered`: that is false in keyboard modality (right after typing,
            // before the mouse moves), and the wheel must still scroll then.
            if phase == DispatchPhase::Bubble && hb.should_handle_scroll(window) {
                view.update(cx, |v, cx| v.scroll_wheel(e, window, cx));
                cx.stop_propagation();
            }
        });
    }
}

fn text_run(len: usize, font: &Font, c: &CellSnap, color: u32) -> TextRun {
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

#[allow(clippy::too_many_arguments)]
fn paint_cursor(
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

/// IME composition text drawn over the grid at the cursor, underlined.
#[allow(clippy::too_many_arguments)]
fn paint_preedit(
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

/// Thin overlay scrollbar on the right edge while there is scrollback.
fn paint_scrollbar(bounds: Bounds<Pixels>, snapshot: &Snapshot, window: &mut Window) {
    let history = snapshot.history_size;
    if history == 0 {
        return;
    }
    let rows = snapshot.size.rows as f32;
    let total = history as f32 + rows;
    let h = bounds.size.height;
    let thumb_h = (h * (rows / total)).max(px(16.));
    let top_line = (history - snapshot.display_offset.min(history)) as f32;
    let travel = h - thumb_h;
    let y = bounds.origin.y + travel * (top_line / history as f32);
    let w = px(4.);
    let x = bounds.origin.x + bounds.size.width - w - px(1.);
    let alpha = if snapshot.display_offset > 0 {
        0.75
    } else {
        0.3
    };
    window.paint_quad(fill(
        Bounds::new(point(x, bounds.origin.y), size(w, h)),
        hsla_of(palette::SCROLLBAR).opacity(0.08),
    ));
    window.paint_quad(
        fill(
            Bounds::new(point(x, y), size(w, thumb_h)),
            hsla_of(palette::SCROLLBAR).opacity(alpha),
        )
        .corner_radii(px(2.)),
    );
}

/// Shape grid text. Narrow cells get a fixed `cell_w` advance per glyph. A
/// wide (2-cell) char is shaped at its natural advance and placed by
/// [`wide_fit`]; returns the line and the x offset inside its first cell.
/// Shapes a segment. `fit_cells` 0: a run of narrow chars on the cell grid
/// (fixed advance); 1 or 2: one glyph cluster (a wide char, or a char with
/// zero-width marks, which must not get a cell advance each) fitted into
/// that many cells; more: an emoji sequence joined across cells (ZWJ, skin
/// tone, flag), see [`cluster_fit`].
fn shape_cells(
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
const WIDE_MAX_GROW: f32 = 1.35;

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
pub(crate) fn wide_fit(advance: f32, ink: Option<(f32, f32)>, slot: f32, scale: f32) -> (f32, f32) {
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
pub(crate) fn cluster_fit(advance: f32, slot: f32, scale: f32) -> (f32, f32) {
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

struct Segment {
    start_col: usize,
    /// 0: narrow chars on the grid; 1/2: one cluster fitted into that many
    /// cells (a wide char, or a char with zero-width marks).
    fit_cells: usize,
    text: String,
    /// (byte length, style source cell)
    runs: Vec<(usize, CellSnap)>,
}

fn same_style(a: &CellSnap, b: &CellSnap) -> bool {
    a.fg == b.fg
        && a.bold == b.bold
        && a.italic == b.italic
        && a.underline == b.underline
        && a.strikeout == b.strikeout
}

/// The zero-width chars of one row (`start..start+cols` of the grid), by column.
fn row_zerowidth(all: &[(usize, String)], start: usize, cols: usize) -> Vec<(usize, &str)> {
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
fn segments(cells: &[CellSnap], zw: &[(usize, &str)]) -> Vec<Segment> {
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

pub fn hsla_of(c: u32) -> Hsla {
    rgb(c).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    // GPUI's glob exports its own `test` attribute; these are plain Rust tests.
    use core::prelude::v1::test;

    fn cell(c: char, wide: bool) -> CellSnap {
        CellSnap {
            c,
            fg: 0xffffff,
            bg: 0,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            wide,
        }
    }

    /// Slot of two Cascadia Mono cells at 16 px (cell 9.375 px).
    const SLOT: f32 = 18.75;

    /// Ink of a placed glyph inside the slot, in device pixels.
    fn placed_ink(advance: f32, ink: (f32, f32), scale: f32) -> (f32, f32) {
        let (dx, k) = wide_fit(advance, Some(ink), SLOT, scale);
        ((dx + ink.0 * k) * scale, (dx + (ink.0 + ink.1) * k) * scale)
    }

    #[test]
    fn wide_glyphs_fill_their_two_cells() {
        // Hangul, kana and kanji from the fallback font: 1 em of advance → grown to fill the slot.
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let (dx, k) = wide_fit(16., Some((1., 14.)), SLOT, scale);
            assert!((16. * k - SLOT).abs() < 0.01, "{k}");
            assert_eq!(dx, 0.);
        }
        // Very narrow glyphs grow at most 1.35× and are centered, snapped to device pixels.
        for scale in [1.0, 1.5] {
            let (dx, k) = wide_fit(10., None, 22.85, scale);
            assert_eq!(k, WIDE_MAX_GROW);
            assert!((dx - (22.85 - 13.5) / 2.).abs() <= 0.5 / scale, "{dx}");
            assert!(
                ((dx * scale).round() - dx * scale).abs() < 1e-4,
                "snapped to device pixels"
            );
        }
        // Exactly two cells: unchanged.
        assert_eq!(wide_fit(22.85, None, 22.85, 1.0), (0., 1.));
    }

    #[test]
    fn one_scale_per_font_whatever_the_ink() {
        // Kana ink is narrower than kanji ink in the same font; both must get the
        // same scale and origin (ink-based fitting would blow kana up, so a line
        // of kana and kanji would change size glyph by glyph).
        let kana = wide_fit(16., Some((3., 9.5)), SLOT, 1.5);
        let kanji = wide_fit(16., Some((0.7, 14.6)), SLOT, 1.5);
        assert_eq!(kana, kanji);
    }

    #[test]
    fn emoji_keep_their_size_and_center_by_ink() {
        // Segoe UI Emoji at 16 px: advance 21.97 (> slot), ink ≈ 17 px.
        for scale in [1.0, 1.5] {
            let (dx, k) = wide_fit(21.97, Some((2.5, 17.)), SLOT, scale);
            assert_eq!(k, 1., "ink fits: natural size, not 0.85×");
            assert!(dx < 0., "origin moves left so the ink is centered: {dx}");
            let (l, r) = placed_ink(21.97, (2.5, 17.), scale);
            let slot = SLOT * scale;
            assert!(l >= 0. && r <= slot, "inside the slot: {l}..{r} of {slot}");
            assert!(((slot - r) - l).abs() <= 1., "centered: {l} / {}", slot - r);
        }
    }

    #[test]
    fn wider_glyphs_shrink_to_the_slot() {
        // Ink wider than the slot: shrink the ink to the slot.
        let (l, r) = placed_ink(30., (2., 26.), 1.0);
        assert!(l.abs() < 0.5 && (r - SLOT).abs() < 0.5, "{l}..{r}");
        // No ink box: shrink by advance.
        let (dx, k) = wide_fit(30., None, 24., 1.0);
        assert_eq!(dx, 0.);
        assert!((k - 0.8).abs() < 1e-6);
        // Degenerate inputs leave the glyph alone.
        assert_eq!(wide_fit(0., None, 24., 1.0), (0., 1.));
        assert_eq!(wide_fit(30., Some((0., 0.)), 24., 1.0).1, 0.8);
    }

    #[test]
    fn segments_isolate_wide_chars() {
        // "문서: a" — 문(2) 서(2) ':' ' ' 'a'
        let cells = [
            cell('문', true),
            cell('\0', false),
            cell('서', true),
            cell('\0', false),
            cell(':', false),
            cell(' ', false),
            cell('a', false),
        ];
        let segs = segments(&cells, &[]);
        let shape: Vec<(usize, usize, &str)> = segs
            .iter()
            .map(|s| (s.start_col, s.fit_cells, s.text.as_str()))
            .collect();
        assert_eq!(shape, vec![(0, 2, "문"), (2, 2, "서"), (4, 0, ": a")]);
    }

    /// editor-12: a char with zero-width marks is one cluster in its cells,
    /// never a cell advance per mark.
    #[test]
    fn segments_keep_zero_width_marks_with_their_cell() {
        // a é(e + U+0301) b, then a wide emoji with a ZWJ sequence.
        let cells = [
            cell('a', false),
            cell('e', false),
            cell('b', false),
            cell('👨', true),
            cell('\0', false),
            cell('c', false),
        ];
        let zw = [(1, "\u{301}"), (3, "\u{200D}👩")];
        let segs = segments(&cells, &zw);
        let shape: Vec<(usize, usize, &str)> = segs
            .iter()
            .map(|s| (s.start_col, s.fit_cells, s.text.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, 0, "a"),
                (1, 1, "e\u{301}"),
                (2, 0, "b"),
                (3, 2, "👨\u{200D}👩"),
                (5, 0, "c")
            ]
        );
        assert_eq!(
            segs[1].runs.iter().map(|r| r.0).sum::<usize>(),
            segs[1].text.len(),
            "runs cover the marks"
        );
        let all = vec![
            (1, "x".to_string()),
            (12, "y".to_string()),
            (25, "z".to_string()),
        ];
        assert_eq!(row_zerowidth(&all, 10, 10), vec![(2, "y")]);
    }

    /// ui-editor-08: alacritty puts each emoji of a sequence in its own
    /// cells and hangs the ZWJ on the cell before. A family (ZWJ), a skin
    /// tone and a flag each stay one shaping cluster over all their cells;
    /// an unrelated wide char next to them does not join.
    #[test]
    fn emoji_sequences_across_cells_are_one_cluster() {
        let cells = [
            cell('👨', true),
            cell('\0', false),
            cell('👩', true),
            cell('\0', false),
            cell('👧', true),
            cell('\0', false),
            cell('👍', true),
            cell('\0', false),
            cell('🏽', true),
            cell('\0', false),
            cell('🇰', true),
            cell('\0', false),
            cell('🇷', true),
            cell('\0', false),
            cell('🇰', true),
            cell('\0', false),
            cell('문', true),
            cell('\0', false),
            cell('a', false),
        ];
        let zw = [(0, "\u{200D}"), (2, "\u{200D}")];
        let segs = segments(&cells, &zw);
        let shape: Vec<(usize, usize, &str)> = segs
            .iter()
            .map(|s| (s.start_col, s.fit_cells, s.text.as_str()))
            .collect();
        assert_eq!(
            shape,
            vec![
                (0, 6, "👨\u{200D}👩\u{200D}👧"),
                (6, 4, "👍🏽"),
                (10, 4, "🇰🇷"),
                (14, 2, "🇰"),
                (16, 2, "문"),
                (18, 0, "a"),
            ]
        );
        assert_eq!(
            segs[0].runs.iter().map(|r| r.0).sum::<usize>(),
            segs[0].text.len(),
            "runs cover the whole sequence"
        );
    }

    #[test]
    fn joined_cluster_is_centered_never_enlarged() {
        // One combined glyph (2 cells of advance) in 6 cells: natural size, centered.
        assert_eq!(cluster_fit(20., 60., 1.), (20., 1.));
        // Parts drawn side by side (wider than the slot): shrunk to fit.
        let (dx, k) = cluster_fit(90., 60., 1.);
        assert_eq!(dx, 0.);
        assert!((k - 60. / 90.).abs() < 1e-6);
        assert_eq!(cluster_fit(0., 60., 1.), (0., 1.));
    }
}
