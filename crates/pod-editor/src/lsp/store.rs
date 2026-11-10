//! Per-worktree language-server manager (a GPUI entity).
//!
//! - one server process per (server id, root), started on first use;
//! - lifecycle: availability check -> spawn -> `initialize` -> running;
//!   crash -> restart with backoff (1..16 s, 5 attempts) -> failed;
//! - full-text document sync (`didOpen/didChange/didSave/didClose`);
//! - diagnostics store (editor char columns) with change events;
//! - typed requests whose results are converted to editor columns.
//!
//! Editor positions are `lsp_types::Position` in *chars* (what
//! gpui-component uses); [`crate::text::PosEncoding`] converts to the
//! server's unit at the boundary.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use futures::StreamExt as _;
use futures::channel::mpsc::unbounded;
use gpui_kit::{AppContext as _, Context, EventEmitter, Task};
use lsp_types as lt;
use serde_json::{Value, json};

use super::registry::{
    Availability, Language, RepoConfig, ServerConfig, ServerRegistry, check_available, find_root,
    language_for_path,
};
use super::server::{self, Initialized, ServerEvent, ServerProcess, restart_backoff};
use super::snippet::Snippet;
use crate::text::{self, PosEncoding};
use crate::uri::{path_key, path_to_uri, uri_to_path};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ServerKey {
    pub id: String,
    pub root: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ServerState {
    Checking,
    /// Not on PATH / probe failed: (reason, install hint).
    NotInstalled(String, String),
    Starting,
    Running,
    Restarting {
        attempt: u32,
        reason: String,
    },
    Failed(String),
    Stopping,
    Stopped,
    /// Stopped by the host to stay under its server cap (LRU); restarts the
    /// next time a document of this server is opened or activated.
    Suspended,
}

impl ServerState {
    /// Korean status-bar text.
    pub fn label(&self, id: &str) -> String {
        match self {
            ServerState::Checking => format!("{id} 확인 중"),
            ServerState::NotInstalled(_, _) => format!("{id} 설치 안 됨"),
            ServerState::Starting => format!("{id} 시작 중"),
            ServerState::Running => format!("{id} 실행 중"),
            ServerState::Restarting { attempt, .. } => {
                format!("{id} 재시작 중 ({attempt}/{})", server::MAX_RESTARTS)
            }
            ServerState::Failed(_) => format!("{id} 중지됨 (오류)"),
            ServerState::Stopping => format!("{id} 종료 중"),
            ServerState::Stopped => format!("{id} 종료됨"),
            ServerState::Suspended => format!("{id} 일시 중지됨"),
        }
    }
    pub fn is_running(&self) -> bool {
        matches!(self, ServerState::Running)
    }
}

struct Slot {
    cfg: ServerConfig,
    root: PathBuf,
    state: ServerState,
    process: Option<Arc<ServerProcess>>,
    init: Option<Initialized>,
    generation: u64,
    restarts: u32,
    /// When the current process finished `initialize` (crash counting).
    running_since: Option<Instant>,
    progress: BTreeMap<String, String>,
    progress_titles: BTreeMap<String, String>,
    /// Last open/edit/request for LRU eviction by the host.
    last_used: Cell<Instant>,
    _tasks: Vec<Task<()>>,
}

#[derive(Clone, Debug)]
pub struct DocState {
    pub path: PathBuf,
    pub uri: lt::Uri,
    pub language: Language,
    pub version: i32,
    pub text: String,
    pub server: Option<ServerKey>,
    pub opened_on_server: bool,
}

/// A diagnostic with its range in editor (char) columns.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDiagnostics {
    pub path: PathBuf,
    pub items: Vec<lt::Diagnostic>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Location {
    pub path: PathBuf,
    pub range: lt::Range,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEdit {
    pub path: PathBuf,
    /// Editor-column edits, in document order.
    pub edits: Vec<lt::TextEdit>,
}

/// What a completion item still needs after the editor inserted its text:
/// a tab-stop session and/or auto-import (`additionalTextEdits`).
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionExtra {
    /// Plain text the editor inserts for the item (snippet already expanded).
    pub text: String,
    pub snippet: Option<Snippet>,
    /// Editor-column edits elsewhere in the file (usually `use` lines).
    pub additional: Vec<lt::TextEdit>,
    /// The original item, when the server fills `additionalTextEdits` only
    /// on `completionItem/resolve` (requested after insertion).
    pub resolve: Option<lt::CompletionItem>,
}

/// Extras of the last completion response, matched against the next
/// insertion of the same text on the same line.
struct PendingCompletion {
    path: String,
    line: u32,
    at: Instant,
    items: Vec<CompletionExtra>,
}

/// Pending extras expire after this long (a stale menu is not reused).
const PENDING_COMPLETION_TTL: Duration = Duration::from_secs(120);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LspStoreEvent {
    DiagnosticsChanged(PathBuf),
    ServersChanged,
    /// A server asked to edit files (`workspace/applyEdit`, e.g. from a
    /// code action's command). The host applies them and answers `reply`.
    ApplyEdit {
        label: Option<String>,
        edits: Vec<FileEdit>,
        reply: server::EditReply,
    },
}

#[derive(Clone, Debug)]
pub struct ServerStatus {
    pub id: String,
    pub root: PathBuf,
    pub state: ServerState,
    pub progress: Option<String>,
}

impl ServerStatus {
    /// Short status-bar text: "rust-analyzer 실행 중 / 색인 중 / 설치 안 됨".
    pub fn short_label(&self) -> String {
        match (&self.state, &self.progress) {
            (ServerState::Running, Some(_)) => format!("{} 색인 중", self.id),
            (s, _) => s.label(&self.id),
        }
    }
}

/// A live server process, for a host-wide cap (see [`LspStore::running_servers`]).
#[derive(Clone, Debug)]
pub struct RunningServer {
    pub key: ServerKey,
    pub last_used: Instant,
}

pub struct LspStore {
    worktree: PathBuf,
    registry: ServerRegistry,
    pub config_errors: Vec<String>,
    /// The worktree's `.pod/lsp.json`, if any, and whether it is trusted.
    repo_config: Option<RepoConfig>,
    slots: HashMap<ServerKey, Slot>,
    docs: HashMap<String, DocState>,
    diagnostics: BTreeMap<String, FileDiagnostics>,
    /// Shortens timers in tests.
    pub backoff_scale: f32,
    enabled: bool,
    pending_completion: Option<PendingCompletion>,
}

impl EventEmitter<LspStoreEvent> for LspStore {}

impl LspStore {
    pub fn new(worktree: PathBuf) -> Self {
        let (registry, config_errors, repo_config) = ServerRegistry::load(&worktree);
        let mut s = Self::with_registry(worktree, registry, config_errors);
        s.repo_config = repo_config;
        s
    }

    pub fn with_registry(
        worktree: PathBuf,
        registry: ServerRegistry,
        config_errors: Vec<String>,
    ) -> Self {
        LspStore {
            worktree,
            registry,
            config_errors,
            repo_config: None,
            slots: HashMap::new(),
            docs: HashMap::new(),
            diagnostics: BTreeMap::new(),
            backoff_scale: 1.0,
            enabled: true,
            pending_completion: None,
        }
    }

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

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Log text for the server of `path` (status-bar "로그 보기"): state,
    /// root, the last stderr lines and configuration errors.
    pub fn server_log(&self, path: Option<&Path>) -> String {
        let mut out = Vec::new();
        let key = path
            .and_then(|p| self.docs.get(&path_key(p)))
            .and_then(|d| d.server.clone());
        let mut keys: Vec<&ServerKey> = match &key {
            Some(k) => vec![k],
            None => self.slots.keys().collect(),
        };
        keys.sort_by(|a, b| a.id.cmp(&b.id));
        for k in keys {
            let Some(s) = self.slots.get(k) else { continue };
            out.push(format!("# {} — {}", s.state.label(&k.id), s.root.display()));
            match &s.state {
                ServerState::Failed(e) | ServerState::Restarting { reason: e, .. } => {
                    out.push(e.clone())
                }
                ServerState::NotInstalled(why, hint) => {
                    out.push(why.clone());
                    out.push(hint.clone());
                }
                _ => {}
            }
            if let Some(p) = &s.process {
                out.extend(p.stderr_lines(200));
            }
        }
        if !self.enabled {
            out.push("(언어 서버 꺼짐)".into());
        }
        out.extend(self.config_errors.iter().map(|e| format!("설정 오류: {e}")));
        if let Some(r) = self.untrusted_repo_config() {
            out.push(format!(
                "승인 대기: {} (승인 전에는 이 파일의 명령을 쓰지 않습니다)",
                r.path.display()
            ));
            out.extend(r.commands.iter().map(|c| format!("  {c}")));
        }
        out.join("\n")
    }

    pub fn registry(&self) -> &ServerRegistry {
        &self.registry
    }

    /// The worktree's `.pod/lsp.json` when it waits for the user's approval
    /// (its commands are not used until then).
    pub fn untrusted_repo_config(&self) -> Option<&RepoConfig> {
        self.repo_config.as_ref().filter(|r| !r.trusted)
    }

    /// The user approved the worktree's `.pod/lsp.json` as shown: record
    /// `(repository, content hash)`, reload the registry and restart the
    /// servers with it. Refuses when the file changed since it was shown.
    pub fn trust_repo_config(&mut self, hash: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let dir = super::registry::user_config_dir().ok_or("사용자 설정 폴더를 찾지 못했습니다")?;
        let (_, _, now) = ServerRegistry::load_with(&self.worktree, Some(&dir));
        let now = now.ok_or(".pod/lsp.json이 없어졌습니다")?;
        if now.hash != hash {
            self.repo_config = Some(now);
            cx.notify();
            return Err(
                ".pod/lsp.json이 그 사이에 바뀌었습니다. 바뀐 내용을 다시 확인하세요".into(),
            );
        }
        now.trust(&dir)
            .map_err(|e| format!("승인을 저장하지 못했습니다: {e}"))?;
        self.reload_registry(Some(&dir), cx);
        Ok(())
    }

    /// Re-reads the server configuration and restarts running servers so
    /// they pick it up.
    pub fn reload_registry(&mut self, user_dir: Option<&Path>, cx: &mut Context<Self>) {
        let (registry, errors, repo) = ServerRegistry::load_with(&self.worktree, user_dir);
        self.registry = registry;
        self.config_errors = errors;
        self.repo_config = repo;
        if self.enabled {
            self.set_enabled(false, cx);
            self.set_enabled(true, cx);
        }
        cx.emit(LspStoreEvent::ServersChanged);
        cx.notify();
    }

    pub fn doc(&self, path: &Path) -> Option<&DocState> {
        self.docs.get(&path_key(path))
    }

    pub fn statuses(&self) -> Vec<ServerStatus> {
        let mut v: Vec<_> = self
            .slots
            .iter()
            .map(|(k, s)| ServerStatus {
                id: k.id.clone(),
                root: s.root.clone(),
                state: s.state.clone(),
                progress: s.progress.values().next_back().cloned(),
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id).then(a.root.cmp(&b.root)));
        v
    }

    /// Status of the server responsible for `path` (None: no server for this language).
    pub fn status_for(&self, path: &Path) -> Option<ServerStatus> {
        let doc = self.docs.get(&path_key(path))?;
        let key = doc.server.as_ref()?;
        let s = self.slots.get(key)?;
        Some(ServerStatus {
            id: key.id.clone(),
            root: s.root.clone(),
            state: s.state.clone(),
            progress: s.progress.values().next_back().cloned(),
        })
    }

    pub fn diagnostics(&self) -> impl Iterator<Item = &FileDiagnostics> {
        self.diagnostics.values()
    }

    pub fn diagnostics_for(&self, path: &Path) -> &[lt::Diagnostic] {
        self.diagnostics
            .get(&path_key(path))
            .map(|d| d.items.as_slice())
            .unwrap_or(&[])
    }

    pub fn diagnostic_counts(&self) -> (usize, usize) {
        let mut e = 0;
        let mut w = 0;
        for d in self.diagnostics.values().flat_map(|f| &f.items) {
            match d.severity {
                Some(lt::DiagnosticSeverity::ERROR) => e += 1,
                Some(lt::DiagnosticSeverity::WARNING) => w += 1,
                _ => {}
            }
        }
        (e, w)
    }

    // ---- documents -------------------------------------------------------

    /// Registers an open buffer; starts its server if needed.
    pub fn open_document(&mut self, path: &Path, text: String, cx: &mut Context<Self>) {
        let Some(language) = language_for_path(path) else {
            return;
        };
        let k = path_key(path);
        if self.docs.contains_key(&k) {
            self.change_document(path, text);
            return;
        }
        let server = if self.enabled {
            self.registry
                .for_language(language.lsp_id)
                .cloned()
                .map(|cfg| {
                    let root =
                        find_root(path, &self.worktree, &cfg.root_markers, cfg.root_strategy);
                    let key = ServerKey {
                        id: cfg.id.clone(),
                        root: path_key(&root),
                    };
                    self.ensure_slot(key.clone(), cfg, root, cx);
                    self.resume_slot(&key, cx);
                    key
                })
        } else {
            None
        };
        self.docs.insert(
            k.clone(),
            DocState {
                path: path.to_path_buf(),
                uri: path_to_uri(path),
                language,
                version: 0,
                text,
                server: server.clone(),
                opened_on_server: false,
            },
        );
        if let Some(key) = server
            && self.slots.get(&key).is_some_and(|s| s.state.is_running())
        {
            self.send_did_open(&k);
        }
        cx.emit(LspStoreEvent::ServersChanged);
    }

    /// Full-text sync; a no-op when the text is unchanged.
    pub fn change_document(&mut self, path: &Path, text: String) {
        let k = path_key(path);
        let Some(doc) = self.docs.get_mut(&k) else {
            return;
        };
        if doc.text == text {
            return;
        }
        doc.version += 1;
        doc.text = text;
        if !doc.opened_on_server {
            return;
        }
        if let Some(s) = doc.server.as_ref().and_then(|k| self.slots.get(k)) {
            s.last_used.set(Instant::now());
        }
        let Some(proc_) = doc
            .server
            .as_ref()
            .and_then(|s| self.slots.get(s))
            .and_then(|s| s.process.clone())
        else {
            return;
        };
        let _ = proc_.client.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":doc.uri,"version":doc.version},"contentChanges":[{"text":doc.text}]}),
        );
    }

    /// One editor edit (byte range of the previous text replaced by
    /// `new_text`). Sent as a ranged `didChange` when the server syncs
    /// incrementally, else as full text. If the store's copy no longer
    /// matches (an edit was missed), the range is rejected and the next
    /// full-text [`Self::change_document`] repairs it.
    pub fn edit_document(&mut self, path: &Path, start: usize, old_end: usize, new_text: &str) {
        let k = path_key(path);
        let key = self.docs.get(&k).and_then(|d| d.server.clone());
        let incremental = key
            .as_ref()
            .is_some_and(|key| self.sync_kind(key) == lt::TextDocumentSyncKind::INCREMENTAL);
        let enc = key
            .as_ref()
            .map(|key| self.encoding(key))
            .unwrap_or_default();
        let Some(doc) = self.docs.get_mut(&k) else {
            return;
        };
        let Some(range) = wire_range_of(&doc.text, start, old_end, enc) else {
            return;
        };
        doc.text.replace_range(start..old_end, new_text);
        doc.version += 1;
        if !doc.opened_on_server {
            return;
        }
        if let Some(s) = doc.server.as_ref().and_then(|k| self.slots.get(k)) {
            s.last_used.set(Instant::now());
        }
        let Some(proc_) = doc
            .server
            .as_ref()
            .and_then(|s| self.slots.get(s))
            .and_then(|s| s.process.clone())
        else {
            return;
        };
        let change = if incremental {
            json!({"range": range, "text": new_text})
        } else {
            json!({"text": doc.text})
        };
        let _ = proc_.client.notify(
            "textDocument/didChange",
            json!({"textDocument":{"uri":doc.uri,"version":doc.version},"contentChanges":[change]}),
        );
    }

    /// How the server wants document changes (full text before initialize).
    fn sync_kind(&self, key: &ServerKey) -> lt::TextDocumentSyncKind {
        match self
            .slots
            .get(key)
            .and_then(|s| s.init.as_ref())
            .and_then(|i| i.capabilities.text_document_sync.as_ref())
        {
            Some(lt::TextDocumentSyncCapability::Kind(k)) => *k,
            Some(lt::TextDocumentSyncCapability::Options(o)) => {
                o.change.unwrap_or(lt::TextDocumentSyncKind::FULL)
            }
            None => lt::TextDocumentSyncKind::FULL,
        }
    }

    pub fn save_document(&mut self, path: &Path) {
        let Some(doc) = self.docs.get(&path_key(path)) else {
            return;
        };
        if !doc.opened_on_server {
            return;
        }
        if let Some(p) = doc
            .server
            .as_ref()
            .and_then(|s| self.slots.get(s))
            .and_then(|s| s.process.clone())
        {
            let _ = p.client.notify(
                "textDocument/didSave",
                json!({"textDocument":{"uri":doc.uri}}),
            );
        }
    }

    pub fn close_document(&mut self, path: &Path) {
        let Some(doc) = self.docs.remove(&path_key(path)) else {
            return;
        };
        if !doc.opened_on_server {
            return;
        }
        if let Some(p) = doc
            .server
            .as_ref()
            .and_then(|s| self.slots.get(s))
            .and_then(|s| s.process.clone())
        {
            let _ = p.client.notify(
                "textDocument/didClose",
                json!({"textDocument":{"uri":doc.uri}}),
            );
        }
    }

    fn send_did_open(&mut self, doc_key: &str) {
        let Some(doc) = self.docs.get_mut(doc_key) else {
            return;
        };
        let Some(p) = doc
            .server
            .as_ref()
            .and_then(|s| self.slots.get(s))
            .and_then(|s| s.process.clone())
        else {
            return;
        };
        let ok = p
            .client
            .notify("textDocument/didOpen", json!({"textDocument":{"uri":doc.uri,"languageId":doc.language.lsp_id,"version":doc.version,"text":doc.text}}))
            .is_ok();
        doc.opened_on_server = ok;
    }

    // ---- server lifecycle -----------------------------------------------

    fn ensure_slot(
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
        res: Result<Initialized, super::rpc::RpcError>,
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
                        match super::registry::which(&slot.cfg.command) {
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
        let paths: Vec<String> = self.diagnostics.keys().cloned().collect();
        let owned: Vec<String> = paths
            .into_iter()
            .filter(|p| {
                self.docs
                    .get(p)
                    .is_some_and(|d| d.server.as_ref() == Some(key))
            })
            .collect();
        for p in owned {
            if let Some(d) = self.diagnostics.remove(&p) {
                cx.emit(LspStoreEvent::DiagnosticsChanged(d.path));
            }
        }
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
        let owned: Vec<String> = self
            .diagnostics
            .keys()
            .filter(|p| {
                self.docs
                    .get(*p)
                    .is_some_and(|d| d.server.as_ref() == Some(key))
            })
            .cloned()
            .collect();
        for p in owned {
            if let Some(d) = self.diagnostics.remove(&p) {
                cx.emit(LspStoreEvent::DiagnosticsChanged(d.path));
            }
        }
        cx.emit(LspStoreEvent::ServersChanged);
        cx.notify();
        let Some(p) = proc_ else {
            return Task::ready(());
        };
        let timer = cx.background_executor().timer(Duration::from_secs(2));
        cx.background_spawn(async move { p.shutdown(timer).await })
    }

    fn resume_slot(&mut self, key: &ServerKey, cx: &mut Context<Self>) {
        let Some(slot) = self.slots.get_mut(key) else {
            return;
        };
        if !matches!(slot.state, ServerState::Suspended) {
            return;
        }
        slot.restarts = 0;
        slot.last_used.set(Instant::now());
        match super::registry::which(&slot.cfg.command) {
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

    // ---- diagnostics -----------------------------------------------------

    fn text_for(&self, path: &Path) -> Option<String> {
        if let Some(d) = self.docs.get(&path_key(path)) {
            return Some(d.text.clone());
        }
        let len = std::fs::metadata(path).ok()?.len();
        if len > text::LARGE_FILE_BYTES {
            return None;
        }
        text::load(path).ok().map(|l| l.text)
    }

    fn encoding(&self, key: &ServerKey) -> PosEncoding {
        self.slots
            .get(key)
            .and_then(|s| s.init.as_ref())
            .map(|i| i.encoding)
            .unwrap_or_default()
    }

    fn on_diagnostics(&mut self, key: &ServerKey, params: Value, cx: &mut Context<Self>) {
        let Ok(p) = serde_json::from_value::<lt::PublishDiagnosticsParams>(params) else {
            return;
        };
        let Some(path) = uri_to_path(&p.uri) else {
            return;
        };
        let enc = self.encoding(key);
        if stale_diagnostics(
            p.version,
            self.docs.get(&path_key(&path)).map(|d| d.version),
        ) {
            // Ranges of an older text: lines may have moved or gone, in any
            // encoding. The server publishes again for the current version.
            return;
        }
        let items = if enc == PosEncoding::Utf32 || p.diagnostics.is_empty() {
            p.diagnostics
        } else {
            let text = self.text_for(&path).unwrap_or_default();
            p.diagnostics
                .into_iter()
                .map(|mut d| {
                    d.range = text::range_from_wire(&text, d.range, enc);
                    d
                })
                .collect()
        };
        let k = path_key(&path);
        if items.is_empty() {
            self.diagnostics.remove(&k);
        } else {
            self.diagnostics.insert(
                k,
                FileDiagnostics {
                    path: path.clone(),
                    items,
                },
            );
        }
        cx.emit(LspStoreEvent::DiagnosticsChanged(path));
        cx.notify();
    }

    // ---- requests --------------------------------------------------------

    fn request_target(
        &self,
        path: &Path,
        pos: lt::Position,
    ) -> Result<(
        Arc<ServerProcess>,
        PosEncoding,
        ServerKey,
        lt::TextDocumentPositionParams,
    )> {
        let doc = self
            .docs
            .get(&path_key(path))
            .ok_or_else(|| anyhow!("문서가 열려 있지 않습니다"))?;
        let key = doc
            .server
            .clone()
            .ok_or_else(|| anyhow!("이 언어를 위한 서버가 없습니다"))?;
        let slot = self.slots.get(&key).ok_or_else(|| anyhow!("서버 없음"))?;
        if !doc.opened_on_server {
            return Err(anyhow!("{}", slot.state.label(&key.id)));
        }
        let p = slot
            .process
            .clone()
            .ok_or_else(|| anyhow!("{}", slot.state.label(&key.id)))?;
        slot.last_used.set(Instant::now());
        let enc = self.encoding(&key);
        let wire = text::pos_to_wire(&doc.text, pos, enc);
        Ok((
            p,
            enc,
            key,
            lt::TextDocumentPositionParams {
                text_document: lt::TextDocumentIdentifier {
                    uri: doc.uri.clone(),
                },
                position: wire,
            },
        ))
    }

    pub fn has_capability(&self, path: &Path, cap: fn(&lt::ServerCapabilities) -> bool) -> bool {
        self.docs
            .get(&path_key(path))
            .and_then(|d| d.server.as_ref())
            .and_then(|k| self.slots.get(k))
            .and_then(|s| s.init.as_ref())
            .is_some_and(|i| cap(&i.capabilities))
    }

    fn range_from_server(&self, path: &Path, r: lt::Range, enc: PosEncoding) -> lt::Range {
        if enc == PosEncoding::Utf32 {
            return r;
        }
        match self.text_for(path) {
            Some(t) => text::range_from_wire(&t, r, enc),
            None => r,
        }
    }

    pub fn hover(
        &self,
        path: &Path,
        pos: lt::Position,
        cx: &mut Context<Self>,
    ) -> Task<Result<Option<lt::Hover>>> {
        let (p, enc, _, tdp) = match self.request_target(path, pos) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let fut = p
            .client
            .request_typed::<lt::request::HoverRequest>(lt::HoverParams {
                text_document_position_params: tdp,
                work_done_progress_params: Default::default(),
            });
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let mut h = fut.await.map_err(|e| anyhow!("{e}"))?;
            if let Some(hv) = h.as_mut()
                && let Some(r) = hv.range
            {
                hv.range = Some(this.update(cx, |s, _| s.range_from_server(&path, r, enc))?);
            }
            Ok(h)
        })
    }

    pub fn definition(
        &self,
        path: &Path,
        pos: lt::Position,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<Location>>> {
        let (p, enc, _, tdp) = match self.request_target(path, pos) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let fut = p
            .client
            .request_typed::<lt::request::GotoDefinition>(lt::GotoDefinitionParams {
                text_document_position_params: tdp,
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            });
        cx.spawn(async move |this, cx| {
            let res = fut.await.map_err(|e| anyhow!("{e}"))?;
            let raw: Vec<(lt::Uri, lt::Range)> = match res {
                None => vec![],
                Some(lt::GotoDefinitionResponse::Scalar(l)) => vec![(l.uri, l.range)],
                Some(lt::GotoDefinitionResponse::Array(v)) => {
                    v.into_iter().map(|l| (l.uri, l.range)).collect()
                }
                Some(lt::GotoDefinitionResponse::Link(v)) => v
                    .into_iter()
                    .map(|l| (l.target_uri, l.target_selection_range))
                    .collect(),
            };
            this.update(cx, |s, _| s.locations(raw, enc))
        })
    }

    fn locations(&self, raw: Vec<(lt::Uri, lt::Range)>, enc: PosEncoding) -> Vec<Location> {
        raw.into_iter()
            .filter_map(|(u, r)| {
                uri_to_path(&u).map(|path| Location {
                    range: self.range_from_server(&path, r, enc),
                    path,
                })
            })
            .collect()
    }

    pub fn references(
        &self,
        path: &Path,
        pos: lt::Position,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<Location>>> {
        let (p, enc, _, tdp) = match self.request_target(path, pos) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let fut = p
            .client
            .request_typed::<lt::request::References>(lt::ReferenceParams {
                text_document_position: tdp,
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: lt::ReferenceContext {
                    include_declaration: true,
                },
            });
        cx.spawn(async move |this, cx| {
            let res = fut.await.map_err(|e| anyhow!("{e}"))?.unwrap_or_default();
            let mut v = this.update(cx, |s, _| {
                s.locations(res.into_iter().map(|l| (l.uri, l.range)).collect(), enc)
            })?;
            v.sort_by(|a, b| {
                a.path
                    .cmp(&b.path)
                    .then(a.range.start.line.cmp(&b.range.start.line))
                    .then(a.range.start.character.cmp(&b.range.start.character))
            });
            Ok(v)
        })
    }

    pub fn completion(
        &self,
        path: &Path,
        pos: lt::Position,
        context: lt::CompletionContext,
        cx: &mut Context<Self>,
    ) -> Task<Result<lt::CompletionResponse>> {
        let (p, enc, _, tdp) = match self.request_target(path, pos) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let fut = p
            .client
            .request_typed::<lt::request::Completion>(lt::CompletionParams {
                text_document_position: tdp,
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: Some(context),
            });
        let resolve = self.has_capability(path, |c| {
            c.completion_provider
                .as_ref()
                .and_then(|o| o.resolve_provider)
                == Some(true)
        });
        let path = path.to_path_buf();
        let timer = cx.background_executor().timer(RESOLVE_WAIT);
        cx.spawn(async move |this, cx| {
            let res = fut
                .await
                .map_err(|e| anyhow!("{e}"))?
                .unwrap_or(lt::CompletionResponse::Array(vec![]));
            let text = this.update(cx, |s, _| s.text_for(&path).unwrap_or_default())?;
            let prefix = word_prefix(&text, pos);
            let (mut items, list) = match res {
                lt::CompletionResponse::Array(v) => (filter_completions(v, &prefix), None),
                lt::CompletionResponse::List(mut l) => (
                    filter_completions(std::mem::take(&mut l.items), &prefix),
                    Some(l),
                ),
            };
            // `completionItem/resolve` for the first few items (docs, detail,
            // the final edit). The menu has no lazy hook, so wait briefly.
            if resolve {
                let picks: Vec<usize> = items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| i.documentation.is_none())
                    .map(|(n, _)| n)
                    .take(RESOLVE_TOP)
                    .collect();
                let futs: Vec<_> = picks
                    .iter()
                    .map(|&n| {
                        p.client
                            .request_typed::<lt::request::ResolveCompletionItem>(items[n].clone())
                    })
                    .collect();
                let all = futures::future::join_all(futs);
                if let futures::future::Either::Left((done, _)) =
                    futures::future::select(Box::pin(all), Box::pin(timer)).await
                {
                    for (n, r) in picks.into_iter().zip(done) {
                        if let Ok(item) = r {
                            items[n] = item;
                        }
                    }
                }
            }
            let extras: Vec<CompletionExtra> = items
                .iter()
                .filter_map(|i| completion_extra(i, resolve))
                .map(|mut e| {
                    if enc != PosEncoding::Utf32 {
                        for ed in &mut e.additional {
                            ed.range = text::range_from_wire(&text, ed.range, enc);
                        }
                    }
                    e
                })
                .collect();
            this.update(cx, |s, _| s.note_completion(&path, pos.line, extras))?;
            for item in &mut items {
                expand_snippet_item(item);
            }
            let res = match list {
                None => lt::CompletionResponse::Array(items),
                Some(mut l) => {
                    l.items = items;
                    lt::CompletionResponse::List(l)
                }
            };
            if enc == PosEncoding::Utf32 {
                return Ok(res);
            }
            let fix = |mut item: lt::CompletionItem| {
                match item.text_edit.as_mut() {
                    Some(lt::CompletionTextEdit::Edit(e)) => {
                        e.range = text::range_from_wire(&text, e.range, enc)
                    }
                    Some(lt::CompletionTextEdit::InsertAndReplace(e)) => {
                        e.insert = text::range_from_wire(&text, e.insert, enc);
                        e.replace = text::range_from_wire(&text, e.replace, enc);
                    }
                    None => {}
                }
                item
            };
            Ok(match res {
                lt::CompletionResponse::Array(v) => {
                    lt::CompletionResponse::Array(v.into_iter().map(fix).collect())
                }
                lt::CompletionResponse::List(mut l) => {
                    l.items = l.items.into_iter().map(fix).collect();
                    lt::CompletionResponse::List(l)
                }
            })
        })
    }

    /// Remembers the extras of a completion response requested on `line`
    /// (replaces the previous response's).
    pub fn note_completion(&mut self, path: &Path, line: u32, items: Vec<CompletionExtra>) {
        self.pending_completion = Some(PendingCompletion {
            path: path_key(path),
            line,
            at: Instant::now(),
            items,
        });
    }

    /// The extras of a completion item the editor just inserted as
    /// `inserted` on `line` of `path` (from the last response, once).
    /// Single typed characters never match: a completion inserts a word.
    pub fn take_completion(
        &mut self,
        path: &Path,
        line: u32,
        inserted: &str,
    ) -> Option<CompletionExtra> {
        let p = self.pending_completion.as_ref()?;
        if p.path != path_key(path)
            || p.line != line
            || p.at.elapsed() > PENDING_COMPLETION_TTL
            || inserted.chars().count() < 2
        {
            return None;
        }
        let hit = p.items.iter().find(|e| e.text == inserted)?.clone();
        self.pending_completion = None;
        Some(hit)
    }

    /// `completionItem/resolve` for an inserted item's `additionalTextEdits`
    /// (editor columns).
    pub fn resolve_additional_edits(
        &self,
        path: &Path,
        item: lt::CompletionItem,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<lt::TextEdit>>> {
        let (p, enc, _, _) = match self.request_target(path, lt::Position::default()) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let fut = p
            .client
            .request_typed::<lt::request::ResolveCompletionItem>(item);
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let item = fut.await.map_err(|e| anyhow!("{e}"))?;
            let mut edits = item.additional_text_edits.unwrap_or_default();
            if enc != PosEncoding::Utf32 {
                let text = this.update(cx, |s, _| s.text_for(&path).unwrap_or_default())?;
                for e in &mut edits {
                    e.range = text::range_from_wire(&text, e.range, enc);
                }
            }
            Ok(edits)
        })
    }

    /// `textDocument/codeAction` for an editor (char-position) range, with
    /// the stored diagnostics that overlap it. Bare commands come back as
    /// actions with only `command` set; disabled actions are dropped.
    pub fn code_actions(
        &self,
        path: &Path,
        range: lt::Range,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<lt::CodeAction>>> {
        let (p, enc, _, tdp) = match self.request_target(path, range.start) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let text = self.text_for(path).unwrap_or_default();
        let wire = lt::Range::new(tdp.position, text::pos_to_wire(&text, range.end, enc));
        let diagnostics: Vec<lt::Diagnostic> = self
            .diagnostics_for(path)
            .iter()
            .filter(|d| d.range.start <= range.end && range.start <= d.range.end)
            .cloned()
            .map(|mut d| {
                d.range = lt::Range::new(
                    text::pos_to_wire(&text, d.range.start, enc),
                    text::pos_to_wire(&text, d.range.end, enc),
                );
                d
            })
            .collect();
        let fut = p
            .client
            .request_typed::<lt::request::CodeActionRequest>(lt::CodeActionParams {
                text_document: tdp.text_document,
                range: wire,
                context: lt::CodeActionContext {
                    diagnostics,
                    only: None,
                    trigger_kind: Some(lt::CodeActionTriggerKind::INVOKED),
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            });
        cx.background_spawn(async move {
            let res = fut.await.map_err(|e| anyhow!("{e}"))?.unwrap_or_default();
            Ok(res
                .into_iter()
                .map(|a| match a {
                    lt::CodeActionOrCommand::CodeAction(a) => a,
                    lt::CodeActionOrCommand::Command(c) => lt::CodeAction {
                        title: c.title.clone(),
                        command: Some(c),
                        ..Default::default()
                    },
                })
                .filter(|a| a.disabled.is_none())
                .collect())
        })
    }

    /// Edits of a code action (after `codeAction/resolve` when the server
    /// fills them lazily) as editor-position file edits, and its command.
    pub fn resolve_code_action(
        &self,
        path: &Path,
        action: lt::CodeAction,
        cx: &mut Context<Self>,
    ) -> Task<Result<(Vec<FileEdit>, Option<lt::Command>)>> {
        let (p, enc, _, _) = match self.request_target(path, lt::Position::default()) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let can_resolve = self.has_capability(path, |c| matches!(&c.code_action_provider, Some(lt::CodeActionProviderCapability::Options(o)) if o.resolve_provider == Some(true)));
        let fut = (action.edit.is_none() && can_resolve).then(|| {
            p.client
                .request_typed::<lt::request::CodeActionResolveRequest>(action.clone())
        });
        cx.spawn(async move |this, cx| {
            let action = match fut {
                Some(f) => f.await.map_err(|e| anyhow!("{e}"))?,
                None => action,
            };
            let edits = match action.edit {
                Some(we) => this.update(cx, |s, _| s.workspace_edit(we, enc))?,
                None => Vec::new(),
            };
            Ok((edits, action.command))
        })
    }

    /// `workspace/executeCommand` (a code action's command).
    pub fn execute_command(
        &self,
        path: &Path,
        command: lt::Command,
        cx: &mut Context<Self>,
    ) -> Task<Result<()>> {
        let (p, _, _, _) = match self.request_target(path, lt::Position::default()) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let fut = p
            .client
            .request_typed::<lt::request::ExecuteCommand>(lt::ExecuteCommandParams {
                command: command.command,
                arguments: command.arguments.unwrap_or_default(),
                work_done_progress_params: Default::default(),
            });
        cx.background_spawn(async move {
            fut.await.map_err(|e| anyhow!("{e}"))?;
            Ok(())
        })
    }

    /// Rename: returns the edits for preview; nothing is applied here.
    pub fn rename(
        &self,
        path: &Path,
        pos: lt::Position,
        new_name: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<Vec<FileEdit>>> {
        let (p, enc, _, tdp) = match self.request_target(path, pos) {
            Ok(x) => x,
            Err(e) => return Task::ready(Err(e)),
        };
        let fut = p
            .client
            .request_typed::<lt::request::Rename>(lt::RenameParams {
                text_document_position: tdp,
                new_name,
                work_done_progress_params: Default::default(),
            });
        cx.spawn(async move |this, cx| {
            let we = fut
                .await
                .map_err(|e| anyhow!("{e}"))?
                .ok_or_else(|| anyhow!("이름을 바꿀 수 없는 위치입니다"))?;
            this.update(cx, |s, _| s.workspace_edit(we, enc))
        })
    }

    pub fn workspace_edit(&self, we: lt::WorkspaceEdit, enc: PosEncoding) -> Vec<FileEdit> {
        let mut by_file: BTreeMap<String, FileEdit> = BTreeMap::new();
        let mut push = |uri: &lt::Uri, edits: Vec<lt::TextEdit>| {
            let Some(path) = uri_to_path(uri) else { return };
            let text = if enc == PosEncoding::Utf32 {
                None
            } else {
                self.text_for(&path)
            };
            let e = by_file.entry(path_key(&path)).or_insert_with(|| FileEdit {
                path: path.clone(),
                edits: vec![],
            });
            for mut ed in edits {
                if let Some(t) = &text {
                    ed.range = text::range_from_wire(t, ed.range, enc);
                }
                e.edits.push(ed);
            }
        };
        // `documentChanges` wins when a server sends both (LSP 3.17
        // WorkspaceEdit): applying both would make every edit twice.
        if let Some(dc) = we.document_changes {
            let ops: Vec<lt::TextDocumentEdit> = match dc {
                lt::DocumentChanges::Edits(v) => v,
                lt::DocumentChanges::Operations(v) => v
                    .into_iter()
                    .filter_map(|o| match o {
                        lt::DocumentChangeOperation::Edit(e) => Some(e),
                        lt::DocumentChangeOperation::Op(_) => None,
                    })
                    .collect(),
            };
            for e in ops {
                let edits = e
                    .edits
                    .into_iter()
                    .map(|x| match x {
                        lt::OneOf::Left(t) => t,
                        lt::OneOf::Right(a) => a.text_edit,
                    })
                    .collect();
                push(&e.text_document.uri, edits);
            }
        } else if let Some(changes) = we.changes {
            for (u, edits) in changes {
                push(&u, edits);
            }
        }
        let mut v: Vec<FileEdit> = by_file.into_values().collect();
        for f in &mut v {
            f.edits
                .sort_by_key(|a| (a.range.start.line, a.range.start.character));
        }
        v
    }
}

/// Diagnostics published for another version of an open document. Dropped
/// for every position encoding: freshness is separate from column units
/// (ui-editor-09). Unversioned publishes are always used.
fn stale_diagnostics(published: Option<i32>, current: Option<i32>) -> bool {
    matches!((published, current), (Some(p), Some(c)) if p != c)
}

/// A server that ran this long after `initialize` counts as healthy: its
/// next crash starts the backoff over instead of moving toward `Failed`.
const STABLE_RUN: Duration = Duration::from_secs(60);

/// The crash count to back off from after an exit: crashes hours apart in a
/// long session must not add up to a permanent failure.
fn restarts_after_exit(restarts: u32, ran_for: Option<Duration>, stable: Duration) -> u32 {
    if ran_for.is_some_and(|d| d >= stable) {
        0
    } else {
        restarts
    }
}

/// The wire range of bytes `start..old_end` of `text` (None if they are not
/// valid char boundaries of it: the copy is out of sync).
pub fn wire_range_of(
    text: &str,
    start: usize,
    old_end: usize,
    enc: PosEncoding,
) -> Option<lt::Range> {
    if start > old_end
        || old_end > text.len()
        || !text.is_char_boundary(start)
        || !text.is_char_boundary(old_end)
    {
        return None;
    }
    let at = |o| text::pos_to_wire(text, text::position_of(text, o), enc);
    Some(lt::Range::new(at(start), at(old_end)))
}

/// Completion items resolved before the menu opens, and how long to wait.
const RESOLVE_TOP: usize = 5;
const RESOLVE_WAIT: Duration = Duration::from_millis(250);

/// LSP snippet to plain text: placeholders keep their default text, tab
/// stops and `$0` vanish, choices take their first option, variables their
/// default (`${TM_FILENAME:x}` is `x`). Tab stops: [`Snippet::parse`].
pub fn expand_snippet(s: &str) -> String {
    Snippet::parse(s).text
}

/// The text the editor inserts for `item` (gpui-component's order: the
/// edit's text, else `insertText`, else the label).
pub fn inserted_text(item: &lt::CompletionItem) -> &str {
    match &item.text_edit {
        Some(lt::CompletionTextEdit::Edit(e)) => &e.new_text,
        Some(lt::CompletionTextEdit::InsertAndReplace(e)) => &e.new_text,
        None => item.insert_text.as_deref().unwrap_or(&item.label),
    }
}

/// Snippet stops and auto-import edits of `item` (before it is made plain
/// text), when it has any. `resolvable`: the server fills
/// `additionalTextEdits` on `completionItem/resolve`.
pub fn completion_extra(item: &lt::CompletionItem, resolvable: bool) -> Option<CompletionExtra> {
    let snippet = (item.insert_text_format == Some(lt::InsertTextFormat::SNIPPET))
        .then(|| Snippet::parse(inserted_text(item)))
        .filter(|s| !s.is_trivial());
    let additional = item.additional_text_edits.clone().unwrap_or_default();
    let resolve = (item.additional_text_edits.is_none() && resolvable && item.data.is_some())
        .then(|| item.clone());
    if snippet.is_none() && additional.is_empty() && resolve.is_none() {
        return None;
    }
    let text = match &snippet {
        Some(s) => s.text.clone(),
        None if item.insert_text_format == Some(lt::InsertTextFormat::SNIPPET) => {
            expand_snippet(inserted_text(item))
        }
        None => inserted_text(item).to_string(),
    };
    Some(CompletionExtra {
        text,
        snippet,
        additional,
        resolve,
    })
}

/// Makes a snippet item plain text (its insert text and its edit).
pub fn expand_snippet_item(item: &mut lt::CompletionItem) {
    if item.insert_text_format != Some(lt::InsertTextFormat::SNIPPET) {
        return;
    }
    if let Some(t) = item.insert_text.as_mut() {
        *t = expand_snippet(t);
    }
    match item.text_edit.as_mut() {
        Some(lt::CompletionTextEdit::Edit(e)) => e.new_text = expand_snippet(&e.new_text),
        Some(lt::CompletionTextEdit::InsertAndReplace(e)) => {
            e.new_text = expand_snippet(&e.new_text)
        }
        None => {}
    }
    item.insert_text_format = Some(lt::InsertTextFormat::PLAIN_TEXT);
}

/// Identifier characters immediately before `pos` (editor columns).
pub fn word_prefix(text: &str, pos: lt::Position) -> String {
    let line = text::line_of(text, pos.line);
    let before: Vec<char> = line.chars().take(pos.character as usize).collect();
    let start = before
        .iter()
        .rposition(|c| !(c.is_alphanumeric() || *c == '_'))
        .map(|i| i + 1)
        .unwrap_or(0);
    before[start..].iter().collect()
}

/// Servers return the whole candidate set (clients filter). Keep items whose
/// filter text fuzzily matches the typed prefix, best first, then the
/// server's `sortText` order.
pub fn filter_completions(items: Vec<lt::CompletionItem>, prefix: &str) -> Vec<lt::CompletionItem> {
    if prefix.is_empty() {
        return items;
    }
    let q = crate::fuzzy::Query::new(prefix);
    let mut scored: Vec<(i32, usize, lt::CompletionItem)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, it)| {
            let key = it.filter_text.clone().unwrap_or_else(|| it.label.clone());
            let m = crate::fuzzy::score(&q, &key)?;
            // Prefix matches first.
            let bonus = if key.to_lowercase().starts_with(&prefix.to_lowercase()) {
                1000
            } else {
                0
            };
            Some((m.score + bonus, i, it))
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| {
                a.2.sort_text
                    .as_deref()
                    .unwrap_or(&a.2.label)
                    .cmp(b.2.sort_text.as_deref().unwrap_or(&b.2.label))
            })
            .then(a.1.cmp(&b.1))
    });
    scored.into_iter().map(|(_, _, it)| it).collect()
}

/// The order to apply edits back to front: by start, last first, and among
/// edits at the same start the later one first, so several inserts at one
/// spot end up in array order (LSP: "inserted in the order they appear").
pub fn back_to_front<K: Ord + Copy>(starts: &[K]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..starts.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse((starts[i], i)));
    order
}

/// Applies editor-column edits to an LF text (used for files not open in a tab).
pub fn apply_edits(text: &str, edits: &[lt::TextEdit]) -> String {
    let starts: Vec<(u32, u32)> = edits
        .iter()
        .map(|e| (e.range.start.line, e.range.start.character))
        .collect();
    let mut out = text.to_string();
    for e in back_to_front(&starts).into_iter().map(|i| &edits[i]) {
        let s = text::offset_of(&out, e.range.start);
        let en = text::offset_of(&out, e.range.end).max(s);
        out.replace_range(s..en, &e.new_text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_expand_to_their_default_text() {
        assert_eq!(expand_snippet("finish($0)"), "finish()");
        assert_eq!(
            expand_snippet("println!(\"${1:msg}\");$0"),
            "println!(\"msg\");"
        );
        assert_eq!(
            expand_snippet("fn ${1:name}(${2:args}) {\n\t$0\n}"),
            "fn name(args) {\n\t\n}"
        );
        assert_eq!(
            expand_snippet("${1|one,two|} ${2:${3:nested}}"),
            "one nested"
        );
        assert_eq!(
            expand_snippet(r"cost \$5 ${TM_FILENAME:file} $TM_SELECTED_TEXT"),
            "cost $5 file "
        );
        assert_eq!(expand_snippet("${1}x${2/a/b/}"), "x");
        assert_eq!(expand_snippet("한글 ${1:값}"), "한글 값");
        let mut item = lt::CompletionItem {
            label: "f".into(),
            insert_text: Some("f(${1:x})".into()),
            insert_text_format: Some(lt::InsertTextFormat::SNIPPET),
            ..Default::default()
        };
        expand_snippet_item(&mut item);
        assert_eq!(
            (item.insert_text.as_deref(), item.insert_text_format),
            (Some("f(x)"), Some(lt::InsertTextFormat::PLAIN_TEXT))
        );
        let mut plain = lt::CompletionItem {
            label: "g".into(),
            insert_text: Some("g$1".into()),
            ..Default::default()
        };
        expand_snippet_item(&mut plain);
        assert_eq!(
            plain.insert_text.as_deref(),
            Some("g$1"),
            "plain-text items are left alone"
        );
    }

    #[test]
    fn completion_extras_match_the_inserted_text_once() {
        let use_line = lt::TextEdit {
            range: lt::Range::new(lt::Position::new(0, 0), lt::Position::new(0, 0)),
            new_text: "use std::collections::HashMap;\n".into(),
        };
        let item = lt::CompletionItem {
            label: "HashMap".into(),
            insert_text: Some("HashMap::new($0)".into()),
            insert_text_format: Some(lt::InsertTextFormat::SNIPPET),
            additional_text_edits: Some(vec![use_line.clone()]),
            ..Default::default()
        };
        let e = completion_extra(&item, false).expect("extra");
        assert_eq!(e.text, "HashMap::new()");
        assert_eq!(
            e.snippet
                .as_ref()
                .map(|s| (s.stops.len(), s.stops[0].clone())),
            Some((1, 13..13))
        );
        assert_eq!(e.additional, vec![use_line]);
        // Nothing to do after inserting a plain word.
        assert_eq!(
            completion_extra(
                &lt::CompletionItem {
                    label: "len".into(),
                    ..Default::default()
                },
                true
            ),
            None
        );
        // Lazily resolved auto-import: keep the item for `completionItem/resolve`.
        let lazy = lt::CompletionItem {
            label: "Rc".into(),
            data: Some(json!({"imports": 1})),
            ..Default::default()
        };
        assert_eq!(
            completion_extra(&lazy, true)
                .and_then(|e| e.resolve)
                .map(|i| i.label),
            Some("Rc".into())
        );
        assert_eq!(completion_extra(&lazy, false), None);

        let mut st =
            LspStore::with_registry(PathBuf::from("C:/w"), ServerRegistry::default(), vec![]);
        let file = Path::new("C:/w/src/main.rs");
        st.note_completion(file, 4, vec![e.clone()]);
        assert_eq!(
            st.take_completion(file, 3, "HashMap::new()"),
            None,
            "other line"
        );
        assert_eq!(
            st.take_completion(Path::new("C:/w/src/lib.rs"), 4, "HashMap::new()"),
            None,
            "other file"
        );
        assert_eq!(st.take_completion(file, 4, "H"), None, "typed character");
        assert_eq!(st.take_completion(file, 4, "HashMap::new()"), Some(e));
        assert_eq!(
            st.take_completion(file, 4, "HashMap::new()"),
            None,
            "only once"
        );
    }

    /// Replaying ranged changes the way a server does (wire columns) gives
    /// the editor's text, in UTF-16 and UTF-32, across emoji and Korean.
    #[test]
    fn incremental_ranges_replay_to_the_same_text() {
        fn apply_wire(text: &str, r: lt::Range, new: &str, enc: PosEncoding) -> String {
            let a = text::offset_of(text, text::pos_from_wire(text, r.start, enc));
            let b = text::offset_of(text, text::pos_from_wire(text, r.end, enc));
            format!("{}{}{}", &text[..a], new, &text[b..])
        }
        for enc in [PosEncoding::Utf16, PosEncoding::Utf32] {
            let mut editor = String::from("fn a() {}\nlet s = \"😀한글\";\n");
            let mut server = editor.clone();
            // (start byte, old end byte, inserted text) against the editor text.
            let after_emoji = editor.find('한').unwrap();
            // Each edit is computed against the editor text as it is then.
            for step in 0..3 {
                let (s0, e0, new) = match step {
                    0 => (after_emoji, after_emoji, "x"),
                    1 => (0, 2, "pub fn"),
                    _ => (editor.len(), editor.len(), "// 끝\n"),
                };
                let r = wire_range_of(&editor, s0, e0, enc).unwrap();
                server = apply_wire(&server, r, new, enc);
                editor.replace_range(s0..e0, new);
                assert_eq!(server, editor, "{enc:?}");
            }
        }
        // Inside a multi-byte char: out of sync, rejected.
        let t = "😀";
        assert_eq!(wire_range_of(t, 1, 2, PosEncoding::Utf16), None);
        assert_eq!(wire_range_of(t, 0, 9, PosEncoding::Utf16), None);
        assert_eq!(
            wire_range_of(t, 4, 4, PosEncoding::Utf16),
            Some(lt::Range::new(
                lt::Position::new(0, 2),
                lt::Position::new(0, 2)
            ))
        );
    }

    #[test]
    fn completion_filtering() {
        assert_eq!(word_prefix("    task.fi\n", lt::Position::new(0, 11)), "fi");
        assert_eq!(word_prefix("x", lt::Position::new(0, 0)), "");
        let item = |l: &str| lt::CompletionItem {
            label: l.into(),
            ..Default::default()
        };
        let out = filter_completions(
            vec![
                item("title"),
                item("clone_from"),
                item("finish"),
                item("done"),
            ],
            "fi",
        );
        let labels: Vec<_> = out.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels[0], "finish");
        assert!(!labels.contains(&"done"));
        assert_eq!(filter_completions(vec![item("a")], "").len(), 1);
    }

    #[test]
    fn apply_edits_back_to_front() {
        let t = "let foo = 1;\nfoo + foo\n";
        let e = |l, s, en| lt::TextEdit {
            range: lt::Range::new(lt::Position::new(l, s), lt::Position::new(l, en)),
            new_text: "bar".into(),
        };
        assert_eq!(
            apply_edits(t, &[e(0, 4, 7), e(1, 0, 3), e(1, 6, 9)]),
            "let bar = 1;\nbar + bar\n"
        );
    }

    /// editor-05: crash → healthy run → crash backs off from the start;
    /// crashes before `initialize` or soon after it keep counting.
    #[test]
    fn crash_count_resets_after_a_stable_run() {
        let stable = Duration::from_secs(60);
        assert_eq!(
            restarts_after_exit(4, Some(Duration::from_secs(3600)), stable),
            0
        );
        assert_eq!(
            restart_backoff(restarts_after_exit(4, Some(stable), stable)),
            Some(Duration::from_secs(1))
        );
        assert_eq!(
            restarts_after_exit(4, Some(Duration::from_secs(5)), stable),
            4
        );
        assert_eq!(restarts_after_exit(2, None, stable), 2);
    }

    /// Diagnostics for another version are dropped whatever the server's
    /// encoding (UTF-32 included, ui-editor-09); unversioned ones are used.
    #[test]
    fn stale_diagnostics_are_skipped_for_every_encoding() {
        assert!(stale_diagnostics(Some(3), Some(4)));
        assert!(!stale_diagnostics(Some(4), Some(4)));
        assert!(!stale_diagnostics(None, Some(4)));
        assert!(!stale_diagnostics(Some(3), None));
    }

    /// editor-04: inserts at one spot keep their array order.
    #[test]
    fn inserts_at_the_same_spot_keep_their_order() {
        let ins = |t: &str| lt::TextEdit {
            range: lt::Range::new(lt::Position::new(0, 0), lt::Position::new(0, 0)),
            new_text: t.into(),
        };
        assert_eq!(
            apply_edits("x\n", &[ins("use a;\n"), ins("use b;\n")]),
            "use a;\nuse b;\nx\n"
        );
        assert_eq!(back_to_front(&[3, 1, 3, 0]), vec![2, 0, 1, 3]);
    }

    /// editor-03: a server sending both `changes` and `documentChanges`
    /// gets its edits applied once (`documentChanges` wins).
    #[test]
    fn document_changes_win_over_changes() {
        let st = LspStore::with_registry(PathBuf::from("C:/w"), ServerRegistry::default(), vec![]);
        let uri = crate::uri::path_to_uri(Path::new("C:/w/a.rs"));
        let edit = lt::TextEdit {
            range: lt::Range::new(lt::Position::new(0, 0), lt::Position::new(0, 3)),
            new_text: "bar".into(),
        };
        let we = lt::WorkspaceEdit {
            changes: Some([(uri.clone(), vec![edit.clone()])].into_iter().collect()),
            document_changes: Some(lt::DocumentChanges::Edits(vec![lt::TextDocumentEdit {
                text_document: lt::OptionalVersionedTextDocumentIdentifier { uri, version: None },
                edits: vec![lt::OneOf::Left(edit.clone())],
            }])),
            change_annotations: None,
        };
        let files = st.workspace_edit(we, PosEncoding::Utf32);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].edits, vec![edit]);
    }
}
