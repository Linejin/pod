//! LSP snippet syntax (`$1`, `${1:default}`, `${1|a,b|}`, `$0`, variables)
//! parsed into plain text plus tab-stop ranges, and the tab-stop session the
//! editor runs after inserting a snippet completion.

use std::collections::BTreeMap;
use std::ops::Range;

/// A parsed snippet: the text to insert and its tab stops.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snippet {
    pub text: String,
    /// Byte ranges into `text` in visiting order: `$1`, `$2`, … then `$0`
    /// (or the end of the text when the snippet has no `$0`). The first
    /// occurrence of each index; mirrors are not linked.
    pub stops: Vec<Range<usize>>,
}

impl Snippet {
    /// Parses LSP snippet syntax. Placeholders keep their default text,
    /// choices take their first option, variables their default
    /// (`${TM_FILENAME:x}` is `x`, `$TM_SELECTED_TEXT` is empty).
    pub fn parse(s: &str) -> Snippet {
        let chars: Vec<char> = s.chars().collect();
        let mut out = String::new();
        let mut found: BTreeMap<u32, Range<usize>> = BTreeMap::new();
        parse(&chars, &mut 0, &mut out, &mut found, false);
        let final_stop = found.remove(&0);
        let mut stops: Vec<Range<usize>> = found.into_values().collect();
        if !stops.is_empty() || final_stop.is_some() {
            stops.push(final_stop.unwrap_or(out.len()..out.len()));
        }
        Snippet { text: out, stops }
    }

    /// Whether inserting the text with the caret at its end already is the
    /// whole snippet (no stop to visit).
    pub fn is_trivial(&self) -> bool {
        match self.stops.as_slice() {
            [] => true,
            [only] => only.start == self.text.len() && only.end == self.text.len(),
            _ => false,
        }
    }
}

fn ident(chars: &[char], i: &mut usize) -> String {
    let start = *i;
    while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
        *i += 1;
    }
    chars[start..*i].iter().collect()
}

fn record(found: &mut BTreeMap<u32, Range<usize>>, name: &str, range: Range<usize>) {
    if let Ok(n) = name.parse::<u32>() {
        found.entry(n).or_insert(range);
    }
}

fn parse(
    chars: &[char],
    i: &mut usize,
    out: &mut String,
    found: &mut BTreeMap<u32, Range<usize>>,
    in_brace: bool,
) {
    while *i < chars.len() {
        let c = chars[*i];
        let next = chars.get(*i + 1).copied();
        if c == '\\' && matches!(next, Some('$' | '}' | '\\' | ',' | '|')) {
            out.push(next.unwrap_or('\\'));
            *i += 2;
        } else if in_brace && c == '}' {
            *i += 1;
            return;
        } else if c == '$' && next.is_some_and(|n| n.is_ascii_alphanumeric() || n == '_') {
            // `$1`, `$0`: an empty stop; `$TM_SELECTED_TEXT`: nothing.
            *i += 1;
            let name = ident(chars, i);
            record(found, &name, out.len()..out.len());
        } else if c == '$' && next == Some('{') {
            *i += 2;
            let name = ident(chars, i);
            let start = out.len();
            match chars.get(*i) {
                Some(':') => {
                    *i += 1;
                    parse(chars, i, out, found, true);
                    record(found, &name, start..out.len());
                }
                Some('|') => {
                    *i += 1;
                    let mut first = String::new();
                    while *i < chars.len() && !matches!(chars[*i], ',' | '|') {
                        if chars[*i] == '\\' && *i + 1 < chars.len() {
                            *i += 1;
                        }
                        first.push(chars[*i]);
                        *i += 1;
                    }
                    while *i < chars.len() && chars[*i] != '}' {
                        *i += 1;
                    }
                    *i += 1;
                    out.push_str(&first);
                    record(found, &name, start..out.len());
                }
                _ => {
                    // `${1}` or a transform: an empty stop, skip to the matching brace.
                    let mut depth = 1;
                    while *i < chars.len() && depth > 0 {
                        match chars[*i] {
                            '{' => depth += 1,
                            '}' => depth -= 1,
                            _ => {}
                        }
                        *i += 1;
                    }
                    record(found, &name, start..start);
                }
            }
        } else {
            out.push(c);
            *i += 1;
        }
    }
}

/// One edit as the editor reports it (byte offsets before the edit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

/// Tab stops of an inserted snippet, kept in step with later edits.
/// Tab / Shift+Tab move between stops (selecting the placeholder text),
/// Esc or leaving the last stop ends it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetSession {
    /// Absolute byte ranges in the buffer, visiting order.
    pub stops: Vec<Range<usize>>,
    pub current: usize,
}

impl SnippetSession {
    /// Session for `snippet` inserted at byte `at`, or `None` when there is
    /// nothing to visit.
    pub fn new(snippet: &Snippet, at: usize) -> Option<Self> {
        if snippet.is_trivial() {
            return None;
        }
        Some(SnippetSession {
            stops: snippet
                .stops
                .iter()
                .map(|r| r.start + at..r.end + at)
                .collect(),
            current: 0,
        })
    }

    pub fn current_range(&self) -> Range<usize> {
        self.stops[self.current].clone()
    }

