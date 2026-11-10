//! Native-QA interaction tests (docs/10 §12, D3 checklist): the former MANUAL
//! rows, proven inside the app on GPUI's headless test platform.
//!
//! What this proves: the real `Workspace` view, the real key bindings
//! ([`crate::install`]), the real toolkit dialogs, inputs and focus handling,
//! and the real DirectWrite text system (layout bounds use real Malgun Gothic
//! metrics). Keystrokes go through GPUI's keystroke dispatch
//! (`Window::dispatch_keystroke`: key bindings, key-down handlers, then text
//! input). IME composition is driven through the focused control's own
//! `EntityInputHandler` — the calls GPUI's Windows backend makes for
//! `WM_IME_COMPOSITION` (GCS_COMPSTR → `replace_and_mark_text_in_range`,
//! GCS_RESULTSTR → `replace_text_in_range`). No mouse event is ever simulated.
//!
//! What it does not prove: the Win32 message pump (`WM_KEYDOWN` translation),
//! the Microsoft IME candidate window, Narrator speech or the compositor. The
//! data source is [`TestBackend`], an in-process podd double served on the
//! test thread; nothing is spawned or contacted.
//!
//! Every test prints `[native-qa]` step lines (`--nocapture`); the saved run
//! is the evidence in `crates/pod-app/qa/native/`.

use std::cell::RefCell;
use std::sync::Arc;

use ::gpui::{TestAppContext, TestDispatcher, VisualTestContext};
use gpui_kit::component::WindowExt;
use gpui_kit::component::input::AnyInputState;
use gpui_kit::*;
// GPUI's glob exports its own `test` attribute; these are plain Rust tests.
use core::prelude::v1::test;

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
enum Ime<'a> {
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

    fn log(&mut self, step: &str) {
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
    fn ime(&mut self, steps: &[Ime]) {
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
    fn input_state(&mut self) -> (String, Option<String>) {
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

    fn focused_terminal_view(&mut self) -> Option<Entity<pod_terminal_view::TerminalView>> {
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

    fn toast_titles(&mut self) -> Vec<String> {
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

/// One repo, a working terminal and a blocked one, each in its own task.
pub(crate) fn seed_two_tasks(b: &mut TestBackend) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    b.tasks.push(TestBackend::task(
        "sample/로그인-수정",
        "로그인-수정",
        "sample",
        &["w1"],
    ));
    b.tasks.push(TestBackend::task(
        "sample/review-bot",
        "review-bot",
        "sample",
        &["b1"],
    ));
    b.terminals.push(TestBackend::terminal(
        "w1",
        "로그인 작업",
        Some("sample/로그인-수정"),
        "working",
    ));
    b.terminals.push(TestBackend::terminal(
        "b1",
        "리뷰 봇",
        Some("sample/review-bot"),
        "blocked",
    ));
}

/// Palette by keyboard: Ctrl+K, IME-commit `query`, `extra` keys, Enter.
fn palette(h: &mut Harness, query: &str, extra: &str) {
    h.keys("ctrl-k");
    assert!(h.dialog_open(), "Ctrl+K opened the palette");
    h.ime(&[Ime::Commit(query)]);
    if !extra.is_empty() {
        h.keys(extra);
    }
    h.keys("enter");
}

/// Opens a terminal (or palette command without a dialog) by keyboard.
fn jump_to(h: &mut Harness, query: &str) {
    palette(h, query, "");
    assert!(!h.dialog_open(), "Enter closed the palette");
}

// ---- (a) keyboard-only flow -------------------------------------------------

#[test]
fn a_keyboard_only_repo_task_terminal_blocked_work_done() {
    let mut h = Harness::new("a-flow", |b| {
        // A blocked terminal already exists (from an earlier session); no repository is registered.
        b.tasks.push(TestBackend::task(
            "other/review-bot",
            "review-bot",
            "other",
            &["b1"],
        ));
        b.terminals.push(TestBackend::terminal(
            "b1",
            "리뷰 봇",
            Some("other/review-bot"),
            "blocked",
        ));
    });
    h.log("launch");
    // Launch: the workspace root holds focus, so shortcuts work without a click.
    let root = h.root_focus();
    assert!(h.is_focused(&root), "launch focus: {}", h.focus_name());

    // 1. Open repository: Ctrl+K → "저장소 열기" → Enter → path → Enter.
    palette(&mut h, "저장소 열기", "");
    assert!(h.dialog_open(), "repository dialog open");
    assert!(
        h.focus_name().starts_with("dialog text input"),
        "path field focused: {}",
        h.focus_name()
    );
    h.type_text("C:/fixtures/sample");
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c == "repo.add C:/fixtures/sample"),
        "Enter submitted the path: {:?}",
        h.backend.calls
    );
    assert!(!h.dialog_open(), "repository dialog closed after success");
    let after_repo = h.focus_name();
    assert!(
        h.is_focused(&root),
        "focus back on the workspace after the repository dialog: {after_repo}"
    );

    // 2. New task: Ctrl+N → name → Enter.
    h.keys("ctrl-n");
    assert!(h.dialog_open(), "new task dialog open");
    assert!(
        h.focus_name().starts_with("dialog text input"),
        "name field focused: {}",
        h.focus_name()
    );
    h.type_text("login-fix");
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("task.create sample/login-fix Shell")),
        "Enter created a shell task: {:?}",
        h.backend.calls
    );
    assert!(!h.dialog_open(), "dialog closed after creation");
    let term = h
        .backend
        .tasks
        .iter()
        .find(|t| t.id == "sample/login-fix")
        .map(|t| t.terminals[0].clone())
        .expect("task terminal");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some(term.as_str()),
        "the new task's terminal has focus"
    );

    // 3. Typing goes to that terminal.
    h.type_text("echo hi");
    h.keys("enter");
    assert_eq!(h.backend.typed_text(&term), "echo hi\r");

    // 4. Find the blocked request: Ctrl+Shift+B notifications → Tab → Enter.
    h.keys("ctrl-shift-b");
    assert!(h.dialog_open(), "notifications open");
    // Dialog tab order: the blocked notice, then the dialog's Close button.
    h.press("tab");
    let on_notice = h.focus_name();
    // Enter (not only Space) activates the focused notice button.
    h.press("enter");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "Enter on the blocked notice opened b1 (Tab landed on: {on_notice})"
    );
    assert!(!h.dialog_open());

    // 4b. The palette command reaches it too.
    jump_to(&mut h, "login-fix");
    assert_eq!(h.focused_terminal().as_deref(), Some(term.as_str()));
    jump_to(&mut h, "응답할 요청 찾기");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "palette → next blocked request"
    );

    // 5. Back to work.
    jump_to(&mut h, "login-fix");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some(term.as_str()),
        "back in the task terminal"
    );

    // 6. Done confirmation: b1 finishes while we work elsewhere → toast; jumping
    //    to it acknowledges (terminal.ack, done → idle).
    h.backend.acks.clear();
    h.backend.set_status("b1", "done");
    h.settle();
    assert!(
        h.toast_titles()
            .iter()
            .any(|t| t == "리뷰 봇 · 완료했습니다"),
        "done toast: {:?}",
        h.toast_titles()
    );
    assert!(
        !h.backend.acks.contains(&"b1".to_string()),
        "not acknowledged before the user looks"
    );
    jump_to(&mut h, "리뷰 봇");
    assert_eq!(h.focused_terminal().as_deref(), Some("b1"));
    assert!(
        h.backend.acks.contains(&"b1".to_string()),
        "focusing b1 sent terminal.ack: {:?}",
        h.backend.calls
    );
}

/// Before the root focus fix, nothing was focused at launch and GPUI
/// dispatched Ctrl+K to the window root only: the shortcut did nothing until
/// the user clicked. Also covers focus recovery after the focused panel closes.
#[test]
fn a_shortcuts_work_at_launch_and_after_the_focused_panel_closes() {
    let mut h = Harness::new("a-launch", seed_two_tasks);
    h.keys("ctrl-k");
    assert!(h.dialog_open(), "Ctrl+K at launch opens the palette");
    h.keys("escape");
    let root = h.root_focus();
    assert!(
        h.is_focused(&root),
        "Esc returns to the workspace root: {}",
        h.focus_name()
    );

    jump_to(&mut h, "로그인 작업");
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    // Close the focused panel the way the dock does (its close action).
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| w.close_panels_of("w1", window, cx));
    });
    h.settle();
    h.log("closed w1 panel");
    assert!(
        h.focused().is_some(),
        "focus is not lost when the focused panel closes"
    );
    h.keys("ctrl-n");
    assert!(
        h.dialog_open(),
        "Ctrl+N still works after the focused panel closed"
    );
}

