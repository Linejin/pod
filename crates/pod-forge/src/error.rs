//! Forge failures, classified from `git` / `gh` exit codes and stderr.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ForgeErrorKind {
    /// The worktree has no git remote that names a GitHub repository.
    NoRemote,
    /// A remote exists but is not on GitHub (GitLab, a local path...).
    UnsupportedHost,
    /// `gh` (or `git`) is not installed / not on PATH.
    ToolMissing,
    /// `gh auth login` is needed (or git could not authenticate).
    NotLoggedIn,
    /// Authenticated but not allowed (push access, merge rights, 403).
    NoPermission,
    /// GitHub API rate limit (primary or secondary).
    RateLimited,
    /// No such pull request / repository (404).
    NotFound,
    /// `gh pr create`: a pull request for the head branch already exists.
    AlreadyExists,
    /// `git push` was refused because the remote branch has commits the
    /// local branch lacks (non-fast-forward: someone else pushed).
    Rejected,
    /// Host unreachable / DNS / TLS.
    Network,
    /// The command ran longer than its time limit and was killed.
    Timeout,
    /// Anything else; the message carries the tool's stderr.
    Failed,
}

impl ForgeErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoRemote => "no_remote",
            Self::UnsupportedHost => "unsupported_host",
            Self::ToolMissing => "tool_missing",
            Self::NotLoggedIn => "not_logged_in",
            Self::NoPermission => "no_permission",
            Self::RateLimited => "rate_limited",
            Self::NotFound => "not_found",
            Self::AlreadyExists => "already_exists",
            Self::Rejected => "rejected",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForgeError {
    pub kind: ForgeErrorKind,
    pub message: String,
}

impl ForgeError {
    pub fn new(kind: ForgeErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn no_remote(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorKind::NoRemote, message)
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorKind::Failed, message)
    }

    /// Classify a failed `gh` / `git` run from its exit code and stderr.
    /// `gh` exits 4 when authentication is required.
    pub fn classify(tool: &str, code: Option<i32>, stderr: &str) -> Self {
        let redacted = crate::remote::redact_urls_in(stderr);
        let text = redacted.trim();
        let l = text.to_ascii_lowercase();
        let has = |needles: &[&str]| needles.iter().any(|n| l.contains(n));
        let kind = if tool == "gh" && code == Some(4)
            || has(&[
                "gh auth login",
                "not logged into",
                "authentication failed",
                "could not read username",
                "bad credentials",
                "http 401",
                "requires authentication",
                "terminal prompts disabled",
            ]) {
            ForgeErrorKind::NotLoggedIn
        } else if has(&["rate limit", "abuse detection"]) {
            ForgeErrorKind::RateLimited
        } else if has(&[
            "no git remotes found",
            "none of the git remotes",
            "no such remote",
            "does not appear to be a git repository",
        ]) {
            ForgeErrorKind::NoRemote
        } else if has(&["already exists"]) && has(&["pull request"]) {
            ForgeErrorKind::AlreadyExists
        } else if has(&[
            "http 403",
            "permission to ",
            "must have push access",
            "must have admin rights",
            "resource not accessible",
            "not have the correct permissions",
            "not authorized",
            "protected branch",
            "is not allowed to",
        ]) {
            ForgeErrorKind::NoPermission
        } else if has(&["updates were rejected", "non-fast-forward", "(fetch first)"]) {
            ForgeErrorKind::Rejected
        } else if has(&[
            "no pull requests found",
            "could not resolve to a pullrequest",
            "could not resolve to a repository",
            "http 404",
            "repository not found",
            "not found",
        ]) {
            ForgeErrorKind::NotFound
        } else if has(&[
            "could not resolve host",
            "unable to access",
            "failed to connect",
            "connection timed out",
            "connection refused",
            "error connecting to",
            "tls handshake",
            "no such host",
        ]) {
            ForgeErrorKind::Network
        } else {
            ForgeErrorKind::Failed
        };
        let first = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("");
        let message = if text.is_empty() {
            format!(
                "{tool} exited with code {}",
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            )
        } else if text.len() > 600 {
            // Keep the useful head; gh prints its error first.
            format!("{first} …")
        } else {
            text.to_string()
        };
        Self { kind, message }
    }
}

impl std::fmt::Display for ForgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind.as_str(), self.message)
    }
}

