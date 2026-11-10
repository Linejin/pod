//! Fan-out groups (P5, docs/09 §8, backend docs/11 §3.2): the New Group
//! dialog (`group.create`) and the Group panel — one card per member task
//! with live status, branch, +/− totals, changed files and check result,
//! the group check (`group.check` / `group.check_cancel`, results from
//! `group.check` events) and Pick winner (`group.pick`).
//!
//! Data: `group.show` (members, check job) on open, after a pick and when a
//! check job ends; `group.compare` (diff stats per member, git runs in podd,
//! seconds for big groups) on open, on ⟳ and when a member's git summary from
//! the regular task poll changes (debounced). Member status comes live from the
//! workspace's terminal list / status stream; nothing here polls on a timer.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::Sizable as _;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{Frame, FrameKind, StatusSummary};
use podd::protocol::{
    self as p, AgentKind, AgentSpec, CheckJob, CheckJobState, CheckResult, CompareMember,
    GroupCompareResult, GroupInfo, GroupMember, GroupPickResult, codes, orch_codes,
};
use serde_json::json;

use crate::orch::{mode, summary_badge, summary_of, summary_short, tc, terminal_badge, tokens};
use crate::source::{PoddSource, SourceError, split_command_line};
use crate::vm::{TaskVm, TerminalVm};
use crate::workspace::{ToastKind, Workspace};

pub const GROUP_PANEL_NAME: &str = "PodGroup";
/// Coalesce compare triggers (task poll + several members ending turns).
const COMPARE_DEBOUNCE: Duration = Duration::from_millis(800);
/// Changed files listed per card before "… N more".
const FILES_PER_CARD: usize = 6;
/// Output lines shown when a check result is expanded.
const OUTPUT_LINES: usize = 14;

// ───────────────────────────── New Group dialog ─────────────────────────────

/// Agents the dialog fans out to: per kind, how many members.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentCounts {
    pub claude: usize,
    pub codex: usize,
    pub shell: usize,
    /// Custom command line and how many members run it.
    pub custom: String,
    pub custom_count: usize,
}

impl AgentCounts {
    /// `group.create` agents, in dialog order (claude, codex, shell, custom).
    pub fn specs(&self) -> Vec<AgentSpec> {
        let mut v = Vec::new();
        v.extend(std::iter::repeat_n(
            AgentSpec::Kind(AgentKind::Claude),
            self.claude,
        ));
        v.extend(std::iter::repeat_n(
            AgentSpec::Kind(AgentKind::Codex),
            self.codex,
        ));
        v.extend(std::iter::repeat_n(
            AgentSpec::Kind(AgentKind::Shell),
            self.shell,
        ));
        let argv = split_command_line(&self.custom);
        if !argv.is_empty() {
            v.extend(std::iter::repeat_n(
                AgentSpec::Argv { argv },
                self.custom_count,
            ));
        }
        v
    }

    /// Member task names podd will create (`<name>-<agent>-<n>`, docs/11 §3.2).
    pub fn member_names(&self, name: &str) -> Vec<String> {
        let mut n: HashMap<&str, usize> = HashMap::new();
        self.specs()
            .iter()
            .map(|s| {
                let kind = match s {
                    AgentSpec::Kind(AgentKind::Claude) => "claude",
                    AgentSpec::Kind(AgentKind::Codex) => "codex",
                    AgentSpec::Kind(AgentKind::Shell) => "shell",
                    AgentSpec::Argv { .. } => "argv",
                };
                let i = n.entry(kind).or_default();
                *i += 1;
                format!("{name}-{kind}-{i}")
            })
            .collect()
    }
}

/// The check command line as argv. Run through `cmd /c` so `.cmd` shims
/// (npm, pnpm) and shell syntax work (podd resolves argv[0] to an `.exe`).
pub fn check_argv(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    (!line.is_empty()).then(|| vec!["cmd".into(), "/c".into(), line.into()])
}

/// The command line shown for a stored check argv (inverse of [`check_argv`]).
pub fn check_line(argv: &[String]) -> String {
    match argv {
        [c, flag, rest @ ..]
            if !rest.is_empty()
                && c.eq_ignore_ascii_case("cmd")
                && flag.eq_ignore_ascii_case("/c") =>
        {
            rest.join(" ")
        }
        _ => argv.join(" "),
    }
}

