//! Pure classification of a process list: which processes are interactive
//! agents (one per shim -> binary chain), which session each runs and
//! which Pod terminal it runs in. Tested with synthetic process lists.

use std::collections::{HashMap, HashSet};

use super::{ProcInfo, RunningAgent};
use crate::matching::norm_path;
use crate::{Agent, Found, is_uuid, last_activity};

/// Should [`snapshot`](super::snapshot) read this process's command line and cwd?
pub fn is_candidate_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "claude.exe" | "codex.exe" | "node.exe" | "claude" | "codex" | "node"
    )
}

/// Split a Windows command line (CommandLineToArgvW rules, simplified:
/// quotes group, `\"` is a literal quote, backslashes otherwise literal).
pub fn split_cmdline(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut any = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'"') => {
                chars.next();
                cur.push('"');
                any = true;
            }
            '"' => {
                in_q = !in_q;
                any = true;
            }
            c if c.is_whitespace() && !in_q => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

fn file_name_lower(s: &str) -> String {
    s.rsplit(['\\', '/'])
        .next()
        .unwrap_or(s)
        .to_ascii_lowercase()
}

/// Which agent a process is, and the index of the argument after which the
/// agent's own arguments start.
fn agent_and_args(p: &ProcInfo) -> Option<(Agent, Vec<String>)> {
    let name = p.name.to_ascii_lowercase();
    let args = p.cmdline.as_deref().map(split_cmdline).unwrap_or_default();
    if is_desktop_app(&args) {
        return None;
    }
    let rest = |i: usize| {
        args.get(i + 1..)
            .map(<[String]>::to_vec)
            .unwrap_or_default()
    };
    match name.trim_end_matches(".exe") {
        "claude" => Some((Agent::Claude, rest(0))),
        "codex" => Some((Agent::Codex, rest(0))),
        "node" => {
            // `node <...>\@anthropic-ai\claude-code\cli.js` / `<...>\@openai\codex\bin\codex.js`
            for (i, a) in args.iter().enumerate().skip(1) {
                let l = a.replace('/', "\\").to_ascii_lowercase();
                if l.contains("@anthropic-ai\\claude-code") {
                    return Some((Agent::Claude, rest(i)));
                }
                if l.contains("@openai\\codex") || file_name_lower(&l) == "codex.js" {
                    return Some((Agent::Codex, rest(i)));
                }
                if !a.starts_with('-') {
                    break;
                }
            }
            None
        }
        _ => None,
    }
}

