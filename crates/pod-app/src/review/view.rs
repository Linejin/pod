//! How the Review panel looks: the toolbar, the file list, the comment column
//! (tabs, cards, composer, deliveries) and its drawer, the diff with inline
//! cards and drafts, the panel body and the dock tab title. It draws the
//! state and the [`super::model`] plans; every handler calls the controller.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::diff::{Diff, DiffAnnotation, DiffHoverHighlight, DiffLinePosition};
use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::{Disableable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use podd::protocol::{CommentState, ReviewComment};

use super::copy::{COMMENTS_DRAWER_NAME, COMMENTS_DRAWER_TIP, state_label};
use super::model::{
    delivery_rows, diff_side, draft_label, inline_comments, line_totals, location, notes_per_file,
    open_count, sidebar_plan,
};
use super::{
    AddComment, COMMENTS_MIN_W, COMMENTS_W, CloseCommentsDrawer, DRAWER_CONTEXT, KEY_CONTEXT,
    NextChange, NextFile, OpenInEditor, PrevChange, PrevFile, RefreshDiff, ReviewPanel,
    SendToAgent, ToggleComments, color, debug,
};
use crate::qa_marks::QaId;

fn state_color(s: CommentState) -> u32 {
    match s {
        CommentState::Open => color::info(),
        CommentState::Queued => color::warn(),
        CommentState::Sent => color::ok(),
        CommentState::Resolved => color::muted(),
    }
}

impl ReviewPanel {
    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (adds, dels) = line_totals(&self.files);
        // Icon tools (docs/10 §D4.6). Accessible names stay what they were: the
        // tooltip for icon-only tools, the earlier label (with its ✓ state) for toggles.
        let icon_tool = |id: &'static str, glyph: pod_ui::Icon, tip: &'static str| {
            Button::new(id)
                .icon(pod_ui::kit_icon(glyph))
                .small()
                .ghost()
                .tooltip(tip)
                .accessibility_label(tip)
        };
        let toggle = |id: &'static str,
                      glyph: pod_ui::Icon,
                      visible: &'static str,
                      name: &'static str,
                      on: bool,
                      tip: &'static str| {
            Button::new(id)
                .icon(pod_ui::kit_icon(glyph))
                .label(visible)
                .small()
                .ghost()
                .when(on, |b| gpui_kit::component::Selectable::selected(b, true))
                .tooltip(tip)
                .accessibility_label(name)
        };
        let segment = |id: &'static str, glyph: pod_ui::Icon, label: &'static str, on: bool| {
            Button::new(id)
                .icon(pod_ui::kit_icon(glyph))
                .label(label)
                .small()
                .ghost()
                .when(on, |b| gpui_kit::component::Selectable::selected(b, true))
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .min_h(px(36.))
            .py_1()
            .bg(rgb(color::bar()))
            .border_b_1()
            .border_color(rgb(color::border()))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(color::muted()))
                    .child(self.base_label.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .flex()
                    .gap_1()
                    .child(crate::tr!(FILES; ; self.files.len()))
                    .child(
                        div()
                            .text_color(rgb(color::add()))
                            .child(format!("+{adds}")),
                    )
                    .child(
                        div()
                            .text_color(rgb(color::del()))
                            .child(format!("−{dels}")),
                    ),
            )
            .when(self.loading, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(color::warn()))
                        .child(crate::strings::REFRESHING),
                )
            })
            .child(div().flex_1())
            // Segmented "나란히 / 한 줄로": neutral selected face, no filled accent.
            .child(
                pod_ui::segmented(crate::theme::mode())
                    .child(
                        segment(
                            "rv-split",
                            pod_ui::Icon::Columns2,
                            crate::strings::SPLIT,
                            self.split,
                        )
                        .tooltip(
                            crate::strings::SIDE_BY_SIDE_NEEDS_A_WIDE_PANEL_HIDE_FILES_COMMENTS,
                        )
                        .on_click(cx.listener(|p, _, _, cx| p.set_split(true, cx))),
                    )
                    .child(
                        segment(
                            "rv-unified",
                            pod_ui::Icon::Rows2,
                            crate::strings::INLINE,
                            !self.split,
                        )
                        .on_click(cx.listener(|p, _, _, cx| p.set_split(false, cx))),
                    ),
            )
            .child(
                Button::new("rv-collapse")
                    .icon(pod_ui::kit_icon(pod_ui::Icon::ChevronsUpDown))
                    .label(if self.collapsed {
                        crate::strings::EXPAND
                    } else {
                        crate::strings::COLLAPSE
                    })
                    .small()
                    .ghost()
                    .tooltip(crate::strings::SHOW_OR_FOLD_UNCHANGED_LINES)
                    .on_click(cx.listener(|p, _, _, cx| p.toggle_collapse(cx))),
            )
            .child(
                toggle(
                    "rv-wrap",
                    pod_ui::Icon::WrapText,
                    crate::strings::WRAP_169,
                    if self.wrap {
                        crate::strings::WRAP
                    } else {
                        crate::strings::WRAP_169
                    },
                    self.wrap,
                    crate::strings::SOFT_WRAP_LONG_LINES,
                )
                .on_click(cx.listener(|p, _, _, cx| p.toggle_wrap(cx))),
            )
            .child(
                icon_tool(
                    "rv-prev",
                    pod_ui::Icon::ArrowUp,
                    crate::strings::PREVIOUS_CHANGE_SHIFT_F7_ALT,
                )
                .on_click(cx.listener(|p, _, _, cx| p.previous_change(cx))),
            )
            .child(
                icon_tool(
                    "rv-next",
                    pod_ui::Icon::ArrowDown,
                    crate::strings::NEXT_CHANGE_F7_ALT,
                )
                .on_click(cx.listener(|p, _, _, cx| p.next_change(cx))),
            )
            .child(
                icon_tool(
                    "rv-pfile",
                    pod_ui::Icon::ChevronLeft,
                    crate::strings::PREVIOUS_FILE_ALT_PGUP,
                )
                .on_click(cx.listener(|p, _, _, cx| p.step_file(false, cx))),
            )
            .child(
                icon_tool(
                    "rv-nfile",
                    pod_ui::Icon::ChevronRight,
                    crate::strings::NEXT_FILE_ALT_PGDN,
                )
                .on_click(cx.listener(|p, _, _, cx| p.step_file(true, cx))),
            )
            .child(
                toggle(
                    "rv-files",
                    pod_ui::Icon::PanelLeft,
                    crate::strings::FILES_176,
                    if self.show_files {
                        crate::strings::FILES_175
                    } else {
                        crate::strings::FILES_176
                    },
                    self.show_files,
                    crate::strings::SHOW_OR_HIDE_THE_FILE_LIST,
                )
                .on_click(cx.listener(|p, _, _, cx| p.toggle_file_list(cx))),
            )
            .child({
                let (on, tip) = if self.comments_drawer {
                    (self.drawer_open, COMMENTS_DRAWER_TIP)
                } else {
                    (
                        self.show_comments,
                        crate::strings::SHOW_OR_HIDE_THE_COMMENTS_PANE,
                    )
                };
                toggle(
                    "rv-notes",
                    pod_ui::Icon::MessageSquare,
                    crate::strings::COMMENTS_179,
                    if on {
                        crate::strings::COMMENTS
                    } else {
                        crate::strings::COMMENTS_179
                    },
                    on,
                    tip,
                )
                .on_click(cx.listener(|p, _, window, cx| p.toggle_comments(window, cx)))
            })
            .child(
                icon_tool(
                    "rv-refresh",
                    pod_ui::Icon::RefreshCw,
                    crate::strings::RE_READ_CHANGES_COMMENTS_AND_DELIVERIES_F5,
                )
                .on_click(cx.listener(|p, _, _, cx| p.refresh_all(cx))),
            )
    }

    fn render_file_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let files = self.files.clone();
        let per_file = Rc::new(notes_per_file(&self.comments));
        let current = self.current_file;
        div()
            .w(px(250.))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(color::pane()))
            .border_r_1()
            .border_color(rgb(color::border()))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(color::muted()))
                    .child(crate::tr!(FILES_182; ; files.len())),
            )
            .child(
                uniform_list(
                    "rv-files",
                    files.len(),
                    cx.processor(move |this, range: Range<usize>, _window, cx| {
                        range
                            .map(|ix| {
                                let f = &this.files[ix];
                                let (letter, mark) = f.status_mark();
                                let notes = per_file.get(f.path.as_ref()).copied().unwrap_or(0);
                                div()
                                    .id(("rv-file", ix))
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .px_3()
                                    .h(px(24.))
                                    .text_xs()
                                    .cursor_pointer()
                                    .when(ix == current, |d| d.bg(rgb(color::selected())))
                                    .hover(|s| s.bg(rgb(color::hover())))
                                    .on_click(cx.listener(move |p, e: &ClickEvent, window, cx| {
                                        if e.click_count() > 1 {
                                            let path = p.files[ix].path.to_string();
                                            p.open_in_editor(path, None, window, cx);
                                        } else {
                                            p.go_file(ix, cx);
                                        }
                                    }))
                                    .context_menu({
                                        let (weak, path) =
                                            (cx.entity().downgrade(), f.path.to_string());
                                        move |menu, _, _| {
                                            let (weak, path) = (weak.clone(), path.clone());
                                            menu.item(
                                                PopupMenuItem::new(crate::strings::OPEN_IN_EDITOR)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = weak.update(cx, |p, cx| {
                                                            p.open_in_editor(
                                                                path.clone(),
                                                                None,
                                                                window,
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                            )
                                        }
                                    })
                                    .child(div().flex_none().text_color(rgb(mark)).child(letter))
                                    .child({
                                        // Ellipsized path; the whole path is the tooltip (no mid-word wrap).
                                        let tip = f.path.clone();
                                        div()
                                            .id(("rv-file-path", ix))
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .tooltip(move |window, cx| {
                                                gpui_kit::component::tooltip::Tooltip::new(
                                                    tip.clone(),
                                                )
                                                .build(window, cx)
                                            })
                                            .child(f.path.clone())
                                    })
                                    .when(notes > 0, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::info()))
                                                .child(format!("💬{notes}")),
                                        )
                                    })
                                    .when(f.eol_only, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::muted()))
                                                .child(crate::strings::EOL_ONLY),
                                        )
                                    })
                                    .when(f.binary, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::muted()))
                                                .child(crate::strings::BINARY),
                                        )
                                    })
                                    .when(!f.binary, |d| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::add()))
                                                .child(format!("+{}", f.additions)),
                                        )
                                        .child(
                                            div()
                                                .flex_none()
                                                .text_color(rgb(color::del()))
                                                .child(format!("−{}", f.deletions)),
                                        )
                                    })
                            })
                            .collect()
                    }),
                )
                .flex_1(),
            )
    }

    /// "코멘트 | PR" switch at the top of the sidebar (P6).
    fn render_sidebar_tabs(&self, cx: &mut Context<Self>) -> AnyElement {
        let tab = |id: &'static str,
                   label: &'static str,
                   tip: &'static str,
                   on: bool,
                   pr: bool,
                   cx: &mut Context<Self>| {
            Button::new(id)
                .label(label)
                .xsmall()
                .tooltip(tip)
                .when(on, |b| b.primary())
                .when(!on, |b| b.ghost())
                .on_click(cx.listener(move |p, _, _, cx| p.set_pr_tab(pr, cx)))
        };
        let pr_label = self
            .pr
            .read(cx)
            .info()
            .and_then(|i| i.status.pr.as_ref())
            .map(|p| format!("#{}", p.number));
        div()
            .qa_id(|| "review-sidebar-tabs".into())
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .pt_1()
            .child(tab(
                "rv-tab-comments",
                crate::pr_panel::s::TAB_COMMENTS,
                crate::pr_panel::s::TAB_COMMENTS_TIP,
                !self.pr_tab,
                false,
                cx,
            ))
            .child(tab(
                "rv-tab-pr",
                crate::pr_panel::s::TAB_PR,
                crate::pr_panel::s::TAB_PR_TIP,
                self.pr_tab,
                true,
                cx,
            ))
            .when_some(pr_label, |d, l| {
                d.child(div().text_xs().text_color(rgb(color::muted())).child(l))
            })
            .into_any_element()
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let tabs = self.render_sidebar_tabs(cx);
        if self.pr_tab {
            return div()
                .w(px(COMMENTS_W))
                .min_w(px(COMMENTS_MIN_W))
                .flex_shrink(1.)
                .h_full()
                .flex()
                .flex_col()
                .bg(rgb(color::pane()))
                .border_l_1()
                .border_color(rgb(color::border()))
                .child(tabs)
                .child(div().flex_1().min_h_0().child(self.pr.clone()))
                .into_any_element();
        }
        let plan = sidebar_plan(
            &self.comments,
            &self.checked,
            self.show_resolved,
            self.sending,
            self.agent_status.as_ref(),
            self.agent_terminal.as_deref(),
            self.outcome.as_ref(),
        );
        let weak = cx.entity().downgrade();
        let mut list = div()
            .id("rv-comments")
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        if plan.visible.is_empty() {
            list = list.child(
                words(crate::strings::NO_COMMENTS_YET_CLICK_A_LINE_OR_DRAG_LINE_NUMBERS_THEN_TO_COMMENT, "rv-hint").text_xs().text_color(rgb(color::muted())),
            );
        }
        for c in &plan.visible {
            let c2 = c.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("rv-card-{}", c.id)))
                    .cursor_pointer()
                    .on_click(cx.listener(move |p, _, _, cx| p.scroll_to_comment(&c2, cx)))
                    .child(comment_card(c, true, self.checked.contains(&c.id), &weak)),
            );
        }
        let deliveries = delivery_rows(&self.deliveries, &self.submits)
            .into_iter()
            .map(|row| {
                let (id, term) = (row.id.clone(), row.terminal.clone());
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(words(&row.text, "rv-delivery").text_color(rgb(row.color)))
                            .when_some(row.submit, |c, (t, sc)| {
                                c.child(words(&t, "rv-submit").text_color(rgb(sc)))
                            }),
                    )
                    .when(row.unconfirmed, |r| {
                        r.child(
                            Button::new(SharedString::from(format!("rv-enter-{}", row.id)))
                                .label(crate::strings::PRESS_ENTER)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(move |p, _, _, cx| p.press_enter(&term, cx))),
                        )
                    })
                    .when(row.queued, |r| {
                        r.child(
                            Button::new(SharedString::from(format!("rv-cancel-{}", row.id)))
                                .label(crate::strings::CANCEL)
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    p.cancel_delivery(id.clone(), cx)
                                })),
                        )
                    })
            });
        let outcome = plan.refusal.map(|t| (t, color::del()));
        div()
            .w(px(COMMENTS_W))
            .min_w(px(COMMENTS_MIN_W))
            .flex_shrink(1.)
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(color::pane()))
            .border_l_1()
            .border_color(rgb(color::border()))
            .child(tabs)
            .child(
                div()
                    .flex()
                    .items_center()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(color::muted()))
                    .child(words(&crate::tr!(COMMENTS_OPEN_TOTAL; ; plan.open, plan.total), "rv-count").flex_1())
                    .when(plan.resolved > 0, |d| {
                        let resolved = plan.resolved;
                        d.child(
                            Button::new("rv-show-resolved")
                                .label(if self.show_resolved {
                                    crate::tr!(HIDE_RESOLVED_RESOLVED; resolved;)
                                } else {
                                    crate::tr!(SHOW_RESOLVED_RESOLVED; resolved;)
                                })
                                .xsmall()
                                .ghost()
                                .on_click(cx.listener(|p, _, _, cx| p.toggle_show_resolved(cx))),
                        )
                    }),
            )
            .child(list)
            .child(
                div()
                    .qa_id(|| "review-composer".into())
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_2()
                    .border_t_1()
                    .border_color(rgb(color::border()))
                    .child(words(crate::strings::SEND_TO_AGENT_194, "rv-send-title").text_xs().text_color(rgb(color::muted())))
                    .child(Textarea::new(&self.instructions))
                    .child(
                        Button::new("rv-send")
                            .label(if self.sending { crate::strings::SENDING.to_string() } else { plan.send_label })
                            .primary()
                            .small()
                            .disabled(!plan.can_send)
                            .tooltip(crate::strings::REVIEW_SEND_CTRL_SHIFT_ENTER_CHECKED_COMMENTS_ELSE_ALL_OPEN_ONES)
                            .on_click(cx.listener(|p, _, _, cx| p.send(cx))),
                    )
                    .child(words(&plan.agent, "rv-agent").text_xs().text_color(rgb(color::muted())))
                    .when_some(outcome, |d, (t, col)| d.child(words(&t, "rv-outcome").text_xs().text_color(rgb(col))))
                    .children(deliveries),
            )
            .into_any_element()
    }

    /// The comment column as a drawer over the diff (D4.6, < 560 px): a dialog with
    /// its own focus and key context, so Esc closes it and focus returns to the panel.
    fn render_drawer(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("rv-comments-drawer")
            .qa_id(|| "review-comments-drawer".into())
            .track_focus(&self.drawer_focus)
            .key_context(DRAWER_CONTEXT)
            .role(Role::Dialog)
            .aria_label(COMMENTS_DRAWER_NAME)
            .on_action(cx.listener(|p, _: &CloseCommentsDrawer, window, cx| {
                p.close_comments_drawer(window, cx)
            }))
            .occlude()
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .w(px(COMMENTS_W))
            .max_w_full()
            .flex()
            .shadow_lg()
            .child(self.render_sidebar(cx))
            .into_any_element()
    }

    fn render_diff(&self, cx: &mut Context<Self>) -> Diff {
        let mut views: HashMap<ElementId, AnnView> = HashMap::new();
        let mut annotations = Vec::new();
        for (c, line) in inline_comments(&self.comments, &self.files) {
            let id = ElementId::from(SharedString::from(format!("rv-ann-{}", c.id)));
            annotations.push(DiffAnnotation::line(
                id.clone(),
                DiffLinePosition::new(c.file.clone(), diff_side(c.side), line),
            ));
            views.insert(id, AnnView::Comment(c.clone()));
        }
        if let Some(d) = &self.draft {
            let id = ElementId::from(SharedString::from("rv-draft"));
            annotations.push(DiffAnnotation::line(
                id.clone(),
                DiffLinePosition::new(d.path.clone(), diff_side(d.side), d.end_line as usize),
            ));
            let label = draft_label(d.side, d.line, d.end_line);
            views.insert(
                id,
                AnnView::Draft {
                    input: d.input.clone(),
                    title: crate::tr!(NEW_COMMENT_ON_LABEL; label; d.path).into(),
                    error: d
                        .error
                        .as_ref()
                        .map(|e| format!("{}: {e}", crate::strings::COMMENT_NOT_SAVED)),
                },
            );
        }
        let views = Rc::new(views);
        let weak = cx.entity().downgrade();
        let (weak_ann, weak_click, weak_add, weak_hover, weak_open) =
            (weak.clone(), weak.clone(), weak.clone(), weak.clone(), weak);
        Diff::new(&self.state)
            .size_full()
            .soft_wrap(self.wrap)
            .hover_highlight(DiffHoverHighlight::Both)
            .annotations(annotations)
            .render_annotation(move |ann, _window, _cx| match views.get(ann.id()) {
                Some(AnnView::Comment(c)) => div()
                    .mx_2()
                    .my_1()
                    .child(comment_card(c, false, false, &weak_ann))
                    .into_any_element(),
                Some(AnnView::Draft {
                    input,
                    title,
                    error,
                }) => {
                    let (w_save, w_cancel) = (weak_ann.clone(), weak_ann.clone());
                    div()
                        .mx_2()
                        .my_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(rgb(color::accent()))
                        .bg(rgb(color::draft_bg()))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(rgb(color::muted()))
                                .child(title.clone()),
                        )
                        .child(Textarea::new(input))
                        .when_some(error.as_deref(), |d, e| {
                            d.child(
                                words(e, "rv-draft-error")
                                    .text_xs()
                                    .text_color(rgb(color::del())),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(
                                    Button::new("rv-draft-save")
                                        .label(crate::strings::COMMENTS_179)
                                        .small()
                                        .primary()
                                        .on_click(move |_, window, cx| {
                                            let _ =
                                                w_save.update(cx, |p, cx| p.save_draft(window, cx));
                                        }),
                                )
                                .child(
                                    Button::new("rv-draft-cancel")
                                        .label(crate::strings::CANCEL)
                                        .small()
                                        .ghost()
                                        .on_click(move |_, _, cx| {
                                            let _ = w_cancel.update(cx, |p, cx| p.cancel_draft(cx));
                                        }),
                                ),
                        )
                        .into_any_element()
                }
                None => div().into_any_element(),
            })
            .on_line_click(move |pos, _ev, window, cx| {
                let (path, side, line) = (pos.path().to_string(), pos.side(), pos.line());
                let _ =
                    weak_click.update(cx, |p, cx| p.open_draft(path, side, line, line, window, cx));
            })
            .on_add_annotation(move |range, window, cx| {
                let (path, side, start, end) = (
                    range.path().to_string(),
                    range.side(),
                    range.start(),
                    range.end(),
                );
                let _ =
                    weak_add.update(cx, |p, cx| p.open_draft(path, side, start, end, window, cx));
            })
            .on_line_hover(move |pos, _window, cx| {
                let pos = pos.cloned();
                let _ = weak_hover.update(cx, |p, _| p.set_hovered(pos));
            })
            .render_header_prefix(move |file, _window, _cx| {
                let (weak, path) = (weak_open.clone(), file.path().to_string());
                Button::new(SharedString::from(format!("rv-open-{path}")))
                    .label(crate::strings::OPEN_IN_EDITOR_SHORT)
                    .accessibility_label(format!("{} {path}", crate::strings::OPEN_IN_EDITOR))
                    .tooltip(crate::strings::OPEN_IN_EDITOR_TIP)
                    .xsmall()
                    .ghost()
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let _ = weak
                            .update(cx, |p, cx| p.open_in_editor(path.clone(), None, window, cx));
                    })
            })
    }
}

