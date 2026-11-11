//! What the page shows, as plain data: the actions a session offers, its
//! heading, state label and age, and the card / page lines. No window, no
//! requests, no clock (`now` is an argument).

use podd::protocol::{CwdMatch, DiscoveredAgent, DiscoveredSession, SessionState, SessionsList};

use super::copy::{self, s};

pub(super) fn state_label(s: SessionState) -> &'static str {
    match s {
        SessionState::RunningPod => s::RUNNING_POD,
        SessionState::RunningOther => s::RUNNING_OTHER,
        SessionState::Resumable => s::RESUMABLE,
        SessionState::Stale => s::STALE,
    }
}

pub(super) fn agent_label(a: DiscoveredAgent) -> &'static str {
    a.label()
}

/// "3분 전" style age of a unix-ms time.
pub(super) fn ago(ms: Option<u64>, now_ms: u64) -> String {
    let Some(ms) = ms else {
        return copy::NO_TIME.into();
    };
    let d = now_ms.saturating_sub(ms) / 1000;
    match d {
        0..60 => copy::JUST_NOW.into(),
        60..3600 => copy::minutes_ago(d / 60),
        3600..86_400 => copy::hours_ago(d / 3600),
        _ => copy::days_ago(d / 86_400),
    }
}

/// The card heading: title, else first prompt, else the agent and short id.
pub(super) fn heading(x: &DiscoveredSession) -> String {
    x.title
        .clone()
        .or_else(|| x.preview.clone())
        .unwrap_or_else(|| copy::untitled(agent_label(x.agent), x.id.get(..8).unwrap_or(&x.id)))
}

/// Which actions a session offers.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Actions {
    pub open_terminal: Option<String>,
    pub resume: bool,
    /// `Some(false)`: register to the matching task/repo; `Some(true)`: register the repository first.
    pub register: Option<bool>,
}

pub(super) fn actions(x: &DiscoveredSession) -> Actions {
    Actions {
        open_terminal: x.running.as_ref().and_then(|r| r.pod_terminal.clone()),
        resume: x.running.is_none() && x.cwd_exists,
        register: if x.link.is_some() || !x.cwd_exists {
            None
        } else {
            Some(x.matched.is_empty())
        },
    }
}

/// The task a match or link names, else its repository, else nothing.
pub(super) fn place(m: &CwdMatch) -> String {
    m.task.clone().or(m.repo.clone()).unwrap_or_default()
}

/// The line while there is no list: unavailable after a failed read (and no
/// read in flight), else loading.
pub(super) fn no_list_line(failed: bool, loading: bool) -> &'static str {
    if failed && !loading {
        s::UNAVAILABLE
    } else {
        s::LOADING
    }
}

/// The notes under the toolbar: last scan, session count, unknown agents.
pub(super) fn notes_line(list: &SessionsList, now: u64) -> String {
    let mut notes = Vec::new();
    if let Some(ms) = list.scanned_ms {
        notes.push(copy::scanned_at(&ago(Some(ms), now)));
    }
    notes.push(copy::session_count(list.sessions.len()));
    if !list.unknown_running.is_empty() {
        notes.push(copy::unknown_running(list.unknown_running.len()));
    }
    notes.join(copy::SEP)
}

/// The line of an empty list: never scanned, or nothing found.
pub(super) fn empty_line(list: &SessionsList) -> &'static str {
    if list.scanned_ms.is_none() {
        s::NOT_SCANNED
    } else {
        s::EMPTY
    }
}

/// A card's facts: tool, folder, branch, last activity, messages, model.
pub(super) fn facts_line(x: &DiscoveredSession, now: u64) -> String {
    let mut facts = vec![agent_label(x.agent).to_string(), x.cwd.clone()];
    if let Some(b) = &x.branch {
        facts.push(copy::branch(b));
    }
    facts.push(copy::last_activity(&ago(x.last_ms, now)));
    if let Some(n) = x.messages {
        facts.push(copy::messages(n));
    }
    if let Some(m) = &x.model {
        facts.push(m.clone());
    }
    facts.join(copy::SEP)
}

/// Where a card's session belongs: linked, matched by folder, or neither.
pub(super) fn link_line(x: &DiscoveredSession) -> String {
    match (&x.link, x.matched.is_empty()) {
        (Some(l), _) => copy::labeled(s::LINKED, &place(l)),
        (None, false) => copy::labeled(s::MATCHED, &place(&x.matched)),
        (None, true) => s::UNKNOWN_FOLDER.to_string(),
    }
}
