//! The 리뷰 중심 layout's pure model: task rows from the workspace's tasks,
//! terminals and gates, the layout's own view state (selection, expanded
//! terminal, strip, 패널 view), list keys and F6 regions, the rules the
//! frame's bookkeeping follows, and the size formulas. No GPUI context, RPC
//! or clock here.
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

use podd::protocol::GateInfo;

use super::copy as s;
use crate::vm::{AgentState, Liveness, TaskVm, TerminalVm};

/// Terminals docked at the bottom per task.
pub const MAX_TILES: usize = 4;

/// One task as the left list shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub branch: Option<String>,
    pub base: Option<String>,
    /// The task's terminals in display order (see [`task_terminals`]).
    pub terminals: Vec<String>,
    /// Short status counts (`작업 2 · 응답 1`).
    pub status: String,
    pub open_comments: usize,
    pub gates: usize,
}

/// The task's terminals: its own list first (podd's order, agent first),
/// then any other terminal that reports this task. Unknown ids are skipped.
pub fn task_terminals(task: &TaskVm, terminals: &[TerminalVm]) -> Vec<String> {
    let mut ids: Vec<String> = task
        .terminals
        .iter()
        .filter(|id| terminals.iter().any(|t| &t.id == *id))
        .cloned()
        .collect();
    for t in terminals
        .iter()
        .filter(|t| t.task_id.as_deref() == Some(task.id.as_str()))
    {
        if !ids.contains(&t.id) {
            ids.push(t.id.clone());
        }
    }
    ids
}

/// Pending gates of a task: raised for the task or one of its terminals.
pub fn task_gates(task: &str, terminals: &[String], pending: &[GateInfo]) -> Vec<GateInfo> {
    pending
        .iter()
        .filter(|g| {
            g.task.as_deref() == Some(task)
                || g.terminal.as_ref().is_some_and(|t| terminals.contains(t))
        })
        .cloned()
        .collect()
}

/// Rows for every task, in the workspace's order.
pub fn build_rows(
    tasks: &[TaskVm],
    terminals: &[TerminalVm],
    pending: &[GateInfo],
) -> Vec<TaskRow> {
    tasks
        .iter()
        .map(|k| {
            let ids = task_terminals(k, terminals);
            let summary = crate::orch::summary_of(&ids, terminals);
            TaskRow {
                id: k.id.clone(),
                title: k.title.clone(),
                branch: k.branch.clone(),
                base: k.base.clone(),
                status: crate::orch::summary_short(&summary),
                open_comments: k.review_open,
                gates: task_gates(&k.id, &ids, pending).len(),
                terminals: ids,
            }
        })
        .collect()
}

/// The status glyph of each row: the summary badge of its terminals.
pub(super) fn row_badges(
    rows: &[TaskRow],
    terminals: &[TerminalVm],
) -> HashMap<String, Option<pod_ui::StatusBadge>> {
    rows.iter()
        .map(|r| {
            (
                r.id.clone(),
                crate::orch::summary_badge(&crate::orch::summary_of(&r.terminals, terminals)),
            )
        })
        .collect()
}

/// `branch → base` (the header's first words).
pub fn branch_line(branch: Option<&str>, base: Option<&str>) -> String {
    match (branch, base) {
        (Some(b), Some(base)) => s::branch_to_base(b, base),
        (Some(b), None) => b.to_string(),
        (None, _) => s::NO_BRANCH.to_string(),
    }
}

/// The row's accessible name: everything the row shows, in reading order.
pub fn row_name(r: &TaskRow) -> String {
    let mut name = s::row_name(
        &r.title,
        &branch_line(r.branch.as_deref(), r.base.as_deref()),
        &r.status,
    );
    if r.open_comments > 0 {
        name.push_str(&s::row_count(s::COMMENTS, r.open_comments));
    }
    if r.gates > 0 {
        name.push_str(&s::row_count(s::GATES, r.gates));
    }
    name
}

