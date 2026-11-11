//! View models: what the UI renders, decoupled from podd's wire types.
//!
//! The data source ([`crate::source`]) is the only module that turns podd
//! replies into these (its `mapping` holds the `from_json` decoders).
//! Features podd does not have yet (tasks, live agent status) are optional
//! here: the UI shows them only when the connected daemon advertises them
//! ([`Capabilities`]). Labels and counts for display are in [`presentation`].

mod presentation;

pub use presentation::{counts, program_label};

/// Agent state, as the P1 status engine names it (docs/01 §4.2 (2)).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Done,
    Error,
    Unknown,
    /// The agent process ended (the terminal may still run a shell).
    Exited,
}

/// Live agent status of a terminal or task (podd protocol v4, docs/07 §8.3).
/// Includes status provenance and timestamps for source/freshness inspection.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusVm {
    pub final_state: Option<AgentState>,
    pub since: u64,
    pub confidence: f32,
    pub state: AgentState,
    /// Blocked kind (`permission`, `question`, `auth`, `other`).
    pub kind: Option<String>,
    /// Where the state came from (`hook`, `osc`, `heuristic`, `process`).
    pub source: Option<String>,
    /// podd's display label: `blocked:question`, `done (+1 running)`.
    pub badge: Option<String>,
    /// Held-open sub-records still running.
    pub running_children: usize,
    /// Turn counter (increments when the agent leaves `working`).
    pub turn: u64,
    /// podd's agent guess for the foreground program (`claude`, `codex`, OSC `app=` …).
    pub agent: Option<String>,
    /// OSC 7501 `app=`: the program reported itself.
    pub app: Option<String>,
}

impl StatusVm {
    pub fn of(state: AgentState) -> Self {
        Self {
            final_state: None,
            since: 0,
            confidence: 0.,
            state,
            kind: None,
            source: None,
            badge: None,
            running_children: 0,
            turn: 0,
            agent: None,
            app: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Liveness {
    Running,
    Exited {
        code: Option<i32>,
        killed: bool,
    },
    /// The holder vanished without reporting an exit.
    Lost,
}

/// One podd terminal as the sidebar and panels show it.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalVm {
    pub id: String,
    /// podd name (unique among running terminals), if any.
    pub name: Option<String>,
    /// Short program label: `claude`, `codex`, `powershell` …
    pub program: String,
    pub argv: Vec<String>,
    pub cwd: String,
    pub live: Liveness,
    pub attached: usize,
    pub created_ms: u64,
    /// Task this terminal belongs to (podd task model, when available).
    pub task_id: Option<String>,
    pub status: Option<StatusVm>,
    /// v7: the Pod terminal this one was started from (`pod run` inside a Pod
    /// terminal). Kept by podd after either terminal exits.
    pub parent_terminal: Option<String>,
    /// v7: the task the parent terminal belonged to when this one started.
    pub parent_task: Option<String>,
    /// v19: podd cannot reach this terminal's holder; the reason it gave
    /// (empty when none). Shown as `Lost`, the reason goes in the row tooltip.
    pub unreachable: Option<String>,
}

impl TerminalVm {
    pub fn is_running(&self) -> bool {
        self.live == Liveness::Running
    }
}

/// Git facts of a task worktree (podd `TaskGit`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TaskGitVm {
    pub ahead: u32,
    pub behind: u32,
    pub dirty: u32,
    pub unpushed: u32,
    /// `sha summary` of the last commit.
    pub last_commit: Option<String>,
}

/// A task (git worktree + agent terminal), when podd has the task model (docs/08).
#[derive(Clone, Debug, PartialEq)]
pub struct TaskVm {
    pub id: String,
    pub title: String,
    pub repo: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub agent: Option<String>,
    pub worktree: Option<String>,
    pub worktree_exists: bool,
    pub terminals: Vec<String>,
    pub git: Option<TaskGitVm>,
    pub git_error: Option<String>,
    /// Open review comments (podd v5 `TaskInfo.review.open`).
    pub review_open: usize,
    /// Aggregate status of the task's terminals.
    pub status: Option<StatusVm>,
    /// Task / terminal this task was started from (`pod task new` / `pod group new`
    /// inside a Pod terminal), for the agent tree (P5).
    pub parent_task: Option<String>,
    pub parent_terminal: Option<String>,
    /// Fan-out group this task is a member of (v5 `TaskInfo.group`).
    pub group: Option<String>,
    /// Task prompt (v3), prefills the pull request title/body.
    pub prompt: Option<String>,
    /// Last known pull request (v10 `TaskInfo.pr`, docs/15).
    pub pr: Option<podd::protocol::TaskPr>,
}

/// A registered repository (`repo.list`).
#[derive(Clone, Debug, PartialEq)]
pub struct RepoVm {
    pub name: String,
    pub path: String,
    pub default_base: Option<String>,
}

/// Optional daemon features, probed on every (re)connect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// `task.list` exists.
    pub tasks: bool,
    /// Terminals carry live agent status.
    pub status: bool,
    /// `terminal.rename` exists (otherwise renames are app-local labels).
    pub rename: bool,
    /// Protocol v5: `review.*` comments and delivery (docs/11).
    pub review: bool,
    /// Protocol v6: fan-out groups with background `group.check`, gates with
    /// expiry (docs/11 §3.2–3.3) — the orchestration UI (P5).
    pub orch: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnState {
    /// First connection attempt (may auto-start podd).
    Connecting,
    Connected {
        pid: u32,
        version: String,
    },
    /// Lost the daemon; retrying (and restarting it) every second.
    Reconnecting {
        attempt: u32,
        error: String,
    },
}

impl ConnState {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

/// Counts for the status bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub running: usize,
    pub working: usize,
    pub blocked: usize,
    pub done: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn program_labels() {
        assert_eq!(
            program_label(&argv(&[r"C:\nvm\claude.cmd", "--x"])),
            "claude"
        );
        assert_eq!(
            program_label(&argv(&["powershell.exe", "-NoLogo"])),
            "powershell"
        );
        assert_eq!(program_label(&argv(&["cmd.exe", "/c", "codex"])), "codex");
        assert_eq!(program_label(&argv(&["cmd.exe"])), "cmd");
        assert_eq!(program_label(&[]), "?");
    }

