//! The Group panel's render: the header frame (title, status, winner and its
//! PR button, prompt, check field with Run/Cancel check and ⟳ Compare), the
//! check line, the pick banner and one card per member. What each part shows
//! is planned in [`super::model`]; the handlers only call
//! [`super::controller`] (and open the pick dialog, [`super::dialog`]).

use gpui_kit::component::Disableable as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{Frame, FrameKind};
use podd::protocol::{CheckJob, CompareMember, GroupMember};

use super::model::{
    MemberLive, PickBanner, card_plan, file_counts, header_plan, job_line, pick_banner,
};
use super::{GroupPanel, check_summary, copy, tail_lines};
use crate::orch::{mode, summary_badge, tc, tokens};

/// Changed files listed per card before "… N more".
const FILES_PER_CARD: usize = 6;
/// Output lines shown when a check result is expanded.
const OUTPUT_LINES: usize = 14;

impl GroupPanel {
    fn render_header(&self, live: &MemberLive, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let info = self.info.as_ref();
        let job = info.and_then(|g| g.check_job.clone());
        let summary = &live.summary;
        let plan = header_plan(&self.group_id, info, summary);
        let running = plan.running;
        Frame::new("group-head", FrameKind::List, plan.title, plan.subtitle)
            .render(
                mode(),
                [
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .when_some(summary_badge(summary), |d, b| d.child(b.render(mode())))
                        .when_some(plan.winner, |d, w| {
                            let task = w.clone();
                            d.child(div().px_2().py_1().rounded_sm().bg(tc(t.success_bg)).child(pod_ui::icon_text(pod_ui::Icon::CircleCheck, t.success_fg, crate::tr!(WINNER_W; w;)))).child(
                                // P6: the winner goes on to a pull request (docs/15).
                                Button::new("grp-winner-pr")
                                    .label(crate::pr_panel::s::CREATE_PR_FOR_WINNER)
                                    .xsmall()
                                    .tooltip(crate::pr_panel::s::CREATE_PR_FOR_WINNER_TIP)
                                    .on_click(cx.listener(move |p, _, window, cx| {
                                        p.open_winner_pr(task.clone(), window, cx)
                                    })),
                            )
                        })
                        .into_any_element(),
                    div()
                        .text_xs()
                        .text_color(tc(t.text_secondary))
                        .max_h(px(60.))
                        .overflow_hidden()
                        .child(plan.prompt)
                        .into_any_element(),
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(div().text_xs().text_color(tc(t.text_secondary)).child(crate::strings::CHECK))
                        .child(div().w(px(320.)).child(Input::new(&self.check_input)))
                        .child(
                            Button::new("grp-run-check")
                                .label(if running { crate::strings::CHECKING } else { crate::strings::RUN_CHECK })
                                .primary()
                                .disabled(running || self.check_busy)
                                .tooltip(crate::strings::GROUP_CHECK_RUN_THE_COMMAND_IN_EVERY_MEMBER_S_WORKTREE_AT_ONCE)
                                .on_click(cx.listener(|p, _, _, cx| p.run_check(cx))),
                        )
                        .when(running, |d| {
                            d.child(
                                Button::new("grp-cancel-check")
                                    .label(crate::strings::CANCEL_CHECK)
                                    .danger()
                                    .on_click(cx.listener(|p, _, _, cx| p.cancel_check(cx))),
                            )
                        })
                        .child(
                            Button::new("grp-compare")
                                .icon(pod_ui::kit_icon(pod_ui::Icon::RefreshCw))
                                .label(if self.compare_loading { crate::strings::COMPARING } else { crate::strings::COMPARE })
                                .ghost()
                                .disabled(self.compare_loading)
                                .tooltip(crate::strings::GROUP_COMPARE_RE_READ_EVERY_MEMBER_S_DIFF_AGAINST_THE_BASE)
                                .on_click(cx.listener(|p, _, _, cx| p.refresh(cx))),
                        )
                        .into_any_element(),
                    self.render_job_line(job.as_ref()).into_any_element(),
                    self.render_pick_banner().into_any_element(),
                ],
            )
            .max_w_full()
    }

    fn render_job_line(&self, job: Option<&CheckJob>) -> impl IntoElement {
        let (text, fg) = job_line(self.check_error.as_ref(), self.info.as_ref(), job);
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(tc(fg))
            .child(text)
    }

    fn render_pick_banner(&self) -> impl IntoElement {
        let t = tokens();
        match pick_banner(self.pick.as_ref()) {
            PickBanner::None => div(),
            PickBanner::Refused(text) => div().text_sm().text_color(tc(t.danger_fg)).child(text),
            PickBanner::Done { line, kept } => div()
                .flex()
                .flex_col()
                .text_sm()
                .child(div().text_color(tc(t.success_fg)).child(line))
                .children(
                    kept.into_iter()
                        .map(|k| div().text_color(tc(t.warning_fg)).child(k)),
                ),
        }
    }

