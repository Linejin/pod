//! The notifications dialog (Ctrl+Shift+B): requests that need the user,
//! then recent events.
use gpui_kit::component::WindowExt;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::qa_marks::QaId;
use crate::theme::mode;
use crate::workspace::Workspace;

impl Workspace {
    /// Notifications (docs/10 §D4.6): a flat 32px list, icon + title + time; requests that
    /// need the user first (Enter/click opens the terminal), then recent events. Rebuilt on
    /// every render, so a request answered while the dialog is open leaves the list.
    pub(crate) fn open_notifications(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ws = cx.entity().downgrade();
        window.open_dialog(cx, move |d: Dialog, _, cx| {
            let tk = crate::theme::tokens();
            let now = crate::orch::now_ms();
            let (blocked, events) = ws
                .upgrade()
                .map(|w| {
                    let w = w.read(cx);
                    let blocked: Vec<_> = w
                        .terminals
                        .iter()
                        .filter(|t| {
                            t.status
                                .as_ref()
                                .is_some_and(|s| s.state == crate::vm::AgentState::Blocked)
                        })
                        .cloned()
                        .collect();
                    let events: Vec<_> = w
                        .toasts
                        .iter()
                        .rev()
                        .map(|t| (t.id, t.kind, t.title.clone(), t.body.clone(), t.at))
                        .collect();
                    (blocked, events)
                })
                .unwrap_or_default();
            let mut body = div()
                .id("notification-list")
                .role(Role::List)
                .aria_label(crate::strings::NOTIFICATIONS)
                .max_h(px(420.))
                .overflow_y_scroll()
                .flex()
                .flex_col();
            for t in &blocked {
                let id = t.id.clone();
                let ws = ws.clone();
                let name = t.name.as_deref().unwrap_or(&t.id).to_string();
                let since = t.status.as_ref().map(|s| s.since).filter(|s| *s > 0);
                let open = move |window: &mut Window, cx: &mut App| {
                    window.close_dialog(cx);
                    let _ = ws.update(cx, |w, cx| w.open_terminal(&id, false, window, cx));
                };
                body = body.child(
                    notice_row(
                        format!("notice-{}", t.id),
                        pod_ui::Icon::ShieldQuestion,
                        tk.warning_fg,
                        name,
                        crate::orch::terminal_badge(t).label(),
                        since.map(|s| ago(now, s)),
                    )
                    .role(Role::Button)
                    .tab_index(0)
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(tk.bg_hover)))
                    .focus_visible(|s| pod_ui::focus_style(s, mode()))
                    .on_click({
                        let open = open.clone();
                        move |_, window, cx| open(window, cx)
                    })
                    .on_key_down(move |e, window, cx| {
                        if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                            open(window, cx);
                            cx.stop_propagation();
                        }
                    }),
                );
            }
            for (id, kind, title, detail, at) in &events {
                let (glyph, fg) = (kind.icon(), kind.fg(&tk));
                body = body.child(
                    notice_row(
                        format!("event-{id}"),
                        glyph,
                        fg,
                        title.to_string(),
                        detail.to_string(),
                        Some(ago(now, *at)),
                    )
                    .role(Role::ListItem),
                );
            }
            if blocked.is_empty() && events.is_empty() {
                body = body.child(
                    div()
                        .py_3()
                        .text_sm()
                        .text_color(rgb(tk.text_muted))
                        .child(crate::strings::NO_NOTIFICATIONS),
                );
            }
            d.title(crate::strings::NOTIFICATIONS)
                .w(px(560.))
                .child(body)
        });
    }
}

/// One 32px notification row: icon 16 + title (semibold) + muted detail + time on the right.
/// The accessible name carries all of it.
fn notice_row(
    id: String,
    glyph: pod_ui::Icon,
    fg: u32,
    title: String,
    detail: String,
    time: Option<String>,
) -> Stateful<Div> {
    let tk = crate::theme::tokens();
    let name = match &time {
        Some(t) => format!("{title} · {detail} · {t}"),
        None => format!("{title} · {detail}"),
    };
    div()
        .id(SharedString::from(id.clone()))
        .qa_id(move || id)
        .aria_label(name)
        .h(px(32.))
        .flex_shrink_0()
        .px_2()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .rounded(px(4.))
        .text_sm()
        .child(pod_ui::icon_sized(glyph, 16., fg))
        .child(
            div()
                .flex_shrink_0()
                .max_w(px(220.))
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(tk.text_primary))
                .child(title),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(rgb(tk.text_secondary))
                .child(detail),
        )
        .when_some(time, |d, t| {
            d.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(rgb(tk.text_muted))
                    .child(t),
            )
        })
}

/// `방금` / `N분 전` / `N시간 전` / `N일 전` for a millisecond timestamp.
pub(super) fn ago(now_ms: u64, at_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    match s {
        0..60 => crate::strings::JUST_NOW.into(),
        60..3600 => format!("{}분 전", s / 60),
        3600..86400 => format!("{}시간 전", s / 3600),
        _ => format!("{}일 전", s / 86400),
    }
}
