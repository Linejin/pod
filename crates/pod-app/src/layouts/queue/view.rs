//! Render of the response queue: the column (header, cards, compact rows),
//! the 06 title-bar pills and activity bar, and the body around the dock.
//! Everything shown is decided by `model`; events call the column's
//! controller methods (`queue.rs`) or hand a [`UiIntent`] to the host.
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{Icon, Tone, semantic_button};

use super::model::{self, COMPACT_ROWS, Counts, Need, NeedKind, Row, Section, Variant, age};
use super::{QueueColumn, copy};
use crate::layouts::kit::{Clock, Island, LayoutActions, LayoutMetrics, UiIntent, color};
use crate::qa_marks::QaId;

/// A section's "모두 보기" toggle, an activity-bar command.
pub(super) type Activate = Box<dyn Fn(&mut Window, &mut App)>;

impl Render for QueueColumn {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let variant = Variant::current(cx);
        let model = self.model(cx);
        let keys = model.keys(self.all_working, self.all_done);
        // A decided / vanished item hands the selection to the one that took its place.
        model::follow_selection(&mut self.selected, &mut self.sel_index, &keys);
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        let mono = m.surface == pod_ui::themes::Surface::Mono;
        let rounded = matches!(
            m.surface,
            pod_ui::themes::Surface::Layered | pod_ui::themes::Surface::Islands
        );
        let now = Clock::System.now_ms();
        let list_focused = self.focus.contains_focused(window, cx);
        let me = cx.entity().downgrade();

