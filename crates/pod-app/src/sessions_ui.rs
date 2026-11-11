//! "발견된 세션" page (docs/18 §7): agent sessions found on this PC, with
//! resume / register / hide. Discovery runs in podd (`sessions.*`); this
//! module only shows and asks. Uses the existing components and tokens.
//!
//! Layout (A-P7): this file owns the page's state ([`SessionsUi`]), the panel
//! with its dock identity (`PodSessions`) and the open / is-open entry points.
//! The actions a session offers, headings, ages and the card / page lines
//! are in [`model`]; the `sessions.*` requests with their busy / error
//! transitions and the resumed terminal in [`controller`]; the page and the
//! cards in [`view`]; the wording ([`s`]) in [`copy`]. Tests:
//! `sessions_ui::tests` (pure, and the page in the headless workspace against
//! a scripted podd on fixture data).

use gpui_kit::component::dock::panel_handle;
use gpui_kit::*;
use podd::protocol::SessionsList;

use crate::workspace::Workspace;

mod controller;
mod copy;
mod model;
#[cfg(test)]
mod tests;
mod view;

pub(crate) use copy::s;

/// What the app knows about discovered sessions.
#[derive(Default)]
pub(crate) struct SessionsUi {
    pub list: Option<SessionsList>,
    pub loading: bool,
    pub scanning: bool,
    pub failed: bool,
    pub show_hidden: bool,
    /// A resumed terminal to open once the workspace lists it.
    pub pending_open: Option<String>,
    pub panel: Option<WeakEntity<SessionsPanel>>,
}

impl Global for SessionsUi {}

fn state(cx: &mut App) -> &mut SessionsUi {
    cx.default_global::<SessionsUi>()
}

/// Is the page open (the workspace's empty state must not cover it)?
pub(crate) fn page_open(cx: &App) -> bool {
    cx.try_global::<SessionsUi>()
        .and_then(|s| s.panel.as_ref())
        .is_some_and(|p| p.upgrade().is_some())
}

/// Open (or focus) the page in the center dock.
pub(crate) fn open_page(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    use gpui_kit::component::dock::{DockPlacement, PanelId};
    if let Some(p) = state(cx).panel.as_ref().and_then(|p| p.upgrade()) {
        ws.dock.update(cx, |d, cx| {
            d.select_panel(PanelId::from(p.entity_id()), window, cx)
        });
        let focus = p.read(cx).focus.clone();
        focus.focus(window, cx);
        controller::refresh(ws, cx);
        return;
    }
    let workspace = cx.entity().downgrade();
    let panel = cx.new(|cx| {
        let sub = workspace
            .upgrade()
            .map(|w| cx.observe(&w, |_, _, cx| cx.notify()));
        SessionsPanel {
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
    controller::refresh(ws, cx);
    ws.touch(cx);
}

pub(crate) struct SessionsPanel {
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    _sub: Option<Subscription>,
}

impl EventEmitter<gpui_kit::base::dock::PanelEvent> for SessionsPanel {}
impl Focusable for SessionsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_kit::base::dock::Panel for SessionsPanel {
    fn panel_name(&self) -> &'static str {
        "PodSessions"
    }
    fn dump(&self, _: &App) -> gpui_kit::base::dock::PanelState {
        gpui_kit::base::dock::PanelState::new("PodSessions")
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.defer(|cx| state(cx).panel = None);
    }
}
impl gpui_kit::component::dock::Panel for SessionsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(s::SECTION.into())
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(s::SECTION)
    }
}
