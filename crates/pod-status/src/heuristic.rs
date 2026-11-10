//! L3 heuristic adapters (terminal title glyphs, screen patterns, quiet time)
//! and the L0 process adapter (foreground process → agent kind).
//!
//! All patterns live in `static` tables so they can be updated without
//! touching the logic. Entries carry `verified`: `true` only when the string
//! or glyph was observed at runtime (docs/04 §1.3–§1.4, §4.3); everything else
//! is documented-but-unobserved and may need adjusting.

use crate::types::{AgentKind, AgentState, BlockedKind, Millis};
use serde::{Deserialize, Serialize};

// ───────────────────────────── title glyphs ─────────────────────────────

/// What a title says about the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleSignal {
    /// An explicit idle mark (Claude `✳ `).
    Idle,
    /// A spinner glyph.
    Working,
    /// No indicator; for agents whose idle state is "no spinner" (Codex) this
    /// starts the quiet-time timer.
    NoIndicator,
}

/// Which agents a rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentTag {
    Claude,
    Codex,
    /// Claude or Codex.
    Any,
}

impl AgentTag {
    fn matches(self, agent: Option<&AgentKind>) -> bool {
        matches!(
            (self, agent),
            (AgentTag::Any, Some(AgentKind::Claude | AgentKind::Codex))
                | (AgentTag::Claude, Some(AgentKind::Claude))
                | (AgentTag::Codex, Some(AgentKind::Codex))
        )
    }
}

/// A title rule: the first non-space char of the title is tested.
#[derive(Debug, Clone, Copy)]
pub struct TitleRule {
    pub agent: AgentTag,
    /// Inclusive code point range of the leading glyph.
    pub first: char,
    pub last: char,
    pub signal: TitleSignal,
    pub verified: bool,
    pub note: &'static str,
}

/// Title glyph table, evaluated top to bottom; first match wins.
pub static TITLE_RULES: &[TitleRule] = &[
    TitleRule {
        agent: AgentTag::Claude,
        first: '\u{2733}',
        last: '\u{2733}',
        signal: TitleSignal::Idle,
        verified: true,
        note: "Claude idle mark `✳ ` (docs/04 §3.4, §4.3)",
    },
    TitleRule {
        agent: AgentTag::Any,
        first: '\u{2800}',
        last: '\u{28FF}',
        signal: TitleSignal::Working,
        verified: true,
        note: "Braille spinner; Codex observed `⠋ work` (docs/04 §4.3)",
    },
    TitleRule {
        agent: AgentTag::Any,
        first: '\u{25D0}',
        last: '\u{25D3}',
        signal: TitleSignal::Working,
        verified: false,
        note: "quarter-circle spinner ◐◑◒◓ (docs/04 §3.4, not observed)",
    },
];

/// Agents whose title shows no glyph when idle (absence of spinner → idle after quiet time).
pub static NO_GLYPH_IDLE_AGENTS: &[AgentTag] = &[AgentTag::Codex];

/// Classify a terminal title. `None` = this title says nothing for this agent.
pub fn classify_title(agent: Option<&AgentKind>, title: &str) -> Option<TitleSignal> {
    let first = title.trim_start().chars().next();
    if let Some(c) = first {
        for rule in TITLE_RULES {
            if rule.agent.matches(agent) && (rule.first..=rule.last).contains(&c) {
                return Some(rule.signal);
            }
        }
    }
    if NO_GLYPH_IDLE_AGENTS.iter().any(|t| t.matches(agent)) {
        return Some(TitleSignal::NoIndicator);
    }
    None
}

/// If the agent is not known yet, guess it from the title (Claude's `✳ ` mark).
pub fn agent_from_title(title: &str) -> Option<AgentKind> {
    let t = title.trim_start();
    if t.starts_with('\u{2733}') && t.contains("Claude") {
        return Some(AgentKind::Claude);
    }
    None
}

// ──────────────────────────── screen patterns ────────────────────────────

/// A screen-text pattern meaning "the agent waits for the user".
#[derive(Debug, Clone, Copy)]
pub struct ScreenPattern {
    pub agent: AgentTag,
    /// Lower-case substring searched in the lower-cased screen text.
    pub needle: &'static str,
    pub kind: BlockedKind,
    pub label: &'static str,
    pub verified: bool,
}