/// Before the dialog patch, Enter anywhere in a Pod dialog ran the toolkit's
/// default Confirm (close): a focused button never got its keyboard click and
/// Enter in a field closed the dialog even when validation failed.
#[test]
fn a_enter_in_dialogs_goes_to_the_focused_control() {
    let mut h = Harness::new("a-dialog-enter", seed_two_tasks);
    h.keys("ctrl-n");
    h.press("enter");
    assert!(
        h.dialog_open(),
        "Enter with an empty name keeps the dialog open (validation error shown)"
    );
    assert!(!h.backend.calls.iter().any(|c| c.starts_with("task.create")));
    // Tab past the name/base fields to the agent choice buttons; Enter selects, never closes.
    let mut agent_buttons = 0;
    for _ in 0..8 {
        h.press("tab");
        if h.focus_name().starts_with("dialog control") {
            h.press("enter");
            assert!(
                h.dialog_open(),
                "Enter on a focused dialog button did not close the dialog"
            );
            agent_buttons += 1;
            if agent_buttons == 2 {
                break;
            }
        }
    }
    assert_eq!(agent_buttons, 2, "two option buttons reached by Tab");
    // Back to the name field (Shift+Tab past the base field), type, Enter.
    while !h.focus_name().starts_with("dialog text input") {
        h.press("shift-tab");
    }
    h.press("shift-tab");
    assert!(
        h.focus_name().starts_with("dialog text input"),
        "name field: {}",
        h.focus_name()
    );
    h.type_text("enter-fix");
    h.press("enter");
    let created = h
        .backend
        .calls
        .iter()
        .find(|c| c.starts_with("task.create"))
        .cloned();
    println!("[native-qa] a-dialog-enter | {created:?}");
    assert!(
        created.is_some(),
        "Enter in a field submits: {:?}",
        h.backend.calls
    );
}

// ---- (b) focus return ------------------------------------------------------

#[test]
fn b_escape_returns_focus_to_the_opener() {
    let mut h = Harness::new("b-focus-return", seed_two_tasks);
    jump_to(&mut h, "로그인 작업");
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));

    // Openers reachable by keyboard, each closed with Esc.
    // (name, shortcut, or palette query + extra keys)
    type Opener<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
    let openers: &[Opener] = &[
        ("palette (Ctrl+K)", "ctrl-k", None),
        ("new task (Ctrl+N)", "ctrl-n", None),
        ("new terminal (Ctrl+Shift+T)", "ctrl-shift-t", None),
        ("notifications (Ctrl+Shift+B)", "ctrl-shift-b", None),
        ("new comparison group (Ctrl+Shift+N)", "ctrl-shift-n", None),
        (
            "shortcut help (palette)",
            "",
            Some(("키보드 단축키 도움말", "")),
        ),
        ("open repository (palette)", "", Some(("저장소 열기", ""))),
        // Rows for "review-bot": terminal b1 (detail), then the task itself.
        ("task details (palette)", "", Some(("review-bot", "down"))),
    ];
    for (name, keys, query) in openers {
        match query {
            Some((q, extra)) => palette(&mut h, q, extra),
            None => h.keys(keys),
        }
        assert!(h.dialog_open(), "{name}: dialog open");
        let inside = h.focus_name();
        assert!(
            inside.starts_with("dialog"),
            "{name}: focus moved into the dialog, got {inside}"
        );
        h.keys("escape");
        assert!(!h.dialog_open(), "{name}: Esc closed it");
        assert_eq!(
            h.focused_terminal().as_deref(),
            Some("w1"),
            "{name}: focus returned to terminal w1, got {}",
            h.focus_name()
        );
    }
    // Typing after the last return reaches w1.
    h.backend.typed.clear();
    h.type_text("ok");
    assert_eq!(h.backend.typed_text("w1"), "ok");

    // From the sidebar tree as the opener.
    let sidebar = h.sidebar_focus();
    h.cx.update(|window, cx| sidebar.focus(window, cx));
    h.settle();
    h.log("sidebar focused");
    for keys in ["ctrl-n", "ctrl-k", "ctrl-shift-b"] {
        h.keys(keys);
        assert!(h.dialog_open());
        h.keys("escape");
        assert!(
            h.is_focused(&sidebar),
            "{keys} → Esc returns to the sidebar tree, got {}",
            h.focus_name()
        );
    }

    // Settings and the status inspector are dock tabs, not modals: Ctrl+, moves
    // focus into Settings; Ctrl+K → terminal is the keyboard way back.
    h.keys("ctrl-,");
    assert!(
        h.focus_name().starts_with("settings panel"),
        "Ctrl+, focuses Settings: {}",
        h.focus_name()
    );
    jump_to(&mut h, "로그인 작업");
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    h.keys("ctrl-shift-s");
    assert!(
        h.focus_name().starts_with("inspector panel"),
        "Ctrl+Shift+S focuses the inspector: {}",
        h.focus_name()
    );
}

// ---- (c) Tab / Shift+Tab traversal ----------------------------------------

/// Tab from the current focus until it cycles; Shift+Tab must retrace the
/// exact sequence in reverse. Every Tab moves focus; none loses it.
fn traverse(h: &mut Harness, label: &str, max: usize) -> Vec<String> {
    let (stops, mismatches) = traverse_report(h, label, max);
    assert!(
        mismatches.is_empty(),
        "{label}: Shift+Tab is not the reverse of Tab: {mismatches:?}"
    );
    stops
}

/// [`traverse`] that reports Shift+Tab mismatches instead of failing.
fn traverse_report(h: &mut Harness, label: &str, max: usize) -> (Vec<String>, Vec<String>) {
    let start = h.focused().expect("a focused start");
    let mut forward = Vec::new();
    for i in 0..max {
        h.cx.simulate_keystrokes("tab");
        h.settle();
        let f = h
            .focused()
            .unwrap_or_else(|| panic!("{label}: Tab {i} lost focus"));
        let name = h.focus_name();
        println!("[native-qa] {label} | tab {i} → {name}");
        if forward
            .last()
            .is_some_and(|(prev, _): &(FocusHandle, String)| *prev == f)
        {
            panic!("{label}: Tab {i} did not move focus ({name})");
        }
        if forward
            .first()
            .is_some_and(|(first, _): &(FocusHandle, String)| *first == f)
        {
            break;
        }
        forward.push((f, name));
    }
    assert!(
        forward.len() >= 3,
        "{label}: at least three tab stops, got {forward:?}",
        forward = forward.iter().map(|x| &x.1).collect::<Vec<_>>()
    );
    // Shift+Tab walks back: from stop[0] to stop[n-1], …, stop[1], stop[0].
    let n = forward.len();
    let mut mismatches = Vec::new();
    for i in 0..n {
        h.cx.simulate_keystrokes("shift-tab");
        h.settle();
        let expected = &forward[(2 * n - 1 - i) % n];
        let got = h.focused();
        if got.as_ref() != Some(&expected.0) {
            let m = format!(
                "Shift+Tab {i}: expected {}, got {}",
                expected.1,
                h.focus_name()
            );
            println!("[native-qa] {label} | MISMATCH {m}");
            mismatches.push(m);
            // Resynchronise on the expected stop and continue.
            let f = expected.0.clone();
            h.cx.update(|window, cx| f.focus(window, cx));
            h.settle();
        }
    }
    let _ = start;
    println!(
        "[native-qa] {label} | {n} tab stops, Shift+Tab mismatches: {}",
        mismatches.len()
    );
    (forward.into_iter().map(|x| x.1).collect(), mismatches)
}

#[test]
fn c_tab_order_cycles_and_reverses_on_main_screens() {
    let mut h = Harness::new("c-tab", seed_two_tasks);
    // Workspace without panels: title bar, sidebar, filter, status bar.
    let stops = traverse(&mut h, "workspace", 60);
    assert!(
        stops.iter().any(|s| s == "sidebar tree"),
        "sidebar tree is a tab stop: {stops:?}"
    );
    assert!(
        stops.iter().any(|s| s.starts_with("sidebar filter")),
        "sidebar filter is a tab stop: {stops:?}"
    );
    let tree = stops.iter().position(|s| s == "sidebar tree").unwrap();
    let filter = stops
        .iter()
        .position(|s| s.starts_with("sidebar filter"))
        .unwrap();
    assert!(
        tree < filter,
        "tree before its filter (visual order): {stops:?}"
    );

    // Settings open (dock tab with category/value choices).
    h.keys("ctrl-,");
    // 모양 (docs/10 §D5.3) adds ~50 choices (18 preset cards, styles, accents, layouts).
    let settings = traverse(&mut h, "settings", 200);
    assert!(
        settings
            .iter()
            .filter(|s| s.starts_with("settings panel"))
            .count()
            >= 3,
        "settings choices are tab stops: {settings:?}"
    );

    // Agents tree (Ctrl+Shift+E) is a tab stop with arrow-key navigation inside.
    h.keys("ctrl-shift-e");
    assert!(
        h.focus_name().starts_with("agents panel"),
        "Ctrl+Shift+E focuses the agent tree: {}",
        h.focus_name()
    );
    let agents = traverse(&mut h, "agents", 200);
    assert!(
        agents.iter().any(|s| s.starts_with("agents panel")),
        "agent tree is a tab stop: {agents:?}"
    );
    h.keys("ctrl-shift-i");
    assert!(
        h.focus_name().starts_with("gates panel"),
        "Ctrl+Shift+I focuses the approvals inbox: {}",
        h.focus_name()
    );

    // Inside a dialog Tab stays in the dialog (focus trap) and Esc still closes it.
    h.keys("ctrl-n");
    let dialog = traverse(&mut h, "new-task dialog", 40);
    assert!(
        dialog.iter().all(|s| s.starts_with("dialog")),
        "focus trapped in the dialog: {dialog:?}"
    );
    assert!(
        dialog
            .iter()
            .filter(|s| s.starts_with("dialog text input"))
            .count()
            >= 2,
        "name and base fields: {dialog:?}"
    );
    h.keys("escape");
    assert!(!h.dialog_open());
}

