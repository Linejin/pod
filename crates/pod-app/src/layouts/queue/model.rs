//! The response queue's pure model: cards (응답 필요) and compact rows (작업 중,
//! 완료) from the workspace data, counts, keyboard order, selection hand-over,
//! gate buttons and ages. No GPUI state; "now" is an argument.
use std::collections::HashSet;

use pod_ui::{ControlState, Icon, Tone};
use podd::protocol::{GateDecision, GateInfo, GateState};

use super::copy;
use crate::agent_kind::AgentKind;
use crate::vm::{AgentState, Liveness, TaskVm, TerminalVm};

/// Rows shown in 작업 중 / 완료 before "모두 보기".
pub const COMPACT_ROWS: usize = 5;

/// Why a card is in the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeedKind {
    /// Permission request; decidable inline when it is a pending gate.
    Approval,
    /// The agent asked a question (answer in its terminal).
    Question,
    /// Blocked for another reason (auth, unknown).
    Other,
    /// Error state, lost, or a non-zero exit.
    Failed,
}

impl NeedKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Approval => copy::APPROVAL,
            Self::Question => copy::QUESTION,
            Self::Other => copy::NEEDS,
            Self::Failed => copy::FAILED,
        }
    }
    pub(crate) fn icon(self) -> Icon {
        match self {
            Self::Approval | Self::Other => Icon::ShieldQuestion,
            Self::Question => Icon::MessageCircleQuestion,
            Self::Failed => Icon::OctagonX,
        }
    }
    pub(crate) fn tone(self) -> Tone {
        if self == Self::Failed {
            Tone::Danger
        } else {
            Tone::Warning
        }
    }
    /// The card's terminal action: 답하기 for a question, else 열기.
    pub(crate) fn open_label(self) -> &'static str {
        if self == Self::Question {
            copy::ANSWER
        } else {
            copy::OPEN
        }
    }
}

/// One card of the 응답 필요 section.
#[derive(Clone, Debug, PartialEq)]
pub struct Need {
    /// Stable selection key: `gate:<id>`, `term:<id>` (blocked) or `fail:<id>`.
    pub key: String,
    pub kind: NeedKind,
    /// Pending gate behind this card: 이번만 허용 / 거부 decide it inline.
    pub gate: Option<String>,
    /// The gate's choices (v17): one approve button each, sending that choice.
    pub options: Vec<String>,
    /// Terminal that 열기 focuses.
    pub terminal: Option<String>,
    /// Task title (else the terminal label, else the gate title).
    pub title: String,
    /// `Worker 1 · 승인 필요`.
    pub meta: String,
    /// Gate message or task id.
    pub detail: String,
    /// Unix ms the request started (0: unknown).
    pub since: u64,
    /// The worker's agent (docs/10 §D5.11); none for a gate without a terminal.
    pub agent: Option<AgentKind>,
}

impl Need {
    /// Accessible name of the card: title, kind, agent (when known), meta.
    pub(crate) fn accessible_name(&self) -> String {
        match &self.agent {
            Some(a) => format!(
                "{}, {}, {}, {}",
                self.title,
                self.kind.label(),
                a.accessible_name(),
                self.meta
            ),
            None => format!("{}, {}, {}", self.title, self.kind.label(), self.meta),
        }
    }
    /// A gate card always shows its buttons; other cards only while selected.
    pub(crate) fn shows_actions(&self, selected: bool) -> bool {
        self.gate.is_some() || selected
    }
}

/// One compact row of 작업 중 / 완료.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub key: String,
    pub terminal: String,
    pub label: String,
    pub task: String,
    pub since: u64,
    pub agent: AgentKind,
}

impl Row {
    /// Accessible name: label, agent, section (`done`: 완료, else 작업 중), task.
    pub(crate) fn accessible_name(&self, done: bool) -> String {
        format!(
            "{}, {}, {}, {}",
            self.label,
            self.agent.accessible_name(),
            if done { copy::DONE } else { copy::WORKING },
            self.task
        )
    }
}

