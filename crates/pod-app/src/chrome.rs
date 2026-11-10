//! Host-owned commands, settings and status inspector.
use crate::theme::{Settings, mode};
use crate::workspace::{ToastKind, Workspace};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Enter, Input, InputEvent, InputState};
use gpui_kit::component::{TitleBar, WindowExt};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::qa_marks::QaId;

actions!(
    pod_chrome,
    [
        JumpAlternate,
        Jump,
        SettingsAction,
        NewTask,
        Notifications,
        InspectStatus
    ]
);
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-k", Jump, None),
        KeyBinding::new("ctrl-shift-k", JumpAlternate, None),
        KeyBinding::new("ctrl-,", SettingsAction, None),
        KeyBinding::new("ctrl-n", NewTask, None),
        KeyBinding::new("ctrl-shift-b", Notifications, None),
        KeyBinding::new("ctrl-shift-s", InspectStatus, None),
    ]);
}
/// A dialog's primary action, shared by its button and Enter in its fields.
pub(crate) type Submit = std::rc::Rc<dyn Fn(&mut Window, &mut App)>;

/// Enter in a dialog's text field runs the dialog's primary action, as its
/// button does (keyboard-only use). Ignored while an IME is composing.
pub(crate) fn submit_on_enter(
    input: &Entity<InputState>,
    submit: Submit,
    window: &mut Window,
    cx: &mut App,
) {
    window
        .subscribe(input, cx, move |input, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let composing = input.update(cx, |i, cx| i.marked_text_range(window, cx).is_some());
                if !composing {
                    submit(window, cx);
                }
            }
        })
        .detach();
}