        // ── header ──
        let new_task = {
            let actions = self.actions.clone();
            pod_ui::IconButton::new("queue-new-task", Icon::Plus, copy::NEW_TASK)
                .tooltip(copy::NEW_TASK_TIP)
                .render(mode, move |window, cx| {
                    actions.dispatch(UiIntent::NewTask, window, cx);
                })
        };
        let stat = |n: usize, label: &'static str, fg: u32| {
            div()
                .qa_id(move || format!("queue-stat-{label}"))
                .flex()
                .flex_col()
                .min_w(px(56.))
                .flex_1()
                .child(
                    div()
                        .text_size(px(m.density.ui_font * 1.7))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(color(fg))
                        .child(n.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(color(tk.text_secondary))
                        .whitespace_nowrap()
                        .child(label),
                )
        };
        let search = Input::new(&self.filter).small().prefix(pod_ui::icon_sized(
            Icon::Search,
            14.,
            tk.text_muted,
        ));
        let header =
            match variant {
                Variant::Classic => div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .px_3()
                    .pt_2()
                    .pb_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_h(px(m.density.panel_header))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(copy::TITLE),
                            )
                            .child(new_task),
                    )
                    .child(
                        div()
                            .qa_id(|| "queue-counts".into())
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(stat(model.counts.working, copy::WORKING, tk.text_primary))
                            .child(stat(model.counts.needs, copy::NEEDS, tk.warning_fg))
                            .child(stat(model.counts.done, copy::DONE, tk.text_primary))
                            .child(stat(
                                model.counts.failed,
                                copy::FAILED,
                                if model.counts.failed > 0 {
                                    tk.danger_fg
                                } else {
                                    tk.text_primary
                                },
                            )),
                    )
                    .child(search)
                    .into_any_element(),
                Variant::Mission => {
                    let toggle =
                        pod_ui::IconButton::new("queue-filter", Icon::ListFilter, copy::FILTER)
                            .render(mode, {
                                let me = me.clone();
                                move |window, cx| {
                                    let _ = me.update(cx, |q, cx| {
                                        q.filter_open = !q.filter_open;
                                        if q.filter_open {
                                            q.filter.update(cx, |f, cx| f.focus(window, cx));
                                        }
                                        cx.notify();
                                    });
                                }
                            });
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .px_3()
                        .pt_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_h(px(m.density.panel_header))
                                .child(pod_ui::icon_sized(Icon::ShieldQuestion, 16., tk.warning_fg))
                                .child(
                                    div()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .whitespace_nowrap()
                                        .child(format!("{} {}", copy::NEEDS, model.counts.needs)),
                                )
                                .child(div().flex_1())
                                .child(new_task)
                                .child(toggle),
                        )
                        .when(self.filter_open, |d| d.child(search))
                        .into_any_element()
                }
            };

        // ── list ──
        let mut children: Vec<AnyElement> = Vec::new();
        let mut index = HashMap::new();
        let section_head = |id: &'static str,
                            icon: Icon,
                            fg: u32,
                            label: &'static str,
                            n: usize,
                            toggle: Option<(bool, Activate)>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .pt_3()
                .pb_1()
                .min_h(px(m.density.row))
                .child(pod_ui::icon_sized(icon, 14., fg))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .whitespace_nowrap()
                        .child(format!("{label} {n}")),
                )
                .child(div().flex_1())
                .when_some(toggle, |d, (all, f)| {
                    let name = if all { copy::SHOW_LESS } else { copy::SHOW_ALL };
                    d.child(
                        div()
                            .id(SharedString::from(format!("{id}-all")))
                            .role(Role::Button)
                            .aria_label(SharedString::from(format!("{label} {name}")))
                            .tab_index(0)
                            .px_1()
                            .rounded(px(m.radius.control))
                            .text_xs()
                            .text_color(color(tk.accent_fg))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|s| s.bg(color(tk.bg_hover)))
                            .focus_visible(move |s| pod_ui::focus_style(s, mode))
                            .child(name)
                            .on_click(move |_, window, cx| f(window, cx)),
                    )
                })
                .into_any_element()
        };

        if variant == Variant::Classic {
            children.push(section_head(
                "queue-needs",
                Icon::ShieldQuestion,
                tk.warning_fg,
                copy::NEEDS,
                model.counts.needs,
                None,
            ));
        }
        if model.needs.is_empty() {
            let text = if self.filter.read(cx).value().trim().is_empty() {
                copy::EMPTY
            } else {
                copy::NO_MATCH
            };
            children.push(
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(color(tk.text_muted))
                    .child(text)
                    .into_any_element(),
            );
        }
        for n in &model.needs {
            index.insert(n.key.clone(), children.len());
            let selected = self.selected.as_ref() == Some(&n.key);
            children.push(self.render_card(n, selected, list_focused, variant, now, cx));
        }
        let rows = |this: &Self,
                    rows: &[Row],
                    all: bool,
                    icon: Icon,
                    fg: u32,
                    done: bool,
                    children: &mut Vec<AnyElement>,
                    index: &mut HashMap<String, usize>| {
            for r in rows
                .iter()
                .take(if all { rows.len() } else { COMPACT_ROWS })
            {
                index.insert(r.key.clone(), children.len());
                let selected = this.selected.as_ref() == Some(&r.key);
                let (key, term) = (r.key.clone(), r.terminal.clone());
                let me = me.clone();
                children.push(
                    div()
                        .id(SharedString::from(format!("queue-row-{}", r.key)))
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(r.accessible_name(done)))
                        .aria_selected(selected)
                        .mx_2()
                        .px_2()
                        .min_h(px(m.density.row))
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(m.radius.control))
                        .when(selected, |d| d.bg(color(tk.bg_selected)))
                        .when(selected && list_focused, |d| {
                            d.border_1().border_color(color(tk.focus_ring))
                        })
                        .when(!selected, |d| d.hover(|s| s.bg(color(tk.bg_hover))))
                        .cursor_pointer()
                        .child(pod_ui::icon_sized(icon, 14., fg))
                        .child(crate::agent_kind::mark_el(
                            &format!("queue-{}", r.key),
                            &r.agent,
                            14.,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .gap_1()
                                .text_sm()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .max_w(px(140.))
                                        .truncate()
                                        .child(r.label.clone()),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_color(color(tk.text_muted))
                                        .child(format!("· {}", r.task)),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(color(tk.text_muted))
                                .child(age(r.since, now)),
                        )
                        .on_click(move |_, window, cx| {
                            let _ = me.update(cx, |q, cx| q.open(&key, Some(&term), window, cx));
                        })
                        .into_any_element(),
                );
            }
        };
        let toggle_of = |which: Section| -> Activate {
            let me = me.clone();
            Box::new(move |_: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |q, cx| {
                    match which {
                        Section::Working => q.all_working = !q.all_working,
                        _ => q.all_done = !q.all_done,
                    }
                    cx.notify();
                });
            })
        };
        let wtoggle = (model.working.len() > COMPACT_ROWS)
            .then(|| (self.all_working, toggle_of(Section::Working)));
        children.push(section_head(
            "queue-working",
            Icon::LoaderCircle,
            tk.info_fg,
            copy::WORKING,
            model.working.len(),
            wtoggle,
        ));
        rows(
            self,
            &model.working,
            self.all_working,
            Icon::LoaderCircle,
            tk.info_fg,
            false,
            &mut children,
            &mut index,
        );
        let dtoggle =
            (model.done.len() > COMPACT_ROWS).then(|| (self.all_done, toggle_of(Section::Done)));
        children.push(section_head(
            "queue-done",
            Icon::CircleCheck,
            tk.success_fg,
            copy::DONE,
            model.done.len(),
            dtoggle,
        ));
        rows(
            self,
            &model.done,
            self.all_done,
            Icon::CircleCheck,
            tk.success_fg,
            true,
            &mut children,
            &mut index,
        );
        children.push(div().h(px(8.)).into_any_element());
        self.child_index = index;

        let keys_for_handler = keys.clone();
        let list = div()
            .id("queue-list")
            .qa_id(|| "queue-list".into())
            .track_focus(&self.focus)
            .role(Role::List)
            .aria_label(copy::LIST)
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .on_key_down(cx.listener(move |q, e: &KeyDownEvent, window, cx| {
                q.on_key(e, &keys_for_handler, window, cx)
            }))
            .children(children);

        div()
            .id("queue-column")
            .qa_id(|| "queue-column".into())
            .size_full()
            .flex()
            .flex_col()
            .min_h_0()
            .when(!rounded, |d| d.bg(color(tk.bg_chrome)))
            .when(!rounded && !mono, |d| {
                d.border_r(px(m.border_width))
                    .border_color(color(tk.border_subtle))
            })
            .child(header)
            .child(list)
    }
}