/// Dialog validation: the reason the group cannot be created yet.
pub fn validate(
    name: &str,
    prompt: &str,
    counts: &AgentCounts,
    has_repo: bool,
) -> Option<&'static str> {
    let total = counts.specs().len();
    if !has_repo {
        Some(crate::strings::CHOOSE_A_REPOSITORY_OR_ENTER_THE_PATH_OF_ONE_TO_REGISTER)
    } else if name.trim().is_empty() {
        Some(crate::strings::ENTER_A_GROUP_NAME)
    } else if !name
        .trim()
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        Some(crate::strings::NAMES_USE_LETTERS_DIGITS_AND_ONLY)
    } else if prompt.trim().is_empty() {
        Some(crate::strings::ENTER_THE_PROMPT_EVERY_AGENT_GETS)
    } else if total == 0 {
        Some(crate::strings::ADD_AT_LEAST_ONE_AGENT)
    } else if total > 16 {
        Some(crate::strings::AT_MOST_16_MEMBERS_PER_GROUP)
    } else if counts.custom_count > 0 && split_command_line(&counts.custom).is_empty() {
        Some(crate::strings::ENTER_THE_CUSTOM_AGENT_S_COMMAND_LINE)
    } else {
        None
    }
}

impl Workspace {
    /// New Group: repo, name, base, prompt, agents with counts, optional check
    /// command -> `group.create`, then the Group panel.
    pub fn open_new_group_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.caps.orch {
            self.raise_toast(
                ToastKind::Warning,
                crate::strings::GROUPS_NEED_A_NEWER_PODD,
                crate::strings::PODD_PROTOCOL_V6_GROUP_IS_REQUIRED,
                cx,
            );
            return;
        }
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
                            .accessibility_label(format!("{title} 줄이기"))
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
                            .accessibility_label(format!("{title} 늘리기"))
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
                    "Claude",
                    crate::strings::PROMPT_AS_CLI_ARGUMENT,
                    |c| c.claude,
                    |c, v| c.claude = v,
                ))
                .child(stepper(
                    "grp-codex",
                    "Codex",
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
            let members = preview_counts.member_names(if group_name.is_empty() {
                "<name>"
            } else {
                &group_name
            });
            let preview = if members.is_empty() {
                crate::strings::NO_MEMBERS_YET.to_string()
            } else {
                crate::tr!(MEMBER_TASK_S_ONE_WORKTREE_EACH; ; members.len(), members.join(", "))
            };
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

    fn create_group(
        &mut self,
        add_repo: Option<String>,
        mut params: p::GroupCreateParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.clone();
        let n = params.agents.len();
        self.raise_toast(
            ToastKind::Info,
            crate::tr!(CREATING_GROUP; ; params.name),
            crate::tr!(N_MEMBER_TASK_S_WORKTREE_AGENT_EACH; n;),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = add_repo {
                match source.add_repo(&path, None).await {
                    Ok(Ok(r)) => params.repo = r.name,
                    Ok(Err(e)) => {
                        let _ = this.update(cx, |w, cx| {
                            w.raise_toast(
                                ToastKind::Error,
                                crate::strings::COULD_NOT_ADD_REPOSITORY,
                                e.message,
                                cx,
                            )
                        });
                        return;
                    }
                    Err(_) => return,
                }
            }
            let title = format!("{}/{}", params.repo, params.name);
            let result = source.call::<p::GroupCreate>(&params).await;
            let _ = this.update_in(cx, |w, window, cx| match result {
                Ok(info) => {
                    // Members are new tasks: re-read the task list now, not in up to 3 s.
                    w.source.refresh_tasks();
                    w.raise_toast(
                        ToastKind::Success,
                        crate::tr!(GROUP_CREATED; ; info.id),
                        crate::tr!(MEMBERS_FROM; ; info.members.len(), info.base),
                        cx,
                    );
                    let id = info.id.clone();
                    w.open_group(&id, Some(info), window, cx);
                }
                Err(e) => w.raise_toast(
                    ToastKind::Error,
                    crate::tr!(COULD_NOT_CREATE_GROUP_TITLE; title;),
                    error_text(&e),
                    cx,
                ),
            });
        })
        .detach();
    }
}

