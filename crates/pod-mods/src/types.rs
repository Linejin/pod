//! Wire types of the extension system, shared by podd (server) and its
//! clients (pod-app, pod-cli) without the QuickJS runtime.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::manifest::{LayoutPreset, Permission};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModState {
    Disabled,
    Running,
    Stopped,
    Invalid,
}

/// Why a mod was stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// CPU time limit.
    Cpu,
    /// Memory limit.
    Memory,
    /// Too many uncaught errors in a row.
    Errors,
    /// The entry script failed to load.
    Load,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PermissionInfo {
    pub name: Permission,
    pub requested: bool,
    pub granted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CheckInfo {
    pub name: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommandInfo {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DetectorInfo {
    #[serde(rename = "match")]
    pub names: Vec<String>,
}

/// What the settings page and the palette need to know about one mod.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModInfo {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub enabled: bool,
    pub state: ModState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<StopReason>,
    /// Manifest problem (invalid) or the failure that stopped the mod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub permissions: Vec<PermissionInfo>,
    #[serde(default)]
    pub checks: Vec<CheckInfo>,
    #[serde(default)]
    pub commands: Vec<CommandInfo>,
    #[serde(default)]
    pub detectors: Vec<DetectorInfo>,
    #[serde(default)]
    pub layouts: Vec<LayoutPreset>,
    pub log_lines: usize,
    /// QuickJS heap in KiB while running.
    pub memory_kib: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModList {
    /// Folder mods are read from.
    pub root: String,
    pub mods: Vec<ModInfo>,
    /// Events and samples dropped because the queue was full.
    pub dropped_events: u64,
}
