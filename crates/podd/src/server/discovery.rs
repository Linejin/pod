//! Session discovery in the daemon (docs/18): `sessions.*` RPCs, the scan
//! worker and the `sessions.changed` event.
//!
//! State lives in `<state_dir>\sessions\`: `sessions.json` (settings, hidden
//! keys, links) and `cache.json` (the scan cache with the session metadata).
//! Discovery is **off** until the user turns it on; while off nothing is
//! read from the agents' stores or the process list. Everything read from the
//! agents is read-only (`pod-discover`).
//!
//! Persistence failures are reported, never swallowed (security-04):
//! turning discovery or previews off takes effect in memory at once, and an
//! error (`PERSIST_FAILED`) says what is still on disk; the cleanup and the
//! settings write are retried on every later `sessions.*` call and at start.
//! Files that may hold session summaries are only `cache.json`, its
//! `cache.json.tmp` and a moved-aside `cache.corrupt-*.json` of older
//! versions: a temporary file is removed when its write fails, an unreadable
//! cache is deleted (it is rebuilt by the next scan), and all three are
//! deleted when discovery is turned off. `sessions.json` holds only
//! settings, hidden keys and links; an unreadable one is moved aside.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use pod_discover::matching::{RepoPath, TaskPath, match_cwd};
use pod_discover::{Agent, Cache, Found, Limits, RunningAgent, session_key};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::status::{lifecycle_subscribers, publish_sessions};
use super::{Daemon, log, params, to_value};
use crate::protocol::*;

/// `(name, path)` of repositories and `(id, repo, worktree)` of tasks, paths
/// [`crate::git::normalize`]d.
type MatchInputs = (Vec<(String, String)>, Vec<(String, String, String)>);

/// Session cwds normalized once per view: a cwd recorded in another
/// spelling (8.3 short name, letter case) still matches the registered paths.
type CwdCache = BTreeMap<String, String>;

fn normalized(p: &str) -> String {
    crate::git::normalize(Path::new(p.trim()))
        .display()
        .to_string()
}

/// The repository / task worktree a session's `cwd` belongs to, compared in
/// normalized spelling (`repos`/`tasks` from [`Daemon::match_inputs`]).
fn match_session_cwd(
    cwd: &str,
    repos: &[(String, String)],
    tasks: &[(String, String, String)],
    cwds: &mut CwdCache,
) -> CwdMatch {
    if cwd.trim().is_empty() {
        return CwdMatch::default();
    }
    let rp: Vec<RepoPath> = repos
        .iter()
        .map(|(n, p)| RepoPath { name: n, path: p })
        .collect();
    let tp: Vec<TaskPath> = tasks
        .iter()
        .map(|(i, r, w)| TaskPath {
            id: i,
            repo: r,
            worktree: w,
        })
        .collect();
    let cwd = cwds
        .entry(cwd.to_string())
        .or_insert_with(|| normalized(cwd));
    match_cwd(cwd, &rp, &tp)
}

/// Sessions idle longer than this are "오래됨".
pub const STALE_AFTER_MS: u64 = 30 * 24 * 3600 * 1000;
/// Background rescan period (only while on and a lifecycle subscriber is connected).
const RESCAN: Duration = Duration::from_secs(120);

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    enabled: bool,
    #[serde(default = "default_true")]
    preview: bool,
    #[serde(default)]
    hidden: BTreeSet<String>,
    #[serde(default)]
    links: BTreeMap<String, CwdMatch>,
}

impl Default for Stored {
    fn default() -> Self {
        Stored {
            version: 1,
            enabled: false,
            preview: true,
            hidden: BTreeSet::new(),
            links: BTreeMap::new(),
        }
    }
}

#[derive(Default)]
struct State {
    dir: PathBuf,
    stored: Stored,
    cache: Cache,
    found: Vec<Found>,
    running: Vec<RunningAgent>,
    scanned_ms: Option<u64>,
    truncated: bool,
    /// What the last event announced (count, running), to publish on change only.
    announced: Option<(bool, usize, usize)>,
    /// `sessions.json` differs from `stored` (a write failed).
    settings_dirty: bool,
    /// The cache files differ from memory: to be written (on) or deleted
    /// (off) by [`State::persist`].
    cache_dirty: bool,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// One scan at a time.
fn scan_lock() -> &'static tokio::sync::Mutex<()> {
    static L: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    L.get_or_init(Default::default)
}