impl QueueColumn {
    fn render_card(
        &self,
        n: &Need,
        selected: bool,
        list_focused: bool,
        variant: Variant,
        now: u64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        let mono = m.surface == pod_ui::themes::Surface::Mono;
        let (tone_fg, _) = n.kind.tone().colors(tk);
        let mono_font = LayoutMetrics::current(cx).mono_font;
        let me = cx.entity().downgrade();
        let busy = n.gate.as_ref().and_then(|g| self.busy.get(g)).copied();
        // Buttons are tab stops only on the selected card (Tab goes list → its actions → dock).
        let show_actions = n.shows_actions(selected);
        // Pressing an action must not also "click" (open) the card under it.
        let mut actions = div()
            .flex()
            .flex_wrap()
            .gap_2()
            .items_center()
            .pt_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
        let stop = |d: Stateful<Div>| if selected { d } else { d.tab_stop(false) };
        if let Some(gate) = &n.gate {
            // A gate with options (v17) is approved by picking one; otherwise 이번만 허용.
            for b in model::gate_buttons(gate, &n.options) {
                let (me, gate) = (me.clone(), gate.clone());
                let name = b.accessible_name(&n.title);
                let tone = b.tone();
                let (d, choice) = (b.decision, b.choice);
                actions = actions.child(stop(
                    semantic_button(
                        SharedString::from(b.id),
                        b.label,
                        tone,
                        mode,
                        model::button_state(busy, d),
                        move |window, cx| {
                            let _ = me.update(cx, |q, cx| {
                                q.decide(gate.clone(), d, choice.clone(), window, cx)
                            });
                        },
                    )
                    .aria_label(SharedString::from(name)),
                ));
            }
        }
        if let Some(t) = &n.terminal {
            let (me, key, t) = (me.clone(), n.key.clone(), t.clone());
            let label = n.kind.open_label();
            actions = actions.child(stop(link_button(
                SharedString::from(format!("queue-open-{}", n.key)),
                label,
                format!("{label}: {}", n.title),
                move |window, cx| {
                    let _ = me.update(cx, |q, cx| q.open(&key, Some(&t), window, cx));
                },
            )));
        }
        if n.kind == NeedKind::Failed {
            let (me, key, since) = (me.clone(), n.key.clone(), n.since);
            actions = actions.child(stop(link_button(
                SharedString::from(format!("queue-dismiss-{}", n.key)),
                copy::DISMISS,
                format!("{}: {}", copy::DISMISS, n.title),
                move |window, cx| {
                    let _ = me.update(cx, |q, cx| q.dismiss(&key, since, window, cx));
                },
            )));
        }
        let error = n.gate.as_ref().and_then(|g| self.errors.get(g)).cloned();
        let (key, term) = (n.key.clone(), n.terminal.clone());
        let card = div()
            .id(SharedString::from(format!("queue-card-{}", n.key)))
            .qa_id({
                let k = n.key.clone();
                move || format!("queue-card-{k}")
            })
            .role(Role::ListItem)
            .aria_label(SharedString::from(n.accessible_name()))
            .aria_selected(selected)
            .relative()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .cursor_pointer()
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .child(div().pt(px(2.)).flex_shrink_0().child(pod_ui::icon_sized(
                        n.kind.icon(),
                        16.,
                        tone_fg,
                    )))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(n.title.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(color(tk.text_muted))
                            .child(age(n.since, now)),
                    ),
            )
            .child(
                div()
                    .pl(px(24.))
                    .flex()
                    .items_center()
                    .gap_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(color(tk.text_secondary))
                    .when_some(n.agent.as_ref(), |d, a| {
                        d.child(crate::agent_kind::mark_el(
                            &format!("queue-{}", n.key),
                            a,
                            14.,
                        ))
                    })
                    .child(div().flex_1().min_w_0().truncate().child(n.meta.clone())),
            )
            .when(!n.detail.is_empty(), |d| {
                let detail = div().pl(px(24.)).text_xs().text_color(color(tk.text_muted));
                // A gate message is the request itself (a command): wrap it; a task id truncates.
                d.child(if n.gate.is_some() {
                    detail.font_family(mono_font).child(n.detail.clone())
                } else {
                    detail.truncate().child(n.detail.clone())
                })
            })
            .when(show_actions, |d| {
                let k = n.key.clone();
                d.child(
                    actions
                        .pl(px(24.))
                        .qa_id(move || format!("queue-actions-{k}")),
                )
            })
            .when_some(error, |d, e| {
                d.child(
                    div()
                        .pl(px(24.))
                        .text_xs()
                        .text_color(color(tk.danger_fg))
                        .child(e),
                )
            })
            .on_click(move |_, window, cx| {
                let _ = me.update(cx, |q, cx| q.open(&key, term.as_deref(), window, cx));
            });
        let card = match variant {
            // 03: full-width rows split by hairlines; the selection is a filled row with an accent bar.
            Variant::Classic => card
                .when(!mono, |d| {
                    d.border_b(px(m.border_width))
                        .border_color(color(tk.border_subtle))
                })
                .when(selected, |d| {
                    d.bg(color(tk.bg_selected)).child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(px(3.))
                            .bg(color(if list_focused {
                                tk.focus_ring
                            } else {
                                tk.accent_fg
                            })),
                    )
                })
                .when(!selected, |d| d.hover(|s| s.bg(color(tk.bg_hover)))),
            // 06: separate cards with a status-colored left edge; selection gets a full status border.
            Variant::Mission => card
                .mx_2()
                .mb_2()
                .rounded(px(m.radius.card))
                .overflow_hidden()
                // Selection is the accent edge, not a fill: the success/danger button
                // fills must keep their contrast against the card in every style.
                .bg(color(tk.bg_raised))
                .border_1()
                .border_color(color(if selected {
                    tk.accent_fg
                } else {
                    tk.border_subtle
                }))
                .when(selected, |d| d.border_2())
                .when(!selected, |d| d.hover(|s| s.bg(color(tk.bg_hover))))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(3.))
                        .bg(color(tone_fg)),
                )
                .when(selected && list_focused, |d| {
                    d.border_color(color(tk.focus_ring))
                }),
        };
        card.into_any_element()
    }
}

