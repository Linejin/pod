//! Recorded `gh` 2.100 JSON from the public repository cli/cli (read-only
//! `gh pr view` / `gh pr checks` / `gh api` calls, 2026-11-09), and check
//! lists for the UI states built from them. Used by unit tests, the fake
//! forge and the app's fixture scenes. Nothing here is Pod's own data.
//!
//! - `view_open.json` — cli/cli#14964 (open, review required, BLOCKED)
//! - `view_draft.json` — cli/cli#14927 (draft)
//! - `view_merged.json` — cli/cli#14976 (merged, approved)
//! - `checks_open.json` — #14964: 21 checks, pass + skipping
//! - `checks_failed.json` — #14927: one `build (windows-latest)` FAILURE
//! - `checks_merged.json` — #14976: 11 checks, all pass
//! - `review_comments.json` — `gh api repos/cli/cli/pulls/14976/comments --paginate`

use crate::{CheckBucket, PrCheck, gh};

pub const VIEW_OPEN: &str = include_str!("../fixtures/view_open.json");
pub const VIEW_DRAFT: &str = include_str!("../fixtures/view_draft.json");
pub const VIEW_MERGED: &str = include_str!("../fixtures/view_merged.json");
pub const CHECKS_OPEN: &str = include_str!("../fixtures/checks_open.json");
pub const CHECKS_FAILED: &str = include_str!("../fixtures/checks_failed.json");
pub const CHECKS_MERGED: &str = include_str!("../fixtures/checks_merged.json");
pub const REVIEW_COMMENTS: &str = include_str!("../fixtures/review_comments.json");

fn parsed(json: &str) -> Vec<PrCheck> {
    gh::parse_checks(json).expect("recorded checks fixture parses")
}

/// All checks passed (cli/cli#14976).
pub fn checks_passed() -> Vec<PrCheck> {
    parsed(CHECKS_MERGED)
}

/// One failure among passes and skips (cli/cli#14927).
pub fn checks_failed() -> Vec<PrCheck> {
    parsed(CHECKS_FAILED)
}

/// The passed list with its three integration-test jobs still running
/// (`gh` reports `IN_PROGRESS`, bucket `pending`, no completion time).
pub fn checks_pending() -> Vec<PrCheck> {
    let mut v = checks_passed();
    for c in v
        .iter_mut()
        .filter(|c| c.name.starts_with("integration-tests"))
    {
        c.bucket = CheckBucket::Pending;
        c.state = "IN_PROGRESS".into();
        c.completed_at = None;
    }
    v
}

/// Checks for a UI state word: `passed`, `failed`, `pending`, else none.
pub fn checks_for(verdict: &str) -> Vec<PrCheck> {
    match verdict {
        "passed" => checks_passed(),
        "failed" => checks_failed(),
        "pending" => checks_pending(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChecksSummary;

    #[test]
    fn scenario_verdicts() {
        assert_eq!(ChecksSummary::of(&checks_passed()).verdict(), "passed");
        assert_eq!(ChecksSummary::of(&checks_failed()).verdict(), "failed");
        let p = ChecksSummary::of(&checks_pending());
        assert_eq!((p.verdict(), p.pending), ("pending", 3));
        assert!(checks_for("none").is_empty());
    }
}