#[derive(Clone)]
struct Entry {
    id: String,
    title: String,
    detail: String,
}
const COMMANDS: &[(&str, &str, &str)] = &[
    (
        "shortcuts",
        "키보드 단축키 도움말",
        "단축키 · Keyboard shortcuts",
    ),
    ("new-task", crate::strings::NEW_TASK, "Ctrl+N"),
    (
        "quick-open",
        crate::strings::QUICK_OPEN_FILE,
        crate::strings::QUICK_OPEN_FILE_DETAIL,
    ),
    (
        "files",
        crate::strings::FILES_PANEL,
        crate::strings::FILES_PANEL_DETAIL,
    ),
    (
        "save-all",
        crate::strings::SAVE_ALL_FILES,
        crate::strings::SAVE_ALL_FILES_DETAIL,
    ),
    (
        "open-repo",
        crate::strings::OPEN_REPOSITORY,
        crate::strings::REGISTER_A_LOCAL_GIT_REPOSITORY,
    ),
    ("terminal", crate::strings::NEW_TERMINAL, "Ctrl+Shift+T"),
    ("settings", crate::strings::SETTINGS, "Ctrl+,"),
    (
        "extensions",
        crate::mods_ui::s::OPEN_PAGE,
        crate::mods_ui::s::OPEN_PAGE_DETAIL,
    ),
    (
        "sessions",
        crate::sessions_ui::s::SECTION,
        crate::sessions_ui::s::OPEN_PAGE_DETAIL,
    ),
    (
        "blocked",
        crate::strings::FIND_NEXT_BLOCKED_REQUEST,
        crate::strings::RETURN_TO_THE_TERMINAL_THAT_NEEDS_YOU,
    ),
    (
        "inspect",
        crate::strings::INSPECT_CURRENT_TERMINAL_STATUS,
        crate::strings::SOURCE_AND_FRESHNESS,
    ),
    (
        "notifications",
        crate::strings::NOTIFICATIONS,
        crate::strings::PENDING_REQUESTS_AND_RECENT_EVENTS,
    ),
    (
        "ctrl-k",
        crate::strings::SEND_CTRL_K_TO_CURRENT_TERMINAL,
        crate::strings::TERMINAL_SHORTCUT_PASSTHROUGH,
    ),
    (
        "ctrl-p",
        crate::strings::SEND_CTRL_P_TO_CURRENT_TERMINAL,
        crate::strings::TERMINAL_SHORTCUT_PASSTHROUGH,
    ),
    (
        "terminal-links",
        crate::strings::OPEN_LINK_FROM_TERMINAL,
        crate::strings::OPEN_LINK_FROM_TERMINAL_DETAIL,
    ),
    (
        "install-hooks",
        crate::strings::INSTALL_AGENT_HOOKS,
        crate::strings::INSTALL_AGENT_HOOKS_DETAIL,
    ),
    (
        "grid",
        crate::strings::ARRANGE_PANELS_IN_A_GRID,
        "Ctrl+Shift+G",
    ),
    (
        "new-group",
        crate::strings::NEW_COMPARISON_GROUP,
        crate::strings::COMPARE_PARALLEL_TASKS,
    ),
    (
        "gates",
        crate::strings::OPEN_GATES_INBOX,
        crate::strings::PENDING_APPROVALS,
    ),
    (
        "agents",
        crate::strings::OPEN_AGENT_TREE,
        crate::strings::TASKS_GROUPS_AND_PARENT_RELATIONS,
    ),
];
impl Workspace {
    fn terminal_task_label<'a>(&'a self, t: &'a crate::vm::TerminalVm) -> &'a str {
        t.task_id
            .as_deref()
            .or_else(|| {
                self.tasks
                    .iter()
                    .find(|task| task.terminals.contains(&t.id))
                    .map(|task| task.id.as_str())
            })
            .unwrap_or(crate::strings::TERMINALS)
    }
    pub(crate) fn title_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let context = self
            .focused_terminal
            .as_deref()
            .and_then(|id| self.terminal(id))
            .map(|t| format!("{} / {}", self.terminal_task_label(t), self.label_of(t)))
            .unwrap_or_else(|| crate::strings::YOUR_WORKSPACE.into());
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = mode();
        let alt = cx.global::<Settings>().jump_shortcut == "ctrl-shift-k";
        // The tooltip is also the accessible description: Narrator reads the shortcut.
        let jump_tip = if alt {
            "명령·터미널 검색 (Ctrl+Shift+K)"
        } else {
            "명령·터미널 검색 (Ctrl+K)"
        };
        let ws = cx.entity().downgrade();
        let open_jump = std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
            let _ = ws.update(cx, |w, cx| w.open_jump(window, cx));
        });
        let key_jump = open_jump.clone();
        // Search pill (docs/10 §D4.6): 232 wide (min 160), 26 high; name unchanged ("명령·터미널 검색").
        let pill = div()
            .id("jump")
            .role(Role::Button)
            .aria_label(crate::strings::SEARCH_COMMANDS_TERMINALS_CTRL_K)
            .tab_index(0)
            .w(px(232.))
            .min_w(px(120.))
            .flex_shrink(1.)
            .h(px(26.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(m.radius.control + 2.))
            .border_1()
            .border_color(rgb(tk.border_subtle))
            .bg(rgb(tk.bg_surface))
            .text_xs()
            .text_color(rgb(tk.text_muted))
            .cursor_pointer()
            .hover(|s| s.bg(rgb(tk.bg_hover)))
            .focus_visible(move |s| pod_ui::focus_style(s, mode))
            .child(pod_ui::icon_sized(pod_ui::Icon::Search, 14., tk.text_muted))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(crate::strings::SEARCH_PILL_PLACEHOLDER),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .px_1()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(rgb(tk.border_subtle))
                    .child(if alt { "Ctrl Shift K" } else { "Ctrl K" }),
            )
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(jump_tip).build(window, cx)
            })
            .on_click(move |_, window, cx| open_jump(window, cx))
            .on_key_down(move |e, window, cx| {
                if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    key_jump(window, cx);
                    cx.stop_propagation();
                }
            });
        let ws = cx.entity().downgrade();
        TitleBar::new().h(px(m.density.titlebar.max(32.))).child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex_shrink_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Pod"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(40.))
                        .truncate()
                        .text_xs()
                        .text_color(rgb(tk.text_muted))
                        .child(context),
                )
                .when_some(crate::layout_mode::title_bar_extra(self, cx), |d, e| {
                    d.child(e)
                })
                .child(
                    div()
                        .qa_id(|| "title-actions".into())
                        .flex()
                        .flex_shrink(1.)
                        .min_w_0()
                        .items_center()
                        .gap_1()
                        .child(pill)
                        .child(
                            pod_ui::IconButton::new(
                                "settings",
                                pod_ui::Icon::Settings,
                                crate::strings::SETTINGS,
                            )
                            .tooltip("설정 (Ctrl+,)")
                            .render(mode, move |window, cx| {
                                let _ = ws.update(cx, |w, cx| w.open_settings(window, cx));
                            }),
                        ),
                ),
        )
    }
    pub(crate) fn command(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = id.strip_prefix("term:") {
            self.open_terminal(id, false, window, cx);
            return;
        }
        if let Some(id) = id.strip_prefix("task:") {
            self.open_task_dialog(id.into(), window, cx);
            return;
        }
        if crate::mods_ui::run(self, id, window, cx) || crate::orch_ui::run(self, id, window, cx) {
            return;
        }
        if let Some(next) = theme_command(id, cx.global::<Settings>()) {
            if id == "theme-reload" {
                let errors = crate::theme::reload_themes();
                if !errors.is_empty() {
                    let detail = errors
                        .iter()
                        .map(|(p, r)| format!("{}: {r}", p.display()))
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.raise_toast(
                        ToastKind::Warning,
                        crate::strings::THEME_FILE_SKIPPED,
                        detail,
                        cx,
                    );
                }
            }
            commit_settings(next, &cx.entity().downgrade(), window, cx);
            return;
        }
        if let Some(n) = id.strip_prefix("link:").and_then(|n| n.parse().ok()) {
            self.open_link_choice(n, window, cx);
            return;
        }
        match id {
            "shortcuts" => {
                let shortcut = if cx.global::<Settings>().jump_shortcut == "ctrl-shift-k" {
                    "Ctrl+Shift+K"
                } else {
                    "Ctrl+K"
                };
                window.open_dialog(cx, move |d, _, _| {
                    // Two columns: one column outgrew a 960 px window (the last rows were cut off).
                    let rows: Vec<String> =
                        std::iter::once(format!("{shortcut} — 명령·터미널 검색"))
                            .chain(
                                SHORTCUTS
                                    .iter()
                                    .map(|(keys, label)| format!("{keys} — {label}")),
                            )
                            .collect();
                    let half = rows.len().div_ceil(2);
                    let column = |rows: &[String]| {
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .children(rows.iter().map(|r| div().child(r.clone())))
                    };
                    d.title("키보드 단축키").w(px(940.)).child(
                        div()
                            .id("shortcut-list")
                            .flex()
                            .gap_6()
                            .text_sm()
                            .child(column(&rows[..half]))
                            .child(column(&rows[half..])),
                    )
                });
            }
            "new-task" => self.open_new_task_dialog(window, cx),
            "quick-open" => self.quick_open_files(true, window, cx),
            "files" => self.quick_open_files(false, window, cx),
            "save-all" => self.save_all_editors(window, cx),
            "open-repo" => self.open_repository(window, cx),
            "terminal" => self.open_new_dialog(window, cx),
            "settings" => self.open_settings(window, cx),
            "extensions" => crate::mods_ui::open_page(self, window, cx),
            "sessions" => crate::sessions_ui::open_page(self, window, cx),
            "grid" => self.arrange_grid(window, cx),
            "new-group" => self.open_new_group_dialog(window, cx),
            "gates" => self.open_gates(window, cx),
            "agents" => self.open_agent_tree(window, cx),
            "inspect" => self.open_inspector(window, cx),
            "notifications" => self.open_notifications(window, cx),
            "blocked" => {
                let id = self
                    .terminals
                    .iter()
                    .find(|t| {
                        t.status
                            .as_ref()
                            .is_some_and(|s| s.state == crate::vm::AgentState::Blocked)
                    })
                    .map(|t| t.id.clone());
                if let Some(id) = id {
                    self.open_terminal(&id, false, window, cx);
                } else {
                    self.raise_toast(
                        ToastKind::Info,
                        crate::strings::NO_BLOCKED_REQUESTS,
                        crate::strings::ALL_TERMINALS_ARE_CLEAR_OF_REPORTED_REQUESTS,
                        cx,
                    );
                }
            }
            "ctrl-k" => self.send_to_focused_terminal(0x0b),
            "ctrl-p" => self.send_to_focused_terminal(0x10),
            "terminal-links" => self.open_link_picker(window, cx),
            "install-hooks" => self.request_hook_install(window, cx),
            _ => {}
        }
    }
    pub(crate) fn open_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(crate::strings::LOCAL_GIT_REPOSITORY_PATH)
        });
        let busy = std::rc::Rc::new(std::cell::Cell::new(false));
        let error = std::rc::Rc::new(std::cell::RefCell::new(String::new()));
        let ws = cx.entity().downgrade();
        let focus = input.focus_handle(cx);
        let submit: Submit = {
            let (input, busy, error) = (input.clone(), busy.clone(), error.clone());
            std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
                if busy.replace(true) {
                    return;
                }
                let path = input.read(cx).value().trim().to_string();
                let busy = busy.clone();
                let error = error.clone();
                let _ = ws.update(cx, |w, cx| {
                    let source = w.source.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        let result = source.add_repo(&path, None).await;
                        busy.set(false);
                        let _ = this.update_in(cx, |w, window, cx| {
                            match result {
                                Ok(Ok(repo)) => {
                                    w.repos.retain(|r| r.name != repo.name);
                                    w.repos.push(repo);
                                    window.close_dialog(cx);
                                    w.touch(cx);
                                }
                                Ok(Err(e)) => *error.borrow_mut() = e.message,
                                Err(_) => {
                                    *error.borrow_mut() =
                                        crate::strings::CONNECTION_CLOSED_RETRY_WHEN_CONNECTED
                                            .into()
                                }
                            }
                            window.refresh();
                        });
                    })
                    .detach();
                });
            })
        };
        submit_on_enter(&input, submit.clone(), window, cx);
        window.open_dialog(cx, move |d: Dialog, _, _| {
            let submit = submit.clone();
            d.title(crate::strings::OPEN_REPOSITORY)
                .w(px(560.))
                .child(Input::new(&input))
                .child(
                    div()
                        .text_color(rgb(crate::theme::tokens().danger_fg))
                        .child(error.borrow().clone()),
                )
                .child(
                    Button::new("repo-open")
                        .label(if busy.get() {
                            crate::strings::OPENING
                        } else {
                            crate::strings::OPEN
                        })
                        .primary()
                        .on_click(move |_, window, cx| submit(window, cx)),
                )
        });
        focus.focus(window, cx);
    }
    /// A control byte for the terminal the user works in (shortcuts the app keeps for itself).
    pub(crate) fn send_to_focused_terminal(&mut self, byte: u8) {
        if let Some(id) = &self.focused_terminal {
            self.source.input(id, vec![byte]);
        }
    }
    /// A palette over caller-made `(id, title, detail)` entries; Enter runs `command(id)`.
    pub(crate) fn open_picker(
        &mut self,
        title: &'static str,
        entries: Vec<(String, String, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let entries = entries
            .into_iter()
            .map(|(id, title, detail)| Entry { id, title, detail })
            .collect();
        let ws = cx.entity().downgrade();
        let palette =
            cx.new(|cx| Palette::new(entries, ws, Some(crate::strings::FILTER_LINKS), window, cx));
        let input = palette.read(cx).input.clone();
        window.open_dialog(cx, move |d: Dialog, _, _| {
            d.title(title)
                .bg(rgb(crate::theme::tokens().bg_raised))
                .overflow_hidden()
                .w(px(640.))
                .child(palette.clone())
        });
        input.focus_handle(cx).focus(window, cx);
    }
    pub(crate) fn open_jump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut entries: Vec<Entry> = self
            .terminals
            .iter()
            .map(|t| Entry {
                id: format!("term:{}", t.id),
                title: self.label_of(t),
                detail: format!(
                    "{} / {}",
                    self.terminal_task_label(t),
                    crate::orch::terminal_badge(t).label()
                ),
            })
            .collect();
        entries.extend(self.tasks.iter().map(|t| Entry {
            id: format!("task:{}", t.id),
            title: t.title.clone(),
            detail: t.id.clone(),
        }));
        entries.extend(COMMANDS.iter().map(|(id, title, detail)| Entry {
            id: (*id).into(),
            title: (*title).into(),
            detail: (*detail).into(),
        }));
        entries.extend(
            crate::mods_ui::palette_entries(cx)
                .into_iter()
                .map(|(id, title, detail)| Entry { id, title, detail }),
        );
        entries.extend(
            crate::orch_ui::palette_entries(cx)
                .into_iter()
                .map(|(id, title, detail)| Entry { id, title, detail }),
        );
        entries.extend(
            theme_palette_entries()
                .into_iter()
                .map(|(id, title, detail)| Entry { id, title, detail }),
        );
        let recents = &cx.global::<Settings>().recents;
        entries.sort_by_key(|e| {
            recents
                .iter()
                .position(|id| id == &e.id)
                .unwrap_or(usize::MAX)
        });
        let ws = cx.entity().downgrade();
        let palette = cx.new(|cx| Palette::new(entries, ws, None, window, cx));
        let input = palette.read(cx).input.clone();
        window.open_dialog(cx, move |d: Dialog, _, _| {
            d.title(crate::strings::JUMP_TO_WORK)
                .bg(rgb(crate::theme::tokens().bg_raised))
                .overflow_hidden()
                .w(px(640.))
                .child(palette.clone())
        });
        input.focus_handle(cx).focus(window, cx);
    }
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_chrome_panel(false, window, cx);
    }
    pub(crate) fn open_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_chrome_panel(true, window, cx);
    }
    fn open_chrome_panel(&mut self, inspector: bool, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::dock::{DockPlacement, PanelId, panel_handle};
        let slot = if inspector {
            &mut self.inspector_panel
        } else {
            &mut self.settings_panel
        };
        if let Some(panel) = slot.as_ref().and_then(|p| p.upgrade()) {
            self.dock.update(cx, |d, cx| {
                d.select_panel(PanelId::from(panel.entity_id()), window, cx)
            });
            panel.read(cx).focus_handle(cx).focus(window, cx);
            return;
        }
        let workspace = cx.entity().downgrade();
        let panel = cx.new(|cx| ChromePanel::new(workspace, inspector, cx));
        *slot = Some(panel.downgrade());
        let focus = panel.read(cx).focus_handle(cx);
        self.dock.update(cx, |d, cx| {
            d.add_panel_view(
                panel_handle(panel),
                if inspector {
                    DockPlacement::Right
                } else {
                    DockPlacement::Center
                },
                if inspector { Some(px(304.)) } else { None },
                window,
                cx,
            )
        });
        focus.focus(window, cx);
        self.touch(cx);
    }
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
                let (glyph, fg) = match kind {
                    ToastKind::Info => (pod_ui::Icon::Info, tk.info_fg),
                    ToastKind::Success => (pod_ui::Icon::CircleCheck, tk.success_fg),
                    ToastKind::Warning => (pod_ui::Icon::TriangleAlert, tk.warning_fg),
                    ToastKind::Error => (pod_ui::Icon::OctagonX, tk.danger_fg),
                };
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
pub(crate) fn ago(now_ms: u64, at_ms: u64) -> String {
    let s = now_ms.saturating_sub(at_ms) / 1000;
    match s {
        0..60 => crate::strings::JUST_NOW.into(),
        60..3600 => format!("{}분 전", s / 60),
        3600..86400 => format!("{}시간 전", s / 3600),
        _ => format!("{}일 전", s / 86400),
    }
}

