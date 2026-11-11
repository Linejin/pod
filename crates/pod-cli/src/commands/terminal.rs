//! Terminal commands: run, ls, keep, send, keys, read, wait, resize, attach,
//! status, ack, kill, rm.

use std::collections::BTreeMap;
use std::process::ExitCode;

use anyhow::bail;
use podd::protocol::*;

use crate::args::Cmd;
use crate::context::CliContext;
use crate::output::{code_suffix, print_json, print_suggestion, state_word};
use crate::{attach, orch, reconnect, status};

/// Runs one of the terminal commands of [`Cmd`] (main routes only those here).
pub(crate) fn run(cmd: Cmd, ctx: &CliContext) -> anyhow::Result<ExitCode> {
    let json = ctx.json;
    match cmd {
        Cmd::Run {
            name,
            cwd,
            task,
            env,
            inherit_env,
            cols,
            rows,
            argv,
        } => {
            // Paths are the endpoint's: a remote cwd is passed as given,
            // and the local current directory means nothing there.
            let cwd = match cwd {
                Some(c) if ctx.is_remote() => Some(c),
                Some(c) => Some(std::path::absolute(c)?),
                None if task.is_some() || ctx.is_remote() => None,
                None => Some(std::env::current_dir()?),
            };
            let mut vars: BTreeMap<String, String> = if inherit_env {
                podd::env::sanitize(std::env::vars())
            } else {
                BTreeMap::new()
            };
            for kv in &env {
                match kv.split_once('=') {
                    Some((k, v)) if !k.is_empty() => podd::env::set_var(&mut vars, k, v),
                    _ => bail!("--env expects KEY=VALUE, got {kv:?}"),
                }
            }
            let params = RunParams {
                argv,
                cwd: cwd.map(|c| c.display().to_string()),
                name,
                env: vars,
                cols,
                rows,
                task,
                // Inside a pod terminal: record it as the new terminal's parent.
                parent_terminal: ctx.caller_terminal(),
            };
            let info = ctx.connect()?.call::<TerminalRun>(&params)?;
            if json {
                print_json(&info);
            } else {
                println!("{}", info.id);
            }
        }
        Cmd::Ls { all } => {
            let mut list = ctx.connect()?.call::<TerminalList>(&Empty {})?;
            let total = list.terminals.len();
            if !all {
                // Incompatible and unreachable holders may still own live
                // processes: always shown.
                list.terminals.retain(|t| {
                    t.state != TerminalState::Exited || t.incompatible_holder.is_some()
                });
                let ids: Vec<&str> = list.terminals.iter().map(|t| t.id.as_str()).collect();
                list.retention.retain(|r| ids.contains(&r.id.as_str()));
            }
            let hidden = total - list.terminals.len();
            if json {
                print_json(&list);
            } else {
                print_table(&list.terminals);
                if hidden > 0 {
                    println!("({hidden} exited terminal(s) hidden; pod ls --all)");
                }
            }
        }
        Cmd::Keep { terminal, unkeep } => {
            let r = ctx.connect()?.call::<TerminalKeep>(&KeepParams {
                target: TerminalRef::parse(&terminal),
                keep: !unkeep,
            })?;
            if json {
                print_json(&r);
            } else if r.kept {
                println!("{} kept", r.id);
            } else {
                println!("{} follows the retention policy", r.id);
            }
        }
        Cmd::Send {
            terminal,
            text,
            no_enter,
            no_confirm,
        } => {
            let r = ctx.connect()?.call::<TerminalSend>(&SendParams {
                target: TerminalRef::parse(&terminal),
                text,
                enter: !no_enter,
                confirm: !no_confirm,
            })?;
            if json {
                print_json(&r);
            } else if let Some(s) = r
                .submit
                .as_ref()
                .filter(|s| s.state == SubmitState::Unconfirmed)
            {
                eprintln!(
                    "pod: warning: {} did not start a turn after Enter (pressed {} times); \
                     the text may still be in its input box",
                    r.id, s.enters
                );
            }
        }
        Cmd::Keys { terminal, keys } => {
            let r = ctx.connect()?.call::<TerminalKeys>(&KeysParams {
                target: TerminalRef::parse(&terminal),
                keys,
            })?;
            if json {
                print_json(&r);
            }
        }
        Cmd::Read {
            terminal,
            history,
            lines,
            raw,
        } => {
            let mode = if history {
                ReadMode::History
            } else {
                ReadMode::Screen
            };
            let r = ctx.connect()?.call::<TerminalRead>(&ReadParams {
                target: TerminalRef::parse(&terminal),
                mode,
                lines,
                raw,
            })?;
            if json {
                print_json(&r);
            } else {
                for l in &r.lines {
                    println!("{l}");
                }
                print_suggestion(r.suggestion.as_deref());
            }
        }
        Cmd::Wait {
            gate: Some(gate),
            timeout,
            ..
        } => {
            let timeout_ms = timeout.map(|s| (s.max(0.0) * 1000.0) as u64);
            return orch::wait_gate(&mut ctx.connect()?, gate, timeout_ms, json);
        }
        Cmd::Wait {
            terminal,
            status: wait_status,
            turn_end,
            timeout,
            ..
        } if wait_status.is_some() || turn_end => {
            let timeout_ms = timeout.map(|s| (s.max(0.0) * 1000.0) as u64);
            return status::wait(
                &mut ctx.connect()?,
                &terminal.unwrap_or_default(),
                wait_status,
                turn_end,
                timeout_ms,
                json,
            );
        }
        Cmd::Status { terminal } => {
            let t = ctx
                .connect()?
                .call::<TerminalShow>(&TerminalRef::parse(&terminal))?;
            if json {
                print_json(&t.status);
            } else {
                status::print_status(&t);
            }
        }
        Cmd::Ack { terminal } => {
            let t = ctx
                .connect()?
                .call::<TerminalAck>(&TerminalRef::parse(&terminal))?;
            if json {
                print_json(&t.status);
            } else {
                println!("{} {}", t.id, status::status_cell(&t));
            }
        }
        Cmd::Wait {
            terminal,
            exit: _,
            output,
            timeout,
            ..
        } => {
            let until = match output {
                Some(s) => WaitUntil::OutputContains(s),
                None => WaitUntil::Exit,
            };
            let timeout_ms = timeout.map(|s| (s.max(0.0) * 1000.0) as u64);
            let terminal = terminal.unwrap_or_default();
            // Survives a daemon restart: terminals outlive podd (docs/12 B6).
            let r = reconnect::wait(
                &mut ctx.connect()?,
                timeout_ms,
                &terminal,
                |c, timeout_ms| {
                    c.call::<TerminalWait>(&WaitParams {
                        target: TerminalRef::parse(&terminal),
                        until: until.clone(),
                        timeout_ms,
                    })
                },
            )?;
            if json {
                print_json(&r);
            } else {
                let t = &r.terminal;
                match r.outcome {
                    WaitOutcome::Matched if t.state == TerminalState::Exited => {
                        println!("{} exited{}", t.id, code_suffix(t.exit_code))
                    }
                    WaitOutcome::Matched => println!("{} matched", t.id),
                    WaitOutcome::TimedOut => {
                        println!("{} timed out after {} ms", t.id, r.waited_ms)
                    }
                    WaitOutcome::Exited => println!(
                        "{} exited{} without matching",
                        t.id,
                        code_suffix(t.exit_code)
                    ),
                }
            }
            if r.outcome != WaitOutcome::Matched {
                return Ok(ExitCode::from(1));
            }
        }
        Cmd::Resize {
            terminal,
            cols,
            rows,
        } => {
            let t = ctx.connect()?.call::<TerminalResize>(&ResizeParams {
                target: TerminalRef::parse(&terminal),
                cols,
                rows,
            })?;
            if json {
                print_json(&t);
            } else {
                println!("{} {}x{}", t.id, t.cols, t.rows);
            }
        }
        Cmd::Attach {
            terminal,
            scrollback,
            no_resize,
        } => {
            let code = attach::run(
                &ctx.pipe,
                attach::AttachOptions {
                    terminal,
                    scrollback,
                    take_size: !no_resize,
                },
            )?;
            return Ok(ExitCode::from(code as u8));
        }
        Cmd::Kill { terminal } => {
            let t = ctx
                .connect()?
                .call::<TerminalKill>(&TerminalRef::parse(&terminal))?;
            if json {
                print_json(&t);
            } else {
                println!("{} {}", t.id, state_word(&t));
            }
        }
        Cmd::Rm { terminal, force } => {
            let r = ctx.connect()?.call::<TerminalRm>(&RmParams {
                target: TerminalRef::parse(&terminal),
                force,
            })?;
            if json {
                print_json(&r);
            } else {
                println!("{} removed", r.id);
            }
        }
        _ => unreachable!("not a terminal command"),
    }
    Ok(ExitCode::SUCCESS)
}

