//! `pod task archive|unarchive|resume|cleanup|suggest-name` and
//! `pod repo config|trust|set` (docs/19 S1–S3).

use std::path::PathBuf;

use anyhow::Context;
use clap::Subcommand;
use podd::client::Client;
use podd::protocol::*;

use crate::{print_json, print_rows};

#[derive(Subcommand)]
pub enum SessionTaskCmd {
    /// Close the task's terminals (after the repository's archive script);
    /// worktree, branch and agent session stay.
    Archive {
        task: String,
        /// Archive even while an agent is working or blocked.
        #[arg(long)]
        force: bool,
        /// Archive even when the archive script fails or is not approved.
        #[arg(long)]
        ignore_script_failure: bool,
    },
    /// Re-open an archived task and resume its agent session.
    Unarchive {
        task: String,
        /// Only mark it active; start no agent.
        #[arg(long)]
        no_agent: bool,
    },
    /// Start the task's agent again: `claude --resume <id>` / `codex resume
    /// <id>` for the recorded session, else a fresh agent.
    Resume {
        task: String,
        /// Session id to resume instead of the recorded one.
        #[arg(long)]
        session: Option<String>,
        /// Agent of --session (claude or codex).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Tasks by cleanup tier: protected, review, ready.
    Cleanup {
        #[arg(long)]
        repo: Option<String>,
        /// Clean tasks idle this many days are ready (default 7).
        #[arg(long)]
        days: Option<u32>,
    },
    /// Print a free task name for a repository.
    SuggestName { repo: String },
}

#[derive(Subcommand)]
pub enum SessionRepoCmd {
    /// Show .pod/config.toml, its commands, hash and approval.
    Config {
        repo: String,
        /// Read the file from this task's worktree.
        #[arg(long)]
        task: Option<String>,
    },
    /// Approve the repository's commands (shown by `pod repo config`) for this content hash.
    Trust {
        repo: String,
        /// The hash shown by `pod repo config`.
        hash: String,
        /// Withdraw the approval instead.
        #[arg(long)]
        revoke: bool,
        #[arg(long)]
        task: Option<String>,
    },
    /// Personal settings for a repository (no approval needed).
    Set {
        repo: String,
        /// Worktree root for new tasks ("" = default).
        #[arg(long, value_name = "DIR")]
        worktrees_root: Option<String>,
        /// Personal setup script (replaces the repository's).
        #[arg(long, value_name = "SCRIPT")]
        setup: Option<String>,
        /// Personal archive script.
        #[arg(long, value_name = "SCRIPT")]
        archive: Option<String>,
        /// Remove the personal settings.
        #[arg(long)]
        clear_personal: bool,
    },
}

pub fn task_cmd(cmd: SessionTaskCmd, c: &mut Client, json: bool) -> anyhow::Result<()> {
    match cmd {
        SessionTaskCmd::Archive {
            task,
            force,
            ignore_script_failure,
        } => {
            let r = c.call::<TaskArchive>(&TaskArchiveParams {
                task,
                force,
                ignore_script_failure,
            })?;
            if json {
                print_json(&r);
            } else {
                let script = match &r.script {
                    None => String::new(),
                    Some(s) if s.skipped.is_some() => format!(
                        "; archive script skipped: {}",
                        s.skipped.as_deref().unwrap_or("")
                    ),
                    Some(s) if s.ok => "; archive script ok".into(),
                    Some(s) => format!("; archive script failed (exit {:?}), ignored", s.exit_code),
                };
                println!(
                    "{} archived (terminals: {}){script}",
                    r.task,
                    list(&r.terminals)
                );
            }
        }
        SessionTaskCmd::Unarchive { task, no_agent } => {
            let r = c.call::<TaskUnarchive>(&TaskUnarchiveParams {
                task,
                start_agent: Some(!no_agent),
                ..Default::default()
            })?;
            print_resume(&r, json);
        }
        SessionTaskCmd::Resume {
            task,
            session,
            agent,
        } => {
            let r = c.call::<TaskResume>(&TaskResumeParams {
                task,
                session,
                agent,
                ..Default::default()
            })?;
            print_resume(&r, json);
        }
        SessionTaskCmd::Cleanup { repo, days } => {
            let r = c.call::<TaskCleanupCandidates>(&CleanupParams {
                repo,
                inactive_days: days,
            })?;
            if json {
                print_json(&r);
            } else if r.candidates.is_empty() {
                println!("no tasks");
            } else {
                let rows = r
                    .candidates
                    .iter()
                    .map(|x| vec![tier(x.tier).into(), x.task.clone(), x.reasons.join("; ")])
                    .collect();
                print_rows(&["TIER", "TASK", "WHY"], rows);
            }
        }
        SessionTaskCmd::SuggestName { repo } => {
            let r = c.call::<TaskSuggestName>(&SuggestNameParams { repo })?;
            if json {
                print_json(&r);
            } else {
                println!("{}", r.name);
            }
        }
    }
    Ok(())
}

pub fn repo_cmd(cmd: SessionRepoCmd, c: &mut Client, json: bool) -> anyhow::Result<()> {
    let r = match cmd {
        SessionRepoCmd::Config { repo, task } => {
            c.call::<RepoConfigMethod>(&RepoConfigParams { repo, task })?
        }
        SessionRepoCmd::Trust {
            repo,
            hash,
            revoke,
            task,
        } => c.call::<RepoSet>(&RepoSetParams {
            repo,
            task,
            trust: Some(TrustParams {
                hash,
                approve: !revoke,
            }),
            ..Default::default()
        })?,
        SessionRepoCmd::Set {
            repo,
            worktrees_root,
            setup,
            archive,
            clear_personal,
        } => {
            let personal = if setup.is_some() || archive.is_some() {
                let cur = c.call::<RepoConfigMethod>(&RepoConfigParams {
                    repo: repo.clone(),
                    task: None,
                })?;
                let mut p = cur.personal.unwrap_or_default();
                if let Some(s) = setup {
                    p.setup = Some(SetupSection {
                        script: Some(s),
                        mode: None,
                    });
                }
                if let Some(a) = archive {
                    p.archive = Some(ArchiveSection { script: Some(a) });
                }
                Some(p)
            } else {
                None
            };
            c.call::<RepoSet>(&RepoSetParams {
                repo,
                worktrees_root,
                personal,
                clear_personal,
                ..Default::default()
            })?
        }
    };
    if json {
        print_json(&r);
        return Ok(());
    }
    let kv = |k: &str, v: &str| println!("{k:<10} {v}");
    kv("repo", &r.repo);
    kv(
        "file",
        &format!("{}{}", r.path, if r.exists { "" } else { " (none)" }),
    );
    if let Some(e) = &r.error {
        kv("error", e);
    }
    if let Some(h) = &r.hash {
        kv("hash", h);
        kv(
            "approved",
            if r.trusted {
                "yes"
            } else {
                "no — review the commands, then: pod repo trust <repo> <hash>"
            },
        );
    }
    kv(
        "mode",
        match r.setup_mode {
            SetupMode::Parallel => "parallel",
            SetupMode::Wait => "wait",
        },
    );
    if let Some(w) = &r.worktrees_root {
        kv("worktrees", w);
    }
    for cmd in &r.commands {
        println!("  [{}] {} ({})", cmd.kind, cmd.command, cmd.source);
    }
    Ok(())
}

fn print_resume(r: &TaskResumeResult, json: bool) {
    if json {
        print_json(r);
        return;
    }
    match (&r.terminal, &r.resumed_session) {
        (Some(t), Some(s)) => println!(
            "{}: resumed session {s} in {} ({})",
            r.task,
            t.id,
            r.argv.join(" ")
        ),
        (Some(t), None) => println!(
            "{}: agent started in {} ({})",
            r.task,
            t.id,
            r.argv.join(" ")
        ),
        (None, _) => println!("{}: active", r.task),
    }
}

fn tier(t: CleanupTier) -> &'static str {
    match t {
        CleanupTier::Protected => "protected",
        CleanupTier::Review => "review",
        CleanupTier::Ready => "ready",
    }
}

