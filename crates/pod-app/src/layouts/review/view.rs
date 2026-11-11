//! The layout's render: the task list on the left; on the right the canvas
//! header (branch, agents, gates), the canvas (Review, dock or an expanded
//! terminal) and the terminal strip; and the shell around the layout. The
//! model decides; this file draws and hands events to the view's controller
//! methods.
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::themes::{Metrics, Surface};
use podd::protocol::{GateDecision, GateInfo};

use super::model::{
    Accent, Canvas, Selected, accent, approve_choices, canvas_width, header_height, island_gap,
    strip_height,
};
use super::{
    KEY_CONTEXT, NextRegion, PrevRegion, ReviewView, Synced, TaskRow, ToggleExpand, Who,
    approve_label, branch_line, copy as s, row_name, tiles,
};
use crate::layouts::kit::{BodyFlow, ConnectionShell, Island, LayoutMetrics, ToastAnchor, color};
use crate::panel::TerminalPanel;
use crate::qa_marks::QaId;
use crate::vm::TerminalVm;

/// The window body: the layout under the connection warning and the hook
/// prompt, toasts top right over it.
pub(super) fn shell(
    warning: Option<String>,
    hook_banner: Option<AnyElement>,
    view: AnyElement,
    toasts: AnyElement,
) -> AnyElement {
    ConnectionShell {
        warning,
        hook_banner,
        canvas: Some(crate::theme::tokens().bg_canvas),
        flow: BodyFlow::Column,
        body: view,
        toasts,
        anchor: ToastAnchor::TOP_RIGHT,
    }
    .render()
    .into_any_element()
}

/// The task list and the canvas as rounded islands (Layered and Islands
/// styles): card radius, clipped, the caller's background kept; a hairline
/// for Islands, the style's elevation whenever it has a shadow.
fn island_wrap(m: Metrics, border: u32, d: Stateful<Div>) -> Stateful<Div> {
    let mode = crate::theme::mode();
    let d = d.rounded(px(m.radius.card)).overflow_hidden();
    match m.surface {
        Surface::Islands => d
            .shadow(pod_ui::tokens::elevation(1, mode))
            .border(px(m.border_width))
            .border_color(color(border)),
        _ if m.shadow != pod_ui::themes::Shadow::None => {
            d.shadow(pod_ui::tokens::elevation(1, mode))
        }
        _ => d,
    }
}

/// A small text button (Enter/Space, Korean name).
fn text_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    icon: Option<pod_ui::Icon>,
    primary: bool,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let on = Rc::new(on);
    let key = on.clone();
    let label: SharedString = label.into();
    let (fg, bg, border) = if primary {
        (
            t.action_primary_fg,
            t.action_primary_bg,
            t.action_primary_bg,
        )
    } else {
        (t.text_primary, t.bg_surface, t.border_control)
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(0)
        .h(px(pod_ui::hit_area()))
        .px(px(10.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .rounded(px(m.radius.control))
        .border(px(m.border_width))
        .border_color(color(border))
        .bg(color(bg))
        .text_color(color(fg))
        .font_weight(FontWeight::SEMIBOLD)
        .when_some(icon, |d, i| d.child(pod_ui::icon_sized(i, 14., fg)))
        .child(label)
        .cursor_pointer()
        .hover(move |s| {
            if primary {
                s.bg(color(t.action_primary_hover))
            } else {
                s.bg(color(t.bg_hover))
            }
        })
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on(window, cx);
        })
        .on_key_down(move |e, window, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space")
                && !e.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
                key(window, cx);
            }
        })
}

/// 승인 — or, for a gate with options (v17), one `승인: <option>` per choice.
fn approve_buttons(g: &GateInfo, view: WeakEntity<ReviewView>, gate: String) -> Vec<Stateful<Div>> {
    approve_choices(g)
        .into_iter()
        .enumerate()
        .map(|(i, choice)| {
            let (view, gate) = (view.clone(), gate.clone());
            let label = approve_label(choice.as_deref());
            text_button(
                SharedString::from(format!("review-approve-{}-{i}", g.id)),
                label,
                Some(pod_ui::Icon::Check),
                i == 0,
                move |window, cx| {
                    let _ = view.update(cx, |v, cx| {
                        v.resolve(
                            gate.clone(),
                            GateDecision::Approve,
                            choice.clone(),
                            window,
                            cx,
                        )
                    });
                },
            )
        })
        .collect()
}

