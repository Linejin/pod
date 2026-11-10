//! `file:` URI <-> filesystem path conversion, Windows first.
//!
//! Language servers disagree on drive-letter spelling (`file:///C:/x`,
//! `file:///c%3A/x`, `file:///c:/x`), so every URI coming back from a server
//! goes through [`uri_to_path`] and paths are compared with [`path_key`].

use std::path::{Component, Path, PathBuf, Prefix};
use std::str::FromStr;

/// Characters left unescaped in a path segment (RFC 3986 unreserved plus
/// the sub-delims servers commonly leave alone). Everything else is
/// percent-encoded, including spaces, `#`, `%`, `?` and non-ASCII bytes.
fn keep(b: u8) -> bool {
    b.is_ascii_alphanumeric()
        || matches!(
            b,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b'@'
        )
}

fn encode_segment(out: &mut String, seg: &str) {
    for &b in seg.as_bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

/// Strips the `\\?\` verbatim prefix `canonicalize` adds on Windows.
pub fn simplify(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

/// Builds the URI string for an absolute path.
///
/// `C:\a b\x.rs` -> `file:///C:/a%20b/x.rs`, `\\srv\share\x` -> `file://srv/share/x`.
pub fn path_to_uri_string(path: &Path) -> String {
    let path = simplify(path);
    let mut out = String::from("file://");
    let mut segments: Vec<String> = Vec::new();
    for comp in path.components() {
        match comp {
            Component::Prefix(p) => match p.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => {
                    out.push('/');
                    out.push((d as char).to_ascii_uppercase());
                    out.push(':');
                }
                Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                    out.push_str(&server.to_string_lossy());
                    out.push('/');
                    encode_segment(&mut out, &share.to_string_lossy());
                }
                _ => {}
            },
            Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => segments.push("..".into()),
            Component::Normal(s) => segments.push(s.to_string_lossy().into_owned()),
        }
    }
    if segments.is_empty() {
        out.push('/');
    }
    for seg in segments {
        out.push('/');
        encode_segment(&mut out, &seg);
    }
    out
}

pub fn path_to_uri(path: &Path) -> lsp_types::Uri {
    lsp_types::Uri::from_str(&path_to_uri_string(path)).expect("encoded file URI is valid")
}

fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Parses a `file:` URI string into a native path. `None` for other schemes.
pub fn uri_str_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .strip_prefix("file://")
        .or_else(|| uri.strip_prefix("FILE://"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (host, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let path = decode(path)?;
    if cfg!(windows) {
        let host = decode(host)?;
        if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
            return Some(PathBuf::from(format!(
                r"\\{host}{}",
                path.replace('/', "\\")
            )));
        }
        // "/c:/x" -> "C:\x"
        let p = path.strip_prefix('/').unwrap_or(&path);
        let b = p.as_bytes();
        if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
            let drive = (b[0] as char).to_ascii_uppercase();
            let tail = p[2..].replace('/', "\\");
            let tail = if tail.is_empty() {
                "\\".to_string()
            } else {
                tail
            };
            return Some(PathBuf::from(format!("{drive}:{tail}")));
        }
        Some(PathBuf::from(path.replace('/', "\\")))
    } else {
        Some(PathBuf::from(path))
    }
}

pub fn uri_to_path(uri: &lsp_types::Uri) -> Option<PathBuf> {
    uri_str_to_path(uri.as_str())
}

