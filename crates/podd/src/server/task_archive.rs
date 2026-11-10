//! Archive, cleanup and resume (docs/19 §3.5–§3.6, S3).
//!
//! - `task.archive`: run the archive script first (failure = nothing
//!   changes), then close the task's terminals. Worktree, branch and the
//!   recorded agent session stay; `task.unarchive` / `task.resume` re-open.
//! - `task.resume`: a new agent terminal running the agent's own resume
//!   command for the recorded (or given) session id.
//! - `task.cleanup_candidates`: protected / review / ready tiers.
//! - `task.remove` guards: locked or nested worktrees are refused, and the
//!   branch is deleted only when it is proven merged into the base.

use std::path::{Path, PathBuf};

use super::repo_config;
use super::sessions::{self, resume_argv};
use super::tasks::{TaskRecord, blocking};
use super::{Daemon, log};
use crate::git;
use crate::protocol::*;

const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const DEFAULT_INACTIVE_DAYS: u32 = 7;

/// Running agent states that make a task busy.
fn busy_state(s: &str) -> bool {
    matches!(s, "working" | "blocked")
}

/// Why `task.remove` must not touch `wt`: it is locked, or another
/// registered worktree lives inside it (removing it would delete that one).
pub fn remove_blocker(entries: &[git::WorktreeEntry], wt: &Path) -> Option<String> {
    if entries
        .iter()
        .any(|e| e.locked && git::same_path(&e.path, wt))
    {
        return Some(format!(
            "worktree {} is locked (git worktree unlock first)",
            wt.display()
        ));
    }
    let inner = entries
        .iter()
        .find(|e| !git::same_path(&e.path, wt) && git::is_inside(&e.path, wt))?;
    Some(format!(
        "another worktree ({}) is inside {}; remove it first",
        inner.path.display(),
        wt.display()
    ))
}

/// Whether `delete_branch` may delete `branch`: never a branch that existed
/// before the task; otherwise only with proof that its *current* tip is
/// merged (orch-04): the tip is reachable from the base, or it is exactly
/// the head commit of the merged pull request. Commits added after the
/// merge keep the branch.
pub fn branch_delete_refusal(repo: &Path, rec: &TaskRecord) -> Option<String> {
    if rec.branch_existed {
        return Some(format!("{} existed before the task; kept", rec.branch));
    }
    if !git::branch_exists(repo, &rec.branch) {
        return None;
    }
    let tip_ref = format!("refs/heads/{}", rec.branch);
    let base = git::base_rev(repo, &rec.base, &rec.base_commit);
    if git::is_ancestor(repo, &tip_ref, &base) {
        return None;
    }
    let merged = rec
        .pr
        .as_ref()
        .filter(|p| p.state == pod_forge::PrState::Merged);
    if let Some(pr) = merged {
        let tip = git::resolve_commit(repo, &tip_ref);
        if pr.head_oid.is_some() && pr.head_oid == tip {
            return None;
        }
        return Some(match &pr.head_oid {
            Some(h) => format!(
                "{} has commits after pull request #{} was merged (merged {}, now {}); kept",
                rec.branch,
                pr.number,
                short(h),
                tip.as_deref().map(short).unwrap_or("?")
            ),
            None => format!(
                "{} is not merged into {} and the merged commit of pull request #{} is unknown; kept (delete it yourself with git branch -D if you are sure)",
                rec.branch, rec.base, pr.number
            ),
        });
    }
    Some(format!(
        "{} is not merged into {}; kept (delete it yourself with git branch -D if you are sure)",
        rec.branch, rec.base
    ))
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(9)]
}

impl Daemon {
    fn busy_terminal(&self, rec: &TaskRecord) -> Option<(String, String)> {
        let terms = self.terminals.lock().unwrap();
        rec.terminals.iter().find_map(|id| {
            let t = terms.iter().find(|t| &t.id == id)?;
            if !t.is_running() {
                return None;
            }
            let st = t.info().status?.state;
            busy_state(&st).then(|| (id.clone(), st))
        })
    }

