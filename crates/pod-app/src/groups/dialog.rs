//! The group dialogs: New Group (repository, name, base, prompt, agents with
//! counts, optional check command → `group.create`), Pick winner (remove the
//! other members, delete their branches → `group.pick`) and the confirmation
//! when podd kept members with work in them (→ `group.pick` with `force`).
//! Validation and the member preview are [`super::model`]'s; the requests
//! are [`super::controller`]'s.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use podd::protocol as p;

use super::model::{member_preview, pick_others};
use super::{AgentCounts, GroupPanel, check_argv, copy, validate};
use crate::orch::{tc, tokens};
use crate::workspace::Workspace;

impl Workspace {
    /// The New Group dialog (the capability check is the facade's,
    /// [`Workspace::open_new_group_dialog`]).
    pub(super) fn new_group_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let repos = self.repos.clone();
        let repo = Rc::new(RefCell::new(repos.first().map(|r| r.name.clone())));
        let counts = Rc::new(RefCell::new(AgentCounts {
            claude: 1,
            codex: 1,
            shell: 0,
            custom: String::new(),
            custom_count: 0,
        }));
        let error: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let repo_path =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::strings::REPO_PATH));
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::GROUP_NAME_E_G_LOGIN_FIX_MEMBERS_NAME_AGENT_N)
        });
        let base = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::BASE_REF_DEFAULT_THE_REPO_S_DEFAULT_BRANCH)
        });
        let prompt = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 8)
                .placeholder(crate::strings::THE_PROMPT_EVERY_AGENT_GETS)
        });
        let custom = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::CUSTOM_AGENT_COMMAND_LINE_E_G_AIDER_YES)
        });
        let check = cx.new(|cx| {
            InputState::new(window, cx).placeholder(
                crate::strings::OPTIONAL_CHECK_COMMAND_E_G_CARGO_TEST_RUNS_VIA_CMD_C_IN_EACH_WORK,
            )
        });
        let ws = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog: Dialog, window, cx| {
            if first.replace(false) {
                name.focus_handle(cx).focus(window, cx);
            }
            let t = tokens();
            let label =
                move |s: &'static str| div().text_xs().text_color(tc(t.text_secondary)).child(s);
            let field = move |l: &'static str, el: AnyElement| {
                div().flex().flex_col().gap_1().child(label(l)).child(el)
            };

            // Repository choice (same as New task).
            let selected_repo = repo.borrow().clone();
            // A radio segment group, as in 새 작업 (docs/10 §D4.6: one filled CTA per dialog).
            let mode = crate::theme::mode();
            let mut repo_row =
                pod_ui::radio_group("grp-repo-group", crate::strings::REPOSITORY, mode)
                    .self_start();
            for (i, r) in repos.iter().enumerate() {
                let (cell, rname) = (repo.clone(), r.name.clone());
                let path: SharedString = r.path.clone().into();
                repo_row = repo_row.child(
                    pod_ui::choice(
                        ("grp-repo", i),
                        r.name.clone(),
                        format!("{}: {} ({})", crate::strings::REPOSITORY, r.name, r.path),
                        None,
                        selected_repo.as_deref() == Some(r.name.as_str()),
                        true,
                        mode,
                        move |window, _| {
                            *cell.borrow_mut() = Some(rname.clone());
                            window.refresh();
                        },
                    )
                    .tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)
                    }),
                );
            }
            {
                let cell = repo.clone();
                repo_row = repo_row.child(pod_ui::segment(
                    "grp-repo-new",
                    crate::strings::ADD_REPOSITORY,
                    Some(pod_ui::Icon::Plus),
                    selected_repo.is_none(),
                    mode,
                    move |window, _| {
                        *cell.borrow_mut() = None;
                        window.refresh();
                    },
                ));
            }

            // Agents with a count each.
            let c = counts.borrow().clone();
            let stepper = |id: &'static str,
                           title: &'static str,
                           hint: &'static str,
                           get: fn(&AgentCounts) -> usize,
                           set: fn(&mut AgentCounts, usize)| {
                let (minus, plus) = (counts.clone(), counts.clone());
                let n = get(&c);
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(80.)).text_sm().child(title))
                    .child(
                        Button::new((id, 0usize))
                            .label("−")
                            .accessibility_label(copy::decrease(title))
                            .compact()
                            .disabled(n == 0)
                            .on_click(move |_, window, _| {
                                let mut c = minus.borrow_mut();
                                let v = get(&c).saturating_sub(1);
                                set(&mut c, v);
                                window.refresh();
                            }),
                    )
                    .child(
                        div()
                            .w(px(24.))
                            .text_center()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(n.to_string()),
                    )
                    .child(
                        Button::new((id, 1usize))
                            .label("+")
                            .accessibility_label(copy::increase(title))
                            .compact()
                            .disabled(n >= 16)
                            .on_click(move |_, window, _| {
                                let mut c = plus.borrow_mut();
                                let v = get(&c) + 1;
                                set(&mut c, v);
                                window.refresh();
                            }),
                    )
                    .child(div().text_xs().text_color(tc(t.text_muted)).child(hint))
            };
            let agents = div()
                .flex()
                .flex_col()
                .gap_1()
                .child(stepper(
                    "grp-claude",
                    copy::CLAUDE,
                    crate::strings::PROMPT_AS_CLI_ARGUMENT,
                    |c| c.claude,
                    |c, v| c.claude = v,
                ))
                .child(stepper(
                    "grp-codex",
                    copy::CODEX,
                    crate::strings::PROMPT_AS_CLI_ARGUMENT,
                    |c| c.codex,
                    |c, v| c.codex = v,
                ))
                .child(stepper(
                    "grp-shell",
                    crate::strings::SHELL,
                    crate::strings::POWERSHELL_PROMPT_TYPED_AS_A_COMMAND,
                    |c| c.shell,
                    |c, v| c.shell = v,
                ))
                .child(stepper(
                    "grp-custom",
                    crate::strings::CUSTOM,
                    crate::strings::ANY_PROGRAM_ARGV_BELOW_PROMPT_TYPED_IN,
                    |c| c.custom_count,
                    |c, v| c.custom_count = v,
                ))
                .when(c.custom_count > 0, |d| d.child(Input::new(&custom)));

            let group_name = name.read(cx).value().trim().to_string();
            let mut preview_counts = c.clone();
            preview_counts.custom = custom.read(cx).value().to_string();
            let preview = member_preview(&preview_counts, &group_name);
            let err = error.borrow().clone();

            let (ws_ok, repo_ok, counts_ok, error_ok) =
                (ws.clone(), repo.clone(), counts.clone(), error.clone());
            let (path_ok, name_ok, base_ok, prompt_ok, custom_ok, check_ok) = (
                repo_path.clone(),
                name.clone(),
                base.clone(),
                prompt.clone(),
                custom.clone(),
                check.clone(),
            );
            dialog
                .title(crate::strings::NEW_GROUP_FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS)
                .w(px(640.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(field(
                            crate::strings::REPOSITORY,
                            repo_row.into_any_element(),
                        ))
                        .when(selected_repo.is_none(), |d| d.child(Input::new(&repo_path)))
                        .child(
                            div()
                                .flex()
                                .gap_3()
                                .child(div().flex_1().child(field(
                                    crate::strings::NAME,
                                    Input::new(&name).into_any_element(),
                                )))
                                .child(div().flex_1().child(field(
                                    crate::strings::BASE,
                                    Input::new(&base).into_any_element(),
                                ))),
                        )
                        .child(field(
                            crate::strings::PROMPT,
                            Textarea::new(&prompt).into_any_element(),
                        ))
                        .child(field(crate::strings::AGENT, agents.into_any_element()))
                        .child(field(
                            crate::strings::CHECK_OPTIONAL,
                            Input::new(&check).into_any_element(),
                        ))
                        .child(div().text_xs().text_color(tc(t.text_muted)).child(preview))
                        .when_some(err, |d, e| {
                            d.child(div().text_sm().text_color(tc(t.danger_fg)).child(e))
                        }),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("grp-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("grp-create")
                                .label(crate::strings::CREATE_GROUP)
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let text = |e: &Entity<InputState>, cx: &App| {
                                        let v = e.read(cx).value().trim().to_string();
                                        (!v.is_empty()).then_some(v)
                                    };
                                    let mut counts = counts_ok.borrow().clone();
                                    counts.custom = custom_ok.read(cx).value().to_string();
                                    let group_name = text(&name_ok, cx).unwrap_or_default();
                                    let prompt_text = prompt_ok.read(cx).value().to_string();
                                    let repo_name = repo_ok.borrow().clone();
                                    let new_path = text(&path_ok, cx);
                                    if let Some(why) = validate(
                                        &group_name,
                                        &prompt_text,
                                        &counts,
                                        repo_name.is_some() || new_path.is_some(),
                                    ) {
                                        *error_ok.borrow_mut() = Some(why.into());
                                        window.refresh();
                                        return;
                                    }
                                    let params = p::GroupCreateParams {
                                        repo: repo_name.clone().unwrap_or_default(),
                                        name: group_name,
                                        base: text(&base_ok, cx),
                                        prompt: prompt_text.trim_end().to_string(),
                                        agents: counts.specs(),
                                        check: text(&check_ok, cx).and_then(|l| check_argv(&l)),
                                        ..Default::default()
                                    };
                                    window.close_dialog(cx);
                                    let add = if repo_name.is_none() { new_path } else { None };
                                    let _ = ws_ok.update(cx, |w, cx| {
                                        w.create_group(add, params, window, cx)
                                    });
                                }),
                        ),
                )
        });
    }
}

