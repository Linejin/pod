//! Readers of git's porcelain and `-z` output. Nothing here runs a process.

use crate::protocol::{CommitInfo, DiffFile};

use super::native_path;
use super::worktree::WorktreeEntry;

/// `config -z --get-regexp` output (`key\nvalue\0`…) → driver names with a
/// non-empty program.
pub(super) fn parse_filter_drivers(s: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for entry in s.split('\0').filter(|e| !e.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        if value.trim().is_empty() {
            continue;
        }
        let Some((name, _)) = key
            .strip_prefix("filter.")
            .and_then(|rest| rest.rsplit_once('.'))
        else {
            continue;
        };
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// `config -z --list --show-origin --show-scope` output
/// (`scope\0origin\0key\nvalue\0`…) read in `dir` → (filter drivers with a
/// program, every config file read plus every include target named, read
/// or not). Relative origins are relative to `dir`, relative includes to
/// their file's folder, `~/` to `home`.
pub(super) fn parse_config_list(
    s: &str,
    dir: &std::path::Path,
    home: Option<&std::path::Path>,
) -> (Vec<String>, Vec<std::path::PathBuf>) {
    use std::path::{Path, PathBuf};
    let mut drivers: Vec<String> = Vec::new();
    let mut sources: Vec<PathBuf> = Vec::new();
    let mut add = |p: PathBuf| {
        if !sources.contains(&p) {
            sources.push(p);
        }
    };
    let fields: Vec<&str> = s.split('\0').collect();
    for t in fields.chunks(3) {
        let [_scope, origin, kv] = t else { continue };
        let file = origin.strip_prefix("file:").map(|f| {
            let f = native_path(f);
            if f.is_absolute() { f } else { dir.join(f) }
        });
        if let Some(f) = &file {
            add(f.clone());
        }
        let (key, value) = kv.split_once('\n').unwrap_or((kv, ""));
        let key_l = key.to_ascii_lowercase();
        let include = key_l == "include.path"
            || (key_l.starts_with("includeif.") && key_l.ends_with(".path"));
        if include && !value.trim().is_empty() {
            let v = value.trim();
            let target = if let Some(rest) = v.strip_prefix("~/").or_else(|| v.strip_prefix("~\\"))
            {
                home.map(|h| h.join(native_path(rest)))
            } else {
                let p = native_path(v);
                if p.is_absolute() {
                    Some(p)
                } else {
                    file.as_deref().and_then(Path::parent).map(|d| d.join(p))
                }
            };
            if let Some(t) = target {
                add(t);
            }
        }
        if value.trim().is_empty() {
            continue;
        }
        if let Some((name, field)) = key
            .strip_prefix("filter.")
            .and_then(|rest| rest.rsplit_once('.'))
            && matches!(
                field.to_ascii_lowercase().as_str(),
                "clean" | "smudge" | "process"
            )
            && !drivers.iter().any(|n| n == name)
        {
            drivers.push(name.to_string());
        }
    }
    (drivers, sources)
}

/// Parse `git worktree list --porcelain -z`: attribute lines separated by
/// NUL, records separated by an empty attribute.
pub fn parse_worktree_list(s: &str) -> Vec<WorktreeEntry> {
    let mut out = Vec::new();
    let mut cur: Option<WorktreeEntry> = None;
    for field in s.split('\0') {
        if field.is_empty() {
            if let Some(w) = cur.take() {
                out.push(w);
            }
            continue;
        }
        let (key, value) = field.split_once(' ').unwrap_or((field, ""));
        match key {
            "worktree" => {
                if let Some(w) = cur.take() {
                    out.push(w);
                }
                cur = Some(WorktreeEntry {
                    path: native_path(value),
                    ..Default::default()
                });
            }
            _ => {
                let Some(w) = cur.as_mut() else { continue };
                match key {
                    "HEAD" => w.head = Some(value.to_string()),
                    "branch" => {
                        w.branch = Some(value.strip_prefix("refs/heads/").unwrap_or(value).into())
                    }
                    "bare" => w.bare = true,
                    "detached" => w.detached = true,
                    "prunable" => w.prunable = true,
                    "locked" => w.locked = true,
                    _ => {}
                }
            }
        }
    }
    if let Some(w) = cur {
        out.push(w);
    }
    out
}

pub(super) fn parse_left_right(s: &str) -> Option<(u32, u32)> {
    let mut it = s.split_whitespace();
    let left = it.next()?.parse().ok()?;
    let right = it.next()?.parse().ok()?;
    Some((left, right))
}

/// Entries in `git status --porcelain=v1 -z` (a rename/copy carries an extra
/// NUL-terminated source path that is not an entry of its own).
pub fn count_status_entries(s: &str) -> u32 {
    let mut n = 0;
    let mut fields = s.split('\0').filter(|f| !f.is_empty());
    while let Some(f) = fields.next() {
        n += 1;
        let xy = f.as_bytes();
        if xy.len() >= 2 && (matches!(xy[0], b'R' | b'C') || matches!(xy[1], b'R' | b'C')) {
            fields.next();
        }
    }
    n
}

pub(super) fn parse_commit(s: &str) -> Option<CommitInfo> {
    let mut it = s.trim_end_matches(['\r', '\n']).split('\0');
    let sha = it.next()?.to_string();
    let summary = it.next()?.to_string();
    let author = it.next()?.to_string();
    let secs: u64 = it.next()?.trim().parse().ok()?;
    (!sha.is_empty()).then_some(CommitInfo {
        sha,
        summary,
        author,
        time_ms: secs * 1000,
    })
}

/// `(status, old_path, path)` from `--name-status -z`.
pub fn parse_name_status(s: &str) -> Vec<(String, Option<String>, String)> {
    let mut out = Vec::new();
    let mut it = s.split('\0').filter(|f| !f.is_empty());
    while let Some(code) = it.next() {
        let letter = code.chars().next().unwrap_or('M');
        if matches!(letter, 'R' | 'C') {
            let (Some(old), Some(new)) = (it.next(), it.next()) else {
                break;
            };
            out.push((letter.to_string(), Some(old.to_string()), new.to_string()));
        } else {
            let Some(path) = it.next() else { break };
            out.push((letter.to_string(), None, path.to_string()));
        }
    }
    out
}

/// `(added, deleted, path)` from `--numstat -z`; counts are `None` for binary
/// files. A rename is `a\td\t\0old\0new\0` and keyed by the new path.
pub fn parse_numstat(s: &str) -> Vec<(Option<u64>, Option<u64>, String)> {
    let mut out = Vec::new();
    let mut it = s.split('\0');
    while let Some(rec) = it.next() {
        if rec.is_empty() {
            continue;
        }
        let mut cols = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(path)) = (cols.next(), cols.next(), cols.next()) else {
            continue;
        };
        let path = if path.is_empty() {
            let _old = it.next();
            match it.next() {
                Some(new) => new.to_string(),
                None => break,
            }
        } else {
            path.to_string()
        };
        out.push((a.parse().ok(), d.parse().ok(), path));
    }
    out
}

pub(super) fn merge_diff(
    names: &[(String, Option<String>, String)],
    stats: &[(Option<u64>, Option<u64>, String)],
) -> Vec<DiffFile> {
    names
        .iter()
        .map(|(status, old, path)| {
            let st = stats.iter().find(|(_, _, p)| p == path);
            let (added, deleted) = st.map(|(a, d, _)| (*a, *d)).unwrap_or((Some(0), Some(0)));
            DiffFile {
                path: path.clone(),
                old_path: old.clone(),
                status: status.clone(),
                binary: added.is_none() && deleted.is_none(),
                added,
                deleted,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn worktree_list_porcelain() {
        let s = "worktree C:/src/app\0HEAD 1111\0branch refs/heads/main\0\0\
                 worktree C:/Users/me/pod/worktrees/app/feat x\0HEAD 2222\0branch refs/heads/pod/feat-x\0\0\
                 worktree C:/tmp/gone\0HEAD 3333\0detached\0prunable gitdir file points to non-existent location\0\0";
        let w = parse_worktree_list(s);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].path, PathBuf::from(r"C:\src\app"));
        assert_eq!(w[0].branch.as_deref(), Some("main"));
        assert_eq!(
            w[1].path,
            PathBuf::from(r"C:\Users\me\pod\worktrees\app\feat x")
        );
        assert_eq!(w[1].branch.as_deref(), Some("pod/feat-x"));
        assert!(w[2].detached && w[2].prunable && w[2].branch.is_none());
    }

    #[test]
    fn numstat_and_name_status() {
        let numstat =
            "3\t0\tnew.txt\x001\t2\tREADME.md\0-\t-\timg.png\x000\t0\t\0old name.rs\0new name.rs\0";
        let n = parse_numstat(numstat);
        assert_eq!(
            n,
            vec![
                (Some(3), Some(0), "new.txt".into()),
                (Some(1), Some(2), "README.md".into()),
                (None, None, "img.png".into()),
                (Some(0), Some(0), "new name.rs".into()),
            ]
        );
        let names = "A\0new.txt\0M\0README.md\0M\0img.png\0R100\0old name.rs\0new name.rs\0";
        let s = parse_name_status(names);
        assert_eq!(
            s[3],
            ("R".into(), Some("old name.rs".into()), "new name.rs".into())
        );
        let files = merge_diff(&s, &n);
        assert_eq!(files[0].added, Some(3));
        assert_eq!(files[1].deleted, Some(2));
        assert!(files[2].binary && files[2].added.is_none());
        assert_eq!(files[3].old_path.as_deref(), Some("old name.rs"));
    }

    #[test]
    fn status_entries_skip_rename_sources() {
        assert_eq!(count_status_entries(""), 0);
        assert_eq!(
            count_status_entries(" M a.txt\0R  new.txt\0old.txt\0?? b c.txt\0"),
            3
        );
    }

    #[test]
    fn misc_parsers() {
        assert_eq!(parse_left_right("2\t5\n"), Some((2, 5)));
        let c = parse_commit("abc\0fix: thing\0Pod Test\x001700000000\n").unwrap();
        assert_eq!(c.summary, "fix: thing");
        assert_eq!(c.time_ms, 1_700_000_000_000);
    }

    /// TM-21: every config file and include target (missing ones too), and
    /// the drivers with a program.
    #[test]
    fn config_list_sources_and_drivers() {
        use std::path::{Path, PathBuf};
        let s = "system\0file:C:/Git/etc/gitconfig\0filter.lfs.clean\ngit-lfs clean -- %f\0\
                 local\0file:.git/config\0include.path\n../wt/evil.cfg\0\
                 local\0file:.git/config\0includeif.gitdir:C:/x/.path\n~/inc.cfg\0\
                 local\0file:.git/config\0includeIf.onbranch:main.path\nD:/abs.cfg\0\
                 local\0file:C:/r/wt/evil.cfg\0filter.Evil.process\nsh e.sh\0\
                 local\0file:C:/r/wt/evil.cfg\0filter.off.smudge\n\0\
                 command\0command line:\0filter.cli.clean\nx\0";
        let (drivers, sources) =
            parse_config_list(s, Path::new(r"C:\r"), Some(Path::new(r"C:\home")));
        assert_eq!(drivers, ["lfs", "Evil", "cli"]);
        let want: Vec<PathBuf> = [
            r"C:\Git\etc\gitconfig",
            r"C:\r\.git\config",
            r"C:\r\.git\..\wt\evil.cfg",
            r"C:\home\inc.cfg",
            r"D:\abs.cfg",
            r"C:\r\wt\evil.cfg",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(sources, want);
    }
}
