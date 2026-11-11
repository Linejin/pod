//! P4 backend: review comments on a task's diff, sent to the task's agent.
//!
//! - Comments are stored per task in the orchestration store (survive
//!   daemon restarts). The quoted code is captured when the comment is added
//!   (worktree file for `new`, merge base for `old`), so the prompt shows what
//!   the reviewer saw even if the agent has edited the file since.
//! - `review.send` formats the selected comments into one prompt and types it
//!   into the agent terminal as a bracketed paste followed by Enter once the
//!   program has taken the paste in (`submit.rs`), so multi-line text
//!   arrives as one message in Claude Code and Codex. The delivery records
//!   whether the agent then started a turn (`submit`, docs/12 B1); if not,
//!   Enter is pressed once more. It delivers at once when the agent is idle/done/unknown, queues
//!   while it is working, and refuses while it is blocked on the user.
//! - A worker delivers queued prompts on the next idle/done, one per terminal
//!   at a time: after a delivery the next one waits until the agent's turn
//!   counter advances (it worked on the prompt and stopped). Programs whose
//!   status comes only from heuristics (or not at all: shells) may never
//!   advance it, so for them the previous prompt also counts as handled once
//!   the status changed after the delivery, or once the terminal has been
//!   quiet for `QUIET_MS`; `INFLIGHT_FALLBACK_MS` is the last resort.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast;

use super::orch_store::{DeliveryOrigin, DeliveryRecord, Store};
use super::status;
use super::submit;
use super::terminal::Terminal;
use super::{Daemon, log};
use crate::git;
use crate::protocol::*;

/// Earliest Enter after the paste (`submit::MIN_ENTER_DELAY_MS`); the real
/// pause waits for the program to draw the paste and go quiet.
const ENTER_DELAY_MS: u64 = submit::MIN_ENTER_DELAY_MS;
/// After a delivery to a terminal whose turn counter never advances, the next
/// queued delivery goes out after this long anyway.
const INFLIGHT_FALLBACK_MS: u64 = 30_000;
/// Terminals that do not report turns (status source not `hook`/`osc`): the
/// previous prompt is handled once the output has been quiet this long...
const QUIET_MS: u64 = 1_500;
/// ...but never sooner than this after it was typed (paste + Enter + echo).
const SETTLE_MS: u64 = ENTER_DELAY_MS + 850;
/// Queued deliveries older than this expire (`podd --review-queue-timeout`).
pub const DEFAULT_QUEUE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Longest quoted range per comment.
const MAX_CODE_LINES: usize = 40;
const MAX_BODY: usize = 16 * 1024;
const MAX_COMMENTS_PER_ADD: usize = 500;

const PASTE_START: &[u8] = b"\x1b[200~";
const PASTE_END: &[u8] = b"\x1b[201~";

fn not_found(m: String) -> RpcError {
    RpcError::new(orch_codes::REVIEW_NOT_FOUND, m)
}

fn invalid(m: impl Into<String>) -> RpcError {
    RpcError::new(codes::INVALID_PARAMS, m)
}

/// `/`-separated, relative, no `..`.
pub fn normalize_rel_path(p: &str) -> Result<String, RpcError> {
    let s = p.trim().replace('\\', "/");
    let s = s.trim_start_matches("./").to_string();
    if s.is_empty()
        || s.starts_with('/')
        || s.contains(':')
        || s.split('/').any(|c| c == ".." || c.is_empty())
    {
        return Err(invalid(format!(
            "file {p:?} must be a relative path inside the worktree"
        )));
    }
    Ok(s)
}

/// Remove control characters that would end the bracketed paste early or
/// drive the terminal (ESC, other C0 except tab/newline, DEL, C1).
pub fn sanitize(s: &str) -> String {
    s.replace("\r\n", "\n")
        .chars()
        .filter(|&c| c == '\n' || c == '\t' || !(c.is_control()))
        .collect()
}

/// Lines `first..=last` (1-based) of `text`, or `None` when out of range.
pub fn extract_lines(text: &str, first: u32, last: u32) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let n = lines.len() - usize::from(text.ends_with('\n'));
    let (a, b) = (first as usize, last as usize);
    if a == 0 || a > n {
        return None;
    }
    let b = b.min(n).min(a + MAX_CODE_LINES - 1);
    let mut out = lines[a - 1..b].join("\n");
    if (last as usize) > b {
        out.push_str("\n…");
    }
    Some(out)
}

