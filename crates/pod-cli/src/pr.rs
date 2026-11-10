//! `pod pr …`: a task's pull request (protocol v10, docs/15).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::Subcommand;
use podd::client::Client;
use podd::protocol::*;

#[derive(Subcommand)]
pub enum PrCmd {
    /// Show the task's pull request: remote, state, review, checks.
    Status {
        task: String,
        /// Use the last known state; do not contact GitHub.
        #[arg(long)]
        cached: bool,
    },
    /// Push the task branch and open a pull request.
    Create {
        task: String,
        /// Title (default: the first line of the task prompt, else the task name).
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "body_file")]
        body: Option<String>,
        /// Read the body from a file (UTF-8).
        #[arg(long, value_name = "FILE")]
        body_file: Option<PathBuf>,
        /// Open as a draft.
        #[arg(long)]
        draft: bool,
        /// Base branch on the remote (default: the task's base branch).
        #[arg(long)]
        base: Option<String>,
        /// Do not push first (the branch is already on the remote).
        #[arg(long)]
        no_push: bool,
    },
    /// Merge the task's pull request on GitHub.
    Merge {
        task: String,
        #[arg(long, value_enum)]
        method: MethodArg,
        /// Required: merging changes the remote repository.
        #[arg(long)]
        yes: bool,
    },
    /// Open the pull request in the browser.
    Open {
        task: String,
        /// Print the URL instead of opening it.
        #[arg(long)]
        print: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum MethodArg {
    Merge,
    Squash,
    Rebase,
}

impl From<MethodArg> for MergeMethod {
    fn from(m: MethodArg) -> Self {
        match m {
            MethodArg::Merge => MergeMethod::Merge,
            MethodArg::Squash => MergeMethod::Squash,
            MethodArg::Rebase => MergeMethod::Rebase,
        }
    }
}

fn state_word(pr: &PrView) -> String {
    match (pr.state, pr.draft) {
        (PrState::Open, true) => "open (draft)".into(),
        (PrState::Open, false) => "open".into(),
        (PrState::Merged, _) => "merged".into(),
        (PrState::Closed, _) => "closed".into(),
    }
}

fn review_word(r: Option<ReviewDecision>) -> &'static str {
    match r {
        Some(ReviewDecision::Approved) => "approved",
        Some(ReviewDecision::ChangesRequested) => "changes requested",
        Some(ReviewDecision::ReviewRequired) => "review required",
        None => "no review required",
    }
}

fn bucket_mark(b: CheckBucket) -> &'static str {
    match b {
        CheckBucket::Pass => "ok  ",
        CheckBucket::Fail => "FAIL",
        CheckBucket::Cancel => "cancel",
        CheckBucket::Pending => "... ",
        CheckBucket::Skipping => "skip",
    }
}

pub fn summary_line(s: &ChecksSummary) -> String {
    if s.total == 0 {
        return "no checks".into();
    }
    let mut parts = vec![format!("{} passed", s.passed)];
    if s.failed > 0 {
        parts.push(format!("{} failed", s.failed));
    }
    if s.pending > 0 {
        parts.push(format!("{} pending", s.pending));
    }
    if s.skipped > 0 {
        parts.push(format!("{} skipped", s.skipped));
    }
    format!("{} ({})", parts.join(", "), s.verdict())
}

pub fn print_info(i: &TaskPrInfo) {
    println!("task      {}", i.task);
    let base = i.default_base.as_deref().unwrap_or("?");
    println!(
        "branch    {} -> {base} ({} ahead, {} unpushed, {} uncommitted)",
        i.branch, i.ahead, i.unpushed, i.dirty
    );
    match &i.status.remote {
        Some(r) => println!("remote    {}  {}", r.name, r.slug.web_url()),
        None => println!("remote    none"),
    }
    match &i.status.pr {
        Some(pr) => {
            println!("pr        #{} {}  {}", pr.number, state_word(pr), pr.url);
            println!("title     {}", pr.title);
            println!("review    {}", review_word(pr.review_decision));
            // GitHub's merge state only means something while the pull request is open.
            if let Some(m) = pr
                .merge_state
                .as_ref()
                .filter(|_| pr.state == PrState::Open)
            {
                println!("mergeable {m}");
            }
            println!("checks    {}", summary_line(&i.status.summary));
            for c in &i.status.checks {
                println!(
                    "  {} {}{}",
                    bucket_mark(c.bucket),
                    c.name,
                    c.link
                        .as_deref()
                        .map(|l| format!("  {l}"))
                        .unwrap_or_default()
                );
            }
            if !i.status.comments.is_empty() {
                println!("comments  {}", i.status.comments.len());
            }
        }
        None if i.status.remote.is_some() => println!("pr        none for {}", i.branch),
        None => {}
    }
    if let Some(e) = &i.status.error {
        println!("note      {}: {}", e.kind.as_str(), e.message);
    }
    if i.cached && i.status.error.is_some() && i.status.pr.is_some() {
        println!("(last known state: GitHub could not be read)");
    } else if i.cached {
        println!("(cached: GitHub was not contacted)");
    }
}

