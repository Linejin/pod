//! The feed's controller: the [`Feed`] entity with its view state, the
//! `terminal.attach` stream per terminal of the selected task, the per-frame
//! sync before drawing ([`Feed::sync_frame`]) and every event handler the view
//! calls.
//!
//! Reads of tasks, terminals, gates and the focused terminal go through the
//! kit's frame ([`LayoutActions::read`]); the copy toast is a [`UiIntent`].
//! What the kit does not carry yet stays a small legacy adapter over the
//! workspace (`Feed::ws`): change notifications, terminal labels, the attach
//! pipe, typing into a terminal, the workbench's live terminal panels, the
//! gate decision and the fixture's gate.
use std::collections::{HashMap, HashSet};

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use podd::protocol::{self as p, GateInfo};

use super::copy as s;
use crate::attach::{AttachHandle, Chunk};
use crate::layout_mode::LayoutMode;
use crate::layouts::blocks_model::{
    Approval, Block, BlockKey, FeedBlock, Kind, TermBlocks, build_feed, cycle_target, decision,
    default_expanded, gates_by_terminal, nav, state_label, task_terminals, title_of, who,
};
use crate::layouts::kit::{Clock, LayoutActions, LayoutMetrics, UiIntent};
use crate::panel::TerminalPanel;
use crate::vm::{Liveness, TaskVm, TerminalVm};
use crate::workspace::{ToastKind, Workspace};

/// One terminal's output stream and blocks.
struct Stream {
    blocks: TermBlocks,
    /// Status key the blocks were cut for.
    status_key: String,
    /// The lifecycle end was applied.
    ended: bool,
    _attach: Option<AttachHandle>,
    _pump: Option<Task<()>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Tasks,
    Feed,
    Input,
}

pub struct Feed {
    /// Legacy adapter: what the kit does not carry yet (see the module doc).
    ws: WeakEntity<Workspace>,
    /// Reads (frame) and UI intents of the host.
    actions: LayoutActions,
    /// "Now" of block starts and ends, status steps and the fixture.
    clock: Clock,
    pub task: Option<String>,
    /// Only this terminal's blocks (`None`: all agents).
    pub filter: Option<String>,
    pub selected: Option<BlockKey>,
    /// Folds the user chose (over [`default_expanded`]).
    pub toggled: HashMap<BlockKey, bool>,
    /// The terminal the bottom input types into.
    pub target: Option<String>,
    /// The raw terminal is shown instead of the blocks.
    pub raw: bool,
    raw_terminal: Option<String>,
    own_panel: Option<Entity<TerminalPanel>>,
    streams: HashMap<String, Stream>,
    pub(super) focus: FocusHandle,
    pub(super) task_focus: HashMap<String, FocusHandle>,
    block_focus: HashMap<BlockKey, FocusHandle>,
    pub input: Entity<InputState>,
    pub(super) scroll: ScrollHandle,
    pub(super) task_scroll: UniformListScrollHandle,
    /// Gates being resolved.
    busy: HashSet<String>,
    /// Follow new output (the selection is on the newest block or none).
    follow: bool,
    fixture: bool,
    fixture_gate: bool,
    placeholder: SharedString,
    /// (blocks, task, newest block's lines) last scrolled for: following scrolls only when it changes.
    follow_sig: (usize, Option<String>, usize),
    seen_focus: Option<String>,
    entered: bool,
    shown: bool,
    _subs: Vec<Subscription>,
}

/// What one frame of the feed draws, read and settled by [`Feed::sync_frame`].
pub(super) struct Shown {
    /// The selected task's terminals.
    pub(super) terms: Vec<TerminalVm>,
    pub(super) feed: Vec<FeedBlock>,
    /// Focus handle of each block of `feed`, in order.
    pub(super) focus: Vec<FocusHandle>,
    pub(super) tasks: Vec<TaskVm>,
    /// Pending gates, oldest first.
    pub(super) pending: Vec<GateInfo>,
    /// The workspace's label of each terminal in `terms`.
    pub(super) labels: HashMap<String, String>,
    pub(super) metrics: LayoutMetrics,
    /// The raw terminal's panel, while the raw view is shown.
    pub(super) raw_panel: Option<Entity<TerminalPanel>>,
    /// `who` of the input target, if there is one.
    pub(super) target_name: Option<String>,
}

