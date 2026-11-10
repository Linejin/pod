//! Wire types for task pull requests (protocol v10, docs/15): `task.pr.*`,
//! `TaskInfo.pr` and the `task.pr.changed` notification. The forge types
//! themselves come from `pod-forge`; everything is re-exported from
//! [`crate::protocol`]. Daemon side: `server/pr.rs`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use pod_forge::{
    CheckBucket, ChecksSummary, ForgeError, ForgeErrorKind, ForgeRemote, MergeMethod, PrCheck,
    PrComment, PrState, PrStatus, PrView, RepoSlug, ReviewDecision,
};

use crate::protocol::Method;

macro_rules! method {
    ($ty:ident, $name:literal, $params:ty, $result:ty) => {
        pub struct $ty;
        impl Method for $ty {
            const NAME: &'static str = $name;
            type Params = $params;
            type Result = $result;
        }
    };
}

method!(
    TaskPrStatusMethod,
    "task.pr.status",
    TaskPrStatusParams,
    TaskPrInfo
);
method!(
    TaskPrCreate,
    "task.pr.create",
    TaskPrCreateParams,
    TaskPrInfo
);
method!(TaskPrMerge, "task.pr.merge", TaskPrMergeParams, TaskPrInfo);

/// `task.pr.changed` on a `status.subscribe` stream with `lifecycle: true`:
/// the task's pull request (number, state, checks, review) changed.
pub const NOTIFY_TASK_PR_CHANGED: &str = "task.pr.changed";

pub const PR_METHODS: &[&str] = &[
    TaskPrStatusMethod::NAME,
    TaskPrCreate::NAME,
    TaskPrMerge::NAME,
];

/// Application error codes added with protocol v10.
pub mod pr_codes {
    /// A forge (git push / gh) operation failed. `data` = `ForgeError`
    /// `{kind, message}`; `kind` says why (`no_remote`, `not_logged_in`,
    /// `no_permission`, `rate_limited`, `not_found`, `network`, `timeout`,
    /// `tool_missing`, `unsupported_host`, `failed`).
    pub const FORGE_FAILED: i64 = -32021;
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskPrStatusParams {
    pub task: String,
    /// Answer from the last known state (`TaskInfo.pr`) without contacting
    /// GitHub. The remote is still detected (local git only).
    #[serde(default)]
    pub cached: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskPrCreateParams {
    pub task: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
    /// Base branch on the remote; default [`TaskPrInfo::default_base`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    /// Push the task branch first (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskPrMergeParams {
    pub task: String,
    pub method: MergeMethod,
}

/// `TaskInfo.pr` (v10): the last known pull request of the task's branch,
/// refreshed by `task.pr.*` calls and, while it is open, by a slow timer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskPr {
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: PrState,
    #[serde(default)]
    pub draft: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_decision: Option<ReviewDecision>,
    #[serde(default)]
    pub checks: ChecksSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_state: Option<String>,
    /// Head commit as last read (for a merged pull request: what was
    /// merged). `None` in records older than this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_oid: Option<String>,
    /// When the daemon last read it from the forge (Unix ms).
    pub updated_ms: u64,
}

impl TaskPr {
    pub fn from_view(v: &PrView, checks: ChecksSummary, updated_ms: u64) -> Self {
        Self {
            number: v.number,
            url: v.url.clone(),
            title: v.title.clone(),
            state: v.state,
            draft: v.draft,
            review_decision: v.review_decision,
            checks,
            merge_state: v.merge_state.clone(),
            head_oid: v.head_oid.clone(),
            updated_ms,
        }
    }

    /// Same pull request facts, ignoring when they were read.
    pub fn same_facts(&self, other: &TaskPr) -> bool {
        Self {
            updated_ms: 0,
            ..self.clone()
        } == Self {
            updated_ms: 0,
            ..other.clone()
        }
    }
}

/// `task.pr.*` result: the task's branch facts and the forge's answer.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskPrInfo {
    pub task: String,
    /// Head branch (`pod/<name>`).
    pub branch: String,
    /// Base branch a new pull request targets by default (the task's base
    /// without a `<remote>/` prefix); absent when the base is a commit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_base: Option<String>,
    /// Commits on the branch that are not on the base.
    pub ahead: u32,
    /// Commits not on any remote-tracking branch (push needed).
    pub unpushed: u32,
    /// Uncommitted entries; they are not part of a pull request.
    pub dirty: u32,
    #[serde(flatten)]
    pub status: PrStatus,
    /// `true` when GitHub was not contacted (`cached`, or the remote is missing).
    #[serde(default)]
    pub cached: bool,
    pub refreshed_ms: u64,
}

/// `task.pr.changed` (v10).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskPrChangedEvent {
    pub seq: u64,
    /// Task id `<repo>/<name>`.
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<TaskPr>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn info_flattens_the_forge_status() {
        let info = TaskPrInfo {
            task: "r/a".into(),
            branch: "pod/a".into(),
            default_base: Some("main".into()),
            ahead: 1,
            unpushed: 1,
            dirty: 0,
            status: PrStatus::failed(ForgeError::no_remote("this repository has no git remote")),
            cached: true,
            refreshed_ms: 5,
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["error"]["kind"], "no_remote");
        assert!(v.get("pr").is_none());
        let back: TaskPrInfo = serde_json::from_value(v).unwrap();
        assert_eq!(back, info);
        let p: TaskPrCreateParams =
            serde_json::from_value(json!({"task": "a", "title": "t"})).unwrap();
        assert!(p.push.is_none() && !p.draft && p.body.is_empty());
        let m: TaskPrMergeParams =
            serde_json::from_value(json!({"task": "a", "method": "squash"})).unwrap();
        assert_eq!(m.method, MergeMethod::Squash);
    }
}
