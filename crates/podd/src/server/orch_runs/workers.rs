//! Workers after the start: list, show and read them, stop (only once
//! the process tree is gone), keep, abandon, release (archive the output,
//! close the terminal), a person typing keeps one, and `orch.commit` in a
//! worker's own worktree.

use std::path::PathBuf;
use std::sync::Arc;

use pod_orch::engine::Orch;
use pod_orch::model::attempt_mailbox;

use super::ARCHIVE_BYTES;
use super::OrchRuns;
use super::identity::who;
use super::rpc::oe;
use super::state::{Stopping, cached, remember};
use crate::protocol::*;
use crate::server::terminal::Terminal;
use crate::server::{Daemon, log, status};

#[derive(Default)]
pub(super) struct Releases {
    claimed: std::collections::HashSet<String>,
    failures: std::collections::HashMap<String, ReleaseFailure>,
}

struct ReleaseFailure {
    message: String,
    retry_at_ms: u64,
}

/// An in-memory claim is the release linearization point. Before it, keep/input
/// wins under the engine lock; afterwards keep fails and input cannot promise
/// preservation. Dropping on every exit (including errors/cancellation) permits
/// a later keep or retry. No state lock spans terminal or filesystem operations.
struct ReleaseClaim<'a> {
    runs: &'a OrchRuns,
    attempt: String,
    active: bool,
}

impl Drop for ReleaseClaim<'_> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        self.runs
            .releases
            .lock()
            .unwrap()
            .claimed
            .remove(&self.attempt);
    }
}

impl OrchRuns {
    // The caller holds the engine lock, shared with release admission.
    fn ensure_not_releasing(&self, attempt: &str) -> Result<(), RpcError> {
        if self.releases.lock().unwrap().claimed.contains(attempt) {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!("worker {attempt} release already claimed; retry if release fails"),
            ));
        }
        Ok(())
    }

    fn claim_release(
        &self,
        attempt: &str,
        automatic: bool,
    ) -> Result<Option<(ReleaseClaim<'_>, Attempt)>, RpcError> {
        let o = self.engine.lock().unwrap();
        let a = o.attempt(attempt).map_err(oe)?;
        // Selection by the supervisor is only a hint: keep/input may have won
        // since then, or another release may already have removed the terminal.
        if automatic
            && (a.worker != WorkerState::Finished
                || !a.release_at_ms.is_some_and(|at| at <= crate::now_ms()))
        {
            return Ok(None);
        }
        if automatic
            && self
                .releases
                .lock()
                .unwrap()
                .failures
                .get(attempt)
                .is_some_and(|failure| crate::now_ms() < failure.retry_at_ms)
        {
            return Ok(None);
        }
        if a.worker == WorkerState::Released {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!("worker {attempt} was already released"),
            ));
        }
        self.ensure_not_releasing(attempt)?;
        self.releases
            .lock()
            .unwrap()
            .claimed
            .insert(attempt.to_string());
        Ok(Some((
            ReleaseClaim {
                runs: self,
                attempt: attempt.to_string(),
                active: true,
            },
            a.clone(),
        )))
    }
}

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
        let failure = self
            .runs
            .releases
            .lock()
            .unwrap()
            .failures
            .get(&a.id)
            .map(|f| f.message.clone());
        let (next, next_text) = match failure {
            Some(error) => (
                format!(
                    "Release failed: {error}. Resources retained where available; automatic release will retry if still due, or keep/retry release manually."
                ),
                None,
            ),
            None => (next.en(), Some(next)),
        };
        WorkerInfo {
            attempt: a.public(),
            item_title: o.item(&a.item).map(|i| i.title.clone()).unwrap_or_default(),
            status,
            last_output_ms: t.as_ref().map(|t| t.last_output_ms()).unwrap_or(0),
            waiting_answer: waiting,
            unread,
            next,
            next_text,
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
            let mut o = self.orch();
            self.runs.ensure_not_releasing(&a.id)?;
            if o.attempt(&a.id).map_err(oe)?.worker == WorkerState::Released {
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!("worker {} was already released", a.id),
                ));
            }
            o.set_worker(&a.id, WorkerState::Kept, None, now)
                .map_err(oe)?;
            self.runs.releases.lock().unwrap().failures.remove(&a.id);
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
            self.release_worker(&a.id, false).await?;
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
    pub(super) async fn release_worker(
        &self,
        attempt: &str,
        automatic: bool,
    ) -> Result<Option<Attempt>, RpcError> {
        let Some((mut claim, a)) = self.runs.claim_release(attempt, automatic)? else {
            return Ok(None);
        };
        #[cfg(debug_assertions)]
        if automatic {
            release_test_gate("claimed").await;
        }
        let result = self.release_claimed_worker(&a, automatic).await;
        let changed = {
            let mut releases = self.runs.releases.lock().unwrap();
            // Publish the error and free the claim atomically. A worker.show
            // caller observing the error can immediately issue keep/retry.
            releases.claimed.remove(attempt);
            claim.active = false;
            match &result {
                Ok(_) => releases.failures.remove(attempt).is_some(),
                Err(error) => {
                    let changed = releases
                        .failures
                        .get(attempt)
                        .is_none_or(|f| f.message != error.message);
                    releases.failures.insert(
                        attempt.to_string(),
                        ReleaseFailure {
                            message: error.message.clone(),
                            retry_at_ms: crate::now_ms().saturating_add(1_000),
                        },
                    );
                    changed
                }
            }
        };
        // Release the claim before announcing failure: a caller reacting to
        // the event can keep/retry immediately. Unwinding/cancellation also
        // drops it, without requiring a successful persistence operation.
        drop(claim);
        if changed {
            status::publish_orch_changed(&a.run, Some(&a.item), Some(&a.id));
        }
        result.map(Some)
    }

    async fn release_claimed_worker(
        &self,
        a: &Attempt,
        automatic: bool,
    ) -> Result<Attempt, RpcError> {
        let attempt = &a.id;
        let mut log_path = a.log_path.clone();
        if let Some(t) = a.terminal.as_deref().and_then(|t| self.terminal_by_id(t)) {
            if automatic && t.incompatible.is_some() {
                return Err(RpcError::new(
                    codes::INTERNAL_ERROR,
                    "cannot archive output from an incompatible holder; use explicit release only if discarding its output is intended",
                ));
            }
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
            let archive_path = path.display().to_string();
            // No engine/config/lifecycle mutex is held during disk I/O.
            // Keep the write within this claim's lifetime, including when the
            // release future is cancelled (no detached writer can outlive it).
            let write = std::fs::write(&path, text);
            match write {
                Ok(()) => {
                    log_path = Some(archive_path.clone());
                    // Remember the archive BEFORE removal, so a failed remove
                    // or a retry after terminal disappearance still finds it.
                    self.orch()
                        .update_attempt(attempt, |a| a.log_path = Some(archive_path))
                        .map_err(oe)?;
                }
                Err(e) if automatic => {
                    return Err(RpcError::new(
                        codes::INTERNAL_ERROR,
                        format!("cannot archive {}: {e}; terminal preserved", a.id),
                    ));
                }
                // `orch.worker.release` is an explicit request to force-close
                // resources; preserve that existing discard-on-error intent.
                Err(e) => log(&format!(
                    "orch: explicit release {} proceeds without archive: {e}",
                    a.id
                )),
            }
            self.rm(RmParams {
                target: TerminalRef::id(t.id.clone()),
                force: true,
            })
            .await?;
            #[cfg(debug_assertions)]
            if automatic {
                release_test_gate("removed").await;
            }
        } else if automatic
            && a.terminal
                .as_deref()
                .is_some_and(|tid| self.holder_retrying(tid))
        {
            return Err(RpcError::new(
                codes::INTERNAL_ERROR,
                "holder unreachable; archive and release will retry after reconnection",
            ));
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
            if self.runs.ensure_not_releasing(&id).is_err() {
                log(&format!(
                    "orch: input into {terminal} arrived after release claim for {id}"
                ));
                continue;
            }
            if o.set_worker(&id, WorkerState::Kept, None, crate::now_ms())
                .is_ok()
            {
                self.runs.releases.lock().unwrap().failures.remove(&id);
                log(&format!(
                    "orch: a person typed into {terminal}: worker {id} is kept"
                ));
                status::publish_orch_changed(&run, Some(&item), Some(&id));
            }
        }
    }
}