/// The docked tiles (first [`MAX_TILES`]) and how many terminals did not fit.
pub fn tiles(terminals: &[String]) -> (Vec<String>, usize) {
    let shown: Vec<String> = terminals.iter().take(MAX_TILES).cloned().collect();
    let hidden = terminals.len() - shown.len();
    (shown, hidden)
}

/// The selected task, its terminals (display order) and its pending gates:
/// what the canvas header shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Selected {
    pub(super) task: Option<TaskVm>,
    pub(super) terms: Vec<TerminalVm>,
    pub(super) gates: Vec<GateInfo>,
}

impl Selected {
    pub(super) fn of(
        selected: Option<&str>,
        tasks: &[TaskVm],
        terminals: &[TerminalVm],
        pending: &[GateInfo],
    ) -> Self {
        let task = selected
            .and_then(|id| tasks.iter().find(|t| t.id == id))
            .cloned();
        let ids = task
            .as_ref()
            .map(|k| task_terminals(k, terminals))
            .unwrap_or_default();
        let terms: Vec<TerminalVm> = ids
            .iter()
            .filter_map(|id| terminals.iter().find(|t| &t.id == id).cloned())
            .collect();
        let gates = task
            .as_ref()
            .map(|k| task_gates(&k.id, &ids, pending))
            .unwrap_or_default();
        Self { task, terms, gates }
    }

    /// Blocked agents without a gate of their own: the header offers to
    /// answer them in their terminal.
    pub(super) fn asks(&self) -> Vec<&TerminalVm> {
        self.terms
            .iter()
            .filter(|x| {
                x.is_running()
                    && x.status
                        .as_ref()
                        .is_some_and(|s| s.state == AgentState::Blocked)
            })
            .filter(|x| {
                !self
                    .gates
                    .iter()
                    .any(|g| g.terminal.as_deref() == Some(x.id.as_str()))
            })
            .collect()
    }

    /// The header shows its gate rows: a pending gate, or an agent reporting
    /// blocked (running or not).
    pub(super) fn shows_gate_rows(&self) -> bool {
        !self.gates.is_empty()
            || self.terms.iter().any(|x| {
                x.status
                    .as_ref()
                    .is_some_and(|s| s.state == AgentState::Blocked)
            })
    }
}

/// Keyboard movement in the task list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Home,
    End,
}

/// What a key on a task row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowKey {
    Nav(Nav),
    /// Enter: into the task's Review.
    Open,
}

/// The row's action for an unmodified key; modified keys are not the row's.
pub(super) fn row_key(key: &str, modified: bool) -> Option<RowKey> {
    if modified {
        return None;
    }
    Some(match key {
        "up" => RowKey::Nav(Nav::Up),
        "down" => RowKey::Nav(Nav::Down),
        "home" => RowKey::Nav(Nav::Home),
        "end" => RowKey::Nav(Nav::End),
        "enter" => RowKey::Open,
        _ => return None,
    })
}

/// A key on the list itself (no row focused yet) that picks the first task,
/// modifiers or not.
pub(super) fn list_key(key: &str) -> bool {
    matches!(key, "up" | "down" | "home" | "end")
}

/// A keyboard region (F6 / Shift+F6 cycle through them in this order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Region {
    Tasks,
    /// The Review (or the dock in the 패널 view).
    Canvas,
    Terminal(String),
}

/// What the canvas shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Canvas {
    /// The 패널 view: the dock.
    Dock,
    /// A terminal over the whole canvas.
    Terminal(String),
    /// The selected task's Review.
    Review(String),
    /// No task to review.
    NoTask,
}

/// The layout's own view state (never the tasks' or terminals' state).
#[derive(Clone, Debug, PartialEq)]
pub struct ReviewModel {
    /// The task whose Review is the canvas.
    pub selected: Option<String>,
    /// A terminal shown over the whole canvas.
    pub expanded: Option<String>,
    /// The bottom terminals show their tiles (else only their header).
    pub strip_open: bool,
    /// The canvas shows the dock (other panels) instead of the Review.
    pub show_dock: bool,
}

impl Default for ReviewModel {
    fn default() -> Self {
        Self {
            selected: None,
            expanded: None,
            strip_open: true,
            show_dock: false,
        }
    }
}

