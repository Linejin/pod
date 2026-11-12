//! The kit reproduces what the modes compute and draw today: font, line,
//! colour, clock and warning values, and the style and geometry of the shared
//! pieces next to verbatim copies of the code they will replace. The actions
//! handle keeps synchronous and deferred order apart.
//!
//! Not `gpui_kit::*`: gpui's own `test` attribute would shadow the std one.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use ::gpui::{TestAppContext, VisualTestContext};
use gpui_kit::component::dock::DockArea;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, Div, Entity, EntityId, FocusHandle,
    IntoElement, ParentElement, Pixels, Render, Styled, Subscription, Window, div, px, rgb, size,
};
use pod_ui::themes::{Active, Metrics, Shadow, Surface, ThemeChoice};
use podd::protocol::GateResolveParams;

use super::metrics::DEFAULT_FONT_SIZE;
use super::model::connection_warning;
use super::{
    AppOnce, BodyFlow, Clock, ConnectionShell, FrameToasts, GateDone, HostChange, HostCommands,
    HostHandles, HostUpdate, Island, LayoutActions, LayoutFrame, LayoutMetrics, OnChange,
    OnChangeIn, OpenPanels, PanelDetail, Slot, ToastAnchor, UiIntent, WindowKey, color,
    panel_header, warning_banner,
};
use crate::panel::TerminalPanel;
use crate::qa_marks::QaId as _;
use crate::review::ReviewPanel;
use crate::source::PoddSource;
use crate::source::TestBackend;
use crate::vm::{ConnState, TaskVm, TerminalVm};

/// Runs `f` until the process-wide style and tokens stayed the same across
/// the call: other tests in this process switch them.
fn steady<R>(mut f: impl FnMut() -> R) -> R {
    for _ in 0..100 {
        let (style, tokens) = (pod_ui::themes::active(), crate::theme::tokens());
        let r = f();
        if Arc::ptr_eq(&style, &pod_ui::themes::active()) && tokens == crate::theme::tokens() {
            return r;
        }
    }
    panic!("the active style kept changing");
}

/// Metrics of every built-in style, without activating any.
fn style_metrics() -> Vec<(String, Metrics)> {
    let reg = pod_ui::themes::registry();
    reg.styles
        .iter()
        .map(|s| {
            let choice = ThemeChoice {
                style: s.id.clone(),
                ..Default::default()
            };
            (s.id.clone(), Active::new(&reg, &choice).metrics)
        })
        .collect()
}

fn style_of(mut d: Div) -> ::gpui::StyleRefinement {
    d.style().clone()
}

// ───────────────────────────── values ─────────────────────────────

#[test]
fn clock_is_the_system_epoch_or_a_frozen_instant() {
    assert_eq!(Clock::default(), Clock::System);
    assert_eq!(
        Clock::frozen_or_system(0),
        Clock::System,
        "0 = not frozen, like the orchestration fixture"
    );
    assert_eq!(
        Clock::frozen_or_system(1_767_225_600_000),
        Clock::Frozen(1_767_225_600_000)
    );
    assert_eq!(Clock::Frozen(42).now_ms(), 42);
    let before = podd::now_ms();
    let now = Clock::System.now_ms();
    let after = crate::orch::now_ms();
    assert!(
        before <= now && now <= after,
        "{before} ≤ {now} ≤ {after}: the epoch clock podd and orch read"
    );
}

#[test]
fn color_is_the_token_colour_of_every_copy() {
    let tk = crate::theme::tokens();
    for hex in [
        0x000000,
        0xFFFFFF,
        0x1F6FEB,
        0x7F7F7F,
        tk.bg_canvas,
        tk.accent_fg,
    ] {
        assert_eq!(color(hex), crate::orch::tc(hex), "{hex:06X}");
        assert_eq!(color(hex), pod_ui::tokens::color(hex), "{hex:06X}");
    }
}