impl Feed {
    pub(super) fn new(
        ws: WeakEntity<Workspace>,
        actions: LayoutActions,
        fixture: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx));
        let mut subs = vec![
            cx.observe_global::<crate::theme::Settings>(|f: &mut Self, cx| {
                let here =
                    cx.global::<crate::theme::Settings>().layout == LayoutMode::BlockTerminal.id();
                if !here {
                    // Leaving: the feed's attaches and its own panel go; the dock was never touched.
                    f.own_panel = None;
                    f.streams.clear();
                    f.entered = false;
                } else if !f.shown {
                    f.entered = true;
                }
                f.shown = here;
            }),
        ];
        subs.push(cx.subscribe_in(
            &input,
            window,
            |f: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    f.send_input(window, cx);
                }
            },
        ));
        Self {
            ws,
            actions,
            clock: Clock::System,
            task: None,
            filter: None,
            selected: None,
            toggled: HashMap::new(),
            target: None,
            raw: false,
            raw_terminal: None,
            own_panel: None,
            streams: HashMap::new(),
            focus: cx.focus_handle().tab_stop(true),
            task_focus: HashMap::new(),
            block_focus: HashMap::new(),
            input,
            scroll: ScrollHandle::new(),
            task_scroll: UniformListScrollHandle::new(),
            busy: HashSet::new(),
            follow: true,
            fixture,
            fixture_gate: false,
            placeholder: SharedString::default(),
            follow_sig: (0, None, 0),
            seen_focus: None,
            entered: true,
            shown: true,
            _subs: subs,
        }
    }

    #[cfg(test)]
    pub fn own_panel(&self) -> Option<Entity<TerminalPanel>> {
        self.own_panel.clone()
    }
    #[cfg(test)]
    pub fn attached(&self) -> Vec<String> {
        let mut v: Vec<String> = self.streams.keys().cloned().collect();
        v.sort();
        v
    }
    /// Test hook: output for a terminal as if the attach stream delivered it.
    #[cfg(test)]
    pub fn ingest(&mut self, id: &str, chunk: Chunk, cx: &mut Context<Self>) {
        self.apply_chunks(id, vec![chunk], cx);
    }

    pub fn block_focus(&mut self, key: &BlockKey, cx: &mut Context<Self>) -> FocusHandle {
        self.block_focus
            .entry(key.clone())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }
    pub fn task_focus(&mut self, id: &str, cx: &mut Context<Self>) -> FocusHandle {
        self.task_focus
            .entry(id.to_string())
            .or_insert_with(|| cx.focus_handle())
            .clone()
    }

    /// Legacy adapter: the workspace re-renders with the feed (its title bar
    /// and status bar show the feed's task).
    pub(super) fn notify_all(&self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |_, cx| cx.notify());
        }
    }

    /// The task's terminals, the feed, and the gates (built from the frame every
    /// time). Terminal labels are the workspace's (`label_of`): the frame does
    /// not carry them yet.
    pub fn feed(&self, cx: &App) -> Vec<FeedBlock> {
        let Some(ws) = self.ws.upgrade() else {
            return Vec::new();
        };
        let w = ws.read(cx);
        self.actions
            .read(cx, |f| {
                let Some(task) = self.task.as_deref().and_then(|id| f.task(id)) else {
                    return Vec::new();
                };
                let terms = task_terminals(task, f.terminals);
                let gates = gates_by_terminal(&terms, &task.id, f.gates);
                let blocks: HashMap<String, Vec<Block>> = self
                    .streams
                    .iter()
                    .map(|(k, s)| (k.clone(), s.blocks.blocks.clone()))
                    .collect();
                build_feed(&terms, &blocks, &gates, self.filter.as_deref(), |t| {
                    w.label_of(t)
                })
            })
            .unwrap_or_default()
    }

    /// Legacy adapter: the workspace's label of each terminal.
    fn labels(&self, terms: &[TerminalVm], cx: &App) -> HashMap<String, String> {
        let Some(ws) = self.ws.upgrade() else {
            return HashMap::new();
        };
        let w = ws.read(cx);
        terms
            .iter()
            .map(|x| (x.id.clone(), w.label_of(x)))
            .collect()
    }

    pub(super) fn expanded(&self, f: &FeedBlock) -> bool {
        self.toggled
            .get(&f.key)
            .copied()
            .unwrap_or_else(|| default_expanded(f))
    }

    pub fn is_expanded(&self, key: &BlockKey, cx: &App) -> bool {
        self.feed(cx)
            .iter()
            .find(|f| &f.key == key)
            .is_some_and(|f| self.expanded(f))
    }

    /// Pick a task: its terminals' streams start, the target is its first running terminal.
    pub fn set_task(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.task.as_deref() == Some(id) {
            return;
        }
        self.task = Some(id.to_string());
        self.filter = None;
        self.selected = None;
        self.target = None;
        self.raw = false;
        self.follow = true;
        self.notify_all(cx);
    }

    /// A filter chip: one terminal's blocks, or all (`None`).
    pub(super) fn set_filter(&mut self, filter: Option<String>, cx: &mut Context<Self>) {
        self.filter = filter;
        self.notify_all(cx);
    }

    /// Attach to the selected task's terminals, detach from the rest; apply status changes.
    fn sync_streams(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        // Legacy adapter: the attach pipe is the workspace source's.
        let pipe = ws.read(cx).source.pipe().to_string();
        let terms = self.task_terms(cx);
        self.streams
            .retain(|id, _| terms.iter().any(|t| &t.id == id));
        for t in &terms {
            if !self.streams.contains_key(&t.id) {
                let stream = if self.fixture {
                    let mut blocks = TermBlocks::new(100, 30);
                    blocks.reset(b"", 100, 30, 0);
                    fixture_feed(&t.id, &mut blocks, self.clock.now_ms());
                    Stream {
                        blocks,
                        status_key: String::new(),
                        ended: false,
                        _attach: None,
                        _pump: None,
                    }
                } else {
                    let (tx, mut rx) = mpsc::unbounded();
                    let attach = AttachHandle::spawn(pipe.clone(), t.id.clone(), tx);
                    let id = t.id.clone();
                    let pump = cx.spawn(async move |this, cx| {
                        while rx.next().await.is_some() {
                            if this.update(cx, |f, cx| f.drain(&id, cx)).is_err() {
                                break;
                            }
                        }
                    });
                    Stream {
                        blocks: TermBlocks::new(80, 24),
                        status_key: String::new(),
                        ended: false,
                        _attach: Some(attach),
                        _pump: Some(pump),
                    }
                };
                self.streams.insert(t.id.clone(), stream);
            }
            let s = self.streams.get_mut(&t.id).expect("inserted above");
            if !s.blocks.started {
                continue;
            }
            // Agent status: a new step per state (only once the stream started).
            if let Some(st) = t.status.as_ref() {
                let key = format!("{:?}|{}|{}", st.state, st.since, st.turn);
                if key != s.status_key {
                    s.status_key = key;
                    let (label, finished) = state_label(st.state);
                    s.blocks.status(label, finished, self.clock.now_ms());
                }
            }
            if let Liveness::Exited { code, .. } = t.live
                && !s.ended
            {
                s.ended = true;
                s.blocks.exited(code, self.clock.now_ms());
            }
        }
    }

    fn drain(&mut self, id: &str, cx: &mut Context<Self>) {
        let chunks = self
            .streams
            .get(id)
            .and_then(|s| s._attach.as_ref())
            .map(|a| a.take())
            .unwrap_or_default();
        self.apply_chunks(id, chunks, cx);
    }

    fn apply_chunks(&mut self, id: &str, chunks: Vec<Chunk>, cx: &mut Context<Self>) {
        let Some(s) = self.streams.get_mut(id) else {
            return;
        };
        let now = self.clock.now_ms();
        for c in chunks {
            match c {
                Chunk::Reset { snapshot, info } => {
                    s.blocks.reset(&snapshot, info.cols, info.rows, now)
                }
                Chunk::Output(b) => s.blocks.feed(&b, now),
                Chunk::Exited { code, .. } => {
                    s.ended = true;
                    s.blocks.exited(code, now);
                }
                Chunk::Lagged | Chunk::Disconnected | Chunk::Gone => {}
            }
        }
        if self.follow {
            self.scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    pub fn select(&mut self, key: &BlockKey, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(key.clone());
        let feed = self.feed(cx);
        if let Some(i) = feed.iter().position(|f| &f.key == key) {
            self.follow = i + 1 == feed.len();
            // The newest block shows whole (its end, not just its top).
            if self.follow {
                self.scroll.scroll_to_bottom();
            } else {
                self.scroll.scroll_to_item(i);
            }
        }
        let f = self.block_focus(key, cx);
        f.focus(window, cx);
        cx.notify();
    }

    pub fn toggle(&mut self, key: &BlockKey, cx: &mut Context<Self>) {
        let now = self.is_expanded(key, cx);
        self.toggled.insert(key.clone(), !now);
        cx.notify();
    }

    pub fn copy(&mut self, key: &BlockKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.feed(cx).into_iter().find(|f| &f.key == key) else {
            return;
        };
        let mut text = f.block.text();
        if f.block.kind != Kind::Command {
            text = format!("{}\n{text}", title_of(&f));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.actions.dispatch(
            UiIntent::Toast {
                kind: ToastKind::Success,
                title: s::COPIED.into(),
                body: f.who.clone().into(),
            },
            window,
            cx,
        );
    }

    /// The raw terminal of `id` (or the target), focused.
    pub fn show_raw(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let id = id
            .or(self.target.clone())
            .or_else(|| self.selected.as_ref().map(|k| k.terminal.clone()));
        self.raw = true;
        self.raw_terminal = id;
        if let Some(panel) = self.raw_panel(window, cx) {
            let f = panel.read(cx).view.read(cx).focus_handle(cx);
            f.focus(window, cx);
        }
        self.notify_all(cx);
    }

    pub fn hide_raw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.raw = false;
        match self.selected.clone() {
            Some(k) => self.select(&k, window, cx),
            None => self.focus.focus(window, cx),
        }
        self.notify_all(cx);
    }

    /// The header's 원본 터미널 보기 / 블록으로 보기 button.
    pub(super) fn toggle_raw(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.raw {
            self.hide_raw(window, cx)
        } else {
            self.show_raw(None, window, cx)
        }
    }

    /// The workbench's own panel for the raw terminal when it has one (no new
    /// attach), else a feed-owned one. Legacy adapter: the live panels, the
    /// panel registry and a new panel's workspace are the workspace's.
    fn raw_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TerminalPanel>> {
        let id = self.raw_terminal.clone()?;
        let ws = self.ws.upgrade()?;
        let own = self.own_panel.as_ref().map(|p| p.entity_id());
        let existing = ws
            .read(cx)
            .live_panels()
            .into_iter()
            .find(|p| Some(p.entity_id()) != own && p.read(cx).terminal_id == id);
        if let Some(p) = existing {
            self.own_panel = None;
            return Some(p);
        }
        if let Some(p) = self
            .own_panel
            .as_ref()
            .filter(|p| p.read(cx).terminal_id == id)
        {
            return Some(p.clone());
        }
        if !self
            .actions
            .read(cx, |f| f.terminal(&id).is_some())
            .unwrap_or(false)
        {
            return None;
        }
        let g = cx.global::<crate::workspace::AppGlobals>();
        let (source, panels) = (g.source.clone(), g.panels.clone());
        let label: SharedString = g
            .labels
            .borrow()
            .get(&id)
            .cloned()
            .unwrap_or_else(|| id.clone())
            .into();
        let weak = self.ws.clone();
        let panel = cx.new(|cx| TerminalPanel::new(source, weak, id, label, window, cx));
        panels.borrow_mut().push(panel.downgrade());
        self.own_panel = Some(panel.clone());
        Some(panel)
    }

    /// Enter in the input: the text and a carriage return to the target terminal.
    fn send_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        // Legacy adapter: the kit has no terminal input intent.
        if let Some(ws) = self.ws.upgrade() {
            ws.read(cx)
                .source
                .input(&target, format!("{text}\r").into_bytes());
        }
        self.input.update(cx, |i, cx| i.set_value("", window, cx));
        self.follow = true;
        self.scroll.scroll_to_bottom();
        cx.notify();
    }

    fn task_terms(&self, cx: &App) -> Vec<TerminalVm> {
        self.actions
            .read(cx, |f| {
                self.task
                    .as_deref()
                    .and_then(|id| f.task(id))
                    .map(|t| {
                        task_terminals(t, f.terminals)
                            .into_iter()
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    pub fn cycle(&mut self, back: bool, cx: &mut Context<Self>) {
        let terms = self.task_terms(cx);
        let refs: Vec<&TerminalVm> = terms.iter().collect();
        self.target = cycle_target(&refs, self.target.as_deref(), back);
        cx.notify();
    }

    pub fn focus_region(&mut self, r: Region, window: &mut Window, cx: &mut Context<Self>) {
        match r {
            Region::Tasks => {
                if let Some(t) = self.task.clone() {
                    let f = self.task_focus(&t, cx);
                    f.focus(window, cx);
                }
            }
            Region::Feed => {
                if self.raw {
                    if let Some(p) = self.raw_panel(window, cx) {
                        p.read(cx).view.read(cx).focus_handle(cx).focus(window, cx);
                    }
                    return;
                }
                let feed = self.feed(cx);
                let key = self
                    .selected
                    .clone()
                    .filter(|k| feed.iter().any(|f| &f.key == k))
                    .or_else(|| feed.last().map(|f| f.key.clone()));
                match key {
                    Some(k) => self.select(&k, window, cx),
                    None => self.focus.focus(window, cx),
                }
            }
            Region::Input => self.input.update(cx, |i, cx| i.focus(window, cx)),
        }
    }

    fn region(&self, window: &Window, cx: &App) -> Region {
        if self
            .task_focus
            .values()
            .any(|f| f.contains_focused(window, cx))
        {
            Region::Tasks
        } else if self
            .input
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            Region::Input
        } else {
            Region::Feed
        }
    }

    /// F6: 작업 → 블록 → 입력 → 작업.
    pub fn switch_region(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The raw view has no input row: 작업 ↔ 터미널.
        let next = match (self.region(window, cx), self.raw) {
            (Region::Tasks, _) => Region::Feed,
            (Region::Feed, false) => Region::Input,
            (Region::Feed, true) | (Region::Input, _) => Region::Tasks,
        };
        self.focus_region(next, window, cx);
    }

    /// Approve / reject through the gate path (`gate.resolve`), as the gates
    /// inbox does. Legacy adapter: the request goes through the workspace's
    /// source and the answer into its gate state, in one update.
    pub fn resolve(
        &mut self,
        key: BlockKey,
        gate: GateInfo,
        a: Approval,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.ws.upgrade() else { return };
        let Some((decision, choice)) = decision(&gate, a) else {
            return;
        };
        if !self.busy.insert(gate.id.clone()) {
            return;
        }
        let id = gate.id.clone();
        let refocus = self.block_focus(&key, cx).contains_focused(window, cx);
        let rx = ws
            .read(cx)
            .source
            .call::<p::GateResolve>(&p::GateResolveParams {
                id: id.clone(),
                decision,
                note: None,
                choice,
            });
        cx.spawn_in(window, async move |this, cx| {
            let r = rx.await;
            let _ = this.update_in(cx, |f, window, cx| {
                f.busy.remove(&id);
                // The buttons go with the gate: focus stays on the block (or the
                // newest one when the block was only the approval).
                let h = f.block_focus(&key, cx);
                // Unless the user moved on to the tasks or the input meanwhile.
                let refocus = refocus && f.region(window, cx) == Region::Feed;
                ws.update(cx, |w, cx| {
                    match r {
                        Ok(g) => {
                            w.orch.apply(g);
                        }
                        Err(e) => {
                            w.raise_toast(ToastKind::Error, s::COULD_NOT_RESOLVE, e.message, cx)
                        }
                    }
                    cx.notify();
                });
                if refocus {
                    if f.feed(cx).iter().any(|b| b.key == key) {
                        h.focus(window, cx);
                    } else {
                        f.selected = None;
                        f.focus_region(Region::Feed, window, cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn on_block_key(
        &mut self,
        f: &FeedBlock,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &ev.keystroke;
        if k.modifiers.control && k.key == "c" {
            self.copy(&f.key, window, cx);
            return true;
        }
        if k.modifiers.modified() {
            return false;
        }
        match k.key.as_str() {
            "up" | "down" | "home" | "end" => {
                if let Some(next) = nav(&self.feed(cx), Some(&f.key), &k.key) {
                    self.select(&next, window, cx);
                }
            }
            "enter" | "space" => self.toggle(&f.key, cx),
            "right" if !self.expanded(f) => self.toggle(&f.key, cx),
            "left" if self.expanded(f) => self.toggle(&f.key, cx),
            "c" => self.copy(&f.key, window, cx),
            "t" => self.show_raw(Some(f.key.terminal.clone()), window, cx),
            "y" | "a" | "n" if f.gate.is_some() => {
                let a = match k.key.as_str() {
                    "y" => Approval::Once,
                    "a" => Approval::Always,
                    _ => Approval::Reject,
                };
                if let Some(g) = f.gate.clone() {
                    self.resolve(f.key.clone(), g, a, window, cx);
                }
            }
            _ => return false,
        }
        true
    }

    /// A task row clicked: show that task and focus its row.
    pub(super) fn pick_task(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.set_task(id, cx);
        let h = self.task_focus(id, cx);
        h.focus(window, cx);
    }

    pub(super) fn on_task_key(
        &mut self,
        id: &str,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &ev.keystroke;
        if k.modifiers.modified() {
            return false;
        }
        let Some(ids) = self.actions.read(cx, |f| {
            f.tasks
                .iter()
                .map(|t| t.id.clone())
                .collect::<Vec<String>>()
        }) else {
            return false;
        };
        let Some(i) = ids.iter().position(|x| x == id) else {
            return false;
        };
        let j = match k.key.as_str() {
            "up" => i.checked_sub(1),
            "down" => (i + 1 < ids.len()).then_some(i + 1),
            "home" => Some(0),
            "end" => ids.len().checked_sub(1),
            "enter" | "space" => {
                self.focus_region(Region::Feed, window, cx);
                return true;
            }
            _ => return false,
        };
        if let Some(j) = j {
            let next = ids[j].clone();
            self.set_task(&next, cx);
            self.task_scroll.scroll_to_item(j, ScrollStrategy::Nearest);
            let f = self.task_focus(&next, cx);
            f.focus(window, cx);
        }
        true
    }

    /// Entering the layout: a focused terminal's task is shown and that
    /// terminal becomes the input target (focus in the input); otherwise the
    /// newest block takes focus.
    fn place_focus_on_enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use gpui_kit::component::WindowExt as _;
        if window.has_active_dialog(cx) {
            return;
        }
        let Some(ws) = self.ws.upgrade() else { return };
        // Legacy adapter: which live panel holds the keyboard focus.
        let focused = ws
            .read(cx)
            .live_panels()
            .into_iter()
            .find(|p| {
                p.read(cx)
                    .view
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
            .map(|p| p.read(cx).terminal_id.clone());
        if let Some(id) = focused {
            if let Some(task) = self
                .actions
                .read(cx, |f| f.terminal(&id).and_then(|t| t.task_id.clone()))
                .flatten()
            {
                self.set_task(&task, cx);
            }
            self.target = Some(id);
            let input = self.input.clone();
            window.defer(cx, move |window, cx| {
                input.update(cx, |i, cx| i.focus(window, cx))
            });
            return;
        }
        let me = cx.entity().downgrade();
        window.defer(cx, move |window, cx| {
            let _ = me.update(cx, |f, cx| f.focus_region(Region::Feed, window, cx));
        });
    }

    /// Before drawing: settle the task, streams, target, selection, scrolling
    /// and focus for this frame, in this order, and read what the view draws.
    /// `None` once the window is gone (the view draws nothing).
    pub(super) fn sync_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Shown> {
        // The task: the one chosen, else the focused terminal's, else the first.
        let (focused, first_task) = self.actions.read(cx, |f| {
            (
                f.focused_terminal.map(str::to_string),
                f.tasks.first().map(|t| t.id.clone()),
            )
        })?;
        if focused != self.seen_focus {
            self.seen_focus = focused.clone();
            if let Some(task) = focused.as_deref().and_then(|id| {
                self.actions
                    .read(cx, |f| f.terminal(id).and_then(|t| t.task_id.clone()))
                    .flatten()
            }) {
                self.set_task(&task, cx);
                self.target = focused.clone();
            }
        }
        if self.task.as_deref().is_none_or(|id| {
            self.actions
                .read(cx, |f| f.task(id).is_none())
                .unwrap_or(true)
        }) && let Some(id) = first_task
        {
            self.task = Some(id);
        }
        self.sync_streams(cx);
        if self.fixture && !self.fixture_gate && self.task.is_some() {
            self.fixture_gate = true;
            // Legacy adapter: the fixture's gate goes into the workspace's gate state.
            let now = self.clock.now_ms();
            if let Some(ws) = self.ws.upgrade() {
                ws.update(cx, |w, _| {
                    w.orch.apply(fixture_gate(now));
                });
            }
        }
        let terms = self.task_terms(cx);
        let term_refs: Vec<&TerminalVm> = terms.iter().collect();
        if self
            .target
            .as_deref()
            .is_none_or(|id| !terms.iter().any(|t| t.id == id && t.is_running()))
        {
            self.target = cycle_target(&term_refs, None, false);
        }
        let feed = self.feed(cx);
        if self
            .selected
            .as_ref()
            .is_some_and(|k| !feed.iter().any(|f| &f.key == k))
        {
            self.selected = feed.last().map(|f| f.key.clone());
        }
        // Following new output: the selection stays on the newest block.
        if self.selected.is_none() || self.follow {
            let newest = feed.last().map(|f| f.key.clone());
            if newest != self.selected {
                // Focus on the old newest block moves along with the selection.
                let had = self
                    .selected
                    .as_ref()
                    .and_then(|k| self.block_focus.get(k))
                    .is_some_and(|h| h.is_focused(window));
                self.selected = newest.clone();
                if had && let Some(k) = newest {
                    let h = self.block_focus(&k, cx);
                    window.defer(cx, move |window, cx| h.focus(window, cx));
                }
            }
        }
        let sig = (
            feed.len(),
            self.task.clone(),
            feed.last().map_or(0, |f| f.block.lines.len()),
        );
        if sig != self.follow_sig {
            if self.follow || sig.1 != self.follow_sig.1 {
                self.scroll.scroll_to_bottom();
            }
            self.follow_sig = sig;
        }
        if std::mem::take(&mut self.entered) {
            self.place_focus_on_enter(window, cx);
        }
        let focus: Vec<FocusHandle> = feed.iter().map(|f| self.block_focus(&f.key, cx)).collect();

        let labels = self.labels(&terms, cx);
        let (tasks, pending, metrics) = self.actions.read(cx, |f| {
            (f.tasks.to_vec(), f.gates.to_vec(), f.metrics.clone())
        })?;
        for k in &tasks {
            self.task_focus(&k.id, cx);
        }
        let raw_panel = if self.raw {
            self.raw_panel(window, cx)
        } else {
            None
        };
        let target_name = self
            .target
            .as_ref()
            .and_then(|id| terms.iter().find(|x| &x.id == id))
            .map(|x| who(x, labels.get(&x.id).map(String::as_str).unwrap_or(&x.id)));
        let placeholder: SharedString = match &target_name {
            Some(n) => s::input_placeholder(n).into(),
            None => s::NO_TARGET.into(),
        };
        if self.placeholder != placeholder {
            self.placeholder = placeholder.clone();
            self.input
                .update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
        }
        Some(Shown {
            terms,
            feed,
            focus,
            tasks,
            pending,
            labels,
            metrics,
            raw_panel,
            target_name,
        })
    }
}

impl Focusable for Feed {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ───────────────────────────── fixture ─────────────────────────────

/// Scripted output for the read-only UI fixture (visual captures): Pod's
/// shell marks around a few commands, so the feed looks populated without a
/// daemon. `now` is the epoch ms the commands are placed before.
pub(super) fn fixture_feed(id: &str, b: &mut TermBlocks, now: u64) {
    const A: &str = "\x1b]133;A\x07";
    const B: &str = "\x1b]133;B\x07";
    const C: &str = "\x1b]133;C\x07";
    let n: usize = id
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0);
    let base = now.saturating_sub(120_000);
    let prompt = "PS C:\\src\\pod> ";
    let run = |b: &mut TermBlocks, cmd: &str, out: &str, at: u64, took: Option<(u64, i32)>| {
        b.feed(format!("{A}{prompt}{B}{cmd}\r\n").as_bytes(), base + at);
        b.feed(C.as_bytes(), base + at);
        b.feed(out.replace('\n', "\r\n").as_bytes(), base + at + 10);
        if let Some((ms, code)) = took {
            b.feed(format!("\x1b]133;D;{code}\x07").as_bytes(), base + at + ms);
        }
    };
    match n % 3 {
        0 => {
            run(
                b,
                "cargo test -p pod-ui wide_glyph",
                "running 1 test\ntest wide_glyph::hangul_takes_two_cells ... ok\n\ntest result: ok. 1 passed; 0 failed\n",
                0,
                Some((3_100, 0)),
            );
            run(
                b,
                "cargo build -p pod-ui",
                "   Compiling pod-ui v0.1.0 (C:\\src\\pod\\crates\\pod-ui)\n• 수정 crates/pod-ui/tests/wide_glyph.rs\n  + #[test] fn hangul_takes_two_cells() {\n  +     assert_eq!(layout(\"한\").cols, 2);\n",
                30_000,
                None,
            );
        }
        1 => run(
            b,
            "cargo clippy -p pod-ui -- -D warnings",
            "변경 요약  terminal.rs 셀 폭 계산 · status.rs 스피너 폭 고정\n",
            50_000,
            None,
        ),
        _ => run(
            b,
            "git status --short",
            " M crates/pod-ui/src/terminal.rs\n M crates/pod-ui/src/status.rs\n?? crates/pod-ui/tests/wide_glyph.rs\n",
            100_000,
            Some((400, 0)),
        ),
    }
}

/// The fixture's one pending approval (block terminal captures only), asked
/// 12 seconds before `now`.
pub(super) fn fixture_gate(now: u64) -> GateInfo {
    GateInfo {
        id: "g-fixture".into(),
        title: "명령 실행".into(),
        message: "cargo clippy 실행 승인이 필요합니다.".into(),
        task: Some("sample/task-000".into()),
        group: None,
        terminal: Some("fixture-1".into()),
        state: p::GateState::Pending,
        created_ms: now.saturating_sub(12_000),
        resolved_ms: None,
        note: None,
        expires_ms: None,
        run: None,
        item: None,
        options: vec!["이번만 허용".into(), "항상 허용".into()],
        choice: None,
        requested_by: None,
        decided_by: None,
    }
}