/// Text action (열기, 답하기, 확인): accent text, keyboard operable.
fn link_button(
    id: SharedString,
    label: &'static str,
    name: String,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let activate = Rc::new(activate);
    let key = activate.clone();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(SharedString::from(name))
        .tab_index(0)
        .min_h(px(pod_ui::hit_area()))
        .px_2()
        .flex()
        .items_center()
        .rounded(px(m.radius.control))
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(tk.accent_fg))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(color(tk.bg_hover)))
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .child(label)
        .on_click(move |_, window, cx| activate(window, cx))
        .on_key_down(move |e, window, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                key(window, cx);
                cx.stop_propagation();
            }
        })
}

/// Jumps to a section of the queue from the title bar (focuses the list).
pub(super) type OpenSection = Rc<dyn Fn(Section, &mut Window, &mut App)>;

/// 06: status pills in the title bar; each jumps to its section.
pub(super) fn title_pills(c: Counts, open: OpenSection) -> AnyElement {
    let tk = crate::theme::tokens();
    let mode = crate::theme::mode();
    let pill = |section: Section, icon: Icon, tone: Tone, label: &'static str, n: usize| {
        let (fg, bg) = tone.colors(tk);
        let open = open.clone();
        let go = Rc::new(move |window: &mut Window, cx: &mut App| open(section, window, cx));
        let key = go.clone();
        div()
            .id(SharedString::from(format!("queue-pill-{label}")))
            .role(Role::Button)
            .aria_label(SharedString::from(copy::pill_name(label, n)))
            .tab_index(0)
            .flex_shrink_0()
            .h(px(24.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded_full()
            .bg(color(bg))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(color(fg))
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|s| s.border_1().border_color(color(fg)))
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .child(pod_ui::icon_sized(icon, 14., fg))
            .child(format!("{label} {n}"))
            .on_click(move |_, window, cx| go(window, cx))
            .on_key_down(move |e, window, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    key(window, cx);
                    cx.stop_propagation();
                }
            })
    };
    div()
        .id("queue-pills")
        .qa_id(|| "queue-pills".into())
        .role(Role::Group)
        .aria_label(copy::PILLS)
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .child(pill(
            Section::Needs,
            Icon::ShieldQuestion,
            Tone::Warning,
            copy::NEEDS,
            c.needs,
        ))
        .child(pill(
            Section::Failed,
            Icon::OctagonX,
            Tone::Danger,
            copy::FAILED,
            c.failed,
        ))
        .child(pill(
            Section::Working,
            Icon::LoaderCircle,
            Tone::Info,
            copy::WORKING,
            c.working,
        ))
        .child(pill(
            Section::Done,
            Icon::CircleCheck,
            Tone::Success,
            copy::DONE,
            c.done,
        ))
        .into_any_element()
}

