//! The dense layout's pure plans: what the 세부 정보 column shows and which
//! actions it offers, the F6 region and 출력 keyboard steps, and the sizes
//! that grow with the UI font. No GPUI state; the style comes from
//! `pod_ui::themes::metrics()` as in every mode, and "now" is an argument.
use pod_ui::{ControlState, Icon, Tone};
use podd::protocol::{self as p, GateDecision, GateInfo, GateState};

use super::{LOG_ROWS, copy};
use crate::layouts::kit::UiIntent;
use crate::source::RunSpec;
use crate::vm::{TaskVm, TerminalVm};

// ───────────────────────────── sizes ─────────────────────────────

/// Row height: the style's row, grown with the font so text is never clipped.
pub fn row_height(ui: f32) -> f32 {
    pod_ui::themes::metrics()
        .density
        .row
        .max((ui * 1.5).ceil() + 4.)
}

/// Height of a panel header (출력, 세부 정보): the style's, at least a row.
pub(crate) fn header_height(ui: f32) -> f32 {
    pod_ui::themes::metrics()
        .density
        .panel_header
        .max(row_height(ui))
}

/// Height of the 출력 panel (header + [`LOG_ROWS`] rows).
pub fn output_height(ui: f32) -> f32 {
    let m = pod_ui::themes::metrics();
    let row = row_height(ui);
    m.density.panel_header.max(row) + LOG_ROWS as f32 * row + m.border_width + 4.
}

/// Width of the 세부 정보 column.
pub fn details_width(ui: f32) -> f32 {
    (pod_ui::themes::metrics().density.sidebar + 24.).max(ui * 20.)
}

// ───────────────────────────── selection ─────────────────────────────

/// What the 세부 정보 column shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Sel {
    Task(String),
    Terminal(String),
}

impl Sel {
    /// From a sidebar row key (`task:<id>` / `terminal:<id>`); repository rows select nothing.
    pub fn from_sidebar_key(k: &str) -> Option<Self> {
        if let Some(id) = k.strip_prefix("task:") {
            Some(Self::Task(id.into()))
        } else {
            k.strip_prefix("terminal:")
                .map(|id| Self::Terminal(id.into()))
        }
    }
}

/// The selection the column shows: nothing when its task or terminal vanished.
pub(crate) fn shown_selection(
    sel: Option<&Sel>,
    tasks: &[TaskVm],
    terminals: &[TerminalVm],
) -> Option<Sel> {
    sel.filter(|s| match s {
        Sel::Task(id) => tasks.iter().any(|t| &t.id == id),
        Sel::Terminal(id) => terminals.iter().any(|t| &t.id == id),
    })
    .cloned()
}

/// Status key of a terminal: 마지막 줄 is re-read only when it changes.
pub(crate) fn line_key(t: &TerminalVm) -> String {
    let s = t.status.as_ref();
    format!(
        "{:?}|{}|{}|{:?}",
        s.map(|s| s.state),
        s.map_or(0, |s| s.since),
        s.map_or(0, |s| s.turn),
        t.live
    )
}

/// The log follows its newest line unless the user selected an older one.
pub(crate) fn at_tail(selected: Option<u64>, newest: Option<u64>) -> bool {
    selected.is_none_or(|s| newest.is_none_or(|n| n == s))
}

// ───────────────────────────── keyboard ─────────────────────────────

/// The regions F6 cycles through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Tree,
    Dock,
    Output,
    Details,
}

/// F6 / Shift+F6 over `n` shown regions (`n > 0`) from the one holding focus;
/// from outside them, the first.
pub(crate) fn cycle_step(at: Option<usize>, n: usize, forward: bool) -> usize {
    match at {
        Some(i) if forward => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        None => 0,
    }
}

/// A key on the 출력 log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogKey {
    /// Select this line.
    Select(usize),
    /// Enter / Space: the selected line (if any) into the details column;
    /// `open` (Enter) also opens its terminal. The key is taken either way.
    Activate { line: Option<usize>, open: bool },
}

/// ↑↓ Home End PageUp PageDown select, Enter / Space activate; `at` is the
/// selected line of `len` visible ones. `None`: not the log's key (or no lines).
pub(crate) fn log_key(key: &str, at: Option<usize>, len: usize) -> Option<LogKey> {
    if len == 0 {
        return None;
    }
    let last = len - 1;
    let line = match key {
        "down" => at.map_or(last, |i| (i + 1).min(last)),
        "up" => at.map_or(last, |i| i.saturating_sub(1)),
        "home" => 0,
        "end" => last,
        "pagedown" => at.map_or(last, |i| (i + LOG_ROWS).min(last)),
        "pageup" => at.map_or(0, |i| i.saturating_sub(LOG_ROWS)),
        "enter" | "space" => {
            return Some(LogKey::Activate {
                line: at,
                open: key == "enter",
            });
        }
        _ => return None,
    };
    Some(LogKey::Select(line))
}

