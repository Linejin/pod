//! Headless model of the orchestration screens (docs/19 §8, S7): what the
//! Run board, worker cards, inbox, cleanup screen and settings show, and
//! which actions each row offers. Pure data in, view rows out — no GPUI —
//! so every rule here has a plain unit test.
//!
//! Data comes from `orch.run.show/list`, `orch.worker.list`, `orch.mail.list`
//! and `task.cleanup_candidates`, re-read only when the status stream says
//! something changed (`orch.changed`, `orch.mail`; see `orch_ui/mod.rs`).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use podd::protocol::{
    Attempt, AttemptState, CleanupCandidate, CleanupTier, Item, ItemState, Lifecycle, Mail,
    MailType, OrchMailEvent, OrchSettings, RunInfo, WorkerInfo, WorkerState,
};
use serde_json::Value;

use super::s;

/// Board columns, in order (docs/19 §8: 대기·준비·진행·차단·완료·실패).
pub const COLUMNS: [ItemState; 6] = [
    ItemState::Pending,
    ItemState::Ready,
    ItemState::Assigned,
    ItemState::Blocked,
    ItemState::Completed,
    ItemState::Failed,
];

/// `r12` / `a3` / `m40` -> 12 / 3 / 40 (ids sort numerically).
pub fn id_number(id: &str) -> u64 {
    id.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0)
}

/// A worker waiting for resources (docs/19 §5.1, S6). Read from the raw
/// attempt JSON so the board shows it whether or not the daemon has S6.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueVm {
    pub kind: String,
    pub reason: String,
    pub position: usize,
}

impl QueueVm {
    pub fn from_attempt_json(a: &Value) -> Option<QueueVm> {
        let q = a.get("queue")?;
        if q.is_null() {
            return None;
        }
        Some(QueueVm {
            kind: q
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("queued")
                .to_string(),
            reason: q
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            position: q.get("position").and_then(Value::as_u64).unwrap_or(0) as usize,
        })
    }
}

/// Queue states of a raw `RunInfo` JSON, by attempt id.
pub fn queues_of(raw: &Value) -> HashMap<String, QueueVm> {
    raw.get("attempts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some((
                        x.get("id")?.as_str()?.to_string(),
                        QueueVm::from_attempt_json(x)?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What the app knows about one run.
#[derive(Clone, Debug, PartialEq)]
pub struct RunEntry {
    pub info: RunInfo,
    pub queues: HashMap<String, QueueVm>,
    /// `orch.worker.list` (status, last output, waiting answer, next).
    pub workers: Vec<WorkerInfo>,
    /// `orch.mail.list` (whole thread, nothing acked).
    pub mail: Vec<Mail>,
    pub mail_loaded: bool,
}

impl RunEntry {
    pub fn new(info: RunInfo, queues: HashMap<String, QueueVm>) -> Self {
        RunEntry {
            info,
            queues,
            workers: Vec::new(),
            mail: Vec::new(),
            mail_loaded: false,
        }
    }

    pub fn id(&self) -> &str {
        &self.info.run.id
    }

    pub fn item(&self, id: &str) -> Option<&Item> {
        self.info.items.iter().find(|i| i.id == id)
    }

    pub fn attempt(&self, id: &str) -> Option<&Attempt> {
        self.info.attempts.iter().find(|a| a.id == id)
    }

    pub fn worker(&self, attempt: &str) -> Option<&WorkerInfo> {
        self.workers.iter().find(|w| w.attempt.id == attempt)
    }

    /// Nothing runs and every item ended (an empty run is not finished).
    pub fn is_finished(&self) -> bool {
        !self.info.items.is_empty()
            && self.info.attempts.iter().all(|a| !a.state.is_open())
            && self
                .info
                .items
                .iter()
                .all(|i| matches!(i.state, ItemState::Completed | ItemState::Failed))
    }

    /// Questions from workers that have no answer yet.
    pub fn open_questions(&self) -> usize {
        threads(self).iter().filter(|t| t.open_question).count()
    }
}

// ───────────────────────────── items (DAG) ─────────────────────────────

/// One item on the board.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemRow {
    pub id: String,
    pub title: String,
    pub state: ItemState,
    /// Dependency depth (0 = no deps): the board indents by it.
    pub depth: usize,
    pub deps: Vec<String>,
    pub attempts: usize,
    pub circuit_open: bool,
}

/// Longest dependency chain above each item (cycles and unknown deps are cut).
pub fn item_depths(items: &[Item]) -> HashMap<String, usize> {
    fn depth(
        id: &str,
        by: &HashMap<&str, &Item>,
        memo: &mut HashMap<String, usize>,
        path: &mut HashSet<String>,
    ) -> usize {
        if let Some(d) = memo.get(id) {
            return *d;
        }
        let Some(item) = by.get(id) else { return 0 };
        if !path.insert(id.to_string()) {
            return 0;
        }
        let d = item
            .deps
            .iter()
            .filter(|d| by.contains_key(d.as_str()))
            .map(|d| depth(d, by, memo, path) + 1)
            .max()
            .unwrap_or(0);
        path.remove(id);
        memo.insert(id.to_string(), d);
        d
    }
    let by: HashMap<&str, &Item> = items.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut memo = HashMap::new();
    for i in items {
        depth(&i.id, &by, &mut memo, &mut HashSet::new());
    }
    memo
}

/// Items in dependency order (depth, then id).
pub fn item_rows(run: &RunEntry) -> Vec<ItemRow> {
    let depths = item_depths(&run.info.items);
    let mut rows: Vec<ItemRow> = run
        .info
        .items
        .iter()
        .map(|i| ItemRow {
            id: i.id.clone(),
            title: i.title.clone(),
            state: i.state,
            depth: depths.get(&i.id).copied().unwrap_or(0),
            deps: i.deps.clone(),
            attempts: run.info.attempts.iter().filter(|a| a.item == i.id).count(),
            circuit_open: i.circuit_open,
        })
        .collect();
    rows.sort_by_key(|r| (r.depth, id_number(&r.id)));
    rows
}

/// Items grouped by board column (every column present, possibly empty).
pub fn columns(run: &RunEntry) -> Vec<(ItemState, Vec<ItemRow>)> {
    let rows = item_rows(run);
    COLUMNS
        .iter()
        .map(|c| (*c, rows.iter().filter(|r| r.state == *c).cloned().collect()))
        .collect()
}

pub fn item_state_label(st: ItemState) -> &'static str {
    match st {
        ItemState::Pending => s::COL_PENDING,
        ItemState::Ready => s::COL_READY,
        ItemState::Assigned => s::COL_ASSIGNED,
        ItemState::Blocked => s::COL_BLOCKED,
        ItemState::Completed => s::COL_COMPLETED,
        ItemState::Failed => s::COL_FAILED,
    }
}

// ───────────────────────────── worker cards ─────────────────────────────

/// Buttons a worker card offers (docs/19 §5.2 rules).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkerActions {
    pub stop: bool,
    pub abandon: bool,
    pub keep: bool,
    pub release: bool,
    pub open_terminal: bool,
    pub open_review: bool,
}

