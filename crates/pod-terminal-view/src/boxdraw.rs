//! Box-drawing (U+2500–U+257F) and block-element (U+2580–U+259F) glyphs,
//! painted as quads that fill the whole cell.
//!
//! Font glyphs for these code points are designed for a line height of about
//! 1.0–1.2x; with our 1.3x line height they leave gaps between rows (e.g. the
//! Claude Code logo). So the text path renders such cells as a space (keeping
//! the monospace advance) and this module paints them instead.
//!
//! Diagonals ╱╲╳ (U+2571–2573) are stroked corner to corner as paths, so
//! they join seamlessly across rows too.
//!
//! All geometry is computed in *device* pixels and snapped to integers, so
//! neighbouring cells share exact edges and anti-aliasing never shows a seam.

use gpui_kit::*;

/// True if `c` is painted here instead of being shaped as text.
#[inline]
pub fn is_custom(c: char) -> bool {
    matches!(c as u32, 0x2500..=0x259F)
}

/// Cell rectangle in device pixels.
#[derive(Clone, Copy)]
pub struct Cell {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

/// Device-pixel cell rectangle for (`col`, `row`) of a grid at `origin`.
pub fn cell_rect(
    origin: Point<Pixels>,
    cell_w: Pixels,
    line_h: Pixels,
    col: usize,
    row: usize,
    scale: f32,
) -> Cell {
    let snap = |v: f32| (v * scale).round() as i32;
    let (ox, oy) = (origin.x.as_f32(), origin.y.as_f32());
    let (w, h) = (cell_w.as_f32(), line_h.as_f32());
    Cell {
        x0: snap(ox + w * col as f32),
        x1: snap(ox + w * (col + 1) as f32),
        y0: snap(oy + h * row as f32),
        y1: snap(oy + h * (row + 1) as f32),
    }
}

/// Paint `c` (which must satisfy [`is_custom`]) into `cell` with color `fg`.
pub fn paint(c: char, cell: Cell, fg: Hsla, scale: f32, font_size: f32, window: &mut Window) {
    let cp = c as u32;
    let mut p = Painter {
        cell,
        fg,
        scale,
        light: ((font_size * scale / 13.0).round() as i32).max(1),
        window,
    };
    if cp >= 0x2580 {
        p.block(cp);
    } else {
        match LINES[(cp - 0x2500) as usize] {
            L::Arms(u, r, d, l) => p.arms([u, r, d, l]),
            L::Dash { heavy, vertical, n } => p.dashes(heavy, vertical, n),
            L::Arc(k) => p.arc(k),
            L::Diag(mask) => p.diagonal(mask),
        }
    }
}

struct Painter<'a> {
    cell: Cell,
    fg: Hsla,
    scale: f32,
    /// Light stroke width in device pixels.
    light: i32,
    window: &'a mut Window,
}

