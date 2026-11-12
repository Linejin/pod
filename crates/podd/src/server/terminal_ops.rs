//! Terminal operations: `terminal.run` (start a holder and connect to it),
//! input (`send`, `keys`, `input`), `read`/`wait`, resize, kill and rm, and
//! finding a terminal by id or name.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use super::runtime::stopping_err;
use super::terminal::{self, Terminal};
use super::{Daemon, log, status, submit};
use crate::endpoint::{self, ENV_SOCKET, ENV_TERMINAL_ID};
use crate::holder_proto::{Frame, HolderSpec};
use crate::protocol::*;

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 30;
/// How long `terminal.run` waits for its new holder to serve its pipe.
const HOLDER_START_TIMEOUT: Duration = Duration::from_secs(15);

/// This daemon's folder when the `pod` CLI sits next to it (installed,
/// portable and dev builds ship them together), else `None`.
fn pod_cli_dir() -> Option<String> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    dir.join("pod.exe")
        .is_file()
        .then(|| dir.display().to_string())
}

/// Input that did not reach the holder (daemon-01).
fn input_err(t: &Terminal, e: terminal::InputRefused) -> RpcError {
    match e {
        terminal::InputRefused::Backlog { in_flight } => RpcError::new(
            codes::INPUT_BACKLOG,
            format!(
                "terminal {} has not read {in_flight} bytes of earlier input yet; nothing was sent, retry later",
                t.id
            ),
        ),
        terminal::InputRefused::Gone => exited_err(t),
    }
}

pub(in crate::server) fn exited_err(t: &Terminal) -> RpcError {
    RpcError::new(
        codes::TERMINAL_EXITED,
        format!("terminal {} has exited", t.id),
    )
}