/// Daemon error with its name for the codes the orchestration UI cares about.
pub fn error_text(e: &SourceError) -> String {
    let name = match e.code {
        Some(orch_codes::GROUP_EXISTS) => "GROUP_EXISTS: ",
        Some(orch_codes::GROUP_NOT_FOUND) => "GROUP_NOT_FOUND: ",
        Some(orch_codes::INVALID_STATE) => "INVALID_STATE: ",
        Some(codes::TASK_DIRTY) => "TASK_DIRTY: ",
        _ => "",
    };
    format!("{name}{}", e.message)
}

// ───────────────────────────── Group panel ─────────────────────────────

/// Outcome of the last `group.pick` for the banner.
#[derive(Clone, Debug)]
enum PickOutcome {
    Done(GroupPickResult),
    Failed(String),
}

pub struct GroupPanel {
    pub group_id: String,
    source: PoddSource,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    info: Option<GroupInfo>,
    show_error: Option<String>,
    compare: Option<GroupCompareResult>,
    compare_error: Option<String>,
    compare_loading: bool,
    compare_again: bool,
    /// Members' git summaries the last compare was started for, and the
    /// latest ones from the workspace (`data_changed`).
    compare_sig: String,
    latest_sig: String,
    compare_timer: Option<Task<()>>,
    check_input: Entity<InputState>,
    /// Text for `check_input`, applied on the next render (needs a window).
    pending_check_text: Option<String>,
    check_busy: bool,
    check_error: Option<String>,
    expanded: HashSet<String>,
    pick: Option<PickOutcome>,
}

