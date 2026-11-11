//! The status bar: what it says ([`StatusText`]), which of its items fit
//! ([`StatusPlan`]), and drawing them.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::qa_marks::QaId;
use crate::vm::{self, ConnState};

use super::Workspace;

impl Workspace {
    pub(crate) fn status_branch(&self, window: &Window, cx: &App) -> Option<&str> {
        self.focused_task(window, cx)
            .and_then(|id| self.task(&id))
            .and_then(|task| task.branch.as_deref())
            .filter(|name| !name.is_empty())
    }

    pub(super) fn render_status_bar(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let text = StatusText::new(
            &self.conn,
            vm::counts(&self.terminals),
            self.tasks.len(),
            self.live_panels().len(),
            self.toasts.len(),
            &tk,
        );
        let plan = StatusPlan::for_width(
            f32::from(window.viewport_size().width),
            crate::layout_mode::status_counts(cx),
            self.caps.status,
        );
        let branch = self.status_branch(window, cx);
        // Grouped items (docs/10 §D4.6): icon + text, 1×12 separators between groups.
        let sep = || {
            div()
                .w(px(1.))
                .h(px(12.))
                .flex_shrink_0()
                .bg(rgb(tk.border_subtle))
        };
        let item = |glyph: pod_ui::Icon, fg: u32, text: String| {
            div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .child(pod_ui::icon_sized(glyph, 14., fg))
                .child(div().text_color(rgb(fg)).child(text))
        };
        let conn_tip: SharedString = text.conn_tip().into();
        // Left: connection and counts. It may clip (items drop by `plan` first); the right
        // group (상태 · 알림) never shrinks, so it stays reachable at the 800 px minimum.
        let left = div()
            .id("status-summary")
            .qa_id(|| "status-summary".into())
            .role(Role::Status)
            // The whole summary as one status (screen readers do not see the painted text).
            .aria_label(text.summary(self.caps.status, self.caps.tasks))
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .flex()
            .items_center()
            .gap_3()
            .child(
                item(text.conn_icon, text.conn_color, text.conn.clone())
                    .id("status-conn")
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(conn_tip.clone())
                            .build(window, cx)
                    }),
            )
            .when(plan.terminals, |d| {
                d.child(sep()).child(item(
                    pod_ui::Icon::SquareTerminal,
                    tk.text_secondary,
                    text.terminals.clone(),
                ))
            })
            .when(plan.counts, |d| {
                d.child(sep())
                    .when(plan.working, |d| {
                        d.child(item(
                            pod_ui::Icon::LoaderCircle,
                            tk.info_fg,
                            text.working.clone(),
                        ))
                    })
                    .child(
                        div()
                            .qa_id(|| "status-blocked".into())
                            .flex_shrink_0()
                            .child(item(
                                pod_ui::Icon::ShieldQuestion,
                                tk.warning_fg,
                                text.blocked.clone(),
                            )),
                    )
                    .when(plan.done, |d| {
                        d.child(item(
                            pod_ui::Icon::CircleCheck,
                            tk.success_fg,
                            text.done.clone(),
                        ))
                    })
            })
            .when(self.caps.tasks && plan.tasks, |d| {
                d.child(item(
                    pod_ui::Icon::Layers,
                    tk.text_secondary,
                    text.tasks.clone(),
                ))
            })
            .when_some(branch.filter(|_| plan.branch), |d, branch| {
                let name = format!("{}: {branch}", crate::strings::CURRENT_BRANCH);
                let tip: SharedString = name.clone().into();
                d.child(sep()).child(
                    div()
                        .id("status-branch")
                        .qa_id(|| "status-branch".into())
                        .role(Role::Status)
                        .aria_label(name)
                        .h(px(24.))
                        .max_w(px(180.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(pod_ui::icon_sized(
                            pod_ui::Icon::GitBranch,
                            14.,
                            tk.text_secondary,
                        ))
                        .child(div().min_w_0().truncate().child(branch.to_string()))
                        .tooltip(move |window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                .build(window, cx)
                        }),
                )
            })
            .when_some(self.lsp_status(cx), |d, (label, color)| {
                let ws = cx.entity().downgrade();
                d.child(sep()).child(
                    div()
                        .qa_id(|| "status-lsp".into())
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .gap_1()
                        .child(div().size(px(7.)).rounded_full().bg(rgb(color)))
                        .child(
                            Button::new("status-lsp")
                                .label(label)
                                .tooltip(crate::strings::LSP_STATUS_TOOLTIP)
                                .xsmall()
                                .compact()
                                .dropdown_menu(move |menu, _, _| {
                                    let item = |menu: gpui_kit::component::menu::PopupMenu,
                                                label: &'static str,
                                                f: fn(
                                        &mut Self,
                                        &mut Window,
                                        &mut Context<Self>,
                                    )| {
                                        let ws = ws.clone();
                                        menu.item(
                                            gpui_kit::component::menu::PopupMenuItem::new(label)
                                                .on_click(move |_, window, cx| {
                                                    let _ = ws.update(cx, |w, cx| f(w, window, cx));
                                                }),
                                        )
                                    };
                                    let menu =
                                        item(menu, crate::strings::LSP_MENU_RESTART, |w, _, cx| {
                                            w.lsp_restart(cx)
                                        });
                                    let menu =
                                        item(menu, crate::strings::LSP_MENU_OFF, |w, _, cx| {
                                            w.lsp_turn_off(cx)
                                        });
                                    item(menu, crate::strings::LSP_MENU_LOG, |w, window, cx| {
                                        w.lsp_show_log(window, cx)
                                    })
                                }),
                        ),
                )
            })
            .when(self.caps.orch, |d| {
                d.child(sep()).child(
                    div()
                        .flex_shrink_0()
                        .child(crate::orch::status_bar_gates(self, cx)),
                )
            })
            .child(div().flex_1())
            .when_some(self.stats_line.clone().filter(|_| plan.stats), |d, s| {
                d.child(div().text_color(rgb(tk.text_muted)).truncate().child(s))
            });
        div()
            .qa_id(|| "status-bar".into())
            .h(px(m.density.statusbar))
            .flex_shrink_0()
            .px_2()
            .flex()
            .items_center()
            .gap_3()
            .overflow_hidden()
            .text_xs()
            .text_color(rgb(tk.text_secondary))
            .bg(rgb(tk.bg_chrome))
            .when(m.surface != pod_ui::themes::Surface::Mono, |d| {
                d.border_t(px(m.border_width))
                    .border_color(rgb(tk.border_subtle))
            })
            .child(left)
            .child(
                div()
                    .qa_id(|| "status-inspect".into())
                    .flex_shrink_0()
                    .child(
                        Button::new("status-inspect")
                            .label(crate::strings::STATUS)
                            .icon(pod_ui::kit_icon(pod_ui::Icon::Info))
                            .tooltip("상태 인스펙터 (Ctrl+Shift+S)")
                            .xsmall()
                            .ghost()
                            .on_click(cx.listener(|w, _, window, cx| w.open_inspector(window, cx))),
                    ),
            )
            .child(
                div()
                    .qa_id(|| "status-notifications".into())
                    .flex_shrink_0()
                    .child(
                        Button::new("notifications")
                            .label(text.notifications.clone())
                            .icon(pod_ui::kit_icon(pod_ui::Icon::Bell))
                            .tooltip("알림 (Ctrl+Shift+B)")
                            // xsmall: a compact (26px) button overflowed the 24px bar.
                            .xsmall()
                            .ghost()
                            .on_click(
                                cx.listener(|w, _, window, cx| w.open_notifications(window, cx)),
                            ),
                    ),
            )
    }
}

