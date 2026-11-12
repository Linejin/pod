//! `pod review|group|gate` and `pod wait --gate` (docs/11).

use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Subcommand, ValueEnum};
use podd::client::Client;
use podd::endpoint;
use podd::protocol::*;

use crate::{print_json, print_rows, short_sha};

#[derive(Clone, Copy, ValueEnum)]
pub enum SideArg {
    Old,
    New,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum CommentStateArg {
    Open,
    Queued,
    Sent,
    Resolved,
}

#[derive(Subcommand)]
pub enum ReviewCmd {
    /// Add a comment on a task's diff (or many with --from-json).
    Add {
        task: String,
        /// File relative to the worktree root.
        #[arg(required_unless_present = "from_json")]
        file: Option<String>,
        /// 1-based line.
        #[arg(required_unless_present = "from_json")]
        line: Option<u32>,
        /// Last line of a range.
        #[arg(long)]
        end_line: Option<u32>,
        /// `new` = worktree version (default), `old` = base version.
        #[arg(long, value_enum, default_value = "new")]
        side: SideArg,
        #[arg(long, required_unless_present = "from_json")]
        body: Option<String>,
        /// A review UI export: `{"comments": [{path|file, side, line, end_line?, body, code?}]}`
        /// or a bare array.
        #[arg(long, value_name = "FILE", conflicts_with_all = ["file", "line", "body"])]
        from_json: Option<std::path::PathBuf>,
    },
    /// List comments.
    Ls {
        task: Option<String>,
        #[arg(long, value_enum)]
        state: Option<CommentStateArg>,
    },
    /// Send comments to the task's agent (delivered when it is idle/done,
    /// queued while it works, refused while it waits on you).
    Send {
        task: String,
        /// Comment ids (default with no ids: every open comment).
        ids: Vec<String>,
        /// Also send every open comment.
        #[arg(long)]
        all: bool,
        #[arg(long)]
        instructions: Option<String>,
        /// Type into this terminal instead of the task's agent terminal.
        #[arg(long)]
        terminal: Option<String>,
        /// Print the prompt; deliver nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Resolve (or --reopen) comments.
    Resolve {
        #[arg(required = true)]
        ids: Vec<String>,
        #[arg(long)]
        note: Option<String>,
        #[arg(long)]
        reopen: bool,
    },
    /// Delete comments.
    Rm {
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Deliveries made by `review send`.
    Deliveries { task: Option<String> },
    /// Cancel a queued delivery.
    Cancel { id: String },
}

#[derive(Subcommand)]
pub enum GroupCmd {
    /// Fan out: one task per agent from the same base and prompt.
    /// A command after `--` becomes the group's check (run by `group check`).
    New {
        repo: String,
        name: String,
        #[arg(long)]
        prompt: String,
        /// claude | codex | shell | a JSON argv array; repeatable, in order.
        #[arg(long = "agent", required = true, value_name = "AGENT")]
        agents: Vec<String>,
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        #[arg(last = true, value_name = "CHECK")]
        check: Vec<String>,
    },
    /// List groups.
    Ls {
        #[arg(long)]
        repo: Option<String>,
    },
    /// Show a group with its members' status and git facts.
    Show { group: String },
    /// Per-member changed files, +/- totals and check results.
    Compare { group: String },
    /// Run the check command in every member worktree (exit 1 if any fails).
    /// A command after `--` replaces the stored check. The daemon runs it as a
    /// background job; this waits for it unless --no-wait.
    Check {
        group: String,
        /// Per-member time limit in seconds (default 600).
        #[arg(long, value_name = "S")]
        timeout: Option<f64>,
        /// Start the job and print its id; results show up in `group show`.
        #[arg(long, conflicts_with = "cancel")]
        no_wait: bool,
        /// Cancel the running check job of the group (kills its commands).
        #[arg(long, conflicts_with_all = ["timeout", "check"])]
        cancel: bool,
        #[arg(last = true, value_name = "CHECK")]
        check: Vec<String>,
    },
    /// Mark the winner; --remove-others removes the rest (safely unless --force).
    Pick {
        group: String,
        task: String,
        #[arg(long)]
        remove_others: bool,
        #[arg(long)]
        force: bool,
        #[arg(long)]
        delete_branch: bool,
    },
    /// Forget a group (member tasks stay).
    Rm { group: String },
}

#[derive(Subcommand)]
pub enum GateCmd {
    /// Ask the human for approval. Inside a Pod terminal the gate is tied to
    /// this terminal and its task; elsewhere pass --task or --group.
    Request {
        message: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        group: Option<String>,
        /// Resolve the gate as expired after S seconds without a decision.
        #[arg(long, value_name = "S")]
        expires: Option<u64>,
        /// Block until resolved: exit 0 approved, 1 rejected, expired or timed out.
        #[arg(long)]
        wait: bool,
        #[arg(long, value_name = "S", requires = "wait")]
        timeout: Option<f64>,
    },
    /// List gates.
    Ls {
        #[arg(long)]
        task: Option<String>,
        #[arg(long)]
        group: Option<String>,
        /// Only pending gates.
        #[arg(long)]
        pending: bool,
    },
    Show {
        id: String,
    },
    Approve {
        id: String,
        #[arg(long)]
        note: Option<String>,
        /// The chosen option (gates with options).
        #[arg(long)]
        choice: Option<String>,
    },
    Reject {
        id: String,
        #[arg(long)]
        note: Option<String>,
    },
}

pub(crate) fn parse_agent(s: &str) -> anyhow::Result<AgentSpec> {
    let t = s.trim();
    if t.starts_with('[') {
        let argv: Vec<String> =
            serde_json::from_str(t).with_context(|| format!("--agent {s:?}: bad JSON argv"))?;
        return Ok(AgentSpec::Argv { argv });
    }
    Ok(AgentSpec::Kind(match t {
        "claude" => AgentKind::Claude,
        "codex" => AgentKind::Codex,
        "shell" => AgentKind::Shell,
        _ => bail!("--agent must be claude, codex, shell or a JSON argv array, not {s:?}"),
    }))
}

#[cfg(test)]
pub fn parse_agent_for_test(s: &str) -> bool {
    parse_agent(s).is_ok()
}

fn secs_to_ms(s: Option<f64>) -> Option<u64> {
    s.map(|s| (s.max(0.0) * 1000.0) as u64)
}

fn state_name(s: CommentState) -> &'static str {
    match s {
        CommentState::Open => "open",
        CommentState::Queued => "queued",
        CommentState::Sent => "sent",
        CommentState::Resolved => "resolved",
    }
}

fn delivery_state(s: DeliveryState) -> &'static str {
    match s {
        DeliveryState::Queued => "queued",
        DeliveryState::Delivered => "delivered",
        DeliveryState::Cancelled => "cancelled",
        DeliveryState::Failed => "failed",
        DeliveryState::Preview => "preview",
    }
}

fn gate_state(s: GateState) -> &'static str {
    match s {
        GateState::Pending => "pending",
        GateState::Approved => "approved",
        GateState::Rejected => "rejected",
        GateState::Expired => "expired",
    }
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or("");
    if l.chars().count() > 60 {
        format!("{}…", l.chars().take(59).collect::<String>())
    } else {
        l.to_string()
    }
}

