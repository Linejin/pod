//! The git side of `review.diff` / `review.blob` (docs/22 TM-16): the
//! changed files come from `git::diff` (`--no-ext-diff --no-textconv`,
//! every filter program blanked, fsmonitor off); attributes from
//! `check-attr`, the old side from `cat-file`, which run no program. The
//! worktree side is read like `fs.read` (held, no links followed).

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use super::journal::hex;
use crate::git;
use crate::protocol::{EolAttrs, ReviewFile};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// git for a read of a task's tree: fixed options, no prompt, no console,
/// no inherited variable pointing git elsewhere (as `git::command`).
fn git_cmd(dir: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    let exe = std::env::var_os(git::ENV_GIT).unwrap_or_else(|| "git".into());
    let mut cmd = Command::new(exe);
    cmd.arg("-C")
        .arg(dir)
        .args(["-c", "core.longpaths=true", "-c", "core.quotepath=false"])
        .args(["-c", "core.fsmonitor=false"])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    pod_process::git::clear_overrides(&mut cmd);
    cmd
}

pub(crate) struct Listing {
    pub merge_base: String,
    pub head: String,
    pub autocrlf: Option<String>,
    pub files: Vec<ReviewFile>,
}

/// The changed files of the worktree `root` against its merge base with
/// `base`. `read` gives a worktree file's bytes (`None`: deleted, a link,
/// too large, unreadable) the way `fs.read` reads it.
pub(crate) fn list(
    root: &Path,
    base: &str,
    base_commit: &str,
    read: impl Fn(&str) -> Option<Vec<u8>>,
) -> Result<Listing, String> {
    let (merge_base, changed) = git::diff(root, base, base_commit)?;
    let head = git::resolve_commit(root, "HEAD").unwrap_or_default();
    let autocrlf = git::output(root, &["config", "--get", "core.autocrlf"])
        .ok()
        .filter(|o| o.ok)
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty());
    let paths: Vec<&str> = changed.iter().map(|f| f.path.as_str()).collect();
    let attrs = check_attr(root, &paths)?;
    let mut files = Vec::with_capacity(changed.len());
    for f in changed {
        let bytes = if f.status == "D" { None } else { read(&f.path) };
        let (mut added, mut binary) = (f.added, f.binary);
        if f.status == "?" {
            // Counted here from the held read, never through a link.
            match &bytes {
                Some(b) if b.contains(&0) => (added, binary) = (None, true),
                Some(b) => {
                    let n = b.iter().filter(|c| **c == b'\n').count()
                        + usize::from(!b.is_empty() && !b.ends_with(b"\n"));
                    (added, binary) = (Some(n as u64), false);
                }
                None => added = None,
            }
        }
        files.push(ReviewFile {
            attrs: attrs.get(&f.path).cloned().unwrap_or_default(),
            revision: bytes.as_deref().map(revision),
            size: bytes.as_ref().map(|b| b.len() as u64),
            path: f.path,
            old_path: f.old_path,
            status: f.status,
            binary,
            added,
            deleted: f.deleted,
        });
    }
    Ok(Listing {
        merge_base,
        head,
        autocrlf,
        files,
    })
}

/// SHA-256 of `bytes`, lowercase hex: a file's revision.
pub(crate) fn revision(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

/// The snapshot id of a listing: merge base, HEAD and every file's path,
/// status and revision.
pub(crate) fn snapshot(l: &Listing) -> String {
    let mut h = Sha256::new();
    for s in [&l.merge_base, &l.head] {
        h.update(s.as_bytes());
        h.update([0]);
    }
    for f in &l.files {
        for s in [
            f.path.as_str(),
            f.old_path.as_deref().unwrap_or(""),
            f.status.as_str(),
            f.revision.as_deref().unwrap_or("-"),
        ] {
            h.update(s.as_bytes());
            h.update([0]);
        }
    }
    hex(&h.finalize()[..16])
}

/// `git check-attr -z text crlf eol` of `paths` (in batches; runs no
/// program).
fn check_attr(
    root: &Path,
    paths: &[&str],
) -> Result<std::collections::HashMap<String, EolAttrs>, String> {
    let mut out = std::collections::HashMap::new();
    let mut i = 0;
    while i < paths.len() {
        let mut args = vec!["check-attr", "-z", "text", "crlf", "eol", "--"];
        let mut len = 0;
        while i < paths.len() && (len == 0 || len + paths[i].len() < 16 * 1024) {
            len += paths[i].len() + 1;
            args.push(paths[i]);
            i += 1;
        }
        let text = git::run(root, &args)?;
        let mut parts = text.split('\0');
        while let (Some(p), Some(attr), Some(value)) = (parts.next(), parts.next(), parts.next()) {
            let e: &mut EolAttrs = out.entry(p.to_string()).or_default();
            match attr {
                "text" => e.text = value.into(),
                "crlf" => e.crlf = value.into(),
                "eol" => e.eol = value.into(),
                _ => {}
            }
        }
    }
    Ok(out)
}

/// What [`blob`] found.
pub(crate) enum Blob {
    Missing,
    TooLarge(u64),
    Bytes(Vec<u8>),
}

/// The blob `spec` (`<commit>:<path>` or `:<path>`) of the repository of
/// `root`, raw: `cat-file` applies no filter or textconv.
pub(crate) fn blob(root: &Path, spec: &str, limit: u64) -> Result<Blob, String> {
    let mut child = git_cmd(root)
        .args(["cat-file", "--batch-check"])
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run git: {e}"))?;
    let mut stdin = child.stdin.take().expect("piped");
    stdin
        .write_all(format!("{spec}\n").as_bytes())
        .map_err(|e| format!("git cat-file: {e}"))?;
    drop(stdin);
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git cat-file: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git cat-file --batch-check: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let mut f = line.split_whitespace();
    let (Some(id), Some(kind), Some(size)) = (f.next(), f.next(), f.next()) else {
        return Ok(Blob::Missing);
    };
    if kind != "blob" || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(Blob::Missing);
    }
    let size: u64 = size.parse().map_err(|_| format!("git cat-file: {line}"))?;
    if size > limit {
        return Ok(Blob::TooLarge(size));
    }
    let out = git_cmd(root)
        .args(["cat-file", "blob", id])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git cat-file blob: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(Blob::Bytes(out.stdout))
}
