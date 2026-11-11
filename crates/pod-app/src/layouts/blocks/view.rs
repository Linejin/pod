//! Render of the 블록 터미널: the shell around the feed, the task sidebar with
//! the agent chips, the header, the blocks (or the raw terminal) and the
//! input. What is shown comes from [`Feed::sync_frame`] and the blocks model;
//! events call the feed's controller methods (`controller.rs`).
use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::controller::{Feed, Region, Shown};
use super::copy as s;
use super::{INPUT_CONTEXT, KEY_CONTEXT, LeaveInput, NextTarget, PrevTarget, SwitchRegion};
use crate::layouts::blocks_model::{
    Approval, FeedBlock, Kind, Origin, block_name, decision, drawer_width, status_text, title_of,
    who,
};
use crate::layouts::kit::{BodyFlow, ConnectionShell, LayoutMetrics, ToastAnchor, color};
use crate::qa_marks::QaId;
use crate::vm::{AgentState, TaskVm};

/// The block terminal's shell: the connection warning and the hook prompt
/// above the feed on the canvas colour, toasts top-right.
pub(super) fn shell(
    warning: Option<String>,
    hook_banner: Option<AnyElement>,
    feed: AnyElement,
    toasts: AnyElement,
) -> AnyElement {
    ConnectionShell {
        warning,
        hook_banner,
        canvas: Some(crate::theme::tokens().bg_canvas),
        flow: BodyFlow::Column,
        body: feed,
        toasts,
        anchor: ToastAnchor::TOP_RIGHT,
    }
    .render()
    .into_any_element()
}

/// Item height of a task row: grows with the UI font.
pub(super) fn row_height(metrics: &LayoutMetrics) -> f32 {
    pod_ui::hit_area().max(metrics.line() + 8.)
}

/// A text button (Enter/Space, Korean name); `key` is the shortcut hint shown after the label.
#[allow(clippy::too_many_arguments)]
fn text_button(
    id: impl Into<ElementId>,
    label: &'static str,
    key: Option<&'static str>,
    icon: Option<pod_ui::Icon>,
    primary: bool,
    enabled: bool,
    tab: bool,
    tip: Option<&'static str>,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let on = Rc::new(on);
    let keyed = on.clone();
    let (fg, bg, border) = if !enabled {
        (t.text_disabled, t.bg_surface, t.border_subtle)
    } else if primary {
        (
            t.action_primary_fg,
            t.action_primary_bg,
            t.action_primary_bg,
        )
    } else {
        // Mono has no lines: a secondary button needs a face of its own.
        (
            t.text_primary,
            if m.surface == pod_ui::themes::Surface::Mono {
                t.bg_hover
            } else {
                t.bg_surface
            },
            t.border_control,
        )
    };
    let name = match key {
        Some(k) => format!("{label} ({k})"),
        None => label.to_string(),
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(name)
        .when(enabled && tab, |d| d.tab_index(0))
        .when(!enabled || !tab, |d| d.tab_stop(false))
        .h(px(pod_ui::hit_area()))
        .px(px(10.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .rounded(px(m.radius.control))
        .border(px(m.border_width))
        .border_color(color(border))
        .bg(color(bg))
        .text_color(color(fg))
        .font_weight(FontWeight::SEMIBOLD)
        .when_some(icon, |d, i| d.child(pod_ui::icon_sized(i, 14., fg)))
        .child(label)
        .when_some(key, |d, k| {
            d.child(
                div()
                    .px_1()
                    .rounded(px(m.radius.control.min(4.)))
                    .border(px(1.))
                    .border_color(color(if primary { fg } else { t.border_subtle }))
                    .text_xs()
                    .font_weight(FontWeight::NORMAL)
                    .child(k),
            )
        })
        .when(enabled, |d| {
            d.cursor_pointer()
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
                        keyed(window, cx);
                    }
                })
        })
        .when_some(tip, |d, tip| {
            d.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
            })
        })
}

/// What one block element needs besides the block.
struct BlockCtx {
    feed: WeakEntity<Feed>,
    selected: Option<crate::layouts::blocks_model::BlockKey>,
    mono: SharedString,
}