#[test]
fn ui_font_is_the_user_size_moved_by_density_and_clamped() {
    for (id, m) in style_metrics() {
        for size in [6., 9., 12., 13., 14., 16., 20., 24., 30., 40.] {
            let k = LayoutMetrics::new(size, m, "Cascadia Mono");
            assert_eq!(
                k.ui_font,
                crate::theme::ui_font_size(size, m.density.ui_font),
                "{id} {size}"
            );
            assert!((10. ..=24.).contains(&k.ui_font), "{id} {size}");
            assert_eq!(k.line(), (k.ui_font * 1.5).ceil(), "{id} {size}");
            assert_eq!(k.style, m);
            assert_eq!(k.mono_font.as_ref(), "Cascadia Mono");
        }
    }
}

/// `LayoutMetrics::current` against the modes' own (public) font and height
/// functions, with and without settings.
#[test]
fn metrics_equal_the_modes_own_font_and_line() {
    let cx = TestAppContext::single();
    cx.update(|cx| {
        for user in [None, Some(9.), Some(13.), Some(16.), Some(20.), Some(28.)] {
            if let Some(size) = user {
                cx.set_global(crate::theme::Settings {
                    font_size: size,
                    terminal_font: "Cascadia Mono".into(),
                    ..Default::default()
                });
            }
            let cx: &App = cx;
            let (k, dense_font, dense_row, review_row, card, hit) = steady(|| {
                let k = LayoutMetrics::current(cx);
                let dense_row = crate::layouts::dense::row_height(k.ui_font);
                (
                    k,
                    crate::layouts::dense::ui_font(cx),
                    dense_row,
                    crate::layouts::review::ReviewView::row_height(cx),
                    crate::layouts::kanban::Board::card_height(cx),
                    pod_ui::hit_area(),
                )
            });
            let (m, line) = (k.style, k.line());
            let expect_font =
                crate::theme::ui_font_size(user.unwrap_or(DEFAULT_FONT_SIZE), m.density.ui_font);
            assert_eq!(k.ui_font, expect_font, "{user:?}");
            assert_eq!(k.ui_font, dense_font, "dense ui_font, {user:?}");
            // The modes' row heights differ, but all grow with the same line.
            assert_eq!(dense_row, m.density.row.max(line + 4.), "dense, {user:?}");
            assert_eq!(
                review_row,
                line * 2. + 2. + 8. * 2. + m.border_width * 2. + 4.,
                "review rows, {user:?}"
            );
            let action = hit.max(line + 6.);
            assert_eq!(
                card,
                12. * 2. + line * 2. + (line + 8.) + action + 6. * 3. + m.border_width * 2.,
                "kanban cards, {user:?}"
            );
            let mono = if user.is_some() { "Cascadia Mono" } else { "" };
            assert_eq!(k.mono_font.as_ref(), mono, "{user:?}");
        }
    });
    cx.update(|cx| cx.quit());
}

#[test]
fn warning_text_is_the_modes_connection_banner() {
    // Verbatim from the modes' render_body (workspace.rs, layouts/*.rs).
    let reference = |conn: &ConnState| match conn {
        crate::vm::ConnState::Reconnecting { attempt, error } => {
            Some(crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR; attempt,error;))
        }
        crate::vm::ConnState::Connecting => {
            Some(crate::strings::CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED.to_string())
        }
        crate::vm::ConnState::Connected { .. } => None,
    };
    let reconnecting = ConnState::Reconnecting {
        attempt: 3,
        error: "pipe closed".into(),
    };
    for conn in [
        ConnState::Connecting,
        ConnState::Connected {
            pid: 7,
            version: "0.1.0".into(),
        },
        reconnecting.clone(),
    ] {
        assert_eq!(connection_warning(&conn), reference(&conn), "{conn:?}");
    }
    let text = connection_warning(&reconnecting).expect("warning while reconnecting");
    assert!(text.contains('3') && text.contains("pipe closed"), "{text}");
}

