//! The tree sidebar: which rows it has ([`TreeData`]), what a task row says
//! ([`TaskRow`]), its view, the keyboard tree model and accessible names.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{ContextMenuExt, PopupMenuItem};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::qa_marks::QaId;
use crate::vm::{RepoVm, TaskVm, TerminalVm};

use super::Workspace;

impl Workspace {
    pub(crate) fn sidebar_focus_handle(&self, cx: &App) -> FocusHandle {
        self.sidebar.read(cx).focus.clone()
    }

    #[cfg(test)]
    pub(crate) fn sidebar_filter(&self, cx: &App) -> Entity<InputState> {
        self.sidebar.read(cx).filter.clone()
    }

    /// Type `query` into the sidebar search (render tests).
    #[cfg(test)]
    pub(crate) fn set_sidebar_query(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let filter = self.sidebar_filter(cx);
        filter.update(cx, |s, cx| s.set_value(query.to_string(), window, cx));
        self.sidebar.update(cx, |_, cx| cx.notify());
    }

    // ---- rendering ------------------------------------------------------

    fn render_sidebar(&self, ws: &WeakEntity<Self>, list: AnyElement) -> impl IntoElement {
        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        // Icon buttons keep the earlier text labels as accessible names (UIA/QA unchanged).
        let button = |id: &'static str,
                      glyph: pod_ui::Icon,
                      name: &'static str,
                      tip: &'static str,
                      f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            let ws = ws.clone();
            pod_ui::IconButton::new(id, glyph, name)
                .tooltip(tip)
                .render(mode, move |window, cx| {
                    let _ = ws.update(cx, |w, cx| f(w, window, cx));
                })
        };
        div()
            .qa_id(|| "sidebar".into())
            .w(px(m.density.sidebar))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(t.bg_chrome))
            .when(m.surface == pod_ui::themes::Surface::Flat, |d| d.border_r(px(m.border_width)).border_color(rgb(t.border_subtle)))
            .child(
                // One compact header row (docs/10 §D4.6): title + icon actions, never wrapping.
                div()
                    .h(px(m.density.panel_header.max(32.)))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl_3()
                    .pr_1()
                    .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(crate::strings::YOUR_WORKSPACE))
                    .child(
                        div()
                            .qa_id(|| "sidebar-actions".into())
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(2.))
                            .child(button("grid", pod_ui::Icon::LayoutGrid, crate::strings::GRID, crate::strings::ARRANGE_OPEN_PANELS_IN_A_GRID_CTRL_SHIFT_G, |w, window, cx| w.arrange_grid(window, cx)))
                            .when(self.caps.tasks, |d| {
                                d.child(button("new-task", pod_ui::Icon::Plus, crate::strings::TASK_107, crate::strings::NEW_TASK_GIT_WORKTREE_AGENT_TERMINAL, |w, window, cx| w.open_new_task_dialog(window, cx)))
                            })
                            .child(button("new", pod_ui::Icon::SquareTerminal, crate::strings::NEW, crate::strings::NEW_TERMINAL_CTRL_SHIFT_T, |w, window, cx| w.open_new_dialog(window, cx)))
                            .when(self.caps.orch, |d| {
                                d.child(button("new-group", pod_ui::Icon::Users, crate::strings::GROUP_330, crate::strings::FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS_CTRL_SHIFT_N, |w, window, cx| w.open_new_group_dialog(window, cx)))
                                    .child(button("agent-tree", pod_ui::Icon::ListTree, crate::strings::AGENT, crate::strings::AGENT_TREE_GROUPS_TASKS_AND_TERMINALS_STARTED_FROM_OTHER_TERMINAL, |w, window, cx| w.open_agent_tree(window, cx)))
                            }),
                    ),
            )
            .child(list)
            // Remote machines (docs/17 R4); draws nothing without profiles.
            .child(self.machines.clone())
    }

    fn render_row(&self, t: &TerminalVm, ws: &WeakEntity<Self>) -> impl IntoElement {
        let mut badge = crate::orch::terminal_badge(t);
        if !self.conn.is_connected() {
            badge.stale_since = Some(crate::strings::CONNECTION_LOST.into());
        }
        let detail = terminal_row_detail(t, &badge);
        let id = t.id.clone();
        let open_ws = ws.clone();
        let menu_ws = ws.clone();
        let selected = self.focused_terminal.as_deref() == Some(&t.id);
        let tk = crate::theme::tokens();
        let agent = crate::agent_kind::identify_in(t, &self.tasks).kind;
        // Under a task: chevron slot (16) + gap; loose terminals sit at the task level.
        let indent = if self.tasks.iter().any(|task| task.terminals.contains(&t.id)) {
            34.
        } else {
            12.
        };
        div()
            .id(SharedString::from(format!("row-{id}")))
            .qa_id({
                let id = id.clone();
                move || format!("row-{id}")
            })
            .role(Role::TreeItem)
            .aria_label(format!(
                "{} · {} | {detail}",
                self.label_of(t),
                agent.accessible_name()
            ))
            .w_full()
            .h_full()
            .flex()
            .items_center()
            .gap_2()
            .pl(px(indent))
            .pr_2()
            .min_w_0()
            .border_l_2()
            .border_color(if selected {
                rgb(tk.accent_fg)
            } else {
                gpui_kit::transparent_black().into()
            })
            .when(selected, |d| d.bg(rgb(tk.bg_selected)))
            .hover(|s| {
                s.bg(rgb(if selected {
                    tk.bg_selected
                } else {
                    tk.bg_hover
                }))
            })
            // Worker row (docs/10 §D4.6, §D5.11): agent mark + name + status icon and text on the right.
            .child(crate::agent_kind::mark_el(
                &format!("row-{id}"),
                &agent,
                pod_ui::tokens::ICON_SIZE,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(if selected {
                        tk.text_primary
                    } else {
                        tk.text_secondary
                    }))
                    .child(self.label_of(t)),
            )
            .child(badge.render_compact(crate::theme::mode()))
            .on_click(move |_, window, cx| {
                let _ = open_ws.update(cx, |w, cx| w.open_terminal(&id, false, window, cx));
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(detail.clone()).build(window, cx)
            })
            .context_menu({
                let id = t.id.clone();
                let running = t.is_running();
                move |menu, _, _| {
                    let ws = menu_ws.clone();
                    let id = id.clone();
                    menu.item(
                        PopupMenuItem::new(if running {
                            crate::strings::KILL_TERMINAL_112
                        } else {
                            crate::strings::REMOVE_EXITED_TERMINAL
                        })
                        .on_click(move |_, window, cx| {
                            let _ = ws.update(cx, |w, cx| {
                                if running {
                                    w.confirm_kill(id.clone(), window, cx)
                                } else {
                                    w.remove(id.clone(), window, cx)
                                }
                            });
                        }),
                    )
                }
            })
    }
}

