//! Worktree sessions (docs/19 S1–S2): auto names with retired-name
//! tracking, qualified base refs, brief files with a safe first line, open
//! hints, the setup terminal and default tabs, typed brief delivery and the
//! agent session id taken from hook payloads.
//!
//! `task.create` itself stays in `tasks.rs`; this module holds the pieces it
//! calls and the background work that follows a create (setup watcher,
//! typed brief). Background work needs an `Arc<Daemon>`; [`start`] keeps a
//! weak reference for that.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::repo_config::{self, Effective};
use super::status::{publish_task, task_event};
use super::tasks::{ENV_TASK_ID, RepoRecord, TaskRecord, blocking};
use super::{Daemon, log};
use crate::git;
use crate::protocol::*;

/// Environment variable with the brief file's path.
pub const ENV_BRIEF: &str = "POD_BRIEF";
/// Version of the header Pod writes at the top of a brief file.
pub const BRIEF_VERSION: u32 = 1;
/// The agent's first input when a brief is given: no quotes, `%` or line
/// breaks, and no path, so it survives `.cmd` shims and any cmd.exe quoting.
pub const DEFAULT_FIRST_LINE: &str =
    "Read the task brief in the file named by POD_BRIEF and carry it out.";
const MAX_FIRST_LINE: usize = 400;
/// Typed briefs: how long output must be quiet before typing (agents
/// without a status report).
const QUIET_BEFORE_TYPING: Duration = Duration::from_millis(1500);
/// Typed briefs: give up waiting for an agent that reports status this long
/// (it may sit in a dialog: typing would answer it).
const AGENT_READY_TIMEOUT: Duration = Duration::from_secs(600);
/// Typed briefs into programs without status: type after this at the latest.
const QUIET_READY_TIMEOUT: Duration = Duration::from_secs(60);

static DAEMON: OnceLock<Weak<Daemon>> = OnceLock::new();

fn daemon() -> Option<Arc<Daemon>> {
    DAEMON.get().and_then(Weak::upgrade)
}

/// After rediscovery: keep a handle for background work and pick up setup
/// runs and typed briefs a previous daemon left unfinished.
pub(super) fn start(d: &Arc<Daemon>) {
    let _ = DAEMON.set(Arc::downgrade(d));
    let tasks: Vec<TaskRecord> = d.registry.lock().unwrap().tasks.clone();
    for t in tasks {
        if let Some(s) = &t.setup
            && s.state == SetupState::Running
            && let Some(term) = &s.terminal
        {
            spawn_setup_watch(t.id.clone(), term.clone());
        } else if t
            .brief
            .as_ref()
            .is_some_and(|b| b.state == BriefState::Pending && b.delivery == BriefDelivery::Typed)
        {
            spawn_brief_delivery(t.id.clone());
        }
    }
}

// ───────────────────────────── names ─────────────────────────────

/// Short, distinct, easy to type and to say.
const WORDS: &[&str] = &[
    "amber", "aspen", "birch", "brook", "cedar", "cliff", "cloud", "coral", "delta", "dune",
    "ember", "fern", "fjord", "flint", "frost", "grove", "harbor", "hazel", "heath", "iris", "ivy",
    "juniper", "lagoon", "larch", "linden", "maple", "meadow", "mesa", "moss", "oak", "opal",
    "orchid", "pebble", "pine", "prairie", "quartz", "reef", "ridge", "river", "sage", "shore",
    "sierra", "slate", "spruce", "summit", "thistle", "tide", "tundra", "willow", "yarrow",
];

/// First free name: the words from `start` on, then `<word>-2`, `-3`, ...
pub fn suggest_name(start: usize, taken: impl Fn(&str) -> bool) -> String {
    for n in 1u32.. {
        for i in 0..WORDS.len() {
            let w = WORDS[(start + i) % WORDS.len()];
            let name = if n == 1 {
                w.to_string()
            } else {
                format!("{w}-{n}")
            };
            if !taken(&name) {
                return name;
            }
        }
    }
    unreachable!("names are unbounded")
}

