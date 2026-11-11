//! 응답 대기열·관제 layout (presets 03 "응답 중심 관제" and 06 "관제형", docs/10 §D5.7).
//!
//! Left: the response queue — counts, a card for everything that needs the
//! user (pending gates with inline 이번만 허용 / 거부, blocked agents, failures),
//! then compact 작업 중 and 완료 sections. Right: the workspace's own dock, the
//! same entity every layout renders, so switching layouts keeps every panel,
//! terminal attachment and the dock arrangement as they were.
//!
//! 06 ([`Variant::Mission`], chosen by the `mission` style) moves the counts
//! into title-bar pills and adds an activity bar on the far left.
//!
//! The data is the workspace's (terminals, tasks, gates, the gate decision
//! path in [`crate::gates::resolve_gate`]). The queue keeps view state only
//! (selection, expanded sections, filter, dismissed failures) in one
//! [`QueueColumn`] per workspace, held in a global so it survives layout
//! switches. It re-renders on the workspace's change notifications; no timer.
//!
//! Files (docs/20-refactor/app.md A-P10): `model` holds the pure queue,
//! selection and gate-button plans, `view` the render, `copy` the strings.
//! This file keeps the layout module, the column with its controller methods
//! and the legacy adapter. Reads go through the kit's frame and UI actions
//! through [`LayoutActions`]; what the kit does not carry yet stays on the
//! workspace: observing it, terminal labels, the gate decision, showing a
//! terminal without focus and making the column from the title bar.

pub mod copy;
mod model;
mod view;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::dock::PanelId;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use podd::protocol::{GateDecision, GateInfo, GateState};

pub use self::model::{QueueModel, Section, Variant, age, build};

use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{LayoutActions, LayoutFrame, UiIntent};
use crate::vm::TerminalVm;
use crate::workspace::Workspace;

actions!(pod_queue, [FocusQueue]);

/// Moves keyboard focus to the queue list (from a terminal, the title bar …).
pub const FOCUS_QUEUE_KEY: &str = "ctrl-shift-q";
/// `--ui-fixture --fixture-scene queue`: a pending gate, a question and four open panels.
pub const FIXTURE_SCENE: &str = "queue";

impl Variant {
    pub fn current(cx: &App) -> Self {
        cx.try_global::<crate::theme::Settings>()
            .map(|s| Self::of_style(&s.style))
            .unwrap_or(Self::Classic)
    }
}

/// The queue for a frame. Terminal labels are the workspace's (`label_of`):
/// the frame does not carry them yet.
fn model_of(
    f: &LayoutFrame<'_>,
    label: &dyn Fn(&TerminalVm) -> String,
    query: &str,
    dismissed: &HashSet<(String, u64)>,
) -> QueueModel {
    build(f.terminals, f.tasks, f.gates, label, query, dismissed)
}

// ───────────────────────────── the column view ─────────────────────────────

/// View state of the queue column (one per workspace, kept across layout switches).
pub struct QueueColumn {
    /// Legacy adapter: observed for changes; terminal labels, the gate
    /// decision and showing a panel without focus (not in the kit yet).
    ws: WeakEntity<Workspace>,
    /// Reads (frame) and UI intents of the host.
    actions: LayoutActions,
    pub(crate) focus: FocusHandle,
    filter: Entity<InputState>,
    pub(crate) selected: Option<String>,
    /// Index of the selection, so the next card is selected when one is decided.
    sel_index: usize,
    pub(crate) all_working: bool,
    pub(crate) all_done: bool,
    /// 06: queue column hidden by the activity bar.
    pub(crate) hidden: bool,
    /// 06: the search field is shown.
    filter_open: bool,
    busy: HashMap<String, GateDecision>,
    errors: HashMap<String, String>,
    dismissed: HashSet<(String, u64)>,
    scroll: ScrollHandle,
    /// Scroll child index of each item in the last frame.
    child_index: HashMap<String, usize>,
    _subs: Vec<Subscription>,
}

