//! The `git` processes of the diff input: the command, one-shot runs and one
//! `git cat-file --batch` for many blobs. [`crate::eol`] uses the command
//! from here, so it does not depend on the model façade.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

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

pub(super) fn git(repo: &Path, args: &[&str]) -> Result<Vec<u8>> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
