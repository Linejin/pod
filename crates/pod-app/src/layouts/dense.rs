//! 고밀도 + 세부 열 layout (preset 07 "고밀도 편집기", docs/10 §D5.10).
//!
//! Everything for supervising many agents on one screen:
//! activity bar · the workspace's own task tree · the workspace's own dock,
//! with an **출력** panel under it (status changes, gates, exits and
//! notifications as a compact log) · a **세부 정보** column on the right for
//! the selected task or agent (branch, worktree, status history, last lines,
//! PR, actions).
//!
//! - **No forked state.** Tree, dock, terminals, tasks, gates and toasts are
//!   the workspace's. The layout keeps view state only ([`DenseView`]: panel
//!   visibility, selection, the event log it recorded, a last-lines cache),
//!   one per workspace in a global, so it survives layout switches.
//! - **The log is recorded, not polled.** [`EventLog::observe`] diffs the
//!   workspace's data against the previous snapshot on every workspace change
//!   notification; the first observation seeds it with what needs attention.
//! - **Switching is live.** The dock entity is the one every layout renders:
//!   no panel is created, dropped or re-attached by switching.
//! - **Density from the style.** Rows, header heights, sidebar width, UI font
//!   and radii come from `pod_ui::themes::metrics()`; row heights grow with
//!   the user's font size so Korean text is never clipped.
//!
//! Files (docs/20-refactor/app.md A-P11): `log` holds the event log model,
//! `model` the selection, keyboard, size and action plans, `view` the render,
//! `copy` the strings. This file keeps the layout module, the per-window
//! [`DenseView`] with its controller methods and the shortcuts. Reads go
//! through the kit's frame (tasks, terminals, focused terminal, pending gates,
//! open panels, fonts) and UI actions through [`LayoutActions`] (open a
//! terminal, review, files or PR, the activity bar's commands, slot focus);
//! what the kit does not carry yet stays a small legacy adapter over the
//! workspace: observing it and its tree, the tree's selected row, every gate
//! seen, the toasts, terminal labels, the podd source (last lines, gate
//! decisions, 다시 실행), the terminal panels (F6, bringing one forward) and
//! adding a re-run terminal.
pub mod copy;
mod log;
mod model;
mod view;

use std::collections::{BTreeMap, HashMap, HashSet};

use gpui_kit::component::dock::PanelId;
use gpui_kit::*;
use podd::protocol::{self as p, GateDecision, GateInfo, GateState};

pub use self::log::{EventLog, Inputs, LogEvent, ToastIn};
// Named at its old path by the kit's tests (`layouts::kit::tests`).
#[cfg(test)]
pub(crate) use self::model::row_height;
use self::model::{LogKey, Panel, Region, Sel};
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{Clock, LayoutActions, LayoutFrame, Slot, UiIntent};
use crate::source::PoddSource;
use crate::vm::{AgentState, Liveness, TerminalVm};
use crate::workspace::{ToastKind, Workspace};

actions!(
    pod_dense,
    [NextRegion, PrevRegion, FocusOutput, FocusDetails]
);

/// Key context of the layout body (its shortcuts work only here).
pub const KEY_CONTEXT: &str = "PodDense";
/// Focus the 출력 log (shows the panel when hidden).
pub const FOCUS_OUTPUT_KEY: &str = "ctrl-shift-u";
/// Focus the 세부 정보 column (shows it when hidden).
pub const FOCUS_DETAILS_KEY: &str = "ctrl-shift-d";
/// `--ui-fixture --fixture-scene dense`: gate on fixture-1, four panels, fixture-1 selected.
pub const FIXTURE_SCENE: &str = "dense";
/// The `--ui-fixture` clock of this layout (Unix ms): the orchestration
/// fixture's frozen instant, 2026-11-10 09:30:00 UTC. The 출력 log and
/// 상태 기록 show `HH:MM:SS`, so captures need one fixed "now".
pub const FIXTURE_NOW: u64 = 1_794_303_000_000;
/// Set once by [`freeze_fixture_clock`] (fixture runs only); 0 = the system clock.
static FIXTURE_CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `--ui-fixture` only, before the window opens: views created from now on
/// read [`FIXTURE_NOW`] instead of the system clock. Real runs never call it.
pub fn freeze_fixture_clock() {
    FIXTURE_CLOCK.store(FIXTURE_NOW, std::sync::atomic::Ordering::Relaxed);
}

