//! Both versions of each file: two files on disk, a git working tree against
//! a revision, or the changed files of a podd task (old side through one
//! `cat-file --batch`, new side from the worktree, line endings cleaned).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

use super::git::{cat_files, git};
use super::{ChangedFile, MAX_TEXT_BYTES, SourceFile};
use crate::eol;

pub fn from_two_files(old: &Path, new: &Path) -> Result<Vec<SourceFile>> {
    let read =
        |p: &Path| std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()));
    let name = new
        .file_name()
        .unwrap_or(new.as_os_str())
        .to_string_lossy()
        .into_owned();
    Ok(vec![SourceFile {
        path: name,
        old: Some(read(old)?),
        new: Some(read(new)?),
        ..Default::default()
    }])
}

/// Text, or `None` for binary (NUL byte) or oversized content.
fn text_or_none(bytes: Vec<u8>) -> Option<String> {
    if bytes.len() > MAX_TEXT_BYTES || bytes.contains(&0) {
        None
    } else {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Both versions of each changed file: the old side from `base` (the task's
/// merge base) via one `git cat-file --batch`, the new side from the worktree.
pub fn load_changed(worktree: &Path, base: &str, files: &[ChangedFile]) -> Result<Vec<SourceFile>> {
    let old_specs: Vec<(usize, String)> = old_side_wanted(files)
        .map(|(i, p)| (i, format!("{base}:{p}")))
        .collect();
    let specs: Vec<String> = old_specs.iter().map(|(_, s)| s.clone()).collect();
    let blobs = cat_files(worktree, &specs)?;
    let mut old: Vec<Option<Vec<u8>>> = vec![None; files.len()];
    for ((i, _), blob) in old_specs.into_iter().zip(blobs) {
        old[i] = blob;
    }
    let (mut out, crlf) = assemble(files, old, |f| std::fs::read(worktree.join(&f.path)).ok());
    clean_worktree_eol(worktree, &mut out, &crlf)?;
    Ok(out)
}

fn added(f: &ChangedFile) -> bool {
    matches!(f.status.as_str(), "A" | "?")
}

/// The files whose old side is read, with the path to read it at.
pub(crate) fn old_side_wanted(files: &[ChangedFile]) -> impl Iterator<Item = (usize, &str)> {
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| !added(f) && !f.binary)
        .map(|(i, f)| (i, f.old_path.as_deref().unwrap_or(&f.path)))
}

/// Does the new side of `f` get read?
fn new_side_wanted(f: &ChangedFile) -> bool {
    f.status != "D" && !f.binary
}

/// The rules shared by the local and the remote load (docs/17 R5: the same
/// review either way): binary and oversized sides, added/deleted sides.
/// `old[i]` is file `i`'s old bytes (when [`old_side_wanted`]); `new` reads
/// the worktree side (`None`: missing or refused). Also the indexes of the
/// files whose new side has CRLF (for [`apply_eol`]).
pub(crate) fn assemble(
    files: &[ChangedFile],
    mut old: Vec<Option<Vec<u8>>>,
    mut new: impl FnMut(&ChangedFile) -> Option<Vec<u8>>,
) -> (Vec<SourceFile>, Vec<usize>) {
    let mut out = Vec::with_capacity(files.len());
    let mut crlf: Vec<usize> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let mut binary = f.binary;
        let old_text = match old.get_mut(i).and_then(Option::take) {
            Some(b) => {
                let t = text_or_none(b);
                binary |= t.is_none();
                t
            }
            None => None,
        };
        let new_text = if !new_side_wanted(f) {
            None
        } else {
            match new(f) {
                Some(b) => {
                    let t = text_or_none(b);
                    binary |= t.is_none();
                    t
                }
                None => None,
            }
        };
        let (old_text, new_text) = if binary {
            (None, None)
        } else {
            (old_text, new_text)
        };
        if new_text.as_deref().is_some_and(|t| t.contains("\r\n")) {
            crlf.push(out.len());
        }
        out.push(SourceFile {
            path: f.path.clone(),
            old_path: f.old_path.clone(),
            old: if binary {
                (!added(f)).then(String::new)
            } else {
                old_text
            },
            new: if binary {
                (f.status != "D").then(String::new)
            } else {
                new_text
            },
            binary,
        });
    }
    (out, crlf)
}

/// Converts the worktree side of `files[idx]` (text with CRLF) the way
/// `git diff` does before comparing ([`crate::eol`]), so `core.autocrlf` or
/// `text`/`eol` attributes do not mark every line as changed. Costs a config
/// read, `git check-attr` and one `cat-file --batch` (index versions) only
/// when some changed file has CRLF.
fn clean_worktree_eol(repo: &Path, files: &mut [SourceFile], idx: &[usize]) -> Result<()> {
    if idx.is_empty() {
        return Ok(());
    }
    let autocrlf = eol::autocrlf(repo);
    let paths: Vec<&str> = idx.iter().map(|&i| files[i].path.as_str()).collect();
    let attrs = eol::attributes(repo, &paths)?;
    let index_specs: Vec<String> = paths.iter().map(|p| format!(":{p}")).collect();
    let index = cat_files(repo, &index_specs)?;
    apply_eol(files, idx, attrs, index, autocrlf);
    Ok(())
}

/// The cleaning itself, from the facts: per `idx` entry its `(text, crlf,
/// eol)` attributes and its index blob, and `core.autocrlf`.
pub(crate) fn apply_eol(
    files: &mut [SourceFile],
    idx: &[usize],
    attrs: Vec<(String, String, String)>,
    index: Vec<Option<Vec<u8>>>,
    autocrlf: eol::AutoCrlf,
) {
    for ((&i, (text, crlf, eol_attr)), index) in idx.iter().zip(attrs).zip(index) {
        let action = eol::action(&text, &crlf, &eol_attr, autocrlf);
        if let Some(new) = files[i].new.as_deref()
            && let Some(cleaned) = eol::clean(new, action, index.as_deref())
        {
            files[i].new = Some(cleaned);
        }
    }
}

/// Working tree (tracked changes + untracked files) against `rev`.
pub fn from_git(repo: &Path, rev: &str) -> Result<Vec<SourceFile>> {
    let top = String::from_utf8(git(repo, &["rev-parse", "--show-toplevel"])?)?
        .trim()
        .to_string();
    let top = PathBuf::from(top);
    let status = git(&top, &["diff", "--name-status", "--no-renames", "-z", rev])?;
    let mut entries: Vec<ChangedFile> = Vec::new();
    let mut parts = status.split(|b| *b == 0).filter(|s| !s.is_empty());
    while let (Some(code), Some(path)) = (parts.next(), parts.next()) {
        entries.push(ChangedFile {
            path: String::from_utf8_lossy(path).into_owned(),
            status: (code[0] as char).to_string(),
            ..Default::default()
        });
    }
    let untracked = git(&top, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for p in untracked.split(|b| *b == 0).filter(|s| !s.is_empty()) {
        entries.push(ChangedFile {
            path: String::from_utf8_lossy(p).into_owned(),
            status: "?".into(),
            ..Default::default()
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let mut files = load_changed(&top, rev, &entries)?;
    // The spike skipped binaries; keep that for `--git`.
    files.retain(|f| !f.binary && (f.old.is_some() || f.new.is_some()));
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::full_context_patch;

    /// Real git: worktree vs a commit, through `cat-file --batch`.
    #[test]
    fn load_changed_from_git() {
        let dir = std::env::temp_dir().join(format!("pod-diff-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let run = |args: &[&str]| git(&dir, args).unwrap();
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("src/keep.rs"), "fn a() {}\nfn b() {}\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        std::fs::write(dir.join("bin.dat"), [0u8, 1, 2]).unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "base"]);
        let base = String::from_utf8(run(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_string();
        std::fs::write(dir.join("src/keep.rs"), "fn a() {}\nfn b2() {}\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::write(dir.join("new.md"), "# hi\n").unwrap();
        std::fs::write(dir.join("bin.dat"), [0u8, 9]).unwrap();
        let files = vec![
            ChangedFile {
                path: "bin.dat".into(),
                status: "M".into(),
                ..Default::default()
            },
            ChangedFile {
                path: "gone.txt".into(),
                status: "D".into(),
                ..Default::default()
            },
            ChangedFile {
                path: "missing.rs".into(),
                status: "M".into(),
                ..Default::default()
            },
            ChangedFile {
                path: "new.md".into(),
                status: "?".into(),
                ..Default::default()
            },
            ChangedFile {
                path: "src/keep.rs".into(),
                status: "M".into(),
                ..Default::default()
            },
        ];
        let got = load_changed(&dir, &base, &files).unwrap();
        assert!(got[0].binary);
        assert_eq!(
            (got[1].old.as_deref(), got[1].new.as_deref()),
            (Some("bye\n"), None)
        );
        assert_eq!((got[2].old.as_deref(), got[2].new.as_deref()), (None, None));
        assert_eq!(
            (got[3].old.as_deref(), got[3].new.as_deref()),
            (None, Some("# hi\n"))
        );
        assert_eq!(got[4].old.as_deref(), Some("fn a() {}\nfn b() {}\n"));
        assert_eq!(got[4].new.as_deref(), Some("fn a() {}\nfn b2() {}\n"));
        let all = from_git(&dir, "HEAD").unwrap();
        assert_eq!(
            all.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            ["gone.txt", "new.md", "src/keep.rs"]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Size limit and NUL detection of one blob / worktree file.
    #[test]
    fn text_limit_and_nul_bytes() {
        assert_eq!(text_or_none(b"a\r\nb".to_vec()).as_deref(), Some("a\r\nb"));
        assert_eq!(text_or_none(vec![b'a', 0, b'b']), None);
        assert_eq!(
            text_or_none(vec![b'x'; MAX_TEXT_BYTES]).map(|t| t.len()),
            Some(MAX_TEXT_BYTES)
        );
        assert_eq!(text_or_none(vec![b'x'; MAX_TEXT_BYTES + 1]), None);
        assert_eq!(
            text_or_none(vec![0xff, b'\n']).as_deref(),
            Some("\u{fffd}\n")
        );
    }

    /// `(added, deleted)` lines of a full-context patch.
    fn patch_counts(patch: &str) -> (u64, u64) {
        let body = patch.split_once("\n@@ ").map_or("", |(_, b)| b);
        body.lines()
            .skip(1)
            .fold((0, 0), |(a, d), l| match l.as_bytes().first() {
                Some(b'+') => (a + 1, d),
                Some(b'-') => (a, d + 1),
                _ => (a, d),
            })
    }

    /// Line-ending scenarios against `git diff --numstat` (the reference):
    /// committed bytes, config, attributes, worktree bytes -> same counts.
    #[test]
    fn line_endings_match_git_diff() {
        /// (name, committed, autocrlf, info/attributes, worktree, expected (+, -), eol only)
        type Case = (
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            &'static str,
            (u64, u64),
            bool,
        );
        let cases: &[Case] = &[
            (
                "autocrlf true, LF repo, CRLF checkout, one edit",
                "a\nb\nc\nd\n",
                "true",
                "",
                "a\r\nB\r\nc\r\nd\r\n",
                (1, 1),
                false,
            ),
            (
                "autocrlf input, CRLF edit",
                "a\nb\nc\nd\n",
                "input",
                "",
                "a\r\nB\r\nc\r\nd\r\n",
                (1, 1),
                false,
            ),
            (
                "autocrlf false, CRLF edit = whole file",
                "a\nb\nc\n",
                "false",
                "",
                "a\r\nB\r\nc\r\n",
                (3, 3),
                false,
            ),
            (
                "autocrlf false, pure conversion",
                "a\nb\nc\n",
                "false",
                "",
                "a\r\nb\r\nc\r\n",
                (3, 3),
                true,
            ),
            (
                "text eol=crlf attribute",
                "a\nb\nc\n",
                "false",
                "*.txt text eol=crlf\n",
                "a\r\nB\r\nc\r\n",
                (1, 1),
                false,
            ),
            (
                "text=auto attribute",
                "a\nb\nc\n",
                "false",
                "*.txt text=auto\n",
                "a\r\nB\r\nc\r\n",
                (1, 1),
                false,
            ),
            (
                "-text attribute beats autocrlf",
                "a\nb\nc\n",
                "true",
                "*.txt -text\n",
                "a\r\nB\r\nc\r\n",
                (3, 3),
                false,
            ),
            (
                "genuinely CRLF in repo, one edit",
                "a\r\nb\r\nc\r\n",
                "true",
                "",
                "a\r\nB\r\nc\r\n",
                (1, 1),
                false,
            ),
            (
                "genuinely CRLF in repo, converted to LF",
                "a\r\nb\r\nc\r\n",
                "true",
                "",
                "a\nb\nc\n",
                (3, 3),
                true,
            ),
            (
                "mixed endings",
                "a\nb\nc\nd\n",
                "true",
                "",
                "a\r\nb\nC\r\nd\n",
                (1, 1),
                false,
            ),
            (
                "lone CR is not normalized by auto",
                "a\nb\n",
                "true",
                "",
                "a\rx\r\nb\r\n",
                (2, 2),
                false,
            ),
        ];
        for (k, &(name, committed, autocrlf, attrs, worktree, expected, eol_only)) in
            cases.iter().enumerate()
        {
            let dir = std::env::temp_dir().join(format!("pod-diff-eol-{}-{k}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let run = |args: &[&str]| git(&dir, args).unwrap();
            run(&["init", "-q"]);
            run(&["config", "user.email", "t@example.com"]);
            run(&["config", "user.name", "t"]);
            run(&["config", "core.autocrlf", "false"]);
            run(&["config", "core.safecrlf", "false"]);
            std::fs::write(dir.join("f.txt"), committed).unwrap();
            run(&["add", "."]);
            run(&["commit", "-q", "-m", "base"]);
            run(&["config", "core.autocrlf", autocrlf]);
            std::fs::create_dir_all(dir.join(".git/info")).unwrap();
            std::fs::write(dir.join(".git/info/attributes"), attrs).unwrap();
            std::fs::write(dir.join("f.txt"), worktree).unwrap();
            let numstat =
                String::from_utf8(run(&["diff", "--numstat", "HEAD", "--", "f.txt"])).unwrap();
            let mut cols = numstat.split_whitespace();
            let git_counts: (u64, u64) = (
                cols.next().unwrap().parse().unwrap(),
                cols.next().unwrap().parse().unwrap(),
            );
            assert_eq!(git_counts, expected, "{name}: git diff itself");
            let files = vec![ChangedFile {
                path: "f.txt".into(),
                status: "M".into(),
                ..Default::default()
            }];
            let got = load_changed(&dir, "HEAD", &files).unwrap();
            assert_eq!(
                patch_counts(&full_context_patch(&got[0])),
                git_counts,
                "{name}: {got:?}"
            );
            let (old, new) = (
                got[0].old.as_deref().unwrap(),
                got[0].new.as_deref().unwrap(),
            );
            assert_eq!(crate::eol::eol_only_change(old, new), eol_only, "{name}");
            // Line numbers are unchanged by the conversion (comment excerpts and
            // the delivered text read the raw worktree file by line number).
            assert_eq!(new.lines().count(), worktree.lines().count(), "{name}");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
