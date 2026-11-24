//! One browser tab in the editor dock. The native child follows the completed
//! GPUI frame, including cached views, tabs, splits, and blocking overlays.

use std::{cell::RefCell, rc::Rc};

use futures::{StreamExt, channel::mpsc};
use gpui_kit::base::Disableable;
use gpui_kit::base::dock::{Panel, PanelEvent};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{DockPlacement, PanelId, panel_handle};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable, WindowExt};
use gpui_kit::*;
use windows::Win32::Foundation::RECT;

use crate::workspace::Workspace;

mod native;

const PANEL_NAME: &str = "PodBrowser";
const TITLE: &str = "웹 브라우저";
const OPEN_ERROR: &str = "페이지를 열지 못했습니다. 주소와 개발 서버 상태를 확인하세요.";

pub(crate) struct BrowserPanel {
    workspace: WeakEntity<Workspace>,
    address: Entity<InputState>,
    last_address: String,
    pending: Option<String>,
    browser: Option<native::Browser>,
    starting: bool,
    status: SharedString,
    back: bool,
    forward: bool,
    surface: Rc<RefCell<Option<Hitbox>>>,
    events: mpsc::UnboundedSender<native::Event>,
    _receive: Task<()>,
    _subscriptions: [Subscription; 2],
}

/// Only web addresses are passed to the renderer, including redirects.
fn http_address(input: &str) -> Option<url::Url> {
    let input = input.trim();
    if input.chars().any(char::is_control) {
        return None;
    }
    let url = url::Url::parse(input).ok()?;
    (matches!(url.scheme(), "http" | "https") && url.host().is_some()).then_some(url)
}

fn viewport(bounds: Bounds<Pixels>, scale: f32) -> Option<RECT> {
    let rect = RECT {
        left: (f32::from(bounds.left()) * scale).ceil() as i32,
        top: (f32::from(bounds.top()) * scale).ceil() as i32,
        right: (f32::from(bounds.right()) * scale).floor() as i32,
        bottom: (f32::from(bounds.bottom()) * scale).floor() as i32,
    };
    (rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

impl BrowserPanel {
    fn new(workspace: WeakEntity<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| InputState::new(window, cx).placeholder("http://localhost:3000"));
        let enter = cx.subscribe_in(&address, window, |panel, address, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. })
                && !address.update(cx, |i, cx| i.marked_text_range(window, cx).is_some())
            {
                panel.open_address(window, cx);
            }
        });
        let moved = cx.observe_window_bounds(window, |panel, _, cx| {
            if let Some(browser) = &panel.browser {
                browser.parent_moved();
            }
            // Rebuild the surface hitbox at the new client size/DPI, even
            // when the active dock tab was otherwise eligible for caching.
            cx.notify();
        });
        let (events, mut rx) = mpsc::unbounded();
        let receive = cx.spawn_in(window, async move |this, cx| {
            while let Some(event) = rx.next().await {
                if this
                    .update_in(cx, |panel, window, cx| {
                        panel.on_browser_event(event, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            workspace,
            address,
            last_address: String::new(),
            pending: None,
            browser: None,
            starting: false,
            status: "웹 주소를 입력하고 Enter를 누르세요.".into(),
            back: false,
            forward: false,
            surface: Rc::default(),
            events,
            _receive: receive,
            _subscriptions: [enter, moved],
        }
    }

    fn display_address(&mut self, address: String, window: &mut Window, cx: &mut Context<Self>) {
        let can_replace = self.address.read(cx).value().as_ref() == self.last_address;
        if can_replace {
            self.address.update(cx, |input, cx| {
                input.set_value(address.as_str(), window, cx)
            });
        }
        self.last_address = address;
    }

    fn open_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.address.read(cx).value();
        let Some(url) = http_address(&input) else {
            self.status = "http:// 또는 https:// 주소를 입력하세요.".into();
            cx.notify();
            return;
        };
        let address: String = url.into();
        self.address.update(cx, |input, cx| {
            input.set_value(address.as_str(), window, cx)
        });
        self.last_address.clone_from(&address);
        if let Some(browser) = &self.browser {
            self.status = if browser.navigate(&address).is_ok() {
                "".into()
            } else {
                OPEN_ERROR.into()
            };
        } else {
            self.pending = Some(address);
            if !self.starting {
                self.starting = true;
                self.status = "WebView2 시작 중…".into();
                let profile = crate::layout::config_dir().join("browser-profile");
                if let Err(error) = native::start(window, &profile, &self.events) {
                    self.startup_failed(error.to_string());
                }
            }
        }
        cx.notify();
    }

    fn startup_failed(&mut self, detail: String) {
        self.starting = false;
        self.pending = None;
        self.status =
            format!("WebView2를 시작하지 못했습니다. Windows 런타임을 확인하세요. ({detail})")
                .into();
    }

    fn on_browser_event(
        &mut self,
        event: native::Event,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use native::Event;
        if self.events.is_closed() {
            return;
        }
        match event {
            Event::Ready(browser) => {
                self.starting = false;
                self.status = "".into();
                if let Some(address) = self.pending.take()
                    && browser.navigate(&address).is_err()
                {
                    self.status = OPEN_ERROR.into();
                }
                self.browser = Some(browser);
                self.sync_native_view(window, cx);
            }
            Event::StartupFailed(detail) => self.startup_failed(detail),
            Event::Address(address) => {
                if http_address(&address).is_some() {
                    self.display_address(address, window, cx);
                }
            }
            Event::History { back, forward } => {
                self.back = back;
                self.forward = forward;
            }
            Event::NavigationFailed => self.status = OPEN_ERROR.into(),
            Event::NavigationSucceeded => {
                if self.status.as_ref() == OPEN_ERROR {
                    self.status = "".into();
                }
            }
            Event::ReturnFocus => {
                if let Some(browser) = &self.browser {
                    browser.focus_parent();
                    self.focus_address(window, cx);
                }
            }
            Event::BrowserFailed => {
                self.browser = None;
                self.back = false;
                self.forward = false;
                self.status = "브라우저가 종료되었습니다. Enter로 다시 열 수 있습니다.".into();
            }
        }
        cx.notify();
    }

    fn sync_native_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.is_none() {
            return;
        }
        let rect = self.visible_viewport(window, cx);
        if let Some(browser) = &mut self.browser
            && browser.set_viewport(rect).is_err()
        {
            self.browser = None;
            self.status =
                "브라우저 영역을 표시하지 못했습니다. Enter로 다시 열 수 있습니다.".into();
            cx.notify();
        }
    }

    fn visible_viewport(&self, window: &mut Window, cx: &mut App) -> Option<RECT> {
        if window.has_active_dialog(cx) || window.has_active_sheet(cx) || cx.has_active_drag() {
            None
        } else {
            self.surface
                .borrow()
                .as_ref()
                .and_then(|hit| hit.native_child_bounds(window))
                .and_then(|bounds| viewport(bounds, window.scale_factor()))
        }
    }

    fn browser_action(
        &mut self,
        action: fn(&native::Browser) -> windows::core::Result<()>,
        cx: &mut Context<Self>,
    ) {
        if self
            .browser
            .as_ref()
            .is_some_and(|browser| action(browser).is_err())
        {
            self.status = OPEN_ERROR.into();
            cx.notify();
        }
    }
    fn focus_address(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle(cx), cx);
        self.address
            .update(cx, |input, cx| input.select_all(window, cx));
    }
}

