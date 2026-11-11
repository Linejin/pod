//! Render of the dense layout: the activity bar, the 출력 log, the 세부 정보
//! column and the body around the workspace's tree and dock. What it shows is
//! decided by `model` and the view state; events call the view's controller
//! methods (`dense.rs`) or hand a [`UiIntent`] to the host. The shared
//! pieces come from the kit: the island frame, the panel header, the
//! connection warning, the UI and mono fonts and the token colours.
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{Icon, semantic_button};

use super::log::{LogEvent, clock};
use super::model::{self, Act, BarEntry, Link, Panel, Sel};
use super::{
    DenseView, FocusDetails, FocusOutput, HISTORY_ROWS, KEY_CONTEXT, NextRegion, PrevRegion, copy,
};
use crate::layouts::kit::{
    Island, LayoutActions, LayoutFrame, LayoutMetrics, Slot, UiIntent, color, panel_header,
    warning_banner,
};
use crate::qa_marks::QaId;
use crate::vm::{TaskVm, TerminalVm};

/// Local time zone offset in minutes (read once; DST changes during a session are ignored).
fn local_offset_min() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::SYSTEMTIME;
            use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
            // SAFETY: both calls only fill the SYSTEMTIME passed in.
            let (mut l, mut u): (SYSTEMTIME, SYSTEMTIME) =
                unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
            unsafe {
                GetLocalTime(&mut l);
                GetSystemTime(&mut u);
            }
            super::log::offset_between((l.wDay, l.wHour, l.wMinute), (u.wDay, u.wHour, u.wMinute))
        }
        #[cfg(not(windows))]
        {
            0
        }
    })
}

/// Text action: accent text, keyboard operable, at least the style's hit area tall.
fn link_button(
    id: SharedString,
    label: &'static str,
    name: String,
    icon: Option<Icon>,
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
        .gap_2()
        .rounded(px(m.radius.control))
        .text_color(color(tk.accent_fg))
        .whitespace_nowrap()
        .cursor_pointer()
        .hover(|s| s.bg(color(tk.bg_hover)))
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .when_some(icon, |d, i| {
            d.child(pod_ui::icon_sized(i, 14., tk.accent_fg))
        })
        .child(label)
        .on_click(move |_, window, cx| activate(window, cx))
        .on_key_down(move |e, window, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                key(window, cx);
                cx.stop_propagation();
            }
        })
}

/// A details action as a [`link_button`]: an intent goes to the host, 다시
/// 실행 to the view's controller.
fn link_el(link: Link, view: &WeakEntity<DenseView>, actions: &LayoutActions) -> AnyElement {
    let Link {
        id,
        label,
        name,
        icon,
        act,
    } = link;
    let (view, actions) = (view.clone(), actions.clone());
    link_button(
        id.into(),
        label,
        name,
        Some(icon),
        move |window, cx| match &act {
            Act::Intent(intent) => actions.dispatch(intent.clone(), window, cx),
            Act::Rerun(id) => {
                let _ = view.update(cx, |v, cx| v.rerun(id, cx));
            }
        },
    )
    .into_any_element()
}

