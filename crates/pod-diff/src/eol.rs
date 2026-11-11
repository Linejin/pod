//! Git's "clean" line-ending conversion for the worktree side of a diff.
//!
//! The old side of a review comes from a blob (what git stores); the new side
//! is read from the worktree (what git checked out). With `core.autocrlf=true`
//! or `text`/`eol` attributes those differ only by CRLF vs LF, and comparing
//! them raw marks every line as changed. `git diff` first converts the
//! worktree file the way `git add` would ("clean"); this module does the same
//! for the CRLF part (filters, `ident` and `working-tree-encoding` are not
//! applied), so the review shows what `git diff` shows.
//!
//! The rules follow git's documented behavior (gitattributes(5) "text",
//! "eol"; git-config(1) `core.autocrlf`):
//! - `-text` (or `-crlf`): never converted.
//! - `text` / `eol=lf|crlf` / `crlf` / `crlf=input`: CRLF -> LF.
//! - `text=auto`, or no attribute with `core.autocrlf` true/input: CRLF -> LF
//!   unless the content looks binary, or the index version already has CRLF
//!   (git's "safer autocrlf": a file committed with CRLF stays CRLF).
//! - no attribute and `core.autocrlf` false (the default): not converted.

use std::path::Path;

use anyhow::Result;

use crate::model::git::git_command;

/// How the clean conversion treats one path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Never converted.
    Binary,
    /// Always CRLF -> LF.
    Text,
    /// CRLF -> LF unless binary-looking or the index version has CRLF.
    Auto,
}

/// `core.autocrlf`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AutoCrlf {
    #[default]
    False,
    True,
    Input,
}

impl AutoCrlf {
    /// Value of `git config core.autocrlf` (`None` = unset).
    pub fn parse(v: Option<&str>) -> Self {
        match v.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("input") => Self::Input,
            Some("true" | "yes" | "on" | "1") => Self::True,
            _ => Self::False,
        }
    }
}

/// The action for a path from its `text`, `crlf` and `eol` attribute values as
/// `git check-attr` prints them (`set`, `unset`, `unspecified` or a value).
pub fn action(text: &str, crlf: &str, eol: &str, autocrlf: AutoCrlf) -> Action {
    #[derive(PartialEq)]
    enum A {
        Undefined,
        Binary,
        Text,
        Auto,
    }
    let check = |v: &str| match v {
        "set" => A::Text,
        "unset" => A::Binary,
        "auto" => A::Auto,
        "input" => A::Text,
        _ => A::Undefined,
    };
    let mut a = check(text);
    if a == A::Undefined {
        a = check(crlf);
    }
    // Setting `eol` implies text (unless the path is explicitly binary).
    if a == A::Undefined && matches!(eol, "lf" | "crlf") {
        a = A::Text;
    }
    match a {
        A::Binary => Action::Binary,
        A::Text => Action::Text,
        A::Auto => Action::Auto,
        A::Undefined => match autocrlf {
            AutoCrlf::False => Action::Binary,
            AutoCrlf::True | AutoCrlf::Input => Action::Auto,
        },
    }
}

#[derive(Default)]
struct Stats {
    nul: usize,
    lonecr: usize,
    crlf: usize,
    printable: usize,
    nonprintable: usize,
}

fn stats(b: &[u8]) -> Stats {
    let mut s = Stats::default();
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'\r' => {
                if b.get(i + 1) == Some(&b'\n') {
                    s.crlf += 1;
                } else {
                    s.lonecr += 1;
                }
            }
            b'\n' => {}
            127 => s.nonprintable += 1,
            0 => {
                s.nul += 1;
                s.nonprintable += 1;
            }
            8 | b'\t' | 0x1b | 0x0c => s.printable += 1,
            c if c < 32 => s.nonprintable += 1,
            _ => s.printable += 1,
        }
    }
    // A DOS end-of-file marker is not counted against the text.
    if b.last() == Some(&0x1a) {
        s.nonprintable = s.nonprintable.saturating_sub(1);
    }
    s
}

fn looks_binary(s: &Stats) -> bool {
    s.lonecr > 0 || s.nul > 0 || (s.printable >> 7) < s.nonprintable
}

/// Text that git's auto conversion keeps as CRLF (the "safer autocrlf" check
/// on the index version).
pub fn has_text_crlf(b: &[u8]) -> bool {
    b.contains(&b'\r') && {
        let s = stats(b);
        !looks_binary(&s) && s.crlf > 0
    }
}

/// The worktree content as `git add` would store it, as far as line endings
/// go. `index` is the index version of the path, if any.
pub fn clean(text: &str, action: Action, index: Option<&[u8]>) -> Option<String> {
    let b = text.as_bytes();
    if action == Action::Binary || !b.contains(&b'\r') {
        return None;
    }
    let s = stats(b);
    if s.crlf == 0 {
        return None;
    }
    if action == Action::Auto && (looks_binary(&s) || index.is_some_and(has_text_crlf)) {
        return None;
    }
    Some(text.replace("\r\n", "\n"))
}