/// A UI callback shared by click and key handlers.
type Action = Rc<dyn Fn(&mut Window, &mut App)>;

/// What one task row needs besides the row.
struct RowCtx {
    view: WeakEntity<ReviewView>,
    selected: Option<String>,
    focus: HashMap<String, FocusHandle>,
    badges: HashMap<String, Option<pod_ui::StatusBadge>>,
    height: f32,
}

fn render_row(r: &TaskRow, ctx: &RowCtx) -> AnyElement {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let selected = ctx.selected.as_deref() == Some(r.id.as_str());
    let (glyph, glyph_fg) = match ctx.badges.get(&r.id).cloned().flatten() {
        Some(b) => b.glyph(mode),
        None => (pod_ui::Icon::Circle, t.text_muted),
    };
    let focus = ctx.focus.get(&r.id).cloned();
    let view = ctx.view.clone();
    let id = r.id.clone();
    let sub = branch_line(r.branch.as_deref(), r.base.as_deref());
    let row = div()
        .id(SharedString::from(format!("review-task-{id}")))
        .qa_id({
            let id = id.clone();
            move || format!("review-task-{id}")
        })
        .when_some(focus, |d, f| d.track_focus(&f.tab_stop(selected)))
        .role(Role::ListItem)
        .aria_label(row_name(r))
        .aria_selected(selected)
        .h(px(ctx.height - 4.))
        .w_full()
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(2.))
        .px(px(10.))
        .rounded(px(m.radius.control))
        .border(px(m.border_width))
        .border_color(color(if selected && m.surface == Surface::Mono {
            t.text_primary
        } else {
            t.bg_surface
        }))
        .bg(color(if selected {
            t.bg_selected
        } else {
            t.bg_surface
        }))
        .when(!selected, |d| d.hover(move |s| s.bg(color(t.bg_hover))))
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .cursor_pointer()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(pod_ui::icon(glyph, glyph_fg))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(selected, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .text_color(color(t.text_primary))
                        .child(r.title.clone()),
                )
                .when(r.gates > 0, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(color(t.warning_fg))
                            .child(pod_ui::icon_sized(
                                pod_ui::Icon::ShieldQuestion,
                                12.,
                                t.warning_fg,
                            ))
                            .child(r.gates.to_string()),
                    )
                })
                .when(r.open_comments > 0, |d| {
                    d.child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .gap_1()
                            .text_xs()
                            .text_color(color(t.text_secondary))
                            .child(pod_ui::icon_sized(
                                pod_ui::Icon::MessageSquare,
                                12.,
                                t.text_secondary,
                            ))
                            .child(r.open_comments.to_string()),
                    )
                }),
        )
        .child(
            div()
                .qa_id({
                    let id = id.clone();
                    move || format!("review-task-sub-{id}")
                })
                .pl(px(22.))
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(color(t.text_muted))
                .child(s::row_sub(&sub, &r.status)),
        )
        .on_click({
            let (view, id) = (view.clone(), id.clone());
            move |_, window, cx| {
                let _ = view.update(cx, |v, cx| v.select(&id, window, cx));
            }
        })
        .on_key_down(move |e, window, cx| {
            if view
                .update(cx, |v, cx| v.on_row_key(&id, e, window, cx))
                .unwrap_or(false)
            {
                cx.stop_propagation();
            }
        });
    div()
        .h(px(ctx.height))
        .w_full()
        .px(px(6.))
        .pb(px(4.))
        .child(row)
        .into_any_element()
}

