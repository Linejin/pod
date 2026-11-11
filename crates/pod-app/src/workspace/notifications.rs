//! Toasts: raising them, how each kind looks and how long it stays, and
//! the stack in the corner.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::*;

use super::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

impl ToastKind {
    /// Seconds an info or success toast stays, counted only while neither the
    /// pointer nor keyboard focus is on the toasts; warnings and errors stay
    /// until closed.
    pub(crate) fn ttl(self) -> Option<f64> {
        match self {
            Self::Info => Some(7.),
            Self::Success => Some(5.),
            Self::Warning | Self::Error => None,
        }
    }

    /// Its colour token: the toast's edge, the notification row's icon.
    pub(crate) fn fg(self, tk: &pod_ui::Tokens) -> u32 {
        match self {
            Self::Info => tk.info_fg,
            Self::Success => tk.success_fg,
            Self::Warning => tk.warning_fg,
            Self::Error => tk.danger_fg,
        }
    }

    /// Its icon in the notifications list.
    pub(crate) fn icon(self) -> pod_ui::Icon {
        match self {
            Self::Info => pod_ui::Icon::Info,
            Self::Success => pod_ui::Icon::CircleCheck,
            Self::Warning => pod_ui::Icon::TriangleAlert,
            Self::Error => pod_ui::Icon::OctagonX,
        }
    }

    /// Warnings and errors interrupt a screen reader (alert); the rest are status.
    pub(crate) fn role(self) -> Role {
        if matches!(self, Self::Warning | Self::Error) {
            Role::Alert
        } else {
            Role::Status
        }
    }
}

pub(crate) struct Toast {
    pub(crate) id: u64,
    visible: bool,
    paused: Rc<Cell<bool>>,
    pub(crate) kind: ToastKind,
    pub(crate) title: SharedString,
    pub(crate) body: SharedString,
    pub(super) terminal_request: Option<String>,
    /// Raised at (ms since the epoch), shown as `N분 전` in 알림.
    pub(crate) at: u64,
}

impl Workspace {
    // ---- notifications --------------------------------------------------

    /// Show a toast in the bottom-right corner for a few seconds.
    pub fn raise_toast(
        &mut self,
        kind: ToastKind,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let title = title.into();
        let body = body.into();
        if self
            .toasts
            .iter()
            .any(|t| t.visible && t.kind == kind && t.title == title && t.body == body)
        {
            return;
        }
        let id = self.next_toast;
        self.next_toast += 1;
        let paused = Rc::new(Cell::new(false));
        self.toasts.push(Toast {
            id,
            kind,
            title,
            body,
            visible: true,
            paused: paused.clone(),
            terminal_request: None,
            at: crate::orch::now_ms(),
        });
        if self.toasts.len() > 100 {
            self.toasts.remove(0);
        }
        if let Some(ttl) = kind.ttl() {
            let focus_paused = self.toast_focus_paused.clone();
            cx.spawn(async move |this, cx| {
                let mut elapsed = 0.;
                while elapsed < ttl {
                    cx.background_executor()
                        .timer(Duration::from_millis(250))
                        .await;
                    if !paused.get() && !focus_paused.get() {
                        elapsed += 0.25;
                    }
                    if this.upgrade().is_none() {
                        return;
                    }
                }
                let _ = this.update(cx, |w, cx| {
                    if let Some(t) = w.toasts.iter_mut().find(|t| t.id == id) {
                        t.visible = false;
                    }
                    cx.notify();
                });
            })
            .detach();
        }
        cx.notify();
    }

    /// Titles of the toasts still on screen (tests).
    #[cfg(test)]
    pub(crate) fn visible_toasts(&self) -> Vec<String> {
        self.toasts
            .iter()
            .filter(|t| t.visible)
            .map(|t| t.title.to_string())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn toast_focus_handle(&self) -> FocusHandle {
        self.toast_focus.clone()
    }

    pub(crate) fn render_toasts(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .max_h(px(240.))
            .id("toast-stack")
            .role(Role::List)
            .aria_label("최근 알림")
            .track_focus(&self.toast_focus)
            .tab_index(0)
            .focus_visible(|s| pod_ui::focus_style(s, crate::theme::mode()))
            .overflow_y_scroll()
            .children(
                self.toasts
                    .iter()
                    .rev()
                    .filter(|t| t.visible)
                    .take(3)
                    .map(|t| {
                        let paused = t.paused.clone();
                        let id = t.id;
                        let accent = rgb(t.kind.fg(&crate::theme::tokens()));
                        div()
                            .id(("toast", t.id as usize))
                            .role(t.kind.role())
                            .aria_label(format!("{}: {}", t.title, t.body))
                            .flex()
                            .flex_col()
                            .gap_1()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(crate::theme::tokens().bg_raised))
                            .border_l_4()
                            .border_color(accent)
                            .on_hover(move |hover, _, _| paused.set(*hover))
                            .shadow_lg()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .child(
                                        div()
                                            .flex_1()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(t.title.clone()),
                                    )
                                    .child({
                                        // Close: a 28px icon button (docs/10 §D4.6), a tab stop inside the
                                        // stack, so keyboard focus there pauses the timers too.
                                        let ws = cx.entity().downgrade();
                                        pod_ui::IconButton::new(
                                            ("dismiss", id as usize),
                                            pod_ui::Icon::X,
                                            crate::strings::CLOSE,
                                        )
                                        .render(
                                            crate::theme::mode(),
                                            move |_, cx| {
                                                let _ = ws.update(cx, |w, cx| {
                                                    if let Some(t) =
                                                        w.toasts.iter_mut().find(|t| t.id == id)
                                                    {
                                                        t.visible = false;
                                                    }
                                                    cx.notify();
                                                });
                                            },
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(rgb(crate::theme::tokens().text_secondary))
                                    .child(t.body.clone()),
                            )
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use gpui_kit::Role;

    use super::ToastKind::{self, *};

    const KINDS: [ToastKind; 4] = [Info, Success, Warning, Error];

    /// Info 7 s, success 5 s, warnings and errors until closed; each kind's
    /// colour, icon and screen-reader role.
    #[test]
    fn toast_kinds_time_out_colour_and_announce() {
        let tk = pod_ui::Tokens::for_mode(pod_ui::Mode::Dark);
        assert_eq!(KINDS.map(ToastKind::ttl), [Some(7.), Some(5.), None, None]);
        assert_eq!(
            KINDS.map(|k| k.fg(&tk)),
            [tk.info_fg, tk.success_fg, tk.warning_fg, tk.danger_fg]
        );
        assert_eq!(
            KINDS.map(ToastKind::icon),
            [
                pod_ui::Icon::Info,
                pod_ui::Icon::CircleCheck,
                pod_ui::Icon::TriangleAlert,
                pod_ui::Icon::OctagonX
            ]
        );
        assert_eq!(
            KINDS.map(ToastKind::role),
            [Role::Status, Role::Status, Role::Alert, Role::Alert]
        );
    }
}
