//! Control state and the button-like controls: icon button, semantic button, segments.
use super::focus::focus_style;
use super::icons::{Icon, icon, icon_sized};
use crate::tokens::{HEIGHTS, HIT_AREA, RADII, SPACE, color};
use crate::{Mode, Tokens};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
/// Icon button hit area for the active density (28 compact, 36 comfortable and up).
pub fn hit_area() -> f32 {
    if crate::themes::metrics().density.row >= 34. {
        36.
    } else {
        HIT_AREA
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ControlState {
    #[default]
    Ready,
    Disabled,
    Loading,
    Error(String),
}
impl ControlState {
    pub fn interactive(&self) -> bool {
        matches!(self, Self::Ready | Self::Error(_))
    }
}
/// A keyboard-operable icon control. The label is both accessible name and tooltip.
pub struct IconButton {
    pub id: ElementId,
    pub icon: Icon,
    pub label: SharedString,
    pub state: ControlState,
    /// Longer tooltip (e.g. with the shortcut); the label stays the accessible name.
    pub tooltip: Option<SharedString>,
}
impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            state: ControlState::Ready,
            tooltip: None,
        }
    }
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    pub fn render(
        self,
        mode: Mode,
        on_activate: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let t = Tokens::for_mode(mode);
        let enabled = self.state.interactive();
        let action = std::rc::Rc::new(on_activate);
        let key = action.clone();
        let label = self.tooltip.clone().unwrap_or_else(|| self.label.clone());
        div()
            .id(self.id)
            .role(Role::Button)
            .aria_label(self.label)
            .tab_index(if enabled { 0 } else { -1 })
            .size(px(hit_area()))
            .rounded(px(crate::themes::metrics().radius.control))
            .border_2()
            .border_color(gpui_kit::transparent_black())
            .flex()
            .items_center()
            .justify_center()
            .text_color(color(if enabled {
                t.text_secondary
            } else {
                t.text_disabled
            }))
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(color(t.bg_hover)))
                    .active(|s| s.bg(color(t.bg_pressed)))
                    .focus_visible(|s| focus_style(s, mode))
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
            })
            .child(icon(
                if self.state == ControlState::Loading {
                    Icon::LoaderCircle
                } else {
                    self.icon
                },
                if enabled {
                    t.text_secondary
                } else {
                    t.text_disabled
                },
            ))
            .on_click(move |_, w, cx| {
                if enabled {
                    action(w, cx);
                }
            })
            .on_key_down(move |e, w, cx| {
                if enabled && matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    key(w, cx);
                    cx.stop_propagation();
                }
            })
    }
}

/// Visible text of a [`semantic_button`] while it runs (Korean UI; the accessible name stays the label).
pub const BUSY_LABEL: &str = "처리 중…";
pub(super) fn button_text(label: SharedString, state: &ControlState) -> SharedString {
    if *state == ControlState::Loading {
        BUSY_LABEL.into()
    } else {
        label
    }
}

/// Semantic action control: hover adds a border and press adds an inset, not color alone.
/// Status indicators must use StatusBadge instead.
pub fn semantic_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: crate::Tone,
    mode: Mode,
    state: ControlState,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Tokens::for_mode(mode);
    let (fg, bg) = tone.colors(t);
    let label = label.into();
    let enabled = state.interactive();
    let activate = std::rc::Rc::new(activate);
    let key = activate.clone();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(if enabled { 0 } else { -1 })
        .h(px(HEIGHTS[1]))
        .px(px(SPACE[3]))
        .rounded(px(RADII[2]))
        .flex()
        .items_center()
        .border_1()
        .border_color(color(bg))
        .bg(color(bg))
        .text_color(color(if enabled { fg } else { t.text_disabled }))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.border_color(color(fg)))
                .active(|s| {
                    s.shadow(vec![BoxShadow {
                        inset: true,
                        color: color(fg),
                        offset: point(px(0.), px(1.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                    }])
                })
                .focus_visible(|s| focus_style(s, mode))
        })
        .child(button_text(label, &state))
        .on_click(move |_, w, cx| {
            if enabled {
                activate(w, cx);
            }
        })
        .on_key_down(move |e, w, cx| {
            if enabled && matches!(e.keystroke.key.as_str(), "enter" | "space") {
                key(w, cx);
                cx.stop_propagation();
            }
        })
}

/// One option of a segmented control: neutral raised face + 600 weight when selected (no filled accent).
/// Exposed as a radio button whose checked state is the selection (UIA SelectionItem.IsSelected),
/// so the current choice is never carried by colour alone (docs/10 §4.1).
pub fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: Option<Icon>,
    selected: bool,
    mode: Mode,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = label.into();
    choice(
        id,
        label.clone(),
        label,
        glyph,
        selected,
        true,
        mode,
        activate,
    )
}

/// [`segment`] with its own accessible name (e.g. `밀도: 고밀도`) and an enabled flag.
#[allow(clippy::too_many_arguments)]
pub fn choice(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    name: impl Into<SharedString>,
    glyph: Option<Icon>,
    selected: bool,
    enabled: bool,
    mode: Mode,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Tokens::for_mode(mode);
    let activate = std::rc::Rc::new(activate);
    let key = activate.clone();
    let fg = if !enabled {
        t.text_disabled
    } else if selected {
        t.text_primary
    } else {
        t.text_secondary
    };
    div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(name.into())
        .aria_toggled(if selected {
            Toggled::True
        } else {
            Toggled::False
        })
        .tab_index(if enabled { 0 } else { -1 })
        .h(px(24.))
        .px(px(8.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(6.))
        .rounded(px(crate::themes::metrics().radius.control))
        .text_size(px(12.))
        .whitespace_nowrap()
        .text_color(color(fg))
        .border_1()
        .border_color(color(if selected {
            t.border_subtle
        } else {
            t.bg_chrome
        }))
        .when(selected, |d| {
            d.bg(color(t.bg_raised)).font_weight(FontWeight::SEMIBOLD)
        })
        .when(enabled && !selected, |d| {
            d.cursor_pointer().hover(|s| s.bg(color(t.bg_hover)))
        })
        .when(enabled, |d| d.focus_visible(|s| focus_style(s, mode)))
        .when_some(glyph, |d, g| d.child(icon_sized(g, 14., fg)))
        .child(label.into())
        .on_click(move |_, w, cx| {
            if enabled {
                activate(w, cx);
            }
        })
        .on_key_down(move |e, w, cx| {
            if enabled && matches!(e.keystroke.key.as_str(), "enter" | "space") {
                key(w, cx);
                cx.stop_propagation();
            }
        })
}
/// Container for [`segment`]s.
pub fn segmented(mode: Mode) -> Div {
    let t = Tokens::for_mode(mode);
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(crate::themes::metrics().radius.control + 2.))
        .bg(color(t.bg_chrome))
}
/// [`segmented`] as a named radio group (`테마`, `밀도` …); wraps when narrow.
pub fn radio_group(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    mode: Mode,
) -> Stateful<Div> {
    segmented(mode)
        .id(id)
        .role(Role::RadioGroup)
        .aria_label(name.into())
        .flex_wrap()
        .min_w_0()
}
