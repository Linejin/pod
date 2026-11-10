//! Links in terminal output: URLs and file references.
//!
//! Detected forms (one token, no spaces):
//! - `http://…` / `https://…` (trailing punctuation and unbalanced `)` trimmed)
//! - `path:line` and `path:line:col` (rustc, gcc, grep -n, eslint …), Windows
//!   (`C:\src\a.rs:3`, `src\a.rs:3`) and POSIX (`src/a.rs:3`) separators
//! - `path(line)` and `path(line,col)` (MSVC, `file.rs(12,5): error …`)
//!
//! A path must look like one: it contains a separator or ends in a file name
//! with an extension. `12:30:45` or `localhost:8080` are not links. Whether
//! the file exists is the host's decision ([`crate::TerminalView::set_link_filter`]).
//!
//! Detection runs over *logical* lines: soft-wrapped rows are joined, so a
//! long path broken by the terminal width is still one link.

use crate::selection::{RowSource, SelPoint};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum LinkTarget {
    Url(String),
    /// A file reference as printed (relative or absolute), 1-based line/column.
    File {
        path: String,
        line: u32,
        col: Option<u32>,
    },
}

impl LinkTarget {
    /// Text for lists and accessible names (`src/a.rs:3:5`, the URL).
    pub fn label(&self) -> String {
        match self {
            LinkTarget::Url(u) => u.clone(),
            LinkTarget::File {
                path,
                line,
                col: Some(c),
            } => format!("{path}:{line}:{c}"),
            LinkTarget::File {
                path,
                line,
                col: None,
            } => format!("{path}:{line}"),
        }
    }
}

/// A link in a string, as a char-index range `start..end`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub start: usize,
    pub end: usize,
    pub target: LinkTarget,
}

/// A link on the terminal grid: inclusive cell range in absolute lines
/// (see [`crate::selection`]), so it can be underlined like a selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLink {
    pub target: LinkTarget,
    /// The characters as printed.
    pub text: String,
    pub start: SelPoint,
    pub end: SelPoint,
}

impl TerminalLink {
    pub fn contains(&self, p: SelPoint) -> bool {
        self.start <= p && p <= self.end
    }
}

fn is_delimiter(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '"' | '\'' | '`' | '<' | '>' | '|' | '[' | ']' | '{' | '}' | '\0' | '│'
        )
}

/// All links in `text` (char indices), left to right.
pub fn find_links(text: &str) -> Vec<Found> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if is_delimiter(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !is_delimiter(chars[i]) {
            i += 1;
        }
        if let Some(f) = token_link(&chars[start..i], start) {
            out.push(f);
        }
    }
    out
}

fn token_link(tok: &[char], offset: usize) -> Option<Found> {
    let s: String = tok.iter().collect();
    // URL anywhere in the token (`(see https://x)`, `url=https://x`).
    for scheme in ["https://", "http://"] {
        if let Some(byte) = s.find(scheme) {
            let lead = s[..byte].chars().count();
            let url: Vec<char> = tok[lead..].to_vec();
            let end = trim_url(&url);
            if end <= scheme.len() {
                return None;
            }
            let url: String = url[..end].iter().collect();
            return Some(Found {
                start: offset + lead,
                end: offset + lead + end,
                target: LinkTarget::Url(url),
            });
        }
    }
    // File reference: skip leading openers, trim trailing punctuation.
    let mut a = 0;
    while a < tok.len() && matches!(tok[a], '(' | ',' | ';' | '=') {
        a += 1;
    }
    let mut b = tok.len();
    while b > a && matches!(tok[b - 1], '.' | ',' | ';' | ':' | '!' | '?') {
        b -= 1;
    }
    // An unbalanced trailing `)` (not MSVC's `(12,5)`) belongs to the prose.
    while b > a && tok[b - 1] == ')' && !tok[a..b].contains(&'(') {
        b -= 1;
    }
    let body: String = tok[a..b].iter().collect();
    let (path, line, col, bytes) = parse_file_ref(&body)?;
    let len = body[..bytes].chars().count();
    Some(Found {
        start: offset + a,
        end: offset + a + len,
        target: LinkTarget::File { path, line, col },
    })
}

/// Length of the URL after trimming trailing punctuation and unbalanced `)`.
fn trim_url(url: &[char]) -> usize {
    let mut end = url.len();
    loop {
        if end == 0 {
            return 0;
        }
        let c = url[end - 1];
        if matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '*') {
            end -= 1;
            continue;
        }
        if c == ')' {
            let open = url[..end].iter().filter(|&&c| c == '(').count();
            let close = url[..end].iter().filter(|&&c| c == ')').count();
            if close > open {
                end -= 1;
                continue;
            }
        }
        return end;
    }
}