struct Palette {
    /// A picker (links): no recents heading.
    picker: bool,
    scroll: ScrollHandle,
    input: Entity<InputState>,
    entries: Vec<Entry>,
    selection: pod_ui::ListSelection,
    workspace: WeakEntity<Workspace>,
    _sub: Subscription,
}
impl Palette {
    /// `picker`: a caller-made list (its filter placeholder); `None` is the jump palette.
    fn new(
        entries: Vec<Entry>,
        workspace: WeakEntity<Workspace>,
        picker: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(picker.unwrap_or(crate::strings::SEARCH_TASKS_TERMINALS_OR_COMMANDS))
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |p: &mut Self, _, event, _window, cx| match event {
                InputEvent::Change => {
                    p.filter(cx);
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => {}
                _ => {}
            },
        );
        let mut selection = pod_ui::ListSelection::default();
        selection.replace(entries.iter().map(|e| e.id.clone()).collect());
        Self {
            scroll: ScrollHandle::new(),
            input,
            entries,
            selection,
            workspace,
            picker: picker.is_some(),
            _sub: sub,
        }
    }
    fn filter(&mut self, cx: &App) {
        let query = self.input.read(cx).value().to_lowercase();
        let commands = query.starts_with('>');
        let query = query.trim_start_matches('>').trim();
        self.selection.replace(
            self.entries
                .iter()
                .filter(|e| {
                    (!commands || !e.id.contains(':'))
                        && (fuzzy_positions(&e.title, query).is_some()
                            || fuzzy_positions(&e.detail, query).is_some())
                })
                .map(|e| e.id.clone())
                .collect(),
        );
    }
    fn activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composing = self.input.update(cx, |input, cx| {
            input.marked_text_range(window, cx).is_some()
        });
        let Some(id) = self
            .selection
            .activation(composing, &pod_ui::ControlState::Ready)
            .map(str::to_owned)
        else {
            return;
        };
        // Picker choices (`link:<n>`) are not worth remembering.
        if !id.starts_with("link:") {
            let mut settings = cx.global::<Settings>().clone();
            settings.recents.retain(|recent| recent != &id);
            settings.recents.insert(0, id.clone());
            settings.recents.truncate(20);
            let _ = settings.save();
            cx.set_global(settings);
        }
        window.close_dialog(cx);
        let _ = self
            .workspace
            .update(cx, |w, cx| w.command(&id, window, cx));
    }
}
impl Render for Palette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.input.read(cx).value().to_lowercase();
        let commands = query.starts_with('>');
        let query = query.trim_start_matches('>').trim();
        let rows: Vec<_> = self
            .entries
            .iter()
            .filter(|e| {
                (!commands || !e.id.contains(':'))
                    && (fuzzy_positions(&e.title, query).is_some()
                        || fuzzy_positions(&e.detail, query).is_some())
            })
            .cloned()
            .collect();
        let row_ids: Vec<_> = rows.iter().map(|e| e.id.clone()).collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Input::new(&self.input))
            .child(div().text_xs().child(if query.is_empty() && !self.picker {
                crate::strings::RECENT
            } else {
                crate::strings::RESULTS
            }))
            .on_action(cx.listener(|p, _: &Enter, window, cx| {
                p.activate(window, cx);
                cx.stop_propagation();
            }))
            .capture_key_down(cx.listener(move |p, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "down" || e.keystroke.key == "up" {
                    p.selection.navigate(e.keystroke.key == "up");
                    if let Some(index) = row_ids
                        .iter()
                        .position(|id| Some(id) == p.selection.selected.as_ref())
                    {
                        p.scroll.scroll_to_item(index);
                    }
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("jump-results")
                    .max_h(px(340.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(rows.iter().map(|e| {
                        let id = e.id.clone();
                        let selected = self.selection.selected.as_ref() == Some(&e.id);
                        div()
                            .id(SharedString::from(e.id.clone()))
                            .role(Role::ListBoxOption)
                            .aria_label(format!("{} - {}", e.title, e.detail))
                            .aria_selected(selected)
                            .h(px(36.))
                            .w_full()
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_3()
                            .cursor_pointer()
                            .border_l_2()
                            .border_color(rgb(if selected {
                                crate::theme::tokens().accent_fg
                            } else {
                                crate::theme::tokens().bg_surface
                            }))
                            .bg(rgb(if selected {
                                crate::theme::tokens().bg_selected
                            } else {
                                crate::theme::tokens().bg_surface
                            }))
                            .hover(|s| s.bg(rgb(crate::theme::tokens().bg_hover)))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(highlight(&e.title, query)),
                            )
                            .child(
                                div()
                                    .max_w(px(320.))
                                    .truncate()
                                    .text_xs()
                                    .text_color(rgb(crate::theme::tokens().text_muted))
                                    .child(highlight(&e.detail, query)),
                            )
                            .on_click(cx.listener(move |p, _, window, cx| {
                                p.selection.selected = Some(id.clone());
                                p.activate(window, cx);
                            }))
                    })),
            )
            .when(rows.is_empty(), |d| {
                d.child(crate::strings::NO_MATCHING_ITEMS_CLEAR_THE_SEARCH_TO_SHOW_ALL_COMMANDS)
                    .child(
                        Button::new("clear-search")
                            .label(crate::strings::CLEAR_SEARCH)
                            .on_click(cx.listener(|p, _, window, cx| {
                                p.input.update(cx, |i, cx| i.set_value("", window, cx));
                            })),
                    )
            })
            .child(
                div()
                    .text_xs()
                    .child(crate::tr!(UP_DOWN_NAVIGATE_ENTER_OPEN_ESC_CLOSE_RESULTS; ; rows.len())),
            )
    }
}

/// Palette entries "테마: …" (docs/10 §D5.3): presets, modes, user styles, reload.
fn theme_palette_entries() -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = pod_ui::themes::PRESETS
        .iter()
        .map(|p| {
            (
                format!("theme-preset:{}", p.number),
                format!("테마: {} {}", p.number, p.name),
                crate::strings::THEME_PRESET_DETAIL.into(),
            )
        })
        .collect();
    for (id, name) in [("dark", "다크"), ("light", "라이트"), ("system", "시스템")] {
        out.push((
            format!("theme-mode:{id}"),
            format!("테마: {name}"),
            crate::strings::THEME_MODE_DETAIL.into(),
        ));
    }
    for s in pod_ui::themes::registry()
        .styles
        .iter()
        .filter(|s| s.path.is_some())
    {
        out.push((
            format!("theme-style:{}", s.id),
            format!("테마: {} (사용자)", s.name),
            crate::strings::THEME_STYLE.into(),
        ));
    }
    out.push((
        "theme-reload".into(),
        crate::strings::THEME_RELOAD.into(),
        crate::strings::THEME_RELOAD_DETAIL.into(),
    ));
    out
}
/// Settings after a theme palette command, or `None` when `id` is not one.
pub(crate) fn theme_command(id: &str, settings: &Settings) -> Option<Settings> {
    if let Some(n) = id.strip_prefix("theme-preset:") {
        return pod_ui::themes::PRESETS
            .iter()
            .find(|p| p.number == n)
            .map(|p| settings.with_preset(p));
    }
    if let Some(m) = id.strip_prefix("theme-mode:") {
        return ["dark", "light", "system"].contains(&m).then(|| Settings {
            theme: m.into(),
            ..settings.clone()
        });
    }
    if let Some(style) = id.strip_prefix("theme-style:") {
        return pod_ui::themes::registry().get(style).map(|_| Settings {
            style: style.into(),
            ..settings.clone()
        });
    }
    (id == "theme-reload").then(|| settings.clone())
}

