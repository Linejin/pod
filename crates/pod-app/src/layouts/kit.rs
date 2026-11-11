//! Layout kit (docs/20-refactor/app.md §2, card A-F0): the narrow contract a
//! layout mode renders against, and the presentation pieces the modes share.
//!
//! Nothing here names the Workspace: `workspace/layout_host.rs` (A-F1)
//! implements [`LayoutHost`] and [`ActionSink`] over it. Until a mode offers a
//! [`LayoutRenderer`] through `LayoutModule::renderer`, it keeps rendering
//! through `LayoutModule::render_body`; the modes move over one at a time.
mod clock;
mod metrics;
mod model;
#[cfg(test)]
mod tests;
mod widgets;

pub(crate) use clock::Clock;
pub(crate) use metrics::{DEFAULT_FONT_SIZE, LayoutMetrics, color};
pub(crate) use model::{
    ActionSink, LayoutActions, LayoutFrame, LayoutHost, LayoutRenderer, OpenPanels, Slot, UiIntent,
    connection_warning,
};
pub(crate) use widgets::{
    BodyFlow, ConnectionShell, Island, ToastAnchor, panel_header, warning_banner,
};
