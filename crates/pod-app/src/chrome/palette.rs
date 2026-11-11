//! The jump palette (Ctrl+K: terminals, tasks and every command, recents
//! first) and the pickers other features open over their own entries.
use gpui_kit::component::WindowExt;
use gpui_kit::component::button::Button;
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Enter, Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::commands::COMMANDS;
use crate::theme::Settings;
use crate::workspace::Workspace;

#[derive(Clone)]
struct Entry {
    id: String,
    title: String,
    detail: String,
}

impl Workspace {
    pub(super) fn terminal_task_label<'a>(&'a self, t: &'a crate::vm::TerminalVm) -> &'a str {
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
        entries.extend(COMMANDS.iter().map(|c| Entry {
            id: c.id.into(),
            title: c.label.into(),
            detail: c.detail.into(),
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
            super::settings::theme_palette_entries()
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

/// Unicode subsequence matching, returning byte ranges for GPUI shaping.
pub(super) fn fuzzy_positions(text: &str, query: &str) -> Option<Vec<std::ops::Range<usize>>> {
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