    fn info(extra: Value) -> Value {
        let mut v = json!({
            "id": "t3", "argv": ["claude"], "cwd": "C:\\w", "state": "running",
            "cols": 80, "rows": 24, "created_ms": 5, "attached": 2
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        v
    }

    #[test]
    fn terminal_from_p0_info() {
        let t = TerminalVm::from_json(&info(json!({}))).unwrap();
        assert_eq!(t.id, "t3");
        assert_eq!(t.program, "claude");
        assert!(t.is_running());
        assert_eq!(t.attached, 2);
        assert_eq!(t.status, None);
        assert_eq!(t.task_id, None);
        let x = TerminalVm::from_json(&info(json!({"state": "exited", "exit_code": 3}))).unwrap();
        assert_eq!(
            x.live,
            Liveness::Exited {
                code: Some(3),
                killed: false
            }
        );
        assert_eq!(x.live_label(), "종료 3");
        let l = TerminalVm::from_json(&info(json!({"state": "exited", "lost": true}))).unwrap();
        assert_eq!(l.live, Liveness::Lost);
    }

    #[test]
    fn terminal_with_future_fields() {
        let t = TerminalVm::from_json(&info(json!({
            "status": {"state": "blocked", "kind": "permission", "source": "hook", "confidence": 0.95,
                       "since": 1, "turn": 2, "badge": "blocked:permission", "records": 1,
                       "running_children": 0, "blocked_children": 0, "hooks_accepted": 1, "hooks_rejected": 0},
            "task": {"id": "k1"}
        })))
        .unwrap();
        assert_eq!(t.task_id.as_deref(), Some("k1"));
        let s = t.status.unwrap();
        assert_eq!(s.state, AgentState::Blocked);
        assert_eq!(s.source.as_deref(), Some("hook"));
        assert_eq!(s.kind.as_deref(), Some("permission"));
        assert!(s.label().contains("승인 필요"));
        assert_eq!(s.turn, 2);
        let w =
            TerminalVm::from_json(&info(json!({"status": "working", "task_id": "k2"}))).unwrap();
        assert_eq!(w.status.unwrap().state, AgentState::Working);
        assert_eq!(w.task_id.as_deref(), Some("k2"));
    }

    #[test]
    fn terminal_parent_fields_v7() {
        let t = TerminalVm::from_json(&info(json!({
            "state": "exited", "exit_code": 7, "parent_terminal": "t2", "parent_task": "sample/app-fix"
        })))
        .unwrap();
        assert_eq!(t.parent_terminal.as_deref(), Some("t2"));
        assert_eq!(t.parent_task.as_deref(), Some("sample/app-fix"));
        assert_eq!(
            t.live,
            Liveness::Exited {
                code: Some(7),
                killed: false
            }
        );
        let none = TerminalVm::from_json(&info(json!({}))).unwrap();
        assert_eq!((none.parent_terminal, none.parent_task), (None, None));
    }

    #[test]
    fn task_from_protocol_v3() {
        let k = TaskVm::from_json(&json!({
            "id": "demo/feat-1", "repo": "demo", "name": "feat-1", "branch": "pod/feat-1",
            "base": "main", "base_commit": "abc", "worktree": "C:/wt/demo/feat-1",
            "agent": "claude", "created_ms": 1, "worktree_exists": true,
            "terminals": [{"id": "t4"}],
            "git": {"ahead": 1, "behind": 2, "dirty": 3, "unpushed": 1,
                    "last_commit": {"sha": "0123456789abcdef", "summary": "add f", "author": "a", "time_ms": 0}}
        }))
        .unwrap();
        assert_eq!(k.title, "demo/feat-1");
        assert_eq!(k.agent.as_deref(), Some("claude"));
        assert_eq!(k.terminals, ["t4"]);
        assert_eq!(k.git_summary().as_deref(), Some("↑1 ↓2 · 미커밋 3"));
        assert_eq!(
            k.git.unwrap().last_commit.as_deref(),
            Some("0123456789 add f")
        );
    }

    #[test]
    fn task_parse_and_counts() {
        let k = TaskVm::from_json(&json!({"id": "k1", "name": "fix bug", "branch": "b", "terminals": ["t1", {"id": "t2"}]})).unwrap();
        assert_eq!(k.title, "fix bug");
        assert_eq!(k.terminals, ["t1", "t2"]);
        assert!(TaskVm::from_json(&json!({"title": "no id"})).is_none());

        let a = TerminalVm::from_json(&info(json!({"status": "working"}))).unwrap();
        let b = TerminalVm::from_json(&info(json!({"status": "blocked"}))).unwrap();
        let c = TerminalVm::from_json(&info(json!({"state": "exited"}))).unwrap();
        assert_eq!(
            counts(&[a, b, c]),
            Counts {
                total: 3,
                running: 2,
                working: 1,
                blocked: 1,
                done: 0
            }
        );
    }
}