/// The dock's tab group tracks the active panel's focus handle on its frame,
/// before its tab bar. When that handle was also the agent tree's tab stop it
/// was registered twice: Tab from Settings skipped the agents header straight
/// into the tree (first copy) while Shift+Tab from the tree left from the
/// second copy to the header menu, so Shift+Tab did not retrace Tab.
#[test]
fn c_shift_tab_from_the_agents_tree_reverses_tab() {
    let mut h = Harness::new("c-agents-shift-tab", seed_two_tasks);
    h.keys("ctrl-,");
    h.keys("ctrl-shift-e");
    let tree = h.focused().expect("agents tree focused");
    assert!(
        h.focus_name().starts_with("agents panel"),
        "{}",
        h.focus_name()
    );
    // The stop visually before the tree is its own panel header's menu button.
    h.keys("shift-tab");
    assert_eq!(
        h.focus_name(),
        "agents panel control [Root > Popover]",
        "Shift+Tab from the tree goes to its header menu"
    );
    h.keys("tab");
    assert_eq!(
        h.focused().as_ref(),
        Some(&tree),
        "Tab from the header menu returns to the tree, got {}",
        h.focus_name()
    );
    // Walking back past the header into Settings and forward again retraces every stop.
    let mut back = vec![tree.clone()];
    for _ in 0..6 {
        h.keys("shift-tab");
        back.push(h.focused().expect("a stop"));
    }
    for i in (0..6).rev() {
        h.keys("tab");
        assert_eq!(
            h.focused().as_ref(),
            Some(&back[i]),
            "Tab retraces Shift+Tab step {i}, got {}",
            h.focus_name()
        );
    }
    assert_eq!(h.focused().as_ref(), Some(&tree));
}

#[test]
fn c_review_controls_are_reachable_by_keyboard() {
    let wt = std::env::temp_dir().join(format!("pod-native-qa-review-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&wt);
    let wt = pod_editor::fixture::create(&wt, true).expect("fixture repo");
    let mut h = Harness::new("c-review", |b| {
        seed_two_tasks(b);
        b.tasks[0].worktree = Some(wt.to_string_lossy().into_owned());
        b.worktrees.insert("sample/로그인-수정".into(), wt.clone());
    });
    jump_to(&mut h, "로그인 작업");
    h.keys("ctrl-shift-r");
    assert!(
        h.focus_name().starts_with("review panel"),
        "Ctrl+Shift+R focuses Review: {}",
        h.focus_name()
    );
    assert!(
        h.backend.calls.iter().any(|c| c.starts_with("task.diff")),
        "diff requested: {:?}",
        h.backend.calls
    );
    let stops = traverse(&mut h, "review", 200);
    assert!(
        stops
            .iter()
            .any(|s| s.starts_with("review panel text input")),
        "agent instructions box reachable: {stops:?}"
    );
    // Keyboard comment: Alt+PageDown (file) then Ctrl+Shift+C opens a draft on its first change.
    h.keys("alt-pagedown");
    h.keys("ctrl-shift-c");
    assert!(
        h.focus_name().starts_with("review panel text input"),
        "comment draft focused: {}",
        h.focus_name()
    );
}

// ---- (d) Korean IME composition --------------------------------------------

#[test]
fn d_ime_terminal_sends_bytes_only_on_commit() {
    let mut h = Harness::new("d-ime-terminal", seed_two_tasks);
    jump_to(&mut h, "로그인 작업");
    h.backend.typed.clear();
    h.ime(&[Ime::Compose("ㅎ"), Ime::Compose("하"), Ime::Compose("한")]);
    assert_eq!(h.backend.typed_text("w1"), "", "no bytes while composing");
    let view = h.focused_terminal_view().expect("terminal view");
    let marked =
        h.cx.update(|_, cx| view.read(cx).marked_text().map(str::to_owned));
    assert_eq!(marked.as_deref(), Some("한"), "preedit kept for drawing");
    // Candidate window anchor = the cursor cell (preedit drawn there).
    let anchor = h.cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.bounds_for_range(0..1, Bounds::default(), window, cx)
        })
    });
    let anchor = anchor.expect("IME anchor");
    println!("[native-qa] d-ime-terminal | IME anchor {anchor:?}");
    assert!(
        anchor.size.width > px(0.) && anchor.size.height > px(0.),
        "anchor is a cell"
    );
    h.ime(&[
        Ime::Commit("한"),
        Ime::Compose("ㄱ"),
        Ime::Compose("그"),
        Ime::Compose("글"),
    ]);
    assert_eq!(
        h.backend.typed_text("w1"),
        "한",
        "first syllable sent on commit only"
    );
    h.ime(&[Ime::Commit("글")]);
    assert_eq!(h.backend.typed_text("w1"), "한글");
    // Esc while composing belongs to the IME: no ESC byte; cancel clears the preedit.
    h.ime(&[Ime::Compose("ㅁ")]);
    h.keys("escape");
    h.ime(&[Ime::Compose("")]);
    let marked =
        h.cx.update(|_, cx| view.read(cx).marked_text().map(str::to_owned));
    assert_eq!(marked, None, "cancel cleared the preedit");
    assert_eq!(
        h.backend.typed_text("w1"),
        "한글",
        "no ESC and no cancelled syllable sent"
    );
}

#[test]
fn d_ime_palette_sidebar_filter_new_task_name() {
    let mut h = Harness::new("d-ime-inputs", seed_two_tasks);
    // Palette: Enter while composing must not activate a half-typed query.
    h.keys("ctrl-k");
    h.ime(&[Ime::Compose("ㄹ"), Ime::Compose("리")]);
    assert_eq!(
        h.input_state(),
        (String::from("리"), Some(String::from("리")))
    );
    h.keys("enter");
    assert!(
        h.dialog_open(),
        "Enter during composition does not activate"
    );
    h.ime(&[
        Ime::Commit("리"),
        Ime::Compose("뷰"),
        Ime::Commit("뷰"),
        Ime::Commit(" 봇"),
    ]);
    assert_eq!(h.input_state(), (String::from("리뷰 봇"), None));
    // Cancel a composition: value unchanged.
    h.ime(&[Ime::Compose("ㅋ"), Ime::Compose("")]);
    assert_eq!(h.input_state(), (String::from("리뷰 봇"), None));
    h.keys("enter");
    assert_eq!(
        h.focused_terminal().as_deref(),
        Some("b1"),
        "committed Korean query opened b1"
    );

    // Sidebar filter: Tab from the tree to its filter, compose, commit, filter applies.
    let sidebar = h.sidebar_focus();
    h.cx.update(|window, cx| sidebar.focus(window, cx));
    h.settle();
    // Visual order: tree, header buttons (grid / task / new, orchestration), then the filter.
    let mut tabs = 0;
    while !h.focus_name().starts_with("sidebar filter") {
        tabs += 1;
        assert!(
            tabs <= 8,
            "Tab reaches the filter within the sidebar header: {}",
            h.focus_name()
        );
        h.keys("tab");
    }
    assert!(
        h.bounds("row-b1").is_some(),
        "b1 row painted before filtering"
    );
    h.ime(&[Ime::Compose("로"), Ime::Compose("로그")]);
    assert_eq!(
        h.input_state(),
        (String::from("로그"), Some(String::from("로그")))
    );
    h.ime(&[Ime::Commit("로그"), Ime::Commit("인")]);
    assert_eq!(h.input_state(), (String::from("로그인"), None));
    assert!(h.bounds("row-w1").is_some(), "matching row stays");
    assert!(
        h.bounds("row-b1").is_none(),
        "non-matching row filtered out"
    );

    // New task name: compose, cancel (Esc in the IME), compose again, commit, Enter.
    h.keys("ctrl-n");
    h.ime(&[Ime::Commit("로그인"), Ime::Compose("ㅅ"), Ime::Compose("")]);
    assert_eq!(
        h.input_state(),
        (String::from("로그인"), None),
        "cancelled syllable dropped"
    );
    h.ime(&[Ime::Compose("수"), Ime::Compose("수정")]);
    assert!(
        !h.backend.calls.iter().any(|c| c.starts_with("task.create")),
        "no create while composing: {:?}",
        h.backend.calls
    );
    // Korean IME order: commit, then the Enter key arrives.
    h.ime(&[Ime::Commit("수정")]);
    h.keys("enter");
    assert!(
        h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("task.create sample/로그인수정")),
        "created with the full name: {:?}",
        h.backend.calls
    );
}

