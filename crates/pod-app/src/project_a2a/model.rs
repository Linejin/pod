//! The A2A section's state, without GPUI: per-project peers, cards, the
//! draft → review → send flow, and the remote tasks this project sent.
//!
//! Everything is keyed by project (registered repository name). A result
//! carries the project and peer it was requested for and is applied there
//! only, so switching the graph to another repository never shows, or
//! mixes in, another project's peers, tasks or replies.

use std::collections::BTreeMap;
use std::time::Duration;

use pod_a2a::model::{AgentCard, Message, Part, PartContent, StateClass, Task, TaskState};
use pod_a2a::{A2aError, SelectedInterface};

use super::config::ProjectPeers;

/// Auto refresh: delays between GetTask rounds for active tasks.
const POLL_DELAYS: [u64; 8] = [2, 3, 5, 8, 13, 21, 30, 30];
/// GetTask rounds per task before auto refresh stops (manual refresh resets).
pub const MAX_AUTO_POLLS: u32 = 20;
/// Tasks refreshed per round.
pub const MAX_POLL_BATCH: usize = 8;
/// Remote tasks and direct replies kept per project (oldest dropped).
pub const MAX_TRACKED: usize = 50;
/// Characters of one text part shown.
pub const MAX_PART_CHARS: usize = 4000;
/// Largest outgoing text (the client limit is 1 MiB; the review shows all).
pub const MAX_TEXT_BYTES: usize = 64 * 1024;

/// Delay before the next auto refresh after `polls` rounds; `None`: stop.
pub fn poll_delay(polls: u32) -> Option<Duration> {
    if polls >= MAX_AUTO_POLLS {
        return None;
    }
    let i = (polls as usize).min(POLL_DELAYS.len() - 1);
    Some(Duration::from_secs(POLL_DELAYS[i]))
}

/// A fetched Agent Card and the interface the client would use.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub card: AgentCard,
    pub card_url: String,
    pub interface: SelectedInterface,
    /// The card declares an HTTP Bearer scheme (the only one Pod can send).
    pub bearer_declared: bool,
}

impl Resolved {
    /// The skill `id`, if the card advertises it.
    pub fn skill(&self, id: &str) -> Option<&pod_a2a::model::AgentSkill> {
        self.card.skills.iter().find(|s| s.id == id)
    }

    /// Media types the skill takes in (its own, else the card default).
    pub fn input_modes(&self, skill: Option<&str>) -> Vec<String> {
        match skill.and_then(|s| self.skill(s)) {
            Some(s) if !s.input_modes.is_empty() => s.input_modes.clone(),
            _ => self.card.default_input_modes.clone(),
        }
    }

    /// Media types the skill returns (its own, else none: no preference).
    pub fn output_modes(&self, skill: Option<&str>) -> Vec<String> {
        match skill.and_then(|s| self.skill(s)) {
            Some(s) if !s.output_modes.is_empty() => s.output_modes.clone(),
            Some(_) => self.card.default_output_modes.clone(),
            None => Vec::new(),
        }
    }
}

/// The card of one peer.
#[derive(Clone, Debug)]
pub enum CardState {
    Loading,
    Ready(Box<Resolved>),
    /// Discovery or selection failed; the error is kept as returned.
    Failed {
        code: &'static str,
        error: String,
    },
}

/// Exactly what one SendMessage will carry, frozen when the user asks to
/// review. Sending uses this value, not the live draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Review {
    pub repo: String,
    pub peer_id: String,
    pub peer_display: String,
    pub card_url: String,
    pub interface_url: String,
    pub skill: Option<(String, String)>,
    pub accepted_output_modes: Vec<String>,
    pub text: String,
    pub message_id: String,
    pub task_id: String,
    pub context_id: String,
    pub bearer: bool,
    /// A deliberate resend with the message id of an unconfirmed send.
    pub resend: bool,
}