/// What the status bar says (docs/10 §D4.6), decided before it is drawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StatusText {
    /// 연결: its text, colour token and icon. PID and version stay out of
    /// the bar: they are the tooltip and accessible name (`conn_detail`).
    pub(crate) conn: String,
    pub(crate) conn_detail: Option<String>,
    pub(crate) conn_color: u32,
    pub(crate) conn_icon: pod_ui::Icon,
    /// `터미널 N · 실행 N · 패널 N`.
    pub(crate) terminals: String,
    pub(crate) working: String,
    pub(crate) blocked: String,
    pub(crate) done: String,
    pub(crate) tasks: String,
    /// The notifications button.
    pub(crate) notifications: String,
}

impl StatusText {
    /// `tasks`, `panels`, `toasts`: how many tasks, live terminal panels and
    /// toasts (shown or not) there are; `tk` gives the connection colour.
    pub(crate) fn new(
        conn: &ConnState,
        c: vm::Counts,
        tasks: usize,
        panels: usize,
        toasts: usize,
        tk: &pod_ui::Tokens,
    ) -> Self {
        let (conn, conn_detail, conn_color, conn_icon) = match conn {
            ConnState::Connecting => (
                crate::strings::PODD_CONNECTING.to_string(),
                None,
                tk.warning_fg,
                pod_ui::Icon::Plug,
            ),
            ConnState::Connected { pid, version } => (
                crate::strings::PODD_CONNECTED.to_string(),
                Some(format!("podd {version} · pid {pid}")),
                tk.success_fg,
                pod_ui::Icon::Plug,
            ),
            ConnState::Reconnecting { attempt, .. } => (
                crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT; attempt;),
                None,
                tk.warning_fg,
                pod_ui::Icon::Unplug,
            ),
        };
        Self {
            conn,
            conn_detail,
            conn_color,
            conn_icon,
            terminals: crate::tr!(TERMINALS_RUNNING_PANELS; ; c.total, c.running, panels),
            working: crate::tr!(WORKING; ; c.working),
            blocked: crate::tr!(BLOCKED; ; c.blocked),
            done: crate::tr!(DONE; ; c.done),
            tasks: crate::tr!(TASKS; ; tasks),
            notifications: crate::tr!(NOTIFICATIONS_122; ; toasts),
        }
    }

    /// The left group as one status: connection (detail) · terminals, then
    /// the agent counts when podd reports `status` and the task count when
    /// it has `tasks`.
    pub(crate) fn summary(&self, status: bool, tasks: bool) -> String {
        let mut summary = vec![
            self.conn_detail
                .as_ref()
                .map_or(self.conn.clone(), |d| format!("{} ({d})", self.conn)),
            self.terminals.clone(),
        ];
        if status {
            summary.extend([
                self.working.clone(),
                self.blocked.clone(),
                self.done.clone(),
            ]);
        }
        if tasks {
            summary.push(self.tasks.clone());
        }
        summary.join(" · ")
    }

    /// The connection item's tooltip: connection · detail · terminals, or
    /// the connection alone without a detail.
    pub(crate) fn conn_tip(&self) -> String {
        self.conn_detail.as_ref().map_or(self.conn.clone(), |d| {
            format!("{} · {d} · {}", self.conn, self.terminals)
        })
    }
}

