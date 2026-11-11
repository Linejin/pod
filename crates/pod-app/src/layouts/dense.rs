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
//!   one per window in the kit's state slot, so it survives layout switches.
//! - **The log is recorded, not polled.** [`EventLog::observe`] diffs the
//!   window's data against the previous snapshot on every host change
//!   notification; the first observation seeds it with what needs attention.
//! - **Switching is live.** The dock entity is the one every layout renders:
//!   no panel is created, dropped or re-attached by switching.
//! - **Density from the style.** Rows, header heights, sidebar width, UI font
//!   and radii come from `pod_ui::themes::metrics()`; row heights grow with
//!   the user's font size so Korean text is never clipped.
//!
//! Files (docs/20-refactor/app.md A-P11): `log` holds the event log model,
//! `model` the selection, keyboard, size and action plans, `view` the render,
//! `copy` the strings, `fixture` the layout's half of the visual-QA scene.
//! This file keeps the layout module with its narrow renderer, the per-window
//! [`DenseView`] with its controller methods and the shortcuts.
//!
//! A-T2c: the body is drawn through `LayoutModule::renderer` and the view
//! reaches the window only through the kit: the frame (tasks, terminals, the
//! focused terminal, pending and all gates, toasts, labels, the tree's
//! selected row, open panels, fonts), the host's slots (tree, dock, toasts,
//! hook banner) and [`LayoutActions`] (the view as the window's state slot,
//! change observation of the host and its tree, slot focus, the terminal
//! panels, UI intents, the podd source for 마지막 줄 and 다시 실행, the gate
//! path, window-less updates). Since T2 nothing here names the workspace:
//! `fixture_scenes::dense` sets the scene's data up and lends a frame.
pub mod copy;
mod fixture;
mod log;
mod model;
mod view;

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::*;
use podd::protocol::{self as p, GateDecision};

pub(crate) use self::fixture::{fixture_select, fixture_view};
pub use self::log::{EventLog, Inputs, LogEvent, ToastIn};
// Named at its old path by the kit's tests (`layouts::kit::tests`).
#[cfg(test)]
pub(crate) use self::model::row_height;
use self::model::{LogKey, Panel, Region, Sel};
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::layouts::kit::{
    self, Clock, HostChange, HostUpdate, LayoutActions, LayoutFrame, LayoutHost, LayoutRenderer,
    Slot, UiIntent,
};
use crate::vm::TerminalVm;
use crate::workspace::ToastKind;

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
}

// ───────────────────────────── view state ─────────────────────────────