impl Painter<'_> {
    fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Hsla) {
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let s = self.scale;
        let b = Bounds::new(
            point(px(x0 as f32 / s), px(y0 as f32 / s)),
            size(px((x1 - x0) as f32 / s), px((y1 - y0) as f32 / s)),
        );
        self.window.paint_quad(fill(b, color));
    }

    /// Fill the fraction [fx0,fx1]×[fy0,fy1] of the cell.
    fn frac(&mut self, fx0: f32, fy0: f32, fx1: f32, fy1: f32) {
        let Cell { x0, y0, x1, y1 } = self.cell;
        let (w, h) = ((x1 - x0) as f32, (y1 - y0) as f32);
        let fx = |f: f32| x0 + (w * f).round() as i32;
        let fy = |f: f32| y0 + (h * f).round() as i32;
        self.rect(fx(fx0), fy(fy0), fx(fx1), fy(fy1), self.fg);
    }

    fn block(&mut self, cp: u32) {
        const UL: u8 = 1;
        const UR: u8 = 2;
        const LL: u8 = 4;
        const LR: u8 = 8;
        match cp {
            0x2580 => self.frac(0., 0., 1., 0.5),
            0x2581..=0x2588 => self.frac(0., 1. - (cp - 0x2580) as f32 / 8., 1., 1.),
            0x2589..=0x258F => self.frac(0., 0., (0x2590 - cp) as f32 / 8., 1.),
            0x2590 => self.frac(0.5, 0., 1., 1.),
            0x2591..=0x2593 => {
                let alpha = (cp - 0x2590) as f32 * 0.25;
                let Cell { x0, y0, x1, y1 } = self.cell;
                self.rect(x0, y0, x1, y1, self.fg.opacity(alpha));
            }
            0x2594 => self.frac(0., 0., 1., 1. / 8.),
            0x2595 => self.frac(7. / 8., 0., 1., 1.),
            _ => {
                let q = match cp {
                    0x2596 => LL,
                    0x2597 => LR,
                    0x2598 => UL,
                    0x2599 => UL | LL | LR,
                    0x259A => UL | LR,
                    0x259B => UL | UR | LL,
                    0x259C => UL | UR | LR,
                    0x259D => UR,
                    0x259E => UR | LL,
                    _ => UR | LL | LR, // 0x259F
                };
                for (bit, fx, fy) in [(UL, 0., 0.), (UR, 0.5, 0.), (LL, 0., 0.5), (LR, 0.5, 0.5)] {
                    if q & bit != 0 {
                        self.frac(fx, fy, fx + 0.5, fy + 0.5);
                    }
                }
            }
        }
    }

    fn center(&self) -> (i32, i32) {
        let Cell { x0, y0, x1, y1 } = self.cell;
        (x0 + (x1 - x0) / 2, y0 + (y1 - y0) / 2)
    }

    fn width(&self, w: u8) -> i32 {
        match w {
            HEAVY => self.light * 2,
            _ => self.light,
        }
    }

    /// Straight lines: each arm runs from the cell edge to (past) the center.
    fn arms(&mut self, a: [u8; 4]) {
        let [up, right, down, left] = a;
        let Cell { x0, y0, x1, y1 } = self.cell;
        let (xc, yc) = self.center();
        let t = self.light;
        // Double lines sit at center ± gap.
        let gap = t;
        let fg = self.fg;
        // Stroke extent [a, b) across an axis at `c` for an arm of weight `w`.
        let span = |w: u8, c: i32, p: &Self| -> (i32, i32) {
            if w == DOUBLE {
                (c - gap - t / 2, c + gap - t / 2 + t)
            } else {
                let tw = p.width(w);
                (c - tw / 2, c - tw / 2 + tw)
            }
        };
        // Band covered by the horizontal arms (in y) and the vertical arms (in x);
        // single/heavy perpendicular arms run up to its far edge so joins are square.
        let band = |a: u8, b: u8, c: i32, p: &Self| -> Option<(i32, i32)> {
            [a, b]
                .into_iter()
                .filter(|&w| w != 0)
                .map(|w| span(w, c, p))
                .reduce(|x, y| (x.0.min(y.0), x.1.max(y.1)))
        };
        let band_h = band(left, right, yc, self);
        let band_v = band(up, down, xc, self);

        // One line of a double arm, on the side whose perpendicular arm is `same`
        // (the other side's is `other`). Returns the [lo, hi) extent of the
        // perpendicular stroke it must reach, along the arm's axis.
        let dbl_end = |same: u8, c: i32, band: Option<(i32, i32)>, p: &Self| -> (i32, i32) {
            match same {
                // Inner corner: stop at the near line of the perpendicular double.
                DOUBLE => (c + gap - t / 2, c - gap - t / 2 + t),
                // Meet a single/heavy perpendicular across its full width.
                0 => band.unwrap_or_else(|| span(DOUBLE, c, p)),
                w => span(w, c, p),
            }
        };

        // Horizontal arms.
        for (w, dir) in [(left, -1), (right, 1)] {
            if w == 0 {
                continue;
            }
            if w == DOUBLE {
                for (s, same) in [(-1, up), (1, down)] {
                    let y = yc + s * gap;
                    let (ya, yb) = (y - t / 2, y - t / 2 + t);
                    let (lo, hi) = dbl_end(same, xc, band_v, self);
                    if dir > 0 {
                        self.rect(lo, ya, x1, yb, fg);
                    } else {
                        self.rect(x0, ya, hi, yb, fg);
                    }
                }
            } else {
                let (ya, yb) = span(w, yc, self);
                let (lo, hi) = band_v.unwrap_or((xc, xc));
                if dir > 0 {
                    self.rect(lo.min(xc), ya, x1, yb, fg);
                } else {
                    self.rect(x0, ya, hi.max(xc), yb, fg);
                }
            }
        }
        // Vertical arms.
        for (w, dir) in [(up, -1), (down, 1)] {
            if w == 0 {
                continue;
            }
            if w == DOUBLE {
                for (s, same) in [(-1, left), (1, right)] {
                    let x = xc + s * gap;
                    let (xa, xb) = (x - t / 2, x - t / 2 + t);
                    let (lo, hi) = dbl_end(same, yc, band_h, self);
                    if dir > 0 {
                        self.rect(xa, lo, xb, y1, fg);
                    } else {
                        self.rect(xa, y0, xb, hi, fg);
                    }
                }
            } else {
                let (xa, xb) = span(w, xc, self);
                let (lo, hi) = band_h.unwrap_or((yc, yc));
                if dir > 0 {
                    self.rect(xa, lo.min(yc), xb, y1, fg);
                } else {
                    self.rect(xa, y0, xb, hi.max(yc), fg);
                }
            }
        }
    }

    fn dashes(&mut self, heavy: bool, vertical: bool, n: i32) {
        let Cell { x0, y0, x1, y1 } = self.cell;
        let (xc, yc) = self.center();
        let tw = if heavy { self.light * 2 } else { self.light };
        let fg = self.fg;
        let (a, b) = if vertical { (y0, y1) } else { (x0, x1) };
        let len = b - a;
        // n dashes with n gaps; each gap is half a period, centered so
        // consecutive cells keep a regular rhythm.
        for i in 0..n {
            let s = a + len * (2 * i) / (2 * n) + len / (4 * n);
            let e = a + len * (2 * i + 1) / (2 * n) + len / (4 * n);
            if vertical {
                self.rect(xc - tw / 2, s, xc - tw / 2 + tw, e, fg);
            } else {
                self.rect(s, yc - tw / 2, e, yc - tw / 2 + tw, fg);
            }
        }
    }

    /// ╱ ╲ ╳: anti-aliased strokes from corner to corner, extended past the
    /// cell and clipped to it, so diagonal runs connect without gaps.
    fn diagonal(&mut self, mask: u8) {
        let Cell { x0, y0, x1, y1 } = self.cell;
        let s = self.scale;
        let d = |v: f32| px(v / s);
        let (fx0, fy0, fx1, fy1) = (x0 as f32, y0 as f32, x1 as f32, y1 as f32);
        let (w, h) = (fx1 - fx0, fy1 - fy0);
        // Extend the segment by 1/4 cell beyond both corners (same slope).
        let (ex, ey) = (w * 0.25, h * 0.25);
        let width = d(self.light as f32 * 1.2);
        let clip = Bounds::new(point(d(fx0), d(fy0)), size(d(w), d(h)));
        let fg = self.fg;
        let mut lines = Vec::new();
        if mask & 1 != 0 {
            lines.push(((fx0 - ex, fy1 + ey), (fx1 + ex, fy0 - ey)));
        }
        if mask & 2 != 0 {
            lines.push(((fx0 - ex, fy0 - ey), (fx1 + ex, fy1 + ey)));
        }
        self.window
            .with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                for ((ax, ay), (bx, by)) in lines {
                    let mut path = PathBuilder::stroke(width);
                    path.move_to(point(d(ax), d(ay)));
                    path.line_to(point(d(bx), d(by)));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, fg);
                    }
                }
            });
    }

    /// Rounded corners ╭╮╯╰: a bordered, rounded quad clipped to the cell.
    fn arc(&mut self, k: u8) {
        let Cell { x0, y0, x1, y1 } = self.cell;
        let (xc, yc) = self.center();
        let t = self.light;
        let (cw, ch) = (x1 - x0, y1 - y0);
        let r = cw.min(ch) / 2 + t / 2;
        // Big quad whose one rounded corner sits at the cell center.
        let (w, h) = (cw + t, ch + t);
        let (left_edge, top_edge) = (xc - t / 2, yc - t / 2);
        let (qx, qy, border, corners) = match k {
            // ╭ arc down and right: top-left corner
            0 => (
                left_edge,
                top_edge,
                Edges {
                    top: t,
                    left: t,
                    ..Default::default()
                },
                (r, 0, 0, 0),
            ),
            // ╮ arc down and left: top-right corner
            1 => (
                left_edge + t - w,
                top_edge,
                Edges {
                    top: t,
                    right: t,
                    ..Default::default()
                },
                (0, r, 0, 0),
            ),
            // ╯ arc up and left: bottom-right corner
            2 => (
                left_edge + t - w,
                top_edge + t - h,
                Edges {
                    bottom: t,
                    right: t,
                    ..Default::default()
                },
                (0, 0, r, 0),
            ),
            // ╰ arc up and right: bottom-left corner
            _ => (
                left_edge,
                top_edge + t - h,
                Edges {
                    bottom: t,
                    left: t,
                    ..Default::default()
                },
                (0, 0, 0, r),
            ),
        };
        let s = self.scale;
        let d = |v: i32| px(v as f32 / s);
        let q = quad(
            Bounds::new(point(d(qx), d(qy)), size(d(w), d(h))),
            Corners {
                top_left: d(corners.0),
                top_right: d(corners.1),
                bottom_right: d(corners.2),
                bottom_left: d(corners.3),
            },
            transparent_black(),
            Edges {
                top: d(border.top),
                right: d(border.right),
                bottom: d(border.bottom),
                left: d(border.left),
            },
            self.fg,
            BorderStyle::Solid,
        );
        let clip = Bounds::new(point(d(x0), d(y0)), size(d(cw), d(ch)));
        self.window
            .with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                window.paint_quad(q)
            });
    }
}