impl Daemon {
    pub(in crate::server) fn list(&self) -> Vec<TerminalInfo> {
        self.terminals
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.info())
            .collect()
    }

    /// By id: exact. By name: the running terminal with that name, else the
    /// most recently created exited one.
    pub(in crate::server) fn resolve(&self, r: &TerminalRef) -> Result<Arc<Terminal>, RpcError> {
        let terms = self.terminals.lock().unwrap();
        let found = match (&r.id, &r.name) {
            (Some(id), _) => terms.iter().find(|t| &t.id == id).cloned(),
            (None, Some(name)) => terms
                .iter()
                // max_by_key keeps the last maximum: the newest running one, else the newest.
                .filter(|t| t.name.as_deref() == Some(name))
                .max_by_key(|t| t.is_running())
                .cloned(),
            (None, None) => {
                return Err(RpcError::new(
                    codes::INVALID_PARAMS,
                    "either id or name is required",
                ));
            }
        };
        drop(terms);
        found.ok_or_else(|| {
            let msg = match self.unreachable_reason(r) {
                Some((id, why)) => format!(
                    "the holder of terminal {id} is unreachable ({why}); podd keeps retrying, \
                     `pod kill {id}` forgets it"
                ),
                None => format!("no terminal {r}"),
            };
            RpcError::new(codes::TERMINAL_NOT_FOUND, msg)
        })
    }

    pub(in crate::server) fn running(&self, r: &TerminalRef) -> Result<Arc<Terminal>, RpcError> {
        let t = self.resolve(r)?;
        if !t.is_running() {
            return Err(exited_err(&t));
        }
        Ok(t)
    }

    /// Clean base environment + our variables + per-run overrides (see `env.rs`).
    pub(in crate::server) fn terminal_env(
        &self,
        id: &str,
        overrides: BTreeMap<String, String>,
    ) -> BTreeMap<String, String> {
        let base = crate::winsec::user_environment().unwrap_or_else(|e| {
            log(&format!(
                "user environment unavailable ({e}); using the daemon's"
            ));
            std::env::vars().collect()
        });
        let mut env = crate::env::sanitize(base);
        crate::env::set_var(&mut env, "TERM", "xterm-256color");
        crate::env::set_var(&mut env, "COLORTERM", "truecolor");
        crate::env::set_var(&mut env, ENV_TERMINAL_ID, id);
        crate::env::set_var(&mut env, ENV_SOCKET, self.config.pipe.as_str());
        if let Some(dir) = pod_cli_dir() {
            crate::env::prepend_path(&mut env, &dir);
        }
        for (k, v) in overrides {
            crate::env::set_var(&mut env, &k, v);
        }
        // docs/19 §20: a worker talks to the restricted worker endpoint.
        super::agent_launch::worker_endpoint_env(&mut env, &self.config.pipe);
        env
    }

    pub(in crate::server) async fn run(&self, p: RunParams) -> Result<TerminalInfo, RpcError> {
        if p.argv.is_empty() || p.argv[0].is_empty() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "argv must not be empty",
            ));
        }
        if let Some(name) = &p.name
            && (name.is_empty() || is_terminal_id(name))
        {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("invalid name {name:?} (must not look like an id)"),
            ));
        }
        let cwd = match &p.cwd {
            Some(c) => PathBuf::from(c),
            None => std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(".")),
        };
        if !cwd.is_dir() {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("cwd {} is not a directory", cwd.display()),
            ));
        }
        let cols = p.cols.unwrap_or(DEFAULT_COLS).clamp(2, 1000);
        let rows = p.rows.unwrap_or(DEFAULT_ROWS).clamp(2, 500);

        // Reserve the name while the holder starts.
        let _reservation = match &p.name {
            Some(name) => {
                let terms = self.terminals.lock().unwrap();
                let mut pending = self.pending_names.lock().unwrap();
                if let Some(t) = terms
                    .iter()
                    .find(|t| t.name.as_ref() == Some(name) && t.is_running())
                {
                    return Err(RpcError::new(
                        codes::NAME_TAKEN,
                        format!("name {name:?} is used by running terminal {}", t.id),
                    ));
                }
                if !pending.insert(name.clone()) {
                    return Err(RpcError::new(
                        codes::NAME_TAKEN,
                        format!("name {name:?} is being started"),
                    ));
                }
                Some(NameReservation {
                    daemon: self,
                    name: name.clone(),
                })
            }
            None => None,
        };

        // From here until the terminal is listed, `server.stop kill:true`
        // waits for this start (daemon-02).
        let _start = self.begin_start()?;
        let (parent_terminal, parent_task) = self.parent_of(p.parent_terminal.as_deref());
        let id = format!("t{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let spawn_err = |m: String| RpcError::new(codes::SPAWN_FAILED, m);
        let dir = &self.config.state_dir;
        std::fs::create_dir_all(dir).map_err(|e| spawn_err(format!("state dir: {e}")))?;
        let record = dir.join(format!("{id}.json"));
        let spec_path = dir.join(format!("{id}.spec.json"));
        let err_path = dir.join(format!("{id}.err"));
        let _ = std::fs::remove_file(&err_path);
        let pipe = endpoint::holder_pipe(&self.config.pipe, &id);
        let spec = HolderSpec {
            id: id.clone(),
            name: p.name.clone(),
            argv: p.argv.clone(),
            cwd: cwd.display().to_string(),
            env: status::hook_env(self.terminal_env(&id, p.env), dir, &id)?,
            cols,
            rows,
            pipe: pipe.clone(),
            record_path: record.display().to_string(),
            created_ms: crate::now_ms(),
            parent_terminal,
            parent_task,
        };
        std::fs::write(&spec_path, serde_json::to_vec(&spec).unwrap())
            .map_err(|e| spawn_err(format!("spec: {e}")))?;

        let mut cmd = Command::new(&self.config.holder_exe);
        cmd.arg("--spec").arg(&spec_path).current_dir(dir);
        let child = crate::client::spawn_detached(&mut cmd).map_err(|e| {
            let _ = std::fs::remove_file(&spec_path);
            spawn_err(format!(
                "cannot start holder {}: {e}",
                self.config.holder_exe.display()
            ))
        })?;

        let deadline = Instant::now() + HOLDER_START_TIMEOUT;
        let term = loop {
            if self.is_stopping_with_kill() {
                // Not connected yet: end the holder process; its Job Object
                // (kill on close) takes the child with it.
                let _ = child.kill();
                let _ = std::fs::remove_file(&spec_path);
                let _ = std::fs::remove_file(&record);
                log(&format!("run {id}: stopped while starting; holder killed"));
                return Err(stopping_err());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match Terminal::connect_within(&pipe, left.max(Duration::from_millis(100))).await {
                Ok(t) => break t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound && Instant::now() < deadline => {
                    if let Ok(Some(status)) = child.try_wait() {
                        let msg = std::fs::read_to_string(&err_path)
                            .unwrap_or_else(|_| format!("holder exited with {status}"));
                        let _ = std::fs::remove_file(&err_path);
                        let _ = std::fs::remove_file(&spec_path);
                        return Err(spawn_err(format!("failed to start terminal: {msg}")));
                    }
                    tokio::time::sleep(Duration::from_millis(15)).await;
                }
                Err(e) => {
                    let _ = child.kill();
                    return Err(spawn_err(format!("cannot connect to holder: {e}")));
                }
            }
        };
        let info = term.info();
        log(&format!(
            "run {} {:?} in {} (holder pid {})",
            info.id, info.argv, info.cwd, term.holder_pid
        ));
        self.terminals.lock().unwrap().push(term.clone());
        // Listed after `server.stop kill:true` took its kill set (it waited
        // at most START_GRACE): end it here, as the stop would have.
        if self.is_stopping_with_kill() {
            self.kill_and_wait(&term).await;
            if term.is_running() {
                let _ = child.kill();
            }
            term.send_to_holder(Frame::Release);
            log(&format!("run {}: stopped while starting; killed", info.id));
            return Err(stopping_err());
        }
        status::publish_terminal(&info.id, TerminalChange::Created, Some(info.clone()));
        Ok(info)
    }

    /// The caller's terminal as a parent: (terminal, task it belongs to) when
    /// this daemon knows the terminal, else nothing.
    pub(crate) fn parent_of(&self, caller: Option<&str>) -> (Option<String>, Option<String>) {
        let Some(tid) = caller.filter(|t| !t.is_empty()) else {
            return (None, None);
        };
        if !self.terminals.lock().unwrap().iter().any(|t| t.id == tid) {
            log(&format!("unknown parent terminal {tid} ignored"));
            return (None, None);
        }
        let task = self
            .registry
            .lock()
            .unwrap()
            .tasks
            .iter()
            .find(|t| t.terminals.iter().any(|x| x == tid))
            .map(|t| t.id.clone());
        (Some(tid.to_string()), task)
    }

    pub(in crate::server) async fn send(&self, p: SendParams) -> Result<SendResult, RpcError> {
        let t = self.running(&p.target)?;
        let mut bytes = p.text.len();
        let chars = p.text.chars().count();
        t.remember_sent(&p.text);
        let written = crate::now_ms();
        if !p.text.is_empty() {
            t.try_input(p.text.into_bytes())
                .map_err(|e| input_err(&t, e))?;
        }
        let mut submit = None;
        if p.enter {
            // A separate write once the program has taken the text in: TUIs
            // that detect pastes by timing (Codex on Windows) turn an early
            // Enter into a newline (docs/12 B1, `submit.rs`).
            let since = if chars > 0 { written } else { 0 };
            submit = submit::press_enter(&t, since, chars, p.confirm, |_| {}).await;
            bytes += submit.as_ref().map_or(1, |s| s.enters as usize);
            if let Some(s) = &submit
                && s.state == SubmitState::Unconfirmed
            {
                log(&format!(
                    "{}: sent text was not confirmed as submitted (Enter pressed {} times)",
                    t.id, s.enters
                ));
            }
        }
        Ok(SendResult {
            id: t.id.clone(),
            bytes,
            submit,
            dropped_replies: None,
        })
    }

    pub(in crate::server) fn keys(&self, p: KeysParams) -> Result<SendResult, RpcError> {
        let t = self.running(&p.target)?;
        let app_cursor = t.app_cursor();
        let mut bytes = Vec::new();
        for k in &p.keys {
            let b = crate::keys::key_bytes(k, app_cursor).ok_or_else(|| {
                RpcError::new(codes::INVALID_PARAMS, format!("unknown key {k:?}"))
            })?;
            bytes.extend(b);
        }
        let n = bytes.len();
        t.try_input(bytes).map_err(|e| input_err(&t, e))?;
        Ok(SendResult {
            id: t.id.clone(),
            bytes: n,
            submit: None,
            dropped_replies: None,
        })
    }

    pub(in crate::server) fn input(&self, p: InputParams) -> Result<SendResult, RpcError> {
        let t = self.running(&p.target)?;
        let bytes = b64_decode(&p.data)
            .map_err(|e| RpcError::new(codes::INVALID_PARAMS, format!("data: {e}")))?;
        // Attach clients answer terminal queries: drop duplicates and late replies.
        let (n, dropped) = t.attach_input(&bytes).map_err(|e| input_err(&t, e))?;
        // A person typed: drafts block wake-ups; workers become kept (docs/19 짠5.2).
        if t.note_human_input(&bytes) {
            self.orch_on_human_input(&t.id);
        }
        if dropped.total() > 0 {
            log(&format!(
                "{}: dropped query replies from attach input ({} duplicate, {} late)",
                t.id, dropped.duplicate, dropped.late
            ));
        }
        Ok(SendResult {
            id: t.id.clone(),
            bytes: n,
            submit: None,
            dropped_replies: (dropped.total() > 0).then_some(crate::protocol::DroppedReplies {
                duplicate: dropped.duplicate,
                late: dropped.late,
            }),
        })
    }

    /// `terminal.input_once` (v20, docs/17 R4): `terminal.input` at most once
    /// per `(stream, seq)`; `check` reports and fences without writing.
    pub(in crate::server) fn input_once(
        &self,
        p: InputOnceParams,
    ) -> Result<InputOnceResult, RpcError> {
        if !valid_input_stream(&p.stream) {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                format!("stream: 1-{INPUT_STREAM_MAX} characters A-Za-z0-9_- expected"),
            ));
        }
        // A check may ask about a terminal that has exited since.
        let t = if p.check {
            self.resolve(&p.target)?
        } else {
            self.running(&p.target)?
        };
        let bytes = if p.check {
            Vec::new()
        } else {
            b64_decode(&p.data)
                .map_err(|e| RpcError::new(codes::INVALID_PARAMS, format!("data: {e}")))?
        };
        let outcome = t
            .input_once(&p.stream, p.seq, &bytes, p.check)
            .map_err(|e| input_err(&t, e))?;
        let (outcome, n, dropped) = match outcome {
            terminal::OnceOutcome::Written { bytes: n, dropped } => {
                if t.note_human_input(&bytes) {
                    self.orch_on_human_input(&t.id);
                }
                if dropped.total() > 0 {
                    log(&format!(
                        "{}: dropped query replies from attach input ({} duplicate, {} late)",
                        t.id, dropped.duplicate, dropped.late
                    ));
                }
                let dropped = (dropped.total() > 0).then_some(DroppedReplies {
                    duplicate: dropped.duplicate,
                    late: dropped.late,
                });
                (InputOnceOutcome::Written, n, dropped)
            }
            terminal::OnceOutcome::Already { bytes: n } => {
                log(&format!(
                    "{}: input {}#{} seen before; not written again",
                    t.id, p.stream, p.seq
                ));
                (InputOnceOutcome::Already, n, None)
            }
            terminal::OnceOutcome::Fenced => (InputOnceOutcome::Fenced, 0, None),
        };
        Ok(InputOnceResult {
            id: t.id.clone(),
            outcome,
            bytes: n,
            dropped_replies: dropped,
        })
    }

    pub(in crate::server) fn read(&self, p: ReadParams) -> Result<ReadResult, RpcError> {
        let t = self.resolve(&p.target)?;
        let (mut lines, cursor, suggestion) = if p.raw {
            let (l, c) = t.read_lines(p.mode);
            (l, c, None)
        } else {
            t.read_lines_for_agents(p.mode)
        };
        if let Some(n) = p.lines {
            let skip = lines.len().saturating_sub(n);
            lines.drain(..skip);
        }
        Ok(ReadResult {
            id: t.id.clone(),
            mode: p.mode,
            lines,
            cursor,
            state: t.info().state,
            suggestion,
        })
    }

    pub(in crate::server) async fn wait(&self, p: WaitParams) -> Result<WaitResult, RpcError> {
        let t = self.resolve(&p.target)?;
        let start = Instant::now();
        let deadline = p
            .timeout_ms
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        let outcome = loop {
            // Register for the next change *before* checking, so none is missed.
            let notified = t.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();

            let running = t.is_running();
            match &p.until {
                WaitUntil::Exit if !running => break WaitOutcome::Matched,
                WaitUntil::OutputContains(s) if t.output_contains(s) => break WaitOutcome::Matched,
                WaitUntil::OutputContains(_) if !running => break WaitOutcome::Exited,
                _ => {}
            }
            match deadline {
                Some(d) => {
                    if tokio::time::timeout_at(d, notified).await.is_err() {
                        break WaitOutcome::TimedOut;
                    }
                }
                None => notified.await,
            }
        };
        Ok(WaitResult {
            outcome,
            terminal: t.info(),
            waited_ms: start.elapsed().as_millis() as u64,
        })
    }

    /// Resize and wait until the holder confirmed it (the `Resized` frame).
    pub(in crate::server) async fn resize_to(
        &self,
        t: &Terminal,
        cols: u16,
        rows: u16,
    ) -> Result<(), RpcError> {
        if cols < 2 || rows < 2 || cols > 1000 || rows > 500 {
            return Err(RpcError::new(
                codes::INVALID_PARAMS,
                "size out of range (2..=1000 x 2..=500)",
            ));
        }
        if t.size() == (cols, rows) {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        loop {
            let notified = t.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if t.size() == (cols, rows) {
                return Ok(());
            }
            if !t.is_running() {
                return Err(exited_err(t));
            }
            // (Re)send inside the loop: cheap, and robust against a reconnect.
            t.send_to_holder(Frame::Resize { cols, rows });
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(RpcError::new(
                    codes::INTERNAL_ERROR,
                    "holder did not confirm the resize",
                ));
            }
        }
    }

    pub(in crate::server) async fn resize(
        &self,
        p: ResizeParams,
    ) -> Result<TerminalInfo, RpcError> {
        let t = self.running(&p.target)?;
        self.resize_to(&t, p.cols, p.rows).await?;
        Ok(t.info())
    }

    pub(in crate::server) async fn kill_and_wait(&self, t: &Terminal) {
        if !t.is_running() {
            return;
        }
        t.send_to_holder(Frame::Kill);
        let deadline = Instant::now() + Duration::from_secs(5);
        while t.is_running() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub(in crate::server) async fn kill(&self, r: TerminalRef) -> Result<TerminalInfo, RpcError> {
        let t = match self.resolve(&r) {
            Ok(t) => t,
            // An unreachable holder: give up on it (daemon-03).
            Err(e) => return self.give_up_unreachable(&r).ok_or(e),
        };
        self.kill_and_wait(&t).await;
        log(&format!("kill {}", t.id));
        Ok(t.info())
    }

    pub(in crate::server) async fn rm(&self, p: RmParams) -> Result<RmResult, RpcError> {
        let t = match self.resolve(&p.target) {
            Ok(t) => t,
            Err(e) => {
                let info = self.give_up_unreachable(&p.target).ok_or(e)?;
                return Ok(RmResult { id: info.id });
            }
        };
        if let Some(why) = &t.incompatible {
            // We cannot speak to this holder: only `force` ends it, by
            // terminating the holder process (its Job Object then ends the tree).
            if !p.force {
                return Err(RpcError::new(
                    codes::TERMINAL_RUNNING,
                    format!(
                        "terminal {} has an {why}; use force to terminate its holder (pid {})",
                        t.id, t.holder_pid
                    ),
                ));
            }
            if let Err(e) = crate::winsec::terminate_process(t.holder_pid) {
                log(&format!(
                    "{}: cannot terminate holder {}: {e}",
                    t.id, t.holder_pid
                ));
            }
            let record = self.config.state_dir.join(format!("{}.json", t.id));
            let _ = std::fs::remove_file(&record);
            let _ = std::fs::remove_file(record.with_extension("log"));
            self.terminals.lock().unwrap().retain(|x| x.id != t.id);
            status::on_removed(self, &t);
            self.forget_task_terminal(&t.id);
            self.forget_kept(&t.id);
            status::publish_terminal(&t.id, TerminalChange::Removed, None);
            log(&format!("rm {} (incompatible holder terminated)", t.id));
            return Ok(RmResult { id: t.id.clone() });
        }
        if t.is_running() {
            if !p.force {
                return Err(RpcError::new(
                    codes::TERMINAL_RUNNING,
                    format!("terminal {} is running (kill it first or use force)", t.id),
                ));
            }
            self.kill_and_wait(&t).await;
        }
        t.send_to_holder(Frame::Release);
        let record = self.config.state_dir.join(format!("{}.json", t.id));
        // The holder removes its record on release; a lost holder cannot.
        if t.info().lost {
            let _ = std::fs::remove_file(&record);
            let _ = std::fs::remove_file(record.with_extension("log"));
        }
        self.terminals.lock().unwrap().retain(|x| x.id != t.id);
        status::on_removed(self, &t);
        self.forget_task_terminal(&t.id);
        self.forget_kept(&t.id);
        status::publish_terminal(&t.id, TerminalChange::Removed, None);
        log(&format!("rm {}", t.id));
        Ok(RmResult { id: t.id.clone() })
    }
}

struct NameReservation<'a> {
    daemon: &'a Daemon,
    name: String,
}

impl Drop for NameReservation<'_> {
    fn drop(&mut self) {
        self.daemon.pending_names.lock().unwrap().remove(&self.name);
    }
}

pub(in crate::server) fn id_number(id: &str) -> u64 {
    id.trim_start_matches('t').parse().unwrap_or(0)
}
