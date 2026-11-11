//! `pod` — CLI for the podd daemon. Used by people and by agents running
//! inside pod terminals (`POD_TERMINAL_ID` / `POD_SOCKET` are set there).
//!
//! Exit codes: 0 ok, 1 wait timed out / condition not met (also `server stop`
//! when podd has not exited in time), 2 error, 3 daemon not running
//! (`server status|stop` only).
//!
//! Layout (docs/20-refactor/daemon.md §2): `args` holds the clap
//! definitions, `context` the pipe, `--machine`, `--json` and the
//! connection policy, `output` the shared printing, `commands` the
//! terminal, repo, task and server commands; the other modules are the
//! specialised commands. `main` parses, routes and exits.

mod args;
mod attach;
mod commands;
mod context;
mod guide;
mod hooks_install;
mod machine;
mod mobile;
mod orch;
mod orch_run;
mod output;
mod pr;
mod reconnect;
mod remote_stdio;
mod session;
mod sessions;
mod status;

use std::process::ExitCode;

use clap::Parser;

use args::Cmd;
use context::CliContext;

// Root names the command modules use (`crate::print_json`, ...).
pub(crate) use args::Cli;
pub(crate) use context::caller_terminal;
pub(crate) use output::{print_json, print_rows, print_suggestion, short_sha};

fn main() -> ExitCode {
    // The command enum and `real_main`'s match are large; debug builds
    // overflowed Windows' 1 MiB main-thread stack. Run on a bigger one.
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(run)
        .expect("spawn the main thread")
        .join()
        .unwrap_or(ExitCode::from(101))
}

fn run() -> ExitCode {
    let cli = Cli::parse();
    match real_main(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("pod: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn real_main(cli: Cli) -> anyhow::Result<ExitCode> {
    let Cli {
        json,
        pipe,
        machine,
        cmd,
    } = cli;
    let ctx = CliContext::new(json, pipe.as_deref(), machine, &cmd)?;
    let pipe = ctx.pipe.as_str();
    match cmd {
        // Guides describe this binary; they never need a daemon.
        Cmd::Guide { cmd } => guide::run(cmd),
        Cmd::RemoteStdio => Ok(remote_stdio::run(pipe)),
        Cmd::Machine { cmd } => machine::run(cmd, json),
        Cmd::Hooks { cmd } => status::hooks(cmd, json),
        Cmd::Server { cmd } => commands::server::run(cmd, &ctx),
        Cmd::Repo { cmd } => commands::repo::run(cmd, &ctx),
        Cmd::Task { cmd } => commands::task::run(cmd, &ctx),
        Cmd::Review { cmd } => orch::review(cmd, &mut ctx.connect()?, json),
        Cmd::Group { cmd } => orch::group(cmd, &mut ctx.connect()?, pipe, json),
        Cmd::Gate { cmd } => orch::gate(cmd, &mut ctx.connect()?, pipe, json),
        Cmd::Orch { cmd } => orch_run::run(cmd, &mut ctx.connect()?, pipe, json),
        Cmd::Pr { cmd } => pr::run(cmd, &mut ctx.connect()?, json),
        Cmd::Sessions { cmd } => sessions::run(cmd, &mut ctx.connect()?, json),
        Cmd::Mobile { cmd } => mobile::run(cmd, &mut ctx.connect()?, json),
        cmd @ (Cmd::Run { .. }
        | Cmd::Ls { .. }
        | Cmd::Keep { .. }
        | Cmd::Send { .. }
        | Cmd::Keys { .. }
        | Cmd::Read { .. }
        | Cmd::Wait { .. }
        | Cmd::Resize { .. }
        | Cmd::Attach { .. }
        | Cmd::Status { .. }
        | Cmd::Ack { .. }
        | Cmd::Kill { .. }
        | Cmd::Rm { .. }) => commands::terminal::run(cmd, &ctx),
    }
}