const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

#[derive(Clone, Copy)]
enum L {
    /// Weights of the (up, right, down, left) arms: 0 none, 1 light, 2 heavy, 3 double.
    Arms(u8, u8, u8, u8),
    Dash {
        heavy: bool,
        vertical: bool,
        n: i32,
    },
    /// 0 ╭, 1 ╮, 2 ╯, 3 ╰
    Arc(u8),
    /// Diagonals: bit 1 = ╱ (lower-left to upper-right), bit 2 = ╲.
    Diag(u8),
}

const fn a(s: &[u8; 4]) -> L {
    L::Arms(s[0] - b'0', s[1] - b'0', s[2] - b'0', s[3] - b'0')
}
const fn dash(heavy: bool, vertical: bool, n: i32) -> L {
    L::Dash { heavy, vertical, n }
}

/// U+2500..=U+257F, arms as "urdl" digits.
const LINES: [L; 128] = [
    a(b"0101"),
    a(b"0202"),
    a(b"1010"),
    a(b"2020"), // 2500 ─━│┃
    dash(false, false, 3),
    dash(true, false, 3),
    dash(false, true, 3),
    dash(true, true, 3), // 2504 ┄┅┆┇
    dash(false, false, 4),
    dash(true, false, 4),
    dash(false, true, 4),
    dash(true, true, 4), // 2508 ┈┉┊┋
    a(b"0110"),
    a(b"0210"),
    a(b"0120"),
    a(b"0220"), // 250C ┌┍┎┏
    a(b"0011"),
    a(b"0012"),
    a(b"0021"),
    a(b"0022"), // 2510 ┐┑┒┓
    a(b"1100"),
    a(b"1200"),
    a(b"2100"),
    a(b"2200"), // 2514 └┕┖┗
    a(b"1001"),
    a(b"1002"),
    a(b"2001"),
    a(b"2002"), // 2518 ┘┙┚┛
    a(b"1110"),
    a(b"1210"),
    a(b"2110"),
    a(b"1120"), // 251C ├┝┞┟
    a(b"2120"),
    a(b"2210"),
    a(b"1220"),
    a(b"2220"), // 2520 ┠┡┢┣
    a(b"1011"),
    a(b"1012"),
    a(b"2011"),
    a(b"1021"), // 2524 ┤┥┦┧
    a(b"2021"),
    a(b"2012"),
    a(b"1022"),
    a(b"2022"), // 2528 ┨┩┪┫
    a(b"0111"),
    a(b"0112"),
    a(b"0211"),
    a(b"0212"), // 252C ┬┭┮┯
    a(b"0121"),
    a(b"0122"),
    a(b"0221"),
    a(b"0222"), // 2530 ┰┱┲┳
    a(b"1101"),
    a(b"1102"),
    a(b"1201"),
    a(b"1202"), // 2534 ┴┵┶┷
    a(b"2101"),
    a(b"2102"),
    a(b"2201"),
    a(b"2202"), // 2538 ┸┹┺┻
    a(b"1111"),
    a(b"1112"),
    a(b"1211"),
    a(b"1212"), // 253C ┼┽┾┿
    a(b"2111"),
    a(b"1121"),
    a(b"2121"),
    a(b"2112"), // 2540 ╀╁╂╃
    a(b"2211"),
    a(b"1122"),
    a(b"1221"),
    a(b"2212"), // 2544 ╄╅╆╇
    a(b"1222"),
    a(b"2122"),
    a(b"2221"),
    a(b"2222"), // 2548 ╈╉╊╋
    dash(false, false, 2),
    dash(true, false, 2),
    dash(false, true, 2),
    dash(true, true, 2), // 254C ╌╍╎╏
    a(b"0303"),
    a(b"3030"),
    a(b"0310"),
    a(b"0130"), // 2550 ═║╒╓
    a(b"0330"),
    a(b"0013"),
    a(b"0031"),
    a(b"0033"), // 2554 ╔╕╖╗
    a(b"1300"),
    a(b"3100"),
    a(b"3300"),
    a(b"1003"), // 2558 ╘╙╚╛
    a(b"3001"),
    a(b"3003"),
    a(b"1310"),
    a(b"3130"), // 255C ╜╝╞╟
    a(b"3330"),
    a(b"1013"),
    a(b"3031"),
    a(b"3033"), // 2560 ╠╡╢╣
    a(b"0313"),
    a(b"0131"),
    a(b"0333"),
    a(b"1303"), // 2564 ╤╥╦╧
    a(b"3101"),
    a(b"3303"),
    a(b"1313"),
    a(b"3131"), // 2568 ╨╩╪╫
    a(b"3333"),
    L::Arc(0),
    L::Arc(1),
    L::Arc(2), // 256C ╬╭╮╯
    L::Arc(3),
    L::Diag(1),
    L::Diag(2),
    L::Diag(3), // 2570 ╰╱╲╳
    a(b"0001"),
    a(b"1000"),
    a(b"0100"),
    a(b"0010"), // 2574 ╴╵╶╷
    a(b"0002"),
    a(b"2000"),
    a(b"0200"),
    a(b"0020"), // 2578 ╸╹╺╻
    a(b"0201"),
    a(b"1020"),
    a(b"0102"),
    a(b"2010"), // 257C ╼╽╾╿
];