/// `private`: the file may hold session summaries; an unreadable one is
/// deleted instead of moved aside (the next scan rebuilds it).
fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path, private: bool) -> T {
    let Ok(bytes) = std::fs::read(path) else {
        return T::default();
    };
    match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) if private => {
            let gone = std::fs::remove_file(path);
            log(&format!(
                "sessions: {} unreadable ({e}); deleted: {gone:?}",
                path.display()
            ));
            T::default()
        }
        Err(e) => {
            // Keep the broken file for inspection; start empty.
            let aside = path.with_extension(format!("corrupt-{}.json", crate::now_ms()));
            log(&format!(
                "sessions: {} unreadable ({e}); moved to {}",
                path.display(),
                aside.display()
            ));
            let _ = std::fs::rename(path, aside);
            T::default()
        }
    }
}

/// Through `<name>.json.tmp` + rename; a failed write removes the temporary
/// file (it may hold summaries).
fn write_json<T: Serialize>(path: &Path, v: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let res = serde_json::to_vec_pretty(v)
        .map_err(|e| e.to_string())
        .and_then(|b| std::fs::write(&tmp, b).map_err(|e| e.to_string()))
        .and_then(|_| std::fs::rename(&tmp, path).map_err(|e| e.to_string()));
    if let Err(e) = res {
        let _ = std::fs::remove_file(&tmp);
        log(&format!("sessions: cannot write {}: {e}", path.display()));
        return Err(format!("{}: {e}", path.display()));
    }
    Ok(())
}

fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The files in `dir` that may hold session summaries (see the module
/// doc), `cache.json` itself only with `main`; the paths that could not be
/// deleted come back.
fn remove_cache_files(dir: &Path, main: bool) -> Vec<(PathBuf, String)> {
    let mut files = vec![dir.join("cache.json.tmp")];
    if main {
        files.push(dir.join("cache.json"));
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        files.extend(entries.flatten().map(|e| e.path()).filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("cache.corrupt-") && n.ends_with(".json"))
        }));
    }
    files
        .into_iter()
        .filter_map(|f| remove_if_present(&f).err().map(|e| (f, e)))
        .collect()
}

impl State {
    fn save_settings(&self) -> Result<(), String> {
        write_json(&self.dir.join("sessions.json"), &self.stored)
    }
    fn save_cache(&self) -> Result<(), String> {
        write_json(&self.dir.join("cache.json"), &self.cache)
    }