fn print_table(terms: &[TerminalInfo]) {
    if terms.is_empty() {
        println!("no terminals");
        return;
    }
    let rows: Vec<[String; 8]> = terms
        .iter()
        .map(|t| {
            [
                t.id.clone(),
                t.name.clone().unwrap_or_else(|| "-".into()),
                state_word(t),
                status::status_cell(t),
                t.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into()),
                format!("{}x{}", t.cols, t.rows),
                // An unreachable holder has no known command: say why instead.
                match &t.unreachable {
                    Some(why) if t.state == TerminalState::Unreachable => why.clone(),
                    _ => t.argv.join(" "),
                },
                t.cwd.clone(),
            ]
        })
        .collect();
    let header = [
        "ID", "NAME", "STATE", "STATUS", "PID", "SIZE", "COMMAND", "CWD",
    ];
    let mut w = header.map(str::len);
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |cells: [&str; 8]| {
        let mut s = String::new();
        for (i, c) in cells.iter().enumerate() {
            if i == cells.len() - 1 {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<width$}  ", width = w[i]));
            }
        }
        println!("{}", s.trim_end());
    };
    line(header);
    for r in &rows {
        line([&r[0], &r[1], &r[2], &r[3], &r[4], &r[5], &r[6], &r[7]]);
    }
}
