//! Which GitHub repository a worktree pushes to, read from its git remotes
//! (`git remote -v`). Purely local: no network, nothing created.

use std::path::Path;
use std::time::Duration;

use crate::error::{ForgeError, ForgeErrorKind};
use crate::runner::{Invocation, Runner, run_ok};
use crate::{ForgeRemote, RepoSlug};

/// Environment override: extra GitHub Enterprise host names (comma separated).
pub const ENV_HOSTS: &str = "POD_GITHUB_HOSTS";

/// `github.com`, `*.ghe.com`, `GH_HOST` and [`ENV_HOSTS`] are GitHub.
pub fn is_github_host(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    if h == "github.com" || h == "www.github.com" || h.ends_with(".ghe.com") {
        return true;
    }
    let listed = |var: &str| {
        std::env::var(var)
            .ok()
            .is_some_and(|v| v.split(',').any(|x| x.trim().eq_ignore_ascii_case(&h)))
    };
    listed("GH_HOST") || listed(ENV_HOSTS)
}

/// Host and `owner/name` of a remote URL: `https://host/o/r(.git)`,
/// `ssh://git@host[:port]/o/r.git`, `git@host:o/r.git`. `None` for local
/// paths and other shapes.
pub fn parse_url(url: &str) -> Option<RepoSlug> {
    let u = url.trim();
    let (host, path) = if let Some(rest) = u
        .strip_prefix("https://")
        .or_else(|| u.strip_prefix("http://"))
        .or_else(|| u.strip_prefix("ssh://"))
        .or_else(|| u.strip_prefix("git://"))
    {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?;
        (host, path)
    } else if let Some((left, path)) = u.split_once(':') {
        // scp-like `user@host:o/r.git`; a drive letter (`C:\x`) is a path.
        if left.len() <= 1 || left.contains(['/', '\\']) || path.starts_with('\\') {
            return None;
        }
        (left.rsplit('@').next()?, path)
    } else {
        return None;
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    if parts.next().is_some() || owner.is_empty() || name.is_empty() || host.is_empty() {
        return None;
    }
    Some(RepoSlug {
        host: host.to_ascii_lowercase(),
        owner: owner.into(),
        name: name.into(),
    })
}

/// `url` with the credentials of its user info (`https://user:token@host`,
/// `https://token@host`) replaced by `***`, so they never reach task state,
/// `--json` output or error text. An ssh user name alone (`ssh://git@host`,
/// `git@host:o/r`) is not a secret and stays.
pub fn redact_url(url: &str) -> String {
    redact_urls_in(url)
}

/// [`redact_url`] for every `scheme://` URL inside `text` (tool messages).
pub fn redact_urls_in(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("://") {
        let scheme_start = rest[..i]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')))
            .map_or(0, |j| j + 1);
        let scheme = rest[scheme_start..i].to_ascii_lowercase();
        let after = &rest[i + 3..];
        let auth_len = after
            .find(|c: char| {
                c == '/'
                    || c == '?'
                    || c == '#'
                    || c.is_whitespace()
                    || matches!(c, '\'' | '"' | '<' | '>')
            })
            .unwrap_or(after.len());
        out.push_str(&rest[..i + 3]);
        let authority = &after[..auth_len];
        match authority.rfind('@') {
            Some(at) if !(scheme == "ssh" && !authority[..at].contains(':')) => {
                out.push_str("***");
                out.push_str(&authority[at..]);
            }
            _ => out.push_str(authority),
        }
        rest = &after[auth_len..];
    }
    out.push_str(rest);
    out
}

/// `(name, fetch url)` pairs of `git remote -v` output, in order.
pub fn parse_remote_v(out: &str) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let mut it = line.split_whitespace();
        if let (Some(name), Some(url), kind) = (it.next(), it.next(), it.next())
            && kind != Some("(push)")
            && !v.iter().any(|(n, _)| n == name)
        {
            v.push((name.to_string(), url.to_string()));
        }
    }
    v
}

/// Choose the remote: `origin` if it is a GitHub repository, else the first
/// that is. Errors: no remotes → `NoRemote`; remotes but none on GitHub →
/// `UnsupportedHost`.
pub fn choose(remotes: &[(String, String)]) -> Result<ForgeRemote, ForgeError> {
    if remotes.is_empty() {
        return Err(ForgeError::no_remote("this repository has no git remote"));
    }
    let github: Vec<ForgeRemote> = remotes
        .iter()
        .filter_map(|(name, url)| {
            parse_url(url)
                .filter(|s| is_github_host(&s.host))
                .map(|slug| ForgeRemote {
                    name: name.clone(),
                    url: redact_url(url),
                    slug,
                })
        })
        .collect();
    if let Some(r) = github
        .iter()
        .find(|r| r.name == "origin")
        .or_else(|| github.first())
    {
        return Ok(r.clone());
    }
    let names: Vec<String> = remotes
        .iter()
        .map(|(n, u)| format!("{n} ({})", redact_url(u)))
        .collect();
    Err(ForgeError::new(
        ForgeErrorKind::UnsupportedHost,
        format!(
            "no remote points to a GitHub repository: {}",
            names.join(", ")
        ),
    ))
}

