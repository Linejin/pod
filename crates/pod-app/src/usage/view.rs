//! Drawing the monitor: the status-bar entry and its details popover.
//!
//! - The entry is one xsmall ghost button (one tab stop) whose accessible
//!   name says everything the painted text says; it is not a live region, so
//!   the minute-by-minute countdown is never announced.
//! - Width ([`EntryPlan`]): each service with its labelled window and
//!   countdown (wide), percentage and countdown (medium), or the single most
//!   constrained window, named by its service mark (compact, 800 px).
//!   Segments have fixed minimum widths so the countdown never shifts the bar.
//! - Details: per service its windows (5 h, weekly, extra model buckets),
//!   status, last sync, source and plan; refresh, Claude setup copy and
//!   "기록 지우기"; detected services without a quota source.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::popover::Popover;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::UsageMonitor;
use super::copy;
use super::model::{self, ServiceView, Tone, WindowView};
use crate::qa_marks::QaId;

/// How much the entry shows at a window width (logical px).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryPlan {
    /// `[mark] 5h 76% · 2시간 13분` per service.
    Full,
    /// `[mark] 76% · 2시간 13분` per service.
    Medium,
    /// `[mark] 76%`: the most constrained window only.
    Compact,
}

impl EntryPlan {
    pub fn for_width(width: f32) -> Self {
        if width >= 1400. {
            Self::Full
        } else if width >= 1100. {
            Self::Medium
        } else {
            Self::Compact
        }
    }

    /// Fixed minimum width of one service segment (no churn as text changes).
    fn segment_min(self) -> f32 {
        match self {
            Self::Full => 132.,
            Self::Medium => 96.,
            Self::Compact => 44.,
        }
    }
}

fn medium_text(v: &ServiceView) -> String {
    match v.representative.map(|i| &v.windows[i]) {
        Some(w) => match &w.countdown {
            Some(c) => format!("{}% · {c}", w.remaining.round()),
            None => format!("{}%", w.remaining.round()),
        },
        None => v.status.clone(),
    }
}

/// The entry's text per service, as painted: `(service, text, tone)`.
pub fn entry_segments(
    views: &[ServiceView],
    plan: EntryPlan,
) -> Vec<(model::ServiceId, String, Tone)> {
    match plan {
        EntryPlan::Compact => {
            let agg = model::aggregate(views);
            vec![(
                agg.service.unwrap_or_else(|| {
                    views.first().map_or(model::ServiceId::Codex, |v| v.service)
                }),
                agg.text,
                agg.tone,
            )]
        }
        _ => views
            .iter()
            .map(|v| {
                let text = if plan == EntryPlan::Full {
                    v.compact()
                } else {
                    medium_text(v)
                };
                (v.service, text, v.entry_tone())
            })
            .collect(),
    }
}

impl Render for UsageMonitor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now();
        let views = self.views(now);
        let plan = EntryPlan::for_width(f32::from(window.viewport_size().width));
        let tk = crate::theme::tokens();
        let agg = model::aggregate(&views);
        let name = copy::entry_name(&agg.sentence);
        let segments = entry_segments(&views, plan);
        let min = plan.segment_min();
        let row =
            div()
                .flex()
                .items_center()
                .gap_2()
                .children(
                    segments
                        .into_iter()
                        .enumerate()
                        .map(|(i, (service, text, tone))| {
                            div()
                                .min_w(px(min))
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .gap_1()
                                .child(pod_ui::agent_mark(service.mark(), 12., &tk))
                                .child(
                                    div()
                                        .text_color(rgb(tone.color(&tk)))
                                        .whitespace_nowrap()
                                        .child(text),
                                )
                                .qa_id(move || format!("status-usage-segment-{i}"))
                        }),
                );
        let me = cx.entity();
        div().qa_id(|| "status-usage".into()).flex_shrink_0().child(
            Popover::new("usage-details")
                .anchor(Anchor::BottomRight)
                .trigger(
                    Button::new("status-usage")
                        .xsmall()
                        .ghost()
                        .accessibility_label(name)
                        .tooltip(copy::ENTRY_TOOLTIP)
                        .child(row),
                )
                .content(move |_, _, cx| details(&me, cx)),
        )
    }
}