/// Read the quoted code of a comment from the worktree or the merge base.
fn capture_code(
    worktree: &Path,
    base: &str,
    base_commit: &str,
    c: &NewComment,
    file: &str,
) -> Option<String> {
    let last = c.end_line.unwrap_or(c.line);
    let text = match c.side {
        Side::New => {
            let bytes = std::fs::read(worktree.join(file)).ok()?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Side::Old => {
            let mb = git::merge_base(worktree, base, base_commit);
            git::show_file(worktree, &mb, file).ok()?
        }
    };
    if text.contains('\0') {
        return None; // binary
    }
    extract_lines(&text, c.line, last)
}

/// A fence longer than any backtick run in `code`.
fn fence(code: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in code.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat((longest + 1).max(3))
}

/// The prompt typed into the agent (line breaks are `\n` here).
pub fn format_prompt(task: &str, comments: &[ReviewComment], instructions: Option<&str>) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "Code review feedback on your changes in this worktree (task {task}). \
         Please address each of the {} comment(s) below. File paths are relative \
         to the worktree root; line numbers refer to the current file unless marked \
         as the base version. When you are done, reply with a short summary per comment.\n",
        comments.len()
    ));
    if let Some(i) = instructions.map(sanitize).filter(|i| !i.trim().is_empty()) {
        out.push_str("\nInstructions from the reviewer:\n");
        out.push_str(i.trim());
        out.push('\n');
    }
    for (n, c) in comments.iter().enumerate() {
        let range = match c.end_line {
            Some(e) if e > c.line => format!("{}-{e}", c.line),
            _ => c.line.to_string(),
        };
        let side = match c.side {
            Side::New => "",
            Side::Old => " (base version, before your changes)",
        };
        out.push_str(&format!("\n{}. {}:{range}{side}\n", n + 1, c.file));
        if let Some(code) = &c.code {
            let code = sanitize(code);
            let f = fence(&code);
            out.push_str(&f);
            out.push('\n');
            for (i, l) in code.split('\n').enumerate() {
                if l == "…" {
                    out.push_str("…\n");
                } else {
                    out.push_str(&format!("{:>4} | {l}\n", c.line as usize + i));
                }
            }
            out.push_str(&f);
            out.push('\n');
        }
        out.push_str("Reviewer: ");
        out.push_str(sanitize(&c.body).trim());
        out.push('\n');
    }
    out
}

/// The bytes typed for a prompt: bracketed paste with `\r` line breaks
/// (what terminals send for a pasted newline). Enter follows separately.
/// Text of the `error` of a delivery that expired in the queue.
pub fn expired_reason(waited_ms: u64, timeout: Duration) -> String {
    format!(
        "expired: queued for {} (review queue timeout {}); comments are open again",
        human_ms(waited_ms),
        human_ms(timeout.as_millis() as u64)
    )
}

fn human_ms(ms: u64) -> String {
    match ms {
        0..=59_999 => format!("{}s", ms / 1000),
        60_000..=3_599_999 => format!("{}m", ms / 60_000),
        _ => format!("{}h{}m", ms / 3_600_000, (ms % 3_600_000) / 60_000),
    }
}

/// Whether a delivery queued at `created_ms` has expired at `now`.
pub fn queue_expired(created_ms: u64, now: u64, timeout: Option<Duration>) -> bool {
    timeout.is_some_and(|t| now.saturating_sub(created_ms) >= t.as_millis() as u64)
}

pub fn paste_bytes(prompt: &str) -> Vec<u8> {
    let body = prompt.replace("\r\n", "\n").replace('\n', "\r");
    let body = body.trim_end_matches('\r');
    let mut b = PASTE_START.to_vec();
    b.extend_from_slice(body.as_bytes());
    b.extend_from_slice(PASTE_END);
    b
}

/// The agent can take input now (not working, not waiting on the user, running).
fn deliverable(s: &StatusInfo) -> bool {
    let (state, _) = s.display_state();
    !matches!(state.as_str(), "working" | "blocked" | "exited")
}

/// What [`inflight`] needs to know about the terminal right now.
#[derive(Debug, Clone, Copy)]
struct TermNow<'a> {
    status: &'a StatusInfo,
    last_output_ms: u64,
    now: u64,
}

/// A previous delivery to this terminal has not been worked on yet.
fn inflight(store: &Store, terminal: &str, t: TermNow<'_>) -> bool {
    let Some(d) = store
        .deliveries
        .iter()
        .filter(|d| d.info.terminal == terminal && d.info.state == DeliveryState::Delivered)
        .max_by_key(|d| d.info.delivered_ms)
    else {
        return false;
    };
    let Some(turn_at) = d.turn_at_delivery else {
        return false;
    };
    // Enter not pressed yet, or not confirmed yet (`submit.rs`).
    if d.submit
        .as_ref()
        .is_some_and(|s| s.state == SubmitState::Pending)
    {
        return true;
    }
    let delivered = d
        .submit
        .as_ref()
        .and_then(|s| s.entered_ms)
        .or(d.info.delivered_ms)
        .unwrap_or(0);
    let elapsed = t.now.saturating_sub(delivered);
    if t.status.turn > turn_at || elapsed >= INFLIGHT_FALLBACK_MS {
        return false;
    }
    let reports_turns = matches!(t.status.source.as_deref(), Some("hook" | "osc"));
    if reports_turns {
        return true; // wait for the turn to end
    }
    if elapsed < SETTLE_MS {
        return true;
    }
    // The (heuristic) state changed after the prompt went in: it reacted.
    if t.status.since > delivered + ENTER_DELAY_MS {
        return false;
    }
    // Nothing reports on this terminal: done once it has been quiet.
    t.now.saturating_sub(t.last_output_ms.max(delivered)) < QUIET_MS
}