impl std::error::Error for ForgeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use ForgeErrorKind::*;

    fn k(tool: &str, code: i32, s: &str) -> ForgeErrorKind {
        ForgeError::classify(tool, Some(code), s).kind
    }

    /// Messages recorded from gh 2.100 / git 2.5x on this machine (read-only
    /// commands) and from GitHub's documented API errors.
    #[test]
    fn classifies_real_messages() {
        assert_eq!(
            k(
                "gh",
                4,
                "To get started with GitHub CLI, please run:  gh auth login\nAlternatively, populate the GH_TOKEN environment variable with a GitHub API authentication token."
            ),
            NotLoggedIn
        );
        assert_eq!(
            k(
                "gh",
                1,
                "You are not logged into any GitHub hosts. To log in, run: gh auth login"
            ),
            NotLoggedIn
        );
        assert_eq!(k("gh", 1, "no git remotes found"), NoRemote);
        assert_eq!(
            k(
                "gh",
                1,
                "no pull requests found for branch \"pod/nothing-here\""
            ),
            NotFound
        );
        assert_eq!(
            k(
                "gh",
                1,
                "GraphQL: Could not resolve to a PullRequest with the number of 999999999. (repository.pullRequest)"
            ),
            NotFound
        );
        assert_eq!(
            k(
                "gh",
                1,
                "HTTP 403: API rate limit exceeded for user ID 1. (https://api.github.com/graphql)"
            ),
            RateLimited
        );
        assert_eq!(
            k(
                "gh",
                1,
                "GraphQL: You have exceeded a secondary rate limit."
            ),
            RateLimited
        );
        assert_eq!(
            k(
                "gh",
                1,
                "GraphQL: SJin-Choi does not have the correct permissions to execute `MergePullRequest` (mergePullRequest)"
            ),
            NoPermission
        );
        assert_eq!(
            k("gh", 1, "HTTP 403: Resource not accessible by integration"),
            NoPermission
        );
        assert_eq!(
            k(
                "gh",
                1,
                "a pull request for branch \"pod/x\" into branch \"main\" already exists:\nhttps://github.com/o/r/pull/7"
            ),
            AlreadyExists
        );
        assert_eq!(
            k(
                "git",
                128,
                "remote: Permission to o/r.git denied to someone.\nfatal: unable to access 'https://github.com/o/r.git/': The requested URL returned error: 403"
            ),
            NoPermission
        );
        assert_eq!(
            k(
                "git",
                128,
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled"
            ),
            NotLoggedIn
        );
        assert_eq!(
            k(
                "git",
                128,
                "remote: Repository not found.\nfatal: repository 'https://github.com/o/missing.git/' not found"
            ),
            NotFound
        );
        assert_eq!(
            k(
                "git",
                128,
                "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com"
            ),
            Network
        );
        assert_eq!(
            k(
                "git",
                128,
                "fatal: 'origin' does not appear to be a git repository"
            ),
            NoRemote
        );
        assert_eq!(
            k(
                "gh",
                1,
                "pull request create failed: GraphQL: No commits between main and pod/x (createPullRequest)"
            ),
            Failed
        );
        // Recorded 2026-11-09 against a real repository (docs/15 §9): another
        // clone pushed to the task branch first.
        assert_eq!(
            k(
                "git",
                1,
                "error: failed to push some refs to 'https://github.com/o/r.git'
hint: Updates were rejected because the remote contains work that you do not
hint: have locally. This is usually caused by another repository pushing to
hint: the same ref."
            ),
            Rejected
        );
        assert_eq!(
            k(
                "git",
                1,
                "error: failed to push some refs to 'https://github.com/o/r.git'
hint: Updates were rejected because the tip of your current branch is behind"
            ),
            Rejected
        );
        assert_eq!(
            k(
                "git",
                1,
                "! [remote rejected] HEAD -> main (protected branch hook declined)"
            ),
            NoPermission
        );
    }

    #[test]
    fn empty_stderr_names_the_exit_code() {
        let e = ForgeError::classify("gh", Some(2), "  ");
        assert_eq!(e.kind, Failed);
        assert_eq!(e.message, "gh exited with code 2");
        assert_eq!(serde_json::to_value(&e).unwrap()["kind"], "failed");
    }
}