fn status_icon(f: &FeedBlock, t: &pod_ui::Tokens) -> (pod_ui::Icon, u32) {
    let b = &f.block;
    if f.gate.is_some() {
        (pod_ui::Icon::ShieldQuestion, t.warning_fg)
    } else if b.kind == Kind::Tui {
        (
            pod_ui::Icon::Monitor,
            if b.open { t.info_fg } else { t.text_muted },
        )
    } else if b.open {
        (pod_ui::Icon::LoaderCircle, t.info_fg)
    } else if b.failed() {
        (pod_ui::Icon::OctagonX, t.danger_fg)
    } else if b.origin == Origin::Earlier {
        (pod_ui::Icon::Circle, t.text_muted)
    } else {
        (pod_ui::Icon::CircleCheck, t.success_fg)
    }
}

/// Body lines shown for an expanded block (the newest).
const SHOWN_LINES: usize = 200;

fn render_block(f: &FeedBlock, expanded: bool, ctx: &BlockCtx, focus: FocusHandle) -> AnyElement {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    use pod_ui::themes::Surface;
    let selected = ctx.selected.as_ref() == Some(&f.key);
    let dom = f.key.dom();
    let (glyph, glyph_fg) = status_icon(f, &t);
    let waiting = f.gate.is_some();
    let card_bg = match m.surface {
        Surface::Flat | Surface::Mono => t.bg_surface,
        _ => t.bg_raised,
    };
    let border = if waiting {
        t.warning_fg
    } else if selected {
        t.accent_fg
    } else if m.surface == Surface::Mono {
        card_bg
    } else {
        t.border_subtle
    };
    let feed = ctx.feed.clone();
    let mono = ctx.mono.clone();
    let act = |g: fn(&mut Feed, &FeedBlock, &mut Window, &mut Context<Feed>)| {
        let (feed, fb) = (feed.clone(), f.clone());
        move |window: &mut Window, cx: &mut App| {
            let _ = feed.update(cx, |me, cx| g(me, &fb, window, cx));
        }
    };

    // ── header ──
    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .px(px(12.))
        .min_h(px(m.density.panel_header.max(pod_ui::hit_area() + 4.)))
        .child(pod_ui::icon_sized(
            if expanded {
                pod_ui::Icon::ChevronDown
            } else {
                pod_ui::Icon::ChevronRight
            },
            14.,
            t.text_muted,
        ))
        .child(pod_ui::icon(glyph, glyph_fg))
        .child(crate::agent_kind::mark_el(
            &format!("blocks-{}", f.key.dom()),
            &f.kind,
            14.,
        ))
        .child(
            div()
                .flex_shrink_0()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color(t.text_primary))
                .child(f.who.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(mono.clone())
                .text_color(color(t.text_primary))
                .when(f.block.kind == Kind::Command, |d| {
                    d.font_weight(FontWeight::SEMIBOLD)
                })
                .child(title_of(f)),
        )
        .child(
            div()
                .qa_id({
                    let dom = dom.clone();
                    move || format!("blocks-status-{dom}")
                })
                .flex_shrink_0()
                .text_sm()
                .text_color(color(if f.block.failed() {
                    t.danger_fg
                } else if waiting {
                    t.warning_fg
                } else {
                    t.text_muted
                }))
                .child(status_text(f)),
        )
        .when(f.block.kind == Kind::Tui, |d| {
            d.child(text_button(
                SharedString::from(format!("blocks-full-{dom}")),
                s::FULL_TERMINAL,
                None,
                Some(pod_ui::Icon::Maximize2),
                false,
                true,
                selected,
                None,
                act(|me, fb, window, cx| me.show_raw(Some(fb.key.terminal.clone()), window, cx)),
            ))
        })
        .child({
            let b = pod_ui::IconButton::new(
                SharedString::from(format!("blocks-copy-{dom}")),
                pod_ui::Icon::Copy,
                s::COPY,
            )
            .tooltip(s::COPY_TIP)
            .render(mode, act(|me, fb, window, cx| me.copy(&fb.key, window, cx)));
            if selected { b } else { b.tab_stop(false) }
        });

    // ── body ──
    let body = expanded.then(|| {
        let lines = &f.block.lines;
        let skip = lines.len().saturating_sub(SHOWN_LINES);
        let omitted = skip + f.block.dropped;
        div()
            .flex()
            .flex_col()
            .px(px(12.))
            .py(px(8.))
            .border_t(px(m.border_width))
            .border_color(color(t.border_subtle))
            .font_family(mono.clone())
            .text_sm()
            .text_color(color(t.text_secondary))
            .when(omitted > 0, |d| {
                d.child(
                    div()
                        .text_color(color(t.text_muted))
                        .child(s::omitted(omitted)),
                )
            })
            .children(lines[skip..].iter().map(|l| {
                let (fg, text) = line_color(l, &t);
                div()
                    .min_h(px(4.))
                    .text_color(color(fg))
                    .child(if text.is_empty() {
                        " ".to_string()
                    } else {
                        text
                    })
            }))
            .when(lines.is_empty() && f.gate.is_none(), |d| {
                d.child(
                    div()
                        .text_color(color(t.text_muted))
                        .child(if f.block.open {
                            s::WAITING_FOR_OUTPUT
                        } else {
                            s::NO_OUTPUT
                        }),
                )
            })
    });

    // ── approval (gate) ──
    let approval = f.gate.clone().filter(|_| expanded).map(|g| {
        // Buttons stay focusable while the request runs (a second press is ignored).
        let always_ok = decision(&g, Approval::Always).is_some();
        let mk = |a: Approval| {
            let (feed, key, g) = (feed.clone(), f.key.clone(), g.clone());
            move |window: &mut Window, cx: &mut App| {
                let _ = feed.update(cx, |me, cx| {
                    me.resolve(key.clone(), g.clone(), a, window, cx)
                });
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px(px(12.))
            .pb(px(10.))
            .pt(px(if expanded && !f.block.lines.is_empty() {
                0.
            } else {
                8.
            }))
            .child(
                div()
                    .text_color(color(t.warning_fg))
                    .child(format!("{} · {}", g.title, g.message)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(text_button(
                        SharedString::from(format!("blocks-once-{dom}")),
                        s::ALLOW_ONCE,
                        Some("Y"),
                        Some(pod_ui::Icon::Check),
                        true,
                        true,
                        selected,
                        None,
                        mk(Approval::Once),
                    ))
                    .child(text_button(
                        SharedString::from(format!("blocks-always-{dom}")),
                        s::ALLOW_ALWAYS,
                        Some("A"),
                        None,
                        false,
                        always_ok,
                        selected,
                        (!always_ok).then_some(s::NO_ALWAYS_TIP),
                        mk(Approval::Always),
                    ))
                    .child(text_button(
                        SharedString::from(format!("blocks-reject-{dom}")),
                        s::REJECT,
                        Some("N"),
                        None,
                        false,
                        true,
                        selected,
                        None,
                        mk(Approval::Reject),
                    )),
            )
    });

    let name = block_name(f, expanded);
    let fb = f.clone();
    let key_feed = feed.clone();
    let click_feed = feed.clone();
    let click_key = f.key.clone();
    div()
        .id(SharedString::from(format!("blocks-block-{dom}")))
        .qa_id({
            let dom = dom.clone();
            move || format!("blocks-block-{dom}")
        })
        .track_focus(&focus.tab_stop(selected))
        .role(Role::ListItem)
        .aria_label(name)
        .aria_selected(selected)
        .aria_expanded(expanded)
        .w_full()
        // Full height in the scrolling feed: never squeezed (the body is never clipped).
        .flex_shrink_0()
        .flex()
        .flex_col()
        .relative()
        .overflow_hidden()
        .rounded(px(m.radius.card))
        .bg(color(if selected && !waiting {
            t.bg_selected
        } else {
            card_bg
        }))
        .border(px(if waiting || selected {
            m.border_width.max(2.)
        } else {
            m.border_width
        }))
        .border_color(color(border))
        // Waiting stripe: never the only carrier (icon + 승인 대기 text too).
        .when(waiting, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(4.))
                    .bg(color(t.warning_fg)),
            )
        })
        .when(
            m.shadow != pod_ui::themes::Shadow::None && m.surface != Surface::Flat,
            |d| d.shadow(pod_ui::tokens::elevation(1, mode)),
        )
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .child(
            div()
                .id(SharedString::from(format!("blocks-head-{dom}")))
                .cursor_pointer()
                .hover(move |s| s.bg(color(t.bg_hover)))
                .on_click(move |_, window, cx| {
                    let _ = click_feed.update(cx, |me, cx| {
                        me.select(&click_key, window, cx);
                        me.toggle(&click_key, cx);
                    });
                })
                .child(header),
        )
        .when_some(body, |d, b| d.child(b))
        .when_some(approval, |d, a| d.child(a))
        .on_key_down(move |e, window, cx| {
            if key_feed
                .update(cx, |me, cx| me.on_block_key(&fb, e, window, cx))
                .unwrap_or(false)
            {
                cx.stop_propagation();
            }
        })
        .into_any_element()
}