/// Screen pattern table; first match wins.
pub static SCREEN_PATTERNS: &[ScreenPattern] = &[
    // Claude folder-trust dialog (no OSC 7501 is sent while it is shown, docs/04 §1.4).
    ScreenPattern {
        agent: AgentTag::Claude,
        needle: "is this a project you trust",
        kind: BlockedKind::Permission,
        label: "trust",
        verified: true,
    },
    ScreenPattern {
        agent: AgentTag::Claude,
        needle: "quick safety check",
        kind: BlockedKind::Permission,
        label: "trust",
        verified: false,
    },
    ScreenPattern {
        agent: AgentTag::Claude,
        needle: "trust this folder",
        kind: BlockedKind::Permission,
        label: "trust",
        verified: false,
    },
    // Claude tool permission prompts (not observed; docs/04 §6).
    ScreenPattern {
        agent: AgentTag::Claude,
        needle: "do you want to proceed?",
        kind: BlockedKind::Permission,
        label: "permission",
        verified: false,
    },
    ScreenPattern {
        agent: AgentTag::Claude,
        needle: "do you want to make this edit",
        kind: BlockedKind::Permission,
        label: "permission",
        verified: false,
    },
    ScreenPattern {
        agent: AgentTag::Claude,
        needle: "do you want to create",
        kind: BlockedKind::Permission,
        label: "permission",
        verified: false,
    },
    // Codex trust dialog (dialog seen in docs/04 §5; exact wording from §4.3).
    ScreenPattern {
        agent: AgentTag::Codex,
        needle: "trust this folder?",
        kind: BlockedKind::Permission,
        label: "trust",
        verified: false,
    },
    // Codex approval prompts (not observed; docs/04 §6).
    ScreenPattern {
        agent: AgentTag::Codex,
        needle: "would you like to run the following command",
        kind: BlockedKind::Permission,
        label: "permission",
        verified: false,
    },
    ScreenPattern {
        agent: AgentTag::Codex,
        needle: "would you like to make the following edits",
        kind: BlockedKind::Permission,
        label: "permission",
        verified: false,
    },
    ScreenPattern {
        agent: AgentTag::Codex,
        needle: "allow command?",
        kind: BlockedKind::Permission,
        label: "permission",
        verified: false,
    },
];

/// Find a blocking prompt in the visible screen text.
pub fn match_screen(agent: Option<&AgentKind>, screen: &str) -> Option<&'static ScreenPattern> {
    let lower = screen.to_lowercase();
    SCREEN_PATTERNS
        .iter()
        .find(|p| p.agent.matches(agent) && lower.contains(p.needle))
}

// ───────────────────────────── process adapter ─────────────────────────────

/// Foreground process of a terminal, as podd sees it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: Option<u32>,
    /// Image name, e.g. `claude.exe`, `cmd.exe`.
    pub name: String,
    pub argv: Vec<String>,
}

