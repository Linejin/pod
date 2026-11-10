//! P5 backend: fan-out groups. One prompt, N agents, N tasks from the same
//! base commit; compare their diffs and an optional check command; pick a
//! winner and (safely) remove the rest.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use pod_forge::bounded::{self, End, Keep};

use super::orch_store::{GroupMemberRecord, GroupRecord};
use super::status::{self, HubEvent};
use super::tasks::validate_name;
use super::{Daemon, log};
use crate::git;
use crate::protocol::*;

const MAX_AGENTS: usize = 16;
const DEFAULT_CHECK_TIMEOUT_MS: u64 = 600_000;
const OUTPUT_TAIL: usize = 4096;

fn agent_label(a: &AgentSpec) -> &'static str {
    match a {
        AgentSpec::Kind(AgentKind::Claude) => "claude",
        AgentSpec::Kind(AgentKind::Codex) => "codex",
        AgentSpec::Kind(AgentKind::Shell) => "shell",
        AgentSpec::Argv { .. } => "argv",
    }
}

/// `<name>-<agent>-<n>`, numbered per agent label from 1.
pub fn member_names(name: &str, agents: &[AgentSpec]) -> Vec<String> {
    let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
    agents
        .iter()
        .map(|a| {
            let l = agent_label(a);
            let n = seen.entry(l).or_insert(0);
            *n += 1;
            format!("{name}-{l}-{n}")
        })
        .collect()
}

fn takes_cli_prompt(a: &AgentSpec) -> bool {
    matches!(
        a,
        AgentSpec::Kind(AgentKind::Claude) | AgentSpec::Kind(AgentKind::Codex)
    )
}

/// Run `argv` in `dir` with a time limit; capture the tail of its output.
/// Setting `cancel` kills the command (and everything it started).
pub fn run_check(
    dir: &Path,
    argv: &[String],
    timeout: Duration,
    cancel: &AtomicBool,
) -> CheckResult {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let start = Instant::now();
    let fail = |e: String| CheckResult {
        argv: argv.to_vec(),
        passed: false,
        exit_code: None,
        timed_out: false,
        duration_ms: start.elapsed().as_millis() as u64,
        finished_ms: crate::now_ms(),
        output_tail: String::new(),
        error: Some(e),
    };
    if !dir.is_dir() {
        return fail(format!("worktree {} is missing", dir.display()));
    }
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..])
        .current_dir(dir)
        .creation_flags(CREATE_NO_WINDOW);
    for v in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        cmd.env_remove(v);
    }
    // One process tree, a whole-call deadline and a bounded tail
    // (pod_forge::bounded, shared with archive scripts and gh).
    let mut opts = bounded::Options::new(timeout, Keep::Tail(OUTPUT_TAIL));
    opts.merge = true;
    let o = match bounded::run(&mut cmd, &opts, Some(cancel)) {
        Ok(o) => o,
        Err(e) => return fail(format!("cannot start {:?}: {e}", argv[0])),
    };
    let (exit_code, timed_out, cancelled) = match o.end {
        End::Exited { code, .. } => (code, false, false),
        End::TimedOut => (None, true, false),
        End::Cancelled => (None, false, true),
        End::TooLarge { .. } => (None, false, false),
    };
    CheckResult {
        argv: argv.to_vec(),
        passed: !timed_out && !cancelled && exit_code == Some(0),
        exit_code,
        timed_out,
        duration_ms: start.elapsed().as_millis() as u64,
        finished_ms: crate::now_ms(),
        output_tail: bounded::tail_str(&o.stdout.text(), OUTPUT_TAIL),
        error: cancelled.then(|| "cancelled".to_string()),
    }
}

fn group_not_found(s: &str) -> RpcError {
    RpcError::new(orch_codes::GROUP_NOT_FOUND, format!("no group {s:?}"))
}

impl Daemon {
    fn group_record(&self, s: &str) -> Result<GroupRecord, RpcError> {
        self.orch.lock().unwrap().group(s).cloned()
    }