/// `(text, crlf, eol)` attribute values of each path, via one
/// `git check-attr -z --stdin`.
pub fn attributes(repo: &Path, paths: &[&str]) -> Result<Vec<(String, String, String)>> {
    use std::io::Write as _;
    use std::process::Stdio;
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = git_command(repo)
        .args(["check-attr", "-z", "--stdin", "text", "crlf", "eol"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input: Vec<u8> = paths.iter().flat_map(|p| p.bytes().chain([0])).collect();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output()?;
    let _ = writer.join();
    if !out.status.success() {
        anyhow::bail!("git check-attr failed");
    }
    // `<path>\0<attr>\0<value>\0`, three attributes per path, in input order.
    let mut fields = out.stdout.split(|b| *b == 0);
    let mut result = Vec::with_capacity(paths.len());
    for _ in paths {
        let mut vals = [String::new(), String::new(), String::new()];
        for _ in 0..3 {
            let (Some(_path), Some(attr), Some(value)) =
                (fields.next(), fields.next(), fields.next())
            else {
                anyhow::bail!("git check-attr: short output");
            };
            let value = String::from_utf8_lossy(value).into_owned();
            match attr {
                b"text" => vals[0] = value,
                b"crlf" => vals[1] = value,
                b"eol" => vals[2] = value,
                _ => {}
            }
        }
        result.push((vals[0].clone(), vals[1].clone(), vals[2].clone()));
    }
    Ok(result)
}

/// `core.autocrlf` of the repository.
pub fn autocrlf(repo: &Path) -> AutoCrlf {
    let out = git_command(repo)
        .args(["config", "--get", "core.autocrlf"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => AutoCrlf::parse(Some(&String::from_utf8_lossy(&o.stdout))),
        _ => AutoCrlf::False,
    }
}

/// The two texts differ, and only in line endings (a whole-file conversion).
pub fn eol_only_change(old: &str, new: &str) -> bool {
    old != new && old.replace("\r\n", "\n") == new.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions() {
        let u = "unspecified";
        assert_eq!(action(u, u, u, AutoCrlf::False), Action::Binary);
        assert_eq!(action(u, u, u, AutoCrlf::True), Action::Auto);
        assert_eq!(action(u, u, u, AutoCrlf::Input), Action::Auto);
        assert_eq!(action("set", u, u, AutoCrlf::False), Action::Text);
        assert_eq!(action("unset", u, u, AutoCrlf::True), Action::Binary);
        assert_eq!(action("auto", u, u, AutoCrlf::False), Action::Auto);
        assert_eq!(action("auto", u, "lf", AutoCrlf::False), Action::Auto);
        assert_eq!(action(u, u, "crlf", AutoCrlf::False), Action::Text);
        assert_eq!(action("unset", u, "lf", AutoCrlf::True), Action::Binary);
        assert_eq!(action(u, "input", u, AutoCrlf::False), Action::Text);
        assert_eq!(action(u, "unset", u, AutoCrlf::True), Action::Binary);
        assert_eq!(AutoCrlf::parse(Some("TRUE\n")), AutoCrlf::True);
        assert_eq!(AutoCrlf::parse(Some("input")), AutoCrlf::Input);
        assert_eq!(AutoCrlf::parse(None), AutoCrlf::False);
    }

    #[test]
    fn clean_rules() {
        let crlf = "a\r\nb\r\n";
        assert_eq!(clean(crlf, Action::Text, None).as_deref(), Some("a\nb\n"));
        assert_eq!(clean(crlf, Action::Binary, None), None);
        assert_eq!(clean(crlf, Action::Auto, None).as_deref(), Some("a\nb\n"));
        assert_eq!(
            clean(crlf, Action::Auto, Some(b"a\nb\n")).as_deref(),
            Some("a\nb\n")
        );
        // Safer autocrlf: committed with CRLF stays CRLF.
        assert_eq!(clean(crlf, Action::Auto, Some(b"a\r\nb\r\n")), None);
        // ...but `text` always normalizes.
        assert_eq!(
            clean(crlf, Action::Text, Some(b"a\r\nb\r\n")).as_deref(),
            Some("a\nb\n")
        );
        // Mixed endings: every CRLF goes, lone LFs stay.
        assert_eq!(
            clean("a\r\nb\nc\r\n", Action::Auto, None).as_deref(),
            Some("a\nb\nc\n")
        );
        // A lone CR makes auto treat it as binary.
        assert_eq!(clean("a\rb\r\n", Action::Auto, None), None);
        assert_eq!(
            clean("a\rb\r\n", Action::Text, None).as_deref(),
            Some("a\rb\n")
        );
        assert_eq!(clean("a\nb\n", Action::Text, None), None);
    }

    #[test]
    fn eol_only() {
        assert!(eol_only_change("a\nb\n", "a\r\nb\r\n"));
        assert!(!eol_only_change("a\nb\n", "a\r\nB\r\n"));
        assert!(!eol_only_change("a\n", "a\n"));
    }
}
