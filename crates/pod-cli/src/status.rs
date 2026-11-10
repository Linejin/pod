//! Agent status commands: `pod status`, `pod ack`, `pod wait --status/--turn-end`,
//! `pod hooks install --dry-run`, and the STATUS column of `pod ls`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::bail;
use clap::{Subcommand, ValueEnum};
use pod_status::HookAgent;
use pod_status::install::{self, InstallScope};
use podd::client::Client;
use podd::protocol::*;

use crate::hooks_install::long_path;

/// Values of `pod wait --status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum WaitStatus {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
    Exited,
    AnyChange,
}

impl WaitStatus {
    fn wire(self) -> &'static str {
        match self {
            WaitStatus::Idle => "idle",
            WaitStatus::Working => "working",
            WaitStatus::Done => "done",
            WaitStatus::Blocked => "blocked",
            WaitStatus::Error => "error",
            WaitStatus::Exited => "exited",
            WaitStatus::AnyChange => "any_change",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Scope {
    /// `~/.claude/settings.json` + `~/.codex/hooks.json` (env-guarded hooks).
    Global,
    /// Separate profile dirs, used via CLAUDE_CONFIG_DIR / CODEX_HOME.
    Profile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum AgentArg {
    Claude,
    Codex,
    All,
}

#[derive(clap::Args, Clone)]
pub struct HookTarget {
    /// global: ~/.claude/settings.json + ~/.codex/hooks.json (the decided
    /// policy). profile: separate CLAUDE_CONFIG_DIR / CODEX_HOME dirs.
    #[arg(long, value_enum, default_value_t = Scope::Global)]
    scope: Scope,
    #[arg(long, value_enum, default_value_t = AgentArg::All)]
    agent: AgentArg,
    /// Home directory for --scope global (default: %USERPROFILE%). Tests use a temporary one.
    #[arg(long)]
    home: Option<PathBuf>,
    /// Claude profile dir for --scope profile (default: %LOCALAPPDATA%\pod\profiles\default\claude).
    #[arg(long)]
    claude_config_dir: Option<PathBuf>,
    /// Codex profile dir for --scope profile (default: %LOCALAPPDATA%\pod\profiles\default\codex).
    #[arg(long)]
    codex_home: Option<PathBuf>,
    /// Stable directory the hooks run pod-hook.exe from (default: %LOCALAPPDATA%\Pod\bin).
    #[arg(long)]
    bin_dir: Option<PathBuf>,
    /// pod-hook.exe to install (default: next to pod.exe).
    #[arg(long)]
    source: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum HooksCmd {
    /// Install Pod's agent hooks: copy pod-hook.exe to the bin dir, merge the
    /// hooks into the agents' config (backups first, other tools' hooks
    /// kept), and trust Pod's Codex hooks via `codex app-server`.
    Install {
        #[command(flatten)]
        target: HookTarget,
        /// Print what would change; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Do not write Codex [hooks.state] trust entries.
        #[arg(long)]
        no_codex_trust: bool,
    },
    /// Remove Pod's hook entries (backups first); everything else stays.
    Uninstall {
        #[command(flatten)]
        target: HookTarget,
        #[arg(long)]
        dry_run: bool,
    },
    /// Show what is installed.
    Status {
        #[command(flatten)]
        target: HookTarget,
        /// Also ask `codex app-server` whether Pod's Codex hooks are trusted.
        #[arg(long)]
        check_codex: bool,
    },
}

/// `ls` cell: `idle (osc)`, `blocked:permission (hook)`, `done (+1 running)`.
pub fn status_cell(t: &TerminalInfo) -> String {
    match &t.status {
        None => "-".into(),
        Some(s) => match &s.source {
            Some(src) if s.state != "exited" => format!("{} ({src})", s.badge),
            _ => s.badge.clone(),
        },
    }
}

fn ago(since: u64) -> String {
    let now = podd::now_ms();
    let s = now.saturating_sub(since) / 1000;
    match s {
        0..60 => format!("{s}s ago"),
        60..3600 => format!("{}m{}s ago", s / 60, s % 60),
        _ => format!("{}h{}m ago", s / 3600, (s % 3600) / 60),
    }
}

pub fn print_status(t: &TerminalInfo) {
    let Some(s) = &t.status else {
        println!("{}: no status (daemon without status engine)", t.id);
        return;
    };
    println!(
        "{}  {}  source={} confidence={:.2} since={} turn={}",
        t.id,
        s.badge,
        s.source.as_deref().unwrap_or("-"),
        s.confidence,
        ago(s.since),
        s.turn
    );
    let mut kv = Vec::new();
    if let Some(a) = &s.agent {
        kv.push(format!("agent={a}"));
    }
    if let Some(a) = &s.app {
        kv.push(format!("app={a}"));
    }
    if let Some(o) = &s.outcome {
        kv.push(format!("outcome={o}"));
    }
    if let Some(f) = &s.final_state {
        kv.push(format!("final={f}"));
    }
    if let Some(p) = s.progress {
        kv.push(format!("progress={p}%"));
    }
    if s.records > 0 {
        kv.push(format!(
            "records={} running={} blocked={}",
            s.records, s.running_children, s.blocked_children
        ));
    }
    if s.hooks_accepted + s.hooks_rejected > 0 {
        kv.push(format!(
            "hooks={} rejected={}",
            s.hooks_accepted, s.hooks_rejected
        ));
    }
    if !kv.is_empty() {
        println!("  {}", kv.join(" "));
    }
    if let Some(x) = &s.title {
        println!("  title: {x}");
    }
    if let Some(x) = &s.msg {
        println!("  msg: {x}");
    }
}

pub fn wait(
    c: &mut Client,
    terminal: &str,
    status: Option<WaitStatus>,
    turn_end: bool,
    timeout_ms: Option<u64>,
    json: bool,
) -> anyhow::Result<ExitCode> {
    // Survives a daemon restart (docs/12 B6): terminals and their status
    // (turn counter, hook evidence) are restored, so ask again.
    let what = format!("the status of {terminal}");
    let r = crate::reconnect::wait(c, timeout_ms, &what, |c, timeout_ms| {
        c.call::<StatusWait>(&StatusWaitParams {
            target: TerminalRef::parse(terminal),
            status: status.map(|s| s.wire().to_string()),
            turn_end,
            timeout_ms,
        })
    })?;
    if json {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        let id = &r.terminal.id;
        let label = r.status.state_label();
        match r.outcome {
            WaitOutcome::Matched => println!("{id} {label} (turn {})", r.status.turn),
            WaitOutcome::TimedOut => {
                println!("{id} timed out after {} ms ({label})", r.waited_ms)
            }
            WaitOutcome::Exited => println!("{id} exited ({label})"),
        }
    }
    Ok(if r.outcome == WaitOutcome::Matched {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn default_profile_dir(sub: &str) -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("pod").join("profiles").join("default").join(sub)
}

fn request(
    t: HookTarget,
    dry_run: bool,
    codex_trust: bool,
) -> anyhow::Result<crate::hooks_install::Request> {
    let agents = match t.agent {
        AgentArg::Claude => vec![HookAgent::Claude],
        AgentArg::Codex => vec![HookAgent::Codex],
        AgentArg::All => vec![HookAgent::Claude, HookAgent::Codex],
    };
    let scope = match t.scope {
        Scope::Global => {
            if t.claude_config_dir.is_some() || t.codex_home.is_some() {
                bail!("--claude-config-dir/--codex-home apply to --scope profile");
            }
            match t.home.or_else(install::user_home) {
                Some(home) => InstallScope::Global {
                    home: long_path(&home),
                },
                None => bail!("no home directory; pass --home"),
            }
        }
        Scope::Profile => {
            if t.home.is_some() {
                bail!("--home applies to --scope global");
            }
            InstallScope::Profile {
                claude_config_dir: long_path(
                    &t.claude_config_dir
                        .unwrap_or_else(|| default_profile_dir("claude")),
                ),
                codex_home: long_path(
                    &t.codex_home.unwrap_or_else(|| default_profile_dir("codex")),
                ),
            }
        }
    };
    let source = t.source.unwrap_or_else(|| {
        std::env::current_exe()
            .unwrap_or_default()
            .with_file_name("pod-hook.exe")
    });
    // Agent configs get the long spelling of every path (8.3 short names
    // expanded): it is what Codex keys its trust entries by, and it keeps
    // working where short names are off or stripped.
    Ok(crate::hooks_install::Request {
        scope,
        agents,
        bin_dir: long_path(
            &t.bin_dir
                .unwrap_or_else(crate::hooks_install::default_bin_dir),
        ),
        source,
        dry_run,
        codex_trust,
    })
}

pub fn hooks(cmd: HooksCmd, json: bool) -> anyhow::Result<ExitCode> {
    use crate::hooks_install as hi;
    let (outcome, verb) = match cmd {
        HooksCmd::Status {
            target,
            check_codex,
        } => {
            let st = hi::status(&request(target, true, false)?, check_codex)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&st)?);
                return Ok(ExitCode::SUCCESS);
            }
            let cur = match st.bin_current {
                Some(true) => ", current",
                Some(false) => ", differs from this build",
                None => "",
            };
            println!(
                "pod-hook: {} ({}{cur})",
                st.bin.display(),
                if st.bin_exists { "present" } else { "missing" }
            );
            for a in &st.agents {
                println!(
                    "{}: {} - {} Pod event(s), {} other hook group(s){}",
                    a.agent,
                    a.path.display(),
                    a.pod_events.len(),
                    a.foreign_groups,
                    if a.exists { "" } else { " (file missing)" }
                );
                for (exe, ok) in &a.hook_exes {
                    println!("  runs {exe}{}", if *ok { "" } else { "  (MISSING)" });
                }
            }
            if let Some(t) = &st.codex_trust {
                println!(
                    "codex trust: {} Pod hook(s), {} untrusted",
                    t.pod_hooks, t.untrusted
                );
            }
            if let Some(e) = &st.codex_trust_error {
                println!("codex trust: unknown ({e})");
            }
            return Ok(ExitCode::SUCCESS);
        }
        HooksCmd::Install {
            target,
            dry_run,
            no_codex_trust,
        } => (
            hi::install(&request(target, dry_run, !no_codex_trust)?)?,
            "install",
        ),
        HooksCmd::Uninstall { target, dry_run } => (
            hi::uninstall(&request(target, dry_run, false)?)?,
            "uninstall",
        ),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
        return Ok(ExitCode::SUCCESS);
    }
    if outcome.dry_run {
        println!("# dry run ({verb}): nothing is written");
    }
    println!(
        "pod-hook: {}{}",
        outcome.hook_exe.display(),
        if outcome.binary_copied {
            " (copied)"
        } else {
            ""
        }
    );
    for f in &outcome.files {
        print!("{}: {} {}", f.agent, f.action, f.path.display());
        match &f.backup {
            Some(b) => println!(" (backup {})", b.display()),
            None => println!(),
        }
        if let Some(c) = &f.content {
            print!("{c}");
        }
    }
    if let Some(t) = &outcome.codex_trust {
        println!(
            "codex trust: {} Pod hook(s), {} newly trusted, {} untrusted{}",
            t.pod_hooks,
            t.newly_trusted,
            t.untrusted,
            t.config_backup
                .as_ref()
                .map(|b| format!(" (config backup {})", b.display()))
                .unwrap_or_default()
        );
    }
    for n in &outcome.notes {
        println!("note: {n}");
    }
    Ok(ExitCode::SUCCESS)
}

/// Task STATUS cell: the aggregate badge and which terminal decided it.
pub fn task_status_cell(t: &TaskInfo) -> String {
    match &t.status {
        None => "-".into(),
        Some(s) if s.terminals > 1 => format!("{} [{}]", s.badge, s.terminal),
        Some(s) => s.badge.clone(),
    }
}
