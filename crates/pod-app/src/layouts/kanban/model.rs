//! The board's pure model: cards from the workspace's terminals, tasks and
//! gates, the view state (order, pins, selection, filters), keyboard
//! movement, the actions a card offers and the size formulas. No GPUI
//! context, RPC or clock here.
use std::collections::{HashMap, HashSet};

use podd::protocol::GateInfo;

use super::copy as s;
use crate::vm::{AgentState, Liveness, TaskVm, TerminalVm};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Working,
    NeedsYou,
    Done,
}

impl Column {
    pub const ALL: [Self; 3] = [Self::Working, Self::NeedsYou, Self::Done];
    pub fn index(self) -> usize {
        match self {
            Self::Working => 0,
            Self::NeedsYou => 1,
            Self::Done => 2,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Working => s::WORKING,
            Self::NeedsYou => s::NEEDS_YOU,
            Self::Done => s::DONE,
        }
    }
    pub(super) fn icon(self) -> pod_ui::Icon {
        match self {
            Self::Working => pod_ui::Icon::LoaderCircle,
            Self::NeedsYou => pod_ui::Icon::ShieldQuestion,
            Self::Done => pod_ui::Icon::CircleCheck,
        }
    }
    pub(super) fn fg(self, t: &pod_ui::Tokens) -> u32 {
        match self {
            Self::Working => t.info_fg,
            Self::NeedsYou => t.warning_fg,
            Self::Done => t.success_fg,
        }
    }
}

/// The column a terminal belongs in, and whether it failed — from the agent's
/// reported state and the process lifecycle only.
pub fn column_of(t: &TerminalVm) -> (Column, bool) {
    match t.live {
        Liveness::Exited { killed: true, .. }
        | Liveness::Exited {
            code: Some(0) | None,
            ..
        } => (Column::Done, false),
        Liveness::Exited { .. } | Liveness::Lost => (Column::NeedsYou, true),
        Liveness::Running => match t.status.as_ref().map(|s| s.state) {
            None | Some(AgentState::Working | AgentState::Unknown) => (Column::Working, false),
            Some(AgentState::Blocked) => (Column::NeedsYou, false),
            Some(AgentState::Error) => (Column::NeedsYou, true),
            Some(AgentState::Done | AgentState::Idle | AgentState::Exited) => (Column::Done, false),
        },
    }
}

/// One terminal as the board shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Card {
    pub id: String,
    /// `Claude · 이름`.
    pub title: String,
    pub task: Option<String>,
    pub task_title: String,
    pub repo: Option<String>,
    /// Filter value: the agent's name (`Claude`), or the program for a plain shell.
    pub agent: String,
    /// Which service runs here (docs/10 §D5.11): the mark before the title.
    pub kind: crate::agent_kind::AgentKind,
    pub column: Column,
    pub failed: bool,
    /// The status label (`승인 필요`, `실패 종료 (1)` …), always shown.
    pub status: String,
    /// The pending gate this card can approve or reject.
    pub gate: Option<GateInfo>,
    /// The process ended: `다시 실행` starts the same command again.
    pub rerun: bool,
    /// Last-line cache key: changes when the status or lifecycle changes.
    pub line_key: String,
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// Cards for every terminal, in the workspace's order.
pub fn build_cards(
    terminals: &[TerminalVm],
    tasks: &[TaskVm],
    pending: &[GateInfo],
    label: impl Fn(&TerminalVm) -> String,
) -> Vec<Card> {
    let mut gate_of: HashMap<&str, &GateInfo> = HashMap::new();
    for g in pending {
        if let Some(t) = g
            .terminal
            .as_deref()
            .filter(|t| terminals.iter().any(|x| x.id == *t))
        {
            gate_of.entry(t).or_insert(g);
        } else if let Some(task) = g.task.as_deref()
            && let Some(first) = terminals
                .iter()
                .find(|x| x.task_id.as_deref() == Some(task))
        {
            gate_of.entry(first.id.as_str()).or_insert(g);
        }
    }
    terminals
        .iter()
        .map(|t| {
            let task = t
                .task_id
                .as_deref()
                .and_then(|id| tasks.iter().find(|k| k.id == id));
            let (column, failed) = column_of(t);
            let kind = crate::agent_kind::identify(t, task).kind;
            let agent = if kind.is_agent() {
                kind.name()
            } else {
                capitalized(&t.program)
            };
            let status = t.status.as_ref();
            Card {
                id: t.id.clone(),
                title: format!("{agent} · {}", label(t)),
                task: t.task_id.clone(),
                task_title: task
                    .map(|k| k.title.clone())
                    .unwrap_or_else(|| s::NO_TASK.to_string()),
                repo: task.and_then(|k| k.repo.clone()),
                agent,
                kind,
                column,
                failed,
                status: crate::orch::terminal_badge(t).label(),
                gate: gate_of.get(t.id.as_str()).map(|g| (*g).clone()),
                rerun: !t.is_running(),
                line_key: format!(
                    "{:?}|{:?}|{}|{}",
                    t.live,
                    status.map(|s| s.state),
                    status.map_or(0, |s| s.since),
                    status.map_or(0, |s| s.turn)
                ),
            }
        })
        .collect()
}

