//! The daemon's lifetime: bind the pipe (the single-daemon lock), serve
//! connections until `server.stop`, then shut down: stop accepting, kill or
//! leave the terminals, let the requests in flight finish.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::watch;

use super::connection::serve_connection;
use super::tasks::Registry;
use super::terminal::{self, Terminal};
use super::{
    Daemon, ServerConfig, discovery, gates, lifecycle, log, mobile, mods, orch_runs, orch_store,
    pr, query_dedup, remote, retention, review, sessions, status,
};
use crate::holder_proto::Frame;
use crate::protocol::*;
use crate::winsec::UserOnly;

const ERROR_ACCESS_DENIED: i32 = 5;

/// Create the first pipe instance. `first_pipe_instance` makes creation fail
/// when another process already owns this pipe name: that is the
/// single-daemon lock (atomic, released by the OS when we die). The DACL
/// grants access to the current user only.
pub fn bind(config: &ServerConfig) -> anyhow::Result<(NamedPipeServer, UserOnly)> {
    let sa = UserOnly::new()?;
    // SAFETY: `sa` outlives the call.
    let res = unsafe {
        ServerOptions::new()
            .first_pipe_instance(true)
            .reject_remote_clients(true)
            .create_with_security_attributes_raw(&config.pipe, sa.as_ptr())
    };
    match res {
        Ok(s) => Ok((s, sa)),
        Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => {
            anyhow::bail!("another podd already owns {} (lock held)", config.pipe)
        }
        Err(e) => Err(anyhow::anyhow!("cannot create pipe {}: {e}", config.pipe)),
    }
}

/// Serve until `server.stop`.
pub async fn serve(
    config: ServerConfig,
    first: NamedPipeServer,
    sa: UserOnly,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(&config.state_dir)?;
    terminal::set_attach_queue_limit(config.attach_queue_limit);
    query_dedup::set_reply_window_ms(config.reply_window_ms);
    let (shutdown, mut shutdown_rx) = watch::channel(None);
    let registry = Registry::load(&config.state_dir);
    let orch = orch_store::Store::load(&config.state_dir);
    let runs = orch_runs::OrchRuns::load(&config.state_dir);
    let machine_id = remote::load_or_create_machine_id(&config.state_dir);
    let mobile = mobile::Mobile::load(&config.state_dir, mobile::session::system_clock());
    let daemon = Arc::new(Daemon {
        config,
        sa,
        started_ms: crate::now_ms(),
        started: Instant::now(),
        next_id: AtomicU64::new(1),
        terminals: Mutex::new(Vec::new()),
        pending_names: Mutex::new(HashSet::new()),
        unreachable: Mutex::new(Vec::new()),
        shutdown,
        in_flight: AtomicUsize::new(0),
        starting: Mutex::new(0),
        registry: Mutex::new(registry),
        task_ops: tokio::sync::Mutex::new(()),
        orch: Mutex::new(orch),
        runs,
        review_wake: tokio::sync::Notify::new(),
        kept: Mutex::new(HashSet::new()),
        gate_wake: tokio::sync::Notify::new(),
        check_jobs: Mutex::new(Default::default()),
        forge: Arc::from(pod_forge::from_env()),
        pr_ops: tokio::sync::Mutex::new(()),
        early_sessions: Mutex::new(Default::default()),
        early_wake: tokio::sync::Notify::new(),
        machine_id,
        boot_id: remote::new_boot_id(),
        mobile,
    });
    log(&format!(
        "listening on {} (pid {}, state {})",
        daemon.config.pipe,
        std::process::id(),
        daemon.config.state_dir.display()
    ));
    status::set_state_dir(&daemon.config.state_dir);
    daemon.rediscover().await;
    lifecycle::start_holder_retry(&daemon);
    daemon.load_kept();
    daemon.interrupt_stale_check_jobs();
    status::start(&daemon);
    review::start(&daemon);
    gates::start(&daemon);
    retention::start(&daemon);
    lifecycle::start(&daemon);
    sessions::start(&daemon);
    pr::start(&daemon);
    mods::start(&daemon);
    orch_runs::start(&daemon);
    discovery::start(&daemon);
    {
        let reg = daemon.registry.lock().unwrap();
        // Never hand out an id a task still refers to.
        daemon
            .next_id
            .fetch_max(reg.max_terminal_number() + 1, Ordering::Relaxed);
        log(&format!(
            "{} repo(s), {} task(s); worktrees under {}",
            reg.repos.len(),
            reg.tasks.len(),
            daemon.config.worktrees_root.display()
        ));
    }

    let mut server = first;
    loop {
        tokio::select! {
            res = server.connect() => {
                res?;
                // Always keep one instance listening before handing this one off.
                // SAFETY: `daemon.sa` outlives the call.
                let next = unsafe {
                    ServerOptions::new()
                        .reject_remote_clients(true)
                        .create_with_security_attributes_raw(&daemon.config.pipe, daemon.sa.as_ptr())?
                };
                let conn = std::mem::replace(&mut server, next);
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_connection(daemon, conn).await {
                        log(&format!("connection error: {e}"));
                    }
                });
            }
            _ = shutdown_rx.changed() => break,
        }
    }

    // Stop taking connections, but keep the pipe name until this process
    // exits: a client connecting now waits (all instances busy) and then
    // finds no pipe, so it starts / reaches the *next* daemon instead of an
    // instance nobody serves. The name also keeps a new daemon from binding
    // while this one still talks to the holders.
    // Both handles are leaked on purpose: they must outlive `serve` (main
    // exits right after it returns, and the OS closes them then).
    if let Some(plug) = plug_listener(&daemon, server).await {
        std::mem::forget(plug);
    }

    let kill = shutdown_rx.borrow().unwrap_or(false);
    if kill {
        // Terminals still starting are either listed before this returns or
        // end themselves (`run` checks the stop after listing): one rule for
        // every terminal created before the stop.
        let deadline = Instant::now() + START_GRACE;
        while *daemon.starting.lock().unwrap() > 0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let terms: Vec<Arc<Terminal>> = daemon.terminals.lock().unwrap().clone();
        for t in &terms {
            t.send_to_holder(Frame::Kill);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while terms.iter().any(|t| t.is_running()) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        for t in &terms {
            t.send_to_holder(Frame::Release);
        }
        log(&format!(
            "stopping; killed and released {} terminal(s)",
            terms.len()
        ));
    } else {
        log("stopping; terminals stay alive in their holders");
    }
    // Requests accepted before the stop (including the stop's own reply)
    // finish; long waits are cut off and their clients reconnect.
    let deadline = Instant::now() + IN_FLIGHT_GRACE;
    while daemon.in_flight.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Let the last responses and frames go out.
    tokio::time::sleep(Duration::from_millis(200)).await;
    log("stopped");
    Ok(())
}