/// Desktop (Electron) apps that share an executable name with a CLI: the
/// Claude desktop app is also `claude.exe`. Store-installed apps live under
/// `WindowsApps`; Electron helpers carry `--type=`.
fn is_desktop_app(args: &[String]) -> bool {
    let exe = args
        .first()
        .map(|a| a.replace('/', "\\").to_ascii_lowercase())
        .unwrap_or_default();
    exe.contains(r"\windowsapps\")
        || exe.contains(r"\anthropicclaude\")
        || args.iter().any(|a| a.starts_with("--type="))
}

/// Options that take a separate value (so the value is not a subcommand).
fn takes_value(agent: Agent, flag: &str) -> bool {
    if flag.contains('=') {
        return false;
    }
    match agent {
        Agent::Codex => matches!(
            flag,
            "-c" | "--config"
                | "-m"
                | "--model"
                | "-p"
                | "--profile"
                | "-s"
                | "--sandbox"
                | "-a"
                | "--ask-for-approval"
                | "-C"
                | "--cd"
                | "-i"
                | "--image"
                | "--add-dir"
                | "--enable"
                | "--disable"
                | "--remote"
                | "--local-provider"
                | "--remote-auth-token-env"
        ),
        Agent::Claude => matches!(
            flag,
            "--model"
                | "--session-id"
                | "--settings"
                | "--add-dir"
                | "--permission-mode"
                | "--mcp-config"
                | "--append-system-prompt"
                | "--system-prompt"
                | "--allowed-tools"
                | "--allowedTools"
                | "--disallowed-tools"
                | "--disallowedTools"
                | "--output-format"
                | "--input-format"
                | "--fallback-model"
                | "--agents"
                | "--agent"
                | "--setting-sources"
                | "--effort"
                | "-n"
                | "--name"
        ),
    }
}

const CLAUDE_NON_INTERACTIVE: &[&str] = &[
    "mcp",
    "config",
    "doctor",
    "update",
    "upgrade",
    "install",
    "setup-token",
    "plugin",
    "plugins",
    "daemon",
    "migrate-installer",
    "auth",
    "api-key",
    "agents",
];
const CODEX_NON_INTERACTIVE: &[&str] = &[
    "exec",
    "e",
    "review",
    "login",
    "logout",
    "mcp",
    "mcp-server",
    "plugin",
    "app-server",
    "remote-control",
    "app",
    "completion",
    "update",
    "doctor",
    "sandbox",
    "debug",
    "apply",
    "a",
    "queue",
    "archive",
    "delete",
    "migrate-rollouts",
    "unarchive",
    "cloud",
    "exec-server",
    "features",
    "help",
    "agents",
];

/// Is this an interactive session (not `codex app-server`, `claude mcp`,
/// `claude -p`, ...)?
fn interactive(agent: Agent, args: &[String]) -> bool {
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if agent == Agent::Claude && matches!(a.as_str(), "-p" | "--print") {
            return false;
        }
        if a.starts_with('-') {
            skip = takes_value(agent, a);
            continue;
        }
        let list = match agent {
            Agent::Claude => CLAUDE_NON_INTERACTIVE,
            Agent::Codex => CODEX_NON_INTERACTIVE,
        };
        return !list.contains(&a.as_str());
    }
    true
}

/// Interactive agent processes, one per shim -> binary chain (the outermost),
/// with a session id when the command line names one (`--resume <id>`,
/// `resume <id>`).
pub fn find_agents(procs: &[ProcInfo]) -> Vec<RunningAgent> {
    let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut kinds: HashMap<u32, (Agent, Vec<String>)> = HashMap::new();
    for p in procs {
        if let Some(k) = agent_and_args(p) {
            kinds.insert(p.pid, k);
        }
    }
    // Outermost same-agent ancestor of each candidate.
    let mut groups: HashMap<u32, Vec<u32>> = HashMap::new();
    for p in procs {
        let Some((agent, _)) = kinds.get(&p.pid) else {
            continue;
        };
        let mut top = p.pid;
        for anc in ancestors(p, &by_pid) {
            if kinds.get(&anc.pid).is_some_and(|(a, _)| a == agent) {
                top = anc.pid;
            }
        }
        groups.entry(top).or_default().push(p.pid);
    }
    let mut out = Vec::new();
    for (top, mut pids) in groups {
        let Some(p) = by_pid.get(&top) else { continue };
        let Some((agent, args)) = kinds.get(&top) else {
            continue;
        };
        if !interactive(*agent, args) {
            continue;
        }
        pids.sort_unstable();
        let session_id = pids
            .iter()
            .filter_map(|pid| kinds.get(pid))
            .flat_map(|(_, args)| args.iter())
            .map(|a| a.to_ascii_lowercase())
            .find(|a| is_uuid(a));
        let cwd = p.cwd.clone().or_else(|| {
            pids.iter()
                .filter_map(|pid| by_pid.get(pid)?.cwd.clone())
                .next()
        });
        out.push(RunningAgent {
            agent: Some(*agent),
            pid: top,
            pids,
            start_ms: p.start_ms,
            cwd,
            session_id,
            guessed: false,
            pod_terminal: None,
        });
    }
    out.sort_by_key(|a| a.pid);
    out
}

/// Parents of `p`, nearest first. A parent created after its child is a
/// reused pid and ends the chain. At most 64 steps.
fn ancestors<'a>(p: &ProcInfo, by_pid: &HashMap<u32, &'a ProcInfo>) -> Vec<&'a ProcInfo> {
    let mut out = Vec::new();
    let mut seen = HashSet::from([p.pid]);
    let (mut ppid, mut child_start) = (p.ppid, p.start_ms);
    for _ in 0..64 {
        let Some(parent) = by_pid.get(&ppid) else {
            break;
        };
        if !seen.insert(parent.pid) || (parent.start_ms > child_start && child_start != 0) {
            break;
        }
        out.push(*parent);
        ppid = parent.ppid;
        child_start = parent.start_ms;
    }
    out
}