/// Where the send flow is.
#[derive(Clone, Debug, Default)]
pub enum Submit {
    #[default]
    Compose,
    Reviewing(Box<Review>),
    Sending(Box<Review>),
    /// Not sent (known outcome): the draft is kept.
    Failed {
        review: Box<Review>,
        error: String,
    },
}

/// A send that may have reached the peer without a valid answer. Kept until
/// a send with the same message id is answered or the user lets it go; Pod
/// never resends it on its own.
#[derive(Clone, Debug)]
pub struct Unconfirmed {
    pub review: Box<Review>,
    pub error: String,
}

impl Submit {
    pub fn busy(&self) -> bool {
        matches!(self, Self::Sending(_))
    }
}

/// Continuing a remote task or conversation with the next message.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Continue {
    pub peer_id: String,
    pub task_id: String,
    pub context_id: String,
}

/// A task this project created or continued at a peer (by SendMessage).
#[derive(Clone, Debug)]
pub struct RemoteTask {
    pub peer_id: String,
    pub peer_display: String,
    pub task_id: String,
    pub context_id: String,
    /// The message id Pod sent that produced or continued this task.
    pub message_id: String,
    /// Last valid snapshot from the peer (peer data).
    pub task: Task,
    /// GetTask answers received (0: only the SendMessage result).
    pub fetched: u32,
    /// Last refresh error, kept next to the last good snapshot.
    pub error: Option<String>,
    pub refreshing: bool,
    /// Auto refresh rounds since the last open or manual refresh.
    pub polls: u32,
    /// The peer does not answer GetTask for this task (no auto refresh).
    pub no_refresh: Option<String>,
}

impl RemoteTask {
    pub fn state(&self) -> &TaskState {
        &self.task.status.state
    }

    pub fn active(&self) -> bool {
        self.state().class() == StateClass::Active
    }

    /// Wants an automatic GetTask.
    pub fn wants_poll(&self) -> bool {
        self.active() && self.no_refresh.is_none() && self.polls < MAX_AUTO_POLLS
    }
}

/// A direct Message reply (no task was created).
#[derive(Clone, Debug)]
pub struct DirectReply {
    pub peer_id: String,
    pub peer_display: String,
    pub sent_message_id: String,
    pub reply: Message,
}

/// One project's A2A state.
#[derive(Clone, Debug)]
pub struct ProjectA2a {
    pub peers: ProjectPeers,
    /// The peer chosen explicitly (no peer is preselected).
    pub selected: Option<String>,
    pub cards: BTreeMap<String, CardState>,
    /// Skill chosen per peer (`None`: no skill preference).
    pub skill: BTreeMap<String, String>,
    pub continuing: Option<Continue>,
    pub submit: Submit,
    pub unconfirmed: Option<Unconfirmed>,
    /// Newest first.
    pub tasks: Vec<RemoteTask>,
    /// Newest first.
    pub replies: Vec<DirectReply>,
}

impl Default for ProjectA2a {
    fn default() -> Self {
        Self {
            peers: ProjectPeers::NoFile,
            selected: None,
            cards: BTreeMap::new(),
            skill: BTreeMap::new(),
            continuing: None,
            submit: Submit::Compose,
            unconfirmed: None,
            tasks: Vec::new(),
            replies: Vec::new(),
        }
    }
}

/// Why the draft cannot be reviewed yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotReady {
    NoPeer,
    CardMissing,
    EmptyText,
    TooLong(usize),
    /// The chosen skill (or the card) does not take `text/plain`.
    TextNotAccepted(Vec<String>),
    /// A token is configured but the card declares no HTTP Bearer scheme.
    TokenNotDeclared,
    Busy,
    /// An earlier send's outcome is unknown: resend it or let it go first.
    Unconfirmed,
}

