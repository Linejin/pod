//! v9 lifecycle events: `terminal.changed`, `task.changed`, `task.removing`
//! and `task.touch` (docs/03 §4.3).
//!
//! Terminal and task create/remove events are published where those things
//! happen (`run`, `rm`, `Terminal::on_frame`, `task_create`, `task_remove`).
//! This module adds what needs state: a worker that keeps the last announced
//! aggregate status and git facts per task and publishes `task.changed` when
//! they differ.
//!
//! - **status**: recomputed on every `status.changed` of a task terminal (no
//!   git, cheap).
//! - **git** (ahead, behind, dirty, unpushed, HEAD): refreshed when a task
//!   terminal's state changed (an agent turn usually edits or commits), on
//!   `task.touch`, and every [`GIT_POLL`] while at least one lifecycle
//!   subscriber is connected (edits made outside Pod). Without lifecycle
//!   subscribers nothing is polled.
//!
//! The first facts of a task are a baseline and produce no event.
//!
//! It also keeps the holders rediscovery could not reach (daemon-03):
//! listed as `unreachable`, retried in the background with backoff, and
//! forgotten only by `terminal.kill`/`terminal.rm` (see [`Unreachable`]).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::broadcast;

use super::status::{self, HubEvent, lifecycle_subscribers, publish_task, task_event};
use super::tasks::{TaskRecord, blocking};
use super::terminal::Terminal;
use super::{Daemon, log};
use crate::git;
use crate::holder_proto::{Frame, HolderRecord};
use crate::protocol::*;

/// Git facts poll period while a lifecycle subscriber is connected.
pub const GIT_POLL: Duration = Duration::from_secs(5);
/// How often queued git refreshes run (coalesces bursts of status events).
const TICK: Duration = Duration::from_millis(250);
/// Default wait between `task.removing` and deleting the worktree when a
/// lifecycle subscriber is connected (`POD_TASK_REMOVE_GRACE_MS`).
pub const DEFAULT_REMOVE_GRACE_MS: u64 = 300;
const MAX_REMOVE_GRACE_MS: u64 = 10_000;

/// What was last announced for one task.
#[derive(Default)]
struct Snap {
    status: Option<TaskStatus>,
    git: Option<TaskGit>,
}

fn snaps() -> &'static Mutex<HashMap<String, Snap>> {
    static S: OnceLock<Mutex<HashMap<String, Snap>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Grace between `task.removing` and deletion.
