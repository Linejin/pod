//! Layout kit (docs/20-refactor/app.md §2, card A-F0): the narrow contract a
//! layout mode renders against, and the presentation pieces the modes share.
//!
//! Nothing here names the Workspace: `workspace/layout_host.rs` (A-F1)
//! implements [`LayoutHost`] and both halves of `model::ActionSink` over it —
//! [`HostHandles`] (render-safe) and [`HostCommands`] (update phase), no
//! default methods (rf2-kit). Every mode (the workbench too) draws through
//! the [`LayoutRenderer`] its `LayoutModule::renderer` returns (card T2).
mod clock;
mod host;
mod metrics;
mod model;
#[cfg(test)]
mod tests;
mod widgets;

pub(crate) use clock::Clock;
pub(crate) use host::{
    AppOnce, GateDone, HostChange, HostUpdate, OnChange, OnChangeIn, PanelWatch, WindowKey,
    state_in_window, states,
};
// The QA modules look a window's state up by key, and release one by hand.
#[cfg(test)]
pub(crate) use host::{release_window, state};
pub(crate) use metrics::{LayoutMetrics, color};
pub(crate) use model::{
    FrameToasts, HostCommands, HostHandles, LayoutActions, LayoutFrame, LayoutHost, LayoutRenderer,
    OpenPanels, PanelDetail, Slot, UiIntent,
};
pub(crate) use widgets::{
    BodyFlow, ConnectionShell, Island, ToastAnchor, panel_header, warning_banner,
};