/// Tasks the terminals belong to: the header's 작업 count.
pub(super) fn task_count(terminals: &[TerminalVm]) -> usize {
    terminals
        .iter()
        .filter_map(|t| t.task_id.as_deref())
        .collect::<HashSet<&str>>()
        .len()
}

/// A refused board edit, with the reason the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    /// The drop target is in another column: columns are the agent's state.
    StateIsTheAgents,
    Unknown,
}

/// Keyboard movement on the board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

/// The movement a key names (`up`, `down`, `left`, `right`, `home`, `end`).
pub(super) fn nav_key(key: &str) -> Option<Nav> {
    match key {
        "up" => Some(Nav::Up),
        "down" => Some(Nav::Down),
        "left" => Some(Nav::Left),
        "right" => Some(Nav::Right),
        "home" => Some(Nav::Home),
        "end" => Some(Nav::End),
        _ => None,
    }
}

/// What a key does on a focused card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CardKey {
    /// Enter: the card's terminal in the drawer, focused.
    Open,
    /// Alt+↑/↓: swap with the neighbour.
    Move { down: bool },
    /// Alt+Home: pinned to the top of the column.
    PinTop,
    /// Arrows, Home, End: the selection moves.
    Nav(Nav),
}

/// The card's handling of a key; `None` leaves it to the parents. Alt wins
/// over the other modifiers; any other modifier leaves the key alone.
pub(super) fn card_key(key: &str, alt: bool, modified: bool) -> Option<CardKey> {
    if key == "enter" {
        return (!modified).then_some(CardKey::Open);
    }
    let nav = nav_key(key)?;
    if alt {
        return match nav {
            Nav::Up | Nav::Down => Some(CardKey::Move {
                down: nav == Nav::Down,
            }),
            Nav::Home => Some(CardKey::PinTop),
            _ => None,
        };
    }
    (!modified).then_some(CardKey::Nav(nav))
}

/// The board's own view state (never the terminals' state).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BoardModel {
    /// Manual order (drag / Alt+↑↓); cards not listed keep the natural order after it.
    pub order: Vec<String>,
    /// Cards pinned to the top of their column.
    pub pinned: HashSet<String>,
    /// The card with the keyboard selection.
    pub selected: Option<String>,
    /// The card shown in the drawer.
    pub opened: Option<String>,
    pub repo: Option<String>,
    pub agent: Option<String>,
}

impl BoardModel {
    pub fn visible(&self, c: &Card) -> bool {
        self.repo
            .as_ref()
            .is_none_or(|r| c.repo.as_ref() == Some(r))
            && self.agent.as_ref().is_none_or(|a| &c.agent == a)
    }

    /// The filtered cards per column: pinned first, then the manual order, then the natural one.
    pub fn columns(&self, cards: &[Card]) -> [Vec<Card>; 3] {
        let rank: HashMap<&str, usize> = self
            .order
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        let mut cols: [Vec<(usize, Card)>; 3] = Default::default();
        for (natural, c) in cards.iter().enumerate().filter(|(_, c)| self.visible(c)) {
            cols[c.column.index()].push((natural, c.clone()));
        }
        cols.map(|mut v| {
            v.sort_by_key(|(natural, c)| {
                (
                    !self.pinned.contains(&c.id),
                    rank.get(c.id.as_str()).copied().unwrap_or(usize::MAX),
                    *natural,
                )
            });
            v.into_iter().map(|(_, c)| c).collect()
        })
    }

    /// `id` placed at `index` of its column's list (without it), pinned or not.
    fn place(&mut self, column: &[Card], id: &str, index: usize, pinned: bool) {
        let mut ids: Vec<String> = column
            .iter()
            .map(|c| c.id.clone())
            .filter(|c| c != id)
            .collect();
        ids.insert(index.min(ids.len()), id.to_string());
        self.order.retain(|x| !ids.contains(x));
        self.order.extend(ids);
        if pinned {
            self.pinned.insert(id.to_string());
        } else {
            self.pinned.remove(id);
        }
    }

