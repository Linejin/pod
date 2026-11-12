use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{AnyElement, App, Focusable, IntoElement, ParentElement, Styled, Window, div};

use super::ToastKind;
use super::binding::watching;
use crate::layout_mode::{LayoutMode, LayoutModule, LayoutRegistry};
use crate::layouts::kit::{
    LayoutActions, LayoutFrame, LayoutHost, LayoutMetrics, LayoutRenderer, OpenPanels, Slot,
    UiIntent,
};
use crate::native_qa::{Harness, seed_two_tasks};

#[test]
fn unreachable_terminal_row_tooltip_gives_the_reason() {
    use crate::vm::{Liveness, TerminalVm};
    let base = serde_json::json!({
        "id": "t9", "argv": ["claude"], "cwd": "C:/w", "cols": 80, "rows": 24,
        "created_ms": 1, "attached": 0
    });
    let with = |extra: serde_json::Value| {
        let mut v = base.clone();
        let m = v.as_object_mut().unwrap();
        m.extend(extra.as_object().unwrap().clone());
        TerminalVm::from_json(&v).unwrap()
    };
    let t = with(serde_json::json!({
        "state": "unreachable", "unreachable": "holder pipe closed"
    }));
    assert_eq!(t.live, Liveness::Lost, "still dimmed as lost");
    let detail = super::sidebar::terminal_row_detail(&t, &crate::orch::terminal_badge(&t));
    assert!(
        detail.starts_with("podd가 이 터미널에 연결할 수 없음: holder pipe closed | "),
        "{detail}"
    );
    // No reason from podd: the tooltip still says why it is dimmed.
    let t = with(serde_json::json!({"state": "unreachable"}));
    let detail = super::sidebar::terminal_row_detail(&t, &crate::orch::terminal_badge(&t));
    assert!(
        detail.starts_with("podd가 이 터미널에 연결할 수 없음 | "),
        "{detail}"
    );
    // A plain lost terminal has no unreachable reason.
    let t = with(serde_json::json!({"state": "exited", "lost": true}));
    assert_eq!(t.lost_reason(), None);
    let detail = super::sidebar::terminal_row_detail(&t, &crate::orch::terminal_badge(&t));
    assert!(!detail.contains("연결할 수 없음"), "{detail}");
}

#[test]
fn tree_keys_follow_the_aria_tree_pattern() {
    use super::sidebar::{TreeNode, TreeStep::*, tree_key};
    let n = |depth, parent, expanded| TreeNode {
        depth,
        parent,
        expanded,
    };
    // repo / task (open) / two terminals / task (folded)
    let nodes = [
        n(0, true, true),
        n(1, true, true),
        n(2, false, true),
        n(2, false, true),
        n(1, true, false),
    ];
    assert_eq!(
        tree_key(&nodes, None, "down"),
        Some(Select(0)),
        "first ↓ selects the first row, not the second"
    );
    assert_eq!(tree_key(&nodes, None, "end"), Some(Select(4)));
    assert_eq!(tree_key(&nodes, Some(2), "home"), Some(Select(0)));
    assert_eq!(tree_key(&nodes, Some(0), "end"), Some(Select(4)));
    assert_eq!(
        tree_key(&nodes, Some(3), "left"),
        Some(Select(1)),
        "← on a terminal goes to its task"
    );
    assert_eq!(
        tree_key(&nodes, Some(1), "left"),
        Some(Fold(1, false)),
        "← on an open task folds it"
    );
    assert_eq!(
        tree_key(&nodes, Some(4), "left"),
        Some(Select(0)),
        "← on a folded task goes to the repo"
    );
    assert_eq!(tree_key(&nodes, Some(4), "right"), Some(Fold(4, true)));
    assert_eq!(
        tree_key(&nodes, Some(1), "right"),
        Some(Select(2)),
        "→ on an open task enters its first child"
    );
    assert_eq!(
        tree_key(&nodes, Some(4), "down"),
        Some(Select(4)),
        "stays on the last row"
    );
    assert_eq!(tree_key(&nodes, Some(0), "x"), None);
    assert_eq!(tree_key(&[], None, "down"), None);
}

#[test]
fn task_name_carries_answer_and_failure_counts() {
    use crate::source::TestBackend;
    let t = TestBackend::task("s/t", "로그인", "s", &["b1", "w1"]);
    let terminals = [
        TestBackend::terminal("b1", "리뷰 봇", Some("s/t"), "blocked"),
        TestBackend::terminal("w1", "작업자", Some("s/t"), "working"),
    ];
    let name = super::sidebar::task_accessible_name(&t, &terminals);
    assert!(name.starts_with("로그인"), "{name}");
    assert!(
        name.contains("응답 1") && name.contains("작업 중 1"),
        "{name}"
    );
    assert!(!name.contains("오류 0"), "only non-zero counts: {name}");
}