/// One comment card (sidebar and inline). Handlers go through a weak handle
/// because inline cards are built inside the diff's annotation renderer.
fn comment_card(
    c: &ReviewComment,
    in_sidebar: bool,
    checked: bool,
    weak: &WeakEntity<ReviewPanel>,
) -> Div {
    let key = |s: &str| {
        SharedString::from(format!(
            "{s}-{}-{}",
            if in_sidebar { "s" } else { "i" },
            c.id
        ))
    };
    let resolved = c.state == CommentState::Resolved;
    let (id_res, id_rm, id_chk) = (c.id.clone(), c.id.clone(), c.id.clone());
    let (w_res, w_rm, w_chk) = (weak.clone(), weak.clone(), weak.clone());
    let header = div()
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .text_color(rgb(color::muted()))
        .when(in_sidebar && !resolved, |d| {
            d.child(
                div()
                    .id(key("chk"))
                    .cursor_pointer()
                    .px_1()
                    .child(if checked { "☑" } else { "☐" })
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        let _ = w_chk.update(cx, |p, cx| p.toggle_checked(&id_chk, cx));
                    }),
            )
        })
        .child({
            let place: SharedString = if in_sidebar {
                format!("{} · {}", c.id, location(c))
            } else {
                format!("💬 {} · {}", c.id, location(c))
            }
            .into();
            let tip = place.clone();
            div()
                .id(key("place"))
                .flex_1()
                .min_w_0()
                .truncate()
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .child(place)
        })
        .child(
            div()
                .text_color(rgb(state_color(c.state)))
                .child(state_label(c.state)),
        );
    let actions = div()
        .flex()
        .gap_1()
        .child(
            Button::new(key("res"))
                .label(if resolved {
                    crate::strings::REOPEN
                } else {
                    crate::strings::RESOLVE
                })
                .xsmall()
                .ghost()
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = w_res.update(cx, |p, cx| p.resolve(id_res.clone(), resolved, cx));
                }),
        )
        .child(
            Button::new(key("rm"))
                .label(crate::strings::DELETE)
                .xsmall()
                .ghost()
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    let _ = w_rm.update(cx, |p, cx| p.remove(id_rm.clone(), cx));
                }),
        );
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(if checked {
            color::accent()
        } else {
            color::border()
        }))
        .bg(rgb(if in_sidebar {
            color::card()
        } else {
            color::draft_bg()
        }))
        .when(resolved, |d| d.opacity(0.6))
        .child(header)
        .child(
            div()
                .flex()
                .flex_col()
                .text_sm()
                .children(c.body.lines().map(|l| words(l, "rv-body").min_h(px(18.)))),
        )
        .when_some(c.note.clone(), |d, n| {
            d.child(
                words(&crate::tr!(NOTE_N; n;), "rv-note")
                    .text_xs()
                    .text_color(rgb(color::muted())),
            )
        })
        .child(actions)
}

