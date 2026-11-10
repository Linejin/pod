//! Wire types for mobile approval (docs/17 §4, step M1).
//!
//! Two surfaces, kept apart on purpose:
//!
//! - **PC management** (`mobile.devices`, `mobile.revoke`, `mobile.policy`,
//!   `mobile.audit`, `mobile.settings`): ordinary methods on the user-only
//!   pipe, for the PC app and `pod mobile`. Never reachable from a phone.
//! - **The mobile API** ([`mobile_api`]): the only operations a mobile
//!   channel (LAN TLS in M2, relay + E2EE in M4/M5) will ever be able to
//!   call. They are *not* pipe methods; the daemon routes them through
//!   `server/mobile/api.rs` with an authenticated device context. M1 has no
//!   network listener at all.
//!
//! Everything here is re-exported from [`crate::protocol`].

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::protocol::{DeliveryState, GateDecision, GateState, Method, SubmitCheck};

macro_rules! method {
    ($ty:ident, $name:literal, $params:ty, $result:ty) => {
        pub struct $ty;
        impl Method for $ty {
            const NAME: &'static str = $name;
            type Params = $params;
            type Result = $result;
        }
    };
}

// ───────────────────────── PC management (pipe) ─────────────────────────

method!(
    MobileDevices,
    "mobile.devices",
    MobileDevicesParams,
    MobileDevicesResult
);
method!(
    MobileRevoke,
    "mobile.revoke",
    MobileRevokeParams,
    MobileRevokeResult
);
method!(
    MobilePolicy,
    "mobile.policy",
    MobilePolicyParams,
    MobileDeviceInfo
);
method!(
    MobileAudit,
    "mobile.audit",
    MobileAuditParams,
    MobileAuditResult
);
method!(
    MobileSettingsMethod,
    "mobile.settings",
    MobileSettingsParams,
    MobileSettings
);

pub const MOBILE_METHODS: &[&str] = &[
    MobileDevices::NAME,
    MobileRevoke::NAME,
    MobilePolicy::NAME,
    MobileAudit::NAME,
    MobileSettingsMethod::NAME,
];

/// What a paired device may do, per task. Deny by default: a device with no
/// entry for a task cannot see it; each capability is granted separately
/// (`output` and `message` are never implied).
#[derive(
    Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum MobileCap {
    /// See the task (name, aggregate status) and its pending gates.
    Status,
    /// Read the agent terminal's last output (bounded, control characters removed).
    Output,
    /// Approve / reject the task's pending gates.
    Approve,
    /// Send a short text message to the task's agent (also: send its open review comments).
    Message,
}