pub fn worker_actions(a: &Attempt, queued: bool) -> WorkerActions {
    let has_terminal = a.terminal.is_some() && a.worker != WorkerState::Released;
    WorkerActions {
        // Stop kills the process tree: only a started worker has one.
        stop: a.state.is_open() && !queued && a.terminal.is_some(),
        abandon: a.state.is_open(),
        keep: matches!(a.worker, WorkerState::Active | WorkerState::Finished),
        release: a.worker != WorkerState::Released && !queued,
        open_terminal: has_terminal,
        open_review: a.task.is_some(),
    }
}

/// One worker (attempt) card.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkerCard {
    pub attempt: String,
    pub item: String,
    pub item_title: String,
    pub agent: String,
    pub task: Option<String>,
    pub terminal: Option<String>,
    pub state: AttemptState,
    pub worker: WorkerState,
    /// The agent's live status (podd `StatusInfo`), while it has a terminal.
    pub status: Option<Value>,
    /// Last output or message, Unix ms (0 = none).
    pub last_activity_ms: u64,
    /// Newest message from or to this worker: `(type, subject)`.
    pub last_mail: Option<(MailType, String)>,
    /// Its unanswered question.
    pub question: Option<String>,
    pub waiting_answer: bool,
    pub unread: usize,
    pub next: String,
    pub queue: Option<QueueVm>,
    pub release_at_ms: Option<u64>,
    pub error: Option<String>,
    pub depth: u32,
    pub actions: WorkerActions,
    /// orch-02: since when a typed wake-up was not confirmed as taken.
    pub wake_unconfirmed_ms: Option<u64>,
}

impl WorkerCard {
    /// Short state text for the card's chip (Korean).
    pub fn state_label(&self) -> String {
        if let Some(q) = &self.queue {
            return if q.position > 0 {
                format!("{} ({}번째)", s::QUEUED, q.position)
            } else {
                s::QUEUED.to_string()
            };
        }
        attempt_state_label(self.state).to_string()
    }

    /// Tone of the chip: `ok`, `warn`, `bad` or `muted`.
    pub fn tone(&self) -> Tone {
        if self.queue.is_some()
            || self.question.is_some()
            || self.waiting_answer
            || self.wake_unconfirmed_ms.is_some()
        {
            return Tone::Warn;
        }
        match self.state {
            AttemptState::Starting | AttemptState::Ready => Tone::Accent,
            AttemptState::Succeeded => Tone::Ok,
            AttemptState::Failed | AttemptState::StartFailed | AttemptState::Lost => Tone::Bad,
            AttemptState::Unknown => Tone::Warn,
            AttemptState::Stopped | AttemptState::Abandoned => Tone::Muted,
        }
    }

    /// The accessible name of the card: everything a screen reader needs to
    /// pick it out of the list.
    pub fn aria(&self) -> String {
        let mut parts = vec![
            format!("{} {}", s::WORKER, self.attempt),
            self.item_title.clone(),
            self.agent.clone(),
            self.state_label(),
            worker_state_label(self.worker).to_string(),
        ];
        if self.question.is_some() {
            parts.push(s::HAS_QUESTION.into());
        }
        if self.wake_unconfirmed_ms.is_some() {
            parts.push(s::WAKE_UNCONFIRMED.into());
        }
        parts.join(", ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Ok,
    Warn,
    Bad,
    Muted,
}

pub fn attempt_state_label(st: AttemptState) -> &'static str {
    match st {
        AttemptState::Starting => s::A_STARTING,
        AttemptState::Ready => s::A_READY,
        AttemptState::Succeeded => s::A_SUCCEEDED,
        AttemptState::Failed => s::A_FAILED,
        AttemptState::StartFailed => s::A_START_FAILED,
        AttemptState::Stopped => s::A_STOPPED,
        AttemptState::Abandoned => s::A_ABANDONED,
        AttemptState::Lost => s::A_LOST,
        AttemptState::Unknown => s::A_UNKNOWN,
    }
}

pub fn worker_state_label(w: WorkerState) -> &'static str {
    match w {
        WorkerState::Active => s::W_ACTIVE,
        WorkerState::Finished => s::W_FINISHED,
        WorkerState::Released => s::W_RELEASED,
        WorkerState::Kept => s::W_KEPT,
    }
}

/// Cards of a run's workers that still hold resources (released ones live
/// on the cleanup screen), open ones first, then by attempt number.
pub fn cards(run: &RunEntry) -> Vec<WorkerCard> {
    let mut out: Vec<WorkerCard> = run
        .info
        .attempts
        .iter()
        .filter(|a| a.worker != WorkerState::Released)
        .map(|a| card(run, a))
        .collect();
    out.sort_by_key(|c| (!c.state.is_open(), id_number(&c.attempt)));
    out
}

pub fn card(run: &RunEntry, a: &Attempt) -> WorkerCard {
    let w = run.worker(&a.id);
    let me = format!("attempt:{}", a.id);
    let mine: Vec<&Mail> = run
        .mail
        .iter()
        .filter(|m| m.kind != MailType::Heartbeat && (m.from == me || m.recipients.contains(&me)))
        .collect();
    let last = mine.iter().max_by_key(|m| id_number(&m.id));
    let question = run
        .mail
        .iter()
        .filter(|m| m.kind == MailType::Question && m.from == me && !answered(run, &m.id))
        .max_by_key(|m| id_number(&m.id))
        .map(super::ko::subject);
    let last_mail_ms = mine.iter().map(|m| m.created_ms).max().unwrap_or(0);
    let queue = run.queues.get(&a.id).cloned();
    WorkerCard {
        attempt: a.id.clone(),
        item: a.item.clone(),
        item_title: w
            .map(|w| w.item_title.clone())
            .or_else(|| run.item(&a.item).map(|i| i.title.clone()))
            .unwrap_or_else(|| a.item.clone()),
        agent: a.agent.clone(),
        task: a.task.clone(),
        terminal: a.terminal.clone(),
        state: a.state,
        worker: a.worker,
        status: w
            .and_then(|w| w.status.as_ref())
            .and_then(|s| serde_json::to_value(s).ok()),
        last_activity_ms: w
            .map(|w| w.last_output_ms)
            .unwrap_or(0)
            .max(last_mail_ms)
            .max(a.ready_ms.unwrap_or(0)),
        last_mail: last.map(|m| (m.kind, super::ko::subject(m))),
        question,
        waiting_answer: w.is_some_and(|w| w.waiting_answer),
        unread: w.map(|w| w.unread).unwrap_or(0),
        next: w.map(super::ko::next).unwrap_or_default(),
        actions: worker_actions(a, queue.is_some()),
        queue,
        release_at_ms: a
            .release_at_ms
            .filter(|_| a.worker == WorkerState::Finished),
        error: a.error.clone(),
        depth: a.depth,
        wake_unconfirmed_ms: w.and_then(|w| w.wake_unconfirmed_ms),
    }
}

// ───────────────────────────── inbox ─────────────────────────────

/// A message thread of the inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub root: Mail,
    pub replies: Vec<Mail>,
    /// A worker's question nobody answered yet (shown first).
    pub open_question: bool,
    /// Choice buttons (`payload.options`, docs/19 §4.5).
    pub choices: Vec<String>,
    /// The sender has a mailbox, so a reply reaches it.
    pub can_reply: bool,
}

impl Thread {
    pub fn last_ms(&self) -> u64 {
        self.replies.last().unwrap_or(&self.root).created_ms
    }
}

