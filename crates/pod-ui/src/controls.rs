//! Settings controls (docs/10 §D4.6 Settings): quiet category column, select (dropdown) and switch.
//! Each control's accessibility is a plain [`A11y`] value so tests can check it without a window.
use crate::components::{Icon, focus_style, icon_sized};
use crate::tokens::*;
use crate::{Mode, Tokens};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;

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

// ── Select ─────────────────────────────────────────────────────────────────

/// Width of a settings control column (docs/10 §D4.6: 폭200 control).
pub const CONTROL_WIDTH: f32 = 200.;

/// What a key (or click) asks a [`select`] to do; the caller owns the open/highlight state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectEvent {
    /// Open with this option highlighted.
    Open(usize),
    /// Move the highlight (list open).
    Move(usize),
    /// Choose this option and close.
    Pick(usize),
    Close,
}

/// Keyboard model (Windows combo box): closed — Up/Down/Home/End change the value directly,
/// Alt+Down/Enter/Space/F4 open; open — Up/Down/Home/End move, Enter/Space/Alt+Up pick, Esc/Tab close.
pub fn select_key(
    key: &str,
    alt: bool,
    open: Option<usize>,
    selected: Option<usize>,
    len: usize,
) -> Option<SelectEvent> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    match open {
        None => {
            let at = selected.unwrap_or(0);
            match key {
                "down" if alt => Some(SelectEvent::Open(at)),
                "enter" | "space" | "f4" => Some(SelectEvent::Open(at)),
                "down" => Some(SelectEvent::Pick(selected.map_or(0, |s| (s + 1).min(last)))),
                "up" => Some(SelectEvent::Pick(
                    selected.map_or(0, |s| s.saturating_sub(1)),
                )),
                "home" => Some(SelectEvent::Pick(0)),
                "end" => Some(SelectEvent::Pick(last)),
                _ => None,
            }
        }
        Some(h) => match key {
            "up" if alt => Some(SelectEvent::Pick(h)),
            "enter" | "space" => Some(SelectEvent::Pick(h)),
            "escape" | "tab" | "f4" => Some(SelectEvent::Close),
            "down" => Some(SelectEvent::Move((h + 1).min(last))),
            "up" => Some(SelectEvent::Move(h.saturating_sub(1))),
            "home" => Some(SelectEvent::Move(0)),
            "end" => Some(SelectEvent::Move(last)),
            _ => None,
        },
    }
}

/// A click on the closed/open control. GPUI also turns an Enter/Space key-up on a focused element
/// into a click; keys are already handled on key-down ([`select_key`]), so keyboard clicks are
/// ignored — otherwise Enter would pick and then reopen the list.
pub fn select_click(
    enabled: bool,
    keyboard: bool,
    open: Option<usize>,
    selected: Option<usize>,
) -> Option<SelectEvent> {
    if !enabled || keyboard {
        return None;
    }
    Some(if open.is_some() {
        SelectEvent::Close
    } else {
        SelectEvent::Open(selected.unwrap_or(0))
    })
}

/// Type-ahead: the next option (after `from`, wrapping) whose label starts with the typed
/// character, case-insensitively. `None` for non-printing input or no match.
pub fn select_type_ahead(ch: &str, options: &[SharedString], from: Option<usize>) -> Option<usize> {
    let mut chars = ch.chars();
    let c = chars.next()?;
    if chars.next().is_some() || c.is_control() || c.is_whitespace() || options.is_empty() {
        return None;
    }
    let n = options.len();
    let start = from.map_or(0, |f| f + 1);
    (0..n).map(|k| (start + k) % n).find(|&i| {
        options[i]
            .chars()
            .next()
            .is_some_and(|f| f.to_lowercase().eq(c.to_lowercase()))
    })
}

/// Keeps the highlighted option visible in the 8-row list (scrolls the least, on next prepaint).
/// Open starts from the top; Move (keys, type-ahead) scrolls only when the row is outside the
/// view. Hover moves are not passed here: that row is already under the pointer.
pub fn select_reveal(scroll: &ScrollHandle, ev: SelectEvent) {
    match ev {
        SelectEvent::Open(i) => {
            scroll.set_offset(Point::default());
            scroll.scroll_to_item(i);
        }
        SelectEvent::Move(i) => scroll.scroll_to_item(i),
        SelectEvent::Pick(_) | SelectEvent::Close => {}
    }
}

