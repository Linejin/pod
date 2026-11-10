//! `pod-mods` — Pod's extension system (docs/16).
//!
//! Small sandboxed JavaScript mods (QuickJS via rquickjs) that react to
//! daemon events, add palette commands, detect agent status from terminal
//! screens and titles, and ship declarative layout presets. Every host call
//! goes through a permission and scope check; each mod has its own memory
//! and CPU limits and is stopped, not the host, when it misbehaves.
//!
//! The embedding daemon implements [`Host`] and drives a `ModHandle`
//! (feature `runtime`, default). Without it only the manifest, host and wire
//! types build (for clients of the daemon).

pub mod host;
#[cfg(feature = "runtime")]
pub mod manager;
pub mod manifest;
#[cfg(feature = "runtime")]
pub mod runtime;
pub mod types;

pub use host::{
    GroupFacts, GroupMemberFacts, Host, Notice, Scope, StatusHint, TaskFacts, TerminalFacts,
};
#[cfg(feature = "runtime")]
pub use manager::{EVENTS, ModConfig, ModHandle, ORCH_EVENTS, STATE_FILE, program_name};
pub use manifest::{Axis, LayoutNode, LayoutPreset, Manifest, PanelKind, Permission};
#[cfg(feature = "runtime")]
pub use runtime::{Failure, Limits};
pub use types::{
    CheckInfo, CommandInfo, DetectorInfo, ModInfo, ModList, ModState, PermissionInfo, StopReason,
};
