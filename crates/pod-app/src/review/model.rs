//! Review policy and display plans (docs/20-refactor/app.md §2, "순수 표시
//! 정책"): what "Send" sends, how a delivery, a submit check and a refusal
//! read, when a turn ended, what the toasts say, and what the toolbar, the
//! file list, the comment column and the inline cards show. Inputs are plain
//! data; nothing here touches a window or an entity, sends a request or reads
//! the clock. Colors are theme tokens, as in the rest of the panel.

use std::collections::{HashMap, HashSet};

use gpui_kit::component::diff::DiffSide;
use pod_diff::{FileRow, SourceFile};
use podd::protocol::{
    self as p, CommentState, DeliveryInfo, DeliveryState, NewComment, ReviewComment, Side, codes,
    orch_codes,
};

use super::color;
use super::copy::side_label;
use crate::source::SourceError;
use crate::vm::{AgentState, StatusVm, TaskVm};
use crate::workspace::ToastKind;

pub(super) fn side_of(side: DiffSide) -> Side {
    match side {
        DiffSide::Original => Side::Old,
        DiffSide::Modified => Side::New,
    }
}

pub(super) fn diff_side(side: Side) -> DiffSide {
    match side {
        Side::Old => DiffSide::Original,
        Side::New => DiffSide::Modified,
    }
}

pub(super) fn location(c: &ReviewComment) -> String {
    let lines = match c.end_line {
        Some(e) if e != c.line => format!("{}–{e}", c.line),
        _ => c.line.to_string(),
    };
    format!("{}:{lines} ({})", c.file, side_label(c.side))
}

/// What the last "Send to agent" did, shown under the send box.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum SendOutcome {
    /// podd stored or typed the delivery; the live state is in `deliveries`.
    Delivery(String),
    /// podd refused right away (agent blocked, terminal exited, nothing to send).
    Refused(String),
}

/// Delivery state as one line of text and a color.
pub(super) fn delivery_text(d: &DeliveryInfo) -> (String, u32) {
    let n = d.comment_ids.len();
    match d.state {
        DeliveryState::Queued => (
            crate::tr!(N_COMMENT_S_QUEUED_UNTIL_THE_AGENT_IS_IDLE_WAS; n; d.id, AgentState::parse(&d.agent_state).label()),
            color::warn(),
        ),
        DeliveryState::Delivered => (
            crate::tr!(N_COMMENT_S_DELIVERED_TO; n; d.id, d.terminal, AgentState::parse(d.delivered_state.as_deref().unwrap_or(&d.agent_state)).label()),
            color::ok(),
        ),
        DeliveryState::Cancelled => (crate::tr!(CANCELLED; ; d.id), color::muted()),
        DeliveryState::Failed => (
            crate::tr!(FAILED; ; d.id, d.error.as_deref().unwrap_or(crate::strings::NOT_DELIVERED)),
            color::del(),
        ),
        DeliveryState::Preview => (crate::tr!(PREVIEW; ; d.id), color::muted()),
    }
}

/// v8 submit check of a delivered prompt as a short line and a color; `None`
/// when there is nothing to say (no check, or a shell that cannot confirm).
/// `unconfirmed` means the prompt may still sit in the agent's input box.
pub(super) fn submit_text(s: &p::SubmitCheck) -> Option<(String, u32)> {
    match s.state {
        p::SubmitState::Pending => Some((crate::strings::SUBMIT_PENDING.into(), color::muted())),
        p::SubmitState::Confirmed => Some((crate::strings::SUBMIT_CONFIRMED.into(), color::ok())),
        p::SubmitState::Unconfirmed => {
            Some((crate::strings::SUBMIT_UNCONFIRMED.into(), color::warn()))
        }
        p::SubmitState::Unverifiable => None,
    }
}

/// Whether `new` newly reports an unconfirmed submit (toast once).
pub(super) fn became_unconfirmed(old: Option<&p::SubmitCheck>, new: &p::SubmitCheck) -> bool {
    new.state == p::SubmitState::Unconfirmed
        && old.is_none_or(|o| o.state != p::SubmitState::Unconfirmed)
}