#[test]
fn empty_workspace_offers_one_action_for_its_situation() {
    use crate::layouts::workbench::EmptyState::{self, *};
    assert_eq!(EmptyState::of(false, true, true), Connecting);
    assert_eq!(
        EmptyState::of(true, true, true),
        NoRepository,
        "nothing yet: open a repository"
    );
    assert_eq!(
        EmptyState::of(true, false, true),
        NoTask,
        "registered repository: new task, not 저장소 열기"
    );
    assert_eq!(EmptyState::of(true, false, false), PickTask);
    assert!(!NoTask.copy().1.contains("저장소"), "{}", NoTask.copy().1);
}

#[test]
fn window_title_names_the_app_and_the_task() {
    assert_eq!(super::shell::os_window_title(None), "Pod");
    assert_eq!(super::shell::os_window_title(Some("  ")), "Pod");
    assert_eq!(
        super::shell::os_window_title(Some("로그인-수정")),
        "로그인-수정 — Pod"
    );
}

#[test]
fn working_and_task_counts_read_differently() {
    assert_eq!(crate::tr!(WORKING; ; 49), "작업 중 49");
    assert_eq!(crate::tr!(TASKS; ; 100), "작업 100개");
    assert_ne!(crate::tr!(WORKING; ; 1), crate::tr!(TASKS; ; 1));
}

#[test]
fn status_bar_drops_totals_before_answers_and_notifications() {
    use super::StatusPlan;
    let wide = StatusPlan::for_width(1600., true, true);
    assert!(wide.terminals && wide.counts && wide.working && wide.done && wide.tasks && wide.stats);
    let min = StatusPlan::for_width(800., true, true);
    assert!(min.counts, "응답 stays at the minimum window");
    assert!(!min.terminals && !min.working && !min.done && !min.tasks && !min.stats);
    let mid = StatusPlan::for_width(1050., true, true);
    assert!(
        mid.working && mid.done && !mid.tasks && !mid.terminals,
        "{mid:?}"
    );
    assert!(
        !StatusPlan::for_width(1600., false, true).counts,
        "06 관제형: counts are in the title bar"
    );
    assert!(
        !StatusPlan::for_width(1600., true, false).counts,
        "no status capability"
    );
}

#[test]
fn needs_you_toast_suppressed_only_for_the_terminal_in_view() {
    // Looking at t1 in the active window: no toast for t1, toast for t2.
    assert!(watching(true, Some("t1"), "t1"));
    assert!(!watching(true, Some("t1"), "t2"));
    // E2E B5: t1 was the last terminal focused, but a Review/Group panel
    // has focus now -> nothing in view -> toast.
    assert!(!watching(true, None, "t1"));
    // Window in the background: always toast.
    assert!(!watching(false, Some("t1"), "t1"));
}

// ---- layout host (layout_host.rs, docs/20-refactor/app.md A-F1) ----------

/// What a kit renderer was lent, render by render.
#[derive(Default)]
struct KitLog {
    bodies: usize,
    title_extras: usize,
    terminals: usize,
    slots: Vec<(Slot, bool)>,
    second_toasts: Option<bool>,
}

/// The workbench with a narrow renderer that records what the host lends.
struct KitWorkbench(Rc<RefCell<KitLog>>);
impl LayoutModule for KitWorkbench {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Workbench
    }
    fn renderer(&self) -> Rc<dyn LayoutRenderer> {
        Rc::new(KitRenderer(self.0.clone()))
    }
}

struct KitRenderer(Rc<RefCell<KitLog>>);
impl LayoutRenderer for KitRenderer {
    fn render_body(
        &self,
        frame: &LayoutFrame<'_>,
        host: &mut dyn LayoutHost,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let mut log = self.0.borrow_mut();
        log.bodies += 1;
        log.terminals = frame.terminals.len();
        log.slots.clear();
        let mut body = div().size_full().flex();
        for slot in [Slot::Dock, Slot::Sidebar, Slot::Toasts, Slot::HookBanner] {
            let lent = host.slot(slot, window, cx);
            log.slots.push((slot, lent.is_some()));
            if let Some(e) = lent {
                body = body.child(e);
            }
        }
        log.second_toasts = Some(host.slot(Slot::Toasts, window, cx).is_some());
        body.into_any_element()
    }
    fn title_bar_extra(
        &self,
        _: &LayoutFrame<'_>,
        _: &LayoutActions,
        _: &mut App,
    ) -> Option<AnyElement> {
        self.0.borrow_mut().title_extras += 1;
        None
    }
}

