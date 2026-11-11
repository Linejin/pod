//! 집중 레일's pure model: which task a panel belongs to, the per-task dock
//! arrangement ([`Shape`]) and its plan, the task rows, the tab strip (recent
//! tasks, fitting, stepping) and the keyboard steps of the strip and the
//! drawer lists. No GPUI state: panel ids are a type parameter and "now" or
//! the window size come in as arguments.
use std::collections::HashSet;
use std::hash::Hash;

use super::{RAIL_WIDTH, TAB_TEXT_MAX, copy};
use crate::vm::{TaskVm, TerminalVm};

/// Tasks remembered for the tab strip after the rail leaves them.
pub(crate) const RECENT_MAX: usize = 8;

/// Which task a dock panel belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Owner {
    Task(String),
    /// Settings, gates, agent tree, groups, extensions …: shown with every task.
    Global,
}

/// The owner of a workbench panel: a terminal panel belongs to the task that
/// lists its terminal (else the terminal's own task), an editor or review
/// panel to the task in its saved state; everything else is global.
pub(crate) fn panel_owner(
    panel_name: &str,
    terminal: Option<&str>,
    task: Option<&str>,
    tasks: &[TaskVm],
    terminals: &[TerminalVm],
) -> Owner {
    match terminal {
        Some(t) => tasks
            .iter()
            .find(|x| x.terminals.iter().any(|y| y == t))
            .map(|x| x.id.clone())
            .or_else(|| {
                terminals
                    .iter()
                    .find(|x| x.id == t)
                    .and_then(|x| x.task_id.clone())
            })
            .map(Owner::Task)
            .unwrap_or(Owner::Global),
        None => match task {
            Some(task)
                if matches!(
                    panel_name,
                    crate::editor_panel::EDITOR_PANEL_NAME | crate::review::REVIEW_PANEL_NAME
                ) =>
            {
                Owner::Task(task.to_owned())
            }
            _ => Owner::Global,
        },
    }
}