/// The clock a new [`DenseView`] reads: frozen in fixture runs, else the system's.
fn fixture_clock() -> Clock {
    Clock::frozen_or_system(FIXTURE_CLOCK.load(std::sync::atomic::Ordering::Relaxed))
}
/// Visible rows of the 출력 panel.
pub const LOG_ROWS: usize = 6;
/// Lines read for 마지막 줄.
pub const LAST_LINES: usize = 3;
/// Entries of 상태 기록.
pub const HISTORY_ROWS: usize = 6;

/// UI font of the style at the user's font size: the kit's
/// `LayoutMetrics::ui_font`, which the render reads from the frame.
#[cfg(test)]
pub fn ui_font(cx: &App) -> f32 {
    crate::layouts::kit::LayoutMetrics::current(cx).ui_font
}

/// A pending `terminal.read` for 마지막 줄.
struct LineRead {
    id: String,
    key: String,
    source: PoddSource,
}

// ───────────────────────────── view state ─────────────────────────────

/// View state of the layout (one per workspace, kept across layout switches).
pub struct DenseView {
    /// Legacy adapter: observed for changes and read for what the kit does
    /// not carry (see the file comment).
    ws: WeakEntity<Workspace>,
    /// Reads (frame), slot focus and UI intents of the host.
    actions: LayoutActions,
    pub(crate) log: EventLog,
    pub(crate) selected: Option<Sel>,
    seen_sidebar: Option<String>,
    seen_focus: Option<String>,
    pub(crate) log_focus: FocusHandle,
    pub(crate) details_focus: FocusHandle,
    /// Selected log line (`LogEvent::seq`).
    pub(crate) log_sel: Option<u64>,
    log_scroll: UniformListScrollHandle,
    /// Lines before this `seq` are hidden (출력 지우기).
    cleared: u64,
    pub(crate) show_tree: bool,
    pub(crate) show_output: bool,
    pub(crate) show_details: bool,
    lines: HashMap<String, (String, Vec<String>)>,
    reading: HashSet<String>,
    busy: HashMap<String, GateDecision>,
    errors: HashMap<String, String>,
    /// "Now" of the log lines and the details column (frozen in fixture runs).
    pub(crate) clock: Clock,
    _subs: Vec<Subscription>,
}

#[derive(Default)]
struct DenseViews(HashMap<EntityId, Entity<DenseView>>);
impl Global for DenseViews {}

/// The workspace's view state, created on first use.
pub(crate) fn view(ws: &Workspace, cx: &mut Context<Workspace>) -> Entity<DenseView> {
    let id = cx.entity_id();
    if let Some(v) = cx
        .try_global::<DenseViews>()
        .and_then(|m| m.0.get(&id))
        .cloned()
    {
        return v;
    }
    let me = cx.entity().downgrade();
    let sidebar = ws.sidebar.clone();
    let actions = ws.layout_actions(cx);
    let v = cx.new(|cx| DenseView::new(me, &sidebar, actions, cx));
    // The body is drawn by the workspace: redraw it when the view changes.
    let sub = cx.observe(&v, |_, _, cx| cx.notify());
    v.update(cx, |v, _| v._subs.push(sub));
    cx.default_global::<DenseViews>().0.insert(id, v.clone());
    v
}

/// The view if the layout was ever shown.
#[cfg(test)]
pub(crate) fn existing_view(ws: EntityId, cx: &App) -> Option<Entity<DenseView>> {
    cx.try_global::<DenseViews>()
        .and_then(|m| m.0.get(&ws))
        .cloned()
}