/// Activity bar ([`model::activity_bar`]): panel toggles call the view, commands go to the host.
fn activity_bar(view: &Entity<DenseView>, cx: &App) -> AnyElement {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let v = view.read(cx);
    let actions = v.actions.clone();
    let toggle =
        |id: &'static str, icon: Icon, label: &'static str, tip: &'static str, panel: Panel| {
            let view = view.downgrade();
            div()
                .rounded(px(m.radius.control))
                .relative()
                .when(v.shows(panel), |d| {
                    d.bg(color(tk.bg_selected)).child(
                        div()
                            .absolute()
                            .left_0()
                            .top(px(6.))
                            .bottom(px(6.))
                            .w(px(2.))
                            .bg(color(tk.accent_fg)),
                    )
                })
                .child(
                    pod_ui::IconButton::new(id, icon, label)
                        .tooltip(tip)
                        .render(mode, move |window, cx| {
                            let _ = view.update(cx, |v, cx| {
                                v.toggle(panel, window, cx);
                                cx.notify();
                            });
                        }),
                )
                .into_any_element()
        };
    let command =
        |id: &'static str, icon: Icon, label: &'static str, tip: &'static str, intent: UiIntent| {
            let actions = actions.clone();
            pod_ui::IconButton::new(id, icon, label)
                .tooltip(tip)
                .render(mode, move |window, cx| {
                    actions.dispatch(intent.clone(), window, cx)
                })
                .into_any_element()
        };
    let entries = model::activity_bar().into_iter().map(|e| match e {
        BarEntry::Toggle {
            panel,
            id,
            icon,
            label,
            tip,
        } => toggle(id, icon, label, tip, panel),
        BarEntry::Command {
            id,
            icon,
            label,
            tip,
            intent,
        } => command(id, icon, label, tip, intent),
        BarEntry::Spacer => div().flex_1().into_any_element(),
    });
    div()
        .id("dense-activity")
        .qa_id(|| "dense-activity".into())
        .role(Role::Toolbar)
        .aria_label(copy::ACTIVITY)
        .w(px(pod_ui::hit_area() + 12.))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .py_1()
        .bg(color(tk.bg_chrome))
        .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
            d.border_r(px(m.border_width))
                .border_color(color(tk.border_subtle))
        })
        .children(entries)
        .into_any_element()
}

/// The 출력 panel: a compact, virtualized log.
fn output_panel(
    view: &Entity<DenseView>,
    metrics: &LayoutMetrics,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let ui = metrics.ui_font;
    let row = model::row_height(ui);
    let mono = metrics.mono_font.clone();
    let offset = local_offset_min();
    let v = view.read(cx);
    let events = Rc::new(v.visible());
    let focused = v.log_focus.contains_focused(window, cx);
    let sel = v.log_sel;
    let focus = v.log_focus.clone();
    let scroll = v.log_scroll.clone();
    let n = events.len();
    let weak = view.downgrade();
    let close = {
        let weak = weak.clone();
        pod_ui::IconButton::new("dense-output-close", Icon::X, copy::CLOSE_OUTPUT).render(
            mode,
            move |window, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.show_output = false;
                    if let Some(f) = v.actions.focus_handle(Slot::Dock, cx) {
                        f.focus(window, cx);
                    }
                    cx.notify();
                });
            },
        )
    };
    let clear = {
        let weak = weak.clone();
        pod_ui::IconButton::new("dense-output-clear", Icon::Trash2, copy::CLEAR).render(
            mode,
            move |_, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.cleared = v.log.events().next_back().map_or(0, |e| e.seq + 1);
                    v.log_sel = None;
                    cx.notify();
                });
            },
        )
    };
    let title = div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .child(copy::OUTPUT),
        )
        .child(
            div()
                .text_color(color(tk.text_muted))
                .whitespace_nowrap()
                .child(format!("{} {n}", copy::EVENTS)),
        );
    let list = if n == 0 {
        div()
            .px_3()
            .py_2()
            .text_color(color(tk.text_muted))
            .child(copy::LOG_EMPTY)
            .into_any_element()
    } else {
        let rows = events.clone();
        let weak = weak.clone();
        let mono = mono.clone();
        uniform_list("dense-log-rows", n, move |range, _, _| {
            rows[range]
                .iter()
                .map(|e| {
                    let selected = sel == Some(e.seq);
                    let (fg, _) = e.tone.colors(tk);
                    let text_fg = if model::plain_text(e.tone) {
                        tk.text_primary
                    } else {
                        fg
                    };
                    let time = clock(e.at, offset);
                    let ev = e.clone();
                    let weak = weak.clone();
                    div()
                        .id(SharedString::from(format!("dense-log-{}", e.seq)))
                        .role(Role::ListItem)
                        .aria_label(SharedString::from(format!(
                            "{time}, {}, {}, {}",
                            e.source, e.who, e.text
                        )))
                        .aria_selected(selected)
                        .h(px(row))
                        .px_3()
                        .flex()
                        .items_center()
                        .gap_3()
                        .font_family(mono.clone())
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .cursor_pointer()
                        .when(selected, |d| d.bg(color(tk.bg_selected)))
                        .when(selected && focused, |d| {
                            d.border_1().border_color(color(tk.focus_ring))
                        })
                        .when(!selected, |d| d.hover(|s| s.bg(color(tk.bg_hover))))
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(color(tk.text_muted))
                                .child(time),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .w(px(ui * 3.))
                                .truncate()
                                .text_color(color(tk.text_secondary))
                                .child(e.source.clone()),
                        )
                        .when_some(e.agent.as_ref(), |d, a| {
                            d.child(crate::agent_kind::mark_el(
                                &format!("dense-log-{}", e.seq),
                                a,
                                14.,
                            ))
                        })
                        .child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(ui * 14.))
                                .truncate()
                                .text_color(color(tk.text_primary))
                                .child(e.who.clone()),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(color(tk.text_muted))
                                .child("→"),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(color(text_fg))
                                .child(e.text.clone()),
                        )
                        .on_click(move |click, window, cx| {
                            let _ = weak.update(cx, |v, cx| {
                                v.log_focus.focus(window, cx);
                                v.activate_event(&ev, click.click_count() > 1, window, cx);
                            });
                        })
                        .into_any_element()
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&scroll)
        .flex_1()
        .min_h_0()
        .into_any_element()
    };
    let on_key = view.downgrade();
    div()
        .id("dense-output")
        .qa_id(|| "dense-output".into())
        .role(Role::Region)
        .aria_label(copy::OUTPUT)
        .flex_shrink_0()
        .h(px(model::output_height(ui)))
        .flex()
        .flex_col()
        .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
            d.border_t(px(m.border_width))
                .border_color(color(tk.border_subtle))
        })
        .bg(color(tk.bg_surface))
        .child(panel_header(
            "dense-output-header",
            title,
            vec![clear.into_any_element(), close.into_any_element()],
            model::header_height(ui),
        ))
        .child(
            div()
                .id("dense-log")
                .qa_id(|| "dense-log".into())
                .track_focus(&focus)
                .role(Role::List)
                .aria_label(copy::LOG)
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .py_1()
                .on_key_down(move |e, window, cx| {
                    let _ = on_key.update(cx, |v, cx| v.on_log_key(e, window, cx));
                })
                .child(list),
        )
        .into_any_element()
}

