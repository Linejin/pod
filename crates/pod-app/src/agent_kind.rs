//! Which AI service runs in a terminal (docs/10 §D5.11): one identity function
//! for every surface that lists agents (sidebar, agent tree, kanban/queue/rail,
//! orchestration workers, discovered sessions, terminal headers).
//!
//! Evidence, strongest first: a hook message (authenticated, names its agent) >
//! the program's own OSC 7501 `app=` > the task's agent (its first terminal
//! only; later ones are extra shells) > the launch argv > podd's foreground
//! process guess. Nothing → a plain shell.
use gpui_kit::*;

use crate::qa_marks::QaId;
use crate::vm::{TaskVm, TerminalVm};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgentKind {
    Claude,
    Codex,
    Gemini,
    Aider,
    /// Another self-reporting agent (OSC `app=`, task agent), by its name.
    OtherAgent(String),
    /// No agent: a plain terminal.
    Shell,
}

/// Where the identity came from, strongest first (`Ord`: smaller wins).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdSource {
    Hook,
    Osc,
    Task,
    Argv,
    Process,
    /// Nothing named an agent.
    Fallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentIdentity {
    pub kind: AgentKind,
    pub source: IdSource,
}

/// Programs that run something else: look at their arguments.
const WRAPPERS: [&str; 13] = [
    "cmd", "node", "npx", "pnpm", "bun", "bunx", "deno", "python", "python3", "py", "uv", "uvx",
    "pipx",
];
/// npm/pip packages of the CLIs, for `node …/cli.js` launches.
const PACKAGES: [(&str, AgentKind); 4] = [
    ("@anthropic-ai/claude-code", AgentKind::Claude),
    ("@openai/codex", AgentKind::Codex),
    ("@google/gemini-cli", AgentKind::Gemini),
    ("aider-chat", AgentKind::Aider),
];

fn stem(s: &str) -> String {
    let base = s
        .trim()
        .trim_matches('"')
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(s)
        .to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".com", ".ps1", ".js", ".mjs", ".py"] {
        if let Some(x) = base.strip_suffix(ext) {
            return x.to_string();
        }
    }
    base
}

impl AgentKind {
    /// A known agent CLI by program / app / task-agent name (`claude.cmd`, `codex-cli`).
    pub fn known(name: &str) -> Option<Self> {
        match stem(name).as_str() {
            "claude" | "claude-code" | "claude_code" => Some(Self::Claude),
            "codex" | "codex-cli" => Some(Self::Codex),
            "gemini" | "gemini-cli" => Some(Self::Gemini),
            "aider" | "aider-chat" => Some(Self::Aider),
            _ => None,
        }
    }

    /// A self-reported agent name (hook, OSC `app=`, task agent): a known CLI or
    /// [`Self::OtherAgent`]. Shell names and empty values are not agents.
    pub fn reported(name: &str) -> Option<Self> {
        let n = name.trim();
        if n.is_empty() || n == "?" || is_shell(n) {
            return None;
        }
        Some(Self::known(n).unwrap_or_else(|| Self::OtherAgent(n.to_string())))
    }

    /// Display name (Latin service names are not translated).
    pub fn name(&self) -> String {
        match self {
            Self::Claude => "Claude".into(),
            Self::Codex => "Codex".into(),
            Self::Gemini => "Gemini".into(),
            Self::Aider => "Aider".into(),
            Self::OtherAgent(n) => {
                let mut c = n.chars();
                c.next()
                    .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                    .unwrap_or_default()
            }
            Self::Shell => crate::strings::SHELL.into(),
        }
    }

    /// Accessible name and tooltip: `Claude 에이전트`, `셸 터미널`.
    pub fn accessible_name(&self) -> String {
        match self {
            Self::Shell => crate::strings::AGENT_SHELL_TERMINAL.into(),
            k => format!("{} {}", k.name(), crate::strings::AGENT_SUFFIX),
        }
    }

    pub fn is_agent(&self) -> bool {
        *self != Self::Shell
    }