impl ReviewModel {
    /// Keep the selection on an existing task: `prefer` (the task the user
    /// was in) when the selection is gone, else the first task.
    pub fn settle(&mut self, rows: &[TaskRow], prefer: Option<&str>) {
        if self
            .selected
            .as_deref()
            .is_none_or(|id| !rows.iter().any(|r| r.id == id))
        {
            self.selected = prefer
                .filter(|p| rows.iter().any(|r| r.id == *p))
                .map(str::to_owned)
                .or_else(|| rows.first().map(|r| r.id.clone()));
        }
        // An expanded terminal must still be one of the selected task's tiles.
        let tiles = self.tiles_in(rows);
        if self.expanded.as_ref().is_some_and(|e| !tiles.contains(e)) {
            self.expanded = None;
        }
    }

    /// The selected task's docked tiles.
    pub(super) fn tiles_in(&self, rows: &[TaskRow]) -> Vec<String> {
        self.selected
            .as_deref()
            .and_then(|id| rows.iter().find(|r| r.id == id))
            .map(|r| tiles(&r.terminals).0)
            .unwrap_or_default()
    }

    /// Switch the canvas to task `id`. Returns whether the task changed.
    pub fn select(&mut self, id: &str) -> bool {
        self.show_dock = false;
        if self.selected.as_deref() == Some(id) {
            return false;
        }
        self.selected = Some(id.to_string());
        self.expanded = None;
        true
    }

    /// The task a key moves the selection to (`None`: nowhere to go).
    pub fn nav(&self, rows: &[TaskRow], key: Nav) -> Option<String> {
        let i = self
            .selected
            .as_deref()
            .and_then(|id| rows.iter().position(|r| r.id == id));
        let j = match (key, i) {
            (_, None) | (Nav::Home, _) => 0,
            (Nav::End, _) => rows.len().checked_sub(1)?,
            (Nav::Up, Some(i)) => i.checked_sub(1)?,
            (Nav::Down, Some(i)) => i + 1,
        };
        rows.get(j).map(|r| r.id.clone())
    }

    /// Ctrl+Shift+X / the tile's button: `id` over the whole canvas, or back to its tile.
    pub fn toggle_expand(&mut self, id: &str) {
        self.expanded = if self.expanded.as_deref() == Some(id) {
            None
        } else {
            Some(id.to_string())
        };
        if self.expanded.is_some() {
            self.strip_open = true;
            self.show_dock = false;
        }
    }

    /// The canvas tabs: 패널 (`true`, no terminal over the canvas) or 리뷰.
    pub(super) fn show_panels(&mut self, on: bool) {
        self.show_dock = on;
        if on {
            self.expanded = None;
        }
    }

    /// 응답하기: terminal `id` in view, over the canvas when it is not docked.
    pub(super) fn answer(&mut self, tiles: &[String], id: &str) {
        self.strip_open = true;
        self.show_dock = false;
        if !tiles.iter().any(|t| t == id) {
            self.expanded = Some(id.to_string());
        }
    }

    /// Entering the layout with terminal `id` of `task` focused: its task is
    /// the canvas and the terminal stays in view.
    pub(super) fn enter_on_terminal(&mut self, rows: &[TaskRow], id: &str, task: &str) {
        self.select(task);
        self.settle(rows, None);
        if !self.tiles_in(rows).iter().any(|t| t == id) {
            self.expanded = Some(id.to_string());
        }
        self.strip_open = true;
    }

    /// The F6 stops, in order: tasks, canvas (unless a terminal covers it), each shown terminal.
    pub fn regions(&self, tiles: &[String]) -> Vec<Region> {
        let mut r = vec![Region::Tasks];
        match &self.expanded {
            Some(id) => r.push(Region::Terminal(id.clone())),
            None => {
                r.push(Region::Canvas);
                if self.strip_open && !self.show_dock {
                    r.extend(tiles.iter().cloned().map(Region::Terminal));
                }
            }
        }
        r
    }