/// How long shutdown waits for requests that were already being answered.
const IN_FLIGHT_GRACE: Duration = Duration::from_secs(3);
/// How long `server.stop kill:true` waits for terminals still starting
/// (longer than a holder connect, see [`terminal::CONNECT_TIMEOUT`]).
const START_GRACE: Duration = Duration::from_secs(20);

/// Occupy the last listening instance with a client handle of our own so no
/// other client can connect to it. Returns the instance and the handle.
async fn plug_listener(
    daemon: &Arc<Daemon>,
    mut server: NamedPipeServer,
) -> Option<(NamedPipeServer, std::fs::File)> {
    for _ in 0..8 {
        match crate::transport::open_pipe(&daemon.config.pipe) {
            Ok(f) => return Some((server, f)),
            Err(_) => {
                // A client took the instance just before the stop: serve it
                // (every request gets SERVER_STOPPING), then plug a new one.
                if tokio::time::timeout(Duration::from_millis(500), server.connect())
                    .await
                    .is_err()
                {
                    return None;
                }
                // SAFETY: `daemon.sa` outlives the call.
                let next = unsafe {
                    ServerOptions::new()
                        .reject_remote_clients(true)
                        .create_with_security_attributes_raw(
                            &daemon.config.pipe,
                            daemon.sa.as_ptr(),
                        )
                };
                let Ok(next) = next else { return None };
                let conn = std::mem::replace(&mut server, next);
                let daemon = daemon.clone();
                tokio::spawn(async move {
                    let _ = serve_connection(daemon, conn).await;
                });
            }
        }
    }
    None
}

/// One `terminal.run` between its stop check and the end of its start.
pub(in crate::server) struct StartGuard<'a>(&'a Daemon);

impl Drop for StartGuard<'_> {
    fn drop(&mut self) {
        *self.0.starting.lock().unwrap() -= 1;
    }
}

/// Counts a request as in flight until dropped.
pub(in crate::server) struct InFlight<'a>(&'a Daemon);

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.in_flight.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Daemon {
    pub(in crate::server) fn begin_request(&self) -> InFlight<'_> {
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        InFlight(self)
    }

    /// `server.stop` was called.
    pub(in crate::server) fn is_stopping(&self) -> bool {
        self.shutdown.borrow().is_some()
    }

    /// `server.stop kill:true` was called.
    pub(in crate::server) fn is_stopping_with_kill(&self) -> bool {
        *self.shutdown.borrow() == Some(true)
    }

    /// Register a terminal start, or refuse it once the stop was called. The
    /// check and the count share a lock with the shutdown's wait, so a start
    /// is either waited for or sees the stop.
    pub(in crate::server) fn begin_start(&self) -> Result<StartGuard<'_>, RpcError> {
        let mut n = self.starting.lock().unwrap();
        if self.is_stopping() {
            return Err(stopping_err());
        }
        *n += 1;
        Ok(StartGuard(self))
    }

    pub(in crate::server) fn status(&self) -> ServerStatusResult {
        let terms = self.terminals.lock().unwrap();
        ServerStatusResult {
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL_VERSION,
            pid: std::process::id(),
            pipe: self.config.pipe.clone(),
            started_ms: self.started_ms,
            uptime_ms: self.started.elapsed().as_millis() as u64,
            terminals: terms.len(),
            running: terms.iter().filter(|t| t.is_running()).count(),
            state_dir: self.config.state_dir.display().to_string(),
        }
    }

    pub(in crate::server) fn stop(&self, p: StopParams) -> ServerStopResult {
        let running = self
            .terminals
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.is_running())
            .count();
        // Reply first; the accept loop stops right after.
        let _ = self.shutdown.send(Some(p.kill));
        ServerStopResult {
            stopping: true,
            killed: if p.kill { running } else { 0 },
            detached: if p.kill { 0 } else { running },
        }
    }
}

pub(in crate::server) fn stopping_err() -> RpcError {
    RpcError::new(
        codes::SERVER_STOPPING,
        "podd is stopping (server.stop); nothing was done, retry once it has exited",
    )
}