#[test]
fn d_ime_review_comment_editor() {
    let wt = std::env::temp_dir().join(format!("pod-native-qa-ime-review-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&wt);
    let wt = pod_editor::fixture::create(&wt, true).expect("fixture repo");
    let mut h = Harness::new("d-ime-review", |b| {
        seed_two_tasks(b);
        b.tasks[0].worktree = Some(wt.to_string_lossy().into_owned());
        b.worktrees.insert("sample/로그인-수정".into(), wt.clone());
    });
    jump_to(&mut h, "로그인 작업");
    h.keys("ctrl-shift-r");
    h.keys("ctrl-shift-c");
    assert!(
        h.focus_name().starts_with("review panel text input"),
        "draft focused: {}",
        h.focus_name()
    );
    h.ime(&[
        Ime::Compose("좋"),
        Ime::Commit("좋"),
        Ime::Compose("아"),
        Ime::Commit("아"),
        Ime::Compose("요"),
    ]);
    assert_eq!(
        h.input_state(),
        (String::from("좋아요"), Some(String::from("요")))
    );
    assert!(
        !h.backend
            .calls
            .iter()
            .any(|c| c.starts_with("review.comments.add")),
        "nothing saved while composing"
    );
    // Microsoft Korean IME: Enter first commits the syllable (GCS_RESULTSTR),
    // then VK_RETURN reaches the app.
    h.ime(&[Ime::Commit("요")]);
    assert_eq!(h.input_state(), (String::from("좋아요"), None));
    h.keys("enter");
    let adds: Vec<_> = h
        .backend
        .calls
        .iter()
        .filter(|c| c.starts_with("review.comments.add"))
        .cloned()
        .collect();
    assert_eq!(adds.len(), 1, "saved once: {adds:?}");
    assert!(
        adds[0].contains("\"body\":\"좋아요\""),
        "saved with the full text: {adds:?}"
    );
}

// ---- (e) DPI ----------------------------------------------------------------

/// The logical viewport a maximized window gets on a 1920×1080 monitor at
/// each Windows scale (the smallest common desktop; worst case for overflow).
const SCALES: [f32; 4] = [1.0, 1.25, 1.5, 1.75];

fn within(inner: Bounds<Pixels>, outer: Bounds<Pixels>) -> bool {
    let e = px(0.5);
    inner.origin.x >= outer.origin.x - e
        && inner.origin.y >= outer.origin.y - e
        && inner.right() <= outer.right() + e
        && inner.bottom() <= outer.bottom() + e
}

#[test]
fn e_dpi_scales_keep_chrome_rows_dialogs_inside_the_window() {
    for scale in SCALES {
        let logical = size(
            px((1920. / scale).floor()),
            px((1080. / scale).floor() - 48. / scale),
        );
        let mut h = Harness::sized("e-dpi", scale, logical, seed_two_tasks);
        let scale_seen = h.cx.update(|window, _| window.scale_factor());
        assert_eq!(scale_seen, scale);
        let view = Bounds::new(point(px(0.), px(0.)), h.viewport());
        println!("[native-qa] e-dpi | scale {scale} viewport {:?}", view.size);
        let check = |h: &mut Harness, sel: &str| -> Bounds<Pixels> {
            let b = h
                .bounds(sel)
                .unwrap_or_else(|| panic!("scale {scale}: {sel} not painted"));
            println!("[native-qa] e-dpi | scale {scale} {sel} {b:?}");
            assert!(
                within(b, view),
                "scale {scale}: {sel} {b:?} outside viewport {view:?}"
            );
            b
        };
        let title = check(&mut h, "title-actions");
        assert!(
            title.size.height <= px(36.),
            "title actions fit the 36px title bar"
        );
        let sidebar = check(&mut h, "sidebar");
        let actions = check(&mut h, "sidebar-actions");
        assert!(
            within(actions, sidebar),
            "scale {scale}: sidebar buttons {actions:?} clipped by sidebar {sidebar:?}"
        );
        for row in ["row-w1", "row-b1"] {
            let r = check(&mut h, row);
            assert!(
                r.right() <= sidebar.right() + px(0.5),
                "scale {scale}: {row} wider than the sidebar"
            );
            // Rows keep the active style's row height (docs/10 §D5: 28 in the default 04 조용한 편집기).
            let row_h = pod_ui::themes::metrics().density.row;
            assert!(
                r.size.height >= px(row_h - 0.5),
                "scale {scale}: {row} keeps its {row_h}px height"
            );
        }
        let bar = check(&mut h, "status-bar");
        assert!(
            (bar.bottom() - view.bottom()).abs() < px(1.),
            "status bar at the bottom edge"
        );
        for item in ["status-inspect", "status-notifications"] {
            let b = check(&mut h, item);
            assert!(
                within(b, bar),
                "scale {scale}: {item} {b:?} clipped by status bar {bar:?}"
            );
        }
        for (name, keys) in [
            ("palette", "ctrl-k"),
            ("new task", "ctrl-n"),
            ("new terminal", "ctrl-shift-t"),
            ("notifications", "ctrl-shift-b"),
            ("new group", "ctrl-shift-n"),
        ] {
            h.keys(keys);
            let d = check(&mut h, "dialog-0");
            println!("[native-qa] e-dpi | scale {scale} dialog {name} {d:?}");
            h.keys("escape");
        }
    }
}

// ---- (f) follow-ups: terminal links, Ctrl+P passthrough, hook prompt, LSP menu ----

fn center(b: Bounds<Pixels>) -> Point<Pixels> {
    point(
        b.origin.x + b.size.width / 2.,
        b.origin.y + b.size.height / 2.,
    )
}

/// A task whose worktree (and terminal cwd) is a real temporary folder.
fn seed_link_task(b: &mut TestBackend, wt: &str) {
    b.repos.push(crate::vm::RepoVm {
        name: "sample".into(),
        path: "C:/fixtures/sample".into(),
        default_base: Some("main".into()),
    });
    let mut t = TestBackend::task("sample/links", "links", "sample", &["L1"]);
    t.worktree = Some(wt.to_string());
    t.worktree_exists = true;
    b.tasks.push(t);
    let mut term = TestBackend::terminal("L1", "링크 터미널", Some("sample/links"), "working");
    term.cwd = wt.to_string();
    b.terminals.push(term);
}

fn link_worktree() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().expect("temp dir");
    let wt = dir.path().join("wt");
    std::fs::create_dir_all(wt.join("docs")).expect("worktree");
    std::fs::write(
        wt.join("docs").join("notes.txt"),
        "첫 줄\n둘째 줄 abc\n셋째 줄\n",
    )
    .expect("file");
    let s = wt.to_string_lossy().replace('\\', "/");
    (dir, s)
}

fn terminal_view(h: &mut Harness, id: &str) -> Entity<pod_terminal_view::TerminalView> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_panels()
            .into_iter()
            .find(|p| p.read(cx).terminal_id == id)
            .map(|p| p.read(cx).view.clone())
    })
    .expect("terminal panel")
}

/// Editor of `task`: (active path, 0-based cursor line).
fn editor_cursor(h: &mut Harness, task: &str) -> Option<(String, u32)> {
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let e = ws
            .read(cx)
            .live_editors()
            .into_iter()
            .find(|e| e.read(cx).task_id == task)?;
        let ed = e.read(cx).editor.read(cx);
        let path = ed.active_path()?.to_string_lossy().replace('\\', "/");
        let line = ed.active_editor()?.read(cx).cursor_position().line;
        Some((path, line))
    })
}

#[test]
fn f_terminal_links_keyboard_picker_and_ctrl_click() {
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("f-links", |b| seed_link_task(b, &wt));
    jump_to(&mut h, "링크 터미널");
    assert_eq!(h.focused_terminal().as_deref(), Some("L1"));
    let view = terminal_view(&mut h, "L1");
    h.cx.update(|_, cx| {
        view.update(cx, |v, cx| v.feed("error: docs/notes.txt:2:4 참고, https://example.com/docs.\r\nmissing.rs:9 은 없는 파일\r\n".as_bytes(), cx));
    });
    h.settle();
    let links = h.cx.update(|_, cx| view.read(cx).visible_links());
    let labels: Vec<String> = links.iter().map(|l| l.target.label()).collect();
    h.log(&format!("visible links {labels:?}"));
    assert_eq!(
        labels,
        vec![
            "docs/notes.txt:2:4".to_string(),
            "https://example.com/docs".into()
        ],
        "missing.rs is not a file: not offered"
    );

    // Keyboard: palette, 터미널에서 링크 열기, Enter on the first link.
    palette(&mut h, "터미널에서 링크 열기", "");
    assert!(h.dialog_open(), "the link picker is open");
    h.keys("enter");
    assert!(!h.dialog_open());
    let (path, line) = editor_cursor(&mut h, "sample/links").expect("the task editor opened");
    assert!(path.ends_with("docs/notes.txt"), "{path}");
    assert_eq!(line, 1, "cursor on line 2 (0-based 1)");

    // Mouse: Ctrl+hover underlines, Ctrl+click opens the URL; a plain click does not.
    // (Ctrl+K inside the editor is its chord prefix, so return to the terminal directly.)
    let ws = h.ws.clone();
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.open_terminal("L1", false, window, cx)));
    h.settle();
    let url = links[1].clone();
    let cell =
        h.cx.update(|_, cx| view.read(cx).cell_bounds(url.start.col + 3, 0))
            .expect("painted");
    let at = center(cell);
    h.cx.simulate_mouse_move(at, None, Modifiers::none());
    h.settle();
    assert!(
        h.cx.update(|_, cx| view.read(cx).hovered_link().is_none()),
        "no underline without Ctrl"
    );
    h.cx.simulate_modifiers_change(Modifiers::control());
    h.settle();
    let by_modifiers =
        h.cx.update(|_, cx| view.read(cx).hovered_link().map(|l| l.target.label()));
    assert_eq!(
        by_modifiers.as_deref(),
        Some("https://example.com/docs"),
        "pressing Ctrl over the link underlines it"
    );
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.settle();
    assert!(
        h.cx.update(|_, cx| view.read(cx).hovered_link().is_none()),
        "releasing Ctrl removes the underline"
    );
    h.cx.simulate_mouse_move(at, None, Modifiers::control());
    h.settle();
    let hovered =
        h.cx.update(|_, cx| view.read(cx).hovered_link().map(|l| l.target.label()));
    assert_eq!(
        hovered.as_deref(),
        Some("https://example.com/docs"),
        "moving with Ctrl held underlines too"
    );
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.cx.simulate_click(at, Modifiers::none());
    h.settle();
    crate::term_links::OPENED_URLS
        .with(|o| assert!(o.borrow().is_empty(), "plain click opened nothing"));
    h.cx.simulate_click(at, Modifiers::control());
    h.settle();
    h.log("ctrl+click on the URL");
    crate::term_links::OPENED_URLS.with(|o| {
        assert_eq!(
            o.borrow().as_slice(),
            ["https://example.com/docs".to_string()]
        )
    });
    assert!(
        h.backend.typed_text("L1").is_empty(),
        "link clicks never reach the program"
    );
}

