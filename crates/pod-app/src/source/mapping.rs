//! podd wire → view models ([`crate::vm`]). Decoders take the raw JSON where
//! newer daemons add fields (status, task) so they are picked up without a
//! protocol bump on our side; shapes they do not know fall back leniently
//! instead of dropping the whole record. The associated `from_json`
//! functions keep their old `crate::vm` paths.

use podd::protocol as p;
use serde_json::Value;

use crate::vm::{
    AgentState, Liveness, RepoVm, StatusVm, TaskGitVm, TaskVm, TerminalVm, program_label,
};

impl AgentState {
    pub fn parse(s: &str) -> Self {
        match s {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "error" => Self::Error,
            "exited" => Self::Exited,
            _ => Self::Unknown,
        }
    }
}

impl StatusVm {
    /// From `StatusInfo` / `TaskStatus` JSON; also accepts a bare `"working"`.
    pub fn from_json(v: &Value) -> Option<Self> {
        match v {
            Value::String(s) => Some(Self::of(AgentState::parse(s))),
            Value::Object(m) => {
                let state = m.get("state").and_then(Value::as_str)?;
                let text = |k: &str| m.get(k).and_then(Value::as_str).map(str::to_owned);
                let num = |k: &str| m.get(k).and_then(Value::as_u64).unwrap_or(0);
                Some(Self {
                    final_state: text("final_state").map(|s| AgentState::parse(&s)),
                    since: num("since"),
                    confidence: m.get("confidence").and_then(Value::as_f64).unwrap_or(0.) as f32,
                    state: AgentState::parse(state),
                    kind: text("kind"),
                    source: text("source"),
                    badge: text("badge"),
                    running_children: num("running_children") as usize,
                    turn: num("turn"),
                    agent: text("agent"),
                    app: text("app"),
                })
            }
            _ => None,
        }
    }
}

impl TerminalVm {
    /// Build from one `terminal.list` entry. Takes the raw JSON so fields
    /// added by newer daemons (status, task) are picked up without a
    /// protocol bump on our side.
    pub fn from_json(v: &Value) -> Option<Self> {
        // `status` is read leniently below; keep a shape mismatch there from
        // dropping the whole terminal.
        let mut base = v.clone();
        if let Some(m) = base.as_object_mut() {
            m.remove("status");
        }
        let info: p::TerminalInfo = serde_json::from_value(base).ok()?;
        // An unreachable holder (v19) cannot be attached: shown like a lost one.
        let unreachable = (info.state == p::TerminalState::Unreachable)
            .then(|| info.unreachable.clone().unwrap_or_default());
        let live = if info.lost || unreachable.is_some() {
            Liveness::Lost
        } else if info.state == p::TerminalState::Exited {
            Liveness::Exited {
                code: info.exit_code,
                killed: info.killed,
            }
        } else {
            Liveness::Running
        };
        let task_id = ["task", "task_id"]
            .iter()
            .find_map(|k| v.get(*k))
            .and_then(|t| {
                t.as_str()
                    .map(str::to_owned)
                    .or_else(|| t.get("id").and_then(Value::as_str).map(str::to_owned))
            });
        let status = v.get("status").and_then(StatusVm::from_json);
        Some(Self {
            program: program_label(&info.argv),
            id: info.id,
            name: info.name,
            argv: info.argv,
            cwd: info.cwd,
            live,
            attached: info.attached,
            created_ms: info.created_ms,
            task_id,
            status,
            parent_terminal: info.parent_terminal,
            parent_task: info.parent_task,
            unreachable,
        })
    }
}

impl TaskVm {
    pub fn from_json(v: &Value) -> Option<Self> {
        // Typed path first (podd protocol v3), lenient fallback for other shapes.
        let status = v.get("status").and_then(StatusVm::from_json);
        if let Ok(t) = serde_json::from_value::<p::TaskInfo>(v.clone()) {
            return Some(Self {
                status,
                title: t.id.clone(),
                repo: Some(t.repo),
                branch: Some(t.branch),
                base: Some(t.base),
                agent: Some(t.agent.to_string()),
                worktree: Some(t.worktree),
                worktree_exists: t.worktree_exists,
                terminals: t.terminals.into_iter().map(|x| x.id).collect(),
                git: t.git.map(|g| TaskGitVm {
                    ahead: g.ahead,
                    behind: g.behind,
                    dirty: g.dirty,
                    unpushed: g.unpushed,
                    last_commit: g
                        .last_commit
                        .map(|c| format!("{} {}", &c.sha[..c.sha.len().min(10)], c.summary)),
                }),
                git_error: t.git_error,
                review_open: t.review.map(|r| r.open).unwrap_or(0),
                parent_task: t.parent_task,
                parent_terminal: t.parent_terminal,
                group: t.group,
                prompt: t.prompt,
                pr: t.pr,
                id: t.id,
            });
        }
        let id = v.get("id")?.as_str()?.to_string();
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_owned);
        let terminals = v
            .get("terminals")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| {
                        t.as_str()
                            .map(str::to_owned)
                            .or_else(|| t.get("id").and_then(Value::as_str).map(str::to_owned))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            title: text("title")
                .or_else(|| text("name"))
                .unwrap_or_else(|| id.clone()),
            repo: text("repo"),
            branch: text("branch"),
            base: text("base"),
            agent: text("agent"),
            worktree: text("worktree").or_else(|| text("path")),
            worktree_exists: true,
            terminals,
            git: None,
            git_error: None,
            review_open: 0,
            status,
            parent_task: text("parent_task"),
            parent_terminal: text("parent_terminal"),
            group: text("group"),
            prompt: text("prompt"),
            pr: v
                .get("pr")
                .and_then(|p| serde_json::from_value(p.clone()).ok()),
            id,
        })
    }
}

/// A `terminal.list` result: undecodable entries are skipped, a result
/// without `terminals` is an empty list.
pub(super) fn terminal_list(list: &Value) -> Vec<TerminalVm> {
    list.get("terminals")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(TerminalVm::from_json).collect())
        .unwrap_or_default()
}

/// A `task.list` result: `{"tasks": […]}` or a bare array.
pub(super) fn task_list(v: &Value) -> Vec<TaskVm> {
    v.get("tasks")
        .or(Some(v))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(TaskVm::from_json).collect())
        .unwrap_or_default()
}

/// A `repo.list` entry or the `repo.add` result.
pub(super) fn repo(r: p::RepoInfo) -> RepoVm {
    RepoVm {
        name: r.name,
        path: r.path,
        default_base: r.default_base,
    }
}
