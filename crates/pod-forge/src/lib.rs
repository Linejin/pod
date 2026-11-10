//! Forge operations for Pod tasks (docs/15): find the GitHub repository a
//! worktree pushes to, push the task branch, open / view / merge / close its
//! pull request, and read its checks and review comments.
//!
//! - [`Forge`] is the provider trait. [`gh::GhForge`] implements it with the
//!   `git` and `gh` CLIs (JSON output only), [`fake::FakeForge`] with an
//!   in-memory (optionally file-backed) state for tests and UI fixtures.
//! - [`runner`] spawns processes the Windows way: no console window, no
//!   prompts, a timeout that kills the child, stdin from a string.
//! - [`bounded`] is the shared bounded child-process runner (process-tree
//!   Job Object, whole-call deadline, bounded output) that `runner` and
//!   podd's archive scripts and group checks use.
//! - [`ForgeError`] maps tool failures to kinds the UI can explain (not
//!   logged in, no remote, no permission, rate limit...).
//! - [`status::collect`] is the one "what is this branch's PR doing" query
//!   shared by podd and the app's fixtures.
//!
//! Nothing here creates repositories or remotes: without a GitHub remote the
//! answer is [`ForgeErrorKind::NoRemote`].

pub mod bounded;
pub mod error;
pub mod fake;
pub mod fixtures;
pub mod gh;
pub mod remote;
pub mod runner;
pub mod status;
mod time;

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use error::{ForgeError, ForgeErrorKind};
pub use status::PrStatus;

/// `owner/name` on a host (`github.com` or a GitHub Enterprise host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepoSlug {
    pub host: String,
    pub owner: String,
    pub name: String,
}

impl RepoSlug {
    /// `owner/name`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// The `--repo` argument of `gh`: `owner/name` on github.com, else
    /// `host/owner/name`.
    pub fn repo_arg(&self) -> String {
        if self.host.eq_ignore_ascii_case("github.com") {
            self.full_name()
        } else {
            format!("{}/{}/{}", self.host, self.owner, self.name)
        }
    }

    pub fn web_url(&self) -> String {
        format!("https://{}/{}/{}", self.host, self.owner, self.name)
    }
}

/// The git remote a task pushes to and the repository it names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ForgeRemote {
    /// Remote name, e.g. `origin`.
    pub name: String,
    /// Fetch URL as configured.
    pub url: String,
    pub slug: RepoSlug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrState {
    Open,
    Closed,
    Merged,
}

/// GitHub's review decision (`reviewDecision`); absent when the repository
/// requires no review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    ReviewRequired,
}

/// `gh pr checks` buckets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckBucket {
    Pass,
    Fail,
    Pending,
    Skipping,
    Cancel,
}

/// One check run or commit status of the pull request's head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrCheck {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    pub bucket: CheckBucket,
    /// GitHub's state word (`SUCCESS`, `FAILURE`, `IN_PROGRESS`, ...).
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
}

/// Check counts. `cancel` counts as failed; `skipping` as skipped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ChecksSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub pending: u32,
    pub skipped: u32,
}

impl ChecksSummary {
    pub fn of(checks: &[PrCheck]) -> Self {
        let mut s = Self::default();
        for c in checks {
            s.total += 1;
            match c.bucket {
                CheckBucket::Pass => s.passed += 1,
                CheckBucket::Fail | CheckBucket::Cancel => s.failed += 1,
                CheckBucket::Pending => s.pending += 1,
                CheckBucket::Skipping => s.skipped += 1,
            }
        }
        s
    }

    /// One word for badges: `none`, `failed`, `pending` or `passed`.
    pub fn verdict(&self) -> &'static str {
        if self.total == 0 {
            "none"
        } else if self.failed > 0 {
            "failed"
        } else if self.pending > 0 {
            "pending"
        } else {
            "passed"
        }
    }
}

/// A pull request as `gh pr view --json` describes it (the fields Pod uses).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrView {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: PrState,
    pub draft: bool,
    /// Head branch name.
    pub head: String,
    /// Head commit (`headRefOid`); for a merged pull request, the commit
    /// that was merged. Branch deletion compares it with the current tip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_oid: Option<String>,
    /// Base branch name.
    pub base: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<ReviewDecision>,
    /// `mergeStateStatus` (`CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY`, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_state: Option<String>,
    /// `mergeable` (`MERGEABLE`, `CONFLICTING`, `UNKNOWN`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mergeable: Option<String>,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub changed_files: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merged_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub closed_at: Option<String>,
}

/// A line comment of a review (`GET /repos/{o}/{r}/pulls/{n}/comments`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrComment {
    pub id: u64,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
    pub body: String,
    pub author: String,
    pub url: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

