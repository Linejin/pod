//! Fuzzy path matcher for quick open (clean-room, no external matcher).
//!
//! A query matches a path when its characters appear in order, ignoring case
//! (`/` and `\` are equivalent). Among matches the score prefers, in order of
//! weight: characters inside the file name, matches at word starts (after a
//! separator, `_`, `-`, `.`, or a lower->upper camel step), runs of
//! consecutive characters, then shorter paths. The window is found greedily
//! from the left and then tightened from the right (as in fzf v1), which is
//! linear in the path length; a 50k-file list scores in tens of milliseconds.

pub const SCORE_MATCH: i32 = 16;
pub const BONUS_BOUNDARY: i32 = 24;
pub const BONUS_FILENAME_START: i32 = 32;
pub const BONUS_CONSECUTIVE: i32 = 20;
pub const BONUS_IN_FILENAME: i32 = 12;
pub const BONUS_EXACT_CASE: i32 = 1;
pub const PENALTY_GAP: i32 = 2;
pub const PENALTY_GAP_MAX: i32 = 12;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub score: i32,
    /// Char indices into the candidate, ascending, one per query char.
    pub indices: Vec<usize>,
}

fn fold(c: char) -> char {
    if c == '\\' {
        '/'
    } else {
        c.to_ascii_lowercase()
    }
}

fn is_sep(c: char) -> bool {
    matches!(c, '/' | '\\' | '_' | '-' | '.' | ' ')
}

/// Prepared query: folded chars plus the original for case bonuses.
#[derive(Clone, Debug, Default)]
pub struct Query {
    raw: Vec<char>,
    folded: Vec<char>,
}

impl Query {
    pub fn new(q: &str) -> Self {
        let raw: Vec<char> = q.chars().filter(|c| !c.is_whitespace()).collect();
        let folded = raw.iter().map(|&c| fold(c)).collect();
        Query { raw, folded }
    }
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }
}

/// Scores `candidate` (a relative path). `None` if it does not match.
pub fn score(query: &Query, candidate: &str) -> Option<Match> {
    if query.is_empty() {
        return Some(Match {
            score: 0,
            indices: vec![],
        });
    }
    let chars: Vec<char> = candidate.chars().collect();
    let folded: Vec<char> = chars.iter().map(|&c| fold(c)).collect();
    let q = &query.folded;
    // Leftmost end of a subsequence match.
    let mut qi = 0;
    let mut end = None;
    for (i, &c) in folded.iter().enumerate() {
        if c == q[qi] {
            qi += 1;
            if qi == q.len() {
                end = Some(i);
                break;
            }
        }
    }
    let end = end?;
    // Tighten from the right: latest start that still matches.
    let mut qi = q.len();
    let mut start = end;
    for i in (0..=end).rev() {
        if folded[i] == q[qi - 1] {
            qi -= 1;
            if qi == 0 {
                start = i;
                break;
            }
        }
    }
    let name_start = chars
        .iter()
        .rposition(|&c| c == '/' || c == '\\')
        .map(|i| i + 1)
        .unwrap_or(0);
    // Prefer an alignment entirely inside the file name when one exists.
    let mut best = align(query, &chars, &folded, start, name_start);
    if start < name_start
        && let Some(m) = align_from(query, &chars, &folded, name_start, name_start)
        && best.as_ref().is_none_or(|b| m.score > b.score)
    {
        best = Some(m);
    }
    best
}

fn align(
    query: &Query,
    chars: &[char],
    folded: &[char],
    from: usize,
    name_start: usize,
) -> Option<Match> {
    align_from(query, chars, folded, from, name_start)
}

/// Greedy alignment starting at `from`, preferring word-start positions for
/// each query char when one is available before the next forced match.
fn align_from(
    query: &Query,
    chars: &[char],
    folded: &[char],
    from: usize,
    name_start: usize,
) -> Option<Match> {
    let q = &query.folded;
    let mut indices = Vec::with_capacity(q.len());
    let mut pos = from;
    for (k, &qc) in q.iter().enumerate() {
        // First occurrence at or after pos.
        let first = (pos..folded.len()).find(|&i| folded[i] == qc)?;
        // Is there a boundary occurrence later that still leaves room for the rest?
        let mut chosen = first;
        let prev_matched = indices.last().copied();
        let consecutive = prev_matched.is_some_and(|p: usize| p + 1 == first);
        if !consecutive && !is_boundary(chars, first) {
            let rest = &q[k + 1..];
            for i in first + 1..folded.len() {
                if folded[i] == qc && is_boundary(chars, i) && fits(folded, i + 1, rest) {
                    chosen = i;
                    break;
                }
            }
        }
        indices.push(chosen);
        pos = chosen + 1;
    }
    let mut s = 0i32;
    let mut prev: Option<usize> = None;
    for (k, &i) in indices.iter().enumerate() {
        s += SCORE_MATCH;
        if i == name_start {
            s += BONUS_FILENAME_START;
        } else if is_boundary(chars, i) {
            s += BONUS_BOUNDARY;
        }
        if i >= name_start {
            s += BONUS_IN_FILENAME;
        }
        if chars[i] == query.raw[k] {
            s += BONUS_EXACT_CASE;
        }
        match prev {
            Some(p) if p + 1 == i => s += BONUS_CONSECUTIVE,
            Some(p) => s -= ((i - p - 1) as i32 * PENALTY_GAP).min(PENALTY_GAP_MAX),
            None => {}
        }
        prev = Some(i);
    }
    // Shorter paths win ties.
    s -= (chars.len() as i32) / 8;
    Some(Match { score: s, indices })
}