/// Names of removed tasks per repository (`<state>\tasks\retired.json`).
/// Suggested names never reuse them: agents key their conversation history
/// by the working directory, so a reused worktree path would show an old
/// conversation in a new task.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Retired {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub repos: BTreeMap<String, BTreeSet<String>>,
}

impl Retired {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("tasks").join("retired.json")
    }

    pub fn load(state_dir: &Path) -> Self {
        std::fs::read(Self::path(state_dir))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn contains(&self, repo: &str, name: &str) -> bool {
        self.repos.get(repo).is_some_and(|s| s.contains(name))
    }

    /// Record a removed task's name (best effort; logged on failure).
    pub fn retire(state_dir: &Path, repo: &str, name: &str) {
        let mut r = Self::load(state_dir);
        if !r
            .repos
            .entry(repo.to_string())
            .or_default()
            .insert(name.to_string())
        {
            return;
        }
        r.version = 1;
        let path = Self::path(state_dir);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(&r)?)?;
            std::fs::rename(&tmp, &path)
        };
        if let Err(e) = write() {
            log(&format!("cannot save {}: {e}", path.display()));
        }
    }
}

fn random_index() -> usize {
    // Low bits of the clock: varied enough to spread suggestions, and the
    // taken-check makes any start correct.
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize ^ d.as_secs() as usize)
        .unwrap_or(0);
    t.wrapping_mul(2_654_435_761) % WORDS.len()
}

/// Worktree root of a repository (its own setting, else the daemon's),
/// normalized so task worktrees are recorded in git's spelling (a setting or
/// `--worktrees-root` may name it with an 8.3 short name).
pub(super) fn worktrees_root(daemon: &Daemon, repo: &RepoRecord) -> PathBuf {
    let root = repo
        .worktrees_root
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| daemon.config.worktrees_root.clone());
    crate::git::normalize(&root)
}

impl Daemon {
    /// A name no live or removed task of `repo` used, whose branch
    /// `pod/<name>` and worktree directory do not exist.
    pub(super) async fn suggest_task_name(&self, repo: &RepoRecord) -> Result<String, RpcError> {
        let retired = Retired::load(&self.config.state_dir);
        let live: BTreeSet<String> = self
            .registry
            .lock()
            .unwrap()
            .tasks
            .iter()
            .filter(|t| t.repo == repo.name)
            .map(|t| t.name.clone())
            .collect();
        let (repo_path, root, rname) = (
            PathBuf::from(&repo.path),
            worktrees_root(self, repo).join(&repo.name),
            repo.name.clone(),
        );
        let start = random_index();
        blocking(move || {
            suggest_name(start, |n| {
                live.contains(n)
                    || retired.contains(&rname, n)
                    || root.join(n).exists()
                    || git::branch_exists(&repo_path, &format!("pod/{n}"))
            })
        })
        .await
    }

    pub(super) async fn task_suggest_name(
        &self,
        p: SuggestNameParams,
    ) -> Result<SuggestNameResult, RpcError> {
        let repo = self.repo_record(&p.repo)?;
        Ok(SuggestNameResult {
            name: self.suggest_task_name(&repo).await?,
        })
    }
}

// ───────────────────────────── base refs ─────────────────────────────

/// Qualify `base` when the same name exists in several ref namespaces, so a
/// tag never shadows a branch: with a `/` the remote-tracking branch wins
/// (`origin/main`), else the local branch. Unambiguous names, commit ids and
/// expressions are returned unchanged.
pub fn qualify_base(repo: &Path, base: &str) -> String {
    if base.starts_with("refs/") || base.starts_with('-') || base.is_empty() {
        return base.to_string();
    }
    let order: [&str; 3] = if base.contains('/') {
        ["refs/remotes/", "refs/heads/", "refs/tags/"]
    } else {
        ["refs/heads/", "refs/remotes/", "refs/tags/"]
    };
    let found: Vec<String> = order
        .iter()
        .map(|ns| format!("{ns}{base}"))
        .filter(|r| git::ref_exists(repo, r))
        .collect();
    match found.as_slice() {
        [first, _, ..] => first.clone(),
        _ => base.to_string(),
    }
}

// ───────────────────────────── briefs ─────────────────────────────