fn set_comment_states(store: &mut Store, ids: &[String], state: CommentState, delivery: &str) {
    for c in store.comments.iter_mut().filter(|c| ids.contains(&c.id)) {
        if c.state != CommentState::Resolved {
            c.state = state;
        }
        c.delivery = Some(delivery.to_string());
    }
}

/// Submit state of a delivery that was just typed in (Enter still to come).
fn submit_pending() -> SubmitCheck {
    SubmitCheck {
        state: SubmitState::Pending,
        enters: 0,
        signal: None,
        entered_ms: None,
        settled_ms: None,
    }
}

/// A prompt on its way to an agent terminal ([`Daemon::enqueue_prompt`]).
pub(super) struct PromptJob {
    pub task: String,
    pub terminal: Arc<Terminal>,
    pub prompt: String,
    pub comment_ids: Vec<String>,
    pub origin: Option<DeliveryOrigin>,
}

impl PromptJob {
    /// The delivery as it would be recorded (`id = "preview"`).
    fn preview(&self, label: String) -> DeliveryInfo {
        DeliveryInfo {
            id: "preview".into(),
            task: self.task.clone(),
            terminal: self.terminal.id.clone(),
            comment_ids: self.comment_ids.clone(),
            state: DeliveryState::Preview,
            prompt: self.prompt.clone(),
            created_ms: crate::now_ms(),
            delivered_ms: None,
            agent_state: label,
            delivered_state: None,
            app_bracketed_paste: None,
            error: None,
        }
    }
}

/// Whether the terminal takes prompts at all right now (docs/11 §3.1 rule
/// 1): exited -> `TERMINAL_EXITED`, waiting on the user -> `AGENT_BLOCKED`.
/// Returns its status and display label (working means: queue).
pub(super) fn takes_prompts(t: &Terminal) -> Result<(StatusInfo, String), RpcError> {
    if !t.is_running() {
        return Err(super::exited_err(t));
    }
    let status = t.status.info();
    let label = match status.display_state() {
        (s, Some(k)) => format!("{s}:{k}"),
        (s, None) => s,
    };
    if label.starts_with("blocked") {
        let mut e = RpcError::new(
            orch_codes::AGENT_BLOCKED,
            format!(
                "the agent in {} is waiting for you ({label}{}); answer it first, then send again",
                t.id,
                status
                    .msg
                    .as_deref()
                    .map(|m| format!(": {m}"))
                    .unwrap_or_default()
            ),
        );
        e.data = Some(serde_json::json!({"state": label, "msg": status.msg}));
        return Err(e);
    }
    Ok((status, label))
}

/// The delivery already recorded for this origin (same device + operation
/// id), or `OPERATION_CONFLICT` when that id was used for another request.
pub(super) fn replay_of(st: &Store, o: &DeliveryOrigin) -> Result<Option<DeliveryInfo>, RpcError> {
    let Some(prev) = st.deliveries.iter().find(|d| {
        d.origin
            .as_ref()
            .is_some_and(|x| x.device == o.device && x.operation_id == o.operation_id)
    }) else {
        return Ok(None);
    };
    if prev.origin.as_ref().is_some_and(|x| x.digest == o.digest) {
        Ok(Some(prev.info.clone()))
    } else {
        Err(RpcError::new(
            mobile_codes::OPERATION_CONFLICT,
            format!(
                "operation {} was already used for a different request ({})",
                o.operation_id, prev.info.id
            ),
        ))
    }
}

impl Daemon {
    /// Paste the prompt now; press Enter and confirm the submission in the
    /// background (`submit.rs`). `false`: the holder is gone.
    fn deliver_prompt(self: &Arc<Self>, t: &Arc<Terminal>, id: &str, prompt: &str) -> bool {
        let written = crate::now_ms();
        if !t.input(paste_bytes(prompt)) {
            return false;
        }
        let chars = prompt.chars().count();
        let (d, t, id) = (self.clone(), t.clone(), id.to_string());
        tokio::spawn(async move {
            let pending = |c: &SubmitCheck| d.set_submit(&id, c.clone());
            match submit::press_enter(&t, written, chars, true, pending).await {
                Some(c) => d.set_submit(&id, c),
                None => {
                    let _ = d.fail_delivery(&id, "the terminal's holder is gone");
                }
            }
        });
        true
    }

    /// Record (and announce) the submit state of a delivery.
    fn set_submit(&self, id: &str, check: SubmitCheck) {
        let mut st = self.orch.lock().unwrap();
        let Some(r) = st.deliveries.iter_mut().find(|r| r.info.id == id) else {
            return;
        };
        r.submit = Some(check.clone());
        let info = r.info.clone();
        if let Err(e) = self.save_orch(&mut st) {
            log(&e.message);
        }
        drop(st);
        match check.state {
            SubmitState::Confirmed => log(&format!(
                "review: {id} submitted ({}, Enter x{})",
                check.signal.as_deref().unwrap_or("?"),
                check.enters
            )),
            SubmitState::Unconfirmed => log(&format!(
                "review: {id} typed in but not confirmed after Enter x{}; \
                 the prompt may still be in the agent's input box",
                check.enters
            )),
            _ => {}
        }
        super::status::publish_delivery(info, Some(check));
    }