#[test]
fn f_palette_sends_ctrl_p_and_ctrl_k_to_the_current_terminal() {
    let mut h = Harness::new("f-passthrough", seed_two_tasks);
    jump_to(&mut h, "로그인 작업");
    palette(&mut h, "현재 터미널에 Ctrl+P 보내기", "");
    palette(&mut h, "현재 터미널에 Ctrl+K 보내기", "");
    assert_eq!(
        h.backend.typed_text("w1"),
        "\u{10}\u{b}",
        "^P then ^K reached the terminal"
    );
}

#[test]
fn f_hook_prompt_installs_only_after_the_user_confirms() {
    use crate::hooks_prompt::{HookCliHandle, MockHookCli};
    let cli = Arc::new(MockHookCli::missing());
    let options = StartOptions {
        hook_cli: Some(HookCliHandle(cli.clone())),
        ..Default::default()
    };
    let mut h = Harness::with_options("f-hooks", options, seed_two_tasks);
    assert!(
        h.bounds("hooks-banner").is_some(),
        "banner shown for missing hooks"
    );
    assert!(
        cli.installs.lock().unwrap().is_empty(),
        "nothing runs at launch but status"
    );
    // The banner does not take focus (non-blocking).
    let root = h.root_focus();
    assert!(
        h.is_focused(&root),
        "launch focus stays on the workspace: {}",
        h.focus_name()
    );

    // 훅 설치: dry run, preview; 취소 installs nothing.
    let install = h.bounds("hooks-install").expect("install button");
    h.cx.simulate_click(center(install), Modifiers::none());
    h.settle();
    assert!(h.dialog_open(), "preview dialog");
    assert_eq!(
        *cli.installs.lock().unwrap(),
        vec![true],
        "only a dry run so far"
    );
    let cancel = h.bounds("hooks-cancel").expect("cancel");
    h.cx.simulate_click(center(cancel), Modifiers::none());
    h.settle();
    assert!(!h.dialog_open());
    assert_eq!(
        *cli.installs.lock().unwrap(),
        vec![true],
        "cancel: no install"
    );
    assert!(
        h.bounds("hooks-banner").is_some(),
        "banner stays until installed or dismissed"
    );

    // Keyboard path: palette command, preview, 설치.
    palette(&mut h, "에이전트 훅 설치", "");
    assert!(h.dialog_open());
    let confirm = h.bounds("hooks-confirm").expect("confirm");
    h.cx.simulate_click(center(confirm), Modifiers::none());
    h.settle();
    h.log("confirmed install");
    assert_eq!(
        *cli.installs.lock().unwrap(),
        vec![true, true, false],
        "the real install ran once, after confirmation"
    );
    assert!(
        h.bounds("hooks-banner").is_none(),
        "banner gone after install"
    );
    assert!(
        h.toast_titles()
            .iter()
            .any(|t| t == crate::strings::HOOKS_INSTALLED),
        "{:?}",
        h.toast_titles()
    );
}

#[test]
fn f_hook_prompt_later_is_remembered_per_build() {
    use crate::hooks_prompt::{HookCliHandle, MockHookCli};
    let cli = Arc::new(MockHookCli::missing());
    let options = StartOptions {
        hook_cli: Some(HookCliHandle(cli.clone())),
        ..Default::default()
    };
    let mut h = Harness::with_options("f-hooks-later", options, seed_two_tasks);
    let later = h.bounds("hooks-later").expect("later button");
    h.cx.simulate_click(center(later), Modifiers::none());
    h.settle();
    assert!(h.bounds("hooks-banner").is_none());
    let saved = h.cx.update(|_, cx| {
        cx.global::<crate::theme::Settings>()
            .hooks_dismissed
            .clone()
    });
    assert_eq!(saved, crate::hooks_prompt::dismiss_key("fixture-hook-1"));
    assert!(cli.installs.lock().unwrap().is_empty());
    // Same build again: no banner. A new pod-hook build asks again.
    let ws = h.ws.clone();
    let again: Arc<dyn crate::hooks_prompt::HookCli> = cli.clone();
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.start_hook_check(again, cx)));
    h.settle();
    assert!(
        h.bounds("hooks-banner").is_none(),
        "dismissed for this build"
    );
    let newer: Arc<dyn crate::hooks_prompt::HookCli> = Arc::new(MockHookCli {
        key: "fixture-hook-2".into(),
        ..MockHookCli::missing()
    });
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.start_hook_check(newer, cx)));
    h.settle();
    assert!(
        h.bounds("hooks-banner").is_some(),
        "a new pod-hook build asks again"
    );
}

#[test]
fn f_status_bar_language_server_menu_off_restart_log() {
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("f-lsp-menu", |b| seed_link_task(b, &wt));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("docs/notes.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    assert!(
        h.bounds("status-lsp").is_none(),
        "no server for .txt: no status item"
    );
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.lsp_turn_off(cx)));
    h.settle();
    assert!(
        h.bounds("status-lsp").is_some(),
        "the off state stays visible so it can be turned back on"
    );
    let enabled = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            ws.read(cx)
                .lsp_target(cx)
                .map(|(s, _)| s.read(cx).is_enabled())
        })
    };
    assert_eq!(enabled(&mut h), Some(false));
    h.cx.update(|window, cx| ws.update(cx, |w, cx| w.lsp_show_log(window, cx)));
    h.settle();
    assert!(h.dialog_open(), "log dialog");
    h.keys("escape");
    h.cx.update(|_, cx| ws.update(cx, |w, cx| w.lsp_restart(cx)));
    h.settle();
    assert_eq!(enabled(&mut h), Some(true), "restart turns it back on");
}

// ---- (g) protocol v9: task.touch on save, task.removing ----

fn open_notes(h: &mut Harness) -> Entity<pod_editor::EditorWorkspace> {
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("docs/notes.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    h.cx.update(|_, cx| {
        ws.read(cx)
            .live_editors()
            .into_iter()
            .find(|e| e.read(cx).task_id == "sample/links")
            .map(|e| e.read(cx).editor.clone())
    })
    .expect("editor")
}

fn type_in_editor(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>, text: &'static str) {
    h.cx.update(|window, cx| {
        if let Some(st) = ed.read(cx).active_editor() {
            st.update(cx, |s, cx| s.insert(text, window, cx));
        }
    });
    h.settle();
}

#[test]
fn g_saving_in_the_editor_touches_the_task() {
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("g-touch", |b| seed_link_task(b, &wt));
    let ed = open_notes(&mut h);
    type_in_editor(&mut h, &ed, "추가 ");
    h.cx.update(|window, cx| ed.update(cx, |e, cx| e.save_all(window, cx)));
    h.settle();
    let touch = h
        .backend
        .calls
        .iter()
        .find(|c| c.starts_with("task.touch"))
        .cloned();
    h.log(&format!("{touch:?}"));
    let touch = touch.expect("task.touch sent after save");
    assert!(
        touch.contains("\"task\":\"sample/links\"") && touch.contains("docs/notes.txt"),
        "{touch}"
    );
}

#[test]
fn g_task_removing_saves_unsaved_edits_or_closes_a_clean_editor() {
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("g-removing", |b| seed_link_task(b, &wt));
    let ed = open_notes(&mut h);
    type_in_editor(&mut h, &ed, "에이전트가 지우기 전 ");
    assert_eq!(h.cx.update(|_, cx| ed.read(cx).dirty_paths().len()), 1);
    // `pod task remove` from a CLI: podd announces the removal first.
    let _ = h
        .backend
        .events
        .unbounded_send(crate::source::SourceEvent::TaskRemoving {
            id: "sample/links".into(),
            worktree: wt.clone(),
        });
    h.settle();
    assert_eq!(
        h.cx.update(|_, cx| ed.read(cx).dirty_paths().len()),
        0,
        "unsaved edits were saved (podd then stops with TASK_DIRTY)"
    );
    let saved =
        std::fs::read_to_string(std::path::Path::new(&wt).join("docs/notes.txt")).expect("file");
    assert!(saved.contains("에이전트가 지우기 전"), "{saved}");
    assert!(
        h.toast_titles()
            .iter()
            .any(|t| t == crate::strings::TASK_REMOVING_SAVED),
        "{:?}",
        h.toast_titles()
    );
    let ws = h.ws.clone();
    let open = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .iter()
                .any(|e| e.read(cx).task_id == "sample/links")
        })
    };
    assert!(open(&mut h), "editor kept while the removal stops");
    // Clean now: a second announcement closes the editor (language servers stop).
    let _ = h
        .backend
        .events
        .unbounded_send(crate::source::SourceEvent::TaskRemoving {
            id: "sample/links".into(),
            worktree: wt.clone(),
        });
    h.settle();
    assert!(
        !open(&mut h),
        "clean editor closed before the worktree goes"
    );
}

