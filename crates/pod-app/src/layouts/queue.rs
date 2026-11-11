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
//! The data is the host's (terminals, tasks, gates, labels; the gate decision
//! path through [`LayoutActions::resolve_gate`]). The queue keeps view state
//! only (selection, expanded sections, filter, dismissed failures) in one
//! [`QueueColumn`] per window, in the kit's state slot, so it survives layout
//! switches. It re-renders on the host's change notifications; no timer.
//!
//! Files (docs/20-refactor/app.md A-P10): `model` holds the pure queue,
//! selection and gate-button plans, `view` the render, `copy` the strings,
//! `fixture` the visual-QA scene. This file keeps the layout module and its
//! narrow renderer and the column with its controller methods. Since A-T2b
//! the queue is on the layout kit only: the frame for reads, the host's slots
//! for the dock, toasts and hook prompt, and [`LayoutActions`] for the rest.

pub mod copy;
mod fixture;
mod model;
mod view;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use podd::protocol::GateDecision;

pub use self::fixture::fixture_scene;
pub use self::model::{QueueModel, Section, Variant, age, build};

use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{
    self, HostChange, LayoutActions, LayoutFrame, LayoutHost, LayoutRenderer, Slot, UiIntent,
};

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

/// The queue for a frame, with the terminals' saved labels.
fn model_of(f: &LayoutFrame<'_>, query: &str, dismissed: &HashSet<(String, u64)>) -> QueueModel {
    build(
        f.terminals,
        f.tasks,
        f.gates,
        &|t| f.label_of(t),
        query,
        dismissed,
    )
}

// ───────────────────────────── the column view ─────────────────────────────

/// View state of the queue column (one per window, kept across layout switches).
pub struct QueueColumn {
    /// The host: reads (frame), change observation, the gate decision path
    /// and UI intents.
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

/// The window's queue column (the kit's state slot), made on first use: by
/// the body, or by a title-bar pill before the body showed it.
pub(crate) fn column(
    actions: &LayoutActions,
    window: &mut Window,
    cx: &mut App,
) -> Entity<QueueColumn> {
    actions.state_or_insert_with(window, cx, |window, cx| {
        let actions = actions.clone();
        cx.new(|cx| QueueColumn::new(actions, window, cx))
    })
}

struct KeysBound;
impl Global for KeysBound {}

impl QueueColumn {
    fn new(actions: LayoutActions, window: &mut Window, cx: &mut Context<Self>) -> Self {
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
                        // The column of this window, else the only one.
                        let col = kit::state_in_window::<QueueColumn>(window.window_handle(), cx)
                            .or_else(|| {
                                let cols = kit::states::<QueueColumn>(cx);
                                (cols.len() == 1).then(|| cols[0].clone())
                            });
                        if let Some(col) = col {
                            col.update(cx, |q, cx| q.focus_list(window, cx));
                        }
                    });
                });
            });
            cx.set_global(KeysBound);
        }
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder(copy::SEARCH));
        // Tasks, terminals, statuses and gates all notify the host: that is the only refresh.
        let mut subs: Vec<Subscription> = actions
            .observe(&[HostChange::Data], cx, |_, _, cx| cx.notify())
            .into_iter()
            .collect();
        subs.push(cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify()));
        Self {
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
        self.actions
            .read(cx, |f| model_of(f, &query, &self.dismissed))
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
        if let Some(t) = terminal {
            self.actions
                .dispatch(UiIntent::RevealTerminal { id: t.to_owned() }, window, cx);
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

    /// 이번만 허용 / 거부 through the host's one gate decision path
    /// (`gates::resolve_gate`: the answer merged, or a failure toast); the
    /// completion clears the card's busy state or keeps its error.
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
        // No host (its window is gone): nothing to decide with.
        if self.actions.source(cx).is_none() {
            return;
        }
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
        self.actions.resolve_gate(params, cx, move |r, cx| {
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

    fn status_counts(&self, cx: &App) -> bool {
        Variant::current(cx) != Variant::Mission
    }

    fn renderer(&self) -> Rc<dyn LayoutRenderer> {
        Rc::new(Renderer)
    }
}

/// The queue's narrow renderer: the body, and the 06 title-bar pills.
struct Renderer;

impl LayoutRenderer for Renderer {
    /// The window body: [activity bar] · queue column (+ toasts) · dock.
    fn render_body(
        &self,
        frame: &LayoutFrame<'_>,
        host: &mut dyn LayoutHost,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let actions = host.actions();
        let column = column(&actions, window, cx);
        let variant = Variant::current(cx);
        let toasts = host
            .slot(Slot::Toasts, window, cx)
            .unwrap_or_else(|| div().into_any_element());
        let hook_banner = host.slot(Slot::HookBanner, window, cx);
        let dock = host
            .slot(Slot::Dock, window, cx)
            .unwrap_or_else(|| div().into_any_element());
        // The app-level pages (extensions, sessions) count as open panels here.
        let empty = frame.panels.dock_empty()
            && !frame.panel_detail.mods_page
            && !frame.panel_detail.sessions_page;
        view::body(
            view::Body {
                column,
                variant,
                warning: frame.warning(),
                hook_banner,
                dock,
                empty,
                toasts,
                actions,
            },
            cx,
        )
    }

    /// 06: status pills in the title bar; each jumps to its section. A pill
    /// makes the column when the body never showed it.
    fn title_bar_extra(
        &self,
        frame: &LayoutFrame<'_>,
        actions: &LayoutActions,
        cx: &mut App,
    ) -> Option<AnyElement> {
        (Variant::current(cx) == Variant::Mission).then(|| {
            let counts = model_of(frame, "", &HashSet::new()).counts;
            let actions = actions.clone();
            let open: view::OpenSection = Rc::new(move |section, window, cx| {
                let col = column(&actions, window, cx);
                col.update(cx, |q, cx| q.focus_section(section, window, cx));
            });
            view::title_pills(counts, open)
        })
    }
}

#[cfg(test)]
mod tests;