    fn render_card(
        &self,
        m: &GroupMember,
        cmp: Option<&CompareMember>,
        live: &MemberLive,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = tokens();
        let plan = card_plan(m, self.info.as_ref(), cmp, live);
        let (picked, checking, check) = (plan.picked, plan.checking, plan.check);
        let (id_term, id_review, id_pick) = (m.task.clone(), m.task.clone(), m.task.clone());
        let expanded = self.expanded.contains(&m.task);
        let id_toggle = m.task.clone();

        let stats = match cmp {
            Some(c) if c.error.is_some() => div()
                .text_xs()
                .text_color(tc(t.danger_fg))
                .child(c.error.clone().unwrap_or_default()),
            Some(c) => div()
                .flex()
                .flex_wrap()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .text_color(tc(t.success_fg))
                        .child(format!("+{}", c.added)),
                )
                .child(
                    div()
                        .text_color(tc(t.danger_fg))
                        .child(format!("−{}", c.deleted)),
                )
                .child(
                    div()
                        .text_color(tc(t.text_secondary))
                        .child(crate::tr!(FILE_S; ; c.files_changed)),
                )
                .when_some(c.ahead, |d, a| {
                    d.child(pod_ui::icon_text(
                        pod_ui::Icon::ArrowUp,
                        t.text_muted,
                        a.to_string(),
                    ))
                })
                .when(c.dirty.unwrap_or(0) > 0, |d| {
                    d.child(
                        div()
                            .text_color(tc(t.text_muted))
                            .child(crate::tr!(DIRTY; ; c.dirty.unwrap_or(0))),
                    )
                }),
            None if self.compare_loading => div()
                .text_xs()
                .text_color(tc(t.text_muted))
                .child(crate::strings::COMPARING),
            None => div().text_xs().text_color(tc(t.text_muted)).child(
                self.compare_error
                    .clone()
                    .unwrap_or_else(|| crate::strings::NO_DIFF_YET.into()),
            ),
        };
        let files = cmp.map(|c| c.files.clone()).unwrap_or_default();
        let more = files.len().saturating_sub(FILES_PER_CARD);
        let file_list = div()
            .flex()
            .flex_col()
            .text_xs()
            .font_family("Consolas")
            .children(files.iter().take(FILES_PER_CARD).map(|f| {
                let counts = file_counts(f);
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .w(px(12.))
                            .text_color(tc(t.info_fg))
                            .child(f.status.clone()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(f.path.clone()))
                    .child(div().text_color(tc(t.text_muted)).child(counts))
            }))
            .when(more > 0, |d| {
                d.child(
                    div()
                        .text_color(tc(t.text_muted))
                        .child(crate::tr!(MORE_MORE; more;)),
                )
            });