fn print_comments(cs: &[ReviewComment]) {
    if cs.is_empty() {
        println!("no comments");
        return;
    }
    let rows = cs
        .iter()
        .map(|c| {
            let range = match c.end_line {
                Some(e) => format!("{}-{e}", c.line),
                None => c.line.to_string(),
            };
            let side = match c.side {
                Side::Old => " (old)",
                Side::New => "",
            };
            vec![
                c.id.clone(),
                c.task.clone(),
                state_name(c.state).into(),
                format!("{}:{range}{side}", c.file),
                first_line(&c.body),
            ]
        })
        .collect();
    print_rows(&["ID", "TASK", "STATE", "WHERE", "COMMENT"], rows);
}

fn print_delivery(d: &DeliveryInfo) {
    println!(
        "{} {} -> {} ({} comment(s); agent was {}){}",
        d.id,
        delivery_state(d.state),
        d.terminal,
        d.comment_ids.len(),
        d.agent_state,
        d.error
            .as_deref()
            .map(|e| format!(": {e}"))
            .unwrap_or_default()
    );
}

pub(crate) fn print_gate(g: &GateInfo) {
    println!(
        "{} {} {}{}{}{}{}",
        g.id,
        gate_state(g.state),
        g.title,
        g.task
            .as_deref()
            .or(g.group.as_deref())
            .map(|t| format!(" [{t}]"))
            .unwrap_or_default(),
        g.item
            .as_deref()
            .map(|i| format!(" [item {i}]"))
            .unwrap_or_default(),
        match (&g.choice, g.options.is_empty()) {
            (Some(c), _) => format!(" -> {c}"),
            (None, false) => format!(" options: {}", g.options.join(" | ")),
            (None, true) => String::new(),
        },
        g.note
            .as_deref()
            .map(|n| format!(" ({n})"))
            .unwrap_or_default()
    );
}