/// A snippet completion with an auto-import, inserted the way the
/// completion menu does (`insert_completion`): the import lands above, the
/// first placeholder is selected, Tab / Shift+Tab walk the stops, typing
/// replaces a placeholder, and Esc ends the session (Tab indents again).
#[test]
fn g_snippet_completion_tab_stops_and_auto_import() {
    use pod_editor::lsp::types as lt;
    let (_dir, wt) = link_worktree();
    let mut h = Harness::new("g-snippet", |b| seed_link_task(b, &wt));
    let ed = open_notes(&mut h);
    let item = lt::CompletionItem {
        label: "pair".into(),
        insert_text: Some("pair(${1:left}, ${2:right})$0".into()),
        insert_text_format: Some(lt::InsertTextFormat::SNIPPET),
        additional_text_edits: Some(vec![lt::TextEdit {
            range: lt::Range::default(),
            new_text: "use pairs::pair;\n".into(),
        }]),
        ..Default::default()
    };
    let extra = pod_editor::lsp::store::completion_extra(&item, false).expect("extra");
    let mut shown = item.clone();
    pod_editor::lsp::store::expand_snippet_item(&mut shown);
    let state =
        h.cx.update(|_, cx| ed.read(cx).active_editor())
            .expect("editor state");
    h.cx.update(|window, cx| {
        let path = ed.read(cx).active_path().expect("path").to_path_buf();
        let lsp = ed.read(cx).lsp().clone();
        lsp.update(cx, |s, _| s.note_completion(&path, 0, vec![extra]));
        state.update(cx, |s, cx| s.insert_completion(&shown, 0..0, window, cx));
    });
    h.settle();
    let read = |h: &mut Harness| {
        h.cx.update(|_, cx| {
            let s = state.read(cx);
            let text = s.value().to_string();
            let sel = s.selected_range();
            (
                text.clone(),
                text[sel].to_string(),
                ed.read(cx).snippet_active(),
            )
        })
    };
    let (text, selected, active) = read(&mut h);
    h.log(&format!("after insert: selected {selected:?}"));
    assert!(
        text.starts_with("use pairs::pair;\npair(left, right)"),
        "{text:?}"
    );
    assert_eq!(
        (selected.as_str(), active),
        ("left", true),
        "first placeholder selected"
    );
    // Focus the code editor so keys go through its bindings (Tab = IndentInline).
    h.cx.update(|window, cx| state.update(cx, |s, cx| s.focus(window, cx)));
    h.keys("tab");
    assert_eq!(read(&mut h).1, "right");
    h.keys("shift-tab");
    assert_eq!(read(&mut h).1, "left");
    h.type_text("a");
    let (text, _, active) = read(&mut h);
    assert!(
        text.starts_with("use pairs::pair;\npair(a, right)"),
        "typing replaced the placeholder: {text:?}"
    );
    assert!(active);
    h.keys("tab");
    assert_eq!(read(&mut h).1, "right");
    h.keys("escape");
    assert!(!read(&mut h).2, "Esc ends the session");
    let before = read(&mut h).0;
    h.keys("tab");
    assert_ne!(
        read(&mut h).0,
        before,
        "Tab edits (indents) again after the session"
    );
}

// ---- (h) editor tabs: preview, MRU, close prompt, reopen, restore (docs/13 §9) ----

fn tabs_worktree() -> (tempfile::TempDir, String) {
    let (dir, wt) = link_worktree();
    for n in ["alpha", "beta", "gamma", "delta"] {
        std::fs::write(
            std::path::Path::new(&wt).join(format!("{n}.txt")),
            format!("{n} 첫 줄\n"),
        )
        .expect("file");
    }
    (dir, wt)
}

/// Tabs as `name`, `name~` (preview), `name*` (unsaved); the active one in [ ].
fn tab_strip(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>) -> String {
    h.cx.update(|_, cx| {
        ed.read(cx)
            .tab_states()
            .iter()
            .map(|t| {
                let mut s = t
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .trim_end_matches(".txt")
                    .to_string();
                if t.preview {
                    s.push('~');
                }
                if t.dirty {
                    s.push('*');
                }
                if t.active { format!("[{s}]") } else { s }
            })
            .collect::<Vec<_>>()
            .join(" ")
    })
}

/// Whether keyboard focus is in the active tab's code editor.
fn editor_focused(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>) -> bool {
    h.cx.update(|window, cx| {
        ed.read(cx)
            .active_editor()
            .is_some_and(|s| s.focus_handle(cx).is_focused(window))
    })
}

/// Quick open by keyboard: Ctrl+P, the name, Enter.
fn quick_open(h: &mut Harness, name: &str) {
    h.keys("ctrl-p");
    h.type_text(name);
    h.keys("enter");
}

#[test]
fn h_editor_tabs_keyboard_preview_mru_close_reopen() {
    let (_dir, wt) = tabs_worktree();
    let mut h = Harness::new("h-tabs", |b| seed_link_task(b, &wt));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("alpha.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    let ed =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|e| e.read(cx).editor.clone())
        })
        .expect("editor");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "[alpha]",
        "an explicit open is a pinned tab"
    );
    assert!(editor_focused(&mut h, &ed));

    // Quick open makes a preview tab; the next one replaces it in place.
    quick_open(&mut h, "beta.txt");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [beta~]");
    quick_open(&mut h, "gamma.txt");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [gamma~]",
        "preview replaced, not added"
    );
    assert!(editor_focused(&mut h, &ed), "focus follows the opened file");
    // Typing pins it.
    h.type_text("x");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [gamma*]");
    quick_open(&mut h, "delta.txt");
    assert_eq!(tab_strip(&mut h, &ed), "alpha gamma* [delta~]");

    // Ctrl+Tab: previous file; release Ctrl and Ctrl+Tab again toggles back.
    h.keys("ctrl-tab");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [gamma*] delta~");
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.settle();
    h.keys("ctrl-tab");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha gamma* [delta~]",
        "Ctrl+Tab, release, Ctrl+Tab toggles"
    );
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.settle();
    // Held Ctrl: Tab Tab walks further back (delta → gamma → alpha).
    h.keys("ctrl-tab ctrl-tab");
    assert_eq!(tab_strip(&mut h, &ed), "[alpha] gamma* delta~");
    h.keys("ctrl-shift-tab");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [gamma*] delta~",
        "Ctrl+Shift+Tab steps back while held"
    );
    h.cx.simulate_modifiers_change(Modifiers::none());
    h.settle();
    assert!(editor_focused(&mut h, &ed));

    // Ctrl+PageDown / Ctrl+PageUp: by position, wrapping.
    h.keys("ctrl-pagedown");
    assert_eq!(tab_strip(&mut h, &ed), "alpha gamma* [delta~]");
    h.keys("ctrl-pagedown");
    assert_eq!(tab_strip(&mut h, &ed), "[alpha] gamma* delta~");
    h.keys("ctrl-pageup");
    assert_eq!(tab_strip(&mut h, &ed), "alpha gamma* [delta~]");
    h.keys("ctrl-pageup");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [gamma*] delta~");

    // Ctrl+W on an unsaved tab asks; Esc cancels and focus returns to the editor.
    h.keys("ctrl-w");
    assert!(
        h.cx.update(|_, cx| ed.read(cx).close_prompt_open()),
        "save/discard/cancel prompt"
    );
    h.keys("escape");
    assert!(!h.cx.update(|_, cx| ed.read(cx).close_prompt_open()));
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [gamma*] delta~",
        "cancel keeps the tab"
    );
    assert!(
        editor_focused(&mut h, &ed),
        "focus back in the editor after cancel: {}",
        h.focus_name()
    );
    // Ctrl+W, Enter: save and close; focus moves to the most recent other tab.
    h.keys("ctrl-w");
    h.keys("enter");
    let saved =
        std::fs::read_to_string(std::path::Path::new(&wt).join("gamma.txt")).expect("gamma");
    assert!(saved.starts_with('x'), "saved before closing: {saved:?}");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [delta~]",
        "most recently used tab (delta) is active"
    );
    assert!(
        editor_focused(&mut h, &ed),
        "focus in the next tab's editor: {}",
        h.focus_name()
    );

    // Ctrl+Shift+T reopens gamma where it was (pinned), even though the app
    // binds Ctrl+Shift+T to "new terminal" outside the editor.
    h.keys("ctrl-shift-t");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [gamma] delta~");
    assert!(editor_focused(&mut h, &ed));

    // Restart: the panel's saved state restores order, preview, active tab and MRU.
    let state =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|p| gpui_kit::base::dock::Panel::dump(p.read(cx), cx))
        })
        .expect("panel");
    let session =
        crate::editor_panel::EditorPanel::session_of_state(&state).expect("session saved");
    h.log(&format!(
        "saved session {}",
        serde_json::to_string(&session).unwrap()
    ));
    assert_eq!(
        session
            .tabs
            .iter()
            .map(|t| (t.path.as_str(), t.preview))
            .collect::<Vec<_>>(),
        [
            ("alpha.txt", false),
            ("gamma.txt", false),
            ("delta.txt", true)
        ]
    );
    let weak = ws.downgrade();
    let root = std::path::PathBuf::from(&wt);
    let restored = h.cx.update(|window, cx| {
        cx.new(|cx| {
            crate::editor_panel::EditorPanel::new(
                "sample/links".into(),
                root,
                Some(&session),
                weak,
                window,
                cx,
            )
        })
    });
    h.settle();
    let red = h.cx.update(|_, cx| restored.read(cx).editor.clone());
    assert_eq!(tab_strip(&mut h, &red), "alpha [gamma] delta~");
    h.cx.update(|window, cx| red.update(cx, |e, cx| e.switch_mru(true, window, cx)));
    h.settle();
    assert_eq!(
        tab_strip(&mut h, &red),
        "alpha gamma [delta~]",
        "Ctrl+Tab order survived the restart"
    );

    // Ctrl+W on clean tabs closes at once; the last one returns focus to the tree.
    // (The restored copy above took focus; give it back to the live editor.)
    drop(restored);
    h.cx.update(|window, cx| {
        if let Some(st) = ed.read(cx).active_editor() {
            st.update(cx, |s, cx| s.focus(window, cx));
        }
    });
    h.settle();
    h.keys("ctrl-w ctrl-w ctrl-w");
    assert_eq!(tab_strip(&mut h, &ed), "");
    let tree_focused = h.cx.update(|window, cx| {
        ed.read(cx).focus_handle(cx).contains_focused(window, cx)
            && ed.read(cx).active_editor().is_none()
    });
    assert!(
        tree_focused,
        "focus stays in the files panel (tree): {}",
        h.focus_name()
    );
    assert_eq!(
        h.cx.update(|_, cx| ed.read(cx).closed_paths().len()),
        3,
        "all three can be reopened"
    );
}

