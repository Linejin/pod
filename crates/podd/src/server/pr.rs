//! Task pull requests (protocol v10, docs/15): `task.pr.status`,
//! `task.pr.create`, `task.pr.merge`.
//!
//! Everything runs through [`pod_forge::Forge`] in the task's worktree on a
//! blocking thread: the `gh` provider by default, the fake one with
//! `POD_FORGE=fake:<state.json>`. The last known pull request is stored in
//! the task record (`TaskInfo.pr`) and announced as `task.pr.changed` when
//! its facts change. While a lifecycle subscriber (an app) is connected,
//! tasks whose pull request is **open** are re-read every `POD_PR_POLL`
//! (default 2 min, `off` disables); nothing else is polled.
//!
//! Pod never creates repositories or remotes: without a GitHub remote the
//! status says `no_remote` and create refuses with `FORGE_FAILED`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use pod_forge::status::{self as forge_status, PrStatus};
use pod_forge::{CreatePr, ForgeError, ForgeRemote, PrState, PrView};

use super::status::{lifecycle_subscribers, publish_task_pr};
use super::tasks::{TaskRecord, blocking};
use super::{Daemon, log};
use crate::git;
use crate::protocol::*;

/// Open pull requests are re-read this often while an app is connected.
pub const DEFAULT_POLL: Duration = Duration::from_secs(120);

fn poll_period() -> Option<Duration> {
    std::env::var("POD_PR_POLL")
        .ok()
        .and_then(|v| super::retention::parse_duration(&v))
        .unwrap_or(Some(DEFAULT_POLL))
}

fn forge_err(e: ForgeError) -> RpcError {
    let what = match e.kind {
        ForgeErrorKind::NoRemote => "the repository has no GitHub remote (Pod does not create one)",
        ForgeErrorKind::UnsupportedHost => "the remote is not a GitHub repository",
        ForgeErrorKind::ToolMissing => "the GitHub CLI (gh) or git is not installed",
        ForgeErrorKind::NotLoggedIn => "not logged in to GitHub (run `gh auth login`)",
        ForgeErrorKind::NoPermission => "no permission on the GitHub repository",
        ForgeErrorKind::RateLimited => "GitHub rate limit reached; try again later",
        ForgeErrorKind::NotFound => "not found on GitHub",
        ForgeErrorKind::AlreadyExists => "a pull request already exists",
        ForgeErrorKind::Rejected => {
            "the remote branch has commits the task does not have (push rejected; pull or rebase in the task first)"
        }
        ForgeErrorKind::Network => "GitHub is unreachable",
        ForgeErrorKind::Timeout => "the GitHub command timed out",
        ForgeErrorKind::Failed => "the GitHub command failed",
    };
    RpcError {
        code: pr_codes::FORGE_FAILED,
        message: format!("{what}: {}", e.message),
        data: serde_json::to_value(&e).ok(),
    }
}

fn invalid_state(m: impl Into<String>) -> RpcError {
    RpcError::new(orch_codes::INVALID_STATE, m)
}

/// The remote branch a pull request targets by default: the task's base
/// without `refs/heads/`, `refs/remotes/<remote>/` or `<remote>/`. `None`
/// for a commit id or `HEAD`.
pub fn default_base(base: &str, remote: Option<&str>) -> Option<String> {
    let mut b = base.trim();
    b = b.strip_prefix("refs/heads/").unwrap_or(b);
    if let Some(r) = remote {
        b = b.strip_prefix(&format!("refs/remotes/{r}/")).unwrap_or(b);
        b = b.strip_prefix(&format!("{r}/")).unwrap_or(b);
    }
    b = b.strip_prefix("origin/").unwrap_or(b);
    let hex = b.len() >= 7 && b.len() <= 40 && b.bytes().all(|c| c.is_ascii_hexdigit());
    if b.is_empty() || b == "HEAD" || hex || b.starts_with('-') {
        None
    } else {
        Some(b.to_string())
    }
}

/// A partial `PrView` from the stored record (cached answers).
fn view_of(pr: &TaskPr, branch: &str) -> PrView {
    PrView {
        number: pr.number,
        url: pr.url.clone(),
        title: pr.title.clone(),
        state: pr.state,
        draft: pr.draft,
        head: branch.to_string(),
        head_oid: pr.head_oid.clone(),
        base: String::new(),
        review_decision: pr.review_decision,
        merge_state: pr.merge_state.clone(),
        mergeable: None,
        additions: 0,
        deletions: 0,
        changed_files: 0,
        author: None,
        merged_at: None,
        closed_at: None,
    }
}