/// Timestamp stays on the VM; freshness is computed only when rendered.
pub(crate) fn freshness(status: Option<&crate::vm::StatusVm>) -> String {
    match status {
        Some(s) if s.since > 0 => {
            crate::tr!(S_AGO; ; crate::orch::now_ms().saturating_sub(s.since) / 1000)
        }
        _ => crate::strings::NOT_REPORTED.into(),
    }
}

pub(crate) struct ChromePanel {
    workspace: WeakEntity<Workspace>,
    inspector: bool,
    _sub: Option<Subscription>,
    focus: FocusHandle,
    /// Settings: selected category (index into [`CATEGORIES`]), one focus handle per row.
    category: usize,
    category_focus: [FocusHandle; 3],
    /// Settings: the open select and its highlighted option.
    open_select: Option<(&'static str, usize)>,
    /// Scroll of the open select's list (one list is open at a time).
    select_scroll: ScrollHandle,
}
impl ChromePanel {
    fn new(workspace: WeakEntity<Workspace>, inspector: bool, cx: &mut Context<Self>) -> Self {
        let sub = workspace
            .upgrade()
            .map(|w| cx.observe(&w, |_, _, cx| cx.notify()));
        let category_focus = [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()];
        ChromePanel {
            workspace,
            inspector,
            focus: cx.focus_handle(),
            _sub: sub,
            category: 0,
            category_focus,
            open_select: None,
            select_scroll: ScrollHandle::new(),
        }
    }
}
const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+N", "새 작업"),
    ("Ctrl+Shift+T", "새 터미널"),
    ("Ctrl+,", "설정"),
    ("Ctrl+Shift+S", "상태 인스펙터"),
    ("Ctrl+Shift+B", "알림"),
    ("Ctrl+Shift+R", "변경 검토"),
    ("Ctrl+Shift+G", "패널 격자 정렬"),
    ("Ctrl+Shift+N", "새 비교 그룹"),
    ("Ctrl+Shift+I", "승인 요청"),
    ("Ctrl+Shift+E", "에이전트 트리"),
    ("Ctrl+P", "파일 열기 (현재 작업 폴더)"),
    ("Ctrl+S / Ctrl+K S", "편집기: 저장 / 모두 저장"),
    ("Ctrl+W", "편집기: 탭 닫기 (저장 확인)"),
    ("Ctrl+.", "편집기: 코드 작업 (빠른 수정)"),
    ("Ctrl+클릭", "터미널: 파일:줄 · 주소 열기"),
    ("Alt+1 / Alt+2", "편집기: 파일 트리 / 편집기로 초점"),
    ("F2 / Delete", "파일 트리: 이름 바꾸기 / 삭제"),
    ("Ctrl+Shift+O", "검토: 가리킨 줄을 편집기에서 열기"),
    ("Tab / Shift+Tab", "다음 / 이전 컨트롤"),
    ("Esc", "대화 상자 닫고 이전 포커스로"),
    ("↑ / ↓, Enter", "검색 결과 탐색 / 열기"),
    ("F7 / Shift+F7", "검토: 다음 / 이전 변경"),
    ("Alt+↓ / Alt+↑", "검토: 다음 / 이전 변경"),
    ("Alt+PageDown / Alt+PageUp", "검토: 다음 / 이전 파일"),
    ("Ctrl+Shift+C", "검토: 의견 추가"),
    ("Ctrl+Shift+Enter", "검토: 미해결 의견 보내기"),
    ("F5", "검토 새로 고침"),
];
/// Restore without borrowing the workspace while DockArea::load holds its lease.
pub(crate) fn build_chrome_panel(
    ctx: gpui_kit::component::dock::PanelBuildContext,
    _: &mut Window,
    cx: &mut App,
) -> std::sync::Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let inspector = ctx.state().panel_name == "PodInspector";
    let g = cx.global::<crate::workspace::AppGlobals>();
    let (workspace, restore) = (g.workspace.clone(), g.restore.clone());
    let panel = cx.new(|cx| ChromePanel::new(workspace, inspector, cx));
    restore.borrow_mut().built.push(if inspector {
        crate::workspace::RestoredPanel::Inspector(panel.downgrade())
    } else {
        crate::workspace::RestoredPanel::Settings(panel.downgrade())
    });
    gpui_kit::component::dock::panel_handle(panel)
}
impl EventEmitter<gpui_kit::base::dock::PanelEvent> for ChromePanel {}
impl Focusable for ChromePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_kit::base::dock::Panel for ChromePanel {
    fn panel_name(&self) -> &'static str {
        if self.inspector {
            "PodInspector"
        } else {
            "PodSettings"
        }
    }
    fn dump(&self, _: &App) -> gpui_kit::base::dock::PanelState {
        gpui_kit::base::dock::PanelState::new(if self.inspector {
            "PodInspector"
        } else {
            "PodSettings"
        })
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let inspector = self.inspector;
        cx.defer(move |cx| {
            let _ = ws.update(cx, |w, cx| {
                if inspector {
                    w.inspector_panel = None;
                } else {
                    w.settings_panel = None;
                }
                w.touch(cx);
            });
        });
    }
}
impl gpui_kit::component::dock::Panel for ChromePanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(
            if self.inspector {
                crate::strings::STATUS
            } else {
                crate::strings::SETTINGS
            }
            .into(),
        )
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(if self.inspector {
            crate::strings::STATUS
        } else {
            crate::strings::SETTINGS
        })
    }
}
impl Render for ChromePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.workspace.clone();
        let content = if self.inspector {
            let terminal = ws.upgrade().and_then(|w| {
                let w = w.read(cx);
                w.focused_terminal
                    .as_deref()
                    .and_then(|id| w.terminals.iter().find(|t| t.id == id))
                    .cloned()
                    .map(|t| (t, w.conn.is_connected()))
            });
            if let Some((t, connected)) = terminal {
                let mut badge = crate::orch::terminal_badge(&t);
                if !connected {
                    badge.stale_since = Some(crate::strings::CONNECTION_LOST.into());
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(badge.render(mode()))
                    .child(badge.detail())
                    .child(crate::tr!(INSPECTOR_TERMINAL; ; t.id, t.program))
                    .child(crate::tr!(INSPECTOR_UPDATED; ; freshness(t.status.as_ref())))
            } else {
                div().child(crate::strings::INSPECTOR_EMPTY)
            }
        } else {
            self.settings_view(cx)
        };
        div()
            .id("chrome-panel")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .py_4()
            // Settings: 좌우24 (docs/10 §D4.6).
            .px(px(if self.inspector { 16. } else { 24. }))
            .bg(rgb(crate::theme::tokens().bg_surface))
            .text_color(rgb(crate::theme::tokens().text_primary))
            .child(content)
    }
}