// ───────────────────────────── activity bar ─────────────────────────────

/// A panel of the layout the activity bar shows or hides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Panel {
    Tree,
    Output,
    Details,
}

/// One entry of the activity bar, top to bottom.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BarEntry {
    /// Shows or hides a panel; marked while it is shown.
    Toggle {
        panel: Panel,
        id: &'static str,
        icon: Icon,
        label: &'static str,
        tip: &'static str,
    },
    /// A workspace command, handed to the host.
    Command {
        id: &'static str,
        icon: Icon,
        label: &'static str,
        tip: &'static str,
        intent: UiIntent,
    },
    /// Pushes the rest to the bottom.
    Spacer,
}

/// Tree toggle, files, agent tree, search, gates; output/details toggles and
/// settings at the bottom.
pub(crate) fn activity_bar() -> Vec<BarEntry> {
    let command =
        |id: &'static str, icon: Icon, label: &'static str, tip: &'static str, intent: UiIntent| {
            BarEntry::Command {
                id,
                icon,
                label,
                tip,
                intent,
            }
        };
    vec![
        BarEntry::Toggle {
            panel: Panel::Tree,
            id: "dense-toggle-tree",
            icon: Icon::Layers,
            label: copy::EXPLORER,
            tip: copy::EXPLORER_TIP,
        },
        command(
            "dense-files",
            Icon::FileText,
            copy::FILES,
            copy::FILES_TIP,
            UiIntent::QuickOpenFiles { quick_open: true },
        ),
        command(
            "dense-agents",
            Icon::Bot,
            copy::AGENTS,
            copy::AGENTS_TIP,
            UiIntent::OpenAgentTree,
        ),
        command(
            "dense-search",
            Icon::Search,
            copy::SEARCH,
            copy::SEARCH_TIP,
            UiIntent::OpenJump,
        ),
        command(
            "dense-gates",
            Icon::ShieldQuestion,
            copy::GATES,
            copy::GATES_TIP,
            UiIntent::OpenGates,
        ),
        BarEntry::Spacer,
        BarEntry::Toggle {
            panel: Panel::Output,
            id: "dense-toggle-output",
            icon: Icon::Rows2,
            label: copy::OUTPUT_TOGGLE,
            tip: copy::OUTPUT_TIP,
        },
        BarEntry::Toggle {
            panel: Panel::Details,
            id: "dense-toggle-details",
            icon: Icon::PanelRight,
            label: copy::DETAILS_TOGGLE,
            tip: copy::DETAILS_TIP,
        },
        command(
            "dense-settings",
            Icon::Settings,
            copy::SETTINGS,
            copy::SETTINGS_TIP,
            UiIntent::OpenSettings,
        ),
    ]
}

// ───────────────────────────── details ─────────────────────────────

/// Muted, secondary and info lines read in the primary text colour; the
/// others in their tone's.
pub(crate) fn plain_text(tone: Tone) -> bool {
    matches!(tone, Tone::Muted | Tone::Secondary | Tone::Info)
}

/// `#12 열림 · title`.
pub(crate) fn pr_text(pr: &p::TaskPr) -> String {
    let state = format!("{:?}", pr.state).to_lowercase();
    let state = match state.as_str() {
        "open" if pr.draft => copy::PR_DRAFT,
        "open" => copy::PR_OPEN,
        "merged" => copy::PR_MERGED,
        "closed" => copy::PR_CLOSED,
        _ => "",
    };
    format!("#{} {} · {}", pr.number, state, pr.title)
}

/// The pending gate of a terminal decided inline: the first by gate id, as in
/// the session's gate map (`OrchState::gates`), whatever order `gates` is in.
pub(crate) fn pending_gate<'a>(gates: &'a [GateInfo], terminal: &str) -> Option<&'a GateInfo> {
    gates
        .iter()
        .filter(|g| g.state == GateState::Pending && g.terminal.as_deref() == Some(terminal))
        .min_by(|a, b| a.id.cmp(&b.id))
}

/// One decision button of an inline gate.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GateButton {
    pub(crate) decision: GateDecision,
    pub(crate) label: String,
    /// Element id (`dense-allow-<gate>[-<i>]`, `dense-reject-<gate>`).
    pub(crate) id: String,
    /// The option picked (v17 gates with options).
    pub(crate) choice: Option<String>,
    pub(crate) tone: Tone,
    /// Accessible name.
    pub(crate) name: String,
}