/// Text for a refused `review.send`.
pub(super) fn refusal_text(e: &SourceError) -> String {
    match e.code {
        // podd's message already says what the agent waits for (docs/11 §3.1).
        Some(orch_codes::AGENT_BLOCKED) => crate::tr!(REFUSED_AGENT_BLOCKED; ; e.message),
        Some(codes::TERMINAL_EXITED) => crate::tr!(REFUSED_AGENT_TERMINAL_EXITED; ; e.message),
        Some(orch_codes::INVALID_STATE) => crate::tr!(NOTHING_TO_SEND; ; e.message),
        _ => crate::tr!(SEND_FAILED; ; e.message),
    }
}

/// Deliveries that left `queued` between two reads (for toasts).
pub(super) fn finished_deliveries<'a>(
    before: &[DeliveryInfo],
    after: &'a [DeliveryInfo],
) -> Vec<&'a DeliveryInfo> {
    let was_queued: HashSet<&str> = before
        .iter()
        .filter(|d| d.state == DeliveryState::Queued)
        .map(|d| d.id.as_str())
        .collect();
    after
        .iter()
        .filter(|d| d.state != DeliveryState::Queued && was_queued.contains(d.id.as_str()))
        .collect()
}

/// The toast for a delivery that left `queued`: delivered, or not.
pub(super) fn finished_toast(d: &DeliveryInfo, task: &str) -> (ToastKind, String, String) {
    let (text, _) = delivery_text(d);
    let ok = d.state == DeliveryState::Delivered;
    let kind = if ok {
        ToastKind::Success
    } else {
        ToastKind::Warning
    };
    let title = crate::tr!(REVIEW_FOR; ; if ok { crate::strings::DELIVERED } else { crate::strings::NOT_DELIVERED }, task);
    (kind, title, text)
}

/// The toast for a `review.send` podd took: typed in now, or queued.
pub(super) fn sent_toast(d: &DeliveryInfo, task: &str) -> (ToastKind, String, String) {
    let (text, _) = delivery_text(d);
    match d.state {
        DeliveryState::Delivered => (ToastKind::Success, crate::tr!(REVIEW_SENT_TO; ; task), text),
        _ => (ToastKind::Info, crate::tr!(REVIEW_QUEUED_FOR; ; task), text),
    }
}

/// Did the agent just finish a turn (time to re-read the diff)?
pub(super) fn turn_ended(old: Option<&StatusVm>, new: &StatusVm) -> bool {
    match old {
        Some(o) => {
            new.turn > o.turn
                || (o.state == AgentState::Working && new.state != AgentState::Working)
        }
        None => false,
    }
}

/// Which comments "Send" sends: the checked ones that can be sent, or all
/// open ones when nothing is checked. `None` = `all_open`; `Some(empty)` =
/// only resolved comments are checked, so nothing is sent (never the
/// unchecked ones instead).
pub(super) fn send_selection(
    comments: &[ReviewComment],
    checked: &HashSet<String>,
) -> Option<Vec<String>> {
    let checked: Vec<&ReviewComment> = comments
        .iter()
        .filter(|c| checked.contains(&c.id))
        .collect();
    if checked.is_empty() {
        return None;
    }
    Some(
        checked
            .into_iter()
            .filter(|c| c.state != CommentState::Resolved)
            .map(|c| c.id.clone())
            .collect(),
    )
}

/// `review.send` for `selection` (see [`send_selection`]) with the trimmed
/// instructions; empty instructions are left out.
pub(super) fn send_params(
    task: &str,
    selection: Option<Vec<String>>,
    instructions: String,
) -> p::ReviewSendParams {
    p::ReviewSendParams {
        task: task.to_string(),
        all_open: selection.is_none(),
        comment_ids: selection.unwrap_or_default(),
        instructions: (!instructions.is_empty()).then_some(instructions),
        ..Default::default()
    }
}

