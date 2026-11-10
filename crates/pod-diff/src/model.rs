//! Diff input: two files, a git working tree, or a podd task, turned into
//! full-context patches that gpui-component's `DiffFile::parse` accepts.
//!
//! Why full context: the component only folds/expands lines that exist in the
//! patch. A `git diff` patch has 3 lines of context, so "expand" could never
//! reveal the rest of the file. We compute the line diff ourselves with
//! `similar` and emit every line, then let `DiffState` collapse unchanged runs.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};
use similar::{Algorithm, ChangeTag, TextDiff};

use crate::eol;

/// Files larger than this are shown as "too large" instead of being diffed.
pub const MAX_TEXT_BYTES: usize = 8 * 1024 * 1024;

/// One file of the review: both complete versions (None = added/deleted).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceFile {
    /// `/`-separated path (the new path of a rename).
    pub path: String,
    /// Previous path of a rename / copy.
    pub old_path: Option<String>,
    pub old: Option<String>,
    pub new: Option<String>,
    /// Binary or too large: listed, but no lines.
    pub binary: bool,
}

impl SourceFile {
    pub fn text(&self, new_side: bool) -> &str {
        if new_side {
            self.new.as_deref().unwrap_or("")
        } else {
            self.old.as_deref().unwrap_or("")
        }
    }

    /// 1-based inclusive line range of one side, for comment excerpts.
    pub fn lines(&self, new_side: bool, start: usize, end: usize) -> String {
        self.text(new_side)
            .split_inclusive('\n')
            .skip(start.saturating_sub(1))
            .take(end.saturating_sub(start) + 1)
            .collect()
    }

    /// Number of lines of the longer side.
    pub fn line_count(&self) -> usize {
        self.text(false)
            .lines()
            .count()
            .max(self.text(true).lines().count())
    }
}

fn quoted(name: &str) -> String {
    format!(
        "\"{}\"",
        name.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

/// A one-hunk, full-context git-style patch for one file. Lines are written
/// verbatim (CRLF preserved), so `similar`'s own unified writer is not used.
pub fn full_context_patch(file: &SourceFile) -> String {
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let a = quoted(&format!("a/{old_path}"));
    let b = quoted(&format!("b/{}", file.path));
    let mut patch = format!("diff --git {a} {b}\n");
    if file.old_path.as_deref().is_some_and(|o| o != file.path) {
        patch.push_str(&format!(
            "rename from {}\nrename to {}\n",
            quoted(old_path),
            quoted(&file.path)
        ));
    }
    match (&file.old, &file.new) {
        (None, Some(_)) => patch.push_str("new file mode 100644\n"),
        (Some(_), None) => patch.push_str("deleted file mode 100644\n"),
        _ => {}
    }
    if file.binary {
        patch.push_str(&format!("Binary files {a} and {b} differ\n"));
        return patch;
    }
    let old = file.old.as_deref().unwrap_or("");
    let new = file.new.as_deref().unwrap_or("");
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let old_name = if file.old.is_some() {
        a
    } else {
        "/dev/null".into()
    };
    let new_name = if file.new.is_some() {
        b
    } else {
        "/dev/null".into()
    };
    patch.push_str(&format!("--- {old_name}\n+++ {new_name}\n"));
    if old_lines.is_empty() && new_lines.is_empty() {
        return patch;
    }
    patch.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        usize::from(!old_lines.is_empty()),
        old_lines.len(),
        usize::from(!new_lines.is_empty()),
        new_lines.len()
    ));
    // Patience keeps moved blocks of code readable; Myers is the fallback default.
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_slices(&old_lines, &new_lines);
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            patch.push(match change.tag() {
                ChangeTag::Equal => ' ',
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
            });
            patch.push_str(change.value());
            if !change.value().ends_with('\n') {
                patch.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    patch
}

/// All files as one patch.
pub fn patch_of(files: &[SourceFile]) -> String {
    files.iter().map(full_context_patch).collect()
}

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

/// `git` with `CREATE_NO_WINDOW`: a GUI-subsystem parent has no console, so
/// each console child would otherwise get its own (a visible flash and
/// ~250ms per call, docs/05 §5-2).
pub fn git_command(repo: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(repo);
    cmd.env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = git_command(repo)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// Text, or `None` for binary (NUL byte) or oversized content.
fn text_or_none(bytes: Vec<u8>) -> Option<String> {
    if bytes.len() > MAX_TEXT_BYTES || bytes.contains(&0) {
        None
    } else {
        Some(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// Read many blobs (`<rev>:<path>` specs) with one `git cat-file --batch`
/// process instead of one `git show` per file (~50ms each on Windows).
/// Missing objects come back as `None`.
pub fn cat_files(repo: &Path, specs: &[String]) -> Result<Vec<Option<Vec<u8>>>> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git_command(repo)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("starting git cat-file")?;
    let mut stdin = child.stdin.take().context("cat-file stdin")?;
    let input: String = specs.iter().map(|s| format!("{s}\n")).collect();
    // Write on another thread: git answers while we are still writing.
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(input.as_bytes());
    });
    let mut out = BufReader::new(child.stdout.take().context("cat-file stdout")?);
    let mut result = Vec::with_capacity(specs.len());
    let mut header = String::new();
    for _ in specs {
        header.clear();
        if out.read_line(&mut header)? == 0 {
            bail!("git cat-file ended early");
        }
        match batch_header_size(&header) {
            Some(size) => {
                let mut buf = vec![0u8; size];
                out.read_exact(&mut buf)?;
                let mut nl = [0u8; 1];
                out.read_exact(&mut nl)?;
                result.push(Some(buf));
            }
            None => result.push(None),
        }
    }
    let _ = writer.join();
    let _ = child.wait();
    Ok(result)
}

/// The object size of a `cat-file --batch` header: `"<sha> <type> <size>"`.
/// Anything else is `"<spec> missing"` / `"<spec> ambiguous"`, whose spec
/// may itself hold spaces and numbers (`HEAD:a b 12 missing`).
fn batch_header_size(header: &str) -> Option<usize> {
    let parts: Vec<&str> = header.trim_end_matches(['\n', '\r']).split(' ').collect();
    let [sha, kind, size] = parts.as_slice() else {
        return None;
    };
    let is_sha = matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit());
    let is_kind = matches!(*kind, "blob" | "tree" | "commit" | "tag");
    if is_sha && is_kind {
        size.parse().ok()
    } else {
        None
    }
}

/// One changed file as podd's `task.diff` lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    /// `A M D R C T ?`
    pub status: String,
    pub binary: bool,
}

