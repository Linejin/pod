//! The review diff of a task whose worktree is reached through its daemon
//! (docs/17 §2.4 and R5): the daemon lists the changed files with their
//! attributes and revisions (`review.diff`), gives the old side and the
//! index blobs (`review.blob`) and the worktree side (`fs.read`); this
//! module builds the same [`SourceFile`]s a local task gets, with the same
//! rules ([`super::load`]): binary and oversized sides, added and deleted
//! sides, CRLF cleaned the way `git diff` does. Nothing here runs git or
//! reads a local path, and no diff or textconv program runs anywhere.
//!
//! A worktree file whose revision differs from the listing changed after
//! it was listed: it is reported in [`Remote::stale`] (list again).

use anyhow::Result;

use super::load::{apply_eol, assemble, old_side_wanted};
use super::{ChangedFile, SourceFile};
use crate::eol::AutoCrlf;

/// One changed file as the daemon lists it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListedFile {
    pub file: ChangedFile,
    /// `check-attr` `text`, `crlf`, `eol` of the path.
    pub attrs: (String, String, String),
    /// The worktree file's revision when listed.
    pub revision: Option<String>,
}

/// The daemon's listing of a task's changes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    /// The commit the old side comes from.
    pub merge_base: String,
    /// The worktree's `core.autocrlf`.
    pub autocrlf: Option<String>,
    pub files: Vec<ListedFile>,
}

/// Where the bytes come from (the daemon's `review.blob` / `fs.read`).
pub trait ReviewSource {
    /// The blob of `path` at `rev` (`""`: the index); `None` when absent.
    fn blob(&self, rev: &str, path: &str) -> Result<Option<Vec<u8>>>;
    /// The worktree file and its revision; `None` when missing, refused
    /// (link, too large) or unreadable.
    fn worktree(&self, path: &str) -> Result<Option<(Vec<u8>, String)>>;
}

/// The files, and those whose worktree side changed since listed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Remote {
    pub files: Vec<SourceFile>,
    pub stale: Vec<String>,
}

/// Build the review's files from a listing.
pub fn load_remote(src: &dyn ReviewSource, listing: &Listing) -> Result<Remote> {
    let changed: Vec<ChangedFile> = listing.files.iter().map(|f| f.file.clone()).collect();
    let mut old: Vec<Option<Vec<u8>>> = vec![None; changed.len()];
    for (i, path) in old_side_wanted(&changed) {
        old[i] = src.blob(&listing.merge_base, path)?;
    }
    let mut stale = Vec::new();
    let mut failed = None;
    let (mut files, crlf) = assemble(&changed, old, |f| {
        let i = changed.iter().position(|c| c.path == f.path)?;
        match src.worktree(&f.path) {
            Ok(Some((bytes, rev))) => {
                if listing.files[i].revision.as_deref() != Some(rev.as_str()) {
                    stale.push(f.path.clone());
                }
                Some(bytes)
            }
            Ok(None) => None,
            Err(e) => {
                failed.get_or_insert(e);
                None
            }
        }
    });
    if let Some(e) = failed {
        return Err(e);
    }
    if !crlf.is_empty() {
        let mut attrs = Vec::with_capacity(crlf.len());
        let mut index = Vec::with_capacity(crlf.len());
        for &i in &crlf {
            attrs.push(listing.files[i].attrs.clone());
            index.push(src.blob("", &listing.files[i].file.path)?);
        }
        apply_eol(
            &mut files,
            &crlf,
            attrs,
            index,
            AutoCrlf::parse(listing.autocrlf.as_deref()),
        );
    }
    Ok(Remote { files, stale })
}

#[cfg(test)]
mod tests {
    use super::super::git::{cat_files, git};
    use super::super::load::load_changed;
    use super::*;
    use crate::eol;

