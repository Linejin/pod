//! Typed requests (hover, definition, references, completion, code
//! actions, commands, rename) with results in editor columns, and what a
//! completion still needs after insertion: snippet stops and extra edits
//! (`additionalTextEdits`, also from `completionItem/resolve`).

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow};
use gpui_kit::{AppContext as _, Context, Task};
use lsp_types as lt;

use super::{CompletionExtra, FileEdit, Location, LspStore, PendingCompletion, ServerKey};
use crate::lsp::server::ServerProcess;
use crate::lsp::snippet::Snippet;
use crate::text::{self, PosEncoding};
use crate::uri::path_key;

/// Pending extras expire after this long (a stale menu is not reused).
const PENDING_COMPLETION_TTL: Duration = Duration::from_secs(120);

/// Completion items resolved before the menu opens, and how long to wait.
const RESOLVE_TOP: usize = 5;
const RESOLVE_WAIT: Duration = Duration::from_millis(250);

impl LspStore {
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
}

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