fn bar(remaining: f64, tone: Tone, tk: &pod_ui::Tokens) -> Div {
    let frac = (remaining / 100.0).clamp(0.0, 1.0) as f32;
    div()
        .h(px(4.))
        .w_full()
        .rounded_full()
        .bg(rgb(tk.border_subtle))
        .child(
            div()
                .h_full()
                .rounded_full()
                .w(relative(frac))
                .bg(rgb(match tone {
                    Tone::Normal => tk.success_fg,
                    t => t.color(tk),
                })),
        )
}

fn window_row(id: String, w: &WindowView, tk: &pod_ui::Tokens) -> Stateful<Div> {
    let sentence = format!("{} {}, {}", w.label, w.value, w.reset);
    div()
        .id(SharedString::from(id))
        .role(Role::Group)
        .aria_label(sentence)
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().truncate().child(w.label.clone()))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(w.tone.color(tk)))
                        .child(w.value.clone()),
                ),
        )
        .child(bar(w.remaining, w.tone, tk))
        .child(div().text_color(rgb(tk.text_muted)).child(w.reset.clone()))
}

fn service_card(
    i: usize,
    v: &ServiceView,
    me: &Entity<UsageMonitor>,
    cx: &App,
    tk: &pod_ui::Tokens,
) -> Stateful<Div> {
    let m = me.read(cx);
    let title = match &v.profile {
        Some(p) => format!("{} · {} {p}", v.service.name(), copy::PROFILE),
        None => v.service.name().to_string(),
    };
    let meta: Vec<String> = [
        v.synced.clone(),
        v.plan.as_ref().map(|p| format!("{} {p}", copy::PLAN)),
    ]
    .into_iter()
    .flatten()
    .collect();
    let is_claude = v.service == model::ServiceId::Claude;
    let claude_connected = m.claude_collector().is_some_and(|c| c.connected());
    let setup = m
        .claude_setup()
        .filter(|_| is_claude && i_is_first_claude(i, me, cx));
    let other_statusline = matches!(
        m.claude_collector(),
        Some(super::claude::Collector::Other(_))
    );
    let can_forget = is_claude && m.has_claude_records() && i_is_first_claude(i, me, cx);
    div()
        .id(SharedString::from(format!("usage-card-{i}")))
        .qa_id(move || format!("usage-card-{i}"))
        .role(Role::Group)
        .aria_label(v.sentence())
        .flex()
        .flex_col()
        .gap_2()
        .py_2()
        .when(i > 0, |d| {
            d.border_t_1().border_color(rgb(tk.border_subtle))
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(pod_ui::agent_mark(v.service.mark(), 14., tk))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(tk.text_primary))
                        .child(title),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_color(rgb(v.tone.color(tk)))
                        .child(v.status.clone()),
                ),
        )
        .children(
            v.windows
                .iter()
                .enumerate()
                .map(|(j, w)| window_row(format!("usage-window-{i}-{j}"), w, tk)),
        )
        .when_some(v.hint.clone(), |d, h| {
            d.child(div().text_color(rgb(tk.text_muted)).child(h))
        })
        .when(!meta.is_empty(), |d| {
            d.child(div().text_color(rgb(tk.text_muted)).child(meta.join(" · ")))
        })
        .when_some(v.source, |d, s| {
            d.child(
                div()
                    .text_color(rgb(tk.text_muted))
                    .child(format!("{}: {s}", copy::SOURCE)),
            )
        })
        .when(is_claude && claude_connected, |d| {
            d.child(
                div()
                    .text_color(rgb(tk.text_muted))
                    .child(copy::HINT_CLAUDE_DEPENDS),
            )
        })
        .when(is_claude && other_statusline && !claude_connected, |d| {
            d.child(
                div()
                    .text_color(rgb(tk.text_muted))
                    .child(copy::HINT_CLAUDE_OTHER_STATUSLINE),
            )
        })
        .when(setup.is_some() || can_forget, |d| {
            let setup_btn = setup.clone().map(|json| {
                let ws = cx
                    .try_global::<crate::workspace::AppGlobals>()
                    .map(|g| g.workspace.clone());
                Button::new(SharedString::from(format!("usage-copy-setup-{i}")))
                    .label(copy::COPY_SETUP)
                    .icon(pod_ui::kit_icon(pod_ui::Icon::Copy))
                    .tooltip(copy::SETUP_COMMAND)
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(json.clone()));
                        if let Some(ws) = ws.as_ref().and_then(WeakEntity::upgrade) {
                            ws.update(cx, |w, cx| {
                                w.raise_toast(
                                    crate::workspace::ToastKind::Info,
                                    copy::COPIED_SETUP,
                                    copy::COPIED_SETUP_BODY,
                                    cx,
                                )
                            });
                        }
                    })
            });
            let forget_btn = can_forget.then(|| {
                let me = me.clone();
                Button::new(SharedString::from(format!("usage-forget-{i}")))
                    .label(copy::FORGET)
                    .xsmall()
                    .ghost()
                    .on_click(move |_, _, cx| me.update(cx, |m, cx| m.forget_claude(cx)))
            });
            d.child(
                div()
                    .flex()
                    .gap_1()
                    .children(setup_btn)
                    .children(forget_btn),
            )
        })
}