/// A `label  element` row of the details column.
fn field_el(label: &'static str, value: impl IntoElement, ui: f32) -> Div {
    let tk = crate::theme::tokens();
    div()
        .flex()
        .items_center()
        .gap_2()
        .min_h(px(model::row_height(ui)))
        .py(px(2.))
        .child(
            div()
                .flex_shrink_0()
                .w(px(ui * 5.5))
                .text_color(color(tk.text_muted))
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(color(tk.text_primary))
                .child(value),
        )
}

/// One `label  value` row of the details column.
fn field(
    label: &'static str,
    value: impl Into<SharedString>,
    fg: u32,
    ui: f32,
    truncate: bool,
) -> Div {
    let tk = crate::theme::tokens();
    let value = value.into();
    let v = div().flex_1().min_w_0().text_color(color(fg));
    let v = if truncate {
        v.truncate().child(value.clone())
    } else {
        v.child(value.clone())
    };
    div()
        .flex()
        .items_start()
        .gap_2()
        .min_h(px(model::row_height(ui)))
        .py(px(2.))
        .child(
            div()
                .flex_shrink_0()
                .w(px(ui * 5.5))
                .text_color(color(tk.text_muted))
                .child(label),
        )
        .child(v)
}

fn section_title(text: &'static str) -> Div {
    let tk = crate::theme::tokens();
    div()
        .pt_3()
        .pb_1()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(tk.text_secondary))
        .child(text)
}