    /// Bring the disk in line with memory: settings saved, and nothing that
    /// memory has forgotten left in the cache files. Errors name each path.
    fn persist(&mut self) -> Result<(), RpcError> {
        let mut failed: Vec<(PathBuf, String)> = Vec::new();
        if self.settings_dirty {
            match self.save_settings() {
                Ok(()) => self.settings_dirty = false,
                Err(e) => failed.push((self.dir.join("sessions.json"), e)),
            }
        }
        if self.cache_dirty {
            let mut left = Vec::new();
            if self.stored.enabled {
                if let Err(e) = self.save_cache() {
                    left.push((self.dir.join("cache.json"), e));
                }
                if !self.stored.preview {
                    left.extend(remove_cache_files(&self.dir, false));
                }
            } else {
                left = remove_cache_files(&self.dir, true);
            }
            let res = if left.is_empty() { Ok(()) } else { Err(left) };
            match res {
                Ok(()) => self.cache_dirty = false,
                Err(mut v) => failed.append(&mut v),
            }
        }
        if failed.is_empty() {
            return Ok(());
        }
        let what = failed
            .iter()
            .map(|(_, e)| e.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        let mut e = RpcError::new(
            sessions_codes::PERSIST_FAILED,
            format!(
                "the change is in effect, but the disk could not be updated (session summaries may remain until this succeeds; Pod retries on the next sessions call): {what}"
            ),
        );
        e.data = Some(serde_json::json!({
            "paths": failed.iter().map(|(p, _)| p.display().to_string()).collect::<Vec<_>>(),
            "error": what,
        }));
        Err(e)
    }
}

/// Load the state and start the rescan worker.
pub(super) fn start(daemon: &Arc<Daemon>) {
    {
        let mut st = state().lock().unwrap();
        st.dir = daemon.config.state_dir.join("sessions");
        st.stored = read_json(&st.dir.join("sessions.json"), false);
        if !st.stored.enabled {
            // An earlier "off" whose cleanup failed, or leftovers of older
            // versions: nothing read from the agents stays while off.
            st.cache_dirty = true;
            if let Err(e) = st.persist() {
                log(&format!("sessions: {}", e.message));
            }
        } else {
            st.cache = read_json(&st.dir.join("cache.json"), true);
            let _ = remove_if_present(&st.dir.join("cache.json.tmp"));
            if !st.stored.preview {
                st.cache.strip_previews();
                st.cache_dirty = true;
                if let Err(e) = st.persist() {
                    log(&format!("sessions: {}", e.message));
                }
            }
            // Show what the last scan knew until the first new scan ends.
            st.found = st
                .cache
                .files
                .iter()
                .filter_map(|(path, c)| {
                    Some(Found {
                        agent: c.agent,
                        meta: c.meta.clone()?,
                        path: PathBuf::from(path),
                        mtime_ms: c.mtime_ms,
                    })
                })
                .collect();
            st.found
                .sort_by_key(|f| std::cmp::Reverse(pod_discover::last_activity(f)));
        }
    }
    let d = Arc::downgrade(daemon);
    tokio::spawn(async move {
        // First scan shortly after start, then periodically while someone listens.
        let mut first = true;
        loop {
            tokio::time::sleep(if first {
                Duration::from_secs(5)
            } else {
                RESCAN
            })
            .await;
            let Some(daemon) = d.upgrade() else { return };
            let on = state().lock().unwrap().stored.enabled;
            if on && (first || lifecycle_subscribers() > 0) {
                daemon.sessions_rescan().await;
            }
            first = false;
        }
    });
}

pub(super) fn handles(method: &str) -> bool {
    method.starts_with("sessions.")
}

fn not_found(s: &str) -> RpcError {
    RpcError::new(
        sessions_codes::SESSION_NOT_FOUND,
        format!("no discovered session {s:?} (run `pod sessions scan`)"),
    )
}

fn off() -> RpcError {
    RpcError::new(
        sessions_codes::DISCOVERY_OFF,
        "session discovery is off (turn it on with `pod sessions scan --enable`)",
    )
}

/// Unix ms of "now" for staleness.
fn now() -> u64 {
    crate::now_ms()
}

/// Resolve `"<agent>:<id>"` or a unique id prefix to a key.
fn resolve(found: &[Found], s: &str) -> Result<(Agent, String), RpcError> {
    let s = s.trim().to_ascii_lowercase();
    if let Some((a, id)) = s.split_once(':')
        && let Some(agent) = Agent::parse(a)
    {
        return found
            .iter()
            .find(|f| f.agent == agent && f.meta.id == id)
            .map(|f| (f.agent, f.meta.id.clone()))
            .ok_or_else(|| not_found(&s));
    }
    let hits: Vec<&Found> = found
        .iter()
        .filter(|f| !s.is_empty() && f.meta.id.starts_with(&s))
        .collect();
    match hits[..] {
        [one] => Ok((one.agent, one.meta.id.clone())),
        [] => Err(not_found(&s)),
        _ => Err(RpcError::new(
            sessions_codes::SESSION_NOT_FOUND,
            format!("{s:?} matches {} sessions; use <agent>:<id>", hits.len()),
        )),
    }
}

/// The resume program for an agent (`POD_SESSIONS_<AGENT>_EXE` for tests).
fn resume_exe(agent: Agent) -> Option<String> {
    let var = match agent {
        Agent::Claude => "POD_SESSIONS_CLAUDE_EXE",
        Agent::Codex => "POD_SESSIONS_CODEX_EXE",
    };
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

impl Daemon {
    /// Repositories and task worktrees for cwd matching.
    fn match_inputs(&self) -> MatchInputs {
        let (repos, tasks): MatchInputs = {
            let reg = self.registry.lock().unwrap();
            (
                reg.repos
                    .iter()
                    .map(|r| (r.name.clone(), r.path.clone()))
                    .collect(),
                reg.tasks
                    .iter()
                    .map(|t| (t.id.clone(), t.repo.clone(), t.worktree.clone()))
                    .collect(),
            )
        };
        (
            repos
                .into_iter()
                .map(|(n, p)| (n, normalized(&p)))
                .collect(),
            tasks
                .into_iter()
                .map(|(i, r, w)| (i, r, normalized(&w)))
                .collect(),
        )
    }

    /// Build the wire view of one session.
    fn session_view(
        st: &State,
        f: &Found,
        repos: &[(String, String)],
        tasks: &[(String, String, String)],
        cwds: &mut CwdCache,
    ) -> DiscoveredSession {
        let key = session_key(f.agent, &f.meta.id);
        let matched = match_session_cwd(&f.meta.cwd, repos, tasks, cwds);
        let running = st.running.iter().find(|r| {
            r.agent == Some(f.agent) && r.session_id.as_deref() == Some(f.meta.id.as_str())
        });
        let cwd_exists = Path::new(&f.meta.cwd).is_dir();
        let last = pod_discover::last_activity(f);
        let state = match running {
            Some(r) if r.pod_terminal.is_some() => SessionState::RunningPod,
            Some(_) => SessionState::RunningOther,
            None if !cwd_exists || now().saturating_sub(last) > STALE_AFTER_MS => {
                SessionState::Stale
            }
            None => SessionState::Resumable,
        };
        DiscoveredSession {
            agent: f.agent,
            id: f.meta.id.clone(),
            cwd: f.meta.cwd.clone(),
            title: f.meta.title.clone(),
            preview: if st.stored.preview {
                f.meta.preview.clone()
            } else {
                None
            },
            model: f.meta.model.clone(),
            branch: f.meta.branch.clone(),
            version: f.meta.version.clone(),
            origin: f.meta.origin.clone(),
            started_ms: f.meta.started_ms,
            last_ms: Some(last),
            messages: f.meta.messages,
            state,
            running: running.map(|r| SessionRunning {
                pid: r.pid,
                pod_terminal: r.pod_terminal.clone(),
                guessed: r.guessed,
            }),
            matched,
            link: st.stored.links.get(&key).cloned(),
            hidden: st.stored.hidden.contains(&key),
            cwd_exists,
            resume_argv: pod_discover::resume::resume_argv(
                f.agent,
                &f.meta.id,
                resume_exe(f.agent).as_deref(),
            )
            .unwrap_or_default(),
            key,
        }
    }

    fn sessions_view(&self, include_hidden: bool) -> SessionsList {
        let (repos, tasks) = self.match_inputs();
        let st = state().lock().unwrap();
        if !st.stored.enabled {
            return SessionsList {
                enabled: false,
                preview: st.stored.preview,
                ..Default::default()
            };
        }
        let mut cwds = CwdCache::new();
        let all: Vec<DiscoveredSession> = st
            .found
            .iter()
            .map(|f| Self::session_view(&st, f, &repos, &tasks, &mut cwds))
            .collect();
        let hidden = all.iter().filter(|s| s.hidden).count();
        let sessions = all
            .into_iter()
            .filter(|s| include_hidden || !s.hidden)
            .collect();
        let unknown_running = st
            .running
            .iter()
            .filter(|r| {
                r.session_id.is_none()
                    || !st.found.iter().any(|f| {
                        Some(f.agent) == r.agent && Some(&f.meta.id) == r.session_id.as_ref()
                    })
            })
            .map(|r| UnknownRunning {
                agent: r.agent,
                pid: r.pid,
                cwd: r.cwd.clone(),
                pod_terminal: r.pod_terminal.clone(),
            })
            .collect();
        SessionsList {
            enabled: true,
            preview: st.stored.preview,
            scanned_ms: st.scanned_ms,
            truncated: st.truncated,
            sessions,
            hidden,
            unknown_running,
        }
    }

    fn one_view(&self, agent: Agent, id: &str) -> Result<DiscoveredSession, RpcError> {
        let (repos, tasks) = self.match_inputs();
        let st = state().lock().unwrap();
        let f = st
            .found
            .iter()
            .find(|f| f.agent == agent && f.meta.id == id)
            .ok_or_else(|| not_found(id))?;
        Ok(Self::session_view(
            &st,
            f,
            &repos,
            &tasks,
            &mut CwdCache::new(),
        ))
    }

    /// Publish `sessions.changed` when the list or settings changed.
    fn sessions_announce(&self, force: bool) {
        let (enabled, count, running) = {
            let st = state().lock().unwrap();
            (
                st.stored.enabled,
                st.found.len(),
                st.running.iter().filter(|r| r.session_id.is_some()).count(),
            )
        };
        let now = (enabled, count, running);
        let mut st = state().lock().unwrap();
        if force || st.announced != Some(now) {
            st.announced = Some(now);
            drop(st);
            publish_sessions(SessionsChangedEvent {
                seq: 0,
                enabled,
                count,
                running,
            });
        }
    }

    /// Scan the stores and processes (low priority, bounded). No-op when off.
    pub(super) async fn sessions_rescan(&self) {
        let _one = scan_lock().lock().await;
        let (preview, mut cache) = {
            let mut st = state().lock().unwrap();
            if !st.stored.enabled {
                return;
            }
            (st.stored.preview, std::mem::take(&mut st.cache))
        };
        // Pod's own terminals: holder and child pids, for de-duplication.
        let pod: Vec<(String, Vec<u32>)> = self
            .list()
            .into_iter()
            .filter(|t| t.state == TerminalState::Running)
            .map(|t| (t.id, t.pid.into_iter().chain(t.holder_pid).collect()))
            .collect();
        let processes = !matches!(
            std::env::var("POD_DISCOVER_PROCESSES").as_deref(),
            Ok("off" | "0")
        );
        let started = std::time::Instant::now();
        let res = tokio::task::spawn_blocking(move || {
            let _bg = Background::begin();
            let mut sources = pod_discover::default_sources();
            let limits = Limits {
                preview,
                ..Limits::default()
            };
            let out = pod_discover::scan(&mut sources, &mut cache, &limits);
            let mut agents = if processes {
                let procs = pod_discover::procs::snapshot();
                let mut a = pod_discover::procs::find_agents(&procs);
                pod_discover::procs::mark_pod_terminals(&mut a, &procs, &pod);
                a
            } else {
                Vec::new()
            };
            pod_discover::procs::attach_sessions(&mut agents, &out.running_ids, &out.sessions);
            (out, agents, cache)
        })
        .await;
        let Ok((out, agents, cache)) = res else {
            log("sessions: scan worker failed");
            return;
        };
        log(&format!(
            "sessions: scan {} ms, {} sessions ({} parsed, {} cached, {} KiB read{}), {} running",
            started.elapsed().as_millis(),
            out.sessions.len(),
            out.parsed,
            out.cached,
            out.bytes_read / 1024,
            if out.truncated { ", truncated" } else { "" },
            agents.len()
        ));
        {
            let mut st = state().lock().unwrap();
            if !st.stored.enabled {
                // Turned off while scanning: forget the result.
                return;
            }
            st.cache = cache;
            if !st.stored.preview {
                st.cache.strip_previews();
            }
            st.found = out.sessions;
            if !st.stored.preview {
                for f in st.found.iter_mut() {
                    f.meta.preview = None;
                }
            }
            st.running = agents;
            st.scanned_ms = Some(crate::now_ms());
            st.truncated = out.truncated;
            st.cache_dirty = true;
            if let Err(e) = st.persist() {
                log(&format!("sessions: after the scan: {}", e.message));
            }
        }
        self.sessions_announce(false);
    }

    /// Applies at once in memory (off forgets everything read); the disk
    /// follows, and a failure there is the error (security-04).
    fn sessions_settings(&self, p: SessionsSettingsParams) -> Result<(), RpcError> {
        let mut st = state().lock().unwrap();
        if let Some(on) = p.enabled {
            st.stored.enabled = on;
            if !on {
                // Forget everything read from the agents; keep the user's choices.
                st.cache = Cache::default();
                st.found.clear();
                st.running.clear();
                st.scanned_ms = None;
                st.truncated = false;
                st.cache_dirty = true;
            }
        }
        if let Some(pv) = p.preview {
            st.stored.preview = pv;
            if !pv {
                st.cache.strip_previews();
                for f in st.found.iter_mut() {
                    f.meta.preview = None;
                }
                st.cache_dirty = true;
            }
        }
        st.settings_dirty = true;
        st.persist()
    }

    async fn sessions_register(
        &self,
        p: SessionsRegisterParams,
    ) -> Result<DiscoveredSession, RpcError> {
        let (agent, id) = resolve(&state().lock().unwrap().found, &p.session)?;
        let view = self.one_view(agent, &id)?;
        let link = if let Some(task) = p.task.as_deref() {
            let rec = self.task_record(task)?;
            CwdMatch {
                repo: Some(rec.repo),
                task: Some(rec.id),
            }
        } else if !view.matched.is_empty() {
            view.matched.clone()
        } else if p.add_repo {
            let cwd = PathBuf::from(&view.cwd);
            let root = tokio::task::spawn_blocking(move || main_checkout(&cwd))
                .await
                .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, e.to_string()))?
                .map_err(|m| RpcError::new(codes::NOT_A_REPO, m))?;
            let taken: Vec<String> = self
                .registry
                .lock()
                .unwrap()
                .repos
                .iter()
                .map(|r| r.name.clone())
                .collect();
            let name = repo_name_for(&root, &taken);
            let info = self
                .repo_add(RepoAddParams {
                    path: root.display().to_string(),
                    name: Some(name),
                })
                .await?;
            // A linked worktree of that repository may be a Pod task: match again.
            let again = self.one_view(agent, &id)?.matched;
            if again.is_empty() {
                CwdMatch {
                    repo: Some(info.name),
                    task: None,
                }
            } else {
                again
            }
        } else {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!(
                    "no registered repository holds {}; register it (add_repo)",
                    view.cwd
                ),
            ));
        };
        {
            let mut st = state().lock().unwrap();
            st.stored.links.insert(view.key.clone(), link);
            st.settings_dirty = true;
            st.persist()?;
        }
        log(&format!("sessions: registered {}", view.key));
        self.sessions_announce(true);
        self.one_view(agent, &id)
    }

    async fn sessions_resume(
        self: Arc<Self>,
        p: SessionsResumeParams,
    ) -> Result<TerminalInfo, RpcError> {
        let (agent, id) = resolve(&state().lock().unwrap().found, &p.session)?;
        let view = self.one_view(agent, &id)?;
        if let Some(r) = &view.running {
            let mut e = RpcError::new(
                sessions_codes::SESSION_RUNNING,
                match &r.pod_terminal {
                    Some(t) => format!("{} is running in Pod terminal {t}", view.key),
                    None => format!(
                        "{} is running in another app's terminal (pid {}); resume it after it ends",
                        view.key, r.pid
                    ),
                },
            );
            e.data = Some(serde_json::json!({ "pid": r.pid, "pod_terminal": r.pod_terminal }));
            return Err(e);
        }
        if !view.cwd_exists {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!("the session folder {} no longer exists", view.cwd),
            ));
        }
        // The linked (else matching) task, when it still exists.
        let task = view
            .link
            .as_ref()
            .and_then(|l| l.task.clone())
            .or_else(|| view.matched.task.clone())
            .and_then(|t| self.task_record(&t).ok());
        let info = match task {
            // The session ran at the root of a Pod task worktree: that is the
            // task's own agent session, so resume it the way `task.resume`
            // does (agent terminal, recorded `agent_session`, Codex trust for
            // the Pod worktree).
            Some(rec) if crate::git::same_path(Path::new(&rec.worktree), Path::new(&view.cwd)) => {
                let r = self
                    .task_resume(TaskResumeParams {
                        task: rec.id.clone(),
                        session: Some(id.clone()),
                        agent: Some(agent.as_str().into()),
                        ..Default::default()
                    })
                    .await?;
                self.update_task(&rec.id, |t| match t.agent_session.as_mut() {
                    Some(s) if s.id == id => {
                        s.source = "discovery".into();
                        true
                    }
                    _ => false,
                });
                r.terminal.ok_or_else(|| {
                    RpcError::new(codes::INTERNAL_ERROR, "task.resume started no terminal")
                })?
            }
            // Elsewhere (a repository checkout, a sub-folder, an unknown
            // folder): the same resume command in the session's cwd, joined to
            // the task if any. No Codex folder trust is granted here: only
            // Pod-created worktrees are trusted automatically.
            task => {
                let argv =
                    pod_discover::resume::resume_argv(agent, &id, resume_exe(agent).as_deref())
                        .map_err(|m| RpcError::new(codes::INVALID_PARAMS, m))?;
                self.run_in_task(RunParams {
                    argv,
                    cwd: Some(view.cwd.clone()),
                    name: p.name,
                    task: task.map(|t| t.id),
                    ..Default::default()
                })
                .await?
            }
        };
        log(&format!("sessions: resumed {} in {}", view.key, info.id));
        // Pick the new process up soon (it now runs inside Pod).
        let d = Arc::downgrade(&self);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            if let Some(d) = d.upgrade() {
                d.sessions_rescan().await;
            }
        });
        Ok(info)
    }

    fn sessions_hide(&self, p: SessionsHideParams) -> Result<DiscoveredSession, RpcError> {
        let (agent, id) = resolve(&state().lock().unwrap().found, &p.session)?;
        {
            let mut st = state().lock().unwrap();
            let key = session_key(agent, &id);
            if p.hidden.unwrap_or(true) {
                st.stored.hidden.insert(key);
            } else {
                st.stored.hidden.remove(&key);
            }
            st.settings_dirty = true;
            st.persist()?;
        }
        self.sessions_announce(true);
        self.one_view(agent, &id)
    }
}

