//! Server processes: availability check, spawn and `initialize`, server
//! events, crash restarts with backoff, manual restart, LRU
//! suspend/resume, turning LSP off and on, shutdown.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::unbounded;
use gpui_kit::{AppContext as _, Context, Task};
use lsp_types as lt;

use super::{LspStore, LspStoreEvent, RunningServer, ServerKey, ServerState, Slot};
use crate::lsp::registry::{self, Availability, ServerConfig, check_available};
use crate::lsp::rpc::RpcError;
use crate::lsp::server::{self, Initialized, ServerEvent, ServerProcess, restart_backoff};
use crate::text::PosEncoding;
use crate::uri::path_key;

impl LspStore {
    /// Disables LSP entirely (e.g. a host setting, the status-bar "끄기");
    /// stops running servers. Enabling again re-registers every open
    /// document, which starts their servers.
    pub fn set_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.enabled == enabled {
            return;
        }
        self.enabled = enabled;
        if !enabled {
            self.shutdown_all(cx).detach();
            self.slots.clear();
            for d in self.docs.values_mut() {
                d.server = None;
            }
            let owned: Vec<String> = self.diagnostics.keys().cloned().collect();
            for k in owned {
                if let Some(d) = self.diagnostics.remove(&k) {
                    cx.emit(LspStoreEvent::DiagnosticsChanged(d.path));
                }
            }
        } else {
            let docs: Vec<(PathBuf, String)> = self
                .docs
                .values()
                .map(|d| (d.path.clone(), d.text.clone()))
                .collect();
            self.docs.clear();
            for (p, t) in docs {
                self.open_document(&p, t, cx);
            }
        }
        cx.emit(LspStoreEvent::ServersChanged);
        cx.notify();
    }

    pub(super) fn ensure_slot(
        &mut self,
        key: ServerKey,
        cfg: ServerConfig,
        root: PathBuf,
        cx: &mut Context<Self>,
    ) {
        if self.slots.contains_key(&key) {
            return;
        }
        self.slots.insert(
            key.clone(),
            Slot {
                cfg: cfg.clone(),
                root,
                state: ServerState::Checking,
                process: None,
                init: None,
                generation: 0,
                restarts: 0,
                running_since: None,
                progress: BTreeMap::new(),
                progress_titles: BTreeMap::new(),
                last_used: Cell::new(Instant::now()),
                _tasks: vec![],
            },
        );
        let check = cx.background_spawn(async move { check_available(&cfg) });
        let k2 = key.clone();
        let t = cx.spawn(async move |this, cx| {
            let avail = check.await;
            let _ = this.update(cx, |s, cx| match avail {
                Availability::Installed(exe) => s.start_process(k2, exe, cx),
                Availability::Missing(reason) => {
                    if let Some(slot) = s.slots.get_mut(&k2) {
                        slot.state =
                            ServerState::NotInstalled(reason, slot.cfg.install_hint.clone());
                    }
                    cx.emit(LspStoreEvent::ServersChanged);
                    cx.notify();
                }
            });
        });
        self.slots.get_mut(&key).expect("slot")._tasks.push(t);
    }

    fn start_process(&mut self, key: ServerKey, exe: PathBuf, cx: &mut Context<Self>) {
        let Some(slot) = self.slots.get_mut(&key) else {
            return;
        };
        slot.generation += 1;
        let generation = slot.generation;
        slot.running_since = None;
        slot.state = if slot.restarts == 0 {
            ServerState::Starting
        } else {
            slot.state.clone()
        };
        slot.progress.clear();
        slot.progress_titles.clear();
        let (tx, mut rx) = unbounded::<(u64, ServerEvent)>();
        let process = match ServerProcess::spawn(&exe, &slot.cfg, &slot.root, tx, generation) {
            Ok(p) => Arc::new(p),
            Err(e) => {
                slot.state = ServerState::Failed(format!("실행 실패: {e}"));
                cx.emit(LspStoreEvent::ServersChanged);
                return;
            }
        };
        slot.process = Some(process.clone());
        let root = slot.root.clone();
        let opts = slot.cfg.initialization_options.clone();
        // Event pump for this process generation.
        let k_ev = key.clone();
        let pump = cx.spawn(async move |this, cx| {
            while let Some((g, ev)) = rx.next().await {
                if this
                    .update(cx, |s, cx| s.on_server_event(&k_ev, g, ev, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let client = process.client.clone();
        let init =
            cx.background_spawn(async move { server::initialize(&client, &root, opts).await });
        let k_init = key.clone();
        let t = cx.spawn(async move |this, cx| {
            let res = init.await;
            let _ = this.update(cx, |s, cx| s.on_initialized(&k_init, generation, res, cx));
        });
        slot._tasks = vec![pump, t];
        cx.emit(LspStoreEvent::ServersChanged);
        cx.notify();
    }

    fn on_initialized(
        &mut self,
        key: &ServerKey,
        generation: u64,
        res: Result<Initialized, RpcError>,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.slots.get_mut(key) else {
            return;
        };
        if slot.generation != generation {
            return;
        }
        match res {
            Ok(init) => {
                slot.init = Some(init);
                slot.state = ServerState::Running;
                slot.running_since = Some(Instant::now());
                let docs: Vec<String> = self
                    .docs
                    .iter()
                    .filter(|(_, d)| d.server.as_ref() == Some(key))
                    .map(|(k, _)| k.clone())
                    .collect();
                for k in docs {
                    if let Some(d) = self.docs.get_mut(&k) {
                        d.opened_on_server = false;
                    }
                    self.send_did_open(&k);
                }
            }
            Err(e) => {
                // A crash during initialize also arrives as Exited; only
                // record the error if the process is still around.
                if slot.process.as_ref().is_some_and(|p| p.client.is_alive()) {
                    slot.state = ServerState::Failed(format!("initialize 실패: {e}"));
                }
            }
        }
        cx.emit(LspStoreEvent::ServersChanged);
        cx.notify();
    }

    fn on_server_event(
        &mut self,
        key: &ServerKey,
        generation: u64,
        ev: ServerEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(slot) = self.slots.get_mut(key) else {
            return;
        };
        if slot.generation != generation {
            return;
        }
        match ev {
            ServerEvent::Exited => self.on_exited(key, cx),
            ServerEvent::ApplyEdit { params, reply } => {
                let enc = slot
                    .init
                    .as_ref()
                    .map(|i| i.encoding)
                    .unwrap_or(PosEncoding::Utf16);
                match serde_json::from_value::<lt::ApplyWorkspaceEditParams>(params) {
                    Ok(p) => {
                        let edits = self.workspace_edit(p.edit, enc);
                        cx.emit(LspStoreEvent::ApplyEdit {
                            label: p.label,
                            edits,
                            reply,
                        });
                    }
                    Err(_) => reply.send(false),
                }
            }
            ServerEvent::Notification { method, params } => match method.as_str() {
                "textDocument/publishDiagnostics" => self.on_diagnostics(key, params, cx),
                "$/progress" => {
                    let token = params
                        .get("token")
                        .map(|t| t.to_string())
                        .unwrap_or_default();
                    let v = &params["value"];
                    match v["kind"].as_str() {
                        Some("end") => {
                            slot.progress.remove(&token);
                            slot.progress_titles.remove(&token);
                        }
                        Some(_) => {
                            if let Some(t) = v["title"].as_str() {
                                slot.progress_titles.insert(token.clone(), t.to_string());
                            }
                            let title = slot
                                .progress_titles
                                .get(&token)
                                .cloned()
                                .unwrap_or_default();
                            let mut s = title;
                            if let Some(m) = v["message"].as_str() {
                                s = format!("{s} · {m}");
                            }
                            if let Some(p) = v["percentage"].as_u64() {
                                s = format!("{s} {p}%");
                            }
                            slot.progress.insert(token, s);
                        }
                        None => {}
                    }
                    cx.emit(LspStoreEvent::ServersChanged);
                    cx.notify();
                }
                _ => {}
            },
        }
    }

    fn on_exited(&mut self, key: &ServerKey, cx: &mut Context<Self>) {
        let backoff_scale = self.backoff_scale;
        let Some(slot) = self.slots.get_mut(key) else {
            return;
        };
        let stderr = slot
            .process
            .as_ref()
            .map(|p| p.stderr_summary())
            .unwrap_or_default();
        slot.process = None;
        slot.init = None;
        slot.progress.clear();
        slot.progress_titles.clear();
        for d in self.docs.values_mut() {
            if d.server.as_ref() == Some(key) {
                d.opened_on_server = false;
            }
        }
        if matches!(slot.state, ServerState::Suspended) {
            cx.emit(LspStoreEvent::ServersChanged);
            return;
        }
        if matches!(slot.state, ServerState::Stopping | ServerState::Stopped) {
            slot.state = ServerState::Stopped;
            cx.emit(LspStoreEvent::ServersChanged);
            return;
        }
        let reason = if stderr.is_empty() {
            "서버가 예기치 않게 종료되었습니다".to_string()
        } else {
            stderr
        };
        slot.restarts = restarts_after_exit(
            slot.restarts,
            slot.running_since.take().map(|t| t.elapsed()),
            STABLE_RUN.mul_f32(backoff_scale),
        );
        match restart_backoff(slot.restarts) {
            Some(delay) => {
                slot.restarts += 1;
                slot.state = ServerState::Restarting {
                    attempt: slot.restarts,
                    reason,
                };
                let delay = delay.mul_f32(backoff_scale);
                let k = key.clone();
                let generation = slot.generation;
                let t = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |s, cx| {
                        let Some(slot) = s.slots.get(&k) else { return };
                        if slot.generation != generation
                            || !matches!(slot.state, ServerState::Restarting { .. })
                        {
                            return;
                        }
                        match registry::which(&slot.cfg.command) {
                            Some(exe) => s.start_process(k, exe, cx),
                            None => {
                                if let Some(slot) = s.slots.get_mut(&k) {
                                    slot.state = ServerState::NotInstalled(
                                        "PATH에서 사라졌습니다".into(),
                                        slot.cfg.install_hint.clone(),
                                    );
                                }
                            }
                        }
                    });
                });
                slot._tasks.push(t);
            }
            None => slot.state = ServerState::Failed(reason),
        }
        // Stale diagnostics from a dead server are misleading.
        self.clear_server_diagnostics(key, cx);
        cx.emit(LspStoreEvent::ServersChanged);
        cx.notify();
    }

    /// Manual restart (status-bar action): resets the crash counter.
    pub fn restart(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(key) = self
            .docs
            .get(&path_key(path))
            .and_then(|d| d.server.clone())
        else {
            return;
        };
        if let Some(slot) = self.slots.get_mut(&key) {
            if let Some(p) = slot.process.take() {
                p.kill();
            }
            slot.restarts = 0;
            slot.generation += 1;
        }
        self.slots.remove(&key);
        let docs: Vec<(PathBuf, String)> = self
            .docs
            .values()
            .filter(|d| d.server.as_ref() == Some(&key))
            .map(|d| (d.path.clone(), d.text.clone()))
            .collect();
        for (p, _) in &docs {
            self.docs.remove(&path_key(p));
        }
        for (p, t) in docs {
            self.open_document(&p, t, cx);
        }
    }

    /// Servers with a live process (starting, running or restarting), for a
    /// host-wide cap with LRU eviction ([`Self::suspend`]).
    pub fn running_servers(&self) -> Vec<RunningServer> {
        self.slots
            .iter()
            .filter(|(_, s)| {
                s.process.is_some()
                    || matches!(
                        s.state,
                        ServerState::Checking
                            | ServerState::Starting
                            | ServerState::Restarting { .. }
                    )
            })
            .map(|(k, s)| RunningServer {
                key: k.clone(),
                last_used: s.last_used.get(),
            })
            .collect()
    }

    /// Marks the server of `path` as used now and restarts it if the host
    /// suspended it (the user came back to one of its files).
    pub fn touch(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(key) = self
            .docs
            .get(&path_key(path))
            .and_then(|d| d.server.clone())
        else {
            return;
        };
        if let Some(s) = self.slots.get(&key) {
            s.last_used.set(Instant::now());
        }
        self.resume_slot(&key, cx);
    }

    /// Politely stops one server (kills its tree after 2 s) and marks it
    /// [`ServerState::Suspended`]. Its documents stay registered; diagnostics
    /// from it are cleared. Used by the host to cap concurrent servers.
    pub fn suspend(&mut self, key: &ServerKey, cx: &mut Context<Self>) -> Task<()> {
        let Some(slot) = self.slots.get_mut(key) else {
            return Task::ready(());
        };
        slot.generation += 1;
        slot.state = ServerState::Suspended;
        slot.init = None;
        slot.progress.clear();
        slot.progress_titles.clear();
        slot._tasks.clear();
        let proc_ = slot.process.take();
        for d in self.docs.values_mut() {
            if d.server.as_ref() == Some(key) {
                d.opened_on_server = false;
            }
        }
        self.clear_server_diagnostics(key, cx);
        cx.emit(LspStoreEvent::ServersChanged);
        cx.notify();
        let Some(p) = proc_ else {
            return Task::ready(());
        };
        let timer = cx.background_executor().timer(Duration::from_secs(2));
        cx.background_spawn(async move { p.shutdown(timer).await })
    }

    pub(super) fn resume_slot(&mut self, key: &ServerKey, cx: &mut Context<Self>) {
        let Some(slot) = self.slots.get_mut(key) else {
            return;
        };
        if !matches!(slot.state, ServerState::Suspended) {
            return;
        }
        slot.restarts = 0;
        slot.last_used.set(Instant::now());
        match registry::which(&slot.cfg.command) {
            Some(exe) => self.start_process(key.clone(), exe, cx),
            None => {
                slot.state = ServerState::NotInstalled(
                    "PATH에서 사라졌습니다".into(),
                    slot.cfg.install_hint.clone(),
                );
                cx.emit(LspStoreEvent::ServersChanged);
            }
        }
    }

    /// Polite shutdown of every server; kills trees after 2 s.
    pub fn shutdown_all(&mut self, cx: &mut Context<Self>) -> Task<()> {
        let mut procs = Vec::new();
        for slot in self.slots.values_mut() {
            slot.state = ServerState::Stopping;
            if let Some(p) = slot.process.take() {
                procs.push(p);
            }
        }
        for d in self.docs.values_mut() {
            d.opened_on_server = false;
        }
        cx.emit(LspStoreEvent::ServersChanged);
        let ex = cx.background_executor().clone();
        cx.background_spawn(async move {
            let all = procs
                .iter()
                .map(|p| p.shutdown(ex.timer(Duration::from_secs(2))));
            futures::future::join_all(all).await;
        })
    }

    /// Kills every server process tree immediately (window close path).
    pub fn kill_all(&mut self) {
        for slot in self.slots.values_mut() {
            slot.state = ServerState::Stopped;
            if let Some(p) = slot.process.take() {
                p.kill();
            }
        }
    }
}

/// A server that ran this long after `initialize` counts as healthy: its
/// next crash starts the backoff over instead of moving toward `Failed`.
const STABLE_RUN: Duration = Duration::from_secs(60);

/// The crash count to back off from after an exit: crashes hours apart in a
/// long session must not add up to a permanent failure.
pub(super) fn restarts_after_exit(
    restarts: u32,
    ran_for: Option<Duration>,
    stable: Duration,
) -> u32 {
    if ran_for.is_some_and(|d| d >= stable) {
        0
    } else {
        restarts
    }
}