/// Diff-ish colors for body lines (`+`, `-`, `M`, `??`, `error`), text unchanged.
fn line_color(l: &str, t: &pod_ui::Tokens) -> (u32, String) {
    let trimmed = l.trim_start();
    let fg = if trimmed.starts_with("error") || trimmed.starts_with("오류") {
        t.danger_fg
    } else if trimmed.starts_with('+') && !trimmed.starts_with("++") {
        t.diff_add_fg
    } else if trimmed.starts_with('-') && !trimmed.starts_with("--") {
        t.diff_del_fg
    } else if trimmed.starts_with("warning") {
        t.warning_fg
    } else {
        t.text_secondary
    };
    (fg, l.to_string())
}

impl Render for Feed {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(Shown {
            terms,
            feed,
            focus,
            tasks,
            pending,
            labels: label_of,
            metrics,
            raw_panel,
            target_name,
            panels,
            dock,
        }) = self.sync_frame(window, cx)
        else {
            return div().into_any_element();
        };

        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        use pod_ui::themes::Surface;
        let gap = m.gap.max(8.);
        let me = cx.entity().downgrade();
        let mono = gpui_kit::component::theme::Theme::global(cx)
            .mono_font_family
            .clone();
        let task = self
            .task
            .as_deref()
            .and_then(|id| tasks.iter().find(|t| t.id == id))
            .cloned();
        let needs = |task: &TaskVm| -> usize {
            let blocked = task
                .terminals
                .iter()
                .filter(|id| {
                    terms.iter().chain(std::iter::empty()).any(|x| {
                        &x.id == *id
                            && x.status
                                .as_ref()
                                .is_some_and(|s| s.state == AgentState::Blocked)
                    })
                })
                .count();
            let gates = pending
                .iter()
                .filter(|g| {
                    g.task.as_deref() == Some(task.id.as_str())
                        || g.terminal
                            .as_ref()
                            .is_some_and(|x| task.terminals.contains(x))
                })
                .count();
            gates.max(blocked)
        };