/// Setup and forget act on all Claude profiles: show them once.
fn i_is_first_claude(i: usize, me: &Entity<UsageMonitor>, cx: &App) -> bool {
    let m = me.read(cx);
    let views = m.views(m.now());
    views
        .iter()
        .position(|v| v.service == model::ServiceId::Claude)
        == Some(i)
}

/// Agents running in Pod's terminals that have no documented quota source.
fn unsupported_detected(cx: &App) -> Vec<String> {
    use crate::agent_kind::{AgentKind, identify_in};
    let Some(ws) = cx
        .try_global::<crate::workspace::AppGlobals>()
        .and_then(|g| g.workspace.upgrade())
    else {
        return Vec::new();
    };
    let w = ws.read(cx);
    let mut names: Vec<String> = w
        .terminals
        .iter()
        .filter_map(|t| match identify_in(t, &w.tasks).kind {
            AgentKind::Gemini => Some("Gemini".to_string()),
            AgentKind::Aider => Some("Aider".to_string()),
            AgentKind::OtherAgent(n) => Some(n),
            _ => None,
        })
        .collect();
    names.sort();
    names.dedup();
    names.truncate(6);
    names
}

fn details(me: &Entity<UsageMonitor>, cx: &App) -> AnyElement {
    let tk = crate::theme::tokens();
    let m = me.read(cx);
    let views = m.views(m.now());
    let reading = m.reading();
    let others = unsupported_detected(cx);
    let refresh = {
        let me = me.clone();
        Button::new("usage-refresh")
            .label(if reading {
                copy::REFRESHING
            } else {
                copy::REFRESH
            })
            .icon(pod_ui::kit_icon(pod_ui::Icon::RefreshCw))
            .loading(reading)
            .xsmall()
            .ghost()
            .on_click(move |_, _, cx| me.update(cx, |m, cx| m.refresh(cx)))
    };
    div()
        .id("usage-details-body")
        .qa_id(|| "usage-details".into())
        .w(px(340.))
        .max_h(px(520.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .text_xs()
        .text_color(rgb(tk.text_secondary))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .pb_1()
                .child(
                    div()
                        .flex_1()
                        .text_sm()
                        .text_color(rgb(tk.text_primary))
                        .child(copy::TITLE),
                )
                .child(refresh),
        )
        .children(
            views
                .iter()
                .enumerate()
                .map(|(i, v)| service_card(i, v, me, cx, &tk)),
        )
        .when(!others.is_empty(), |d| {
            d.child(
                div()
                    .id("usage-others")
                    .qa_id(|| "usage-others".into())
                    .role(Role::Group)
                    .aria_label(format!(
                        "{}: {} — {}",
                        copy::OTHER_SERVICES,
                        others.join(", "),
                        copy::UNSUPPORTED_SERVICE
                    ))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(tk.border_subtle))
                    .child(
                        div()
                            .text_color(rgb(tk.text_primary))
                            .child(copy::OTHER_SERVICES),
                    )
                    .children(others.iter().map(|n| {
                        div()
                            .flex()
                            .gap_2()
                            .child(div().flex_shrink_0().child(n.clone()))
                            .child(
                                div()
                                    .text_color(rgb(tk.text_muted))
                                    .child(copy::UNSUPPORTED_SERVICE),
                            )
                    })),
            )
        })
        .child(
            div()
                .pt_1()
                .text_color(rgb(tk.text_muted))
                .child(copy::NOT_SUMMED),
        )
        .into_any_element()
}
