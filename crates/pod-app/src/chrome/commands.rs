//! Static commands (docs/20-refactor/app.md §2 "명령"): one [`CommandSpec`]
//! row links a palette id and its text to the key bound for it here and to
//! what it runs, so the palette entry, the key and `Workspace::command` take
//! one path. Dynamic commands stay with their feature (terminals, tasks, mods,
//! orchestration, themes, links); `Workspace::command` routes those first.
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::*;

use super::copy;
use super::settings::{commit_settings, theme_command};
use super::{Submit, submit_on_enter};
use crate::theme::Settings;
use crate::workspace::{ToastKind, Workspace};

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

/// What a command does to the workspace.
pub(super) type Run = fn(&mut Workspace, &mut Window, &mut Context<Workspace>);

/// A key bound for a command: its keystrokes, the key context it works in
/// (`None`: everywhere) and, typed by the command's action, the binding and
/// the handler on the workspace root.
#[derive(Clone, Copy)]
pub(super) struct Shortcut {
    pub(super) keys: &'static str,
    pub(super) context: Option<&'static str>,
    pub(super) bind: fn(&'static str, Option<&'static str>) -> KeyBinding,
    listen: fn(Stateful<Div>, Run, &mut Context<Workspace>) -> Stateful<Div>,
}

/// `keys` everywhere, dispatching `A`.
const fn key<A: Action + Default>(keys: &'static str) -> Shortcut {
    Shortcut {
        keys,
        context: None,
        bind: bind::<A>,
        listen: listen::<A>,
    }
}

fn bind<A: Action + Default>(keys: &'static str, context: Option<&'static str>) -> KeyBinding {
    KeyBinding::new(keys, A::default(), context)
}

fn listen<A: Action>(root: Stateful<Div>, run: Run, cx: &mut Context<Workspace>) -> Stateful<Div> {
    root.on_action(cx.listener(move |w, _: &A, window, cx| run(w, window, cx)))
}

/// One static palette command.
pub(super) struct CommandSpec {
    /// Palette id: `Workspace::command(id)` runs it and recents remember it.
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    /// The palette's second column: the key as text, or a description.
    pub(super) detail: &'static str,
    /// The key bound here. A command without one has its key bound elsewhere
    /// (main.rs, shown in `detail`) or none.
    pub(super) shortcut: Option<Shortcut>,
    pub(super) run: Run,
}

/// The static commands in palette order.
pub(super) const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: "shortcuts",
        label: copy::SHORTCUTS_LABEL,
        detail: copy::SHORTCUTS_DETAIL,
        shortcut: None,
        run: |w, window, cx| w.open_shortcuts(window, cx),
    },
    CommandSpec {
        id: "new-task",
        label: crate::strings::NEW_TASK,
        detail: "Ctrl+N",
        shortcut: Some(key::<NewTask>("ctrl-n")),
        run: |w, window, cx| w.open_new_task_dialog(window, cx),
    },
    CommandSpec {
        id: "quick-open",
        label: crate::strings::QUICK_OPEN_FILE,
        detail: crate::strings::QUICK_OPEN_FILE_DETAIL,
        shortcut: None,
        run: |w, window, cx| w.quick_open_files(true, window, cx),
    },
    CommandSpec {
        id: "files",
        label: crate::strings::FILES_PANEL,
        detail: crate::strings::FILES_PANEL_DETAIL,
        shortcut: None,
        run: |w, window, cx| w.quick_open_files(false, window, cx),
    },
    CommandSpec {
        id: "save-all",
        label: crate::strings::SAVE_ALL_FILES,
        detail: crate::strings::SAVE_ALL_FILES_DETAIL,
        shortcut: None,
        run: |w, window, cx| w.save_all_editors(window, cx),
    },
    CommandSpec {
        id: "open-repo",
        label: crate::strings::OPEN_REPOSITORY,
        detail: crate::strings::REGISTER_A_LOCAL_GIT_REPOSITORY,
        shortcut: None,
        run: |w, window, cx| w.open_repository(window, cx),
    },
    CommandSpec {
        id: "terminal",
        label: crate::strings::NEW_TERMINAL,
        detail: "Ctrl+Shift+T",
        shortcut: None,
        run: |w, window, cx| w.open_new_dialog(window, cx),
    },
    CommandSpec {
        id: "settings",
        label: crate::strings::SETTINGS,
        detail: "Ctrl+,",
        shortcut: Some(key::<SettingsAction>("ctrl-,")),
        run: |w, window, cx| w.open_settings(window, cx),
    },
    CommandSpec {
        id: "machines",
        label: crate::machines_ui::CATEGORY,
        detail: crate::machines_ui::COMMAND_DETAIL,
        shortcut: None,
        run: |w, window, cx| w.open_machine_settings(window, cx),
    },
    CommandSpec {
        id: "extensions",
        label: crate::mods_ui::s::OPEN_PAGE,
        detail: crate::mods_ui::s::OPEN_PAGE_DETAIL,
        shortcut: None,
        run: |w, window, cx| crate::mods_ui::open_page(w, window, cx),
    },
    CommandSpec {
        id: "sessions",
        label: crate::sessions_ui::s::SECTION,
        detail: crate::sessions_ui::s::OPEN_PAGE_DETAIL,
        shortcut: None,
        run: |w, window, cx| crate::sessions_ui::open_page(w, window, cx),
    },
    CommandSpec {
        id: "blocked",
        label: crate::strings::FIND_NEXT_BLOCKED_REQUEST,
        detail: crate::strings::RETURN_TO_THE_TERMINAL_THAT_NEEDS_YOU,
        shortcut: None,
        run: |w, window, cx| w.open_next_blocked(window, cx),
    },
    CommandSpec {
        id: "inspect",
        label: crate::strings::INSPECT_CURRENT_TERMINAL_STATUS,
        detail: crate::strings::SOURCE_AND_FRESHNESS,
        shortcut: Some(key::<InspectStatus>("ctrl-shift-s")),
        run: |w, window, cx| w.open_inspector(window, cx),
    },
    CommandSpec {
        id: "notifications",
        label: crate::strings::NOTIFICATIONS,
        detail: crate::strings::PENDING_REQUESTS_AND_RECENT_EVENTS,
        shortcut: Some(key::<Notifications>("ctrl-shift-b")),
        run: |w, window, cx| w.open_notifications(window, cx),
    },
    CommandSpec {
        id: "ctrl-k",
        label: crate::strings::SEND_CTRL_K_TO_CURRENT_TERMINAL,
        detail: crate::strings::TERMINAL_SHORTCUT_PASSTHROUGH,
        shortcut: None,
        run: |w, _, _| w.send_to_focused_terminal(0x0b),
    },
    CommandSpec {
        id: "ctrl-p",
        label: crate::strings::SEND_CTRL_P_TO_CURRENT_TERMINAL,
        detail: crate::strings::TERMINAL_SHORTCUT_PASSTHROUGH,
        shortcut: None,
        run: |w, _, _| w.send_to_focused_terminal(0x10),
    },
    CommandSpec {
        id: "terminal-links",
        label: crate::strings::OPEN_LINK_FROM_TERMINAL,
        detail: crate::strings::OPEN_LINK_FROM_TERMINAL_DETAIL,
        shortcut: None,
        run: |w, window, cx| w.open_link_picker(window, cx),
    },
    CommandSpec {
        id: "install-hooks",
        label: crate::strings::INSTALL_AGENT_HOOKS,
        detail: crate::strings::INSTALL_AGENT_HOOKS_DETAIL,
        shortcut: None,
        run: |w, window, cx| w.request_hook_install(window, cx),
    },
    CommandSpec {
        id: "grid",
        label: crate::strings::ARRANGE_PANELS_IN_A_GRID,
        detail: "Ctrl+Shift+G",
        shortcut: None,
        run: |w, window, cx| w.arrange_grid(window, cx),
    },
    CommandSpec {
        id: "new-group",
        label: crate::strings::NEW_COMPARISON_GROUP,
        detail: crate::strings::COMPARE_PARALLEL_TASKS,
        shortcut: None,
        run: |w, window, cx| w.open_new_group_dialog(window, cx),
    },
    CommandSpec {
        id: "gates",
        label: crate::strings::OPEN_GATES_INBOX,
        detail: crate::strings::PENDING_APPROVALS,
        shortcut: None,
        run: |w, window, cx| w.open_gates(window, cx),
    },
    CommandSpec {
        id: "agents",
        label: crate::strings::OPEN_AGENT_TREE,
        detail: crate::strings::TASKS_GROUPS_AND_PARENT_RELATIONS,
        shortcut: None,
        run: |w, window, cx| w.open_agent_tree(window, cx),
    },
];