/// Start the slow refresh of open pull requests.
pub(super) fn start(daemon: &Arc<Daemon>) {
    let Some(period) = poll_period() else {
        log("pull request polling off (POD_PR_POLL)");
        return;
    };
    let daemon = daemon.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(period).await;
            if lifecycle_subscribers() == 0 {
                continue;
            }
            for id in daemon.open_pr_tasks() {
                if let Ok(rec) = daemon.task_record(&id) {
                    let _ = daemon.pr_refresh(&rec, false).await;
                }
            }
        }
    });
}

struct Facts {
    ahead: u32,
    unpushed: u32,
    dirty: u32,
}

impl Daemon {
    fn open_pr_tasks(&self) -> Vec<String> {
        self.registry
            .lock()
            .unwrap()
            .tasks
            .iter()
            .filter(|t| t.pr.as_ref().is_some_and(|p| p.state == PrState::Open))
            .map(|t| t.id.clone())
            .collect()
    }

    fn pr_worktree(rec: &TaskRecord) -> Result<PathBuf, RpcError> {
        let wt = PathBuf::from(&rec.worktree);
        if wt.is_dir() {
            Ok(wt)
        } else {
            Err(invalid_state(format!(
                "task {} has no worktree directory",
                rec.id
            )))
        }
    }

    async fn pr_facts(rec: &TaskRecord, wt: &Path) -> Facts {
        let (wt, base, commit) = (wt.to_path_buf(), rec.base.clone(), rec.base_commit.clone());
        match blocking(move || git::facts(&wt, &base, &commit)).await {
            Ok(Ok(g)) => Facts {
                ahead: g.ahead,
                unpushed: g.unpushed,
                dirty: g.dirty,
            },
            _ => Facts {
                ahead: 0,
                unpushed: 0,
                dirty: 0,
            },
        }
    }

    /// Store the task's pull request; `task.pr.changed` when its facts changed.
    fn store_pr(&self, id: &str, pr: Option<TaskPr>) {
        let changed = {
            let mut reg = self.registry.lock().unwrap();
            let Some(t) = reg.tasks.iter_mut().find(|t| t.id == id) else {
                return;
            };
            let changed = match (&t.pr, &pr) {
                (Some(a), Some(b)) => !a.same_facts(b),
                (None, None) => false,
                _ => true,
            };
            let stamp_only = !changed && t.pr != pr;
            t.pr = pr.clone();
            if (changed || stamp_only)
                && let Err(e) = self.save_registry(&mut reg)
            {
                log(&format!("pr: cannot save registry: {}", e.message));
            }
            changed
        };
        if changed {
            log(&format!(
                "pr: {id} -> {}",
                pr.as_ref()
                    .map(|p| format!("#{} {:?} checks {}", p.number, p.state, p.checks.verdict()))
                    .unwrap_or_else(|| "none".into())
            ));
            publish_task_pr(id, pr);
        }
    }

    fn pr_info(rec: &TaskRecord, f: &Facts, status: PrStatus, cached: bool) -> TaskPrInfo {
        let remote = status.remote.as_ref().map(|r| r.name.clone());
        TaskPrInfo {
            task: rec.id.clone(),
            branch: rec.branch.clone(),
            default_base: default_base(&rec.base, remote.as_deref()),
            ahead: f.ahead,
            unpushed: f.unpushed,
            dirty: f.dirty,
            status,
            cached,
            refreshed_ms: crate::now_ms(),
        }
    }