impl ProjectA2a {
    /// Replaces the peer list (reload); drops state of peers that are gone.
    pub fn set_peers(&mut self, peers: ProjectPeers) {
        let keep = |id: &str| peers.usable(id).is_some();
        if self.selected.as_deref().is_some_and(|s| !keep(s)) {
            self.selected = None;
        }
        self.cards.retain(|id, _| keep(id));
        self.skill.retain(|id, _| keep(id));
        if self.continuing.as_ref().is_some_and(|c| !keep(&c.peer_id)) {
            self.continuing = None;
        }
        // A changed definition must be fetched again.
        if self.peers != peers {
            self.cards.clear();
        }
        self.peers = peers;
    }

    /// Selects `id` explicitly. Returns whether its card must be fetched.
    pub fn select(&mut self, id: &str) -> bool {
        if self.peers.usable(id).is_none() || self.submit.busy() {
            return false;
        }
        if self.selected.as_deref() != Some(id) {
            self.selected = Some(id.to_string());
            if let Submit::Reviewing(_) = self.submit {
                self.submit = Submit::Compose;
            }
            if self.continuing.as_ref().is_some_and(|c| c.peer_id != id) {
                self.continuing = None;
            }
        }
        !matches!(
            self.cards.get(id),
            Some(CardState::Ready(_)) | Some(CardState::Loading)
        )
    }

    pub fn card(&self) -> Option<&CardState> {
        self.selected.as_ref().and_then(|id| self.cards.get(id))
    }

    pub fn resolved(&self) -> Option<&Resolved> {
        match self.card()? {
            CardState::Ready(r) => Some(r),
            _ => None,
        }
    }

    /// The skill chosen for the selected peer, if the card still has it.
    pub fn chosen_skill(&self) -> Option<&str> {
        let peer = self.selected.as_deref()?;
        let id = self.skill.get(peer)?;
        self.resolved()?.skill(id).map(|s| s.id.as_str())
    }

    /// Freezes the draft into a review, or says why it cannot be sent.
    pub fn review(&self, repo: &str, text: &str, message_id: String) -> Result<Review, NotReady> {
        if self.submit.busy() {
            return Err(NotReady::Busy);
        }
        if self.unconfirmed.is_some() {
            return Err(NotReady::Unconfirmed);
        }
        let Some(peer) = self.selected.as_deref() else {
            return Err(NotReady::NoPeer);
        };
        let Some(def) = self.peers.usable(peer) else {
            return Err(NotReady::NoPeer);
        };
        let Some(r) = self.resolved() else {
            return Err(NotReady::CardMissing);
        };
        if text.trim().is_empty() {
            return Err(NotReady::EmptyText);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Err(NotReady::TooLong(text.len()));
        }
        let skill = self.chosen_skill();
        let inputs = r.input_modes(skill);
        if !inputs.is_empty() && !inputs.iter().any(|m| accepts_text(m)) {
            return Err(NotReady::TextNotAccepted(inputs));
        }
        if def.has_token() && !r.bearer_declared {
            return Err(NotReady::TokenNotDeclared);
        }
        let cont = self
            .continuing
            .as_ref()
            .filter(|c| c.peer_id == peer)
            .cloned()
            .unwrap_or_default();
        Ok(Review {
            repo: repo.to_string(),
            peer_id: peer.to_string(),
            peer_display: def.display(),
            card_url: r.card_url.clone(),
            interface_url: r.interface.url.clone(),
            skill: skill.and_then(|s| r.skill(s)).map(|s| {
                (
                    s.id.clone(),
                    pod_a2a::sanitize_line(if s.name.is_empty() { &s.id } else { &s.name }, 80),
                )
            }),
            accepted_output_modes: r.output_modes(skill),
            text: text.to_string(),
            message_id,
            task_id: cont.task_id,
            context_id: cont.context_id,
            bearer: def.has_token() && r.bearer_declared,
            resend: false,
        })
    }

    /// A resend of the unconfirmed message: same id, same text, same target.
    pub fn resend_review(&self) -> Option<Review> {
        if self.submit.busy() {
            return None;
        }
        self.unconfirmed.as_ref().map(|u| Review {
            resend: true,
            ..(*u.review).clone()
        })
    }