fn read_export(path: &std::path::Path) -> anyhow::Result<Vec<NewComment>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let arr = match v.get("comments") {
        Some(c) => c.clone(),
        None => v,
    };
    serde_json::from_value(arr).context("comments must be [{file|path, side, line, body}]")
}

pub fn review(cmd: ReviewCmd, c: &mut Client, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        ReviewCmd::Add {
            task,
            file,
            line,
            end_line,
            side,
            body,
            from_json,
        } => {
            let comments = match from_json {
                Some(p) => read_export(&p)?,
                None => vec![NewComment {
                    file: file.unwrap_or_default(),
                    side: match side {
                        SideArg::Old => Side::Old,
                        SideArg::New => Side::New,
                    },
                    line: line.unwrap_or(0),
                    end_line,
                    body: body.unwrap_or_default(),
                    code: None,
                }],
            };
            let r = c.call::<ReviewCommentsAdd>(&ReviewAddParams { task, comments })?;
            if json {
                print_json(&r);
            } else {
                print_comments(&r.comments);
            }
        }
        ReviewCmd::Ls { task, state } => {
            let r = c.call::<ReviewCommentsList>(&ReviewListParams {
                task,
                state: state.map(|s| match s {
                    CommentStateArg::Open => CommentState::Open,
                    CommentStateArg::Queued => CommentState::Queued,
                    CommentStateArg::Sent => CommentState::Sent,
                    CommentStateArg::Resolved => CommentState::Resolved,
                }),
            })?;
            if json {
                print_json(&r);
            } else {
                print_comments(&r.comments);
            }
        }
        ReviewCmd::Send {
            task,
            ids,
            all,
            instructions,
            terminal,
            dry_run,
        } => {
            let all_open = all || ids.is_empty();
            let d = c.call::<ReviewSend>(&ReviewSendParams {
                task,
                comment_ids: ids,
                all_open,
                instructions,
                terminal,
                dry_run,
            })?;
            if json {
                print_json(&d);
            } else if dry_run {
                print!("{}", d.prompt);
            } else {
                print_delivery(&d);
            }
        }
        ReviewCmd::Resolve { ids, note, reopen } => {
            let r = c.call::<ReviewCommentsResolve>(&ReviewResolveParams { ids, note, reopen })?;
            if json {
                print_json(&r);
            } else {
                print_comments(&r.comments);
            }
        }
        ReviewCmd::Rm { ids } => {
            let r = c.call::<ReviewCommentsRemove>(&ReviewIdsParams { ids })?;
            if json {
                print_json(&r);
            } else {
                for x in &r.comments {
                    println!("{} removed", x.id);
                }
            }
        }
        ReviewCmd::Deliveries { task } => {
            let r = c.call::<ReviewDeliveries>(&ReviewDeliveriesParams { task })?;
            if json {
                print_json(&r);
            } else if r.deliveries.is_empty() {
                println!("no deliveries");
            } else {
                for d in &r.deliveries {
                    print_delivery(d);
                }
            }
        }
        ReviewCmd::Cancel { id } => {
            let d = c.call::<ReviewCancel>(&DeliveryRef { id })?;
            if json {
                print_json(&d);
            } else {
                print_delivery(&d);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn member_status(m: &GroupMember) -> String {
    if m.removed {
        return "removed".into();
    }
    m.info
        .as_ref()
        .and_then(|i| i.status.as_ref())
        .map(|s| s.badge.clone())
        .unwrap_or_else(|| "-".into())
}

fn check_cell(c: &Option<CheckResult>) -> String {
    match c {
        None => "-".into(),
        Some(r) if r.passed => "pass".into(),
        Some(r) if r.timed_out => "timeout".into(),
        Some(r) if r.error.is_some() => "error".into(),
        Some(r) => format!(
            "FAIL ({})",
            r.exit_code.map(|c| c.to_string()).unwrap_or("?".into())
        ),
    }
}

fn print_group(g: &GroupInfo) {
    let kv = |k: &str, v: &str| println!("{k:<8} {v}");
    kv("group", &g.id);
    kv(
        "base",
        &format!("{} ({})", g.base, short_sha(&g.base_commit)),
    );
    kv("prompt", &first_line(&g.prompt));
    if let Some(c) = &g.check {
        kv("check", &c.join(" "));
    }
    if let Some(p) = &g.picked {
        kv("picked", p);
    }
    let rows = g
        .members
        .iter()
        .map(|m| {
            let git = m
                .info
                .as_ref()
                .and_then(|i| i.git.as_ref())
                .map(|g| format!("{} ahead, {} dirty", g.ahead, g.dirty))
                .unwrap_or_else(|| "-".into());
            vec![
                format!("{}{}", if m.picked { "* " } else { "  " }, m.task),
                m.agent.to_string(),
                member_status(m),
                git,
                check_cell(&m.check),
            ]
        })
        .collect();
    print_rows(&["  TASK", "AGENT", "STATUS", "GIT", "CHECK"], rows);
    for gate in &g.gates {
        print_gate(gate);
    }
}

pub fn group(cmd: GroupCmd, c: &mut Client, pipe: &str, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        GroupCmd::New {
            repo,
            name,
            prompt,
            agents,
            base,
            check,
        } => {
            let agents = agents
                .iter()
                .map(|a| parse_agent(a))
                .collect::<anyhow::Result<Vec<_>>>()?;
            let g = c.call::<GroupCreate>(&GroupCreateParams {
                repo,
                name,
                base,
                prompt,
                agents,
                check: (!check.is_empty()).then_some(check),
                parent_terminal: crate::caller_terminal(pipe),
                cols: None,
                rows: None,
            })?;
            if json {
                print_json(&g);
            } else {
                print_group(&g);
            }
        }
        GroupCmd::Ls { repo } => {
            let l = c.call::<GroupList>(&GroupListParams { repo })?;
            if json {
                print_json(&l);
            } else if l.groups.is_empty() {
                println!("no groups");
            } else {
                let rows = l
                    .groups
                    .iter()
                    .map(|g| {
                        vec![
                            g.id.clone(),
                            g.members.iter().filter(|m| !m.removed).count().to_string(),
                            g.picked.clone().unwrap_or_else(|| "-".into()),
                            g.gates
                                .iter()
                                .filter(|x| x.state == GateState::Pending)
                                .count()
                                .to_string(),
                            first_line(&g.prompt),
                        ]
                    })
                    .collect();
                print_rows(&["GROUP", "MEMBERS", "PICKED", "GATES", "PROMPT"], rows);
            }
        }
        GroupCmd::Show { group } => {
            let g = c.call::<GroupShow>(&GroupRef { group })?;
            if json {
                print_json(&g);
            } else {
                print_group(&g);
            }
        }
        GroupCmd::Compare { group } => {
            let r = c.call::<GroupCompare>(&GroupRef { group })?;
            if json {
                print_json(&r);
            } else {
                let rows = r
                    .members
                    .iter()
                    .map(|m| {
                        vec![
                            format!("{}{}", if m.picked { "* " } else { "  " }, m.task),
                            m.agent.to_string(),
                            if m.removed {
                                "removed".into()
                            } else {
                                m.status.clone().unwrap_or_else(|| "-".into())
                            },
                            m.files_changed.to_string(),
                            format!("+{}", m.added),
                            format!("-{}", m.deleted),
                            check_cell(&m.check),
                            m.error.clone().unwrap_or_default(),
                        ]
                    })
                    .collect();
                print_rows(
                    &[
                        "  TASK", "AGENT", "STATUS", "FILES", "ADDED", "DELETED", "CHECK", "",
                    ],
                    rows,
                );
            }
        }
        GroupCmd::Check {
            group,
            timeout,
            no_wait,
            cancel,
            check,
        } => {
            let r = if cancel {
                c.call::<GroupCheckCancel>(&GroupRef { group })?
            } else {
                c.call::<GroupCheck>(&GroupCheckParams {
                    group,
                    check: (!check.is_empty()).then_some(check),
                    timeout_ms: secs_to_ms(timeout),
                    tasks: Vec::new(),
                    wait: !no_wait,
                })?
            };
            let job_state = r.job.as_ref().map(|j| j.state);
            if json {
                print_json(&r);
            } else if let (Some(j), Some(CheckJobState::Running)) = (&r.job, job_state) {
                println!(
                    "check job {} running on {} member(s) of {}",
                    j.id,
                    j.tasks.len(),
                    r.group
                );
            } else {
                if let Some(j) = &r.job
                    && j.state != CheckJobState::Finished
                {
                    println!("check job {} {:?}", j.id, j.state);
                }
                for m in &r.results {
                    println!(
                        "{}  {}  {} ms",
                        m.task,
                        check_cell(&Some(m.result.clone())),
                        m.result.duration_ms
                    );
                    if !m.result.passed {
                        if let Some(e) = &m.result.error {
                            println!("    {e}");
                        }
                        for l in m
                            .result
                            .output_tail
                            .lines()
                            .rev()
                            .take(5)
                            .collect::<Vec<_>>()
                            .iter()
                            .rev()
                        {
                            println!("    | {l}");
                        }
                    }
                }
                println!(
                    "{}",
                    if r.all_passed {
                        "all passed"
                    } else {
                        "CHECK FAILED"
                    }
                );
            }
            if job_state == Some(CheckJobState::Running) && !cancel {
                return Ok(ExitCode::SUCCESS); // --no-wait
            }
            if !r.all_passed {
                return Ok(ExitCode::from(1));
            }
        }
        GroupCmd::Pick {
            group,
            task,
            remove_others,
            force,
            delete_branch,
        } => {
            let r = c.call::<GroupPick>(&GroupPickParams {
                group,
                task,
                remove_others,
                force,
                delete_branch,
            })?;
            if json {
                print_json(&r);
            } else {
                println!("{}: picked {}", r.group, r.picked);
                for x in &r.removed {
                    println!("  removed {}", x.id);
                }
                for k in &r.kept {
                    println!("  kept {}: {}", k.task, k.message);
                }
            }
            if !r.kept.is_empty() {
                return Ok(ExitCode::from(1));
            }
        }
        GroupCmd::Rm { group } => {
            let r = c.call::<GroupRemove>(&GroupRef { group })?;
            if json {
                print_json(&r);
            } else {
                println!("{} forgotten (tasks kept)", r.id);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Block on a gate: 0 approved, 1 rejected or timed out.
pub fn wait_gate(
    c: &mut Client,
    id: String,
    timeout_ms: Option<u64>,
    json: bool,
) -> anyhow::Result<ExitCode> {
    // Survives a daemon restart: the gate is persisted (docs/12 B6).
    let what = format!("gate {id}");
    let r = crate::reconnect::wait(c, timeout_ms, &what, |c, timeout_ms| {
        c.call::<GateWait>(&GateWaitParams {
            id: id.clone(),
            timeout_ms,
        })
    })?;
    if json {
        print_json(&r);
    } else if r.outcome == WaitOutcome::TimedOut {
        println!("{} still pending after {} ms", r.gate.id, r.waited_ms);
    } else {
        print_gate(&r.gate);
    }
    Ok(match (r.outcome, r.gate.state) {
        (WaitOutcome::Matched, GateState::Approved) => ExitCode::SUCCESS,
        _ => ExitCode::from(1),
    })
}

pub fn gate(cmd: GateCmd, c: &mut Client, pipe: &str, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        GateCmd::Request {
            message,
            title,
            task,
            group,
            expires,
            wait,
            timeout,
        } => {
            let terminal = crate::caller_terminal(pipe);
            let task = task.or_else(|| {
                terminal
                    .as_ref()
                    .and_then(|_| std::env::var("POD_TASK_ID").ok())
                    .filter(|t| !t.is_empty())
            });
            if terminal.is_none() && task.is_none() && group.is_none() {
                bail!(
                    "not inside a Pod terminal of {pipe} ({} unset); pass --task or --group",
                    endpoint::ENV_TERMINAL_ID
                );
            }
            let g = c.call::<GateCreate>(&GateCreateParams {
                task,
                group,
                terminal,
                title,
                message,
                expires_in_s: expires,
                ..Default::default()
            })?;
            if wait {
                if !json {
                    println!("{} pending: waiting for approval", g.id);
                }
                return wait_gate(c, g.id, secs_to_ms(timeout), json);
            }
            if json {
                print_json(&g);
            } else {
                print_gate(&g);
            }
        }
        GateCmd::Ls {
            task,
            group,
            pending,
        } => {
            let l = c.call::<GateList>(&GateListParams {
                task,
                group,
                state: pending.then_some(GateState::Pending),
            })?;
            if json {
                print_json(&l);
            } else if l.gates.is_empty() {
                println!("no gates");
            } else {
                let rows = l
                    .gates
                    .iter()
                    .map(|g| {
                        vec![
                            g.id.clone(),
                            gate_state(g.state).into(),
                            g.task
                                .clone()
                                .or(g.group.clone())
                                .unwrap_or_else(|| "-".into()),
                            g.terminal.clone().unwrap_or_else(|| "-".into()),
                            g.title.clone(),
                            first_line(&g.message),
                        ]
                    })
                    .collect();
                print_rows(
                    &["ID", "STATE", "TASK/GROUP", "TERMINAL", "TITLE", "MESSAGE"],
                    rows,
                );
            }
        }
        GateCmd::Show { id } => {
            let g = c.call::<GateShow>(&GateRef { id })?;
            if json {
                print_json(&g);
            } else {
                print_gate(&g);
                println!("{}", g.message);
            }
        }
        // O1 (docs/19 §19): inside a Pod terminal the decision goes through
        // `orch.gate.decide` with this terminal as the caller, so a
        // coordinator agent meets the same trust policy as `pod orch gate
        // decide` (and a worker cannot decide). Outside: the person's path.
        GateCmd::Approve { id, note, choice } => {
            return decide_here(c, pipe, id, GateDecision::Approve, note, choice, json);
        }
        GateCmd::Reject { id, note } => {
            return decide_here(c, pipe, id, GateDecision::Reject, note, None, json);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `pod gate approve|reject`: from a Pod terminal through the agent trust
/// policy (`orch.gate.decide` with the caller), else `gate.resolve`. A
/// daemon without `orch.gate.decide` (before v17) gets `gate.resolve`.
fn decide_here(
    c: &mut Client,
    pipe: &str,
    id: String,
    decision: GateDecision,
    note: Option<String>,
    choice: Option<String>,
    json: bool,
) -> anyhow::Result<ExitCode> {
    let caller = crate::orch_run::caller(pipe);
    if caller.is_empty() {
        return resolve_gate(c, id, decision, note, choice, json);
    }
    let r = c.call::<OrchGateDecide>(&OrchGateDecideParams {
        id: id.clone(),
        choice: choice.clone(),
        reject: decision == GateDecision::Reject,
        note: note.clone(),
        caller,
        request_id: None,
    });
    let g = match r {
        Err(podd::client::ClientError::Rpc(e)) if e.code == codes::METHOD_NOT_FOUND => {
            return resolve_gate(c, id, decision, note, choice, json);
        }
        r => r?,
    };
    if json {
        print_json(&g);
    } else {
        print_gate(&g);
    }
    Ok(ExitCode::SUCCESS)
}

pub fn resolve_gate(
    c: &mut Client,
    id: String,
    decision: GateDecision,
    note: Option<String>,
    choice: Option<String>,
    json: bool,
) -> anyhow::Result<ExitCode> {
    let g = c.call::<GateResolve>(&GateResolveParams {
        id,
        decision,
        note,
        choice,
    })?;
    if json {
        print_json(&g);
    } else {
        print_gate(&g);
    }
    Ok(ExitCode::SUCCESS)
}