/// Which status-bar items fit (docs/10 §D4.6): as the window narrows, branch/terminal
/// totals go first, then 완료, then 작업 중. 연결, 응답 and 알림 always stay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct StatusPlan {
    pub branch: bool,
    pub terminals: bool,
    pub counts: bool,
    pub working: bool,
    pub done: bool,
    pub tasks: bool,
    pub stats: bool,
}

impl StatusPlan {
    /// `width`: window width in logical px; `counts`: the layout wants 작업 중/응답/완료
    /// here (06 shows them in the title bar); `status`: podd reports agent status.
    pub(crate) fn for_width(width: f32, counts: bool, status: bool) -> Self {
        let counts = counts && status;
        Self {
            branch: width >= 1400.,
            terminals: width >= 1180.,
            counts,
            working: width >= 900.,
            done: width >= 1000.,
            tasks: width >= 1080.,
            stats: width >= 1400.,
        }
    }
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;

    use super::StatusText;
    use crate::strings::{PODD_CONNECTED, PODD_CONNECTING};
    use crate::vm::{ConnState, Counts};

    const COUNTS: Counts = Counts {
        total: 3,
        running: 2,
        working: 1,
        blocked: 1,
        done: 1,
    };

    /// Connected: the PID and version are the detail, in the summary and
    /// the tooltip; the counts follow in order.
    #[test]
    fn status_text_names_the_connection_then_the_counts() {
        let tk = pod_ui::Tokens::for_mode(pod_ui::Mode::Dark);
        let conn = ConnState::Connected {
            pid: 42,
            version: "0.9".into(),
        };
        let s = StatusText::new(&conn, COUNTS, 2, 1, 4, &tk);
        assert_eq!(
            (s.conn.as_str(), s.conn_detail.as_deref()),
            (PODD_CONNECTED, Some("podd 0.9 · pid 42"))
        );
        assert_eq!(
            (s.conn_color, s.conn_icon),
            (tk.success_fg, pod_ui::Icon::Plug)
        );
        let terminals = crate::tr!(TERMINALS_RUNNING_PANELS; ; 3, 2, 1);
        assert_eq!(s.terminals, terminals);
        let (working, blocked, done, tasks) = (
            crate::tr!(WORKING; ; 1),
            crate::tr!(BLOCKED; ; 1),
            crate::tr!(DONE; ; 1),
            crate::tr!(TASKS; ; 2),
        );
        assert_eq!(
            (&s.working, &s.blocked, &s.done, &s.tasks),
            (&working, &blocked, &done, &tasks)
        );
        assert_eq!(s.notifications, crate::tr!(NOTIFICATIONS_122; ; 4));
        let head = format!("{PODD_CONNECTED} (podd 0.9 · pid 42) · {terminals}");
        assert_eq!(
            s.summary(true, true),
            format!("{head} · {working} · {blocked} · {done} · {tasks}")
        );
        assert_eq!(s.summary(false, true), format!("{head} · {tasks}"));
        assert_eq!(s.summary(false, false), head);
        assert_eq!(
            s.conn_tip(),
            format!("{PODD_CONNECTED} · podd 0.9 · pid 42 · {terminals}")
        );
    }