    pub fn mark(&self) -> pod_ui::AgentMark {
        match self {
            Self::Claude => pod_ui::AgentMark::Claude,
            Self::Codex => pod_ui::AgentMark::Codex,
            Self::Gemini => pod_ui::AgentMark::Gemini,
            Self::Aider => pod_ui::AgentMark::Aider,
            Self::OtherAgent(_) => pod_ui::AgentMark::Generic,
            Self::Shell => pod_ui::AgentMark::Shell,
        }
    }

    /// Words the sidebar search matches for this kind (lowercase).
    pub fn search_terms(&self) -> String {
        match self {
            Self::Claude => "claude 클로드".into(),
            Self::Codex => "codex 코덱스".into(),
            Self::Gemini => "gemini 제미나이".into(),
            Self::Aider => "aider 에이더".into(),
            Self::OtherAgent(n) => format!("{} agent 에이전트", n.to_lowercase()),
            Self::Shell => "shell 셸 터미널".into(),
        }
    }

    /// Discovered sessions (pod-discover) know their tool.
    pub fn of_discovered(a: podd::protocol::DiscoveredAgent) -> Self {
        match a {
            podd::protocol::DiscoveredAgent::Claude => Self::Claude,
            podd::protocol::DiscoveredAgent::Codex => Self::Codex,
        }
    }
}

fn is_shell(name: &str) -> bool {
    matches!(
        stem(name).as_str(),
        "shell"
            | "terminal"
            | "pwsh"
            | "powershell"
            | "cmd"
            | "bash"
            | "sh"
            | "zsh"
            | "fish"
            | "nu"
            | "wsl"
            | "git-bash"
            | "conhost"
            | "openconsole"
    )
}

/// A task's agent (podd `AgentSpec`): `claude`, `codex`, `shell` or a command line.
/// A command that names no known CLI is still the user's chosen agent.
pub fn from_task_agent(spec: &str) -> Option<AgentKind> {
    let words: Vec<String> = spec.split_whitespace().map(str::to_owned).collect();
    let first = words.first()?;
    if let Some(k) = from_argv(&words) {
        return Some(k);
    }
    let name = stem(first);
    (!is_shell(&name) && !WRAPPERS.contains(&name.as_str()) && name != "?")
        .then_some(AgentKind::OtherAgent(name))
}

/// The agent a launch command runs (`claude --x`, `cmd /c codex`, `node …/@openai/codex/bin.js`).
pub fn from_argv(argv: &[String]) -> Option<AgentKind> {
    let first = argv.first()?;
    if let Some(k) = AgentKind::known(first) {
        return Some(k);
    }
    if !WRAPPERS.contains(&stem(first).as_str()) {
        return None;
    }
    for arg in argv.iter().skip(1) {
        let a = arg.replace('\\', "/").to_ascii_lowercase();
        if let Some((_, k)) = PACKAGES.iter().find(|(p, _)| a.contains(p)) {
            return Some(k.clone());
        }
        // `cmd /c "claude --resume x"`: the first word of a quoted command.
        if let Some(k) = a.split_whitespace().next().and_then(AgentKind::known) {
            return Some(k);
        }
    }
    None
}

/// The identity of terminal `t` (its task, if known, for the task's agent).
pub fn identify(t: &TerminalVm, task: Option<&TaskVm>) -> AgentIdentity {
    let task_agent = task
        .filter(|k| {
            k.terminals.first() == Some(&t.id)
                || (t.task_id.as_deref() == Some(k.id.as_str()) && k.terminals.is_empty())
        })
        .and_then(|k| k.agent.as_deref());
    identify_parts(t.status.as_ref(), task_agent, &t.argv)
}