impl MobileCap {
    pub const ALL: [MobileCap; 4] = [Self::Status, Self::Output, Self::Approve, Self::Message];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Output => "output",
            Self::Approve => "approve",
            Self::Message => "message",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s.trim())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileDevicesParams {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileDeviceInfo {
    /// `m<N>`, never reused.
    pub id: String,
    pub name: String,
    /// The device's public identity key (M4: Noise static key). Placeholder
    /// text until pairing exists.
    pub public_key: String,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_ms: Option<u64>,
    /// Bumped on revoke: every session of an older epoch is dead.
    pub auth_epoch: u64,
    /// Task id -> granted capabilities.
    #[serde(default)]
    pub policy: BTreeMap<String, Vec<MobileCap>>,
    /// Live sessions (in memory; none survive a daemon restart).
    #[serde(default)]
    pub sessions: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileSettings {
    /// Store message bodies in the audit log (default off: SHA-256 + length only).
    #[serde(default)]
    pub audit_message_bodies: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileDevicesResult {
    /// Network listener state. Always `"off"` in M1 (no sockets, TLS or relay).
    pub listener: String,
    pub devices: Vec<MobileDeviceInfo>,
    pub settings: MobileSettings,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileRevokeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Revoke every device.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileRevokeResult {
    pub revoked: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobilePolicyParams {
    pub device: String,
    /// Task reference (`<repo>/<name>` or unique name).
    pub task: String,
    #[serde(default)]
    pub allow: Vec<MobileCap>,
    #[serde(default)]
    pub deny: Vec<MobileCap>,
    /// Remove every capability for this task (applied before `allow`).
    #[serde(default)]
    pub clear: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileAuditParams {
    /// Newest entries, default 100, at most 1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileSettingsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audit_message_bodies: Option<bool>,
}

/// One audit record: who, what, when, result. Message text only as length +
/// SHA-256 unless `audit_message_bodies` is on.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileAuditEntry {
    pub ts_ms: u64,
    /// Device id, or `local` for decisions made on the PC.
    pub actor: String,
    /// `gate.decide`, `message`, `tasks`, `output`, `revoke`, `policy`, `pair`, ...
    pub action: String,
    /// `ok`, `queued`, `replayed`, `denied`, `error`.
    pub result: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_len: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msg_body: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileAuditResult {
    pub entries: Vec<MobileAuditEntry>,
}

/// Error codes of the mobile surfaces (own block, clear of v12's file codes).
pub mod mobile_codes {
    /// Unknown device / method outside the mobile API / capability missing.
    pub const MOBILE_DENIED: i64 = -32060;
    /// No such session, expired (8 h absolute, 15 min idle), or the device was revoked.
    pub const MOBILE_AUTH: i64 = -32061;
    /// Too many requests; `data = {retry_after_ms}`.
    pub const RATE_LIMITED: i64 = -32062;
    /// Same operation id with a different request.
    pub const OPERATION_CONFLICT: i64 = -32063;
    /// Message refused: empty, too long, control characters, invalid UTF-8.
    pub const MESSAGE_INVALID: i64 = -32064;
}

// ───────────────────────── the mobile API (not on the pipe) ─────────────────────────

/// Operation names a mobile channel can call. Nothing else is routable.
pub mod mobile_api {
    pub const TASKS: &str = "tasks";
    pub const TASK: &str = "task";
    pub const OUTPUT: &str = "output";
    pub const GATE_DECIDE: &str = "gate.decide";
    pub const MESSAGE: &str = "message";
    pub const DELIVERY: &str = "delivery";
    pub const ALL: [&str; 6] = [TASKS, TASK, OUTPUT, GATE_DECIDE, MESSAGE, DELIVERY];
}

/// A gate as the phone sees it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileGate {
    pub id: String,
    pub task: String,
    pub title: String,
    pub message: String,
    pub state: GateState,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_ms: Option<u64>,
    /// v17: choices of an orchestration gate (approve with one of them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
}

/// A task as the phone sees it: no prompt, argv, environment, paths or tokens.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileTask {
    pub id: String,
    pub repo: String,
    pub name: String,
    /// Aggregate status (`working`, `idle`, `blocked`, ... or `unknown`).
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// What this device may do with the task.
    pub caps: Vec<MobileCap>,
    /// Pending gates of the task.
    pub gates: Vec<MobileGate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileTasksResult {
    pub server_ms: u64,
    pub tasks: Vec<MobileTask>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileTaskParams {
    pub task: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileOutputResult {
    pub task: String,
    /// At most 100 lines / 16 KiB, control characters removed.
    pub lines: Vec<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileGateDecideParams {
    pub gate: String,
    pub decision: GateDecision,
    /// v17: the chosen option of a gate with options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
    /// Idempotency key chosen by the device (1..=64 of `[A-Za-z0-9_-]`).
    pub operation_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileGateDecideResult {
    pub gate: MobileGate,
    /// The same operation was already committed; this is its stored result.
    pub replayed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileMessageParams {
    pub task: String,
    /// Plain text, at most 1000 characters / 4 KiB / 20 lines; no control
    /// characters except newline. May be empty with `with_open_comments`.
    #[serde(default)]
    pub text: String,
    pub operation_id: String,
    /// Deliver the task's open review comments, with `text` as the
    /// reviewer's instructions.
    #[serde(default)]
    pub with_open_comments: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileMessageResult {
    /// Delivery id (`d<N>`), same records as `review.deliveries`.
    pub delivery: String,
    /// `delivered` (typed in now; submit confirmation follows) or `queued`
    /// (the agent is working; sent when it is idle).
    pub state: DeliveryState,
    pub agent_state: String,
    pub replayed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileDeliveryParams {
    pub delivery: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MobileDeliveryResult {
    pub delivery: String,
    pub task: String,
    pub state: DeliveryState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub submit: Option<SubmitCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
