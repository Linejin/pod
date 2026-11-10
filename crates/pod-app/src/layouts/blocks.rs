//! 블록 터미널 (preset 11, docs/10 §D5.8): the selected task as a chronological
//! feed of command blocks — one per shell command or agent step — with the
//! approval inside the block and one input at the bottom.
//!
//! - **Blocks come from the output** ([`super::blocks_model`]): OSC 133 marks
//!   (Pod's own shells emit them, [`super::shell_integration`]), agent status
//!   transitions (hook / OSC 7501 via podd), prompt detection otherwise. A
//!   full-screen program is one live block; "전체 터미널로 보기" opens the raw
//!   terminal, which is never cut or re-spawned.
//! - **No forked state.** Tasks, terminals and gates are the workspace's;
//!   approvals go through `gate.resolve` (the gates inbox path), input through
//!   `terminal.input`. The feed keeps view state only: task, filter, target,
//!   selection, folds and the block text it read.
//! - **No polling.** Output arrives on a `terminal.attach` stream per terminal
//!   of the selected task (event driven, dropped when the task or layout
//!   changes); status changes arrive with the workspace's events.
//! - **Switching is live.** The dock is never touched; the raw view shows the
//!   workbench's own terminal panel when it has one (same entity, same
//!   attach), else a feed-owned one dropped when the layout changes.
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use podd::protocol::{self as p, GateDecision, GateInfo};

use super::blocks_model::{Block, Kind, Origin, TermBlocks, duration_text};
use crate::attach::{AttachHandle, Chunk};
use crate::layout_mode::{LayoutMode, LayoutModule};
use crate::panel::TerminalPanel;
use crate::qa_marks::QaId;
use crate::vm::{AgentState, Liveness, TaskVm, TerminalVm};
use crate::workspace::{ToastKind, Workspace};

actions!(
    pod_blocks,
    [SwitchRegion, NextTarget, PrevTarget, LeaveInput]
);

/// Key context of the layout (F6) and of its input (Tab changes the target).
pub const KEY_CONTEXT: &str = "PodBlocks";
pub const INPUT_CONTEXT: &str = "PodBlocksInput";

/// Korean UI text of the layout.
pub mod s {
    pub const TASKS: &str = "작업";
    pub const ALL_AGENTS: &str = "모든 에이전트";
    pub const SHELL: &str = "셸";
    pub const CHRONO: &str = "시간순";
    pub const NEEDS_YOU: &str = "응답 필요";
    pub const ALLOW_ONCE: &str = "이번만 허용";
    pub const ALLOW_ALWAYS: &str = "항상 허용";
    pub const REJECT: &str = "거부";
    pub const NO_ALWAYS_TIP: &str = "이 승인 요청은 ‘항상 허용’ 선택지를 주지 않았습니다.";
    pub const COPY: &str = "복사";
    pub const COPIED: &str = "블록을 복사했습니다";
    pub const FULL_TERMINAL: &str = "전체 터미널로 보기";
    pub const RAW_ON: &str = "원본 터미널 보기";
    pub const RAW_OFF: &str = "블록으로 보기";
    pub const RUNNING: &str = "실행 중";
    pub const WAITING: &str = "승인 대기";
    pub const EARLIER: &str = "이전 출력";
    pub const OUTPUT: &str = "출력";
    pub const DONE: &str = "완료";
    pub const ENDED: &str = "끝남";
    pub const TUI_LIVE: &str = "전체 화면 · 실행 중";
    pub const TUI_DONE: &str = "전체 화면 · 끝남";
    pub const APPROVAL: &str = "승인 요청";
    pub const NO_TASK: &str = "작업을 고르세요";
    pub const NO_BLOCKS: &str =
        "아직 블록이 없습니다. 아래 입력으로 명령을 보내거나 터미널 출력이 오면 여기에 쌓입니다.";
    pub const NO_TARGET: &str = "입력할 터미널이 없습니다";
    pub const FEED: &str = "블록 피드";
    pub const INPUT_HINT: &str = "Tab 대상 변경 · Esc 블록으로 · F6 영역 이동";
    pub const OMITTED: &str = "줄 생략";
    pub const COULD_NOT_RESOLVE: &str = "승인 요청을 처리하지 못했습니다";
    pub const RAW_EMPTY: &str = "보여 줄 터미널이 없습니다.";
}

// ───────────────────────────── model ─────────────────────────────

/// A block's identity in the feed.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockKey {
    pub terminal: String,
    pub seq: u64,
}

impl BlockKey {
    fn dom(&self) -> String {
        format!("{}-{}", self.terminal, self.seq)
    }
}

/// A block as the feed shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct FeedBlock {
    pub key: BlockKey,
    /// `Claude · 구현`, `셸 · 빌드`.
    pub who: String,
    /// The terminal's agent (docs/10 §D5.11): the mark before `who`.
    pub kind: crate::agent_kind::AgentKind,
    pub block: Block,
    /// A pending gate of this block's terminal (on its newest block only).
    pub gate: Option<GateInfo>,
    /// The newest block of its terminal.
    pub last: bool,
}

const SHELLS: [&str; 6] = ["powershell", "pwsh", "cmd", "bash", "sh", "zsh"];

pub fn is_shell(program: &str) -> bool {
    SHELLS.contains(&program.to_ascii_lowercase().trim_end_matches(".exe"))
}

/// `Claude · 이름` for agents, `셸 · 이름` for shells.
pub fn who(t: &TerminalVm, label: &str) -> String {
    // The identified agent (hook/OSC/argv, docs/10 §D5.11) first, then the program name.
    let kind = crate::agent_kind::identify(t, None).kind;
    let agent = if kind.is_agent() {
        kind.name()
    } else if is_shell(&t.program) {
        s::SHELL.to_string()
    } else {
        let mut c = t.program.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    };
    format!("{agent} · {label}")
}

/// Step title for an agent state.
pub fn state_label(s: AgentState) -> (&'static str, bool) {
    match s {
        AgentState::Working => ("작업 중", false),
        AgentState::Blocked => (s::NEEDS_YOU, false),
        AgentState::Done => (s::DONE, true),
        AgentState::Idle => ("입력 대기", true),
        AgentState::Error => ("오류", true),
        AgentState::Unknown => ("상태 확인 중", false),
        AgentState::Exited => ("에이전트 종료", true),
    }
}

/// The terminals of `task` in the task's order (then any others that name it).
pub fn task_terminals<'a>(task: &TaskVm, terminals: &'a [TerminalVm]) -> Vec<&'a TerminalVm> {
    let mut out: Vec<&TerminalVm> = task
        .terminals
        .iter()
        .filter_map(|id| terminals.iter().find(|t| &t.id == id))
        .collect();
    for t in terminals
        .iter()
        .filter(|t| t.task_id.as_deref() == Some(task.id.as_str()))
    {
        if !out.iter().any(|x| x.id == t.id) {
            out.push(t);
        }
    }
    out
}

/// The pending gate per terminal: a terminal's own gate, else the task's on its first terminal.
pub fn gates_by_terminal(
    terms: &[&TerminalVm],
    task: &str,
    pending: &[GateInfo],
) -> HashMap<String, GateInfo> {
    let mut out = HashMap::new();
    for g in pending {
        if let Some(t) = g
            .terminal
            .as_deref()
            .filter(|t| terms.iter().any(|x| x.id == *t))
        {
            out.entry(t.to_string()).or_insert_with(|| g.clone());
        } else if g.terminal.is_none()
            && g.task.as_deref() == Some(task)
            && let Some(first) = terms.first()
        {
            out.entry(first.id.clone()).or_insert_with(|| g.clone());
        }
    }
    out
}