impl DenseView {
    fn new(
        ws: WeakEntity<Workspace>,
        sidebar: &Entity<crate::workspace::Sidebar>,
        actions: LayoutActions,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = Vec::new();
        if let Some(w) = ws.upgrade() {
            // Tasks, terminals, statuses, gates and toasts all notify the workspace: the only refresh.
            subs.push(cx.observe(&w, |v, w, cx| {
                let (changed, read) = {
                    let w = w.read(cx);
                    let pending = w.orch.pending();
                    let frame = w.layout_frame(&pending, cx);
                    let label = |t: &TerminalVm| w.label_of(t);
                    v.absorb(&frame, ws_reads(w, &label, cx))
                };
                v.finish(changed, read, cx);
            }));
        }
        // The tree's selection drives the details column.
        subs.push(cx.observe(sidebar, |v, s, cx| {
            let key = s.read(cx).selected.clone();
            if key != v.seen_sidebar {
                v.seen_sidebar = key.clone();
                if let Some(sel) = key.as_deref().and_then(Sel::from_sidebar_key) {
                    v.selected = Some(sel);
                    cx.notify();
                }
            }
        }));
        Self {
            ws,
            actions,
            log: EventLog::default(),
            selected: None,
            seen_sidebar: None,
            seen_focus: None,
            log_focus: cx.focus_handle().tab_stop(true),
            details_focus: cx.focus_handle().tab_stop(true),
            log_sel: None,
            log_scroll: UniformListScrollHandle::new(),
            cleared: 0,
            show_tree: true,
            show_output: true,
            show_details: true,
            lines: HashMap::new(),
            reading: HashSet::new(),
            busy: HashMap::new(),
            errors: HashMap::new(),
            clock: fixture_clock(),
            _subs: subs,
        }
    }

    /// Record the workspace's changes and follow its focused terminal.
    fn sync(&mut self, f: &LayoutFrame<'_>, ws: WsReads<'_>, cx: &mut Context<Self>) {
        let (changed, read) = self.absorb(f, ws);
        self.finish(changed, read, cx);
    }

    /// [`Self::sync`] without touching the context (the workspace may be borrowed from it).
    fn absorb(&mut self, f: &LayoutFrame<'_>, ws: WsReads<'_>) -> (bool, Option<LineRead>) {
        let mut changed = false;
        if f.focused_terminal != self.seen_focus.as_deref() {
            self.seen_focus = f.focused_terminal.map(str::to_owned);
            if let Some(t) = &self.seen_focus {
                self.selected = Some(Sel::Terminal(t.clone()));
                changed = true;
            }
        }
        if self.selected.is_none()
            && let Some(sel) = ws.sidebar_key.as_deref().and_then(Sel::from_sidebar_key)
        {
            self.selected = Some(sel);
            changed = true;
        }
        let inputs = Inputs {
            terminals: f.terminals,
            tasks: f.tasks,
            gates: ws.gates,
            toasts: ws.toasts,
            label: ws.label,
        };
        let at_tail = model::at_tail(self.log_sel, self.log.events().next_back().map(|e| e.seq));
        if self.log.observe(&inputs, self.clock.now_ms()) {
            changed = true;
            if at_tail {
                // Follow the newest line unless the user is reading older ones.
                self.log_scroll.scroll_to_item(
                    self.visible_events().saturating_sub(1),
                    ScrollStrategy::Bottom,
                );
            }
        }
        (changed, self.line_read(f, ws.source))
    }

    fn finish(&mut self, changed: bool, read: Option<LineRead>, cx: &mut Context<Self>) {
        if let Some(r) = read {
            self.start_read(r, cx);
        }
        if changed {
            cx.notify();
        }
    }

    fn visible_events(&self) -> usize {
        self.log.events().filter(|e| e.seq >= self.cleared).count()
    }

    fn visible(&self) -> Vec<LogEvent> {
        self.log
            .events()
            .filter(|e| e.seq >= self.cleared)
            .cloned()
            .collect()
    }