/// `<state>\briefs\<repo>\<name>.md`.
pub fn brief_path(state_dir: &Path, repo: &str, name: &str) -> PathBuf {
    state_dir
        .join("briefs")
        .join(repo)
        .join(format!("{name}.md"))
}

/// Text that may be typed into an agent or passed through a `.cmd` shim.
pub fn check_safe_line(line: &str) -> Result<(), String> {
    if line.trim().is_empty() {
        return Err("the first line is empty".into());
    }
    if line.len() > MAX_FIRST_LINE {
        return Err(format!(
            "the first line is longer than {MAX_FIRST_LINE} bytes"
        ));
    }
    if let Some(c) = line
        .chars()
        .find(|c| matches!(c, '"' | '%') || c.is_control())
    {
        return Err(format!(
            "the first line must not contain quotes, '%', line breaks or control characters (found {c:?})"
        ));
    }
    Ok(())
}

/// The agent's first line: `template` (default [`DEFAULT_FIRST_LINE`]) with
/// `{brief_path}` replaced.
pub fn first_line(template: Option<&str>, path: &Path) -> Result<String, RpcError> {
    let line = template
        .unwrap_or(DEFAULT_FIRST_LINE)
        .replace("{brief_path}", &path.display().to_string());
    check_safe_line(&line).map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
    Ok(line)
}

/// The brief file: Pod's header, then the brief as given.
pub fn brief_text(task: &str, branch: &str, worktree: &str, brief: &str) -> String {
    format!(
        "<!-- pod brief_version {BRIEF_VERSION} -->\n\
         # Task brief\n\n\
         - Task: `{task}`\n\
         - Branch: `{branch}`\n\
         - Worktree: `{worktree}`\n\n\
         Work only in this worktree. Pod wrote this file; the brief below is from \
         whoever started the task.\n\n\
         ---\n\n\
         {brief}\n"
    )
}

pub fn write_brief(path: &Path, text: &str) -> Result<(), RpcError> {
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, text)
    };
    write().map_err(|e| {
        RpcError::new(
            codes::INTERNAL_ERROR,
            format!("cannot write the brief {}: {e}", path.display()),
        )
    })
}

/// How the first line reaches the agent.
pub fn brief_delivery(agent: &AgentSpec, mode: SetupMode, waits_for_setup: bool) -> BriefDelivery {
    match agent {
        // Typing a sentence into PowerShell would run it as a command.
        AgentSpec::Kind(AgentKind::Shell) => BriefDelivery::Env,
        AgentSpec::Kind(_) if mode == SetupMode::Parallel || !waits_for_setup => {
            BriefDelivery::Argv
        }
        _ => BriefDelivery::Typed,
    }
}

pub fn default_open_hint(from_terminal: bool) -> OpenHint {
    if from_terminal {
        OpenHint {
            focus: false,
            reason: OpenReason::Worker,
        }
    } else {
        OpenHint {
            focus: true,
            reason: OpenReason::User,
        }
    }
}

// ───────────────────────────── agent sessions ─────────────────────────────

/// Session ids are passed as arguments (maybe through a `.cmd` shim): allow
/// only plain id characters.
pub fn valid_session_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

/// The agent's own resume command (`claude --resume <id>`,
/// `codex resume <id>`); Codex also gets the task's per-run trust
/// (`agent_launch.rs`). Session discovery (docs/18) uses the same commands.
pub fn resume_argv(agent: &str, session: &str, worktree: &Path) -> Result<Vec<String>, RpcError> {
    if !valid_session_id(session) {
        return Err(RpcError::new(
            codes::INVALID_PARAMS,
            format!("invalid session id {session:?}"),
        ));
    }
    let (kind, argv) = match agent {
        "claude" => (
            AgentKind::Claude,
            vec!["claude".into(), "--resume".into(), session.to_string()],
        ),
        "codex" => (
            AgentKind::Codex,
            vec!["codex".into(), "resume".into(), session.to_string()],
        ),
        other => {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("cannot resume a {other:?} session (claude or codex)"),
            ));
        }
    };
    Ok(super::agent_launch::task_agent_argv(
        &AgentSpec::Kind(kind),
        argv,
        worktree,
    ))
}