    /// Without a detail the summary starts with the plain connection text
    /// and the tooltip is only that.
    #[test]
    fn status_text_while_connecting_or_reconnecting() {
        let tk = pod_ui::Tokens::for_mode(pod_ui::Mode::Light);
        let s = StatusText::new(&ConnState::Connecting, COUNTS, 0, 0, 0, &tk);
        assert_eq!(
            (
                s.conn.as_str(),
                s.conn_detail.as_deref(),
                s.conn_color,
                s.conn_icon
            ),
            (PODD_CONNECTING, None, tk.warning_fg, pod_ui::Icon::Plug)
        );
        assert_eq!(s.conn_tip(), PODD_CONNECTING);
        assert_eq!(
            s.summary(false, false),
            format!("{PODD_CONNECTING} · {}", s.terminals)
        );
        let attempt = 3;
        let conn = ConnState::Reconnecting {
            attempt,
            error: "pipe closed".into(),
        };
        let s = StatusText::new(&conn, COUNTS, 0, 0, 0, &tk);
        assert_eq!(
            s.conn,
            crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT; attempt;)
        );
        assert_eq!(
            (s.conn_color, s.conn_icon),
            (tk.warning_fg, pod_ui::Icon::Unplug)
        );
        assert!(
            !s.conn.contains("pipe closed"),
            "the error stays out of the bar"
        );
        assert_eq!(s.conn_tip(), s.conn);
    }
}