fn answered(run: &RunEntry, question: &str) -> bool {
    run.mail.iter().any(|m| {
        m.kind == MailType::Answer
            && (m.reply_to.as_deref() == Some(question) || m.thread.as_deref() == Some(question))
    })
}

pub fn choices_of(m: &Mail) -> Vec<String> {
    m.payload
        .as_ref()
        .and_then(|p| p.get("options"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .filter(|o| !o.trim().is_empty())
                .take(8)
                .collect()
        })
        .unwrap_or_default()
}

/// Threads of a run: open questions first (oldest first, they wait longest),
/// then the rest newest first. Heartbeats are board-only (never here).
pub fn threads(run: &RunEntry) -> Vec<Thread> {
    let mut by_root: BTreeMap<u64, Thread> = BTreeMap::new();
    let mut index: HashMap<String, u64> = HashMap::new();
    let mut mail: Vec<&Mail> = run
        .mail
        .iter()
        .filter(|m| m.kind != MailType::Heartbeat)
        .collect();
    mail.sort_by_key(|m| id_number(&m.id));
    for m in mail {
        let root = m
            .thread
            .as_deref()
            .or(m.reply_to.as_deref())
            .and_then(|r| index.get(r).copied());
        match root.and_then(|r| by_root.get_mut(&r)) {
            Some(t) => {
                t.replies.push(m.clone());
                index.insert(m.id.clone(), id_number(&t.root.id));
            }
            None => {
                let n = id_number(&m.id);
                index.insert(m.id.clone(), n);
                by_root.insert(
                    n,
                    Thread {
                        root: m.clone(),
                        replies: Vec::new(),
                        open_question: false,
                        choices: choices_of(m),
                        can_reply: m.from.starts_with("attempt:") || m.from.starts_with("run:"),
                    },
                );
            }
        }
    }
    let mut out: Vec<Thread> = by_root.into_values().collect();
    for t in &mut out {
        t.open_question = t.root.kind == MailType::Question
            && t.root.from.starts_with("attempt:")
            && !t.replies.iter().any(|r| r.kind == MailType::Answer);
    }
    out.sort_by(|a, b| match (a.open_question, b.open_question) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (true, true) => id_number(&a.root.id).cmp(&id_number(&b.root.id)),
        (false, false) => b
            .last_ms()
            .cmp(&a.last_ms())
            .then(id_number(&b.root.id).cmp(&id_number(&a.root.id))),
    });
    out
}

/// `run:r1` -> 코디네이터, `attempt:a2` -> `워커 a2 · <item>`, `human` -> 사람.
pub fn address_label(run: &RunEntry, addr: &str) -> String {
    if addr.starts_with("run:") {
        return s::COORDINATOR.to_string();
    }
    if let Some(a) = addr.strip_prefix("attempt:") {
        let title = run
            .attempt(a)
            .and_then(|x| run.item(&x.item))
            .map(|i| i.title.clone());
        return match title {
            Some(t) => format!("{} {a} · {t}", s::WORKER),
            None => format!("{} {a}", s::WORKER),
        };
    }
    if addr == "human" {
        return s::HUMAN.to_string();
    }
    addr.to_string()
}

pub fn mail_type_label(t: MailType) -> &'static str {
    match t {
        MailType::Question => s::M_QUESTION,
        MailType::Answer => s::M_ANSWER,
        MailType::Done => s::M_DONE,
        MailType::Escalation => s::M_ESCALATION,
        MailType::Status => s::M_STATUS,
        MailType::Handoff => s::M_HANDOFF,
        MailType::Heartbeat => s::M_HEARTBEAT,
        MailType::Gate => s::M_GATE,
    }
}

// ───────────────────────────── gates (v17 options) ─────────────────────────────

/// One decision button of a pending gate: `(label, approve?, choice)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateButton {
    pub label: String,
    pub approve: bool,
    pub choice: Option<String>,
}

/// A gate with options is approved by picking one (`choice`, docs/19 §4.5);
/// without options it is approve / reject. Reject is always offered.
pub fn gate_buttons(g: &podd::protocol::GateInfo) -> Vec<GateButton> {
    let mut out: Vec<GateButton> = if g.options.is_empty() {
        vec![GateButton {
            label: crate::strings::APPROVE.to_string(),
            approve: true,
            choice: None,
        }]
    } else {
        g.options
            .iter()
            .map(|o| GateButton {
                label: o.clone(),
                approve: true,
                choice: Some(o.clone()),
            })
            .collect()
    };
    out.push(GateButton {
        label: crate::strings::REJECT.to_string(),
        approve: false,
        choice: None,
    });
    out
}

/// Gates that belong to a run: by the gate's `run` (v17), else by the task
/// of one of its workers.
pub fn run_gates<'a>(
    run: &RunEntry,
    gates: &'a [podd::protocol::GateInfo],
) -> Vec<&'a podd::protocol::GateInfo> {
    let tasks: Vec<&String> = run
        .info
        .attempts
        .iter()
        .filter_map(|a| a.task.as_ref())
        .collect();
    gates
        .iter()
        .filter(|g| match &g.run {
            Some(r) => r == run.id(),
            None => g.task.as_ref().is_some_and(|t| tasks.contains(&t)),
        })
        .collect()
}

// ───────────────────────────── notifications ─────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Warning,
    Error,
}

/// A person-facing notification (docs/19 §4.3: blocked, gates, breaker,
/// `unknown`, and escalations — never a worker's ordinary turn or report).
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub title: String,
    pub body: String,
    /// Where a click goes: the run's coordinator terminal.
    pub run: String,
}

pub fn notice_for_mail(e: &OrchMailEvent) -> Option<Notice> {
    (e.kind == MailType::Escalation).then(|| Notice {
        kind: NoticeKind::Warning,
        title: format!("{} · {}", s::NOTICE_ESCALATION, e.run),
        body: super::ko::event_subject(e.text.as_ref(), &e.subject),
        run: e.run.clone(),
    })
}

/// Notices for what changed between two snapshots of a run: a start circuit
/// breaker that opened, a stop that could not be proven.
pub fn notices_for_change(old: Option<&RunInfo>, new: &RunInfo) -> Vec<Notice> {
    let Some(old) = old else { return Vec::new() };
    let mut out = Vec::new();
    for i in &new.items {
        let was = old
            .items
            .iter()
            .find(|o| o.id == i.id)
            .is_some_and(|o| o.circuit_open);
        if i.circuit_open && !was {
            out.push(Notice {
                kind: NoticeKind::Error,
                title: format!("{} · {}", s::NOTICE_BREAKER, new.run.id),
                body: format!("{} {} — {}", s::ITEM, i.id, i.title),
                run: new.run.id.clone(),
            });
        }
    }
    for a in &new.attempts {
        let was = old.attempts.iter().find(|o| o.id == a.id).map(|o| o.state);
        if a.state == AttemptState::Unknown && was.is_some_and(|w| w != AttemptState::Unknown) {
            out.push(Notice {
                kind: NoticeKind::Warning,
                title: format!("{} · {}", s::NOTICE_UNKNOWN, new.run.id),
                body: format!(
                    "{} {} — {}",
                    s::WORKER,
                    a.id,
                    a.error.clone().unwrap_or_default()
                ),
                run: new.run.id.clone(),
            });
        }
    }
    out
}