/// Read the remotes of `dir` with `git remote -v` and [`choose`].
pub fn detect(runner: &dyn Runner, dir: &Path) -> Result<ForgeRemote, ForgeError> {
    let out = run_ok(
        runner,
        dir,
        &Invocation::new("git", &["remote", "-v"], Duration::from_secs(20)),
    )?;
    choose(&parse_remote_v(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(url: &str) -> Option<String> {
        parse_url(url).map(|s| format!("{} {}", s.host, s.full_name()))
    }

    #[test]
    fn url_shapes() {
        assert_eq!(
            slug("https://github.com/SJin-Choi/pod.git").as_deref(),
            Some("github.com SJin-Choi/pod")
        );
        assert_eq!(
            slug("https://github.com/o/r").as_deref(),
            Some("github.com o/r")
        );
        assert_eq!(
            slug("https://user:tok@github.com/o/r.git/").as_deref(),
            Some("github.com o/r")
        );
        assert_eq!(
            slug("git@github.com:o/r.git").as_deref(),
            Some("github.com o/r")
        );
        assert_eq!(
            slug("ssh://git@github.com:22/o/r.git").as_deref(),
            Some("github.com o/r")
        );
        assert_eq!(
            slug("ssh://git@GitHub.com/o/r").as_deref(),
            Some("github.com o/r")
        );
        assert_eq!(slug("C:\\repos\\bare.git"), None);
        assert_eq!(slug("C:/repos/bare.git"), None);
        assert_eq!(slug("../bare.git"), None);
        assert_eq!(slug("https://github.com/o"), None);
        assert_eq!(slug("https://gitlab.com/group/sub/r.git"), None);
    }

    /// editor-13: tokens in remote URLs never reach state or messages.
    #[test]
    fn credentials_are_redacted() {
        assert_eq!(
            redact_url("https://user:tok@github.com/o/r.git/"),
            "https://***@github.com/o/r.git/"
        );
        assert_eq!(
            redact_url("https://ghp_secret@github.com/o/r"),
            "https://***@github.com/o/r"
        );
        assert_eq!(
            redact_url("https://github.com/o/r.git"),
            "https://github.com/o/r.git"
        );
        assert_eq!(
            redact_url("ssh://git@github.com:22/o/r.git"),
            "ssh://git@github.com:22/o/r.git"
        );
        assert_eq!(
            redact_url("ssh://git:pw@github.com/o/r.git"),
            "ssh://***@github.com/o/r.git"
        );
        assert_eq!(
            redact_url("git@github.com:o/r.git"),
            "git@github.com:o/r.git"
        );
        assert_eq!(
            redact_url("https://a@b.com/x?y=https://c:d@e.com/z"),
            "https://***@b.com/x?y=https://***@e.com/z"
        );
        assert_eq!(
            redact_urls_in("fatal: unable to access 'https://me:tok@github.com/o/r.git/': 403"),
            "fatal: unable to access 'https://***@github.com/o/r.git/': 403"
        );
        let v = vec![(
            "origin".to_string(),
            "https://me:tok@github.com/o/r.git".to_string(),
        )];
        let r = choose(&v).unwrap();
        assert_eq!(r.url, "https://***@github.com/o/r.git");
        assert!(!serde_json::to_string(&r).unwrap().contains("tok"));
        let lab = vec![(
            "origin".to_string(),
            "https://me:tok@gitlab.com/o/r.git".to_string(),
        )];
        assert!(!choose(&lab).unwrap_err().message.contains("tok"));
    }

    #[test]
    fn remote_v_and_choice() {
        let out = "upstream\thttps://github.com/up/r.git (fetch)\nupstream\thttps://github.com/up/r.git (push)\norigin\tgit@github.com:me/r.git (fetch)\norigin\tgit@github.com:me/r.git (push)\n";
        let v = parse_remote_v(out);
        assert_eq!(v.len(), 2);
        let r = choose(&v).unwrap();
        assert_eq!(
            (r.name.as_str(), r.slug.full_name().as_str()),
            ("origin", "me/r")
        );
        // No origin on GitHub: the first GitHub remote.
        let v = vec![
            ("origin".to_string(), "C:/bare.git".to_string()),
            ("gh".to_string(), "https://github.com/o/r".to_string()),
        ];
        assert_eq!(choose(&v).unwrap().name, "gh");
        assert_eq!(choose(&[]).unwrap_err().kind, ForgeErrorKind::NoRemote);
        let only_lab = vec![(
            "origin".to_string(),
            "https://gitlab.com/o/r.git".to_string(),
        )];
        assert_eq!(
            choose(&only_lab).unwrap_err().kind,
            ForgeErrorKind::UnsupportedHost
        );
        let local = vec![("origin".to_string(), "C:/remotes/r.git".to_string())];
        assert_eq!(
            choose(&local).unwrap_err().kind,
            ForgeErrorKind::UnsupportedHost
        );
    }

    #[test]
    fn detects_from_a_real_repository_without_network() {
        let dir = std::env::temp_dir().join(format!("pod-forge-remote-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args(args)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        let r = crate::runner::ProcessRunner;
        assert_eq!(detect(&r, &dir).unwrap_err().kind, ForgeErrorKind::NoRemote);
        git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/pod-fixtures/demo.git",
        ]);
        let found = detect(&r, &dir).unwrap();
        assert_eq!(found.slug.full_name(), "pod-fixtures/demo");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