    /// The selected terminal's last lines need a read when its status key changed.
    fn line_read(&self, f: &LayoutFrame<'_>, source: &PoddSource) -> Option<LineRead> {
        let Some(Sel::Terminal(id)) = &self.selected else {
            return None;
        };
        let t = f.terminal(id)?;
        let key = model::line_key(t);
        if self.reading.contains(id) || self.lines.get(id).is_some_and(|(k, _)| *k == key) {
            return None;
        }
        Some(LineRead {
            id: id.clone(),
            key,
            source: source.clone(),
        })
    }

    fn start_read(&mut self, r: LineRead, cx: &mut Context<Self>) {
        let LineRead { id, key, source } = r;
        if !self.reading.insert(id.clone()) {
            return;
        }
        let rx = source.call::<p::TerminalRead>(&p::ReadParams {
            target: p::TerminalRef::id(id.clone()),
            mode: p::ReadMode::Screen,
            lines: Some(LAST_LINES + 4),
            raw: false,
        });
        cx.spawn(async move |this, cx| {
            let lines: Vec<String> = rx
                .await
                .ok()
                .map(|r| {
                    r.lines
                        .into_iter()
                        .map(|l| l.trim_end().to_string())
                        .filter(|l| !l.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let start = lines.len().saturating_sub(LAST_LINES);
            let _ = this.update(cx, |v, cx| {
                v.reading.remove(&id);
                v.lines.insert(id, (key, lines[start..].to_vec()));
                cx.notify();
            });
        })
        .detach();
    }

    #[cfg(test)]
    pub(crate) fn last_lines(&self, id: &str) -> Option<&[String]> {
        self.lines.get(id).map(|(_, l)| l.as_slice())
    }

    pub(crate) fn select(&mut self, sel: Sel, cx: &mut Context<Self>) {
        self.selected = Some(sel);
        // Legacy adapter: the podd source.
        let read = self.ws.upgrade().and_then(|w| {
            let w = w.read(cx);
            self.actions
                .read(cx, |f| self.line_read(f, &w.source))
                .flatten()
        });
        self.finish(true, read, cx);
    }

    /// The regions in F6 order that are shown now, with the handle to focus.
    fn regions(&self, window: &Window, cx: &App) -> Vec<(Region, FocusHandle, bool)> {
        // Legacy adapter: the terminal panels and their views.
        let Some(ws) = self.ws.upgrade() else {
            return Vec::new();
        };
        let w = ws.read(cx);
        let (Some(tree), Some(dock)) = (
            self.actions.focus_handle(Slot::Sidebar, cx),
            self.actions.focus_handle(Slot::Dock, cx),
        ) else {
            return Vec::new();
        };
        let mut r = Vec::new();
        if self.show_tree {
            let inside = tree.contains_focused(window, cx);
            r.push((Region::Tree, tree, inside));
        }
        let dock_in = dock.contains_focused(window, cx);
        // Prefer the terminal the user worked in last.
        let panels = w.live_panels();
        let term = w
            .focused_terminal
            .as_ref()
            .and_then(|id| panels.iter().find(|p| &p.read(cx).terminal_id == id))
            .or(panels.first())
            .map(|p| p.read(cx).view.read(cx).focus_handle(cx));
        let term_in = panels.iter().any(|p| {
            p.read(cx)
                .view
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        });
        r.push((Region::Dock, term.unwrap_or(dock), dock_in || term_in));
        if self.show_output {
            r.push((
                Region::Output,
                self.log_focus.clone(),
                self.log_focus.contains_focused(window, cx),
            ));
        }
        if self.show_details {
            r.push((
                Region::Details,
                self.details_focus.clone(),
                self.details_focus.contains_focused(window, cx),
            ));
        }
        r
    }

    /// F6 / Shift+F6: the next / previous region.
    pub(crate) fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let regions = self.regions(window, cx);
        if regions.is_empty() {
            return;
        }
        let at = regions.iter().position(|(_, _, inside)| *inside);
        let next = model::cycle_step(at, regions.len(), forward);
        self.focus_region(regions[next].0, regions[next].1.clone(), window, cx);
    }

    fn focus_region(
        &mut self,
        r: Region,
        f: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if r == Region::Output && self.log_sel.is_none() {
            self.log_sel = self.log.events().next_back().map(|e| e.seq);
        }
        f.focus(window, cx);
        cx.notify();
    }

    /// Whether a panel of the layout is shown.
    fn shows(&self, panel: Panel) -> bool {
        match panel {
            Panel::Tree => self.show_tree,
            Panel::Output => self.show_output,
            Panel::Details => self.show_details,
        }
    }

    /// The activity bar's toggles; a tree shown again takes the keyboard.
    fn toggle(&mut self, panel: Panel, window: &mut Window, cx: &mut Context<Self>) {
        match panel {
            Panel::Tree => {
                self.show_tree = !self.show_tree;
                if self.show_tree
                    && let Some(f) = self.actions.focus_handle(Slot::Sidebar, cx)
                {
                    f.focus(window, cx);
                }
            }
            Panel::Output => self.show_output = !self.show_output,
            Panel::Details => self.show_details = !self.show_details,
        }
    }

    pub(crate) fn focus_output(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_output = true;
        self.focus_region(Region::Output, self.log_focus.clone(), window, cx);
    }

    pub(crate) fn focus_details(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_details = true;
        self.focus_region(Region::Details, self.details_focus.clone(), window, cx);
    }

    /// Keyboard of the log: ↑↓ Home End PageUp PageDown select, Enter shows the line's agent.
    fn on_log_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let events = self.visible();
        let at = self
            .log_sel
            .and_then(|s| events.iter().position(|e| e.seq == s));
        match model::log_key(e.keystroke.key.as_str(), at, events.len()) {
            Some(LogKey::Select(i)) => {
                self.log_sel = Some(events[i].seq);
                self.log_scroll.scroll_to_item(i, ScrollStrategy::Nearest);
                cx.notify();
                cx.stop_propagation();
            }
            Some(LogKey::Activate { line, open }) => {
                if let Some(ev) = line.map(|i| events[i].clone()) {
                    self.activate_event(&ev, open, window, cx);
                }
                cx.stop_propagation();
            }
            None => {}
        }
    }

    /// A log line: its agent (or task) into the details column, its panel brought forward.
    /// `open` (Enter) also opens the terminal and moves focus there.
    fn activate_event(
        &mut self,
        ev: &LogEvent,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.log_sel = Some(ev.seq);
        let sel = ev
            .terminal
            .clone()
            .map(Sel::Terminal)
            .or_else(|| ev.task.clone().map(Sel::Task));
        let Some(sel) = sel else {
            cx.notify();
            return;
        };
        self.select(sel.clone(), cx);
        let Sel::Terminal(t) = sel else { return };
        if open {
            self.actions.dispatch(
                UiIntent::OpenTerminal {
                    id: t,
                    another: false,
                },
                window,
                cx,
            );
        } else if let Some(ws) = self.ws.upgrade() {
            // Legacy adapter: bringing a terminal's panel forward in the dock.
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
    }

    /// 이번만 허용 / 거부 through the one gate decision path.
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
        // Legacy adapter: the podd source and the gate controller's workspace.
        let Some(ws) = self.ws.upgrade() else { return };
        let source = ws.read(cx).source.clone();
        self.busy.insert(gate.clone(), decision);
        self.errors.remove(&gate);
        // The buttons go away when podd answers: keep the keyboard in the column.
        self.details_focus.focus(window, cx);
        let me = cx.entity().downgrade();
        let id = gate.clone();
        let params = p::GateResolveParams {
            id: gate,
            decision,
            note: None,
            choice,
        };
        crate::gates::resolve_gate(&source, self.ws.clone(), params, cx, move |r, cx| {
            let _ = me.update(cx, |v, cx| {
                v.busy.remove(&id);
                if let Err(e) = r {
                    v.errors.insert(id, e.text);
                }
                cx.notify();
            });
        });
        cx.notify();
    }

    /// 다시 실행: the same command in the same folder and task (`terminal.run`).
    fn rerun(&mut self, id: &str, cx: &mut Context<Self>) {
        // Legacy adapter: the podd source, adding the new terminal and the
        // error toast (no window here for an intent).
        let Some(ws) = self.ws.upgrade() else { return };
        let Some(t) = self.actions.read(cx, |f| f.terminal(id).cloned()).flatten() else {
            return;
        };
        let rx = ws.read(cx).source.run(model::rerun_spec(&t));
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |v, cx| {
                match r {
                    Ok(Ok(new)) => {
                        ws.update(cx, |w, cx| {
                            if !w.terminals.iter().any(|x| x.id == new.id) {
                                w.terminals.push(new.clone());
                            }
                            cx.notify();
                        });
                        v.selected = Some(Sel::Terminal(new.id));
                    }
                    Ok(Err(e)) => ws.update(cx, |w, cx| {
                        w.raise_toast(ToastKind::Error, copy::COULD_NOT_RERUN, e.message, cx)
                    }),
                    Err(_) => ws.update(cx, |w, cx| {
                        w.raise_toast(
                            ToastKind::Error,
                            copy::COULD_NOT_RERUN,
                            crate::strings::PODD_CLIENT_STOPPED,
                            cx,
                        )
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

// ───────────────────────────── layout module ─────────────────────────────

/// The 고밀도 + 세부 열 layout module (preset 07).
pub struct DenseLayout;

impl LayoutModule for DenseLayout {
    fn mode(&self) -> LayoutMode {
        LayoutMode::DenseDetails
    }

    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        render_body(ws, window, cx)
    }
}

/// Region and panel shortcuts. F6 is bound only inside the layout (terminal
/// apps keep F6 elsewhere); Ctrl+Shift+U/D work from anywhere in the window
/// (the workspace root holds focus outside the layout) while it is shown.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f6", NextRegion, Some(KEY_CONTEXT)),
        KeyBinding::new("shift-f6", PrevRegion, Some(KEY_CONTEXT)),
        KeyBinding::new(FOCUS_OUTPUT_KEY, FocusOutput, None),
        KeyBinding::new(FOCUS_DETAILS_KEY, FocusDetails, None),
    ]);
    cx.on_action(|_: &FocusOutput, cx: &mut App| {
        with_active_view(cx, |v, window, cx| v.focus_output(window, cx))
    });
    cx.on_action(|_: &FocusDetails, cx: &mut App| {
        with_active_view(cx, |v, window, cx| v.focus_details(window, cx))
    });
}

/// Run `f` on the view of the active window's workspace when this layout is shown.
fn with_active_view(cx: &mut App, f: fn(&mut DenseView, &mut Window, &mut Context<DenseView>)) {
    let Some(window) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = window.update(cx, |_, window, cx| {
            let layout = cx
                .try_global::<crate::theme::Settings>()
                .map(|s| s.layout.clone())
                .unwrap_or_default();
            if layout != LayoutMode::DenseDetails.id() {
                return;
            }
            let ws = cx
                .try_global::<crate::workspace::AppGlobals>()
                .map(|g| g.workspace.entity_id());
            let Some(v) = ws.and_then(|id| {
                cx.try_global::<DenseViews>()
                    .and_then(|m| m.0.get(&id).cloned())
            }) else {
                return;
            };
            v.update(cx, |v, cx| f(v, window, cx));
        });
    });
}

// ───────────── legacy adapter: workspace → frame, reads and view ─────────────

/// What one observation reads from the workspace beside the frame: the kit
/// does not carry these yet.
struct WsReads<'a> {
    /// Every gate seen this session (`OrchState::gates`); the frame has only
    /// the pending ones.
    gates: &'a BTreeMap<String, GateInfo>,
    toasts: Vec<ToastIn>,
    /// Terminal labels (`Workspace::label_of`).
    label: &'a dyn Fn(&TerminalVm) -> String,
    /// The tree's selected row key.
    sidebar_key: Option<String>,
    /// For 마지막 줄 (`terminal.read`).
    source: &'a PoddSource,
}