/// Why merging now may be a mistake: failed or still running checks.
/// GitHub decides (branch protection); Pod only warns.
pub fn checks_warning(s: &ChecksSummary) -> Option<String> {
    let plural = |n: u32| if n == 1 { "" } else { "s" };
    match (s.failed, s.pending) {
        (0, 0) => None,
        (f, 0) => Some(format!("{f} check{} failed", plural(f))),
        (0, p) => Some(format!("{p} check{} still running", plural(p))),
        (f, p) => Some(format!("{f} check{} failed, {p} still running", plural(f))),
    }
}

fn default_title(t: &TaskInfo) -> String {
    t.prompt
        .as_deref()
        .and_then(|p| p.lines().map(str::trim).find(|l| !l.is_empty()))
        .map(|l| l.chars().take(72).collect())
        .unwrap_or_else(|| t.name.clone())
}

/// Open an https URL with the default browser (no shell parsing of `&`).
fn open_url(url: &str) -> anyhow::Result<()> {
    if !url.starts_with("https://") {
        bail!("refusing to open a non-https URL: {url}");
    }
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .context("cannot start the browser")?;
    Ok(())
}

pub fn run(cmd: PrCmd, c: &mut Client, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        PrCmd::Status { task, cached } => {
            let i = c.call::<TaskPrStatusMethod>(&TaskPrStatusParams { task, cached })?;
            if json {
                crate::print_json(&i);
            } else {
                print_info(&i);
            }
        }
        PrCmd::Create {
            task,
            title,
            body,
            body_file,
            draft,
            base,
            no_push,
        } => {
            let body = match (body, body_file) {
                (Some(b), _) => b,
                (None, Some(f)) => std::fs::read_to_string(&f)
                    .with_context(|| format!("cannot read {}", f.display()))?,
                (None, None) => String::new(),
            };
            let title = match title {
                Some(t) => t,
                None => default_title(&c.call::<TaskShow>(&TaskRef { task: task.clone() })?),
            };
            let i = c.call::<TaskPrCreate>(&TaskPrCreateParams {
                task,
                title,
                body,
                draft,
                base,
                push: Some(!no_push),
            })?;
            if json {
                crate::print_json(&i);
            } else {
                print_info(&i);
            }
        }
        PrCmd::Merge { task, method, yes } => {
            // The preview reads GitHub so its checks are current; with --yes the
            // last known state is enough for the warning.
            let pre = c.call::<TaskPrStatusMethod>(&TaskPrStatusParams {
                task: task.clone(),
                cached: yes,
            })?;
            let warning = pre
                .status
                .pr
                .as_ref()
                .and(checks_warning(&pre.status.summary));
            if !yes {
                match &pre.status.pr {
                    Some(pr) => {
                        eprintln!(
                            "pod: would merge #{} ({}) of {task}; checks: {}",
                            pr.number,
                            pr.url,
                            summary_line(&pre.status.summary)
                        );
                        if let Some(w) = &warning {
                            eprintln!("pod: warning: {w}");
                        }
                        eprintln!("pod: pass --yes to merge");
                    }
                    None => eprintln!("pod: {task} has no known pull request"),
                }
                return Ok(ExitCode::from(2));
            }
            if let Some(w) = &warning {
                eprintln!("pod: warning: {w}; merging anyway (--yes)");
            }
            let i = c.call::<TaskPrMerge>(&TaskPrMergeParams {
                task,
                method: method.into(),
            })?;
            if json {
                crate::print_json(&i);
            } else {
                print_info(&i);
            }
        }
        PrCmd::Open { task, print } => {
            let t = c.call::<TaskShow>(&TaskRef { task: task.clone() })?;
            let url = match t.pr {
                Some(pr) => pr.url,
                None => match c
                    .call::<TaskPrStatusMethod>(&TaskPrStatusParams {
                        task: task.clone(),
                        cached: false,
                    })?
                    .status
                    .pr
                {
                    Some(pr) => pr.url,
                    None => bail!("{task} has no pull request"),
                },
            };
            if print || json {
                println!("{url}");
            } else {
                open_url(&url)?;
                println!("opened {url}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_lines() {
        assert_eq!(summary_line(&ChecksSummary::default()), "no checks");
        let s = ChecksSummary {
            total: 5,
            passed: 2,
            failed: 1,
            pending: 1,
            skipped: 1,
        };
        assert_eq!(
            summary_line(&s),
            "2 passed, 1 failed, 1 pending, 1 skipped (failed)"
        );
        assert_eq!(
            checks_warning(&s).as_deref(),
            Some("1 check failed, 1 still running")
        );
        assert_eq!(
            checks_warning(&ChecksSummary {
                total: 2,
                passed: 1,
                failed: 1,
                ..Default::default()
            })
            .as_deref(),
            Some("1 check failed")
        );
        assert_eq!(
            checks_warning(&ChecksSummary {
                total: 3,
                pending: 3,
                ..Default::default()
            })
            .as_deref(),
            Some("3 checks still running")
        );
        assert_eq!(
            checks_warning(&ChecksSummary {
                total: 2,
                passed: 2,
                ..Default::default()
            }),
            None
        );
    }
}