impl EventEmitter<PanelEvent> for BrowserPanel {}

impl Focusable for BrowserPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.address.focus_handle(cx)
    }
}

impl Panel for BrowserPanel {
    fn panel_name(&self) -> &'static str {
        PANEL_NAME
    }

    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.events.close_channel();
        self.browser = None;
        let workspace = self.workspace.clone();
        let me = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = workspace.update(cx, |workspace, cx| {
                if workspace.browser_panel.as_ref() == Some(&me) {
                    workspace.browser_panel = None;
                    cx.notify();
                }
            });
        });
    }
}

impl gpui_kit::component::dock::Panel for BrowserPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(TITLE.into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        TITLE
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for BrowserPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = crate::theme::tokens();
        let surface = self.surface.clone();
        div()
            .id("browser-panel")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(tokens.bg_canvas))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .p_1()
                    .border_b_1()
                    .border_color(rgb(tokens.border_subtle))
                    .child(
                        Button::new("browser-back")
                            .small()
                            .ghost()
                            .label("뒤로")
                            .disabled(!self.back || self.browser.is_none())
                            .on_click(cx.listener(|p, _, _, cx| {
                                p.browser_action(native::Browser::back, cx)
                            })),
                    )
                    .child(
                        Button::new("browser-forward")
                            .small()
                            .ghost()
                            .label("앞으로")
                            .disabled(!self.forward || self.browser.is_none())
                            .on_click(cx.listener(|p, _, _, cx| {
                                p.browser_action(native::Browser::forward, cx)
                            })),
                    )
                    .child(
                        Button::new("browser-reload")
                            .small()
                            .ghost()
                            .label("새로고침")
                            .disabled(self.browser.is_none())
                            .on_click(cx.listener(|p, _, _, cx| {
                                p.browser_action(native::Browser::reload, cx)
                            })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.address).small().aria_label("웹 주소")),
                    )
                    .child(
                        Button::new("browser-go")
                            .small()
                            .label("이동")
                            .on_click(cx.listener(|p, _, window, cx| p.open_address(window, cx))),
                    )
                    .child(
                        Button::new("browser-page-focus")
                            .small()
                            .ghost()
                            .label("페이지 포커스")
                            .disabled(self.browser.is_none())
                            .on_click(cx.listener(|p, _, _, cx| {
                                p.browser_action(native::Browser::focus_page, cx)
                            })),
                    ),
            )
            .children((!self.status.is_empty()).then(|| {
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(tokens.text_muted))
                    .child(self.status.clone())
            }))
            .child(
                div().relative().flex_1().min_h_0().overflow_hidden().child(
                    canvas(
                        move |bounds, window, _| {
                            surface.replace(Some(
                                window.insert_hitbox(bounds, HitboxBehavior::Normal),
                            ));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .size_full(),
                ),
            )
    }
}