    pub(super) fn save_orch(&self, store: &mut Store) -> Result<(), RpcError> {
        store.save(&self.config.state_dir)
    }

    /// Task id of a reference, falling back to the raw string when no task
    /// matches but comments/deliveries still name it (removed task).
    fn review_task_id(&self, s: &str) -> Result<String, RpcError> {
        match self.task_record(s) {
            Ok(r) => Ok(r.id),
            Err(e) => {
                let st = self.orch.lock().unwrap();
                if st.comments.iter().any(|c| c.task == s)
                    || st.deliveries.iter().any(|d| d.info.task == s)
                {
                    Ok(s.to_string())
                } else {
                    Err(e)
                }
            }
        }
    }

    pub(super) async fn review_add(
        &self,
        p: ReviewAddParams,
    ) -> Result<ReviewCommentsResult, RpcError> {
        let rec = self.task_record(&p.task)?;
        if p.comments.is_empty() {
            return Err(invalid("comments must not be empty"));
        }
        if p.comments.len() > MAX_COMMENTS_PER_ADD {
            return Err(invalid(format!(
                "at most {MAX_COMMENTS_PER_ADD} comments per call"
            )));
        }
        let mut files = Vec::with_capacity(p.comments.len());
        for c in &p.comments {
            let file = normalize_rel_path(&c.file)?;
            if c.line == 0 {
                return Err(invalid(format!("{file}: line numbers start at 1")));
            }
            if c.end_line.is_some_and(|e| e < c.line) {
                return Err(invalid(format!("{file}: end_line is before line")));
            }
            if c.body.trim().is_empty() {
                return Err(invalid(format!("{file}:{}: empty comment body", c.line)));
            }
            if c.body.len() > MAX_BODY {
                return Err(invalid(format!("{file}:{}: comment body too long", c.line)));
            }
            files.push(file);
        }
        // Capture quoted code (git / file reads) off the runtime threads.
        let wt = PathBuf::from(&rec.worktree);
        let (base, commit) = (rec.base.clone(), rec.base_commit.clone());
        let input = p.comments.clone();
        let fs = files.clone();
        let codes: Vec<Option<String>> = tokio::task::spawn_blocking(move || {
            input
                .iter()
                .zip(&fs)
                .map(|(c, f)| match &c.code {
                    Some(code) => Some(code.trim_end_matches(['\r', '\n']).replace("\r\n", "\n")),
                    None if wt.is_dir() => capture_code(&wt, &base, &commit, c, f),
                    None => None,
                })
                .collect()
        })
        .await
        .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("worker failed: {e}")))?;

        let now = crate::now_ms();
        let mut st = self.orch.lock().unwrap();
        let mut added = Vec::new();
        for ((c, file), code) in p.comments.into_iter().zip(files).zip(codes) {
            let rc = ReviewComment {
                id: st.next_id("c"),
                task: rec.id.clone(),
                file,
                side: c.side,
                line: c.line,
                end_line: c.end_line.filter(|e| *e > c.line),
                body: c.body,
                code,
                state: CommentState::Open,
                created_ms: now,
                delivery: None,
                resolved_ms: None,
                note: None,
            };
            st.comments.push(rc.clone());
            added.push(rc);
        }
        self.save_orch(&mut st)?;
        log(&format!(
            "review: {} comment(s) added to {}",
            added.len(),
            rec.id
        ));
        Ok(ReviewCommentsResult { comments: added })
    }

    pub(super) fn review_list(
        &self,
        p: ReviewListParams,
    ) -> Result<ReviewCommentsResult, RpcError> {
        let task = p
            .task
            .as_deref()
            .map(|t| self.review_task_id(t))
            .transpose()?;
        let st = self.orch.lock().unwrap();
        Ok(ReviewCommentsResult {
            comments: st
                .comments
                .iter()
                .filter(|c| task.as_ref().is_none_or(|t| &c.task == t))
                .filter(|c| p.state.is_none_or(|s| c.state == s))
                .cloned()
                .collect(),
        })
    }

    fn check_comment_ids(st: &Store, ids: &[String]) -> Result<(), RpcError> {
        if ids.is_empty() {
            return Err(invalid("ids must not be empty"));
        }
        if let Some(bad) = ids
            .iter()
            .find(|id| !st.comments.iter().any(|c| &c.id == *id))
        {
            return Err(not_found(format!("no comment {bad:?}")));
        }
        Ok(())
    }

    pub(super) fn review_resolve(
        &self,
        p: ReviewResolveParams,
    ) -> Result<ReviewCommentsResult, RpcError> {
        let mut st = self.orch.lock().unwrap();
        Self::check_comment_ids(&st, &p.ids)?;
        let now = crate::now_ms();
        let mut out = Vec::new();
        for c in st.comments.iter_mut().filter(|c| p.ids.contains(&c.id)) {
            if p.reopen {
                c.state = CommentState::Open;
                c.resolved_ms = None;
            } else {
                c.state = CommentState::Resolved;
                c.resolved_ms = Some(now);
            }
            if p.note.is_some() {
                c.note = p.note.clone();
            }
            out.push(c.clone());
        }
        self.save_orch(&mut st)?;
        Ok(ReviewCommentsResult { comments: out })
    }

    pub(super) fn review_remove(
        &self,
        p: ReviewIdsParams,
    ) -> Result<ReviewCommentsResult, RpcError> {
        let mut st = self.orch.lock().unwrap();
        Self::check_comment_ids(&st, &p.ids)?;
        let removed: Vec<ReviewComment> = st
            .comments
            .iter()
            .filter(|c| p.ids.contains(&c.id))
            .cloned()
            .collect();
        st.comments.retain(|c| !p.ids.contains(&c.id));
        self.save_orch(&mut st)?;
        Ok(ReviewCommentsResult { comments: removed })
    }

    pub(super) fn review_deliveries(
        &self,
        p: ReviewDeliveriesParams,
    ) -> Result<ReviewDeliveriesResult, RpcError> {
        let task = p
            .task
            .as_deref()
            .map(|t| self.review_task_id(t))
            .transpose()?;
        let st = self.orch.lock().unwrap();
        let mine = || {
            st.deliveries
                .iter()
                .filter(|d| task.as_ref().is_none_or(|t| &d.info.task == t))
        };
        Ok(ReviewDeliveriesResult {
            deliveries: mine().map(|d| d.info.clone()).collect(),
            submits: mine()
                .filter_map(|d| Some((d.info.id.clone(), d.submit.clone()?)))
                .collect(),
        })
    }

    pub(super) fn review_cancel(&self, p: DeliveryRef) -> Result<DeliveryInfo, RpcError> {
        let mut st = self.orch.lock().unwrap();
        let Some(i) = st.deliveries.iter().position(|d| d.info.id == p.id) else {
            return Err(not_found(format!("no delivery {:?}", p.id)));
        };
        if st.deliveries[i].info.state != DeliveryState::Queued {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!("delivery {} is not queued", p.id),
            ));
        }
        st.deliveries[i].info.state = DeliveryState::Cancelled;
        let ids = st.deliveries[i].info.comment_ids.clone();
        set_comment_states(&mut st, &ids, CommentState::Open, &p.id);
        let info = st.deliveries[i].info.clone();
        self.save_orch(&mut st)?;
        drop(st);
        super::status::publish_delivery(info.clone(), None);
        Ok(info)
    }

    pub(super) async fn review_send(
        self: &Arc<Self>,
        p: ReviewSendParams,
    ) -> Result<DeliveryInfo, RpcError> {
        let rec = self.task_record(&p.task)?;
        // Select comments: explicit ids (any state but resolved) + all open ones.
        let comments: Vec<ReviewComment> = {
            let st = self.orch.lock().unwrap();
            for id in &p.comment_ids {
                match st.comments.iter().find(|c| &c.id == id) {
                    Some(c) if c.task != rec.id => {
                        return Err(not_found(format!(
                            "comment {id} belongs to task {}, not {}",
                            c.task, rec.id
                        )));
                    }
                    Some(c) if c.state == CommentState::Resolved => {
                        return Err(RpcError::new(
                            orch_codes::INVALID_STATE,
                            format!("comment {id} is resolved (reopen it first)"),
                        ));
                    }
                    Some(_) => {}
                    None => return Err(not_found(format!("no comment {id:?}"))),
                }
            }
            let mut v: Vec<ReviewComment> = st
                .comments
                .iter()
                .filter(|c| c.task == rec.id)
                .filter(|c| {
                    p.comment_ids.contains(&c.id) || (p.all_open && c.state == CommentState::Open)
                })
                .cloned()
                .collect();
            v.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
            v
        };
        if comments.is_empty() {
            return Err(RpcError::new(
                orch_codes::INVALID_STATE,
                format!(
                    "nothing to send for {} (pass comment_ids or all_open with open comments)",
                    rec.id
                ),
            ));
        }
        let tid = match &p.terminal {
            Some(t) => t.clone(),
            None => rec.terminals.first().cloned().ok_or_else(|| {
                RpcError::new(
                    codes::TERMINAL_NOT_FOUND,
                    format!("task {} has no terminal", rec.id),
                )
            })?,
        };
        let t = self.resolve(&TerminalRef::parse(&tid))?;
        let (status, label) = takes_prompts(&t)?;
        let prompt = format_prompt(&rec.id, &comments, p.instructions.as_deref());
        let ids: Vec<String> = comments.iter().map(|c| c.id.clone()).collect();
        let job = PromptJob {
            task: rec.id.clone(),
            terminal: t,
            prompt,
            comment_ids: ids,
            origin: None,
        };
        if p.dry_run {
            return Ok(job.preview(label));
        }
        self.enqueue_prompt(job, &status, label)
            .map(|(info, _)| info)
    }

    /// Record a prompt for the agent and deliver it now when it can take
    /// it, else queue it (docs/11 §3.1). Shared by `review.send` and mobile
    /// messages (docs/17 M1). With an origin, a delivery already recorded
    /// for the same device + operation id is returned instead (`true`), and
    /// a different request under that id is refused; the check and the new
    /// record happen under the same store lock, so a retry never types twice.
    pub(super) fn enqueue_prompt(
        self: &Arc<Self>,
        job: PromptJob,
        status: &StatusInfo,
        label: String,
    ) -> Result<(DeliveryInfo, bool), RpcError> {
        let t = job.terminal.clone();
        let ids = job.comment_ids.clone();
        let origin = job.origin.clone();
        let mut info = job.preview(label);
        let now_deliver = {
            let mut st = self.orch.lock().unwrap();
            if let Some(o) = &origin
                && let Some(prev) = replay_of(&st, o)?
            {
                return Ok((prev, true));
            }
            let counters = st.counters.clone();
            let before: Vec<(String, CommentState, Option<String>)> = st
                .comments
                .iter()
                .filter(|c| ids.contains(&c.id))
                .map(|c| (c.id.clone(), c.state, c.delivery.clone()))
                .collect();
            info.id = st.next_id("d");
            let queued_before = st
                .deliveries
                .iter()
                .any(|d| d.info.terminal == t.id && d.info.state == DeliveryState::Queued);
            let now = crate::now_ms();
            let tn = TermNow {
                status,
                last_output_ms: t.last_output_ms(),
                now,
            };
            let go = deliverable(status) && !queued_before && !inflight(&st, &t.id, tn);
            if go {
                info.state = DeliveryState::Delivered;
                info.delivered_ms = Some(now);
                info.delivered_state = Some(info.agent_state.clone());
                info.app_bracketed_paste = Some(t.bracketed_paste());
                set_comment_states(&mut st, &ids, CommentState::Sent, &info.id);
            } else {
                info.state = DeliveryState::Queued;
                set_comment_states(&mut st, &ids, CommentState::Queued, &info.id);
            }
            st.deliveries.push(DeliveryRecord {
                info: info.clone(),
                turn_at_delivery: go.then_some(status.turn),
                submit: go.then(submit_pending),
                origin,
            });
            if let Err(e) = self.save_orch(&mut st) {
                // Nothing was typed yet: undo the record so memory matches disk.
                st.deliveries.pop();
                st.counters = counters;
                for (id, state, delivery) in before {
                    if let Some(c) = st.comments.iter_mut().find(|c| c.id == id) {
                        c.state = state;
                        c.delivery = delivery;
                    }
                }
                return Err(e);
            }
            go
        };
        super::status::publish_delivery(info.clone(), now_deliver.then(submit_pending));
        if now_deliver {
            log(&format!(
                "review: {} -> {} delivered ({} comment(s), agent {})",
                info.id,
                t.id,
                ids.len(),
                info.agent_state
            ));
            if !self.deliver_prompt(&t, &info.id, &info.prompt) {
                info = self.fail_delivery(&info.id, "the terminal's holder is gone")?;
            }
        } else {
            log(&format!(
                "review: {} -> {} queued (agent {})",
                info.id, t.id, info.agent_state
            ));
            self.review_wake.notify_one();
        }
        Ok((info, false))
    }

    fn fail_delivery(&self, id: &str, why: &str) -> Result<DeliveryInfo, RpcError> {
        let mut st = self.orch.lock().unwrap();
        let Some(i) = st.deliveries.iter().position(|d| d.info.id == id) else {
            return Err(not_found(format!("no delivery {id:?}")));
        };
        let d = &mut st.deliveries[i].info;
        d.state = DeliveryState::Failed;
        d.error = Some(why.to_string());
        let ids = d.comment_ids.clone();
        let info = d.clone();
        set_comment_states(&mut st, &ids, CommentState::Open, id);
        self.save_orch(&mut st)?;
        drop(st);
        log(&format!("review: {id} failed: {why}"));
        super::status::publish_delivery(info.clone(), None);
        Ok(info)
    }

    /// One pass of the queue: fail deliveries whose terminal is gone, and
    /// deliver the oldest queued prompt of each terminal that can take it.
    async fn review_pump(self: &Arc<Self>) {
        let queued: Vec<DeliveryInfo> = self
            .orch
            .lock()
            .unwrap()
            .deliveries
            .iter()
            .filter(|d| d.info.state == DeliveryState::Queued)
            .map(|d| d.info.clone())
            .collect();
        let mut done_terminals: Vec<String> = Vec::new();
        // Expire old queued prompts first (also those queued while the daemon
        // was down: `created_ms` is persisted). Fails them with an event.
        let timeout = self.config.review_queue_timeout;
        let now = crate::now_ms();
        let (expired, queued): (Vec<_>, Vec<_>) = queued
            .into_iter()
            .partition(|d| queue_expired(d.created_ms, now, timeout));
        for d in expired {
            if let Some(t) = timeout {
                let _ = self.fail_delivery(&d.id, &expired_reason(now - d.created_ms, t));
            }
        }
        for d in queued {
            if done_terminals.contains(&d.terminal) {
                continue; // one per terminal per pass, in order
            }
            done_terminals.push(d.terminal.clone());
            let t = self
                .terminals
                .lock()
                .unwrap()
                .iter()
                .find(|t| t.id == d.terminal)
                .cloned();
            let Some(t) = t else {
                let _ = self.fail_delivery(&d.id, "the terminal was removed");
                continue;
            };
            if !t.is_running() {
                let _ = self.fail_delivery(&d.id, "the terminal exited before delivery");
                continue;
            }
            let status = t.status.info();
            let (go, delivered) = {
                let mut st = self.orch.lock().unwrap();
                let now = crate::now_ms();
                let still_queued = st
                    .deliveries
                    .iter()
                    .any(|x| x.info.id == d.id && x.info.state == DeliveryState::Queued);
                let tn = TermNow {
                    status: &status,
                    last_output_ms: t.last_output_ms(),
                    now,
                };
                let go = still_queued && deliverable(&status) && !inflight(&st, &t.id, tn);
                if go {
                    let (s, k) = status.display_state();
                    let label = k.map(|k| format!("{s}:{k}")).unwrap_or(s);
                    if let Some(x) = st.deliveries.iter_mut().find(|x| x.info.id == d.id) {
                        x.info.state = DeliveryState::Delivered;
                        x.info.delivered_ms = Some(now);
                        x.info.delivered_state = Some(label);
                        x.info.app_bracketed_paste = Some(t.bracketed_paste());
                        x.turn_at_delivery = Some(status.turn);
                        x.submit = Some(submit_pending());
                    }
                    set_comment_states(&mut st, &d.comment_ids, CommentState::Sent, &d.id);
                    if let Err(e) = self.save_orch(&mut st) {
                        log(&e.message);
                    }
                }
                let delivered = if go {
                    st.deliveries
                        .iter()
                        .find(|x| x.info.id == d.id)
                        .map(|x| x.info.clone())
                } else {
                    None
                };
                (go, delivered)
            };
            if let Some(info) = delivered {
                super::status::publish_delivery(info, Some(submit_pending()));
            }
            if go {
                log(&format!(
                    "review: {} -> {} delivered from the queue (agent {}, turn {})",
                    d.id, t.id, status.state, status.turn
                ));
                if !self.deliver_prompt(&t, &d.id, &d.prompt) {
                    let _ = self.fail_delivery(&d.id, "the terminal's holder is gone");
                }
            }
        }
    }
}

