//! `pod orch ...`: orchestration for agents inside Pod terminals (docs/19 §6).
//!
//! The caller's identity comes from the environment (`POD_TERMINAL_ID`,
//! `POD_ATTEMPT_ID`, `POD_ATTEMPT_TOKEN`) when this `pod` talks to the
//! daemon that owns its terminal. Every mutating call carries a fresh
//! `request_id`; `ask` and `check --wait` reconnect after a daemon restart
//! and resume with the same id.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Subcommand, ValueEnum};
use podd::client::Client;
use podd::protocol::*;

use crate::{print_json, print_rows, reconnect};

pub const ENV_ATTEMPT_ID: &str = "POD_ATTEMPT_ID";
pub const ENV_ATTEMPT_TOKEN: &str = "POD_ATTEMPT_TOKEN";
/// `ask` waits this long by default (the daemon caps it at 30 min).
const ASK_DEFAULT_MS: u64 = 600_000;

#[derive(Subcommand)]
pub enum OrchCmd {
    /// Runs: one objective and its coordinator.
    Run {
        #[command(subcommand)]
        cmd: RunCmd,
    },
    /// Work items of a run (with dependencies).
    Item {
        #[command(subcommand)]
        cmd: ItemCmd,
    },
    /// Start or supervise one worker (attempt).
    Worker {
        #[command(subcommand)]
        cmd: WorkerCmd,
    },
    /// Workers of your run (or --run, or every run outside a Pod terminal).
    Workers {
        #[arg(long)]
        run: Option<String>,
    },
    /// Workers waiting for resources (FIFO), the limits and measurements.
    Queue {
        #[arg(long)]
        run: Option<String>,
    },
    /// Resource limits of the worker queue (daemon settings; an
    /// environment variable still wins). No flags: show them.
    Limits {
        /// Save a limit, e.g. `--set max_workers=4 --set min_free_mem=4G
        /// --set cpu_max_pct=off`.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Return a limit to its default.
        #[arg(long = "reset", value_name = "KEY")]
        reset: Vec<String>,
    },
    /// Decision gates of a run (docs/19 §4.5).
    Gate {
        #[command(subcommand)]
        cmd: OrchGateCmd,
    },
    /// Send a message (workers: to their coordinator by default).
    Send {
        /// run:<id>, attempt:<id>, @all, @idle, @claude, @codex.
        #[arg(long)]
        to: Option<String>,
        #[arg(long = "type", value_enum, default_value = "status")]
        kind: MailTypeArg,
        #[arg(long)]
        subject: String,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long, value_name = "FILE")]
        body_file: Option<PathBuf>,
        #[arg(long)]
        reply_to: Option<String>,
        /// Run for group addresses.
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        high: bool,
    },
    /// Read your unread mail (marks it read).
    Check {
        /// Block until mail arrives.
        #[arg(long)]
        wait: bool,
        /// Seconds to wait (default 600, at most 1800).
        #[arg(long)]
        timeout: Option<f64>,
        /// Leave the mail unread.
        #[arg(long)]
        no_ack: bool,
        #[arg(long)]
        run: Option<String>,
    },
    /// Answer a message (questions get an `answer`).
    Reply {
        #[arg(long)]
        id: String,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        #[arg(long, value_name = "FILE")]
        body_file: Option<PathBuf>,
    },
    /// Workers: ask the coordinator and wait for the answer.
    Ask {
        #[arg(long)]
        question: String,
        #[arg(long)]
        subject: Option<String>,
        /// Seconds to wait (default 600, at most 1800).
        #[arg(long)]
        timeout: Option<f64>,
    },
    /// Workers: report completion, exactly once.
    Done {
        #[arg(long, value_enum)]
        outcome: OutcomeArg,
        /// What you did / what you found / what is left.
        #[arg(
            long,
            required_unless_present = "body_file",
            conflicts_with = "body_file"
        )]
        body: Option<String>,
        /// Read the report from a file: long text with quotes or line
        /// breaks, which Windows PowerShell 5.1 splits when it passes them
        /// to `pod` as one argument.
        #[arg(long, value_name = "FILE")]
        body_file: Option<PathBuf>,
        /// Changed files, comma separated.
        #[arg(long, value_delimiter = ',')]
        files: Vec<String>,
        /// A report file to point at.
        #[arg(long)]
        report: Option<String>,
    },
    /// Workers: wait for other items of your run to finish (e.g. the one
    /// whose branch you build on), then end your turn. Not a missed done:
    /// Pod mails you when they all finished.
    Wait {
        /// An item to wait for (repeat, or comma separated).
        #[arg(long = "item", required = true, value_delimiter = ',')]
        items: Vec<String>,
    },
    /// Workers: commit in your own worktree through Pod (hooks off). Codex
    /// workers use this instead of `git add`/`git commit`, which their
    /// sandbox does not allow.
    Commit {
        /// The commit message.
        #[arg(
            short = 'm',
            long,
            required_unless_present = "message_file",
            conflicts_with = "message_file"
        )]
        message: Option<String>,
        /// Read the message from a file.
        #[arg(long, value_name = "FILE")]
        message_file: Option<PathBuf>,
        /// Stage every change first.
        #[arg(long, short = 'a')]
        all: bool,
        /// Stage these paths first.
        #[arg(required_unless_present = "all")]
        paths: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum OrchGateCmd {
    /// Ask for a decision. A worker's gate blocks its item and goes to the
    /// coordinator; a coordinator's gate goes to a person.
    Request {
        #[arg(long)]
        message: String,
        #[arg(long)]
        title: Option<String>,
        /// A choice (repeat 2..=16 times); without options it is approve/reject.
        #[arg(long = "option", value_name = "TEXT")]
        options: Vec<String>,
        /// The item to block (workers: their own item).
        #[arg(long)]
        item: Option<String>,
        #[arg(long)]
        run: Option<String>,
        /// Expire after S seconds without a decision.
        #[arg(long, value_name = "S")]
        expires: Option<u64>,
        /// Block until decided: exit 0 approved, 1 rejected, expired or timed out.
        #[arg(long)]
        wait: bool,
        #[arg(long, value_name = "S", requires = "wait")]
        timeout: Option<f64>,
    },
    /// Decide a gate of your run (coordinator) or any gate (person).
    Decide {
        id: String,
        #[arg(long, conflicts_with = "reject")]
        choice: Option<String>,
        #[arg(long)]
        reject: bool,
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum RunCmd {
    /// Create a run you coordinate.
    New {
        #[arg(long)]
        objective: String,
    },
    Ls {
        /// Only runs you coordinate or work in.
        #[arg(long)]
        mine: bool,
    },
    Show {
        run: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ItemCmd {
    Add {
        #[arg(
            long,
            conflicts_with = "spec_file",
            required_unless_present = "spec_file"
        )]
        spec: Option<String>,
        #[arg(long, value_name = "FILE")]
        spec_file: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        /// Items that must complete first, comma separated.
        #[arg(long, value_delimiter = ',')]
        deps: Vec<String>,
        #[arg(long)]
        parent: Option<String>,
        #[arg(long)]
        run: Option<String>,
    },
    Ls {
        #[arg(long)]
        ready: bool,
        #[arg(long)]
        run: Option<String>,
    },
    /// Clear the start breaker / make a finished item ready again.
    Reopen { item: String },
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)] // clap args, built once
pub enum WorkerCmd {
    /// Start a worker for an item or a new spec (creates a run if you have none).
    Start {
        #[arg(long, conflicts_with_all = ["spec", "spec_file"])]
        item: Option<String>,
        #[arg(long, conflicts_with = "spec_file")]
        spec: Option<String>,
        #[arg(long, value_name = "FILE")]
        spec_file: Option<PathBuf>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_delimiter = ',')]
        deps: Vec<String>,
        /// claude, codex, shell or a JSON argv array.
        #[arg(long)]
        agent: Option<String>,
        /// new (default), current (your worktree) or an existing task.
        #[arg(long)]
        place: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        name: Option<String>,
        /// Never release it automatically.
        #[arg(long)]
        keep: bool,
        #[arg(long)]
        retry_of: Option<String>,
        #[arg(long)]
        run: Option<String>,
        #[arg(long)]
        objective: Option<String>,
    },
    Show {
        attempt: String,
    },
    /// The worker's terminal output (or its archive after release).
    Read {
        attempt: String,
        #[arg(long)]
        lines: Option<usize>,
    },
    /// Kill the worker's process tree (stopped only when proven gone).
    Stop {
        attempt: String,
    },
    /// End the record; leave the terminal and worktree alone.
    Abandon {
        attempt: String,
    },
    /// Never release it automatically.
    Keep {
        attempt: String,
    },
    /// Close the terminal now (output archived, worktree kept).
    Release {
        attempt: String,
    },
    /// Push a finished worker's task branch and open its pull request (the
    /// `pod pr create` path, docs/15). Title: the item title; body: the
    /// worker's done report.
    Pr {
        attempt: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        draft: bool,
        /// Base branch on the remote (default: the task's base branch).
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        no_push: bool,
        /// Also for a worker that has not reported `succeeded`.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum MailTypeArg {
    Question,
    Answer,
    Escalation,
    Status,
    Handoff,
    Heartbeat,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum OutcomeArg {
    Succeeded,
    Failed,
}

/// The caller from the environment (only for this daemon's terminals).
pub fn caller(pipe: &str) -> OrchCaller {
    let terminal = crate::caller_terminal(pipe);
    let mine = terminal.is_some();
    let var = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty() && mine);
    OrchCaller {
        terminal,
        attempt: var(ENV_ATTEMPT_ID),
        token: var(ENV_ATTEMPT_TOKEN),
    }
}