impl GroupPanel {
    /// Pick winner: remove the other members (default on when there are any)
    /// and, with that, their branches.
    pub(super) fn open_pick_dialog(
        &mut self,
        task: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let others = pick_others(self.info.as_ref(), &task);
        let remove = Rc::new(Cell::new(!others.is_empty()));
        let delete_branch = Rc::new(Cell::new(false));
        let me = cx.entity().downgrade();
        let group = self.group_id.clone();
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let t = tokens();
            let (r, d) = (remove.get(), delete_branch.get());
            let (rc, dc) = (remove.clone(), delete_branch.clone());
            let (me_ok, task_ok, rm_ok, del_ok) = (me.clone(), task.clone(), remove.clone(), delete_branch.clone());
            dialog
                .title(crate::tr!(PICK_TASK_AS_THE_WINNER; task;))
                .w(px(560.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .child(crate::tr!(RECORDS_TASK_AS_THE_WINNER_OF_GROUP_NOTHING_IS_MERGED_THE_WINNER_; task,group;))
                        .when(!others.is_empty(), |el| {
                            el.child(Checkbox::new("pick-remove").label(crate::tr!(REMOVE_THE_OTHER_MEMBERS; ; others.join(", "))).checked(r).on_click(
                                move |v, window, _| {
                                    rc.set(*v);
                                    window.refresh();
                                },
                            ))
                            .child(
                                div()
                                    .pl_6()
                                    .text_xs()
                                    .text_color(tc(t.text_muted))
                                    .child(crate::strings::SAFE_PODD_REFUSES_A_MEMBER_WITH_UNCOMMITTED_CHANGES_OR_UNPUSHED_C),
                            )
                            .child(
                                Checkbox::new("pick-delbranch").label(crate::strings::ALSO_DELETE_THEIR_BRANCHES).checked(d).disabled(!r).on_click(
                                    move |v, window, _| {
                                        dc.set(*v);
                                        window.refresh();
                                    },
                                ),
                            )
                        }),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(Button::new("pick-cancel").label(crate::strings::CANCEL).on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("pick-ok").label(crate::strings::PICK_WINNER).primary().on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let (rm, del) = (rm_ok.get(), rm_ok.get() && del_ok.get());
                            let _ = me_ok.update(cx, |p, cx| p.pick(task_ok.clone(), rm, false, del, window, cx));
                        })),
                )
        });
    }

    /// podd kept members with work in them: show why, offer "Remove anyway".
    pub(super) fn confirm_force(
        &mut self,
        task: String,
        kept: Vec<p::MemberError>,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let me = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let t = tokens();
            let (me_ok, task_ok) = (me.clone(), task.clone());
            dialog
                .title(crate::strings::PODD_KEPT_SOME_MEMBERS)
                .w(px(600.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .children(kept.iter().map(|k| {
                            div()
                                .flex()
                                .flex_col()
                                .child(div().font_weight(FontWeight::SEMIBOLD).child(k.task.clone()))
                                .child(div().text_color(tc(t.warning_fg)).child(k.message.clone()))
                        }))
                        .child(crate::strings::REMOVING_ANYWAY_DELETES_THEIR_WORKTREES_WITH_UNCOMMITTED_CHANGES_),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(Button::new("force-keep").label(crate::strings::KEEP_THEM).on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("force-rm").label(crate::strings::REMOVE_ANYWAY).danger().on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = me_ok.update(cx, |p, cx| p.pick(task_ok.clone(), true, true, delete_branch, window, cx));
                        })),
                )
        });
    }
}