/// The sidebar as a separate view so it can be embedded with `cached`: it is
/// rebuilt only when the workspace data changes, not on every terminal frame.
#[derive(Clone, Debug, PartialEq)]
enum SidebarItem {
    /// (name, accessible label `name · 작업 N`, task count)
    Repo(String, String, usize),
    Task(String),
    Terminal(String),
}

impl SidebarItem {
    fn key(&self) -> String {
        match self {
            Self::Repo(id, _, _) => format!("repo:{id}"),
            Self::Task(id) => format!("task:{id}"),
            Self::Terminal(id) => format!("terminal:{id}"),
        }
    }
}

pub struct Sidebar {
    ws: WeakEntity<Workspace>,
    pub(super) focus: FocusHandle,
    pub(crate) selected: Option<String>,
    scroll: UniformListScrollHandle,
    filter: Entity<InputState>,
    _filter_sub: Subscription,
}

impl Sidebar {
    pub(super) fn new(
        ws: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder(crate::strings::SIDEBAR_SEARCH));
        let sub = cx.subscribe(
            &filter,
            |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify(),
        );
        // `.tab_index()` only applies to element-owned handles; a tracked handle must be a tab stop itself.
        Self {
            ws,
            focus: cx.focus_handle().tab_stop(true),
            selected: None,
            scroll: UniformListScrollHandle::new(),
            filter,
            _filter_sub: sub,
        }
    }

    fn items(&self, cx: &App) -> Vec<SidebarItem> {
        let Some(ws) = self.ws.upgrade() else {
            return vec![];
        };
        let w = ws.read(cx);
        let query = self.filter.read(cx).value().to_lowercase();
        let query = query.trim();
        let empty_collapsed = Default::default();
        let collapsed = if query.is_empty() {
            &cx.global::<crate::theme::Settings>().collapsed
        } else {
            &empty_collapsed
        };
        let label = |t: &TerminalVm| w.label_of(t);
        TreeData::of(w, &label).rows(collapsed, query)
    }
    /// Tree depth of every row key ([`TreeData::depths`]).
    fn depths(&self, cx: &App) -> HashMap<String, usize> {
        let Some(ws) = self.ws.upgrade() else {
            return HashMap::new();
        };
        let w = ws.read(cx);
        let label = |t: &TerminalVm| w.label_of(t);
        TreeData::of(w, &label).depths()
    }
    fn activate(&mut self, item: &SidebarItem, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(item.key());
        match item {
            SidebarItem::Terminal(id) => {
                let _ = self
                    .ws
                    .update(cx, |w, cx| w.open_terminal(id, false, window, cx));
            }
            _ => {
                let mut settings = cx.global::<crate::theme::Settings>().clone();
                let key = item.key();
                if !settings.collapsed.remove(&key) {
                    settings.collapsed.insert(key);
                }
                let _ = settings.save();
                cx.set_global(settings);
                cx.notify();
            }
        }
    }
}

