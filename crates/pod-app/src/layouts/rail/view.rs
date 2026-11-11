//! Render of the 집중 레일: the shell around it, the 44px rail, the task tab
//! strip, the rail dock (or its empty state) and the drawers. What is shown
//! comes from `model`; events call the rail's controller methods (`rail.rs`)
//! or hand a [`UiIntent`] to the host.
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::model::{Badge, Drawer, KeyStep, RowMark, fit_tabs, key_step, strip_room, tab_width};
use super::{
    AttentionDrawer, CONTEXT, NextTask, PrevTask, RAIL_WIDTH, RailButton, RailState, TAB_TEXT_MAX,
    TaskSwitcher, WorkspaceDrawer, copy,
};
use crate::layouts::kit::{BodyFlow, ConnectionShell, Island, ToastAnchor, UiIntent};
use crate::qa_marks::QaId;

/// The rail's shell: the connection warning and the hook prompt above the
/// rail, toasts bottom-left beside the rail (no canvas of its own).
pub(super) fn shell(
    warning: Option<String>,
    hook_banner: Option<AnyElement>,
    rail: AnyElement,
    toasts: AnyElement,
) -> AnyElement {
    ConnectionShell {
        warning,
        hook_banner,
        canvas: None,
        flow: BodyFlow::Row,
        body: rail,
        toasts,
        anchor: ToastAnchor::BottomLeft {
            left: RAIL_WIDTH + 8.,
            bottom: 8.,
            width: 320.,
        },
    }
    .render()
    .into_any_element()
}