/// Save + apply settings live (no restart): GPUI theme, terminal palettes and fonts, editors.
pub(crate) fn commit_settings(
    next: Settings,
    ws: &WeakEntity<Workspace>,
    window: &mut Window,
    cx: &mut App,
) {
    let result = next.save();
    if result.is_ok() {
        next.apply(cx);
    }
    let _ = ws.update(cx, |w, cx| {
        if result.is_ok() {
            for p in w.live_panels() {
                let v = p.read(cx).view.clone();
                v.update(cx, |v, cx| {
                    v.set_palette(crate::theme::terminal_palette(), cx);
                    v.set_font(
                        next.terminal_font.clone().into(),
                        next.terminal_font_size,
                        cx,
                    );
                });
            }
        }
        w.refresh_editor_theme(cx);
        w.touch(cx);
        if let Err(e) = &result {
            w.raise_toast(
                ToastKind::Error,
                crate::strings::SETTINGS_COULD_NOT_BE_SAVED,
                e.to_string(),
                cx,
            );
        }
    });
    window.refresh();
}

/// `밀도: 고밀도` — one option's accessible name (the group is named after the setting too).
fn choice_name(setting: &str, value: &str) -> String {
    format!("{setting}: {value}")
}

/// Settings categories (docs/10 §D4.6 category160): the left column, in order.
const CATEGORIES: [(&str, pod_ui::Icon); 3] = [
    (crate::strings::APPEARANCE, pod_ui::Icon::Palette),
    (crate::strings::SETTINGS_FONTS, pod_ui::Icon::Type),
    (crate::strings::SETTINGS_GENERAL, pod_ui::Icon::Settings),
];
const UI_FONTS: &[&str] = &["Segoe UI", "Malgun Gothic", "Consolas"];
const UI_SIZES: &[&str] = &["12", "13", "14", "16"];
const TERMINAL_FONTS: &[&str] = &["Cascadia Mono", "Consolas", "D2Coding"];
const TERMINAL_SIZES: &[&str] = &["12", "14", "16", "18"];

impl ChromePanel {
    /// Settings (docs/10 §D4.6): quiet category column on the left, the category's content on the right.
    fn settings_view(&self, cx: &mut Context<Self>) -> Div {
        let ws = self.workspace.clone();
        let me = cx.entity().downgrade();
        let settings = cx.global::<Settings>().clone();
        let category = self.category.min(CATEGORIES.len() - 1);
        let go = std::rc::Rc::new({
            let me = me.clone();
            move |i: usize, window: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |p, cx| {
                    p.category = i;
                    p.open_select = None;
                    p.category_focus[i].focus(window, cx);
                    cx.notify();
                });
            }
        });
        let mut list = pod_ui::category_list(
            "settings-categories",
            crate::strings::SETTINGS_CATEGORIES,
            category,
            CATEGORIES.len(),
            {
                let go = go.clone();
                move |i, w, cx| go(i, w, cx)
            },
        );
        for (i, (label, glyph)) in CATEGORIES.iter().enumerate() {
            let go = go.clone();
            list = list.child(pod_ui::category_item(
                SharedString::from(format!("settings-category-{i}")),
                label,
                *glyph,
                i == category,
                &self.category_focus[i],
                mode(),
                move |w, cx| go(i, w, cx),
            ));
        }
        let sel = Selects {
            panel: me,
            open: self.open_select,
            scroll: self.select_scroll.clone(),
            ws: ws.clone(),
        };
        let content = match category {
            0 => appearance_section(&settings, &ws, &sel, cx),
            1 => fonts_section(&settings, &sel),
            _ => general_section(&settings, &ws, cx),
        };
        // Narrow panels wrap the content under the column (좁으면 한 열).
        div()
            .flex()
            .flex_wrap()
            .items_start()
            .gap(px(24.))
            .min_w_0()
            .child(list)
            .child(
                div()
                    .id("settings-content")
                    .role(Role::TabPanel)
                    .aria_label(CATEGORIES[category].0)
                    .flex_1()
                    .min_w(px(280.))
                    .max_w(px(640.))
                    .child(content),
            )
    }
}

