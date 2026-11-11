use super::text::{WIDE_MAX_GROW, cluster_fit, row_zerowidth, segments, wide_fit};
use crate::vt::CellSnap;

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
