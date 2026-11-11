//! Worker cards (docs/19 §5.2: what a card shows, which buttons it offers)
//! and unconfirmed wake-ups (orch-02).

use podd::protocol::{Attempt, AttemptState, Mail, MailType, WorkerInfo, WorkerState};
use serde_json::Value;

use super::mail::answered;
use super::{QueueVm, RunEntry, Tone, id_number};
use crate::orch_ui::{ko, s};

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
        .map(ko::subject);
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
        last_mail: last.map(|m| (m.kind, ko::subject(m))),
        question,
        waiting_answer: w.is_some_and(|w| w.waiting_answer),
        unread: w.map(|w| w.unread).unwrap_or(0),
        next: w.map(ko::next).unwrap_or_default(),
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
