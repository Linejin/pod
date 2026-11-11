//! Claude Code hook events: how each one maps to Pod states, and the events
//! Pod installs in Claude's settings.

use super::{ChildEffect, HookEffect, QUESTION_TOOLS, RootEffect, child_id, s};
use crate::types::{AgentState, BlockedKind, Outcome};
use serde_json::Value;

pub(super) fn map_claude(event: &str, p: &Value) -> HookEffect {
    let agent_id = s(p, "agent_id");
    let tool = s(p, "tool_name");
    match event {
        "SessionStart" => {
            if s(p, "source").as_deref() == Some("compact") {
                return HookEffect::none();
            }
            HookEffect {
                session_boundary: true,
                ..HookEffect::set(AgentState::Idle, None)
            }
        }
        "UserPromptSubmit" => HookEffect::set(AgentState::Working, None),
        "PreToolUse" | "PostToolUse" | "PostToolUseFailure" if agent_id.is_some() => {
            // Activity inside a subagent: update the child, leave the lead alone.
            HookEffect {
                child: Some(ChildEffect::Set {
                    id: child_id(agent_id.as_deref().unwrap()),
                    state: AgentState::Working,
                    title: None,
                }),
                ..HookEffect::none()
            }
        }
        "PreToolUse" => {
            if tool.as_deref().is_some_and(|t| QUESTION_TOOLS.contains(&t)) {
                HookEffect::set(AgentState::Blocked(BlockedKind::Question), tool)
            } else {
                HookEffect::set(AgentState::Working, None)
            }
        }
        "PostToolUse" | "PostToolUseFailure" => HookEffect::set(AgentState::Working, None),
        "PermissionRequest" => HookEffect::set(AgentState::Blocked(BlockedKind::Permission), tool),
        "Elicitation" => {
            HookEffect::set(AgentState::Blocked(BlockedKind::Question), s(p, "message"))
        }
        "ElicitationResult" => HookEffect {
            root: RootEffect::RestoreFromWait,
            ..HookEffect::none()
        },
        "Stop" => {
            if p.get("is_interrupt").and_then(Value::as_bool) == Some(true) {
                HookEffect {
                    outcome: Some(Outcome::Cancelled),
                    ..HookEffect::set(AgentState::Idle, None)
                }
            } else {
                HookEffect {
                    outcome: Some(Outcome::Completed),
                    ..HookEffect::set(AgentState::Done, None)
                }
            }
        }
        "StopFailure" => {
            let msg = s(p, "error").or_else(|| s(p, "error_message"));
            HookEffect {
                outcome: Some(Outcome::Failed),
                ..HookEffect::set(AgentState::Error, msg)
            }
        }
        "SubagentStart" => match agent_id {
            Some(id) => HookEffect {
                child: Some(ChildEffect::Set {
                    id: child_id(&id),
                    state: AgentState::Working,
                    title: s(p, "agent_type"),
                }),
                ..HookEffect::none()
            },
            None => HookEffect::none(),
        },
        "SubagentStop" => match agent_id {
            Some(id) => HookEffect {
                child: Some(ChildEffect::Remove { id: child_id(&id) }),
                ..HookEffect::none()
            },
            None => HookEffect::none(),
        },
        "SessionEnd" => HookEffect {
            root: RootEffect::EndSession,
            ..HookEffect::none()
        },
        _ => HookEffect::none(),
    }
}

/// Hook events Pod installs, per agent (docs/04 §4.3).
pub static CLAUDE_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Elicitation",
    "ElicitationResult",
    "Stop",
    "StopFailure",
    "SubagentStart",
    "SubagentStop",
    "SessionEnd",
];

/// Claude events that take a `matcher` (tool/server name) in settings.json.
pub static CLAUDE_MATCHER_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "Elicitation",
    "ElicitationResult",
];