/// The feed: every block of the task's terminals (or one terminal), oldest first.
/// A terminal with a pending gate and no block yet gets an approval block.
pub fn build_feed(
    terms: &[&TerminalVm],
    blocks: &HashMap<String, Vec<Block>>,
    gates: &HashMap<String, GateInfo>,
    filter: Option<&str>,
    label: impl Fn(&TerminalVm) -> String,
) -> Vec<FeedBlock> {
    let mut out: Vec<(u64, usize, FeedBlock)> = Vec::new();
    for (ti, t) in terms
        .iter()
        .enumerate()
        .filter(|(_, t)| filter.is_none_or(|f| f == t.id))
    {
        let who = who(t, &label(t));
        let kind = crate::agent_kind::identify(t, None).kind;
        let empty = Vec::new();
        let list = blocks.get(&t.id).unwrap_or(&empty);
        let gate = gates.get(&t.id).cloned();
        let n = list.len();
        for (i, b) in list.iter().enumerate() {
            let last = i + 1 == n;
            out.push((
                b.started_ms.or(b.ended_ms).unwrap_or(0),
                ti,
                FeedBlock {
                    key: BlockKey {
                        terminal: t.id.clone(),
                        seq: b.seq,
                    },
                    who: who.clone(),
                    kind: kind.clone(),
                    block: b.clone(),
                    gate: gate.clone().filter(|_| last),
                    last,
                },
            ));
        }
        if n == 0
            && let Some(g) = gate
        {
            let b = Block {
                seq: u64::MAX,
                kind: Kind::Step,
                origin: Origin::Status,
                title: s::APPROVAL.into(),
                lines: Vec::new(),
                dropped: 0,
                started_ms: Some(g.created_ms),
                ended_ms: None,
                exit: None,
                open: true,
            };
            out.push((
                g.created_ms,
                ti,
                FeedBlock {
                    key: BlockKey {
                        terminal: t.id.clone(),
                        seq: u64::MAX,
                    },
                    who: who.clone(),
                    kind: kind.clone(),
                    block: b,
                    gate: Some(g),
                    last: true,
                },
            ));
        }
    }
    out.sort_by_key(|(at, ti, f)| (*at, *ti, f.block.seq));
    out.into_iter().map(|(_, _, f)| f).collect()
}

/// Expanded unless the user folded it: running, waiting for approval, failed,
/// or the newest block of its terminal. Other finished blocks start folded.
pub fn default_expanded(f: &FeedBlock) -> bool {
    f.block.open || f.gate.is_some() || f.block.failed() || f.last
}

/// The header's status text.
pub fn status_text(f: &FeedBlock) -> String {
    let b = &f.block;
    if f.gate.is_some() {
        return s::WAITING.into();
    }
    if b.kind == Kind::Tui {
        return if b.open { s::TUI_LIVE } else { s::TUI_DONE }.into();
    }
    if b.open {
        return s::RUNNING.into();
    }
    if b.origin == Origin::Earlier {
        return s::EARLIER.into();
    }
    let tail = match (b.kind, b.exit) {
        (_, Some(c)) => format!("종료 {c}"),
        (Kind::Command, None) => s::ENDED.into(),
        _ => s::DONE.into(),
    };
    match b.duration_ms() {
        Some(ms) => format!("{} · {tail}", duration_text(ms)),
        None => tail,
    }
}

/// What the header names the block by.
pub fn title_of(f: &FeedBlock) -> String {
    match f.block.kind {
        _ if !f.block.title.is_empty() => f.block.title.clone(),
        Kind::Tui => s::TUI_LIVE.into(),
        _ if f.block.origin == Origin::Earlier => s::EARLIER.into(),
        _ => s::OUTPUT.into(),
    }
}

/// The block's accessible name.
pub fn block_name(f: &FeedBlock, expanded: bool) -> String {
    let mut name = format!("{}, {}, {}", f.who, title_of(f), status_text(f));
    if let Some(g) = &f.gate {
        name.push_str(&format!(", {}: {}", s::APPROVAL, g.message));
    }
    name.push_str(if expanded { ", 펼쳐짐" } else { ", 접힘" });
    name
}

/// The three approval buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    Once,
    Always,
    Reject,
}

fn is_always(o: &str) -> bool {
    let o = o.to_lowercase();
    o.contains("always") || o.contains("항상") || o.contains("don't ask") || o.contains("다시 묻지")
}

/// The `gate.resolve` decision (and option) for a button; `None`: the gate
/// does not offer it (항상 허용 without an "always" option).
pub fn decision(g: &GateInfo, a: Approval) -> Option<(GateDecision, Option<String>)> {
    match a {
        Approval::Reject => Some((GateDecision::Reject, None)),
        Approval::Once if g.options.is_empty() => Some((GateDecision::Approve, None)),
        Approval::Once => {
            let once = g.options.iter().find(|o| {
                let l = o.to_lowercase();
                !is_always(o)
                    && (l.contains("once")
                        || l.contains("이번")
                        || l.contains("allow")
                        || l.contains("허용")
                        || l.contains("yes"))
            });
            once.or_else(|| g.options.iter().find(|o| !is_always(o)))
                .map(|o| (GateDecision::Approve, Some(o.clone())))
        }
        Approval::Always => g
            .options
            .iter()
            .find(|o| is_always(o))
            .map(|o| (GateDecision::Approve, Some(o.clone()))),
    }
}

/// Keyboard movement in the feed.
pub fn nav(feed: &[FeedBlock], from: Option<&BlockKey>, key: &str) -> Option<BlockKey> {
    let i = from.and_then(|k| feed.iter().position(|f| &f.key == k));
    let j = match (key, i) {
        (_, None) => feed.len().checked_sub(1),
        ("up", Some(i)) => i.checked_sub(1),
        ("down", Some(i)) => (i + 1 < feed.len()).then_some(i + 1),
        ("home", _) => (!feed.is_empty()).then_some(0),
        ("end", _) => feed.len().checked_sub(1),
        _ => None,
    }?;
    Some(feed[j].key.clone())
}