pub(super) fn remove_grace_ms() -> u64 {
    if lifecycle_subscribers() == 0 {
        return 0;
    }
    std::env::var("POD_TASK_REMOVE_GRACE_MS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_REMOVE_GRACE_MS)
        .min(MAX_REMOVE_GRACE_MS)
}

/// Record a new git fact; returns whether it differs from an announced one.
/// `None` facts (unknown before) are a baseline.
fn store_git(task: &str, git: &TaskGit) -> bool {
    let mut s = snaps().lock().unwrap();
    let snap = s.entry(task.to_string()).or_default();
    let changed = snap.git.as_ref().is_some_and(|g| g != git);
    snap.git = Some(git.clone());
    changed
}

fn store_status(task: &str, status: &Option<TaskStatus>) -> bool {
    let mut s = snaps().lock().unwrap();
    let snap = s.entry(task.to_string()).or_default();
    let changed = snap.status != *status;
    snap.status = status.clone();
    changed
}

/// `task.create` finished: baseline + `task.changed created`.
pub(super) fn task_created(info: &TaskInfo, open_hint: OpenHint) {
    {
        let mut s = snaps().lock().unwrap();
        s.insert(
            info.id.clone(),
            Snap {
                status: info.status.clone(),
                git: info.git.clone(),
            },
        );
    }
    let mut ev = task_event(&info.id, TaskChange::Created);
    ev.task = Some(Box::new(info.clone()));
    ev.open_hint = Some(open_hint);
    publish_task(ev);
}

/// `task.remove` finished.
pub(super) fn task_removed(id: &str) {
    snaps().lock().unwrap().remove(id);
    publish_task(task_event(id, TaskChange::Removed));
}

/// Start the worker (after rediscovery, so terminals are known).
pub(super) fn start(daemon: &Arc<Daemon>) {
    let daemon = daemon.clone();
    let mut rx = status::subscribe();
    tokio::spawn(async move {
        // Baseline of every task (no events), then follow changes.
        let mut due: HashSet<String> = daemon.task_ids().into_iter().collect();
        for id in &due {
            let st = daemon.task_status_now(id);
            store_status(id, &st);
        }
        let mut baseline = true;
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_poll = Instant::now();
        loop {
            tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(HubEvent::Status(ev)) => {
                        for task in daemon.tasks_of_terminal(&ev.id) {
                            daemon.check_task_status(&task);
                            // Git only on a state transition (`from` set), not
                            // on every confidence/progress update.
                            if ev.from.is_some() || ev.removed {
                                due.insert(task);
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        for task in daemon.task_ids() {
                            daemon.check_task_status(&task);
                            due.insert(task);
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                _ = tick.tick() => {
                    let subscribed = lifecycle_subscribers() > 0;
                    if subscribed && last_poll.elapsed() >= GIT_POLL {
                        due.extend(daemon.task_ids());
                        last_poll = Instant::now();
                    }
                    if !subscribed && !baseline {
                        // Nobody listens: keep the last facts, poll nothing.
                        due.clear();
                        continue;
                    }
                    baseline = false;
                    let ids: Vec<String> = due.drain().collect();
                    for id in ids {
                        if let Ok(rec) = daemon.task_record(&id) {
                            daemon.refresh_git(&rec).await;
                        }
                    }
                }
            }
        }
    });
}

impl Daemon {
    fn task_ids(&self) -> Vec<String> {
        self.registry
            .lock()
            .unwrap()
            .tasks
            .iter()
            .map(|t| t.id.clone())
            .collect()
    }

    pub(super) fn tasks_of_terminal(&self, terminal: &str) -> Vec<String> {
        self.registry
            .lock()
            .unwrap()
            .tasks
            .iter()
            .filter(|t| t.terminals.iter().any(|x| x == terminal))
            .map(|t| t.id.clone())
            .collect()
    }

    /// The task's aggregate status right now (no git).
    pub(super) fn task_status_now(&self, task: &str) -> Option<TaskStatus> {
        let ids = self.task_record(task).ok()?.terminals;
        let infos: Vec<TerminalInfo> = {
            let terms: Vec<Arc<super::terminal::Terminal>> = self.terminals.lock().unwrap().clone();
            terms
                .iter()
                .filter(|t| ids.contains(&t.id))
                .map(|t| t.info())
                .collect()
        };
        // Keep the task's terminal order (ties go to the first terminal).
        let ordered: Vec<&TerminalInfo> = ids
            .iter()
            .filter_map(|id| infos.iter().find(|i| &i.id == id))
            .collect();
        aggregate_task_status(ordered)
    }

    /// Publish `task.changed status` when the aggregate differs from the last one.
    pub(super) fn check_task_status(&self, task: &str) {
        if self.task_record(task).is_err() {
            return;
        }
        let st = self.task_status_now(task);
        if store_status(task, &st) {
            let mut ev = task_event(task, TaskChange::Status);
            ev.status = st;
            publish_task(ev);
        }
    }

    /// The task's terminal list changed: `task.changed terminals` (+ status).
    pub(super) fn task_terminals_changed(&self, task: &str) {
        let Ok(rec) = self.task_record(task) else {
            return;
        };
        let mut ev = task_event(task, TaskChange::Terminals);
        ev.terminals = rec.terminals;
        publish_task(ev);
        self.check_task_status(task);
    }

    /// Fresh git facts; publishes `task.changed git` when they differ from
    /// the last announced ones. Returns (facts, error, changed).
    pub(super) async fn refresh_git(
        &self,
        rec: &TaskRecord,
    ) -> (Option<TaskGit>, Option<String>, bool) {
        let wt = PathBuf::from(&rec.worktree);
        if !wt.is_dir() {
            return (None, Some("worktree directory is missing".into()), false);
        }
        let (base, commit) = (rec.base.clone(), rec.base_commit.clone());
        match blocking(move || git::facts(&wt, &base, &commit)).await {
            Ok(Ok(g)) => {
                // The task may have been removed meanwhile: no event, no snapshot.
                if self.task_record(&rec.id).is_err() {
                    return (Some(g), None, false);
                }
                let changed = store_git(&rec.id, &g);
                if changed {
                    let mut ev = task_event(&rec.id, TaskChange::Git);
                    ev.git = Some(g.clone());
                    publish_task(ev);
                }
                (Some(g), None, changed)
            }
            Ok(Err(e)) => (None, Some(e), false),
            Err(e) => (None, Some(e.message), false),
        }
    }

    /// `task.touch`: refresh git now, then `task.changed touched`.
    pub(super) async fn task_touch(&self, p: TaskTouchParams) -> Result<TaskTouchResult, RpcError> {
        let rec = self.task_record(&p.task)?;
        let (git, git_error, git_changed) = self.refresh_git(&rec).await;
        let mut ev = task_event(&rec.id, TaskChange::Touched);
        ev.paths = p.paths;
        publish_task(ev);
        if let Some(e) = &git_error {
            log(&format!("task.touch {}: {e}", rec.id));
        }
        Ok(TaskTouchResult {
            task: rec.id,
            git,
            git_error,
            git_changed,
        })
    }
}

// ───────────────────────── unreachable holders ─────────────────────────

/// First wait before retrying unreachable holders (`POD_HOLDER_RETRY_MS`).
const DEFAULT_RETRY_MS: u64 = 2_000;
/// Longest wait between two retries.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);
/// Retries after the failed start attempt (`POD_HOLDER_RETRIES`). With the
/// defaults: 2, 4, 8, 16, 32, 60 s, about two minutes in all.
const DEFAULT_RETRIES: u32 = 6;
/// One connect attempt (busy-pipe wait + handshake), as at start.
const RETRY_CONNECT: Duration = Duration::from_secs(5);

/// A holder record found at start whose holder did not answer. Its id stays
/// reserved and its files stay until it is reached or given up.
#[derive(Debug, Clone)]
pub(super) struct Unreachable {
    pub id: String,
    /// The `<id>.json` record file.
    pub path: PathBuf,
    /// `None` while the record cannot be read (re-read on every retry).
    pub rec: Option<HolderRecord>,
    pub reason: String,
    /// Connect attempts so far (the start counts as the first).
    pub attempts: u32,
    /// No more retries; listed until `kill`/`rm`.
    pub gave_up: bool,
}

impl Unreachable {
    pub fn new(id: String, path: PathBuf, rec: Option<HolderRecord>, reason: String) -> Self {
        Self {
            id,
            path,
            rec,
            reason,
            attempts: 1,
            gave_up: false,
        }
    }

    pub fn info(&self) -> TerminalInfo {
        let rec = self.rec.as_ref();
        let mut i = TerminalInfo::new(
            self.id.clone(),
            Vec::new(),
            String::new(),
            TerminalState::Unreachable,
            0,
            0,
            rec.map_or(0, |r| r.created_ms),
        );
        i.name = rec.and_then(|r| r.name.clone());
        i.holder_pid = rec.map(|r| r.holder_pid);
        i.pid = rec.map(|r| r.child_pid);
        i.unreachable = Some(if self.gave_up {
            format!(
                "{}; gave up after {} attempts (pod kill {} forgets it)",
                self.reason, self.attempts, self.id
            )
        } else {
            format!("{} (attempt {}, retrying)", self.reason, self.attempts)
        });
        i
    }

    fn matches(&self, r: &TerminalRef) -> bool {
        match (&r.id, &r.name) {
            (Some(id), _) => *id == self.id,
            (None, Some(name)) => self.rec.as_ref().and_then(|r| r.name.as_ref()) == Some(name),
            (None, None) => false,
        }
    }
}

/// Wait before retry `n` (0-based): `base`, doubling, at most [`MAX_RETRY_WAIT`].
pub(super) fn retry_wait(n: u32, base: Duration) -> Duration {
    base.saturating_mul(1u32 << n.min(16)).min(MAX_RETRY_WAIT)
}

fn retry_settings() -> (Duration, u32) {
    let env = |k: &str| {
        std::env::var(k)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
    };
    let base = env("POD_HOLDER_RETRY_MS")
        .unwrap_or(DEFAULT_RETRY_MS)
        .max(10);
    let retries = env("POD_HOLDER_RETRIES").map_or(DEFAULT_RETRIES, |n| n.min(100) as u32);
    (Duration::from_millis(base), retries)
}

/// What one retry of one record found.
enum Retry {
    Reached(Arc<Terminal>),
    /// The record or the holder's pipe is gone: nothing left to reach.
    Gone,
    Failed(Option<HolderRecord>, String),
}

async fn retry_one(u: Unreachable) -> Retry {
    let rec = match u.rec {
        Some(r) => r,
        None => match std::fs::read(&u.path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Retry::Gone,
            Err(e) => return Retry::Failed(None, format!("record unreadable: {e}")),
            Ok(b) => match serde_json::from_slice::<HolderRecord>(&b) {
                Ok(r) => r,
                Err(e) => return Retry::Failed(None, format!("record unreadable: {e}")),
            },
        },
    };
    match Terminal::connect_within(&rec.pipe, RETRY_CONNECT).await {
        Ok(t) => Retry::Reached(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Retry::Gone,
        Err(e) => Retry::Failed(Some(rec), e.to_string()),
    }
}

/// Start retrying the unreachable holders (after rediscovery). Ends once
/// every one was reached, is gone, or was given up.
pub(super) fn start_holder_retry(daemon: &Arc<Daemon>) {
    if daemon.unreachable.lock().unwrap().is_empty() {
        return;
    }
    let daemon = daemon.clone();
    let (base, retries) = retry_settings();
    tokio::spawn(async move {
        for n in 0..retries {
            tokio::time::sleep(retry_wait(n, base)).await;
            // daemon-security-04: a round counts as a terminal start, so
            // `server.stop kill:true` waits for it before it collects the
            // terminals to kill, and no round begins once the stop was called.
            let Ok(_start) = daemon.begin_start() else {
                return;
            };
            #[cfg(debug_assertions)]
            test_round_delay().await;
            let todo: Vec<Unreachable> = daemon
                .unreachable
                .lock()
                .unwrap()
                .iter()
                .filter(|u| !u.gave_up)
                .cloned()
                .collect();
            if todo.is_empty() {
                return;
            }
            let last = n + 1 == retries;
            let tries: Vec<_> = todo
                .into_iter()
                .map(|u| (u.id.clone(), tokio::spawn(retry_one(u))))
                .collect();
            for (id, h) in tries {
                let res = h
                    .await
                    .unwrap_or_else(|e| Retry::Failed(None, e.to_string()));
                if let Some(t) = daemon.retried(&id, res, last) {
                    daemon.end_if_stopped(&t).await;
                }
            }
        }
        // `retries == 0`: the start attempt was the only one.
        for u in daemon.unreachable.lock().unwrap().iter_mut() {
            u.gave_up = true;
        }
    });
}

/// Debug-only test knob (`POD_HOLDER_TEST_RETRY_ROUND_DELAY_MS`): each retry
/// round waits this long after it registered as a start, so a test can land
/// `server.stop` inside a round deterministically.
#[cfg(debug_assertions)]
async fn test_round_delay() {
    if let Some(ms) = std::env::var("POD_HOLDER_TEST_RETRY_ROUND_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        tokio::time::sleep(Duration::from_millis(ms.min(30_000))).await;
    }
}

impl Daemon {
    /// A holder re-attached after `server.stop kill:true` was called: end it
    /// as the stop would have (the stop waits for retry rounds, so this only
    /// matters when a round outlived its grace).
    async fn end_if_stopped(&self, t: &Arc<Terminal>) {
        if !self.is_stopping_with_kill() {
            return;
        }
        self.kill_and_wait(t).await;
        t.send_to_holder(Frame::Release);
        log(&format!("{}: re-attached while stopping; killed", t.id));
    }

    /// Apply the result of one retry of `id` (unless it was given up
    /// meanwhile). Returns the terminal when it was re-attached.
    fn retried(&self, id: &str, res: Retry, last: bool) -> Option<Arc<Terminal>> {
        let mut list = self.unreachable.lock().unwrap();
        let Some(pos) = list.iter().position(|u| u.id == id) else {
            // Forgotten by kill/rm while we tried: leave the holder alone.
            return None;
        };
        match res {
            Retry::Reached(t) => {
                let u = list.remove(pos);
                drop(list);
                let info = t.info();
                log(&format!(
                    "re-attached {} after {} attempt(s) (holder pid {})",
                    t.id,
                    u.attempts + 1,
                    t.holder_pid
                ));
                {
                    let mut terms = self.terminals.lock().unwrap();
                    if terms.iter().any(|x| x.id == t.id) {
                        log(&format!(
                            "{}: already listed; dropping the late connection",
                            t.id
                        ));
                        return None;
                    }
                    let n = super::id_number(&t.id);
                    let at = terms.partition_point(|x| super::id_number(&x.id) < n);
                    terms.insert(at, t.clone());
                }
                status::publish_terminal(&info.id.clone(), TerminalChange::Created, Some(info));
                return Some(t);
            }
            Retry::Gone => {
                let u = list.remove(pos);
                drop(list);
                log(&format!("{}: holder gone; removing its record", u.id));
                self.remove_holder_files(&u.id, &u.path);
                status::publish_terminal(&u.id, TerminalChange::Removed, None);
            }
            Retry::Failed(rec, why) => {
                let u = &mut list[pos];
                u.attempts += 1;
                u.reason = why;
                if rec.is_some() {
                    u.rec = rec;
                }
                if last {
                    u.gave_up = true;
                    log(&format!(
                        "{}: still unreachable after {} attempts ({}); no more retries, \
                         its id stays reserved until pod kill {}",
                        u.id, u.attempts, u.reason, u.id
                    ));
                }
            }
        }
        None
    }

    fn remove_holder_files(&self, id: &str, record: &std::path::Path) {
        let _ = std::fs::remove_file(record);
        let _ = std::fs::remove_file(record.with_extension("log"));
        status::remove_token(&self.config.state_dir, id);
    }

    /// The unreachable terminals, as listed by `terminal.list`.
    pub(super) fn unreachable_infos(&self) -> Vec<TerminalInfo> {
        self.unreachable
            .lock()
            .unwrap()
            .iter()
            .map(Unreachable::info)
            .collect()
    }

    /// `(id, reason)` when `r` names an unreachable terminal (for errors).
    pub(super) fn unreachable_reason(&self, r: &TerminalRef) -> Option<(String, String)> {
        self.unreachable
            .lock()
            .unwrap()
            .iter()
            .find(|u| u.matches(r))
            .map(|u| (u.id.clone(), u.reason.clone()))
    }

    /// `terminal.kill`/`terminal.rm` of an unreachable terminal: stop
    /// retrying, forget it and release its id (its record, log and token are
    /// removed, so the next start does not reserve it). The holder process is
    /// not touched: its pid was never confirmed by a hello and may belong to
    /// another process by now.
    pub(super) fn give_up_unreachable(&self, r: &TerminalRef) -> Option<TerminalInfo> {
        let u = {
            let mut list = self.unreachable.lock().unwrap();
            let pos = list.iter().position(|u| u.matches(r))?;
            list.remove(pos)
        };
        let info = u.info();
        self.remove_holder_files(&u.id, &u.path);
        self.forget_task_terminal(&u.id);
        self.forget_kept(&u.id);
        status::publish_terminal(&u.id, TerminalChange::Removed, None);
        log(&format!(
            "gave up unreachable {} (holder pid {:?} left alone)",
            u.id, info.holder_pid
        ));
        Some(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_waits_double_up_to_a_cap() {
        let b = Duration::from_secs(2);
        let waits: Vec<u64> = (0..8).map(|n| retry_wait(n, b).as_secs()).collect();
        assert_eq!(waits, [2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(retry_wait(u32::MAX, b), MAX_RETRY_WAIT);
    }

    #[test]
    fn unreachable_info_says_why_and_matches_id_and_name() {
        let rec = HolderRecord {
            id: "t3".into(),
            name: Some("agent".into()),
            pipe: r"\\.\pipe\x".into(),
            holder_pid: 11,
            child_pid: 12,
            created_ms: 5,
        };
        let mut u = Unreachable::new("t3".into(), "t3.json".into(), Some(rec), "busy".into());
        let i = u.info();
        assert_eq!(i.state, TerminalState::Unreachable);
        assert_eq!((i.holder_pid, i.pid, i.created_ms), (Some(11), Some(12), 5));
        assert!(i.unreachable.as_deref().unwrap().contains("busy"), "{i:?}");
        u.gave_up = true;
        assert!(u.info().unreachable.unwrap().contains("gave up"));
        assert!(u.matches(&TerminalRef::parse("t3")));
        assert!(u.matches(&TerminalRef::parse("agent")));
        assert!(!u.matches(&TerminalRef::parse("t4")));
        // Clients before v19 see it as a lost exited terminal.
        let old = u.info().for_protocol(18);
        assert_eq!((old.state, old.lost), (TerminalState::Exited, true));
        assert_eq!(u.info().for_protocol(19).state, TerminalState::Unreachable);
    }

    fn g(dirty: u32, ahead: u32) -> TaskGit {
        TaskGit {
            dirty,
            ahead,
            ..Default::default()
        }
    }

    #[test]
    fn git_baseline_then_changes() {
        let id = "lifecycle-test/git";
        assert!(!store_git(id, &g(0, 0)), "first facts are a baseline");
        assert!(!store_git(id, &g(0, 0)));
        assert!(store_git(id, &g(1, 0)), "dirty changed");
        assert!(store_git(id, &g(1, 1)), "ahead changed");
        assert!(!store_git(id, &g(1, 1)));
        snaps().lock().unwrap().remove(id);
    }

    #[test]
    fn status_changes_are_detected() {
        let id = "lifecycle-test/status";
        let st = |state: &str| {
            Some(TaskStatus {
                state: state.into(),
                kind: None,
                terminal: "t1".into(),
                badge: state.into(),
                terminals: 1,
            })
        };
        assert!(store_status(id, &st("idle")), "unknown -> idle");
        assert!(!store_status(id, &st("idle")));
        assert!(store_status(id, &st("working")));
        assert!(store_status(id, &None), "status gone");
        snaps().lock().unwrap().remove(id);
    }
}
