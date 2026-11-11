//! pod-diff: the review diff model shared by `pod-app` (Review panel) and
//! `diff-spike` (the P4 reference spike, docs/05).
//!
//! - [`model`]: GUI-free input layer — both versions of each file (two files,
//!   a git working tree, or the files of a podd task via one
//!   `git cat-file --batch`), turned into full-context patches.
//! - [`parse`] / [`FileRow`]: the thin adapter over gpui-component's `diff`
//!   module (docs/05 §7: keep `DiffFile::parse` behind one function so an
//!   upstream break is fixed here only).

pub mod eol;
pub mod model;

use gpui_kit::SharedString;
use gpui_kit::component::diff::{DiffFile, DiffFileStatus};

pub use model::{ChangedFile, SourceFile};

/// Lines of unchanged context kept around changes when collapsed.
pub const CONTEXT_LINES: usize = 3;

/// One row of a file list.
#[derive(Clone, Debug, PartialEq)]
pub struct FileRow {
    pub path: SharedString,
    pub status: DiffFileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub binary: bool,
    /// Both versions differ only in line endings (CRLF/LF): every line shows
    /// as changed, as in `git diff`, and the list says so.
    pub eol_only: bool,
}

impl FileRow {
    /// Letter and color (0xRRGGBB) for the status column.
    pub fn status_mark(&self) -> (&'static str, u32) {
        match self.status {
            DiffFileStatus::Added => ("A", 0x3fb950),
            DiffFileStatus::Deleted => ("D", 0xf85149),
            DiffFileStatus::Renamed => ("R", 0x58a6ff),
            _ => ("M", 0xd29922),
        }
    }
}

/// Parsed files for `DiffState` plus their list rows. A parse failure yields
/// no files and the error text.
///
/// A source missing on both sides (deleted and re-created while the review
/// loaded, or listed by git yet absent) has nothing to show and would fail
/// the whole parse: it is skipped, the rest parses, and the text names it
/// (`missing on both sides, skipped: a, b`).
pub fn parse(sources: &[SourceFile]) -> (Vec<DiffFile>, Vec<FileRow>, Option<String>) {
    let has_side = |s: &&SourceFile| s.old.is_some() || s.new.is_some();
    let missing: Vec<&str> = sources
        .iter()
        .filter(|s| !has_side(s))
        .map(|s| s.path.as_str())
        .collect();
    if missing.is_empty() {
        return parse_present(sources);
    }
    let present: Vec<SourceFile> = sources.iter().filter(has_side).cloned().collect();
    let (files, rows, error) = parse_present(&present);
    let skipped = format!("missing on both sides, skipped: {}", missing.join(", "));
    (files, rows, error.or(Some(skipped)))
}

/// [`parse`] of sources that each have at least one side.
fn parse_present(sources: &[SourceFile]) -> (Vec<DiffFile>, Vec<FileRow>, Option<String>) {
    let patch = model::patch_of(sources);
    match DiffFile::parse(&patch) {
        Ok(files) => {
            let rows = files
                .iter()
                .enumerate()
                .map(|(i, f)| FileRow {
                    // One patch per source, in order.
                    eol_only: sources.get(i).is_some_and(|s| {
                        s.path == f.path().as_ref()
                            && eol::eol_only_change(s.text(false), s.text(true))
                    }),
                    path: f.path().clone(),
                    status: f.status(),
                    additions: f.additions(),
                    deletions: f.deletions(),
                    binary: f.is_binary(),
                })
                .collect();
            (files, rows, None)
        }
        Err(e) => (
            Vec::new(),
            Vec::new(),
            Some(format!("line {}: {}", e.line(), e.message())),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{FileRow, SourceFile, parse};
    use gpui_kit::component::diff::DiffFileStatus;

    #[test]
    fn parse_rows_match_sources() {
        let sources = vec![
            SourceFile {
                path: "a.rs".into(),
                old: Some("x\ny\n".into()),
                new: Some("x\nz\nw\n".into()),
                ..Default::default()
            },
            SourceFile {
                path: "dir/new file.md".into(),
                old: None,
                new: Some("hi\n".into()),
                ..Default::default()
            },
            SourceFile {
                path: "gone.txt".into(),
                old: Some("bye\n".into()),
                new: None,
                ..Default::default()
            },
            SourceFile {
                path: "img.png".into(),
                old: Some(String::new()),
                new: Some(String::new()),
                binary: true,
                ..Default::default()
            },
            SourceFile {
                path: "b.rs".into(),
                old_path: Some("old_b.rs".into()),
                old: Some("same\n".into()),
                new: Some("same\n".into()),
                ..Default::default()
            },
        ];
        let (files, rows, err) = parse(&sources);
        assert_eq!(err, None);
        assert_eq!(files.len(), 5);
        let r = |i: usize| {
            (
                &rows[i].path[..],
                rows[i].status,
                rows[i].additions,
                rows[i].deletions,
            )
        };
        assert_eq!(r(0), ("a.rs", DiffFileStatus::Modified, 2, 1));
        assert_eq!(r(1), ("dir/new file.md", DiffFileStatus::Added, 1, 0));
        assert_eq!(r(2), ("gone.txt", DiffFileStatus::Deleted, 0, 1));
        assert!(rows[3].binary, "{:?}", rows[3]);
        assert_eq!(rows[4].status, DiffFileStatus::Renamed);
        assert_eq!(rows[4].path.as_ref(), "b.rs");
        assert!(rows.iter().all(|r| !r.eol_only));
        let crlf = SourceFile {
            path: "crlf.txt".into(),
            old: Some("a\nb\n".into()),
            new: Some("a\r\nb\r\n".into()),
            ..Default::default()
        };
        let (_, rows, _) = parse(&[crlf]);
        assert!(
            rows[0].eol_only && (rows[0].additions, rows[0].deletions) == (2, 2),
            "{rows:?}"
        );
        let _: &FileRow = &rows[0];
    }

    /// A file listed as changed but absent on both sides (deleted and
    /// re-created while the review loaded, or listed by git yet gone) is
    /// skipped and named; the other files still parse, rows stay in order.
    #[test]
    fn a_file_missing_on_both_sides_is_skipped_not_fatal() {
        let src = |path: &str, old: Option<&str>, new: Option<&str>| SourceFile {
            path: path.into(),
            old: old.map(Into::into),
            new: new.map(Into::into),
            ..Default::default()
        };
        let sources = vec![
            src("a.rs", Some("x\n"), Some("y\n")),
            src("gone 1.rs", None, None),
            src("crlf.txt", Some("a\nb\n"), Some("a\r\nb\r\n")),
            src("gone 2.rs", None, None),
        ];
        let (files, rows, note) = parse(&sources);
        assert_eq!(files.len(), 2, "{note:?}");
        let paths: Vec<&str> = rows.iter().map(|r| r.path.as_ref()).collect();
        assert_eq!(paths, ["a.rs", "crlf.txt"]);
        assert_eq!(rows[0].status, DiffFileStatus::Modified);
        assert!(rows[1].eol_only, "eol_only follows the right source");
        assert_eq!(
            note.as_deref(),
            Some("missing on both sides, skipped: gone 1.rs, gone 2.rs")
        );
        // Only missing files: nothing to show, and the text says why.
        let (files, rows, note) = parse(&sources[1..2]);
        assert!(files.is_empty() && rows.is_empty());
        assert_eq!(
            note.as_deref(),
            Some("missing on both sides, skipped: gone 1.rs")
        );
    }
}