    pub(super) async fn task_archive(
        &self,
        p: TaskArchiveParams,
    ) -> Result<TaskArchiveResult, RpcError> {
        let _op = self.task_ops.lock().await;
        let rec = self.task_record(&p.task)?;
        if rec.lifecycle == Lifecycle::Archived {
            return Ok(TaskArchiveResult {
                task: rec.id,
                terminals: Vec::new(),
                script: None,
            });
        }
        if !p.force
            && let Some((terminal, state)) = self.busy_terminal(&rec)
        {
            let mut e = RpcError::new(
                session_codes::TASK_BUSY,
                format!(
                    "task {}: {terminal} is {state}; archive with force to stop it",
                    rec.id
                ),
            );
            e.data = Some(serde_json::json!({"terminal": terminal, "state": state}));
            return Err(e);
        }

        // 1. The archive script, before anything changes.
        let repo = self.repo_record(&rec.repo)?;
        let wt = PathBuf::from(&rec.worktree);
        let script = if wt.is_dir() {
            let w = wt.clone();
            let loaded = blocking(move || repo_config::load(&w)).await?;
            let eff = repo_config::effective(&loaded, &repo);
            match eff.archive.clone() {
                None => None,
                Some(c) if eff.uses_untrusted(&c) => {
                    let why = format!(
                        "the archive script of {} is not approved (hash {})",
                        loaded.path.display(),
                        eff.hash.as_deref().unwrap_or("-")
                    );
                    if !p.ignore_script_failure {
                        let mut e = RpcError::new(
                            session_codes::ARCHIVE_FAILED,
                            format!("{why}; nothing changed"),
                        );
                        e.data = Some(serde_json::json!({"hash": eff.hash, "output": ""}));
                        return Err(e);
                    }
                    Some(ScriptRun {
                        ok: false,
                        exit_code: None,
                        output: String::new(),
                        skipped: Some(why),
                    })
                }
                Some(c) => {
                    let env = self.terminal_env("archive", Self::task_env(&rec));
                    let (cmd, w) = (c.command.clone(), wt.clone());
                    let run = blocking(move || {
                        repo_config::run_script(&cmd, &w, &env, repo_config::script_timeout())
                    })
                    .await?;
                    if !run.ok && !p.ignore_script_failure {
                        let mut e = RpcError::new(
                            session_codes::ARCHIVE_FAILED,
                            format!(
                                "task {}: archive script failed (exit {}); nothing changed",
                                rec.id,
                                run.exit_code
                                    .map(|c| c.to_string())
                                    .unwrap_or_else(|| "-".into())
                            ),
                        );
                        e.data = Some(
                            serde_json::json!({"exit_code": run.exit_code, "output": run.output}),
                        );
                        return Err(e);
                    }
                    Some(run)
                }
            }
        } else {
            None
        };

        // 2. Close the terminals (agents, language servers: memory back).
        let mut closed = Vec::new();
        for tid in &rec.terminals {
            let known = self.terminals.lock().unwrap().iter().any(|t| &t.id == tid);
            if known {
                self.rm(RmParams {
                    target: TerminalRef::id(tid.clone()),
                    force: true,
                })
                .await?;
                closed.push(tid.clone());
            }
        }
        self.update_task(&rec.id, |t| {
            t.lifecycle = Lifecycle::Archived;
            t.archived_ms = Some(crate::now_ms());
            true
        });
        log(&format!("task {}: archived (terminals {closed:?})", rec.id));
        Ok(TaskArchiveResult {
            task: rec.id,
            terminals: closed,
            script,
        })
    }

    pub(super) async fn task_unarchive(
        &self,
        p: TaskUnarchiveParams,
    ) -> Result<TaskResumeResult, RpcError> {
        let rec = self.task_record(&p.task)?;
        self.update_task(&rec.id, |t| {
            let was = t.lifecycle;
            t.lifecycle = Lifecycle::Active;
            t.archived_ms = None;
            was != Lifecycle::Active
        });
        if p.start_agent == Some(false) {
            return Ok(TaskResumeResult {
                task: rec.id,
                terminal: None,
                argv: Vec::new(),
                resumed_session: None,
            });
        }
        self.task_resume(TaskResumeParams {
            task: rec.id,
            cols: p.cols,
            rows: p.rows,
            ..Default::default()
        })
        .await
    }