/// View state of the layout (one per window, kept across layout switches).
pub struct DenseView {
    /// The host: frame reads, change observation, slot focus, panels,
    /// intents, podd and updates.
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

/// The window's view state, made on first use; it lives as long as the
/// window's host (the kit's state slot).
fn view(actions: &LayoutActions, window: &mut Window, cx: &mut App) -> Entity<DenseView> {
    actions.state_or_insert_with(window, cx, |_, cx| {
        let v = cx.new(|cx| DenseView::new(actions.clone(), cx));
        // The body is drawn by the host: redraw it when the view changes.
        let host = actions.clone();
        let sub = cx.observe(&v, move |_, cx| {
            host.post(vec![HostUpdate::Notify], cx);
        });
        v.update(cx, |v, _| v._subs.push(sub));
        v
    })
}

/// The view if the layout was ever shown in the window of host `ws`.
#[cfg(test)]
pub(crate) fn existing_view(ws: EntityId, cx: &App) -> Option<Entity<DenseView>> {
    kit::state::<DenseView>(kit::WindowKey(ws), cx)
}

impl DenseView {
    fn new(actions: LayoutActions, cx: &mut Context<Self>) -> Self {
        // Tasks, terminals, statuses, gates and toasts all notify the host:
        // the only refresh. The tree's selection drives the details column.
        let subs = actions
            .observe(
                &[HostChange::Data, HostChange::Tree],
                cx,
                |v, change, cx| match change {
                    HostChange::Data => v.refresh(cx),
                    HostChange::Tree => v.follow_tree(cx),
                    HostChange::DockLayout => {}
                },
            )
            .into_iter()
            .collect();
        Self {
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

    /// Record the window's changes and follow its focused terminal.
    fn sync(&mut self, f: &LayoutFrame<'_>, cx: &mut Context<Self>) {
        let (changed, read) = self.absorb(f);
        self.finish(changed, read, cx);
    }

    /// [`Self::sync`] on a fresh frame after the host changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let actions = self.actions.clone();
        if let Some((changed, read)) = actions.read(cx, |f| self.absorb(f)) {
            self.finish(changed, read, cx);
        }
    }

    /// The tree's selected row into the details column when it changed.
    fn follow_tree(&mut self, cx: &mut Context<Self>) {
        let key = self
            .actions
            .read(cx, |f| f.tree_selection.clone())
            .flatten();
        if key != self.seen_sidebar {
            self.seen_sidebar = key.clone();
            if let Some(sel) = key.as_deref().and_then(Sel::from_sidebar_key) {
                self.selected = Some(sel);
                cx.notify();
            }
        }
    }

    /// [`Self::sync`] without touching the context.
    fn absorb(&mut self, f: &LayoutFrame<'_>) -> (bool, Option<LineRead>) {
        let mut changed = false;
        if f.focused_terminal != self.seen_focus.as_deref() {
            self.seen_focus = f.focused_terminal.map(str::to_owned);
            if let Some(t) = &self.seen_focus {
                self.selected = Some(Sel::Terminal(t.clone()));
                changed = true;
            }
        }
        if self.selected.is_none()
            && let Some(sel) = f.tree_selection.as_deref().and_then(Sel::from_sidebar_key)
        {
            self.selected = Some(sel);
            changed = true;
        }
        let label = |t: &TerminalVm| f.label_of(t);
        let inputs = Inputs {
            terminals: f.terminals,
            tasks: f.tasks,
            gates: f.all_gates,
            toasts: toasts_in(f),
            label: &label,
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
        (changed, self.line_read(f))
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
    fn line_read(&self, f: &LayoutFrame<'_>) -> Option<LineRead> {
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
        })
    }

    /// `terminal.read` of the selected terminal's last lines, through the
    /// window's podd source.
    fn start_read(&mut self, r: LineRead, cx: &mut Context<Self>) {
        let LineRead { id, key } = r;
        let Some(source) = self.actions.source(cx) else {
            return;
        };
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
        let read = self.actions.read(cx, |f| self.line_read(f)).flatten();
        self.finish(true, read, cx);
    }

    /// The regions in F6 order that are shown now, with the handle to focus.
    fn regions(&self, window: &Window, cx: &App) -> Vec<(Region, FocusHandle, bool)> {
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
        let panels = self.actions.terminal_panels(cx);
        let term = self
            .actions
            .read(cx, |f| f.focused_terminal.map(str::to_owned))
            .flatten()
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
        } else {
            // Its panel forward in the dock; the keyboard stays in the log.
            self.actions
                .dispatch(UiIntent::RevealTerminal { id: t }, window, cx);
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
        // Nothing to decide with once the host is gone (its dock goes with it).
        if self.busy.contains_key(&gate) || self.actions.dock(cx).is_none() {
            return;
        }
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
        self.actions.resolve_gate(params, cx, move |r, cx| {
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
        let Some(source) = self.actions.source(cx) else {
            return;
        };
        let Some(t) = self.actions.read(cx, |f| f.terminal(id).cloned()).flatten() else {
            return;
        };
        let rx = source.run(model::rerun_spec(&t));
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |v, cx| {
                // No window here for an intent: one host update each, as the
                // workspace did them — the new terminal joins the list and the
                // window redraws, or an error toast.
                let error = |body: SharedString| HostUpdate::Toast {
                    kind: ToastKind::Error,
                    title: copy::COULD_NOT_RERUN.into(),
                    body,
                };
                match r {
                    Ok(Ok(new)) => {
                        v.actions.post(
                            vec![HostUpdate::AddTerminal(new.clone()), HostUpdate::Notify],
                            cx,
                        );
                        v.selected = Some(Sel::Terminal(new.id));
                    }
                    Ok(Err(e)) => {
                        v.actions.post(vec![error(e.message.into())], cx);
                    }
                    Err(_) => {
                        v.actions
                            .post(vec![error(crate::strings::PODD_CLIENT_STOPPED.into())], cx);
                    }
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

    fn renderer(&self) -> Rc<dyn LayoutRenderer> {
        Rc::new(DenseRenderer)
    }
}

/// The window body over the kit: the frame, the host's slots and the view
/// state go to the render.
struct DenseRenderer;

impl LayoutRenderer for DenseRenderer {
    fn render_body(
        &self,
        frame: &LayoutFrame<'_>,
        host: &mut dyn LayoutHost,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let view = view(&host.actions(), window, cx);
        // First frame of a fresh view: record the current state (later changes arrive by observation).
        if view.read(cx).log.is_empty() {
            view.update(cx, |v, cx| v.sync(frame, cx));
        }
        // The app-level pages (extensions, sessions) count as open panels here.
        let empty = frame.panels.dock_empty()
            && !frame.panel_detail.mods_page
            && !frame.panel_detail.sessions_page;
        let mut slot = |slot: Slot| {
            host.slot(slot, window, cx)
                .unwrap_or_else(|| div().into_any_element())
        };
        let (toasts, sidebar, dock) = (slot(Slot::Toasts), slot(Slot::Sidebar), slot(Slot::Dock));
        let hook_banner = host.slot(Slot::HookBanner, window, cx);
        let label = |t: &TerminalVm| frame.label_of(t);
        view::body(
            view::Body {
                view,
                warning: frame.warning(),
                hook_banner,
                sidebar,
                dock,
                toasts,
                empty,
            },
            frame,
            &label,
            window,
            cx,
        )
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

/// Run `f` on the view of the active window when this layout is shown.
fn with_active_view(cx: &mut App, f: fn(&mut DenseView, &mut Window, &mut Context<DenseView>)) {
    let Some(handle) = cx.active_window() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, cx| {
            let layout = cx
                .try_global::<crate::theme::Settings>()
                .map(|s| s.layout.clone())
                .unwrap_or_default();
            if layout != LayoutMode::DenseDetails.id() {
                return;
            }
            let Some(v) = kit::state_in_window::<DenseView>(handle, cx) else {
                return;
            };
            v.update(cx, |v, cx| f(v, window, cx));
        });
    });
}

/// The frame's toasts as the log reads them.
fn toasts_in(f: &LayoutFrame<'_>) -> Vec<ToastIn> {
    f.toasts
        .iter()
        .map(|t| ToastIn {
            id: t.id,
            kind: t.kind,
            title: t.title.to_string(),
            body: t.body.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