impl Render for RailState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_focus(window, cx);
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        use pod_ui::themes::Surface;
        let island = Island::current();
        let rounded = island.rounded();
        let gap = island.gap();
        let lines = !matches!(m.surface, Surface::Mono);
        let border = m.border_width.max(1.);

        // Rail.
        let attention_n = self.attention.len();
        let rail_button = |this: &Self,
                           b: RailButton,
                           icon: pod_ui::Icon,
                           label: String,
                           tip: &'static str,
                           selected: bool,
                           expanded: Option<bool>,
                           badge: Option<(usize, u32, u32)>,
                           cx: &mut Context<Self>| {
            let focus = this.buttons[b as usize].clone();
            div()
                .id(SharedString::from(format!("rail-{b:?}").to_lowercase()))
                .track_focus(&focus)
                .role(Role::Button)
                .aria_label(label)
                .when_some(expanded, |d, e| d.aria_expanded(e))
                .size(px(36.))
                .flex_shrink_0()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(m.radius.control))
                .cursor_pointer()
                .when(selected, |d| d.bg(rgb(tk.bg_selected)))
                .when(!selected, |d| d.hover(|s| s.bg(rgb(tk.bg_hover))))
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
                })
                .when(selected, |d| {
                    d.child(
                        div()
                            .absolute()
                            .left(px(-4.))
                            .top(px(8.))
                            .bottom(px(8.))
                            .w(px(2.))
                            .rounded(px(1.))
                            .bg(rgb(tk.accent_fg)),
                    )
                })
                .child(pod_ui::icon_sized(
                    icon,
                    18.,
                    if selected {
                        tk.text_primary
                    } else {
                        tk.text_secondary
                    },
                ))
                .when_some(badge, |d, (n, fg, bg)| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(-2.))
                            .right(px(-4.))
                            .child(pod_ui::count_badge(n, fg, bg)),
                    )
                })
                .on_click(cx.listener(move |r, _: &ClickEvent, window, cx| r.press(b, window, cx)))
                .on_key_down(cx.listener(move |r, e: &KeyDownEvent, window, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        r.press(b, window, cx);
                        cx.stop_propagation();
                    }
                }))
        };
        let attention_label = copy::attention_button(attention_n);
        let drawer = self.drawer;
        let rail = island
            .wrap(div())
            .id("rail")
            .role(Role::Toolbar)
            .aria_label(copy::RAIL)
            .w(px(RAIL_WIDTH))
            .flex_shrink_0()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .gap_1()
            .py(px(6.))
            .when(!rounded, |d| d.bg(rgb(tk.bg_chrome)))
            .when(!rounded && lines, |d| {
                d.border_r(px(border)).border_color(rgb(tk.border_subtle))
            })
            .qa_id(|| "rail".into())
            .child(rail_button(
                self,
                RailButton::Workspace,
                pod_ui::Icon::PanelLeft,
                copy::WORKSPACE.into(),
                copy::WORKSPACE_TIP,
                drawer == Some(Drawer::Workspace),
                Some(drawer == Some(Drawer::Workspace)),
                None,
                cx,
            ))
            .child(rail_button(
                self,
                RailButton::Terminal,
                pod_ui::Icon::SquareTerminal,
                copy::TERMINAL.into(),
                copy::TERMINAL_TIP,
                false,
                None,
                None,
                cx,
            ))
            .child(rail_button(
                self,
                RailButton::Files,
                pod_ui::Icon::FileText,
                copy::FILES.into(),
                copy::FILES_TIP,
                false,
                None,
                None,
                cx,
            ))
            .child(rail_button(
                self,
                RailButton::Agents,
                pod_ui::Icon::ListTree,
                copy::AGENTS.into(),
                copy::AGENTS_TIP,
                false,
                None,
                None,
                cx,
            ))
            .child(div().flex_1())
            .child(rail_button(
                self,
                RailButton::Attention,
                pod_ui::Icon::Bell,
                attention_label,
                copy::ATTENTION_TIP,
                drawer == Some(Drawer::Attention),
                Some(drawer == Some(Drawer::Attention)),
                (attention_n > 0).then_some((attention_n, tk.warning_fg, tk.warning_bg)),
                cx,
            ));

        // Task tabs.
        let tab_h = m.density.tabbar.max(32.);
        let font = self.font;
        let widths: Vec<f32> = self
            .tabs
            .iter()
            .map(|i| {
                tab_width(
                    &self.rows[*i].title,
                    font,
                    self.rows[*i].needs_you(),
                    self.rows[*i].agents.len(),
                )
            })
            .collect();
        let current_ix = self
            .current
            .as_deref()
            .and_then(|c| self.tabs.iter().position(|i| self.rows[*i].id == c));
        // Room: window minus the rail, the gaps, "+", "+N" and 작업 전환.
        let avail = strip_room(window.viewport_size().width.to_f64() as f32, gap, font);
        let visible: Vec<usize> = fit_tabs(&widths, current_ix, avail.max(0.))
            .into_iter()
            .map(|k| self.tabs[k])
            .collect();
        let hidden = self.tabs.len().saturating_sub(visible.len());
        let strip_focused = self.strip_focus.is_focused(window);
        let cursor = self.strip_cursor.clone().or_else(|| self.current.clone());
        let n_tabs = visible.len();
        let tabs = visible
            .iter()
            .enumerate()
            .map(|(k, i)| {
                let row = &self.rows[*i];
                let is_current = self.current.as_deref() == Some(row.id.as_str());
                let at_cursor = strip_focused && cursor.as_deref() == Some(row.id.as_str());
                let dot = row
                    .summary
                    .priority()
                    .map(|st| pod_ui::status_visual(st).tone.colors(tk).0);
                let badge = row.badge().map(|b| match b {
                    Badge::Failed(n) => pod_ui::count_badge(n, tk.danger_fg, tk.danger_bg),
                    Badge::Waiting(n) => pod_ui::count_badge(n, tk.warning_fg, tk.warning_bg),
                });
                let id = row.id.clone();
                let tip = format!(
                    "{} · {}",
                    row.branch.as_deref().unwrap_or("—"),
                    row.summary.label()
                );
                div()
                    .id(SharedString::from(format!("rail-tab-{}", row.id)))
                    .role(Role::Tab)
                    .aria_label(row.accessible_name())
                    .aria_selected(is_current)
                    .aria_position_in_set(k + 1)
                    .aria_size_of_set(n_tabs)
                    .h_full()
                    .flex_shrink_0()
                    .px_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .border_b_2()
                    .border_color(if is_current {
                        rgb(tk.accent_fg).into()
                    } else {
                        gpui_kit::transparent_black()
                    })
                    .text_color(rgb(if is_current {
                        tk.text_primary
                    } else {
                        tk.text_secondary
                    }))
                    .when(is_current, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .when(at_cursor, |d| {
                        d.bg(rgb(tk.bg_hover)).border_color(rgb(tk.focus_ring))
                    })
                    .when(!is_current, |d| d.hover(|s| s.bg(rgb(tk.bg_hover))))
                    .cursor_pointer()
                    .when(is_current, |d| d.qa_id(|| "rail-current-tab".into()))
                    .child(
                        div()
                            .w(px(8.))
                            .flex_shrink_0()
                            .when_some(dot, |d, c| d.child(pod_ui::status_dot(c))),
                    )
                    .child(
                        div()
                            .max_w(px(TAB_TEXT_MAX))
                            .min_w_0()
                            .truncate()
                            .child(row.title.clone()),
                    )
                    .when(!row.agents.is_empty(), |d| {
                        d.child(crate::agent_kind::stack_el(
                            &format!("rail-tab-{}", row.id),
                            &row.agents,
                            12.,
                        ))
                    })
                    .when_some(badge, |d, b| d.child(b))
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                    .on_click(cx.listener(move |r, _: &ClickEvent, window, cx| {
                        r.strip_cursor = Some(id.clone());
                        r.switch_to(&id, true, window, cx);
                    }))
            })
            .collect::<Vec<_>>();
        let visible_ids: Vec<String> = visible.iter().map(|i| self.rows[*i].id.clone()).collect();
        let strip = div()
            .id("rail-task-tabs")
            .track_focus(&self.strip_focus)
            .role(Role::TabList)
            .aria_label(copy::TABS)
            .aria_keyshortcuts("Control+Alt+PageDown Control+Alt+PageUp")
            .h_full()
            .min_w_0()
            .flex()
            .overflow_hidden()
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .children(tabs)
            .on_key_down(cx.listener(move |r, e: &KeyDownEvent, window, cx| {
                let at = r
                    .strip_cursor
                    .clone()
                    .or_else(|| r.current.clone())
                    .and_then(|c| visible_ids.iter().position(|x| *x == c))
                    .unwrap_or(0);
                match key_step(
                    e.keystroke.key.as_str(),
                    at,
                    visible_ids.len(),
                    "left",
                    "right",
                ) {
                    Some(KeyStep::Activate) => {
                        if let Some(id) = visible_ids.get(at).cloned() {
                            r.switch_to(&id, false, window, cx);
                        }
                        cx.stop_propagation();
                    }
                    Some(KeyStep::To(i)) => {
                        r.strip_cursor = visible_ids.get(i).cloned();
                        cx.stop_propagation();
                        cx.notify();
                    }
                    None => {}
                }
            }));
        let actions = self.actions.clone();
        let tasks_open = drawer == Some(Drawer::Tasks);
        let switcher = div()
            .id("rail-switcher")
            .track_focus(&self.switcher_focus)
            .role(Role::Button)
            .aria_label(copy::SWITCHER)
            .aria_expanded(tasks_open)
            .aria_keyshortcuts("Control+Alt+T")
            .h(px(28.))
            .px_2()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .rounded(px(m.radius.control))
            .text_color(rgb(tk.text_secondary))
            .cursor_pointer()
            .when(tasks_open, |d| d.bg(rgb(tk.bg_selected)))
            .hover(|s| s.bg(rgb(tk.bg_hover)))
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .tooltip(|window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(copy::SWITCHER_TIP).build(window, cx)
            })
            .qa_id(|| "rail-switcher".into())
            .child(div().flex_shrink_0().child(copy::SWITCHER))
            .child(pod_ui::icon_sized(
                pod_ui::Icon::ChevronDown,
                14.,
                tk.text_muted,
            ))
            .on_click(cx.listener(|r, _: &ClickEvent, window, cx| {
                r.toggle_drawer(Drawer::Tasks, window, cx)
            }))
            .on_key_down(cx.listener(|r, e: &KeyDownEvent, window, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space" | "down") {
                    r.toggle_drawer(Drawer::Tasks, window, cx);
                    cx.stop_propagation();
                }
            }));
        let more = (hidden > 0).then(|| {
            div()
                .id("rail-more-tasks")
                .role(Role::Button)
                .aria_label(copy::hidden_tasks(hidden))
                .tab_index(0)
                .h(px(24.))
                .px_2()
                .flex_shrink_0()
                .flex()
                .items_center()
                .rounded(px(m.radius.control))
                .border_1()
                .border_color(rgb(tk.border_subtle))
                .text_xs()
                .text_color(rgb(tk.text_secondary))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk.bg_hover)))
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .child(format!("+{hidden}"))
                .on_click(cx.listener(|r, _: &ClickEvent, window, cx| {
                    r.toggle_drawer(Drawer::Tasks, window, cx)
                }))
                .on_key_down(cx.listener(|r, e: &KeyDownEvent, window, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        r.toggle_drawer(Drawer::Tasks, window, cx);
                        cx.stop_propagation();
                    }
                }))
        });
        let top = div()
            .h(px(tab_h))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .pr_2()
            .text_size(px(font))
            .when(!rounded, |d| d.bg(rgb(tk.bg_chrome)))
            .when(!rounded && lines, |d| {
                d.border_b(px(border)).border_color(rgb(tk.border_subtle))
            })
            .qa_id(|| "rail-tabs".into())
            .child(strip)
            .child(
                pod_ui::IconButton::new(
                    "rail-new-task",
                    pod_ui::Icon::Plus,
                    crate::strings::NEW_TASK,
                )
                .tooltip(copy::NEW_TASK_TIP)
                .render(mode, move |window, cx| {
                    actions.dispatch(UiIntent::NewTask, window, cx);
                }),
            )
            .child(div().flex_1())
            .children(more)
            .child(switcher);

        // Dock (or the empty state).
        let shown_empty = self.shown_panels(cx).is_empty();
        let empty = shown_empty.then(|| self.render_empty(cx));
        let main = island
            .wrap(div())
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .relative()
            .child(top)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.dock.clone())
                    .children(empty)
                    .children(self.render_drawer(window, cx)),
            );

        div()
            .id("rail-body")
            .key_context(CONTEXT)
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
            .on_action(cx.listener(|r, _: &NextTask, window, cx| r.step_task(1, window, cx)))
            .on_action(cx.listener(|r, _: &PrevTask, window, cx| r.step_task(-1, window, cx)))
            .on_action(cx.listener(|r, _: &TaskSwitcher, window, cx| {
                r.toggle_drawer(Drawer::Tasks, window, cx)
            }))
            .on_action(cx.listener(|r, _: &AttentionDrawer, window, cx| {
                r.toggle_drawer(Drawer::Attention, window, cx)
            }))
            .on_action(cx.listener(|r, _: &WorkspaceDrawer, window, cx| {
                r.toggle_drawer(Drawer::Workspace, window, cx)
            }))
            .on_action(
                cx.listener(|r, _: &crate::workspace::ArrangeGrid, window, cx| {
                    r.rearrange(window, cx)
                }),
            )
            .child(rail)
            .child(main)
    }
}

