//! Persisted state of review comments, deliveries, groups and gates
//! (`<state_dir>\orch\store.json`, written atomically like the task
//! registry: temp file + rename; a corrupt file is moved aside).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::log;
use crate::protocol::*;

const STORE_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryRecord {
    pub info: DeliveryInfo,
    /// Status turn counter when the prompt was typed in; the next queued
    /// delivery to the same terminal waits for a later turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_at_delivery: Option<u64>,
    /// v8: whether the agent took the prompt as a new turn (docs/12 B1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<crate::protocol::SubmitCheck>,
    /// Who asked for it when not the PC (docs/17 M1: a mobile message).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<DeliveryOrigin>,
}

/// A delivery requested through the mobile API. `operation_id` + `digest`
/// make the request idempotent; stored with the delivery itself, so the
/// replay record and the delivery are one atomic write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeliveryOrigin {
    pub device: String,
    pub operation_id: String,
    /// SHA-256 of the request (task, text, with_open_comments).
    pub digest: String,
}

/// Who decided a gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum GateActor {
    /// The PC (pipe: app, CLI).
    Local,
    Mobile {
        device: String,
    },
    /// v17: a coordinator agent (`run:<id>`) deciding its worker's gate.
    Agent {
        from: String,
    },
}

impl GateActor {
    pub fn label(&self) -> String {
        match self {
            Self::Local => "local".into(),
            Self::Mobile { device } => device.clone(),
            Self::Agent { from } => from.clone(),
        }
    }
}

/// Audit record of one gate decision, committed in the same store write as
/// the gate transition (docs/17 §4.3). Also the idempotency record of
/// `operation_id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GateOpRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    pub gate: String,
    pub decision: GateDecision,
    /// v17: the chosen option (gates with options).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    pub actor: GateActor,
    pub prev: GateState,
    pub new: GateState,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMemberRecord {
    pub task: String,
    pub agent: AgentSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<CheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRecord {
    pub id: String,
    pub repo: String,
    pub name: String,
    pub base: String,
    pub base_commit: String,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<Vec<String>>,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picked: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picked_ms: Option<u64>,
    pub members: Vec<GroupMemberRecord>,
    /// The last (or running) background `group.check` (v6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_job: Option<CheckJob>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    #[serde(default)]
    pub version: u32,
    /// Last number handed out per id prefix (`c`, `d`, `g`).
    #[serde(default)]
    pub counters: BTreeMap<String, u64>,
    #[serde(default)]
    pub comments: Vec<ReviewComment>,
    #[serde(default)]
    pub deliveries: Vec<DeliveryRecord>,
    #[serde(default)]
    pub groups: Vec<GroupRecord>,
    #[serde(default)]
    pub gates: Vec<GateInfo>,
    /// Gate decisions (who, when, operation id), newest last, bounded.
    #[serde(default)]
    pub gate_ops: Vec<GateOpRecord>,
}

impl Store {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("orch").join("store.json")
    }

    pub fn load(state_dir: &Path) -> Self {
        let path = Self::path(state_dir);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                log(&format!("cannot read {}: {e}", path.display()));
                return Self::default();
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(s) => s,
            Err(e) => {
                let aside = path.with_extension(format!("corrupt-{}.json", crate::now_ms()));
                log(&format!(
                    "store {} is corrupt ({e}); moved to {}",
                    path.display(),
                    aside.display()
                ));
                let _ = std::fs::rename(&path, aside);
                Self::default()
            }
        }
    }

    pub fn save(&mut self, state_dir: &Path) -> Result<(), RpcError> {
        self.version = STORE_VERSION;
        let path = Self::path(state_dir);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
            std::fs::rename(&tmp, &path)
        };
        write().map_err(|e| {
            RpcError::new(
                codes::INTERNAL_ERROR,
                format!("cannot save {}: {e}", path.display()),
            )
        })
    }

    /// Next id with this prefix: `c1`, `c2`...
    pub fn next_id(&mut self, prefix: &str) -> String {
        let n = self.counters.entry(prefix.to_string()).or_insert(0);
        *n += 1;
        format!("{prefix}{n}")
    }

    pub fn review_counts(&self, task: &str) -> Option<ReviewCounts> {
        let mut c = ReviewCounts::default();
        let mut any = false;
        for x in self.comments.iter().filter(|x| x.task == task) {
            any = true;
            match x.state {
                CommentState::Open => c.open += 1,
                CommentState::Queued => c.queued += 1,
                CommentState::Sent => c.sent += 1,
                CommentState::Resolved => c.resolved += 1,
            }
        }
        any.then_some(c)
    }

    pub fn task_gates(&self, task: &str) -> Vec<GateInfo> {
        self.gates
            .iter()
            .filter(|g| g.task.as_deref() == Some(task))
            .cloned()
            .collect()
    }

    pub fn group_of(&self, task: &str) -> Option<String> {
        self.groups
            .iter()
            .find(|g| g.members.iter().any(|m| m.task == task))
            .map(|g| g.id.clone())
    }

    /// By id (`<repo>/<name>`), else by name when unique.
    pub fn group(&self, s: &str) -> Result<&GroupRecord, RpcError> {
        if let Some(g) = self.groups.iter().find(|g| g.id == s) {
            return Ok(g);
        }
        let mut by_name = self.groups.iter().filter(|g| g.name == s);
        match (by_name.next(), by_name.next()) {
            (Some(g), None) => Ok(g),
            (Some(_), Some(_)) => Err(RpcError::new(
                orch_codes::GROUP_NOT_FOUND,
                format!("group name {s:?} is ambiguous; use <repo>/<name>"),
            )),
            _ => Err(RpcError::new(
                orch_codes::GROUP_NOT_FOUND,
                format!("no group {s:?}"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_roundtrip_ids_and_corrupt_file() {
        let dir = std::env::temp_dir().join(format!(
            "pod-orch-store-test-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let mut s = Store::default();
        assert_eq!(s.next_id("c"), "c1");
        assert_eq!(s.next_id("c"), "c2");
        assert_eq!(s.next_id("g"), "g1");
        s.comments.push(ReviewComment {
            id: "c1".into(),
            task: "app/x".into(),
            file: "a.rs".into(),
            side: Side::New,
            line: 3,
            end_line: None,
            body: "why?".into(),
            code: Some("let x = 1;".into()),
            state: CommentState::Open,
            created_ms: 1,
            delivery: None,
            resolved_ms: None,
            note: None,
        });
        s.save(&dir).unwrap();
        let mut back = Store::load(&dir);
        assert_eq!(back.version, STORE_VERSION);
        assert_eq!(back.comments.len(), 1);
        assert_eq!(back.next_id("c"), "c3");
        assert_eq!(back.review_counts("app/x").unwrap().open, 1);
        assert!(back.review_counts("app/y").is_none());

        std::fs::write(Store::path(&dir), b"{oops").unwrap();
        assert!(Store::load(&dir).comments.is_empty());
        let aside = std::fs::read_dir(dir.join("orch"))
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("corrupt"));
        assert!(aside);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