/// Builds settings selects that share the panel's one open-list state.
struct Selects {
    panel: WeakEntity<ChromePanel>,
    open: Option<(&'static str, usize)>,
    scroll: ScrollHandle,
    ws: WeakEntity<Workspace>,
}
impl Selects {
    /// A labelled select whose option `i` saves `nexts[i]` immediately.
    fn row(
        &self,
        key: &'static str,
        label: &str,
        labels: Vec<String>,
        selected: Option<usize>,
        nexts: Vec<Settings>,
    ) -> Div {
        let open = self.open.filter(|(k, _)| *k == key).map(|(_, h)| h);
        let (panel, ws) = (self.panel.clone(), self.ws.clone());
        let labels: Vec<SharedString> = labels.into_iter().map(Into::into).collect();
        setting_line(
            label,
            pod_ui::select(
                SharedString::from(format!("select-{key}")),
                label,
                &labels,
                selected,
                open,
                &self.scroll,
                true,
                mode(),
                move |ev, window, cx| {
                    let state = match ev {
                        pod_ui::SelectEvent::Open(h) | pod_ui::SelectEvent::Move(h) => {
                            Some((key, h))
                        }
                        _ => None,
                    };
                    let _ = panel.update(cx, |p, cx| {
                        p.open_select = state;
                        cx.notify();
                    });
                    if let pod_ui::SelectEvent::Pick(i) = ev
                        && let Some(next) = nexts.get(i)
                    {
                        commit_settings(next.clone(), &ws, window, cx);
                    }
                },
            ),
        )
    }
    /// A select over fixed string values; a current value outside the list is kept as the last option.
    fn values(
        &self,
        key: &'static str,
        label: &str,
        values: &[&str],
        current: &str,
        set: fn(&mut Settings, &str),
        settings: &Settings,
    ) -> Div {
        let (values, selected) = select_options(values, current);
        let nexts = values
            .iter()
            .map(|v| {
                let mut next = settings.clone();
                set(&mut next, v);
                next
            })
            .collect();
        self.row(key, label, values, Some(selected), nexts)
    }
}

/// Options for a select: the fixed values plus the current one when it is custom; and its index.
fn select_options(values: &[&str], current: &str) -> (Vec<String>, usize) {
    let mut out: Vec<String> = values.iter().map(|v| v.to_string()).collect();
    let at = out.iter().position(|v| v == current).unwrap_or_else(|| {
        out.push(current.to_string());
        out.len() - 1
    });
    (out, at)
}

/// Label column (110) + one control (select 200 / switch), vertically centred (docs/10 §D4.6 두 열).
fn setting_line(label: &str, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .min_w_0()
        .child(div().w(px(110.)).flex_shrink_0().child(label.to_string()))
        .child(control)
}

/// A boolean setting as a switch (UIA Toggle), saved immediately.
fn switch_line(id: &str, label: &str, on: bool, next: Settings, ws: &WeakEntity<Workspace>) -> Div {
    let ws = ws.clone();
    setting_line(
        label,
        pod_ui::switch(
            SharedString::from(format!("switch-{id}")),
            label,
            on,
            true,
            mode(),
            move |window, cx| commit_settings(next.clone(), &ws, window, cx),
        ),
    )
}

/// Settings › 글꼴: UI and terminal font and size selects.
fn fonts_section(settings: &Settings, sel: &Selects) -> Div {
    let size = |f: f32| f.to_string();
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_title(crate::strings::SETTINGS_FONTS))
        .child(sel.values(
            "font",
            crate::strings::setting_label("Font"),
            UI_FONTS,
            &settings.font,
            |s, v| s.font = v.into(),
            settings,
        ))
        .child(sel.values(
            "font-size",
            crate::strings::setting_label("UI size"),
            UI_SIZES,
            &size(settings.font_size),
            |s, v| s.font_size = v.parse().unwrap_or(13.),
            settings,
        ))
        .child(sel.values(
            "terminal-font",
            crate::strings::setting_label("Terminal font"),
            TERMINAL_FONTS,
            &settings.terminal_font,
            |s, v| s.terminal_font = v.into(),
            settings,
        ))
        .child(sel.values(
            "terminal-size",
            crate::strings::setting_label("Terminal size"),
            TERMINAL_SIZES,
            &size(settings.terminal_font_size),
            |s, v| s.terminal_font_size = v.parse().unwrap_or(14.),
            settings,
        ))
}

/// Settings › 일반: motion and notification switches, the jump shortcut, links to other settings pages.
fn general_section(settings: &Settings, ws: &WeakEntity<Workspace>, cx: &mut App) -> Div {
    let label = crate::strings::setting_label("Jump shortcut");
    let shortcuts = ["ctrl-k", "ctrl-shift-k"]
        .into_iter()
        .map(|value| {
            let next = Settings {
                jump_shortcut: value.into(),
                ..settings.clone()
            };
            let ws = ws.clone();
            pod_ui::choice(
                SharedString::from(format!("Jump shortcut-{value}")),
                value,
                choice_name(label, value),
                None,
                settings.jump_shortcut == value,
                true,
                mode(),
                move |window, cx| commit_settings(next.clone(), &ws, window, cx),
            )
        })
        .collect();
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_title(crate::strings::SETTINGS_GENERAL))
        .child(switch_line(
            "motion",
            crate::strings::REDUCE_MOTION,
            settings.reduced_motion,
            Settings {
                reduced_motion: !settings.reduced_motion,
                ..settings.clone()
            },
            ws,
        ))
        .child(switch_line(
            "os-notifications",
            crate::strings::OS_NOTIFICATIONS,
            settings.os_notifications,
            Settings {
                os_notifications: !settings.os_notifications,
                ..settings.clone()
            },
            ws,
        ))
        .child(setting_row("Jump shortcut", label, shortcuts))
        .child(div().text_xs().child(crate::strings::CTRL_K))
        .child(crate::mods_ui::settings_link(ws, cx))
        .child(crate::orch_ui::settings_link(ws, cx))
}

/// One setting: label column + a named radio group of options (docs/10 §D4.6).
fn setting_row(id: &str, label: &str, options: Vec<Stateful<Div>>) -> Div {
    let label: SharedString = label.to_string().into();
    div()
        .flex()
        .items_start()
        .gap_3()
        .min_w_0()
        .child(
            div()
                .w(px(110.))
                .flex_shrink_0()
                .pt(px(4.))
                .child(label.clone()),
        )
        .child(
            pod_ui::radio_group(SharedString::from(format!("group-{id}")), label, mode())
                .flex_1()
                .children(options),
        )
}

/// Settings › 모양 notices: skipped theme files, rejected agent marks (docs/10 §D5.11)
/// and user styles under WCAG AA.
fn appearance_notices(
    reg: &pod_ui::themes::ThemeRegistry,
) -> Vec<(pod_ui::Tone, &'static str, String)> {
    let line =
        |(path, reason): (std::path::PathBuf, String)| format!("{} — {reason}", path.display());
    let mut out: Vec<_> = reg
        .errors
        .iter()
        .cloned()
        .map(|e| {
            (
                pod_ui::Tone::Warning,
                crate::strings::THEME_FILE_SKIPPED,
                line(e),
            )
        })
        .collect();
    out.extend(pod_ui::agent::override_errors().into_iter().map(|e| {
        (
            pod_ui::Tone::Warning,
            crate::strings::AGENT_MARK_SKIPPED,
            line(e),
        )
    }));
    out.extend(reg.warnings.iter().cloned().map(|e| {
        (
            pod_ui::Tone::Info,
            crate::strings::THEME_LOW_CONTRAST,
            line(e),
        )
    }));
    out
}