/// A request id unique to this invocation.
fn request_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("cli-{}-{nanos}", std::process::id())
}

fn read_text(inline: Option<String>, file: Option<PathBuf>, what: &str) -> anyhow::Result<String> {
    match (inline, file) {
        (Some(s), _) => Ok(s),
        (None, Some(f)) => {
            std::fs::read_to_string(&f).with_context(|| format!("{what}: {}", f.display()))
        }
        (None, None) => Ok(String::new()),
    }
}

fn secs_ms(s: Option<f64>) -> anyhow::Result<Option<u64>> {
    match s {
        Some(v) if !v.is_finite() || v < 0.0 => {
            bail!("--timeout must be a non-negative number of seconds")
        }
        Some(v) => Ok(Some((v * 1000.0) as u64)),
        None => Ok(None),
    }
}

fn mail_type(k: MailTypeArg) -> MailType {
    match k {
        MailTypeArg::Question => MailType::Question,
        MailTypeArg::Answer => MailType::Answer,
        MailTypeArg::Escalation => MailType::Escalation,
        MailTypeArg::Status => MailType::Status,
        MailTypeArg::Handoff => MailType::Handoff,
        MailTypeArg::Heartbeat => MailType::Heartbeat,
    }
}

fn word<T: serde::Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn print_mail(m: &Mail) {
    println!(
        "{} {} from {} to {}: {}",
        m.id,
        word(m.kind),
        m.from,
        m.to,
        m.subject
    );
    if let Some(r) = &m.reply_to {
        println!("  re {r}");
    }
    for l in m.body.lines() {
        println!("  {l}");
    }
}