/// Debug integration-test rendezvous, absent from release builds. File signals
/// force selection/claim -> competing request -> continuation; elapsed time is
/// only a bounded failure escape, never the ordering assertion.
#[cfg(debug_assertions)]
pub(super) async fn release_test_gate(phase: &str) {
    let Some(dir) = std::env::var_os("POD_TEST_ORCH_RELEASE_GATE") else {
        return;
    };
    let dir = PathBuf::from(dir);
    if std::fs::remove_file(dir.join(format!("{phase}.armed"))).is_err() {
        return;
    }
    if std::fs::write(dir.join(format!("{phase}.arrived")), b"ready").is_err() {
        return;
    }
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    while !dir.join(format!("{phase}.proceed")).exists() {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
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
    use super::*;

    #[test]
    fn dropping_release_claim_allows_keep_and_retry_without_state_locks() {
        let dir = std::env::temp_dir().join(format!(
            "podd-release-claim-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let runs = OrchRuns::load(&dir);
        {
            let mut o = runs.engine.lock().unwrap();
            o.create_run("release test", Some("t1"), None, 1).unwrap();
            o.add_item("r1", None, "test worker", &[], None, 1).unwrap();
            o.begin_attempt(
                pod_orch::engine::BeginAttempt {
                    run: "r1".into(),
                    item: "i1".into(),
                    agent: "test".into(),
                    place: "current".into(),
                    retry_of: None,
                    token_hash: "test".into(),
                },
                1,
            )
            .unwrap();
            o.end_attempt("a1", AttemptState::Stopped, "finished", None, Some(0), 2)
                .unwrap();
        }
        let (claim, _) = runs.claim_release("a1", true).unwrap().unwrap();
        {
            let _engine = runs
                .engine
                .try_lock()
                .expect("claim must not hold the engine during I/O");
            assert!(runs.config.try_lock().is_ok());
            assert!(runs.ensure_not_releasing("a1").is_err());
        }
        // Error unwinding and cancelled futures drop exactly this guard.
        drop(claim);
        {
            let _engine = runs.engine.lock().unwrap();
            runs.ensure_not_releasing("a1").unwrap();
        }
        let (retry, _) = runs.claim_release("a1", true).unwrap().unwrap();
        drop(retry);
        {
            let mut o = runs.engine.lock().unwrap();
            runs.ensure_not_releasing("a1").unwrap();
            o.set_worker("a1", WorkerState::Kept, None, 3).unwrap();
        }
        assert!(runs.claim_release("a1", true).unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

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