/// 06: activity bar on the far left (queue toggle, agent tree, files, review,
/// gates; settings at the bottom). `review` runs the review command.
fn activity_bar(
    column: &Entity<QueueColumn>,
    actions: &LayoutActions,
    review: Activate,
    cx: &App,
) -> AnyElement {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let shown = !column.read(cx).hidden;
    let col = column.downgrade();
    let toggle =
        div()
            .rounded(px(m.radius.control))
            .when(shown, |d| d.bg(color(tk.bg_selected)))
            .child(
                pod_ui::IconButton::new("activity-queue", Icon::LayoutGrid, copy::SHOW_QUEUE)
                    .render(mode, move |window, cx| {
                        let _ = col.update(cx, |q, cx| {
                            q.hidden = !q.hidden;
                            if !q.hidden {
                                q.focus.focus(window, cx);
                            }
                            cx.notify();
                        });
                    }),
            );
    let item =
        |id: &'static str, icon: Icon, label: &'static str, tip: &'static str, f: Activate| {
            pod_ui::IconButton::new(id, icon, label)
                .tooltip(tip)
                .render(mode, move |window, cx| f(window, cx))
        };
    let intent = |intent: UiIntent| -> Activate {
        let actions = actions.clone();
        Box::new(move |window, cx| actions.dispatch(intent.clone(), window, cx))
    };
    div()
        .id("activity-bar")
        .qa_id(|| "activity-bar".into())
        .role(Role::Toolbar)
        .aria_label(copy::ACTIVITY)
        .w(px(pod_ui::hit_area() + 16.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .py_2()
        .bg(color(tk.bg_chrome))
        .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
            d.border_r(px(m.border_width))
                .border_color(color(tk.border_subtle))
        })
        .child(toggle)
        .child(item(
            "activity-agents",
            Icon::Bot,
            copy::AGENTS,
            copy::AGENTS_TIP,
            intent(UiIntent::OpenAgentTree),
        ))
        .child(item(
            "activity-files",
            Icon::FileText,
            copy::FILES,
            copy::FILES_TIP,
            intent(UiIntent::QuickOpenFiles { quick_open: false }),
        ))
        .child(item(
            "activity-review",
            Icon::GitCompareArrows,
            copy::REVIEW,
            copy::REVIEW_TIP,
            review,
        ))
        .child(item(
            "activity-gates",
            Icon::ShieldQuestion,
            copy::GATES,
            copy::GATES_TIP,
            intent(UiIntent::OpenGates),
        ))
        .child(div().flex_1())
        .child(item(
            "activity-settings",
            Icon::Settings,
            copy::SETTINGS,
            copy::SETTINGS_TIP,
            intent(UiIntent::OpenSettings),
        ))
        .into_any_element()
}