/// The precedence itself, from loose parts: live status (hook / OSC / process
/// guess), a task or orchestration agent spec, and the launch argv.
pub fn identify_parts(
    status: Option<&crate::vm::StatusVm>,
    task_agent: Option<&str>,
    argv: &[String],
) -> AgentIdentity {
    let found = |kind, source| Some(AgentIdentity { kind, source });
    let hook = status
        .filter(|s| s.source.as_deref() == Some("hook"))
        .and_then(|s| s.agent.as_deref())
        .and_then(AgentKind::reported);
    let osc = status
        .and_then(|s| s.app.as_deref())
        .and_then(AgentKind::reported);
    let process = status
        .and_then(|s| s.agent.as_deref())
        .and_then(AgentKind::reported);
    hook.and_then(|k| found(k, IdSource::Hook))
        .or_else(|| osc.and_then(|k| found(k, IdSource::Osc)))
        .or_else(|| {
            task_agent
                .and_then(from_task_agent)
                .and_then(|k| found(k, IdSource::Task))
        })
        .or_else(|| from_argv(argv).and_then(|k| found(k, IdSource::Argv)))
        .or_else(|| process.and_then(|k| found(k, IdSource::Process)))
        .unwrap_or(AgentIdentity {
            kind: AgentKind::Shell,
            source: IdSource::Fallback,
        })
}

/// [`identify`] with the task looked up in `tasks`.
pub fn identify_in(t: &TerminalVm, tasks: &[TaskVm]) -> AgentIdentity {
    let task = t
        .task_id
        .as_deref()
        .and_then(|id| tasks.iter().find(|k| k.id == id))
        .or_else(|| tasks.iter().find(|k| k.terminals.contains(&t.id)));
    identify(t, task)
}

/// Distinct agent kinds of a task's terminals, in first-seen order (shells only if nothing else).
pub fn task_kinds(task: &TaskVm, terminals: &[TerminalVm]) -> Vec<AgentKind> {
    let mut out: Vec<AgentKind> = Vec::new();
    for id in &task.terminals {
        if let Some(t) = terminals.iter().find(|t| &t.id == id) {
            let k = identify(t, Some(task)).kind;
            if k.is_agent() && !out.contains(&k) {
                out.push(k);
            }
        }
    }
    if out.is_empty()
        && let Some(k) = task.agent.as_deref().and_then(from_task_agent)
    {
        out.push(k);
    }
    out
}

/// The mark as a named image, recorded for render tests as
/// `agent-mark:<place>:<accessible name>` (docs/10 §D5.11).
pub fn mark_el(place: &str, kind: &AgentKind, size: f32) -> Stateful<Div> {
    let name = kind.accessible_name();
    let key = format!("agent-mark:{place}:{name}");
    pod_ui::agent_mark_el(
        SharedString::from(format!("agent-mark-{place}")),
        kind.mark(),
        name,
        size,
        &crate::theme::tokens(),
    )
    .qa_id(move || key)
}

/// A task's agents as small side-by-side marks (`agent-stack:<place>:<label>`).
pub fn stack_el(place: &str, kinds: &[AgentKind], size: f32) -> Stateful<Div> {
    let marks: Vec<_> = kinds.iter().map(|k| (k.mark(), k.name())).collect();
    let key = format!(
        "agent-stack:{place}:{}",
        kinds.iter().map(|k| k.name()).collect::<Vec<_>>().join(",")
    );
    pod_ui::agent_stack(
        SharedString::from(format!("agent-stack-{place}")),
        &marks,
        3,
        size,
        &crate::theme::tokens(),
    )
    .qa_id(move || key)
}

#[cfg(test)]
mod tests {
    use super::*;
    // After the glob: GPUI exports its own `test` attribute; these are plain Rust tests.
    use crate::source::TestBackend;
    use crate::vm::StatusVm;
    use core::prelude::v1::test;

    fn term(argv: &[&str]) -> TerminalVm {
        let mut t = TestBackend::terminal("t1", "작업자", None, "working");
        t.argv = argv.iter().map(|s| s.to_string()).collect();
        t.program = crate::vm::program_label(&t.argv);
        t
    }
    fn with_status(
        mut t: TerminalVm,
        source: Option<&str>,
        agent: Option<&str>,
        app: Option<&str>,
    ) -> TerminalVm {
        let s = t
            .status
            .get_or_insert_with(|| StatusVm::of(crate::vm::AgentState::Working));
        s.source = source.map(str::to_owned);
        s.agent = agent.map(str::to_owned);
        s.app = app.map(str::to_owned);
        t
    }
    fn task(agent: &str, terminals: &[&str]) -> TaskVm {
        let mut k = TestBackend::task("k1", "작업", "sample", terminals);
        k.agent = Some(agent.into());
        k
    }
    fn id(kind: AgentKind, source: IdSource) -> AgentIdentity {
        AgentIdentity { kind, source }
    }

