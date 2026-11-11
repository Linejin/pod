//! Published diagnostics in editor (char) columns: stored per file, dropped
//! when published for another document version or when their server is
//! gone, with a change event per file.

use std::path::Path;

use gpui_kit::Context;
use lsp_types as lt;
use serde_json::Value;

use super::{FileDiagnostics, LspStore, LspStoreEvent, ServerKey};
use crate::text::{self, PosEncoding};
use crate::uri::{path_key, uri_to_path};

impl LspStore {
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

    pub(super) fn on_diagnostics(
        &mut self,
        key: &ServerKey,
        params: Value,
        cx: &mut Context<Self>,
    ) {
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

    /// Drops the diagnostics of the documents `key` serves (its process
    /// exited or was suspended), one change event per file in path order.
    pub(super) fn clear_server_diagnostics(&mut self, key: &ServerKey, cx: &mut Context<Self>) {
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
    }
}

/// Diagnostics published for another version of an open document. Dropped
/// for every position encoding: freshness is separate from column units
/// (ui-editor-09). Unversioned publishes are always used.
pub(super) fn stale_diagnostics(published: Option<i32>, current: Option<i32>) -> bool {
    matches!((published, current), (Some(p), Some(c)) if p != c)
}