fn list(v: &[String]) -> String {
    if v.is_empty() {
        "none".into()
    } else {
        v.join(", ")
    }
}

/// `--brief TEXT` or `--brief-file F` (`-` = stdin).
pub fn read_brief(text: Option<String>, file: Option<PathBuf>) -> anyhow::Result<Option<String>> {
    match (text, file) {
        (Some(_), Some(_)) => anyhow::bail!("give --brief or --brief-file, not both"),
        (Some(t), None) => Ok(Some(t)),
        (None, Some(f)) if f.as_os_str() == "-" => {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)
                .context("reading the brief from stdin")?;
            Ok(Some(s))
        }
        (None, Some(f)) => Ok(Some(
            std::fs::read_to_string(&f).with_context(|| format!("reading {}", f.display()))?,
        )),
        (None, None) => Ok(None),
    }
}

/// Extra lines for `pod task show`.
pub fn print_session_fields(t: &TaskInfo) {
    let kv = |k: &str, v: &str| println!("{k:<10} {v}");
    if t.lifecycle == Lifecycle::Archived {
        kv("lifecycle", "archived");
    }
    if let Some(s) = &t.setup {
        let mut v = format!("{:?}", s.state).to_lowercase();
        if let Some(c) = s.exit_code {
            v.push_str(&format!(" (exit {c})"));
        }
        if let Some(h) = &s.hash {
            v.push_str(&format!(" — approve with: pod repo trust {} {h}", t.repo));
        }
        if let Some(e) = &s.error {
            v.push_str(&format!(" — {e}"));
        }
        kv("setup", &v);
    }
    if let Some(b) = &t.brief {
        let how = format!("{:?}, {:?}", b.delivery, b.state).to_lowercase();
        kv("brief", &format!("{} ({how})", b.path));
    }
    if let Some(s) = &t.agent_session {
        kv(
            "session",
            &format!("{} {} (from {})", s.agent, s.id, s.source),
        );
    }
}
