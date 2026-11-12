//! Workers after the start: list, show and read them, stop (only once
//! the process tree is gone), keep, abandon, release (archive the output,
//! close the terminal), a person typing keeps one, and `orch.commit` in a
//! worker's own worktree.

use std::path::PathBuf;
use std::sync::Arc;

use pod_orch::engine::Orch;
use pod_orch::model::attempt_mailbox;

use super::ARCHIVE_BYTES;
use super::identity::who;
use super::rpc::oe;
use super::state::{Stopping, cached, remember};
use crate::protocol::*;
use crate::server::terminal::Terminal;
use crate::server::{Daemon, log, status};

impl Daemon {
    /// For mod events: an attempt's state word and its task.
    pub(in crate::server) fn orch_attempt_facts(
        &self,
        attempt: Option<&str>,
    ) -> (Option<String>, Option<String>) {
        let Some(id) = attempt else {
            return (None, None);
        };
        let o = self.orch();
        let Ok(a) = o.attempt(id) else {
            return (None, None);
        };
        let state = serde_json::to_value(a.state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string));
        (state, a.task.clone())
    }

    pub(super) fn terminal_by_id(&self, id: &str) -> Option<Arc<Terminal>> {
        self.terminals
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.id == id)
            .cloned()
    }

    pub(super) fn task_of_terminal(
        &self,
        terminal: &str,
    ) -> Option<crate::server::tasks::TaskRecord> {
        let reg = self.registry.lock().unwrap();
        reg.tasks
            .iter()
            .find(|t| t.terminals.iter().any(|x| x == terminal))
            .cloned()
    }

    pub(super) fn worker_info(&self, o: &Orch, a: &Attempt) -> WorkerInfo {
        let t = a.terminal.as_deref().and_then(|id| self.terminal_by_id(id));
        let status = t.as_ref().map(|t| t.status.info());
        let waiting = o.has_open_question(&a.id);
        let unread = o.unread(&attempt_mailbox(&a.id)).len();
        use pod_orch::text as tx;
        let next = match (a.state, a.worker) {
            (_, WorkerState::Released) => MsgText::new(tx::NEXT_RELEASED),
            (s, _) if s.is_open() && waiting => MsgText::new(tx::NEXT_WAITING_ANSWER),
            (pod_orch::AttemptState::Starting, _) if a.is_queued() => {
                let q = a.queue.as_ref().unwrap();
                MsgText::new(tx::NEXT_QUEUED)
                    .with("position", q.position)
                    .with("reason", &q.reason)
            }
            (pod_orch::AttemptState::Starting, _) => MsgText::new(tx::NEXT_STARTING),
            (s, _)
                if s.is_open()
                    && !a.waiting_on.is_empty()
                    && status.as_ref().is_none_or(|s| s.state != "working") =>
            {
                MsgText::new(tx::NEXT_WAITING_ITEMS).with("items", a.waiting_on.join(", "))
            }
            (s, _) if s.is_open() => match status.as_ref().map(|s| s.state.as_str()) {
                Some("blocked") => MsgText::new(tx::NEXT_BLOCKED),
                Some("working") => MsgText::new(tx::NEXT_WORKING),
                _ => MsgText::new(tx::NEXT_IDLE),
            },
            (_, WorkerState::Kept) => MsgText::new(tx::NEXT_KEPT),
            (_, WorkerState::Finished) => match a.release_at_ms {
                Some(ms) => MsgText::new(tx::NEXT_RELEASE_IN)
                    .with("secs", ms.saturating_sub(crate::now_ms()) / 1000),
                None => MsgText::new(tx::NEXT_FINISHED),
            },
            _ => MsgText::new(tx::NEXT_FINISHED),
        };
        WorkerInfo {
            attempt: a.public(),
            item_title: o.item(&a.item).map(|i| i.title.clone()).unwrap_or_default(),
            status,
            last_output_ms: t.as_ref().map(|t| t.last_output_ms()).unwrap_or(0),
            waiting_answer: waiting,
            unread,
            next: next.en(),
            next_text: Some(next),
            wake_unconfirmed_ms: self.runs.unconfirmed.since(o, &attempt_mailbox(&a.id)),
        }
    }

    pub(super) fn orch_worker_list(&self, p: OrchRunRef) -> Result<OrchWorkerListResult, RpcError> {
        let o = self.orch();
        let runs: Vec<String> = match p.run {
            Some(r) => vec![r],
            None if p.caller.is_empty() => o.runs().map(|r| r.rec.run.id.clone()).collect(),
            None => vec![self.caller_run(&o, None, &p.caller)?],
        };
        let mut workers = Vec::new();
        for r in runs {
            let st = o.run(&r).map_err(oe)?;
            for a in &st.rec.attempts {
                workers.push(self.worker_info(&o, a));
            }
        }
        Ok(OrchWorkerListResult { workers })
    }

    pub(super) fn orch_worker_show(&self, p: OrchAttemptRef) -> Result<WorkerInfo, RpcError> {
        let o = self.orch();
        let a = o.attempt(&p.attempt).map_err(oe)?;
        Ok(self.worker_info(&o, a))
    }

    pub(super) fn orch_worker_read(
        &self,
        p: OrchWorkerReadParams,
    ) -> Result<OrchWorkerReadResult, RpcError> {
        let a = self.orch().attempt(&p.attempt).map_err(oe)?.clone();
        let n = p.lines.unwrap_or(200);
        let tail = |mut v: Vec<String>| {
            let skip = v.len().saturating_sub(n);
            v.drain(..skip);
            v
        };
        if let Some(t) = a.terminal.as_deref().and_then(|t| self.terminal_by_id(t)) {
            let (lines, _, suggestion) = t.read_lines_for_agents(ReadMode::History);
            return Ok(OrchWorkerReadResult {
                attempt: a.id,
                source: "terminal".into(),
                lines: tail(lines),
                suggestion,
            });
        }
        if let Some(path) = &a.log_path {
            let text = std::fs::read_to_string(path)
                .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("{path}: {e}")))?;
            return Ok(OrchWorkerReadResult {
                attempt: a.id,
                source: "archive".into(),
                lines: tail(text.lines().map(str::to_string).collect()),
                suggestion: None,
            });
        }
        Err(RpcError::new(
            orch_codes::INVALID_STATE,
            format!("worker {} has no terminal and no archived output", a.id),
        ))
    }

    pub(super) async fn orch_worker_stop(&self, p: OrchAttemptRef) -> Result<WorkerInfo, RpcError> {
        let _ops = self.runs.ops.lock().await;
        if let Some(r) = cached(&self.orch(), p.request_id.as_deref(), OrchWorkerStop::NAME)? {
            return Ok(r);
        }
        let (a, w) = {
            let o = self.orch();
            (
                o.attempt(&p.attempt).map_err(oe)?.clone(),
                who(&o, &p.caller)?,
            )
        };
        if !a.state.is_open() {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!(
                    "attempt {} already ended ({:?}); use release to close its terminal",
                    a.id, a.state
                ),
            ));
        }
        // Stopped only once the process tree is proven gone.
        let _stopping = Stopping::new(&self.runs, &a.id);
        let gone = match a.terminal.as_deref() {
            Some(tid) => match self.terminal_by_id(tid) {
                Some(t) => {
                    self.kill_and_wait(&t).await;
                    !t.is_running()
                }
                // orch-quality-02: an unreachable holder may still run it.
                None => !self.holder_retrying(tid),
            },
            None => true,
        };
        let (state, why) = if gone {
            (
                pod_orch::AttemptState::Stopped,
                format!("stopped by {}", w.from),
            )
        } else {
            (
                pod_orch::AttemptState::Unknown,
                format!("stop by {} not confirmed: the process still runs", w.from),
            )
        };
        let info = {
            let mut o = self.orch();
            o.end_attempt(
                &a.id,
                state,
                &why,
                None,
                self.orch_release_ms(),
                crate::now_ms(),
            )
            .map_err(oe)?;
            let info = self.worker_info(&o, o.attempt(&a.id).map_err(oe)?);
            remember(&mut o, p.request_id.as_deref(), OrchWorkerStop::NAME, &info)?;
            info
        };
        log(&format!("orch: {} {why}", a.id));
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        Ok(info)
    }

    pub(super) async fn orch_worker_simple(
        &self,
        method: &'static str,
        p: OrchAttemptRef,
    ) -> Result<WorkerInfo, RpcError> {
        let _ops = self.runs.ops.lock().await;
        if let Some(r) = cached(&self.orch(), p.request_id.as_deref(), method)? {
            return Ok(r);
        }
        let (a, w) = {
            let o = self.orch();
            (
                o.attempt(&p.attempt).map_err(oe)?.clone(),
                who(&o, &p.caller)?,
            )
        };
        let now = crate::now_ms();
        if method == OrchWorkerAbandon::NAME {
            let mut o = self.orch();
            if !o
                .end_attempt(
                    &a.id,
                    pod_orch::AttemptState::Abandoned,
                    &format!("abandoned by {}", w.from),
                    Some(WorkerState::Kept),
                    None,
                    now,
                )
                .map_err(oe)?
            {
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!("attempt {} already ended", a.id),
                ));
            }
        } else if method == OrchWorkerKeep::NAME {
            if a.worker == WorkerState::Released {
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!("worker {} was already released", a.id),
                ));
            }
            self.orch()
                .set_worker(&a.id, WorkerState::Kept, None, now)
                .map_err(oe)?;
        } else {
            // release
            if a.worker == WorkerState::Released {
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!("worker {} was already released", a.id),
                ));
            }
            if a.state.is_open() {
                self.orch()
                    .end_attempt(
                        &a.id,
                        pod_orch::AttemptState::Abandoned,
                        &format!("released by {}", w.from),
                        None,
                        None,
                        now,
                    )
                    .map_err(oe)?;
            }
            self.release_worker(&a.id).await?;
        }
        let mut o = self.orch();
        let info = self.worker_info(&o, o.attempt(&a.id).map_err(oe)?);
        remember(&mut o, p.request_id.as_deref(), method, &info)?;
        log(&format!("orch: {method} {} by {}", a.id, w.from));
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        Ok(info)
    }

    /// Archive the last output, close the terminal (its Job Object ends the
    /// process tree), keep the task and worktree.
    pub(super) async fn release_worker(&self, attempt: &str) -> Result<Attempt, RpcError> {
        let a = self.orch().attempt(attempt).map_err(oe)?.clone();
        let mut log_path = None;
        if let Some(t) = a.terminal.as_deref().and_then(|t| self.terminal_by_id(t)) {
            let (lines, _) = t.read_lines(ReadMode::History);
            let mut text = lines.join("\n");
            if text.len() > ARCHIVE_BYTES {
                let mut cut = text.len() - ARCHIVE_BYTES;
                while !text.is_char_boundary(cut) {
                    cut += 1;
                }
                text.drain(..cut);
            }
            let path = self
                .runs
                .engine
                .lock()
                .unwrap()
                .root()
                .join(&a.run)
                .join(format!("{}.log", a.id));
            match std::fs::write(&path, text) {
                Ok(()) => log_path = Some(path.display().to_string()),
                Err(e) => log(&format!("orch: cannot archive {}: {e}", a.id)),
            }
            self.rm(RmParams {
                target: TerminalRef::id(t.id.clone()),
                force: true,
            })
            .await?;
        }
        let a = self
            .orch()
            .set_worker(
                attempt,
                WorkerState::Released,
                log_path.as_deref(),
                crate::now_ms(),
            )
            .map_err(oe)?;
        log(&format!("orch: released worker {attempt}"));
        status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        Ok(a)
    }

    /// `orch.commit` (docs/19 §18.1, §18.6): stage and commit in the calling
    /// worker's own task worktree. podd runs git itself (plumbing only;
    /// hooks, fsmonitor and repository filter programs never run, §18.5),
    /// so a sandboxed worker can commit without being allowed an
    /// unsandboxed `git`, which would read a config it wrote.
    pub(super) async fn orch_commit(
        &self,
        p: OrchCommitParams,
    ) -> Result<OrchCommitResult, RpcError> {
        let Some(attempt) = p.caller.attempt.clone() else {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "commit is for workers (POD_ATTEMPT_ID is not set); use git",
            ));
        };
        let a = self
            .orch()
            .verify_token(&attempt, p.caller.token.as_deref())
            .map_err(oe)?
            .clone();
        if !a.state.is_open() {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!("worker {attempt} has ended"),
            ));
        }
        if p.message.trim().is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "the commit message must not be empty (-m)",
            ));
        }
        if !p.all && p.paths.is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "say what to commit: --all or paths",
            ));
        }
        let Some(task) = a.task.clone() else {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!(
                    "worker {attempt} has no task worktree of its own; commit with git (with approval)"
                ),
            ));
        };
        let rec = self.task_record(&task)?;
        let wt = PathBuf::from(&rec.worktree);
        // TM-21: the worktree must still be a linked worktree of the task's
        // repository (the fixed root), not whatever its `.git` names now.
        let repo = self
            .registry
            .lock()
            .unwrap()
            .repos
            .iter()
            .find(|r| r.name == rec.repo)
            .map(|r| PathBuf::from(&r.path));
        let state = self.config.state_dir.clone();
        let (msg, all, paths) = (p.message.clone(), p.all, p.paths.clone());
        let r = crate::server::tasks::blocking(move || {
            // Only a hooks folder this podd created (docs/19 §21.2).
            let no_hooks = super::launch::no_hooks_dir(&state)?;
            match &repo {
                Some(repo) => {
                    crate::git::commit_for_task_worker(&wt, repo, &no_hooks, &msg, all, &paths)
                }
                None => Err("the task's repository is not registered any more; podd does not commit for workers outside one. Commit with git (with approval) or ask a human".to_string()),
            }
        })
        .await?;
        let (commit, summary) = r.map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
        log(&format!(
            "orch: {attempt} committed {commit} in {task} (no hooks, no git filters)"
        ));
        Ok(OrchCommitResult {
            attempt,
            task,
            next: commit_next(&commit),
            commit,
            summary,
        })
    }

    /// A person typed into `terminal` (attach input): its workers become kept.
    pub(in crate::server) fn orch_on_human_input(&self, terminal: &str) {
        let mut o = self.orch();
        let ids: Vec<(String, String, String)> = o
            .attempts()
            .filter(|a| {
                a.terminal.as_deref() == Some(terminal)
                    && matches!(a.worker, WorkerState::Active | WorkerState::Finished)
            })
            .map(|a| (a.id.clone(), a.run.clone(), a.item.clone()))
            .collect();
        for (id, run, item) in ids {
            if o.set_worker(&id, WorkerState::Kept, None, crate::now_ms())
                .is_ok()
            {
                log(&format!(
                    "orch: a person typed into {terminal}: worker {id} is kept"
                ));
                status::publish_orch_changed(&run, Some(&item), Some(&id));
            }
        }
    }
}

/// The `next` line of `orch.commit` (TM-21, docs/19 §18.6): the commit ran
/// no repository hook (pre-commit checks included) and no git filter (LFS
/// paths are refused, not stored unconverted), and it is neither a merge
/// nor a passing check.
fn commit_next(commit: &str) -> String {
    format!(
        "Committed {} on your branch. Pod ran no repository hooks (pre-commit checks did not run) and no git filters such as LFS (paths that need one are refused): this commit is not a merge and not a passing check. Report with `pod orch done` when the work is finished.",
        &commit[..commit.len().min(12)]
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_worker_commit_says_hooks_and_filters_were_skipped() {
        let n = super::commit_next("0123456789abcdef0123");
        assert!(
            n.starts_with("Committed 0123456789ab on your branch."),
            "{n}"
        );
        for word in [
            "no repository hooks",
            "pre-commit",
            "LFS",
            "not a merge",
            "not a passing check",
            "pod orch done",
        ] {
            assert!(n.contains(word), "{word}: {n}");
        }
    }
}