/// The 세부 정보 column for the selected task or agent. `label` names a
/// terminal as the window does (`LayoutFrame::label_of`).
fn details_column(
    view: &Entity<DenseView>,
    f: &LayoutFrame<'_>,
    label_of: &dyn Fn(&TerminalVm) -> String,
    cx: &App,
) -> AnyElement {
    let tk = crate::theme::tokens();
    let mode = crate::theme::mode();
    let ui = f.metrics.ui_font;
    let mono = f.metrics.mono_font.clone();
    let offset = local_offset_min();
    let v = view.read(cx);
    let now = v.clock.now_ms();
    let weak = view.downgrade();
    let host = v.actions.clone();
    let focus = v.details_focus.clone();
    let mut body = div().w_full().flex().flex_col().px_3().pb_3();
    let mut name = String::from(copy::NOTHING);

    // The selection, or nothing when it vanished.
    let sel = model::shown_selection(v.selected.as_ref(), f.tasks, f.terminals);
    let mut history_ids: Vec<String> = Vec::new();
    let mut actions: Vec<AnyElement> = Vec::new();
    match &sel {
        None => {
            body = body.child(
                div()
                    .pt_2()
                    .text_color(color(tk.text_muted))
                    .child(copy::NOTHING),
            );
        }
        Some(Sel::Terminal(id)) => {
            let t = f.terminal(id).cloned().expect("filtered above");
            let task: Option<TaskVm> = t.task_id.as_deref().and_then(|k| f.task(k)).cloned();
            let label = label_of(&t);
            let badge = crate::orch::terminal_badge(&t);
            let (_, state_fg) = badge.glyph(mode);
            let agent = crate::agent_kind::identify_in(&t, f.tasks).kind;
            name = format!("{label}, {}, {}", agent.accessible_name(), badge.label());
            body = body
                .child(field(copy::NAME, label.clone(), tk.text_primary, ui, false))
                .child(field_el(
                    copy::AGENT,
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(crate::agent_kind::mark_el("dense-details", &agent, 16.))
                        .child(agent.name()),
                    ui,
                ))
                .child(field(copy::STATE, badge.label(), state_fg, ui, false))
                .when_some(task.as_ref(), |d, k| {
                    d.child(field(
                        copy::TASK,
                        k.title.clone(),
                        tk.text_primary,
                        ui,
                        false,
                    ))
                })
                .child(field(
                    copy::PROGRAM,
                    t.program.clone(),
                    tk.text_primary,
                    ui,
                    false,
                ))
                .child(field(
                    copy::COMMAND,
                    t.argv.join(" "),
                    tk.text_secondary,
                    ui,
                    true,
                ))
                .when_some(task.as_ref().and_then(|k| k.branch.clone()), |d, b| {
                    d.child(field(copy::BRANCH, b, tk.text_primary, ui, true))
                })
                .child(field(
                    copy::WORKTREE,
                    task.as_ref()
                        .and_then(|k| k.worktree.clone())
                        .unwrap_or_else(|| t.cwd.clone()),
                    tk.text_secondary,
                    ui,
                    true,
                ))
                .when_some(
                    t.status
                        .as_ref()
                        .map(|s| crate::layouts::queue::age(s.since, now))
                        .filter(|a| !a.is_empty()),
                    |d, a| d.child(field(copy::ELAPSED, a, tk.text_primary, ui, false)),
                )
                .when_some(t.status.as_ref().and_then(|s| s.source.clone()), |d, s| {
                    d.child(field(copy::ORIGIN, s, tk.text_secondary, ui, false))
                })
                .child(field(
                    copy::PR,
                    task.as_ref()
                        .and_then(|k| k.pr.as_ref())
                        .map(model::pr_text)
                        .unwrap_or_else(|| copy::NO_PR.into()),
                    tk.text_primary,
                    ui,
                    false,
                ));
            // ── actions ──
            if let Some(g) = model::pending_gate(f.gates, id) {
                let busy = v.busy.get(&g.id).copied();
                body = body
                    .child(div().pt_2().text_color(color(tk.warning_fg)).child(format!(
                        "{} · {}",
                        copy::GATE_REQUESTED,
                        g.title
                    )))
                    .child(
                        div()
                            .font_family(mono.clone())
                            .text_color(color(tk.text_secondary))
                            .child(g.message.clone()),
                    );
                for b in model::gate_buttons(g) {
                    let (weak, gid) = (weak.clone(), g.id.clone());
                    let (d, choice) = (b.decision, b.choice);
                    actions.push(
                        semantic_button(
                            SharedString::from(b.id),
                            b.label,
                            b.tone,
                            mode,
                            model::button_state(busy, d),
                            move |window, cx| {
                                let _ = weak.update(cx, |v, cx| {
                                    v.decide(gid.clone(), d, choice.clone(), window, cx)
                                });
                            },
                        )
                        .aria_label(SharedString::from(b.name))
                        .into_any_element(),
                    );
                }
                if let Some(e) = v.errors.get(&g.id) {
                    body = body.child(div().text_color(color(tk.danger_fg)).child(e.clone()));
                }
            }
            actions.push(link_el(model::terminal_link(&t, &label), &weak, &host));
            if let Some(k) = &task {
                actions.extend(
                    model::task_links(k)
                        .into_iter()
                        .map(|l| link_el(l, &weak, &host)),
                );
            }
            history_ids.push(t.id.clone());
            // ── last lines ──
            let lines = v
                .lines
                .get(&t.id)
                .map(|(_, l)| l.clone())
                .unwrap_or_default();
            let lines_el = div()
                .qa_id(|| "dense-last-lines".into())
                .flex()
                .flex_col()
                .p_2()
                .rounded(px(pod_ui::themes::metrics().radius.control))
                .bg(color(tk.bg_raised))
                .font_family(mono.clone())
                .text_color(color(tk.text_secondary))
                .when(lines.is_empty(), |d| {
                    d.text_color(color(tk.text_muted)).child(copy::NO_LINES)
                })
                .children(lines.into_iter().map(|l| div().truncate().child(l)));
            body = body
                .child(section_title(copy::ACTIONS))
                .child(
                    div()
                        .qa_id(|| "dense-actions".into())
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .items_center()
                        .children(actions),
                )
                .child(section_title(copy::LAST_LINES))
                .child(lines_el);
        }
        Some(Sel::Task(id)) => {
            let k = f.task(id).cloned().expect("filtered above");
            let badge = crate::orch::status_badge(k.status.as_ref());
            let (_, state_fg) = badge.glyph(mode);
            name = format!("{}, {}", k.title, badge.label());
            let kinds = crate::agent_kind::task_kinds(&k, f.terminals);
            body = body
                .child(field(
                    copy::NAME,
                    k.title.clone(),
                    tk.text_primary,
                    ui,
                    false,
                ))
                .when(!kinds.is_empty(), |d| {
                    d.child(field_el(
                        copy::AGENT,
                        crate::agent_kind::stack_el("dense-details", &kinds, 16.),
                        ui,
                    ))
                })
                .child(field(copy::STATE, badge.label(), state_fg, ui, false))
                .when_some(k.repo.clone(), |d, r| {
                    d.child(field(copy::REPO, r, tk.text_primary, ui, true))
                })
                .when_some(k.branch.clone(), |d, b| {
                    d.child(field(copy::BRANCH, b, tk.text_primary, ui, true))
                })
                .when_some(k.worktree.clone(), |d, w| {
                    d.child(field(copy::WORKTREE, w, tk.text_secondary, ui, true))
                })
                .when_some(k.git_summary(), |d, g| {
                    d.child(field(copy::GIT, g, tk.text_primary, ui, false))
                })
                .when(k.review_open > 0, |d| {
                    d.child(field(
                        copy::COMMENTS,
                        copy::comments(k.review_open),
                        tk.warning_fg,
                        ui,
                        false,
                    ))
                })
                .child(field(
                    copy::PR,
                    k.pr.as_ref()
                        .map(model::pr_text)
                        .unwrap_or_else(|| copy::NO_PR.into()),
                    tk.text_primary,
                    ui,
                    false,
                ));
            actions.extend(
                model::task_links(&k)
                    .into_iter()
                    .map(|l| link_el(l, &weak, &host)),
            );
            body = body.child(section_title(copy::ACTIONS)).child(
                div()
                    .qa_id(|| "dense-actions".into())
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .children(actions),
            );
            // ── its agents ──
            let mut agents = div().flex().flex_col();
            for tid in &k.terminals {
                let Some(t) = f.terminal(tid) else { continue };
                let label = label_of(t);
                let b = crate::orch::terminal_badge(t);
                let (icon, fg) = b.glyph(mode);
                let (weak, tid2) = (weak.clone(), tid.clone());
                agents = agents.child(
                    div()
                        .id(SharedString::from(format!("dense-agent-{tid}")))
                        .role(Role::Button)
                        .aria_label(SharedString::from(format!("{label}, {}", b.label())))
                        .tab_index(0)
                        .min_h(px(model::row_height(ui).max(pod_ui::hit_area())))
                        .px_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(pod_ui::themes::metrics().radius.control))
                        .cursor_pointer()
                        .hover(|s| s.bg(color(tk.bg_hover)))
                        .focus_visible(move |s| pod_ui::focus_style(s, mode))
                        .child(crate::agent_kind::mark_el(
                            &format!("dense-agent-{tid}"),
                            &crate::agent_kind::identify(t, Some(&k)).kind,
                            14.,
                        ))
                        .child(pod_ui::icon_sized(icon, 14., fg))
                        .child(div().flex_1().min_w_0().truncate().child(label))
                        .child(div().flex_shrink_0().text_color(color(fg)).child(b.label()))
                        .on_click({
                            let (weak, tid2) = (weak.clone(), tid2.clone());
                            move |_, _, cx| {
                                let _ = weak
                                    .update(cx, |v, cx| v.select(Sel::Terminal(tid2.clone()), cx));
                            }
                        })
                        .on_key_down(move |e, _, cx| {
                            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                                let _ = weak
                                    .update(cx, |v, cx| v.select(Sel::Terminal(tid2.clone()), cx));
                                cx.stop_propagation();
                            }
                        }),
                );
                history_ids.push(tid.clone());
            }
            body = body
                .child(section_title(copy::AGENTS_OF_TASK))
                .child(agents);
        }
    }
    if sel.is_some() {
        let history: Vec<LogEvent> = v
            .log
            .history(&history_ids)
            .take(HISTORY_ROWS)
            .cloned()
            .collect();
        let mut list = div().qa_id(|| "dense-history".into()).flex().flex_col();
        if history.is_empty() {
            list = list.child(
                div()
                    .text_color(color(tk.text_muted))
                    .child(copy::NO_HISTORY),
            );
        }
        for e in history {
            let (fg, _) = e.tone.colors(tk);
            list = list.child(
                div()
                    .flex()
                    .items_start()
                    .gap_2()
                    .min_h(px(model::row_height(ui)))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(mono.clone())
                            .text_color(color(tk.text_muted))
                            .child(clock(e.at, offset)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(color(if model::plain_text(e.tone) {
                                tk.text_primary
                            } else {
                                fg
                            }))
                            .child(e.text.clone()),
                    ),
            );
        }
        body = body.child(section_title(copy::HISTORY)).child(list);
    }
    let close = {
        let weak = weak.clone();
        pod_ui::IconButton::new("dense-details-close", Icon::X, copy::CLOSE_DETAILS).render(
            mode,
            move |_, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.show_details = false;
                    cx.notify();
                });
            },
        )
    };
    let m = pod_ui::themes::metrics();
    div()
        .id("dense-details")
        .qa_id(|| "dense-details".into())
        .w(px(model::details_width(ui)))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .min_h_0()
        .bg(color(tk.bg_chrome))
        .when(
            m.surface != pod_ui::themes::Surface::Mono
                && !matches!(
                    m.surface,
                    pod_ui::themes::Surface::Layered | pod_ui::themes::Surface::Islands
                ),
            |d| {
                d.border_l(px(m.border_width))
                    .border_color(color(tk.border_subtle))
            },
        )
        .child(panel_header(
            "dense-details-header",
            div().font_weight(FontWeight::SEMIBOLD).child(copy::DETAILS),
            vec![close.into_any_element()],
            model::header_height(ui),
        ))
        .child(
            div()
                .id("dense-details-body")
                .track_focus(&focus)
                .role(Role::Region)
                .aria_label(SharedString::from(format!("{}: {name}", copy::DETAILS)))
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(body),
        )
        .into_any_element()
}

