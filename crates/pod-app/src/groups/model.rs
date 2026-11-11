//! Group policy and display plans (docs/20-refactor/app.md §2, "순수 표시
//! 정책"): the New Group dialog's agents, member names and validation, the
//! check command line, daemon error names, the members' git signature and
//! live status, how a check request, a check event and a pick reply apply,
//! and what the header, the check line, the pick banner and a member card
//! show. Inputs are plain data; nothing here touches a window or an entity,
//! sends a request or reads the clock. Colors are theme tokens, as in the
//! rest of the panel.

use std::collections::HashMap;

use pod_ui::StatusSummary;
use podd::protocol::{
    self as p, AgentKind, AgentSpec, CheckJob, CheckJobState, CheckResult, CompareMember, DiffFile,
    GroupCompareResult, GroupInfo, GroupMember, GroupPickResult, codes, orch_codes,
};

use super::copy;
use crate::orch::{summary_of, summary_short, terminal_badge, tokens};
use crate::source::{SourceError, split_command_line};
use crate::vm::{TaskVm, TerminalVm};

// ───────────────────────────── New Group dialog ─────────────────────────────

/// Agents the dialog fans out to: per kind, how many members.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentCounts {
    pub claude: usize,
    pub codex: usize,
    pub shell: usize,
    /// Custom command line and how many members run it.
    pub custom: String,
    pub custom_count: usize,
}

impl AgentCounts {
    /// `group.create` agents, in dialog order (claude, codex, shell, custom).
    pub fn specs(&self) -> Vec<AgentSpec> {
        let mut v = Vec::new();
        v.extend(std::iter::repeat_n(
            AgentSpec::Kind(AgentKind::Claude),
            self.claude,
        ));
        v.extend(std::iter::repeat_n(
            AgentSpec::Kind(AgentKind::Codex),
            self.codex,
        ));
        v.extend(std::iter::repeat_n(
            AgentSpec::Kind(AgentKind::Shell),
            self.shell,
        ));
        let argv = split_command_line(&self.custom);
        if !argv.is_empty() {
            v.extend(std::iter::repeat_n(
                AgentSpec::Argv { argv },
                self.custom_count,
            ));
        }
        v
    }

    /// Member task names podd will create (`<name>-<agent>-<n>`, docs/11 §3.2).
    pub fn member_names(&self, name: &str) -> Vec<String> {
        let mut n: HashMap<&str, usize> = HashMap::new();
        self.specs()
            .iter()
            .map(|s| {
                let kind = match s {
                    AgentSpec::Kind(AgentKind::Claude) => "claude",
                    AgentSpec::Kind(AgentKind::Codex) => "codex",
                    AgentSpec::Kind(AgentKind::Shell) => "shell",
                    AgentSpec::Argv { .. } => "argv",
                };
                let i = n.entry(kind).or_default();
                *i += 1;
                format!("{name}-{kind}-{i}")
            })
            .collect()
    }
}

/// The check command line as argv. Run through `cmd /c` so `.cmd` shims
/// (npm, pnpm) and shell syntax work (podd resolves argv[0] to an `.exe`).
pub fn check_argv(line: &str) -> Option<Vec<String>> {
    let line = line.trim();
    (!line.is_empty()).then(|| vec!["cmd".into(), "/c".into(), line.into()])
}

/// The command line shown for a stored check argv (inverse of [`check_argv`]).
pub fn check_line(argv: &[String]) -> String {
    match argv {
        [c, flag, rest @ ..]
            if !rest.is_empty()
                && c.eq_ignore_ascii_case("cmd")
                && flag.eq_ignore_ascii_case("/c") =>
        {
            rest.join(" ")
        }
        _ => argv.join(" "),
    }
}

