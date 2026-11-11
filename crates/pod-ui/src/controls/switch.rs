//! Boolean switch with visible Korean state text and the UIA Toggle pattern.
use super::A11y;
use crate::components::focus_style;
use crate::tokens::{HEIGHTS, SPACE, color};
use crate::{Mode, Tokens};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

// ── Switch ─────────────────────────────────────────────────────────────────

/// Visible state text next to a switch (Korean; the state is never colour or knob position only).
pub fn switch_state_text(on: bool) -> &'static str {
    if on { "켬" } else { "끔" }
}
/// UIA: Toggle pattern (ToggleState On/Off) named after the setting.
pub fn switch_a11y(label: &str, on: bool) -> A11y {
    A11y {
        toggled: Some(if on { Toggled::True } else { Toggled::False }),
        ..A11y::new(Role::Switch, label.to_string())
    }
}

/// Boolean setting: 32×18 track + 12px knob + `켬`/`끔`. Space/Enter or click toggles (once).
pub fn switch(
    id: impl Into<ElementId>,
    label: &str,
    on: bool,
    enabled: bool,
    mode: Mode,
    toggle: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Tokens::for_mode(mode);
    let (track, knob, edge) = match (enabled, on) {
        (false, _) => (t.bg_surface, t.text_disabled, t.text_disabled),
        (true, true) => (
            t.action_primary_bg,
            t.action_primary_fg,
            t.action_primary_bg,
        ),
        (true, false) => (t.bg_surface, t.text_secondary, t.border_control),
    };
    switch_a11y(label, on)
        .apply(div().id(id))
        .tab_index(if enabled { 0 } else { -1 })
        .h(px(HEIGHTS[3]))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(SPACE[2]))
        .rounded(px(crate::themes::metrics().radius.control))
        .text_size(px(13.))
        .text_color(color(if enabled {
            t.text_primary
        } else {
            t.text_disabled
        }))
        .when(enabled, |d| {
            d.cursor_pointer().focus_visible(|s| focus_style(s, mode))
        })
        .child(
            div()
                .w(px(32.))
                .h(px(18.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .when(on, |d| d.justify_end())
                .px(px(2.))
                .rounded_full()
                .border_1()
                .border_color(color(edge))
                .bg(color(track))
                .child(div().size(px(12.)).rounded_full().bg(color(knob))),
        )
        .child(switch_state_text(on))
        // Click and Enter/Space (GPUI maps their key-up on a focused element to a click):
        // one activation path, so a key press toggles exactly once.
        .on_click(move |_, w, cx| {
            if enabled {
                toggle(w, cx);
            }
        })
}