#[test]
fn the_shell_draws_a_kit_renderer_and_falls_back_without_one() {
    let mut h = Harness::new("layout-host-renderer", seed_two_tasks);
    let log = Rc::new(RefCell::new(KitLog::default()));
    let module = KitWorkbench(log.clone());
    h.cx.update(|_, cx| {
        let mut r = LayoutRegistry::new();
        r.register(Rc::new(module));
        cx.set_global(r);
    });
    h.settle();
    {
        let log = log.borrow();
        assert!(
            log.bodies > 0,
            "the shell drew the body through the renderer"
        );
        assert!(log.title_extras > 0, "the title bar asked the renderer too");
        assert_eq!(
            log.terminals, 2,
            "the frame carries the workspace terminals"
        );
        assert_eq!(
            log.slots,
            [
                (Slot::Dock, true),
                (Slot::Sidebar, true),
                (Slot::Toasts, true),
                (Slot::HookBanner, false),
            ],
            "no hook problem: no banner"
        );
        assert_eq!(log.second_toasts, Some(false), "an element is lent once");
    }
    // The built-in registry: its own workbench renderer draws, not this one.
    let seen = (log.borrow().bodies, log.borrow().title_extras);
    h.cx.update(|_, cx| cx.set_global(LayoutRegistry::new()));
    h.settle();
    assert_eq!((log.borrow().bodies, log.borrow().title_extras), seen);
}

#[test]
fn the_layout_frame_borrows_the_workspace_state() {
    let mut h = Harness::new("layout-host-frame", seed_two_tasks);
    let ws = h.ws.clone();
    h.cx.update(|_, cx| {
        let w = ws.read(cx);
        let gates = w.orch.pending();
        let f = w.layout_frame(&gates, cx);
        assert!(std::ptr::eq(f.tasks, &w.tasks[..]), "tasks are not copied");
        assert!(std::ptr::eq(f.terminals, &w.terminals[..]));
        assert!(std::ptr::eq(f.conn, &w.conn));
        assert_eq!(f.tasks.len(), 2);
        assert_eq!(f.terminals.len(), 2);
        assert_eq!(f.gates.len(), gates.len());
        assert_eq!(f.focused_terminal, w.focused_terminal.as_deref());
        let open = OpenPanels {
            terminals: w.live_panels().len(),
            reviews: w.live_reviews().len(),
            editors: w.live_editors().len(),
            orch: w.orch.has_panels(),
            settings: w
                .settings_panel
                .as_ref()
                .is_some_and(|p| p.upgrade().is_some()),
            inspector: w
                .inspector_panel
                .as_ref()
                .is_some_and(|p| p.upgrade().is_some()),
            remote_terminals: w
                .machines
                .read(cx)
                .panels
                .iter()
                .filter(|p| p.upgrade().is_some())
                .count(),
        };
        assert_eq!(f.panels, open);
        assert_eq!(f.metrics, LayoutMetrics::current(cx));
        assert_eq!(f.warning(), None, "connected");
    });
}

#[test]
fn layout_actions_run_intents_through_the_workspace() {
    let mut h = Harness::new("layout-host-actions", seed_two_tasks);
    let ws = h.ws.clone();
    let actions =
        h.cx.update(|_, cx| ws.update(cx, |w, cx| w.layout_actions(cx)));
    assert_eq!(
        h.cx.update(|_, cx| actions.read(cx, |f| f.terminals.len())),
        Some(2)
    );

    let panels = |h: &mut Harness| {
        h.cx.update(|_, cx| actions.read(cx, |f| f.panels.terminals))
            .expect("window open")
    };
    let open = |another| UiIntent::OpenTerminal {
        id: "w1".into(),
        another,
    };
    h.cx.update(|window, cx| actions.dispatch(open(false), window, cx));
    h.settle();
    assert_eq!(h.focused_terminal().as_deref(), Some("w1"));
    let one = panels(&mut h);
    h.cx.update(|window, cx| actions.dispatch(open(false), window, cx));
    h.settle();
    assert_eq!(
        panels(&mut h),
        one,
        "an open terminal is focused, not opened twice"
    );
    h.cx.update(|window, cx| actions.dispatch(open(true), window, cx));
    h.settle();
    assert_eq!(panels(&mut h), one + 1, "another view on request");

    let sidebar = h.sidebar_focus();
    let (toasts, dock) = h.cx.update(|_, cx| {
        let w = ws.read(cx);
        (w.toast_focus.clone(), w.dock.read(cx).focus_handle(cx))
    });
    h.cx.update(|_, cx| {
        assert_eq!(
            actions.focus_handle(Slot::Sidebar, cx),
            Some(sidebar.clone())
        );
        assert_eq!(actions.focus_handle(Slot::Toasts, cx), Some(toasts.clone()));
        assert_eq!(actions.focus_handle(Slot::Dock, cx), Some(dock));
        assert_eq!(actions.focus_handle(Slot::HookBanner, cx), None);
    });

    h.cx.update(|window, cx| {
        actions.dispatch(
            UiIntent::Toast {
                kind: ToastKind::Info,
                title: "kit-toast".into(),
                body: "from a mode".into(),
            },
            window,
            cx,
        )
    });
    let titles = h.cx.update(|_, cx| ws.read(cx).visible_toasts());
    assert!(titles.iter().any(|t| t == "kit-toast"), "{titles:?}");

    assert!(!h.dialog_open());
    h.cx.update(|window, cx| actions.dispatch(UiIntent::NewTask, window, cx));
    h.settle();
    assert!(h.dialog_open(), "the new-task dialog");
}