fn fixture() -> (Vec<TaskVm>, Vec<TerminalVm>) {
    let terminals = vec![
        TestBackend::terminal("t1", "one", Some("k1"), "working"),
        TestBackend::terminal("t2", "two", None, "blocked"),
        TestBackend::terminal("t3", "three", Some("k1"), "done"),
        // A duplicate id: lookups take the first, like `Workspace::terminal`.
        TestBackend::terminal("t1", "shadow", None, "idle"),
    ];
    let tasks = vec![
        TestBackend::task("k1", "first", "repo", &["t1", "t3"]),
        TestBackend::task("k2", "second", "repo", &[]),
    ];
    (tasks, terminals)
}

fn frame<'a>(
    tasks: &'a [TaskVm],
    terminals: &'a [TerminalVm],
    conn: &'a ConnState,
) -> LayoutFrame<'a> {
    LayoutFrame {
        tasks,
        repos: &[],
        terminals,
        conn,
        gates: &[],
        focused_terminal: Some("t2"),
        panels: OpenPanels::default(),
        panel_detail: PanelDetail::default(),
        metrics: LayoutMetrics::new(DEFAULT_FONT_SIZE, pod_ui::themes::metrics(), ""),
        labels: &NO_LABELS,
        all_gates: &NO_GATES,
        toasts: FrameToasts(&[]),
        tree_selection: None,
        fixture: false,
    }
}

static NO_LABELS: BTreeMap<String, String> = BTreeMap::new();
static NO_GATES: BTreeMap<String, podd::protocol::GateInfo> = BTreeMap::new();

#[test]
fn frame_reads_like_the_workspace_lookups() {
    let (tasks, terminals) = fixture();
    let conn = ConnState::Connecting;
    let f = frame(&tasks, &terminals, &conn);
    assert_eq!(
        f.terminal("t1").and_then(|t| t.name.as_deref()),
        Some("one")
    );
    assert_eq!(
        f.terminal("t3").and_then(|t| t.task_id.as_deref()),
        Some("k1")
    );
    assert!(f.terminal("nope").is_none());
    assert_eq!(f.task("k2").map(|t| t.title.as_str()), Some("second"));
    assert!(f.task("nope").is_none());
    assert_eq!(f.warning(), connection_warning(&conn));
    assert_eq!(f.focused_terminal, Some("t2"));
}

#[test]
fn dock_is_empty_only_without_any_panel() {
    assert!(OpenPanels::default().dock_empty());
    let one = OpenPanels::default();
    for p in [
        OpenPanels {
            terminals: 1,
            ..one
        },
        OpenPanels { reviews: 1, ..one },
        OpenPanels { editors: 2, ..one },
        OpenPanels { orch: true, ..one },
        OpenPanels {
            settings: true,
            ..one
        },
        OpenPanels {
            inspector: true,
            ..one
        },
        OpenPanels {
            remote_terminals: 1,
            ..one
        },
    ] {
        assert!(!p.dock_empty(), "{p:?}");
    }
}

// ───────────────────────────── actions ─────────────────────────────

/// Host side of the actions handle: a fixed window state and an intent log.
/// Only what the kit's own tests read does something; every other method is
/// an explicit no-op here (the trait has no defaults, rf2-kit), and the real
/// host is tested in `workspace::layout_host::tests`.
struct Sink {
    tasks: Vec<TaskVm>,
    terminals: Vec<TerminalVm>,
    conn: ConnState,
    gone: bool,
    focus: Option<FocusHandle>,
    log: RefCell<Vec<UiIntent>>,
    /// Release hooks the kit registered, run by the test.
    released: RefCell<Vec<AppOnce>>,
}

impl Sink {
    fn new(gone: bool, focus: Option<FocusHandle>) -> Rc<Self> {
        let (tasks, terminals) = fixture();
        Rc::new(Self {
            tasks,
            terminals,
            conn: ConnState::Connecting,
            gone,
            focus,
            log: RefCell::default(),
            released: RefCell::default(),
        })
    }
}

