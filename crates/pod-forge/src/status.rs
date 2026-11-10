//! "What is this branch's pull request doing": remote, PR, checks, review
//! comments in one answer. Shared by podd (`task.pr.status`) and fixtures.

use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{ForgeError, ForgeErrorKind};
use crate::{ChecksSummary, Forge, ForgeRemote, PrCheck, PrComment, PrState, PrView};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PrStatus {
    /// The GitHub remote; absent with `error.kind = no_remote` (or
    /// `unsupported_host`) when the repository has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<ForgeRemote>,
    /// The branch's pull request (open one first, else the newest).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrView>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<PrCheck>,
    #[serde(default)]
    pub summary: ChecksSummary,
    /// Review line comments (open pull requests only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<PrComment>,
    /// The first failure. With a `pr` it concerns checks or comments only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ForgeError>,
}

impl PrStatus {
    pub fn failed(error: ForgeError) -> Self {
        Self {
            error: Some(error),
            ..Self::default()
        }
    }

    pub fn no_remote(&self) -> bool {
        self.remote.is_none()
            && self.error.as_ref().is_some_and(|e| {
                matches!(
                    e.kind,
                    ForgeErrorKind::NoRemote | ForgeErrorKind::UnsupportedHost
                )
            })
    }
}

/// Detect the remote, find the pull request of `branch` (by `known` number
/// first), and read its checks and (when open) review comments.
pub fn collect(forge: &dyn Forge, dir: &Path, branch: &str, known: Option<u64>) -> PrStatus {
    let remote = match forge.detect(dir) {
        Ok(r) => r,
        Err(e) => return PrStatus::failed(e),
    };
    let mut st = PrStatus {
        remote: Some(remote.clone()),
        ..PrStatus::default()
    };
    let pr = match known.map(|n| forge.view(dir, &remote, n)) {
        Some(Ok(pr)) if pr.head == branch || pr.head.is_empty() => Ok(Some(pr)),
        // Unknown or re-targeted number: look the branch up again.
        Some(Ok(_))
        | Some(Err(ForgeError {
            kind: ForgeErrorKind::NotFound,
            ..
        }))
        | None => forge.find_for_branch(dir, &remote, branch),
        Some(Err(e)) => Err(e),
    };
    match pr {
        Ok(Some(pr)) => {
            fill(forge, dir, &remote, &mut st, &pr);
            st.pr = Some(pr);
        }
        Ok(None) => {}
        Err(e) => st.error = Some(e),
    }
    st
}

/// Checks and comments of `pr` into `st` (errors kept, not fatal).
pub fn fill(forge: &dyn Forge, dir: &Path, remote: &ForgeRemote, st: &mut PrStatus, pr: &PrView) {
    match forge.checks(dir, remote, pr.number) {
        Ok(c) => {
            st.summary = ChecksSummary::of(&c);
            st.checks = c;
        }
        Err(e) => st.error = Some(e),
    }
    if pr.state == PrState::Open {
        match forge.review_comments(dir, remote, pr.number) {
            Ok(c) => st.comments = c,
            Err(e) => {
                st.error.get_or_insert(e);
            }
        }
    }
}