/// The tree's delete prompt answers Esc and Enter from the keyboard (it
/// takes focus while shown; before, the keys went nowhere).
#[test]
fn h_tree_delete_prompt_esc_cancels_enter_deletes() {
    let (_dir, wt) = tabs_worktree();
    let mut h = Harness::new("h-delete", |b| seed_link_task(b, &wt));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("alpha.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    let ed =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|e| e.read(cx).editor.clone())
        })
        .expect("editor");
    let alpha = std::path::Path::new(&wt).join("alpha.txt");
    // Alt+1: tree; Home is docs/, Down is alpha.txt.
    h.keys("alt-1 home down");
    h.keys("delete");
    assert!(
        h.cx.update(|_, cx| ed.read(cx).has_overlay()),
        "delete prompt shown"
    );
    h.keys("escape");
    assert!(
        !h.cx.update(|_, cx| ed.read(cx).has_overlay()),
        "Esc closed the prompt"
    );
    assert!(alpha.exists(), "Esc kept the file");
    h.keys("alt-1");
    h.keys("delete");
    h.keys("enter");
    assert!(!alpha.exists(), "Enter deleted the file");
    assert_eq!(tab_strip(&mut h, &ed), "", "its tab closed");
    assert!(
        h.cx.update(|_, cx| ed.read(cx).closed_paths().is_empty()),
        "a deleted file is not offered by Ctrl+Shift+T"
    );
}

// ---- (h) extensions: layout presets chosen from the palette (docs/16 §7.2) ----

/// The example `pod-layouts` presets as the daemon lists them, plus a mod
/// whose preset file failed validation (as `mod.list` reports it).
fn mods_backend(b: &mut TestBackend) {
    seed_two_tasks(b);
    b.terminals.push(TestBackend::terminal(
        "w2",
        "로그인 테스트",
        Some("sample/로그인-수정"),
        "idle",
    ));
    b.tasks[0].terminals.push("w2".into());
    let preset = |f: &str| {
        let text = std::fs::read_to_string(format!(
            "{}/../../examples/mods/pod-layouts/layouts/{f}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("example preset");
        serde_json::to_value(podd::protocol::LayoutPreset::parse(&text).expect("valid preset"))
            .expect("json")
    };
    let list = serde_json::json!({
        "enabled": true, "root": "C:/fixtures/mods", "dropped_events": 0,
        "mods": [
            {"name": "pod-layouts", "version": "0.1.0", "title": "기본 레이아웃 프리셋", "enabled": true, "state": "running",
             "permissions": [], "layouts": [preset("review-focus.json"), preset("agents-quad.json")], "log_lines": 1, "memory_kib": 0},
            {"name": "broken-layout", "version": "0.1.0", "enabled": true, "state": "invalid",
             "error": "layout layouts/bad.json: a split needs 2-6 children", "permissions": [], "log_lines": 0, "memory_kib": 0}
        ]
    });
    b.rpc = Some(Box::new(move |method, _| {
        (method == "mod.list").then(|| Ok(list.clone()))
    }));
}

/// Center dock as `h[…]`/`v[…]` stacks of `tabs(…)` with `terminal:<id>` / `review:<task>` leaves.
fn dock_shape(p: &gpui_kit::base::dock::PanelState) -> String {
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

fn center_and_right(h: &mut Harness) -> (String, String) {
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

#[test]
fn h_layout_preset_from_the_palette_rearranges_and_survives_restart() {
    let dir = tempfile::tempdir().expect("config dir");
    crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
    {
        let mut h = Harness::new("h-layout-preset", mods_backend);
        jump_to(&mut h, "로그인 테스트");
        jump_to(&mut h, "로그인 작업");
        assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
        jump_to(&mut h, "리뷰 집중");
        assert_eq!(
            h.focused_terminal().as_deref(),
            Some("w1"),
            "focus stays in the user's terminal"
        );
        let (center, right) = center_and_right(&mut h);
        h.log(&format!("center after 리뷰 집중: {center}"));
        // Review left; the task's first terminal right; its second (not in
        // the preset) joins the terminal slot as a tab; gates go to the right dock.
        assert_eq!(
            center, "h[tabs(review:sample/로그인-수정),tabs(terminal:w1,terminal:w2)]",
            "preset 리뷰 집중 as declared"
        );
        assert!(
            right.contains(crate::gates::GATES_PANEL_NAME),
            "gates in the right dock: {right}"
        );
        // (The scripted backend serves no review comments; that toast is not about layouts.)
        let layout_errors: Vec<String> = h
            .toast_titles()
            .into_iter()
            .filter(|t| t.starts_with("레이아웃") || t.starts_with("적용할"))
            .collect();
        assert!(
            layout_errors.is_empty(),
            "no layout error: {layout_errors:?}"
        );

        jump_to(&mut h, "4분할 에이전트");
        let (center, _) = center_and_right(&mut h);
        h.log(&format!("center after 4분할: {center}"));
        assert_eq!(
            center, "h[tabs(terminal:w1,review:sample/로그인-수정),tabs(terminal:w2)]",
            "two terminals: one row; the review panel stays (as a tab), not closed"
        );

        jump_to(&mut h, "리뷰 집중");
        // The real quit path persists the layout (main.rs on_app_quit).
        let ws = h.ws.clone();
        h.cx.update(|_, cx| ws.update(cx, |w, cx| w.on_quit(cx)))
            .detach();
        h.settle();
    }
    assert!(
        dir.path().join("layout.json").is_file(),
        "layout saved on quit"
    );
    {
        let mut h = Harness::new("h-layout-restart", mods_backend);
        let (center, right) = center_and_right(&mut h);
        h.log(&format!("center after restart: {center}"));
        assert_eq!(
            center, "h[tabs(review:sample/로그인-수정),tabs(terminal:w1,terminal:w2)]",
            "restart restores the preset layout"
        );
        assert!(right.contains(crate::gates::GATES_PANEL_NAME), "{right}");
    }
    crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = None);
}

#[test]
fn h_missing_and_invalid_presets_explain_themselves_in_korean() {
    let dir = tempfile::tempdir().expect("config dir");
    crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = Some(dir.path().to_path_buf()));
    let mut h = Harness::new("h-layout-errors", mods_backend);
    // A palette entry whose preset vanished (mod turned off meanwhile).
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            assert!(crate::mods_ui::run(
                w,
                "layout:pod-layouts:gone",
                window,
                cx
            ))
        })
    });
    h.settle();
    let titles = h.toast_titles();
    assert!(
        titles
            .iter()
            .any(|t| t == crate::mods_ui::s::LAYOUT_MISSING),
        "{titles:?}"
    );
    // A mod whose preset file is invalid: Settings → 확장 says so in Korean.
    let line = h.cx.update(|_, cx| {
        let list = cx
            .global::<crate::mods_ui::ModsUi>()
            .list
            .clone()
            .expect("mod.list answered");
        let m = list
            .list
            .mods
            .iter()
            .find(|m| m.name == "broken-layout")
            .cloned()
            .expect("broken mod listed");
        crate::mods_ui::error_summary(&m).expect("error line")
    });
    h.log(&format!("invalid preset card: {line}"));
    assert!(
        line.starts_with("레이아웃 프리셋 오류: layout layouts/bad.json"),
        "{line}"
    );
    // Its presets never reach the palette.
    let entries = h.cx.update(|_, cx| crate::mods_ui::palette_entries(cx));
    assert!(
        entries
            .iter()
            .all(|(id, _, _)| !id.starts_with("layout:broken-layout")),
        "{entries:?}"
    );
    assert_eq!(
        entries
            .iter()
            .filter(|(id, _, _)| id.starts_with("layout:pod-layouts:"))
            .count(),
        2
    );
    crate::layout::TEST_CONFIG_DIR.with(|d| *d.borrow_mut() = None);
}

/// Selected item of the open tab menu.
fn menu_selected(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>) -> Option<String> {
    h.cx.update(|_, cx| {
        ed.read(cx)
            .tab_menu_items()
            .into_iter()
            .find(|i| i.selected)
            .map(|i| i.label)
    })
}

