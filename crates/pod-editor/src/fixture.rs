//! Small demo repository (Rust + Python + TS + JSON + Markdown) used by the
//! demo binary and the rust-analyzer smoke test. Created as a real git
//! repository with a few working-tree changes so status markers show.

use std::path::{Path, PathBuf};
use std::process::Command;

const FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        include_str!("../fixtures/demo/Cargo.toml.fixture"),
    ),
    ("src/lib.rs", include_str!("../fixtures/demo/src/lib.rs")),
    (
        "src/geometry.rs",
        include_str!("../fixtures/demo/src/geometry.rs"),
    ),
    ("src/main.rs", include_str!("../fixtures/demo/src/main.rs")),
    (
        "scripts/tool.py",
        include_str!("../fixtures/demo/scripts/tool.py"),
    ),
    ("web/app.ts", include_str!("../fixtures/demo/web/app.ts")),
    (
        "config/settings.json",
        include_str!("../fixtures/demo/config/settings.json"),
    ),
    ("README.md", include_str!("../fixtures/demo/README.md")),
    (
        "docs/한글 노트.md",
        include_str!("../fixtures/demo/docs/한글 노트.md"),
    ),
    (".gitignore", "target/\n*.log\n"),
];

/// Author and committer date of the fixture commit: with the fixed tree,
/// name and message, `HEAD` gets the same hash on every run (the review
/// scene prints it).
pub const COMMIT_DATE: &str = "2026-01-01T00:00:00+0000";

fn git(dir: &Path, args: &[&str]) -> std::io::Result<()> {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).args(args);
    c.env("GIT_AUTHOR_DATE", COMMIT_DATE)
        .env("GIT_COMMITTER_DATE", COMMIT_DATE);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }
}

/// The sources are `include_str!`ed, so their line endings follow the
/// checkout (CRLF with `core.autocrlf=true`). Written always as LF, so every
/// checkout produces the same files and the same editor scene (ui-editor-07).
fn lf(body: &str) -> String {
    body.replace("\r\n", "\n")
}

/// Writes the fixture into `dest` (created; must be empty or absent).
/// `with_changes` adds: README modified, `notes/todo.md` untracked, and a
/// call with the wrong argument count in `src/main.rs` (a diagnostic).
pub fn create(dest: &Path, with_changes: bool) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dest)?;
    for (rel, body) in FILES {
        let p = dest.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(p, lf(body))?;
    }
    // Git is optional: without it the tree simply has no markers.
    if git(dest, &["init", "-q"]).is_ok() {
        let _ = git(dest, &["config", "user.email", "fixture@pod.invalid"]);
        let _ = git(dest, &["config", "user.name", "Pod fixture"]);
        let _ = git(dest, &["config", "core.autocrlf", "false"]);
        let _ = git(dest, &["add", "-A"]);
        let _ = git(dest, &["commit", "-qm", "fixture"]);
    }
    std::fs::write(dest.join("target.log"), "ignored by .gitignore\n")?;
    if with_changes {
        let readme = dest.join("README.md");
        let mut s = std::fs::read_to_string(&readme)?;
        s.push_str("- `docs/` — 한글 문서\n");
        std::fs::write(readme, s)?;
        std::fs::create_dir_all(dest.join("notes"))?;
        std::fs::write(dest.join("notes/todo.md"), "# TODO\n\n- LSP 확인\n")?;
        let main = dest.join("src/main.rs");
        let s = std::fs::read_to_string(&main)?.replace("    let mut task", "    let total = word_count(&message, 2);\n    println!(\"{total}\");\n\n    let mut task");
        std::fs::write(main, s)?;
    }
    Ok(dest.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// LF and CRLF checkouts of the fixture sources write identical bytes.
    #[test]
    fn fixture_bytes_do_not_depend_on_checkout_line_endings() {
        for (rel, body) in FILES {
            let crlf = body.replace("\r\n", "\n").replace('\n', "\r\n");
            let only_lf = body.replace("\r\n", "\n");
            assert_eq!(lf(&crlf), lf(&only_lf), "{rel}");
            assert!(!lf(body).contains('\r'), "{rel}");
        }
        let dir = std::env::temp_dir().join(format!("pod-fixture-eol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create(&dir, true).unwrap();
        for (rel, body) in FILES {
            let written = std::fs::read(dir.join(rel)).unwrap();
            assert!(!written.contains(&b'\r'), "{rel} written with CR");
            if !matches!(*rel, "README.md" | "src/main.rs") {
                assert_eq!(written, lf(body).into_bytes(), "{rel}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// A-F0b: the commit used the current time, so `HEAD` (shown by the review
    /// scene) changed from run to run. Two runs a second apart get one hash.
    #[test]
    fn fixture_commit_has_a_fixed_date_and_hash() {
        let base = std::env::temp_dir().join(format!("pod-fixture-head-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let heads: Vec<Option<String>> = ["a", "b"]
            .iter()
            .enumerate()
            .map(|(i, name)| {
                if i > 0 {
                    // Commit times have one-second resolution.
                    std::thread::sleep(std::time::Duration::from_millis(1100));
                }
                let dir = create(&base.join(name), true).unwrap();
                git_out(&dir, &["rev-parse", "HEAD"])
            })
            .collect();
        let Some(Some(head)) = heads.first().cloned() else {
            let _ = std::fs::remove_dir_all(&base);
            eprintln!("skipped: no git");
            return;
        };
        assert_eq!(heads[1].as_deref(), Some(head.as_str()));
        // 2026-01-01T00:00:00Z, author and committer.
        let dates = git_out(&base.join("a"), &["log", "-1", "--format=%at %ct"]).unwrap();
        assert_eq!(dates, "1767225600 1767225600");
        let _ = std::fs::remove_dir_all(&base);
    }
}