    /// Whether the session continues after this edit. Typing inside (or at
    /// either end of) a stop grows/shrinks it; edits before a stop shift it;
    /// an edit straddling a stop boundary ends the session.
    pub fn apply(&mut self, e: Edit) -> bool {
        let delta = e.new_end as isize - e.old_end as isize;
        let shift = |x: usize| (x as isize + delta).max(0) as usize;
        let current = self.current;
        for (n, r) in self.stops.iter_mut().enumerate() {
            let inside = e.start >= r.start && e.old_end <= r.end;
            if n == current && inside {
                r.end = shift(r.end);
            } else if e.old_end <= r.start {
                *r = shift(r.start)..shift(r.end);
            } else if e.start >= r.end {
                // After this stop: unchanged.
            } else if inside {
                r.end = shift(r.end);
            } else {
                return false;
            }
        }
        true
    }

    /// Next stop (`forward`) or previous; `None` ends the session (after
    /// the last stop, which is where the caret stays).
    pub fn step(&mut self, forward: bool) -> Option<Range<usize>> {
        if forward {
            if self.current + 1 >= self.stops.len() {
                return None;
            }
            self.current += 1;
        } else {
            self.current = self.current.saturating_sub(1);
        }
        Some(self.current_range())
    }

    /// Whether the session is on its final stop (`$0`).
    pub fn at_last(&self) -> bool {
        self.current + 1 >= self.stops.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stops(s: &str) -> (String, Vec<String>) {
        let sn = Snippet::parse(s);
        let texts = sn
            .stops
            .iter()
            .map(|r| sn.text[r.clone()].to_string())
            .collect();
        (sn.text, texts)
    }

    #[test]
    fn parses_stops_in_visiting_order() {
        let sn = Snippet::parse("fn ${1:name}(${2:args}) {\n\t$0\n}");
        assert_eq!(sn.text, "fn name(args) {\n\t\n}");
        assert_eq!(sn.stops, vec![3..7, 8..12, 17..17]);
        assert_eq!(
            stops("println!(\"${1:msg}\");$0"),
            (
                "println!(\"msg\");".to_string(),
                vec!["msg".to_string(), String::new()]
            )
        );
        // No `$0`: the session ends at the end of the text.
        let sn = Snippet::parse("${2:b} ${1:a}");
        assert_eq!(sn.stops, vec![2..3, 0..1, 3..3]);
        // Choices, nested placeholders, empty `${1}`, korean text.
        let (t, v) = stops("${1|one,two|} ${2:${3:nested}}");
        assert_eq!(t, "one nested");
        assert_eq!(v, ["one", "nested", "nested", ""]);
        let (t, v) = stops("한글 ${1:값}$0");
        assert_eq!(t, "한글 값");
        assert_eq!(v, ["값", ""]);
        assert_eq!(Snippet::parse("a${1}b").stops, vec![1..1, 2..2]);
    }

    #[test]
    fn trivial_snippets_need_no_session() {
        assert!(Snippet::parse("plain").is_trivial());
        assert!(Snippet::parse("done()$0").is_trivial());
        assert!(!Snippet::parse("finish($0)").is_trivial());
        assert!(SnippetSession::new(&Snippet::parse("x$0"), 4).is_none());
        // `finish($0)` → a single stop inside the parentheses.
        let s = SnippetSession::new(&Snippet::parse("finish($0)"), 10).unwrap();
        assert_eq!((s.stops.len(), s.stops[0].clone()), (1, 17..17));
    }

    #[test]
    fn session_follows_edits_and_steps() {
        // "fn name(args) {}" inserted at 100.
        let mut s =
            SnippetSession::new(&Snippet::parse("fn ${1:name}(${2:args}) {}$0"), 100).unwrap();
        assert_eq!(s.current_range(), 103..107);
        // Typing over the selected placeholder: replace "name" with "go".
        assert!(s.apply(Edit {
            start: 103,
            old_end: 107,
            new_end: 105
        }));
        assert_eq!(s.stops, vec![103..105, 106..110, 114..114]);
        // Typing at the end of the current stop extends it.
        assert!(s.apply(Edit {
            start: 105,
            old_end: 105,
            new_end: 106
        }));
        assert_eq!(s.stops[0], 103..106);
        assert_eq!(s.step(true), Some(107..111));
        // An auto-import above shifts every stop.
        assert!(s.apply(Edit {
            start: 0,
            old_end: 0,
            new_end: 20
        }));
        assert_eq!(s.stops, vec![123..126, 127..131, 135..135]);
        assert_eq!(s.step(false), Some(123..126));
        assert_eq!(s.step(true), Some(127..131));
        assert_eq!(s.step(true), Some(135..135));
        assert!(s.at_last());
        assert_eq!(s.step(true), None);
        // Adjacent stops: typing at the end of the current one does not grow the next.
        let mut s2 = SnippetSession::new(&Snippet::parse("${1:a}${2:b}"), 0).unwrap();
        assert!(s2.apply(Edit {
            start: 1,
            old_end: 1,
            new_end: 2
        }));
        assert_eq!(s2.stops, vec![0..2, 2..3, 3..3]);
        // Edits after the snippet leave it alone; one across a boundary ends it.
        assert!(s.apply(Edit {
            start: 200,
            old_end: 210,
            new_end: 200
        }));
        assert!(!s.apply(Edit {
            start: 125,
            old_end: 128,
            new_end: 125
        }));
    }
}
