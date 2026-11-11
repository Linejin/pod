//! Settings: the theme commands of the palette, saving settings, and the
//! settings page (모양 · 글꼴 · 일반) the chrome panel shows.
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::inspector::ChromePanel;
use crate::theme::{Settings, mode};
use crate::workspace::{ToastKind, Workspace};

/// Palette entries "테마: …" (docs/10 §D5.3): presets, modes, user styles, reload.
pub(super) fn theme_palette_entries() -> Vec<(String, String, String)> {
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
    for (id, name) in super::copy::THEME_MODES {
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
pub(super) fn theme_command(id: &str, settings: &Settings) -> Option<Settings> {
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
pub(super) const CATEGORIES: [(&str, pod_ui::Icon); 3] = [
    (crate::strings::APPEARANCE, pod_ui::Icon::Palette),
    (crate::strings::SETTINGS_FONTS, pod_ui::Icon::Type),
    (crate::strings::SETTINGS_GENERAL, pod_ui::Icon::Settings),
];
pub(super) const UI_FONTS: &[&str] = &["Segoe UI", "Malgun Gothic", "Consolas"];
pub(super) const UI_SIZES: &[&str] = &["12", "13", "14", "16"];
pub(super) const TERMINAL_FONTS: &[&str] = &["Cascadia Mono", "Consolas", "D2Coding"];
pub(super) const TERMINAL_SIZES: &[&str] = &["12", "14", "16", "18"];

impl ChromePanel {
    /// Settings (docs/10 §D4.6): quiet category column on the left, the category's content on the right.
    pub(super) fn settings_view(&self, cx: &mut Context<Self>) -> Div {
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
pub(super) fn select_options(values: &[&str], current: &str) -> (Vec<String>, usize) {
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