fn digits(s: &str) -> Option<u32> {
    (!s.is_empty() && s.len() <= 9 && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// A file reference at the start of `s`: `path:line[:col]` or
/// `path(line[,col])`, and how many bytes of `s` it covers (`a.rs:3:text`
/// from `grep -n` is `a.rs:3`).
pub fn parse_file_ref(s: &str) -> Option<(String, u32, Option<u32>, usize)> {
    let b = s.as_bytes();
    let run = |from: usize| {
        b[from.min(b.len())..]
            .iter()
            .take_while(|c| c.is_ascii_digit())
            .count()
    };
    // MSVC: `file.rs(12)` / `file.rs(12,5)`.
    for (i, _) in s.match_indices('(') {
        let n1 = run(i + 1);
        if n1 == 0 || !looks_like_path(&s[..i]) {
            continue;
        }
        let mut j = i + 1 + n1;
        let mut col = None;
        if b.get(j) == Some(&b',') {
            let n2 = run(j + 1);
            if n2 == 0 {
                continue;
            }
            col = digits(&s[j + 1..j + 1 + n2]);
            j += 1 + n2;
        }
        if b.get(j) == Some(&b')') {
            return Some((
                s[..i].to_string(),
                digits(&s[i + 1..i + 1 + n1])?,
                col,
                j + 1,
            ));
        }
    }
    // `path:line[:col]`; the drive colon of `C:\…` is part of the path.
    for (i, _) in s.match_indices(':') {
        if i == 1 && b[0].is_ascii_alphabetic() {
            continue;
        }
        let n1 = run(i + 1);
        let after = i + 1 + n1;
        if n1 == 0 || !(after == b.len() || b[after] == b':') || !looks_like_path(&s[..i]) {
            continue;
        }
        let line = digits(&s[i + 1..after])?;
        let n2 = run(after + 1);
        let end2 = after + 1 + n2;
        if n2 > 0 && (end2 == b.len() || b[end2] == b':') {
            return Some((s[..i].to_string(), line, digits(&s[after + 1..end2]), end2));
        }
        return Some((s[..i].to_string(), line, None, after));
    }
    None
}

/// A separator, or a final component with an extension (`main.rs`, `Makefile.am`).
fn looks_like_path(p: &str) -> bool {
    if p.is_empty() || p.contains("://") || p.bytes().all(|b| b.is_ascii_digit() || b == b':') {
        return false;
    }
    if p.contains('/') || p.contains('\\') {
        // At least one character of a name (not just `/` or `C:\`).
        let rest = match p.as_bytes() {
            [d, b':', ..] if d.is_ascii_alphabetic() => &p[2..],
            _ => p,
        };
        return rest.chars().any(|c| c.is_alphanumeric() || c == '_');
    }
    match p.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && (1..=10).contains(&ext.len())
                && ext.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// Text of one logical line and, per char, the cell it came from.
pub(crate) struct LogicalLine {
    pub text: String,
    pub cells: Vec<SelPoint>,
}

/// Joins soft-wrapped rows `first..` into one string (wide-char spacers skipped).
/// Returns the line and the next absolute line after it.
pub(crate) fn logical_line(
    src: &dyn RowSource,
    first: i64,
    max_rows: usize,
) -> Option<(LogicalLine, i64)> {
    let mut line = LogicalLine {
        text: String::new(),
        cells: Vec::new(),
    };
    let mut abs = first;
    for _ in 0..max_rows.max(1) {
        let row = src.row(abs)?;
        for (col, c) in row.cells.iter().enumerate() {
            if *c == '\0' {
                continue;
            }
            line.text.push(*c);
            line.cells.push(SelPoint::new(abs, col as u16));
        }
        abs += 1;
        if !row.wrapped {
            break;
        }
    }
    // Trailing blanks of the last row are not part of any link.
    Some((line, abs))
}

/// First absolute line of the logical line that contains `abs`.
pub(crate) fn logical_start(src: &dyn RowSource, abs: i64, max_back: usize) -> i64 {
    let mut first = abs;
    for _ in 0..max_back {
        match src.row(first - 1) {
            Some(r) if r.wrapped => first -= 1,
            _ => break,
        }
    }
    first
}

/// Links whose cells lie in absolute lines `top..top+rows` (a link that
/// starts or ends off screen is still returned whole).
pub(crate) fn links_in(src: &dyn RowSource, top: i64, rows: usize) -> Vec<TerminalLink> {
    const MAX_WRAP: usize = 32;
    let bottom = top + rows as i64;
    let mut out = Vec::new();
    let mut abs = logical_start(src, top, MAX_WRAP);
    while abs < bottom {
        let Some((line, next)) = logical_line(src, abs, MAX_WRAP) else {
            break;
        };
        for f in find_links(&line.text) {
            let (Some(s), Some(e)) = (
                line.cells.get(f.start),
                line.cells.get(f.end.saturating_sub(1)),
            ) else {
                continue;
            };
            if e.line < top || s.line >= bottom {
                continue;
            }
            let text: String = line
                .text
                .chars()
                .skip(f.start)
                .take(f.end - f.start)
                .collect();
            out.push(TerminalLink {
                target: f.target,
                text,
                start: *s,
                end: *e,
            });
        }
        abs = next.max(abs + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vt::RowText;

    fn file(path: &str, line: u32, col: Option<u32>) -> LinkTarget {
        LinkTarget::File {
            path: path.into(),
            line,
            col,
        }
    }

    fn targets(s: &str) -> Vec<LinkTarget> {
        find_links(s).into_iter().map(|f| f.target).collect()
    }

    #[test]
    fn rustc_gcc_grep_forms() {
        assert_eq!(
            targets("  --> src/main.rs:12:5"),
            vec![file("src/main.rs", 12, Some(5))]
        );
        assert_eq!(
            targets("lib/a.c:7: warning: x"),
            vec![file("lib/a.c", 7, None)]
        );
        assert_eq!(
            targets("README.md:3:hello"),
            vec![file("README.md", 3, None)]
        );
        assert_eq!(
            targets("./tools/run.ps1:40"),
            vec![file("./tools/run.ps1", 40, None)]
        );
    }

    #[test]
    fn windows_paths_and_msvc() {
        assert_eq!(
            targets(r"C:\src\pod\main.rs:3:9"),
            vec![file(r"C:\src\pod\main.rs", 3, Some(9))]
        );
        assert_eq!(
            targets(r"C:\src\pod\main.rs:3"),
            vec![file(r"C:\src\pod\main.rs", 3, None)]
        );
        assert_eq!(
            targets(r"src\lib.rs:10"),
            vec![file(r"src\lib.rs", 10, None)]
        );
        assert_eq!(
            targets(r"C:\w\file.rs(12,5): error C2065"),
            vec![file(r"C:\w\file.rs", 12, Some(5))]
        );
        assert_eq!(
            targets("file.rs(12): note"),
            vec![file("file.rs", 12, None)]
        );
        assert_eq!(
            targets("C:/src/a.ts:1:1"),
            vec![file("C:/src/a.ts", 1, Some(1))]
        );
    }

    #[test]
    fn urls_trim_punctuation_and_parens() {
        assert_eq!(
            targets("see https://example.com/a?b=1."),
            vec![LinkTarget::Url("https://example.com/a?b=1".into())]
        );
        assert_eq!(
            targets("(https://x.dev/docs)"),
            vec![LinkTarget::Url("https://x.dev/docs".into())]
        );
        assert_eq!(
            targets("https://en.wikipedia.org/wiki/Rust_(language)"),
            vec![LinkTarget::Url(
                "https://en.wikipedia.org/wiki/Rust_(language)".into()
            )]
        );
        assert_eq!(
            targets("url=http://localhost:8080/x"),
            vec![LinkTarget::Url("http://localhost:8080/x".into())]
        );
        assert!(targets("https://").is_empty());
    }

    #[test]
    fn not_links() {
        for s in [
            "12:30:45",
            "localhost:8080",
            "a:1",
            "error: 3 warnings",
            "v1.2:3",
            "ratio 3:4",
            "C:\\:3",
            "foo.:3",
            "x.123:4",
        ] {
            assert!(targets(s).is_empty(), "{s} -> {:?}", targets(s));
        }
        // No line number: plain paths are not links.
        assert!(targets("src/main.rs").is_empty());
    }

    #[test]
    fn char_ranges_cover_the_printed_text() {
        let s = "오류 → src/한글.rs:2:1 끝";
        let f = &find_links(s)[0];
        let text: String = s.chars().skip(f.start).take(f.end - f.start).collect();
        assert_eq!(text, "src/한글.rs:2:1");
        assert_eq!(f.target, file("src/한글.rs", 2, Some(1)));
    }

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

    /// Rows of width `cols`; a trailing `~` marks a soft wrap.
    fn rows(lines: &[&str], cols: usize) -> Rows {
        Rows(
            lines
                .iter()
                .map(|l| {
                    let wrapped = l.ends_with('~');
                    let mut cells: Vec<char> = l.trim_end_matches('~').chars().collect();
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

    #[test]
    fn wrapped_link_spans_rows_and_is_found_from_either_row() {
        // "see src/very/long.rs:42 ok" wrapped at 12 columns.
        let r = rows(&["see src/very~", "/long.rs:42 ~", "ok"], 12);
        let all = links_in(&r, 0, 3);
        assert_eq!(all.len(), 1);
        let l = &all[0];
        assert_eq!(l.target, file("src/very/long.rs", 42, None));
        assert_eq!(
            (l.start, l.end),
            (SelPoint::new(0, 4), SelPoint::new(1, 10))
        );
        // Only the second row visible: the link still comes back whole.
        let from_second = links_in(&r, 1, 1);
        assert_eq!(from_second, all);
        assert!(l.contains(SelPoint::new(1, 0)) && !l.contains(SelPoint::new(1, 11)));
    }

    #[test]
    fn wide_chars_map_to_their_first_cell() {
        let mut r = rows(&["가 a.rs:1"], 10);
        // A wide char occupies two cells: '가', spacer.
        r.0[0].cells = vec!['가', '\0', ' ', 'a', '.', 'r', 's', ':', '1', ' '];
        let l = &links_in(&r, 0, 1)[0];
        assert_eq!((l.start, l.end), (SelPoint::new(0, 3), SelPoint::new(0, 8)));
        assert_eq!(l.text, "a.rs:1");
    }
}