/// Dialog validation: the reason the group cannot be created yet.
pub fn validate(
    name: &str,
    prompt: &str,
    counts: &AgentCounts,
    has_repo: bool,
) -> Option<&'static str> {
    let total = counts.specs().len();
    if !has_repo {
        Some(crate::strings::CHOOSE_A_REPOSITORY_OR_ENTER_THE_PATH_OF_ONE_TO_REGISTER)
    } else if name.trim().is_empty() {
        Some(crate::strings::ENTER_A_GROUP_NAME)
    } else if !name
        .trim()
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        Some(crate::strings::NAMES_USE_LETTERS_DIGITS_AND_ONLY)
    } else if prompt.trim().is_empty() {
        Some(crate::strings::ENTER_THE_PROMPT_EVERY_AGENT_GETS)
    } else if total == 0 {
        Some(crate::strings::ADD_AT_LEAST_ONE_AGENT)
    } else if total > 16 {
        Some(crate::strings::AT_MOST_16_MEMBERS_PER_GROUP)
    } else if counts.custom_count > 0 && split_command_line(&counts.custom).is_empty() {
        Some(crate::strings::ENTER_THE_CUSTOM_AGENT_S_COMMAND_LINE)
    } else {
        None
    }
}

/// The dialog's member preview line for the typed group name (empty: a placeholder name).
pub(super) fn member_preview(counts: &AgentCounts, group_name: &str) -> String {
    let members = counts.member_names(if group_name.is_empty() {
        copy::NAME_PLACEHOLDER
    } else {
        group_name
    });
    if members.is_empty() {
        crate::strings::NO_MEMBERS_YET.to_string()
    } else {
        crate::tr!(MEMBER_TASK_S_ONE_WORKTREE_EACH; ; members.len(), members.join(", "))
    }
}

/// Daemon error with its name for the codes the orchestration UI cares about.
pub fn error_text(e: &SourceError) -> String {
    let name = match e.code {
        Some(orch_codes::GROUP_EXISTS) => "GROUP_EXISTS: ",
        Some(orch_codes::GROUP_NOT_FOUND) => "GROUP_NOT_FOUND: ",
        Some(orch_codes::INVALID_STATE) => "INVALID_STATE: ",
        Some(codes::TASK_DIRTY) => "TASK_DIRTY: ",
        _ => "",
    };
    format!("{name}{}", e.message)
}

// ───────────────────────────── Group panel ─────────────────────────────

/// Outcome of the last `group.pick` for the banner.
#[derive(Clone, Debug)]
pub(super) enum PickOutcome {
    Done(GroupPickResult),
    Failed(String),
}

/// Member task ids (in group order).
pub(super) fn member_ids(info: Option<&GroupInfo>) -> Vec<String> {
    info.map(|g| g.members.iter().map(|m| m.task.clone()).collect())
        .unwrap_or_default()
}

