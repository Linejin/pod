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
//!
//! This file owns the state, the public types and the events; the
//! `impl LspStore` blocks live in private modules by what they do:
//! `lifecycle` (start, restart, LRU suspend/resume, shutdown, server
//! events), `documents` (open/change/save/close), `diagnostics`
//! (published, stale ones dropped), `requests` (hover … rename, completion
//! and its extra edits, code actions) and `edits` (wire positions and
//! `WorkspaceEdit` to editor columns).

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::{Context, EventEmitter, Task};
use lsp_types as lt;

use super::registry::{Language, RepoConfig, ServerConfig, ServerRegistry};
use super::server::{self, Initialized, ServerProcess};
use super::snippet::Snippet;
use crate::uri::path_key;

mod diagnostics;
mod documents;
mod edits;
mod lifecycle;
mod requests;

pub use edits::{apply_edits, back_to_front, wire_range_of};
pub use requests::{
    completion_extra, expand_snippet, expand_snippet_item, filter_completions, inserted_text,
    word_prefix,
};

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
}

#[cfg(test)]
mod tests {
    use super::diagnostics::stale_diagnostics;
    use super::lifecycle::restarts_after_exit;
    use super::server::restart_backoff;
    use super::*;
    use crate::text::{self, PosEncoding};
    use serde_json::json;
    use std::time::Duration;

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