    /// A new agent terminal: the agent's own resume command for the session,
    /// or a fresh agent when none is known.
    pub(super) async fn task_resume(
        &self,
        p: TaskResumeParams,
    ) -> Result<TaskResumeResult, RpcError> {
        let _op = self.task_ops.lock().await;
        let rec = self.task_record(&p.task)?;
        let wt = PathBuf::from(&rec.worktree);
        if !wt.is_dir() {
            return Err(RpcError::new(
                codes::GIT_FAILED,
                format!("worktree {} is missing", wt.display()),
            ));
        }
        // The old agent terminal: refuse while it runs, else forget it.
        if let Some(old) = rec.agent_terminal_id().map(str::to_string) {
            let t = self
                .terminals
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.id == old)
                .cloned();
            if let Some(t) = t {
                if t.is_running() {
                    return Err(RpcError::new(
                        codes::TERMINAL_RUNNING,
                        format!("task {}: agent terminal {old} is still running", rec.id),
                    ));
                }
                self.rm(RmParams {
                    target: TerminalRef::id(old.clone()),
                    force: false,
                })
                .await?;
            }
        }
        let recorded = rec.agent_session.clone();
        let session = p
            .session
            .clone()
            .or_else(|| recorded.as_ref().map(|s| s.id.clone()));
        let (argv, resumed) = match session {
            Some(id) => {
                let agent = p
                    .agent
                    .clone()
                    .or_else(|| recorded.as_ref().map(|s| s.agent.clone()))
                    .unwrap_or_else(|| rec.agent.to_string());
                (resume_argv(&agent, &id, &wt)?, Some((agent, id)))
            }
            None => {
                let argv = super::tasks::agent_argv(&rec.agent, None)?;
                (
                    super::agent_launch::task_agent_argv(&rec.agent, argv, &wt),
                    None,
                )
            }
        };
        let info = self
            .run(RunParams {
                argv: argv.clone(),
                cwd: Some(rec.worktree.clone()),
                name: Some(rec.id.clone()),
                env: Self::task_env(&rec),
                cols: p.cols,
                rows: p.rows,
                ..Default::default()
            })
            .await?;
        let tid = info.id.clone();
        self.update_task(&rec.id, |t| {
            t.terminals.retain(|x| x != &tid);
            t.terminals.insert(0, tid.clone());
            t.agent_terminal = Some(tid.clone());
            t.lifecycle = Lifecycle::Active;
            t.archived_ms = None;
            if let (Some((agent, id)), true) = (&resumed, p.session.is_some()) {
                t.agent_session = Some(AgentSession {
                    agent: agent.clone(),
                    id: id.clone(),
                    captured_ms: crate::now_ms(),
                    source: "user".into(),
                });
            }
            // A hook of the new terminal that came before this update.
            if let Some(s) = self
                .early_sessions
                .lock()
                .unwrap()
                .take(&tid, crate::now_ms())
            {
                t.agent_session = Some(s);
            }
            true
        });
        self.task_terminals_changed(&rec.id);
        log(&format!(
            "task {}: agent {} in {tid} ({argv:?})",
            rec.id,
            if resumed.is_some() {
                "resumed"
            } else {
                "restarted"
            }
        ));
        Ok(TaskResumeResult {
            task: rec.id,
            terminal: Some(info),
            argv,
            resumed_session: resumed.map(|(_, id)| id),
        })
    }

    pub(super) async fn task_cleanup_candidates(
        &self,
        p: CleanupParams,
    ) -> Result<CleanupResult, RpcError> {
        let list = self
            .task_list(TaskListParams {
                repo: p.repo.clone(),
            })
            .await?
            .tasks;
        let days = p.inactive_days.unwrap_or(DEFAULT_INACTIVE_DAYS) as u64;
        let now = crate::now_ms();
        let mut out = Vec::with_capacity(list.len());
        for t in list {
            let rec = self.task_record(&t.id).ok();
            let wt = PathBuf::from(&t.worktree);
            let repo_path = self
                .repo_record(&t.repo)
                .ok()
                .map(|r| PathBuf::from(r.path));
            let (locked, base_ok) = match &repo_path {
                Some(rp) if t.worktree_exists => {
                    let (rp, w, base) = (rp.clone(), wt.clone(), t.base.clone());
                    blocking(move || {
                        let entries = git::worktree_list(&rp).unwrap_or_default();
                        (
                            remove_blocker(&entries, &w),
                            git::resolve_commit(&rp, &base).is_some(),
                        )
                    })
                    .await?
                }
                _ => (None, true),
            };
            let terminal_activity = t
                .terminals
                .iter()
                .filter_map(|x| x.info.as_ref())
                .map(|i| i.exited_ms.unwrap_or(i.created_ms).max(i.created_ms))
                .max()
                .unwrap_or(0);
            let last_activity_ms = t
                .git
                .as_ref()
                .and_then(|g| g.last_commit.as_ref())
                .map(|c| c.time_ms)
                .unwrap_or(0)
                .max(t.created_ms)
                .max(terminal_activity)
                .max(rec.as_ref().and_then(|r| r.archived_ms).unwrap_or(0));
            out.push(classify(&t, locked, base_ok, last_activity_ms, now, days));
        }
        out.sort_by(|a, b| b.tier.cmp(&a.tier).then(a.task.cmp(&b.task)));
        Ok(CleanupResult { candidates: out })
    }
}