/// What the body shows besides the view state: the host's slots and the
/// frame's connection and dock state.
pub(super) struct Body {
    pub(super) view: Entity<DenseView>,
    /// Connection warning (`LayoutFrame::warning`).
    pub(super) warning: Option<String>,
    /// The hook-trust prompt, while one is waiting.
    pub(super) hook_banner: Option<AnyElement>,
    /// The workspace's task tree.
    pub(super) sidebar: AnyElement,
    /// The workspace's dock.
    pub(super) dock: AnyElement,
    /// The toast stack, under the tree.
    pub(super) toasts: AnyElement,
    /// Nothing open in the dock: the empty hint over it.
    pub(super) empty: bool,
}

/// The window body: activity bar · tree (+ toasts) · dock over 출력 · 세부 정보.
pub(super) fn body(
    b: Body,
    f: &LayoutFrame<'_>,
    label_of: &dyn Fn(&TerminalVm) -> String,
    window: &Window,
    cx: &App,
) -> AnyElement {
    let Body {
        view,
        warning,
        hook_banner,
        sidebar,
        dock,
        toasts,
        empty,
    } = b;
    let m = pod_ui::themes::metrics();
    let tk = crate::theme::tokens();
    let island = Island::current();
    let gap = island.gap();
    let (show_tree, show_output, show_details) = {
        let v = view.read(cx);
        (v.show_tree, v.show_output, v.show_details)
    };
    let bar = activity_bar(&view, cx);
    let output = show_output.then(|| output_panel(&view, &f.metrics, window, cx));
    let details = show_details.then(|| details_column(&view, f, label_of, cx));
    let sidebar_w = m.density.sidebar;
    let actions_view = view.downgrade();
    let (v1, v2, v3) = (
        actions_view.clone(),
        actions_view.clone(),
        actions_view.clone(),
    );
    div()
        .id("dense-layout")
        .qa_id(|| "dense-layout".into())
        .key_context(KEY_CONTEXT)
        .on_action(move |_: &NextRegion, window, cx| {
            let _ = actions_view.update(cx, |v, cx| v.cycle(true, window, cx));
        })
        .on_action(move |_: &PrevRegion, window, cx| {
            let _ = v1.update(cx, |v, cx| v.cycle(false, window, cx));
        })
        .on_action(move |_: &FocusOutput, window, cx| {
            let _ = v2.update(cx, |v, cx| v.focus_output(window, cx));
        })
        .on_action(move |_: &FocusDetails, window, cx| {
            let _ = v3.update(cx, |v, cx| v.focus_details(window, cx));
        })
        .flex()
        .flex_1()
        .min_h_0()
        .child(bar)
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
                .when(show_tree, |d| {
                    d.child(
                        div()
                            .id("dense-tree")
                            .qa_id(|| "dense-tree".into())
                            .w(px(sidebar_w))
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(gap))
                            .child(island.wrap(div().flex_1().min_h_0()).child(sidebar))
                            .child(toasts),
                    )
                })
                .child(
                    island
                        .wrap(div())
                        .id("dense-center")
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .when_some(warning, |d, w| d.child(warning_banner(w)))
                        .when_some(hook_banner, |d, b| d.child(b))
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .child(dock)
                                .when(empty, |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .inset_0()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .px_4()
                                            .text_color(rgb(tk.text_muted))
                                            .child(
                                                pod_ui::Frame::new(
                                                    "dense-dock-empty",
                                                    pod_ui::FrameKind::Empty,
                                                    crate::strings::A_CLEAR_SPACE_FOR_YOUR_NEXT_TASK,
                                                    crate::strings::YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_,
                                                )
                                                .render(crate::theme::mode(), []),
                                            ),
                                    )
                                }),
                        )
                        .when_some(output, |d, o| d.child(o)),
                )
                .when_some(details, |d, x| {
                    d.child(island.wrap(div().flex().min_h_0()).child(x))
                }),
        )
        .into_any_element()
}