/// Both versions of each changed file: the old side from `base` (the task's
/// merge base) via one `git cat-file --batch`, the new side from the worktree.
pub fn load_changed(worktree: &Path, base: &str, files: &[ChangedFile]) -> Result<Vec<SourceFile>> {
    let added = |f: &ChangedFile| matches!(f.status.as_str(), "A" | "?");
    let old_specs: Vec<(usize, String)> = files
        .iter()
        .enumerate()
        .filter(|(_, f)| !added(f) && !f.binary)
        .map(|(i, f)| {
            (
                i,
                format!("{base}:{}", f.old_path.as_deref().unwrap_or(&f.path)),
            )
        })
        .collect();
    let specs: Vec<String> = old_specs.iter().map(|(_, s)| s.clone()).collect();
    let blobs = cat_files(worktree, &specs)?;
    let mut old: Vec<Option<Vec<u8>>> = vec![None; files.len()];
    for ((i, _), blob) in old_specs.into_iter().zip(blobs) {
        old[i] = blob;
    }
    let mut out = Vec::with_capacity(files.len());
    let mut crlf: Vec<usize> = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let mut binary = f.binary;
        let old_text = match old[i].take() {
            Some(b) => {
                let t = text_or_none(b);
                binary |= t.is_none();
                t
            }
            None => None,
        };
        let new_text = if f.status == "D" || f.binary {
            None
        } else {
            match std::fs::read(worktree.join(&f.path)) {
                Ok(b) => {
                    let t = text_or_none(b);
                    binary |= t.is_none();
                    t
                }
                Err(_) => None,
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
    clean_worktree_eol(worktree, &mut out, &crlf)?;
    Ok(out)
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
    for ((&i, (text, crlf, eol_attr)), index) in idx.iter().zip(attrs).zip(index) {
        let action = eol::action(&text, &crlf, &eol_attr, autocrlf);
        if let Some(new) = files[i].new.as_deref()
            && let Some(cleaned) = eol::clean(new, action, index.as_deref())
        {
            files[i].new = Some(cleaned);
        }
    }
    Ok(())
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

/// Writes a pair of ~`lines`-line Rust-like files with `changes` scattered
/// edits (modify / insert / delete mix) for the performance run.
pub fn generate_bench(dir: &Path, lines: usize, changes: usize) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(dir)?;
    let (old, new) = bench_texts(lines, changes);
    let (po, pn) = (dir.join("bench_old.rs"), dir.join("bench_new.rs"));
    std::fs::write(&po, old)?;
    std::fs::write(&pn, new)?;
    Ok((po, pn))
}

/// The two texts of [`generate_bench`] (deterministic).
pub fn bench_texts(lines: usize, changes: usize) -> (String, String) {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rnd = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let idents = [
        "alpha", "beta", "gamma", "delta", "value", "count", "buffer", "index", "result", "config",
    ];
    let mut old = Vec::with_capacity(lines);
    for i in 0..lines {
        let a = idents[(rnd() % 10) as usize];
        let b = idents[(rnd() % 10) as usize];
        let line = match i % 12 {
            0 => format!("fn function_{i}({a}: usize, {b}: &str) -> Result<usize, Error> {{"),
            11 => "}".to_string(),
            5 => format!("    // {a} is combined with {b} here (line {i})"),
            7 => format!("    let {a}_{i} = \"{b} string literal {i}\";"),
            _ => format!(
                "    let {a}_{i} = {b}.saturating_add({}) * {};",
                rnd() % 1000,
                rnd() % 17
            ),
        };
        old.push(line);
    }
    let mut new = old.clone();
    if lines > 0 {
        // Choose `changes` distinct targets, apply from the bottom so indices stay valid.
        let mut targets: Vec<usize> = (0..changes).map(|_| (rnd() as usize) % lines).collect();
        targets.sort_unstable();
        targets.dedup();
        for (k, &t) in targets.iter().enumerate().rev() {
            match k % 5 {
                0..=2 => {
                    new[t] = new[t]
                        .replace("saturating_add", "wrapping_add")
                        .replace("string", "text")
                        + " // edited"
                }
                3 => new.insert(t, format!("    tracing::debug!(\"inserted probe {t}\");")),
                _ => {
                    new.remove(t);
                }
            }
        }
    }
    (old.join("\n") + "\n", new.join("\n") + "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(old: Option<&str>, new: Option<&str>) -> SourceFile {
        SourceFile {
            path: "src/a.rs".into(),
            old: old.map(Into::into),
            new: new.map(Into::into),
            ..Default::default()
        }
    }

    #[test]
    fn patch_full_context_and_markers() {
        let p = full_context_patch(&file(Some("a\nb\nc\n"), Some("a\nB\nc\n")));
        assert!(p.starts_with("diff --git \"a/src/a.rs\" \"b/src/a.rs\"\n--- \"a/src/a.rs\"\n+++ \"b/src/a.rs\"\n@@ -1,3 +1,3 @@\n"));
        assert!(p.ends_with(" a\n-b\n+B\n c\n"), "{p}");
    }

    #[test]
    fn patch_added_deleted_no_newline_crlf() {
        let p = full_context_patch(&file(None, Some("x\r\ny")));
        assert!(p.contains("new file mode 100644\n--- /dev/null\n"));
        assert!(
            p.contains("@@ -0,0 +1,2 @@\n+x\r\n+y\n\\ No newline at end of file\n"),
            "{p}"
        );
        let p = full_context_patch(&file(Some("gone\n"), None));
        assert!(
            p.contains("deleted file mode 100644\n")
                && p.contains("+++ /dev/null\n@@ -1,1 +0,0 @@\n-gone\n")
        );
    }

    #[test]
    fn patch_rename_and_binary() {
        let mut f = file(Some("same\n"), Some("same\n"));
        f.old_path = Some("src/old.rs".into());
        let p = full_context_patch(&f);
        assert!(
            p.contains("rename from \"src/old.rs\"\nrename to \"src/a.rs\"\n"),
            "{p}"
        );
        let mut b = file(Some(""), Some(""));
        b.binary = true;
        assert!(full_context_patch(&b).contains("Binary files"));
    }

    #[test]
    fn excerpt_lines() {
        let f = file(Some("1\n2\n3\n"), Some("1\n2\n3\n4\n"));
        assert_eq!(f.lines(true, 2, 3), "2\n3\n");
        assert_eq!(f.lines(false, 3, 9), "3\n");
        assert_eq!(f.line_count(), 4);
    }

    #[test]
    fn bench_is_deterministic() {
        let (a, b) = bench_texts(200, 20);
        assert_eq!(bench_texts(200, 20), (a.clone(), b.clone()));
        assert_ne!(a, b);
        assert_eq!(a.lines().count(), 200);
    }

    /// editor-09: a missing spec whose path has spaces and numbers is not
    /// read as a blob header (it would eat the next file's answer).
    #[test]
    fn cat_file_missing_path_with_spaces_and_numbers() {
        let sha = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(batch_header_size(&format!("{sha} blob 12\n")), Some(12));
        assert_eq!(batch_header_size("HEAD:a b 12 missing\n"), None);
        assert_eq!(batch_header_size("HEAD:My Song 2 3.txt missing\n"), None);
        assert_eq!(batch_header_size("HEAD:x 7 missing\n"), None);
        assert_eq!(batch_header_size("abc ambiguous\n"), None);

        let dir = std::env::temp_dir().join(format!("pod-diff-catfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let run = |args: &[&str]| git(&dir, args).unwrap();
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.join("next.txt"), "next\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "base"]);
        let got = cat_files(
            &dir,
            &["HEAD:My Song 2 3.txt".into(), "HEAD:next.txt".into()],
        )
        .unwrap();
        assert_eq!(got, vec![None, Some(b"next\n".to_vec())]);
        let _ = std::fs::remove_dir_all(&dir);
    }

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