/// The tier of one task. Protected wins over review, review over ready.
pub fn classify(
    t: &TaskInfo,
    locked: Option<String>,
    base_ok: bool,
    last_activity_ms: u64,
    now: u64,
    inactive_days: u64,
) -> CleanupCandidate {
    let mut protect = Vec::new();
    let mut review = Vec::new();
    let mut ready = Vec::new();
    for term in &t.terminals {
        if let Some(i) = &term.info
            && i.state == TerminalState::Running
            && let Some(s) = &i.status
            && busy_state(&s.state)
        {
            protect.push(format!("agent {} is {}", term.id, s.state));
        }
    }
    if let Some(l) = locked {
        protect.push(l);
    }
    if t.setup
        .as_ref()
        .is_some_and(|s| s.state == SetupState::Running)
    {
        protect.push("setup is running".into());
    }
    match &t.git {
        None => protect.push(format!(
            "state unknown: {}",
            t.git_error.as_deref().unwrap_or("no git facts")
        )),
        Some(g) => {
            if g.dirty > 0 {
                review.push(format!("{} uncommitted change(s)", g.dirty));
            }
            if g.unpushed > 0 {
                review.push(format!("{} unpushed commit(s)", g.unpushed));
            }
        }
    }
    if !base_ok {
        review.push(format!("base {} no longer exists", t.base));
    }
    let merged_pr =
        t.pr.as_ref()
            .is_some_and(|p| p.state == pod_forge::PrState::Merged);
    if merged_pr {
        ready.push("pull request merged".into());
    }
    if t.lifecycle == Lifecycle::Archived {
        ready.push("archived".into());
    }
    let idle_days = now.saturating_sub(last_activity_ms) / DAY_MS;
    if idle_days >= inactive_days {
        ready.push(format!("inactive for {idle_days} day(s)"));
    }
    let (tier, reasons) = if !protect.is_empty() {
        (CleanupTier::Protected, protect)
    } else if !review.is_empty() {
        (CleanupTier::Review, review)
    } else if !ready.is_empty() {
        (CleanupTier::Ready, ready)
    } else {
        (CleanupTier::Protected, vec!["recently active".to_string()])
    };
    CleanupCandidate {
        task: t.id.clone(),
        tier,
        reasons,
        lifecycle: t.lifecycle,
        last_activity_ms,
    }
}

