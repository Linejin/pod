//! Focus ownership, modal Tab/Esc handling, list navigation and the focus ring.
use super::buttons::ControlState;
use crate::tokens::{FOCUS_OFFSET, FOCUS_RING, color};
use crate::{Mode, Tokens};
use gpui_kit::*;
/// Small deterministic focus ownership stack. Closing a nested owner returns to its opener.
#[derive(Debug)]
pub struct FocusStack<T> {
    stack: Vec<T>,
}
impl<T> Default for FocusStack<T> {
    fn default() -> Self {
        Self { stack: Vec::new() }
    }
}
impl<T> FocusStack<T> {
    pub fn enter(&mut self, previous: T) {
        self.stack.push(previous);
    }
    pub fn leave(&mut self) -> Option<T> {
        self.stack.pop()
    }
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}
#[derive(Default)]
pub struct FocusReturn {
    stack: FocusStack<FocusHandle>,
}
impl FocusReturn {
    pub fn enter(&mut self, target: &FocusHandle, window: &mut Window, cx: &mut App) {
        if let Some(previous) = window.focused(cx) {
            self.stack.enter(previous);
        }
        target.focus(window, cx);
    }
    pub fn leave(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(previous) = self.stack.leave() {
            previous.focus(window, cx);
        }
    }
}

gpui_kit::actions!(pod_modal, [ModalNext, ModalPrevious, ModalClose]);
/// Set once the modal keys are in the app's keymap.
struct ModalKeysBound;
impl Global for ModalKeysBound {}
/// Tab / Shift+Tab / Escape in a `PodModal` frame. Binds once per app: later
/// calls (every `apply_theme`, i.e. every settings apply) leave the keymap as
/// it is instead of adding the three bindings again.
pub fn install_key_bindings(cx: &mut App) {
    if cx.has_global::<ModalKeysBound>() {
        return;
    }
    cx.set_global(ModalKeysBound);
    cx.bind_keys([
        KeyBinding::new("tab", ModalNext, Some("PodModal")),
        KeyBinding::new("shift-tab", ModalPrevious, Some("PodModal")),
        KeyBinding::new("escape", ModalClose, Some("PodModal")),
    ]);
}
/// Attach to the modal frame after render. Host passes only enabled tab stops.
pub fn modal_keyboard(
    frame: Stateful<Div>,
    owner: &FocusHandle,
    stops: Vec<FocusHandle>,
    close: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let previous = stops.clone();
    frame
        .track_focus(owner)
        .key_context("PodModal")
        .role(Role::Dialog)
        .on_action(move |_: &ModalNext, w, cx| {
            if !stops.is_empty() {
                let i = stops.iter().position(|h| h.is_focused(w));
                stops[next_focus(i, stops.len(), false)].focus(w, cx);
            }
            cx.stop_propagation();
        })
        .on_action(move |_: &ModalPrevious, w, cx| {
            if !previous.is_empty() {
                let i = previous.iter().position(|h| h.is_focused(w));
                previous[next_focus(i, previous.len(), true)].focus(w, cx);
            }
            cx.stop_propagation();
        })
        .on_action(move |_: &ModalClose, w, cx| {
            close(w, cx);
            cx.stop_propagation();
        })
}
pub fn next_focus(current: Option<usize>, len: usize, backwards: bool) -> usize {
    if len == 0 {
        return 0;
    }
    match current {
        None => {
            if backwards {
                len - 1
            } else {
                0
            }
        }
        Some(i) => {
            if backwards {
                (i + len - 1) % len
            } else {
                (i + 1) % len
            }
        }
    }
}

/// Stable-ID list navigation; callers filter IDs without resetting a surviving selection.
#[derive(Clone, Debug, Default)]
pub struct ListSelection {
    pub ids: Vec<String>,
    pub selected: Option<String>,
}
impl ListSelection {
    pub fn replace(&mut self, ids: Vec<String>) {
        self.ids = ids;
        if !self
            .selected
            .as_ref()
            .is_some_and(|id| self.ids.contains(id))
        {
            self.selected = self.ids.first().cloned();
        }
    }
    pub fn navigate(&mut self, backwards: bool) {
        if self.ids.is_empty() {
            self.selected = None;
            return;
        }
        let i = self
            .selected
            .as_ref()
            .and_then(|id| self.ids.iter().position(|x| x == id));
        self.selected = Some(self.ids[next_focus(i, self.ids.len(), backwards)].clone());
    }
    /// Never turn IME composition confirmation into command execution.
    pub fn activation(&self, composing: bool, state: &ControlState) -> Option<&str> {
        if composing || !state.interactive() {
            None
        } else {
            self.selected.as_deref()
        }
    }
}

/// A 2px (style `focus_width`; 3 in 14 접근성) focus ring separated from the control by a 2px gap.
pub fn focus_style(style: StyleRefinement, mode: Mode) -> StyleRefinement {
    let t = Tokens::for_mode(mode);
    let ring = crate::themes::metrics().focus_width.max(FOCUS_RING);
    style.shadow(vec![
        BoxShadow {
            inset: false,
            color: color(t.focus_ring),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(FOCUS_OFFSET + ring),
        },
        BoxShadow {
            inset: false,
            color: color(t.bg_surface),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(FOCUS_OFFSET),
        },
    ])
}
