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

fn git(dir: &Path, args: &[&str]) -> std::io::Result<()> {
    let mut c = Command::new("git");
    c.arg("-C").arg(dir).args(args);
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
}
