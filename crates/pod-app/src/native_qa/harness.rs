//! The GPUI driver every native-QA scenario shares (also used by the layout,
//! orch, PR and review QA modules through the `native_qa` re-exports):
//! isolated config, the headless window, keystroke and IME input, focus
//! naming, and bounds/dock observation.

use std::cell::RefCell;
use std::sync::Arc;

use ::gpui::{TestAppContext, TestDispatcher, VisualTestContext};
use gpui_kit::component::WindowExt;
use gpui_kit::component::input::AnyInputState;
use gpui_kit::*;

use crate::source::{PoddSource, TestBackend};
use crate::workspace::{StartOptions, Workspace};

thread_local! {
    /// DirectWrite, created once per test thread (a `WindowsPlatform` built
    /// only for its text system; it opens no visible window).
    static TEXT_SYSTEM: Arc<dyn PlatformTextSystem> = gpui_kit::platform::current_platform(false).text_system();
}

/// Isolated config dir for every test in this process (settings, layout).
fn isolate_config() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let dir = std::env::temp_dir().join(format!("pod-native-qa-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("config dir");
        // SAFETY: set once, before any test thread reads the variable.
        unsafe { std::env::set_var("POD_APP_CONFIG_DIR", &dir) };
    });
}

/// One IME message, as GPUI's Windows backend forwards it.
#[derive(Clone, Copy)]
pub(super) enum Ime<'a> {
    /// `WM_IME_COMPOSITION` + GCS_COMPSTR: the preedit (empty = cancelled).
    Compose(&'a str),
    /// `WM_IME_COMPOSITION` + GCS_RESULTSTR: committed text.
    Commit(&'a str),
}

pub(crate) struct Harness {
    /// Owns the app; quit on drop.
    app: TestAppContext,
    pub cx: VisualTestContext,
    pub ws: Entity<Workspace>,
    pub backend: TestBackend,
    name: &'static str,
}

impl Harness {
    /// 1440×960 logical at scale 1.0.
    pub fn new(name: &'static str, seed: impl FnOnce(&mut TestBackend)) -> Self {
        Self::sized(name, 1.0, size(px(1440.), px(960.)), seed)
    }

    /// A window of `logical` size at `scale`, Malgun Gothic, reduced motion,
    /// dark theme, connected to the backend populated by `seed`.
    pub fn sized(
        name: &'static str,
        scale: f32,
        logical: Size<Pixels>,
        seed: impl FnOnce(&mut TestBackend),
    ) -> Self {
        Self::build(name, scale, logical, StartOptions::default(), seed)
    }

    /// 1440×960 at scale 1.0 with explicit start options (e.g. a hook CLI).
    pub fn with_options(
        name: &'static str,
        options: StartOptions,
        seed: impl FnOnce(&mut TestBackend),
    ) -> Self {
        Self::build(name, 1.0, size(px(1440.), px(960.)), options, seed)
    }

    fn build(
        name: &'static str,
        scale: f32,
        logical: Size<Pixels>,
        options: StartOptions,
        seed: impl FnOnce(&mut TestBackend),
    ) -> Self {
        isolate_config();
        // Toolkit dialogs report their surface as `dialog-<layer>` (vendor patch).
        gpui_kit::component::dialog::set_dialog_bounds_observer(Some(|layer, bounds| {
            crate::qa_marks::record(format!("dialog-{layer}"), bounds)
        }));
        let dispatcher = TestDispatcher::new(0);
        let app = TestAppContext::build_with_text_system(
            dispatcher,
            Some("native_qa"),
            TEXT_SYSTEM.with(Arc::clone),
        );
        app.update(|cx| {
            gpui_kit::init(cx);
            crate::theme::Settings {
                theme: "dark".into(),
                font: "Malgun Gothic".into(),
                reduced_motion: true,
                ..Default::default()
            }
            .apply_with(false, cx);
            crate::install(cx);
        });
        let (source, events, mut backend) = PoddSource::scripted();
        seed(&mut backend);
        backend.connect();
        let slot = RefCell::new(None);
        let mut app = app;
        let handle = app.add_window(|window, cx| {
            let ws = cx.new(|cx| Workspace::new(source, events, options, window, cx));
            *slot.borrow_mut() = Some(ws.clone());
            gpui_kit::base::Root::new(ws, window, cx)
        });
        let ws = slot.into_inner().expect("workspace built");
        let cx = VisualTestContext::from_window(handle.into(), &app);
        cx.simulate_scale_factor_change(scale);
        cx.simulate_resize(logical);
        // The user's window is the active one while they type (GPUI reports
        // focus paths, e.g. terminal focus → terminal.ack, only for it).
        let mut cx = cx;
        cx.update(|window, _| window.activate_window());
        let mut h = Self {
            app,
            cx,
            ws,
            backend,
            name,
        };
        h.settle();
        h
    }

    /// Serve the backend, run the scheduler and draw until nothing changes.
    pub fn settle(&mut self) {
        for _ in 0..4 {
            self.cx.run_until_parked();
            let served = self.backend.serve();
            self.cx.run_until_parked();
            self.cx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
            self.cx.run_until_parked();
            if served == 0 && self.backend.serve() == 0 {
                break;
            }
        }
        // Full-refresh frame (no view cache): `qa_marks` holds exactly what it painted.
        crate::qa_marks::clear();
        self.cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        self.cx.run_until_parked();
        self.backend.serve();
    }

    pub(super) fn log(&mut self, step: &str) {
        let focus = self.focus_name();
        println!("[native-qa] {} | {step} | focus: {focus}", self.name);
    }

    /// Space-separated keystrokes (`ctrl-k`, `enter`, `shift-tab`, `escape`).
    pub fn keys(&mut self, keys: &str) {
        self.cx.simulate_keystrokes(keys);
        self.settle();
        self.log(&format!("keys {keys}"));
    }

    /// A full key press (down, then up): GPUI clicks a focused button on the
    /// key-up of Enter/Space, so button activation needs both.
    pub fn press(&mut self, key: &'static str) {
        let Ok(keystroke) = Keystroke::parse(key) else {
            panic!("bad key {key}")
        };
        self.cx.simulate_event(KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        self.cx.simulate_event(KeyUpEvent { keystroke });
        self.settle();
        self.log(&format!("press {key}"));
    }

    /// ASCII text typed key by key (each char is a key-down with its `key_char`).
    pub fn type_text(&mut self, text: &str) {
        self.cx.simulate_input(text);
        self.settle();
        self.log(&format!("type {text:?}"));
    }

    /// IME messages for the focused text control (input, textarea or terminal).
    pub(super) fn ime(&mut self, steps: &[Ime]) {
        for step in steps {
            let terminal = self.focused_terminal_view();
            let input = self.cx.update(|window, cx| window.focused_input(cx));
            self.cx.update(|window, cx| {
                let bounds = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
                match (terminal.clone(), input.clone()) {
                    (Some(view), _) => {
                        apply_ime(ElementInputHandler::new(bounds, view), *step, window, cx)
                    }
                    (None, Some(AnyInputState::Input(state))) => {
                        apply_ime(ElementInputHandler::new(bounds, state), *step, window, cx)
                    }
                    (None, Some(AnyInputState::Textarea(state))) => {
                        apply_ime(ElementInputHandler::new(bounds, state), *step, window, cx)
                    }
                    _ => panic!("IME message without a focused text control"),
                }
            });
            self.settle();
            let what = match step {
                Ime::Compose(t) => format!("IME compose {t:?}"),
                Ime::Commit(t) => format!("IME commit {t:?}"),
            };
            self.log(&what);
        }
    }

    /// Value and marked (preedit) text of the focused input/textarea.
    pub(super) fn input_state(&mut self) -> (String, Option<String>) {
        let input = self
            .cx
            .update(|window, cx| window.focused_input(cx))
            .expect("a focused input");
        self.cx.update(|window, cx| {
            let value = input.value(cx).to_string();
            let marked = match &input {
                AnyInputState::Input(s) => s.update(cx, |s, cx| marked_of(s, window, cx)),
                AnyInputState::Textarea(s) => s.update(cx, |s, cx| marked_of(s, window, cx)),
                _ => None,
            };
            (value, marked)
        })
    }

    pub fn dialog_open(&mut self) -> bool {
        self.cx.update(|window, cx| window.has_active_dialog(cx))
    }

    pub fn is_focused(&mut self, handle: &FocusHandle) -> bool {
        self.cx.update(|window, _| handle.is_focused(window))
    }

    pub fn focused(&mut self) -> Option<FocusHandle> {
        self.cx.update(|window, cx| window.focused(cx))
    }

    pub(super) fn focused_terminal_view(
        &mut self,
    ) -> Option<Entity<pod_terminal_view::TerminalView>> {
        let ws = self.ws.clone();
        self.cx.update(|window, cx| {
            ws.read(cx)
                .live_panels()
                .into_iter()
                .map(|p| p.read(cx).view.clone())
                .find(|v| v.read(cx).focus_handle(cx).is_focused(window))
        })
    }

    /// The terminal id whose view holds focus.
    pub fn focused_terminal(&mut self) -> Option<String> {
        let ws = self.ws.clone();
        self.cx.update(|window, cx| {
            ws.read(cx)
                .live_panels()
                .into_iter()
                .find(|p| p.read(cx).view.read(cx).focus_handle(cx).is_focused(window))
                .map(|p| p.read(cx).terminal_id.clone())
        })
    }

    pub fn sidebar_focus(&mut self) -> FocusHandle {
        let ws = self.ws.clone();
        self.cx.update(|_, cx| ws.read(cx).sidebar_focus_handle(cx))
    }

    pub fn root_focus(&mut self) -> FocusHandle {
        let ws = self.ws.clone();
        self.cx.update(|_, cx| ws.read(cx).root_focus_handle())
    }

    fn filter_focus(&mut self) -> FocusHandle {
        let ws = self.ws.clone();
        self.cx
            .update(|_, cx| ws.read(cx).sidebar_filter(cx).focus_handle(cx))
    }

    /// Human-readable focus target, from the handles the app owns.
    pub fn focus_name(&mut self) -> String {
        let Some(f) = self.focused() else {
            return "NONE".into();
        };
        if let Some(t) = self.focused_terminal() {
            return format!("terminal {t}");
        }
        if f == self.root_focus() {
            return "workspace root".into();
        }
        if f == self.filter_focus() {
            return "sidebar filter input".into();
        }
        if f == self.sidebar_focus() {
            return "sidebar tree".into();
        }
        let dialog = self.dialog_open();
        let input = self.cx.update(|window, cx| window.focused_input(cx));
        let ctx = self.context_path();
        let ws = self.ws.clone();
        let panel = self.cx.update(|window, cx| {
            let w = ws.read(cx);
            if w.settings_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_some_and(|p| p.focus_handle(cx).contains_focused(window, cx))
            {
                Some("settings panel")
            } else if w
                .inspector_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_some_and(|p| p.focus_handle(cx).contains_focused(window, cx))
            {
                Some("inspector panel")
            } else if w
                .live_reviews()
                .iter()
                .any(|r| r.read(cx).focus_handle().contains_focused(window, cx))
            {
                Some("review panel")
            } else if w
                .orch
                .tree_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_some_and(|p| p.focus_handle(cx).contains_focused(window, cx))
            {
                Some("agents panel")
            } else if w
                .orch
                .gates_panel
                .as_ref()
                .and_then(|p| p.upgrade())
                .is_some_and(|p| p.focus_handle(cx).contains_focused(window, cx))
            {
                Some("gates panel")
            } else {
                None
            }
        });
        let place = if dialog {
            "dialog"
        } else {
            panel.unwrap_or("workspace")
        };
        match input {
            Some(i) => {
                let (value, _) = self.cx.update(|_, cx| (i.value(cx).to_string(), ()));
                format!("{place} text input (value {value:?}) [{ctx}]")
            }
            None => format!("{place} control [{ctx}]"),
        }
    }

    /// The key-context path of the focused element, e.g. `Root > Dialog > Input`.
    pub fn context_path(&mut self) -> String {
        self.cx.update(|window, _| {
            window
                .context_stack()
                .iter()
                .filter_map(|c| c.primary().map(|e| e.key.to_string()))
                .collect::<Vec<_>>()
                .join(" > ")
        })
    }

    /// Bounds of the element marked `qa_id(selector)` in the last settled frame.
    /// (Not GPUI's `debug_bounds`: that is compiled out without `debug_assertions`,
    /// so it failed every lookup under `cargo test --release`.)
    pub fn bounds(&mut self, selector: &str) -> Option<Bounds<Pixels>> {
        crate::qa_marks::bounds(selector)
    }

    pub fn viewport(&mut self) -> Size<Pixels> {
        self.cx.update(|window, _| window.viewport_size())
    }

    pub(super) fn toast_titles(&mut self) -> Vec<String> {
        let ws = self.ws.clone();
        self.cx.update(|_, cx| {
            ws.read(cx)
                .toasts
                .iter()
                .map(|t| t.title.to_string())
                .collect()
        })
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.cx.run_until_parked();
        self.app.update(|cx| cx.quit());
        self.app.run_until_parked();
    }
}

fn apply_ime<V: EntityInputHandler>(
    mut handler: ElementInputHandler<V>,
    step: Ime,
    window: &mut Window,
    cx: &mut App,
) {
    match step {
        Ime::Compose(text) => {
            let n = text.encode_utf16().count();
            handler.replace_and_mark_text_in_range(
                None,
                text,
                (!text.is_empty()).then_some(n..n),
                window,
                cx,
            )
        }
        Ime::Commit(text) => handler.replace_text_in_range(None, text, window, cx),
    }
}

fn marked_of<V: EntityInputHandler>(
    s: &mut V,
    window: &mut Window,
    cx: &mut Context<V>,
) -> Option<String> {
    let range = s.marked_text_range(window, cx)?;
    let mut adjusted = None;
    s.text_for_range(range, &mut adjusted, window, cx)
}

/// Palette by keyboard: Ctrl+K, IME-commit `query`, `extra` keys, Enter.
pub(super) fn palette(h: &mut Harness, query: &str, extra: &str) {
    h.keys("ctrl-k");
    assert!(h.dialog_open(), "Ctrl+K opened the palette");
    h.ime(&[Ime::Commit(query)]);
    if !extra.is_empty() {
        h.keys(extra);
    }
    h.keys("enter");
}

/// Opens a terminal (or palette command without a dialog) by keyboard.
pub(super) fn jump_to(h: &mut Harness, query: &str) {
    palette(h, query, "");
    assert!(!h.dialog_open(), "Enter closed the palette");
}

// ---- bounds and dock observation ------------------------------------------

pub(super) fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let e = px(0.5);
    inner.origin.x >= outer.origin.x - e
        && inner.origin.y >= outer.origin.y - e
        && inner.right() <= outer.right() + e
        && inner.bottom() <= outer.bottom() + e
}

pub(super) fn center(b: Bounds<Pixels>) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width / 2.,
        b.origin.y + b.size.height / 2.,
    )
}

