//! Window-free parts of the view: absolute-line row access (selection and
//! links read the VT through it) and window position → cell mapping (mouse
//! reports, selection, Ctrl+hover). Keys, IME and Ctrl+click on a live view
//! are proven by pod-app's native QA (`native_qa::ime`, `native_qa::links`,
//! `native_qa::keyboard`).

use gpui_kit::{point, px};

use super::{LayoutInfo, VtRows};
use crate::links::{LinkTarget, links_in};
use crate::selection::{RowSource, SelMode, SelPoint, Selection};
use crate::vt::{AlacrittyVt, GridSize, VtEngine};

fn text(src: &dyn RowSource, line: i64) -> Option<String> {
    let row = src.row(line)?;
    Some(
        row.cells
            .iter()
            .filter(|c| **c != '\0')
            .collect::<String>()
            .trim_end()
            .to_string(),
    )
}

/// Five lines on a three-row screen: two of them in the scrollback.
fn scrolled_vt() -> AlacrittyVt {
    let mut vt = AlacrittyVt::new(GridSize { cols: 40, rows: 3 }, 100);
    vt.feed(b"one\r\nsee https://example.com/a\r\nthree\r\nfour\r\nfive");
    assert_eq!(vt.history_size(), 2);
    vt
}

#[test]
fn rows_are_read_by_absolute_line_from_the_oldest_scrollback_line() {
    let vt = scrolled_vt();
    let rows = VtRows(&vt);
    assert_eq!(rows.cols(), 40);
    let lines: Vec<Option<String>> = (0..5).map(|l| text(&rows, l)).collect();
    assert_eq!(
        lines,
        ["one", "see https://example.com/a", "three", "four", "five"].map(|s| Some(s.into()))
    );
    assert_eq!(text(&rows, -1), None, "before the oldest line");
    assert_eq!(text(&rows, 5), None, "below the screen");
}

#[test]
fn absolute_lines_stay_put_while_the_view_is_scrolled_back() {
    let mut vt = scrolled_vt();
    vt.scroll_display(2);
    assert_eq!(vt.display_offset(), 2);
    let rows = VtRows(&vt);
    assert_eq!(text(&rows, 0).as_deref(), Some("one"));
    assert_eq!(text(&rows, 4).as_deref(), Some("five"));
    // A selection over the scrollback copies the same text either way.
    let sel = Selection {
        mode: SelMode::Simple,
        anchor: SelPoint::new(0, 0),
        head: SelPoint::new(2, 4),
    };
    assert_eq!(sel.to_text(&rows), "one\nsee https://example.com/a\nthree");
}

#[test]
fn links_in_the_scrollback_carry_their_absolute_line() {
    let vt = scrolled_vt();
    let links = links_in(&VtRows(&vt), 0, 3);
    assert_eq!(links.len(), 1, "{links:?}");
    assert_eq!(
        links[0].target,
        LinkTarget::Url("https://example.com/a".into())
    );
    assert_eq!(
        (links[0].start, links[0].end),
        (SelPoint::new(1, 4), SelPoint::new(1, 24))
    );
}

fn layout(cols: u16, rows: u16) -> LayoutInfo {
    LayoutInfo {
        origin: point(px(10.), px(20.)),
        cell_w: px(8.),
        line_h: px(16.),
        size: GridSize { cols, rows },
        cursor: None,
    }
}

#[test]
fn window_positions_map_to_cells_clamped_to_the_grid() {
    let l = layout(80, 24);
    assert_eq!(l.cell_at(point(px(10.), px(20.))), (0, 0, true));
    assert_eq!(l.cell_at(point(px(17.5), px(35.5))), (0, 0, true));
    assert_eq!(l.cell_at(point(px(18.), px(36.))), (1, 1, true));
    assert_eq!(
        l.cell_at(point(px(10. + 8. * 79.5), px(20. + 16. * 23.5))),
        (79, 23, true)
    );
    // Outside: clamped to the nearest cell (drags past the edge keep selecting).
    assert_eq!(l.cell_at(point(px(0.), px(0.))), (0, 0, false));
    assert_eq!(l.cell_at(point(px(9.9), px(30.))), (0, 0, false));
    assert_eq!(
        l.cell_at(point(px(10. + 8. * 80.), px(30.))),
        (79, 0, false)
    );
    assert_eq!(
        l.cell_at(point(px(30.), px(20. + 16. * 24.))),
        (2, 23, false)
    );
    // An empty grid still maps to cell 0.
    assert_eq!(layout(0, 0).cell_at(point(px(50.), px(50.))), (0, 0, false));
}