/// Git summary of every member (ahead/behind/dirty/last commit) from the
/// workspace's task poll.
pub fn git_signature(members: &[String], tasks: &[TaskVm]) -> String {
    members
        .iter()
        .map(|id| match tasks.iter().find(|t| &t.id == id) {
            Some(t) => format!(
                "{id}:{:?}:{}",
                t.git_summary(),
                t.git
                    .as_ref()
                    .and_then(|g| g.last_commit.clone())
                    .unwrap_or_default()
            ),
            None => format!("{id}:-"),
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// Live data of the members from the workspace, gathered before rendering.
pub(super) struct MemberLive {
    pub(super) tasks: HashMap<String, TaskVm>,
    pub(super) badges: HashMap<String, pod_ui::StatusBadge>,
    pub(super) summary: StatusSummary,
}

impl MemberLive {
    /// `ws`: the workspace's tasks and terminals (`None` when it is gone).
    pub(super) fn gather(members: &[GroupMember], ws: Option<(&[TaskVm], &[TerminalVm])>) -> Self {
        let mut tasks = HashMap::new();
        let mut badges = HashMap::new();
        let mut all: Vec<String> = Vec::new();
        if let Some((ws_tasks, ws_terminals)) = ws {
            for m in members.iter().filter(|m| !m.removed) {
                // The workspace's task poll (3 s) may not know a new member yet:
                // fall back to the terminals in the group record.
                let terms: Vec<String> = match ws_tasks.iter().find(|t| t.id == m.task) {
                    Some(t) => {
                        tasks.insert(m.task.clone(), t.clone());
                        t.terminals.clone()
                    }
                    None => m
                        .info
                        .as_ref()
                        .map(|i| i.terminals.iter().map(|x| x.id.clone()).collect())
                        .unwrap_or_default(),
                };
                // The agent terminal (first) stands for the member; others count in the summary.
                if let Some(term) = terms
                    .first()
                    .and_then(|x| ws_terminals.iter().find(|t| &t.id == x))
                {
                    badges.insert(m.task.clone(), terminal_badge(term));
                }
                all.extend(terms);
            }
        }
        let terminals: &[TerminalVm] = ws.map(|(_, t)| t).unwrap_or(&[]);
        Self {
            summary: summary_of(all.iter(), terminals),
            tasks,
            badges,
        }
    }
}

/// The check command to show when a re-read group's stored command changed.
pub(super) fn changed_check_text(old: Option<&GroupInfo>, new: &GroupInfo) -> Option<String> {
    let had_check = old.and_then(|g| g.check.clone());
    if new.check != had_check
        && let Some(c) = &new.check
    {
        Some(check_line(c))
    } else {
        None
    }
}

/// The group's check job is running.
pub(super) fn check_running(info: &GroupInfo) -> bool {
    info.check_job
        .as_ref()
        .is_some_and(|j| j.state == CheckJobState::Running)
}

/// `group.check` for the check field: the edited command is stored, the
/// stored one kept when the field is empty or unchanged. `None`: there is no
/// command at all.
pub(super) fn check_params(
    group: &str,
    line: &str,
    stored: Option<&Vec<String>>,
) -> Option<p::GroupCheckParams> {
    let check = check_argv(line);
    if check.is_none() && stored.is_none() {
        return None;
    }
    Some(p::GroupCheckParams {
        group: group.to_string(),
        // Store the edited command; keep the stored one when the field is empty.
        check: check.filter(|c| Some(c) != stored),
        ..Default::default()
    })
}

/// A `group.check` event: the job, and the result of the member it is about,
/// in the group record and the comparison.
pub(super) fn apply_check_event(
    info: Option<&mut GroupInfo>,
    compare: Option<&mut GroupCompareResult>,
    e: &p::GroupCheckEvent,
) {
    if let Some(info) = info {
        info.check_job = Some(e.job.clone());
        if let Some(m) = &e.member
            && let Some(rec) = info.members.iter_mut().find(|x| x.task == m.task)
        {
            rec.check = Some(m.result.clone());
        }
    }
    if let (Some(m), Some(c)) = (&e.member, compare)
        && let Some(rec) = c.members.iter_mut().find(|x| x.task == m.task)
    {
        rec.check = Some(m.result.clone());
    }
}

/// The event ends the job (its own event, without a member): re-read the group.
pub(super) fn check_job_ended(e: &p::GroupCheckEvent) -> bool {
    e.member.is_none() && e.job.state != CheckJobState::Running
}

/// Terminals of the members a pick removed (their panels close).
pub(super) fn removed_terminals(res: &GroupPickResult) -> Vec<String> {
    res.removed
        .iter()
        .flat_map(|x| x.terminals.clone())
        .collect()
}

/// Body of the "winner" toast: which members went away.
pub(super) fn pick_toast_body(res: &GroupPickResult) -> String {
    let removed: Vec<&str> = res.removed.iter().map(|x| x.id.as_str()).collect();
    if removed.is_empty() {
        crate::strings::NO_MEMBERS_REMOVED.to_string()
    } else {
        crate::tr!(REMOVED; ; removed.join(", "))
    }
}

/// The members a pick may remove: every other member still there.
pub(super) fn pick_others(info: Option<&GroupInfo>, task: &str) -> Vec<String> {
    info.map(|g| {
        g.members
            .iter()
            .filter(|m| m.task != task && !m.removed)
            .map(|m| m.task.clone())
            .collect()
    })
    .unwrap_or_default()
}

/// The panel header: title and subtitle of its frame, the winner, the prompt line.
pub(super) struct HeaderPlan {
    pub(super) title: String,
    pub(super) subtitle: String,
    pub(super) winner: Option<String>,
    pub(super) prompt: String,
    /// The check job is running (Run check disabled, Cancel check shown).
    pub(super) running: bool,
}

pub(super) fn header_plan(
    group_id: &str,
    info: Option<&GroupInfo>,
    summary: &StatusSummary,
) -> HeaderPlan {
    let job = info.and_then(|g| g.check_job.clone());
    let running = job
        .as_ref()
        .is_some_and(|j| j.state == CheckJobState::Running);
    let base = info
        .map(|g| crate::tr!(BASE_253; ; g.base, &g.base_commit[..g.base_commit.len().min(9)]))
        .unwrap_or_default();
    let members = info
        .map(|g| g.members.iter().filter(|m| !m.removed).count())
        .unwrap_or(0);
    HeaderPlan {
        title: crate::tr!(GROUP; ; group_id),
        subtitle: crate::tr!(BASE_MEMBERS_MEMBER_S; base,members; summary_short(summary)),
        winner: info.and_then(|g| g.picked.clone()),
        prompt: crate::tr!(PROMPT_257; ; info.map(|g| g.prompt.clone()).unwrap_or_default()),
        running,
    }
}

/// The check line under the header: the last request error, else the job's
/// progress or outcome with passed/failed counts, and its color.
pub(super) fn job_line(
    check_error: Option<&String>,
    info: Option<&GroupInfo>,
    job: Option<&CheckJob>,
) -> (String, u32) {
    let t = tokens();
    match job {
        _ if check_error.is_some() => (check_error.cloned().unwrap_or_default(), t.danger_fg),
        None => (crate::strings::NO_CHECK_RUN_YET.to_string(), t.text_muted),
        Some(j) => {
            let results: Vec<(String, Option<bool>)> = info
                .iter()
                .flat_map(|g| g.members.iter())
                .filter(|m| j.tasks.contains(&m.task))
                .map(|m| {
                    (
                        m.task.clone(),
                        m.check
                            .as_ref()
                            .filter(|_| j.done.contains(&m.task))
                            .map(|c| c.passed),
                    )
                })
                .collect();
            let passed = results.iter().filter(|r| r.1 == Some(true)).count();
            let failed = results.iter().filter(|r| r.1 == Some(false)).count();
            let line = check_line(&j.argv);
            match j.state {
                CheckJobState::Running => (
                    crate::tr!(CHECK_PROGRESS_REPORT; line,passed,failed; j.id, j.done.len(), j.tasks.len()),
                    t.info_fg,
                ),
                CheckJobState::Finished if j.all_passed == Some(true) => (
                    crate::tr!(CHECK_PASSED_REPORT; line,passed; j.id),
                    t.success_fg,
                ),
                CheckJobState::Finished => (
                    crate::tr!(CHECK_FAILED_REPORT; line,failed,passed; j.id),
                    t.danger_fg,
                ),
                CheckJobState::Cancelled => {
                    (crate::tr!(CHECK_CANCELLED_REPORT; line; j.id), t.warning_fg)
                }
                CheckJobState::Interrupted => (
                    crate::tr!(CHECK_INTERRUPTED_REPORT; line; j.id),
                    t.warning_fg,
                ),
            }
        }
    }
}

/// The banner under the check line after a pick.
#[derive(Debug, PartialEq)]
pub(super) enum PickBanner {
    None,
    Refused(String),
    /// The result line, then one line per member podd kept.
    Done {
        line: String,
        kept: Vec<String>,
    },
}

pub(super) fn pick_banner(pick: Option<&PickOutcome>) -> PickBanner {
    match pick {
        None => PickBanner::None,
        Some(PickOutcome::Failed(m)) => PickBanner::Refused(crate::tr!(PICK_REFUSED_M; m;)),
        Some(PickOutcome::Done(r)) => PickBanner::Done {
            line: crate::tr!(PICK_RESULT; ;
                r.picked,
                if r.removed.is_empty() {
                    String::new()
                } else {
                    crate::tr!(REMOVED_269; ; r.removed.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", "))
                }
            ),
            kept: r
                .kept
                .iter()
                .map(|k| crate::tr!(KEPT; ; k.task, k.message))
                .collect(),
        },
    }
}

/// What a member card shows besides the comparison's numbers.
pub(super) struct CardPlan<'a> {
    /// Task name without the repo.
    pub(super) short: String,
    pub(super) picked: bool,
    /// The running check job has not reached this member yet.
    pub(super) checking: bool,
    /// The comparison's check result, else the group record's.
    pub(super) check: Option<&'a CheckResult>,
    pub(super) badge: Option<pod_ui::StatusBadge>,
    pub(super) branch: String,
    pub(super) border: u32,
}

pub(super) fn card_plan<'a>(
    m: &'a GroupMember,
    info: Option<&GroupInfo>,
    cmp: Option<&'a CompareMember>,
    live: &MemberLive,
) -> CardPlan<'a> {
    let t = tokens();
    let short = m.task.rsplit('/').next().unwrap_or(&m.task).to_string();
    let task = live.tasks.get(&m.task);
    let picked = m.picked || info.and_then(|g| g.picked.as_deref()) == Some(m.task.as_str());
    let job = info.and_then(|g| g.check_job.as_ref());
    let checking = job.is_some_and(|j| {
        j.state == CheckJobState::Running && j.tasks.contains(&m.task) && !j.done.contains(&m.task)
    });
    let check = cmp.and_then(|c| c.check.as_ref()).or(m.check.as_ref());
    let badge = live.badges.get(&m.task).cloned();
    let branch = task
        .and_then(|t| t.branch.clone())
        .or_else(|| m.info.as_ref().map(|i| i.branch.clone()))
        .unwrap_or_else(|| copy::NO_BRANCH.into());
    let border = if picked {
        t.success_fg
    } else if check.is_some_and(|c| !c.passed) && !checking {
        t.danger_fg
    } else {
        t.border_subtle
    };
    CardPlan {
        short,
        picked,
        checking,
        check,
        badge,
        branch,
        border,
    }
}