    /// Leaves a review: a resend goes back to its unconfirmed message.
    pub fn leave_review(&mut self) {
        if let Submit::Reviewing(_) = self.submit {
            self.submit = Submit::Compose;
        }
    }

    /// The user lets an unconfirmed message go (and writes a new one).
    pub fn forget_unconfirmed(&mut self) {
        if self.submit.busy() {
            return;
        }
        self.unconfirmed = None;
        if let Submit::Failed { .. } | Submit::Reviewing(_) = self.submit {
            self.submit = Submit::Compose;
        }
    }

    /// Whether a delivered answer settles the unconfirmed message.
    fn settle(&mut self, review: &Review) {
        if self
            .unconfirmed
            .as_ref()
            .is_some_and(|u| u.review.message_id == review.message_id)
        {
            self.unconfirmed = None;
        }
    }

    /// Records a SendMessage answer for `review`. Returns whether the draft
    /// was delivered (and may be cleared).
    pub fn apply_send(
        &mut self,
        review: Review,
        result: Result<pod_a2a::model::SendMessageResponse, A2aError>,
    ) -> bool {
        match result {
            Ok(pod_a2a::model::SendMessageResponse::Task(task)) => {
                self.settle(&review);
                self.record_task(&review, task);
                self.submit = Submit::Compose;
                self.continuing = None;
                true
            }
            Ok(pod_a2a::model::SendMessageResponse::Message(reply)) => {
                self.settle(&review);
                self.replies.insert(
                    0,
                    DirectReply {
                        peer_id: review.peer_id.clone(),
                        peer_display: review.peer_display.clone(),
                        sent_message_id: review.message_id.clone(),
                        reply,
                    },
                );
                self.replies.truncate(MAX_TRACKED);
                self.submit = Submit::Compose;
                self.continuing = None;
                true
            }
            Err(e) if e.outcome_unknown() => {
                self.submit = Submit::Compose;
                self.unconfirmed = Some(Unconfirmed {
                    review: Box::new(Review {
                        resend: false,
                        ..review
                    }),
                    error: e.to_string(),
                });
                false
            }
            Err(e) => {
                self.submit = Submit::Failed {
                    review: Box::new(review),
                    error: e.to_string(),
                };
                false
            }
        }
    }

    /// The send was refused before anything left (re-resolution changed).
    pub fn refuse_send(&mut self, review: Review, error: String) {
        self.submit = Submit::Failed {
            review: Box::new(review),
            error,
        };
    }

    fn record_task(&mut self, review: &Review, task: Task) {
        let at = self
            .tasks
            .iter()
            .position(|t| t.peer_id == review.peer_id && t.task_id == task.id);
        let entry = RemoteTask {
            peer_id: review.peer_id.clone(),
            peer_display: review.peer_display.clone(),
            task_id: task.id.clone(),
            context_id: task.context_id.clone(),
            message_id: review.message_id.clone(),
            task,
            fetched: 0,
            error: None,
            refreshing: false,
            polls: 0,
            no_refresh: None,
        };
        if let Some(i) = at {
            self.tasks.remove(i);
        }
        self.tasks.insert(0, entry);
        self.tasks.truncate(MAX_TRACKED);
    }