    /// Ask the forge (or the stored record when `cached`) about the branch.
    pub(super) async fn pr_refresh(
        &self,
        rec: &TaskRecord,
        cached: bool,
    ) -> Result<TaskPrInfo, RpcError> {
        let wt = Self::pr_worktree(rec)?;
        let facts = Self::pr_facts(rec, &wt).await;
        let forge = self.forge.clone();
        if cached {
            let w = wt.clone();
            let status = match blocking(move || forge.detect(&w)).await? {
                Ok(remote) => PrStatus {
                    remote: Some(remote),
                    pr: rec.pr.as_ref().map(|p| view_of(p, &rec.branch)),
                    summary: rec.pr.as_ref().map(|p| p.checks).unwrap_or_default(),
                    ..PrStatus::default()
                },
                Err(e) => PrStatus::failed(e),
            };
            return Ok(Self::pr_info(rec, &facts, status, true));
        }
        // After a running create/merge, so an older answer never overwrites a newer one.
        let _op = self.pr_ops.lock().await;
        let rec = &self.task_record(&rec.id)?;
        let (branch, known) = (rec.branch.clone(), rec.pr.as_ref().map(|p| p.number));
        let mut status =
            blocking(move || forge_status::collect(&*forge, &wt, &branch, known)).await?;
        let lookup_failed = status.pr.is_none() && status.error.is_some();
        let stored = rec.pr.as_ref();
        if lookup_failed {
            // GitHub could not be read (timeout, rate limit...): answer with the
            // last known pull request, not "none" (the app would offer to create one).
            if let Some(p) = stored {
                status.pr = Some(view_of(p, &rec.branch));
                status.summary = p.checks;
            }
        } else {
            // The pull request was read but its checks were not: keep the stored summary.
            if status.error.is_some()
                && status.checks.is_empty()
                && let (Some(v), Some(p)) = (status.pr.as_ref(), stored)
                && v.number == p.number
            {
                status.summary = p.checks;
            }
            let now = crate::now_ms();
            self.store_pr(
                &rec.id,
                status
                    .pr
                    .as_ref()
                    .map(|v| TaskPr::from_view(v, status.summary, now)),
            );
        }
        let cached = status.remote.is_none() || (lookup_failed && status.pr.is_some());
        Ok(Self::pr_info(rec, &facts, status, cached))
    }

    pub(super) async fn task_pr_status(
        &self,
        p: TaskPrStatusParams,
    ) -> Result<TaskPrInfo, RpcError> {
        let rec = self.task_record(&p.task)?;
        self.pr_refresh(&rec, p.cached).await
    }

