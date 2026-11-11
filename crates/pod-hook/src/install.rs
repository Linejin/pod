//! `pod-hook install --dry-run`: print, as JSON, the hooks config Pod would
//! install or remove. It only reads agent configs and never writes them.

use std::path::PathBuf;

use pod_status::hook::HookAgent;
use pod_status::install::{self, HookCommandSpec, InstallScope};
use serde_json::Value;

/// `args` follow `install`. Returns the exit code: 0 plan printed, 1 plan
/// failed (an unreadable config), 2 bad arguments or no `--dry-run`.
pub(crate) fn run(args: &[String]) -> i32 {
    let mut dry_run = false;
    let mut uninstall = false;
    let mut block_only = false;
    let mut agents = vec![HookAgent::Claude, HookAgent::Codex];
    let mut home: Option<PathBuf> = None;
    let mut claude_dir: Option<PathBuf> = None;
    let mut codex_home: Option<PathBuf> = None;
    let mut exe: Option<PathBuf> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |name: &str| -> Option<String> {
            let v = it.next().cloned();
            if v.is_none() {
                eprintln!("pod-hook install: {name} needs a value");
            }
            v
        };
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--uninstall" => uninstall = true,
            "--block-only" => block_only = true,
            "--agent" => match val("--agent").as_deref() {
                Some("all") => agents = vec![HookAgent::Claude, HookAgent::Codex],
                Some(x) => match HookAgent::parse(x) {
                    Some(ag) => agents = vec![ag],
                    None => {
                        eprintln!("pod-hook install: unknown agent {x}");
                        return 2;
                    }
                },
                None => return 2,
            },
            "--home" => home = val("--home").map(PathBuf::from),
            "--claude-config-dir" => claude_dir = val("--claude-config-dir").map(PathBuf::from),
            "--codex-home" => codex_home = val("--codex-home").map(PathBuf::from),
            "--exe" => exe = val("--exe").map(PathBuf::from),
            other => {
                eprintln!("pod-hook install: unknown option {other}");
                return 2;
            }
        }
    }
    if !dry_run {
        eprintln!(
            "pod-hook install: only --dry-run is supported; Pod never writes agent configuration from this CLI"
        );
        return 2;
    }
    let exe = exe
        .or_else(|| std::env::current_exe().ok())
        .unwrap_or_else(|| PathBuf::from("pod-hook.exe"));
    let spec = HookCommandSpec::new(exe);

    if block_only {
        let mut out = serde_json::Map::new();
        for ag in &agents {
            out.insert(ag.as_str().into(), install::hooks_block(*ag, &spec));
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default()
        );
        return 0;
    }

    let scope = match (claude_dir, codex_home) {
        (None, None) => match home.or_else(install::user_home) {
            Some(home) => InstallScope::Global { home },
            None => {
                eprintln!("pod-hook install: no home directory; pass --home");
                return 2;
            }
        },
        (Some(c), Some(x)) => InstallScope::Profile {
            claude_config_dir: c,
            codex_home: x,
        },
        _ => {
            eprintln!("pod-hook install: --claude-config-dir and --codex-home go together");
            return 2;
        }
    };
    let plan = if uninstall {
        install::plan_uninstall(&scope, &spec, &agents)
    } else {
        install::plan_install(&scope, &spec, &agents)
    };
    match plan {
        Ok(p) => {
            println!("{}", serde_json::to_string_pretty(&p).unwrap_or_default());
            0
        }
        Err(e) => {
            eprintln!("pod-hook install: {e}");
            1
        }
    }
}