fn print_run(r: &RunInfo) {
    println!(
        "{}  {}  (coordinator {}, depth {}, {} unread)",
        r.run.id,
        r.run.objective,
        r.run.coordinator_terminal.as_deref().unwrap_or("-"),
        r.run.depth,
        r.unread
    );
    let rows = r
        .items
        .iter()
        .map(|i| {
            vec![
                i.id.clone(),
                word(i.state),
                i.attempt.clone().unwrap_or_default(),
                i.deps.join(","),
                i.title.clone(),
            ]
        })
        .collect();
    print_rows(&["ITEM", "STATE", "ATTEMPT", "DEPS", "TITLE"], rows);
}

fn gib(b: u64) -> String {
    format!("{:.1} GiB", b as f64 / (1u64 << 30) as f64)
}

fn print_limits(r: &OrchLimitsResult) {
    let rows = r
        .entries
        .iter()
        .map(|e| {
            let source = match e.source.as_str() {
                "env" => format!("env {}", e.env),
                other => other.to_string(),
            };
            vec![e.key.clone(), e.value.clone(), source, e.help.clone()]
        })
        .collect();
    print_rows(&["KEY", "VALUE", "FROM", "MEANING"], rows);
    println!("settings: {}", r.path);
}

fn print_queue(r: &OrchQueueResult) {
    let l = &r.limits;
    let opt = |v: Option<String>| v.unwrap_or_else(|| "off".into());
    println!(
        "active workers {}/{} (per run {}), cpu {} (limit {}), free memory {} (min {}), free disk {} (min {})",
        r.active,
        l.max_workers,
        l.run_workers,
        r.cpu_pct
            .map(|c| format!("{c:.0}%"))
            .unwrap_or_else(|| "?".into()),
        opt(l.cpu_max_pct.map(|c| format!("{c:.0}%"))),
        r.free_mem.map(gib).unwrap_or_else(|| "?".into()),
        opt(l.min_free_mem.map(gib)),
        r.free_disk.map(gib).unwrap_or_else(|| "?".into()),
        opt(l.min_free_disk.map(gib)),
    );
    if r.queue.is_empty() {
        println!("no queued workers");
        return;
    }
    let rows = r
        .queue
        .iter()
        .map(|w| {
            let q = w.attempt.queue.as_ref();
            vec![
                q.map(|q| q.position.to_string()).unwrap_or_default(),
                w.attempt.id.clone(),
                w.attempt.run.clone(),
                w.attempt.item.clone(),
                q.map(|q| word(q.kind)).unwrap_or_default(),
                q.map(|q| q.reason.clone()).unwrap_or_default(),
            ]
        })
        .collect();
    print_rows(&["#", "ATTEMPT", "RUN", "ITEM", "WAIT", "REASON"], rows);
}

