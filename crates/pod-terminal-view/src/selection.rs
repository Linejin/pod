//! Mouse selection model and selection → text.
//!
//! Points use *absolute* line numbers: 0 is the oldest scrollback line, so a
//! selection stays on the same text while new output scrolls the screen
//! (`abs = vt_line + history_size`).

use crate::vt::RowText;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SelPoint {
    pub line: i64,
    pub col: u16,
}

impl SelPoint {
    pub fn new(line: i64, col: u16) -> Self {
        Self { line, col }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelMode {
    /// Drag: cell to cell.
    Simple,
    /// Double click: whole words.
    Word,
    /// Triple click: whole (soft-wrapped) lines.
    Line,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub mode: SelMode,
    pub anchor: SelPoint,
    pub head: SelPoint,
}

/// Grid text access by absolute line.
pub trait RowSource {
    fn cols(&self) -> u16;
    fn row(&self, line: i64) -> Option<RowText>;
}

/// Characters that end a word on double click (alacritty's default set).
pub const WORD_DELIMITERS: &str = ",│`|:\"' ()[]{}<>\t";

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Delim,
    Word,
}

fn class(c: char) -> Class {
    if c == ' ' || c == '\t' {
        Class::Space
    } else if WORD_DELIMITERS.contains(c) {
        Class::Delim
    } else {
        // Includes '\0' (wide-char spacer): part of the wide char's word.
        Class::Word
    }
}

impl Selection {
    pub fn new(mode: SelMode, at: SelPoint) -> Self {
        Self {
            mode,
            anchor: at,
            head: at,
        }
    }

    /// Normalized inclusive range after word/line expansion.
    pub fn range(&self, src: &dyn RowSource) -> (SelPoint, SelPoint) {
        let (mut start, mut end) = if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        };
        let last_col = src.cols().saturating_sub(1);
        match self.mode {
            SelMode::Simple => {}
            SelMode::Word => {
                if let Some(row) = src.row(start.line) {
                    start.col = word_bounds(&row.cells, start.col).0;
                }
                if let Some(row) = src.row(end.line) {
                    end.col = word_bounds(&row.cells, end.col).1;
                }
            }
            SelMode::Line => {
                while start.line > 0 && src.row(start.line - 1).is_some_and(|r| r.wrapped) {
                    start.line -= 1;
                }
                while src.row(end.line).is_some_and(|r| r.wrapped)
                    && src.row(end.line + 1).is_some()
                {
                    end.line += 1;
                }
                start.col = 0;
                end.col = last_col;
            }
        }
        // Never start on the spacer half of a wide char.
        if let Some(row) = src.row(start.line)
            && start.col > 0
            && row.cells.get(start.col as usize) == Some(&'\0')
        {
            start.col -= 1;
        }
        // Never end on the first half of a wide char: the highlight covers the
        // whole glyph (the text is the same either way).
        if let Some(row) = src.row(end.line)
            && row.cells.get(end.col as usize + 1) == Some(&'\0')
        {
            end.col += 1;
        }
        (start, end)
    }

    /// Selected text: trailing blanks trimmed per line, soft wraps joined.
    pub fn to_text(&self, src: &dyn RowSource) -> String {
        let (start, end) = self.range(src);
        let mut out = String::new();
        let mut line = start.line;
        while line <= end.line {
            let Some(row) = src.row(line) else { break };
            let cols = row.cells.len();
            if cols == 0 {
                line += 1;
                continue;
            }
            let c0 = if line == start.line {
                start.col as usize
            } else {
                0
            };
            let c1 = if line == end.line {
                (end.col as usize).min(cols - 1)
            } else {
                cols - 1
            };
            let mut piece = String::new();
            for (col, c) in row.cells.iter().enumerate().take(c1 + 1).skip(c0) {
                if *c != '\0' {
                    piece.push(*c);
                }
                if let Ok(i) = row
                    .zerowidth
                    .binary_search_by_key(&(col as u16), |(c, _)| *c)
                {
                    piece.push_str(&row.zerowidth[i].1);
                }
            }
            let joins_next = row.wrapped && line != end.line && c1 == cols - 1;
            if !joins_next {
                piece.truncate(piece.trim_end_matches(' ').len());
            }
            out.push_str(&piece);
            if line != end.line && !joins_next {
                out.push('\n');
            }
            line += 1;
        }
        out
    }
}

/// Inclusive [start, end] columns of the word (or blank / delimiter run) at `col`.
pub fn word_bounds(cells: &[char], col: u16) -> (u16, u16) {
    if cells.is_empty() {
        return (0, 0);
    }
    let mut i = (col as usize).min(cells.len() - 1);
    if cells[i] == '\0' && i > 0 {
        i -= 1;
    }
    let k = class(cells[i]);
    if k == Class::Delim {
        let end = if cells.get(i + 1) == Some(&'\0') {
            i + 1
        } else {
            i
        };
        return (i as u16, end as u16);
    }
    let mut s = i;
    while s > 0 && class(cells[s - 1]) == k {
        s -= 1;
    }
    let mut e = i;
    while e + 1 < cells.len() && class(cells[e + 1]) == k {
        e += 1;
    }
    (s as u16, e as u16)
}