/// The chrome's keys: Ctrl+K and Ctrl+Shift+K (the palette, by setting),
/// then the key of every command that has one, in [`COMMANDS`] order.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys(
        [
            KeyBinding::new("ctrl-k", Jump, None),
            KeyBinding::new("ctrl-shift-k", JumpAlternate, None),
        ]
        .into_iter()
        .chain(
            COMMANDS
                .iter()
                .filter_map(|c| c.shortcut)
                .map(|s| (s.bind)(s.keys, s.context)),
        ),
    );
}

/// The handlers of [`bind_keys`] on the workspace root: the jump key in
/// settings opens the palette (Ctrl+K otherwise reaches the terminal, Ctrl+Shift+K
/// propagates), and each command's key runs that command.
pub(crate) fn key_actions(root: Stateful<Div>, cx: &mut Context<Workspace>) -> Stateful<Div> {
    let root = root
        .on_action(cx.listener(|w, _: &Jump, window, cx| {
            if cx.global::<Settings>().jump_shortcut == "ctrl-k" {
                w.open_jump(window, cx);
            } else if let Some(id) = &w.focused_terminal {
                w.source.input(id, vec![11]);
            }
        }))
        .on_action(cx.listener(|w, _: &JumpAlternate, window, cx| {
            if cx.global::<Settings>().jump_shortcut == "ctrl-shift-k" {
                w.open_jump(window, cx);
            } else {
                cx.propagate();
            }
        }));
    COMMANDS.iter().fold(root, |root, c| match c.shortcut {
        Some(s) => (s.listen)(root, c.run, cx),
        None => root,
    })
}

impl Workspace {
    /// Runs a palette id: a terminal or task, a feature's own command (mods,
    /// orchestration, theme, link) or one of [`COMMANDS`]; unknown ids do nothing.
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
        if let Some(c) = COMMANDS.iter().find(|c| c.id == id) {
            (c.run)(self, window, cx);
        }
    }

    /// The keyboard shortcut help (two columns: one outgrew a 960 px window
    /// and the last rows were cut off).
    fn open_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let shortcut = if cx.global::<Settings>().jump_shortcut == "ctrl-shift-k" {
            copy::JUMP_KEY_ALT
        } else {
            copy::JUMP_KEY
        };
        window.open_dialog(cx, move |d, _, _| {
            let rows: Vec<String> = std::iter::once(format!("{shortcut} — {}", copy::JUMP_HELP))
                .chain(
                    copy::SHORTCUTS
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
            d.title(copy::SHORTCUTS_TITLE).w(px(940.)).child(
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
}
