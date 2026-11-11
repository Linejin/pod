//! The `pod` command line: clap definitions only (docs/20-refactor/daemon.md
//! card P3). Running a command is in `commands` and the specialised command
//! modules; `main` routes between them.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use crate::{guide, machine, mobile, orch, orch_run, pr, session, sessions, status};

#[derive(Parser)]
#[command(
    name = "pod",
    version,
    about = "Drive terminals owned by the podd daemon"
)]
pub(crate) struct Cli {
    /// Print machine-readable JSON instead of text.
    #[arg(long, global = true)]
    pub(crate) json: bool,
    /// Pipe name or path (default: $POD_SOCKET, else \\.\pipe\podd-<username>).
    #[arg(long, global = true, value_name = "PIPE")]
    pub(crate) pipe: Option<String>,
    /// Run the command on this SSH machine profile (`pod machine add`)
    /// instead of the local daemon.
    #[arg(long, global = true, value_name = "NAME", conflicts_with = "pipe")]
    pub(crate) machine: Option<String>,
    #[command(subcommand)]
    pub(crate) cmd: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
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
pub(crate) enum RepoCmd {
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
pub(crate) enum AgentArg {
    Claude,
    Codex,
    Shell,
}

#[derive(Subcommand)]
pub(crate) enum TaskCmd {
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
pub(crate) enum SetupArg {
    Run,
    Skip,
    Inherit,
}

#[derive(Subcommand)]
pub(crate) enum ServerCmd {
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
