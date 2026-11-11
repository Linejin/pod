//! Position conversion between the server's unit and editor (char)
//! columns: wire ranges of editor edits, server ranges and locations back,
//! `WorkspaceEdit` to per-file editor edits, and applying those to a text.

use std::collections::BTreeMap;
use std::path::Path;

use lsp_types as lt;

use super::{FileEdit, Location, LspStore, ServerKey};
use crate::text::{self, PosEncoding};
use crate::uri::{path_key, uri_to_path};

impl LspStore {
    /// The position encoding the server of `key` negotiated (the default
    /// before `initialize`).
    pub(super) fn encoding(&self, key: &ServerKey) -> PosEncoding {
        self.slots
            .get(key)
            .and_then(|s| s.init.as_ref())
            .map(|i| i.encoding)
            .unwrap_or_default()
    }

    pub(super) fn range_from_server(
        &self,
        path: &Path,
        r: lt::Range,
        enc: PosEncoding,
    ) -> lt::Range {
        if enc == PosEncoding::Utf32 {
            return r;
        }
        match self.text_for(path) {
            Some(t) => text::range_from_wire(&t, r, enc),
            None => r,
        }
    }

    pub(super) fn locations(
        &self,
        raw: Vec<(lt::Uri, lt::Range)>,
        enc: PosEncoding,
    ) -> Vec<Location> {
        raw.into_iter()
            .filter_map(|(u, r)| {
                uri_to_path(&u).map(|path| Location {
                    range: self.range_from_server(&path, r, enc),
                    path,
                })
            })
            .collect()
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