// ───────────────────────────── cleanup ─────────────────────────────

/// What the cleanup screen may do with a finished worker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanupActions {
    pub release: bool,
    pub keep: bool,
    pub archive: bool,
    pub resume: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CleanupRow {
    pub run: String,
    pub attempt: String,
    pub item_title: String,
    pub task: Option<String>,
    pub state: AttemptState,
    pub worker: WorkerState,
    /// From `task.cleanup_candidates` (S3); `None` = not listed (no task, or not loaded).
    pub tier: Option<CleanupTier>,
    pub reasons: Vec<String>,
    pub archived: bool,
    pub ended_ms: u64,
    pub actions: CleanupActions,
}

impl CleanupRow {
    /// Selected for the bulk action by default: only the `ready` tier.
    pub fn default_selected(&self) -> bool {
        self.tier == Some(CleanupTier::Ready) && self.actions.archive
    }
}

pub fn tier_label(t: Option<CleanupTier>) -> &'static str {
    match t {
        Some(CleanupTier::Protected) => s::TIER_PROTECTED,
        Some(CleanupTier::Review) => s::TIER_REVIEW,
        Some(CleanupTier::Ready) => s::TIER_READY,
        None => s::TIER_NONE,
    }
}

/// Workers whose attempt ended (newest first), with their task's cleanup tier.
pub fn cleanup_rows<'a>(
    runs: impl IntoIterator<Item = &'a RunEntry>,
    candidates: &HashMap<String, CleanupCandidate>,
) -> Vec<CleanupRow> {
    let mut out = Vec::new();
    for run in runs {
        for a in run.info.attempts.iter().filter(|a| !a.state.is_open()) {
            let c = a.task.as_ref().and_then(|t| candidates.get(t));
            let archived = c.is_some_and(|c| c.lifecycle == Lifecycle::Archived);
            let tier = c.map(|c| c.tier);
            let has_terminal = a.terminal.is_some() && a.worker != WorkerState::Released;
            out.push(CleanupRow {
                run: run.id().to_string(),
                attempt: a.id.clone(),
                item_title: run
                    .item(&a.item)
                    .map(|i| i.title.clone())
                    .unwrap_or_else(|| a.item.clone()),
                task: a.task.clone(),
                state: a.state,
                worker: a.worker,
                tier,
                reasons: c.map(|c| c.reasons.clone()).unwrap_or_default(),
                archived,
                ended_ms: a.ended_ms.unwrap_or(a.created_ms),
                actions: CleanupActions {
                    release: has_terminal,
                    keep: a.worker == WorkerState::Finished,
                    // Protected tasks are never archived from here; a task
                    // that is not listed is unknown, so it is not offered either.
                    archive: a.task.is_some()
                        && !archived
                        && matches!(tier, Some(CleanupTier::Ready | CleanupTier::Review)),
                    resume: a.task.is_some() && (archived || a.worker == WorkerState::Released),
                },
            });
        }
    }
    // Ready first, then review, protected, and tasks that are not listed.
    let rank = |t: Option<CleanupTier>| match t {
        Some(CleanupTier::Ready) => 0,
        Some(CleanupTier::Review) => 1,
        Some(CleanupTier::Protected) => 2,
        None => 3,
    };
    out.sort_by_key(|r| {
        (
            rank(r.tier),
            std::cmp::Reverse(r.ended_ms),
            std::cmp::Reverse(id_number(&r.attempt)),
        )
    });
    out
}

/// The default selection of the bulk action.
pub fn default_selection(rows: &[CleanupRow]) -> BTreeSet<String> {
    rows.iter()
        .filter(|r| r.default_selected())
        .map(|r| r.attempt.clone())
        .collect()
}

// ───────────────────────────── settings ─────────────────────────────

/// Choices the settings page offers (buttons, so the page is keyboard-only).
pub const RELEASE_CHOICES: [Option<u64>; 5] = [None, Some(5), Some(10), Some(30), Some(60)];
pub const STALL_CHOICES: [Option<u64>; 4] = [None, Some(10), Some(20), Some(60)];
pub const DEPTH_CHOICES: [u32; 4] = [1, 2, 3, 5];
/// Resource limits (`orch.limits.*`, v18) and the values offered for each,
/// in the daemon's text forms.
pub const LIMIT_CHOICES: [(&str, &[&str]); 6] = [
    ("max_workers", &["1", "2", "3", "4", "6"]),
    ("run_workers", &["2", "4", "8", "16"]),
    ("cpu_max_pct", &["off", "50%", "70%", "85%"]),
    ("min_free_mem", &["off", "2G", "4G", "8G"]),
    ("min_free_disk", &["off", "10G", "20G", "50G"]),
    ("cpu_max_wait", &["off", "10m", "30m", "60m"]),
];

/// Korean label of a limit key.
/// The Codex worker allowance of one repository as a chip (docs/19 §18.1).
pub fn allowance_label(on: Option<bool>) -> (&'static str, Tone) {
    match on {
        Some(true) => (s::ALLOW_ON, Tone::Ok),
        Some(false) => (s::ALLOW_OFF, Tone::Muted),
        None => (s::ALLOW_UNKNOWN, Tone::Muted),
    }
}

pub fn limit_label(key: &str) -> &'static str {
    match key {
        "max_workers" => s::LIM_MAX_WORKERS,
        "run_workers" => s::LIM_RUN_WORKERS,
        "cpu_max_pct" => s::LIM_CPU,
        "min_free_mem" => s::LIM_MEM,
        "min_free_disk" => s::LIM_DISK,
        "cpu_max_wait" => s::LIM_CPU_WAIT,
        _ => "",
    }
}

/// Is `choice` the limit's current value? Text forms may differ in case and
/// a trailing `%` (`70` vs `70%`).
pub fn limit_is(value: &str, choice: &str) -> bool {
    let norm = |v: &str| v.trim().trim_end_matches('%').to_ascii_lowercase();
    norm(value) == norm(choice)
}

/// Text for a choice button.
pub fn limit_choice_label(key: &str, choice: &str) -> String {
    match (key, choice) {
        (_, "off") => s::NEVER.to_string(),
        ("max_workers" | "run_workers", n) => format!("{n}개"),
        (_, v) if v.ends_with('m') => format!("{}분", v.trim_end_matches('m')),
        (_, v) => v.to_string(),
    }
}
pub const PLACE_CHOICES: [&str; 2] = ["new", "current"];

pub fn minutes_label(m: Option<u64>) -> String {
    match m {
        None => s::NEVER.to_string(),
        Some(m) if m % 60 == 0 => format!("{}시간", m / 60),
        Some(m) => format!("{m}분"),
    }
}

pub fn place_label(p: &str) -> &'static str {
    if p == "current" {
        s::PLACE_CURRENT
    } else {
        s::PLACE_NEW
    }
}

/// The settings with one field changed (what a button sends).
pub fn with_change(base: &OrchSettings, field: &str, value: &str) -> OrchSettings {
    let mut s = base.clone();
    let mins = |v: &str| v.parse::<u64>().ok().filter(|m| *m > 0);
    match field {
        "release_after_min" => s.release_after_min = mins(value),
        "stall_after_min" => s.stall_after_min = mins(value),
        "max_depth" => s.max_depth = value.parse().unwrap_or(s.max_depth),
        "default_place" => s.default_place = value.to_string(),
        _ => {}
    }
    s.locked.clear();
    s
}