/// Columns of screen row `line` covered by the inclusive range, if any.
pub fn row_span(range: (SelPoint, SelPoint), line: i64, cols: u16) -> Option<(u16, u16)> {
    let (s, e) = range;
    if line < s.line || line > e.line || cols == 0 {
        return None;
    }
    let c0 = if line == s.line { s.col } else { 0 };
    let c1 = if line == e.line {
        e.col.min(cols - 1)
    } else {
        cols - 1
    };
    (c0 <= c1).then_some((c0, c1))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rows(Vec<RowText>);
    impl RowSource for Rows {
        fn cols(&self) -> u16 {
            self.0.first().map_or(0, |r| r.cells.len() as u16)
        }
        fn row(&self, line: i64) -> Option<RowText> {
            usize::try_from(line)
                .ok()
                .and_then(|l| self.0.get(l).cloned())
        }
    }

    /// `~` at the end of a line marks a soft wrap; `<` marks a wide-char spacer.
    fn rows(lines: &[&str], cols: usize) -> Rows {
        Rows(
            lines
                .iter()
                .map(|l| {
                    let wrapped = l.ends_with('~');
                    let l = l.trim_end_matches('~');
                    let mut cells: Vec<char> =
                        l.chars().map(|c| if c == '<' { '\0' } else { c }).collect();
                    cells.resize(cols, ' ');
                    RowText {
                        cells,
                        wrapped,
                        zerowidth: vec![],
                    }
                })
                .collect(),
        )
    }

    fn sel(mode: SelMode, a: (i64, u16), h: (i64, u16)) -> Selection {
        Selection {
            mode,
            anchor: SelPoint::new(a.0, a.1),
            head: SelPoint::new(h.0, h.1),
        }
    }

    #[test]
    fn simple_single_line() {
        let r = rows(&["hello world"], 20);
        assert_eq!(sel(SelMode::Simple, (0, 0), (0, 4)).to_text(&r), "hello");
        // Reversed drag gives the same text.
        assert_eq!(sel(SelMode::Simple, (0, 10), (0, 6)).to_text(&r), "world");
    }

    #[test]
    fn simple_multi_line_trims_trailing_blanks() {
        let r = rows(&["first", "second", "third"], 10);
        assert_eq!(
            sel(SelMode::Simple, (0, 2), (2, 2)).to_text(&r),
            "rst\nsecond\nthi"
        );
    }

    #[test]
    fn soft_wrap_joins_without_newline() {
        let r = rows(&["abcd~", "ef", "gh"], 4);
        assert_eq!(
            sel(SelMode::Simple, (0, 0), (2, 3)).to_text(&r),
            "abcdef\ngh"
        );
    }

    #[test]
    fn wide_chars() {
        // "한글" = 4 cells.
        let r = rows(&["a한<글<b"], 8);
        assert_eq!(sel(SelMode::Simple, (0, 0), (0, 5)).to_text(&r), "a한글b");
        // Starting on a spacer includes its wide char.
        assert_eq!(sel(SelMode::Simple, (0, 2), (0, 4)).to_text(&r), "한글");
        // Ending on the first half covers the spacer too (full-glyph highlight).
        let s = sel(SelMode::Simple, (0, 0), (0, 3));
        assert_eq!(s.range(&r).1.col, 4);
        assert_eq!(s.to_text(&r), "a한글");
        // Reversed drag from inside 글 back to 한's spacer: both chars, both halves.
        assert_eq!(
            sel(SelMode::Simple, (0, 3), (0, 2)).range(&r),
            (SelPoint::new(0, 1), SelPoint::new(0, 4))
        );
    }

    #[test]
    fn word_selection() {
        let r = rows(&["ls -la src/main.rs (ok)"], 30);
        assert_eq!(
            sel(SelMode::Word, (0, 9), (0, 9)).to_text(&r),
            "src/main.rs"
        );
        assert_eq!(sel(SelMode::Word, (0, 4), (0, 4)).to_text(&r), "-la");
        assert_eq!(sel(SelMode::Word, (0, 20), (0, 20)).to_text(&r), "ok");
        assert_eq!(sel(SelMode::Word, (0, 19), (0, 19)).to_text(&r), "(");
        // Dragging after a double click extends by words.
        assert_eq!(sel(SelMode::Word, (0, 0), (0, 4)).to_text(&r), "ls -la");
    }

    #[test]
    fn word_bounds_korean() {
        let cells: Vec<char> = "안<녕< 세<계<"
            .chars()
            .map(|c| if c == '<' { '\0' } else { c })
            .collect();
        assert_eq!(word_bounds(&cells, 1), (0, 3));
        assert_eq!(word_bounds(&cells, 6), (5, 8));
    }

    #[test]
    fn line_selection_follows_wraps() {
        let r = rows(&["prom", "abcd~", "efgh~", "ij", "next"], 4);
        assert_eq!(sel(SelMode::Line, (2, 1), (2, 1)).to_text(&r), "abcdefghij");
        assert_eq!(sel(SelMode::Line, (0, 0), (0, 0)).to_text(&r), "prom");
    }

    #[test]
    fn row_spans() {
        let range = (SelPoint::new(1, 3), SelPoint::new(3, 2));
        assert_eq!(row_span(range, 0, 10), None);
        assert_eq!(row_span(range, 1, 10), Some((3, 9)));
        assert_eq!(row_span(range, 2, 10), Some((0, 9)));
        assert_eq!(row_span(range, 3, 10), Some((0, 2)));
        assert_eq!(row_span(range, 4, 10), None);
    }
}
