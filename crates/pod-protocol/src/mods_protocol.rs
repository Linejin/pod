//! Wire types for extensions (docs/16 §9): `mod.*` methods and the
//! `mod.notify` / `mod.changed` notifications on `status.subscribe`.
//! Kept apart from `protocol.rs`; re-exported from [`crate::protocol`].

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use pod_mods::{
    Axis, LayoutNode, LayoutPreset, ModInfo, ModList, ModState, Notice, PanelKind, Permission,
    StopReason,
};

use crate::protocol::Empty;

method!(ModListMethod, "mod.list", Empty, ModListResult);
method!(ModSetEnabled, "mod.set_enabled", ModEnableParams, ModInfo);
method!(ModGrant, "mod.grant", ModGrantParams, ModInfo);
method!(ModLog, "mod.log", ModLogParams, ModLogResult);
method!(
    ModCommand,
    "mod.command",
    ModCommandParams,
    ModCommandResult
);
method!(ModReload, "mod.reload", ModReloadParams, ModListResult);

pub const NOTIFY_MOD_NOTIFY: &str = "mod.notify";
pub const NOTIFY_MOD_CHANGED: &str = "mod.changed";

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModListResult {
    /// False when the daemon runs with `POD_MODS=off`.
    pub enabled: bool,
    #[serde(flatten)]
    pub list: ModList,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModEnableParams {
    pub name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModGrantParams {
    pub name: String,
    pub permission: Permission,
    pub granted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModLogParams {
    pub name: String,
    /// Last N lines (default 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tail: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModLogResult {
    pub name: String,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModCommandParams {
    pub name: String,
    pub command: String,
    /// The task the user is in (the command's scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModCommandResult {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModReloadParams {
    /// Only this mod (default: the whole folder).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `mod.notify`: a mod's `pod.notify`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModNotifyEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub notice: Notice,
}

/// `mod.changed`: a mod was enabled, disabled, stopped or reloaded.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ModChangedEvent {
    pub seq: u64,
    #[serde(rename = "mod")]
    pub info: ModInfo,
}