/// Header counts (unfiltered).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub working: usize,
    /// Approvals, questions and other blocked requests (failures are counted apart).
    pub needs: usize,
    pub done: usize,
    pub failed: usize,
}

/// A section of the queue (title-bar pills jump to one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Needs,
    Failed,
    Working,
    Done,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct QueueModel {
    /// Requests first (oldest gate first, then blocked terminals), failures after.
    pub needs: Vec<Need>,
    pub working: Vec<Row>,
    pub done: Vec<Row>,
    pub counts: Counts,
}

fn failed(t: &TerminalVm) -> bool {
    t.status
        .as_ref()
        .is_some_and(|s| s.state == AgentState::Error)
        || t.live == Liveness::Lost
        || matches!(t.live, Liveness::Exited { code: Some(c), killed: false } if c != 0)
}

/// Build the queue from the workspace's data. `pending` are the pending gates
/// ([`crate::orch::OrchState::pending`]); `query` filters the lists (not the counts);
/// `dismissed` hides failure cards the user acknowledged (`fail:<id>` + since).
pub fn build(
    terminals: &[TerminalVm],
    tasks: &[TaskVm],
    pending: &[GateInfo],
    label: &dyn Fn(&TerminalVm) -> String,
    query: &str,
    dismissed: &HashSet<(String, u64)>,
) -> QueueModel {
    let task_title = |t: &TerminalVm| {
        t.task_id.as_deref().map(|id| {
            tasks
                .iter()
                .find(|k| k.id == id)
                .map(|k| k.title.clone())
                .unwrap_or_else(|| id.to_string())
        })
    };
    let mut m = QueueModel::default();
    let mut blocked = Vec::new();
    let mut failures = Vec::new();
    let agent = |t: &TerminalVm| crate::agent_kind::identify_in(t, tasks).kind;
    for t in terminals {
        let since = t.status.as_ref().map(|s| s.since).unwrap_or(0);
        if failed(t) {
            let badge = crate::orch::terminal_badge(t).label();
            failures.push(Need {
                key: format!("fail:{}", t.id),
                kind: NeedKind::Failed,
                gate: None,
                options: Vec::new(),
                terminal: Some(t.id.clone()),
                title: task_title(t).unwrap_or_else(|| label(t)),
                meta: format!("{} · {badge}", label(t)),
                detail: t.task_id.clone().unwrap_or_default(),
                since,
                agent: Some(agent(t)),
            });
            continue;
        }
        if !t.is_running() {
            continue;
        }
        let Some(s) = &t.status else { continue };
        let row = || Row {
            key: format!("term:{}", t.id),
            terminal: t.id.clone(),
            label: label(t),
            task: task_title(t).unwrap_or_default(),
            since,
            agent: agent(t),
        };
        match s.state {
            AgentState::Working => m.working.push(row()),
            AgentState::Done => m.done.push(row()),
            AgentState::Blocked => blocked.push(t),
            _ => {}
        }
    }
    m.counts.working = m.working.len();
    m.counts.done = m.done.len();
    m.counts.failed = failures.len();
    // Gates first; a gate raised from a blocked terminal is that terminal's card.
    let mut merged = HashSet::new();
    for g in pending.iter().filter(|g| g.state == GateState::Pending) {
        let term = g
            .terminal
            .clone()
            .filter(|id| terminals.iter().any(|t| &t.id == id))
            .or_else(|| {
                g.task
                    .as_deref()
                    .and_then(|k| tasks.iter().find(|t| t.id == k))
                    .and_then(|t| t.terminals.first().cloned())
            });
        let t = term
            .as_deref()
            .and_then(|id| terminals.iter().find(|t| t.id == id));
        if let Some(t) = t.filter(|t| blocked.iter().any(|b| b.id == t.id)) {
            merged.insert(t.id.clone());
        }
        let task = g
            .task
            .as_deref()
            .and_then(|k| tasks.iter().find(|t| t.id == k))
            .map(|t| t.title.clone())
            .or_else(|| t.and_then(task_title));
        m.needs.push(Need {
            key: format!("gate:{}", g.id),
            kind: NeedKind::Approval,
            gate: Some(g.id.clone()),
            options: g.options.clone(),
            terminal: term,
            title: task.unwrap_or_else(|| g.title.clone()),
            meta: format!(
                "{} · {}",
                t.map(label).unwrap_or_else(|| crate::orch::gate_origin(g)),
                g.title
            ),
            detail: g.message.clone(),
            since: g.created_ms,
            agent: t.map(agent),
        });
    }
    for t in blocked.into_iter().filter(|t| !merged.contains(&t.id)) {
        let kind = match t.status.as_ref().and_then(|s| s.kind.as_deref()) {
            Some("permission") => NeedKind::Approval,
            Some("question") => NeedKind::Question,
            _ => NeedKind::Other,
        };
        m.needs.push(Need {
            key: format!("term:{}", t.id),
            kind,
            gate: None,
            options: Vec::new(),
            terminal: Some(t.id.clone()),
            title: task_title(t).unwrap_or_else(|| label(t)),
            meta: format!("{} · {}", label(t), kind.label()),
            detail: t.task_id.clone().unwrap_or_default(),
            since: t.status.as_ref().map(|s| s.since).unwrap_or(0),
            agent: Some(agent(t)),
        });
    }
    m.counts.needs = m.needs.len();
    m.needs.extend(
        failures
            .into_iter()
            .filter(|n| !dismissed.contains(&(n.key.clone(), n.since))),
    );
    let q = query.trim().to_lowercase();
    if !q.is_empty() {
        let hit = |parts: &[&str]| parts.iter().any(|p| p.to_lowercase().contains(&q));
        // Agent kinds match too (`claude`, `codex`).
        let terms = |a: Option<&AgentKind>| a.map(|a| a.search_terms()).unwrap_or_default();
        m.needs
            .retain(|n| hit(&[&n.title, &n.meta, &n.detail, &terms(n.agent.as_ref())]));
        m.working
            .retain(|r| hit(&[&r.label, &r.task, &terms(Some(&r.agent))]));
        m.done
            .retain(|r| hit(&[&r.label, &r.task, &terms(Some(&r.agent))]));
    }
    m
}

