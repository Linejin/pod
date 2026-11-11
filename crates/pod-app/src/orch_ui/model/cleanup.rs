//! The cleanup screen (docs/19 S3): finished workers with their task's
//! cleanup tier, in order, and what the screen may do with each.

use std::collections::{BTreeSet, HashMap};

use podd::protocol::{AttemptState, CleanupCandidate, CleanupTier, Lifecycle, WorkerState};

use super::{RunEntry, id_number};
use crate::orch_ui::s;

/// What the cleanup screen may do with a finished worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanupActions {
    pub release: bool,
    pub keep: bool,
    pub archive: bool,
    pub resume: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CleanupRow {
    pub run: String,
    pub attempt: String,
    pub item_title: String,
    pub task: Option<String>,
    pub state: AttemptState,
    pub worker: WorkerState,
    /// From `task.cleanup_candidates` (S3); `None` = not listed (no task, or not loaded).
    pub tier: Option<CleanupTier>,
    pub reasons: Vec<String>,
    pub archived: bool,
    pub ended_ms: u64,
    pub actions: CleanupActions,
}

impl CleanupRow {
    /// Selected for the bulk action by default: only the `ready` tier.
    pub fn default_selected(&self) -> bool {
        self.tier == Some(CleanupTier::Ready) && self.actions.archive
    }
}

pub fn tier_label(t: Option<CleanupTier>) -> &'static str {
    match t {
        Some(CleanupTier::Protected) => s::TIER_PROTECTED,
        Some(CleanupTier::Review) => s::TIER_REVIEW,
        Some(CleanupTier::Ready) => s::TIER_READY,
        None => s::TIER_NONE,
    }
}

/// Workers whose attempt ended (newest first), with their task's cleanup tier.
pub fn cleanup_rows<'a>(
    runs: impl IntoIterator<Item = &'a RunEntry>,
    candidates: &HashMap<String, CleanupCandidate>,
) -> Vec<CleanupRow> {
    let mut out = Vec::new();
    for run in runs {
        for a in run.info.attempts.iter().filter(|a| !a.state.is_open()) {
            let c = a.task.as_ref().and_then(|t| candidates.get(t));
            let archived = c.is_some_and(|c| c.lifecycle == Lifecycle::Archived);
            let tier = c.map(|c| c.tier);
            let has_terminal = a.terminal.is_some() && a.worker != WorkerState::Released;
            out.push(CleanupRow {
                run: run.id().to_string(),
                attempt: a.id.clone(),
                item_title: run
                    .item(&a.item)
                    .map(|i| i.title.clone())
                    .unwrap_or_else(|| a.item.clone()),
                task: a.task.clone(),
                state: a.state,
                worker: a.worker,
                tier,
                reasons: c.map(|c| c.reasons.clone()).unwrap_or_default(),
                archived,
                ended_ms: a.ended_ms.unwrap_or(a.created_ms),
                actions: CleanupActions {
                    release: has_terminal,
                    keep: a.worker == WorkerState::Finished,
                    // Protected tasks are never archived from here; a task
                    // that is not listed is unknown, so it is not offered either.
                    archive: a.task.is_some()
                        && !archived
                        && matches!(tier, Some(CleanupTier::Ready | CleanupTier::Review)),
                    resume: a.task.is_some() && (archived || a.worker == WorkerState::Released),
                },
            });
        }
    }
    // Ready first, then review, protected, and tasks that are not listed.
    let rank = |t: Option<CleanupTier>| match t {
        Some(CleanupTier::Ready) => 0,
        Some(CleanupTier::Review) => 1,
        Some(CleanupTier::Protected) => 2,
        None => 3,
    };
    out.sort_by_key(|r| {
        (
            rank(r.tier),
            std::cmp::Reverse(r.ended_ms),
            std::cmp::Reverse(id_number(&r.attempt)),
        )
    });
    out
}

/// The default selection of the bulk action.
pub fn default_selection(rows: &[CleanupRow]) -> BTreeSet<String> {
    rows.iter()
        .filter(|r| r.default_selected())
        .map(|r| r.attempt.clone())
        .collect()
}