    /// What the canvas shows.
    pub(super) fn canvas(&self) -> Canvas {
        if self.show_dock {
            return Canvas::Dock;
        }
        if let Some(id) = &self.expanded {
            return Canvas::Terminal(id.clone());
        }
        match &self.selected {
            Some(task) => Canvas::Review(task.clone()),
            None => Canvas::NoTask,
        }
    }

    /// The strip shows its tiles (not only its header).
    pub(super) fn strip_tiles(&self) -> bool {
        self.strip_open && !self.show_dock && self.expanded.is_none()
    }
}

/// The region after (or before) `current`; an unknown region starts at the tasks.
pub fn next_region(regions: &[Region], current: Option<&Region>, forward: bool) -> Region {
    let n = regions.len();
    match current.and_then(|c| regions.iter().position(|r| r == c)) {
        Some(i) if forward => regions[(i + 1) % n].clone(),
        Some(i) => regions[(i + n - 1) % n].clone(),
        None => Region::Tasks,
    }
}

// ───────────────────── the frame's bookkeeping ─────────────────────

/// A terminal panel of the workspace holds the focus but this layout does not
/// show it (palette jump, 새 터미널): its task becomes the canvas and the
/// terminal shows docked, or over the canvas when it is not one of the tiles.
/// Returns whether the focus is to be moved into it (it was not in view).
pub(super) fn follow_hidden(
    model: &mut ReviewModel,
    rows: &[TaskRow],
    id: &str,
    task: &str,
) -> bool {
    let in_view = model.selected.as_deref() == Some(task)
        && !model.show_dock
        && (model.tiles_in(rows).iter().any(|t| t == id) || model.expanded.as_deref() == Some(id));
    if in_view {
        return false;
    }
    model.select(task);
    model.strip_open = true;
    let shown = rows
        .iter()
        .find(|r| r.id == task)
        .is_some_and(|r| tiles(&r.terminals).0.iter().any(|t| t == id));
    model.expanded = (!shown).then(|| id.to_string());
    true
}

/// The workspace's focused terminal changed since the last frame (`seen`):
/// its task (`task`, when it has one) becomes the canvas, and the terminal
/// goes over the canvas when it is not one of that task's tiles.
pub(super) fn follow_focus(
    model: &mut ReviewModel,
    seen: &mut Option<String>,
    focused: Option<&str>,
    task: Option<&str>,
    rows: &[TaskRow],
) {
    if seen.as_deref() == focused {
        return;
    }
    *seen = focused.map(str::to_owned);
    if let Some(id) = focused
        && let Some(task) = task
    {
        model.select(task);
        let row = rows.iter().find(|r| r.id == task);
        if row.is_some_and(|r| !tiles(&r.terminals).0.iter().any(|t| t == id)) {
            model.expanded = Some(id.to_string());
        }
    }
}

/// A Review panel opened while the layout shows (Ctrl+Shift+R, PR button):
/// the task of the first workspace Review not seen last frame. Nothing on
/// the first frame (`last_signature` still `usize::MAX`).
pub(super) fn review_opened<'a, K: Eq + Hash>(
    last_signature: usize,
    seen: &HashSet<K>,
    reviews: &'a [(K, String)],
) -> Option<&'a str> {
    if last_signature == usize::MAX {
        return None;
    }
    reviews
        .iter()
        .find(|(e, _)| !seen.contains(e))
        .map(|(_, task)| task.as_str())
}

/// Another panel was opened while the layout shows (settings, editor …):
/// the count of the dock's other panels rose since the last frame
/// (`usize::MAX`: no frame yet).
pub(super) fn dock_rose(last: usize, now: usize) -> bool {
    now > last && last != usize::MAX
}

// ───────────────────────────── tiles ─────────────────────────────

/// The attention stripe of a terminal tile; never the only carrier (icon and
/// status text too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Accent {
    /// A running agent reports blocked.
    Warning,
    /// A running agent reports an error, a non-zero exit, a lost holder.
    Danger,
}