/// The workspace data the tree shows, borrowed for one pass: which rows there
/// are and how deep, decided apart from drawing them.
struct TreeData<'a> {
    repos: &'a [RepoVm],
    tasks: &'a [TaskVm],
    terminals: &'a [TerminalVm],
    /// A terminal's display name (`Workspace::label_of`).
    label: &'a dyn Fn(&TerminalVm) -> String,
}

impl<'a> TreeData<'a> {
    fn of(w: &'a Workspace, label: &'a dyn Fn(&TerminalVm) -> String) -> Self {
        Self {
            repos: &w.repos,
            tasks: &w.tasks,
            terminals: &w.terminals,
            label,
        }
    }

    fn terminal(&self, id: &str) -> Option<&'a TerminalVm> {
        self.terminals.iter().find(|t| t.id == id)
    }

    fn task(&self, id: &str) -> Option<&'a TaskVm> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// The rows: every repo (registered or named by a task) with its tasks and
    /// their terminals unless folded in `collapsed`, then the tasks without a
    /// repo, then terminals of no task. A non-empty `query` (lower case) keeps
    /// the rows that match it or lead to a match; the caller passes no folds
    /// while searching.
    fn rows(&self, collapsed: &BTreeSet<String>, query: &str) -> Vec<SidebarItem> {
        let mut rows = Vec::new();
        let mut included = HashSet::new();
        let repos: std::collections::BTreeSet<_> = self
            .repos
            .iter()
            .map(|r| r.name.clone())
            .chain(self.tasks.iter().filter_map(|t| t.repo.clone()))
            .collect();
        for repo in repos {
            let tasks: Vec<_> = self
                .tasks
                .iter()
                .filter(|t| t.repo.as_deref() == Some(&repo))
                .collect();
            rows.push(SidebarItem::Repo(
                repo.clone(),
                crate::tr!(TASKS_123; ; repo, tasks.len()),
                tasks.len(),
            ));
            for t in &tasks {
                included.extend(t.terminals.iter().cloned());
            }
            if collapsed.contains(&format!("repo:{repo}")) {
                continue;
            }
            for t in tasks {
                rows.push(SidebarItem::Task(t.id.clone()));
                if !collapsed.contains(&format!("task:{}", t.id)) {
                    for id in &t.terminals {
                        if self.terminal(id).is_some() {
                            rows.push(SidebarItem::Terminal(id.clone()));
                        }
                    }
                }
            }
        }
        for t in self.tasks {
            if t.repo.is_none() {
                rows.push(SidebarItem::Task(t.id.clone()));
                for id in &t.terminals {
                    included.insert(id.clone());
                    if !collapsed.contains(&format!("task:{}", t.id)) {
                        rows.push(SidebarItem::Terminal(id.clone()));
                    }
                }
            }
        }
        for t in self.terminals {
            if !included.contains(&t.id) {
                rows.push(SidebarItem::Terminal(t.id.clone()));
            }
        }
        if !query.is_empty() {
            // Agent kinds match too: `claude`, `codex`, `셸` (docs/10 §D5.11).
            let matches_terminal = |id: &str| {
                self.terminal(id).is_some_and(|t| {
                    let kind = crate::agent_kind::identify_in(t, self.tasks).kind;
                    format!(
                        "{} {} {} {}",
                        (self.label)(t),
                        t.id,
                        t.program,
                        kind.search_terms()
                    )
                    .to_lowercase()
                    .contains(query)
                })
            };
            let matches_task_text = |t: &TaskVm| {
                format!(
                    "{} {} {} {}",
                    t.title,
                    t.id,
                    t.branch.as_deref().unwrap_or(""),
                    t.repo.as_deref().unwrap_or("")
                )
                .to_lowercase()
                .contains(query)
            };
            // The task's own agent keeps the task row, not its other (e.g. Codex) terminals.
            let matches_task = |t: &TaskVm| {
                matches_task_text(t)
                    || t.agent
                        .as_deref()
                        .and_then(crate::agent_kind::from_task_agent)
                        .is_some_and(|k| k.search_terms().contains(query))
            };
            rows.retain(|row| match row {
                SidebarItem::Repo(repo, _, _) => {
                    repo.to_lowercase().contains(query)
                        || self.tasks.iter().any(|t| {
                            t.repo.as_ref() == Some(repo)
                                && (matches_task(t)
                                    || t.terminals.iter().any(|id| matches_terminal(id)))
                        })
                }
                SidebarItem::Task(id) => self.task(id).is_some_and(|t| {
                    matches_task(t) || t.terminals.iter().any(|id| matches_terminal(id))
                }),
                SidebarItem::Terminal(id) => {
                    matches_terminal(id)
                        || self
                            .tasks
                            .iter()
                            .any(|t| t.terminals.contains(id) && matches_task_text(t))
                }
            });
        }
        rows
    }

    /// Tree depth of every row key: repo 0, task 1 (0 without a repo), terminal one below its task.
    fn depths(&self) -> HashMap<String, usize> {
        let mut out = HashMap::new();
        for t in self.tasks {
            let depth = usize::from(t.repo.is_some());
            if let Some(repo) = &t.repo {
                out.insert(format!("repo:{repo}"), 0);
            }
            out.insert(format!("task:{}", t.id), depth);
            for id in &t.terminals {
                out.insert(format!("terminal:{id}"), depth + 1);
            }
        }
        out
    }
}