impl Workspace {
    pub(crate) fn open_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panel = match self.browser_panel.as_ref().and_then(WeakEntity::upgrade) {
            Some(panel) => {
                let id = PanelId::from(panel.entity_id());
                self.dock
                    .update(cx, |dock, cx| dock.select_panel(id, window, cx));
                panel
            }
            None => {
                let workspace = cx.weak_entity();
                let panel = cx.new(|cx| BrowserPanel::new(workspace, window, cx));
                self.browser_panel = Some(panel.downgrade());
                self.dock.update(cx, |dock, cx| {
                    dock.add_panel_view(
                        panel_handle(panel.clone()),
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    );
                });
                cx.notify();
                panel
            }
        };
        panel.update(cx, |panel, cx| panel.focus_address(window, cx));
    }

    /// The browser's hitbox may be replayed from the dock's view cache.
    /// Inspect the finished frame even when the panel itself did not render.
    pub(super) fn browser_sync(&self) -> Option<impl IntoElement> {
        let browser = self.browser_panel.clone()?;
        Some(
            canvas(
                |_, _, _| (),
                move |_, _, window, _| {
                    let browser = browser.clone();
                    window.on_next_frame(move |window, cx| {
                        let _ = browser.update(cx, |p, cx| p.sync_native_view(window, cx));
                    });
                },
            )
            .absolute()
            .size(px(0.)),
        )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::WindowExt;
    use gpui_kit::{Bounds, Entity, point, px, size};

    use super::{BrowserPanel, http_address, viewport};
    use crate::native_qa::{Harness, seed_two_tasks};

    fn open(h: &mut Harness) -> Entity<BrowserPanel> {
        let ws = h.ws.clone();
        h.cx.update(|window, cx| {
            ws.update(cx, |ws, cx| ws.command("open-browser", window, cx));
            ws.read(cx)
                .browser_panel
                .as_ref()
                .unwrap()
                .upgrade()
                .unwrap()
        })
    }

    #[test]
    fn palette_reuses_tab_and_surface_follows_dock_and_dialog() {
        let mut h = Harness::new("browser-dock", seed_two_tasks);
        let browser = open(&mut h);
        let name =
            h.cx.update(|_, cx| gpui_kit::component::dock::Panel::tab_name(browser.read(cx), cx));
        assert_eq!(name.as_deref(), Some("웹 브라우저"));
        let visible = |h: &mut Harness| {
            h.cx.update(|w, cx| {
                browser.update(cx, |panel, cx| panel.visible_viewport(w, cx).is_some())
            })
        };
        h.settle();
        assert_eq!(browser.entity_id(), open(&mut h).entity_id());
        h.settle();
        assert!(visible(&mut h));
        let width = |h: &mut Harness| {
            h.cx.update(|w, cx| {
                let rect = browser
                    .update(cx, |panel, cx| panel.visible_viewport(w, cx))
                    .unwrap();
                rect.right - rect.left
            })
        };
        let original = width(&mut h);
        h.cx.simulate_resize(size(px(960.), px(720.)));
        h.settle();
        assert!(width(&mut h) < original);
        let ws = h.ws.clone();
        h.cx.update(|w, cx| ws.update(cx, |ws, cx| ws.open_terminal("w1", false, w, cx)));
        h.settle();
        assert!(!visible(&mut h));
        assert_eq!(browser.entity_id(), open(&mut h).entity_id());
        h.settle();
        assert!(visible(&mut h));
        h.cx.update(|w, cx| w.open_dialog(cx, |dialog, _, _| dialog.title("Fixture dialog")));
        h.settle();
        assert!(!visible(&mut h));
        h.cx.update(|w, cx| w.close_dialog(cx));
        h.settle();
        assert!(visible(&mut h));
    }

    #[test]
    fn supports_http_loopback_and_https_and_rejects_other_schemes() {
        for address in [
            " http://localhost:3000 ",
            "http://[::1]:8080",
            "https://example.org/한글",
        ] {
            assert!(http_address(address).is_some(), "{address}");
        }
        for address in [
            "",
            "localhost:3000",
            "file:///C:/src/index.html",
            "javascript:alert(1)",
            "data:text/html,hello",
            "https://example.org/a\nb",
            "ms-settings:",
            "http://",
        ] {
            assert!(http_address(address).is_none(), "{address}");
        }
    }

    #[test]
    fn viewport_rounds_inward_at_fractional_dpi_and_skips_empty() {
        let bounds = Bounds::new(point(px(3.), px(9.)), size(px(100.), px(50.)));
        let rect = viewport(bounds, 1.25).unwrap();
        assert_eq!(
            (rect.left, rect.top, rect.right, rect.bottom),
            (4, 12, 128, 73)
        );
        assert!(viewport(Bounds::new(point(px(0.), px(0.)), size(px(0.), px(8.))), 1.).is_none());
    }
}