        let check_el =
            match check {
                _ if checking => div()
                    .text_sm()
                    .text_color(tc(t.info_fg))
                    .child(crate::strings::CHECK_RUNNING),
                None => div()
                    .text_sm()
                    .text_color(tc(t.text_muted))
                    .child(crate::strings::CHECK_NOT_RUN),
                Some(c) => {
                    let (text, fg) = check_summary(c);
                    let glyph = if fg == t.success_fg {
                        pod_ui::Icon::CircleCheck
                    } else {
                        pod_ui::Icon::OctagonX
                    };
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .id(SharedString::from(format!("grp-out-{}", m.task)))
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .child(
                                    pod_ui::icon_text(glyph, fg, text)
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD),
                                )
                                .child(div().text_xs().text_color(tc(t.accent_fg)).child(
                                    if expanded {
                                        crate::strings::HIDE_OUTPUT
                                    } else {
                                        crate::strings::SHOW_OUTPUT
                                    },
                                ))
                                .on_click(
                                    cx.listener(move |p, _, _, cx| p.toggle_output(&id_toggle, cx)),
                                ),
                        )
                        .when(expanded, |d| {
                            d.child(
                                div()
                                    .p_2()
                                    .rounded_sm()
                                    .bg(tc(t.terminal_bg))
                                    .text_xs()
                                    .font_family("Consolas")
                                    .text_color(tc(t.terminal_fg))
                                    .children(
                                        tail_lines(&c.output_tail, OUTPUT_LINES)
                                            .into_iter()
                                            .map(|l| div().child(l)),
                                    ),
                            )
                        })
                }
            };

        div()
            .id(SharedString::from(format!("grp-card-{}", m.task)))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .w(px(320.))
            .flex_shrink_0()
            .rounded_lg()
            .border_1()
            .border_color(tc(plan.border))
            .bg(tc(t.bg_surface))
            .when(m.removed, |d| d.opacity(0.55))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(plan.short),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(tc(t.text_secondary))
                            .child(m.agent.to_string()),
                    )
                    .when(picked, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .px_1()
                                .rounded_sm()
                                .bg(tc(t.success_bg))
                                .child(pod_ui::icon_text(
                                    pod_ui::Icon::CircleCheck,
                                    t.success_fg,
                                    crate::strings::WINNER_279,
                                )),
                        )
                    })
                    .when(m.removed, |d| {
                        d.child(
                            div()
                                .text_xs()
                                .text_color(tc(t.text_muted))
                                .child(crate::strings::REMOVED),
                        )
                    }),
            )
            .child(match (m.removed, plan.badge) {
                (true, _) => div()
                    .text_xs()
                    .text_color(tc(t.text_muted))
                    .child(crate::strings::TASK_REMOVED)
                    .into_any_element(),
                (false, Some(b)) => div().flex().child(b.render(mode())).into_any_element(),
                (false, None) => div()
                    .text_xs()
                    .text_color(tc(t.text_muted))
                    .child(crate::strings::NO_TERMINAL)
                    .into_any_element(),
            })
            .child(
                div()
                    .text_xs()
                    .text_color(tc(t.text_secondary))
                    .truncate()
                    .child(copy::branch_line(&plan.branch)),
            )
            .child(stats)
            .child(file_list)
            .child(check_el)
            .child(
                div()
                    .flex()
                    .gap_1()
                    .pt_1()
                    .child(
                        Button::new(SharedString::from(format!("grp-term-{}", m.task)))
                            .label(crate::strings::TERMINALS)
                            .compact()
                            .disabled(m.removed)
                            .tooltip(crate::strings::OPEN_THE_MEMBER_S_AGENT_TERMINAL)
                            .on_click(cx.listener(move |p, _, window, cx| {
                                p.open_terminal(&id_term, window, cx)
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("grp-rev-{}", m.task)))
                            .label(crate::strings::REVIEW_284)
                            .compact()
                            .disabled(m.removed)
                            .tooltip(crate::strings::OPEN_THE_MEMBER_S_REVIEW_PANEL_DIFF_COMMENTS)
                            .on_click(cx.listener(move |p, _, window, cx| {
                                p.open_review(&id_review, window, cx)
                            })),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(SharedString::from(format!("grp-pick-{}", m.task)))
                            .label(crate::strings::PICK_WINNER)
                            .primary()
                            .compact()
                            .disabled(m.removed)
                            .on_click(cx.listener(move |p, _, window, cx| {
                                p.open_pick_dialog(id_pick.clone(), window, cx)
                            })),
                    ),
            )
    }
}

impl Render for GroupPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(text) = self.pending_check_text.take() {
            self.check_input
                .update(cx, |i, cx| i.set_value(text, window, cx));
        }
        let t = tokens();
        let members: Vec<GroupMember> = self
            .info
            .as_ref()
            .map(|g| g.members.clone())
            .unwrap_or_default();
        let live = {
            let ws = self.workspace.upgrade();
            let ws = ws.as_ref().map(|w| w.read(cx));
            MemberLive::gather(
                &members,
                ws.map(|w| (w.tasks.as_slice(), w.terminals.as_slice())),
            )
        };
        let body = match &self.info {
            None => {
                let (title, desc) = match &self.show_error {
                    Some(e) => (
                        crate::strings::COULD_NOT_LOAD_THE_GROUP.to_string(),
                        e.clone(),
                    ),
                    None => (
                        crate::strings::LOADING_GROUP.to_string(),
                        self.group_id.clone(),
                    ),
                };
                div()
                    .p_4()
                    .child(
                        Frame::new("grp-empty", FrameKind::Empty, title, desc).render(mode(), []),
                    )
                    .when(self.show_error.is_none(), |d| {
                        d.child(crate::chrome::loading_placeholder(
                            crate::strings::LOADING_GROUP,
                        ))
                    })
                    .into_any_element()
            }
            Some(info) => {
                let info = info.clone();
                let cards: Vec<AnyElement> = info
                    .members
                    .iter()
                    .map(|m| {
                        let cmp = self
                            .compare
                            .as_ref()
                            .and_then(|c| c.members.iter().find(|x| x.task == m.task));
                        self.render_card(m, cmp, &live, cx).into_any_element()
                    })
                    .collect();
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .child(self.render_header(&live, cx))
                    .child(div().flex().flex_wrap().gap_3().children(cards))
                    .into_any_element()
            }
        };
        div()
            .id("group-panel")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .bg(tc(t.bg_canvas))
            .text_color(tc(t.text_primary))
            .child(body)
    }
}
