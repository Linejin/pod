//! Settings controls (docs/10 §D4.6 Settings): quiet category column, select (dropdown) and switch.
//! Each control's accessibility is a plain [`A11y`] value so tests can check it without a window.
use crate::components::{Icon, focus_style, icon_sized};
use crate::tokens::*;
use crate::{Mode, Tokens};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

mod select;
mod switch;
#[cfg(test)]
mod tests;

pub use select::{
    CONTROL_WIDTH, SelectEvent, option_a11y, select, select_a11y, select_click, select_key,
    select_reveal, select_type_ahead,
};
pub use switch::{switch, switch_a11y, switch_state_text};

/// What an element exposes to UIA (role + states); applied by the renderers below.
#[derive(Clone, Debug, PartialEq)]
pub struct A11y {
    pub role: Role,
    pub name: SharedString,
    pub selected: Option<bool>,
    pub toggled: Option<Toggled>,
    pub expanded: Option<bool>,
    pub value: Option<SharedString>,
}
impl A11y {
    fn new(role: Role, name: impl Into<SharedString>) -> Self {
        Self {
            role,
            name: name.into(),
            selected: None,
            toggled: None,
            expanded: None,
            value: None,
        }
    }
    fn apply(self, d: Stateful<Div>) -> Stateful<Div> {
        let d = d.role(self.role).aria_label(self.name);
        let d = match self.selected {
            Some(s) => d.aria_selected(s),
            None => d,
        };
        let d = match self.toggled {
            Some(t) => d.aria_toggled(t),
            None => d,
        };
        let d = match self.expanded {
            Some(e) => d.aria_expanded(e),
            None => d,
        };
        match self.value {
            Some(v) => d.aria_value(v),
            None => d,
        }
    }
}

// ── Category column ────────────────────────────────────────────────────────

/// Width of the settings category column (docs/10 §D4.6: category160).
pub const CATEGORY_WIDTH: f32 = 160.;

/// Up/Down/Home/End in the category column; `None` = not a navigation key. Stops at the ends.
pub fn category_step(current: usize, len: usize, key: &str) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match key {
        "up" => Some(current.saturating_sub(1)),
        "down" => Some((current + 1).min(len - 1)),
        "home" => Some(0),
        "end" => Some(len - 1),
        _ => None,
    }
}

/// A category is a tab (UIA TabItem, SelectionItem.IsSelected) inside a vertical tab list.
pub fn category_a11y(label: &str, selected: bool) -> A11y {
    A11y {
        selected: Some(selected),
        ..A11y::new(Role::Tab, label.to_string())
    }
}

/// The quiet left column: a vertical tab list; Up/Down/Home/End call `on_move` with the new index.
pub fn category_list(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    selected: usize,
    len: usize,
    on_move: impl Fn(usize, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .role(Role::TabList)
        .aria_label(name.into())
        .aria_orientation(Orientation::Vertical)
        .w(px(CATEGORY_WIDTH))
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(HALF_SPACE))
        .on_key_down(move |e, w, cx| {
            if e.keystroke.modifiers.modified() {
                return;
            }
            if let Some(next) = category_step(selected, len, e.keystroke.key.as_str()) {
                on_move(next, w, cx);
                cx.stop_propagation();
            }
        })
}

/// One category row: 28px, icon 16 + 13px label; selected = bg.selected + 2px left accent + 600 weight
/// (docs/10 §D4.6 행 상태), never a filled button. Only the selected row is a Tab stop (roving).
pub fn category_item(
    id: impl Into<ElementId>,
    label: &str,
    glyph: Icon,
    selected: bool,
    focus: &FocusHandle,
    mode: Mode,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Tokens::for_mode(mode);
    let fg = if selected {
        t.text_primary
    } else {
        t.text_secondary
    };
    focus.clone().tab_index(0).tab_stop(selected);
    category_a11y(label, selected)
        .apply(div().id(id))
        .track_focus(focus)
        .h(px(HEIGHTS[2]))
        .pl(px(SPACE[2] - 2.))
        .pr(px(SPACE[2]))
        .flex()
        .items_center()
        .gap(px(SPACE[2]))
        .rounded(px(crate::themes::metrics().radius.control))
        .border_l_2()
        .border_color(color(if selected { t.accent_fg } else { t.bg_surface }))
        .text_size(px(13.))
        .whitespace_nowrap()
        .text_color(color(fg))
        .cursor_pointer()
        .when(selected, |d| {
            d.bg(color(t.bg_selected)).font_weight(FontWeight::SEMIBOLD)
        })
        .when(!selected, |d| d.hover(|s| s.bg(color(t.bg_hover))))
        .focus_visible(|s| focus_style(s, mode))
        .child(icon_sized(glyph, 16., fg))
        .child(div().min_w_0().truncate().child(label.to_string()))
        .on_click(move |_, w, cx| activate(w, cx))
}