fn ws_reads<'a>(
    w: &'a Workspace,
    label: &'a dyn Fn(&TerminalVm) -> String,
    cx: &App,
) -> WsReads<'a> {
    WsReads {
        gates: &w.orch.gates,
        toasts: toasts_of(w),
        label,
        sidebar_key: w.sidebar.read(cx).selected.clone(),
        source: &w.source,
    }
}

fn toasts_of(w: &Workspace) -> Vec<ToastIn> {
    w.toasts
        .iter()
        .map(|t| ToastIn {
            id: t.id,
            kind: t.kind,
            title: t.title.to_string(),
            body: t.body.to_string(),
        })
        .collect()
}

/// [`DenseView::sync`] from inside a workspace update (render, fixture scene).
fn sync_view(
    v: &Entity<DenseView>,
    w: &Workspace,
    frame: &LayoutFrame<'_>,
    cx: &mut Context<Workspace>,
) {
    let label = |t: &TerminalVm| w.label_of(t);
    let reads = ws_reads(w, &label, cx);
    v.update(cx, |v, cx| v.sync(frame, reads, cx));
}

/// The window body through `LayoutModule::render_body`: the frame, the
/// host's slots and the view state go to the render.
fn render_body(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) -> AnyElement {
    let view = view(ws, cx);
    let pending = ws.orch.pending();
    let frame = ws.layout_frame(&pending, cx);
    // First frame of a fresh view: record the current state (later changes arrive by observation).
    if view.read(cx).log.is_empty() {
        sync_view(&view, ws, &frame, cx);
    }
    // The app-level pages (extensions, sessions) count as open panels here.
    let empty = frame.panels.dock_empty()
        && !crate::mods_ui::page_open(cx)
        && !crate::sessions_ui::page_open(cx);
    let toasts = ws.render_toasts(cx).into_any_element();
    let hook_banner = ws.render_hook_banner(cx);
    let label = |t: &TerminalVm| ws.label_of(t);
    view::body(
        view::Body {
            view,
            warning: frame.warning(),
            hook_banner,
            sidebar: ws.sidebar.clone().into_any_element(),
            dock: ws.dock.clone().into_any_element(),
            toasts,
            empty,
        },
        &frame,
        &label,
        window,
        cx,
    )
}