/// Center dock as `h[…]`/`v[…]` stacks of `tabs(…)` with `terminal:<id>` / `review:<task>` leaves.
pub(super) fn dock_shape(p: &gpui_kit::base::dock::PanelState) -> String {
    use gpui_kit::base::dock::PanelInfo;
    let kids = || {
        p.children
            .iter()
            .map(dock_shape)
            .collect::<Vec<_>>()
            .join(",")
    };
    match &p.info {
        PanelInfo::Stack { axis, .. } => {
            format!("{}[{}]", if *axis == 0 { "h" } else { "v" }, kids())
        }
        PanelInfo::Tabs { .. } => format!("tabs({})", kids()),
        PanelInfo::Panel(v) => match p.panel_name.as_str() {
            crate::panel::PANEL_NAME => {
                format!("terminal:{}", v["terminal"].as_str().unwrap_or("?"))
            }
            crate::review::REVIEW_PANEL_NAME => {
                format!("review:{}", v["task"].as_str().unwrap_or("?"))
            }
            other => other.to_string(),
        },
    }
}

pub(super) fn center_and_right(h: &mut Harness) -> (String, String) {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let d = ws.read(cx).dock.read(cx).dump(cx);
        let right = d
            .right_dock
            .map(|r| serde_json::to_string(&r).unwrap_or_default())
            .unwrap_or_default();
        (dock_shape(&d.center), right)
    })
}