/// One docked (or expanded) terminal: a header with status and a size button, then the panel.
fn render_tile(
    view: WeakEntity<ReviewView>,
    t: &TerminalVm,
    who: &Who,
    panel: Option<Entity<TerminalPanel>>,
    expanded: bool,
    header_h: f32,
) -> Stateful<Div> {
    let tk = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let badge = crate::orch::terminal_badge(t);
    let (glyph, glyph_fg) = badge.glyph(mode);
    let status = badge.label();
    let title = who.title();
    let accent = accent(t).map(|a| match a {
        Accent::Warning => tk.warning_fg,
        Accent::Danger => tk.danger_fg,
    });
    let id = t.id.clone();
    div()
        .id(SharedString::from(format!("review-tile-{id}")))
        .qa_id({
            let id = id.clone();
            move || format!("review-tile-{id}")
        })
        .role(Role::Group)
        .aria_label(s::tile_name(&title, &status))
        .flex_1()
        .min_w(px(160.))
        .h_full()
        .flex()
        .flex_col()
        .min_h_0()
        .bg(color(tk.bg_surface))
        .border_r(px(m.border_width))
        .border_color(color(tk.border_subtle))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .min_h(px(header_h))
                .flex_shrink_0()
                .border_b(px(m.border_width))
                .border_color(color(tk.border_subtle))
                // Attention stripe (blocked: warning, failed: danger); never the only carrier: icon + status text too.
                .when_some(accent, |d, c| d.border_t(px(2.)).border_color(color(c)))
                .child(crate::agent_kind::mark_el(
                    &format!("review-tile-{id}"),
                    &who.kind,
                    pod_ui::tokens::ICON_SIZE,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(pod_ui::icon(glyph, glyph_fg))
                // The status is never the part cut off: the name shrinks first.
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(color(glyph_fg))
                        .child(status),
                )
                .child(
                    pod_ui::IconButton::new(
                        SharedString::from(format!("review-tile-size-{id}")),
                        if expanded {
                            pod_ui::Icon::Minimize2
                        } else {
                            pod_ui::Icon::Maximize2
                        },
                        if expanded { s::SHRINK } else { s::GROW },
                    )
                    .render(mode, move |window, cx| {
                        let _ = view.update(cx, |v, cx| v.expand(&id, window, cx));
                    }),
                ),
        )
        .child(div().flex_1().min_h_0().children(panel))
}

/// The canvas's 리뷰 / 패널 tab.
fn seg(id: &'static str, label: &'static str, on: bool, m: Metrics, f: Action) -> Stateful<Div> {
    let t = crate::theme::tokens();
    let mode = crate::theme::mode();
    let key = f.clone();
    div()
        .id(id)
        .role(Role::Tab)
        .aria_label(label)
        .aria_selected(on)
        .tab_index(0)
        .px_2()
        .h(px(pod_ui::hit_area()))
        .flex()
        .items_center()
        .rounded(px(m.radius.control))
        .text_color(color(if on { t.text_primary } else { t.text_secondary }))
        .when(on, |d| {
            d.bg(color(t.bg_selected)).font_weight(FontWeight::SEMIBOLD)
        })
        .hover(|s| s.bg(color(t.bg_hover)))
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .cursor_pointer()
        .child(label)
        .on_click(move |_, window, cx| f(window, cx))
        .on_key_down(move |e, window, cx| {
            if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                key(window, cx);
            }
        })
}

/// A muted note filling the canvas.
fn canvas_note(text: &'static str) -> AnyElement {
    div()
        .flex_1()
        .p_4()
        .text_color(color(crate::theme::tokens().text_muted))
        .child(text)
        .into_any_element()
}