/// The comment a saved draft adds: lines `line..=end_line` of `path` on
/// `side`, quoting the code the reviewer saw (podd would otherwise read it
/// at add time, docs/11 §1).
pub(super) fn new_comment(
    sources: &[SourceFile],
    path: String,
    side: Side,
    line: u32,
    end_line: u32,
    body: String,
) -> NewComment {
    let code = sources
        .iter()
        .find(|f| f.path == path)
        .map(|f| {
            f.lines(side == Side::New, line as usize, end_line as usize)
                .trim_start_matches('\u{feff}')
                .to_string()
        })
        .filter(|c| !c.is_empty());
    NewComment {
        file: path,
        side,
        line,
        end_line: (end_line != line).then_some(end_line),
        body,
        code,
    }
}

/// Comments that can still be sent ("Send N open", the tab's 💬 badge).
pub(super) fn open_count(comments: &[ReviewComment]) -> usize {
    comments
        .iter()
        .filter(|c| c.state == CommentState::Open)
        .count()
}

/// The file after (`forward`) or before `current` of `n`, wrapping around;
/// `None` when there are no files.
pub(super) fn stepped_file(current: usize, n: usize, forward: bool) -> Option<usize> {
    if n == 0 {
        return None;
    }
    Some(if forward {
        (current + 1) % n
    } else {
        (current + n - 1) % n
    })
}

/// Toolbar totals: lines added and deleted over every file.
pub(super) fn line_totals(files: &[FileRow]) -> (usize, usize) {
    files
        .iter()
        .fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions))
}

/// File list badges: comments per file that are not resolved.
pub(super) fn notes_per_file(comments: &[ReviewComment]) -> HashMap<String, usize> {
    let mut per_file: HashMap<String, usize> = HashMap::new();
    for c in comments
        .iter()
        .filter(|c| c.state != CommentState::Resolved)
    {
        *per_file.entry(c.file.clone()).or_default() += 1;
    }
    per_file
}

/// The comment column below its tabs: the cards, the counts, the Send
/// button and the agent line.
pub(super) struct SidebarPlan {
    pub(super) visible: Vec<ReviewComment>,
    pub(super) resolved: usize,
    pub(super) open: usize,
    pub(super) total: usize,
    pub(super) send_label: String,
    pub(super) can_send: bool,
    pub(super) agent: String,
    /// The last send was refused: why, under the send box.
    pub(super) refusal: Option<String>,
}

/// What the comment column shows for these comments, checks and agent.
pub(super) fn sidebar_plan(
    comments: &[ReviewComment],
    checked: &HashSet<String>,
    show_resolved: bool,
    sending: bool,
    status: Option<&StatusVm>,
    terminal: Option<&str>,
    outcome: Option<&SendOutcome>,
) -> SidebarPlan {
    let visible: Vec<ReviewComment> = comments
        .iter()
        .filter(|c| show_resolved || c.state != CommentState::Resolved)
        .cloned()
        .collect();
    let resolved = comments
        .iter()
        .filter(|c| c.state == CommentState::Resolved)
        .count();
    let selection = send_selection(comments, checked);
    let open = open_count(comments);
    let send_label = match (&selection, open) {
        (Some(ids), _) => crate::tr!(SEND_SELECTED_TO_AGENT; ; ids.len()),
        (None, n) => crate::tr!(SEND_N_OPEN_TO_AGENT; n;),
    };
    let can_send = !sending && (selection.is_some() || open > 0);
    let agent = match status {
        Some(s) => {
            let hint = match s.state {
                AgentState::Working => crate::strings::WILL_QUEUE_UNTIL_IDLE,
                AgentState::Blocked => crate::strings::SEND_WILL_BE_REFUSED,
                _ => crate::strings::DELIVERS_NOW,
            };
            crate::tr!(AGENT_HINT; hint; terminal.unwrap_or("?"), s.label())
        }
        None => crate::tr!(AGENT_189; ; terminal.unwrap_or("—")),
    };
    let refusal = match outcome {
        Some(SendOutcome::Refused(t)) => Some(t.clone()),
        _ => None,
    };
    SidebarPlan {
        visible,
        resolved,
        open,
        total: comments.len(),
        send_label,
        can_send,
        agent,
        refusal,
    }
}