/// One option of a radio group on the orchestration screens (ui-10: radio
/// segments with a checked state, not filled buttons).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOpt {
    pub value: String,
    pub text: String,
    pub selected: bool,
}

/// A settings row as a named radio group. `off_list` is the current value
/// when none of the options is it (set by the environment or the file):
/// shown as a chip, never lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceGroup {
    pub field: String,
    pub label: String,
    pub options: Vec<ChoiceOpt>,
    pub off_list: Option<String>,
    pub locked: bool,
}

impl ChoiceGroup {
    /// The accessible name of one option, e.g. `자동 정리: 10분`.
    pub fn option_name(&self, o: &ChoiceOpt) -> String {
        format!("{}: {}", self.label, o.text)
    }
}

fn group(
    field: &str,
    label: &str,
    opts: Vec<(String, String)>,
    current: &str,
    off_text: String,
    locked: bool,
) -> ChoiceGroup {
    let options: Vec<ChoiceOpt> = opts
        .into_iter()
        .map(|(value, text)| ChoiceOpt {
            selected: value == current,
            value,
            text,
        })
        .collect();
    let off_list = (!options.iter().any(|o| o.selected)).then_some(off_text);
    ChoiceGroup {
        field: field.into(),
        label: label.into(),
        options,
        off_list,
        locked,
    }
}

/// The four orchestration settings as radio groups.
pub fn settings_groups(cur: &OrchSettings) -> Vec<ChoiceGroup> {
    let locked = |f: &str| cur.locked.iter().any(|l| l == f);
    let mins = |m: Option<u64>| m.map(|m| m.to_string()).unwrap_or_default();
    let minute_opts = |c: &[Option<u64>]| {
        c.iter()
            .map(|m| (mins(*m), minutes_label(*m)))
            .collect::<Vec<_>>()
    };
    vec![
        group(
            "default_place",
            s::SET_PLACE,
            PLACE_CHOICES
                .iter()
                .map(|p| (p.to_string(), place_label(p).to_string()))
                .collect(),
            &cur.default_place,
            cur.default_place.clone(),
            locked("default_place"),
        ),
        group(
            "release_after_min",
            s::SET_RELEASE,
            minute_opts(&RELEASE_CHOICES),
            &mins(cur.release_after_min),
            minutes_label(cur.release_after_min),
            locked("release_after_min"),
        ),
        group(
            "stall_after_min",
            s::SET_STALL,
            minute_opts(&STALL_CHOICES),
            &mins(cur.stall_after_min),
            minutes_label(cur.stall_after_min),
            locked("stall_after_min"),
        ),
        group(
            "max_depth",
            s::SET_DEPTH,
            DEPTH_CHOICES
                .iter()
                .map(|d| (d.to_string(), d.to_string()))
                .collect(),
            &cur.max_depth.to_string(),
            cur.max_depth.to_string(),
            locked("max_depth"),
        ),
    ]
}

/// One resource limit as a radio group (`value` in the daemon's text form).
pub fn limit_group(key: &str, choices: &[&str], value: &str, locked: bool) -> ChoiceGroup {
    let options: Vec<ChoiceOpt> = choices
        .iter()
        .map(|c| ChoiceOpt {
            value: c.to_string(),
            text: limit_choice_label(key, c),
            selected: limit_is(value, c),
        })
        .collect();
    let off_list = (!options.iter().any(|o| o.selected)).then(|| value.to_string());
    ChoiceGroup {
        field: key.into(),
        label: limit_label(key).into(),
        options,
        off_list,
        locked,
    }
}

/// The board's run picker: one option per run, the shown one checked.
pub fn run_group(runs: &[&RunEntry], selected: Option<&str>) -> ChoiceGroup {
    let options = runs
        .iter()
        .map(|r| {
            let phase = if r.is_finished() {
                s::FINISHED_RUN
            } else {
                s::ACTIVE
            };
            ChoiceOpt {
                value: r.id().to_string(),
                text: format!("{} · {} · {phase}", r.id(), r.info.run.objective),
                selected: selected == Some(r.id()),
            }
        })
        .collect();
    ChoiceGroup {
        field: "run".into(),
        label: s::RUNS.into(),
        options,
        off_list: None,
        locked: false,
    }
}

// ───────────────────────────── unconfirmed wake-ups ─────────────────────────────

/// orch-02: a wake-up line Pod typed into an agent that was not confirmed
/// as taken (it may sit in the input box). Pod types no other line until
/// that agent checks its mail, so the person is shown it with a retry
/// (Enter in that terminal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StuckWake {
    /// `None` = the coordinator.
    pub attempt: Option<String>,
    pub terminal: Option<String>,
    pub since_ms: u64,
}

impl StuckWake {
    /// Who it is, e.g. `워커 a2` or `코디네이터`.
    pub fn who(&self) -> String {
        match &self.attempt {
            Some(a) => format!("{} {a}", s::WORKER),
            None => s::COORDINATOR.to_string(),
        }
    }

    /// Busy key of its retry.
    pub fn busy_key(&self) -> String {
        format!(
            "rewake:{}",
            self.attempt.as_deref().unwrap_or("coordinator")
        )
    }
}