    #[test]
    fn argv_source_unwraps_shims_and_wrappers() {
        let cases: [(&[&str], Option<AgentKind>); 9] = [
            (&[r"C:\nvm\claude.cmd", "--resume"], Some(AgentKind::Claude)),
            (&["codex.exe"], Some(AgentKind::Codex)),
            (
                &["cmd.exe", "/c", "codex --full-auto"],
                Some(AgentKind::Codex),
            ),
            (
                &[
                    "node",
                    r"C:\npm\node_modules\@anthropic-ai\claude-code\cli.js",
                ],
                Some(AgentKind::Claude),
            ),
            (
                &["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"],
                Some(AgentKind::Codex),
            ),
            (&["npx", "@google/gemini-cli"], Some(AgentKind::Gemini)),
            (&["python", "-m", "aider"], Some(AgentKind::Aider)),
            (&["pwsh.exe", "-NoLogo"], None),
            (&["python", "build.py"], None),
        ];
        for (argv, want) in cases {
            let t = term(argv);
            assert_eq!(from_argv(&t.argv), want, "{argv:?}");
            let source = if want.is_some() {
                IdSource::Argv
            } else {
                IdSource::Fallback
            };
            assert_eq!(
                identify(&t, None),
                id(want.unwrap_or(AgentKind::Shell), source),
                "{argv:?}"
            );
        }
        assert_eq!(from_argv(&[]), None);
    }

    #[test]
    fn hook_source_wins_over_everything() {
        let t = with_status(
            term(&["claude"]),
            Some("hook"),
            Some("codex"),
            Some("gemini"),
        );
        assert_eq!(
            identify(&t, Some(&task("aider", &["t1"]))),
            id(AgentKind::Codex, IdSource::Hook)
        );
        // podd's agent is a hook fact only when the state came from a hook.
        let osc = with_status(term(&["claude"]), Some("osc"), Some("codex"), None);
        assert_eq!(identify(&osc, None).source, IdSource::Argv);
    }

    #[test]
    fn osc_app_beats_task_and_argv_and_names_other_agents() {
        let t = with_status(term(&["pwsh"]), Some("osc"), None, Some("claude-code"));
        assert_eq!(
            identify(&t, Some(&task("codex", &["t1"]))),
            id(AgentKind::Claude, IdSource::Osc)
        );
        let other = with_status(term(&["pwsh"]), Some("osc"), None, Some("my-bot"));
        let who = identify(&other, None);
        assert_eq!(
            who,
            id(AgentKind::OtherAgent("my-bot".into()), IdSource::Osc)
        );
        assert_eq!(who.kind.accessible_name(), "My-bot 에이전트");
        assert_eq!(who.kind.mark(), pod_ui::AgentMark::Generic);
    }

    #[test]
    fn task_agent_applies_to_the_tasks_first_terminal_only() {
        let k = task("codex", &["t1", "t2"]);
        let first = term(&["pwsh"]);
        assert_eq!(
            identify(&first, Some(&k)),
            id(AgentKind::Codex, IdSource::Task)
        );
        let mut extra = term(&["pwsh"]);
        extra.id = "t2".into();
        assert_eq!(
            identify(&extra, Some(&k)),
            id(AgentKind::Shell, IdSource::Fallback),
            "an added shell stays a shell"
        );
        // Task beats argv: a task launched through a wrapper the argv parser cannot read.
        let wrapped = term(&["launcher.exe", "--profile", "x"]);
        assert_eq!(
            identify(&wrapped, Some(&task("gemini", &["t1"]))),
            id(AgentKind::Gemini, IdSource::Task)
        );
        assert_eq!(
            identify(&term(&["claude"]), Some(&task("codex", &["t1"]))).kind,
            AgentKind::Codex,
            "task > argv"
        );
    }