/// Fill in session ids: ids the stores record for a pid first, then the
/// newest session of the same agent and cwd that was active after the
/// process started (`guessed`). One session per process.
pub fn attach_sessions(
    agents: &mut [RunningAgent],
    recorded: &HashMap<Agent, HashMap<u32, String>>,
    sessions: &[Found],
) {
    let mut used: HashSet<(Agent, String)> = HashSet::new();
    for a in agents.iter_mut() {
        let Some(agent) = a.agent else { continue };
        if a.session_id.is_none()
            && let Some(rec) = recorded.get(&agent)
        {
            a.session_id = a
                .pids
                .iter()
                .chain(std::iter::once(&a.pid))
                .find_map(|pid| rec.get(pid).cloned());
        }
        if let Some(id) = &a.session_id {
            used.insert((agent, id.clone()));
        }
    }
    for a in agents.iter_mut() {
        let (Some(agent), None, Some(cwd)) = (a.agent, &a.session_id, &a.cwd) else {
            continue;
        };
        let cwd = norm_path(cwd);
        let best = sessions
            .iter()
            .filter(|s| {
                s.agent == agent
                    && norm_path(&s.meta.cwd) == cwd
                    && !used.contains(&(agent, s.meta.id.clone()))
            })
            .filter(|s| last_activity(s) + 2_000 >= a.start_ms)
            .max_by_key(|s| last_activity(s));
        if let Some(s) = best {
            used.insert((agent, s.meta.id.clone()));
            a.session_id = Some(s.meta.id.clone());
            a.guessed = true;
        }
    }
}