impl ProcessInfo {
    pub fn new(name: &str, argv: &[&str]) -> Self {
        Self {
            pid: None,
            name: name.into(),
            argv: argv.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Executable stems that are the agent itself.
pub static AGENT_EXES: &[(&str, AgentTag)] =
    &[("claude", AgentTag::Claude), ("codex", AgentTag::Codex)];

/// Wrapper processes (cmd shims, interpreters) whose argv is searched for the agent.
pub static WRAPPER_EXES: &[&str] = &[
    "cmd",
    "node",
    "bun",
    "deno",
    "pwsh",
    "powershell",
    "npx",
    "bunx",
];

/// Package paths that identify an agent when run through an interpreter.
pub static AGENT_PACKAGES: &[(&str, AgentTag)] = &[
    ("@anthropic-ai/claude-code", AgentTag::Claude),
    ("@openai/codex", AgentTag::Codex),
];

/// Lower-cased file stem without directory and `.exe/.cmd/.bat/.ps1/.js` suffix.
fn stem(path: &str) -> String {
    let base = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase();
    for ext in [".exe", ".cmd", ".bat", ".ps1", ".js", ".mjs"] {
        if let Some(s) = base.strip_suffix(ext) {
            return s.to_string();
        }
    }
    base
}

fn tag_kind(t: AgentTag) -> Option<AgentKind> {
    match t {
        AgentTag::Claude => Some(AgentKind::Claude),
        AgentTag::Codex => Some(AgentKind::Codex),
        AgentTag::Any => None,
    }
}

/// Detect Claude/Codex from a foreground process, including `cmd.exe /c claude.CMD`
/// npm shims and `node …/@anthropic-ai/claude-code/cli.js`.
pub fn detect_agent(p: &ProcessInfo) -> Option<AgentKind> {
    let name = stem(&p.name);
    if let Some((_, t)) = AGENT_EXES.iter().find(|(s, _)| *s == name) {
        return tag_kind(*t);
    }
    if !WRAPPER_EXES.contains(&name.as_str()) {
        return None;
    }
    for arg in p.argv.iter().skip(1) {
        let a = arg.replace('\\', "/").to_ascii_lowercase();
        if let Some((_, t)) = AGENT_PACKAGES.iter().find(|(pkg, _)| a.contains(pkg)) {
            return tag_kind(*t);
        }
        let s = stem(arg);
        if let Some((_, t)) = AGENT_EXES.iter().find(|(x, _)| *x == s) {
            return tag_kind(*t);
        }
    }
    None
}

// ───────────────────────────── tracker ─────────────────────────────

/// Confidence values of heuristic evidence.
pub const CONF_TITLE: f32 = 0.6;
pub const CONF_SCREEN: f32 = 0.7;
pub const CONF_QUIET: f32 = 0.4;

/// One heuristic conclusion.
#[derive(Debug, Clone, PartialEq)]
pub struct HeuristicVerdict {
    pub state: AgentState,
    pub confidence: f32,
    pub at: Millis,
    pub msg: Option<String>,
}

/// Stateful combination of title, screen and quiet-time signals for one terminal.
///
/// Rules:
/// - spinner glyph → `working`;
/// - explicit idle mark → `done` if a working phase was seen since the last
///   idle/done, otherwise `idle`;
/// - no-glyph agents (Codex): no spinner and no output for `quiet_ms` → same
///   done/idle choice (evaluated in [`HeuristicTracker::tick`]);
/// - a matching screen pattern overrides everything with `blocked{kind}` while
///   it stays on screen.
#[derive(Debug, Clone, Default)]
pub struct HeuristicTracker {
    title_verdict: Option<HeuristicVerdict>,
    screen: Option<(BlockedKind, Millis, &'static str)>,
    saw_working: bool,
    /// Set when a no-glyph agent's title lost its spinner (or has none).
    quiet_from: Option<Millis>,
    last_output: Millis,
}

impl HeuristicTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// The current conclusion, if any.
    pub fn verdict(&self) -> Option<HeuristicVerdict> {
        if let Some((kind, at, label)) = self.screen {
            return Some(HeuristicVerdict {
                state: AgentState::Blocked(kind),
                confidence: CONF_SCREEN,
                at,
                msg: Some(label.to_string()),
            });
        }
        self.title_verdict.clone()
    }

    fn settle(&mut self, now: Millis, conf: f32) {
        let state = if self.saw_working {
            AgentState::Done
        } else {
            AgentState::Idle
        };
        self.saw_working = false;
        self.title_verdict = Some(HeuristicVerdict {
            state,
            confidence: conf,
            at: now,
            msg: None,
        });
    }

    /// A title change. Returns true if the verdict changed.
    pub fn on_title(&mut self, agent: Option<&AgentKind>, title: &str, now: Millis) -> bool {
        let before = self.verdict().map(|v| v.state);
        match classify_title(agent, title) {
            Some(TitleSignal::Working) => {
                self.saw_working = true;
                self.quiet_from = None;
                self.title_verdict = Some(HeuristicVerdict {
                    state: AgentState::Working,
                    confidence: CONF_TITLE,
                    at: now,
                    msg: None,
                });
            }
            Some(TitleSignal::Idle) => {
                self.quiet_from = None;
                let already_idle = matches!(
                    self.title_verdict.as_ref().map(|v| v.state),
                    Some(AgentState::Idle | AgentState::Done)
                );
                if !already_idle || self.saw_working {
                    self.settle(now, CONF_TITLE);
                } else if let Some(v) = self.title_verdict.as_mut() {
                    v.at = now; // re-confirmed
                }
            }
            Some(TitleSignal::NoIndicator) => {
                self.quiet_from.get_or_insert(now);
            }
            None => {}
        }
        before != self.verdict().map(|v| v.state)
    }

    /// Visible screen text changed. Returns true if the verdict changed.
    pub fn on_screen(&mut self, agent: Option<&AgentKind>, screen: &str, now: Millis) -> bool {
        let before = self.verdict().map(|v| v.state);
        match match_screen(agent, screen) {
            Some(p) => {
                if self.screen.map(|(k, _, l)| (k, l)) != Some((p.kind, p.label)) {
                    self.screen = Some((p.kind, now, p.label));
                }
            }
            None => self.screen = None,
        }
        before != self.verdict().map(|v| v.state)
    }

    /// Any PTY output (resets the quiet timer).
    pub fn on_output(&mut self, now: Millis) {
        self.last_output = now;
    }

    /// The agent became known; no-glyph agents start their quiet timer.
    pub fn on_agent(&mut self, agent: Option<&AgentKind>, now: Millis) {
        if NO_GLYPH_IDLE_AGENTS.iter().any(|t| t.matches(agent))
            && self.quiet_from.is_none()
            && !matches!(
                self.title_verdict.as_ref().map(|v| v.state),
                Some(AgentState::Working)
            )
        {
            self.quiet_from = Some(now);
        }
    }

    /// Time passes. Returns true if the verdict changed.
    pub fn tick(&mut self, now: Millis, quiet_ms: Millis) -> bool {
        let Some(from) = self.quiet_from else {
            return false;
        };
        let quiet_since = from.max(self.last_output);
        if now.saturating_sub(quiet_since) < quiet_ms {
            return false;
        }
        let before = self.verdict().map(|v| v.state);
        self.quiet_from = None;
        let is_settled = matches!(
            self.title_verdict.as_ref().map(|v| v.state),
            Some(AgentState::Idle | AgentState::Done)
        );
        if !is_settled || self.saw_working {
            self.settle(now, CONF_QUIET);
        }
        before != self.verdict().map(|v| v.state)
    }

    /// The user acknowledged a finished turn.
    pub fn ack(&mut self) {
        if let Some(v) = self.title_verdict.as_mut()
            && v.state == AgentState::Done
        {
            v.state = AgentState::Idle;
        }
    }
}