        // ── sidebar: tasks + agent chips ──
        let row_h = row_height(&metrics);
        let side_bg = match m.surface {
            Surface::Flat | Surface::Mono => t.bg_surface,
            _ => t.bg_surface,
        };
        let task_rows = {
            let me = me.clone();
            let tasks = Rc::new(tasks.clone());
            let focus = self.task_focus.clone();
            let current = self.task.clone();
            let counts: HashMap<String, usize> = tasks
                .iter()
                .map(|k| {
                    (
                        k.id.clone(),
                        pending
                            .iter()
                            .filter(|g| {
                                g.task.as_deref() == Some(k.id.as_str())
                                    || g.terminal.as_ref().is_some_and(|x| k.terminals.contains(x))
                            })
                            .count(),
                    )
                })
                .collect();
            uniform_list("blocks-tasks", tasks.len(), move |range, _, _| {
                range
                    .map(|i| {
                        let k = &tasks[i];
                        let on = current.as_deref() == Some(k.id.as_str());
                        let n = counts.get(&k.id).copied().unwrap_or(0);
                        let dot = match k.status.as_ref().map(|s| s.state) {
                            Some(AgentState::Blocked) => t.warning_fg,
                            Some(AgentState::Error) => t.danger_fg,
                            Some(AgentState::Working) => t.info_fg,
                            Some(AgentState::Done) => t.success_fg,
                            _ => t.text_muted,
                        };
                        let (me1, me2, id1, id2) =
                            (me.clone(), me.clone(), k.id.clone(), k.id.clone());
                        let name = s::task_row(&k.title, n);
                        div()
                            .id(SharedString::from(format!("blocks-task-{}", k.id)))
                            .qa_id({
                                let id = k.id.clone();
                                move || format!("blocks-task-{id}")
                            })
                            .when_some(focus.get(&k.id).cloned(), |d, f| {
                                d.track_focus(&f.tab_stop(on))
                            })
                            .role(Role::ListItem)
                            .aria_label(name)
                            .aria_selected(on)
                            .h(px(row_h))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(m.radius.control))
                            .when(on, |d| {
                                d.bg(color(t.bg_selected)).font_weight(FontWeight::SEMIBOLD)
                            })
                            .hover(move |s| if on { s } else { s.bg(color(t.bg_hover)) })
                            .focus_visible(move |s| pod_ui::focus_style(s, mode))
                            .cursor_pointer()
                            .child(
                                div()
                                    .size(px(8.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(color(dot)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(color(t.text_primary))
                                    .child(k.title.clone()),
                            )
                            .when(n > 0, |d| {
                                d.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_1()
                                        .rounded(px(m.radius.control))
                                        .bg(color(t.warning_bg))
                                        .text_color(color(t.warning_fg))
                                        .text_xs()
                                        .child(n.to_string()),
                                )
                            })
                            .on_click(move |_, window, cx| {
                                let _ = me1.update(cx, |f, cx| f.pick_task(&id1, window, cx));
                            })
                            .on_key_down(move |e, window, cx| {
                                if me2
                                    .update(cx, |f, cx| f.on_task_key(&id2, e, window, cx))
                                    .unwrap_or(false)
                                {
                                    cx.stop_propagation();
                                }
                            })
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.task_scroll)
            .flex_1()
            .min_h_0()
        };
        let chip = |id: SharedString, label: String, on: bool, value: Option<String>| {
            let me = me.clone();
            let (v1, v2) = (value.clone(), value);
            let me2 = me.clone();
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(s::chip(&label, on))
                .aria_selected(on)
                .tab_index(0)
                .min_h(px(pod_ui::hit_area()))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(m.radius.control.max(10.)))
                .border(px(m.border_width))
                .border_color(color(if on { t.accent_fg } else { t.border_subtle }))
                .when(on, |d| {
                    d.bg(color(t.bg_selected)).font_weight(FontWeight::SEMIBOLD)
                })
                .text_sm()
                .text_color(color(if on { t.text_primary } else { t.text_secondary }))
                .hover(|s| s.bg(color(t.bg_hover)))
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .cursor_pointer()
                .child(label)
                .on_click(move |_, _, cx| {
                    let _ = me.update(cx, |f, cx| f.set_filter(v1.clone(), cx));
                })
                .on_key_down(move |e, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space")
                        && !e.keystroke.modifiers.modified()
                    {
                        cx.stop_propagation();
                        let _ = me2.update(cx, |f, cx| f.set_filter(v2.clone(), cx));
                    }
                })
        };
        let mut chips = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .px(px(gap))
            .py(px(gap))
            .child(chip(
                "blocks-filter-all".into(),
                s::all_agents(terms.len()),
                self.filter.is_none(),
                None,
            ));
        for x in &terms {
            let label = who(x, label_of.get(&x.id).map(String::as_str).unwrap_or(&x.id));
            chips = chips.child(chip(
                SharedString::from(format!("blocks-filter-{}", x.id)),
                label,
                self.filter.as_deref() == Some(x.id.as_str()),
                Some(x.id.clone()),
            ));
        }
        let sidebar = div()
            .id("blocks-sidebar")
            .role(Role::Navigation)
            .aria_label(s::TASKS)
            .w(px(m.density.sidebar.clamp(220., 320.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(color(side_bg))
            .border_r(px(m.border_width))
            .border_color(color(t.border_subtle))
            .child(
                div()
                    .px(px(gap + 4.))
                    .min_h(px(m.density.panel_header.max(36.)))
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::TASKS),
            )
            .child(
                div()
                    .id("blocks-task-list")
                    .role(Role::List)
                    .aria_label(s::task_list(tasks.len()))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .child(task_rows),
            )
            .child(
                div()
                    .border_t(px(m.border_width))
                    .border_color(color(t.border_subtle))
                    .child(chips),
            );

        // ── header ──
        let needs_n = task.as_ref().map_or(0, needs);
        let meta = s::meta(feed.len(), terms.len());
        let raw_me = me.clone();
        let raw = self.raw;
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .px(px(gap + 8.))
            .min_h(px(m.density.panel_header.max(40.)))
            .border_b(px(m.border_width))
            .border_color(color(t.border_subtle))
            .child(
                // Shrinks (with an ellipsis) before the buttons do: beside
                // the panel drawer the feed column is narrow.
                div()
                    .min_w_0()
                    .max_w(px(420.))
                    .truncate()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        task.as_ref()
                            .map_or(s::NO_TASK.to_string(), |k| k.title.clone()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(color(t.text_muted))
                    .child(meta),
            )
            .when(needs_n > 0, |d| {
                d.child(
                    div()
                        .qa_id(|| "blocks-needs".into())
                        .flex_shrink_0()
                        .px_2()
                        .rounded(px(m.radius.control))
                        .bg(color(t.warning_bg))
                        .text_color(color(t.warning_fg))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(s::needs_you(needs_n)),
                )
            })
            .child(
                pod_ui::IconButton::new(
                    "blocks-raw",
                    if raw {
                        pod_ui::Icon::Rows2
                    } else {
                        pod_ui::Icon::SquareTerminal
                    },
                    if raw { s::RAW_OFF } else { s::RAW_ON },
                )
                .render(mode, move |window, cx| {
                    let _ = raw_me.update(cx, |f, cx| f.toggle_raw(window, cx));
                }),
            )
            // The panel drawer's button, while the dock has panels to show.
            .when(panels > 0 || self.drawer_open, |d| {
                let drawer_me = me.clone();
                d.child(
                    pod_ui::IconButton::new(
                        "blocks-drawer-toggle",
                        pod_ui::Icon::PanelRight,
                        if self.drawer_open {
                            s::DRAWER_CLOSE
                        } else {
                            s::DRAWER_OPEN
                        },
                    )
                    .render(mode, move |window, cx| {
                        let _ = drawer_me.update(cx, |f, cx| f.toggle_drawer(window, cx));
                    })
                    .qa_id(|| "blocks-drawer-toggle".into()),
                )
            });

        // ── feed or raw terminal ──
        let body = if self.raw {
            match raw_panel {
                Some(p) => div()
                    .id("blocks-raw-view")
                    .qa_id(|| "blocks-raw-view".into())
                    .flex_1()
                    .min_h_0()
                    .child(p)
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(color(t.text_muted))
                    .child(s::RAW_EMPTY)
                    .into_any_element(),
            }
        } else {
            let ctx = BlockCtx {
                feed: me.clone(),
                selected: self.selected.clone(),
                mono: mono.clone(),
            };
            let list = div()
                .id("blocks-feed")
                .qa_id(|| "blocks-feed".into())
                .role(Role::List)
                .aria_label(s::feed(feed.len()))
                .track_focus(&self.focus.clone().tab_stop(feed.is_empty()))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .flex()
                .flex_col()
                .gap(px(gap))
                .p(px(gap + 4.))
                .when(feed.is_empty(), |d| {
                    d.child(
                        div()
                            .pt_4()
                            .text_color(color(t.text_muted))
                            .child(s::NO_BLOCKS),
                    )
                })
                .children(
                    feed.iter()
                        .zip(focus)
                        .map(|(f, h)| render_block(f, self.expanded(f), &ctx, h)),
                );
            list.into_any_element()
        };

        // ── input ──
        let input = self.input.clone();
        let input_row =
            div()
                .id("blocks-input")
                .qa_id(|| "blocks-input".into())
                .key_context(INPUT_CONTEXT)
                .on_action(cx.listener(|f, _: &NextTarget, _, cx| f.cycle(false, cx)))
                .on_action(cx.listener(|f, _: &PrevTarget, _, cx| f.cycle(true, cx)))
                .on_action(cx.listener(|f, _: &LeaveInput, window, cx| {
                    f.focus_region(Region::Feed, window, cx)
                }))
                .role(Role::Group)
                .aria_label(s::input_row(target_name.as_deref().unwrap_or(s::NO_TARGET)))
                .flex_shrink_0()
                .m(px(gap + 4.))
                .mt(px(0.))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(px(m.radius.card))
                .border(px(m.border_width.max(1.)))
                .border_color(color(t.border_control))
                .bg(color(match m.surface {
                    Surface::Flat | Surface::Mono => t.bg_surface,
                    _ => t.bg_raised,
                }))
                .child(pod_ui::icon(pod_ui::Icon::SquareTerminal, t.text_muted))
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&input)
                            .appearance(false)
                            .disabled(target_name.is_none()),
                    ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .px_1()
                        .rounded(px(m.radius.control.min(4.)))
                        .border(px(1.))
                        .border_color(color(t.border_subtle))
                        .text_xs()
                        .text_color(color(t.text_muted))
                        .child("Enter"),
                );

        let main = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header)
            .child(body)
            .when(!self.raw, |d| d.child(input_row));

        // ── panel drawer: the host's dock at the right ──
        let drawer = self.drawer_open.then(|| {
            let width = drawer_width(
                f32::from(window.viewport_size().width),
                m.density.sidebar.clamp(220., 320.),
            );
            let (esc_me, close_me) = (me.clone(), me.clone());
            div()
                .id("blocks-drawer")
                .qa_id(|| "blocks-drawer".into())
                .track_focus(&self.drawer_focus)
                .role(Role::Region)
                .aria_label(s::DRAWER_REGION)
                .w(px(width))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .min_h_0()
                .bg(color(t.bg_surface))
                .border_l(px(m.border_width))
                .border_color(color(t.border_subtle))
                // Esc that nothing inside used (a terminal keeps its Esc).
                .on_key_down(move |e, window, cx| {
                    if e.keystroke.key == "escape" && !e.keystroke.modifiers.modified() {
                        cx.stop_propagation();
                        let _ = esc_me.update(cx, |f, cx| f.close_drawer(window, cx));
                    }
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .min_h(px(m.density.panel_header.max(40.)))
                        .flex_shrink_0()
                        .border_b(px(m.border_width))
                        .border_color(color(t.border_subtle))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(s::DRAWER),
                        )
                        .child(
                            pod_ui::IconButton::new(
                                "blocks-drawer-close",
                                pod_ui::Icon::X,
                                s::DRAWER_CLOSE,
                            )
                            .render(mode, move |window, cx| {
                                let _ = close_me.update(cx, |f, cx| f.close_drawer(window, cx));
                            }),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .when_some(dock.clone(), |d, dock| d.child(dock)),
                )
        });

        div()
            .id("blocks-layout")
            .qa_id(|| "blocks-layout".into())
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|f, _: &SwitchRegion, window, cx| f.switch_region(window, cx)))
            .on_key_down(cx.listener(|f, e: &KeyDownEvent, window, cx| {
                // The empty feed itself: arrows reach the newest block.
                if f.focus.is_focused(window)
                    && matches!(e.keystroke.key.as_str(), "up" | "down" | "home" | "end")
                {
                    f.focus_region(Region::Feed, window, cx);
                }
                // Esc in the raw terminal view goes back to the blocks only from its header; the terminal keeps Esc.
            }))
            .size_full()
            .flex()
            .min_h_0()
            .text_color(color(t.text_primary))
            .child(sidebar)
            .child(main)
            .when_some(drawer, |d, dr| d.child(dr))
            .into_any_element()
    }
}
