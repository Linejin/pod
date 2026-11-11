//! Rendering of the modal overlays: quick open, go to line, rename and its
//! preview, disk diff, the tree's new-entry and rename inputs, and the
//! confirm prompts (unsaved close, delete).

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lsp_types as lt;
use pod_ui::{Tokens, tokens::color as tc};

use crate::lsp::store::apply_edits;
use crate::text;
use crate::ui::strings as txt;
use crate::uri::path_key;

use super::{EditorWorkspace, Overlay, rel_of};

impl EditorWorkspace {
    pub(super) fn render_overlay(
        &mut self,
        t: Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let mode = self.mode;
        let frame = |title: &str| {
            div()
                .w(px(480.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(12.))
                .bg(tc(t.bg_raised))
                .border_1()
                .border_color(tc(t.border_subtle))
                .rounded(px(12.))
                .shadow(pod_ui::tokens::elevation(2, mode))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tc(t.text_primary))
                        .child(title.to_string()),
                )
        };
        let esc = cx.listener(|this, e: &KeyDownEvent, w, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                this.close_overlay(w, cx);
            }
        });
        let button = |id: &'static str, label: &'static str, primary: bool, danger: bool| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .tab_index(0)
                .focus_visible(move |st| pod_ui::focus_style(st, mode))
                .px(px(12.))
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |d| {
                    d.bg(tc(if danger {
                        t.danger_fg
                    } else {
                        t.action_primary_bg
                    }))
                    .text_color(tc(t.action_primary_fg))
                })
                .when(!primary, |d| {
                    d.border_1()
                        .border_color(tc(t.border_control))
                        .text_color(tc(t.text_primary))
                        .hover(|d| d.bg(tc(t.bg_hover)))
                })
                .child(label)
        };
        let err_line = |e: &Option<String>| {
            e.clone().map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(tc(t.danger_fg))
                    .child(e)
            })
        };
        let el = match &self.overlay {
            Overlay::None => return None,
            Overlay::ConfirmClose { dirty, close, .. } => {
                let many = close.len() > 1;
                let body = if many {
                    txt::unsaved_many_body(close.len(), dirty.len())
                } else {
                    txt::unsaved_body(&rel_of(&self.root, &dirty[0]))
                };
                let files: Vec<String> = if many {
                    dirty.iter().map(|p| rel_of(&self.root, p)).collect()
                } else {
                    vec![]
                };
                let keys =
                    cx.listener(
                        |this, e: &KeyDownEvent, w, cx| match e.keystroke.key.as_str() {
                            "escape" => {
                                cx.stop_propagation();
                                this.resolve_close(None, w, cx);
                            }
                            "enter" if this.prompt_focus.is_focused(w) => {
                                cx.stop_propagation();
                                this.resolve_close(Some(true), w, cx);
                            }
                            _ => {}
                        },
                    );
                frame(txt::UNSAVED_TITLE)
                    .id("confirm-close")
                    .track_focus(&self.prompt_focus)
                    .role(Role::AlertDialog)
                    .aria_label(txt::UNSAVED_TITLE)
                    .capture_key_down(keys)
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(tc(t.text_secondary))
                            .child(body),
                    )
                    .when(!files.is_empty(), |d| {
                        d.child(
                            div()
                                .id("confirm-close-files")
                                .role(Role::List)
                                .aria_label(txt::UNSAVED_TITLE)
                                .max_h(px(160.))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .children(files.into_iter().enumerate().map(|(i, f)| {
                                    div()
                                        .id(("confirm-close-file", i))
                                        .role(Role::ListItem)
                                        .aria_label(f.clone())
                                        .text_size(px(12.))
                                        .font_family("Cascadia Mono")
                                        .text_color(tc(t.text_primary))
                                        .child(f)
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(button("close-cancel", txt::CANCEL, false, false).on_click(
                                cx.listener(|this, _, w, cx| this.resolve_close(None, w, cx)),
                            ))
                            .child(
                                button("close-discard", txt::CLOSE_WITHOUT_SAVING, false, false)
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.resolve_close(Some(false), w, cx)
                                    })),
                            )
                            .child(
                                button(
                                    "close-save",
                                    if many {
                                        txt::SAVE_ALL_AND_CLOSE
                                    } else {
                                        txt::SAVE_AND_CLOSE
                                    },
                                    true,
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, _, w, cx| this.resolve_close(Some(true), w, cx),
                                )),
                            ),
                    )
                    .into_any_element()
            }
            Overlay::NewEntry {
                parent,
                folder,
                input,
                error,
            } => {
                let rel = rel_of(&self.root, parent);
                let title = format!(
                    "{} · {}",
                    if *folder {
                        txt::NEW_FOLDER_TITLE
                    } else {
                        txt::NEW_FILE_TITLE
                    },
                    if rel.is_empty() {
                        "/".to_string()
                    } else {
                        format!("{rel}/")
                    }
                );
                frame(&title)
                    .capture_key_down(esc)
                    .child(Input::new(input))
                    .children(err_line(error))
                    .child(hint(t, txt::ENTER_CREATE))
                    .into_any_element()
            }
            Overlay::RenameEntry { path, input, error } => {
                let title = format!("{} · {}", txt::RENAME_TITLE, rel_of(&self.root, path));
                frame(&title)
                    .capture_key_down(esc)
                    .child(Input::new(input))
                    .children(err_line(error))
                    .child(hint(t, txt::ENTER_RENAME))
                    .into_any_element()
            }
            Overlay::ConfirmDelete(path) => {
                let rel = rel_of(&self.root, path);
                let keys =
                    cx.listener(
                        |this, e: &KeyDownEvent, w, cx| match e.keystroke.key.as_str() {
                            "escape" => {
                                cx.stop_propagation();
                                this.close_overlay(w, cx);
                            }
                            "enter" if this.prompt_focus.is_focused(w) => {
                                cx.stop_propagation();
                                this.commit_delete(w, cx);
                            }
                            _ => {}
                        },
                    );
                frame(txt::DELETE_TITLE)
                    .id("confirm-delete")
                    .track_focus(&self.prompt_focus)
                    .role(Role::AlertDialog)
                    .aria_label(txt::DELETE_TITLE)
                    .capture_key_down(keys)
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(tc(t.text_secondary))
                            .child(txt::delete_body(&rel, path.is_dir())),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                button("delete-cancel", txt::CANCEL, false, false).on_click(
                                    cx.listener(|this, _, w, cx| this.close_overlay(w, cx)),
                                ),
                            )
                            .child(
                                button("delete-ok", txt::DELETE_CONFIRM, true, true).on_click(
                                    cx.listener(|this, _, w, cx| this.commit_delete(w, cx)),
                                ),
                            ),
                    )
                    .into_any_element()
            }
            Overlay::DiskDiff { rel, lines } => {
                let keys = cx.listener(|this, e: &KeyDownEvent, w, cx| {
                    if e.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close_overlay(w, cx);
                    }
                });
                let mut list = div()
                    .id("disk-diff")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .font_family("Cascadia Mono")
                    .text_size(px(12.));
                if lines.is_empty() {
                    list = list.child(div().text_color(tc(t.text_muted)).child("차이가 없습니다"));
                }
                for (tag, line) in lines {
                    let (bg, fg) = match tag {
                        '-' => (Some(t.danger_bg), t.danger_fg),
                        '+' => (Some(t.success_bg), t.success_fg),
                        _ => (None, t.text_muted),
                    };
                    list = list.child(
                        div()
                            .px(px(4.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(tc(fg))
                            .when_some(bg, |d, bg| d.bg(tc(bg)))
                            .child(format!("{tag} {line}")),
                    );
                }
                frame(&format!("디스크(-)와 내 변경(+) 비교 · {rel}"))
                    .id("disk-diff-dialog")
                    .w(px(720.))
                    .track_focus(&self.prompt_focus)
                    .role(Role::AlertDialog)
                    .capture_key_down(keys)
                    .child(list)
                    .child(
                        div().flex().justify_end().child(
                            button("disk-diff-close", "닫기 (Esc)", false, false)
                                .on_click(cx.listener(|this, _, w, cx| this.close_overlay(w, cx))),
                        ),
                    )
                    .into_any_element()
            }
            Overlay::QuickOpen(qo) => qo.clone().into_any_element(),
            Overlay::GotoLine(inp) => frame("줄로 이동 (Ctrl+G)")
                .capture_key_down(esc)
                .child(Input::new(inp))
                .child(hint(t, "Enter: 이동 · Esc: 닫기"))
                .into_any_element(),
            Overlay::Rename { input, .. } => frame("이름 바꾸기 (F2)")
                .capture_key_down(esc)
                .child(Input::new(input))
                .child(hint(t, "Enter: 미리 보기 · Esc: 취소"))
                .into_any_element(),
            Overlay::RenamePreview { new_name, edits } => {
                let total: usize = edits.iter().map(|f| f.edits.len()).sum();
                let root = self.root.clone();
                let mut list = div()
                    .id("rename-preview")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.));
                for f in edits {
                    let text = self
                        .buffers
                        .iter()
                        .find(|b| path_key(&b.path) == path_key(&f.path))
                        .map(|b| b.state.read(cx).value().to_string())
                        .or_else(|| text::load(&f.path).ok().map(|l| l.text))
                        .unwrap_or_default();
                    list = list.child(
                        div()
                            .pt(px(4.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tc(t.text_secondary))
                            .child(format!("{} · {}곳", rel_of(&root, &f.path), f.edits.len())),
                    );
                    for e in f.edits.iter().take(20) {
                        let line = text::line_of(&text, e.range.start.line).to_string();
                        let after = if e.range.start.line == e.range.end.line {
                            apply_edits(
                                &line,
                                &[lt::TextEdit {
                                    range: lt::Range::new(
                                        lt::Position::new(0, e.range.start.character),
                                        lt::Position::new(0, e.range.end.character),
                                    ),
                                    new_text: e.new_text.clone(),
                                }],
                            )
                        } else {
                            e.new_text.clone()
                        };
                        list = list.child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .text_size(px(12.))
                                .font_family("Cascadia Mono")
                                .child(
                                    div()
                                        .w(px(40.))
                                        .flex_none()
                                        .text_color(tc(t.text_muted))
                                        .child(format!("{}", e.range.start.line + 1)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(tc(t.text_primary))
                                        .child(format!("{} → {}", line.trim(), after.trim())),
                                ),
                        );
                    }
                }
                let enter =
                    cx.listener(
                        |this, e: &KeyDownEvent, w, cx| match e.keystroke.key.as_str() {
                            "enter" => {
                                cx.stop_propagation();
                                this.apply_rename(w, cx);
                            }
                            "escape" => {
                                cx.stop_propagation();
                                this.close_overlay(w, cx);
                            }
                            _ => {}
                        },
                    );
                frame(&format!(
                    "이름 바꾸기 미리 보기: `{new_name}` · 파일 {}개 · {total}곳",
                    edits.len()
                ))
                .w(px(640.))
                .capture_key_down(enter)
                .child(list)
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            div()
                                .id("rename-cancel")
                                .px(px(12.))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(tc(t.border_control))
                                .text_color(tc(t.text_primary))
                                .cursor_pointer()
                                .child("취소 (Esc)")
                                .on_click(cx.listener(|this, _, w, cx| this.close_overlay(w, cx))),
                        )
                        .child(
                            div()
                                .id("rename-apply")
                                .px(px(12.))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(tc(t.action_primary_bg))
                                .text_color(tc(t.action_primary_fg))
                                .cursor_pointer()
                                .child("적용 (Enter)")
                                .on_click(cx.listener(|this, _, w, cx| this.apply_rename(w, cx))),
                        ),
                )
                .into_any_element()
            }
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(48.))
                .child(div().child(el))
                .into_any_element(),
        )
    }
}

fn hint(t: Tokens, s: &'static str) -> impl IntoElement {
    div()
        .text_size(px(12.))
        .text_color(tc(t.text_muted))
        .child(s)
}