/// `+3 −1` for a changed file, `bin` for a binary one.
pub(super) fn file_counts(f: &DiffFile) -> String {
    if f.binary {
        copy::BINARY.to_string()
    } else {
        format!("+{} −{}", f.added.unwrap_or(0), f.deleted.unwrap_or(0))
    }
}

/// `check passed · exit 0 · 1.2 s` and its color (the icon is drawn by the caller).
pub fn check_summary(c: &CheckResult) -> (String, u32) {
    let t = tokens();
    let secs = c.duration_ms as f64 / 1000.;
    if let Some(e) = &c.error {
        (crate::tr!(CHECK_ERROR_E; e;), t.danger_fg)
    } else if c.timed_out {
        (
            crate::tr!(CHECK_TIMED_OUT_AFTER_SECS_1_S; secs;),
            t.danger_fg,
        )
    } else if c.passed {
        (
            crate::tr!(CHECK_PASSED_EXIT_SECS_1_S; secs; c.exit_code.unwrap_or(0)),
            t.success_fg,
        )
    } else {
        (
            crate::tr!(CHECK_FAILED_EXIT_SECS_1_S; secs; c.exit_code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())),
            t.danger_fg,
        )
    }
}

/// Last `n` non-empty lines of a check's output tail.
pub fn tail_lines(s: &str, n: usize) -> Vec<String> {
    let lines: Vec<&str> = s
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| l.to_string())
        .collect()
}