fn section_title(text: &'static str) -> Div {
    div()
        .pt_2()
        .text_size(px(15.))
        .line_height(px(24.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

/// A schematic of a style in its own colours and surface model (settings cards).
fn theme_preview(spec: &pod_ui::themes::ThemeSpec, dark: bool) -> Div {
    use pod_ui::themes::Surface;
    let Ok(t) = spec.tokens(if dark { "dark" } else { "light" }, None) else {
        return div();
    };
    let rounded = matches!(spec.surface, Surface::Islands | Surface::Layered);
    let gap = if rounded { 3. } else { 0. };
    let r = if rounded { 3. } else { 0. };
    let line = |w: f32, c: u32| div().h(px(3.)).w(px(w)).rounded(px(1.)).bg(rgb(c));
    let row = |sel: bool, dot: u32, w: f32| {
        div()
            .h(px(8.))
            .flex()
            .items_center()
            .gap(px(3.))
            .px(px(3.))
            .when(sel, |d| {
                d.bg(rgb(t.bg_selected))
                    .border_l_2()
                    .border_color(rgb(t.accent_fg))
            })
            .child(div().size(px(4.)).rounded_full().bg(rgb(dot)))
            .child(line(
                w,
                if sel {
                    t.text_primary
                } else {
                    t.text_secondary
                },
            ))
    };
    let edge = spec.surface == Surface::Flat;
    div()
        .w(px(120.))
        .h(px(74.))
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(t.border_subtle))
        .bg(rgb(t.bg_canvas))
        .child(
            div()
                .h(px(8.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(4.))
                .bg(rgb(t.bg_chrome))
                .child(line(14., t.text_primary)),
        )
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .gap(px(gap))
                .p(px(gap))
                .child(
                    div()
                        .w(px(36.))
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .pt(px(2.))
                        .rounded(px(r))
                        .bg(rgb(t.bg_chrome))
                        .when(edge, |d| d.border_r_1().border_color(rgb(t.border_subtle)))
                        .child(row(false, t.warning_fg, 20.))
                        .child(row(true, t.info_fg, 18.))
                        .child(row(false, t.success_fg, 16.))
                        .child(row(false, t.danger_fg, 22.)),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(gap.max(1.)))
                        .when(!rounded, |d| d.bg(rgb(t.border_subtle)))
                        .children([0, 1].map(|i| {
                            div()
                                .flex_1()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .p(px(3.))
                                .rounded(px(r))
                                .bg(rgb(t.terminal_bg))
                                .child(line(30., t.terminal_fg))
                                .child(line(22., if i == 0 { t.success_fg } else { t.warning_fg }))
                                .when(i == 1, |d| {
                                    d.child(
                                        div()
                                            .w(px(16.))
                                            .h(px(5.))
                                            .rounded(px(1.))
                                            .bg(rgb(t.action_primary_bg)),
                                    )
                                })
                        })),
                ),
        )
}

/// Settings › 모양 (docs/10 §D5.3): presets, style, mode, density, accent, layout.
fn appearance_section(
    settings: &Settings,
    ws: &WeakEntity<Workspace>,
    sel: &Selects,
    cx: &App,
) -> Div {
    use pod_ui::themes::{ACCENTS, PRESETS, registry};
    let reg = registry();
    let dark = crate::theme::mode() == pod_ui::Mode::Dark;
    let tk = crate::theme::tokens();
    let current = settings.preset();
    let layouts = cx.try_global::<crate::layout_mode::LayoutRegistry>();
    let choice =
        |id: String, text: String, name: String, selected: bool, enabled: bool, next: Settings| {
            let ws = ws.clone();
            pod_ui::choice(
                SharedString::from(id),
                text,
                name,
                None,
                selected,
                enabled,
                mode(),
                move |window, cx| commit_settings(next.clone(), &ws, window, cx),
            )
        };
    // Preset cards: one radio group (the selection is the checked state, not only the border colour).
    let mut cards = div()
        .id("theme-presets")
        .role(Role::RadioGroup)
        .aria_label(crate::strings::THEME_PRESETS)
        .flex()
        .flex_wrap()
        .gap_2();
    for p in PRESETS {
        let Some(spec) = reg.get(p.style) else {
            continue;
        };
        let selected = current == Some(p);
        let ready = layouts.is_none_or(|r| {
            crate::layout_mode::LayoutMode::from_id(p.layout).is_some_and(|m| r.available(m))
        });
        let next = settings.with_preset(&p);
        let ws = ws.clone();
        let name = format!("프리셋: {} {}", p.number, p.name);
        let tip = if ready {
            format!("{} {} · {}", p.number, p.name, spec.source)
        } else {
            format!(
                "{} {} · {} · {}",
                p.number,
                p.name,
                spec.source,
                crate::strings::LAYOUT_NOT_READY
            )
        };
        cards = cards.child(
            div()
                .id(SharedString::from(format!("preset-{}", p.number)))
                .role(Role::RadioButton)
                .aria_label(name)
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .tab_index(0)
                .w(px(132.))
                .p(px(5.))
                .flex()
                .flex_col()
                .gap_1()
                .rounded(px(6.))
                .border_2()
                .border_color(rgb(if selected {
                    tk.accent_fg
                } else {
                    tk.border_subtle
                }))
                .cursor_pointer()
                .hover(|s| s.bg(rgb(tk.bg_hover)))
                .focus_visible(|s| pod_ui::focus_style(s, crate::theme::mode()))
                .child(theme_preview(&spec, dark))
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .text_xs()
                        .min_w_0()
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(rgb(tk.text_muted))
                                .child(p.number),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(p.name)),
                )
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .on_click({
                    let next = next.clone();
                    let ws = ws.clone();
                    move |_, window, cx| commit_settings(next.clone(), &ws, window, cx)
                })
                .on_key_down(move |e, window, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space") {
                        commit_settings(next.clone(), &ws, window, cx);
                        cx.stop_propagation();
                    }
                }),
        );
    }
    // Style select (built-in + user themes: a long list).
    let style_labels = reg
        .styles
        .iter()
        .map(|spec| {
            if spec.path.is_some() {
                format!("{} (사용자)", spec.name)
            } else {
                spec.name.clone()
            }
        })
        .collect();
    let style_nexts = reg
        .styles
        .iter()
        .map(|spec| Settings {
            style: spec.id.clone(),
            ..settings.clone()
        })
        .collect();
    let style_at = reg.styles.iter().position(|spec| settings.style == spec.id);
    // Mode row (the existing "테마" names).
    let mut modes = Vec::new();
    for value in ["system", "dark", "light"] {
        let next = Settings {
            theme: value.into(),
            ..settings.clone()
        };
        let text = crate::strings::setting_label(value).to_string();
        modes.push(choice(
            format!("Theme-{value}"),
            text.clone(),
            choice_name(crate::strings::setting_label("Theme"), &text),
            settings.theme == value,
            true,
            next,
        ));
    }
    // Density row: part of 모양 (docs/10 §D5.3).
    let mut density = Vec::new();
    for value in ["theme", "dense", "compact", "comfortable", "accessible"] {
        let next = Settings {
            density: value.into(),
            comfortable: false,
            ..settings.clone()
        };
        let selected = settings.density == value
            || (value == "comfortable" && settings.comfortable && settings.density == "theme");
        let text = crate::strings::setting_label(value).to_string();
        density.push(choice(
            format!("Density-{value}"),
            text.clone(),
            choice_name(crate::strings::setting_label("Density"), &text),
            selected,
            true,
            next,
        ));
    }
    // Accent row.
    let active = pod_ui::themes::active();
    let allowed = active.spec.accent_override;
    let mut accents = Vec::new();
    for (id, name) in std::iter::once(("theme", crate::strings::THEME_DEFAULT)).chain(ACCENTS) {
        let next = Settings {
            accent: id.into(),
            ..settings.clone()
        };
        accents.push(choice(
            format!("accent-{id}"),
            name.into(),
            choice_name(crate::strings::THEME_ACCENT, name),
            settings.accent == id,
            allowed || id == "theme",
            next,
        ));
    }
    // Layout select (seven layouts).
    let all = crate::layout_mode::LayoutMode::ALL;
    let layout_labels = all
        .iter()
        .map(|m| {
            if layouts.is_none_or(|r| r.available(*m)) {
                m.label().to_string()
            } else {
                format!("{} (준비 중)", m.label())
            }
        })
        .collect();
    let layout_nexts = all
        .iter()
        .map(|m| Settings {
            layout: m.id().into(),
            ..settings.clone()
        })
        .collect();
    let layout_at = all.iter().position(|m| settings.layout == m.id());
    let not_ready = crate::layout_mode::LayoutMode::from_id(&settings.layout)
        .is_some_and(|m| layouts.is_some_and(|r| !r.available(m)));
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_title(crate::strings::APPEARANCE))
        .child(cards)
        .child(sel.row("style", crate::strings::THEME_STYLE, style_labels, style_at, style_nexts))
        .child(setting_row("Theme", crate::strings::setting_label("Theme"), modes))
        .child(setting_row("Density", crate::strings::setting_label("Density"), density))
        .child(setting_row("accent", crate::strings::THEME_ACCENT, accents))
        .when(!allowed, |d| d.child(div().pl(px(122.)).text_xs().text_color(rgb(tk.text_muted)).child(crate::strings::THEME_ACCENT_FIXED)))
        .child(sel.row("layout", crate::strings::THEME_LAYOUT, layout_labels, layout_at, layout_nexts))
        .when(not_ready, |d| d.child(pod_ui::banner(pod_ui::Tone::Info, crate::theme::mode(), crate::strings::LAYOUT_NOT_READY, None).text_xs()))
        .child(switch_line("high-contrast", crate::strings::HIGH_CONTRAST_PREVIEW, settings.high_contrast, Settings { high_contrast: !settings.high_contrast, ..settings.clone() }, ws))
        .children(appearance_notices(&reg).into_iter().map(|(tone, title, detail)| pod_ui::banner(tone, crate::theme::mode(), title, Some(detail.into())).text_xs()))
        .child(div().text_xs().text_color(rgb(tk.text_muted)).child(crate::tr!(THEME_FOLDER; ; crate::theme::themes_dir().map(|d| d.display().to_string()).unwrap_or_default())))
}

