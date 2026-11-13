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

/// Versioned HTTP DTOs. Deliberately absent from the local RPC schema.
pub mod mobile_v1 {
    use super::{GateDecision, GateState, MobileCap};
    use serde::de::{self, SeqAccess, Visitor};
    use serde::{Deserialize, Deserializer, Serialize};
    use std::{fmt, marker::PhantomData};

    pub const REQUEST_BYTES: usize = 32 * 1024;
    pub const RESPONSE_BYTES: usize = 64 * 1024;
    pub const OPS: [&str; 6] = super::mobile_api::ALL;

    /// Byte bound checked before copying a borrowed JSON string into the DTO.
    #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
    #[serde(transparent)]
    pub struct Text<const N: usize>(String);
    impl<const N: usize> Text<N> {
        pub fn new(s: impl Into<String>) -> Result<Self, &'static str> {
            let s = s.into();
            if s.len() > N {
                return Err("string budget exceeded");
            }
            Ok(Self(s))
        }
        pub fn as_str(&self) -> &str {
            &self.0
        }
    }
    impl<'de, const N: usize> Deserialize<'de> for Text<N> {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V<const N: usize>;
            impl<const N: usize> Visitor<'_> for V<N> {
                type Value = Text<N>;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "at most {N} UTF-8 bytes")
                }
                fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                    if s.len() > N {
                        return Err(E::custom("string budget exceeded"));
                    }
                    Ok(Text(s.to_owned()))
                }
            }
            d.deserialize_str(V::<N>)
        }
    }
    #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
    #[serde(transparent)]
    pub struct List<T, const N: usize>(Vec<T>);
    impl<T, const N: usize> List<T, N> {
        pub fn new(v: Vec<T>) -> Result<Self, &'static str> {
            if v.len() > N {
                return Err("array budget exceeded");
            }
            Ok(Self(v))
        }
        pub fn as_slice(&self) -> &[T] {
            &self.0
        }
    }
    impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for List<T, N> {
        fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            struct V<T, const N: usize>(PhantomData<T>);
            impl<'de, T: Deserialize<'de>, const N: usize> Visitor<'de> for V<T, N> {
                type Value = List<T, N>;
                fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                    write!(f, "at most {N} items")
                }
                fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Self::Value, A::Error> {
                    let mut v = Vec::new();
                    while v.len() < N {
                        match a.next_element()? {
                            Some(x) => v.push(x),
                            None => return Ok(List(v)),
                        }
                    }
                    if a.next_element::<de::IgnoredAny>()?.is_some() {
                        return Err(de::Error::custom("array budget exceeded"));
                    }
                    Ok(List(v))
                }
            }
            d.deserialize_seq(V::<T, N>(PhantomData))
        }
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(try_from = "u16")]
    pub struct WaitMs(u16);
    impl TryFrom<u16> for WaitMs {
        type Error = &'static str;
        fn try_from(ms: u16) -> Result<Self, Self::Error> {
            if ms > 25000 {
                return Err("long-poll budget exceeded");
            }
            Ok(Self(ms))
        }
    }
    impl WaitMs {
        pub fn get(self) -> u16 {
            self.0
        }
    }

    pub type Id = Text<128>;
    pub type Cursor = Text<512>;
    pub type Fingerprint = Text<64>;
    #[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(transparent)]
    pub struct Secret(Text<64>);
    impl Secret {
        pub fn as_str(&self) -> &str {
            self.0.as_str()
        }
    }
    impl fmt::Debug for Secret {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("[redacted]")
        }
    }

    // Explicit null is required on decision.choice; absence is never a default.
    fn required<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<T, D::Error> {
        T::deserialize(d)
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "op", content = "params", deny_unknown_fields)]
    pub enum Call {
        #[serde(rename = "tasks")]
        Tasks(Tasks),
        #[serde(rename = "task")]
        Task(Task),
        #[serde(rename = "output")]
        Output(Output),
        #[serde(rename = "gate.decide")]
        Decide(Decide),
        #[serde(rename = "message")]
        Message(Message),
        #[serde(rename = "delivery")]
        Delivery(Delivery),
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Tasks {
        pub cursor: Option<Cursor>,
        pub wait_ms: Option<WaitMs>,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Task {
        pub task: Id,
        pub gate: Option<Id>,
        pub cursor: Option<Cursor>,
        pub operation: Option<Operation>,
        pub wait_ms: Option<WaitMs>,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Output {
        pub task: Id,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Decide {
        pub machine: Id,
        pub task: Id,
        pub gate: Id,
        pub revision: u64,
        pub operation_id: Text<64>,
        pub decision: GateDecision,
        #[serde(deserialize_with = "required")]
        pub choice: Option<Text<4096>>,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Message {
        pub task: Id,
        pub operation_id: Text<64>,
        pub text: Text<4096>,
        pub comments: List<Id, 20>,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Delivery {
        pub task: Id,
        pub delivery: Id,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum OperationKind {
        Decision,
        Message,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Operation {
        pub kind: OperationKind,
        pub id: Text<64>,
        pub body_fingerprint: Fingerprint,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Scope {
        pub task: Id,
        pub caps: List<MobileCap, 4>,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum Purpose {
        Claim,
        Finish,
        Session,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Challenge {
        pub purpose: Purpose,
        pub machine: Id,
        pub boot: Id,
        pub subject: Id,
        pub auth_epoch: u64,
        pub nonce: Secret,
        pub csrf: Secret,
        pub expires_ms: u64,
    }
    // Secret-bearing lifecycle DTOs intentionally do not implement Debug.
    #[derive(Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Claim {
        pub pairing_id: Id,
        pub secret: Secret,
        pub code: Text<8>,
        pub nonce: Secret,
        pub public_key: Text<256>,
        pub signature: Text<192>,
        pub name: Text<128>,
        pub scopes: List<Scope, 20>,
    }
    #[derive(Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Claimed {
        pub ticket: Secret,
        pub expires_ms: u64,
    }
    #[derive(Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Finish {
        pub ticket: Secret,
        pub nonce: Secret,
        pub signature: Text<192>,
        pub wait_ms: Option<WaitMs>,
    }
    #[derive(Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct SessionOpen {
        pub device: Id,
        pub auth_epoch: u64,
        pub nonce: Secret,
        pub signature: Text<192>,
    }
    #[derive(Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Session {
        pub device: Id,
        pub boot: Id,
        pub auth_epoch: u64,
        pub bearer: Secret,
        pub csrf: Secret,
        pub expires_ms: u64,
        pub scopes: List<Scope, 20>,
    }
    /// Local lifecycle command, never a seventh app op.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Revoke {
        pub device: Id,
        pub auth_epoch: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "kind", deny_unknown_fields, rename_all = "snake_case")]
    pub enum Origin {
        Unknown,
        LocalUser,
        Mod { id: Id },
        Orch { run: Id, attempt: Id },
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Gate {
        pub machine: Id,
        pub boot: Id,
        pub task: Id,
        pub gate: Id,
        pub revision: u64,
        pub issued_ms: u64,
        pub expires_ms: Option<u64>,
        pub origin: Origin,
        pub title: Text<4096>,
        pub body: Text<49152>,
        pub options: List<Text<4096>, 20>,
        pub state: GateState,
        pub complete: bool,
        pub body_fingerprint: Fingerprint,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct TaskSummary {
        pub id: Id,
        pub name: Text<1024>,
        pub state: Text<64>,
        pub origin: Origin,
        pub received_ms: u64,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct TaskPage {
        pub tasks: List<TaskSummary, 20>,
        pub cursor: Option<Cursor>,
        pub server_ms: u64,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct TaskDetail {
        pub task: TaskSummary,
        pub gates: List<Gate, 20>,
        pub cursor: Option<Cursor>,
        pub operation: Option<OperationResult>,
        pub server_ms: u64,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct OutputResult {
        pub task: Id,
        pub lines: List<Text<16384>, 100>,
        pub truncated: bool,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct DecisionResult {
        pub gate: Gate,
        pub replayed: bool,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct MessageResult {
        pub delivery: Id,
        pub state: super::DeliveryState,
        pub replayed: bool,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct DeliveryResult {
        pub task: Id,
        pub delivery: Id,
        pub state: super::DeliveryState,
        pub error: Option<ErrorCode>,
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(
        tag = "kind",
        content = "result",
        deny_unknown_fields,
        rename_all = "snake_case"
    )]
    pub enum OperationResult {
        Decision(Box<DecisionResult>),
        Message(MessageResult),
    }
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(tag = "op", content = "result", deny_unknown_fields)]
    pub enum Reply {
        #[serde(rename = "tasks")]
        Tasks(TaskPage),
        #[serde(rename = "task")]
        Task(TaskDetail),
        #[serde(rename = "output")]
        Output(OutputResult),
        #[serde(rename = "gate.decide")]
        Decide(DecisionResult),
        #[serde(rename = "message")]
        Message(MessageResult),
        #[serde(rename = "delivery")]
        Delivery(DeliveryResult),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
    pub enum ErrorCode {
        #[serde(rename = "invalid_request")]
        InvalidRequest,
        #[serde(rename = "unauthenticated")]
        Unauthenticated,
        #[serde(rename = "denied")]
        Denied,
        #[serde(rename = "conflict")]
        Conflict,
        #[serde(rename = "stale")]
        Stale,
        #[serde(rename = "too_large")]
        TooLarge,
        #[serde(rename = "rate_limited")]
        RateLimited,
        #[serde(rename = "unavailable")]
        Unavailable,
        #[serde(rename = "storage")]
        Storage,
        #[serde(rename = "unknown_operation")]
        UnknownOperation,
    }
    impl ErrorCode {
        pub fn rpc_code(self) -> i64 {
            -32130 - self as i64
        }
        pub fn http_status(self) -> u16 {
            match self {
                Self::InvalidRequest => 400,
                Self::Unauthenticated => 401,
                Self::Denied => 403,
                Self::Conflict | Self::Stale => 409,
                Self::TooLarge => 413,
                Self::RateLimited => 429,
                Self::Unavailable | Self::Storage => 503,
                Self::UnknownOperation => 404,
            }
        }
    }
    /// Fixed code only: no input, token, body or diagnostic reflection.
    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    pub struct Error {
        pub code: ErrorCode,
        pub retry_after_ms: Option<u32>,
    }
    impl Error {
        pub fn refusal() -> Self {
            Self {
                code: ErrorCode::Unavailable,
                retry_after_ms: None,
            }
        }
    }

    /// Parse the original HTTP bytes, never a Value (which loses duplicate keys).
    pub fn parse_call(bytes: &[u8]) -> Result<Call, Error> {
        if bytes.len() > REQUEST_BYTES {
            return Err(Error {
                code: ErrorCode::TooLarge,
                retry_after_ms: None,
            });
        }
        let call: Call = serde_json::from_slice(bytes).map_err(|_| Error {
            code: ErrorCode::InvalidRequest,
            retry_after_ms: None,
        })?;
        let valid_id = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        };
        let valid = match &call {
            Call::Tasks(p) => p.wait_ms.is_none_or(|n| n.get() <= 25000),
            Call::Task(p) => !p.task.as_str().is_empty() && p.wait_ms.is_none_or(|n| n.get() <= 25000)
                && p.operation.as_ref().is_none_or(|o| valid_id(o.id.as_str()) && o.body_fingerprint.as_str().len() == 64 && o.body_fingerprint.as_str().bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
            Call::Decide(p) => p.revision > 0 && valid_id(p.operation_id.as_str()) && [&p.machine, &p.task, &p.gate].iter().all(|s| !s.as_str().is_empty()),
            Call::Message(p) => valid_id(p.operation_id.as_str()) && !p.task.as_str().is_empty() && ( !p.text.as_str().is_empty() || !p.comments.as_slice().is_empty())
                && p.text.as_str().chars().count() <= 1000 && p.text.as_str().split('\n').count() <= 20
                && !p.text.as_str().chars().any(|c| (c.is_control() && c != '\n') || matches!(c, '\u{061c}' | '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')),
            Call::Output(p) => !p.task.as_str().is_empty(),
            Call::Delivery(p) => !p.task.as_str().is_empty() && !p.delivery.as_str().is_empty(),
        };
        if !valid {
            return Err(Error {
                code: ErrorCode::InvalidRequest,
                retry_after_ms: None,
            });
        }
        Ok(call)
    }

    /// Bounded writer stops before the response buffer exceeds 64 KiB.
    pub fn response_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
        struct Writer(Vec<u8>);
        impl std::io::Write for Writer {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if self.0.len() + bytes.len() > RESPONSE_BYTES {
                    return Err(std::io::Error::other("response budget"));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut w = Writer(Vec::new());
        serde_json::to_writer(&mut w, value).map_err(|_| Error {
            code: ErrorCode::TooLarge,
            retry_after_ms: None,
        })?;
        Ok(w.0)
    }

    /// Canonical domain-separated fingerprint preimage: UTF-8 JSON of a fixed
    /// tuple/struct, no maps. Hashed with SHA-256 in podd (`sha2`).
    /// P2 verifies the purpose/key/nonce and consumes it atomically. These
    /// preimages fix the signed bytes; producing bytes grants no authority.
    pub fn claim_preimage(challenge: &Challenge, p: &Claim) -> Vec<u8> {
        serde_json::to_vec(&(
            "pod-mobile/1/claim",
            challenge,
            &p.pairing_id,
            &p.secret,
            &p.code,
            &p.nonce,
            &p.public_key,
            &p.name,
            scope_preimage(p.scopes.as_slice()),
        ))
        .expect("fixed DTO serialization")
    }
    pub fn finish_preimage(challenge: &Challenge, p: &Finish) -> Vec<u8> {
        serde_json::to_vec(&("pod-mobile/1/finish", challenge, &p.ticket, &p.nonce))
            .expect("fixed DTO serialization")
    }
    pub fn session_preimage(challenge: &Challenge, p: &SessionOpen) -> Vec<u8> {
        serde_json::to_vec(&(
            "pod-mobile/1/session",
            challenge,
            &p.device,
            p.auth_epoch,
            &p.nonce,
        ))
        .expect("fixed DTO serialization")
    }

    pub fn decision_preimage(p: &Decide) -> Vec<u8> {
        serde_json::to_vec(&(
            "pod-mobile/1/decision",
            &p.machine,
            &p.task,
            &p.gate,
            p.revision,
            p.decision,
            &p.choice,
        ))
        .expect("fixed DTO serialization")
    }
    /// Scope is a set: task/cap ordering and duplicate grants do not alter it.
    pub fn scope_preimage(scopes: &[Scope]) -> Vec<u8> {
        let mut canonical =
            std::collections::BTreeMap::<&str, std::collections::BTreeSet<&str>>::new();
        for s in scopes {
            for c in s.caps.as_slice() {
                canonical
                    .entry(s.task.as_str())
                    .or_default()
                    .insert(c.as_str());
            }
        }
        serde_json::to_vec(&("pod-mobile/1/scope", canonical)).expect("fixed DTO serialization")
    }
    pub fn gate_preimage(g: &Gate) -> Vec<u8> {
        serde_json::to_vec(&(
            "pod-mobile/1/gate",
            &g.machine,
            &g.boot,
            &g.task,
            &g.gate,
            g.revision,
            g.issued_ms,
            g.expires_ms,
            &g.origin,
            &g.title,
            &g.body,
            &g.options,
            g.state,
            g.complete,
        ))
        .expect("fixed DTO serialization")
    }
}

#[cfg(test)]
mod mobile_v1_tests {
    use super::mobile_v1::*;
    #[test]
    fn exact_ops_and_duplicate_fields() {
        assert_eq!(
            OPS,
            [
                "tasks",
                "task",
                "output",
                "gate.decide",
                "message",
                "delivery"
            ]
        );
        assert!(parse_call(br#"{"op":"tasks","params":{}}"#).is_ok());
        for bytes in [
            br#"{"op":"tasks","op":"tasks","params":{}}"#.as_slice(),
            br#"{"op":"output","params":{"task":"t1","task":"t2"}}"#,
            br#"{"op":"tasks","params":{"caller":"m1"}}"#,
            br#"{"op":"tasks"}"#,
            br#"{"op":"gate.decide","params":{"machine":"m1","task":"t1","gate":"g1","revision":1,"operation_id":"o","decision":"approve"}}"#,
            br#"{"op":"operation.get","params":{}}"#,
        ] { assert!(parse_call(bytes).is_err()); }
    }
    #[test]
    fn reserved_error_codes_and_redaction() {
        let codes = [
            ErrorCode::InvalidRequest,
            ErrorCode::Unauthenticated,
            ErrorCode::Denied,
            ErrorCode::Conflict,
            ErrorCode::Stale,
            ErrorCode::TooLarge,
            ErrorCode::RateLimited,
            ErrorCode::Unavailable,
            ErrorCode::Storage,
            ErrorCode::UnknownOperation,
        ];
        for (i, code) in codes.into_iter().enumerate() {
            assert_eq!(code.rpc_code(), -32130 - i as i64);
        }
        assert_eq!(ErrorCode::RateLimited.http_status(), 429);
        let secret: Secret = serde_json::from_str(r#""canary""#).unwrap();
        assert_eq!(format!("{secret:?}"), "[redacted]");
    }
}