/// What the body shows around the column; the legacy adapter (`queue.rs`)
/// gathers it from the workspace.
pub(super) struct Body {
    pub column: Entity<QueueColumn>,
    pub variant: Variant,
    /// Connection warning (`LayoutFrame::warning`).
    pub warning: Option<String>,
    pub hook_banner: Option<AnyElement>,
    /// The workspace dock.
    pub dock: AnyElement,
    /// Nothing open in the dock or beside it: the empty-dock hint.
    pub empty: bool,
    pub toasts: AnyElement,
    /// Activity-bar intents (06).
    pub actions: LayoutActions,
    /// The activity bar's review command (06).
    pub review: Activate,
}

/// The window body: [activity bar] · queue column (+ toasts) · dock.
pub(super) fn body(b: Body, cx: &App) -> AnyElement {
    let m = pod_ui::themes::metrics();
    let tk = crate::theme::tokens();
    let island = Island::current();
    let gap = island.gap();
    let hidden = b.variant == Variant::Mission && b.column.read(cx).hidden;
    let queue_width = m.density.sidebar + 40.;
    let Body {
        column,
        variant,
        warning,
        hook_banner,
        dock,
        empty,
        toasts,
        actions,
        review,
    } = b;
    div()
        .id("queue-layout")
        .flex()
        .flex_1()
        .min_h_0()
        .when(variant == Variant::Mission, |d| {
            d.child(activity_bar(&column, &actions, review, cx))
        })
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
                .when(!hidden, |d| {
                    d.child(
                        div()
                            .w(px(queue_width))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(gap))
                            .child(island.wrap(div().flex_1().min_h_0()).child(column.clone())),
                    )
                })
                .child(
                    island
                        .wrap(div())
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .relative()
                        .when_some(warning, |d, w| {
                            d.child(crate::layouts::kit::warning_banner(w))
                        })
                        .when_some(hook_banner, |d, b| d.child(b))
                        .child(div().flex_1().min_h_0().child(dock))
                        .when(empty, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .inset_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .px_4()
                                    .text_sm()
                                    .text_color(rgb(tk.text_muted))
                                    .child(
                                        pod_ui::Frame::new(
                                            "queue-dock-empty",
                                            pod_ui::FrameKind::Empty,
                                            copy::DOCK_EMPTY,
                                            crate::strings::YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_,
                                        )
                                        .render(crate::theme::mode(), []),
                                    ),
                            )
                        })
                        // Toasts sit bottom-left over the dock (the queue column is full height).
                        .child(
                            div()
                                .absolute()
                                .left_2()
                                .bottom_2()
                                .w(px(queue_width.min(320.)))
                                .child(toasts),
                        ),
                ),
        )
        .into_any_element()
}
