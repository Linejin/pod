//! `pod repo`: register, list and unregister repositories (the session
//! subcommands are in `session`).

use std::process::ExitCode;

use podd::protocol::*;

use crate::args::RepoCmd;
use crate::context::CliContext;
use crate::output::{print_json, print_rows};
use crate::session;

pub(crate) fn run(cmd: RepoCmd, ctx: &CliContext) -> anyhow::Result<ExitCode> {
    let c = &mut ctx.connect()?;
    let (json, remote) = (ctx.json, ctx.is_remote());
    match cmd {
        RepoCmd::Session(cmd) => session::repo_cmd(cmd, c, json)?,
        RepoCmd::Add { path, name } => {
            let path = if remote {
                path
            } else {
                std::path::absolute(path)?
            };
            let r = c.call::<RepoAdd>(&RepoAddParams {
                path: path.display().to_string(),
                name,
            })?;
            if json {
                print_json(&r);
            } else {
                println!(
                    "{} {} (default base {})",
                    r.name,
                    r.path,
                    r.default_base.as_deref().unwrap_or("?")
                );
            }
        }
        RepoCmd::Ls => {
            let l = c.call::<RepoList>(&Empty {})?;
            if json {
                print_json(&l);
            } else if l.repos.is_empty() {
                println!("no repositories (pod repo add <path>)");
            } else {
                let rows = l
                    .repos
                    .iter()
                    .map(|r| {
                        vec![
                            r.name.clone(),
                            r.tasks.to_string(),
                            r.default_base.clone().unwrap_or_else(|| "-".into()),
                            r.path.clone(),
                        ]
                    })
                    .collect();
                print_rows(&["NAME", "TASKS", "BASE", "PATH"], rows);
            }
        }
        RepoCmd::Rm { repo } => {
            let r = c.call::<RepoRemove>(&RepoRef { repo })?;
            if json {
                print_json(&r);
            } else {
                println!("{} unregistered", r.name);
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