    pub(super) async fn group_create(
        self: &Arc<Self>,
        p: GroupCreateParams,
    ) -> Result<GroupInfo, RpcError> {
        validate_name("group", &p.name)?;
        if p.agents.is_empty() || p.agents.len() > MAX_AGENTS {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("agents must list 1 to {MAX_AGENTS} agents"),
            ));
        }
        if p.prompt.trim().is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "prompt must not be empty",
            ));
        }
        if let Some(c) = &p.check
            && (c.is_empty() || c[0].is_empty())
        {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "check argv must not be empty",
            ));
        }
        let repo = self.repo_record(&p.repo)?;
        let id = format!("{}/{}", repo.name, p.name);
        let names = member_names(&p.name, &p.agents);
        for n in &names {
            validate_name("member task", n)?;
        }
        {
            let st = self.orch.lock().unwrap();
            if st.groups.iter().any(|g| g.id == id) {
                return Err(RpcError::new(
                    orch_codes::GROUP_EXISTS,
                    format!("group {id} exists"),
                ));
            }
        }
        for n in &names {
            let tid = format!("{}/{n}", repo.name);
            if self.task_record(&tid).is_ok() {
                return Err(RpcError::new(
                    orch_codes::GROUP_EXISTS,
                    format!("member task {tid} already exists"),
                ));
            }
        }
        // One base commit for every member.
        let repo_path = PathBuf::from(&repo.path);
        let base_in = p.base.clone();
        let (base, base_commit) = tokio::task::spawn_blocking(move || {
            let base = base_in.unwrap_or_else(|| git::default_base(&repo_path));
            let commit = git::resolve_commit(&repo_path, &base);
            (base, commit)
        })
        .await
        .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("worker failed: {e}")))?;
        let base_commit = base_commit.ok_or_else(|| {
            RpcError::new(
                codes::INVALID_PARAMS,
                format!("base {base:?} is not a commit in {}", repo.path),
            )
        })?;

        let mut created: Vec<TaskInfo> = Vec::new();
        for (agent, name) in p.agents.iter().zip(&names) {
            let r = self
                .task_create(TaskCreateParams {
                    repo: repo.name.clone(),
                    name: name.clone(),
                    base: Some(base_commit.clone()),
                    agent: Some(agent.clone()),
                    prompt: takes_cli_prompt(agent).then(|| p.prompt.clone()),
                    parent_terminal: p.parent_terminal.clone(),
                    cols: p.cols,
                    rows: p.rows,
                    ..Default::default()
                })
                .await;
            match r {
                Ok(t) => created.push(t),
                Err(e) => {
                    log(&format!(
                        "group {id}: member {name} failed ({e}); rolling back {} member(s)",
                        created.len()
                    ));
                    for t in &created {
                        let _ = self
                            .task_remove(TaskRemoveParams {
                                task: t.id.clone(),
                                force: true,
                                delete_branch: true,
                            })
                            .await;
                    }
                    return Err(e);
                }
            }
        }
        // Record the ref the user asked for and the shared prompt on each task.
        {
            let mut reg = self.registry.lock().unwrap();
            for t in reg
                .tasks
                .iter_mut()
                .filter(|t| created.iter().any(|c| c.id == t.id))
            {
                t.base = base.clone();
                t.prompt = Some(p.prompt.clone());
            }
            reg.save(&self.config.state_dir)?;
        }
        // Shell / argv agents get the prompt typed in.
        for (agent, t) in p.agents.iter().zip(&created) {
            if takes_cli_prompt(agent) {
                continue;
            }
            if let Some(term) = t.terminals.first() {
                let _ = self
                    .send(SendParams {
                        target: TerminalRef::id(term.id.clone()),
                        text: p.prompt.replace("\r\n", "\n").replace('\n', "\r"),
                        enter: true,
                        // Shell/argv members: no agent status to confirm.
                        confirm: false,
                    })
                    .await;
            }
        }
        let rec = GroupRecord {
            id: id.clone(),
            repo: repo.name.clone(),
            name: p.name.clone(),
            base,
            base_commit,
            prompt: p.prompt.clone(),
            check: p.check.clone(),
            created_ms: crate::now_ms(),
            picked: None,
            picked_ms: None,
            members: p
                .agents
                .iter()
                .zip(&created)
                .map(|(a, t)| GroupMemberRecord {
                    task: t.id.clone(),
                    agent: a.clone(),
                    check: None,
                })
                .collect(),
            check_job: None,
        };
        {
            let mut st = self.orch.lock().unwrap();
            st.groups.push(rec.clone());
            self.save_orch(&mut st)?;
        }
        log(&format!(
            "group {id}: {} member(s) from {}",
            created.len(),
            &rec.base_commit[..rec.base_commit.len().min(12)]
        ));
        Ok(self.group_info(rec).await)
    }

    pub(super) async fn group_info(&self, rec: GroupRecord) -> GroupInfo {
        let mut members = Vec::with_capacity(rec.members.len());
        for m in &rec.members {
            let info = match self.task_record(&m.task) {
                Ok(t) => Some(Box::new(self.task_info(t).await)),
                Err(_) => None,
            };
            members.push(GroupMember {
                task: m.task.clone(),
                agent: m.agent.clone(),
                removed: info.is_none(),
                info,
                picked: rec.picked.as_deref() == Some(m.task.as_str()),
                check: m.check.clone(),
            });
        }
        let gates = self
            .orch
            .lock()
            .unwrap()
            .gates
            .iter()
            .filter(|g| {
                g.group.as_deref() == Some(rec.id.as_str())
                    || g.task
                        .as_ref()
                        .is_some_and(|t| rec.members.iter().any(|m| &m.task == t))
            })
            .cloned()
            .collect();
        GroupInfo {
            id: rec.id,
            repo: rec.repo,
            name: rec.name,
            base: rec.base,
            base_commit: rec.base_commit,
            prompt: rec.prompt,
            check: rec.check,
            created_ms: rec.created_ms,
            picked: rec.picked,
            picked_ms: rec.picked_ms,
            members,
            check_job: rec.check_job,
            gates,
        }
    }

    pub(super) async fn group_list(&self, p: GroupListParams) -> Result<GroupListResult, RpcError> {
        let repo = match &p.repo {
            Some(r) => Some(self.repo_record(r)?.name),
            None => None,
        };
        let recs: Vec<GroupRecord> = self
            .orch
            .lock()
            .unwrap()
            .groups
            .iter()
            .filter(|g| repo.as_ref().is_none_or(|r| &g.repo == r))
            .cloned()
            .collect();
        let mut groups = Vec::with_capacity(recs.len());
        for r in recs {
            groups.push(self.group_info(r).await);
        }
        Ok(GroupListResult { groups })
    }

    pub(super) async fn group_show(&self, p: GroupRef) -> Result<GroupInfo, RpcError> {
        let rec = self.group_record(&p.group)?;
        Ok(self.group_info(rec).await)
    }

    pub(super) async fn group_compare(&self, p: GroupRef) -> Result<GroupCompareResult, RpcError> {
        let rec = self.group_record(&p.group)?;
        let mut members = Vec::with_capacity(rec.members.len());
        for m in &rec.members {
            let mut cm = CompareMember {
                task: m.task.clone(),
                agent: m.agent.clone(),
                removed: false,
                picked: rec.picked.as_deref() == Some(m.task.as_str()),
                status: None,
                files: Vec::new(),
                files_changed: 0,
                added: 0,
                deleted: 0,
                ahead: None,
                dirty: None,
                check: m.check.clone(),
                error: None,
            };
            let Ok(t) = self.task_record(&m.task) else {
                cm.removed = true;
                members.push(cm);
                continue;
            };
            let info = self.task_info(t).await;
            cm.status = info.status.as_ref().map(|s| match &s.kind {
                Some(k) => format!("{}:{k}", s.state),
                None => s.state.clone(),
            });
            cm.ahead = info.git.as_ref().map(|g| g.ahead);
            cm.dirty = info.git.as_ref().map(|g| g.dirty);
            match self
                .task_diff(TaskRef {
                    task: m.task.clone(),
                })
                .await
            {
                Ok(d) => {
                    cm.files_changed = d.files.len();
                    cm.added = d.added;
                    cm.deleted = d.deleted;
                    cm.files = d.files;
                }
                Err(e) => cm.error = Some(e.message),
            }
            members.push(cm);
        }
        Ok(GroupCompareResult {
            group: rec.id,
            base: rec.base,
            check: rec.check,
            members,
        })
    }

    /// Store a member result, or the job's end, and announce it.
    fn record_check(
        &self,
        group: &str,
        job_id: &str,
        member: Option<&MemberCheck>,
        end: Option<CheckJobState>,
    ) -> Option<CheckJob> {
        let job = {
            let mut st = self.orch.lock().unwrap();
            let g = st.groups.iter_mut().find(|g| g.id == group)?;
            if g.check_job.as_ref().is_none_or(|j| j.id != job_id) {
                return None; // replaced by a newer job
            }
            if let Some(m) = member
                && let Some(rec) = g.members.iter_mut().find(|x| x.task == m.task)
            {
                rec.check = Some(m.result.clone());
            }
            let all = g
                .members
                .iter()
                .filter(|m| {
                    g.check_job
                        .as_ref()
                        .is_some_and(|j| j.tasks.contains(&m.task))
                })
                .all(|m| m.check.as_ref().is_some_and(|c| c.passed));
            let job = g.check_job.as_mut()?;
            if let Some(m) = member
                && !job.done.contains(&m.task)
            {
                job.done.push(m.task.clone());
            }
            if let Some(state) = end {
                job.state = state;
                job.finished_ms = Some(crate::now_ms());
                job.all_passed = Some(state == CheckJobState::Finished && all);
            }
            let job = job.clone();
            if let Err(e) = self.save_orch(&mut st) {
                log(&e.message);
            }
            job
        };
        status::publish_group_check(group, job.clone(), member.cloned());
        Some(job)
    }

    /// The current check state of a group as a `group.check` result.
    fn check_result(&self, group: &str) -> Result<GroupCheckResult, RpcError> {
        let rec = self.group_record(group)?;
        let job = rec.check_job.clone();
        let tasks: Vec<String> = match &job {
            Some(j) => j.tasks.clone(),
            None => rec.members.iter().map(|m| m.task.clone()).collect(),
        };
        let results: Vec<MemberCheck> = rec
            .members
            .iter()
            .filter(|m| tasks.contains(&m.task))
            .filter(|m| job.as_ref().is_none_or(|j| j.done.contains(&m.task)))
            .filter_map(|m| {
                m.check.clone().map(|result| MemberCheck {
                    task: m.task.clone(),
                    result,
                })
            })
            .collect();
        let finished = job
            .as_ref()
            .is_some_and(|j| j.state == CheckJobState::Finished);
        Ok(GroupCheckResult {
            group: rec.id,
            all_passed: finished && !results.is_empty() && results.iter().all(|r| r.result.passed),
            results,
            job,
        })
    }

    /// Wait until the group's check job `job_id` is no longer running.
    async fn wait_check_end(
        &self,
        group: &str,
        job_id: &str,
        mut rx: tokio::sync::broadcast::Receiver<HubEvent>,
        timeout: Option<Duration>,
    ) {
        let running = || {
            self.group_record(group).is_ok_and(|g| {
                g.check_job
                    .is_some_and(|j| j.id == job_id && j.state == CheckJobState::Running)
            })
        };
        let wait = async {
            while running() {
                match rx.recv().await {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        };
        match timeout {
            Some(t) => {
                let _ = tokio::time::timeout(t, wait).await;
            }
            None => wait.await,
        }
    }

    /// v6: start the check as a background job; reply at once (or, with
    /// `wait`, when it ended). Members run in parallel; each result is stored
    /// and announced as it arrives.
    pub(super) async fn group_check(
        self: &Arc<Self>,
        p: GroupCheckParams,
    ) -> Result<GroupCheckResult, RpcError> {
        let rec = self.group_record(&p.group)?;
        if let Some(c) = &p.check
            && (c.is_empty() || c[0].is_empty())
        {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "check argv must not be empty",
            ));
        }
        let Some(argv) = p.check.clone().or_else(|| rec.check.clone()) else {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!("group {} has no check command (pass check)", rec.id),
            ));
        };
        let timeout_ms = p.timeout_ms.unwrap_or(DEFAULT_CHECK_TIMEOUT_MS).max(1);
        let mut members: Vec<(String, PathBuf)> = Vec::new();
        for m in &rec.members {
            if !p.tasks.is_empty() && !p.tasks.contains(&m.task) {
                continue;
            }
            if let Ok(t) = self.task_record(&m.task) {
                members.push((m.task.clone(), PathBuf::from(&t.worktree)));
            } // else: removed
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = status::subscribe();
        let job = {
            let mut jobs = self.check_jobs.lock().unwrap();
            if jobs.contains_key(&rec.id) {
                let running = rec.check_job.as_ref().map(|j| j.id.clone());
                return Err(RpcError::new(
                    orch_codes::INVALID_STATE,
                    format!(
                        "a check of group {} is running ({}); wait for it or cancel it",
                        rec.id,
                        running.unwrap_or_default()
                    ),
                ));
            }
            let mut st = self.orch.lock().unwrap();
            let id = st.next_id("k");
            let job = CheckJob {
                id,
                state: CheckJobState::Running,
                argv: argv.clone(),
                tasks: members.iter().map(|(t, _)| t.clone()).collect(),
                done: Vec::new(),
                timeout_ms,
                started_ms: crate::now_ms(),
                finished_ms: None,
                all_passed: None,
            };
            let g = st
                .groups
                .iter_mut()
                .find(|g| g.id == rec.id)
                .ok_or_else(|| group_not_found(&rec.id))?;
            g.check = Some(argv.clone());
            g.check_job = Some(job.clone());
            self.save_orch(&mut st)?;
            jobs.insert(rec.id.clone(), cancel.clone());
            job
        };
        log(&format!(
            "group {}: check job {} started on {} member(s)",
            rec.id,
            job.id,
            members.len()
        ));
        status::publish_group_check(&rec.id, job.clone(), None);

        let daemon = self.clone();
        let (group, job_id) = (rec.id.clone(), job.id.clone());
        tokio::spawn(async move {
            let timeout = Duration::from_millis(timeout_ms);
            let mut set = tokio::task::JoinSet::new();
            for (task, wt) in members {
                let (argv, cancel) = (argv.clone(), cancel.clone());
                set.spawn_blocking(move || {
                    let r = run_check(&wt, &argv, timeout, &cancel);
                    MemberCheck { task, result: r }
                });
            }
            while let Some(res) = set.join_next().await {
                let m = match res {
                    Ok(m) => m,
                    Err(e) => {
                        log(&format!("group {group}: check worker failed: {e}"));
                        continue;
                    }
                };
                log(&format!(
                    "group {group}: check in {}: {}",
                    m.task,
                    if m.result.passed { "passed" } else { "FAILED" }
                ));
                daemon.record_check(&group, &job_id, Some(&m), None);
            }
            let state = if cancel.load(Ordering::SeqCst) {
                CheckJobState::Cancelled
            } else {
                CheckJobState::Finished
            };
            daemon.check_jobs.lock().unwrap().remove(&group);
            if let Some(j) = daemon.record_check(&group, &job_id, None, Some(state)) {
                log(&format!(
                    "group {group}: check job {job_id} {:?} (all passed: {})",
                    j.state,
                    j.all_passed.unwrap_or(false)
                ));
            }
        });

        if p.wait {
            self.wait_check_end(&rec.id, &job.id, rx, None).await;
            return self.check_result(&rec.id);
        }
        Ok(GroupCheckResult {
            group: rec.id,
            results: Vec::new(),
            all_passed: false,
            job: Some(job),
        })
    }

    /// v6: kill the running check commands of a group; replies once the job
    /// has ended (state `cancelled`), or with the current state when no job runs.
    pub(super) async fn group_check_cancel(
        &self,
        p: GroupRef,
    ) -> Result<GroupCheckResult, RpcError> {
        let rec = self.group_record(&p.group)?;
        let rx = status::subscribe();
        let flag = self.check_jobs.lock().unwrap().get(&rec.id).cloned();
        if let (Some(flag), Some(job)) = (flag, &rec.check_job) {
            flag.store(true, Ordering::SeqCst);
            log(&format!(
                "group {}: cancelling check job {}",
                rec.id, job.id
            ));
            self.wait_check_end(&rec.id, &job.id, rx, Some(Duration::from_secs(15)))
                .await;
        }
        self.check_result(&rec.id)
    }

    /// At start: jobs recorded as running belonged to a previous daemon.
    pub(super) fn interrupt_stale_check_jobs(&self) {
        let mut st = self.orch.lock().unwrap();
        let mut changed = false;
        for g in st.groups.iter_mut() {
            if let Some(j) = g.check_job.as_mut()
                && j.state == CheckJobState::Running
            {
                j.state = CheckJobState::Interrupted;
                j.finished_ms = Some(crate::now_ms());
                changed = true;
            }
        }
        if changed && let Err(e) = self.save_orch(&mut st) {
            log(&e.message);
        }
    }

    pub(super) async fn group_pick(&self, p: GroupPickParams) -> Result<GroupPickResult, RpcError> {
        let rec = self.group_record(&p.group)?;
        let winner = match rec.members.iter().find(|m| m.task == p.task) {
            Some(m) => m.task.clone(),
            None => {
                // Accept a bare task name too.
                let t = self.task_record(&p.task)?;
                rec.members
                    .iter()
                    .find(|m| m.task == t.id)
                    .map(|m| m.task.clone())
                    .ok_or_else(|| {
                        RpcError::new(
                            codes::INVALID_PARAMS,
                            format!("{} is not a member of group {}", t.id, rec.id),
                        )
                    })?
            }
        };
        if self.task_record(&winner).is_err() {
            return Err(RpcError::new(
                codes::TASK_NOT_FOUND,
                format!("member {winner} was removed"),
            ));
        }
        {
            let mut st = self.orch.lock().unwrap();
            if let Some(g) = st.groups.iter_mut().find(|g| g.id == rec.id) {
                g.picked = Some(winner.clone());
                g.picked_ms = Some(crate::now_ms());
            }
            self.save_orch(&mut st)?;
        }
        log(&format!("group {}: picked {winner}", rec.id));
        let mut removed = Vec::new();
        let mut kept = Vec::new();
        if p.remove_others {
            for m in rec.members.iter().filter(|m| m.task != winner) {
                if self.task_record(&m.task).is_err() {
                    continue; // already gone
                }
                match self
                    .task_remove(TaskRemoveParams {
                        task: m.task.clone(),
                        force: p.force,
                        delete_branch: p.delete_branch,
                    })
                    .await
                {
                    Ok(r) => removed.push(r),
                    Err(e) => kept.push(MemberError {
                        task: m.task.clone(),
                        code: e.code,
                        message: e.message,
                    }),
                }
            }
        }
        Ok(GroupPickResult {
            group: rec.id,
            picked: winner,
            removed,
            kept,
        })
    }

    /// Forget the group record; member tasks are left alone.
    pub(super) fn group_remove(&self, p: GroupRef) -> Result<GroupRemoveResult, RpcError> {
        let mut st = self.orch.lock().unwrap();
        let id = st
            .group(&p.group)
            .map_err(|_| group_not_found(&p.group))?
            .id
            .clone();
        st.groups.retain(|g| g.id != id);
        self.save_orch(&mut st)?;
        Ok(GroupRemoveResult { id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_naming() {
        let a = [
            AgentSpec::Kind(AgentKind::Claude),
            AgentSpec::Kind(AgentKind::Shell),
            AgentSpec::Kind(AgentKind::Claude),
            AgentSpec::Argv {
                argv: vec!["x".into()],
            },
        ];
        assert_eq!(
            member_names("fix", &a),
            ["fix-claude-1", "fix-shell-1", "fix-claude-2", "fix-argv-1"]
        );
    }

    #[test]
    fn check_runner_exit_codes_output_and_timeout() {
        let dir = std::env::temp_dir();
        let ok = run_check(
            &dir,
            &["cmd".into(), "/c".into(), "echo hello-check".into()],
            Duration::from_secs(20),
            &AtomicBool::new(false),
        );
        assert!(ok.passed, "{ok:?}");
        assert!(ok.output_tail.contains("hello-check"));
        let bad = run_check(
            &dir,
            &["cmd".into(), "/c".into(), "exit 3".into()],
            Duration::from_secs(20),
            &AtomicBool::new(false),
        );
        assert!(!bad.passed);
        assert_eq!(bad.exit_code, Some(3));
        let slow = run_check(
            &dir,
            &[
                "cmd".into(),
                "/c".into(),
                "ping -n 30 127.0.0.1 >nul".into(),
            ],
            Duration::from_millis(500),
            &AtomicBool::new(false),
        );
        assert!(slow.timed_out && !slow.passed);
        assert!(slow.duration_ms < 10_000, "{slow:?}");
        let missing = run_check(
            &dir,
            &["no-such-program-xyz".into()],
            Duration::from_secs(5),
            &AtomicBool::new(false),
        );
        assert!(missing.error.is_some() && !missing.passed);
        assert_eq!(
            bounded::tail_str(&"a".repeat(5000), OUTPUT_TAIL)
                .chars()
                .count(),
            OUTPUT_TAIL + 1
        );
    }

    #[test]
    fn check_runner_cancel_kills_the_command() {
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            flag.store(true, Ordering::SeqCst);
        });
        let start = Instant::now();
        let r = run_check(
            &std::env::temp_dir(),
            &[
                "cmd".into(),
                "/c".into(),
                "ping -n 60 127.0.0.1 >nul".into(),
            ],
            Duration::from_secs(120),
            &cancel,
        );
        assert!(!r.passed && !r.timed_out, "{r:?}");
        assert_eq!(r.error.as_deref(), Some("cancelled"));
        assert!(start.elapsed() < Duration::from_secs(15), "{r:?}");
    }
}
