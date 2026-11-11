//! Open documents and their sync with the server: `didOpen`, full-text and
//! ranged `didChange`, `didSave`, `didClose`, and the text of a file for
//! position conversion (open buffer, else disk).

use std::path::Path;
use std::time::Instant;

use gpui_kit::Context;
use lsp_types as lt;
use serde_json::json;

use super::edits::wire_range_of;
use super::{DocState, LspStore, LspStoreEvent, ServerKey};
use crate::lsp::registry::{find_root, language_for_path};
use crate::text;
use crate::uri::{path_key, path_to_uri};

impl LspStore {
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

    pub(super) fn send_did_open(&mut self, doc_key: &str) {
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

    /// The text of `path`: its open document, else the file (not for large
    /// files).
    pub(super) fn text_for(&self, path: &Path) -> Option<String> {
        if let Some(d) = self.docs.get(&path_key(path)) {
            return Some(d.text.clone());
        }
        let len = std::fs::metadata(path).ok()?.len();
        if len > text::LARGE_FILE_BYTES {
            return None;
        }
        text::load(path).ok().map(|l| l.text)
    }
}
