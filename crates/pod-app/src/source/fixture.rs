//! The read-only UI fixture source (`--ui-fixture`): fixed tasks and
//! terminals, fixture answers on the request connection, never podd. Not
//! `cfg(test)`: release captures use it too.

use std::collections::HashMap;
use std::sync::mpsc as std_mpsc;

use futures::channel::mpsc;
use serde_json::{Value, json};

use super::rpc::RpcCmd;
use super::{PoddSource, SourceError, SourceEvent};
use crate::vm::{Capabilities, ConnState, RepoVm, StatusVm, TaskVm, TerminalVm};

impl PoddSource {
    /// Read-only, isolated UI volume fixture: never connects to or starts podd.
    pub fn fixture() -> (Self, mpsc::UnboundedReceiver<SourceEvent>) {
        let (tx, _) = std_mpsc::channel();
        // Editor QA scenes give task-000 a real (fixture) worktree; only then
        // does the fixture answer `task.diff` itself (from that worktree's
        // git). Everything else stays unanswered, as before.
        let fixture_worktree = crate::fixture_scenes::worktree();
        let (rpc, rpc_rx) = std_mpsc::channel::<RpcCmd>();
        let wt = fixture_worktree.clone();
        std::thread::Builder::new()
            .name("fixture-rpc".into())
            .spawn(move || {
                while let Ok((method, params, reply)) = rpc_rx.recv() {
                    // PR scenes (docs/15): `task.pr.*` from the fake forge.
                    if let Some(a) = crate::orch_ui::fixture::answer(method, &params) {
                        let _ = reply.send(a);
                    } else if let Some(a) = crate::pr_panel::fixture_answer(method, &params)
                        .or_else(|| crate::layouts::kanban::fixture_answer(method, &params))
                    {
                        let _ = reply.send(a);
                    } else if let ("task.diff", Some(wt)) = (method, wt.as_ref()) {
                        let _ = reply.send(fixture_task_diff(std::path::Path::new(wt)));
                    } else if method == "review.comments.list" {
                        let _ = reply.send(Ok(json!({"comments": []})));
                    } else if method == "review.deliveries" {
                        let _ = reply.send(Ok(json!({"deliveries": []})));
                    }
                }
            })
            .expect("spawn fixture rpc thread");
        let (input, _) = std_mpsc::channel();
        let (etx, erx) = mpsc::unbounded();
        let mut tasks = Vec::new();
        let mut terminals = Vec::new();
        for n in 0..100 {
            let ids: Vec<String> = (0..3).map(|k| format!("fixture-{}", n * 3 + k)).collect();
            let mut task=TaskVm::from_json(&json!({"id":format!("sample/task-{n:03}"),"name":format!("Task {n:03} - 한글 작업과 레이아웃 🔎"),"agent":"powershell.exe -NoProfile -File C:/fixtures/sample/agent.ps1","repo":"sample","terminals":ids,"branch":format!("pod/task-{n:03}")})).expect("fixture task");
            if n == 0 {
                task.worktree = fixture_worktree.clone();
            }
            for (k, id) in ids.iter().enumerate() {
                let state = ["working", "blocked", "done", "idle", "unknown", "error"][(n + k) % 6];
                terminals.push(TerminalVm {
                    id: id.clone(),
                    name: Some(format!("Worker {}", n * 3 + k)),
                    program: "powershell".into(),
                    argv: vec!["powershell.exe".into(), "-NoProfile".into(), "-File".into(), "C:/fixtures/sample/agent.ps1".into()],
                    cwd: "C:/fixtures/sample".into(),
                    live: crate::vm::Liveness::Running,
                    attached: 0,
                    created_ms: 0,
                    task_id: Some(task.id.clone()),
                    // A mix of agents (docs/10 §D5.11): Claude, Codex, then a plain shell per task.
                    status: StatusVm::from_json(&json!({"state":state,"kind":"permission","source":if state == "unknown" {"heuristic"} else {"osc"},"confidence":if state == "unknown" {0.4} else {1.0},"since":0,
                        "agent":([Some("claude"),Some("codex"),None][k % 3])})),
                    parent_terminal: None,
                    parent_task: None,
                    unreachable: None,
                });
            }
            tasks.push(task);
        }
        let _ = etx.unbounded_send(SourceEvent::Conn(ConnState::Connected {
            pid: 0,
            version: "UI fixture (read only)".into(),
        }));
        let _ = etx.unbounded_send(SourceEvent::Caps(Capabilities {
            tasks: true,
            status: true,
            review: true,
            orch: true,
            ..Default::default()
        }));
        let _ = etx.unbounded_send(SourceEvent::Repos(vec![RepoVm {
            name: "sample".into(),
            path: "C:/fixtures/sample".into(),
            default_base: Some("main".into()),
        }]));
        let _ = etx.unbounded_send(SourceEvent::Tasks(tasks));
        let _ = etx.unbounded_send(SourceEvent::Terminals(terminals));
        if let Some(g) = crate::orch_ui::fixture::gates() {
            let _ = etx.unbounded_send(SourceEvent::Gates(g));
        }
        let _ = etx.unbounded_send(SourceEvent::OrchResync);
        (
            Self {
                pipe: "ui-fixture-no-daemon".into(),
                tx,
                rpc,
                input,
            },
            erx,
        )
    }
}

/// `task.diff` for the editor QA fixture: the worktree against HEAD (tracked
/// changes + untracked files), shaped like podd's answer.
pub(super) fn fixture_task_diff(wt: &std::path::Path) -> Result<Value, SourceError> {
    let git = |args: &[&str]| -> Result<String, SourceError> {
        let mut c = std::process::Command::new("git");
        c.arg("-C").arg(wt).args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        let out = c.output().map_err(|e| SourceError::msg(e.to_string()))?;
        if !out.status.success() {
            return Err(SourceError::msg(
                String::from_utf8_lossy(&out.stderr).into_owned(),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let head = git(&["rev-parse", "HEAD"])?.trim().to_string();
    let mut counts: HashMap<String, (u64, u64)> = HashMap::new();
    for line in git(&["diff", "--numstat", "HEAD"])?.lines() {
        let mut it = line.splitn(3, '\t');
        if let (Some(a), Some(d), Some(path)) = (it.next(), it.next(), it.next()) {
            counts.insert(
                path.to_string(),
                (a.parse().unwrap_or(0), d.parse().unwrap_or(0)),
            );
        }
    }
    let mut files = Vec::new();
    let (mut added, mut deleted) = (0, 0);
    for entry in git(&["status", "--porcelain=v1", "-z", "--untracked-files=all"])?
        .split('\0')
        .filter(|e| e.len() > 3)
    {
        let (xy, path) = entry.split_at(3);
        let status = match xy.trim() {
            "??" => "?",
            x if x.contains('D') => "D",
            x if x.contains('A') => "A",
            _ => "M",
        };
        let (a, d) = if status == "?" {
            (
                std::fs::read_to_string(wt.join(path))
                    .map(|t| t.lines().count() as u64)
                    .unwrap_or(0),
                0,
            )
        } else {
            counts.get(path).copied().unwrap_or((0, 0))
        };
        added += a;
        deleted += d;
        files.push(json!({"path": path, "status": status, "added": a, "deleted": d}));
    }
    Ok(
        json!({"task": "sample/task-000", "base": "HEAD", "merge_base": head, "files": files, "added": added, "deleted": deleted}),
    )
}
