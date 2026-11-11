//! Rendering of the file tree.

use std::path::Path;

use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{Icon, Tokens, icon, tokens::color as tc};

use crate::git::GitMark;
use crate::ui::strings as txt;
use crate::uri::path_key;

use super::{EditorWorkspace, Overlay, TREE_CTX, TREE_ROW_H, TREE_W};

impl EditorWorkspace {
    pub(super) fn render_tree(
        &mut self,
        t: Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.tree_focus.contains_focused(window, cx);
        let mode = self.mode;
        let root_name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let count = self.tree.rows().len();
        div()
            .id("pod-editor-tree")
            .track_focus(&self.tree_focus)
            .key_context(TREE_CTX)
            .on_action(cx.listener(Self::on_tree_up))
            .on_action(cx.listener(Self::on_tree_down))
            .on_action(cx.listener(Self::on_tree_left))
            .on_action(cx.listener(Self::on_tree_right))
            .on_action(cx.listener(Self::on_tree_open))
            .on_action(cx.listener(Self::on_tree_preview))
            .on_action(cx.listener(Self::on_tree_home))
            .on_action(cx.listener(Self::on_tree_end))
            .on_action(cx.listener(Self::on_tree_new_file))
            .on_action(cx.listener(Self::on_tree_rename))
            .on_action(cx.listener(Self::on_tree_delete))
            .tab_index(0)
            .role(Role::Tree)
            .aria_label(txt::TREE_LABEL)
            .focus_visible(move |st| pod_ui::focus_style(st, mode))
            .w(px(TREE_W))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(tc(t.bg_chrome))
            .border_r_1()
            .border_color(tc(t.border_subtle))
            .child(
                div()
                    .h(px(32.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tc(t.text_primary))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(root_name),
                    )
                    .child(div().text_size(px(12.)).text_color(tc(t.text_muted)).child(
                        if self.git_changes > 0 {
                            format!("변경 {}", self.git_changes)
                        } else {
                            String::new()
                        },
                    )),
            )
            .child(
                uniform_list(
                    "pod-editor-tree-rows",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        let t = Tokens::for_mode(this.mode);
                        let sel = this.tree.selected_index();
                        let active_key = this.active_path().map(path_key);
                        range
                            .map(|ix| {
                                let r = &this.tree.rows()[ix];
                                let is_sel = sel == Some(ix);
                                let is_active =
                                    active_key.as_deref() == Some(path_key(&r.path).as_str());
                                let mark_color = r.mark.map(|m| mark_color(m, t));
                                let (row_path, row_dir) = (r.path.clone(), r.is_dir);
                                let ws = cx.entity().downgrade();
                                let row_label = match r.mark {
                                    Some(m) if !r.is_dir => format!("{} ({})", r.name, m.letter()),
                                    _ => r.name.clone(),
                                };
                                let name_color = mark_color.unwrap_or(t.text_primary);
                                let chevron = if r.is_dir {
                                    let mut s =
                                        icon(Icon::ChevronRight, t.text_secondary).size(px(14.));
                                    if r.expanded {
                                        s = s.with_transformation(Transformation::rotate(radians(
                                            std::f32::consts::FRAC_PI_2,
                                        )));
                                    }
                                    div()
                                        .w(px(16.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(s)
                                        .into_any_element()
                                } else {
                                    div().w(px(16.)).flex_none().into_any_element()
                                };
                                div()
                                    .id(("tree-row", ix))
                                    .role(Role::TreeItem)
                                    .aria_label(row_label)
                                    .aria_selected(is_sel)
                                    .w_full()
                                    .h(px(TREE_ROW_H))
                                    .pl(px(8. + r.depth as f32 * pod_ui::tokens::TREE_INDENT))
                                    .pr(px(8.))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .cursor_pointer()
                                    .text_size(px(13.))
                                    .when(is_sel, |d| d.bg(tc(t.bg_selected)))
                                    .when(is_sel && focused, |d| {
                                        d.border_1().border_color(tc(t.accent_fg))
                                    })
                                    .when(!is_sel, |d| d.hover(|d| d.bg(tc(t.bg_hover))))
                                    .on_click(cx.listener(
                                        move |this, e: &ClickEvent, window, cx| {
                                            this.tree_focus.focus(window, cx);
                                            let double = e.click_count() > 1;
                                            this.tree_activate(ix, double, !double, window, cx);
                                        },
                                    ))
                                    .context_menu(move |menu, _, _| {
                                        let dir = if row_dir {
                                            row_path.clone()
                                        } else {
                                            row_path
                                                .parent()
                                                .map(Path::to_path_buf)
                                                .unwrap_or_else(|| row_path.clone())
                                        };
                                        let (w1, w2, w3, w4) =
                                            (ws.clone(), ws.clone(), ws.clone(), ws.clone());
                                        let (d1, d2, p3, p4) =
                                            (dir.clone(), dir, row_path.clone(), row_path.clone());
                                        menu.item(PopupMenuItem::new(txt::NEW_FILE).on_click(
                                            move |_, window, cx| {
                                                let _ = w1.update(cx, |w, cx| {
                                                    w.start_new_entry(d1.clone(), false, window, cx)
                                                });
                                            },
                                        ))
                                        .item(PopupMenuItem::new(txt::NEW_FOLDER).on_click(
                                            move |_, window, cx| {
                                                let _ = w2.update(cx, |w, cx| {
                                                    w.start_new_entry(d2.clone(), true, window, cx)
                                                });
                                            },
                                        ))
                                        .separator()
                                        .item(PopupMenuItem::new(txt::RENAME).on_click(
                                            move |_, window, cx| {
                                                let _ = w3.update(cx, |w, cx| {
                                                    w.start_rename_entry(p3.clone(), window, cx)
                                                });
                                            },
                                        ))
                                        .item(
                                            PopupMenuItem::new(txt::DELETE).on_click(
                                                move |_, window, cx| {
                                                    let _ = w4.update(cx, |w, cx| {
                                                        w.overlay =
                                                            Overlay::ConfirmDelete(p4.clone());
                                                        w.prompt_focus.focus(window, cx);
                                                        cx.notify();
                                                    });
                                                },
                                            ),
                                        )
                                    })
                                    .child(chevron)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_color(tc(name_color))
                                            .when(is_active, |d| {
                                                d.font_weight(FontWeight::SEMIBOLD)
                                            })
                                            .child(r.name.clone()),
                                    )
                                    .when_some(r.mark, |d, m| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .w(px(14.))
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(tc(mark_color.unwrap_or(t.text_muted)))
                                                .child(if r.is_dir { "•" } else { m.letter() }),
                                        )
                                    })
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.tree_scroll)
                .flex_1(),
            )
            .into_any_element()
    }
}

fn mark_color(m: GitMark, t: Tokens) -> u32 {
    match m {
        GitMark::Modified => t.warning_fg,
        GitMark::Added | GitMark::Untracked => t.success_fg,
        GitMark::Deleted | GitMark::Conflicted => t.danger_fg,
        GitMark::Renamed => t.info_fg,
    }
}