impl QueueModel {
    /// Keyboard order (cards, then the visible rows of each section) with each item's terminal.
    pub fn keys(&self, all_working: bool, all_done: bool) -> Vec<(String, Option<String>)> {
        let cap = |all: bool, n: usize| if all { n } else { n.min(COMPACT_ROWS) };
        self.needs
            .iter()
            .map(|n| (n.key.clone(), n.terminal.clone()))
            .chain(
                self.working
                    .iter()
                    .take(cap(all_working, self.working.len()))
                    .map(|r| (r.key.clone(), Some(r.terminal.clone()))),
            )
            .chain(
                self.done
                    .iter()
                    .take(cap(all_done, self.done.len()))
                    .map(|r| (r.key.clone(), Some(r.terminal.clone()))),
            )
            .collect()
    }

    /// First item of a section, if any.
    pub fn first_of(&self, s: Section) -> Option<String> {
        match s {
            Section::Needs => self
                .needs
                .iter()
                .find(|n| n.kind != NeedKind::Failed)
                .map(|n| n.key.clone()),
            Section::Failed => self
                .needs
                .iter()
                .find(|n| n.kind == NeedKind::Failed)
                .map(|n| n.key.clone()),
            Section::Working => self.working.first().map(|r| r.key.clone()),
            Section::Done => self.done.first().map(|r| r.key.clone()),
        }
    }
}

/// A decided / vanished item hands the selection to the one that took its
/// place: while the selected key is listed its index is remembered; once it
/// is gone the item now at that index (or the last one) is selected. Nothing
/// selected stays nothing.
pub(crate) fn follow_selection(
    selected: &mut Option<String>,
    sel_index: &mut usize,
    keys: &[(String, Option<String>)],
) {
    if let Some(s) = selected.as_ref() {
        match keys.iter().position(|(k, _)| k == s) {
            Some(i) => *sel_index = i,
            None => {
                *selected = keys
                    .get((*sel_index).min(keys.len().saturating_sub(1)))
                    .map(|(k, _)| k.clone())
            }
        }
    }
}