    #[test]
    fn process_guess_is_the_last_evidence() {
        // `claude` started inside a PowerShell terminal: argv says shell, podd sees claude.
        let t = with_status(term(&["pwsh"]), Some("heuristic"), Some("claude"), None);
        assert_eq!(identify(&t, None), id(AgentKind::Claude, IdSource::Process));
        let argv_first = with_status(term(&["codex"]), Some("heuristic"), Some("claude"), None);
        assert_eq!(
            identify(&argv_first, None),
            id(AgentKind::Codex, IdSource::Argv)
        );
        let shell_guess = with_status(term(&["pwsh"]), Some("process"), Some("pwsh"), None);
        assert_eq!(
            identify(&shell_guess, None),
            id(AgentKind::Shell, IdSource::Fallback),
            "a shell name is not an agent"
        );
    }

    #[test]
    fn task_agent_specs() {
        assert_eq!(from_task_agent("claude"), Some(AgentKind::Claude));
        assert_eq!(from_task_agent("shell"), None);
        assert_eq!(
            from_task_agent(r"C:\bin\gemini.cmd --yolo"),
            Some(AgentKind::Gemini)
        );
        assert_eq!(
            from_task_agent("cmd /c aider --model x"),
            Some(AgentKind::Aider)
        );
        assert_eq!(
            from_task_agent("my-agent --serve"),
            Some(AgentKind::OtherAgent("my-agent".into()))
        );
        assert_eq!(from_task_agent("pwsh -NoLogo"), None);
        assert_eq!(from_task_agent(""), None);
        // The fixture tasks' `shell` agent never paints an agent mark.
        let shell_task = TestBackend::task("k", "작업", "sample", &["t1"]);
        assert_eq!(
            identify(&term(&["pwsh"]), Some(&shell_task)).kind,
            AgentKind::Shell
        );
    }

    #[test]
    fn names_marks_and_search_terms() {
        assert_eq!(AgentKind::Claude.accessible_name(), "Claude 에이전트");
        assert_eq!(AgentKind::Codex.accessible_name(), "Codex 에이전트");
        assert_eq!(AgentKind::Shell.accessible_name(), "셸 터미널");
        assert!(AgentKind::Claude.search_terms().contains("claude"));
        assert!(AgentKind::Codex.search_terms().contains("codex"));
        let marks: std::collections::HashSet<_> = [
            AgentKind::Claude,
            AgentKind::Codex,
            AgentKind::Gemini,
            AgentKind::Aider,
            AgentKind::OtherAgent("x".into()),
            AgentKind::Shell,
        ]
        .iter()
        .map(|k| k.mark())
        .collect();
        assert_eq!(marks.len(), 6);
        assert_eq!(AgentKind::reported("powershell"), None);
        assert_eq!(AgentKind::reported(""), None);
        assert_eq!(
            AgentKind::of_discovered(podd::protocol::DiscoveredAgent::Codex),
            AgentKind::Codex
        );
    }

    #[test]
    fn task_kinds_are_distinct_and_skip_shells() {
        let k = task("claude", &["t1", "t2", "t3"]);
        let a = term(&["claude"]);
        let mut b = term(&["codex"]);
        b.id = "t2".into();
        let mut c = term(&["pwsh"]);
        c.id = "t3".into();
        assert_eq!(
            task_kinds(&k, &[a.clone(), b, c.clone()]),
            [AgentKind::Claude, AgentKind::Codex]
        );
        let only_shell = task("codex", &["t3"]);
        assert_eq!(
            task_kinds(&only_shell, &[c]),
            [AgentKind::Codex],
            "first terminal takes the task agent"
        );
        assert_eq!(
            task_kinds(&task("codex", &[]), &[]),
            [AgentKind::Codex],
            "no terminals yet: the task's agent"
        );
    }
}