/// A valid (`[A-Za-z0-9._-]`), unused repository name from a folder name:
/// other characters become `-`, a taken name gets `-2`, `-3`, ...
fn repo_name_for(root: &Path, taken: &[String]) -> String {
    let raw = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut base: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    base = base.trim_matches(|c| c == '-' || c == '.').to_string();
    if base.is_empty() {
        base = "repo".into();
    }
    let mut name = base.clone();
    let mut n = 2;
    while taken.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
        name = format!("{base}-{n}");
        n += 1;
    }
    name
}

/// The main checkout holding `dir` (for a linked worktree: the repository
/// owning it), for `repo.add`.
fn main_checkout(dir: &Path) -> Result<PathBuf, String> {
    let s = crate::git::run(
        dir,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
            "--show-toplevel",
        ],
    )?;
    let lines: Vec<&str> = s.lines().collect();
    let [common, top] = lines[..] else {
        return Err(format!("{} is not inside a git work tree", dir.display()));
    };
    let common = crate::git::native_path(common);
    if common
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case(".git"))
        && let Some(parent) = common.parent()
    {
        return Ok(parent.to_path_buf());
    }
    Ok(crate::git::native_path(top))
}

pub(super) async fn dispatch(
    daemon: Arc<Daemon>,
    method: &str,
    p: Value,
) -> Result<Value, RpcError> {
    // Retry a cleanup or settings write that failed earlier (security-04).
    // A write method reports its own result below; a read only logs.
    {
        let mut st = state().lock().unwrap();
        if (st.settings_dirty || st.cache_dirty)
            && let Err(e) = st.persist()
        {
            log(&format!("sessions: {}", e.message));
        }
    }
    match method {
        SessionsListMethod::NAME => {
            let p: SessionsListParams = params(p)?;
            to_value(daemon.sessions_view(p.include_hidden))
        }
        SessionsScan::NAME => {
            let p: SessionsScanParams = params(p)?;
            if p.enable {
                daemon.sessions_settings(SessionsSettingsParams {
                    enabled: Some(true),
                    preview: None,
                })?;
            }
            if !state().lock().unwrap().stored.enabled {
                return Err(off());
            }
            daemon.sessions_rescan().await;
            to_value(daemon.sessions_view(p.include_hidden))
        }
        SessionsSettings::NAME => {
            let p: SessionsSettingsParams = params(p)?;
            let was = state().lock().unwrap().stored.enabled;
            let turning_on = p.enabled == Some(true) && !was;
            let saved = daemon.sessions_settings(p);
            daemon.sessions_announce(true);
            saved?;
            if turning_on {
                let d = daemon.clone();
                tokio::spawn(async move { d.sessions_rescan().await });
            }
            to_value(daemon.sessions_view(false))
        }
        SessionsRegister::NAME => {
            if !state().lock().unwrap().stored.enabled {
                return Err(off());
            }
            to_value(daemon.sessions_register(params(p)?).await?)
        }
        SessionsResume::NAME => {
            if !state().lock().unwrap().stored.enabled {
                return Err(off());
            }
            to_value(daemon.sessions_resume(params(p)?).await?)
        }
        SessionsHide::NAME => {
            if !state().lock().unwrap().stored.enabled {
                return Err(off());
            }
            to_value(daemon.sessions_hide(params(p)?)?)
        }
        other => Err(RpcError::new(
            codes::METHOD_NOT_FOUND,
            format!("unknown method {other:?}"),
        )),
    }
}