/// A key on the queue list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListKey {
    /// Select this item (the arrows start at the first item).
    Select(usize),
    /// Enter / Space: open the selected item, if any.
    Open(Option<usize>),
}

/// The list step for `key` with the selection at `at` among `len` (> 0) items.
pub(crate) fn list_key(key: &str, at: Option<usize>, len: usize) -> Option<ListKey> {
    match key {
        "down" => Some(ListKey::Select(at.map_or(0, |i| (i + 1).min(len - 1)))),
        "up" => Some(ListKey::Select(at.map_or(0, |i| i.saturating_sub(1)))),
        "home" => Some(ListKey::Select(0)),
        "end" => Some(ListKey::Select(len - 1)),
        "enter" | "space" => Some(ListKey::Open(at)),
        _ => None,
    }
}

/// One decision button of a gate card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GateButton {
    pub decision: GateDecision,
    pub label: String,
    /// Element id: `queue-allow-<gate>[-<i>]` or `queue-reject-<gate>`.
    pub id: String,
    /// The option sent with an approval (v17), none otherwise.
    pub choice: Option<String>,
}

impl GateButton {
    /// `선택: <option>: <title>` for an option, `<label>: <title>` otherwise.
    pub(crate) fn accessible_name(&self, title: &str) -> String {
        match &self.choice {
            Some(_) => format!("{}: {}: {title}", copy::CHOOSE, self.label),
            None => format!("{}: {title}", self.label),
        }
    }
    pub(crate) fn tone(&self) -> Tone {
        match self.decision {
            GateDecision::Approve => Tone::Success,
            GateDecision::Reject => Tone::Danger,
        }
    }
}

/// A gate's buttons: one approval per option (v17) or 이번만 허용, then 거부.
pub(crate) fn gate_buttons(gate: &str, options: &[String]) -> Vec<GateButton> {
    let approve = |label: String, id: String, choice: Option<String>| GateButton {
        decision: GateDecision::Approve,
        label,
        id,
        choice,
    };
    let mut out: Vec<GateButton> = if options.is_empty() {
        vec![approve(
            copy::ALLOW_ONCE.to_string(),
            format!("queue-allow-{gate}"),
            None,
        )]
    } else {
        options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                approve(
                    o.clone(),
                    format!("queue-allow-{gate}-{i}"),
                    Some(o.clone()),
                )
            })
            .collect()
    };
    out.push(GateButton {
        decision: GateDecision::Reject,
        label: copy::REJECT.to_string(),
        id: format!("queue-reject-{gate}"),
        choice: None,
    });
    out
}

/// A decision button while `busy` (the decision in flight, if any) is pending:
/// the pressed one loads, the others are disabled.
pub(crate) fn button_state(busy: Option<GateDecision>, d: GateDecision) -> ControlState {
    match busy {
        Some(b) if b == d => ControlState::Loading,
        Some(_) => ControlState::Disabled,
        None => ControlState::Ready,
    }
}

/// Which preset structure: 03 (counts in the column) or 06 (pills + activity bar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Classic,
    Mission,
}

impl Variant {
    /// 06's style (`mission`) gets the 06 structure; every other style the 03 one.
    pub fn of_style(style: &str) -> Self {
        if style == "mission" {
            Self::Mission
        } else {
            Self::Classic
        }
    }
}

/// `12초`, `3분`, `2시간` since `since` (ms); empty when unknown.
pub fn age(since: u64, now: u64) -> String {
    if since == 0 || since > now {
        return String::new();
    }
    match (now - since) / 1000 {
        s @ 0..60 => copy::age_seconds(s),
        s @ 60..3600 => copy::age_minutes(s / 60),
        s @ 3600..86_400 => copy::age_hours(s / 3600),
        s => copy::age_days(s / 86_400),
    }
}
