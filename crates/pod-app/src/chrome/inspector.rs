//! The dock panel of the chrome: the status inspector (Ctrl+Shift+S) or the
//! settings page (Ctrl+,; `settings.rs` draws it), one panel of each at most.
use gpui_kit::*;

use crate::theme::mode;
use crate::workspace::Workspace;

impl Workspace {
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_chrome_panel(false, window, cx);
    }
    pub(crate) fn open_inspector(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_chrome_panel(true, window, cx);
    }
    fn open_chrome_panel(&mut self, inspector: bool, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::dock::{DockPlacement, PanelId, panel_handle};
        let slot = if inspector {
            &mut self.inspector_panel
        } else {
            &mut self.settings_panel
        };
        if let Some(panel) = slot.as_ref().and_then(|p| p.upgrade()) {
            self.dock.update(cx, |d, cx| {
                d.select_panel(PanelId::from(panel.entity_id()), window, cx)
            });
            panel.read(cx).focus_handle(cx).focus(window, cx);
            return;
        }
        let workspace = cx.entity().downgrade();
        let panel = cx.new(|cx| ChromePanel::new(workspace, inspector, cx));
        *slot = Some(panel.downgrade());
        let focus = panel.read(cx).focus_handle(cx);
        self.dock.update(cx, |d, cx| {
            d.add_panel_view(
                panel_handle(panel),
                if inspector {
                    DockPlacement::Right
                } else {
                    DockPlacement::Center
                },
                if inspector { Some(px(304.)) } else { None },
                window,
                cx,
            )
        });
        focus.focus(window, cx);
        self.touch(cx);
    }
}

/// Timestamp stays on the VM; freshness is computed only when rendered.
pub(crate) fn freshness(status: Option<&crate::vm::StatusVm>) -> String {
    match status {
        Some(s) if s.since > 0 => {
            crate::tr!(S_AGO; ; crate::orch::now_ms().saturating_sub(s.since) / 1000)
        }
        _ => crate::strings::NOT_REPORTED.into(),
    }
}

pub(crate) struct ChromePanel {
    pub(super) workspace: WeakEntity<Workspace>,
    inspector: bool,
    _sub: Option<Subscription>,
    focus: FocusHandle,
    /// Settings: selected category (index into `settings::CATEGORIES`), one focus handle per row.
    pub(super) category: usize,
    pub(super) category_focus: [FocusHandle; 3],
    /// Settings: the open select and its highlighted option.
    pub(super) open_select: Option<(&'static str, usize)>,
    /// Scroll of the open select's list (one list is open at a time).
    pub(super) select_scroll: ScrollHandle,
}
impl ChromePanel {
    fn new(workspace: WeakEntity<Workspace>, inspector: bool, cx: &mut Context<Self>) -> Self {
        let sub = workspace
            .upgrade()
            .map(|w| cx.observe(&w, |_, _, cx| cx.notify()));
        let category_focus = [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()];
        ChromePanel {
            workspace,
            inspector,
            focus: cx.focus_handle(),
            _sub: sub,
            category: 0,
            category_focus,
            open_select: None,
            select_scroll: ScrollHandle::new(),
        }
    }
}

/// Restore without borrowing the workspace while DockArea::load holds its lease.
pub(crate) fn build_chrome_panel(
    ctx: gpui_kit::component::dock::PanelBuildContext,
    _: &mut Window,
    cx: &mut App,
) -> std::sync::Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let inspector = ctx.state().panel_name == "PodInspector";
    let g = cx.global::<crate::workspace::AppGlobals>();
    let (workspace, restore) = (g.workspace.clone(), g.restore.clone());
    let panel = cx.new(|cx| ChromePanel::new(workspace, inspector, cx));
    restore.borrow_mut().built.push(if inspector {
        crate::workspace::RestoredPanel::Inspector(panel.downgrade())
    } else {
        crate::workspace::RestoredPanel::Settings(panel.downgrade())
    });
    gpui_kit::component::dock::panel_handle(panel)
}
impl EventEmitter<gpui_kit::base::dock::PanelEvent> for ChromePanel {}
impl Focusable for ChromePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_kit::base::dock::Panel for ChromePanel {
    fn panel_name(&self) -> &'static str {
        if self.inspector {
            "PodInspector"
        } else {
            "PodSettings"
        }
    }
    fn dump(&self, _: &App) -> gpui_kit::base::dock::PanelState {
        gpui_kit::base::dock::PanelState::new(if self.inspector {
            "PodInspector"
        } else {
            "PodSettings"
        })
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        let inspector = self.inspector;
        cx.defer(move |cx| {
            let _ = ws.update(cx, |w, cx| {
                if inspector {
                    w.inspector_panel = None;
                } else {
                    w.settings_panel = None;
                }
                w.touch(cx);
            });
        });
    }
}
impl gpui_kit::component::dock::Panel for ChromePanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(
            if self.inspector {
                crate::strings::STATUS
            } else {
                crate::strings::SETTINGS
            }
            .into(),
        )
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(if self.inspector {
            crate::strings::STATUS
        } else {
            crate::strings::SETTINGS
        })
    }
}
impl ChromePanel {
    /// The status inspector: the focused terminal's badge and detail, its id
    /// and program, and how fresh its status is.
    fn inspector_view(&self, cx: &App) -> Div {
        let terminal = self.workspace.upgrade().and_then(|w| {
            let w = w.read(cx);
            w.focused_terminal
                .as_deref()
                .and_then(|id| w.terminals.iter().find(|t| t.id == id))
                .cloned()
                .map(|t| (t, w.conn.is_connected()))
        });
        if let Some((t, connected)) = terminal {
            let mut badge = crate::orch::terminal_badge(&t);
            if !connected {
                badge.stale_since = Some(crate::strings::CONNECTION_LOST.into());
            }
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(badge.render(mode()))
                .child(badge.detail())
                .child(crate::tr!(INSPECTOR_TERMINAL; ; t.id, t.program))
                .child(crate::tr!(INSPECTOR_UPDATED; ; freshness(t.status.as_ref())))
        } else {
            div().child(crate::strings::INSPECTOR_EMPTY)
        }
    }
}
impl Render for ChromePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = if self.inspector {
            self.inspector_view(cx)
        } else {
            self.settings_view(cx)
        };
        div()
            .id("chrome-panel")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .py_4()
            // Settings: 좌우24 (docs/10 §D4.6).
            .px(px(if self.inspector { 16. } else { 24. }))
            .bg(rgb(crate::theme::tokens().bg_surface))
            .text_color(rgb(crate::theme::tokens().text_primary))
            .child(content)
    }
}