/// Unicode subsequence matching, returning byte ranges for GPUI shaping.
fn fuzzy_positions(text: &str, query: &str) -> Option<Vec<std::ops::Range<usize>>> {
    let mut chars = text.char_indices();
    let mut ranges = Vec::new();
    for needle in query.chars().filter(|c| !c.is_whitespace()) {
        let (index, ch) = chars
            .find(|(_, c)| c.to_lowercase().to_string() == needle.to_lowercase().to_string())?;
        ranges.push(index..index + ch.len_utf8());
    }
    Some(ranges)
}
fn highlight(text: &str, query: &str) -> StyledText {
    let ranges = fuzzy_positions(text, query).unwrap_or_default();
    StyledText::new(text.to_owned()).with_highlights(ranges.into_iter().map(|range| {
        (
            range,
            HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                }),
                ..Default::default()
            },
        )
    }))
}
/// Static loading structure respects reduced motion and never masquerades as an empty result.
pub(crate) fn loading_placeholder(label: &'static str) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .text_sm()
        .text_color(rgb(crate::theme::tokens().text_muted))
        .child(label)
        .children([160., 240., 200.].map(|width| {
            div()
                .w(px(width))
                .max_w_full()
                .h(px(12.))
                .rounded_sm()
                .bg(rgb(crate::theme::tokens().bg_hover))
        }))
}

#[cfg(test)]
mod palette_tests {
    use super::fuzzy_positions;
    #[test]
    fn fuzzy_unicode_ranges_and_order() {
        assert_eq!(
            fuzzy_positions(crate::strings::FUZZY_SAMPLE, crate::strings::FUZZY_QUERY),
            Some(vec![0..3, 4..7])
        );
        assert!(fuzzy_positions("Terminals", "tml").is_some());
        assert!(fuzzy_positions("abc", "ca").is_none());
        assert_eq!(fuzzy_positions("abc", ""), Some(vec![]));
    }
    #[test]
    fn theme_palette_commands_map_to_settings() {
        let base = crate::theme::Settings::default();
        let entries = super::theme_palette_entries();
        assert!(
            entries
                .iter()
                .any(|(id, title, _)| id == "theme-preset:08" && title == "테마: 08 섬형")
        );
        for (id, _, _) in &entries {
            assert!(super::theme_command(id, &base).is_some(), "{id} runs");
        }
        let s = super::theme_command("theme-preset:14", &base).unwrap();
        assert_eq!(
            (s.style.as_str(), s.layout.as_str()),
            ("access", "workbench")
        );
        assert_eq!(
            super::theme_command("theme-mode:light", &base)
                .unwrap()
                .theme,
            "light"
        );
        assert!(super::theme_command("theme-preset:99", &base).is_none());
        assert!(super::theme_command("theme-style:nope", &base).is_none());
        assert!(super::theme_command("new-task", &base).is_none());
    }
    #[test]
    fn documented_shortcuts_match_in_app_help() {
        let design = include_str!("../../../docs/10-ui-design.md");
        for (keys, label) in super::SHORTCUTS {
            assert!(
                design.contains(&format!("| {keys} | {label} |")),
                "missing shortcut {keys}"
            );
        }
    }
}
#[cfg(test)]
mod settings_tests {
    use super::*;
    use core::prelude::v1::test;
    #[test]
    fn settings_categories_are_korean_and_ordered() {
        let labels: Vec<_> = CATEGORIES.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, ["모양", "글꼴", "일반"]);
        assert_eq!(crate::strings::SETTINGS_CATEGORIES, "설정 분류");
    }
    #[test]
    fn select_keeps_a_custom_value_as_its_own_option() {
        assert_eq!(
            select_options(UI_FONTS, "Consolas"),
            (
                vec!["Segoe UI".into(), "Malgun Gothic".into(), "Consolas".into()],
                2
            )
        );
        let (opts, at) = select_options(UI_FONTS, "Pretendard");
        assert_eq!((opts.len(), at, opts[at].as_str()), (4, 3, "Pretendard"));
    }
    #[test]
    fn default_settings_match_a_listed_option() {
        let d = Settings::default();
        assert_eq!(
            select_options(UI_FONTS, &d.font).0.len(),
            UI_FONTS.len(),
            "default UI font is listed"
        );
        assert_eq!(
            select_options(TERMINAL_FONTS, &d.terminal_font).0.len(),
            TERMINAL_FONTS.len()
        );
        // Sizes are written without a trailing `.0` so 13.0 selects "13".
        assert_eq!(
            select_options(UI_SIZES, &d.font_size.to_string()).0.len(),
            UI_SIZES.len()
        );
        assert_eq!(
            select_options(TERMINAL_SIZES, &d.terminal_font_size.to_string())
                .0
                .len(),
            TERMINAL_SIZES.len()
        );
    }
    #[test]
    fn boolean_settings_have_korean_switch_labels() {
        for l in [
            crate::strings::REDUCE_MOTION,
            crate::strings::HIGH_CONTRAST_PREVIEW,
            crate::strings::OS_NOTIFICATIONS,
        ] {
            assert!(l.chars().any(|c| ('가'..='힣').contains(&c)), "{l}");
        }
    }
}
