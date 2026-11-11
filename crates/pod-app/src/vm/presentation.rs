//! Display methods of the view models: labels, summaries and counts as the
//! sidebar, panels and status bar show them. Paths are unchanged
//! (`AgentState::label`, `crate::vm::counts`, `crate::vm::program_label` …).

use super::{AgentState, Counts, Liveness, StatusVm, TaskVm, TerminalVm};

impl AgentState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => crate::strings::IDLE,
            Self::Working => crate::strings::WORKING,
            Self::Blocked => crate::strings::BLOCKED,
            Self::Done => crate::strings::DONE,
            Self::Error => crate::strings::ERROR,
            Self::Unknown => crate::strings::UNKNOWN,
            Self::Exited => crate::strings::EXITED,
        }
    }
}

impl StatusVm {
    /// Label for the sidebar: podd's badge, else state (+ kind).
    pub fn label(&self) -> String {
        crate::orch::status_badge(Some(self)).label()
    }
}

impl TerminalVm {
    /// Why a lost terminal cannot be used, for its tooltip and accessible
    /// name: only unreachable (v19) terminals have one.
    pub fn lost_reason(&self) -> Option<String> {
        let reason = self.unreachable.as_deref()?.trim();
        Some(if reason.is_empty() {
            crate::strings::UNREACHABLE.into()
        } else {
            crate::tr!(UNREACHABLE_R; ; reason)
        })
    }

    pub fn live_label(&self) -> String {
        match self.live {
            Liveness::Running => crate::strings::LIVE.into(),
            Liveness::Exited { killed: true, .. } => crate::strings::KILLED.into(),
            Liveness::Exited { code: Some(c), .. } => crate::tr!(EXITED_C_342; c;),
            Liveness::Exited { code: None, .. } => crate::strings::EXITED.into(),
            Liveness::Lost => crate::strings::LOST.into(),
        }
    }
}

/// `C:\x\claude.cmd --foo` -> `claude`; `cmd.exe /c claude` -> `claude`.
pub fn program_label(argv: &[String]) -> String {
    let stem = |s: &str| {
        let base = s.rsplit(['\\', '/']).next().unwrap_or(s);
        let lower = base.to_ascii_lowercase();
        for ext in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
            if let Some(x) = lower.strip_suffix(ext) {
                return x.to_string();
            }
        }
        lower
    };
    let Some(first) = argv.first() else {
        return "?".into();
    };
    let first = stem(first);
    // Wrapper shells: show what they run.
    if first == "cmd"
        && let Some(i) = argv
            .iter()
            .position(|a| a.eq_ignore_ascii_case("/c") || a.eq_ignore_ascii_case("/k"))
        && let Some(next) = argv.get(i + 1)
    {
        return stem(next.split_whitespace().next().unwrap_or(next));
    }
    first
}

impl TaskVm {
    /// One-line git summary for the sidebar: `↑1 ↓0 · 2 dirty`.
    pub fn git_summary(&self) -> Option<String> {
        let g = self.git.as_ref()?;
        let mut s = format!("↑{} ↓{}", g.ahead, g.behind);
        if g.dirty > 0 {
            s.push_str(&crate::tr!(DIRTY_343; ; g.dirty));
        }
        Some(s)
    }
}

/// Counts for the status bar.
pub fn counts(terminals: &[TerminalVm]) -> Counts {
    let state = |s: AgentState| {
        terminals
            .iter()
            .filter(|t| t.status.as_ref().is_some_and(|x| x.state == s))
            .count()
    };
    Counts {
        total: terminals.len(),
        running: terminals.iter().filter(|t| t.is_running()).count(),
        working: state(AgentState::Working),
        blocked: state(AgentState::Blocked),
        done: state(AgentState::Done),
    }
}
