//! `pod` — CLI for the podd daemon. Used by people and by agents running
//! inside pod terminals (`POD_TERMINAL_ID` / `POD_SOCKET` are set there).
//!
//! Exit codes: 0 ok, 1 wait timed out / condition not met (also `server stop`
//! when podd has not exited in time), 2 error, 3 daemon not running
//! (`server status|stop` only).

mod attach;
mod guide;
mod hooks_install;
mod machine;
mod mobile;
mod orch;
mod orch_run;
mod pr;
mod reconnect;
mod remote_stdio;
mod session;
mod sessions;
mod status;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use podd::client::{Client, ClientError};
use podd::endpoint;
use podd::protocol::*;

#[derive(Parser)]
#[command(
    name = "pod",
    version,
    about = "Drive terminals owned by the podd daemon"
)]
struct Cli {
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    json: bool,
    /// Pipe name or path (default: $POD_SOCKET, else \\.\pipe\podd-<username>).
    #[arg(long, global = true, value_name = "PIPE")]
    pipe: Option<String>,
    /// Run the command on this SSH machine profile (`pod machine add`)
    /// instead of the local daemon.
    #[arg(long, global = true, value_name = "NAME", conflicts_with = "pipe")]
    machine: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a program in a new terminal (kept alive by a holder process) and print its id.
    Run {
        /// Unique name (among running terminals) usable instead of the id.
        #[arg(long)]
        name: Option<String>,
        /// Working directory (default: the task's worktree with --task, else
        /// the current directory).
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Add the terminal to this task (<repo>/<name> or unique name); sets POD_TASK_ID.
        #[arg(long)]
        task: Option<String>,
        /// Extra environment variable, KEY=VALUE (repeatable). Applied last.
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Pass this shell's environment (minus launcher markers such as
        /// NO_COLOR, CLAUDECODE, parent-IDE variables) instead of the clean user environment.
        #[arg(long)]
        inherit_env: bool,
        #[arg(long)]
        cols: Option<u16>,
        #[arg(long)]
        rows: Option<u16>,
        /// Program and arguments.
        #[arg(last = true, required = true, value_name = "ARGV")]
        argv: Vec<String>,
    },
    /// List terminals (exited ones only with --all).
    Ls {
        /// Also show exited terminals.
        #[arg(long, short)]
        all: bool,
    },
    /// Keep an exited terminal from being removed by the retention policy
    /// (--unkeep returns it to the policy).
    Keep {
        terminal: String,
        #[arg(long)]
        unkeep: bool,
    },
    /// Type text into a terminal, followed by Enter unless --no-enter.
    /// Enter is pressed once the program has taken the text in; for an
    /// agent the daemon then checks that a turn started and presses Enter
    /// once more if not (a warning is printed when it still did not).
    Send {
        /// Terminal id (t3) or name.
        terminal: String,
        text: String,
        #[arg(long)]
        no_enter: bool,
        /// Press Enter once, without checking that the agent took the text.
        #[arg(long, conflicts_with = "no_enter")]
        no_confirm: bool,
    },
    /// Press named keys: enter, tab, esc, up, ctrl+c, alt+x, f5, ... or single characters.
    Keys {
        terminal: String,
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Print the screen (or scrollback + screen with --history).
    Read {
        terminal: String,
        #[arg(long)]
        history: bool,
        /// Only the last N lines.
        #[arg(long)]
        lines: Option<usize>,
        /// Keep a grey prompt suggestion at the caret (Claude's suggested
        /// prompt, PSReadLine's prediction) in the text. By default it is
        /// left out and named on a separate line: nobody typed it.
        #[arg(long)]
        raw: bool,
    },
    /// Wait until the program exits (default) or the output contains TEXT
    /// (the echo of text sent with `pod send` does not count), or for an
    /// agent status (--status, --turn-end).
    Wait {
        /// Terminal id or name (not with --gate).
        #[arg(required_unless_present = "gate")]
        terminal: Option<String>,
        /// Wait until this gate is resolved: exit 0 approved, 1 rejected or timed out.
        #[arg(long, value_name = "ID", conflicts_with_all = ["terminal", "exit", "output", "status", "turn_end"])]
        gate: Option<String>,
        #[arg(long, conflicts_with_all = ["output", "status", "turn_end"])]
        exit: bool,
        #[arg(long, value_name = "TEXT", conflicts_with_all = ["status", "turn_end"])]
        output: Option<String>,
        /// Wait for this agent status (returns at once if already in it);
        /// any-change waits for the next state change.
        #[arg(long, value_enum)]
        status: Option<status::WaitStatus>,
        /// Wait until the current (or next) turn ends: the turn counter
        /// advances and the agent is no longer working.
        #[arg(long)]
        turn_end: bool,
        /// Give up after S seconds (fractions allowed); exit code 1.
        #[arg(long, value_name = "S")]
        timeout: Option<f64>,
    },
    /// Resize a terminal (the most recent resize wins, also over attached clients).
    Resize {
        terminal: String,
        cols: u16,
        rows: u16,
    },
    /// Attach this console to a terminal: raw input and output, Ctrl+] detaches.
    Attach {
        terminal: String,
        /// Scrollback lines replayed on attach.
        #[arg(long, default_value_t = 1000)]
        scrollback: usize,
        /// Do not resize the terminal to this console.
        #[arg(long)]
        no_resize: bool,
    },
    /// Show a terminal's agent status.
    Status { terminal: String },
    /// Acknowledge a terminal (the user saw it): done becomes idle.
    Ack { terminal: String },
    /// Agent hook configuration.
    Hooks {
        #[command(subcommand)]
        cmd: status::HooksCmd,
    },
    /// Kill a terminal's whole process tree.
    Kill { terminal: String },
    /// Forget an exited terminal (ends its holder). --force kills it first.
    Rm {
        terminal: String,
        #[arg(long)]
        force: bool,
    },
    /// Registered git repositories (tasks are created in them).
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// Tasks: a git worktree on branch pod/<name> plus the terminal(s) working in it.
    Task {
        #[command(subcommand)]
        cmd: TaskCmd,
    },
    /// Review comments on a task's diff, sent to its agent (P4).
    Review {
        #[command(subcommand)]
        cmd: orch::ReviewCmd,
    },
    /// Fan-out groups: one prompt, several agents, compare and pick (P5).
    Group {
        #[command(subcommand)]
        cmd: orch::GroupCmd,
    },
    /// Orchestration for agents: runs, items, workers, mail (docs/19).
    Orch {
        #[command(subcommand)]
        cmd: orch_run::OrchCmd,
    },
    /// Human approval gates (P5). Agents call `pod gate request` from their terminal.
    Gate {
        #[command(subcommand)]
        cmd: orch::GateCmd,
    },
    /// A task's GitHub pull request: push, create, status, merge, open (docs/15).
    Pr {
        #[command(subcommand)]
        cmd: pr::PrCmd,
    },
    /// Agent sessions found on this PC (Claude Code, Codex): list, resume, link (docs/18).
    Sessions {
        #[command(subcommand)]
        cmd: sessions::SessionsCmd,
    },
    /// Paired phones: permissions per task, revoke, audit (docs/17; no network listener yet).
    Mobile {
        #[command(subcommand)]
        cmd: mobile::MobileCmd,
    },
    /// Daemon control.
    Server {
        #[command(subcommand)]
        cmd: ServerCmd,
    },
    /// Bridge stdin/stdout to this user's podd for a remote client (`ssh -T host
    /// pod remote-stdio`). Starts the installed podd if none runs. stdout
    /// carries protocol lines only; exits when either side closes, leaving
    /// the daemon and its terminals running.
    RemoteStdio,
    /// SSH machine profiles for `pod --machine <name> …` (docs/17 R3).
    Machine {
        #[command(subcommand)]
        cmd: machine::MachineCmd,
    },
    /// Guides for agents in Pod terminals, matched to this version
    /// (`pod guide orchestration`); no topic: the list.
    Guide {
        #[command(subcommand)]
        cmd: Option<guide::GuideCmd>,
    },
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Register a repository (the root of a git work tree).
    Add {
        path: PathBuf,
        /// Registry name (default: the directory name).
        #[arg(long)]
        name: Option<String>,
    },
    /// List registered repositories.
    Ls,
    /// Unregister a repository (it must have no tasks). Nothing on disk changes.
    Rm {
        /// Name or path.
        repo: String,
    },
    #[command(flatten)]
    Session(session::SessionRepoCmd),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum AgentArg {
    Claude,
    Codex,
    Shell,
}

#[derive(Subcommand)]
enum TaskCmd {
    /// Create branch pod/<name> + a worktree, and start the agent in it.
    New {
        /// Repository name or path.
        repo: String,
        /// Task name (default: a suggested one).
        name: Option<String>,
        /// Branch instead of pod/<name> (an existing branch is checked out).
        #[arg(long)]
        branch: Option<String>,
        /// Task brief (any text): written to a file named by POD_BRIEF; the
        /// agent gets a short first line pointing at it.
        #[arg(long, value_name = "TEXT")]
        brief: Option<String>,
        /// Read the brief from a file (- = stdin).
        #[arg(long, value_name = "FILE")]
        brief_file: Option<PathBuf>,
        /// Setup: run (if approved), skip, or inherit the repository's (default).
        #[arg(long, value_enum)]
        setup: Option<SetupArg>,
        /// Type the brief only after setup finished.
        #[arg(long)]
        wait_setup: bool,
        /// Do not move the app's focus to the new task (a tab still opens).
        #[arg(long)]
        no_focus: bool,
        /// Starting point (default: the repository's default branch / HEAD).
        #[arg(long, value_name = "REF")]
        base: Option<String>,
        /// Agent to start (default: shell). Ignored when an argv follows `--`.
        #[arg(long, value_enum)]
        agent: Option<AgentArg>,
        /// Initial prompt for claude / codex.
        #[arg(long, value_name = "TEXT")]
        prompt: Option<String>,
        #[arg(long)]
        cols: Option<u16>,
        #[arg(long)]
        rows: Option<u16>,
        /// Run this program instead of a named agent.
        #[arg(last = true, value_name = "ARGV")]
        argv: Vec<String>,
    },
    /// List tasks with terminal state and git facts.
    Ls {
        /// Only this repository.
        #[arg(long)]
        repo: Option<String>,
    },
    /// Show one task.
    Show { task: String },
    /// Files changed against the base (committed, uncommitted and untracked).
    Diff { task: String },
    /// Tell the daemon files were saved in the task's worktree: git facts are
    /// refreshed now and `task.changed` events go out (editors, scripts).
    Touch {
        task: String,
        /// Saved paths (informational, passed on in the event).
        paths: Vec<String>,
    },
    /// Kill the task's terminals, remove the worktree (and the branch with --delete-branch).
    Rm {
        task: String,
        /// Remove even with uncommitted changes or unpushed commits.
        #[arg(long)]
        force: bool,
        /// Also delete the branch pod/<name>, when it is merged into the base.
        #[arg(long)]
        delete_branch: bool,
    },
    #[command(flatten)]
    Session(session::SessionTaskCmd),
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum SetupArg {
    Run,
    Skip,
    Inherit,
}

#[derive(Subcommand)]
enum ServerCmd {
    /// Show whether the daemon runs and what it holds (does not start it).
    Status,
    /// Stop the daemon. Terminals keep running in their holders unless --kill.
    Stop {
        #[arg(long)]
        kill: bool,
    },
    /// Print the protocol's JSON schema.
    Schema,
}

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

/// Why `cmd` cannot run with `--machine` (None: it can).
fn remote_unsupported(cmd: &Cmd) -> Option<&'static str> {
    match cmd {
        Cmd::Attach { .. } => Some("attach는 아직 원격을 지원하지 않습니다 (docs/17 R4)"),
        Cmd::RemoteStdio => Some("remote-stdio는 원격 쪽에서 ssh가 실행하는 명령입니다"),
        Cmd::Hooks { .. } => Some("hooks는 이 PC의 에이전트 설정만 다룹니다"),
        Cmd::Machine { .. } => Some("machine은 이 PC의 프로필만 다룹니다"),
        Cmd::Server {
            cmd: ServerCmd::Stop { .. },
        } => Some("원격 데몬 중지는 지원하지 않습니다"),
        Cmd::Run {
            inherit_env: true, ..
        } => Some("--inherit-env는 이 PC의 환경 변수를 원격에 보내므로 쓸 수 없습니다"),
        _ => None,
    }
}

fn real_main(cli: Cli) -> anyhow::Result<ExitCode> {
    let remote = cli.machine.clone();
    if let Some(name) = &remote
        && let Some(why) = remote_unsupported(&cli.cmd)
    {
        bail!("--machine {name}: {why}");
    }
    // Over SSH the label is `ssh:<name>`: never equal to a local pipe, so
    // the caller's own terminal is not sent as parent and waits do not
    // reconnect to a local pipe.
    let pipe = match &remote {
        Some(name) => format!("{}{name}", pod_remote::LABEL_PREFIX),
        None => cli
            .pipe
            .as_deref()
            .map(endpoint::pipe_path)
            .unwrap_or_else(endpoint::default_pipe),
    };
    let json = cli.json;
    let connect = || match &remote {
        Some(name) => machine::connect(name),
        None => Client::connect_or_start(&pipe, None)
            .with_context(|| format!("cannot reach podd on {pipe}")),
    };

    match cli.cmd {
        // Guides describe this binary; they never need a daemon.
        Cmd::Guide { cmd } => return guide::run(cmd),
        Cmd::RemoteStdio => return Ok(remote_stdio::run(&pipe)),
        Cmd::Machine { cmd } => return machine::run(cmd, json),
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
                Some(c) if remote.is_some() => Some(c),
                Some(c) => Some(std::path::absolute(c)?),
                None if task.is_some() || remote.is_some() => None,
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
                parent_terminal: caller_terminal(&pipe),
            };
            let info = connect()?.call::<TerminalRun>(&params)?;
            if json {
                print_json(&info);
            } else {
                println!("{}", info.id);
            }
        }
        Cmd::Ls { all } => {
            let mut list = connect()?.call::<TerminalList>(&Empty {})?;
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
            let r = connect()?.call::<TerminalKeep>(&KeepParams {
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
            let r = connect()?.call::<TerminalSend>(&SendParams {
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
            let r = connect()?.call::<TerminalKeys>(&KeysParams {
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
            let r = connect()?.call::<TerminalRead>(&ReadParams {
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
            return orch::wait_gate(&mut connect()?, gate, timeout_ms, json);
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
                &mut connect()?,
                &terminal.unwrap_or_default(),
                wait_status,
                turn_end,
                timeout_ms,
                json,
            );
        }
        Cmd::Status { terminal } => {
            let t = connect()?.call::<TerminalShow>(&TerminalRef::parse(&terminal))?;
            if json {
                print_json(&t.status);
            } else {
                status::print_status(&t);
            }
        }
        Cmd::Ack { terminal } => {
            let t = connect()?.call::<TerminalAck>(&TerminalRef::parse(&terminal))?;
            if json {
                print_json(&t.status);
            } else {
                println!("{} {}", t.id, status::status_cell(&t));
            }
        }
        Cmd::Hooks { cmd } => return status::hooks(cmd, json),
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
            let r = reconnect::wait(&mut connect()?, timeout_ms, &terminal, |c, timeout_ms| {
                c.call::<TerminalWait>(&WaitParams {
                    target: TerminalRef::parse(&terminal),
                    until: until.clone(),
                    timeout_ms,
                })
            })?;
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
            let t = connect()?.call::<TerminalResize>(&ResizeParams {
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
                &pipe,
                attach::AttachOptions {
                    terminal,
                    scrollback,
                    take_size: !no_resize,
                },
            )?;
            return Ok(ExitCode::from(code as u8));
        }
        Cmd::Kill { terminal } => {
            let t = connect()?.call::<TerminalKill>(&TerminalRef::parse(&terminal))?;
            if json {
                print_json(&t);
            } else {
                println!("{} {}", t.id, state_word(&t));
            }
        }
        Cmd::Rm { terminal, force } => {
            let r = connect()?.call::<TerminalRm>(&RmParams {
                target: TerminalRef::parse(&terminal),
                force,
            })?;
            if json {
                print_json(&r);
            } else {
                println!("{} removed", r.id);
            }
        }
        Cmd::Repo { cmd } => repo_cmd(cmd, &mut connect()?, json, remote.is_some())?,
        Cmd::Task { cmd } => task_cmd(cmd, &mut connect()?, &pipe, json)?,
        Cmd::Review { cmd } => return orch::review(cmd, &mut connect()?, json),
        Cmd::Group { cmd } => return orch::group(cmd, &mut connect()?, &pipe, json),
        Cmd::Gate { cmd } => return orch::gate(cmd, &mut connect()?, &pipe, json),
        Cmd::Orch { cmd } => return orch_run::run(cmd, &mut connect()?, &pipe, json),
        Cmd::Pr { cmd } => return pr::run(cmd, &mut connect()?, json),
        Cmd::Sessions { cmd } => return sessions::run(cmd, &mut connect()?, json),
        Cmd::Mobile { cmd } => return mobile::run(cmd, &mut connect()?, json),
        Cmd::Server {
            cmd: ServerCmd::Schema,
        } => {
            println!(
                "{}",
                serde_json::to_string_pretty(&podd::protocol::schema())?
            );
        }
        Cmd::Server {
            cmd: ServerCmd::Status,
        } if remote.is_some() => {
            let s = connect()?.call::<ServerStatus>(&Empty {})?;
            print_server_status(&s, json);
        }
        Cmd::Server { cmd } => {
            // Never auto-start for status/stop.
            let mut c = match Client::connect(&pipe) {
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

fn repo_cmd(cmd: RepoCmd, c: &mut Client, json: bool, remote: bool) -> anyhow::Result<()> {
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
    Ok(())
}

/// The caller's own terminal id when this `pod` runs inside a terminal of
/// the daemon it talks to.
pub(crate) fn caller_terminal(pipe: &str) -> Option<String> {
    let sock = std::env::var(endpoint::ENV_SOCKET).ok()?;
    let id = std::env::var(endpoint::ENV_TERMINAL_ID).ok()?;
    (endpoint::pipe_path(&sock).eq_ignore_ascii_case(pipe) && !id.is_empty()).then_some(id)
}

fn task_cmd(cmd: TaskCmd, c: &mut Client, pipe: &str, json: bool) -> anyhow::Result<()> {
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
            let parent = caller_terminal(pipe);
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
    Ok(())
}

fn short_sha(s: &str) -> &str {
    &s[..s.len().min(10)]
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

/// Left-aligned columns; the last one is not padded.
fn print_rows(header: &[&str], rows: Vec<Vec<String>>) {
    let mut w: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let mut s = String::new();
        let last = cells.len() - 1;
        for (i, c) in cells.iter().enumerate() {
            if i == last {
                s.push_str(c);
            } else {
                s.push_str(&format!("{c:<width$}  ", width = w[i]));
            }
        }
        println!("{}", s.trim_end());
    };
    line(header.to_vec());
    for r in &rows {
        line(r.iter().map(String::as_str).collect());
    }
}

/// A grey prompt suggestion that `read` left out of the lines (docs/19
/// §18.3): named on its own line so a reading agent does not take it for
/// typed input.
pub(crate) fn print_suggestion(s: Option<&str>) {
    if let Some(s) = s {
        println!("[grey prompt suggestion at the caret, not typed: {s:?}]");
    }
}

fn print_json<T: serde::Serialize>(v: &T) {
    println!("{}", serde_json::to_string_pretty(v).expect("serialize"));
}

fn code_suffix(code: Option<i32>) -> String {
    code.map(|c| format!(" (code {c})")).unwrap_or_default()
}

fn state_word(t: &TerminalInfo) -> String {
    match t.state {
        TerminalState::Running => "running".into(),
        TerminalState::Exited if t.incompatible_holder.is_some() => "incompatible holder".into(),
        TerminalState::Exited if t.lost => "lost".into(),
        TerminalState::Exited if t.killed => format!("killed{}", code_suffix(t.exit_code)),
        TerminalState::Exited => format!("exited{}", code_suffix(t.exit_code)),
        TerminalState::Unreachable => "unreachable".into(),
    }
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

fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m{}s", s / 60, s % 60),
        _ => format!("{}h{}m", s / 3600, (s % 3600) / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn run_takes_argv_after_double_dash() {
        let cli = Cli::try_parse_from([
            "pod", "run", "--name", "a", "--", "cmd", "/c", "echo", "--x",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Run { name, argv, .. } => {
                assert_eq!(name.as_deref(), Some("a"));
                assert_eq!(argv, ["cmd", "/c", "echo", "--x"]);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn wait_flags_conflict() {
        assert!(Cli::try_parse_from(["pod", "wait", "t1", "--exit", "--output", "x"]).is_err());
    }

    #[test]
    fn status_commands_parse() {
        assert!(Cli::try_parse_from(["pod", "wait", "t1", "--status", "done"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "wait", "t1", "--status", "any-change"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "wait", "t1", "--turn-end", "--timeout", "5"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "wait", "t1", "--status", "nope"]).is_err());
        assert!(
            Cli::try_parse_from(["pod", "wait", "t1", "--output", "x", "--status", "idle"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["pod", "status", "t1", "--json"]).is_ok());
        assert!(
            Cli::try_parse_from(["pod", "hooks", "install", "--dry-run", "--scope", "profile"])
                .is_ok()
        );
        assert!(Cli::try_parse_from(["pod", "hooks", "uninstall", "--home", "x"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "hooks", "status", "--check-codex"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "hooks", "install", "--scope", "nope"]).is_err());
    }

    #[test]
    fn keys_and_stop_parse() {
        assert!(Cli::try_parse_from(["pod", "keys", "t1", "ctrl+c", "enter"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "server", "stop", "--kill"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "keys", "t1"]).is_err());
    }

    #[test]
    fn orchestration_commands_parse() {
        let ok = |args: &[&str]| {
            let mut v = vec!["pod"];
            v.extend_from_slice(args);
            Cli::try_parse_from(v)
                .map(|_| ())
                .map_err(|e| e.to_string())
        };
        ok(&["review", "add", "a/b", "src/x.rs", "12", "--body", "why?"]).unwrap();
        ok(&[
            "review",
            "add",
            "a/b",
            "x.rs",
            "3",
            "--end-line",
            "5",
            "--side",
            "old",
            "--body",
            "b",
        ])
        .unwrap();
        ok(&["review", "add", "a/b", "--from-json", "c.json"]).unwrap();
        assert!(ok(&["review", "add", "a/b", "x.rs", "3"]).is_err());
        ok(&["review", "ls", "a/b", "--state", "open", "--json"]).unwrap();
        ok(&[
            "review",
            "send",
            "a/b",
            "c1",
            "c2",
            "--instructions",
            "be brief",
        ])
        .unwrap();
        ok(&["review", "send", "a/b", "--all", "--dry-run"]).unwrap();
        ok(&["review", "resolve", "c1", "--note", "fixed"]).unwrap();
        assert!(ok(&["review", "resolve"]).is_err());
        ok(&["review", "deliveries"]).unwrap();
        ok(&["review", "cancel", "d1"]).unwrap();
        ok(&[
            "group",
            "new",
            "app",
            "fix",
            "--prompt",
            "p",
            "--agent",
            "claude",
            "--agent",
            "shell",
            "--",
            "cmd",
            "/c",
            "cargo test",
        ])
        .unwrap();
        ok(&[
            "group",
            "new",
            "app",
            "fix",
            "--prompt",
            "p",
            "--agent",
            "[\"x.exe\",\"a\"]",
        ])
        .unwrap();
        assert!(ok(&["group", "new", "app", "fix", "--prompt", "p"]).is_err());
        ok(&["group", "ls", "--repo", "app"]).unwrap();
        ok(&["group", "compare", "app/fix", "--json"]).unwrap();
        ok(&[
            "group",
            "check",
            "fix",
            "--timeout",
            "30",
            "--",
            "cmd",
            "/c",
            "exit 0",
        ])
        .unwrap();
        ok(&[
            "group",
            "pick",
            "fix",
            "fix-shell-2",
            "--remove-others",
            "--force",
            "--delete-branch",
        ])
        .unwrap();
        ok(&[
            "gate",
            "request",
            "deploy?",
            "--title",
            "Deploy",
            "--wait",
            "--timeout",
            "60",
        ])
        .unwrap();
        assert!(ok(&["gate", "request", "x", "--timeout", "5"]).is_err());
        ok(&["gate", "ls", "--pending"]).unwrap();
        ok(&["gate", "approve", "g1", "--note", "ok"]).unwrap();
        ok(&["gate", "reject", "g1"]).unwrap();
        ok(&["wait", "--gate", "g1", "--timeout", "5"]).unwrap();
        assert!(ok(&["wait", "t1", "--gate", "g1"]).is_err());
        assert!(ok(&["wait"]).is_err());
        ok(&["wait", "t1", "--status", "idle"]).unwrap();
        // v6
        ok(&["gate", "request", "x", "--task", "a/b", "--expires", "30"]).unwrap();
        ok(&["group", "check", "g", "--no-wait"]).unwrap();
        ok(&["group", "check", "g", "--cancel"]).unwrap();
        assert!(ok(&["group", "check", "g", "--cancel", "--no-wait"]).is_err());
        assert!(ok(&["group", "check", "g", "--cancel", "--", "cmd"]).is_err());
        ok(&["ls", "--all"]).unwrap();
        ok(&["ls", "-a"]).unwrap();
        ok(&["keep", "t1", "--unkeep"]).unwrap();
        assert!(orch_agent_ok("claude") && orch_agent_ok("[\"a\"]") && !orch_agent_ok("vim"));
        // docs/19 §17: a long report from a file (PowerShell 5.1 splits quoted text).
        ok(&[
            "orch",
            "done",
            "--outcome",
            "succeeded",
            "--body-file",
            "r.md",
        ])
        .unwrap();
        assert!(
            ok(&["orch", "done", "--outcome", "succeeded"]).is_err(),
            "a body is required"
        );
        assert!(
            ok(&[
                "orch",
                "done",
                "--outcome",
                "succeeded",
                "--body",
                "x",
                "--body-file",
                "r.md"
            ])
            .is_err()
        );
        // docs/17 M1
        ok(&["mobile", "devices"]).unwrap();
        ok(&["mobile", "revoke", "m1"]).unwrap();
        ok(&["mobile", "revoke", "--all"]).unwrap();
        assert!(ok(&["mobile", "revoke"]).is_err());
        assert!(ok(&["mobile", "revoke", "m1", "--all"]).is_err());
        ok(&[
            "mobile",
            "policy",
            "m1",
            "demo/a",
            "--allow",
            "status,approve",
            "--deny",
            "output",
        ])
        .unwrap();
        ok(&["mobile", "policy", "m1", "demo/a", "--clear"]).unwrap();
        assert!(ok(&["mobile", "policy", "m1", "demo/a", "--allow", "shell"]).is_err());
        ok(&["mobile", "audit", "--device", "m1", "--limit", "5"]).unwrap();
        ok(&["mobile", "settings", "--audit-bodies", "on"]).unwrap();
        assert!(ok(&["mobile", "settings", "--audit-bodies", "maybe"]).is_err());
    }

    fn orch_agent_ok(s: &str) -> bool {
        Cli::try_parse_from([
            "pod", "group", "new", "r", "n", "--prompt", "p", "--agent", s,
        ])
        .is_ok()
            && super::orch::parse_agent_for_test(s)
    }

    #[test]
    fn pr_commands_parse() {
        let cli = Cli::try_parse_from([
            "pod",
            "pr",
            "create",
            "demo/a",
            "--title",
            "제목",
            "--draft",
            "--base",
            "dev",
            "--no-push",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Pr {
                cmd:
                    pr::PrCmd::Create {
                        task,
                        title,
                        draft,
                        base,
                        no_push,
                        ..
                    },
            } => {
                assert_eq!(
                    (
                        task.as_str(),
                        title.as_deref(),
                        draft,
                        base.as_deref(),
                        no_push
                    ),
                    ("demo/a", Some("제목"), true, Some("dev"), true)
                );
            }
            _ => panic!("pr create"),
        }
        assert!(
            Cli::try_parse_from(["pod", "pr", "merge", "a"]).is_err(),
            "--method is required"
        );
        assert!(
            Cli::try_parse_from(["pod", "pr", "merge", "a", "--method", "squash", "--yes"]).is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "pod",
                "pr",
                "create",
                "a",
                "--body",
                "x",
                "--body-file",
                "f"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["pod", "pr", "status", "a", "--cached", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "pr", "open", "a", "--print"]).is_ok());
    }

    #[test]
    fn task_commands_parse() {
        let cli = Cli::try_parse_from([
            "pod", "task", "new", "app", "feat-1", "--base", "main", "--agent", "claude",
            "--prompt", "fix it", "--json",
        ])
        .unwrap();
        assert!(cli.json);
        match cli.cmd {
            Cmd::Task {
                cmd:
                    TaskCmd::New {
                        repo,
                        name,
                        base,
                        agent,
                        prompt,
                        argv,
                        ..
                    },
            } => {
                assert_eq!((repo.as_str(), name.as_deref()), ("app", Some("feat-1")));
                assert_eq!(base.as_deref(), Some("main"));
                assert!(matches!(agent, Some(AgentArg::Claude)));
                assert_eq!(prompt.as_deref(), Some("fix it"));
                assert!(argv.is_empty());
            }
            _ => panic!(),
        }
        assert!(Cli::try_parse_from(["pod", "task", "new", "a", "b", "--agent", "vim"]).is_err());
        assert!(Cli::try_parse_from(["pod", "task", "new", "a", "b", "--", "cmd", "/k"]).is_ok());
        assert!(
            Cli::try_parse_from(["pod", "task", "rm", "a/b", "--force", "--delete-branch"]).is_ok()
        );
        assert!(Cli::try_parse_from(["pod", "repo", "add", "C:\\x y"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "repo", "ls", "--json"]).is_ok());
        assert!(Cli::try_parse_from(["pod", "run", "--task", "a/b", "--", "cmd"]).is_ok());
    }

    #[test]
    fn session_commands_parse() {
        // docs/19: name optional, brief, branch, setup flags.
        let cli = Cli::try_parse_from([
            "pod",
            "task",
            "new",
            "app",
            "--brief-file",
            "b.md",
            "--branch",
            "feature/x",
            "--setup",
            "skip",
            "--wait-setup",
            "--no-focus",
        ])
        .unwrap();
        match cli.cmd {
            Cmd::Task {
                cmd:
                    TaskCmd::New {
                        name,
                        brief_file,
                        branch,
                        setup,
                        wait_setup,
                        no_focus,
                        ..
                    },
            } => {
                assert!(name.is_none());
                assert_eq!(brief_file.as_deref(), Some(std::path::Path::new("b.md")));
                assert_eq!(branch.as_deref(), Some("feature/x"));
                assert!(matches!(setup, Some(SetupArg::Skip)) && wait_setup && no_focus);
            }
            _ => panic!(),
        }
        for ok in [
            &[
                "pod",
                "task",
                "archive",
                "a/b",
                "--force",
                "--ignore-script-failure",
            ][..],
            &["pod", "task", "unarchive", "a/b", "--no-agent"],
            &[
                "pod",
                "task",
                "resume",
                "a/b",
                "--session",
                "abc",
                "--agent",
                "codex",
            ],
            &["pod", "task", "cleanup", "--repo", "app", "--days", "3"],
            &["pod", "task", "suggest-name", "app"],
            &["pod", "repo", "config", "app", "--task", "app/x"],
            &["pod", "repo", "trust", "app", "abc123", "--revoke"],
            &[
                "pod",
                "repo",
                "set",
                "app",
                "--setup",
                "npm ci",
                "--worktrees-root",
                r"D:\wt",
            ],
        ] {
            assert!(Cli::try_parse_from(ok.iter().copied()).is_ok(), "{ok:?}");
        }
        assert!(session::read_brief(Some("a".into()), Some("b".into())).is_err());
    }
}
