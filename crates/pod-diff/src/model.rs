//! Diff input: two files, a git working tree, or a podd task, turned into
//! full-context patches that gpui-component's `DiffFile::parse` accepts.
//!
//! Why full context: the component only folds/expands lines that exist in the
//! patch. A `git diff` patch has 3 lines of context, so "expand" could never
//! reveal the rest of the file. We compute the line diff ourselves with
//! `similar` and emit every line, then let `DiffState` collapse unchanged runs.
//!
//! The façade keeps the public types and paths; the work lives in private
//! submodules:
//! - `patch`: the full-context patch text of each file.
//! - `load`: both versions from two files, a git working tree or a task.
//! - `git`: the `git` processes (one `cat-file --batch` for many blobs),
//!   also used by [`crate::eol`].

use std::path::{Path, PathBuf};

use anyhow::Result;

pub(crate) mod git;
mod load;
mod patch;

pub use git::{cat_files, git_command};
pub use load::{from_git, from_two_files, load_changed};
pub use patch::{full_context_patch, patch_of};

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

/// One changed file as podd's `task.diff` lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    /// `A M D R C T ?`
    pub status: String,
    pub binary: bool,
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
    use super::git::git;
    use super::*;

    pub(super) fn file(old: Option<&str>, new: Option<&str>) -> SourceFile {
        SourceFile {
            path: "src/a.rs".into(),
            old: old.map(Into::into),
            new: new.map(Into::into),
            ..Default::default()
        }
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

    /// A populated review on a real repository, pinned before the model was
    /// split into git/patch/load (refactor L6): every source, the patch, the
    /// list rows and `from_git`. Covers one batch process for the old side,
    /// paths with spaces and numbers, binary (flagged, NUL, too large),
    /// rename, add/delete/untracked, missing blob and line-ending cleaning.
    #[test]
    fn populated_review_golden() {
        let dir = std::env::temp_dir().join(format!("pod-diff-golden-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let run = |args: &[&str]| git(&dir, args).unwrap();
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        run(&["config", "core.autocrlf", "false"]);
        run(&["config", "core.safecrlf", "false"]);
        let w = |p: &str, b: &[u8]| std::fs::write(dir.join(p), b).unwrap();
        w("src/keep.rs", b"fn a() {}\nfn b() {}\n");
        w("gone.txt", b"bye\n");
        w("bin.dat", &[0, 1, 2]);
        w("flag.bin", b"text\n");
        w("old name.txt", b"same\nline\n");
        w("crlf.txt", b"a\nb\nc\n");
        w("eolonly.txt", b"x\ny\n");
        w("big.txt", b"small\n");
        w("nonl.txt", b"end");
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "base"]);
        let base = String::from_utf8(run(&["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_string();
        run(&["config", "core.autocrlf", "true"]);
        std::fs::create_dir_all(dir.join(".git/info")).unwrap();
        std::fs::write(dir.join(".git/info/attributes"), "eolonly.txt -text\n").unwrap();
        w("src/keep.rs", b"fn a() {}\nfn b2() {}\n");
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        w("bin.dat", &[0, 9]);
        w("flag.bin", b"text 2\n");
        std::fs::remove_file(dir.join("old name.txt")).unwrap();
        w("new name 2 3.txt", b"same\nline 2\n");
        w("crlf.txt", b"a\r\nB\r\nc\r\n");
        w("eolonly.txt", b"x\r\ny\r\n");
        w("big.txt", &vec![b'x'; MAX_TEXT_BYTES + 1]);
        w("nonl.txt", b"end\nmore");
        w("added.rs", b"fn n() {}\n");
        run(&["add", "added.rs"]);
        std::fs::create_dir_all(dir.join("dir with space")).unwrap();
        w("dir with space/new 1.md", b"# hi\n");
        let cf = |path: &str, old_path: Option<&str>, status: &str, binary: bool| ChangedFile {
            path: path.into(),
            old_path: old_path.map(Into::into),
            status: status.into(),
            binary,
        };
        let files = vec![
            cf("added.rs", None, "A", false),
            cf("big.txt", None, "M", false),
            cf("bin.dat", None, "M", false),
            cf("crlf.txt", None, "M", false),
            cf("dir with space/new 1.md", None, "?", false),
            cf("eolonly.txt", None, "M", false),
            cf("flag.bin", None, "M", true),
            cf("gone.txt", None, "D", false),
            cf("missing 4.rs", None, "M", false),
            cf("new name 2 3.txt", Some("old name.txt"), "R", false),
            cf("nonl.txt", None, "M", false),
            cf("src/keep.rs", None, "M", false),
        ];
        let sources = load_changed(&dir, &base, &files).unwrap();
        let mut got = String::new();
        for s in &sources {
            got.push_str(&format!("{s:?}\n"));
        }
        got.push_str("--- patch\n");
        got.push_str(&patch_of(&sources).replace('\r', "\\r"));
        got.push_str("--- rows\n");
        // A file missing on both sides fails the whole parse (as before L6).
        let (parsed, _, error) = crate::parse(&sources);
        got.push_str(&format!("all: {} files, error {error:?}\n", parsed.len()));
        let present: Vec<SourceFile> = sources
            .iter()
            .filter(|s| s.old.is_some() || s.new.is_some())
            .cloned()
            .collect();
        let (parsed, rows, error) = crate::parse(&present);
        got.push_str(&format!(
            "present: {} files, error {error:?}\n",
            parsed.len()
        ));
        for r in &rows {
            got.push_str(&format!("{r:?}\n"));
        }
        got.push_str("--- from_git\n");
        for s in from_git(&dir, "HEAD").unwrap() {
            got.push_str(&format!("{s:?}\n"));
        }
        let _ = std::fs::remove_dir_all(&dir);
        let expected = r##"SourceFile { path: "added.rs", old_path: None, old: None, new: Some("fn n() {}\n"), binary: false }
SourceFile { path: "big.txt", old_path: None, old: Some(""), new: Some(""), binary: true }
SourceFile { path: "bin.dat", old_path: None, old: Some(""), new: Some(""), binary: true }
SourceFile { path: "crlf.txt", old_path: None, old: Some("a\nb\nc\n"), new: Some("a\nB\nc\n"), binary: false }
SourceFile { path: "dir with space/new 1.md", old_path: None, old: None, new: Some("# hi\n"), binary: false }
SourceFile { path: "eolonly.txt", old_path: None, old: Some("x\ny\n"), new: Some("x\r\ny\r\n"), binary: false }
SourceFile { path: "flag.bin", old_path: None, old: Some(""), new: Some(""), binary: true }
SourceFile { path: "gone.txt", old_path: None, old: Some("bye\n"), new: None, binary: false }
SourceFile { path: "missing 4.rs", old_path: None, old: None, new: None, binary: false }
SourceFile { path: "new name 2 3.txt", old_path: Some("old name.txt"), old: Some("same\nline\n"), new: Some("same\nline 2\n"), binary: false }
SourceFile { path: "nonl.txt", old_path: None, old: Some("end"), new: Some("end\nmore"), binary: false }
SourceFile { path: "src/keep.rs", old_path: None, old: Some("fn a() {}\nfn b() {}\n"), new: Some("fn a() {}\nfn b2() {}\n"), binary: false }
--- patch
diff --git "a/added.rs" "b/added.rs"
new file mode 100644
--- /dev/null
+++ "b/added.rs"
@@ -0,0 +1,1 @@
+fn n() {}
diff --git "a/big.txt" "b/big.txt"
Binary files "a/big.txt" and "b/big.txt" differ
diff --git "a/bin.dat" "b/bin.dat"
Binary files "a/bin.dat" and "b/bin.dat" differ
diff --git "a/crlf.txt" "b/crlf.txt"
--- "a/crlf.txt"
+++ "b/crlf.txt"
@@ -1,3 +1,3 @@
 a
-b
+B
 c
diff --git "a/dir with space/new 1.md" "b/dir with space/new 1.md"
new file mode 100644
--- /dev/null
+++ "b/dir with space/new 1.md"
@@ -0,0 +1,1 @@
+# hi
diff --git "a/eolonly.txt" "b/eolonly.txt"
--- "a/eolonly.txt"
+++ "b/eolonly.txt"
@@ -1,2 +1,2 @@
-x
-y
+x\r
+y\r
diff --git "a/flag.bin" "b/flag.bin"
Binary files "a/flag.bin" and "b/flag.bin" differ
diff --git "a/gone.txt" "b/gone.txt"
deleted file mode 100644
--- "a/gone.txt"
+++ /dev/null
@@ -1,1 +0,0 @@
-bye
diff --git "a/missing 4.rs" "b/missing 4.rs"
--- /dev/null
+++ /dev/null
diff --git "a/old name.txt" "b/new name 2 3.txt"
rename from "old name.txt"
rename to "new name 2 3.txt"
--- "a/old name.txt"
+++ "b/new name 2 3.txt"
@@ -1,2 +1,2 @@
 same
-line
+line 2
diff --git "a/nonl.txt" "b/nonl.txt"
--- "a/nonl.txt"
+++ "b/nonl.txt"
@@ -1,1 +1,2 @@
-end
\ No newline at end of file
+end
+more
\ No newline at end of file
diff --git "a/src/keep.rs" "b/src/keep.rs"
--- "a/src/keep.rs"
+++ "b/src/keep.rs"
@@ -1,2 +1,2 @@
 fn a() {}
-fn b() {}
+fn b2() {}
--- rows
all: 0 files, error Some("line 43: both file sides are missing")
present: 11 files, error None
FileRow { path: "added.rs", status: Added, additions: 1, deletions: 0, binary: false, eol_only: false }
FileRow { path: "big.txt", status: Modified, additions: 0, deletions: 0, binary: true, eol_only: false }
FileRow { path: "bin.dat", status: Modified, additions: 0, deletions: 0, binary: true, eol_only: false }
FileRow { path: "crlf.txt", status: Modified, additions: 1, deletions: 1, binary: false, eol_only: false }
FileRow { path: "dir with space/new 1.md", status: Added, additions: 1, deletions: 0, binary: false, eol_only: false }
FileRow { path: "eolonly.txt", status: Modified, additions: 2, deletions: 2, binary: false, eol_only: true }
FileRow { path: "flag.bin", status: Modified, additions: 0, deletions: 0, binary: true, eol_only: false }
FileRow { path: "gone.txt", status: Deleted, additions: 0, deletions: 1, binary: false, eol_only: false }
FileRow { path: "new name 2 3.txt", status: Renamed, additions: 1, deletions: 1, binary: false, eol_only: false }
FileRow { path: "nonl.txt", status: Modified, additions: 2, deletions: 1, binary: false, eol_only: false }
FileRow { path: "src/keep.rs", status: Modified, additions: 1, deletions: 1, binary: false, eol_only: false }
--- from_git
SourceFile { path: "added.rs", old_path: None, old: None, new: Some("fn n() {}\n"), binary: false }
SourceFile { path: "crlf.txt", old_path: None, old: Some("a\nb\nc\n"), new: Some("a\nB\nc\n"), binary: false }
SourceFile { path: "dir with space/new 1.md", old_path: None, old: None, new: Some("# hi\n"), binary: false }
SourceFile { path: "eolonly.txt", old_path: None, old: Some("x\ny\n"), new: Some("x\r\ny\r\n"), binary: false }
SourceFile { path: "flag.bin", old_path: None, old: Some("text\n"), new: Some("text 2\n"), binary: false }
SourceFile { path: "gone.txt", old_path: None, old: Some("bye\n"), new: None, binary: false }
SourceFile { path: "new name 2 3.txt", old_path: None, old: None, new: Some("same\nline 2\n"), binary: false }
SourceFile { path: "nonl.txt", old_path: None, old: Some("end"), new: Some("end\nmore"), binary: false }
SourceFile { path: "old name.txt", old_path: None, old: Some("same\nline\n"), new: None, binary: false }
SourceFile { path: "src/keep.rs", old_path: None, old: Some("fn a() {}\nfn b() {}\n"), new: Some("fn a() {}\nfn b2() {}\n"), binary: false }
"##;
        assert_eq!(got, expected, "\n{got}");
    }
}