    /// A stand-in for the daemon, backed by a local repository: blobs by
    /// `cat-file`, the worktree by reading files.
    struct LocalDaemon<'a> {
        dir: &'a std::path::Path,
        changed_after_listing: Option<&'a str>,
    }

    impl ReviewSource for LocalDaemon<'_> {
        fn blob(&self, rev: &str, path: &str) -> Result<Option<Vec<u8>>> {
            Ok(cat_files(self.dir, &[format!("{rev}:{path}")])?.remove(0))
        }
        fn worktree(&self, path: &str) -> Result<Option<(Vec<u8>, String)>> {
            let p = self.dir.join(path);
            if self.changed_after_listing == Some(path) {
                std::fs::write(&p, b"changed after the listing\n")?;
            }
            Ok(std::fs::read(&p).ok().map(|b| {
                let r = format!("{}", b.len());
                (b, r)
            }))
        }
    }

    fn listing(dir: &std::path::Path, base: &str, files: &[ChangedFile]) -> Listing {
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        let attrs = eol::attributes(dir, &paths).unwrap();
        let autocrlf =
            String::from_utf8(git(dir, &["config", "--get", "core.autocrlf"]).unwrap_or_default())
                .unwrap()
                .trim()
                .to_string();
        Listing {
            merge_base: base.into(),
            autocrlf: (!autocrlf.is_empty()).then_some(autocrlf),
            files: files
                .iter()
                .zip(attrs)
                .map(|(f, a)| ListedFile {
                    file: f.clone(),
                    attrs: a,
                    revision: std::fs::read(dir.join(&f.path))
                        .ok()
                        .map(|b| format!("{}", b.len())),
                })
                .collect(),
        }
    }

    /// The remote path gives exactly what the local path gives (CRLF with
    /// autocrlf, a `-text` file, binary, rename, delete, add, untracked,
    /// Korean and spaced names), and names a file changed after listing.
    #[test]
    fn remote_load_matches_the_local_load() {
        let dir = std::env::temp_dir().join(format!("pod-diff-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let run = |args: &[&str]| git(&dir, args).unwrap();
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        run(&["config", "core.autocrlf", "false"]);
        let w = |p: &str, b: &[u8]| std::fs::write(dir.join(p), b).unwrap();
        w("src/keep.rs", b"fn a() {}\nfn b() {}\n");
        w("gone.txt", b"bye\n");
        w("bin.dat", &[0, 1, 2]);
        w("old name.txt", b"same\nline\n");
        w("crlf.txt", b"a\nb\nc\n");
        w("eolonly.txt", b"x\ny\n");
        w("한글 문서.md", "가\n나\n".as_bytes());
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
        std::fs::remove_file(dir.join("old name.txt")).unwrap();
        w("new name.txt", b"same\nline 2\n");
        w("crlf.txt", b"a\r\nB\r\nc\r\n");
        w("eolonly.txt", b"x\r\ny\r\n");
        w("한글 문서.md", "가\r\n다\r\n".as_bytes());
        w("added.rs", b"fn n() {}\n");
        run(&["add", "added.rs"]);
        w("untracked new.md", b"# hi\n");
        let cf = |path: &str, old_path: Option<&str>, status: &str| ChangedFile {
            path: path.into(),
            old_path: old_path.map(Into::into),
            status: status.into(),
            binary: false,
        };
        let files = vec![
            cf("added.rs", None, "A"),
            cf("bin.dat", None, "M"),
            cf("crlf.txt", None, "M"),
            cf("eolonly.txt", None, "M"),
            cf("gone.txt", None, "D"),
            cf("new name.txt", Some("old name.txt"), "R"),
            cf("src/keep.rs", None, "M"),
            cf("untracked new.md", None, "?"),
            cf("한글 문서.md", None, "M"),
        ];
        let local = load_changed(&dir, &base, &files).unwrap();
        let l = listing(&dir, &base, &files);
        let src = LocalDaemon {
            dir: &dir,
            changed_after_listing: None,
        };
        let remote = load_remote(&src, &l).unwrap();
        assert_eq!(remote.files, local);
        assert!(remote.stale.is_empty());
        let crlf = local.iter().find(|f| f.path == "crlf.txt").unwrap();
        assert_eq!(
            crlf.new.as_deref(),
            Some("a\nB\nc\n"),
            "cleaned like git diff"
        );

        let src = LocalDaemon {
            dir: &dir,
            changed_after_listing: Some("src/keep.rs"),
        };
        let remote = load_remote(&src, &l).unwrap();
        assert_eq!(remote.stale, ["src/keep.rs"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