impl GroupPanel {
    pub fn new(
        source: PoddSource,
        workspace: WeakEntity<Workspace>,
        group_id: String,
        info: Option<GroupInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = info
            .as_ref()
            .and_then(|g| g.check.as_deref())
            .map(check_line)
            .unwrap_or_default();
        let check_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial)
                .placeholder(crate::strings::CHECK_COMMAND_E_G_CARGO_TEST_CMD_C_IN_EACH_WORKTREE)
        });
        let mut this = Self {
            group_id,
            source,
            workspace,
            focus: cx.focus_handle(),
            info,
            show_error: None,
            compare: None,
            compare_error: None,
            compare_loading: false,
            compare_again: false,
            compare_sig: String::new(),
            latest_sig: String::new(),
            compare_timer: None,
            check_input,
            pending_check_text: None,
            check_busy: false,
            check_error: None,
            expanded: HashSet::new(),
            pick: None,
        };
        this.reload_show(cx);
        this.reload_compare(cx);
        this
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    fn toast(&self, kind: ToastKind, title: String, body: String, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        cx.defer(move |cx| {
            let _ = ws.update(cx, |w, cx| w.raise_toast(kind, title, body, cx));
        });
    }

    /// Member task ids (in group order).
    fn member_ids(&self) -> Vec<String> {
        self.info
            .as_ref()
            .map(|g| g.members.iter().map(|m| m.task.clone()).collect())
            .unwrap_or_default()
    }

    // ---- data ------------------------------------------------------------

    fn reload_show(&mut self, cx: &mut Context<Self>) {
        let rx = self.source.call::<p::GroupShow>(&p::GroupRef {
            group: self.group_id.clone(),
        });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                match r {
                    Ok(info) => {
                        let had_check = p.info.as_ref().and_then(|g| g.check.clone());
                        if info.check != had_check
                            && let Some(c) = &info.check
                        {
                            p.pending_check_text = Some(check_line(c));
                        }
                        p.check_busy = info
                            .check_job
                            .as_ref()
                            .is_some_and(|j| j.state == CheckJobState::Running);
                        p.info = Some(info);
                        p.show_error = None;
                    }
                    Err(e) => p.show_error = Some(error_text(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn reload_compare(&mut self, cx: &mut Context<Self>) {
        if self.compare_loading {
            self.compare_again = true;
            return;
        }
        self.compare_loading = true;
        self.compare_sig = self.latest_sig.clone();
        let rx = self.source.call::<p::GroupCompare>(&p::GroupRef {
            group: self.group_id.clone(),
        });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                p.compare_loading = false;
                match r {
                    Ok(c) => {
                        p.compare = Some(c);
                        p.compare_error = None;
                    }
                    Err(e) => p.compare_error = Some(error_text(&e)),
                }
                if std::mem::take(&mut p.compare_again) {
                    p.reload_compare(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The workspace's tasks / terminals / statuses changed (called while the
    /// workspace is being updated: must not read it back, hence `tasks`).
    /// A member's git summary changing means its diff changed: compare again.
    pub fn data_changed(&mut self, tasks: &[TaskVm], cx: &mut Context<Self>) {
        let sig = git_signature(&self.member_ids(), tasks);
        self.latest_sig = sig.clone();
        if self.compare_sig.is_empty() {
            // The first compare ran before the workspace told us anything.
            self.compare_sig = sig;
        } else if self.info.is_some() && sig != self.compare_sig && self.compare_timer.is_none() {
            self.compare_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(COMPARE_DEBOUNCE).await;
                let _ = this.update(cx, |p, cx| {
                    p.compare_timer = None;
                    p.reload_compare(cx);
                });
            }));
        }
        cx.notify();
    }

    /// `group.check` progress from the status stream.
    pub fn check_event(&mut self, e: &p::GroupCheckEvent, cx: &mut Context<Self>) {
        if let Some(info) = self.info.as_mut() {
            info.check_job = Some(e.job.clone());
            if let Some(m) = &e.member
                && let Some(rec) = info.members.iter_mut().find(|x| x.task == m.task)
            {
                rec.check = Some(m.result.clone());
            }
        }
        if let (Some(m), Some(c)) = (&e.member, self.compare.as_mut())
            && let Some(rec) = c.members.iter_mut().find(|x| x.task == m.task)
        {
            rec.check = Some(m.result.clone());
        }
        self.check_busy = e.job.state == CheckJobState::Running;
        if e.member.is_none() && e.job.state != CheckJobState::Running {
            self.reload_show(cx);
        }
        cx.notify();
    }

    fn run_check(&mut self, cx: &mut Context<Self>) {
        let line = self.check_input.read(cx).value().to_string();
        let stored = self.info.as_ref().and_then(|g| g.check.clone());
        let check = check_argv(&line);
        if check.is_none() && stored.is_none() {
            self.check_error = Some(crate::strings::ENTER_A_CHECK_COMMAND_FIRST.into());
            cx.notify();
            return;
        }
        self.check_busy = true;
        self.check_error = None;
        let params = p::GroupCheckParams {
            group: self.group_id.clone(),
            // Store the edited command; keep the stored one when the field is empty.
            check: check.filter(|c| Some(c) != stored.as_ref()),
            ..Default::default()
        };
        let rx = self.source.call::<p::GroupCheck>(&params);
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                match r {
                    Ok(res) => {
                        if let (Some(info), Some(job)) = (p.info.as_mut(), res.job.clone()) {
                            if info.check.as_ref() != Some(&job.argv) {
                                info.check = Some(job.argv.clone());
                            }
                            p.check_busy = job.state == CheckJobState::Running;
                            info.check_job = Some(job);
                        }
                    }
                    Err(e) => {
                        p.check_busy = false;
                        p.check_error = Some(error_text(&e));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn cancel_check(&mut self, cx: &mut Context<Self>) {
        let rx = self.source.call::<p::GroupCheckCancel>(&p::GroupRef {
            group: self.group_id.clone(),
        });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                if let Err(e) = r {
                    p.check_error = Some(error_text(&e));
                }
                p.reload_show(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn open_pick_dialog(&mut self, task: String, window: &mut Window, cx: &mut Context<Self>) {
        let others: Vec<String> = self
            .info
            .as_ref()
            .map(|g| {
                g.members
                    .iter()
                    .filter(|m| m.task != task && !m.removed)
                    .map(|m| m.task.clone())
                    .collect()
            })
            .unwrap_or_default();
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

    fn pick(
        &mut self,
        task: String,
        remove_others: bool,
        force: bool,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let params = p::GroupPickParams {
            group: self.group_id.clone(),
            task: task.clone(),
            remove_others,
            force,
            delete_branch,
        };
        let rx = self.source.call::<p::GroupPick>(&params);
        cx.spawn_in(window, async move |this, cx| {
            let r = rx.await;
            let _ = this.update_in(cx, |p, window, cx| {
                match r {
                    Ok(res) => {
                        let closed: Vec<String> = res
                            .removed
                            .iter()
                            .flat_map(|x| x.terminals.clone())
                            .collect();
                        if let Some(ws) = p.workspace.upgrade() {
                            ws.update(cx, |w, cx| {
                                for t in &closed {
                                    w.close_panels_of(t, window, cx);
                                }
                            });
                        }
                        let removed: Vec<&str> =
                            res.removed.iter().map(|x| x.id.as_str()).collect();
                        let body = if removed.is_empty() {
                            crate::strings::NO_MEMBERS_REMOVED.to_string()
                        } else {
                            crate::tr!(REMOVED; ; removed.join(", "))
                        };
                        p.toast(
                            ToastKind::Success,
                            crate::tr!(WINNER; ; res.picked),
                            body,
                            cx,
                        );
                        if !res.kept.is_empty() {
                            p.confirm_force(
                                task.clone(),
                                res.kept.clone(),
                                delete_branch,
                                window,
                                cx,
                            );
                        }
                        p.pick = Some(PickOutcome::Done(res));
                    }
                    Err(e) => {
                        let msg = error_text(&e);
                        p.toast(
                            ToastKind::Error,
                            crate::tr!(COULD_NOT_PICK_TASK; task;),
                            msg.clone(),
                            cx,
                        );
                        p.pick = Some(PickOutcome::Failed(msg));
                    }
                }
                p.reload_show(cx);
                p.reload_compare(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// podd kept members with work in them: show why, offer "Remove anyway".
    fn confirm_force(
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

    fn open_terminal(&self, task: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspace.upgrade() else {
            return;
        };
        let task = task.to_string();
        cx.defer_in(window, move |_, window, cx| {
            ws.update(cx, |w, cx| {
                match w.task(&task).and_then(|t| t.terminals.first().cloned()) {
                    Some(t) => w.open_terminal(&t, false, window, cx),
                    None => w.raise_toast(
                        ToastKind::Warning,
                        crate::strings::NO_TERMINAL,
                        crate::tr!(TASK_HAS_NO_TERMINAL_REMOVED; task;),
                        cx,
                    ),
                }
            });
        });
    }

    fn open_review(&self, task: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspace.upgrade() else {
            return;
        };
        let task = task.to_string();
        cx.defer_in(window, move |_, window, cx| {
            ws.update(cx, |w, cx| w.open_review(&task, window, cx))
        });
    }

    // ---- rendering --------------------------------------------------------

    fn render_header(&self, live: &MemberLive, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let info = self.info.as_ref();
        let job = info.and_then(|g| g.check_job.clone());
        let running = job
            .as_ref()
            .is_some_and(|j| j.state == CheckJobState::Running);
        let summary = &live.summary;
        let base = info
            .map(|g| crate::tr!(BASE_253; ; g.base, &g.base_commit[..g.base_commit.len().min(9)]))
            .unwrap_or_default();
        let members = info
            .map(|g| g.members.iter().filter(|m| !m.removed).count())
            .unwrap_or(0);
        let winner = info.and_then(|g| g.picked.clone());
        Frame::new("group-head", FrameKind::List, crate::tr!(GROUP; ; self.group_id), crate::tr!(BASE_MEMBERS_MEMBER_S; base,members; summary_short(summary)))
            .render(
                mode(),
                [
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .when_some(summary_badge(summary), |d, b| d.child(b.render(mode())))
                        .when_some(winner, |d, w| {
                            let ws = self.workspace.clone();
                            let task = w.clone();
                            d.child(div().px_2().py_1().rounded_sm().bg(tc(t.success_bg)).child(pod_ui::icon_text(pod_ui::Icon::CircleCheck, t.success_fg, crate::tr!(WINNER_W; w;)))).child(
                                // P6: the winner goes on to a pull request (docs/15).
                                Button::new("grp-winner-pr")
                                    .label(crate::pr_panel::s::CREATE_PR_FOR_WINNER)
                                    .xsmall()
                                    .tooltip(crate::pr_panel::s::CREATE_PR_FOR_WINNER_TIP)
                                    .on_click(cx.listener(move |_, _, window, cx| {
                                        let (ws, task) = (ws.clone(), task.clone());
                                        cx.defer_in(window, move |_, window, cx| {
                                            let _ = ws.update(cx, |w, cx| w.open_pr(&task, true, window, cx));
                                        });
                                    })),
                            )
                        })
                        .into_any_element(),
                    div()
                        .text_xs()
                        .text_color(tc(t.text_secondary))
                        .max_h(px(60.))
                        .overflow_hidden()
                        .child(crate::tr!(PROMPT_257; ; info.map(|g| g.prompt.clone()).unwrap_or_default()))
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
                                .on_click(cx.listener(|p, _, _, cx| {
                                    p.reload_show(cx);
                                    p.reload_compare(cx);
                                })),
                        )
                        .into_any_element(),
                    self.render_job_line(job.as_ref()).into_any_element(),
                    self.render_pick_banner().into_any_element(),
                ],
            )
            .max_w_full()
    }

    fn render_job_line(&self, job: Option<&CheckJob>) -> impl IntoElement {
        let t = tokens();
        let (text, fg) = match job {
            _ if self.check_error.is_some() => {
                (self.check_error.clone().unwrap_or_default(), t.danger_fg)
            }
            None => (crate::strings::NO_CHECK_RUN_YET.to_string(), t.text_muted),
            Some(j) => {
                let results: Vec<(String, Option<bool>)> = self
                    .info
                    .iter()
                    .flat_map(|g| g.members.iter())
                    .filter(|m| j.tasks.contains(&m.task))
                    .map(|m| {
                        (
                            m.task.clone(),
                            m.check
                                .as_ref()
                                .filter(|_| j.done.contains(&m.task))
                                .map(|c| c.passed),
                        )
                    })
                    .collect();
                let passed = results.iter().filter(|r| r.1 == Some(true)).count();
                let failed = results.iter().filter(|r| r.1 == Some(false)).count();
                let line = check_line(&j.argv);
                match j.state {
                    CheckJobState::Running => (
                        crate::tr!(CHECK_PROGRESS_REPORT; line,passed,failed; j.id, j.done.len(), j.tasks.len()),
                        t.info_fg,
                    ),
                    CheckJobState::Finished if j.all_passed == Some(true) => (
                        crate::tr!(CHECK_PASSED_REPORT; line,passed; j.id),
                        t.success_fg,
                    ),
                    CheckJobState::Finished => (
                        crate::tr!(CHECK_FAILED_REPORT; line,failed,passed; j.id),
                        t.danger_fg,
                    ),
                    CheckJobState::Cancelled => {
                        (crate::tr!(CHECK_CANCELLED_REPORT; line; j.id), t.warning_fg)
                    }
                    CheckJobState::Interrupted => (
                        crate::tr!(CHECK_INTERRUPTED_REPORT; line; j.id),
                        t.warning_fg,
                    ),
                }
            }
        };
        div()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(tc(fg))
            .child(text)
    }

    fn render_pick_banner(&self) -> impl IntoElement {
        let t = tokens();
        match &self.pick {
            None => div(),
            Some(PickOutcome::Failed(m)) => div().text_sm().text_color(tc(t.danger_fg)).child(crate::tr!(PICK_REFUSED_M; m;)),
            Some(PickOutcome::Done(r)) => div()
                .flex()
                .flex_col()
                .text_sm()
                .child(div().text_color(tc(t.success_fg)).child(crate::tr!(PICK_RESULT; ;
                    r.picked,
                    if r.removed.is_empty() {
                        String::new()
                    } else {
                        crate::tr!(REMOVED_269; ; r.removed.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", "))
                    }
                )))
                .children(r.kept.iter().map(|k| div().text_color(tc(t.warning_fg)).child(crate::tr!(KEPT; ; k.task, k.message)))),
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
        let short = m.task.rsplit('/').next().unwrap_or(&m.task).to_string();
        let task = live.tasks.get(&m.task);
        let picked = m.picked
            || self.info.as_ref().and_then(|g| g.picked.as_deref()) == Some(m.task.as_str());
        let job = self.info.as_ref().and_then(|g| g.check_job.as_ref());
        let checking = job.is_some_and(|j| {
            j.state == CheckJobState::Running
                && j.tasks.contains(&m.task)
                && !j.done.contains(&m.task)
        });
        let check = cmp.and_then(|c| c.check.as_ref()).or(m.check.as_ref());
        let badge = live.badges.get(&m.task).cloned();
        let branch = task
            .and_then(|t| t.branch.clone())
            .or_else(|| m.info.as_ref().map(|i| i.branch.clone()))
            .unwrap_or_else(|| "—".into());
        let border = if picked {
            t.success_fg
        } else if check.is_some_and(|c| !c.passed) && !checking {
            t.danger_fg
        } else {
            t.border_subtle
        };
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
                let counts = if f.binary {
                    "bin".to_string()
                } else {
                    format!("+{} −{}", f.added.unwrap_or(0), f.deleted.unwrap_or(0))
                };
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
                                .on_click(cx.listener(move |p, _, _, cx| {
                                    if !p.expanded.remove(&id_toggle) {
                                        p.expanded.insert(id_toggle.clone());
                                    }
                                    cx.notify();
                                })),
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
            .border_color(tc(border))
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
                            .child(short),
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
            .child(match (m.removed, badge) {
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
                    .child(format!("⎇ {branch}")),
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

/// Git summary of every member (ahead/behind/dirty/last commit) from the
/// workspace's task poll.
pub fn git_signature(members: &[String], tasks: &[TaskVm]) -> String {
    members
        .iter()
        .map(|id| match tasks.iter().find(|t| &t.id == id) {
            Some(t) => format!(
                "{id}:{:?}:{}",
                t.git_summary(),
                t.git
                    .as_ref()
                    .and_then(|g| g.last_commit.clone())
                    .unwrap_or_default()
            ),
            None => format!("{id}:-"),
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Live data of the members from the workspace, gathered before rendering.
struct MemberLive {
    tasks: HashMap<String, TaskVm>,
    badges: HashMap<String, pod_ui::StatusBadge>,
    summary: StatusSummary,
}

impl MemberLive {
    fn gather(members: &[GroupMember], ws: Option<&Workspace>) -> Self {
        let mut tasks = HashMap::new();
        let mut badges = HashMap::new();
        let mut all: Vec<String> = Vec::new();
        if let Some(ws) = ws {
            for m in members.iter().filter(|m| !m.removed) {
                // The workspace's task poll (3 s) may not know a new member yet:
                // fall back to the terminals in the group record.
                let terms: Vec<String> = match ws.task(&m.task) {
                    Some(t) => {
                        tasks.insert(m.task.clone(), t.clone());
                        t.terminals.clone()
                    }
                    None => m
                        .info
                        .as_ref()
                        .map(|i| i.terminals.iter().map(|x| x.id.clone()).collect())
                        .unwrap_or_default(),
                };
                // The agent terminal (first) stands for the member; others count in the summary.
                if let Some(term) = terms.first().and_then(|x| ws.terminal(x)) {
                    badges.insert(m.task.clone(), terminal_badge(term));
                }
                all.extend(terms);
            }
        }
        let terminals: &[TerminalVm] = ws.map(|w| w.terminals.as_slice()).unwrap_or(&[]);
        Self {
            summary: summary_of(all.iter(), terminals),
            tasks,
            badges,
        }
    }
}

/// `check passed · exit 0 · 1.2 s` and its color (the icon is drawn by the caller).
pub fn check_summary(c: &CheckResult) -> (String, u32) {
    let t = tokens();
    let secs = c.duration_ms as f64 / 1000.;
    if let Some(e) = &c.error {
        (crate::tr!(CHECK_ERROR_E; e;), t.danger_fg)
    } else if c.timed_out {
        (
            crate::tr!(CHECK_TIMED_OUT_AFTER_SECS_1_S; secs;),
            t.danger_fg,
        )
    } else if c.passed {
        (
            crate::tr!(CHECK_PASSED_EXIT_SECS_1_S; secs; c.exit_code.unwrap_or(0)),
            t.success_fg,
        )
    } else {
        (
            crate::tr!(CHECK_FAILED_EXIT_SECS_1_S; secs; c.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())),
            t.danger_fg,
        )
    }
}

/// Last `n` non-empty lines of a check's output tail.
pub fn tail_lines(s: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = s
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}

impl EventEmitter<PanelEvent> for GroupPanel {}

impl Focusable for GroupPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for GroupPanel {
    fn panel_name(&self) -> &'static str {
        GROUP_PANEL_NAME
    }

    /// Persisted shape; restored at start while the group exists (`layout::prune`).
    fn dump(&self, _cx: &App) -> PanelState {
        let mut s = PanelState::new(GROUP_PANEL_NAME);
        s.info = PanelInfo::panel(json!({ "group": self.group_id }));
        s
    }

    fn on_removed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.compare_timer = None;
        if let Some(ws) = self.workspace.upgrade() {
            cx.defer(move |cx| ws.update(cx, |w, cx| w.orch_panel_removed(cx)));
        }
    }
}

impl gpui_kit::component::dock::Panel for GroupPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(crate::tr!(GROUP_290; ; self.group_id).into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(crate::tr!(GROUP_290; ; self.group_id))
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
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
            MemberLive::gather(&members, ws.as_ref().map(|w| w.read(cx)))
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

#[cfg(test)]
mod tests {
    use super::{
        AgentCounts, check_argv, check_line, check_summary, error_text, tail_lines, validate,
    };
    use crate::source::SourceError;
    use podd::protocol::{AgentSpec, CheckResult, orch_codes};

    #[test]
    fn specs_and_member_names_follow_podd_rules() {
        let c = AgentCounts {
            claude: 1,
            codex: 2,
            shell: 1,
            custom: r#"py -c "print(1)""#.into(),
            custom_count: 1,
        };
        let specs = c.specs();
        assert_eq!(specs.len(), 5);
        assert_eq!(
            specs[4],
            AgentSpec::Argv {
                argv: vec!["py".into(), "-c".into(), "print(1)".into()]
            }
        );
        assert_eq!(
            c.member_names("fx"),
            [
                "fx-claude-1",
                "fx-codex-1",
                "fx-codex-2",
                "fx-shell-1",
                "fx-argv-1"
            ]
        );
        // A custom count without a command line adds nothing.
        let none = AgentCounts {
            custom_count: 2,
            ..Default::default()
        };
        assert!(none.specs().is_empty());
    }

    #[test]
    fn validation_messages() {
        let one = AgentCounts {
            shell: 1,
            ..Default::default()
        };
        assert!(
            validate("fx", "do it", &one, false)
                .unwrap()
                .contains("저장소")
        );
        assert!(validate("", "do it", &one, true).unwrap().contains("이름"));
        assert!(
            validate("a b", "do it", &one, true)
                .unwrap()
                .contains("영문")
        );
        assert!(validate("fx", "  ", &one, true).unwrap().contains("요청"));
        assert!(
            validate("fx", "x", &AgentCounts::default(), true)
                .unwrap()
                .contains("에이전트")
        );
        assert!(
            validate(
                "fx",
                "x",
                &AgentCounts {
                    shell: 17,
                    ..Default::default()
                },
                true
            )
            .unwrap()
            .contains("16")
        );
        assert!(
            validate(
                "fx",
                "x",
                &AgentCounts {
                    custom_count: 1,
                    ..Default::default()
                },
                true
            )
            .is_some()
        );
        assert_eq!(validate("fx.1", "x", &one, true), None);
    }

    #[test]
    fn check_command_round_trip() {
        assert_eq!(
            check_argv("  cargo test  ").unwrap(),
            ["cmd", "/c", "cargo test"]
        );
        assert!(check_argv("  ").is_none());
        assert_eq!(
            check_line(&check_argv("npm test -- --ci").unwrap()),
            "npm test -- --ci"
        );
        assert_eq!(check_line(&["cargo".into(), "test".into()]), "cargo test");
        // `pod group new ... -- cmd /c findstr /c:PASS result.txt` stores separate args.
        let cli: Vec<String> = ["cmd", "/c", "findstr", "/c:PASS", "result.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(check_line(&cli), "findstr /c:PASS result.txt");
    }

    #[test]
    fn check_summaries_report_failure_as_failure() {
        let mut c = CheckResult {
            argv: vec![],
            passed: false,
            exit_code: Some(1),
            timed_out: false,
            duration_ms: 1500,
            finished_ms: 0,
            output_tail: "a\n\nb\r\nc\n".into(),
            error: None,
        };
        assert!(check_summary(&c).0.contains("검사 실패 · 종료 1 · 1.5초"));
        c.timed_out = true;
        assert!(check_summary(&c).0.contains("시간 초과"));
        c.timed_out = false;
        c.passed = true;
        c.exit_code = Some(0);
        assert!(
            check_summary(&c).0.starts_with("검사 통과"),
            "no text glyph: the row draws an icon"
        );
        assert_eq!(tail_lines(&c.output_tail, 2), ["b", "c"]);
    }

    #[test]
    fn error_names() {
        let e = SourceError {
            code: Some(orch_codes::GROUP_EXISTS),
            message: "group demo/fx exists".into(),
        };
        assert_eq!(error_text(&e), "GROUP_EXISTS: group demo/fx exists");
    }
}