pub(super) fn retire(d: &Daemon, rec: &TaskRecord) {
    sessions::Retired::retire(&d.config.state_dir, &rec.repo, &rec.name);
    if let Some(b) = &rec.brief {
        let _ = std::fs::remove_file(&b.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(p: &str, locked: bool) -> git::WorktreeEntry {
        git::WorktreeEntry {
            path: PathBuf::from(p),
            locked,
            ..Default::default()
        }
    }

    #[test]
    fn locked_and_nested_worktrees_block_removal() {
        let wt = Path::new(r"C:\wt\app\oak");
        let main = entry(r"C:\src\app", false);
        assert!(remove_blocker(&[main.clone(), entry(r"C:\wt\app\oak", false)], wt).is_none());
        let r = remove_blocker(&[main.clone(), entry(r"c:/WT/app/oak/", true)], wt).unwrap();
        assert!(r.contains("locked"), "{r}");
        let r = remove_blocker(
            &[
                main.clone(),
                entry(r"C:\wt\app\oak", false),
                entry(r"C:\wt\app\oak\sub\x", false),
            ],
            wt,
        )
        .unwrap();
        assert!(r.contains("inside"), "{r}");
        // A sibling whose name only starts the same is not nested.
        assert!(remove_blocker(&[main, entry(r"C:\wt\app\oak-2", false)], wt).is_none());
    }

    fn task(dirty: u32, unpushed: u32) -> TaskInfo {
        serde_json::from_value(serde_json::json!({
            "id": "app/oak", "repo": "app", "name": "oak", "branch": "pod/oak", "base": "main",
            "base_commit": "abc", "worktree": "C:\\wt\\app\\oak", "agent": "shell", "created_ms": 0,
            "terminals": [], "worktree_exists": true,
            "git": {"ahead": 0, "behind": 0, "dirty": dirty, "unpushed": unpushed}
        }))
        .unwrap()
    }

    #[test]
    fn three_tiers() {
        let now = 30 * DAY_MS;
        let old = 0;
        let recent = now - DAY_MS;
        let c = classify(&task(0, 0), None, true, old, now, 7);
        assert_eq!(c.tier, CleanupTier::Ready, "{c:?}");
        let c = classify(&task(0, 0), None, true, recent, now, 7);
        assert_eq!(c.tier, CleanupTier::Protected, "recent: {c:?}");
        let c = classify(&task(2, 0), None, true, old, now, 7);
        assert_eq!(
            (c.tier, c.reasons[0].as_str()),
            (CleanupTier::Review, "2 uncommitted change(s)")
        );
        let c = classify(&task(0, 1), None, true, old, now, 7);
        assert_eq!(c.tier, CleanupTier::Review);
        let c = classify(&task(0, 0), None, false, old, now, 7);
        assert_eq!(c.tier, CleanupTier::Review, "base gone");
        let c = classify(&task(0, 0), Some("locked".into()), true, old, now, 7);
        assert_eq!(c.tier, CleanupTier::Protected);
        let mut unknown = task(0, 0);
        unknown.git = None;
        assert_eq!(
            classify(&unknown, None, true, old, now, 7).tier,
            CleanupTier::Protected
        );
        let mut archived = task(0, 0);
        archived.lifecycle = Lifecycle::Archived;
        let c = classify(&archived, None, true, recent, now, 7);
        assert_eq!(
            (c.tier, c.reasons[0].as_str()),
            (CleanupTier::Ready, "archived")
        );
        let mut busy = task(3, 0);
        let mut info = TerminalInfo::new(
            "t1",
            vec!["claude".into()],
            "C:\\",
            TerminalState::Running,
            80,
            24,
            0,
        );
        info.status = serde_json::from_value(serde_json::json!({
            "state": "working", "confidence": 1.0, "since": 0, "turn": 0, "badge": "working"
        }))
        .ok();
        busy.terminals.push(TaskTerminal {
            id: "t1".into(),
            info: Some(info),
        });
        let c = classify(&busy, None, true, old, now, 7);
        assert_eq!(
            c.tier,
            CleanupTier::Protected,
            "running agent beats dirty: {c:?}"
        );
    }

    /// orch-04: a merged pull request allows deleting the branch only while
    /// the branch tip is the commit that was merged; commits added after the
    /// merge (not in the base) keep it.
    #[test]
    fn branch_delete_needs_the_current_tip_merged() {
        let dir = std::env::temp_dir().join(format!(
            "pod-orch04-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let g = |args: &[&str]| {
            let o = std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args([
                    "-c",
                    "user.name=pod",
                    "-c",
                    "user.email=pod@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        g(&["init", "-q", "-b", "main"]);
        g(&["commit", "-q", "--allow-empty", "-m", "a"]);
        let a = g(&["rev-parse", "HEAD"]);
        g(&["switch", "-q", "-c", "pod/oak"]);
        g(&["commit", "-q", "--allow-empty", "-m", "b"]);
        let b = g(&["rev-parse", "HEAD"]);
        let mut rec: TaskRecord = serde_json::from_value(serde_json::json!({
            "id": "app/oak", "repo": "app", "name": "oak", "branch": "pod/oak", "base": "main",
            "base_commit": a, "worktree": dir.display().to_string(), "agent": "shell", "created_ms": 0,
            "pr": {"number": 7, "url": "u", "title": "t", "state": "merged", "head_oid": b, "updated_ms": 0}
        }))
        .unwrap();
        // Squash-merged at b (b is not in main): the tip is what was merged.
        assert_eq!(branch_delete_refusal(&dir, &rec), None);
        // A commit after the merge: kept.
        g(&["commit", "-q", "--allow-empty", "-m", "c"]);
        let why = branch_delete_refusal(&dir, &rec).expect("kept");
        assert!(why.contains("after pull request #7 was merged"), "{why}");
        // A record without the merged commit proves nothing either.
        rec.pr.as_mut().unwrap().head_oid = None;
        assert!(branch_delete_refusal(&dir, &rec).is_some());
        // Once the current tip is in the base, it goes.
        g(&["switch", "-q", "main"]);
        g(&["merge", "-q", "--ff-only", "pod/oak"]);
        assert_eq!(branch_delete_refusal(&dir, &rec), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