impl MergeMethod {
    pub fn flag(self) -> &'static str {
        match self {
            MergeMethod::Merge => "--merge",
            MergeMethod::Squash => "--squash",
            MergeMethod::Rebase => "--rebase",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "merge" => Some(Self::Merge),
            "squash" => Some(Self::Squash),
            "rebase" => Some(Self::Rebase),
            _ => None,
        }
    }
}

/// What `create` opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePr {
    pub head: String,
    pub base: String,
    pub title: String,
    pub body: String,
    pub draft: bool,
}

/// Forge operations, run in a worktree directory (`dir`). Blocking; callers
/// on an async runtime use a blocking pool.
pub trait Forge: Send + Sync {
    /// The GitHub remote of `dir`: `origin` when it names a GitHub
    /// repository, else the first remote that does. Never creates one.
    fn detect(&self, dir: &Path) -> Result<ForgeRemote, ForgeError>;
    /// `git push --set-upstream <remote> <branch>`.
    fn push(&self, dir: &Path, remote: &ForgeRemote, branch: &str) -> Result<(), ForgeError>;
    /// Open a pull request (the branch must be pushed). An existing pull
    /// request for the head branch is returned instead of failing.
    fn create(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        req: &CreatePr,
    ) -> Result<PrView, ForgeError>;
    /// The pull request of `branch`: the open one, else the newest.
    fn find_for_branch(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        branch: &str,
    ) -> Result<Option<PrView>, ForgeError>;
    fn view(&self, dir: &Path, remote: &ForgeRemote, number: u64) -> Result<PrView, ForgeError>;
    fn checks(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        number: u64,
    ) -> Result<Vec<PrCheck>, ForgeError>;
    fn review_comments(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        number: u64,
    ) -> Result<Vec<PrComment>, ForgeError>;
    fn merge(
        &self,
        dir: &Path,
        remote: &ForgeRemote,
        number: u64,
        method: MergeMethod,
    ) -> Result<PrView, ForgeError>;
    fn close(&self, dir: &Path, remote: &ForgeRemote, number: u64) -> Result<PrView, ForgeError>;
}

/// The forge podd uses: `POD_FORGE=fake:<state.json>` selects the fake
/// (tests, demos, screenshots), anything else the `gh` CLI.
pub fn from_env() -> Box<dyn Forge> {
    match std::env::var("POD_FORGE") {
        Ok(v) if v.starts_with("fake:") => {
            Box::new(fake::FakeForge::with_file(v["fake:".len()..].into()))
        }
        Ok(v) if v == "fake" => Box::new(fake::FakeForge::new(fake::FakeState::default())),
        _ => Box::new(gh::GhForge::new(runner::ProcessRunner)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(bucket: CheckBucket) -> PrCheck {
        PrCheck {
            name: "x".into(),
            workflow: None,
            bucket,
            state: String::new(),
            link: None,
            description: None,
            started_at: None,
            completed_at: None,
        }
    }

    #[test]
    fn summary_counts_and_verdict() {
        assert_eq!(ChecksSummary::of(&[]).verdict(), "none");
        let s = ChecksSummary::of(&[
            check(CheckBucket::Pass),
            check(CheckBucket::Skipping),
            check(CheckBucket::Pending),
        ]);
        assert_eq!(
            (s.total, s.passed, s.pending, s.skipped, s.failed),
            (3, 1, 1, 1, 0)
        );
        assert_eq!(s.verdict(), "pending");
        let s = ChecksSummary::of(&[
            check(CheckBucket::Pass),
            check(CheckBucket::Cancel),
            check(CheckBucket::Pending),
        ]);
        assert_eq!(s.verdict(), "failed");
        assert_eq!(
            ChecksSummary::of(&[check(CheckBucket::Pass), check(CheckBucket::Skipping)]).verdict(),
            "passed"
        );
    }

    #[test]
    fn slug_args() {
        let s = RepoSlug {
            host: "github.com".into(),
            owner: "o".into(),
            name: "r".into(),
        };
        assert_eq!(s.repo_arg(), "o/r");
        assert_eq!(s.web_url(), "https://github.com/o/r");
        let e = RepoSlug {
            host: "ghe.example.com".into(),
            ..s
        };
        assert_eq!(e.repo_arg(), "ghe.example.com/o/r");
    }

    #[test]
    fn merge_method_words() {
        assert_eq!(MergeMethod::parse("Squash"), Some(MergeMethod::Squash));
        assert_eq!(MergeMethod::parse("ff"), None);
        assert_eq!(MergeMethod::Rebase.flag(), "--rebase");
        assert_eq!(serde_json::to_value(MergeMethod::Merge).unwrap(), "merge");
    }
}
