//! The relative path of a remote file request, as text (docs/22 TM-15): what
//! is refused before anything is opened. The OS side (links, aliases, hard
//! links) is [`super::held`].

use crate::git::pin::bad_name;
use crate::protocol::MAX_PATH_BYTES;

/// A path under a task's worktree: plain names, no `.`/`..`, never `.git`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rel {
    pub parts: Vec<String>,
}

impl Rel {
    /// `/`-separated, as the daemon reads it (`""` for the worktree).
    pub fn text(&self) -> String {
        self.parts.join("/")
    }

    /// The folders on the way to the last part, and the last part.
    pub fn split_leaf(&self) -> Option<(&[String], &str)> {
        self.parts
            .split_last()
            .map(|(leaf, dirs)| (dirs, leaf.as_str()))
    }

    /// A key for per-file locks: the path in lower case (Windows names are
    /// case-insensitive).
    pub fn key(&self) -> String {
        self.text().to_lowercase()
    }
}

/// Parse a client's relative path. `\` counts as `/`; repeated separators
/// collapse. Refused (with the reason): NUL or other control characters,
/// absolute, UNC and device paths (`/x`, `\\server\share`, `\\?\C:\`,
/// `\\.\pipe`), drive paths (`C:\x`, drive-relative `C:x`), `.`/`..`,
/// streams (`a:b`), device names (`NUL`, `com1.txt`), trailing dots or
/// spaces, characters Windows does not allow, `.git` anywhere, and paths
/// longer than [`MAX_PATH_BYTES`].
pub(crate) fn parse(input: &str) -> Result<Rel, String> {
    if input.len() > MAX_PATH_BYTES {
        return Err(format!("longer than {MAX_PATH_BYTES} bytes"));
    }
    if input.chars().any(|c| (c as u32) < 0x20) {
        return Err("a control character (NUL, newline...)".into());
    }
    let s = input.replace('\\', "/");
    if s.starts_with('/') {
        return Err("an absolute, UNC or device path".into());
    }
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        return Err("a drive path (absolute or drive-relative)".into());
    }
    let mut parts = Vec::new();
    for p in s.split('/') {
        if p.is_empty() {
            continue;
        }
        if let Some(why) = bad_name(p) {
            return Err(format!("a part ({p:?}) with {why}"));
        }
        if p.eq_ignore_ascii_case(".git") {
            return Err("a `.git` folder or file (git's own data)".into());
        }
        parts.push(p.to_string());
    }
    Ok(Rel { parts })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_relative_paths_parse() {
        for (input, want) in [
            ("", ""),
            ("a.txt", "a.txt"),
            ("src/main.rs", "src/main.rs"),
            ("src\\main.rs", "src/main.rs"),
            ("dir with space//new 1.md", "dir with space/new 1.md"),
            ("한글/파일 이름.txt", "한글/파일 이름.txt"),
            (".gitignore", ".gitignore"),
            (".github/workflows/ci.yml", ".github/workflows/ci.yml"),
            ("a/", "a"),
            ("com10", "com10"),
        ] {
            assert_eq!(parse(input).unwrap().text(), want, "{input:?}");
        }
    }

    /// TM-15: every spelling that leaves the worktree or that Windows
    /// reads specially is refused as text.
    #[test]
    fn escapes_and_special_names_are_refused() {
        for bad in [
            "..",
            "../secret.txt",
            "a/../../secret.txt",
            "a/./b",
            ".",
            "/etc/passwd",
            "\\Windows\\win.ini",
            "C:\\Windows\\win.ini",
            "C:/x",
            "c:secret.txt",
            "\\\\server\\share\\x",
            "//server/share/x",
            "\\\\?\\C:\\x",
            "\\\\.\\pipe\\podd",
            "//./COM1",
            "a.txt:stream",
            "a.txt::$DATA",
            "dir:alt/x",
            "NUL",
            "src/con.rs",
            "lpt1.txt",
            "trailing.",
            "trailing ",
            "a/b./c",
            "a\0b",
            "a\nb",
            "a*b",
            "a?b",
            "a|b",
            "a<b",
            ".git",
            ".GIT/config",
            "sub/.git/hooks/pre-commit",
            &"a/".repeat(3000),
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
    }
}