    /// Tasks to refresh now (all active ones when `manual`, else those whose
    /// auto refresh has not run out), marked as refreshing.
    pub fn take_refreshes(&mut self, manual: Option<(&str, &str)>) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for t in &mut self.tasks {
            if t.refreshing || out.len() >= MAX_POLL_BATCH {
                continue;
            }
            let due = match manual {
                Some((peer, id)) => t.peer_id == peer && t.task_id == id,
                None => t.wants_poll(),
            };
            if due {
                t.refreshing = true;
                if manual.is_some() {
                    t.polls = 0;
                } else {
                    t.polls += 1;
                }
                out.push((t.peer_id.clone(), t.task_id.clone()));
            }
        }
        out
    }

    /// Opening the section (or coming back to this project) restarts auto
    /// refresh for active tasks.
    pub fn reset_polls(&mut self) {
        for t in &mut self.tasks {
            t.polls = 0;
        }
    }

    /// The delay before the next auto refresh round, if any task wants one.
    pub fn next_poll(&self) -> Option<Duration> {
        self.tasks
            .iter()
            .filter(|t| t.wants_poll())
            .map(|t| t.polls)
            .min()
            .and_then(poll_delay)
    }

    /// Records a GetTask answer for (`peer`, `id`).
    pub fn apply_task(&mut self, peer: &str, id: &str, result: Result<Task, A2aError>) {
        let Some(t) = self
            .tasks
            .iter_mut()
            .find(|t| t.peer_id == peer && t.task_id == id)
        else {
            return;
        };
        t.refreshing = false;
        match result {
            Ok(task) => {
                if !task.context_id.is_empty() {
                    t.context_id = task.context_id.clone();
                }
                t.task = task;
                t.fetched += 1;
                t.error = None;
            }
            Err(e) => {
                if let A2aError::Rpc(rpc) = &e
                    && matches!(
                        rpc.kind,
                        pod_a2a::RpcErrorKind::MethodNotFound
                            | pod_a2a::RpcErrorKind::TaskNotFound
                            | pod_a2a::RpcErrorKind::UnsupportedOperation
                    )
                {
                    t.no_refresh = Some(rpc.kind.name().to_string());
                }
                t.error = Some(e.to_string());
            }
        }
    }

    /// Continue `task` (input-required) with the next message.
    pub fn continue_task(&mut self, peer: &str, id: &str) -> bool {
        let Some(t) = self
            .tasks
            .iter()
            .find(|t| t.peer_id == peer && t.task_id == id)
        else {
            return false;
        };
        if t.state().is_terminal() || self.submit.busy() {
            return false;
        }
        self.continuing = Some(Continue {
            peer_id: peer.to_string(),
            task_id: t.task_id.clone(),
            context_id: t.context_id.clone(),
        });
        self.selected = Some(peer.to_string());
        self.submit = Submit::Compose;
        true
    }

    /// Continue the conversation of direct reply `n` (same context, new task).
    pub fn continue_context(&mut self, n: usize) -> bool {
        let Some(r) = self.replies.get(n) else {
            return false;
        };
        if r.reply.context_id.is_empty() || self.submit.busy() {
            return false;
        }
        self.continuing = Some(Continue {
            peer_id: r.peer_id.clone(),
            task_id: String::new(),
            context_id: r.reply.context_id.clone(),
        });
        self.selected = Some(r.peer_id.clone());
        self.submit = Submit::Compose;
        true
    }
}

/// `text/plain`, `text/*`, `*/*` (parameters ignored).
pub fn accepts_text(mode: &str) -> bool {
    let m = mode
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    matches!(m.as_str(), "text/plain" | "text/*" | "*/*")
}

/// One reply part as display data (peer content, never executed or opened).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PartView {
    Text(String),
    Data(String),
    /// A URL part: shown as text, never fetched or opened.
    Url {
        url: String,
        name: String,
    },
    Raw {
        bytes: Option<usize>,
        name: String,
    },
}

pub fn part_view(p: &Part) -> PartView {
    let name = pod_a2a::sanitize_line(&p.filename, 120);
    match &p.content {
        PartContent::Text(t) => PartView::Text(pod_a2a::sanitize_text(t, MAX_PART_CHARS)),
        PartContent::Data(v) => PartView::Data(pod_a2a::sanitize_text(
            &serde_json::to_string_pretty(v).unwrap_or_default(),
            MAX_PART_CHARS,
        )),
        PartContent::Url(u) => PartView::Url {
            url: pod_a2a::sanitize_line(u, 400),
            name,
        },
        PartContent::Raw(b) => PartView::Raw {
            bytes: Part::raw_len(b),
            name,
        },
    }
}