/// `path` relative to `root` with `/` separators, for display and saved
/// sessions. A plain prefix match comes first; when it fails (Windows 8.3
/// short names such as `RUNNER~1`, a different letter case, `\\?\` forms)
/// both sides are resolved with `canonicalize` and compared again. A path
/// outside `root` is returned whole.
pub fn rel_path(root: &Path, path: &Path) -> String {
    let rel = match path.strip_prefix(root) {
        Ok(r) => r.to_path_buf(),
        Err(_) => match (std::fs::canonicalize(root), std::fs::canonicalize(path)) {
            (Ok(r), Ok(p)) => p
                .strip_prefix(&r)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.to_path_buf()),
            _ => path.to_path_buf(),
        },
    };
    let s = rel.to_string_lossy();
    if cfg!(windows) {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

/// `path` spelled like `root` (a canonical, [`simplify`]d root): a file under
/// the root opened through another spelling (8.3 short name, letter case)
/// maps to the same buffer, tab key and relative path. A file that no
/// longer exists maps through its deepest existing folder. Other paths stay
/// as given.
pub fn under_root_spelling(root: &Path, path: &Path) -> PathBuf {
    if path.starts_with(root) {
        return path.to_path_buf();
    }
    let mut base = path;
    let mut tail = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(base) {
            let mut c = simplify(&c);
            c.extend(tail.iter().rev());
            return if c.starts_with(root) {
                c
            } else {
                path.to_path_buf()
            };
        }
        match (base.parent(), base.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                base = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Comparison key: on Windows paths are case-insensitive and may use either
/// separator, so lowercase and normalise separators.
pub fn path_key(path: &Path) -> String {
    let s = simplify(path).to_string_lossy().into_owned();
    if cfg!(windows) {
        s.replace('/', "\\").to_lowercase()
    } else {
        s
    }
}

/// Key for a URI, equal for every spelling of the same file.
pub fn uri_key(uri: &lsp_types::Uri) -> String {
    match uri_to_path(uri) {
        Some(p) => path_key(&p),
        None => uri.as_str().to_string(),
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn files_opened_through_another_spelling_map_under_the_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Wt")).unwrap();
        std::fs::write(dir.path().join("Wt").join("a.txt"), "a").unwrap();
        let root = simplify(&std::fs::canonicalize(dir.path().join("Wt")).unwrap());
        // Other letter case (and, on CI, 8.3 short names) for the same file.
        let other = dir.path().join("WT").join("a.txt");
        assert_eq!(under_root_spelling(&root, &other), root.join("a.txt"));
        // A file that is gone maps through its deepest existing folder.
        let gone = dir.path().join("WT").join("gone").join("b.txt");
        assert_eq!(
            under_root_spelling(&root, &gone),
            root.join("gone").join("b.txt")
        );
        let outside = dir.path().join("x.txt");
        assert_eq!(under_root_spelling(&root, &outside), outside);
    }

    #[test]
    fn rel_path_survives_a_different_spelling_of_the_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("Wt");
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub").join("beta.txt"), "b").unwrap();
        // Same folder, other letter case: a plain prefix match fails.
        let other = dir.path().join("WT").join("sub").join("beta.txt");
        assert_eq!(rel_path(&root, &other), "sub/beta.txt");
        assert_eq!(rel_path(&root, &root.join("a.txt")), "a.txt");
        let outside = dir.path().join("x.txt");
        assert_eq!(
            rel_path(&root, &outside),
            outside.to_string_lossy().replace('\\', "/")
        );
    }

    #[test]
    fn drive_paths_round_trip() {
        let p = Path::new(r"C:\Users\me\a b\한글#1.rs");
        let u = path_to_uri_string(p);
        assert_eq!(u, "file:///C:/Users/me/a%20b/%ED%95%9C%EA%B8%80%231.rs");
        assert_eq!(uri_str_to_path(&u).unwrap(), p);
        // The typed wrapper accepts what we produce.
        let typed = path_to_uri(p);
        assert_eq!(uri_to_path(&typed).unwrap(), p);
    }

    #[test]
    fn server_spellings_of_drive_letters_agree() {
        let want = PathBuf::from(r"C:\w\src\main.rs");
        for s in [
            "file:///c:/w/src/main.rs",
            "file:///C:/w/src/main.rs",
            "file:///c%3A/w/src/main.rs",
            "file:///C%3a/w/src/main.rs",
        ] {
            assert_eq!(uri_str_to_path(s).unwrap(), want, "{s}");
        }
        let a = path_to_uri(&want);
        let b = lsp_types::Uri::from_str("file:///c%3A/w/src/main.rs").unwrap();
        assert_eq!(uri_key(&a), uri_key(&b));
        assert_eq!(path_key(Path::new("c:/W/src/MAIN.rs")), path_key(&want));
    }

    #[test]
    fn unc_and_verbatim_prefixes() {
        let p = Path::new(r"\\server\share\dir\x.txt");
        let u = path_to_uri_string(p);
        assert_eq!(u, "file://server/share/dir/x.txt");
        assert_eq!(uri_str_to_path(&u).unwrap(), p);
        assert_eq!(
            path_to_uri_string(Path::new(r"\\?\C:\x\y.rs")),
            "file:///C:/x/y.rs"
        );
        assert_eq!(
            simplify(Path::new(r"\\?\UNC\srv\s\f")),
            PathBuf::from(r"\\srv\s\f")
        );
    }

    #[test]
    fn non_file_schemes_are_rejected() {
        assert!(uri_str_to_path("https://example.com/x").is_none());
        assert!(uri_str_to_path("untitled:Untitled-1").is_none());
    }
}
