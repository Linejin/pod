//! Dialogs: new terminal, new task, task details and removal, rename,
//! kill and quit confirmations.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use podd::protocol::{AgentKind, codes};

use crate::source::{Program, RunSpec, TaskSpec};

use super::Workspace;
use super::notifications::ToastKind;

impl Workspace {
    pub(super) fn confirm_kill(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(t) = self.terminal(&id).cloned() else {
            return;
        };
        let ws = cx.entity().downgrade();
        let label = self.label_of(&t);
        let body = crate::tr!(KILL_THE_WHOLE_PROCESS_TREE_ENDS_PANELS_STAY_OPEN_SHOWING_THE_FIN; ; label, t.id, t.program);
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let (ws_ok, id_ok) = (ws.clone(), id.clone());
            dialog
                .title(crate::strings::KILL_TERMINAL)
                .w(px(460.))
                .child(div().text_sm().child(body.clone()))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("kill-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("kill-ok")
                                .label(crate::strings::KILL)
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = ws_ok.update(cx, |w, cx| w.kill(id_ok.clone(), cx));
                                }),
                        ),
                )
        });
    }

    pub(super) fn open_rename_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(t) = self.terminal(&id).cloned() else {
            return;
        };
        let current = self.label_of(&t);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(current));
        let ws = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog: Dialog, window, cx| {
            if first.replace(false) {
                input.focus_handle(cx).focus(window, cx);
            }
            let (ws_ok, id_ok, input_ok) = (ws.clone(), id.clone(), input.clone());
            dialog
                .title(crate::tr!(RENAME_ID; id;))
                .w(px(420.))
                .child(Input::new(&input))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("rn-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("rn-ok")
                                .label(crate::strings::RENAME)
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let name = input_ok.read(cx).value().trim().to_string();
                                    window.close_dialog(cx);
                                    let _ =
                                        ws_ok.update(cx, |w, cx| w.rename(id_ok.clone(), name, cx));
                                }),
                        ),
                )
        });
    }

    pub fn open_new_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let home = std::env::var("USERPROFILE").unwrap_or_default();
        let program = Rc::new(Cell::new(Program::Shell));
        let cwd = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(home)
                .placeholder(crate::strings::WORKING_DIRECTORY)
        });
        let name = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::strings::OPTIONAL_E_G_REVIEW_BOT)
        });
        let custom =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::strings::COMMAND_EXAMPLE));
        let ws = cx.entity().downgrade();
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog: Dialog, window, cx| {
            if first.replace(false) {
                name.focus_handle(cx).focus(window, cx);
            }
            let selected = program.get();
            // One choice: a radio segment group, not filled buttons (docs/10 §D4.6).
            let mut programs =
                pod_ui::radio_group("prog-group", crate::strings::PROGRAM, crate::theme::mode())
                    .self_start();
            for (i, p) in Program::ALL.into_iter().enumerate() {
                let cell = program.clone();
                programs = programs.child(pod_ui::segment(
                    ("prog", i),
                    p.label(),
                    None,
                    p == selected,
                    crate::theme::mode(),
                    move |window, _| {
                        cell.set(p);
                        window.refresh();
                    },
                ));
            }
            let field = |label: &'static str, input: &Entity<InputState>| {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(crate::theme::tokens().text_muted))
                            .child(label),
                    )
                    .child(Input::new(input))
            };
            let (ws_ok, prog_ok, cwd_ok, name_ok, custom_ok) = (
                ws.clone(),
                program.clone(),
                cwd.clone(),
                name.clone(),
                custom.clone(),
            );
            dialog
                .title(crate::strings::NEW_TERMINAL)
                .w(px(520.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(rgb(crate::theme::tokens().text_muted))
                                        .child(crate::strings::PROGRAM),
                                )
                                .child(programs),
                        )
                        .when(selected == Program::Custom, |d| {
                            d.child(field(crate::strings::COMMAND, &custom))
                        })
                        .child(field(crate::strings::WORKING_DIRECTORY, &cwd))
                        .child(field(crate::strings::NAME, &name)),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("new-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("new-run")
                                .label(crate::strings::RUN)
                                .primary()
                                .on_click(move |_, window, cx| {
                                    let argv =
                                        prog_ok.get().argv(custom_ok.read(cx).value().as_ref());
                                    let text = |e: &Entity<InputState>, cx: &App| {
                                        let v = e.read(cx).value().trim().to_string();
                                        (!v.is_empty()).then_some(v)
                                    };
                                    let spec = RunSpec {
                                        argv,
                                        cwd: text(&cwd_ok, cx),
                                        name: text(&name_ok, cx),
                                        ..Default::default()
                                    };
                                    if spec.argv.is_empty() {
                                        return;
                                    }
                                    window.close_dialog(cx);
                                    let _ = ws_ok.update(cx, |w, cx| w.run(spec, window, cx));
                                }),
                        ),
                )
        });
    }

    /// New Task: repo, name, base, agent, optional prompt -> `task.create`,
    /// then open the task's agent terminal.
    pub fn open_new_task_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let busy = Rc::new(Cell::new(false));
        let error = Rc::new(RefCell::new(String::new()));
        let repos = self.repos.clone();
        let repo = Rc::new(RefCell::new(repos.first().map(|r| r.name.clone())));
        let agent = Rc::new(Cell::new(AgentKind::Shell));
        let repo_path =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::strings::REPO_PATH));
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::TASK_NAME_E_G_FIX_LOGIN_BRANCH_POD_NAME)
        });
        let base = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::BASE_REF_DEFAULT_THE_REPO_S_DEFAULT_BRANCH)
        });
        let prompt = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(crate::strings::OPTIONAL_FIRST_PROMPT_CLAUDE_CODEX)
        });
        let ws = cx.entity().downgrade();
        let submit: crate::chrome::Submit = {
            let (busy, error, repo, agent) =
                (busy.clone(), error.clone(), repo.clone(), agent.clone());
            let (repo_path, name, base, prompt) = (
                repo_path.clone(),
                name.clone(),
                base.clone(),
                prompt.clone(),
            );
            Rc::new(move |window: &mut Window, cx: &mut App| {
                if busy.get() {
                    return;
                }
                let text = |e: &Entity<InputState>, cx: &App| {
                    let v = e.read(cx).value().trim().to_string();
                    (!v.is_empty()).then_some(v)
                };
                let Some(task_name) = text(&name, cx) else {
                    *error.borrow_mut() = crate::strings::ENTER_A_TASK_NAME.into();
                    window.refresh();
                    return;
                };
                let new_repo_path = text(&repo_path, cx);
                let repo_name = repo.borrow().clone();
                if repo_name.is_none() && new_repo_path.is_none() {
                    *error.borrow_mut() =
                        crate::strings::CHOOSE_A_REPOSITORY_OR_ENTER_ITS_PATH.into();
                    window.refresh();
                    return;
                }
                let kind = agent.get();
                let spec = TaskSpec {
                    repo: repo_name.clone().unwrap_or_default(),
                    name: task_name,
                    base: text(&base, cx),
                    agent: kind,
                    prompt: if kind == AgentKind::Shell {
                        None
                    } else {
                        text(&prompt, cx)
                    },
                    cols: None,
                    rows: None,
                };
                busy.set(true);
                error.borrow_mut().clear();
                window.refresh();
                let add = if repo_name.is_none() {
                    new_repo_path
                } else {
                    None
                };
                let _ = ws.update(cx, |w, cx| {
                    w.create_task(add, spec, busy.clone(), error.clone(), window, cx)
                });
            })
        };
        // Enter in any text field creates, like the button (keyboard-only flow).
        for input in [&repo_path, &name, &base, &prompt] {
            crate::chrome::submit_on_enter(input, submit.clone(), window, cx);
        }
        let first = Cell::new(true);
        window.open_dialog(cx, move |dialog: Dialog, window, cx| {
            if first.replace(false) {
                name.focus_handle(cx).focus(window, cx);
            }
            let label = |s: &'static str| {
                div()
                    .text_xs()
                    .text_color(rgb(crate::theme::tokens().text_muted))
                    .child(s)
            };
            let selected_repo = repo.borrow().clone();
            let mode = crate::theme::mode();
            // Choices are radio segments (docs/10 §D4.6); the one filled button is 만들기.
            let mut repo_row =
                pod_ui::radio_group("repo-group", crate::strings::REPOSITORY, mode).self_start();
            for (i, r) in repos.iter().enumerate() {
                let (cell, rname) = (repo.clone(), r.name.clone());
                let path: SharedString = r.path.clone().into();
                repo_row = repo_row.child(
                    pod_ui::choice(
                        ("repo", i),
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
                    "repo-new",
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
            let selected_agent = agent.get();
            let mut agent_row =
                pod_ui::radio_group("agent-group", crate::strings::AGENT, mode).self_start();
            for (i, (k, l)) in [
                (AgentKind::Shell, crate::strings::SHELL),
                (AgentKind::Claude, "Claude"),
                (AgentKind::Codex, "Codex"),
            ]
            .into_iter()
            .enumerate()
            {
                let cell = agent.clone();
                agent_row = agent_row.child(pod_ui::segment(
                    ("agent", i),
                    l,
                    None,
                    k == selected_agent,
                    mode,
                    move |window, _| {
                        cell.set(k);
                        window.refresh();
                    },
                ));
            }
            dialog
                .title(crate::strings::NEW_TASK)
                .w(px(560.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .text_color(rgb(crate::theme::tokens().danger_fg))
                                .child(error.borrow().clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label(crate::strings::REPOSITORY))
                                .child(repo_row),
                        )
                        .when(selected_repo.is_none(), |d| d.child(Input::new(&repo_path)))
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label(crate::strings::NAME))
                                .child(Input::new(&name)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label(crate::strings::BASE))
                                .child(Input::new(&base)),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(label(crate::strings::AGENT))
                                .child(agent_row),
                        )
                        .when(selected_agent != AgentKind::Shell, |d| {
                            d.child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(label(crate::strings::PROMPT))
                                    .child(Input::new(&prompt)),
                            )
                        }),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("task-cancel")
                                .label(crate::strings::CANCEL)
                                .disabled(busy.get())
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("task-create")
                                .label(if busy.get() {
                                    crate::strings::CREATING
                                } else {
                                    crate::strings::CREATE
                                })
                                .disabled(busy.get())
                                .primary()
                                .on_click({
                                    let submit = submit.clone();
                                    move |_, window, cx| submit(window, cx)
                                }),
                        ),
                )
        });
    }

    fn create_task(
        &mut self,
        add_repo: Option<String>,
        mut spec: TaskSpec,
        busy: Rc<Cell<bool>>,
        error: Rc<RefCell<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.clone();
        cx.spawn_in(window, async move |this, cx| {
            let fail = |e: crate::source::SourceError| e.message;
            if let Some(path) = add_repo {
                match source.add_repo(&path, None).await {
                    Ok(Ok(r)) => spec.repo = r.name,
                    Ok(Err(e)) => {
                        busy.set(false);
                        *error.borrow_mut() = fail(e);
                        let _ = this.update_in(cx, |_, window, _| window.refresh());
                        return;
                    }
                    Err(_) => {
                        busy.set(false);
                        *error.borrow_mut() = crate::strings::CONNECTION_CLOSED_WHILE_ADDING_REPOSITORY.into();
                        let _ = this.update_in(cx, |_, window, _| window.refresh());
                        return;
                    }
                }
            }
            let title = format!("{}/{}", spec.repo, spec.name);
            let result = source.create_task(spec).await;
            busy.set(false);
            let _ = this.update_in(cx, |w, window, cx| {
                window.refresh();
                match result {
                    Ok(Ok(task)) => {
                        window.close_dialog(cx);
                        w.raise_toast(ToastKind::Success, crate::tr!(TASK_CREATED; ; task.id), task.worktree.clone().unwrap_or_default(), cx);
                        if let Some(t) = task.terminals.first().cloned() {
                            w.tasks.retain(|x| x.id != task.id);
                            w.tasks.push(task);
                            w.open_terminal(&t, false, window, cx);
                        }
                    }
                    Ok(Err(e)) => {
                        *error.borrow_mut() = crate::tr!(COULD_NOT_CREATE_TASK_TITLE; title; fail(e));
                    }
                    Err(_) => {
                        *error.borrow_mut() = crate::strings::CONNECTION_CLOSED_WHILE_CREATING_TASK_CHECK_THE_TASK_LIST_BEFORE_.into();
                    }
                }
            });
        })
        .detach();
    }

    /// Task details (branch, base, ahead/behind, dirty, last commit) and removal.
    pub(crate) fn open_task_dialog(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = cx.entity().downgrade();
        let delete_branch = Rc::new(Cell::new(false));
        window.open_dialog(cx, move |dialog: Dialog, _, cx| {
            let review = ws.upgrade().is_some_and(|w| w.read(cx).caps.review);
            let Some(task) = ws.upgrade().and_then(|w| w.read(cx).task(&id).cloned()) else {
                return dialog.title(crate::tr!(TASK_ID; id;)).child(crate::strings::THIS_TASK_NO_LONGER_EXISTS);
            };
            let row = |k: &'static str, v: String| {
                div()
                    .flex()
                    .gap_3()
                    .child(div().w(px(90.)).text_color(rgb(crate::theme::tokens().text_muted)).child(crate::strings::field_label(k)))
                    .child(div().flex_1().min_w_0().overflow_hidden().child(v))
            };
            let git = task.git.clone().unwrap_or_default();
            let del = delete_branch.get();
            let (ws_rm, id_rm, del_cell) = (ws.clone(), id.clone(), delete_branch.clone());
            let cell = delete_branch.clone();
            dialog
                .title(crate::tr!(TASK; ; task.id))
                .w(px(600.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_sm()
                        .child(row("branch", task.branch.clone().unwrap_or_default()))
                        .child(row("base", task.base.clone().unwrap_or_default()))
                        .child(row("agent", task.agent.clone().unwrap_or_default()))
                        .child(row(
                            "worktree",
                            format!("{}{}", task.worktree.clone().unwrap_or_default(), if task.worktree_exists { "" } else { crate::strings::MISSING }),
                        ))
                        .child(row("terminals", task.terminals.join(", ")))
                        .child(row(
                            "git",
                            match &task.git_error {
                                Some(e) => crate::tr!(UNAVAILABLE_E; e;),
                                None => crate::tr!(AHEAD_BEHIND_DIRTY_UNPUSHED; ; git.ahead, git.behind, git.dirty, git.unpushed),
                            },
                        ))
                        .child(row(crate::strings::LAST_COMMIT, git.last_commit.clone().unwrap_or_else(|| "—".into()))),
                )
                .footer(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Button::new("task-delbranch")
                                .label(if del { crate::strings::DELETE_BRANCH } else { crate::strings::DELETE_BRANCH_94 })
                                .ghost()
                                .on_click(move |_, window, _| {
                                    cell.set(!cell.get());
                                    window.refresh();
                                }),
                        )
                        .child(div().flex_1())
                        .when(review, |d| {
                            let (ws_rv, id_rv) = (ws_rm.clone(), id_rm.clone());
                            d.child(Button::new("task-review").label(crate::strings::REVIEW_CHANGES).primary().on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                let _ = ws_rv.update(cx, |w, cx| w.open_review(&id_rv, window, cx));
                            }))
                        })
                        .child(Button::new("task-close").label(crate::strings::CLOSE).on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("task-remove").label(crate::strings::REMOVE_TASK).danger().on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = ws_rm.update(cx, |w, cx| w.request_remove_task(id_rm.clone(), del_cell.get(), window, cx));
                        })),
                )
        });
    }

    /// `task.remove`; a `TASK_DIRTY` refusal is shown with a "Remove anyway" choice.
    fn remove_task(
        &mut self,
        id: String,
        force: bool,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rx = self.source.remove_task(&id, force, delete_branch);
        cx.spawn_in(window, async move |this, cx| {
            let Ok(result) = rx.await else { return };
            let _ = this.update_in(cx, |w, window, cx| match result {
                Ok(terminals) => {
                    for t in &terminals {
                        w.close_panels_of(t, window, cx);
                    }
                    w.close_editor_of(&id, window, cx);
                    w.tasks.retain(|t| t.id != id);
                    w.raise_toast(
                        ToastKind::Success,
                        crate::tr!(TASK_ID_REMOVED; id;),
                        crate::tr!(TERMINALS_100; ; terminals.join(", ")),
                        cx,
                    );
                }
                Err(e) if e.code == Some(codes::TASK_DIRTY) => {
                    w.confirm_force_remove(id.clone(), delete_branch, e.message, window, cx)
                }
                Err(e) => w.raise_toast(
                    ToastKind::Error,
                    crate::tr!(COULD_NOT_REMOVE_ID; id;),
                    e.message,
                    cx,
                ),
            });
        })
        .detach();
    }

    fn confirm_force_remove(
        &mut self,
        id: String,
        delete_branch: bool,
        reason: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ws = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let (ws_ok, id_ok) = (ws.clone(), id.clone());
            dialog
                .title(crate::strings::PODD_REFUSED_TO_REMOVE_THE_TASK)
                .w(px(520.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .text_sm()
                        .child(div().text_color(rgb(crate::theme::tokens().warning_fg)).child(reason.clone()))
                        .child(crate::strings::REMOVING_ANYWAY_DELETES_THE_WORKTREE_WITH_ITS_UNCOMMITTED_CHANGES),
                )
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(Button::new("force-cancel").label(crate::strings::KEEP_TASK).on_click(|_, window, cx| window.close_dialog(cx)))
                        .child(Button::new("force-ok").label(crate::strings::REMOVE_ANYWAY).danger().on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = ws_ok.update(cx, |w, cx| w.remove_task(id_ok.clone(), true, delete_branch, window, cx));
                        })),
                )
        });
    }

    /// Task removal from the details dialog: unsaved editor files ask first.
    pub(crate) fn request_remove_task(
        &mut self,
        id: String,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dirty: Vec<String> = self
            .editor_of(&id, cx)
            .map(|e| {
                let e = e.read(cx);
                e.editor
                    .read(cx)
                    .dirty_paths()
                    .iter()
                    .map(|p| crate::editor_panel::rel(&e.root, p))
                    .collect()
            })
            .unwrap_or_default();
        if dirty.is_empty() {
            self.remove_task(id, false, delete_branch, window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        let body = crate::tr!(REMOVE_TASK_UNSAVED_BODY; ; id, dirty.len(), dirty.join(", "));
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let (ws_ok, id_ok) = (ws.clone(), id.clone());
            dialog
                .title(crate::strings::UNSAVED_FILES_TITLE)
                .w(px(520.))
                .child(div().text_sm().child(body.clone()))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("rm-unsaved-cancel")
                                .label(crate::strings::KEEP_TASK)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("rm-unsaved-ok")
                                .label(crate::strings::REMOVE_WITHOUT_SAVING)
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = ws_ok.update(cx, |w, cx| {
                                        w.removal_confirmed.insert(id_ok.clone());
                                        w.remove_task(
                                            id_ok.clone(),
                                            false,
                                            delete_branch,
                                            window,
                                            cx,
                                        )
                                    });
                                }),
                        ),
                )
        });
    }

    /// Window close with unsaved editor files: ask (save all / discard / cancel).
    /// Returns whether the window may close now.
    pub(crate) fn confirm_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let unsaved = self.unsaved_files(cx);
        if self.quit_confirmed || unsaved.is_empty() {
            return true;
        }
        let ws = cx.entity().downgrade();
        let body = crate::tr!(UNSAVED_FILES_BODY; ; unsaved.len(), unsaved.join(", "));
        window.open_dialog(cx, move |dialog: Dialog, _, _| {
            let (ws_save, ws_quit) = (ws.clone(), ws.clone());
            dialog
                .title(crate::strings::UNSAVED_FILES_TITLE)
                .w(px(560.))
                .child(div().text_sm().child(body.clone()))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("quit-cancel")
                                .label(crate::strings::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("quit-discard")
                                .label(crate::strings::QUIT_WITHOUT_SAVING)
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let _ = ws_quit.update(cx, |w, _| w.quit_confirmed = true);
                                    cx.quit();
                                }),
                        )
                        .child(
                            Button::new("quit-save")
                                .label(crate::strings::SAVE_ALL_AND_QUIT)
                                .primary()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let ok = ws_save
                                        .update(cx, |w, cx| {
                                            w.save_all_editors(window, cx);
                                            let left = w.unsaved_files(cx).is_empty();
                                            w.quit_confirmed = left;
                                            left
                                        })
                                        .unwrap_or(true);
                                    if ok {
                                        cx.quit();
                                    }
                                }),
                        ),
                )
        });
        false
    }
}
