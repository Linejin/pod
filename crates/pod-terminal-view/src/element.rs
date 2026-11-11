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
//!
//! Paint helpers live in child modules: row segments, shaping and wide-glyph
//! fitting in `element/text.rs`, the cursor in `element/cursor.rs`, the IME
//! preedit in `element/preedit.rs`.

mod cursor;
mod preedit;
#[cfg(test)]
mod tests;
mod text;

use std::time::Instant;

use gpui_kit::*;

use crate::boxdraw;
use crate::palette;
use crate::selection::{SelPoint, row_span};
use crate::view::{LayoutInfo, TerminalView};
use crate::vt::{GridSize, Snapshot};
use cursor::paint_cursor;
use preedit::paint_preedit;
use text::{row_zerowidth, segments, shape_cells, text_run};

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

pub fn hsla_of(c: u32) -> Hsla {
    rgb(c).into()
}