/// The closed control: UIA ComboBox with ExpandCollapse (expanded) and its value.
pub fn select_a11y(name: &str, value: &str, open: bool) -> A11y {
    A11y {
        expanded: Some(open),
        value: Some(value.to_string().into()),
        ..A11y::new(Role::ComboBox, name.to_string())
    }
}
/// One option of the open list: UIA ListItem with SelectionItem.IsSelected.
pub fn option_a11y(label: &str, selected: bool) -> A11y {
    A11y {
        selected: Some(selected),
        ..A11y::new(Role::ListBoxOption, label.to_string())
    }
}

type SelectCallback = Rc<dyn Fn(SelectEvent, &mut Window, &mut App)>;

// Keep the focus subscription with the element, not the caller. Refresh the callback
// each render so blur cannot close a different select through an old host snapshot.
struct SelectFocus {
    focus: FocusHandle,
    open: bool,
    on_event: SelectCallback,
    _subscriptions: Vec<Subscription>,
}

impl SelectFocus {
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.open) {
            (self.on_event)(SelectEvent::Close, window, cx);
        }
    }
}

#[derive(IntoElement)]
struct SelectControl {
    id: ElementId,
    control: Stateful<Div>,
    open: bool,
    enabled: bool,
    on_event: SelectCallback,
}

impl RenderOnce for SelectControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id, cx, |window, cx| {
            let focus = cx.focus_handle();
            let subscriptions = vec![
                cx.on_focus_out(&focus, window, |s: &mut SelectFocus, _, w, cx| {
                    s.close(w, cx)
                }),
                // Windows retains the focused handle on Alt+Tab. Close on deactivation
                // as well as focus-path changes, without stealing focus on return.
                cx.observe_window_activation(window, |s: &mut SelectFocus, w, cx| {
                    if !w.is_window_active() {
                        s.close(w, cx);
                    }
                }),
            ];
            SelectFocus {
                focus,
                open: self.open,
                on_event: self.on_event.clone(),
                _subscriptions: subscriptions,
            }
        });
        let focus = state.update(cx, |s, _| {
            s.open = self.open;
            s.on_event = self.on_event;
            s.focus.clone().tab_stop(self.enabled)
        });
        self.control.track_focus(&focus)
    }
}