impl Render for ReviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(Synced {
            rows,
            badges,
            parts,
            who,
            dock,
        }) = self.sync(window, cx)
        else {
            return div().into_any_element();
        };

        let t = crate::theme::tokens();
        let k = LayoutMetrics::current(cx);
        let m = k.style;
        let mode = crate::theme::mode();
        let rounded = Island::new(m, t.bg_surface, t.border_subtle).rounded();
        let gap = island_gap(rounded, m.gap);
        let header_h = header_height(m.density.panel_header, k.line());
        let me = cx.entity().downgrade();
        let island = move |d: Stateful<Div>| -> Stateful<Div> {
            if rounded {
                island_wrap(m, t.border_subtle, d)
            } else {
                d
            }
        };

        // ── left: task list ──
        let height = Self::row_height(cx);
        let n = rows.len();
        let list_rows = Rc::new(rows.clone());
        let ctx = Rc::new(RowCtx {
            view: me.clone(),
            selected: self.model.selected.clone(),
            focus: self.row_focus.clone(),
            badges,
            height,
        });
        let list = if rows.is_empty() {
            div()
                .flex_1()
                .p_3()
                .text_color(color(t.text_muted))
                .child(s::NO_TASKS)
                .into_any_element()
        } else {
            uniform_list("review-task-list", n, move |range, _, _| {
                list_rows[range]
                    .iter()
                    .map(|r| render_row(r, &ctx))
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.scroll)
            .flex_1()
            .min_h_0()
            .pt(px(4.))
            .into_any_element()
        };
        let left = island(
            div()
                .id("review-tasks")
                .qa_id(|| "review-tasks".into())
                .role(Role::List)
                .aria_label(s::list_name(n))
                .w(px(m.density.sidebar))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .min_h_0()
                .bg(color(t.bg_surface))
                .when(!rounded && m.surface != Surface::Mono, |d| {
                    d.border_r(px(m.border_width))
                        .border_color(color(t.border_subtle))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .min_h(px(header_h))
                        .flex_shrink_0()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(color(t.text_secondary))
                                .child(s::TASKS),
                        )
                        .child(div().text_color(color(t.text_muted)).child(n.to_string())),
                )
                .child(list),
        );

        // ── header of the canvas: branch → base, title, agent statuses, gates ──
        let selected = self.model.selected.clone();
        let (v1, v2) = (me.clone(), me.clone());
        let tabs = div()
            .id("review-canvas-tabs")
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .role(Role::TabList)
            .child(seg(
                "review-canvas-review",
                s::REVIEW_TAB,
                !self.model.show_dock,
                m,
                Rc::new(move |window, cx| {
                    let _ = v1.update(cx, |v, cx| v.show_canvas(false, window, cx));
                }),
            ))
            .child(seg(
                "review-canvas-panels",
                s::PANELS_TAB,
                self.model.show_dock,
                m,
                Rc::new(move |window, cx| {
                    let _ = v2.update(cx, |v, cx| v.show_canvas(true, window, cx));
                }),
            ));
        let mut agents = div()
            .id("review-agents")
            .qa_id(|| "review-agents".into())
            .role(Role::Group)
            .aria_label(s::AGENTS)
            .flex()
            .flex_wrap()
            .items_center()
            .gap_x_3()
            .gap_y_1()
            .min_w_0();
        for x in &parts.terms {
            let badge = crate::orch::terminal_badge(x);
            let (glyph, fg) = badge.glyph(mode);
            let w = who.get(&x.id).cloned().unwrap_or_default();
            agents = agents.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(crate::agent_kind::mark_el(
                        &format!("review-agent-{}", x.id),
                        &w.kind,
                        14.,
                    ))
                    .child(div().text_color(color(t.text_primary)).child(w.title()))
                    .child(pod_ui::icon_sized(glyph, 14., fg))
                    .child(div().text_color(color(fg)).child(badge.label())),
            );
        }
        let mut gate_rows = div().flex().flex_col().gap_1();
        for g in &parts.gates {
            let (ga, gr) = (g.id.clone(), g.id.clone());
            let (va, vr) = (me.clone(), me.clone());
            gate_rows = gate_rows.child(
                div()
                    .id(SharedString::from(format!("review-gate-{}", g.id)))
                    .qa_id({
                        let id = g.id.clone();
                        move || format!("review-gate-{id}")
                    })
                    .role(Role::Group)
                    .aria_label(s::gate_name(&g.title, &g.message))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(m.radius.control))
                    .bg(color(t.warning_bg))
                    .child(pod_ui::icon(pod_ui::Icon::ShieldQuestion, t.warning_fg))
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(color(t.text_primary))
                            .child(g.title.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .text_color(color(t.text_secondary))
                            .child(g.message.clone()),
                    )
                    .children(approve_buttons(g, va, ga))
                    .child(text_button(
                        SharedString::from(format!("review-reject-{}", g.id)),
                        s::REJECT,
                        Some(pod_ui::Icon::X),
                        false,
                        move |window, cx| {
                            let _ = vr.update(cx, |v, cx| {
                                v.resolve(gr.clone(), GateDecision::Reject, None, window, cx)
                            });
                        },
                    )),
            );
        }
        // A blocked agent without a gate: answer it in its terminal.
        for x in parts.asks() {
            let (va, id) = (me.clone(), x.id.clone());
            let w = who.get(&x.id).cloned().unwrap_or_default();
            gate_rows = gate_rows.child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded(px(m.radius.control))
                    .bg(color(t.warning_bg))
                    .child(pod_ui::icon(
                        pod_ui::Icon::MessageCircleQuestion,
                        t.warning_fg,
                    ))
                    .child(crate::agent_kind::mark_el(
                        &format!("review-ask-{}", x.id),
                        &w.kind,
                        14.,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(120.))
                            .text_color(color(t.text_primary))
                            .child(s::ask(&w.title(), &crate::orch::terminal_badge(x).label())),
                    )
                    .child(text_button(
                        SharedString::from(format!("review-answer-{id}")),
                        s::ANSWER,
                        Some(pod_ui::Icon::SquareTerminal),
                        false,
                        move |window, cx| {
                            let _ = va.update(cx, |v, cx| v.answer(&id, window, cx));
                        },
                    )),
            );
        }
        let Selected { task, terms, .. } = &parts;
        let header = div()
            .id("review-header")
            .qa_id(|| "review-header".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .px_3()
            .py_1()
            .border_b(px(m.border_width))
            .border_color(color(if m.surface == Surface::Mono {
                t.bg_surface
            } else {
                t.border_subtle
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_h(px(header_h))
                    .min_w_0()
                    .child(pod_ui::icon(
                        pod_ui::Icon::GitCompareArrows,
                        t.text_secondary,
                    ))
                    .when_some(task.as_ref(), |d, k| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(branch_line(k.branch.as_deref(), k.base.as_deref())),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(color(t.text_secondary))
                                .child(k.title.clone()),
                        )
                        .when_some(k.git_summary(), |d, g| {
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(color(t.text_muted))
                                    .child(g),
                            )
                        })
                    })
                    .child(div().flex_1())
                    .child(tabs),
            )
            .when(!terms.is_empty(), |d| d.child(agents))
            .when(parts.shows_gate_rows(), |d| d.child(gate_rows.pb_1()));

        // ── canvas: Review / dock / an expanded terminal ──
        let (tile_ids, hidden) = selected
            .as_deref()
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .map(|r| tiles(&r.terminals))
            .unwrap_or_default();
        let expanded = self.model.expanded.clone();
        let canvas: AnyElement = match self.model.canvas() {
            Canvas::Dock => div().flex_1().min_h_0().child(dock).into_any_element(),
            Canvas::Terminal(id) => {
                let panel = self.panel_for(&id, window, cx);
                match terms.iter().find(|x| x.id == id) {
                    Some(x) => render_tile(
                        me.clone(),
                        x,
                        &who.get(&id).cloned().unwrap_or_default(),
                        panel,
                        true,
                        header_h,
                    )
                    .into_any_element(),
                    None => div().into_any_element(),
                }
            }
            Canvas::Review(task) => match self.review_for(&task, window, cx) {
                Some(r) => {
                    // The canvas is the window minus this layout's task list: below the
                    // Review's own minimum (files + diff + comments) the file list goes
                    // first, then (< 560) the comment column becomes a drawer, so the
                    // column and 보내기 are never cut off (D4.6).
                    let canvas_w = canvas_width(
                        f32::from(window.viewport_size().width),
                        m.density.sidebar,
                        gap,
                    );
                    self.fit_review(&r, canvas_w, cx);
                    div()
                        .flex_1()
                        .min_h_0()
                        .size_full()
                        .child(r)
                        .into_any_element()
                }
                None => canvas_note(s::NO_REVIEW),
            },
            Canvas::NoTask => canvas_note(s::NO_TASKS),
        };
        // Drop tile panels the canvas no longer shows (their attach ends).
        let keep: HashSet<String> = tile_ids.iter().cloned().chain(expanded.clone()).collect();
        self.own_panels.retain(|id, _| keep.contains(id));

        // ── bottom: the task's terminals, docked small ──
        let strip_tiles = self.model.strip_tiles();
        let strip = (!terms.is_empty() || task.is_some()).then(|| {
            let vh = f32::from(window.viewport_size().height);
            let h = strip_height(strip_tiles && !tile_ids.is_empty(), vh, header_h);
            let open = self.model.strip_open;
            let toggle = me.clone();
            let mut row = div().flex().flex_1().min_h_0();
            if strip_tiles {
                for id in &tile_ids {
                    let panel = self.panel_for(id, window, cx);
                    if let Some(x) = terms.iter().find(|x| &x.id == id) {
                        row = row.child(render_tile(
                            me.clone(),
                            x,
                            &who.get(id).cloned().unwrap_or_default(),
                            panel,
                            false,
                            header_h,
                        ));
                    }
                }
            }
            div()
                .id("review-terminals")
                .qa_id(|| "review-terminals".into())
                .role(Role::Region)
                .aria_label(s::terminals_name(terms.len()))
                .h(px(h))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .bg(color(t.bg_surface))
                .border_t(px(m.border_width))
                .border_color(color(t.border_subtle))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .min_h(px(header_h))
                        .flex_shrink_0()
                        .when(strip_tiles && !tile_ids.is_empty(), |d| {
                            d.border_b(px(m.border_width))
                                .border_color(color(t.border_subtle))
                        })
                        .child(pod_ui::icon(pod_ui::Icon::SquareTerminal, t.text_secondary))
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(s::terminals_title(terms.len())),
                        )
                        .when(hidden > 0, |d| {
                            d.child(
                                div()
                                    .flex_shrink_0()
                                    .text_xs()
                                    .text_color(color(t.text_muted))
                                    .child(s::hidden_tiles(hidden)),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(color(t.text_muted))
                                .child(if terms.is_empty() {
                                    s::NO_TERMINALS
                                } else {
                                    s::TERMINALS_HINT
                                }),
                        )
                        .child(
                            pod_ui::IconButton::new(
                                "review-strip-toggle",
                                if open {
                                    pod_ui::Icon::ChevronDown
                                } else {
                                    pod_ui::Icon::ChevronUp
                                },
                                if open { s::STRIP_CLOSE } else { s::STRIP_OPEN },
                            )
                            .render(mode, move |_, cx| {
                                let _ = toggle.update(cx, |v, cx| v.toggle_strip(cx));
                            }),
                        ),
                )
                .when(strip_tiles, |d| d.child(row))
        });

        let main = island(
            div()
                .id("review-main")
                .qa_id(|| "review-main".into())
                .role(Role::Region)
                .aria_label(s::CANVAS)
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .min_h_0()
                .bg(color(t.bg_canvas))
                .child(header)
                .child(div().flex().flex_col().flex_1().min_h_0().child(canvas))
                .when_some(strip, |d, s| d.child(s)),
        );

        div()
            .id("review-layout")
            .qa_id(|| "review-layout".into())
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|v, _: &NextRegion, window, cx| v.cycle(true, window, cx)))
            .on_action(cx.listener(|v, _: &PrevRegion, window, cx| v.cycle(false, window, cx)))
            .on_action(cx.listener(|v, _: &ToggleExpand, window, cx| v.toggle_expand(window, cx)))
            .on_key_down(
                cx.listener(|v, e: &KeyDownEvent, window, cx| v.on_list_key(e, window, cx)),
            )
            .size_full()
            .flex()
            .min_h_0()
            .when(gap > 0., |d| d.gap(px(gap)).p(px(gap)))
            .text_color(color(t.text_primary))
            .child(left)
            .child(main)
            .into_any_element()
    }
}
