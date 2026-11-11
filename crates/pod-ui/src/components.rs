//! Shared GPUI components. Each topic lives in its own module; this file keeps the
//! `pod_ui::components::*` paths (and the crate root's) as explicit re-exports.
mod buttons;
mod focus;
mod frames;
mod icons;
mod rows;
#[cfg(test)]
mod tests;
mod text;

pub use buttons::{
    BUSY_LABEL, ControlState, IconButton, choice, hit_area, radio_group, segment, segmented,
    semantic_button,
};
pub use focus::{
    FocusReturn, FocusStack, ListSelection, ModalClose, ModalNext, ModalPrevious, focus_style,
    install_key_bindings, modal_keyboard, next_focus,
};
pub use frames::{Frame, FrameKind, banner, toast_timeout};
pub use icons::{Icon, icon, icon_sized, icon_text, kit_icon};
pub use rows::{TreeRow, count_badge, status_dot};
pub use text::{KEEP_ALL_MAX, keep_all, keep_all_start, keep_all_unbreakable, keep_all_words};
