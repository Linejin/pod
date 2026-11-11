//! Codex hook events: how each one maps to Pod states, and the events Pod
//! installs in Codex's hooks config.

use super::{ChildEffect, HookEffect, QUESTION_TOOLS, child_id, s};
use crate::types::{AgentState, BlockedKind, Outcome};
use serde_json::Value;

pub(super) fn map_codex(event: &str, p: &Value) -> HookEffect {
    let tool = s(p, "tool_name");
    match event {
        "SessionStart" => HookEffect {
            session_boundary: true,
            ..HookEffect::set(AgentState::Idle, None)
        },
        "UserPromptSubmit" | "PostToolUse" => HookEffect::set(AgentState::Working, None),
        "PreToolUse" => {
            if tool.as_deref().is_some_and(|t| QUESTION_TOOLS.contains(&t)) {
                HookEffect::set(AgentState::Blocked(BlockedKind::Question), tool)
            } else {
                HookEffect::set(AgentState::Working, None)
            }
        }
        "PermissionRequest" => HookEffect::set(AgentState::Blocked(BlockedKind::Permission), tool),
        "Stop" => HookEffect {
            outcome: Some(Outcome::Completed),
            ..HookEffect::set(AgentState::Done, None)
        },
        "Interrupt" => HookEffect {
            outcome: Some(Outcome::Cancelled),
            ..HookEffect::set(AgentState::Idle, None)
        },
        "SubagentStart" => match s(p, "agent_id") {
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
        "SubagentStop" => match s(p, "agent_id") {
            Some(id) => HookEffect {
                child: Some(ChildEffect::Remove { id: child_id(&id) }),
                ..HookEffect::none()
            },
            None => HookEffect::none(),
        },
        _ => HookEffect::none(),
    }
}

/// Hook events Pod installs for Codex (docs/04 §4.3).
pub static CODEX_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "Interrupt",
    "SubagentStart",
    "SubagentStop",
];