fn fits(folded: &[char], from: usize, rest: &[char]) -> bool {
    let mut k = 0;
    for &c in &folded[from.min(folded.len())..] {
        if k == rest.len() {
            break;
        }
        if c == rest[k] {
            k += 1;
        }
    }
    k == rest.len()
}

fn is_boundary(chars: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let p = chars[i - 1];
    let c = chars[i];
    is_sep(p)
        || (p.is_lowercase() && c.is_uppercase())
        || (!p.is_ascii_digit() && c.is_ascii_digit())
}

/// Ranks `items` by score (desc), then shorter path, then name. Returns
/// `(item index, match)` for at most `limit` matches.
pub fn rank<'a>(
    query: &Query,
    items: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<(usize, Match)> {
    let mut out: Vec<(usize, Match, usize)> = Vec::new();
    for (ix, s) in items.into_iter().enumerate() {
        if let Some(m) = score(query, s) {
            out.push((ix, m, s.len()));
        }
    }
    out.sort_by(|a, b| {
        b.1.score
            .cmp(&a.1.score)
            .then(a.2.cmp(&b.2))
            .then(a.0.cmp(&b.0))
    });
    out.truncate(limit);
    out.into_iter().map(|(i, m, _)| (i, m)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(q: &str, items: &[&str]) -> Vec<String> {
        rank(&Query::new(q), items.iter().copied(), 10)
            .into_iter()
            .map(|(i, _)| items[i].to_string())
            .collect()
    }

    #[test]
    fn subsequence_required_and_case_insensitive() {
        assert!(score(&Query::new("mn"), "src/main.rs").is_some());
        assert!(score(&Query::new("MAIN"), "src/main.rs").is_some());
        assert!(score(&Query::new("xyz"), "src/main.rs").is_none());
        assert!(
            score(&Query::new("src\\m"), "src/main.rs").is_some(),
            "backslash matches slash"
        );
    }

    #[test]
    fn filename_beats_directory_match() {
        let items = ["main/other/readme.md", "src/main.rs"];
        assert_eq!(top("main", &items)[0], "src/main.rs");
    }

    #[test]
    fn word_starts_beat_scattered() {
        let items = ["src/fabric.rs", "src/foo_bar.rs"];
        assert_eq!(top("fb", &items)[0], "src/foo_bar.rs");
        let items = ["src/quickopenview.rs", "src/QuickOpenView.rs"];
        assert_eq!(top("qov", &items)[0], "src/QuickOpenView.rs");
    }

    #[test]
    fn consecutive_and_shorter_win() {
        let items = [
            "crates/pod-editor/src/lsp/client_tests_long_name.rs",
            "crates/pod-editor/src/lsp/client.rs",
        ];
        assert_eq!(
            top("client", &items)[0],
            "crates/pod-editor/src/lsp/client.rs"
        );
        let items = ["a/b/c/d/e/f/main.rs", "main.rs"];
        assert_eq!(top("main.rs", &items)[0], "main.rs");
        let items = ["src/mxaxixn.rs", "src/main.rs"];
        assert_eq!(top("main", &items)[0], "src/main.rs");
    }

    #[test]
    fn path_queries_use_directories() {
        let items = [
            "docs/editor.md",
            "crates/pod-editor/src/lib.rs",
            "crates/pod-app/src/lib.rs",
        ];
        assert_eq!(top("editorlib", &items)[0], "crates/pod-editor/src/lib.rs");
    }

    #[test]
    fn indices_point_at_matched_chars() {
        let m = score(&Query::new("fb"), "src/foo_bar.rs").unwrap();
        let chars: Vec<char> = "src/foo_bar.rs".chars().collect();
        assert_eq!(m.indices.len(), 2);
        assert_eq!(chars[m.indices[0]], 'f');
        assert_eq!(chars[m.indices[1]], 'b');
        assert_eq!(m.indices, vec![4, 8]);
        let m = score(&Query::new("한글"), "docs/한글문서.md").unwrap();
        assert_eq!(m.indices, vec![5, 6]);
    }
}