/// One delivery line under the send box.
pub(super) struct DeliveryRow {
    pub(super) id: String,
    pub(super) terminal: String,
    pub(super) text: String,
    pub(super) color: u32,
    /// The submit check of a delivered prompt, when it has something to say.
    pub(super) submit: Option<(String, u32)>,
    /// "Enter 누르기": the prompt may still sit in the agent's input box.
    pub(super) unconfirmed: bool,
    /// "취소": still waiting for the agent.
    pub(super) queued: bool,
}

/// The newest four deliveries, newest first, with their submit checks.
pub(super) fn delivery_rows(
    deliveries: &[DeliveryInfo],
    submits: &HashMap<String, p::SubmitCheck>,
) -> Vec<DeliveryRow> {
    deliveries
        .iter()
        .rev()
        .take(4)
        .map(|d| {
            let (text, color) = delivery_text(d);
            let submit = submits
                .get(&d.id)
                .filter(|_| d.state == DeliveryState::Delivered);
            DeliveryRow {
                id: d.id.clone(),
                terminal: d.terminal.clone(),
                text,
                color,
                submit: submit.and_then(submit_text),
                unconfirmed: submit.is_some_and(|s| s.state == p::SubmitState::Unconfirmed),
                queued: d.state == DeliveryState::Queued,
            }
        })
        .collect()
}

/// Comments shown inline in the diff: not resolved, in a file the diff
/// lists, each below its last line.
pub(super) fn inline_comments<'a>(
    comments: &'a [ReviewComment],
    files: &[FileRow],
) -> Vec<(&'a ReviewComment, usize)> {
    let present: HashSet<&str> = files.iter().map(|f| f.path.as_ref()).collect();
    comments
        .iter()
        .filter(|c| c.state != CommentState::Resolved && present.contains(c.file.as_str()))
        .map(|c| (c, c.end_line.unwrap_or(c.line) as usize))
        .collect()
}

/// `변경:3` or `변경:3–5` in a draft's title.
pub(super) fn draft_label(side: Side, line: u32, end_line: u32) -> String {
    if line == end_line {
        format!("{}:{}", side_label(side), line)
    } else {
        format!("{}:{}–{}", side_label(side), line, end_line)
    }
}

/// "편집기에서 열기" line: the given one, else the hovered line of that
/// file, else its first change, else 1 (never 0).
pub(super) fn editor_line(
    line: Option<usize>,
    hovered: Option<usize>,
    sources: &[SourceFile],
    path: &str,
) -> usize {
    line.or(hovered)
        .or_else(|| {
            sources
                .iter()
                .find(|f| f.path == path)
                .map(first_changed_line)
        })
        .unwrap_or(1)
        .max(1)
}

/// Keyboard "Add comment" without a selection or a pointer: the file's first
/// change, on the new side unless the file has no new text.
pub(super) fn first_change_target(source: &SourceFile) -> (String, DiffSide, usize) {
    let (path, line) = (source.path.clone(), first_changed_line(source));
    let side = if source.text(true).is_empty() {
        DiffSide::Original
    } else {
        DiffSide::Modified
    };
    (path, side, line)
}

/// What, in the task summary, changes when the agent changes files.
pub(super) fn git_signature(task: &TaskVm) -> Option<String> {
    task.git.as_ref().map(|g| {
        format!(
            "{}|{}|{}",
            g.ahead,
            g.dirty,
            g.last_commit.as_deref().unwrap_or("")
        )
    })
}

/// 1-based first line (new side) where the file differs from its old version.
pub(super) fn first_changed_line(f: &SourceFile) -> usize {
    let (old, new) = (f.text(false), f.text(true));
    let n = old
        .lines()
        .zip(new.lines())
        .take_while(|(a, b)| a == b)
        .count();
    (n + 1).min(new.lines().count().max(1))
}

pub(super) fn hash(s: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