/// One queue column per workspace window.
#[derive(Default)]
struct QueueViews(HashMap<EntityId, Entity<QueueColumn>>);
impl Global for QueueViews {}

/// The workspace's queue column, created on first use.
pub(crate) fn column(
    ws: &Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<QueueColumn> {
    let id = cx.entity_id();
    if let Some(c) = cx
        .try_global::<QueueViews>()
        .and_then(|v| v.0.get(&id))
        .cloned()
    {
        return c;
    }
    let me = cx.entity();
    let actions = ws.layout_actions(cx);
    let c = cx.new(|cx| QueueColumn::new(me, actions, window, cx));
    cx.default_global::<QueueViews>().0.insert(id, c.clone());
    c
}

/// The column if it was ever shown (title-bar pills of a workspace).
fn existing_column(id: EntityId, cx: &App) -> Option<Entity<QueueColumn>> {
    cx.try_global::<QueueViews>()
        .and_then(|v| v.0.get(&id))
        .cloned()
}

struct KeysBound;
impl Global for KeysBound {}

impl QueueColumn {
    fn new(
        ws: Entity<Workspace>,
        actions: LayoutActions,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        if cx.try_global::<KeysBound>().is_none() {
            cx.bind_keys([KeyBinding::new(FOCUS_QUEUE_KEY, FocusQueue, None)]);
            // App level: the shortcut must work wherever focus is (workspace root,
            // terminal, title bar), and the root sits above any layout element.
            let app: &mut App = cx;
            app.on_action(|_: &FocusQueue, cx: &mut App| {
                let Some(window) = cx.active_window() else {
                    return;
                };
                cx.defer(move |cx| {
                    let _ = window.update(cx, |_, window, cx| {
                        let layout = cx
                            .try_global::<crate::theme::Settings>()
                            .map(|s| s.layout.clone())
                            .unwrap_or_default();
                        if layout != LayoutMode::Queue.id() {
                            return;
                        }
                        let Some(views) = cx.try_global::<QueueViews>() else {
                            return;
                        };
                        let cols: Vec<Entity<QueueColumn>> = views.0.values().cloned().collect();
                        // The column whose workspace lives in this window.
                        let ws = cx
                            .try_global::<crate::workspace::AppGlobals>()
                            .map(|g| g.workspace.entity_id());
                        let col = ws
                            .and_then(|id| views.0.get(&id).cloned())
                            .or_else(|| (cols.len() == 1).then(|| cols[0].clone()));
                        if let Some(col) = col {
                            col.update(cx, |q, cx| q.focus_list(window, cx));
                        }
                    });
                });
            });
            cx.set_global(KeysBound);
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(copy::SEARCH));
        let subs = vec![
            // Tasks, terminals, statuses and gates all notify the workspace: that is the only refresh.
            cx.observe(&ws, |_, _, cx| cx.notify()),
            cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()),
        ];
        Self {
            ws: ws.downgrade(),
            actions,
            // A tracked handle must be a tab stop itself (`.tab_index()` is for element-owned ones).
            focus: cx.focus_handle().tab_stop(true),
            filter,
            selected: None,
            sel_index: 0,
            all_working: false,
            all_done: false,
            hidden: false,
            filter_open: false,
            busy: HashMap::new(),
            errors: HashMap::new(),
            dismissed: HashSet::new(),
            scroll: ScrollHandle::new(),
            child_index: HashMap::new(),
            _subs: subs,
        }
    }

    fn model(&self, cx: &App) -> QueueModel {
        let query = self.filter.read(cx).value().to_string();
        let Some(ws) = self.ws.upgrade() else {
            return QueueModel::default();
        };
        let ws = ws.read(cx);
        self.actions
            .read(cx, |f| {
                model_of(f, &|t| ws.label_of(t), &query, &self.dismissed)
            })
            .unwrap_or_default()
    }

    /// Keyboard focus to the list (selecting the first item when nothing is).
    pub(crate) fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.hidden = false;
        if self.selected.is_none() {
            self.selected = self
                .model(cx)
                .keys(self.all_working, self.all_done)
                .first()
                .map(|(k, _)| k.clone());
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Title-bar pill: show the section, select its first item, focus the list.
    pub(crate) fn focus_section(
        &mut self,
        s: Section,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(k) = self.model(cx).first_of(s) {
            self.selected = Some(k);
        }
        self.focus_list(window, cx);
        if let Some(ix) = self.selected.as_ref().and_then(|k| self.child_index.get(k)) {
            self.scroll.scroll_to_item(*ix);
        }
    }

    fn select(
        &mut self,
        key: &str,
        terminal: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(key.to_string());
        if let Some(ix) = self.child_index.get(key) {
            self.scroll.scroll_to_item(*ix);
        }
        // Show the terminal if it already has a panel; keyboard focus stays here.
        // Legacy adapter: the kit has no "show without focus" intent yet.
        if let (Some(t), Some(ws)) = (terminal, self.ws.upgrade()) {
            ws.update(cx, |w, cx| {
                if let Some(p) = w
                    .live_panels()
                    .into_iter()
                    .find(|p| p.read(cx).terminal_id == t)
                {
                    let pid = PanelId::from(p.entity_id());
                    w.dock.update(cx, |a, cx| a.select_panel(pid, window, cx));
                }
            });
        }
        cx.notify();
    }

    /// Select the item and focus its terminal in the dock (opening a panel when needed).
    fn open(
        &mut self,
        key: &str,
        terminal: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected = Some(key.to_string());
        if let Some(t) = terminal {
            self.actions.dispatch(
                UiIntent::OpenTerminal {
                    id: t.to_owned(),
                    another: false,
                },
                window,
                cx,
            );
        }
        cx.notify();
    }

    /// 이번만 허용 / 거부 through the one gate decision path.
    ///
    /// Legacy adapter: `resolve_gate` takes the workspace's source and the
    /// workspace itself; the kit has no gate intent with a completion yet.
    fn decide(
        &mut self,
        gate: String,
        decision: GateDecision,
        choice: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy.contains_key(&gate) {
            return;
        }
        let Some(ws) = self.ws.upgrade() else { return };
        let source = ws.read(cx).source.clone();
        self.busy.insert(gate.clone(), decision);
        self.errors.remove(&gate);
        // The card goes away when podd answers: keep the keyboard in the queue.
        self.focus.focus(window, cx);
        let me = cx.entity().downgrade();
        let id = gate.clone();
        let params = podd::protocol::GateResolveParams {
            id: gate,
            decision,
            note: None,
            choice,
        };
        crate::gates::resolve_gate(&source, self.ws.clone(), params, cx, move |r, cx| {
            let _ = me.update(cx, |q, cx| {
                q.busy.remove(&id);
                if let Err(e) = r {
                    q.errors.insert(id, e.text);
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    fn dismiss(&mut self, key: &str, since: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.dismissed.insert((key.to_string(), since));
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn on_key(
        &mut self,
        e: &KeyDownEvent,
        keys: &[(String, Option<String>)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if keys.is_empty() {
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|s| keys.iter().position(|(k, _)| k == s));
        match model::list_key(e.keystroke.key.as_str(), at, keys.len()) {
            Some(model::ListKey::Open(at)) => {
                if let Some(i) = at {
                    let (k, t) = keys[i].clone();
                    self.open(&k, t.as_deref(), window, cx);
                }
                cx.stop_propagation();
            }
            Some(model::ListKey::Select(i)) => {
                let (k, t) = keys[i].clone();
                self.select(&k, t.as_deref(), window, cx);
                cx.stop_propagation();
            }
            None => {}
        }
    }
}

// ───────────────────────────── layout module ─────────────────────────────

/// The 응답 대기열 layout module (03 and 06).
pub struct QueueLayout;

impl LayoutModule for QueueLayout {
    fn mode(&self) -> LayoutMode {
        LayoutMode::Queue
    }

    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        render_body(ws, window, cx)
    }

    fn title_bar_extra(&self, ws: &Workspace, cx: &mut Context<Workspace>) -> Option<AnyElement> {
        (Variant::current(cx) == Variant::Mission).then(|| title_pills(ws, cx))
    }
    fn status_counts(&self, cx: &App) -> bool {
        Variant::current(cx) != Variant::Mission
    }
}

// ───────────────────── legacy adapter: workspace → view ─────────────────────

/// 06: status pills in the title bar; each jumps to its section. A pill makes
/// the column when it was never shown, which needs the workspace.
fn title_pills(ws: &Workspace, cx: &mut Context<Workspace>) -> AnyElement {
    let gates = ws.orch.pending();
    let counts = model_of(
        &ws.layout_frame(&gates, cx),
        &|t| ws.label_of(t),
        "",
        &HashSet::new(),
    )
    .counts;
    let ws_id = cx.entity_id();
    let ws_weak = cx.entity().downgrade();
    let open: view::OpenSection = Rc::new(move |section, window, cx| {
        let col = existing_column(ws_id, cx).or_else(|| {
            ws_weak
                .upgrade()
                .map(|w| w.update(cx, |w, cx| column(w, window, cx)))
        });
        if let Some(col) = col {
            col.update(cx, |q, cx| q.focus_section(section, window, cx));
        }
    });
    view::title_pills(counts, open)
}

/// The window body: [activity bar] · queue column (+ toasts) · dock.
pub fn render_body(
    ws: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> AnyElement {
    let column = column(ws, window, cx);
    let variant = Variant::current(cx);
    let toasts = ws.render_toasts(cx).into_any_element();
    let hook_banner = ws.render_hook_banner(cx);
    let actions = ws.layout_actions(cx);
    // The activity bar's 변경 검토 runs the review command, as from the keyboard.
    let ws_weak = cx.entity().downgrade();
    let review: view::Activate = Box::new(move |window, cx| {
        let _ = ws_weak.update(cx, |_, cx| {
            window.dispatch_action(Box::new(crate::workspace::OpenReview), cx)
        });
    });
    let gates = ws.orch.pending();
    let frame = ws.layout_frame(&gates, cx);
    // The app-level pages (extensions, sessions) count as open panels here.
    let empty = frame.panels.dock_empty()
        && !crate::mods_ui::page_open(cx)
        && !crate::sessions_ui::page_open(cx);
    view::body(
        view::Body {
            column,
            variant,
            warning: frame.warning(),
            hook_banner,
            dock: ws.dock.clone().into_any_element(),
            empty,
            toasts,
            actions,
            review,
        },
        cx,
    )
}

/// Fixture scene `queue`: a pending gate on the blocked fixture-1, a question on
/// fixture-3, four panels in a grid, the gate card selected (visual QA only).
pub fn fixture_scene(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let now = crate::orch::now_ms();
    w.orch.merge_snapshot(vec![GateInfo {
        id: "g1".into(),
        title: "명령 실행 승인".into(),
        message: "$ cargo clippy -p pod-ui -- -D warnings".into(),
        task: Some("sample/task-000".into()),
        group: None,
        terminal: Some("fixture-1".into()),
        state: GateState::Pending,
        created_ms: now.saturating_sub(12_000),
        resolved_ms: None,
        note: None,
        expires_ms: None,
        run: None,
        item: None,
        options: Vec::new(),
        choice: None,
        requested_by: None,
        decided_by: None,
    }]);
    if let Some(s) = w
        .terminals
        .iter_mut()
        .find(|t| t.id == "fixture-3")
        .and_then(|t| t.status.as_mut())
    {
        s.kind = Some("question".into());
    }
    for id in ["fixture-0", "fixture-1", "fixture-2", "fixture-3"] {
        w.open_terminal(id, false, window, cx);
    }
    w.arrange_grid(window, cx);
    w.touch(cx);
    let col = column(w, window, cx);
    col.update(cx, |q, cx| {
        q.selected = Some("gate:g1".into());
        q.focus_list(window, cx);
    });
}

#[cfg(test)]
mod tests;