/// Windows background mode (low CPU and I/O priority) for the scanning
/// thread, undone on drop (blocking-pool threads are reused).
struct Background(bool);

impl Background {
    fn begin() -> Background {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
            };
            // SAFETY: pseudo-handle of the calling thread.
            Background(
                unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN) } != 0,
            )
        }
        #[cfg(not(windows))]
        Background(false)
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.0 {
            use windows_sys::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_END,
            };
            // SAFETY: as above.
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_END) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_names_are_valid_and_unique() {
        assert_eq!(
            repo_name_for(Path::new(r"C:\src\live repo"), &[]),
            "live-repo"
        );
        assert_eq!(
            repo_name_for(Path::new(r"C:\src\pod"), &["pod".into(), "pod-2".into()]),
            "pod-3"
        );
        assert_eq!(repo_name_for(Path::new(r"C:\src\한글"), &[]), "repo");
        assert_eq!(repo_name_for(Path::new(r"C:\src\.x."), &[]), "x");
    }

    /// A session cwd recorded through an 8.3 short name (GitHub runners'
    /// `RUNNER~1`) matches the repository and task registered in the long
    /// spelling, and a registry written in the short spelling (older state)
    /// still matches a long cwd.
    #[test]
    fn session_cwds_match_in_any_spelling() {
        let t = std::env::temp_dir().join(format!("podd-disc-short-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        std::fs::create_dir_all(t.join("LongRepoFolder").join("sub")).unwrap();
        std::fs::create_dir_all(t.join("LongTaskFolder")).unwrap();
        let repo = crate::git::normalize(&t.join("LongRepoFolder"));
        let wt = crate::git::normalize(&t.join("LongTaskFolder"));
        let (Some(short_repo), Some(short_wt)) = (
            crate::git::short_spelling(&repo),
            crate::git::short_spelling(&wt),
        ) else {
            eprintln!("skipped: no 8.3 short names on this volume");
            let _ = std::fs::remove_dir_all(&t);
            return;
        };
        let s = |p: &Path| p.display().to_string();
        let mut cwds = CwdCache::new();
        // Registered long (as Pod records them now), cwd short.
        let repos = vec![("demo".to_string(), normalized(&s(&repo)))];
        let tasks = vec![(
            "demo/t".to_string(),
            "demo".to_string(),
            normalized(&s(&wt)),
        )];
        let m = match_session_cwd(&s(&short_repo.join("sub")), &repos, &tasks, &mut cwds);
        assert_eq!((m.repo.as_deref(), m.task), (Some("demo"), None));
        let m = match_session_cwd(&s(&short_wt), &repos, &tasks, &mut cwds);
        assert_eq!(m.task.as_deref(), Some("demo/t"));
        // Registered short (older state), cwd long.
        let repos = vec![("demo".to_string(), normalized(&s(&short_repo)))];
        let m = match_session_cwd(&s(&repo.join("sub")), &repos, &[], &mut cwds);
        assert_eq!(m.repo.as_deref(), Some("demo"));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn resolves_keys_and_prefixes() {
        let f = |agent, id: &str| Found {
            agent,
            meta: pod_discover::SessionMeta {
                id: id.into(),
                ..Default::default()
            },
            path: PathBuf::new(),
            mtime_ms: 0,
        };
        let found = [
            f(Agent::Claude, "aaaa-1"),
            f(Agent::Codex, "aaaa-2"),
            f(Agent::Codex, "bbbb"),
        ];
        assert_eq!(
            resolve(&found, "codex:aaaa-2").unwrap(),
            (Agent::Codex, "aaaa-2".to_string())
        );
        assert_eq!(
            resolve(&found, " BBB ").unwrap(),
            (Agent::Codex, "bbbb".to_string())
        );
        assert!(resolve(&found, "aaaa").is_err(), "ambiguous");
        assert!(resolve(&found, "").is_err());
        assert!(resolve(&found, "claude:bbbb").is_err());
    }
}