/// Panels of the `current` task (terminals first, in the given order) and the
/// global ones after the task's other panels. Without a current task only the
/// global panels, as "others". `panels`: (id, owner, is a terminal panel).
pub(crate) fn task_panels<'a, P: Copy + 'a>(
    panels: impl Iterator<Item = (P, &'a Owner, bool)> + Clone,
    current: Option<&str>,
) -> (Vec<P>, Vec<P>) {
    let globals = panels
        .clone()
        .filter(|(_, o, _)| **o == Owner::Global)
        .map(|(id, _, _)| id);
    let Some(cur) = current else {
        return (Vec::new(), globals.collect());
    };
    let mut terminals = Vec::new();
    let mut others = Vec::new();
    for (id, owner, terminal) in panels {
        if !matches!(owner, Owner::Task(t) if t == cur) {
            continue;
        }
        if terminal {
            terminals.push(id);
        } else {
            others.push(id);
        }
    }
    others.extend(globals);
    (terminals, others)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Drawer {
    /// The repository/task/terminal tree (the workbench sidebar).
    Workspace,
    /// 작업 전환: every task.
    Tasks,
    /// 응답 대기: other tasks that wait for the user.
    Attention,
}

/// A dock arrangement in panel ids (the rail remembers one per task).
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Shape<P> {
    Split {
        horizontal: bool,
        children: Vec<(Shape<P>, Option<f32>)>,
    },
    Tabs {
        panels: Vec<P>,
        active: usize,
    },
}

impl<P: Copy + Eq + Hash> Shape<P> {
    pub fn panels(&self) -> Vec<P> {
        let mut out = Vec::new();
        self.collect(&mut out);
        out
    }
    fn collect(&self, out: &mut Vec<P>) {
        match self {
            Self::Split { children, .. } => children.iter().for_each(|(c, _)| c.collect(out)),
            Self::Tabs { panels, .. } => out.extend(panels.iter().copied()),
        }
    }
    /// Keep only `keep`; empty containers go, a split left with one child becomes that child.
    pub fn retain(self, keep: &HashSet<P>) -> Option<Self> {
        match self {
            Self::Tabs { panels, active } => {
                let shown = panels.get(active).copied();
                let kept: Vec<P> = panels.into_iter().filter(|p| keep.contains(p)).collect();
                if kept.is_empty() {
                    return None;
                }
                let active = shown
                    .and_then(|s| kept.iter().position(|p| *p == s))
                    .unwrap_or(0);
                Some(Self::Tabs {
                    panels: kept,
                    active,
                })
            }
            Self::Split {
                horizontal,
                children,
            } => {
                let mut kept: Vec<_> = children
                    .into_iter()
                    .filter_map(|(c, size)| c.retain(keep).map(|c| (c, size)))
                    .collect();
                match kept.len() {
                    0 => None,
                    1 => kept.pop().map(|(c, _)| c),
                    _ => Some(Self::Split {
                        horizontal,
                        children: kept,
                    }),
                }
            }
        }
    }
    /// Add `p` to the first tab group and show it.
    pub fn append(&mut self, p: P) {
        match self {
            Self::Tabs { panels, active } => {
                panels.push(p);
                *active = panels.len() - 1;
            }
            Self::Split { children, .. } => {
                if let Some((c, _)) = children.first_mut() {
                    c.append(p);
                }
            }
        }
    }
    /// Whether `p` is the visible tab of its group.
    pub fn is_shown(&self, p: P) -> bool {
        match self {
            Self::Tabs { panels, active } => panels.get(*active) == Some(&p),
            Self::Split { children, .. } => children.iter().any(|(c, _)| c.is_shown(p)),
        }
    }
    /// The visible tab of the first group.
    pub fn first_shown(&self) -> Option<P> {
        match self {
            Self::Tabs { panels, active } => panels.get(*active).copied(),
            Self::Split { children, .. } => children.first().and_then(|(c, _)| c.first_shown()),
        }
    }
}

/// The first view of a task (gallery 02): its first terminal with the task's
/// other panels as tabs, wide on the left, and the other terminals stacked on
/// the right.
pub(crate) fn default_shape<P: Copy>(
    terminals: &[P],
    others: &[P],
    main_width: Option<f32>,
) -> Option<Shape<P>> {
    let first: Vec<P> = terminals.iter().take(1).chain(others).copied().collect();
    if terminals.len() <= 1 {
        return (!first.is_empty()).then_some(Shape::Tabs {
            panels: first,
            active: 0,
        });
    }
    let mut side: Vec<(Shape<P>, Option<f32>)> = terminals[1..]
        .iter()
        .map(|p| {
            (
                Shape::Tabs {
                    panels: vec![*p],
                    active: 0,
                },
                None,
            )
        })
        .collect();
    let side = if side.len() == 1 {
        side.remove(0).0
    } else {
        Shape::Split {
            horizontal: false,
            children: side,
        }
    };
    Some(Shape::Split {
        horizontal: true,
        children: vec![
            (
                Shape::Tabs {
                    panels: first,
                    active: 0,
                },
                main_width,
            ),
            (side, None),
        ],
    })
}

/// The arrangement for a task: the remembered one minus closed panels plus new
/// ones (shown), or the default when nothing is remembered.
pub(crate) fn plan<P: Copy + Eq + Hash>(
    prev: Option<Shape<P>>,
    terminals: &[P],
    others: &[P],
    main_width: Option<f32>,
) -> Option<Shape<P>> {
    let all: Vec<P> = terminals.iter().chain(others).copied().collect();
    let keep: HashSet<P> = all.iter().copied().collect();
    match prev.and_then(|s| s.retain(&keep)) {
        Some(mut shape) => {
            let have: HashSet<P> = shape.panels().into_iter().collect();
            for p in all.into_iter().filter(|p| !have.contains(p)) {
                shape.append(p);
            }
            Some(shape)
        }
        None => default_shape(terminals, others, main_width),
    }
}

/// One task as the rail shows it (tabs, 작업 전환, 응답 대기).
#[derive(Clone, Debug, Default)]
pub(crate) struct TaskRow {
    pub id: String,
    pub title: String,
    pub branch: Option<String>,
    pub summary: pod_ui::StatusSummary,
    /// Pending orchestration gates of this task.
    pub gates: usize,
    /// First terminal that waits for the user (blocked), else the first failed one.
    pub first_attention: Option<String>,
    /// The task has panels in the dock.
    pub open: bool,
    /// The task's agents (docs/10 §D5.11): small marks on the tab and drawer rows.
    pub agents: Vec<crate::agent_kind::AgentKind>,
}

/// The count badge of a task tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Badge {
    /// Only failures: their count, danger colours.
    Failed(usize),
    /// Requests (blocked agents and gates): their count, warning colours.
    Waiting(usize),
}

/// The leading icon of a drawer row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowMark {
    /// A request waits (blocked agent or gate).
    Waiting,
    Failed,
    /// The current task, nothing waiting.
    Current,
    Idle,
}

/// Where Enter on a 응답 대기 row goes after switching to the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AttentionTarget {
    /// Open (or focus) the waiting terminal.
    Terminal(String),
    /// No waiting terminal but gates: the gates inbox.
    Gates,
    Nothing,
}