/// Fixture scene `dense`: a pending gate on the blocked fixture-1, fixture-3
/// asks a question, four panels in a grid, fixture-1 in the details column,
/// a few status changes and a notification in the log (visual QA only).
pub fn fixture_scene(w: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    for id in ["fixture-0", "fixture-1", "fixture-2", "fixture-3"] {
        w.open_terminal(id, false, window, cx);
    }
    w.arrange_grid(window, cx);
    // The details column follows the focused terminal: end on fixture-1.
    w.open_terminal("fixture-1", false, window, cx);
    let v = view(w, cx);
    {
        let pending = w.orch.pending();
        let frame = w.layout_frame(&pending, cx);
        sync_view(&v, w, &frame, cx);
    }
    let now = v.read(cx).clock.now_ms();
    // Live changes after the log started.
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
        s.state = AgentState::Blocked;
        s.kind = Some("question".into());
    }
    if let Some(t) = w.terminals.iter_mut().find(|t| t.id == "fixture-2") {
        t.live = Liveness::Exited {
            code: Some(1),
            killed: false,
        };
    }
    w.raise_toast(
        ToastKind::Success,
        "Worker 2 · 작업 완료",
        "1분 12초 · 토큰 18.2k",
        cx,
    );
    {
        let pending = w.orch.pending();
        let frame = w.layout_frame(&pending, cx);
        let label = |t: &TerminalVm| w.label_of(t);
        v.update(cx, |v, cx| {
            // Not `select`: that reads the workspace, which this scene is updating.
            v.selected = Some(Sel::Terminal("fixture-1".into()));
            let reads = ws_reads(w, &label, cx);
            v.sync(&frame, reads, cx);
        });
    }
    w.touch(cx);
}

#[cfg(test)]
mod tests;