/// Hook session ids of terminals no task owns yet. An agent's SessionStart
/// hook can arrive before `task.create` (or `task.resume`) has recorded its
/// terminal as the task's agent terminal; the id waits here and is attached
/// when the terminal is registered. Bounded and short-lived: hooks of plain
/// terminals also land here and simply expire.
#[derive(Debug, Default)]
pub struct EarlySessions {
    map: std::collections::HashMap<String, AgentSession>,
}

/// At most this many terminals wait (the oldest is dropped).
pub const EARLY_SESSIONS_MAX: usize = 32;
/// A waiting id is dropped after this long.
pub const EARLY_SESSIONS_TTL_MS: u64 = 60_000;

impl EarlySessions {
    fn prune(&mut self, now: u64) {
        self.map
            .retain(|_, s| now.saturating_sub(s.captured_ms) < EARLY_SESSIONS_TTL_MS);
    }

    /// Remember the latest session id of `terminal`.
    pub fn put(&mut self, terminal: &str, agent: &str, id: &str, now: u64) {
        self.prune(now);
        if !self.map.contains_key(terminal) && self.map.len() >= EARLY_SESSIONS_MAX {
            let oldest = self
                .map
                .iter()
                .min_by_key(|(_, s)| s.captured_ms)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                self.map.remove(&k);
            }
        }
        self.map.insert(
            terminal.to_string(),
            AgentSession {
                agent: agent.to_string(),
                id: id.to_string(),
                captured_ms: now,
                source: "hook".into(),
            },
        );
    }

    /// The waiting session of `terminal`, if still fresh (removed).
    pub fn take(&mut self, terminal: &str, now: u64) -> Option<AgentSession> {
        self.prune(now);
        self.map.remove(terminal)
    }

    pub fn contains(&self, terminal: &str) -> bool {
        self.map.contains_key(terminal)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// A hook message was accepted for `terminal`: remember the agent's session
/// id on the task whose agent terminal it is, or keep it in
/// [`EarlySessions`] until a task registers that terminal.
pub(super) fn note_hook_session(d: &Daemon, terminal: &str, agent: &str, payload: &Value) {
    let Some(id) = payload.get("session_id").and_then(Value::as_str) else {
        return;
    };
    if !valid_session_id(id) {
        return;
    }
    let task = {
        // Lock order registry -> early_sessions (task.create takes the
        // waiting id under the registry lock, so none slips between).
        let reg = d.registry.lock().unwrap();
        let task = reg
            .tasks
            .iter()
            .find(|t| t.agent_terminal_id() == Some(terminal))
            .map(|t| t.id.clone());
        if task.is_none() {
            d.early_sessions
                .lock()
                .unwrap()
                .put(terminal, agent, id, crate::now_ms());
        }
        task
    };
    let Some(task) = task else {
        d.early_wake.notify_waiters();
        return;
    };
    d.update_task(&task, |t| {
        if t.agent_session
            .as_ref()
            .is_some_and(|s| s.id == id && s.agent == agent)
        {
            return false;
        }
        t.agent_session = Some(AgentSession {
            agent: agent.to_string(),
            id: id.to_string(),
            captured_ms: crate::now_ms(),
            source: "hook".into(),
        });
        log(&format!("task {}: agent session {agent} {id}", t.id));
        true
    });
}

// ───────────────────────────── setup and tabs ─────────────────────────────

fn tab_name(task: &str, title: &str) -> String {
    let clean: String = title.chars().filter(|c| !c.is_control()).take(40).collect();
    format!("{task}:{}", clean.trim())
}

/// Debug-only test hook: with this variable set to `1`, `task.create` waits
/// (up to 20 s) until a hook session id of its new agent terminal is waiting
/// in [`EarlySessions`] before it registers the task. That makes the "hook
/// before registration" order deterministic in tests.
#[cfg(debug_assertions)]
pub const TEST_HOLD_REGISTER_ENV: &str = "POD_TEST_HOLD_TASK_REGISTER";

impl Daemon {
    #[cfg(debug_assertions)]
    pub(super) async fn test_hold_until_early_hook(&self, terminal: &str) {
        if std::env::var(TEST_HOLD_REGISTER_ENV).ok().as_deref() != Some("1") {
            return;
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            let notified = self.early_wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.early_sessions.lock().unwrap().contains(terminal) {
                log(&format!(
                    "test hold: hook of {terminal} waits, registering now"
                ));
                return;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                log(&format!("test hold: no hook of {terminal} within 20 s"));
                return;
            }
        }
    }

    /// Change a task record; `f` returns whether it changed anything. Saves
    /// and publishes `task.changed updated` when it did.
    pub(super) fn update_task(&self, id: &str, f: impl FnOnce(&mut TaskRecord) -> bool) -> bool {
        let changed = {
            let mut reg = self.registry.lock().unwrap();
            let Some(t) = reg.tasks.iter_mut().find(|t| t.id == id) else {
                return false;
            };
            if !f(t) {
                return false;
            }
            if let Err(e) = self.save_registry(&mut reg) {
                log(&e.message);
            }
            true
        };
        if changed {
            publish_task(task_event(id, TaskChange::Updated));
        }
        changed
    }

    /// Environment of a task's terminals.
    pub(super) fn task_env(rec: &TaskRecord) -> BTreeMap<String, String> {
        let mut env = rec.env.clone();
        env.insert(ENV_TASK_ID.to_string(), rec.id.clone());
        if let Some(b) = &rec.brief {
            env.insert(ENV_BRIEF.to_string(), b.path.clone());
        }
        env
    }

    async fn run_task_terminal(
        &self,
        rec: &TaskRecord,
        name: String,
        argv: Vec<String>,
    ) -> Result<TerminalInfo, RpcError> {
        self.run(RunParams {
            argv,
            cwd: Some(rec.worktree.clone()),
            name: Some(name),
            env: Self::task_env(rec),
            ..Default::default()
        })
        .await
    }

    /// Start setup (and the default tabs) for a new task. Never fails: a
    /// problem is recorded in `rec.setup`. Called before the task is saved;
    /// terminals are added to `rec.terminals`. Returns the setup terminal to
    /// watch.
    pub(super) async fn start_setup(
        &self,
        rec: &mut TaskRecord,
        eff: &Effective,
        policy: SetupPolicy,
    ) -> Option<String> {
        let skip = policy == SetupPolicy::Skip;
        let mut watch = None;
        if let Some(cmd) = &eff.setup {
            let source = Some(if cmd.personal { "personal" } else { "repo" }.to_string());
            rec.setup = Some(if skip {
                SetupInfo {
                    state: SetupState::Skipped,
                    exit_code: None,
                    terminal: None,
                    source,
                    hash: None,
                    error: None,
                    finished_ms: 0,
                }
            } else if eff.uses_untrusted(cmd) {
                log(&format!(
                    "task {}: setup not approved ({:?})",
                    rec.id, eff.hash
                ));
                SetupInfo {
                    state: SetupState::Untrusted,
                    exit_code: None,
                    terminal: None,
                    source,
                    hash: eff.hash.clone(),
                    error: None,
                    finished_ms: 0,
                }
            } else {
                let name = format!("{}:setup", rec.id);
                match self
                    .run_task_terminal(rec, name, repo_config::script_argv(&cmd.command))
                    .await
                {
                    Ok(t) => {
                        rec.terminals.push(t.id.clone());
                        watch = Some(t.id.clone());
                        SetupInfo {
                            state: SetupState::Running,
                            exit_code: None,
                            terminal: Some(t.id),
                            source,
                            hash: None,
                            error: None,
                            finished_ms: 0,
                        }
                    }
                    Err(e) => {
                        log(&format!(
                            "task {}: setup did not start: {}",
                            rec.id, e.message
                        ));
                        SetupInfo {
                            state: SetupState::Failed,
                            exit_code: None,
                            terminal: None,
                            source,
                            hash: None,
                            error: Some(e.message),
                            finished_ms: crate::now_ms(),
                        }
                    }
                }
            });
        }
        if !skip {
            let tabs: Vec<TabSection> = eff
                .tabs
                .iter()
                .filter(|(title, c)| {
                    let ok = !eff.uses_untrusted(c);
                    if !ok {
                        log(&format!("task {}: tab {title:?} not approved", rec.id));
                    }
                    ok
                })
                .map(|(title, c)| TabSection {
                    title: title.clone(),
                    command: c.command.clone(),
                })
                .collect();
            if watch.is_some() && eff.mode == SetupMode::Wait {
                rec.pending_tabs = tabs;
            } else {
                self.start_tabs(rec, &tabs).await;
            }
        }
        watch
    }

    async fn start_tabs(&self, rec: &mut TaskRecord, tabs: &[TabSection]) {
        for t in tabs {
            match self
                .run_task_terminal(
                    rec,
                    tab_name(&rec.id, &t.title),
                    repo_config::tab_argv(&t.command),
                )
                .await
            {
                Ok(info) => rec.terminals.push(info.id),
                Err(e) => log(&format!(
                    "task {}: tab {:?} did not start: {}",
                    rec.id, t.title, e.message
                )),
            }
        }
    }

    /// The setup terminal exited (or vanished): record the outcome, start the
    /// tabs held back for it, then deliver a brief that waited.
    async fn watch_setup(self: Arc<Self>, task: String, terminal: String) {
        let res = self
            .wait(WaitParams {
                target: TerminalRef::id(terminal.clone()),
                until: WaitUntil::Exit,
                timeout_ms: None,
            })
            .await;
        let info = self
            .resolve(&TerminalRef::id(terminal.clone()))
            .ok()
            .map(|t| t.info());
        let (state, code, error) = match (&res, &info) {
            (Ok(_), Some(i)) if i.exit_code == Some(0) && !i.killed && !i.lost => {
                (SetupState::Ok, Some(0), None)
            }
            (Ok(_), Some(i)) => (
                SetupState::Failed,
                i.exit_code.map(i64::from),
                (i.killed || i.lost).then(|| "setup terminal was killed".to_string()),
            ),
            (Ok(_), None) | (Err(_), _) => (
                SetupState::Failed,
                None,
                Some("setup terminal is gone".to_string()),
            ),
        };
        log(&format!("task {task}: setup {state:?} (exit {code:?})"));
        let mut pending = Vec::new();
        self.update_task(&task, |t| {
            pending = std::mem::take(&mut t.pending_tabs);
            if let Some(s) = t.setup.as_mut() {
                s.state = state;
                s.exit_code = code;
                s.error = error;
                s.finished_ms = crate::now_ms();
            }
            true
        });
        if !pending.is_empty() {
            let _op = self.task_ops.lock().await;
            if let Ok(mut rec) = self.task_record(&task) {
                let before = rec.terminals.len();
                self.start_tabs(&mut rec, &pending).await;
                let added: Vec<String> = rec.terminals[before..].to_vec();
                self.update_task(&task, |t| {
                    t.terminals.extend(added);
                    true
                });
                self.task_terminals_changed(&task);
            }
        }
        if self
            .task_record(&task)
            .ok()
            .and_then(|t| t.brief)
            .is_some_and(|b| {
                b.state == BriefState::Pending
                    && b.delivery == BriefDelivery::Typed
                    && b.after_setup
            })
        {
            self.clone().deliver_brief(task).await;
        }
    }

    /// Type the first line into the agent terminal once it is ready.
    async fn deliver_brief(self: Arc<Self>, task: String) {
        let Ok(rec) = self.task_record(&task) else {
            return;
        };
        let Some(brief) = rec.brief.clone() else {
            return;
        };
        if brief.after_setup
            && rec
                .setup
                .as_ref()
                .is_some_and(|s| s.state == SetupState::Running)
        {
            return; // the setup watcher calls again
        }
        let Some(term_id) = rec.agent_terminal_id().map(str::to_string) else {
            self.brief_done(&task, BriefState::Failed, Some("no agent terminal".into()));
            return;
        };
        let reports_status = matches!(
            rec.agent,
            AgentSpec::Kind(AgentKind::Claude | AgentKind::Codex)
        );
        let started = Instant::now();
        let ready = loop {
            let Ok(t) = self.resolve(&TerminalRef::id(term_id.clone())) else {
                break Err("agent terminal is gone".to_string());
            };
            if !t.is_running() {
                break Err("agent exited before the brief was typed".to_string());
            }
            let info = t.info();
            let state = info
                .status
                .as_ref()
                .map(|s| s.state.as_str())
                .unwrap_or("unknown");
            if matches!(state, "idle" | "done") {
                break Ok(());
            }
            // Output written before the daemon attached is only in the
            // holder's snapshot (`last_output_ms` stays 0): count quiet time
            // from the terminal's start then, or a fast program waits for
            // QUIET_READY_TIMEOUT instead.
            let out = brief_quiet_since(t.last_output_ms(), info.created_ms);
            let quiet =
                crate::now_ms().saturating_sub(out) >= QUIET_BEFORE_TYPING.as_millis() as u64;
            if !reports_status && quiet {
                break Ok(());
            }
            if reports_status && started.elapsed() > AGENT_READY_TIMEOUT {
                break Err(format!(
                    "agent did not become idle (state {state}); brief not typed"
                ));
            }
            if !reports_status && started.elapsed() > QUIET_READY_TIMEOUT {
                break Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        if let Err(e) = ready {
            log(&format!("task {task}: {e}"));
            self.brief_done(&task, BriefState::Failed, Some(e));
            return;
        }
        let r = self
            .send(SendParams {
                target: TerminalRef::id(term_id),
                text: brief.first_line.clone(),
                enter: true,
                confirm: true,
            })
            .await;
        match r {
            Ok(_) => self.brief_done(&task, BriefState::Sent, None),
            Err(e) => self.brief_done(&task, BriefState::Failed, Some(e.message)),
        }
    }

    fn brief_done(&self, task: &str, state: BriefState, error: Option<String>) {
        log(&format!("task {task}: brief {state:?}"));
        self.update_task(task, |t| {
            let Some(b) = t.brief.as_mut() else {
                return false;
            };
            b.state = state;
            b.error = error;
            true
        });
    }
}

pub(super) fn spawn_setup_watch(task: String, terminal: String) {
    if let Some(d) = daemon() {
        tokio::spawn(d.watch_setup(task, terminal));
    }
}

/// When the agent's output went quiet, for typed briefs: its last output
/// seen since attaching, else its start.
fn brief_quiet_since(last_output_ms: u64, created_ms: u64) -> u64 {
    if last_output_ms > 0 {
        last_output_ms
    } else {
        created_ms
    }
}

pub(super) fn spawn_brief_delivery(task: String) {
    if let Some(d) = daemon() {
        tokio::spawn(d.deliver_brief(task));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suggestions_skip_taken_and_retired_names() {
        let taken: BTreeSet<&str> = ["amber", "aspen"].into();
        assert_eq!(suggest_name(0, |n| taken.contains(n)), "birch");
        // Every word taken once: numbered names, still never a taken one.
        let all: BTreeSet<String> = WORDS.iter().map(|w| w.to_string()).collect();
        let n = suggest_name(3, |n| all.contains(n));
        assert_eq!(n, format!("{}-2", WORDS[3]));
        for w in WORDS {
            assert!(super::super::tasks::validate_name("task", w).is_ok(), "{w}");
        }

        // Retired names persist and are excluded.
        let dir = std::env::temp_dir().join(format!(
            "pod-retired-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        Retired::retire(&dir, "app", "birch");
        Retired::retire(&dir, "app", "birch");
        let r = Retired::load(&dir);
        assert!(r.contains("app", "birch") && !r.contains("lib", "birch"));
        assert_eq!(
            suggest_name(0, |n| taken.contains(n) || r.contains("app", n)),
            "brook"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_line_has_no_cmd_or_line_breaking_characters() {
        let p = Path::new(r"C:\Users\me\AppData\Local\pod\state\briefs\app\oak.md");
        let line = first_line(None, p).unwrap();
        assert_eq!(line, DEFAULT_FIRST_LINE);
        assert!(!line.contains(['"', '%', '\r', '\n']));
        let with_path = first_line(Some("Read {brief_path} and do it."), p).unwrap();
        assert!(with_path.contains(r"briefs\app\oak.md"), "{with_path}");
        for bad in ["say \"hi\"", "100% done", "a\rb", "a\nb", "tab\there", ""] {
            assert!(first_line(Some(bad), p).is_err(), "{bad:?}");
        }
        // A path that itself contains '%' is refused rather than mangled.
        assert!(first_line(Some("{brief_path}"), Path::new(r"C:\x%USERNAME%\b.md")).is_err());
        assert!(first_line(Some(&"x".repeat(500)), p).is_err());
    }

    #[test]
    fn brief_file_keeps_the_text_verbatim() {
        let brief = "line 1 \"quoted\" 100%\r\nline 2 & | < > ^\n";
        let t = brief_text("app/oak", "pod/oak", r"C:\w\app\oak", brief);
        assert!(t.contains(brief), "{t}");
        assert!(t.starts_with(&format!("<!-- pod brief_version {BRIEF_VERSION} -->")));
    }

    #[test]
    fn brief_quiet_counts_from_start_without_live_output() {
        assert_eq!(
            brief_quiet_since(0, 1_000),
            1_000,
            "all output was in the snapshot"
        );
        assert_eq!(brief_quiet_since(5_000, 1_000), 5_000);
    }

    #[test]
    fn delivery_and_open_hints() {
        let claude = AgentSpec::Kind(AgentKind::Claude);
        assert_eq!(
            brief_delivery(&claude, SetupMode::Parallel, true),
            BriefDelivery::Argv
        );
        assert_eq!(
            brief_delivery(&claude, SetupMode::Wait, true),
            BriefDelivery::Typed
        );
        assert_eq!(
            brief_delivery(&claude, SetupMode::Wait, false),
            BriefDelivery::Argv
        );
        assert_eq!(
            brief_delivery(&AgentSpec::default(), SetupMode::Parallel, false),
            BriefDelivery::Env
        );
        let argv = AgentSpec::Argv {
            argv: vec!["x".into()],
        };
        assert_eq!(
            brief_delivery(&argv, SetupMode::Parallel, false),
            BriefDelivery::Typed
        );
        assert!(default_open_hint(false).focus);
        assert_eq!(
            default_open_hint(true),
            OpenHint {
                focus: false,
                reason: OpenReason::Worker
            }
        );
    }

    #[test]
    fn resume_commands() {
        let wt = Path::new(r"C:\w\app\oak");
        assert_eq!(
            resume_argv("claude", "abc-123", wt).unwrap(),
            ["claude", "--resume", "abc-123"]
        );
        let codex = resume_argv("codex", "0199a-b", wt).unwrap();
        assert_eq!(codex[0], "codex");
        assert_eq!(&codex[codex.len() - 2..], ["resume", "0199a-b"]);
        assert!(codex.iter().any(|a| a.contains("trust_level")), "{codex:?}");
        for bad in ["", "-x", "a b", "a\"b", "a%b", "a&b"] {
            assert!(resume_argv("claude", bad, wt).is_err(), "{bad:?}");
        }
        assert!(resume_argv("shell", "abc", wt).is_err());
    }

    #[test]
    fn early_sessions_are_bounded_and_expire() {
        let mut e = EarlySessions::default();
        e.put("t1", "claude", "s1", 1_000);
        e.put("t1", "claude", "s2", 1_100);
        assert_eq!(e.len(), 1, "one per terminal, latest wins");
        let s = e.take("t1", 1_200).unwrap();
        assert_eq!(
            (s.agent.as_str(), s.id.as_str(), s.source.as_str()),
            ("claude", "s2", "hook")
        );
        assert!(e.take("t1", 1_200).is_none(), "taken once");

        e.put("old", "codex", "x", 0);
        assert!(e.take("old", EARLY_SESSIONS_TTL_MS).is_none(), "expired");

        for i in 0..EARLY_SESSIONS_MAX + 5 {
            e.put(&format!("t{i}"), "claude", "s", 10 + i as u64);
        }
        assert_eq!(e.len(), EARLY_SESSIONS_MAX);
        assert!(
            !e.contains("t0") && !e.contains("t4"),
            "the oldest went first"
        );
        assert!(e.contains("t5") && e.contains(&format!("t{}", EARLY_SESSIONS_MAX + 4)));
    }
}