impl TaskRow {
    pub fn needs_you(&self) -> bool {
        self.summary.blocked > 0 || self.gates > 0 || self.summary.error > 0
    }
    /// `승인 필요 2 · 실패 1 · 승인 요청 1`, empty when nothing waits.
    pub fn attention_label(&self) -> String {
        let mut parts = Vec::new();
        if self.summary.blocked > 0 {
            parts.push(format!("{} {}", copy::BLOCKED, self.summary.blocked));
        }
        if self.gates > 0 {
            parts.push(format!("{} {}", copy::GATE_REQUESTS, self.gates));
        }
        if self.summary.error > 0 {
            parts.push(format!("{} {}", copy::FAILED, self.summary.error));
        }
        parts.join(" · ")
    }
    /// Accessible name of a tab or row: title, its agents, then what waits.
    pub fn accessible_name(&self) -> String {
        let label = self.attention_label();
        let mut name = self.title.clone();
        if !self.agents.is_empty() {
            name.push_str(" · ");
            name.push_str(
                &self
                    .agents
                    .iter()
                    .map(|k| k.accessible_name())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if label.is_empty() {
            name
        } else {
            format!("{name} — {label}")
        }
    }
    /// Tab badge: requests win over failures; none when nothing waits.
    pub fn badge(&self) -> Option<Badge> {
        let waiting = self.summary.blocked + self.gates;
        if self.summary.error > 0 && waiting == 0 {
            Some(Badge::Failed(self.summary.error))
        } else if waiting > 0 {
            Some(Badge::Waiting(waiting))
        } else {
            None
        }
    }
    /// Drawer row icon: requests, then failures, then the current task.
    pub fn mark(&self, is_current: bool) -> RowMark {
        if self.summary.blocked + self.gates > 0 {
            RowMark::Waiting
        } else if self.summary.error > 0 {
            RowMark::Failed
        } else if is_current {
            RowMark::Current
        } else {
            RowMark::Idle
        }
    }
    /// What Enter on this task's 응답 대기 row opens.
    pub fn attention_target(&self) -> AttentionTarget {
        match &self.first_attention {
            Some(id) => AttentionTarget::Terminal(id.clone()),
            None if self.gates > 0 => AttentionTarget::Gates,
            None => AttentionTarget::Nothing,
        }
    }
}

pub(crate) fn task_rows(
    tasks: &[TaskVm],
    terminals: &[TerminalVm],
    pending_gates: &[podd::protocol::GateInfo],
    open: &HashSet<String>,
) -> Vec<TaskRow> {
    tasks
        .iter()
        .map(|t| {
            let summary = crate::orch::summary_of(t.terminals.iter(), terminals);
            let state_of = |id: &String| {
                terminals
                    .iter()
                    .find(|x| &x.id == id && x.is_running())
                    .and_then(|x| x.status.as_ref())
                    .map(|s| s.state)
            };
            let first_attention = t
                .terminals
                .iter()
                .find(|id| state_of(id) == Some(crate::vm::AgentState::Blocked))
                .or_else(|| {
                    t.terminals
                        .iter()
                        .find(|id| state_of(id) == Some(crate::vm::AgentState::Error))
                })
                .cloned();
            TaskRow {
                id: t.id.clone(),
                title: t.title.clone(),
                branch: t.branch.clone(),
                summary,
                gates: pending_gates
                    .iter()
                    .filter(|g| g.task.as_deref() == Some(t.id.as_str()))
                    .count(),
                first_attention,
                open: open.contains(&t.id),
                agents: crate::agent_kind::task_kinds(t, terminals),
            }
        })
        .collect()
}

/// The current task after the rows changed: kept while it still exists, else
/// the focused one (`focused`, read only then), the first task with panels,
/// or the first task.
pub(crate) fn keep_or_pick(
    current: Option<String>,
    rows: &[TaskRow],
    focused: impl FnOnce() -> Option<String>,
) -> Option<String> {
    if current
        .as_ref()
        .is_none_or(|c| !rows.iter().any(|r| &r.id == c))
    {
        focused()
            .filter(|t| rows.iter().any(|r| &r.id == t))
            .or_else(|| rows.iter().find(|r| r.open).map(|r| r.id.clone()))
            .or_else(|| rows.first().map(|r| r.id.clone()))
    } else {
        current
    }
}

/// The task the rail just left goes first in the recent list (at most
/// [`RECENT_MAX`], no duplicates).
pub(crate) fn push_recent(recent: &mut Vec<String>, prev: String) {
    recent.retain(|x| x != &prev);
    recent.insert(0, prev);
    recent.truncate(RECENT_MAX);
}

/// 응답 대기: other tasks that wait for the user, requests (blocked/gates) before failures.
pub(crate) fn attention(rows: &[TaskRow], current: Option<&str>) -> Vec<usize> {
    let mut out: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| Some(r.id.as_str()) != current && r.needs_you())
        .map(|(i, _)| i)
        .collect();
    out.sort_by_key(|i| rows[*i].summary.blocked + rows[*i].gates == 0);
    out
}

/// Tasks with a tab: open panels, recently used, and the current one (task order).
pub(crate) fn tab_tasks(rows: &[TaskRow], recent: &[String], current: Option<&str>) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| r.open || recent.contains(&r.id) || Some(r.id.as_str()) == current)
        .map(|(i, _)| i)
        .collect()
}

