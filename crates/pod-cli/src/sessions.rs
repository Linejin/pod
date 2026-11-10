//! `pod sessions …`: agent sessions found on this PC (docs/18).

use std::process::ExitCode;

use clap::Subcommand;
use podd::client::Client;
use podd::protocol::*;

#[derive(Subcommand)]
pub enum SessionsCmd {
    /// List discovered sessions (newest first).
    #[command(alias = "ls")]
    List {
        /// Include hidden sessions.
        #[arg(long)]
        all: bool,
    },
    /// Scan the agents' session stores and running processes now.
    Scan {
        /// Turn discovery on first (it is off by default).
        #[arg(long)]
        enable: bool,
    },
    /// Show or change discovery settings.
    Settings {
        /// Discovery on or off.
        #[arg(long, value_parser = on_off)]
        enabled: Option<bool>,
        /// Store and show each session's first prompt.
        #[arg(long, value_parser = on_off)]
        preview: Option<bool>,
    },
    /// Link a session to its Pod task / repository.
    Register {
        /// `claude:<id>`, `codex:<id>` or a unique id prefix.
        session: String,
        /// Link to this task instead of the one matching the session's folder.
        #[arg(long)]
        task: Option<String>,
        /// Register the session's git repository first if Pod does not know it.
        #[arg(long)]
        add_repo: bool,
    },
    /// Resume a session in a new Pod terminal (the agent's own resume command).
    Resume {
        session: String,
        /// Terminal name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Hide a session from the list (`--show` shows it again).
    Hide {
        session: String,
        #[arg(long)]
        show: bool,
    },
}

fn on_off(s: &str) -> Result<bool, String> {
    match s.to_ascii_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        _ => Err("use on or off".into()),
    }
}

fn state_label(s: SessionState) -> &'static str {
    match s {
        SessionState::RunningPod => "running (Pod)",
        SessionState::RunningOther => "running (other terminal)",
        SessionState::Resumable => "resumable",
        SessionState::Stale => "stale",
    }
}

fn ago(ms: Option<u64>) -> String {
    let Some(ms) = ms else { return "-".into() };
    let d = podd::now_ms().saturating_sub(ms) / 1000;
    match d {
        0..60 => format!("{d}s ago"),
        60..3600 => format!("{}m ago", d / 60),
        3600..86_400 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86_400),
    }
}

fn print_list(l: &SessionsList) {
    if !l.enabled {
        println!("session discovery is off; turn it on with `pod sessions scan --enable`");
        return;
    }
    if l.sessions.is_empty() {
        println!(
            "no sessions found{}",
            if l.scanned_ms.is_none() {
                " (not scanned yet)"
            } else {
                ""
            }
        );
    }
    for s in &l.sessions {
        print_one(s);
    }
    let mut notes = Vec::new();
    if l.hidden > 0 {
        notes.push(format!("{} hidden (--all)", l.hidden));
    }
    if !l.unknown_running.is_empty() {
        notes.push(format!(
            "{} running agent(s) without a known session",
            l.unknown_running.len()
        ));
    }
    if l.truncated {
        notes.push("scan stopped at a limit; more appear on the next scan".into());
    }
    if !l.preview {
        notes.push("previews off".into());
    }
    if !notes.is_empty() {
        println!("({})", notes.join("; "));
    }
}

fn print_one(s: &DiscoveredSession) {
    let short = s.id.get(..8).unwrap_or(&s.id);
    let link = s
        .link
        .as_ref()
        .or((!s.matched.is_empty()).then_some(&s.matched))
        .map(|m| m.task.clone().or(m.repo.clone()).unwrap_or_default())
        .unwrap_or_default();
    println!(
        "{:<6} {short}  {:<24} {:>9}  {}{}{}",
        s.agent.as_str(),
        state_label(s.state),
        ago(s.last_ms),
        s.cwd,
        s.branch
            .as_deref()
            .map(|b| format!(" [{b}]"))
            .unwrap_or_default(),
        if link.is_empty() {
            String::new()
        } else {
            format!(
                " -> {link}{}",
                if s.link.is_some() { "" } else { " (match)" }
            )
        },
    );
    if let Some(t) = s.title.as_deref().or(s.preview.as_deref()) {
        println!("       {t}");
    }
}

pub fn run(cmd: SessionsCmd, c: &mut Client, json: bool) -> anyhow::Result<ExitCode> {
    match cmd {
        SessionsCmd::List { all } => {
            let l = c.call::<SessionsListMethod>(&SessionsListParams {
                include_hidden: all,
            })?;
            if json {
                crate::print_json(&l)
            } else {
                print_list(&l)
            }
        }
        SessionsCmd::Scan { enable } => {
            let l = c.call::<SessionsScan>(&SessionsScanParams {
                enable,
                include_hidden: false,
            })?;
            if json {
                crate::print_json(&l)
            } else {
                print_list(&l)
            }
        }
        SessionsCmd::Settings { enabled, preview } => {
            let l = c.call::<SessionsSettings>(&SessionsSettingsParams { enabled, preview })?;
            if json {
                crate::print_json(&l);
            } else {
                println!(
                    "discovery {}, previews {}",
                    if l.enabled { "on" } else { "off" },
                    if l.preview { "on" } else { "off" }
                );
            }
        }
        SessionsCmd::Register {
            session,
            task,
            add_repo,
        } => {
            let s = c.call::<SessionsRegister>(&SessionsRegisterParams {
                session,
                task,
                add_repo,
            })?;
            if json {
                crate::print_json(&s)
            } else {
                print_one(&s)
            }
        }
        SessionsCmd::Resume { session, name } => {
            let t = c.call::<SessionsResume>(&SessionsResumeParams { session, name })?;
            if json {
                crate::print_json(&t);
            } else {
                println!(
                    "{} started: {}  (pod attach {})",
                    t.id,
                    t.argv.join(" "),
                    t.id
                );
            }
        }
        SessionsCmd::Hide { session, show } => {
            let s = c.call::<SessionsHide>(&SessionsHideParams {
                session,
                hidden: Some(!show),
            })?;
            if json {
                crate::print_json(&s);
            } else {
                println!("{} {}", s.key, if s.hidden { "hidden" } else { "shown" });
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
