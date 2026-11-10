//! Session cwd -> registered repository / task worktree (docs/18 §4).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Comparable form of a Windows path: `\\?\` prefix removed, `/` -> `\`,
/// lower case, no trailing separator (except a drive root `c:\`).
pub fn norm_path(p: &str) -> String {
    let p = p.trim();
    let p = match p.strip_prefix(r"\\?\UNC\") {
        Some(r) => format!(r"\\{r}"),
        None => p.strip_prefix(r"\\?\").unwrap_or(p).to_string(),
    };
    let mut s: String = p.replace('/', "\\").to_lowercase();
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    if s.len() == 2 && s.ends_with(':') {
        s.push('\\');
    }
    s
}

/// Is `child` equal to `parent` or inside it (on a path-component boundary)?
pub fn is_within(child: &str, parent: &str) -> bool {
    let (c, p) = (norm_path(child), norm_path(parent));
    if p.is_empty() {
        return false;
    }
    c == p
        || (c.starts_with(&p) && (p.ends_with('\\') || c.as_bytes().get(p.len()) == Some(&b'\\')))
}

/// What a session's cwd belongs to.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CwdMatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
}

impl CwdMatch {
    pub fn is_empty(&self) -> bool {
        self.repo.is_none() && self.task.is_none()
    }
}

pub struct RepoPath<'a> {
    pub name: &'a str,
    pub path: &'a str,
}

pub struct TaskPath<'a> {
    pub id: &'a str,
    pub repo: &'a str,
    pub worktree: &'a str,
}

/// The task whose worktree holds `cwd` (deepest wins) with its repository,
/// else the repository whose checkout holds it (deepest wins).
pub fn match_cwd(cwd: &str, repos: &[RepoPath], tasks: &[TaskPath]) -> CwdMatch {
    if cwd.trim().is_empty() {
        return CwdMatch::default();
    }
    if let Some(t) = tasks
        .iter()
        .filter(|t| is_within(cwd, t.worktree))
        .max_by_key(|t| norm_path(t.worktree).len())
    {
        return CwdMatch {
            repo: Some(t.repo.to_string()),
            task: Some(t.id.to_string()),
        };
    }
    let repo = repos
        .iter()
        .filter(|r| is_within(cwd, r.path))
        .max_by_key(|r| norm_path(r.path).len());
    CwdMatch {
        repo: repo.map(|r| r.name.to_string()),
        task: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(norm_path(r"C:/Users/Me/Repo/"), r"c:\users\me\repo");
        assert_eq!(norm_path(r"\\?\C:\Repo"), r"c:\repo");
        assert_eq!(norm_path(r"\\?\UNC\srv\share\x"), r"\\srv\share\x");
        assert_eq!(norm_path("C:"), r"c:\");
        assert_eq!(norm_path(r"C:\"), r"c:\");
    }

    #[test]
    fn within_respects_component_boundaries() {
        assert!(is_within(r"C:\src\pod", r"c:\SRC\Pod\"));
        assert!(is_within(r"C:\src\pod\crates\x", r"C:\src\pod"));
        assert!(!is_within(r"C:\src\pod-old", r"C:\src\pod"));
        assert!(is_within(r"C:\anything", r"C:\"));
        assert!(!is_within(r"C:\x", ""));
    }

    #[test]
    fn task_beats_repo_and_deepest_wins() {
        let repos = [
            RepoPath {
                name: "pod",
                path: r"C:\src\pod",
            },
            RepoPath {
                name: "inner",
                path: r"C:\src\pod\vendor\inner",
            },
        ];
        let tasks = [TaskPath {
            id: "pod/feat",
            repo: "pod",
            worktree: r"C:\Users\me\pod\worktrees\pod\feat",
        }];
        assert_eq!(
            match_cwd(r"c:\users\me\pod\worktrees\pod\feat\crates", &repos, &tasks),
            CwdMatch {
                repo: Some("pod".into()),
                task: Some("pod/feat".into())
            }
        );
        assert_eq!(
            match_cwd(r"C:\src\pod\vendor\inner\x", &repos, &tasks)
                .repo
                .as_deref(),
            Some("inner")
        );
        assert_eq!(
            match_cwd(r"C:\src\pod", &repos, &tasks).repo.as_deref(),
            Some("pod")
        );
        assert!(match_cwd(r"D:\elsewhere", &repos, &tasks).is_empty());
        assert!(match_cwd("", &repos, &tasks).is_empty());
    }
}