/// Next input target among the running terminals (Tab / Shift+Tab).
pub fn cycle_target(terms: &[&TerminalVm], current: Option<&str>, back: bool) -> Option<String> {
    let live: Vec<&str> = terms
        .iter()
        .filter(|t| t.is_running())
        .map(|t| t.id.as_str())
        .collect();
    if live.is_empty() {
        return None;
    }
    let i = current.and_then(|c| live.iter().position(|x| *x == c));
    let j = match (i, back) {
        (None, _) => 0,
        (Some(i), false) => (i + 1) % live.len(),
        (Some(i), true) => (i + live.len() - 1) % live.len(),
    };
    Some(live[j].to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// ───────────────────────────── module ─────────────────────────────

/// The layout module (registered in [`crate::layout_mode::LayoutRegistry::new`]).
pub struct Blocks;

impl LayoutModule for Blocks {
    fn mode(&self) -> LayoutMode {
        LayoutMode::BlockTerminal
    }
    fn render_body(
        &self,
        ws: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> AnyElement {
        if cx.try_global::<BlocksKeys>().is_none() {
            cx.bind_keys([
                KeyBinding::new("f6", SwitchRegion, Some(KEY_CONTEXT)),
                KeyBinding::new("tab", NextTarget, Some(INPUT_CONTEXT)),
                KeyBinding::new("shift-tab", PrevTarget, Some(INPUT_CONTEXT)),
                KeyBinding::new("escape", LeaveInput, Some(INPUT_CONTEXT)),
            ]);
            cx.set_global(BlocksKeys);
        }
        let feed = match cx.try_global::<FeedGlobal>().map(|g| g.0.clone()) {
            Some(f) => f,
            None => {
                let weak = cx.entity().downgrade();
                let fixture = ws.source.pipe() == "ui-fixture-no-daemon";
                let f = cx.new(|cx| Feed::new(weak, fixture, window, cx));
                // The app keeps the feed (and its view state) while it runs.
                cx.set_global(FeedGlobal(f.clone()));
                f
            }
        };
        let tk = crate::theme::tokens();
        let banner = match &ws.conn {
            crate::vm::ConnState::Reconnecting { attempt, error } => Some(
                crate::tr!(PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR; attempt,error;),
            ),
            crate::vm::ConnState::Connecting => {
                Some(crate::strings::CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED.to_string())
            }
            crate::vm::ConnState::Connected { .. } => None,
        };
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
            .when_some(ws.render_hook_banner(cx), |d, b| d.child(b))
            .child(div().flex().flex_col().flex_1().min_h_0().child(feed))
            .child(
                div()
                    .absolute()
                    .top(px(52.))
                    .right(px(16.))
                    .w(px(340.))
                    .child(ws.render_toasts(cx)),
            )
            .into_any_element()
    }
}

struct BlocksKeys;
impl Global for BlocksKeys {}
struct FeedGlobal(Entity<Feed>);
impl Global for FeedGlobal {}

#[cfg(test)]
/// The feed of this app, if the layout was shown once (tests).
pub fn feed(cx: &App) -> Option<Entity<Feed>> {
    cx.try_global::<FeedGlobal>().map(|g| g.0.clone())
}

// ───────────────────────────── view ─────────────────────────────

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
    ws: WeakEntity<Workspace>,
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
    focus: FocusHandle,
    task_focus: HashMap<String, FocusHandle>,
    block_focus: HashMap<BlockKey, FocusHandle>,
    pub input: Entity<InputState>,
    scroll: ScrollHandle,
    task_scroll: UniformListScrollHandle,
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

impl Feed {
    fn new(
        ws: WeakEntity<Workspace>,
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

    fn notify_all(&self, cx: &mut Context<Self>) {
        cx.notify();
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |_, cx| cx.notify());
        }
    }

    /// The task's terminals, the feed, and the gates (built from the workspace every frame).
    pub fn feed(&self, cx: &App) -> Vec<FeedBlock> {
        let Some(ws) = self.ws.upgrade() else {
            return Vec::new();
        };
        let w = ws.read(cx);
        let Some(task) = self
            .task
            .as_deref()
            .and_then(|id| w.tasks.iter().find(|t| t.id == id))
        else {
            return Vec::new();
        };
        let terms = task_terminals(task, &w.terminals);
        let gates = gates_by_terminal(&terms, &task.id, &w.orch.pending());
        let blocks: HashMap<String, Vec<Block>> = self
            .streams
            .iter()
            .map(|(k, s)| (k.clone(), s.blocks.blocks.clone()))
            .collect();
        build_feed(&terms, &blocks, &gates, self.filter.as_deref(), |t| {
            w.label_of(t)
        })
    }

    fn expanded(&self, f: &FeedBlock) -> bool {
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

    /// Attach to the selected task's terminals, detach from the rest; apply status changes.
    fn sync_streams(&mut self, cx: &mut Context<Self>) {
        let Some(ws) = self.ws.upgrade() else { return };
        let (terms, pipe): (Vec<TerminalVm>, String) = {
            let w = ws.read(cx);
            let terms = self
                .task
                .as_deref()
                .and_then(|id| w.tasks.iter().find(|t| t.id == id))
                .map(|t| {
                    task_terminals(t, &w.terminals)
                        .into_iter()
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            (terms, w.source.pipe().to_string())
        };
        self.streams
            .retain(|id, _| terms.iter().any(|t| &t.id == id));
        for t in &terms {
            if !self.streams.contains_key(&t.id) {
                let stream = if self.fixture {
                    let mut blocks = TermBlocks::new(100, 30);
                    blocks.reset(b"", 100, 30, 0);
                    fixture_feed(&t.id, &mut blocks);
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
                    s.blocks.status(label, finished, now_ms());
                }
            }
            if let Liveness::Exited { code, .. } = t.live
                && !s.ended
            {
                s.ended = true;
                s.blocks.exited(code, now_ms());
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
        let now = now_ms();
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

    pub fn copy(&mut self, key: &BlockKey, cx: &mut Context<Self>) {
        let Some(f) = self.feed(cx).into_iter().find(|f| &f.key == key) else {
            return;
        };
        let mut text = f.block.text();
        if f.block.kind != Kind::Command {
            text = format!("{}\n{text}", title_of(&f));
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        if let Some(ws) = self.ws.upgrade() {
            ws.update(cx, |w, cx| {
                w.raise_toast(ToastKind::Success, s::COPIED, f.who.clone(), cx)
            });
        }
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

    /// The workbench's own panel for the raw terminal when it has one (no new attach), else a feed-owned one.
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
        ws.read(cx).terminal(&id)?;
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
        let Some(ws) = self.ws.upgrade() else {
            return Vec::new();
        };
        let w = ws.read(cx);
        self.task
            .as_deref()
            .and_then(|id| w.tasks.iter().find(|t| t.id == id))
            .map(|t| {
                task_terminals(t, &w.terminals)
                    .into_iter()
                    .cloned()
                    .collect()
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

    /// Approve / reject through the gate path (`gate.resolve`), as the gates inbox does.
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

    fn on_block_key(
        &mut self,
        f: &FeedBlock,
        ev: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let k = &ev.keystroke;
        if k.modifiers.control && k.key == "c" {
            self.copy(&f.key, cx);
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
            "c" => self.copy(&f.key, cx),
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

    fn on_task_key(
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
        let Some(ws) = self.ws.upgrade() else {
            return false;
        };
        let ids: Vec<String> = ws.read(cx).tasks.iter().map(|t| t.id.clone()).collect();
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
            if let Some(task) = ws.read(cx).terminal(&id).and_then(|t| t.task_id.clone()) {
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

    /// Item height of a task row: grows with the UI font.
    fn row_height(cx: &App) -> f32 {
        let line = (Self::ui_font(cx) * 1.5).ceil();
        pod_ui::hit_area().max(line + 8.)
    }

    fn ui_font(cx: &App) -> f32 {
        let user = cx
            .try_global::<crate::theme::Settings>()
            .map_or(13., |s| s.font_size);
        crate::theme::ui_font_size(user, pod_ui::themes::metrics().density.ui_font)
    }
}

impl Focusable for Feed {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// A text button (Enter/Space, Korean name); `key` is the shortcut hint shown after the label.
#[allow(clippy::too_many_arguments)]
fn text_button(
    id: impl Into<ElementId>,
    label: &'static str,
    key: Option<&'static str>,
    icon: Option<pod_ui::Icon>,
    primary: bool,
    enabled: bool,
    tab: bool,
    tip: Option<&'static str>,
    on: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    let on = Rc::new(on);
    let keyed = on.clone();
    let (fg, bg, border) = if !enabled {
        (t.text_disabled, t.bg_surface, t.border_subtle)
    } else if primary {
        (
            t.action_primary_fg,
            t.action_primary_bg,
            t.action_primary_bg,
        )
    } else {
        // Mono has no lines: a secondary button needs a face of its own.
        (
            t.text_primary,
            if m.surface == pod_ui::themes::Surface::Mono {
                t.bg_hover
            } else {
                t.bg_surface
            },
            t.border_control,
        )
    };
    let name = match key {
        Some(k) => format!("{label} ({k})"),
        None => label.to_string(),
    };
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(name)
        .when(enabled && tab, |d| d.tab_index(0))
        .when(!enabled || !tab, |d| d.tab_stop(false))
        .h(px(pod_ui::hit_area()))
        .px(px(10.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_2()
        .rounded(px(m.radius.control))
        .border(px(m.border_width))
        .border_color(rgb(border))
        .bg(rgb(bg))
        .text_color(rgb(fg))
        .font_weight(FontWeight::SEMIBOLD)
        .when_some(icon, |d, i| d.child(pod_ui::icon_sized(i, 14., fg)))
        .child(label)
        .when_some(key, |d, k| {
            d.child(
                div()
                    .px_1()
                    .rounded(px(m.radius.control.min(4.)))
                    .border(px(1.))
                    .border_color(rgb(if primary { fg } else { t.border_subtle }))
                    .text_xs()
                    .font_weight(FontWeight::NORMAL)
                    .child(k),
            )
        })
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(move |s| {
                    if primary {
                        s.bg(rgb(t.action_primary_hover))
                    } else {
                        s.bg(rgb(t.bg_hover))
                    }
                })
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    on(window, cx);
                })
                .on_key_down(move |e, window, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space")
                        && !e.keystroke.modifiers.modified()
                    {
                        cx.stop_propagation();
                        keyed(window, cx);
                    }
                })
        })
        .when_some(tip, |d, tip| {
            d.tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(tip).build(window, cx)
            })
        })
}

/// What one block element needs besides the block.
struct BlockCtx {
    feed: WeakEntity<Feed>,
    selected: Option<BlockKey>,
    mono: SharedString,
}

fn status_icon(f: &FeedBlock, t: &pod_ui::Tokens) -> (pod_ui::Icon, u32) {
    let b = &f.block;
    if f.gate.is_some() {
        (pod_ui::Icon::ShieldQuestion, t.warning_fg)
    } else if b.kind == Kind::Tui {
        (
            pod_ui::Icon::Monitor,
            if b.open { t.info_fg } else { t.text_muted },
        )
    } else if b.open {
        (pod_ui::Icon::LoaderCircle, t.info_fg)
    } else if b.failed() {
        (pod_ui::Icon::OctagonX, t.danger_fg)
    } else if b.origin == Origin::Earlier {
        (pod_ui::Icon::Circle, t.text_muted)
    } else {
        (pod_ui::Icon::CircleCheck, t.success_fg)
    }
}

/// Body lines shown for an expanded block (the newest).
const SHOWN_LINES: usize = 200;

fn render_block(f: &FeedBlock, expanded: bool, ctx: &BlockCtx, focus: FocusHandle) -> AnyElement {
    let t = crate::theme::tokens();
    let m = pod_ui::themes::metrics();
    let mode = crate::theme::mode();
    use pod_ui::themes::Surface;
    let selected = ctx.selected.as_ref() == Some(&f.key);
    let dom = f.key.dom();
    let (glyph, glyph_fg) = status_icon(f, &t);
    let waiting = f.gate.is_some();
    let card_bg = match m.surface {
        Surface::Flat | Surface::Mono => t.bg_surface,
        _ => t.bg_raised,
    };
    let border = if waiting {
        t.warning_fg
    } else if selected {
        t.accent_fg
    } else if m.surface == Surface::Mono {
        card_bg
    } else {
        t.border_subtle
    };
    let feed = ctx.feed.clone();
    let mono = ctx.mono.clone();
    let act = |g: fn(&mut Feed, &FeedBlock, &mut Window, &mut Context<Feed>)| {
        let (feed, fb) = (feed.clone(), f.clone());
        move |window: &mut Window, cx: &mut App| {
            let _ = feed.update(cx, |me, cx| g(me, &fb, window, cx));
        }
    };

    // ── header ──
    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .px(px(12.))
        .min_h(px(m.density.panel_header.max(pod_ui::hit_area() + 4.)))
        .child(pod_ui::icon_sized(
            if expanded {
                pod_ui::Icon::ChevronDown
            } else {
                pod_ui::Icon::ChevronRight
            },
            14.,
            t.text_muted,
        ))
        .child(pod_ui::icon(glyph, glyph_fg))
        .child(crate::agent_kind::mark_el(
            &format!("blocks-{}", f.key.dom()),
            &f.kind,
            14.,
        ))
        .child(
            div()
                .flex_shrink_0()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_primary))
                .child(f.who.clone()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(mono.clone())
                .text_color(rgb(t.text_primary))
                .when(f.block.kind == Kind::Command, |d| {
                    d.font_weight(FontWeight::SEMIBOLD)
                })
                .child(title_of(f)),
        )
        .child(
            div()
                .qa_id({
                    let dom = dom.clone();
                    move || format!("blocks-status-{dom}")
                })
                .flex_shrink_0()
                .text_sm()
                .text_color(rgb(if f.block.failed() {
                    t.danger_fg
                } else if waiting {
                    t.warning_fg
                } else {
                    t.text_muted
                }))
                .child(status_text(f)),
        )
        .when(f.block.kind == Kind::Tui, |d| {
            d.child(text_button(
                SharedString::from(format!("blocks-full-{dom}")),
                s::FULL_TERMINAL,
                None,
                Some(pod_ui::Icon::Maximize2),
                false,
                true,
                selected,
                None,
                act(|me, fb, window, cx| me.show_raw(Some(fb.key.terminal.clone()), window, cx)),
            ))
        })
        .child({
            let b = pod_ui::IconButton::new(
                SharedString::from(format!("blocks-copy-{dom}")),
                pod_ui::Icon::Copy,
                s::COPY,
            )
            .tooltip("복사 (C)")
            .render(mode, act(|me, fb, _, cx| me.copy(&fb.key, cx)));
            if selected { b } else { b.tab_stop(false) }
        });

    // ── body ──
    let body = expanded.then(|| {
        let lines = &f.block.lines;
        let skip = lines.len().saturating_sub(SHOWN_LINES);
        let omitted = skip + f.block.dropped;
        div()
            .flex()
            .flex_col()
            .px(px(12.))
            .py(px(8.))
            .border_t(px(m.border_width))
            .border_color(rgb(t.border_subtle))
            .font_family(mono.clone())
            .text_sm()
            .text_color(rgb(t.text_secondary))
            .when(omitted > 0, |d| {
                d.child(
                    div()
                        .text_color(rgb(t.text_muted))
                        .child(format!("… 앞 {omitted}{}", s::OMITTED)),
                )
            })
            .children(lines[skip..].iter().map(|l| {
                let (fg, text) = line_color(l, &t);
                div()
                    .min_h(px(4.))
                    .text_color(rgb(fg))
                    .child(if text.is_empty() {
                        " ".to_string()
                    } else {
                        text
                    })
            }))
            .when(lines.is_empty() && f.gate.is_none(), |d| {
                d.child(div().text_color(rgb(t.text_muted)).child(if f.block.open {
                    "출력 기다리는 중…"
                } else {
                    "출력 없음"
                }))
            })
    });

    // ── approval (gate) ──
    let approval = f.gate.clone().filter(|_| expanded).map(|g| {
        // Buttons stay focusable while the request runs (a second press is ignored).
        let always_ok = decision(&g, Approval::Always).is_some();
        let mk = |a: Approval| {
            let (feed, key, g) = (feed.clone(), f.key.clone(), g.clone());
            move |window: &mut Window, cx: &mut App| {
                let _ = feed.update(cx, |me, cx| {
                    me.resolve(key.clone(), g.clone(), a, window, cx)
                });
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px(px(12.))
            .pb(px(10.))
            .pt(px(if expanded && !f.block.lines.is_empty() {
                0.
            } else {
                8.
            }))
            .child(
                div()
                    .text_color(rgb(t.warning_fg))
                    .child(format!("{} · {}", g.title, g.message)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(text_button(
                        SharedString::from(format!("blocks-once-{dom}")),
                        s::ALLOW_ONCE,
                        Some("Y"),
                        Some(pod_ui::Icon::Check),
                        true,
                        true,
                        selected,
                        None,
                        mk(Approval::Once),
                    ))
                    .child(text_button(
                        SharedString::from(format!("blocks-always-{dom}")),
                        s::ALLOW_ALWAYS,
                        Some("A"),
                        None,
                        false,
                        always_ok,
                        selected,
                        (!always_ok).then_some(s::NO_ALWAYS_TIP),
                        mk(Approval::Always),
                    ))
                    .child(text_button(
                        SharedString::from(format!("blocks-reject-{dom}")),
                        s::REJECT,
                        Some("N"),
                        None,
                        false,
                        true,
                        selected,
                        None,
                        mk(Approval::Reject),
                    )),
            )
    });

    let name = block_name(f, expanded);
    let fb = f.clone();
    let key_feed = feed.clone();
    let click_feed = feed.clone();
    let click_key = f.key.clone();
    div()
        .id(SharedString::from(format!("blocks-block-{dom}")))
        .qa_id({
            let dom = dom.clone();
            move || format!("blocks-block-{dom}")
        })
        .track_focus(&focus.tab_stop(selected))
        .role(Role::ListItem)
        .aria_label(name)
        .aria_selected(selected)
        .aria_expanded(expanded)
        .w_full()
        // Full height in the scrolling feed: never squeezed (the body is never clipped).
        .flex_shrink_0()
        .flex()
        .flex_col()
        .relative()
        .overflow_hidden()
        .rounded(px(m.radius.card))
        .bg(rgb(if selected && !waiting {
            t.bg_selected
        } else {
            card_bg
        }))
        .border(px(if waiting || selected {
            m.border_width.max(2.)
        } else {
            m.border_width
        }))
        .border_color(rgb(border))
        // Waiting stripe: never the only carrier (icon + 승인 대기 text too).
        .when(waiting, |d| {
            d.child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(4.))
                    .bg(rgb(t.warning_fg)),
            )
        })
        .when(
            m.shadow != pod_ui::themes::Shadow::None && m.surface != Surface::Flat,
            |d| d.shadow(pod_ui::tokens::elevation(1, mode)),
        )
        .focus_visible(move |s| pod_ui::focus_style(s, mode))
        .child(
            div()
                .id(SharedString::from(format!("blocks-head-{dom}")))
                .cursor_pointer()
                .hover(move |s| s.bg(rgb(t.bg_hover)))
                .on_click(move |_, window, cx| {
                    let _ = click_feed.update(cx, |me, cx| {
                        me.select(&click_key, window, cx);
                        me.toggle(&click_key, cx);
                    });
                })
                .child(header),
        )
        .when_some(body, |d, b| d.child(b))
        .when_some(approval, |d, a| d.child(a))
        .on_key_down(move |e, window, cx| {
            if key_feed
                .update(cx, |me, cx| me.on_block_key(&fb, e, window, cx))
                .unwrap_or(false)
            {
                cx.stop_propagation();
            }
        })
        .into_any_element()
}

/// Diff-ish colors for body lines (`+`, `-`, `M`, `??`, `error`), text unchanged.
fn line_color(l: &str, t: &pod_ui::Tokens) -> (u32, String) {
    let trimmed = l.trim_start();
    let fg = if trimmed.starts_with("error") || trimmed.starts_with("오류") {
        t.danger_fg
    } else if trimmed.starts_with('+') && !trimmed.starts_with("++") {
        t.diff_add_fg
    } else if trimmed.starts_with('-') && !trimmed.starts_with("--") {
        t.diff_del_fg
    } else if trimmed.starts_with("warning") {
        t.warning_fg
    } else {
        t.text_secondary
    };
    (fg, l.to_string())
}

impl Render for Feed {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else {
            return div().into_any_element();
        };
        // The task: the one chosen, else the focused terminal's, else the first.
        let (focused, first_task) = {
            let w = ws.read(cx);
            (
                w.focused_terminal.clone(),
                w.tasks.first().map(|t| t.id.clone()),
            )
        };
        if focused != self.seen_focus {
            self.seen_focus = focused.clone();
            if let Some(task) = focused
                .as_deref()
                .and_then(|id| ws.read(cx).terminal(id))
                .and_then(|t| t.task_id.clone())
            {
                self.set_task(&task, cx);
                self.target = focused.clone();
            }
        }
        if self
            .task
            .as_deref()
            .is_none_or(|id| !ws.read(cx).tasks.iter().any(|t| t.id == id))
            && let Some(id) = first_task
        {
            self.task = Some(id);
        }
        self.sync_streams(cx);
        if self.fixture && !self.fixture_gate && self.task.is_some() {
            self.fixture_gate = true;
            ws.update(cx, |w, _| {
                w.orch.apply(fixture_gate());
            });
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

        let t = crate::theme::tokens();
        let m = pod_ui::themes::metrics();
        let mode = crate::theme::mode();
        use pod_ui::themes::Surface;
        let gap = m.gap.max(8.);
        let me = cx.entity().downgrade();
        let mono = gpui_kit::component::theme::Theme::global(cx)
            .mono_font_family
            .clone();
        let (tasks, label_of, pending) = {
            let w = ws.read(cx);
            let labels: HashMap<String, String> = terms
                .iter()
                .map(|x| (x.id.clone(), w.label_of(x)))
                .collect();
            (w.tasks.clone(), labels, w.orch.pending())
        };
        let task = self
            .task
            .as_deref()
            .and_then(|id| tasks.iter().find(|t| t.id == id))
            .cloned();
        let needs = |task: &TaskVm| -> usize {
            let blocked = task
                .terminals
                .iter()
                .filter(|id| {
                    terms.iter().chain(std::iter::empty()).any(|x| {
                        &x.id == *id
                            && x.status
                                .as_ref()
                                .is_some_and(|s| s.state == AgentState::Blocked)
                    })
                })
                .count();
            let gates = pending
                .iter()
                .filter(|g| {
                    g.task.as_deref() == Some(task.id.as_str())
                        || g.terminal
                            .as_ref()
                            .is_some_and(|x| task.terminals.contains(x))
                })
                .count();
            gates.max(blocked)
        };

        // ── sidebar: tasks + agent chips ──
        for k in &tasks {
            self.task_focus(&k.id, cx);
        }
        let row_h = Self::row_height(cx);
        let side_bg = match m.surface {
            Surface::Flat | Surface::Mono => t.bg_surface,
            _ => t.bg_surface,
        };
        let task_rows = {
            let me = me.clone();
            let tasks = Rc::new(tasks.clone());
            let focus = self.task_focus.clone();
            let current = self.task.clone();
            let counts: HashMap<String, usize> = tasks
                .iter()
                .map(|k| {
                    (
                        k.id.clone(),
                        pending
                            .iter()
                            .filter(|g| {
                                g.task.as_deref() == Some(k.id.as_str())
                                    || g.terminal.as_ref().is_some_and(|x| k.terminals.contains(x))
                            })
                            .count(),
                    )
                })
                .collect();
            uniform_list("blocks-tasks", tasks.len(), move |range, _, _| {
                range
                    .map(|i| {
                        let k = &tasks[i];
                        let on = current.as_deref() == Some(k.id.as_str());
                        let n = counts.get(&k.id).copied().unwrap_or(0);
                        let dot = match k.status.as_ref().map(|s| s.state) {
                            Some(AgentState::Blocked) => t.warning_fg,
                            Some(AgentState::Error) => t.danger_fg,
                            Some(AgentState::Working) => t.info_fg,
                            Some(AgentState::Done) => t.success_fg,
                            _ => t.text_muted,
                        };
                        let (me1, me2, id1, id2) =
                            (me.clone(), me.clone(), k.id.clone(), k.id.clone());
                        let name = if n > 0 {
                            format!("{}, 승인 대기 {n}개", k.title)
                        } else {
                            k.title.clone()
                        };
                        div()
                            .id(SharedString::from(format!("blocks-task-{}", k.id)))
                            .qa_id({
                                let id = k.id.clone();
                                move || format!("blocks-task-{id}")
                            })
                            .when_some(focus.get(&k.id).cloned(), |d, f| {
                                d.track_focus(&f.tab_stop(on))
                            })
                            .role(Role::ListItem)
                            .aria_label(name)
                            .aria_selected(on)
                            .h(px(row_h))
                            .px_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(m.radius.control))
                            .when(on, |d| {
                                d.bg(rgb(t.bg_selected)).font_weight(FontWeight::SEMIBOLD)
                            })
                            .hover(move |s| if on { s } else { s.bg(rgb(t.bg_hover)) })
                            .focus_visible(move |s| pod_ui::focus_style(s, mode))
                            .cursor_pointer()
                            .child(
                                div()
                                    .size(px(8.))
                                    .flex_shrink_0()
                                    .rounded_full()
                                    .bg(rgb(dot)),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(t.text_primary))
                                    .child(k.title.clone()),
                            )
                            .when(n > 0, |d| {
                                d.child(
                                    div()
                                        .flex_shrink_0()
                                        .px_1()
                                        .rounded(px(m.radius.control))
                                        .bg(rgb(t.warning_bg))
                                        .text_color(rgb(t.warning_fg))
                                        .text_xs()
                                        .child(n.to_string()),
                                )
                            })
                            .on_click(move |_, window, cx| {
                                let _ = me1.update(cx, |f, cx| {
                                    f.set_task(&id1, cx);
                                    let h = f.task_focus(&id1, cx);
                                    h.focus(window, cx);
                                });
                            })
                            .on_key_down(move |e, window, cx| {
                                if me2
                                    .update(cx, |f, cx| f.on_task_key(&id2, e, window, cx))
                                    .unwrap_or(false)
                                {
                                    cx.stop_propagation();
                                }
                            })
                            .into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .track_scroll(&self.task_scroll)
            .flex_1()
            .min_h_0()
        };
        let chip = |id: SharedString, label: String, on: bool, value: Option<String>| {
            let me = me.clone();
            let (v1, v2) = (value.clone(), value);
            let me2 = me.clone();
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(format!("{label}{}", if on { ", 선택됨" } else { "" }))
                .aria_selected(on)
                .tab_index(0)
                .min_h(px(pod_ui::hit_area()))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(m.radius.control.max(10.)))
                .border(px(m.border_width))
                .border_color(rgb(if on { t.accent_fg } else { t.border_subtle }))
                .when(on, |d| {
                    d.bg(rgb(t.bg_selected)).font_weight(FontWeight::SEMIBOLD)
                })
                .text_sm()
                .text_color(rgb(if on { t.text_primary } else { t.text_secondary }))
                .hover(|s| s.bg(rgb(t.bg_hover)))
                .focus_visible(move |s| pod_ui::focus_style(s, mode))
                .cursor_pointer()
                .child(label)
                .on_click(move |_, _, cx| {
                    let _ = me.update(cx, |f, cx| {
                        f.filter = v1.clone();
                        f.notify_all(cx);
                    });
                })
                .on_key_down(move |e, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space")
                        && !e.keystroke.modifiers.modified()
                    {
                        cx.stop_propagation();
                        let _ = me2.update(cx, |f, cx| {
                            f.filter = v2.clone();
                            f.notify_all(cx);
                        });
                    }
                })
        };
        let mut chips = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .px(px(gap))
            .py(px(gap))
            .child(chip(
                "blocks-filter-all".into(),
                format!("{} {}", s::ALL_AGENTS, terms.len()),
                self.filter.is_none(),
                None,
            ));
        for x in &terms {
            let label = who(x, label_of.get(&x.id).map(String::as_str).unwrap_or(&x.id));
            chips = chips.child(chip(
                SharedString::from(format!("blocks-filter-{}", x.id)),
                label,
                self.filter.as_deref() == Some(x.id.as_str()),
                Some(x.id.clone()),
            ));
        }
        let sidebar = div()
            .id("blocks-sidebar")
            .role(Role::Navigation)
            .aria_label(s::TASKS)
            .w(px(m.density.sidebar.clamp(220., 320.)))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(rgb(side_bg))
            .border_r(px(m.border_width))
            .border_color(rgb(t.border_subtle))
            .child(
                div()
                    .px(px(gap + 4.))
                    .min_h(px(m.density.panel_header.max(36.)))
                    .flex()
                    .items_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(s::TASKS),
            )
            .child(
                div()
                    .id("blocks-task-list")
                    .role(Role::List)
                    .aria_label(format!("{}, {}개", s::TASKS, tasks.len()))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .child(task_rows),
            )
            .child(
                div()
                    .border_t(px(m.border_width))
                    .border_color(rgb(t.border_subtle))
                    .child(chips),
            );

        // ── header ──
        let needs_n = task.as_ref().map_or(0, needs);
        let meta = format!(
            "블록 {} · 에이전트 {} · {}",
            feed.len(),
            terms.len(),
            s::CHRONO
        );
        let raw_me = me.clone();
        let raw = self.raw;
        let header = div()
            .flex()
            .items_center()
            .gap_3()
            .px(px(gap + 8.))
            .min_h(px(m.density.panel_header.max(40.)))
            .border_b(px(m.border_width))
            .border_color(rgb(t.border_subtle))
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(420.))
                    .truncate()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(
                        task.as_ref()
                            .map_or(s::NO_TASK.to_string(), |k| k.title.clone()),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(rgb(t.text_muted))
                    .child(meta),
            )
            .when(needs_n > 0, |d| {
                d.child(
                    div()
                        .qa_id(|| "blocks-needs".into())
                        .flex_shrink_0()
                        .px_2()
                        .rounded(px(m.radius.control))
                        .bg(rgb(t.warning_bg))
                        .text_color(rgb(t.warning_fg))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(format!("{} {needs_n}", s::NEEDS_YOU)),
                )
            })
            .child(
                pod_ui::IconButton::new(
                    "blocks-raw",
                    if raw {
                        pod_ui::Icon::Rows2
                    } else {
                        pod_ui::Icon::SquareTerminal
                    },
                    if raw { s::RAW_OFF } else { s::RAW_ON },
                )
                .render(mode, move |window, cx| {
                    let _ = raw_me.update(cx, |f, cx| {
                        if f.raw {
                            f.hide_raw(window, cx)
                        } else {
                            f.show_raw(None, window, cx)
                        }
                    });
                }),
            );

        // ── feed or raw terminal ──
        let body = if self.raw {
            match self.raw_panel(window, cx) {
                Some(p) => div()
                    .id("blocks-raw-view")
                    .qa_id(|| "blocks-raw-view".into())
                    .flex_1()
                    .min_h_0()
                    .child(p)
                    .into_any_element(),
                None => div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(t.text_muted))
                    .child(s::RAW_EMPTY)
                    .into_any_element(),
            }
        } else {
            let ctx = BlockCtx {
                feed: me.clone(),
                selected: self.selected.clone(),
                mono: mono.clone(),
            };
            let list = div()
                .id("blocks-feed")
                .qa_id(|| "blocks-feed".into())
                .role(Role::List)
                .aria_label(format!("{}, 블록 {}개", s::FEED, feed.len()))
                .track_focus(&self.focus.clone().tab_stop(feed.is_empty()))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .flex()
                .flex_col()
                .gap(px(gap))
                .p(px(gap + 4.))
                .when(feed.is_empty(), |d| {
                    d.child(
                        div()
                            .pt_4()
                            .text_color(rgb(t.text_muted))
                            .child(s::NO_BLOCKS),
                    )
                })
                .children(
                    feed.iter()
                        .zip(focus)
                        .map(|(f, h)| render_block(f, self.expanded(f), &ctx, h)),
                );
            list.into_any_element()
        };

        // ── input ──
        let target = self
            .target
            .as_ref()
            .and_then(|id| terms.iter().find(|x| &x.id == id));
        let target_name =
            target.map(|x| who(x, label_of.get(&x.id).map(String::as_str).unwrap_or(&x.id)));
        let placeholder: SharedString = match &target_name {
            Some(n) => format!("{n}에게 입력 · Tab으로 대상 변경").into(),
            None => s::NO_TARGET.into(),
        };
        let input = self.input.clone();
        if self.placeholder != placeholder {
            self.placeholder = placeholder.clone();
            input.update(cx, |i, cx| i.set_placeholder(placeholder, window, cx));
        }
        let input_row =
            div()
                .id("blocks-input")
                .qa_id(|| "blocks-input".into())
                .key_context(INPUT_CONTEXT)
                .on_action(cx.listener(|f, _: &NextTarget, _, cx| f.cycle(false, cx)))
                .on_action(cx.listener(|f, _: &PrevTarget, _, cx| f.cycle(true, cx)))
                .on_action(cx.listener(|f, _: &LeaveInput, window, cx| {
                    f.focus_region(Region::Feed, window, cx)
                }))
                .role(Role::Group)
                .aria_label(format!(
                    "입력: {} · {}",
                    target_name.clone().unwrap_or_else(|| s::NO_TARGET.into()),
                    s::INPUT_HINT
                ))
                .flex_shrink_0()
                .m(px(gap + 4.))
                .mt(px(0.))
                .flex()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded(px(m.radius.card))
                .border(px(m.border_width.max(1.)))
                .border_color(rgb(t.border_control))
                .bg(rgb(match m.surface {
                    Surface::Flat | Surface::Mono => t.bg_surface,
                    _ => t.bg_raised,
                }))
                .child(pod_ui::icon(pod_ui::Icon::SquareTerminal, t.text_muted))
                .child(
                    div().flex_1().min_w_0().child(
                        Input::new(&input)
                            .appearance(false)
                            .disabled(target.is_none()),
                    ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .px_1()
                        .rounded(px(m.radius.control.min(4.)))
                        .border(px(1.))
                        .border_color(rgb(t.border_subtle))
                        .text_xs()
                        .text_color(rgb(t.text_muted))
                        .child("Enter"),
                );

        let main = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header)
            .child(body)
            .when(!self.raw, |d| d.child(input_row));

        div()
            .id("blocks-layout")
            .qa_id(|| "blocks-layout".into())
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(|f, _: &SwitchRegion, window, cx| f.switch_region(window, cx)))
            .on_key_down(cx.listener(|f, e: &KeyDownEvent, window, cx| {
                // The empty feed itself: arrows reach the newest block.
                if f.focus.is_focused(window)
                    && matches!(e.keystroke.key.as_str(), "up" | "down" | "home" | "end")
                {
                    f.focus_region(Region::Feed, window, cx);
                }
                // Esc in the raw terminal view goes back to the blocks only from its header; the terminal keeps Esc.
            }))
            .size_full()
            .flex()
            .min_h_0()
            .text_color(rgb(t.text_primary))
            .child(sidebar)
            .child(main)
            .into_any_element()
    }
}

// ───────────────────────────── fixture ─────────────────────────────

/// Scripted output for the read-only UI fixture (visual captures): Pod's
/// shell marks around a few commands, so the feed looks populated without a daemon.
fn fixture_feed(id: &str, b: &mut TermBlocks) {
    const A: &str = "\x1b]133;A\x07";
    const B: &str = "\x1b]133;B\x07";
    const C: &str = "\x1b]133;C\x07";
    let n: usize = id
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0);
    let base = now_ms().saturating_sub(120_000);
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

/// The fixture's one pending approval (block terminal captures only).
fn fixture_gate() -> GateInfo {
    GateInfo {
        id: "g-fixture".into(),
        title: "명령 실행".into(),
        message: "cargo clippy 실행 승인이 필요합니다.".into(),
        task: Some("sample/task-000".into()),
        group: None,
        terminal: Some("fixture-1".into()),
        state: p::GateState::Pending,
        created_ms: now_ms().saturating_sub(12_000),
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

// ───────────────────────────── tests ─────────────────────────────

#[cfg(test)]
mod tests {
    use super::{
        Approval, BlockKey, FeedBlock, build_feed, cycle_target, decision, default_expanded,
        gates_by_terminal, nav, status_text, task_terminals, who,
    };
    use crate::layouts::blocks_model::{Block, Kind, Origin};
    use crate::source::TestBackend;
    use core::prelude::v1::test;
    use podd::protocol::{GateDecision, GateInfo, GateState};
    use std::collections::HashMap;

    pub(crate) fn gate(
        id: &str,
        terminal: Option<&str>,
        task: Option<&str>,
        options: &[&str],
    ) -> GateInfo {
        GateInfo {
            id: id.into(),
            title: "명령 실행".into(),
            message: "cargo clippy 실행 승인이 필요합니다.".into(),
            task: task.map(Into::into),
            group: None,
            terminal: terminal.map(Into::into),
            state: GateState::Pending,
            created_ms: 5,
            resolved_ms: None,
            note: None,
            expires_ms: None,
            run: None,
            item: None,
            options: options.iter().map(|s| s.to_string()).collect(),
            choice: None,
            requested_by: None,
            decided_by: None,
        }
    }

    fn block(
        seq: u64,
        kind: Kind,
        title: &str,
        at: Option<u64>,
        end: Option<u64>,
        exit: Option<i32>,
    ) -> Block {
        Block {
            seq,
            kind,
            origin: if at.is_some() {
                Origin::Marks
            } else {
                Origin::Earlier
            },
            title: title.into(),
            lines: vec!["줄".into()],
            dropped: 0,
            started_ms: at,
            ended_ms: end,
            exit,
            open: end.is_none() && at.is_some(),
        }
    }

    #[test]
    fn feed_merges_terminals_chronologically_with_gates_on_the_newest_block() {
        let task = TestBackend::task("pod/a", "글자 정렬", "pod", &["a1", "a2", "a3"]);
        let mut terms = vec![
            TestBackend::terminal("a1", "구현", Some("pod/a"), "working"),
            TestBackend::terminal("a2", "리뷰", Some("pod/a"), "blocked"),
            TestBackend::terminal("a3", "빌드", Some("pod/a"), "done"),
        ];
        terms[0].program = "claude".into();
        terms[1].program = "codex".into();
        let refs = task_terminals(&task, &terms);
        assert_eq!(who(refs[0], "구현"), "Claude · 구현");
        assert_eq!(who(refs[2], "빌드"), "셸 · 빌드");
        let mut blocks = HashMap::new();
        blocks.insert(
            "a1".to_string(),
            vec![
                block(0, Kind::Output, "", None, None, None),
                block(
                    1,
                    Kind::Command,
                    "cargo test",
                    Some(100),
                    Some(3200),
                    Some(0),
                ),
                block(2, Kind::Step, "작업 중", Some(5000), None, None),
            ],
        );
        blocks.insert(
            "a3".to_string(),
            vec![block(
                0,
                Kind::Command,
                "git status --short",
                Some(4000),
                Some(4400),
                Some(0),
            )],
        );
        let gates = gates_by_terminal(
            &refs,
            "pod/a",
            &[
                gate("g1", None, Some("pod/a"), &[]),
                gate("g2", Some("a2"), None, &[]),
            ],
        );
        assert_eq!(
            (gates["a1"].id.as_str(), gates["a2"].id.as_str()),
            ("g1", "g2"),
            "task gate → first terminal, own gate wins"
        );
        let feed = build_feed(&refs, &blocks, &gates, None, |t| {
            t.name.clone().unwrap_or_default()
        });
        let order: Vec<(&str, u64)> = feed
            .iter()
            .map(|f| (f.key.terminal.as_str(), f.key.seq))
            .collect();
        assert_eq!(
            order,
            [("a1", 0), ("a2", u64::MAX), ("a1", 1), ("a3", 0), ("a1", 2)],
            "earlier output first, then by start time"
        );
        let a1_last = feed
            .iter()
            .find(|f| {
                f.key
                    == BlockKey {
                        terminal: "a1".into(),
                        seq: 2,
                    }
            })
            .unwrap();
        assert!(a1_last.gate.is_some() && a1_last.last);
        assert!(
            feed.iter()
                .filter(|f| f.key.terminal == "a1")
                .filter(|f| f.gate.is_some())
                .count()
                == 1,
            "only the newest block carries the gate"
        );
        let approval = &feed[1];
        assert_eq!(
            (
                approval.block.title.as_str(),
                approval.gate.as_ref().map(|g| g.id.as_str())
            ),
            ("승인 요청", Some("g2")),
            "a gate without blocks still shows"
        );
        // Filter: one terminal.
        let only = build_feed(&refs, &blocks, &gates, Some("a3"), |t| t.id.clone());
        assert_eq!(only.len(), 1);
        // Status texts and default folds.
        let st: Vec<String> = feed.iter().map(status_text).collect();
        assert_eq!(
            st,
            [
                "이전 출력",
                "승인 대기",
                "3.1초 · 종료 0",
                "0.4초 · 종료 0",
                "승인 대기"
            ]
        );
        let exp: Vec<bool> = feed.iter().map(default_expanded).collect();
        assert_eq!(
            exp,
            [false, true, false, true, true],
            "finished older blocks start folded"
        );
    }

    #[test]
    fn approval_buttons_map_to_gate_resolve_decisions() {
        let plain = gate("g", None, None, &[]);
        assert_eq!(
            decision(&plain, Approval::Once),
            Some((GateDecision::Approve, None))
        );
        assert_eq!(
            decision(&plain, Approval::Always),
            None,
            "항상 허용 only when the gate offers it"
        );
        assert_eq!(
            decision(&plain, Approval::Reject),
            Some((GateDecision::Reject, None))
        );
        let opts = gate("g", None, None, &["항상 허용", "이번만 허용"]);
        assert_eq!(
            decision(&opts, Approval::Once),
            Some((GateDecision::Approve, Some("이번만 허용".into())))
        );
        assert_eq!(
            decision(&opts, Approval::Always),
            Some((GateDecision::Approve, Some("항상 허용".into())))
        );
        let en = gate("g", None, None, &["Yes", "Yes, and don't ask again"]);
        assert_eq!(
            decision(&en, Approval::Once).map(|d| d.1),
            Some(Some("Yes".into()))
        );
        assert_eq!(
            decision(&en, Approval::Always).map(|d| d.1),
            Some(Some("Yes, and don't ask again".into()))
        );
        assert_eq!(
            decision(&opts, Approval::Reject),
            Some((GateDecision::Reject, None)),
            "reject never carries a choice"
        );
    }

    #[test]
    fn keyboard_navigation_and_input_target_cycle() {
        let key = |s: u64| BlockKey {
            terminal: "t".into(),
            seq: s,
        };
        let fb = |s: u64| FeedBlock {
            key: key(s),
            who: "셸 · t".into(),
            kind: crate::agent_kind::AgentKind::Shell,
            block: block(s, Kind::Command, "x", Some(s), Some(s + 1), Some(0)),
            gate: None,
            last: false,
        };
        let feed = vec![fb(0), fb(1), fb(2)];
        assert_eq!(
            nav(&feed, None, "up"),
            Some(key(2)),
            "no selection: the newest"
        );
        assert_eq!(nav(&feed, Some(&key(1)), "up"), Some(key(0)));
        assert_eq!(nav(&feed, Some(&key(0)), "up"), None);
        assert_eq!(nav(&feed, Some(&key(1)), "down"), Some(key(2)));
        assert_eq!(nav(&feed, Some(&key(2)), "down"), None);
        assert_eq!(nav(&feed, Some(&key(2)), "home"), Some(key(0)));
        assert_eq!(nav(&feed, Some(&key(0)), "end"), Some(key(2)));
        let mut terms = [
            TestBackend::terminal("a", "a", None, "working"),
            TestBackend::terminal("b", "b", None, "done"),
            TestBackend::terminal("c", "c", None, "idle"),
        ];
        terms[1].live = crate::vm::Liveness::Exited {
            code: Some(0),
            killed: false,
        };
        let refs: Vec<_> = terms.iter().collect();
        assert_eq!(cycle_target(&refs, None, false).as_deref(), Some("a"));
        assert_eq!(
            cycle_target(&refs, Some("a"), false).as_deref(),
            Some("c"),
            "exited terminals are skipped"
        );
        assert_eq!(
            cycle_target(&refs, Some("c"), false).as_deref(),
            Some("a"),
            "wraps"
        );
        assert_eq!(cycle_target(&refs, Some("a"), true).as_deref(), Some("c"));
        assert_eq!(cycle_target(&refs[1..2], None, false), None);
    }
}