    /// The column index, the column and the position of card `id`.
    pub(super) fn column_with<'a>(
        &self,
        cols: &'a [Vec<Card>; 3],
        id: &str,
    ) -> Option<(usize, &'a Vec<Card>, usize)> {
        cols.iter()
            .enumerate()
            .find_map(|(ci, col)| col.iter().position(|c| c.id == id).map(|i| (ci, col, i)))
    }

    /// Drop `id` on `target` (before it) or, with `None`, at the end of its column.
    /// The card takes the target's pinned state. Another column is refused.
    pub fn reorder(
        &mut self,
        cards: &[Card],
        id: &str,
        target: Option<&str>,
    ) -> Result<(), Refused> {
        let cols = self.columns(cards);
        let (ci, col, _) = self.column_with(&cols, id).ok_or(Refused::Unknown)?;
        match target {
            Some(t) if t == id => Ok(()),
            Some(t) => {
                let (ti, _, _) = self.column_with(&cols, t).ok_or(Refused::Unknown)?;
                if ti != ci {
                    return Err(Refused::StateIsTheAgents);
                }
                let without: Vec<&Card> = col.iter().filter(|c| c.id != id).collect();
                let index = without
                    .iter()
                    .position(|c| c.id == t)
                    .unwrap_or(without.len());
                let pinned = self.pinned.contains(t);
                self.place(col, id, index, pinned);
                Ok(())
            }
            None => {
                self.place(col, id, usize::MAX, false);
                Ok(())
            }
        }
    }

    /// Drop on a column header: pinned at the very top. Another column is refused.
    pub fn pin_top(&mut self, cards: &[Card], id: &str, column: Column) -> Result<(), Refused> {
        let cols = self.columns(cards);
        let (ci, col, _) = self.column_with(&cols, id).ok_or(Refused::Unknown)?;
        if ci != column.index() {
            return Err(Refused::StateIsTheAgents);
        }
        self.place(col, id, 0, true);
        Ok(())
    }

    /// Alt+↑/↓: swap with the neighbour, taking its pinned state.
    pub fn move_by(&mut self, cards: &[Card], id: &str, down: bool) -> Result<(), Refused> {
        let cols = self.columns(cards);
        let (_, col, i) = self.column_with(&cols, id).ok_or(Refused::Unknown)?;
        let j = if down { i + 1 } else { i.wrapping_sub(1) };
        let Some(neighbour) = col.get(j) else {
            return Ok(());
        };
        let pinned = self.pinned.contains(&neighbour.id);
        self.place(col, id, j, pinned);
        Ok(())
    }

    /// The card a key moves the selection to (`None`: nowhere to go).
    pub fn nav(&self, cols: &[Vec<Card>; 3], from: Option<&str>, key: Nav) -> Option<String> {
        let first = || cols.iter().find_map(|c| c.first()).map(|c| c.id.clone());
        let Some((ci, col, i)) = from.and_then(|id| self.column_with(cols, id)) else {
            return first();
        };
        let side = |step: isize| {
            let mut c = ci as isize + step;
            while (0..3).contains(&c) {
                let target = &cols[c as usize];
                if !target.is_empty() {
                    return Some(target[i.min(target.len() - 1)].id.clone());
                }
                c += step;
            }
            None
        };
        match key {
            Nav::Up => i.checked_sub(1).map(|j| col[j].id.clone()),
            Nav::Down => col.get(i + 1).map(|c| c.id.clone()),
            Nav::Home => col.first().map(|c| c.id.clone()),
            Nav::End => col.last().map(|c| c.id.clone()),
            Nav::Left => side(-1),
            Nav::Right => side(1),
        }
    }

    /// Keep the selection on a visible card (first of the board otherwise).
    pub fn settle_selection(&mut self, cols: &[Vec<Card>; 3]) {
        if self
            .selected
            .as_deref()
            .is_none_or(|id| self.column_with(cols, id).is_none())
        {
            self.selected = cols.iter().find_map(|c| c.first()).map(|c| c.id.clone());
        }
    }
}

/// A terminal focused elsewhere (palette jump, new terminal) is selected and
/// opened once the focus changed since the last frame (`seen`) and the board
/// has its card. True when the drawer must show it.
pub(super) fn follow_focus(
    model: &mut BoardModel,
    seen: &mut Option<String>,
    focused: Option<&str>,
    cards: &[Card],
) -> bool {
    if seen.as_deref() == focused {
        return false;
    }
    *seen = focused.map(str::to_owned);
    let Some(id) = focused.filter(|id| cards.iter().any(|c| c.id == *id)) else {
        return false;
    };
    model.selected = Some(id.to_owned());
    model.opened = Some(id.to_owned());
    true
}

/// A panel was opened while the board shows (settings, review …): the
/// count of non-terminal dock panels rose since the last frame
/// (`usize::MAX`: no frame yet).
pub(super) fn dock_rose(last: usize, now: usize) -> bool {
    now > last && last != usize::MAX
}

