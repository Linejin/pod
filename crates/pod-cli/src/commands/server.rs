//! `pod server`: status, stop and schema. Status and stop never start podd.

use std::process::ExitCode;

use podd::client::ClientError;
use podd::protocol::*;

use crate::args::ServerCmd;
use crate::context::CliContext;
use crate::output::print_json;

pub(crate) fn run(cmd: ServerCmd, ctx: &CliContext) -> anyhow::Result<ExitCode> {
    let (json, pipe) = (ctx.json, ctx.pipe.as_str());
    match cmd {
        ServerCmd::Schema => {
            println!(
                "{}",
                serde_json::to_string_pretty(&podd::protocol::schema())?
            );
        }
        ServerCmd::Status if ctx.is_remote() => {
            let s = ctx.connect()?.call::<ServerStatus>(&Empty {})?;
            print_server_status(&s, json);
        }
        cmd => {
            // Never auto-start for status/stop.
            let mut c = match ctx.connect_running() {
                Ok(c) => c,
                Err(ClientError::NotRunning) => {
                    if json {
                        println!("{}", serde_json::json!({"running": false, "pipe": pipe}));
                    } else {
                        println!("podd is not running ({pipe})");
                    }
                    return Ok(ExitCode::from(3));
                }
                Err(e) => return Err(e.into()),
            };
            match cmd {
                ServerCmd::Status => {
                    let s = c.call::<ServerStatus>(&Empty {})?;
                    print_server_status(&s, json);
                }
                ServerCmd::Stop { kill } => {
                    // Watch the process before asking it to stop: we return
                    // only once it has exited (pipe and state files released),
                    // so the next `pod` command starts a fresh daemon.
                    let pid = c.call::<ServerStatus>(&Empty {})?.pid;
                    let watch = podd::client::ProcessWatch::open(pid);
                    let s = c.call::<ServerStop>(&StopParams { kill })?;
                    drop(c);
                    let exited = watch.is_none_or(|w| w.wait(stop_timeout()));
                    if json {
                        let mut v = serde_json::to_value(&s)?;
                        v["stopped"] = exited.into();
                        v["pid"] = pid.into();
                        print_json(&v);
                    } else if exited && kill {
                        println!("podd stopped (killed {} running terminal(s))", s.killed);
                    } else if exited {
                        println!(
                            "podd stopped ({} running terminal(s) stay alive in their holders)",
                            s.detached
                        );
                    }
                    if !exited {
                        eprintln!(
                            "pod: podd (pid {pid}) is still running {} s after server.stop;                              it may still be ending terminals. Check again with `pod server status`",
                            stop_timeout().as_secs()
                        );
                        return Ok(ExitCode::from(1));
                    }
                }
                ServerCmd::Schema => unreachable!(),
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn print_server_status(s: &ServerStatusResult, json: bool) {
    if json {
        print_json(s);
    } else {
        println!(
            "podd {} running: pid {}, pipe {}, up {}, terminals {} ({} running)",
            s.version,
            s.pid,
            s.pipe,
            fmt_duration(s.uptime_ms),
            s.terminals,
            s.running
        );
    }
}

/// How long `pod server stop` waits for the daemon to exit
/// (`POD_STOP_TIMEOUT` seconds, default 20: up to 5 s ending terminals with
/// `--kill`, 3 s for requests in flight).
fn stop_timeout() -> std::time::Duration {
    std::env::var("POD_STOP_TIMEOUT")
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(std::time::Duration::from_secs_f64)
        .unwrap_or(std::time::Duration::from_secs(20))
}

fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{}s", s / 60, s % 60),
        _ => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}