fn gate(cmd: OrchGateCmd, c: &mut Client, who: OrchCaller, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        OrchGateCmd::Request {
            message,
            title,
            options,
            item,
            run,
            expires,
            wait,
            timeout,
        } => {
            let g = c.call::<OrchGateRequest>(&OrchGateRequestParams {
                title,
                message,
                options,
                item,
                run,
                expires_in_s: expires,
                caller: who,
                request_id: Some(request_id()),
            })?;
            if wait {
                if !json {
                    println!("{} pending: waiting for a decision", g.id);
                }
                return crate::orch::wait_gate(c, g.id, secs_ms(timeout)?, json);
            }
            if json {
                print_json(&g);
            } else {
                crate::orch::print_gate(&g);
                println!(
                    "next: Do not poll. Pod mails you the decision (`pod mail: ...`, then `pod orch check`); end your turn now."
                );
            }
        }
        OrchGateCmd::Decide {
            id,
            choice,
            reject,
            note,
        } => {
            let g = c.call::<OrchGateDecide>(&OrchGateDecideParams {
                id,
                choice,
                reject,
                note,
                caller: who,
                request_id: Some(request_id()),
            })?;
            if json {
                print_json(&g);
            } else {
                crate::orch::print_gate(&g);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_workers(ws: &[WorkerInfo]) {
    let rows = ws
        .iter()
        .map(|w| {
            vec![
                w.attempt.id.clone(),
                w.attempt.item.clone(),
                word(w.attempt.state),
                word(w.attempt.worker),
                w.attempt.agent.clone(),
                w.status
                    .as_ref()
                    .map(|s| s.badge.clone())
                    .unwrap_or_default(),
                w.attempt.task.clone().unwrap_or_default(),
                w.next.clone(),
            ]
        })
        .collect();
    print_rows(
        &[
            "ATTEMPT", "ITEM", "STATE", "WORKER", "AGENT", "STATUS", "TASK", "NEXT",
        ],
        rows,
    );
}

pub fn run(cmd: OrchCmd, c: &mut Client, pipe: &str, json: bool) -> anyhow::Result<ExitCode> {
    let who = caller(pipe);
    let ok = ExitCode::SUCCESS;
    match cmd {
        OrchCmd::Run { cmd } => match cmd {
            RunCmd::New { objective } => {
                let r = c.call::<OrchRunCreate>(&OrchRunCreateParams {
                    objective,
                    caller: who,
                    request_id: Some(request_id()),
                })?;
                if json {
                    print_json(&r);
                } else {
                    println!("{}", r.run.id);
                }
            }
            RunCmd::Ls { mine } => {
                let r = c.call::<OrchRunList>(&OrchRunListParams { mine, caller: who })?;
                if json {
                    print_json(&r);
                } else {
                    for run in &r.runs {
                        print_run(run);
                    }
                }
            }
            RunCmd::Show { run } => {
                let r = c.call::<OrchRunShow>(&OrchRunRef { run, caller: who })?;
                if json {
                    print_json(&r);
                } else {
                    print_run(&r);
                }
            }
        },
        OrchCmd::Item { cmd } => match cmd {
            ItemCmd::Add {
                spec,
                spec_file,
                title,
                deps,
                parent,
                run,
            } => {
                let spec = read_text(spec, spec_file, "--spec-file")?;
                let it = c.call::<OrchItemCreate>(&OrchItemCreateParams {
                    run,
                    title,
                    spec,
                    deps,
                    parent,
                    caller: who,
                    request_id: Some(request_id()),
                })?;
                if json {
                    print_json(&it);
                } else {
                    println!("{} {}", it.id, word(it.state));
                }
            }
            ItemCmd::Ls { ready, run } => {
                let r = c.call::<OrchItemList>(&OrchItemListParams {
                    run,
                    ready,
                    caller: who,
                })?;
                if json {
                    print_json(&r);
                } else {
                    let rows = r
                        .items
                        .iter()
                        .map(|i| {
                            vec![
                                i.id.clone(),
                                word(i.state),
                                i.deps.join(","),
                                i.title.clone(),
                            ]
                        })
                        .collect();
                    print_rows(&["ITEM", "STATE", "DEPS", "TITLE"], rows);
                }
            }
            ItemCmd::Reopen { item } => {
                let it = c.call::<OrchItemUpdate>(&OrchItemUpdateParams {
                    item,
                    reopen: true,
                    caller: who,
                    request_id: Some(request_id()),
                })?;
                if json {
                    print_json(&it);
                } else {
                    println!("{} {}", it.id, word(it.state));
                }
            }
        },
        OrchCmd::Worker { cmd } => return worker(cmd, c, who, json),
        OrchCmd::Workers { run } => {
            let r = c.call::<OrchWorkerList>(&OrchRunRef { run, caller: who })?;
            if json {
                print_json(&r);
            } else {
                print_workers(&r.workers);
            }
        }
        OrchCmd::Queue { run } => {
            let r = c.call::<OrchQueue>(&OrchRunRef { run, caller: who })?;
            if json {
                print_json(&r);
            } else {
                print_queue(&r);
            }
        }
        OrchCmd::Limits { set, reset } => {
            let r = if set.is_empty() && reset.is_empty() {
                c.call::<OrchLimitsGet>(&OrchLimitsGetParams {})?
            } else {
                let mut map = std::collections::BTreeMap::new();
                for kv in set {
                    let Some((k, v)) = kv.split_once('=') else {
                        bail!("--set takes KEY=VALUE (got {kv:?})");
                    };
                    map.insert(k.trim().to_string(), v.trim().to_string());
                }
                c.call::<OrchLimitsSet>(&OrchLimitsSetParams {
                    set: map,
                    reset,
                    caller: who,
                })?
            };
            if json {
                print_json(&r);
            } else {
                print_limits(&r);
            }
        }
        OrchCmd::Gate { cmd } => return gate(cmd, c, who, json),
        OrchCmd::Send {
            to,
            kind,
            subject,
            body,
            body_file,
            reply_to,
            run,
            high,
        } => {
            let body = read_text(body, body_file, "--body-file")?;
            let m = c.call::<OrchMailSend>(&OrchMailSendParams {
                run,
                to,
                kind: mail_type(kind),
                subject,
                body,
                reply_to,
                payload: None,
                priority: if high {
                    Priority::High
                } else {
                    Priority::Normal
                },
                caller: who,
                request_id: Some(request_id()),
            })?;
            if json {
                print_json(&m);
            } else {
                println!("{} sent to {}", m.id, m.recipients.join(", "));
            }
        }
        OrchCmd::Check {
            wait,
            timeout,
            no_ack,
            run,
        } => {
            let timeout_ms = secs_ms(timeout)?;
            let mut call = |c: &mut Client, t: Option<u64>| {
                c.call::<OrchMailCheck>(&OrchMailCheckParams {
                    run: run.clone(),
                    wait,
                    timeout_ms: t,
                    ack: !no_ack,
                    caller: who.clone(),
                })
            };
            let r = if wait {
                let t = timeout_ms.or(Some(600_000));
                reconnect::wait(c, t, "mail", &mut call)?
            } else {
                call(c, None)?
            };
            if json {
                print_json(&r);
            } else {
                if r.messages.is_empty() {
                    println!("no new mail");
                }
                for m in &r.messages {
                    print_mail(m);
                }
                println!("next: {}", r.next);
            }
        }
        OrchCmd::Reply {
            id,
            body,
            body_file,
        } => {
            let body = read_text(body, body_file, "--body-file")?;
            let m = c.call::<OrchMailReply>(&OrchMailReplyParams {
                id,
                body,
                caller: who,
                request_id: Some(request_id()),
            })?;
            if json {
                print_json(&m);
            } else {
                println!("{} sent to {}", m.id, m.recipients.join(", "));
            }
        }
        OrchCmd::Ask {
            question,
            subject,
            timeout,
        } => {
            if who.attempt.is_none() {
                bail!("ask is for workers ({ENV_ATTEMPT_ID} is not set here); use `pod orch send`");
            }
            let timeout_ms = secs_ms(timeout)?.or(Some(ASK_DEFAULT_MS));
            // One request id for the whole wait: after a reconnect the
            // daemon finds the same question and keeps waiting for its answer.
            let rid = request_id();
            let r = reconnect::wait(c, timeout_ms, "the answer", |c, t| {
                c.call::<OrchMailAsk>(&OrchMailAskParams {
                    question: question.clone(),
                    subject: subject.clone(),
                    timeout_ms: t,
                    caller: who.clone(),
                    request_id: Some(rid.clone()),
                })
            })?;
            if json {
                print_json(&r);
            } else {
                match &r.answer {
                    Some(a) => {
                        println!("answer {} to {}:", a.id, r.question.id);
                        for l in a.body.lines() {
                            println!("{l}");
                        }
                    }
                    None => println!("no answer yet to {}", r.question.id),
                }
                println!("next: {}", r.next);
            }
            if r.answer.is_none() {
                return Ok(ExitCode::from(1));
            }
        }
        OrchCmd::Done {
            outcome,
            body,
            body_file,
            files,
            report,
        } => {
            if who.attempt.is_none() {
                bail!("done is for workers ({ENV_ATTEMPT_ID} is not set here)");
            }
            let body = read_text(body, body_file, "--body-file")?;
            let r = c.call::<OrchDone>(&OrchDoneParams {
                outcome: match outcome {
                    OutcomeArg::Succeeded => DoneOutcome::Succeeded,
                    OutcomeArg::Failed => DoneOutcome::Failed,
                },
                body,
                files,
                report,
                caller: who,
                request_id: Some(request_id()),
            })?;
            if json {
                print_json(&r);
            } else {
                println!(
                    "{} {} ({})",
                    r.attempt.id,
                    word(r.attempt.state),
                    if r.accepted {
                        "recorded"
                    } else {
                        "superseded: ignored"
                    }
                );
                println!("next: {}", r.next);
            }
            if !r.accepted {
                return Ok(ExitCode::from(1));
            }
        }
        OrchCmd::Commit {
            message,
            message_file,
            all,
            paths,
        } => {
            if who.attempt.is_none() {
                bail!("commit is for workers ({ENV_ATTEMPT_ID} is not set here); use git");
            }
            let message = read_text(message, message_file, "--message-file")?;
            let r = c.call::<OrchCommit>(&OrchCommitParams {
                message,
                all,
                paths,
                caller: who,
            })?;
            if json {
                print_json(&r);
            } else {
                println!("{} {}", &r.commit[..r.commit.len().min(12)], r.summary);
                println!("next: {}", r.next);
            }
        }
        OrchCmd::Wait { items } => {
            if who.attempt.is_none() {
                bail!(
                    "wait is for workers ({ENV_ATTEMPT_ID} is not set here); a coordinator orders items with --deps"
                );
            }
            let r = c.call::<OrchWait>(&OrchWaitParams { items, caller: who })?;
            if json {
                print_json(&r);
            } else {
                for i in &r.items {
                    println!("{} {}", i.item, word(i.state));
                }
                println!("next: {}", r.next);
            }
        }
    }
    Ok(ok)
}

struct PrArgs {
    title: Option<String>,
    body: Option<String>,
    draft: bool,
    base: Option<String>,
    no_push: bool,
    force: bool,
}

/// The `task.pr.create` request for a worker (docs/19 §16.2): only a worker
/// that reported `succeeded` (unless forced), title = item title, body =
/// its accepted done report.
fn pr_request(c: &mut Client, w: &WorkerInfo, a: PrArgs) -> anyhow::Result<TaskPrCreateParams> {
    let at = &w.attempt;
    let Some(task) = at.task.clone() else {
        bail!(
            "worker {} has no task (placed in an existing worktree?); use `pod pr create <task>`",
            at.id
        );
    };
    if at.state != AttemptState::Succeeded && !a.force {
        bail!(
            "worker {} is {} (not succeeded); pass --force to open a pull request anyway",
            at.id,
            word(at.state)
        );
    }
    let body = match a.body {
        Some(b) => b,
        None => {
            let mail = c
                .call::<OrchMailList>(&OrchMailListParams {
                    run: at.run.clone(),
                })?
                .messages;
            done_report(&mail, &at.id).unwrap_or_default()
        }
    };
    let title = a.title.unwrap_or_else(|| {
        if w.item_title.trim().is_empty() {
            task.clone()
        } else {
            w.item_title.clone()
        }
    });
    Ok(TaskPrCreateParams {
        task,
        title,
        body,
        draft: a.draft,
        base: a.base,
        push: Some(!a.no_push),
    })
}

/// The body of an attempt's accepted done report.
fn done_report(mail: &[Mail], attempt: &str) -> Option<String> {
    let from = format!("attempt:{attempt}");
    mail.iter()
        .rev()
        .find(|m| m.kind == MailType::Done && m.from == from && !m.fenced)
        .map(|m| {
            format!(
                "{}

(Pod orchestration: run {}, attempt {attempt})",
                m.body.trim_end(),
                m.run
            )
        })
}

fn worker(cmd: WorkerCmd, c: &mut Client, who: OrchCaller, json: bool) -> anyhow::Result<ExitCode> {
    let simple = |c: &mut Client, attempt: String, which: &str| -> anyhow::Result<WorkerInfo> {
        let p = OrchAttemptRef {
            attempt,
            caller: who.clone(),
            request_id: Some(request_id()),
        };
        Ok(match which {
            "stop" => c.call::<OrchWorkerStop>(&p)?,
            "abandon" => c.call::<OrchWorkerAbandon>(&p)?,
            "keep" => c.call::<OrchWorkerKeep>(&p)?,
            "release" => c.call::<OrchWorkerRelease>(&p)?,
            _ => c.call::<OrchWorkerShow>(&p)?,
        })
    };
    let (attempt, which) = match cmd {
        WorkerCmd::Start {
            item,
            spec,
            spec_file,
            title,
            deps,
            agent,
            place,
            repo,
            base,
            name,
            keep,
            retry_of,
            run,
            objective,
        } => {
            let spec = match (spec, spec_file) {
                (None, None) => None,
                (s, f) => Some(read_text(s, f, "--spec-file")?),
            };
            if spec.is_none() && item.is_none() {
                bail!("give --item, --spec or --spec-file");
            }
            let agent = agent.as_deref().map(crate::orch::parse_agent).transpose()?;
            let r = c.call::<OrchWorkerStart>(&OrchWorkerStartParams {
                run,
                item,
                spec,
                title,
                deps,
                objective,
                agent,
                place,
                repo,
                base,
                name,
                keep,
                retry_of,
                cols: None,
                rows: None,
                caller: who.clone(),
                request_id: Some(request_id()),
            })?;
            if json {
                print_json(&r);
            } else {
                println!(
                    "{} {} (run {}, item {}, task {}, terminal {})",
                    r.attempt.id,
                    word(r.state),
                    r.run,
                    r.item,
                    r.task.as_deref().unwrap_or("-"),
                    r.terminal.as_deref().unwrap_or("-")
                );
                if let Some(e) = &r.error {
                    println!("error at {}: {e}", r.stage);
                }
                println!("next: {}", r.next);
            }
            return Ok(if r.error.is_some() {
                ExitCode::from(1)
            } else {
                ExitCode::SUCCESS
            });
        }
        WorkerCmd::Read { attempt, lines } => {
            let r = c.call::<OrchWorkerRead>(&OrchWorkerReadParams { attempt, lines })?;
            if json {
                print_json(&r);
            } else {
                for l in &r.lines {
                    println!("{l}");
                }
                crate::print_suggestion(r.suggestion.as_deref());
            }
            return Ok(ExitCode::SUCCESS);
        }
        WorkerCmd::Pr {
            attempt,
            title,
            body,
            draft,
            base,
            no_push,
            force,
        } => {
            let w = simple(c, attempt, "show")?;
            let req = pr_request(
                c,
                &w,
                PrArgs {
                    title,
                    body,
                    draft,
                    base,
                    no_push,
                    force,
                },
            )?;
            let i = c.call::<TaskPrCreate>(&req)?;
            if json {
                print_json(&i);
            } else {
                crate::pr::print_info(&i);
            }
            return Ok(ExitCode::SUCCESS);
        }
        WorkerCmd::Show { attempt } => (attempt, "show"),
        WorkerCmd::Stop { attempt } => (attempt, "stop"),
        WorkerCmd::Abandon { attempt } => (attempt, "abandon"),
        WorkerCmd::Keep { attempt } => (attempt, "keep"),
        WorkerCmd::Release { attempt } => (attempt, "release"),
    };
    let w = simple(c, attempt, which)?;
    if json {
        print_json(&w);
    } else {
        print_workers(std::slice::from_ref(&w));
    }
    // A stop that could not be proven is not a success.
    Ok(if w.attempt.state == AttemptState::Unknown {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(id: &str, from: &str, kind: MailType, body: &str, fenced: bool) -> Mail {
        serde_json::from_value(serde_json::json!({
            "id": id, "run": "r1", "from": from, "to": "run:r1", "recipients": ["run:r1"],
            "type": kind, "subject": "s", "body": body, "created_ms": 1, "fenced": fenced
        }))
        .unwrap()
    }

    #[test]
    fn pr_body_is_the_accepted_done_report() {
        let m = vec![
            mail("m1", "attempt:a2", MailType::Question, "q?", false),
            mail("m2", "attempt:a1", MailType::Done, "other worker", false),
            mail(
                "m3",
                "attempt:a2",
                MailType::Done,
                "fixed expiry\nall tests pass\n",
                false,
            ),
            mail("m4", "attempt:a2", MailType::Done, "late one", true),
        ];
        assert_eq!(
            done_report(&m, "a2").as_deref(),
            Some("fixed expiry\nall tests pass\n\n(Pod orchestration: run r1, attempt a2)")
        );
        assert_eq!(done_report(&m, "a3"), None);
    }
}