/// Repo and agent choices for the filters, sorted.
pub fn filter_choices(cards: &[Card]) -> (Vec<String>, Vec<String>) {
    let mut repos: Vec<String> = cards.iter().filter_map(|c| c.repo.clone()).collect();
    let mut agents: Vec<String> = cards.iter().map(|c| c.agent.clone()).collect();
    for v in [&mut repos, &mut agents] {
        v.sort();
        v.dedup();
    }
    (repos, agents)
}

/// The card's accessible name: everything the card shows, in reading order.
pub fn card_name(c: &Card, line: Option<&str>, pinned: bool) -> String {
    let mut name = s::card_name(&c.title, c.column.label(), &c.status, &c.task_title);
    if let Some(g) = &c.gate {
        name.push_str(&s::card_gate(&g.title));
    }
    if let Some(l) = line.filter(|l| !l.is_empty()) {
        name.push_str(&s::card_line(l));
    }
    if pinned {
        name.push_str(s::CARD_PINNED);
    }
    name
}

/// The line under the task title: the gate's message, else the last output
/// line, else 출력 없음.
pub(super) fn shown_line(c: &Card, line: Option<String>) -> String {
    c.gate
        .as_ref()
        .map(|g| g.message.clone())
        .or(line)
        .unwrap_or_else(|| s::NO_OUTPUT.to_string())
}

/// The status stripe on a card's left edge; never the only carrier (icon and
/// status text say it too).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stripe {
    /// 응답 필요.
    Warning,
    /// 응답 필요 because it failed.
    Danger,
}

pub(super) fn stripe(c: &Card) -> Option<Stripe> {
    match c.column {
        Column::NeedsYou if c.failed => Some(Stripe::Danger),
        Column::NeedsYou => Some(Stripe::Warning),
        _ => None,
    }
}

/// What an action button of a card does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum CardAction {
    /// Approve or reject the card's gate through the gate path; `choice` is
    /// the option picked on a gate that has them (v17) and `label` its text.
    Decide {
        gate: String,
        approve: bool,
        choice: Option<String>,
        label: String,
    },
    /// 응답하기: a question without a gate, answered in the terminal.
    Answer,
    /// 로그: the card's terminal.
    Log,
    /// 다시 실행: the same command again.
    Rerun,
}

/// One action button of a card: its element id and what it does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct CardButton {
    pub(super) id: String,
    pub(super) action: CardAction,
}

/// The card's action buttons in tab order: one approve button per gate
/// option (`orch_ui::model::gate_buttons`) and reject, or 응답하기 for a
/// question without a gate, or 로그; then 다시 실행 once the process ended.
pub(super) fn card_buttons(c: &Card) -> Vec<CardButton> {
    let id = &c.id;
    let mut out = Vec::new();
    if let Some(g) = &c.gate {
        for (n, btn) in crate::orch_ui::model::gate_buttons(g)
            .into_iter()
            .enumerate()
        {
            let bid = match (&btn.choice, btn.approve) {
                (Some(_), _) => format!("kanban-approve-{id}-{n}"),
                (None, true) => format!("kanban-approve-{id}"),
                (None, false) => format!("kanban-reject-{id}"),
            };
            out.push(CardButton {
                id: bid,
                action: CardAction::Decide {
                    gate: g.id.clone(),
                    approve: btn.approve,
                    choice: btn.choice,
                    label: btn.label,
                },
            });
        }
    } else if c.column == Column::NeedsYou && !c.failed {
        out.push(CardButton {
            id: format!("kanban-answer-{id}"),
            action: CardAction::Answer,
        });
    } else {
        out.push(CardButton {
            id: format!("kanban-log-{id}"),
            action: CardAction::Log,
        });
    }
    if c.rerun {
        out.push(CardButton {
            id: format!("kanban-rerun-{id}"),
            action: CardAction::Rerun,
        });
    }
    out
}

/// Space under every card in a column list.
pub(super) const CARD_GAP: f32 = 8.;

/// Height of a card for a text `line` (`LayoutMetrics::line`), the hit area
/// and the style's border: grows with the UI font so Korean text is never
/// clipped at any font size or scale.
pub(super) fn card_height(line: f32, hit_area: f32, border: f32) -> f32 {
    let action = hit_area.max(line + 6.);
    12. * 2. + line * 2. + (line + 8.) + action + 6. * 3. + border * 2.
}

/// Height of the open drawer. With nothing opened yet (`empty`) its header
/// and one line of guidance, so the board keeps the room; else a share of
/// the window (`max`: the larger one), 160 at least.
pub(super) fn drawer_height(
    empty: bool,
    max: bool,
    panel_header: f32,
    ui_font: f32,
    viewport: f32,
) -> f32 {
    if empty {
        panel_header.max(32.) + ui_font * 1.5 + 32.
    } else {
        (viewport * if max { 0.6 } else { 0.32 }).max(160.)
    }
}