/// Start the delivery worker: runs on every status change, on `review.send`
/// queueing, and every second (in-flight fallback).
pub(super) fn start(daemon: &Arc<Daemon>) {
    let daemon = daemon.clone();
    // Enter/confirmation of a delivery cut short by a daemon restart.
    {
        let mut st = daemon.orch.lock().unwrap();
        let mut changed = false;
        for r in st.deliveries.iter_mut() {
            if let Some(s) = r
                .submit
                .as_mut()
                .filter(|s| s.state == SubmitState::Pending)
            {
                s.state = SubmitState::Unconfirmed;
                s.settled_ms = Some(crate::now_ms());
                changed = true;
            }
        }
        if changed && let Err(e) = daemon.save_orch(&mut st) {
            log(&e.message);
        }
    }
    tokio::spawn(async move {
        let mut rx = status::subscribe();
        loop {
            daemon.review_pump().await;
            let has_queue = daemon
                .orch
                .lock()
                .unwrap()
                .deliveries
                .iter()
                .any(|d| d.info.state == DeliveryState::Queued);
            let tick = if has_queue { 250 } else { 5_000 };
            tokio::select! {
                ev = rx.recv() => match ev {
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        tokio::time::sleep(Duration::from_millis(tick)).await;
                    }
                },
                _ = daemon.review_wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_millis(tick)) => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comment(
        file: &str,
        line: u32,
        end: Option<u32>,
        code: Option<&str>,
        body: &str,
    ) -> ReviewComment {
        ReviewComment {
            id: "c1".into(),
            task: "app/x".into(),
            file: file.into(),
            side: Side::New,
            line,
            end_line: end,
            body: body.into(),
            code: code.map(Into::into),
            state: CommentState::Open,
            created_ms: 0,
            delivery: None,
            resolved_ms: None,
            note: None,
        }
    }

    fn status(source: Option<&str>, turn: u64, since: u64) -> StatusInfo {
        serde_json::from_value(serde_json::json!({
            "state": "idle", "source": source, "confidence": 0.5,
            "since": since, "turn": turn, "badge": "idle",
        }))
        .unwrap()
    }

    fn store_delivered_at(ms: u64, turn: u64) -> Store {
        let mut st = Store::default();
        st.deliveries.push(DeliveryRecord {
            info: DeliveryInfo {
                id: "d1".into(),
                task: "app/x".into(),
                terminal: "t1".into(),
                comment_ids: vec![],
                state: DeliveryState::Delivered,
                prompt: String::new(),
                created_ms: ms,
                delivered_ms: Some(ms),
                agent_state: "idle".into(),
                delivered_state: None,
                app_bracketed_paste: None,
                error: None,
            },
            turn_at_delivery: Some(turn),
            submit: None,
            origin: None,
        });
        st
    }

    /// docs/11 §6-3: queued prompts expire after the queue timeout.
    #[test]
    fn queue_expiry() {
        let t = Some(Duration::from_secs(60));
        assert!(!queue_expired(1_000, 60_999, t));
        assert!(queue_expired(1_000, 61_000, t));
        assert!(!queue_expired(0, u64::MAX, None));
        assert_eq!(
            expired_reason(31 * 60_000, DEFAULT_QUEUE_TIMEOUT),
            "expired: queued for 31m (review queue timeout 30m); comments are open again"
        );
        assert_eq!(human_ms(3_660_000), "1h1m");
    }

    /// docs/11 §6-2: a queued prompt for a terminal that never reports a
    /// turn used to wait 30 s; now it goes once the status moved or the
    /// output settled. Self-reporting agents still wait for their turn.
    #[test]
    fn inflight_rules() {
        let st = store_delivered_at(10_000, 3);
        let at = |status: &StatusInfo, last_output_ms, now| {
            inflight(
                &st,
                "t1",
                TermNow {
                    status,
                    last_output_ms,
                    now,
                },
            )
        };
        // No delivery to another terminal.
        assert!(!inflight(
            &st,
            "t2",
            TermNow {
                status: &status(None, 0, 0),
                last_output_ms: 0,
                now: 0
            }
        ));
        // OSC/hook agents: until the turn advances (or the 30 s fallback).
        let osc = status(Some("osc"), 3, 9_000);
        assert!(at(&osc, 0, 15_000));
        assert!(at(&osc, 0, 39_000));
        assert!(!at(&osc, 0, 40_000));
        assert!(!at(&status(Some("osc"), 4, 12_000), 0, 12_500));
        // Shell / heuristic: settle first...
        let shell = status(None, 3, 1_000);
        assert!(at(&shell, 0, 10_500));
        // ...then quiet output decides.
        assert!(at(&shell, 12_000, 12_500), "output 0.5 s ago");
        assert!(!at(&shell, 10_300, 12_000), "quiet for 1.7 s");
        // A heuristic transition after the delivery counts as the reaction.
        let heur = status(Some("heuristic"), 3, 11_000);
        assert!(!at(&heur, 11_900, 12_000));
    }

    #[test]
    fn paths() {
        assert_eq!(normalize_rel_path("src\\a.rs").unwrap(), "src/a.rs");
        assert_eq!(normalize_rel_path("./a.rs").unwrap(), "a.rs");
        for bad in ["", "/etc/x", "C:\\x", "../x", "a/../../b", "a//b"] {
            assert!(normalize_rel_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn line_extraction() {
        let t = "a\r\nb\nc\n";
        assert_eq!(extract_lines(t, 2, 3).unwrap(), "b\nc");
        assert_eq!(extract_lines(t, 3, 9).unwrap(), "c\n…");
        assert!(extract_lines(t, 4, 4).is_none());
        assert!(extract_lines(t, 0, 1).is_none());
        let long: String = (1..=100).map(|i| format!("{i}\n")).collect();
        let x = extract_lines(&long, 1, 100).unwrap();
        assert_eq!(x.lines().count(), MAX_CODE_LINES + 1);
    }

    #[test]
    fn sanitizing_strips_escapes() {
        assert_eq!(sanitize("a\x1b[201~b\r\nc\td\x07"), "a[201~b\nc\td");
    }

    #[test]
    fn prompt_shape_and_paste() {
        let cs = vec![
            comment(
                "src/a.rs",
                3,
                Some(4),
                Some("let x = 1;\nlet y = ```;"),
                "Rename x\nand y",
            ),
            comment("b.txt", 1, None, None, "typo"),
        ];
        let p = format_prompt("app/x", &cs, Some("Keep the API stable."));
        assert!(p.contains("task app/x"));
        assert!(p.contains("Instructions from the reviewer:\nKeep the API stable.\n"));
        assert!(p.contains("\n1. src/a.rs:3-4\n````\n   3 | let x = 1;\n   4 | let y = ```;\n````\nReviewer: Rename x\nand y\n"));
        assert!(p.contains("\n2. b.txt:1\nReviewer: typo\n"));
        let b = paste_bytes("one\ntwo\n");
        assert_eq!(b, b"\x1b[200~one\rtwo\x1b[201~");
    }
}