/// Walks the open tab menu with Down until `label` is selected, then Enter.
fn menu_pick(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>, label: &str) {
    for _ in 0..12 {
        if menu_selected(h, ed).as_deref() == Some(label) {
            h.keys("enter");
            return;
        }
        h.keys("down");
    }
    panic!(
        "menu item {label:?} not reachable; items {:?}",
        h.cx.update(|_, cx| ed.read(cx).tab_menu_items())
    );
}

fn menu_focused(h: &mut Harness, ed: &Entity<pod_editor::EditorWorkspace>) -> bool {
    h.cx.update(|_, cx| ed.read(cx).tab_menu_open())
}

/// Tab menu and tab moving by keyboard only (docs/13 §9): Ctrl+Shift+
/// PageUp/PageDown move the active tab (a preview is pinned, the order is
/// saved), Shift+F10 / the menu key open the tab menu, arrows skip
/// unavailable items, Esc returns focus, bulk closes ask once for all
/// unsaved files, and the path / tree commands work.
#[test]
fn h_editor_tab_menu_and_move_keyboard() {
    let (_dir, wt) = tabs_worktree();
    let mut h = Harness::new("h-tab-menu", |b| seed_link_task(b, &wt));
    let ws = h.ws.clone();
    h.cx.update(|window, cx| {
        ws.update(cx, |w, cx| {
            w.open_editor(
                "sample/links",
                Some(("alpha.txt".into(), 1, 1)),
                false,
                window,
                cx,
            )
        })
    });
    h.settle();
    let ed =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|e| e.read(cx).editor.clone())
        })
        .expect("editor");
    // The editor holds the canonical root; on CI the temp dir is spelled with
    // 8.3 short names (RUNNER~1), so compare against the same spelling.
    let root = pod_editor::uri::simplify(&std::fs::canonicalize(&wt).expect("root"));
    for n in ["beta", "gamma"] {
        let p = root.join(format!("{n}.txt"));
        h.cx.update(|window, cx| ed.update(cx, |e, cx| e.open_file(&p, window, cx)))
            .expect("open");
    }
    quick_open(&mut h, "delta.txt");
    assert_eq!(tab_strip(&mut h, &ed), "alpha beta gamma [delta~]");

    // Ctrl+Shift+PageUp moves the active tab left (and pins the preview).
    h.keys("ctrl-shift-pageup");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha beta [delta] gamma",
        "moved left and pinned"
    );
    h.keys("ctrl-shift-pageup ctrl-shift-pageup ctrl-shift-pageup");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "[delta] alpha beta gamma",
        "stops at the first place (no wrap)"
    );
    h.keys("ctrl-shift-pagedown");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [delta] beta gamma");
    assert!(
        editor_focused(&mut h, &ed),
        "moving keeps focus in the editor"
    );
    // The order is what the layout saves.
    let state =
        h.cx.update(|_, cx| {
            ws.read(cx)
                .live_editors()
                .into_iter()
                .find(|e| e.read(cx).task_id == "sample/links")
                .map(|p| gpui_kit::base::dock::Panel::dump(p.read(cx), cx))
        })
        .expect("panel");
    let session = crate::editor_panel::EditorPanel::session_of_state(&state).expect("session");
    assert_eq!(
        session
            .tabs
            .iter()
            .map(|t| t.path.as_str())
            .collect::<Vec<_>>(),
        ["alpha.txt", "delta.txt", "beta.txt", "gamma.txt"]
    );

    // Shift+F10: the active tab's menu takes focus; Esc gives it back.
    h.keys("shift-f10");
    assert!(menu_focused(&mut h, &ed), "menu open");
    let items = h.cx.update(|_, cx| ed.read(cx).tab_menu_items());
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    h.log(&format!("menu {labels:?}"));
    assert_eq!(
        labels,
        [
            "닫기",
            "다른 탭 닫기",
            "오른쪽 탭 닫기",
            "저장된 탭 닫기",
            "모두 닫기",
            "고정 해제",
            "경로 복사",
            "상대 경로 복사",
            "파일 트리에서 보기"
        ]
    );
    assert_eq!(menu_selected(&mut h, &ed).as_deref(), Some("닫기"));
    assert!(
        h.cx.update(|window, cx| ed.read(cx).tab_menu_has_focus(window)),
        "focus on the menu: {}",
        h.focus_name()
    );
    h.keys("up");
    assert_eq!(
        menu_selected(&mut h, &ed).as_deref(),
        Some("파일 트리에서 보기"),
        "Up wraps to the last item"
    );
    h.keys("home");
    assert_eq!(menu_selected(&mut h, &ed).as_deref(), Some("닫기"));
    h.keys("escape");
    assert!(!menu_focused(&mut h, &ed));
    assert!(
        editor_focused(&mut h, &ed),
        "Esc returns focus to the editor: {}",
        h.focus_name()
    );

    // Copy path / relative path (menu key opens the menu too).
    h.keys("menu");
    menu_pick(&mut h, &ed, "상대 경로 복사");
    assert_eq!(
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .as_deref(),
        Some("delta.txt")
    );
    assert!(editor_focused(&mut h, &ed), "focus back after a command");
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "경로 복사");
    let copied =
        h.cx.update(|_, cx| cx.read_from_clipboard().and_then(|c| c.text()))
            .unwrap_or_default();
    // The editor may hold the canonical spelling of the root (8.3 short names
    // such as RUNNER~1 resolve to the long name), so compare resolved paths.
    assert_eq!(
        std::fs::canonicalize(&copied).ok(),
        std::fs::canonicalize(root.join("delta.txt")).ok(),
        "copied {copied}"
    );

    // Reveal in the file tree: the tree gets focus with the file selected.
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "파일 트리에서 보기");
    let (tree_sel, in_tree) = h.cx.update(|window, cx| {
        let e = ed.read(cx);
        (
            e.tree().selected().map(|p| p.to_path_buf()),
            e.focus_handle(cx).contains_focused(window, cx)
                && !e
                    .active_editor()
                    .is_some_and(|s| s.focus_handle(cx).is_focused(window)),
        )
    });
    assert_eq!(tree_sel, Some(root.join("delta.txt")));
    assert!(in_tree, "focus in the tree: {}", h.focus_name());
    h.keys("alt-2");

    // 고정 해제 → the tab is the preview again; 고정 pins it back.
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "고정 해제");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [delta~] beta gamma");
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "고정");
    assert_eq!(tab_strip(&mut h, &ed), "alpha [delta] beta gamma");

    // Two unsaved tabs; "다른 탭 닫기" asks once, listing both files.
    for n in ["beta", "gamma"] {
        let p = root.join(format!("{n}.txt"));
        h.cx.update(|window, cx| ed.update(cx, |e, cx| e.open_file(&p, window, cx)))
            .expect("open");
        type_in_editor(&mut h, &ed, "z");
    }
    h.cx.update(|window, cx| {
        ed.update(cx, |e, cx| e.open_file(&root.join("delta.txt"), window, cx))
    })
    .expect("open");
    h.settle();
    assert_eq!(tab_strip(&mut h, &ed), "alpha [delta] beta* gamma*");
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "다른 탭 닫기");
    assert!(
        h.cx.update(|_, cx| ed.read(cx).close_prompt_open()),
        "one combined prompt"
    );
    assert_eq!(
        h.cx.update(|_, cx| ed.read(cx).close_prompt_files()),
        ["beta.txt", "gamma.txt"]
    );
    h.keys("escape");
    assert_eq!(
        tab_strip(&mut h, &ed),
        "alpha [delta] beta* gamma*",
        "cancel closes nothing"
    );
    assert!(
        editor_focused(&mut h, &ed),
        "focus back after cancel: {}",
        h.focus_name()
    );
    // "저장된 탭 닫기" never asks: alpha and delta go, the unsaved ones stay.
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "저장된 탭 닫기");
    assert!(!h.cx.update(|_, cx| ed.read(cx).close_prompt_open()));
    assert_eq!(
        tab_strip(&mut h, &ed).replace(['[', ']'], ""),
        "beta* gamma*"
    );
    assert!(editor_focused(&mut h, &ed));
    // "모두 닫기", Enter: both saved, then all closed; Ctrl+Shift+T brings them back in order.
    h.keys("shift-f10");
    menu_pick(&mut h, &ed, "모두 닫기");
    assert_eq!(
        h.cx.update(|_, cx| ed.read(cx).close_prompt_files()),
        ["beta.txt", "gamma.txt"]
    );
    h.keys("enter");
    assert_eq!(tab_strip(&mut h, &ed), "");
    for n in ["beta", "gamma"] {
        let saved = std::fs::read_to_string(root.join(format!("{n}.txt"))).expect("file");
        assert!(
            saved.starts_with('z'),
            "{n} saved before closing: {saved:?}"
        );
    }
    h.keys("ctrl-shift-t ctrl-shift-t");
    assert_eq!(tab_strip(&mut h, &ed).replace(['[', ']'], ""), "beta gamma");

    // "오른쪽 탭 닫기" is unavailable on the last tab: arrows skip it.
    h.keys("shift-f10");
    let items = h.cx.update(|_, cx| ed.read(cx).tab_menu_items());
    let right = items
        .iter()
        .find(|i| i.label == "오른쪽 탭 닫기")
        .expect("item");
    assert!(
        !right.enabled,
        "nothing to the right of the active (last) tab"
    );
    h.keys("down down");
    assert_eq!(
        menu_selected(&mut h, &ed).as_deref(),
        Some("저장된 탭 닫기"),
        "Down skipped the unavailable item"
    );
    h.keys("escape");
    assert!(editor_focused(&mut h, &ed));
}