/// A gate with options (v17) is approved by picking one; otherwise 이번만
/// 허용. 거부 comes last.
pub(crate) fn gate_buttons(g: &GateInfo) -> Vec<GateButton> {
    let approve: Vec<(String, String, Option<String>)> = if g.options.is_empty() {
        vec![(
            copy::ALLOW_ONCE.to_string(),
            format!("dense-allow-{}", g.id),
            None,
        )]
    } else {
        g.options
            .iter()
            .enumerate()
            .map(|(i, o)| {
                (
                    o.clone(),
                    format!("dense-allow-{}-{i}", g.id),
                    Some(o.clone()),
                )
            })
            .collect()
    };
    approve
        .into_iter()
        .map(|(label, id, choice)| (GateDecision::Approve, label, id, choice, Tone::Success))
        .chain(std::iter::once((
            GateDecision::Reject,
            copy::REJECT.to_string(),
            format!("dense-reject-{}", g.id),
            None,
            Tone::Danger,
        )))
        .map(|(decision, label, id, choice, tone)| {
            let name = match &choice {
                Some(_) => format!("{}: {label}: {}", copy::CHOOSE, g.title),
                None => format!("{label}: {}", g.title),
            };
            GateButton {
                decision,
                label,
                id,
                choice,
                tone,
                name,
            }
        })
        .collect()
}

/// A decision button while `busy` is on its way: the one sent loads, the
/// others wait.
pub(crate) fn button_state(busy: Option<GateDecision>, d: GateDecision) -> ControlState {
    match busy {
        Some(b) if b == d => ControlState::Loading,
        Some(_) => ControlState::Disabled,
        None => ControlState::Ready,
    }
}

/// What a text action of the details column does.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Act {
    /// Hand a UI request to the host.
    Intent(UiIntent),
    /// 다시 실행 this terminal's command (the view's controller).
    Rerun(String),
}

/// A text action of the details column.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Link {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    /// Accessible name.
    pub(crate) name: String,
    pub(crate) icon: Icon,
    pub(crate) act: Act,
}

/// A terminal's first action: 터미널 열기 while it runs, else 다시 실행.
pub(crate) fn terminal_link(t: &TerminalVm, label: &str) -> Link {
    if t.is_running() {
        Link {
            id: "dense-open-terminal",
            label: copy::OPEN_TERMINAL,
            name: format!("{}: {label}", copy::OPEN_TERMINAL),
            icon: Icon::SquareTerminal,
            act: Act::Intent(UiIntent::OpenTerminal {
                id: t.id.clone(),
                another: false,
            }),
        }
    } else {
        Link {
            id: "dense-rerun",
            label: copy::RERUN,
            name: format!("{}: {label}", copy::RERUN),
            icon: Icon::RotateCcw,
            act: Act::Rerun(t.id.clone()),
        }
    }
}

/// 변경 검토 · 파일 열기 (with a worktree) · PR 보기 of a task.
pub(crate) fn task_links(k: &TaskVm) -> Vec<Link> {
    let mut links = vec![Link {
        id: "dense-review",
        label: copy::REVIEW,
        name: format!("{}: {}", copy::REVIEW, k.title),
        icon: Icon::GitCompareArrows,
        act: Act::Intent(UiIntent::OpenReview { task: k.id.clone() }),
    }];
    if k.worktree_exists || k.worktree.is_some() {
        links.push(Link {
            id: "dense-files-open",
            label: copy::OPEN_FILES,
            name: format!("{}: {}", copy::OPEN_FILES, k.title),
            icon: Icon::FileText,
            act: Act::Intent(UiIntent::OpenEditor {
                task: k.id.clone(),
                target: None,
                quick_open: false,
            }),
        });
    }
    links.push(Link {
        id: "dense-pr",
        label: copy::OPEN_PR,
        name: format!("{}: {}", copy::OPEN_PR, k.title),
        icon: Icon::GitPullRequest,
        act: Act::Intent(UiIntent::OpenPullRequest {
            task: k.id.clone(),
            form: false,
        }),
    });
    links
}

/// 다시 실행: the same command in the same folder and task (`terminal.run`).
pub(crate) fn rerun_spec(t: &TerminalVm) -> RunSpec {
    RunSpec {
        argv: t.argv.clone(),
        cwd: Some(t.cwd.clone()),
        name: None,
        cols: None,
        rows: None,
        task: t.task_id.clone(),
    }
}