/// Dropdown (docs/10 §D4.6 font select): 32px, border.control, chevron, 200 wide. The open list
/// floats below (deferred) with a check on the current option, so the choice is not colour only.
/// `open` is the highlighted index while open; every interaction is reported through `on_event`.
/// `scroll` is the caller's persistent handle for the open list: key moves, type-ahead and Open
/// scroll the highlighted row into view ([`select_reveal`]).
#[allow(clippy::too_many_arguments)]
pub fn select(
    id: impl Into<ElementId>,
    name: &str,
    options: &[SharedString],
    selected: Option<usize>,
    open: Option<usize>,
    scroll: &ScrollHandle,
    enabled: bool,
    mode: Mode,
    on_event: impl Fn(SelectEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let t = Tokens::for_mode(mode);
    let id = id.into();
    let on_event = Rc::new(on_event);
    let value: SharedString = selected
        .and_then(|i| options.get(i).cloned())
        .unwrap_or_default();
    let len = options.len();
    let radius = crate::themes::metrics().radius.control;
    let fg = if enabled {
        t.text_primary
    } else {
        t.text_disabled
    };
    let list = open.map(|highlight| {
        let mut list = div()
            .id("options")
            .role(Role::ListBox)
            .aria_label(name.to_string())
            .absolute()
            .top(px(HEIGHTS[1] + HALF_SPACE))
            .left_0()
            .w_full()
            .max_h(px(HEIGHTS[2] * 8. + SPACE[1] * 2.))
            .overflow_y_scroll()
            .track_scroll(scroll)
            .p(px(SPACE[1]))
            .flex()
            .flex_col()
            .rounded(px(radius))
            .border_1()
            .border_color(color(t.border_strong))
            .bg(color(t.bg_overlay))
            .shadow_md()
            .on_mouse_down_out({
                let on_event = on_event.clone();
                move |_, w, cx| on_event(SelectEvent::Close, w, cx)
            });
        for (i, label) in options.iter().enumerate() {
            let current = selected == Some(i);
            let on_click = on_event.clone();
            let on_hover = on_event.clone();
            list = list.child(
                option_a11y(label, current)
                    .apply(div().id(i))
                    .when(i == highlight, |d| {
                        d.aria_active_descendant().bg(color(t.bg_hover))
                    })
                    .h(px(HEIGHTS[2]))
                    .flex_shrink_0()
                    .px(px(SPACE[2]))
                    .flex()
                    .items_center()
                    .gap(px(SPACE[2]))
                    .rounded(px(radius))
                    .text_size(px(13.))
                    .text_color(color(t.text_primary))
                    .cursor_pointer()
                    .child(div().w(px(14.)).flex_shrink_0().when(current, |d| {
                        d.child(icon_sized(Icon::Check, 14., t.accent_fg))
                    }))
                    .child(div().min_w_0().truncate().child(label.clone()))
                    .on_hover(move |hovered, w, cx| {
                        if *hovered {
                            on_hover(SelectEvent::Move(i), w, cx);
                        }
                    })
                    .on_click(move |_, w, cx| {
                        on_click(SelectEvent::Pick(i), w, cx);
                        cx.stop_propagation();
                    }),
            );
        }
        deferred(list).with_priority(1)
    });
    let click = on_event.clone();
    let key = on_event.clone();
    let (click_scroll, key_scroll) = (scroll.clone(), scroll.clone());
    let labels: Rc<[SharedString]> = options.into();
    let control = select_a11y(name, &value, open.is_some())
        .apply(div().id(id.clone()))
        .relative()
        .w(px(CONTROL_WIDTH))
        .h(px(HEIGHTS[1]))
        .flex_shrink_0()
        .pl(px(SPACE[3] - 2.))
        .pr(px(SPACE[2]))
        .flex()
        .items_center()
        .gap(px(SPACE[2]))
        .rounded(px(radius))
        .border_1()
        .border_color(color(if open.is_some() {
            t.focus_ring
        } else {
            t.border_control
        }))
        .bg(color(t.bg_surface))
        .text_size(px(13.))
        .text_color(color(fg))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.border_color(color(t.border_strong)))
                .focus_visible(|s| focus_style(s, mode))
        })
        .child(div().flex_1().min_w_0().truncate().child(value))
        .child(icon_sized(
            if open.is_some() {
                Icon::ChevronUp
            } else {
                Icon::ChevronDown
            },
            16.,
            if enabled {
                t.text_secondary
            } else {
                t.text_disabled
            },
        ))
        .children(list)
        .on_click(move |e, w, cx| {
            if let Some(ev) = select_click(enabled, e.is_keyboard(), open, selected) {
                select_reveal(&click_scroll, ev);
                click(ev, w, cx);
            }
        })
        .on_key_down(move |e, w, cx| {
            let m = &e.keystroke.modifiers;
            if !enabled || m.control || m.platform {
                return;
            }
            let k = e.keystroke.key.as_str();
            let ev = if m.shift && k == "tab" {
                // Shift+Tab leaves like Tab: close the list.
                open.map(|_| SelectEvent::Close)
            } else if m.shift {
                None
            } else {
                select_key(k, m.alt, open, selected, len)
            }
            .or_else(|| {
                if m.alt {
                    return None;
                }
                let ch = e.keystroke.key_char.as_deref()?;
                let i = select_type_ahead(ch, &labels, open.or(selected))?;
                Some(if open.is_some() {
                    SelectEvent::Move(i)
                } else {
                    SelectEvent::Pick(i)
                })
            });
            if let Some(ev) = ev {
                select_reveal(&key_scroll, ev);
                key(ev, w, cx);
                // Tab closes the list but still moves focus.
                if k != "tab" {
                    cx.stop_propagation();
                }
            }
        });
    SelectControl {
        id,
        control,
        open: open.is_some(),
        enabled,
        on_event,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    // GPUI's glob exports its own `test` attribute; these are plain Rust tests.
    use core::prelude::v1::test;

    #[test]
    fn category_keys_move_and_stop_at_ends() {
        assert_eq!(category_step(0, 3, "down"), Some(1));
        assert_eq!(category_step(2, 3, "down"), Some(2));
        assert_eq!(category_step(0, 3, "up"), Some(0));
        assert_eq!(category_step(1, 3, "end"), Some(2));
        assert_eq!(category_step(2, 3, "home"), Some(0));
        assert_eq!(category_step(1, 3, "left"), None);
        assert_eq!(category_step(0, 0, "down"), None);
    }

    #[test]
    fn category_is_a_selectable_tab() {
        let a = category_a11y("모양", true);
        assert_eq!(a.role, Role::Tab);
        assert_eq!(a.selected, Some(true));
        assert_eq!(
            category_a11y("글꼴", false).selected,
            Some(false),
            "unselected tabs still expose IsSelected=false"
        );
        assert_eq!(a.toggled, None);
    }

    #[test]
    fn select_closed_keys_change_value_or_open() {
        use SelectEvent::*;
        assert_eq!(select_key("down", false, None, Some(1), 4), Some(Pick(2)));
        assert_eq!(select_key("down", false, None, Some(3), 4), Some(Pick(3)));
        assert_eq!(select_key("up", false, None, Some(0), 4), Some(Pick(0)));
        assert_eq!(
            select_key("down", false, None, None, 4),
            Some(Pick(0)),
            "custom value: first option"
        );
        assert_eq!(select_key("down", true, None, Some(2), 4), Some(Open(2)));
        assert_eq!(select_key("enter", false, None, Some(2), 4), Some(Open(2)));
        assert_eq!(select_key("space", false, None, None, 4), Some(Open(0)));
        assert_eq!(select_key("end", false, None, Some(0), 4), Some(Pick(3)));
        assert_eq!(
            select_key("escape", false, None, Some(0), 4),
            None,
            "Esc on a closed select is not ours"
        );
        assert_eq!(select_key("down", false, None, Some(0), 0), None);
    }

    #[test]
    fn select_open_keys_move_pick_close() {
        use SelectEvent::*;
        assert_eq!(
            select_key("down", false, Some(1), Some(1), 4),
            Some(Move(2))
        );
        assert_eq!(select_key("up", false, Some(0), Some(1), 4), Some(Move(0)));
        assert_eq!(select_key("end", false, Some(0), Some(1), 4), Some(Move(3)));
        assert_eq!(
            select_key("enter", false, Some(3), Some(1), 4),
            Some(Pick(3))
        );
        assert_eq!(select_key("up", true, Some(2), Some(1), 4), Some(Pick(2)));
        assert_eq!(
            select_key("escape", false, Some(2), Some(1), 4),
            Some(Close)
        );
        assert_eq!(select_key("tab", false, Some(2), Some(1), 4), Some(Close));
    }

    #[test]
    fn select_keyboard_click_does_not_reopen_after_enter() {
        use SelectEvent::*;
        // Enter key-down picks (and closes); the key-up click GPUI synthesizes must do nothing.
        assert_eq!(
            select_key("enter", false, Some(1), Some(0), 3),
            Some(Pick(1))
        );
        assert_eq!(select_click(true, true, None, Some(1)), None);
        assert_eq!(select_click(true, true, Some(1), Some(1)), None);
        assert_eq!(select_click(true, false, None, Some(1)), Some(Open(1)));
        assert_eq!(select_click(true, false, Some(2), Some(1)), Some(Close));
        assert_eq!(select_click(false, false, None, Some(1)), None);
    }
    #[test]
    fn select_type_ahead_cycles_matches_case_insensitively() {
        let o: Vec<SharedString> = ["Dark", "dim", "Light", "dusk"]
            .into_iter()
            .map(Into::into)
            .collect();
        assert_eq!(select_type_ahead("d", &o, None), Some(0));
        assert_eq!(select_type_ahead("D", &o, Some(0)), Some(1));
        assert_eq!(select_type_ahead("d", &o, Some(3)), Some(0), "wraps");
        assert_eq!(select_type_ahead("l", &o, Some(2)), Some(2), "only match");
        assert_eq!(select_type_ahead("x", &o, None), None);
        assert_eq!(select_type_ahead("\t", &o, None), None);
        assert_eq!(select_type_ahead("dd", &o, None), None);
        assert_eq!(select_type_ahead("d", &[], None), None);
    }

    #[test]
    fn select_exposes_expand_collapse_value_and_selection() {
        let closed = select_a11y("UI 글꼴", "Segoe UI", false);
        assert_eq!(closed.role, Role::ComboBox);
        assert_eq!(closed.expanded, Some(false));
        assert_eq!(closed.value.as_deref(), Some("Segoe UI"));
        assert_eq!(
            select_a11y("UI 글꼴", "Segoe UI", true).expanded,
            Some(true)
        );
        let o = option_a11y("Consolas", true);
        assert_eq!((o.role, o.selected), (Role::ListBoxOption, Some(true)));
        assert_eq!(option_a11y("D2Coding", false).selected, Some(false));
    }

    #[test]
    fn switch_is_a_toggle_with_korean_state() {
        let on = switch_a11y("고대비 미리보기", true);
        assert_eq!(on.role, Role::Switch);
        assert_eq!(on.toggled, Some(Toggled::True));
        assert_eq!(on.selected, None, "a switch is Toggle, not SelectionItem");
        assert_eq!(
            switch_a11y("Windows 알림", false).toggled,
            Some(Toggled::False)
        );
        assert_eq!(
            (switch_state_text(true), switch_state_text(false)),
            ("켬", "끔")
        );
    }
}
