//! `pod task`: create, list, show, diff, touch and remove tasks (the
//! session subcommands are in `session`).

use std::process::ExitCode;

use podd::protocol::*;

use crate::args::{AgentArg, SetupArg, TaskCmd};
use crate::context::CliContext;
use crate::output::{print_json, print_rows, short_sha, state_word};
use crate::{session, status};

pub(crate) fn run(cmd: TaskCmd, ctx: &CliContext) -> anyhow::Result<ExitCode> {
    let c = &mut ctx.connect()?;
    let json = ctx.json;
    match cmd {
        TaskCmd::Session(cmd) => session::task_cmd(cmd, c, json)?,
        TaskCmd::New {
            repo,
            name,
            branch,
            brief,
            brief_file,
            setup,
            wait_setup,
            no_focus,
            base,
            agent,
            prompt,
            cols,
            rows,
            argv,
        } => {
            let brief = session::read_brief(brief, brief_file)?;
            let parent = ctx.caller_terminal();
            let open_hint = no_focus.then(|| OpenHint {
                focus: false,
                reason: if parent.is_some() {
                    OpenReason::Worker
                } else {
                    OpenReason::User
                },
            });
            let agent = if !argv.is_empty() {
                Some(AgentSpec::Argv { argv })
            } else {
                agent.map(|a| {
                    AgentSpec::Kind(match a {
                        AgentArg::Claude => AgentKind::Claude,
                        AgentArg::Codex => AgentKind::Codex,
                        AgentArg::Shell => AgentKind::Shell,
                    })
                })
            };
            let t = c.call::<TaskCreate>(&TaskCreateParams {
                repo,
                name: name.unwrap_or_default(),
                base,
                agent,
                prompt,
                parent_terminal: parent,
                cols,
                rows,
                branch,
                brief,
                setup: setup.map(|s| match s {
                    SetupArg::Run => SetupPolicy::Run,
                    SetupArg::Skip => SetupPolicy::Skip,
                    SetupArg::Inherit => SetupPolicy::Inherit,
                }),
                setup_mode: wait_setup.then_some(SetupMode::Wait),
                open_hint,
                ..Default::default()
            })?;
            if json {
                print_json(&t);
            } else {
                let term = t.terminals.first().map(|x| x.id.as_str()).unwrap_or("-");
                println!("{}  {}  {}  {}", t.id, term, t.branch, t.worktree);
            }
        }
        TaskCmd::Ls { repo } => {
            let l = c.call::<TaskList>(&TaskListParams { repo })?;
            if json {
                print_json(&l);
            } else if l.tasks.is_empty() {
                println!("no tasks");
            } else {
                let rows = l.tasks.iter().map(task_row).collect();
                print_rows(
                    &[
                        "TASK",
                        "STATUS",
                        "TERMINALS",
                        "BRANCH",
                        "AHEAD",
                        "BEHIND",
                        "DIRTY",
                        "LAST COMMIT",
                    ],
                    rows,
                );
            }
        }
        TaskCmd::Show { task } => {
            let t = c.call::<TaskShow>(&TaskRef { task })?;
            if json {
                print_json(&t);
            } else {
                print_task(&t);
            }
        }
        TaskCmd::Touch { task, paths } => {
            let r = c.call::<TaskTouch>(&TaskTouchParams { task, paths })?;
            if json {
                print_json(&r);
            } else {
                match (&r.git, &r.git_error) {
                    (Some(g), _) => println!(
                        "{}: ahead {} behind {} dirty {} unpushed {}{}",
                        r.task,
                        g.ahead,
                        g.behind,
                        g.dirty,
                        g.unpushed,
                        if r.git_changed { " (changed)" } else { "" }
                    ),
                    (None, Some(e)) => println!("{}: git: {e}", r.task),
                    (None, None) => println!("{}", r.task),
                }
            }
        }
        TaskCmd::Diff { task } => {
            let d = c.call::<TaskDiff>(&TaskRef { task })?;
            if json {
                print_json(&d);
            } else {
                let n = |v: Option<u64>| v.map(|x| x.to_string()).unwrap_or_else(|| "-".into());
                let rows = d
                    .files
                    .iter()
                    .map(|f| {
                        let path = match &f.old_path {
                            Some(o) => format!("{o} -> {}", f.path),
                            None => f.path.clone(),
                        };
                        vec![
                            f.status.clone(),
                            format!("+{}", n(f.added)),
                            format!("-{}", n(f.deleted)),
                            path,
                        ]
                    })
                    .collect();
                print_rows(&["ST", "ADDED", "DELETED", "PATH"], rows);
                println!(
                    "{} file(s), +{} -{} against {} ({})",
                    d.files.len(),
                    d.added,
                    d.deleted,
                    d.base,
                    short_sha(&d.merge_base)
                );
            }
        }
        TaskCmd::Rm {
            task,
            force,
            delete_branch,
        } => {
            let r = c.call::<TaskRemove>(&TaskRemoveParams {
                task,
                force,
                delete_branch,
            })?;
            if json {
                print_json(&r);
            } else {
                println!(
                    "{} removed (terminals: {}; worktree {}; branch {})",
                    r.id,
                    if r.terminals.is_empty() {
                        "none".into()
                    } else {
                        r.terminals.join(", ")
                    },
                    if r.worktree_removed {
                        "removed"
                    } else {
                        "not found"
                    },
                    if r.branch_deleted { "deleted" } else { "kept" }
                );
                if let Some(k) = &r.branch_kept {
                    println!("  {k}");
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn task_terminals(t: &TaskInfo) -> String {
    let parts: Vec<String> = t
        .terminals
        .iter()
        .map(|x| match &x.info {
            Some(i) => format!("{}:{}", x.id, state_word(i)),
            None => format!("{}:gone", x.id),
        })
        .collect();
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(",")
    }
}

fn task_row(t: &TaskInfo) -> Vec<String> {
    let (ahead, behind, dirty, last) = match &t.git {
        Some(g) => (
            g.ahead.to_string(),
            g.behind.to_string(),
            g.dirty.to_string(),
            g.last_commit
                .as_ref()
                .map(|c| format!("{} {}", short_sha(&c.sha), c.summary))
                .unwrap_or_default(),
        ),
        None => (
            "?".into(),
            "?".into(),
            "?".into(),
            t.git_error.clone().unwrap_or_default(),
        ),
    };
    vec![
        t.id.clone(),
        status::task_status_cell(t),
        task_terminals(t),
        t.branch.clone(),
        ahead,
        behind,
        dirty,
        last,
    ]
}

fn print_task(t: &TaskInfo) {
    let kv = |k: &str, v: &str| println!("{k:<10} {v}");
    kv("task", &t.id);
    kv("agent", &t.agent.to_string());
    if let Some(p) = &t.prompt {
        kv("prompt", p);
    }
    kv("branch", &t.branch);
    kv(
        "base",
        &format!("{} (created from {})", t.base, short_sha(&t.base_commit)),
    );
    kv(
        "worktree",
        &format!(
            "{}{}",
            t.worktree,
            if t.worktree_exists { "" } else { " (missing)" }
        ),
    );
    kv("terminals", &task_terminals(t));
    kv("status", &status::task_status_cell(t));
    session::print_session_fields(t);
    if t.parent_task.is_some() || t.parent_terminal.is_some() {
        kv(
            "parent",
            &format!(
                "{} / {}",
                t.parent_task.as_deref().unwrap_or("-"),
                t.parent_terminal.as_deref().unwrap_or("-")
            ),
        );
    }
    match &t.git {
        Some(g) => {
            kv(
                "status",
                &format!(
                    "{} ahead, {} behind, {} dirty, {} unpushed",
                    g.ahead, g.behind, g.dirty, g.unpushed
                ),
            );
            if let Some(c) = &g.last_commit {
                kv(
                    "last",
                    &format!("{} {} ({})", short_sha(&c.sha), c.summary, c.author),
                );
            }
        }
        None => kv("git", t.git_error.as_deref().unwrap_or("unavailable")),
    }
}