impl RailState {
    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let tk = crate::theme::tokens();
        let mode = crate::theme::mode();
        let row = self.current_row().cloned();
        let running = row
            .as_ref()
            .and_then(|r| {
                self.actions.read(cx, |f| {
                    f.task(&r.id)
                        .map(|t| {
                            t.terminals
                                .iter()
                                .filter(|id| f.terminal(id).is_some_and(|x| x.is_running()))
                                .count()
                        })
                        .unwrap_or(0)
                })
            })
            .unwrap_or(0);
        let (title, body) = match &row {
            Some(r) => (
                format!("{} — {}", r.title, copy::EMPTY_TITLE),
                copy::EMPTY_BODY,
            ),
            None => (copy::NO_TASKS.to_string(), copy::NO_TASK_BODY),
        };
        let actions = self.actions.clone();
        let task = row.as_ref().map(|r| r.id.clone());
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .bg(rgb(tk.bg_canvas))
            .text_sm()
            .text_color(rgb(tk.text_muted))
            .child(
                pod_ui::Frame::new("rail-empty", pod_ui::FrameKind::Empty, title, body).render(
                    mode,
                    [div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .when(row.is_some(), |d| {
                            d.child(
                                Button::new("rail-open-terminals")
                                    .label(copy::open_terminals(running))
                                    .primary()
                                    .on_click(cx.listener(|r, _, window, cx| {
                                        r.open_task_terminals(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("rail-open-files")
                                    .label(crate::strings::OPEN_FILES)
                                    .on_click({
                                        let actions = actions.clone();
                                        let task = task.clone();
                                        move |_, window, cx| {
                                            if let Some(task) = &task {
                                                actions.dispatch(
                                                    UiIntent::OpenEditor {
                                                        task: task.clone(),
                                                        target: None,
                                                        quick_open: false,
                                                    },
                                                    window,
                                                    cx,
                                                );
                                            }
                                        }
                                    }),
                            )
                        })
                        .child(
                            Button::new("rail-empty-new-task")
                                .label(crate::strings::NEW_TASK)
                                .on_click(move |_, window, cx| {
                                    actions.dispatch(UiIntent::NewTask, window, cx);
                                }),
                        )
                        .into_any_element()],
                ),
            )
            .into_any_element()
    }

    fn render_drawer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let which = self.drawer?;
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        let (title, width) = match which {
            Drawer::Workspace => (copy::WORKSPACE, m.density.sidebar),
            Drawer::Attention => (copy::ATTENTION_TITLE, 360.),
            Drawer::Tasks => (copy::ALL_TASKS, 380.),
        };
        let header = div()
            .h(px(m.density.panel_header.max(32.)))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .pl_3()
            .pr_1()
            .border_b_1()
            .border_color(rgb(tk.border_subtle))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                pod_ui::IconButton::new("rail-drawer-close", pod_ui::Icon::X, copy::CLOSE)
                    .tooltip(copy::CLOSE_TIP)
                    .render(mode, {
                        let me = cx.entity().downgrade();
                        move |window, cx| {
                            let _ = me.update(cx, |r, cx| r.close_drawer(window, cx));
                        }
                    }),
            );
        let body: AnyElement = match which {
            Drawer::Workspace => match self.sidebar(cx) {
                Some(sidebar) => div()
                    .flex_1()
                    .min_h_0()
                    .pt_2()
                    .child(sidebar)
                    .into_any_element(),
                None => div().into_any_element(),
            },
            Drawer::Tasks | Drawer::Attention => self.render_list(which, window, cx),
        };
        let footer = (which == Drawer::Attention).then(|| {
            let actions = self.actions.clone();
            let actions2 = self.actions.clone();
            let gates = self.rows.iter().map(|r| r.gates).sum::<usize>();
            div()
                .flex_shrink_0()
                .flex()
                .flex_wrap()
                .gap_2()
                .p_2()
                .border_t_1()
                .border_color(rgb(tk.border_subtle))
                .child(
                    Button::new("rail-all-notifications")
                        .label(copy::ALL_NOTIFICATIONS)
                        .on_click(move |_, window, cx| {
                            actions.dispatch(UiIntent::OpenNotifications, window, cx);
                        }),
                )
                .when(gates > 0, |d| {
                    d.child(
                        Button::new("rail-open-gates")
                            .label(format!("{} ({gates})", copy::GATES))
                            .on_click(move |_, window, cx| {
                                actions2.dispatch(UiIntent::OpenGates, window, cx);
                            }),
                    )
                })
        });
        let left = which != Drawer::Tasks;
        Some(
            div()
                .id("rail-drawer")
                .role(Role::Dialog)
                .aria_label(title)
                .absolute()
                .top_0()
                .when(left, |d| d.left_0().bottom_0())
                .when(!left, |d| d.right(px(8.)).max_h(relative(0.8)))
                .w(px(width))
                .max_w(relative(0.9))
                .flex()
                .flex_col()
                .bg(rgb(tk.bg_raised))
                .border_1()
                .border_color(rgb(tk.border_strong))
                .when(!left, |d| d.rounded_b(px(m.radius.overlay)))
                .shadow(pod_ui::tokens::elevation(2, mode))
                .occlude()
                .qa_id(|| "rail-drawer".into())
                .on_key_down(cx.listener(|r, e: &KeyDownEvent, window, cx| {
                    if e.keystroke.key == "escape" {
                        r.close_drawer(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(header)
                .child(body)
                .children(footer)
                .into_any_element(),
        )
    }

    fn render_list(
        &mut self,
        which: Drawer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        let len = self.list_len();
        let focused = self.list_focus.is_focused(window);
        if len == 0 {
            let text = if which == Drawer::Attention {
                copy::ATTENTION_EMPTY
            } else {
                copy::NO_TASKS
            };
            return div()
                .id("rail-list")
                .track_focus(&self.list_focus)
                .role(Role::List)
                .aria_label(text)
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .p_3()
                .text_sm()
                .text_color(rgb(tk.text_muted))
                .child(text)
                .into_any_element();
        }
        let row_h = (m.density.row + 4.).max(30.);
        let list = uniform_list(
            "rail-list-rows",
            len,
            cx.processor(move |r, range: std::ops::Range<usize>, _, cx| {
                let tk = crate::theme::tokens();
                range
                    .map(|ix| {
                        let row_ix = match which {
                            Drawer::Attention => r.attention[ix],
                            _ => ix,
                        };
                        let row = &r.rows[row_ix];
                        let selected = ix == r.list_cursor;
                        let is_current = r.current.as_deref() == Some(row.id.as_str());
                        let (icon, fg) = match row.mark(is_current) {
                            RowMark::Waiting => (pod_ui::Icon::ShieldQuestion, tk.warning_fg),
                            RowMark::Failed => (pod_ui::Icon::OctagonX, tk.danger_fg),
                            RowMark::Current => (pod_ui::Icon::Check, tk.accent_fg),
                            RowMark::Idle => (pod_ui::Icon::Circle, tk.text_muted),
                        };
                        let detail = if which == Drawer::Attention {
                            row.attention_label()
                        } else {
                            row.branch.clone().unwrap_or_default()
                        };
                        div()
                            .id(("rail-row", ix))
                            .role(Role::ListItem)
                            .aria_label(if is_current {
                                copy::current_task(&row.accessible_name())
                            } else {
                                row.accessible_name()
                            })
                            .aria_selected(selected)
                            .aria_position_in_set(ix + 1)
                            .aria_size_of_set(len)
                            .h(px(row_h))
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .when(selected, |d| d.bg(rgb(tk.bg_selected)))
                            .when(selected && focused, |d| {
                                d.border_l_2().border_color(rgb(tk.focus_ring))
                            })
                            .when(!selected, |d| d.hover(|s| s.bg(rgb(tk.bg_hover))))
                            .child(pod_ui::icon_sized(icon, 14., fg))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(tk.text_primary))
                                    .when(is_current, |d| d.font_weight(FontWeight::SEMIBOLD))
                                    .child(row.title.clone()),
                            )
                            .when(!row.agents.is_empty(), |d| {
                                d.child(crate::agent_kind::stack_el(
                                    &format!("rail-row-{}", row.id),
                                    &row.agents,
                                    12.,
                                ))
                            })
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .max_w(px(160.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(if which == Drawer::Attention {
                                        fg
                                    } else {
                                        tk.text_muted
                                    }))
                                    .child(detail),
                            )
                            .on_click(cx.listener(move |r, _: &ClickEvent, window, cx| {
                                r.list_cursor = ix;
                                r.activate_row(ix, window, cx);
                            }))
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h(px(row_h));
        div()
            .id("rail-list")
            .track_focus(&self.list_focus)
            .role(Role::List)
            .aria_label(if which == Drawer::Attention {
                copy::ATTENTION_TITLE
            } else {
                copy::ALL_TASKS
            })
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .qa_id(|| "rail-list".into())
            .flex()
            .flex_col()
            .min_h(px(row_h))
            // The switcher drawer sizes to its rows (basis = height, may shrink
            // under max_h); the side drawers fill their full height.
            .when(which == Drawer::Tasks, |d| {
                d.flex_initial().h(px((len as f32 * row_h).min(480.)))
            })
            .when(which != Drawer::Tasks, |d| d.flex_1())
            .child(list)
            .on_key_down(cx.listener(move |r, e: &KeyDownEvent, window, cx| {
                let len = r.list_len();
                match key_step(e.keystroke.key.as_str(), r.list_cursor, len, "up", "down") {
                    Some(KeyStep::Activate) => {
                        let ix = r.list_cursor;
                        r.activate_row(ix, window, cx);
                        cx.stop_propagation();
                    }
                    Some(KeyStep::To(i)) => {
                        r.list_cursor = i;
                        r.list_scroll.scroll_to_item(i, ScrollStrategy::Center);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    None => {}
                }
            }))
            .into_any_element()
    }
}
