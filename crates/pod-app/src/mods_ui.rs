//! Extensions in the app (docs/16): the "확장" section of Settings (list,
//! enable/disable, permissions, log), mod palette commands and layout
//! presets, and `mod.notify` toasts. Mods run in podd; this module only
//! shows and asks.
//!
//! Layout (A-P6): this file owns what the app knows about the mods
//! ([`ModsUi`]) and the "확장" page's creation and dock identity
//! (`PodExtensions`). Labels, the card's permission toggles and lines, the
//! palette rows, the palette id grammar and notice routing are in [`model`];
//! the `mod.*` requests, podd's mod events and the palette dispatch in
//! [`controller`]; a preset's plan for the center dock in [`layouts`]; the
//! Settings row, the page and the cards in [`view`]; the Korean wording in
//! [`copy`] (its catalog stays `mods_ui::s`). Tests: `mods_ui::tests`.

use std::collections::BTreeMap;

use gpui_kit::component::dock::panel_handle;
use gpui_kit::*;
use podd::protocol::ModListResult;

use crate::workspace::Workspace;

mod controller;
mod copy;
mod layouts;
mod model;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use controller::{on_changed, on_notice, palette_entries, refresh, run};
pub(crate) use copy::s;
pub(crate) use model::error_summary;
pub(crate) use view::settings_link;

/// What the app knows about the daemon's mods.
#[derive(Default)]
pub(crate) struct ModsUi {
    pub list: Option<ModListResult>,
    pub loading: bool,
    pub failed: bool,
    pub log_open: Option<String>,
    pub logs: BTreeMap<String, Vec<String>>,
    /// The open "확장" page, if any.
    pub panel: Option<WeakEntity<ExtensionsPanel>>,
    /// Windows toasts for notices while Pod is not in front (created on first use).
    pub os: Option<crate::os_notify::OsToasts>,
    /// Routes toast clicks back to the workspace.
    pub os_clicks: Option<Task<()>>,
}

impl Global for ModsUi {}

fn state(cx: &mut App) -> &mut ModsUi {
    cx.default_global::<ModsUi>()
}

/// Is the "확장" page open (the workspace's empty state must not cover it)?
pub(crate) fn page_open(cx: &App) -> bool {
    cx.try_global::<ModsUi>()
        .and_then(|s| s.panel.as_ref())
        .is_some_and(|p| p.upgrade().is_some())
}

/// Open (or focus) the "확장" page in the center dock.
pub(crate) fn open_page(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    use gpui_kit::component::dock::{DockPlacement, PanelId};
    if let Some(p) = state(cx).panel.as_ref().and_then(|p| p.upgrade()) {
        ws.dock.update(cx, |d, cx| {
            d.select_panel(PanelId::from(p.entity_id()), window, cx)
        });
        let focus = p.read(cx).focus.clone();
        focus.focus(window, cx);
        return;
    }
    let workspace = cx.entity().downgrade();
    let panel = cx.new(|cx| {
        let sub = workspace
            .upgrade()
            .map(|w| cx.observe(&w, |_, _, cx| cx.notify()));
        ExtensionsPanel {
            workspace,
            focus: cx.focus_handle(),
            _sub: sub,
        }
    });
    state(cx).panel = Some(panel.downgrade());
    let focus = panel.read(cx).focus.clone();
    ws.dock.update(cx, |d, cx| {
        d.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx)
    });
    focus.focus(window, cx);
    refresh(ws, cx);
    ws.touch(cx);
}

/// The "확장" settings page (rendered in [`view`]).
pub(crate) struct ExtensionsPanel {
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    _sub: Option<Subscription>,
}

impl EventEmitter<gpui_kit::base::dock::PanelEvent> for ExtensionsPanel {}
impl Focusable for ExtensionsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_kit::base::dock::Panel for ExtensionsPanel {
    fn panel_name(&self) -> &'static str {
        "PodExtensions"
    }
    fn dump(&self, _: &App) -> gpui_kit::base::dock::PanelState {
        gpui_kit::base::dock::PanelState::new("PodExtensions")
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.defer(|cx| state(cx).panel = None);
    }
}
impl gpui_kit::component::dock::Panel for ExtensionsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(s::SECTION.into())
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(s::SECTION)
    }
}