pub(super) fn accent(t: &TerminalVm) -> Option<Accent> {
    match t.status.as_ref().map(|s| s.state) {
        Some(AgentState::Blocked) if t.is_running() => Some(Accent::Warning),
        Some(AgentState::Error) if t.is_running() => Some(Accent::Danger),
        _ if matches!(t.live, Liveness::Exited { code: Some(c), killed: false } if c != 0)
            || t.live == Liveness::Lost =>
        {
            Some(Accent::Danger)
        }
        _ => None,
    }
}

/// 승인 — or, for a gate with options (v17), one choice per option (podd
/// refuses an approval of such a gate without a choice).
pub(super) fn approve_choices(g: &GateInfo) -> Vec<Option<String>> {
    if g.options.is_empty() {
        vec![None]
    } else {
        g.options.iter().cloned().map(Some).collect()
    }
}

/// `승인` or `승인: <option>`.
pub fn approve_label(choice: Option<&str>) -> String {
    match choice {
        Some(c) => s::approve_option(c),
        None => s::APPROVE.to_string(),
    }
}

/// Who a terminal is, by the same rule as every other layout (docs/10 §D5.11):
/// the agent kind from status/task/argv (`agent_kind::identify_in`), not the program
/// name (a Claude session in PowerShell is Claude), plus the terminal's label.
#[derive(Clone, Debug)]
pub struct Who {
    pub kind: crate::agent_kind::AgentKind,
    pub label: String,
}
impl Default for Who {
    fn default() -> Self {
        Self {
            kind: crate::agent_kind::AgentKind::Shell,
            label: String::new(),
        }
    }
}
impl Who {
    pub fn of(t: &TerminalVm, tasks: &[TaskVm], label: String) -> Self {
        Self {
            kind: crate::agent_kind::identify_in(t, tasks).kind,
            label,
        }
    }
    /// `Claude · Worker 0`.
    pub fn title(&self) -> String {
        s::who(&self.kind.name(), &self.label)
    }
}

// ───────────────────────────── sizes ─────────────────────────────

/// The Review panel's columns (review.rs): file list 250, diff min 320, comments 340.
const REVIEW_FILES_W: f32 = 250.;
const REVIEW_DIFF_MIN_W: f32 = 320.;
const REVIEW_COMMENTS_W: f32 = crate::review::COMMENTS_W;
/// Whether the file list fits beside the diff and the comment column in `canvas_w`.
pub fn review_files_fit(canvas_w: f32) -> bool {
    canvas_w >= REVIEW_FILES_W + REVIEW_DIFF_MIN_W + REVIEW_COMMENTS_W
}
/// D4.6: below 560 px of canvas the comment column becomes a drawer over the diff
/// (between 560 and 660 it narrows towards its 224 minimum beside the 320 diff).
pub fn review_comments_drawer(canvas_w: f32) -> bool {
    canvas_w < crate::review::COMMENTS_DRAWER_BELOW
}

/// Item height of the task list (uniform list) for one text `line`: two
/// lines, the row's padding and border, and the gap under it.
pub(super) fn row_height(line: f32, border_width: f32) -> f32 {
    line * 2. + 2. + 8. * 2. + border_width * 2. + 4.
}

/// Height of the list, canvas, strip and tile headers.
pub(super) fn header_height(panel_header: f32, line: f32) -> f32 {
    panel_header.max(line + 10.)
}

/// Space between and around the islands; 0 when not rounded.
pub(super) fn island_gap(rounded: bool, gap: f32) -> f32 {
    if rounded { gap.max(6.) } else { 0. }
}

/// Width of the canvas: the window minus the task list and the island gaps.
pub(super) fn canvas_width(viewport_w: f32, sidebar: f32, gap: f32) -> f32 {
    viewport_w - sidebar - gap * 3.
}

/// Height of the terminal strip: tiles take 30% of the window (at least two
/// headers and 120 px), else only the header.
pub(super) fn strip_height(tiles_shown: bool, viewport_h: f32, header_h: f32) -> f32 {
    if tiles_shown {
        (viewport_h * 0.3).max(header_h * 2. + 120.)
    } else {
        header_h
    }
}