    pub(super) async fn task_pr_create(
        &self,
        p: TaskPrCreateParams,
    ) -> Result<TaskPrInfo, RpcError> {
        let _op = self.pr_ops.lock().await;
        let rec = self.task_record(&p.task)?;
        let title = p.title.trim().to_string();
        if title.is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "a pull request needs a title",
            ));
        }
        let wt = Self::pr_worktree(&rec)?;
        let facts = Self::pr_facts(&rec, &wt).await;
        if facts.ahead == 0 {
            let mut e = invalid_state(format!(
                "{} has no commits ahead of {}; commit the changes first{}",
                rec.branch,
                rec.base,
                if facts.dirty > 0 {
                    format!(" ({} uncommitted)", facts.dirty)
                } else {
                    String::new()
                }
            ));
            e.data = Some(serde_json::json!({"ahead": facts.ahead, "dirty": facts.dirty}));
            return Err(e);
        }
        let forge = self.forge.clone();
        let w = wt.clone();
        let remote: ForgeRemote = blocking(move || forge.detect(&w))
            .await?
            .map_err(forge_err)?;
        let base = match p.base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
            Some(b) => b.to_string(),
            None => default_base(&rec.base, Some(&remote.name)).ok_or_else(|| {
                RpcError::new(
                    codes::INVALID_PARAMS,
                    format!(
                        "the task's base {:?} is not a branch; pass the pull request's base branch",
                        rec.base
                    ),
                )
            })?,
        };
        if base.starts_with('-') || base.contains(char::is_whitespace) {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("invalid base branch {base:?}"),
            ));
        }
        let req = CreatePr {
            head: rec.branch.clone(),
            base: base.clone(),
            title,
            body: p.body.clone(),
            draft: p.draft,
        };
        let push = p.push.unwrap_or(true);
        log(&format!(
            "task.pr.create {} {} -> {base} (push {push}, draft {})",
            rec.id, rec.branch, p.draft
        ));
        let forge = self.forge.clone();
        let w = wt.clone();
        let r = remote.clone();
        let status = blocking(move || -> Result<PrStatus, ForgeError> {
            if push {
                forge.push(&w, &r, &req.head)?;
            }
            let pr = forge.create(&w, &r, &req)?;
            let mut st = PrStatus {
                remote: Some(r.clone()),
                ..PrStatus::default()
            };
            forge_status::fill(&*forge, &w, &r, &mut st, &pr);
            st.pr = Some(pr);
            Ok(st)
        })
        .await?
        .map_err(forge_err)?;
        let now = crate::now_ms();
        self.store_pr(
            &rec.id,
            status
                .pr
                .as_ref()
                .map(|v| TaskPr::from_view(v, status.summary, now)),
        );
        let facts = Self::pr_facts(&rec, &wt).await;
        Ok(Self::pr_info(&rec, &facts, status, false))
    }

    pub(super) async fn task_pr_merge(&self, p: TaskPrMergeParams) -> Result<TaskPrInfo, RpcError> {
        let _op = self.pr_ops.lock().await;
        let rec = self.task_record(&p.task)?;
        let wt = Self::pr_worktree(&rec)?;
        let forge = self.forge.clone();
        let (w, branch, known, method) = (
            wt.clone(),
            rec.branch.clone(),
            rec.pr.as_ref().map(|x| x.number),
            p.method,
        );
        let res = blocking(move || -> Result<Result<PrStatus, String>, ForgeError> {
            let r = forge.detect(&w)?;
            let pr = match known {
                Some(n) => Some(forge.view(&w, &r, n)?),
                None => forge.find_for_branch(&w, &r, &branch)?,
            };
            let Some(pr) = pr else {
                return Ok(Err(format!(
                    "{branch} has no pull request; create one first"
                )));
            };
            if pr.state != PrState::Open {
                return Ok(Err(format!(
                    "pull request #{} is {:?}, not open",
                    pr.number, pr.state
                )
                .to_lowercase()));
            }
            let merged = forge.merge(&w, &r, pr.number, method)?;
            let mut st = PrStatus {
                remote: Some(r.clone()),
                ..PrStatus::default()
            };
            forge_status::fill(&*forge, &w, &r, &mut st, &merged);
            st.pr = Some(merged);
            Ok(Ok(st))
        })
        .await?
        .map_err(forge_err)?;
        let status = res.map_err(invalid_state)?;
        log(&format!(
            "task.pr.merge {} {:?}: {:?}",
            rec.id,
            p.method,
            status.pr.as_ref().map(|x| x.state)
        ));
        let now = crate::now_ms();
        self.store_pr(
            &rec.id,
            status
                .pr
                .as_ref()
                .map(|v| TaskPr::from_view(v, status.summary, now)),
        );
        let facts = Self::pr_facts(&rec, &wt).await;
        Ok(Self::pr_info(&rec, &facts, status, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_strips_remote_prefixes() {
        assert_eq!(
            default_base("main", Some("origin")).as_deref(),
            Some("main")
        );
        assert_eq!(
            default_base("origin/main", Some("origin")).as_deref(),
            Some("main")
        );
        assert_eq!(
            default_base("upstream/dev", Some("upstream")).as_deref(),
            Some("dev")
        );
        assert_eq!(
            default_base("refs/heads/release/1.0", None).as_deref(),
            Some("release/1.0")
        );
        assert_eq!(
            default_base("pod/feat-1", Some("origin")).as_deref(),
            Some("pod/feat-1")
        );
        assert_eq!(default_base("HEAD", Some("origin")), None);
        assert_eq!(default_base("5fa727c87d", Some("origin")), None);
        assert_eq!(default_base("", None), None);
    }

    #[test]
    fn forge_errors_keep_their_kind_in_data() {
        let e = forge_err(ForgeError::new(
            ForgeErrorKind::NotLoggedIn,
            "To get started with GitHub CLI, please run:  gh auth login",
        ));
        assert_eq!(e.code, pr_codes::FORGE_FAILED);
        assert!(e.message.starts_with("not logged in to GitHub"));
        assert_eq!(e.data.unwrap()["kind"], "not_logged_in");
        // The app recognises the kind by this prefix (pod-app pr_panel::kind_of_message).
        let r = forge_err(ForgeError::new(
            ForgeErrorKind::Rejected,
            "error: failed to push some refs",
        ));
        assert!(
            r.message
                .starts_with("the remote branch has commits the task does not have"),
            "{}",
            r.message
        );
        assert_eq!(r.data.unwrap()["kind"], "rejected");
    }
}