/// Unconfirmed wake-ups of a run: the coordinator first, then workers by
/// number. Only those with a terminal can be retried.
pub fn stuck_wakes(run: &RunEntry) -> Vec<StuckWake> {
    let mut out = Vec::new();
    if let Some(ms) = run.info.wake_unconfirmed_ms {
        out.push(StuckWake {
            attempt: None,
            terminal: run.info.run.coordinator_terminal.clone(),
            since_ms: ms,
        });
    }
    let mut ws: Vec<&WorkerInfo> = run
        .workers
        .iter()
        .filter(|w| w.wake_unconfirmed_ms.is_some())
        .collect();
    ws.sort_by_key(|w| id_number(&w.attempt.id));
    out.extend(ws.into_iter().map(|w| StuckWake {
        attempt: Some(w.attempt.id.clone()),
        terminal: w.attempt.terminal.clone(),
        since_ms: w.wake_unconfirmed_ms.unwrap_or(0),
    }));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    // GPUI's glob exports its own `test` attribute; these are plain Rust tests.
    use core::prelude::v1::test;
    use podd::protocol::{Priority, Run};
    use serde_json::json;

    pub(crate) fn item(id: &str, state: ItemState, deps: &[&str]) -> Item {
        Item {
            id: id.into(),
            run: "r1".into(),
            title: format!("{id} 제목"),
            spec: String::new(),
            deps: deps.iter().map(|d| d.to_string()).collect(),
            parent: None,
            state,
            generation: 1,
            start_failures: 0,
            circuit_open: false,
            attempt: None,
            created_ms: 0,
            updated_ms: 0,
            gates: Vec::new(),
        }
    }

    pub(crate) fn attempt(
        id: &str,
        item: &str,
        state: AttemptState,
        worker: WorkerState,
    ) -> Attempt {
        serde_json::from_value(json!({
            "id": id, "run": "r1", "item": item, "generation": 1, "state": state, "worker": worker,
            "agent": "codex", "place": "new", "depth": 1, "created_ms": id_number(id) * 10,
            "task": format!("demo/{id}"), "terminal": format!("t-{id}")
        }))
        .unwrap()
    }

    pub(crate) fn mail(
        id: &str,
        from: &str,
        to: &str,
        kind: MailType,
        subject: &str,
        reply_to: Option<&str>,
    ) -> Mail {
        Mail {
            id: id.into(),
            run: "r1".into(),
            from: from.into(),
            to: to.into(),
            recipients: vec![to.into()],
            kind,
            subject: subject.into(),
            body: String::new(),
            thread: reply_to.map(str::to_string),
            reply_to: reply_to.map(str::to_string),
            payload: None,
            priority: Priority::Normal,
            created_ms: id_number(id) * 100,
            fenced: false,
            text: None,
            body_text: None,
            read_ms: None,
        }
    }

    pub(crate) fn run(items: Vec<Item>, attempts: Vec<Attempt>) -> RunEntry {
        RunEntry::new(
            RunInfo {
                run: Run {
                    id: "r1".into(),
                    objective: "목표".into(),
                    coordinator_terminal: Some("t0".into()),
                    coordinator_attempt: None,
                    depth: 0,
                    created_ms: 0,
                },
                items,
                attempts,
                unread: 0,
                mail: 0,
                wake_unconfirmed_ms: None,
            },
            HashMap::new(),
        )
    }

    #[test]
    fn allowance_labels() {
        assert_eq!(allowance_label(Some(true)), (s::ALLOW_ON, Tone::Ok));
        assert_eq!(allowance_label(Some(false)).0, s::ALLOW_OFF);
        assert_eq!(allowance_label(None).0, s::ALLOW_UNKNOWN, "older daemon");
    }

    #[test]
    fn columns_follow_item_state_and_dag_depth() {
        let r = run(
            vec![
                item("i3", ItemState::Pending, &["i2"]),
                item("i1", ItemState::Completed, &[]),
                item("i2", ItemState::Assigned, &["i1"]),
                item("i4", ItemState::Ready, &["i1", "i9"]),
            ],
            vec![],
        );
        let rows = item_rows(&r);
        assert_eq!(
            rows.iter()
                .map(|r| (r.id.as_str(), r.depth))
                .collect::<Vec<_>>(),
            [("i1", 0), ("i2", 1), ("i4", 1), ("i3", 2)]
        );
        let cols = columns(&r);
        assert_eq!(cols.len(), 6, "every column, even empty ones");
        let names: Vec<(&str, usize)> = cols
            .iter()
            .map(|(c, v)| (item_state_label(*c), v.len()))
            .collect();
        assert_eq!(
            names,
            [
                ("대기", 1),
                ("준비", 1),
                ("진행", 1),
                ("차단", 0),
                ("완료", 1),
                ("실패", 0)
            ]
        );
    }

    #[test]
    fn dependency_cycles_do_not_hang() {
        let d = item_depths(&[
            item("i1", ItemState::Pending, &["i2"]),
            item("i2", ItemState::Pending, &["i1"]),
        ]);
        assert!(d["i1"] <= 2 && d["i2"] <= 2);
    }

    #[test]
    fn worker_actions_follow_attempt_and_worker_state() {
        let open = attempt("a1", "i1", AttemptState::Ready, WorkerState::Active);
        assert_eq!(
            worker_actions(&open, false),
            WorkerActions {
                stop: true,
                abandon: true,
                keep: true,
                release: true,
                open_terminal: true,
                open_review: true
            }
        );
        // Queued (S6): nothing to stop or release yet, no terminal.
        let mut q = attempt("a2", "i1", AttemptState::Starting, WorkerState::Active);
        q.terminal = None;
        q.task = None;
        assert_eq!(
            worker_actions(&q, true),
            WorkerActions {
                abandon: true,
                keep: true,
                ..Default::default()
            }
        );
        let done = attempt("a3", "i1", AttemptState::Succeeded, WorkerState::Finished);
        let a = worker_actions(&done, false);
        assert!(!a.stop && !a.abandon && a.keep && a.release && a.open_terminal);
        let kept = attempt("a4", "i1", AttemptState::Abandoned, WorkerState::Kept);
        assert!(!worker_actions(&kept, false).keep, "already kept");
        let released = attempt("a5", "i1", AttemptState::Succeeded, WorkerState::Released);
        let a = worker_actions(&released, false);
        assert!(!a.release && !a.open_terminal && a.open_review);
    }

    #[test]
    fn cards_show_questions_queue_and_last_mail() {
        let mut r = run(
            vec![
                item("i1", ItemState::Assigned, &[]),
                item("i2", ItemState::Assigned, &[]),
            ],
            vec![
                attempt("a2", "i2", AttemptState::Starting, WorkerState::Active),
                attempt("a1", "i1", AttemptState::Ready, WorkerState::Active),
                attempt("a3", "i1", AttemptState::Succeeded, WorkerState::Released),
            ],
        );
        r.queues.insert(
            "a2".into(),
            QueueVm {
                kind: "workers".into(),
                reason: "동시 워커 3/3".into(),
                position: 1,
            },
        );
        r.mail = vec![
            mail(
                "m1",
                "attempt:a1",
                "run:r1",
                MailType::Question,
                "DB는?",
                None,
            ),
            mail(
                "m2",
                "attempt:a1",
                "run:r1",
                MailType::Heartbeat,
                "50%",
                None,
            ),
        ];
        let c = cards(&r);
        assert_eq!(
            c.iter().map(|c| c.attempt.as_str()).collect::<Vec<_>>(),
            ["a1", "a2"],
            "released workers are not on the board"
        );
        assert_eq!(c[0].question.as_deref(), Some("DB는?"));
        assert_eq!(
            c[0].last_mail,
            Some((MailType::Question, "DB는?".into())),
            "heartbeats are not the last message"
        );
        assert_eq!(c[0].tone(), Tone::Warn);
        assert!(c[0].aria().contains("질문 있음"));
        assert_eq!(c[1].state_label(), "자원 대기 (1번째)");
        assert!(!c[1].actions.stop);
        r.mail.push(mail(
            "m3",
            "human",
            "attempt:a1",
            MailType::Answer,
            "re: DB는?",
            Some("m1"),
        ));
        let c = cards(&r);
        assert_eq!(c[0].question, None, "answered");
        assert_eq!(c[0].tone(), Tone::Accent);
    }

    #[test]
    fn queue_is_read_from_raw_attempts() {
        let raw = json!({"attempts": [
            {"id": "a1", "queue": {"since_ms": 1, "kind": "cpu", "reason": "CPU 82% > 70%", "position": 2}},
            {"id": "a2", "queue": null}, {"id": "a3"}
        ]});
        let q = queues_of(&raw);
        assert_eq!(q.len(), 1);
        assert_eq!(
            q["a1"],
            QueueVm {
                kind: "cpu".into(),
                reason: "CPU 82% > 70%".into(),
                position: 2
            }
        );
    }

    #[test]
    fn threads_put_open_questions_first_and_group_replies() {
        let mut r = run(
            vec![item("i1", ItemState::Assigned, &[])],
            vec![
                attempt("a1", "i1", AttemptState::Ready, WorkerState::Active),
                attempt("a2", "i1", AttemptState::Ready, WorkerState::Active),
            ],
        );
        let mut q2 = mail(
            "m4",
            "attempt:a2",
            "run:r1",
            MailType::Question,
            "선택?",
            None,
        );
        q2.payload = Some(json!({"options": ["A안", "B안", ""]}));
        r.mail = vec![
            mail(
                "m1",
                "attempt:a1",
                "run:r1",
                MailType::Question,
                "질문1",
                None,
            ),
            mail(
                "m2",
                "run:r1",
                "attempt:a1",
                MailType::Answer,
                "re: 질문1",
                Some("m1"),
            ),
            mail("m3", "attempt:a1", "run:r1", MailType::Done, "끝", None),
            q2,
            mail(
                "m5",
                "attempt:a1",
                "run:r1",
                MailType::Heartbeat,
                "hb",
                None,
            ),
            mail(
                "m6",
                "attempt:a2",
                "run:r1",
                MailType::Escalation,
                "멈춤",
                None,
            ),
        ];
        let t = threads(&r);
        assert_eq!(
            t.iter().map(|t| t.root.id.as_str()).collect::<Vec<_>>(),
            ["m4", "m6", "m3", "m1"]
        );
        assert!(t[0].open_question && t[0].can_reply);
        assert_eq!(t[0].choices, ["A안", "B안"]);
        assert_eq!(t[3].replies.len(), 1);
        assert!(!t[3].open_question);
        assert_eq!(r.open_questions(), 1);
        assert_eq!(address_label(&r, "attempt:a1"), "워커 a1 · i1 제목");
        assert_eq!(address_label(&r, "run:r1"), "코디네이터");
        let mut human = mail("m7", "human", "run:r1", MailType::Status, "메모", None);
        human.from = "human".into();
        r.mail.push(human);
        assert!(
            !threads(&r)
                .iter()
                .find(|t| t.root.id == "m7")
                .unwrap()
                .can_reply,
            "a person has no mailbox"
        );
    }

    #[test]
    fn notices_only_for_escalations_breakers_and_unknown() {
        let ev = |kind| OrchMailEvent {
            seq: 1,
            run: "r1".into(),
            to: "run:r1".into(),
            id: "m1".into(),
            kind,
            subject: "stalled".into(),
            text: None,
            from: "pod".into(),
        };
        assert!(notice_for_mail(&ev(MailType::Escalation)).is_some());
        for k in [
            MailType::Question,
            MailType::Done,
            MailType::Answer,
            MailType::Status,
            MailType::Heartbeat,
        ] {
            assert!(
                notice_for_mail(&ev(k)).is_none(),
                "{k:?} is the coordinator's business"
            );
        }
        let old = run(
            vec![item("i1", ItemState::Ready, &[])],
            vec![attempt(
                "a1",
                "i1",
                AttemptState::Ready,
                WorkerState::Active,
            )],
        );
        let mut new = old.clone();
        new.info.items[0].circuit_open = true;
        new.info.items[0].state = ItemState::Failed;
        new.info.attempts[0].state = AttemptState::Unknown;
        let n = notices_for_change(Some(&old.info), &new.info);
        assert_eq!(
            n.iter().map(|n| n.kind.clone()).collect::<Vec<_>>(),
            [NoticeKind::Error, NoticeKind::Warning]
        );
        assert!(
            notices_for_change(None, &new.info).is_empty(),
            "first load is not news"
        );
        assert!(
            notices_for_change(Some(&new.info), &new.info).is_empty(),
            "no repeats"
        );
    }

    #[test]
    fn cleanup_tiers_select_ready_only_and_never_archive_protected() {
        let r = run(
            vec![item("i1", ItemState::Completed, &[])],
            vec![
                attempt("a1", "i1", AttemptState::Succeeded, WorkerState::Finished),
                attempt("a2", "i1", AttemptState::Failed, WorkerState::Released),
                attempt("a3", "i1", AttemptState::Stopped, WorkerState::Kept),
                attempt("a4", "i1", AttemptState::Ready, WorkerState::Active),
                attempt("a5", "i1", AttemptState::Succeeded, WorkerState::Released),
            ],
        );
        let cand = |task: &str, tier, lifecycle| CleanupCandidate {
            task: task.into(),
            tier,
            reasons: vec!["이유".into()],
            lifecycle,
            last_activity_ms: 0,
        };
        let c: HashMap<String, CleanupCandidate> = [
            cand("demo/a1", CleanupTier::Ready, Lifecycle::Active),
            cand("demo/a2", CleanupTier::Review, Lifecycle::Active),
            cand("demo/a3", CleanupTier::Protected, Lifecycle::Active),
            cand("demo/a5", CleanupTier::Ready, Lifecycle::Archived),
        ]
        .into_iter()
        .map(|c| (c.task.clone(), c))
        .collect();
        let rows = cleanup_rows([&r], &c);
        assert_eq!(
            rows.iter().map(|r| r.attempt.as_str()).collect::<Vec<_>>(),
            ["a5", "a1", "a2", "a3"],
            "ready, review, protected; open attempts excluded"
        );
        let by = |id: &str| rows.iter().find(|r| r.attempt == id).unwrap().clone();
        assert_eq!(
            by("a1").actions,
            CleanupActions {
                release: true,
                keep: true,
                archive: true,
                resume: false
            }
        );
        assert_eq!(
            by("a2").actions,
            CleanupActions {
                release: false,
                keep: false,
                archive: true,
                resume: true
            }
        );
        assert!(!by("a3").actions.archive, "protected is never archived");
        assert!(by("a5").archived && by("a5").actions.resume && !by("a5").actions.archive);
        assert_eq!(
            default_selection(&rows).into_iter().collect::<Vec<_>>(),
            ["a1"]
        );
        // Without candidates (not loaded / old daemon) nothing is archived.
        assert!(
            cleanup_rows([&r], &HashMap::new())
                .iter()
                .all(|r| !r.actions.archive && r.tier.is_none())
        );
    }

    #[test]
    fn settings_changes_and_labels() {
        let base = OrchSettings {
            release_after_min: Some(10),
            stall_after_min: Some(20),
            max_depth: 3,
            default_place: "new".into(),
            locked: vec!["max_depth".into()],
        };
        let s = with_change(&base, "release_after_min", "");
        assert_eq!(s.release_after_min, None);
        assert!(
            s.locked.is_empty(),
            "locked is the daemon's answer, never sent"
        );
        assert_eq!(with_change(&base, "max_depth", "2").max_depth, 2);
        assert_eq!(
            with_change(&base, "default_place", "current").default_place,
            "current"
        );
        assert_eq!(minutes_label(Some(60)), "1시간");
        assert_eq!(minutes_label(Some(5)), "5분");
        assert_eq!(minutes_label(None), "끔");
        assert_eq!(place_label("current"), "호출자 워크트리");
    }

    fn gate(options: &[&str], run: Option<&str>, task: Option<&str>) -> podd::protocol::GateInfo {
        serde_json::from_value(json!({"id": "g1", "title": "배포", "message": "어느 쪽?", "state": "pending", "created_ms": 1,
            "options": options, "run": run, "task": task}))
        .unwrap()
    }

    #[test]
    fn gate_options_become_choice_buttons() {
        let plain = gate_buttons(&gate(&[], None, None));
        assert_eq!(
            plain
                .iter()
                .map(|b| (b.label.as_str(), b.approve, b.choice.clone()))
                .collect::<Vec<_>>(),
            [("승인", true, None), ("거절", false, None)]
        );
        let g = gate_buttons(&gate(&["A안", "B안"], Some("r1"), None));
        assert_eq!(g.len(), 3);
        assert_eq!(
            (g[0].label.as_str(), g[0].choice.as_deref()),
            ("A안", Some("A안"))
        );
        assert!(
            g.iter().filter(|b| b.approve).all(|b| b.choice.is_some()),
            "a gate with options is approved by a choice"
        );
        assert!(!g[2].approve);
        // Belonging to a run: its `run`, else a worker's task.
        let r = run(
            vec![item("i1", ItemState::Blocked, &[])],
            vec![attempt(
                "a1",
                "i1",
                AttemptState::Ready,
                WorkerState::Active,
            )],
        );
        let gates = vec![
            gate(&[], Some("r1"), None),
            gate(&[], Some("r2"), Some("demo/a1")),
            gate(&[], None, Some("demo/a1")),
            gate(&[], None, Some("other")),
        ];
        let mine: Vec<usize> = run_gates(&r, &gates)
            .iter()
            .map(|g| gates.iter().position(|x| std::ptr::eq(x, *g)).unwrap())
            .collect();
        assert_eq!(mine, [0, 2]);
    }

    #[test]
    fn limit_choices_match_text_forms() {
        assert!(limit_is("70", "70%") && limit_is("4G", "4g") && limit_is("off", "off"));
        assert!(!limit_is("70", "85%"));
        assert_eq!(limit_choice_label("cpu_max_wait", "30m"), "30분");
        assert_eq!(limit_choice_label("max_workers", "3"), "3개");
        assert_eq!(limit_choice_label("min_free_mem", "off"), "끔");
        assert!(
            LIMIT_CHOICES
                .iter()
                .all(|(k, c)| !limit_label(k).is_empty() && !c.is_empty())
        );
    }

    #[test]
    fn finished_runs() {
        let mut r = run(
            vec![item("i1", ItemState::Completed, &[])],
            vec![attempt(
                "a1",
                "i1",
                AttemptState::Succeeded,
                WorkerState::Finished,
            )],
        );
        assert!(r.is_finished());
        r.info.attempts[0].state = AttemptState::Ready;
        assert!(!r.is_finished());
        assert!(!run(vec![], vec![]).is_finished());
    }

    /// ui-10: settings, limits and the run picker are radio groups with
    /// exactly one checked option (none when the value is set elsewhere,
    /// then shown as `현재 …`), named `<setting>: <option>`.
    #[test]
    fn choices_are_radio_groups_with_one_checked() {
        let cur = OrchSettings {
            release_after_min: Some(10),
            stall_after_min: Some(15),
            max_depth: 3,
            default_place: "new".into(),
            locked: vec!["max_depth".into()],
        };
        let gs = settings_groups(&cur);
        assert_eq!(
            gs.iter().map(|g| g.field.as_str()).collect::<Vec<_>>(),
            [
                "default_place",
                "release_after_min",
                "stall_after_min",
                "max_depth"
            ]
        );
        let checked = |g: &ChoiceGroup| {
            g.options
                .iter()
                .filter(|o| o.selected)
                .map(|o| o.text.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(checked(&gs[0]), [s::PLACE_NEW]);
        assert_eq!(checked(&gs[1]), ["10분"]);
        assert_eq!(
            gs[1].option_name(&gs[1].options[3]),
            format!("{}: 30분", s::SET_RELEASE)
        );
        assert!(
            checked(&gs[2]).is_empty() && gs[2].off_list.as_deref() == Some("15분"),
            "15 is not offered: shown, not lost"
        );
        assert_eq!(checked(&gs[3]), ["3"]);
        assert!(gs[3].locked && !gs[0].locked);
        let off = settings_groups(&OrchSettings {
            release_after_min: None,
            ..cur.clone()
        });
        assert_eq!(checked(&off[1]), [s::NEVER], "off is an option");

        let lim = limit_group("cpu_max_pct", &["off", "50%", "70%"], "70", true);
        assert_eq!(
            (checked(&lim), lim.off_list.clone(), lim.locked),
            (vec!["70%".to_string()], None, true)
        );
        let odd = limit_group("max_workers", &["1", "2"], "5", false);
        assert_eq!(
            (checked(&odd).len(), odd.off_list.as_deref()),
            (0, Some("5"))
        );

        let a = run(
            vec![],
            vec![attempt(
                "a1",
                "i1",
                AttemptState::Ready,
                WorkerState::Active,
            )],
        );
        let mut b = run(vec![], vec![]);
        b.info.run.id = "r2".into();
        let g = run_group(&[&a, &b], Some("r2"));
        assert_eq!(
            g.options
                .iter()
                .map(|o| (o.value.as_str(), o.selected))
                .collect::<Vec<_>>(),
            [("r1", false), ("r2", true)]
        );
        assert_eq!(
            g.option_name(&g.options[0]),
            format!("{}: r1 · 목표 · {}", s::RUNS, s::ACTIVE)
        );
    }

    /// orch-02: unconfirmed wake-ups of a run — coordinator first, then
    /// workers by number — with since, terminal and a mark on the card.
    #[test]
    fn stuck_wakes_list_coordinator_then_workers() {
        let mut r = run(
            vec![item("i1", ItemState::Assigned, &[])],
            vec![
                attempt("a10", "i1", AttemptState::Ready, WorkerState::Active),
                attempt("a2", "i1", AttemptState::Ready, WorkerState::Active),
            ],
        );
        assert!(stuck_wakes(&r).is_empty());
        let info = |a: &Attempt, ms: Option<u64>| WorkerInfo {
            attempt: Attempt {
                terminal: Some(format!("t-{}", a.id)),
                ..a.clone()
            },
            item_title: "a".into(),
            status: None,
            last_output_ms: 0,
            waiting_answer: false,
            unread: 1,
            next: String::new(),
            next_text: None,
            wake_unconfirmed_ms: ms,
        };
        r.workers = vec![
            info(&r.info.attempts[0], Some(50)),
            info(&r.info.attempts[1], Some(70)),
        ];
        r.info.wake_unconfirmed_ms = Some(40);
        let st = stuck_wakes(&r);
        assert_eq!(
            st,
            [
                StuckWake {
                    attempt: None,
                    terminal: Some("t0".into()),
                    since_ms: 40
                },
                StuckWake {
                    attempt: Some("a2".into()),
                    terminal: Some("t-a2".into()),
                    since_ms: 70
                },
                StuckWake {
                    attempt: Some("a10".into()),
                    terminal: Some("t-a10".into()),
                    since_ms: 50
                },
            ]
        );
        assert_eq!(
            (st[0].who(), st[1].who()),
            (s::COORDINATOR.to_string(), format!("{} a2", s::WORKER))
        );
        assert_ne!(st[0].busy_key(), st[1].busy_key());
        let c = card(&r, &r.info.attempts[1]);
        assert_eq!(c.wake_unconfirmed_ms, Some(70));
        assert!(c.aria().contains(s::WAKE_UNCONFIRMED) && c.tone() == Tone::Warn);
        r.workers[1].wake_unconfirmed_ms = None;
        assert!(
            !card(&r, &r.info.attempts[1])
                .aria()
                .contains(s::WAKE_UNCONFIRMED)
        );
    }
}