/// What a task row shows (docs/10 §D4.6): an 8px status dot, the title, and
/// the needs-you count (failures before answers) or else how many of its
/// terminals there are; the tooltip is `branch · summary`.
#[derive(Debug, PartialEq)]
struct TaskRow {
    title: String,
    dot: u32,
    /// The count badge: (count, foreground, background) tokens.
    badge: Option<(usize, u32, u32)>,
    total: usize,
    tooltip: String,
}

impl TaskRow {
    /// `task`: `None` once it left the list (an empty row); `terminals`: all of them.
    fn new(task: Option<&TaskVm>, terminals: &[TerminalVm], tk: &pod_ui::Tokens) -> Self {
        let members: Vec<_> = task
            .into_iter()
            .flat_map(|t| t.terminals.iter())
            .filter_map(|id| terminals.iter().find(|t| t.id == *id))
            .collect();
        let summary = crate::orch::summary_of(members.iter().map(|t| &t.id), terminals);
        let dot = match summary.priority() {
            Some(state) => pod_ui::status_visual(state).tone.colors(*tk).0,
            None => tk.text_muted,
        };
        let badge = if summary.error > 0 {
            Some((summary.error, tk.danger_fg, tk.danger_bg))
        } else if summary.blocked > 0 {
            Some((summary.blocked, tk.warning_fg, tk.warning_bg))
        } else {
            None
        };
        Self {
            title: task.map(|t| t.title.clone()).unwrap_or_default(),
            dot,
            badge,
            total: summary.total,
            tooltip: format!(
                "{} · {}",
                task.and_then(|t| t.branch.as_deref()).unwrap_or("—"),
                summary.label()
            ),
        }
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else {
            return div().into_any_element();
        };
        let items = Rc::new(self.items(cx));
        let rows = items.clone();
        let depths = self.depths(cx);
        // One row height per theme density (docs/10 §D4.6: task and worker are single lines).
        let height = pod_ui::themes::metrics().density.row;
        let collapsed = cx.global::<crate::theme::Settings>().collapsed.clone();
        let filtering = !self.filter.read(cx).value().trim().is_empty();
        let list = uniform_list(
            "workspace-tree",
            items.len(),
            cx.processor(move |sidebar, range: std::ops::Range<usize>, _, cx| {
                let Some(ws) = sidebar.ws.upgrade() else {
                    return Vec::new();
                };
                let tk = crate::theme::tokens();
                let mode = crate::theme::mode();
                // The tree owns the one chevron (docs/10 §D4.5); leaves get none.
                let chevron = |key: &str| {
                    pod_ui::icon_sized(
                        if !filtering && collapsed.contains(key) {
                            pod_ui::Icon::ChevronRight
                        } else {
                            pod_ui::Icon::ChevronDown
                        },
                        14.,
                        tk.text_muted,
                    )
                };
                range
                    .map(|i| {
                        let item = rows[i].clone();
                        let key = item.key();
                        let action = item.clone();
                        let row = match &item {
                            SidebarItem::Terminal(id) => ws
                                .read(cx)
                                .terminal(id)
                                .map(|t| ws.read(cx).render_row(t, &sidebar.ws).into_any_element())
                                .unwrap_or_else(|| div().into_any_element()),
                            SidebarItem::Repo(name, _, count) => div()
                                .w_full()
                                .min_w_0()
                                .flex()
                                .items_center()
                                .gap_2()
                                .pl_2()
                                .pr_3()
                                .text_color(rgb(tk.text_secondary))
                                .child(chevron(&key))
                                .child(pod_ui::icon_sized(
                                    pod_ui::Icon::FolderGit2,
                                    14.,
                                    tk.text_muted,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(rgb(tk.text_muted))
                                        .child(crate::tr!(TASKS; ; count)),
                                )
                                .into_any_element(),
                            SidebarItem::Task(id) => {
                                let w = ws.read(cx);
                                let task = w.task(id);
                                let kinds = task
                                    .map(|k| crate::agent_kind::task_kinds(k, &w.terminals))
                                    .unwrap_or_default();
                                // 8px status dot + title + count badge: needs-you counts stay visible (docs/10 §D4.6).
                                let TaskRow {
                                    title,
                                    dot,
                                    badge,
                                    total,
                                    tooltip,
                                } = TaskRow::new(task, &w.terminals, &tk);
                                let _ = mode;
                                div()
                                    .id(SharedString::from(format!("task-label-{id}")))
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .pl(px(14.))
                                    .pr_3()
                                    .child(chevron(&key))
                                    .child(pod_ui::status_dot(dot))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(title),
                                    )
                                    // The task's agents as small marks (docs/10 §D5.11).
                                    .when(!kinds.is_empty(), |d| {
                                        d.child(crate::agent_kind::stack_el(
                                            &format!("task-{id}"),
                                            &kinds,
                                            12.,
                                        ))
                                    })
                                    .child(match badge {
                                        Some((n, fg, bg)) => {
                                            pod_ui::count_badge(n, fg, bg).into_any_element()
                                        }
                                        None => div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(rgb(tk.text_muted))
                                            .child(total.to_string())
                                            .into_any_element(),
                                    })
                                    .tooltip({
                                        let detail = tooltip;
                                        move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                detail.clone(),
                                            )
                                            .build(window, cx)
                                        }
                                    })
                                    .context_menu({
                                        let id = id.clone();
                                        let ws = sidebar.ws.clone();
                                        move |menu, _, _| {
                                            let (a, b, c, f) =
                                                (ws.clone(), ws.clone(), ws.clone(), ws.clone());
                                            let (ia, ib, ic, ifl) =
                                                (id.clone(), id.clone(), id.clone(), id.clone());
                                            menu.item(
                                                PopupMenuItem::new(crate::strings::TASK_DETAILS)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = a.update(cx, |w, cx| {
                                                            w.open_task_dialog(
                                                                ia.clone(),
                                                                window,
                                                                cx,
                                                            )
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new(crate::strings::REVIEW_CHANGES)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = b.update(cx, |w, cx| {
                                                            w.open_review(&ib, window, cx)
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new(crate::strings::OPEN_FILES)
                                                    .on_click(move |_, window, cx| {
                                                        let _ = f.update(cx, |w, cx| {
                                                            w.open_editor(
                                                                &ifl, None, false, window, cx,
                                                            )
                                                        });
                                                    }),
                                            )
                                            .item(
                                                PopupMenuItem::new(
                                                    crate::strings::ADD_SHELL_TERMINAL,
                                                )
                                                .on_click(move |_, window, cx| {
                                                    let _ = c.update(cx, |w, cx| {
                                                        w.task_terminal(ic.clone(), window, cx)
                                                    });
                                                }),
                                            )
                                        }
                                    })
                                    .into_any_element()
                            }
                        };
                        div()
                            .id(SharedString::from(key.clone()))
                            .when(!matches!(item, SidebarItem::Terminal(_)), |d| {
                                let name = match &item {
                                    SidebarItem::Repo(_, label, _) => label.clone(),
                                    SidebarItem::Task(id) => {
                                        let w = ws.read(cx);
                                        match w.task(id) {
                                            Some(t) => task_accessible_name(t, &w.terminals),
                                            None => id.clone(),
                                        }
                                    }
                                    SidebarItem::Terminal(id) => id.clone(),
                                };
                                // Repo/task rows fold (←/→): their state is part of the tree semantics.
                                d.role(Role::TreeItem)
                                    .aria_label(name)
                                    .aria_selected(sidebar.selected.as_ref() == Some(&key))
                                    .aria_expanded(filtering || !collapsed.contains(&key))
                            })
                            .h(px(height))
                            .overflow_hidden()
                            .flex()
                            .items_center()
                            .w_full()
                            .when(sidebar.selected.as_ref() == Some(&key), |d| {
                                d.bg(rgb(tk.bg_selected))
                            })
                            .when(
                                sidebar.selected.as_ref() != Some(&key)
                                    && !matches!(item, SidebarItem::Terminal(_)),
                                |d| d.hover(|s| s.bg(rgb(tk.bg_hover))),
                            )
                            .on_click(
                                cx.listener(move |s, _, window, cx| {
                                    s.activate(&action, window, cx)
                                }),
                            )
                            .child(row)
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
        .min_h_0()
        .into_any_element();
        let list = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .px_2()
                    .pb_2()
                    .child(Input::new(&self.filter).small().prefix(pod_ui::icon_sized(
                        pod_ui::Icon::Search,
                        14.,
                        crate::theme::tokens().text_muted,
                    ))),
            )
            .child(list)
            .into_any_element();
        let content = ws.read(cx).render_sidebar(&self.ws, list);
        div()
            .id("sidebar-navigation")
            .h_full()
            .track_focus(&self.focus)
            .tab_index(0)
            .role(Role::Tree)
            .aria_label("저장소와 작업 탐색")
            .focus_visible(|s| pod_ui::focus_style(s, crate::theme::mode()))
            .on_key_down(cx.listener(move |s, e: &KeyDownEvent, window, cx| {
                let selected = items
                    .iter()
                    .position(|r| Some(&r.key()) == s.selected.as_ref());
                if e.keystroke.key == "enter" {
                    if let Some(row) = selected.and_then(|i| items.get(i)) {
                        s.activate(row, window, cx);
                    }
                    cx.stop_propagation();
                    return;
                }
                let filtering = !s.filter.read(cx).value().trim().is_empty();
                let collapsed = cx.global::<crate::theme::Settings>().collapsed.clone();
                let nodes: Vec<TreeNode> = items
                    .iter()
                    .map(|r| TreeNode {
                        depth: depths.get(&r.key()).copied().unwrap_or(0),
                        parent: !matches!(r, SidebarItem::Terminal(_)),
                        expanded: filtering || !collapsed.contains(&r.key()),
                    })
                    .collect();
                let Some(step) = tree_key(&nodes, selected, e.keystroke.key.as_str()) else {
                    return;
                };
                match step {
                    TreeStep::Select(i) => {
                        if let Some(row) = items.get(i) {
                            s.selected = Some(row.key());
                            // Nearest: a row below the edge scrolls into view at the bottom
                            // instead of jumping to the top on every ↓ (the list stays put).
                            s.scroll.scroll_to_item(i, ScrollStrategy::Nearest);
                            cx.notify();
                        }
                    }
                    TreeStep::Fold(i, expand) => {
                        if let Some(row) = items.get(i).filter(|_| !filtering) {
                            let mut settings = cx.global::<crate::theme::Settings>().clone();
                            if expand {
                                settings.collapsed.remove(&row.key());
                            } else {
                                settings.collapsed.insert(row.key());
                            }
                            let _ = settings.save();
                            cx.set_global(settings);
                            cx.notify();
                        }
                    }
                }
                cx.stop_propagation();
            }))
            .child(content)
            .into_any_element()
    }
}

/// Tooltip and accessible-name detail of a sidebar terminal row. An
/// unreachable (v19) terminal is dimmed like a lost one; its reason leads.
pub(super) fn terminal_row_detail(t: &TerminalVm, badge: &pod_ui::StatusBadge) -> String {
    let detail = format!(
        "{} | {}",
        badge.detail(),
        crate::chrome::freshness(t.status.as_ref())
    );
    match t.lost_reason() {
        Some(reason) => format!("{reason} | {detail}"),
        None => detail,
    }
}

/// A sidebar row as the keyboard sees it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TreeNode {
    pub depth: usize,
    /// Repo/task rows fold; terminals are leaves.
    pub parent: bool,
    pub expanded: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TreeStep {
    Select(usize),
    /// Fold row `i` (true = expand).
    Fold(usize, bool),
}

/// WAI-ARIA tree keys (ui-13): ↑/↓ move (the first ↓ with nothing selected picks the
/// first row), Home/End, → expands a folded parent or enters its first child,
/// ← folds an open parent or moves to the parent row.
pub(crate) fn tree_key(nodes: &[TreeNode], selected: Option<usize>, key: &str) -> Option<TreeStep> {
    let last = nodes.len().checked_sub(1)?;
    let Some(i) = selected.filter(|i| *i <= last) else {
        return matches!(key, "down" | "up" | "home" | "right")
            .then_some(TreeStep::Select(if key == "up" { last } else { 0 }))
            .or((key == "end").then_some(TreeStep::Select(last)));
    };
    let node = nodes[i];
    match key {
        "down" => Some(TreeStep::Select((i + 1).min(last))),
        "up" => Some(TreeStep::Select(i.saturating_sub(1))),
        "home" => Some(TreeStep::Select(0)),
        "end" => Some(TreeStep::Select(last)),
        "right" if node.parent && !node.expanded => Some(TreeStep::Fold(i, true)),
        "right" if node.parent => nodes
            .get(i + 1)
            .filter(|n| n.depth > node.depth)
            .map(|_| TreeStep::Select(i + 1))
            .or(Some(TreeStep::Select(i))),
        "left" if node.parent && node.expanded => Some(TreeStep::Fold(i, false)),
        "left" => Some(TreeStep::Select(
            (0..i)
                .rev()
                .find(|j| nodes[*j].depth < node.depth)
                .unwrap_or(i),
        )),
        "right" => Some(TreeStep::Select(i)),
        _ => None,
    }
}

/// A task row's accessible name: title, agents and the counts the dot/badge show
/// (`응답 1 · 실패 1 …`, docs/10 §4.4), so a screen reader finds the task that needs the user.
pub(crate) fn task_accessible_name(t: &TaskVm, terminals: &[TerminalVm]) -> String {
    let kinds = crate::agent_kind::task_kinds(t, terminals);
    let summary = crate::orch::summary_of(t.terminals.iter(), terminals);
    let mut name = t.title.clone();
    if !kinds.is_empty() {
        name = format!(
            "{name} · {}",
            kinds
                .iter()
                .map(|k| k.accessible_name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if summary.total > 0 {
        name = format!("{name} · {}", crate::orch::summary_short(&summary));
    }
    name
}

#[cfg(test)]
mod tests {
    use core::prelude::v1::test;
    use std::collections::BTreeSet;

    use super::{SidebarItem, TaskRow, TreeData};
    use crate::source::TestBackend;
    use crate::vm::{RepoVm, TaskVm, TerminalVm};

    /// Repo `sample` with task `sample/login` (a1 working), task `solo/t`
    /// without a repo (b1 blocked), and x1 in no task.
    fn data() -> (Vec<RepoVm>, Vec<TaskVm>, Vec<TerminalVm>) {
        let repos = vec![RepoVm {
            name: "sample".into(),
            path: "C:/fixtures/sample".into(),
            default_base: Some("main".into()),
        }];
        let mut solo = TestBackend::task("solo/t", "단독", "solo", &["b1"]);
        solo.repo = None;
        let tasks = vec![
            TestBackend::task("sample/login", "로그인", "sample", &["a1"]),
            solo,
        ];
        let terminals = vec![
            TestBackend::terminal("a1", "작업자", Some("sample/login"), "working"),
            TestBackend::terminal("b1", "리뷰 봇", Some("solo/t"), "blocked"),
            TestBackend::terminal("x1", "x 셸", None, "idle"),
        ];
        (repos, tasks, terminals)
    }

    fn label(t: &TerminalVm) -> String {
        t.name.clone().unwrap_or_else(|| t.id.clone())
    }

    fn keys(rows: Vec<SidebarItem>) -> Vec<String> {
        rows.iter().map(SidebarItem::key).collect()
    }

    #[test]
    fn tree_rows_nest_repo_task_terminal_and_fold() {
        let (repos, tasks, terminals) = data();
        let tree = TreeData {
            repos: &repos,
            tasks: &tasks,
            terminals: &terminals,
            label: &label,
        };
        let none = BTreeSet::new();
        let rows = tree.rows(&none, "");
        assert!(
            matches!(&rows[0], SidebarItem::Repo(name, _, 1) if name == "sample"),
            "{rows:?}"
        );
        assert_eq!(
            keys(rows),
            [
                "repo:sample",
                "task:sample/login",
                "terminal:a1",
                "task:solo/t",
                "terminal:b1",
                "terminal:x1"
            ]
        );
        let folded = BTreeSet::from(["repo:sample".to_string()]);
        assert_eq!(
            keys(tree.rows(&folded, "")),
            ["repo:sample", "task:solo/t", "terminal:b1", "terminal:x1"]
        );
        let folded = BTreeSet::from(["task:sample/login".to_string(), "task:solo/t".to_string()]);
        assert_eq!(
            keys(tree.rows(&folded, "")),
            [
                "repo:sample",
                "task:sample/login",
                "task:solo/t",
                "terminal:x1"
            ]
        );
        let depths = tree.depths();
        let depth = |key: &str| depths.get(key).copied();
        assert_eq!(
            [
                depth("repo:sample"),
                depth("task:sample/login"),
                depth("terminal:a1"),
                depth("task:solo/t"),
                depth("terminal:b1"),
                depth("terminal:x1"),
            ],
            [Some(0), Some(1), Some(2), Some(0), Some(1), None]
        );
    }

    #[test]
    fn tree_search_keeps_matches_and_the_rows_that_lead_to_them() {
        let (repos, tasks, terminals) = data();
        let tree = TreeData {
            repos: &repos,
            tasks: &tasks,
            terminals: &terminals,
            label: &label,
        };
        let none = BTreeSet::new();
        assert_eq!(
            keys(tree.rows(&none, "작업자")),
            ["repo:sample", "task:sample/login", "terminal:a1"],
            "a terminal's name keeps its task and repo"
        );
        assert_eq!(
            keys(tree.rows(&none, "단독")),
            ["task:solo/t", "terminal:b1"],
            "a task's title keeps its terminals"
        );
        assert_eq!(keys(tree.rows(&none, "x1")), ["terminal:x1"]);
        assert!(tree.rows(&none, "없는 이름").is_empty());
    }

    #[test]
    fn task_row_counts_answers_and_names_the_branch() {
        let tk = pod_ui::Tokens::for_mode(pod_ui::Mode::Dark);
        let (_, tasks, terminals) = data();
        let row = TaskRow::new(Some(&tasks[1]), &terminals, &tk);
        let summary = crate::orch::summary_of(tasks[1].terminals.iter(), &terminals);
        assert_eq!(
            row,
            TaskRow {
                title: "단독".into(),
                dot: pod_ui::status_visual(summary.priority().unwrap())
                    .tone
                    .colors(tk)
                    .0,
                badge: Some((1, tk.warning_fg, tk.warning_bg)),
                total: 1,
                tooltip: format!("pod/단독 · {}", summary.label()),
            }
        );
        let row = TaskRow::new(Some(&tasks[0]), &terminals, &tk);
        assert_eq!((row.badge, row.total), (None, 1), "working: the total");
        let mut failed = terminals.clone();
        failed.push(TestBackend::terminal("e1", "실패", Some("solo/t"), "error"));
        let mut task = tasks[1].clone();
        task.terminals.push("e1".into());
        let row = TaskRow::new(Some(&task), &failed, &tk);
        assert_eq!(
            row.badge,
            Some((1, tk.danger_fg, tk.danger_bg)),
            "failures before answers"
        );
        let gone = TaskRow::new(None, &terminals, &tk);
        assert_eq!(
            (gone.title.as_str(), gone.dot, gone.badge, gone.total),
            ("", tk.text_muted, None, 0)
        );
        assert!(gone.tooltip.starts_with("— · "), "{}", gone.tooltip);
    }
}
