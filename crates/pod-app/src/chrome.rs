//! Host-owned chrome (docs/20-refactor/app.md §2): the title bar, the
//! commands and their keys, the jump palette, settings, the status inspector
//! and the notifications list.
//!
//! This file is the facade: it keeps the title bar and the dialog helpers the
//! features share. Each concern is a child module: the static command table
//! and palette routing (`commands`), the palette and pickers (`palette`),
//! settings and the theme commands (`settings`), the dock panel that shows
//! settings or the status inspector (`inspector`), the notifications dialog
//! (`notifications`) and the chrome's own copy (`copy`).
use gpui_kit::component::TitleBar;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::qa_marks::QaId;
use crate::theme::{Settings, mode};
use crate::workspace::Workspace;

mod commands;
mod copy;
mod inspector;
mod notifications;
mod palette;
mod settings;
#[cfg(test)]
mod tests;

pub use commands::bind_keys;
pub(crate) use commands::key_actions;
pub(crate) use inspector::{ChromePanel, build_chrome_panel, freshness};
// Named by the layout tests (`layouts::{dense,queue}::tests`) at its old path.
#[cfg(test)]
pub(crate) use settings::commit_settings;

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

impl Workspace {
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
            copy::JUMP_TIP_ALT
        } else {
            copy::JUMP_TIP
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
                    .child(if alt {
                        copy::JUMP_KEYCAP_ALT
                    } else {
                        copy::JUMP_KEYCAP
                    }),
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
                .when_some(self.layout_title_bar_extra(cx), |d, e| d.child(e))
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
                            .tooltip(copy::SETTINGS_TIP)
                            .render(mode, move |window, cx| {
                                let _ = ws.update(cx, |w, cx| w.open_settings(window, cx));
                            }),
                        ),
                ),
        )
    }
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
