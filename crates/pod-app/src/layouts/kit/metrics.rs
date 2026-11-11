//! Font metrics every layout mode computes the same way, and the token colour
//! helper.
//!
//! Only the shared calculations live here: the UI font size, the text line it
//! implies and the terminal font family. Row, card and panel heights use
//! different formulas per mode and stay in each mode (docs/20-refactor/app.md §2).
use gpui_kit::{App, Hsla, SharedString};

/// Font size used when no settings are loaded (the copies in the modes use 13 too).
pub(crate) const DEFAULT_FONT_SIZE: f32 = 13.;

/// Font metrics of one frame, read once from settings and the active style.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct LayoutMetrics {
    /// Non-colour tokens of the active style (`pod_ui::themes::metrics`).
    pub(crate) style: pod_ui::themes::Metrics,
    /// UI font size: the user's size moved by the style's density and clamped
    /// (`theme::ui_font_size`).
    pub(crate) ui_font: f32,
    /// Terminal font family from settings; empty = the platform default.
    pub(crate) mono_font: SharedString,
}

impl LayoutMetrics {
    pub(crate) fn new(
        font_size: f32,
        style: pod_ui::themes::Metrics,
        mono_font: impl Into<SharedString>,
    ) -> Self {
        Self {
            style,
            ui_font: crate::theme::ui_font_size(font_size, style.density.ui_font),
            mono_font: mono_font.into(),
        }
    }

    /// From the settings global and the active style; without settings the
    /// font size is [`DEFAULT_FONT_SIZE`] and the mono font is empty.
    pub(crate) fn current(cx: &App) -> Self {
        let settings = cx.try_global::<crate::theme::Settings>();
        Self::new(
            settings.map_or(DEFAULT_FONT_SIZE, |s| s.font_size),
            pod_ui::themes::metrics(),
            settings
                .map(|s| s.terminal_font.clone())
                .unwrap_or_default(),
        )
    }

    /// One text line at the UI font in whole pixels, `ceil(ui_font × 1.5)`:
    /// what the modes' row and card heights grow with.
    pub(crate) fn line(&self) -> f32 {
        (self.ui_font * 1.5).ceil()
    }
}

/// A design-token colour (`0xRRGGBB`) as GPUI colour: the `tc` helper the
/// orchestration view, the queue and the dense layout each define.
pub(crate) fn color(hex: u32) -> Hsla {
    pod_ui::tokens::color(hex)
}