/// Mark agents that run inside a Pod terminal: one of the terminal's pids
/// (holder or child) is the agent itself or an ancestor.
pub fn mark_pod_terminals(
    agents: &mut [RunningAgent],
    procs: &[ProcInfo],
    pod: &[(String, Vec<u32>)],
) {
    let by_pid: HashMap<u32, &ProcInfo> = procs.iter().map(|p| (p.pid, p)).collect();
    let owner: HashMap<u32, &str> = pod
        .iter()
        .flat_map(|(id, pids)| pids.iter().map(move |p| (*p, id.as_str())))
        .collect();
    for a in agents.iter_mut() {
        if let Some(id) = owner.get(&a.pid) {
            a.pod_terminal = Some(id.to_string());
            continue;
        }
        let Some(p) = by_pid.get(&a.pid) else {
            continue;
        };
        a.pod_terminal = ancestors(p, &by_pid)
            .iter()
            .find_map(|anc| owner.get(&anc.pid))
            .map(|s| s.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SessionMeta;

    const A: &str = "0f8b6a3e-1111-4222-8333-944445555666";
    const B: &str = "1f8b6a3e-1111-4222-8333-944445555666";

    fn p(pid: u32, ppid: u32, name: &str, start: u64, cmd: &str, cwd: &str) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            name: name.into(),
            start_ms: start,
            cmdline: Some(cmd.into()),
            cwd: (!cwd.is_empty()).then(|| cwd.into()),
        }
    }

    fn found(agent: Agent, id: &str, cwd: &str, last: u64) -> Found {
        Found {
            agent,
            meta: SessionMeta {
                id: id.into(),
                cwd: cwd.into(),
                last_ms: Some(last),
                ..Default::default()
            },
            path: Default::default(),
            mtime_ms: last,
        }
    }

    #[test]
    fn splits_command_lines() {
        assert_eq!(
            split_cmdline(r#""C:\Program Files\x\codex.exe" resume  --no-daemon "a b" c\"d"#),
            [
                r"C:\Program Files\x\codex.exe",
                "resume",
                "--no-daemon",
                "a b",
                "c\"d"
            ]
        );
        assert_eq!(split_cmdline(r#"a """#), ["a", ""]);
        assert!(split_cmdline("   ").is_empty());
    }

    #[test]
    fn finds_interactive_agents_and_collapses_shims() {
        let procs = vec![
            p(10, 1, "explorer.exe", 1, "explorer.exe", ""),
            // npm shim (node) -> native codex: one agent, the outer process.
            p(
                20,
                10,
                "node.exe",
                100,
                r#"node "C:\npm\node_modules\@openai\codex\bin\codex.js" resume ABCDEF00-1111-4222-8333-944445555666"#,
                r"C:\src\a",
            ),
            p(
                21,
                20,
                "codex.exe",
                101,
                r#"codex.exe resume ABCDEF00-1111-4222-8333-944445555666"#,
                r"C:\src\a",
            ),
            // Non-interactive helpers are not sessions.
            p(
                30,
                10,
                "codex.exe",
                100,
                "codex app-server --listen stdio",
                "",
            ),
            p(
                31,
                10,
                "codex.exe",
                100,
                "codex -c model=\"x\" exec fix",
                "",
            ),
            p(32, 10, "claude.exe", 100, "claude -p hello", ""),
            p(33, 10, "claude.exe", 100, "claude mcp serve", ""),
            // Interactive: a value after -c is not a subcommand.
            p(
                40,
                10,
                "codex.exe",
                200,
                "codex -c review=1 \"fix the build\"",
                r"C:\src\b",
            ),
            p(
                50,
                10,
                "claude.exe",
                300,
                "claude --model opus",
                r"C:\src\c",
            ),
            p(60, 10, "node.exe", 300, "node server.js", r"C:\src\d"),
            // The desktop app shares the name; neither it nor its helpers are sessions.
            p(
                70,
                10,
                "claude.exe",
                300,
                r#""C:\Program Files\WindowsApps\Claude_1.0.0.0_x64__x\app\Claude.exe""#,
                "",
            ),
            p(
                71,
                1,
                "claude.exe",
                300,
                r#""C:\Apps\Claude.exe" --type=renderer --user-data-dir=x"#,
                "",
            ),
        ];
        let agents = find_agents(&procs);
        type Row<'a> = (u32, Option<Agent>, Vec<u32>, Option<&'a str>);
        let summary: Vec<Row> = agents
            .iter()
            .map(|a| (a.pid, a.agent, a.pids.clone(), a.session_id.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    20,
                    Some(Agent::Codex),
                    vec![20, 21],
                    Some("abcdef00-1111-4222-8333-944445555666")
                ),
                (40, Some(Agent::Codex), vec![40], None),
                (50, Some(Agent::Claude), vec![50], None),
            ]
        );
        assert_eq!(agents[0].cwd.as_deref(), Some(r"C:\src\a"));
    }

    #[test]
    fn reused_parent_pid_does_not_collapse() {
        // pid 20 was reused by a codex started after the child: not its parent.
        let procs = vec![
            p(20, 1, "codex.exe", 500, "codex", ""),
            p(21, 20, "codex.exe", 100, "codex", ""),
        ];
        assert_eq!(find_agents(&procs).len(), 2);
    }

    #[test]
    fn sessions_from_records_then_guesses() {
        let mut agents = vec![
            RunningAgent {
                agent: Some(Agent::Claude),
                pid: 5,
                pids: vec![5, 6],
                start_ms: 1000,
                cwd: Some(r"C:\x".into()),
                ..Default::default()
            },
            RunningAgent {
                agent: Some(Agent::Codex),
                pid: 7,
                pids: vec![7],
                start_ms: 5000,
                cwd: Some(r"c:\y\".into()),
                ..Default::default()
            },
            RunningAgent {
                agent: Some(Agent::Codex),
                pid: 8,
                pids: vec![8],
                start_ms: 9000,
                cwd: Some(r"C:\z".into()),
                ..Default::default()
            },
        ];
        let recorded = HashMap::from([(Agent::Claude, HashMap::from([(6u32, A.to_string())]))]);
        let sessions = vec![
            found(Agent::Codex, A, r"C:\Y", 4000), // before the process started: not it
            found(Agent::Codex, B, r"C:\Y", 6000), // it
            found(Agent::Codex, "other", r"C:\z", 1000), // too old for pid 8
        ];
        attach_sessions(&mut agents, &recorded, &sessions);
        assert_eq!(
            (agents[0].session_id.as_deref(), agents[0].guessed),
            (Some(A), false)
        );
        assert_eq!(
            (agents[1].session_id.as_deref(), agents[1].guessed),
            (Some(B), true)
        );
        assert_eq!(agents[2].session_id, None);
    }

    #[test]
    fn dedupes_against_pod_terminals() {
        let procs = vec![
            p(100, 1, "pod-holder.exe", 10, "", ""),
            p(101, 100, "cmd.exe", 11, "", ""),
            p(102, 101, "claude.exe", 12, "claude", r"C:\w"),
            p(200, 1, "WindowsTerminal.exe", 10, "", ""),
            p(201, 200, "claude.exe", 12, "claude", r"C:\w"),
        ];
        let mut agents = find_agents(&procs);
        mark_pod_terminals(&mut agents, &procs, &[("t3".into(), vec![100, 101])]);
        let pod: Vec<(u32, Option<&str>)> = agents
            .iter()
            .map(|a| (a.pid, a.pod_terminal.as_deref()))
            .collect();
        assert_eq!(pod, [(102, Some("t3")), (201, None)]);
    }
}