/// Prose that wraps only between words (CSS `word-break: keep-all`, docs/10 §D4.6):
/// GPUI breaks Korean between any two syllables, so each word is its own flex piece
/// (pod_ui::keep_all_words), left-aligned. A piece is laid out at its full width, so a
/// word that fits the line is never split; only a word wider than the whole line (a long
/// path, URL or identifier) is capped at the line and wraps inside itself (break-all),
/// so no text is ever hidden (ui-editor-05). Tests find piece `i` as `{qa}-w{i}`.
pub(crate) fn words(text: &str, qa: &'static str) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .gap_x(px(4.))
        .children(
            pod_ui::keep_all_words(text)
                .into_iter()
                .enumerate()
                .map(|(i, w)| {
                    div()
                        .max_w_full()
                        .min_w_0()
                        .qa_id(move || format!("{qa}-w{i}"))
                        .child(SharedString::from(w.to_string()))
                }),
        )
}

/// What an inline annotation shows.
#[derive(Clone)]
enum AnnView {
    Comment(ReviewComment),
    Draft {
        input: Entity<TextareaState>,
        title: SharedString,
        /// The last failed save, kept with the text for a retry.
        error: Option<String>,
    },
}

impl gpui_kit::component::dock::Panel for ReviewPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(crate::tr!(REVIEW; ; self.task_id).into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let open = open_count(&self.comments);
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(crate::tr!(REVIEW; ; self.task_id))
            .when(open > 0, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(rgb(color::info()))
                        .child(format!("💬{open}")),
                )
            })
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for ReviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        debug(format_args!(
            "render: {} files, draft {}",
            self.files.len(),
            self.draft.is_some()
        ));
        self.take_pending(window, cx);
        let center = if let Some(e) = self.load_error.clone().filter(|_| self.files.is_empty()) {
            // Inline error banner (docs/10 §D4.6) with a retry, instead of raw red text.
            div()
                .size_full()
                .p_4()
                .text_sm()
                .child(
                    pod_ui::banner(
                        pod_ui::Tone::Danger,
                        crate::theme::mode(),
                        crate::tr!(COULD_NOT_LOAD_THE_DIFF_E; e;),
                        None,
                    )
                    .child(
                        div().flex_shrink_0().child(
                            Button::new("rv-retry")
                                .icon(pod_ui::kit_icon(pod_ui::Icon::RotateCcw))
                                .label(crate::strings::RETRY)
                                .small()
                                .ghost()
                                .on_click(cx.listener(|p, _, _, cx| p.refresh_all(cx))),
                        ),
                    ),
                )
                .into_any_element()
        } else if !self.loaded_once {
            crate::chrome::loading_placeholder(crate::strings::LOADING_CHANGES)
                .size_full()
                .p_4()
                .into_any_element()
        } else if self.files.is_empty() {
            div()
                .size_full()
                .p_4()
                .text_sm()
                .text_color(rgb(color::muted()))
                .child(words(
                    crate::strings::NO_CHANGES_AGAINST_THE_TASK_BASE_YET,
                    "rv-no-changes",
                ))
                .into_any_element()
        } else {
            self.render_diff(cx).into_any_element()
        };
        div()
            .id("review-panel")
            .track_focus(&self.focus)
            .key_context(KEY_CONTEXT)
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(color::bg()))
            .text_color(rgb(crate::theme::tokens().text_primary))
            .on_action(cx.listener(|p, _: &NextChange, _, cx| p.next_change(cx)))
            .on_action(cx.listener(|p, _: &PrevChange, _, cx| p.previous_change(cx)))
            .on_action(cx.listener(|p, _: &NextFile, _, cx| p.step_file(true, cx)))
            .on_action(cx.listener(|p, _: &PrevFile, _, cx| p.step_file(false, cx)))
            .on_action(cx.listener(|p, _: &AddComment, window, cx| p.add_comment_here(window, cx)))
            .on_action(cx.listener(|p, _: &SendToAgent, _, cx| p.send(cx)))
            .on_action(cx.listener(|p, _: &RefreshDiff, _, cx| p.refresh_all(cx)))
            .on_action(
                cx.listener(|p, _: &OpenInEditor, window, cx| p.open_hovered_in_editor(window, cx)),
            )
            .on_action(
                cx.listener(|p, _: &ToggleComments, window, cx| p.toggle_comments(window, cx)),
            )
            .child(self.render_toolbar(cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .id("review-columns")
                            .qa_id(|| "review-columns".into())
                            .size_full()
                            .flex()
                            .overflow_x_scroll()
                            .when(self.show_files && !self.files.is_empty(), |d| {
                                d.child(self.render_file_list(cx))
                            })
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w(px(320.))
                                    .h_full()
                                    .child(center),
                            )
                            .when(self.show_comments && !self.comments_drawer, |d| {
                                d.child(
                                    div()
                                        .qa_id(|| "review-comments".into())
                                        .h_full()
                                        .flex()
                                        .min_w(px(COMMENTS_MIN_W))
                                        .flex_shrink(1.)
                                        .child(self.render_sidebar(cx)),
                                )
                            }),
                    )
                    .when(self.comments_drawer_open(), |d| {
                        d.child(self.render_drawer(cx))
                    }),
            )
    }
}