impl HostHandles for Sink {
    fn window_key(&self) -> WindowKey {
        WindowKey(EntityId::from(7u64))
    }
    fn focus_handle(&self, slot: Slot, _: &App) -> Option<FocusHandle> {
        (slot == Slot::Sidebar)
            .then(|| self.focus.clone())
            .flatten()
    }
    fn on_release(&self, _: &mut App, f: AppOnce) -> bool {
        if !self.gone {
            self.released.borrow_mut().push(f);
        }
        !self.gone
    }
    fn observe(&self, _: HostChange, _: &mut App, _: OnChange) -> Option<Subscription> {
        None
    }
    fn observe_in(
        &self,
        _: HostChange,
        _: &mut Window,
        _: &mut App,
        _: OnChangeIn,
    ) -> Option<Subscription> {
        None
    }
    fn dock(&self, _: &App) -> Option<Entity<DockArea>> {
        None
    }
    fn view(&self, _: Slot, _: &App) -> Option<AnyView> {
        None
    }
    fn source(&self, _: &App) -> Option<PoddSource> {
        None
    }
}

impl HostCommands for Sink {
    fn with_frame(&self, _: &App, f: &mut dyn FnMut(&LayoutFrame<'_>)) {
        if !self.gone {
            f(&frame(&self.tasks, &self.terminals, &self.conn));
        }
    }
    fn dispatch(&self, intent: UiIntent, _: &mut Window, _: &mut App) {
        self.log.borrow_mut().push(intent);
    }
    fn current_task(&self, _: &Window, _: &App) -> Option<String> {
        None
    }
    fn terminal_panels(&self, _: &App) -> Vec<Entity<TerminalPanel>> {
        Vec::new()
    }
    fn review_panels(&self, _: &App) -> Vec<Entity<ReviewPanel>> {
        Vec::new()
    }
    fn new_terminal_panel(
        &self,
        _: &str,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Entity<TerminalPanel>> {
        None
    }
    fn new_review_panel(
        &self,
        _: &str,
        _: &mut Window,
        _: &mut App,
    ) -> Option<Entity<ReviewPanel>> {
        None
    }
    fn post(&self, _: Vec<HostUpdate>, _: &mut App) -> bool {
        false
    }
    fn resolve_gate(&self, _: GateResolveParams, _: &mut App, _: GateDone) -> bool {
        false
    }
}

struct Blank;
impl Render for Blank {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[test]
fn actions_read_a_fresh_frame_and_nothing_once_the_window_is_gone() {
    let cx = TestAppContext::single();
    cx.update(|cx| {
        let live = LayoutActions::new(Sink::new(false, None));
        let names = live.read(cx, |f| {
            f.terminals.iter().map(|t| t.id.clone()).collect::<Vec<_>>()
        });
        assert_eq!(
            names.as_deref(),
            Some(&["t1", "t2", "t3", "t1"].map(String::from)[..])
        );
        let gone = LayoutActions::new(Sink::new(true, None));
        assert_eq!(gone.read(cx, |f| f.tasks.len()), None);
        // Clones reach the same host.
        assert_eq!(
            live.clone().read(cx, |f| f.task("k1").is_some()),
            Some(true)
        );
    });
    cx.update(|cx| cx.quit());
}

#[test]
fn dispatch_runs_at_once_and_defer_after_the_current_effects() {
    let mut app = TestAppContext::single();
    let handle = app.add_window(|_, _| Blank);
    let mut cx = VisualTestContext::from_window(handle.into(), &app);
    let focus = cx.update(|_, cx| cx.focus_handle());
    let sink = Sink::new(false, Some(focus.clone()));
    let actions = LayoutActions::new(sink.clone());
    let open = UiIntent::OpenTerminal {
        id: "t1".into(),
        another: false,
    };
    cx.update(|window, cx| {
        actions.defer(UiIntent::OpenGates, window, cx);
        actions.dispatch(open.clone(), window, cx);
        assert_eq!(
            *sink.log.borrow(),
            std::slice::from_ref(&open),
            "dispatch is synchronous"
        );
        assert_eq!(actions.focus_handle(Slot::Sidebar, cx), Some(focus.clone()));
        assert_eq!(actions.focus_handle(Slot::Dock, cx), None);
    });
    cx.run_until_parked();
    assert_eq!(
        *sink.log.borrow(),
        [open, UiIntent::OpenGates],
        "deferred intents run after the update that queued them"
    );
    cx.update(|_, cx| cx.quit());
}

#[test]
fn dispatch_then_runs_the_completion_after_the_intent() {
    let mut app = TestAppContext::single();
    let handle = app.add_window(|_, _| Blank);
    let mut cx = VisualTestContext::from_window(handle.into(), &app);
    let sink = Sink::new(false, None);
    let actions = LayoutActions::new(sink.clone());
    let task = UiIntent::OpenTaskTerminals { task: "k1".into() };
    let seen = Rc::new(RefCell::new(Vec::new()));
    cx.update(|window, cx| {
        let (log, seen) = (sink.clone(), seen.clone());
        actions.dispatch_then(task.clone(), window, cx, move |_, _| {
            seen.borrow_mut().push(log.log.borrow().len());
        });
    });
    assert_eq!(*seen.borrow(), [1], "the intent ran before the completion");
    assert_eq!(*sink.log.borrow(), [task]);
    cx.update(|_, cx| cx.quit());
}

#[test]
fn label_of_prefers_the_saved_label_then_the_name_then_the_id() {
    let (tasks, mut terminals) = fixture();
    terminals[1].name = None;
    let t1 = terminals[0].clone();
    let labels = BTreeMap::from([(
        crate::layout::label_key(&t1.id, t1.created_ms),
        "저장한 이름".to_string(),
    )]);
    let conn = ConnState::Connecting;
    let f = LayoutFrame {
        labels: &labels,
        ..frame(&tasks, &terminals, &conn)
    };
    assert_eq!(f.label_of(&terminals[0]), "저장한 이름");
    assert_eq!(f.label_of(&terminals[1]), "t2", "no name: the id");
    assert_eq!(f.label_of(&terminals[2]), "three");
    // The key carries the creation time: a reused id is not renamed.
    let mut again = t1.clone();
    again.created_ms += 1;
    assert_eq!(f.label_of(&again), "one");
}

#[test]
fn other_panels_count_one_each_like_the_review_signature() {
    let (tasks, terminals) = fixture();
    let conn = ConnState::Connecting;
    let base = frame(&tasks, &terminals, &conn);
    assert_eq!(base.other_panels(), 0);
    let f = LayoutFrame {
        panels: OpenPanels {
            // Terminals and reviews are not counted.
            terminals: 3,
            reviews: 2,
            editors: 2,
            orch: true,
            settings: true,
            inspector: true,
            remote_terminals: 2,
        },
        panel_detail: PanelDetail {
            groups: 2,
            gates_panel: true,
            tree_panel: true,
            mods_page: true,
            sessions_page: true,
            // fx-layouts: the orchestration pages count too (opening one
            // switches the kanban drawer and the review canvas to 패널).
            orch_page: true,
        },
        ..base.clone()
    };
    assert_eq!(f.other_panels(), 1 + 1 + 2 + 2 + 2 + 1 + 1 + 1 + 1 + 1);
}

#[test]
fn state_slots_keep_one_entity_per_type_until_the_window_is_released() {
    #[derive(Default)]
    struct RailLike(u32);
    #[derive(Default)]
    struct QueueLike;

    let mut app = TestAppContext::single();
    let handle = app.add_window(|_, _| Blank);
    let mut cx = VisualTestContext::from_window(handle.into(), &app);
    let sink = Sink::new(false, None);
    let actions = LayoutActions::new(sink.clone());
    let made = Rc::new(RefCell::new(0));
    let make = |made: &Rc<RefCell<u32>>| {
        let made = made.clone();
        move |_: &mut Window, cx: &mut App| {
            *made.borrow_mut() += 1;
            let n = *made.borrow();
            cx.new(|_| RailLike(n))
        }
    };
    let weak = cx.update(|window, cx| {
        let a = actions.state_or_insert_with(window, cx, make(&made));
        let b = actions
            .clone()
            .state_or_insert_with(window, cx, make(&made));
        assert_eq!(a.entity_id(), b.entity_id(), "made on first use only");
        assert_eq!(a.read(cx).0, 1);
        let q = actions.state_or_insert_with(window, cx, |_, cx| cx.new(|_| QueueLike));
        assert_ne!(q.entity_id(), a.entity_id(), "one slot per type");
        assert_eq!(
            super::state::<RailLike>(actions.window_key(), cx).map(|s| s.entity_id()),
            Some(a.entity_id())
        );
        assert_eq!(
            super::state_in_window::<RailLike>(window.window_handle(), cx).map(|s| s.entity_id()),
            Some(a.entity_id()),
            "app-level shortcuts find it from the window"
        );
        assert_eq!(super::states::<QueueLike>(cx).len(), 1);
        a.downgrade()
    });
    assert_eq!(
        sink.released.borrow().len(),
        1,
        "one release hook per window, not per state"
    );
    // The host goes away: its hook drops every state of the window.
    let hooks = std::mem::take(&mut *sink.released.borrow_mut());
    cx.update(|_, cx| {
        for f in hooks {
            f(cx);
        }
        assert!(actions.state::<RailLike>(cx).is_none());
        assert!(actions.state::<QueueLike>(cx).is_none());
    });
    cx.run_until_parked();
    assert!(weak.upgrade().is_none(), "the slot owned the only handle");
    // A gone host: the state is made for the caller but not kept.
    let gone = LayoutActions::new(Sink::new(true, None));
    cx.update(|window, cx| {
        let s = gone.state_or_insert_with(window, cx, make(&made));
        assert_eq!(s.read(cx).0, 2);
        assert!(gone.state::<RailLike>(cx).is_none());
    });
    cx.update(|_, cx| cx.quit());
}

// ───────────────────────────── widgets ─────────────────────────────

#[test]
fn island_matches_the_bodies_island_closure() {
    for (id, m) in style_metrics() {
        let (kit, reference, gap, rounded) = steady(|| {
            let tk = crate::theme::tokens();
            let island = Island::new(m, tk.bg_surface, tk.border_subtle);
            // Verbatim from workspace.rs render_workbench_body (and queue.rs, dense.rs).
            let rounded = matches!(m.surface, Surface::Layered | Surface::Islands);
            let gap = if rounded { m.gap } else { 0. };
            let reference = move |d: Div| -> Div {
                if !rounded {
                    return d;
                }
                let d = d
                    .rounded(px(m.radius.card))
                    .overflow_hidden()
                    .bg(rgb(tk.bg_surface));
                match (m.surface, m.shadow) {
                    (Surface::Islands, _) | (_, Shadow::Raised) => d
                        .shadow(pod_ui::tokens::elevation(1, crate::theme::mode()))
                        .border_1()
                        .border_color(rgb(tk.border_subtle)),
                    (_, Shadow::Soft) => {
                        d.shadow(pod_ui::tokens::elevation(1, crate::theme::mode()))
                    }
                    _ => d,
                }
            };
            (
                style_of(island.wrap(div().flex_1().min_h_0())),
                style_of(reference(div().flex_1().min_h_0())),
                (island.gap(), gap),
                (island.rounded(), rounded),
            )
        });
        assert_eq!(kit, reference, "{id}");
        assert_eq!(gap.0, gap.1, "{id}");
        assert_eq!(rounded.0, rounded.1, "{id}");
    }
    let current = steady(|| {
        let tk = crate::theme::tokens();
        (
            Island::current(),
            Island::new(pod_ui::themes::metrics(), tk.bg_surface, tk.border_subtle),
        )
    });
    assert_eq!(current.0, current.1);
}

#[test]
fn panel_header_matches_the_dense_header() {
    for (id, m) in style_metrics() {
        let (kit, reference) = steady(|| {
            let tk = crate::theme::tokens();
            // Verbatim from layouts/dense.rs `panel_header`, metrics passed in.
            let reference = div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .gap_1()
                .pl_3()
                .pr_1()
                .min_h(px(m.density.panel_header.max(30.)))
                .when(m.surface != Surface::Mono, |d| {
                    d.border_b(px(m.border_width))
                        .border_color(crate::orch::tc(tk.border_subtle))
                });
            let kit = super::widgets::header_with(
                m,
                tk.border_subtle,
                "kit-header",
                "제목",
                Vec::new(),
                m.density.panel_header.max(30.),
            );
            (style_of(kit), style_of(reference))
        });
        assert_eq!(kit, reference, "{id}");
    }
    let (kit, with) = steady(|| {
        let tk = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        (
            style_of(panel_header("h", "t", Vec::new(), 28.)),
            style_of(super::widgets::header_with(
                m,
                tk.border_subtle,
                "h",
                "t",
                Vec::new(),
                28.,
            )),
        )
    });
    assert_eq!(kit, with, "the active style");
}

#[test]
fn warning_banner_matches_the_modes_banner_box() {
    let (kit, reference) = steady(|| {
        (
            style_of(warning_banner("podd")),
            // Verbatim (the box around `pod_ui::banner`).
            style_of(div().px_2().pt_2().child(
                pod_ui::banner(pod_ui::Tone::Warning, crate::theme::mode(), "podd", None).text_xs(),
            )),
        )
    });
    assert_eq!(kit, reference);
}

/// What the shells under test contain.
#[derive(Clone, Copy, Debug)]
struct ShellCase {
    warning: bool,
    hook: bool,
}

const SHELL_IDS: [&str; 4] = ["probe-root", "probe-hook", "probe-body", "probe-toasts"];

fn parts(case: ShellCase) -> (Option<String>, Option<AnyElement>, AnyElement, AnyElement) {
    let warning = case.warning.then(|| {
        connection_warning(&ConnState::Reconnecting {
            attempt: 2,
            error: "pipe closed".into(),
        })
        .expect("warning")
    });
    let hook = case.hook.then(|| {
        div()
            .h(px(24.))
            .w_full()
            .qa_id(|| "probe-hook".into())
            .into_any_element()
    });
    let body = div().size_full().qa_id(|| "probe-body".into());
    let toasts = div().h(px(30.)).w_full().qa_id(|| "probe-toasts".into());
    (
        warning,
        hook,
        body.into_any_element(),
        toasts.into_any_element(),
    )
}

/// Verbatim shell of layouts/kanban.rs (also blocks.rs, review.rs) `render_body`.
fn kanban_reference(case: ShellCase) -> Div {
    let (banner, hook, board, toasts) = parts(case);
    let tk = crate::theme::tokens();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .relative()
        .bg(rgb(tk.bg_canvas))
        .when_some(banner, |d, b| {
            d.child(div().px_2().pt_2().child(
                pod_ui::banner(pod_ui::Tone::Warning, crate::theme::mode(), b, None).text_xs(),
            ))
        })
        .when_some(hook, |d, b| d.child(b))
        .child(div().flex().flex_col().flex_1().min_h_0().child(board))
        .child(
            div()
                .absolute()
                .top(px(52.))
                .right(px(16.))
                .w(px(340.))
                .child(toasts),
        )
}

/// Verbatim shell of layouts/rail.rs `render_body`.
fn rail_reference(case: ShellCase) -> Div {
    use crate::layouts::rail::RAIL_WIDTH;
    let (banner, hook, rail, toasts) = parts(case);
    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .relative()
        .when_some(banner, |d, b| {
            d.child(div().px_2().pt_2().child(
                pod_ui::banner(pod_ui::Tone::Warning, crate::theme::mode(), b, None).text_xs(),
            ))
        })
        .when_some(hook, |d, b| d.child(b))
        .child(div().flex().flex_1().min_h_0().child(rail))
        .child(
            div()
                .absolute()
                .left(px(RAIL_WIDTH + 8.))
                .bottom(px(8.))
                .w(px(320.))
                .child(toasts),
        )
}

fn kanban_kit(case: ShellCase) -> Div {
    let (warning, hook_banner, body, toasts) = parts(case);
    ConnectionShell {
        warning,
        hook_banner,
        canvas: Some(crate::theme::tokens().bg_canvas),
        flow: BodyFlow::Column,
        body,
        toasts,
        anchor: ToastAnchor::TOP_RIGHT,
    }
    .render()
}

fn rail_kit(case: ShellCase) -> Div {
    let (warning, hook_banner, body, toasts) = parts(case);
    ConnectionShell {
        warning,
        hook_banner,
        canvas: None,
        flow: BodyFlow::Row,
        body,
        toasts,
        anchor: ToastAnchor::BottomLeft {
            left: crate::layouts::rail::RAIL_WIDTH + 8.,
            bottom: 8.,
            width: 320.,
        },
    }
    .render()
}

/// Builds one shell (the kit or its reference).
type Build = fn(ShellCase) -> Div;

/// A window body that draws one shell.
struct Probe(Build, ShellCase);
impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child((self.0)(self.1).qa_id(|| "probe-root".into()))
    }
}

/// Bounds of the shell's parts, drawn headless at 900×600.
fn shell_bounds(build: Build, case: ShellCase) -> Vec<Option<Bounds<Pixels>>> {
    let mut app = TestAppContext::single();
    app.update(gpui_kit::init);
    let handle = app.add_window(move |_, _| Probe(build, case));
    let mut cx = VisualTestContext::from_window(handle.into(), &app);
    cx.simulate_resize(size(px(900.), px(600.)));
    cx.run_until_parked();
    crate::qa_marks::clear();
    cx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    let out = SHELL_IDS
        .iter()
        .map(|id| crate::qa_marks::bounds(id))
        .collect();
    cx.update(|_, cx| cx.quit());
    out
}

#[test]
fn connection_shell_matches_the_kanban_and_rail_shells() {
    let cases = [
        ShellCase {
            warning: false,
            hook: false,
        },
        ShellCase {
            warning: true,
            hook: false,
        },
        ShellCase {
            warning: false,
            hook: true,
        },
        ShellCase {
            warning: true,
            hook: true,
        },
    ];
    for case in cases {
        let (kit, reference) =
            steady(|| (style_of(kanban_kit(case)), style_of(kanban_reference(case))));
        assert_eq!(kit, reference, "kanban root style {case:?}");
        let (kit, reference) =
            steady(|| (style_of(rail_kit(case)), style_of(rail_reference(case))));
        assert_eq!(kit, reference, "rail root style {case:?}");
        for (name, kit, reference) in [
            ("kanban", kanban_kit as Build, kanban_reference as Build),
            ("rail", rail_kit as Build, rail_reference as Build),
        ] {
            let (kit, reference) =
                steady(|| (shell_bounds(kit, case), shell_bounds(reference, case)));
            assert_eq!(kit, reference, "{name} geometry {case:?}");
            for (id, b) in SHELL_IDS.iter().zip(&kit) {
                let drawn = b.is_some();
                let expect = *id != "probe-hook" || case.hook;
                assert_eq!(drawn, expect, "{name} {id} {case:?}");
            }
            // Not vacuous: the warning and the hook prompt push the body down.
            let (root, body) = (kit[0].expect("root"), kit[2].expect("body"));
            let pushed = body.origin.y - root.origin.y;
            let mut min = px(0.);
            if case.hook {
                min += px(24.);
            }
            if case.warning {
                min += px(8.);
            }
            assert!(
                pushed >= min,
                "{name} {case:?}: body {pushed:?} below the top"
            );
            if !case.hook && !case.warning {
                assert_eq!(pushed, px(0.), "{name}: nothing above the body");
            }
        }
    }
}