/// The task `delta` tabs away from `current`, wrapping.
pub(crate) fn step(ids: &[String], current: Option<&str>, delta: isize) -> Option<String> {
    if ids.is_empty() {
        return None;
    }
    let n = ids.len() as isize;
    let at = current
        .and_then(|c| ids.iter().position(|x| x == c))
        .map(|i| i as isize)
        .unwrap_or(if delta > 0 { -1 } else { 0 });
    Some(ids[(at + delta).rem_euclid(n) as usize].clone())
}

/// Row the 작업 전환 / 응답 대기 list starts on: the current task in 작업 전환,
/// else the first row.
pub(crate) fn drawer_cursor(which: Drawer, rows: &[TaskRow], current: Option<&str>) -> usize {
    match which {
        Drawer::Tasks => current
            .and_then(|c| rows.iter().position(|r| r.id == c))
            .unwrap_or(0),
        _ => 0,
    }
}

/// A key on the tab strip or a drawer list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeyStep {
    /// Move the cursor to this index.
    To(usize),
    /// Enter / Space on the cursor.
    Activate,
}

/// The step for `key` from `at` in `len` items: `back`/`forward` move by one
/// (stopping at the ends), Home/End jump, Enter/Space activate.
pub(crate) fn key_step(
    key: &str,
    at: usize,
    len: usize,
    back: &str,
    forward: &str,
) -> Option<KeyStep> {
    match key {
        k if k == back => Some(KeyStep::To(at.saturating_sub(1))),
        k if k == forward => Some(KeyStep::To((at + 1).min(len.saturating_sub(1)))),
        "home" => Some(KeyStep::To(0)),
        "end" => Some(KeyStep::To(len.saturating_sub(1))),
        "enter" | "space" => Some(KeyStep::Activate),
        _ => None,
    }
}

/// CJK and other full-width characters.
fn is_wide(c: char) -> bool {
    matches!(c as u32, 0x1100..=0x11FF | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE4F | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6 | 0x1F300..=0x1FAFF)
}

/// Conservative width estimate for a single-line label (no text system needed:
/// Hangul is one em wide in Malgun Gothic/Segoe UI, Latin at most 0.6 em).
pub(crate) fn text_width(text: &str, font: f32) -> f32 {
    text.chars()
        .map(|c| {
            if is_wide(c) {
                font
            } else if c.is_ascii() {
                font * 0.6
            } else {
                font * 0.8
            }
        })
        .sum()
}

/// A task tab: padding 12+12, status dot + gap, title (at most [`TAB_TEXT_MAX`]), badge,
/// agent marks (12px each, 2px apart, at most 3 + `+N`).
pub(crate) fn tab_width(title: &str, font: f32, badge: bool, agents: usize) -> f32 {
    let marks = if agents == 0 {
        0.
    } else {
        8. + agents.min(3) as f32 * 14. + if agents > 3 { 20. } else { 0. }
    };
    24. + 16.
        + text_width(title, font).min(TAB_TEXT_MAX)
        + if badge { 28. } else { 0. }
        + marks
        + 2.
}

/// Room for the task tabs in a `viewport`-wide window: minus the rail, the
/// island gaps, "+", "+N" and 작업 전환. May be negative.
pub(crate) fn strip_room(viewport: f32, gap: f32, font: f32) -> f32 {
    let switcher_w = text_width(copy::SWITCHER, font) + 48.;
    viewport - RAIL_WIDTH - 3. * gap - 36. - 56. - switcher_w - 16.
}

/// Tabs that fit in `avail`, in order; the current tab is always shown (last,
/// in place of the ones that no longer fit).
pub(crate) fn fit_tabs(widths: &[f32], current: Option<usize>, avail: f32) -> Vec<usize> {
    let mut out = Vec::new();
    let mut used = 0.;
    for (i, w) in widths.iter().enumerate() {
        if used + w > avail {
            break;
        }
        used += w;
        out.push(i);
    }
    if let Some(c) = current.filter(|c| *c < widths.len() && !out.contains(c)) {
        while used + widths[c] > avail {
            let Some(last) = out.pop() else { break };
            used -= widths[last];
        }
        out.push(c);
    }
    out
}
