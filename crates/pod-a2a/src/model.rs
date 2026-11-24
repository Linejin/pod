//! The A2A 1.0 data model in canonical ProtoJSON.
//!
//! Mirrors `specification/a2a.proto` of the release tag `v1.0.1` (commit
//! 3303592588e388e62e0f69f701af531d2f4e3991) for the messages this client
//! sends or reads: camelCase field names, enums as their proto names, proto3
//! default values omitted on output, `oneof`s as exactly one present key.
//! Reading follows ProtoJSON's parser rules: unknown fields are ignored
//! (spec §5.7), `null` means the default, enums also accept their numbers and
//! integers also accept numeric strings. Push notification configuration
//! is not modelled: this client never sets it.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, Deserializer};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `google.protobuf.Struct`.
pub type Struct = Map<String, Value>;

/// A2A protocol version this client speaks (`A2A-Version`, `protocolVersion`).
pub const PROTOCOL_VERSION: &str = "1.0";
/// The one protocol binding this client implements.
pub const BINDING_JSONRPC: &str = "JSONRPC";

// ---------------------------------------------------------------- helpers

/// ProtoJSON: `null` stands for the field's default value.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

/// ProtoJSON int32: a JSON number or a numeric string.
fn int32_from(v: &Value) -> Result<i32, String> {
    let n = match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64)),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    n.and_then(|n| i32::try_from(n).ok())
        .ok_or_else(|| format!("expected an int32, got {v}"))
}

fn opt_int32<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
    match Option::<Value>::deserialize(d)? {
        None | Some(Value::Null) => Ok(None),
        Some(v) => int32_from(&v).map(Some).map_err(de::Error::custom),
    }
}

fn int32<'de, D: Deserializer<'de>>(d: D) -> Result<i32, D::Error> {
    Ok(opt_int32(d)?.unwrap_or_default())
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A proto enum: its name on output, its name or number on input; values
/// this client does not know are kept, not rejected.
macro_rules! proto_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $var:ident = $num:literal => $text:literal,)+ }) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Eq, Default)]
        pub enum $name {
            #[default]
            $($(#[$vm])* $var,)+
            /// A value outside A2A 1.0 as this client knows it (name or number).
            Unrecognized(String),
        }

        impl $name {
            /// The proto enum name (`Unrecognized`: the text the peer sent).
            pub fn as_proto_name(&self) -> &str {
                match self {
                    $(Self::$var => $text,)+
                    Self::Unrecognized(s) => s,
                }
            }

            /// Parses a proto enum name (exact, as ProtoJSON writes it).
            pub fn from_proto_name(s: &str) -> Option<Self> {
                match s {
                    $($text => Some(Self::$var),)+
                    _ => None,
                }
            }

            fn from_number(n: i64) -> Self {
                match n {
                    $($num => Self::$var,)+
                    other => Self::Unrecognized(other.to_string()),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                match self {
                    Self::Unrecognized(text) => match text.parse::<i64>() {
                        Ok(n) => s.serialize_i64(n),
                        Err(_) => s.serialize_str(text),
                    },
                    known => s.serialize_str(known.as_proto_name()),
                }
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                match Value::deserialize(d)? {
                    Value::Null => Ok(Self::default()),
                    Value::String(s) => {
                        Ok(Self::from_proto_name(&s).unwrap_or(Self::Unrecognized(s)))
                    }
                    Value::Number(n) => n
                        .as_i64()
                        .map(Self::from_number)
                        .ok_or_else(|| de::Error::custom("enum number out of range")),
                    other => Err(de::Error::custom(format!(
                        "expected a {} name or number, got {other}",
                        stringify!($name)
                    ))),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_proto_name())
            }
        }
    };
}

proto_enum!(
    /// `lf.a2a.v1.Role`.
    Role {
        Unspecified = 0 => "ROLE_UNSPECIFIED",
        User = 1 => "ROLE_USER",
        Agent = 2 => "ROLE_AGENT",
    }
);

proto_enum!(
    /// `lf.a2a.v1.TaskState`.
    TaskState {
        Unspecified = 0 => "TASK_STATE_UNSPECIFIED",
        Submitted = 1 => "TASK_STATE_SUBMITTED",
        Working = 2 => "TASK_STATE_WORKING",
        Completed = 3 => "TASK_STATE_COMPLETED",
        Failed = 4 => "TASK_STATE_FAILED",
        Canceled = 5 => "TASK_STATE_CANCELED",
        InputRequired = 6 => "TASK_STATE_INPUT_REQUIRED",
        Rejected = 7 => "TASK_STATE_REJECTED",
        AuthRequired = 8 => "TASK_STATE_AUTH_REQUIRED",
    }
);

/// Where a task state sits in the lifecycle (spec §3.2.2, a2a.proto).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateClass {
    /// Submitted or working: processing continues at the peer.
    Active,
    /// Input or authorization required: the peer waits for the client.
    Interrupted,
    /// Completed, failed, canceled or rejected: no further messages.
    Terminal,
    /// Unspecified or a value this client does not know.
    Unknown,
}

impl TaskState {
    pub fn class(&self) -> StateClass {
        match self {
            Self::Submitted | Self::Working => StateClass::Active,
            Self::InputRequired | Self::AuthRequired => StateClass::Interrupted,
            Self::Completed | Self::Failed | Self::Canceled | Self::Rejected => {
                StateClass::Terminal
            }
            Self::Unspecified | Self::Unrecognized(_) => StateClass::Unknown,
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.class() == StateClass::Terminal
    }

    /// Short lower-case word for people (`input-required`, ...).
    pub fn word(&self) -> &str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::InputRequired => "input-required",
            Self::Rejected => "rejected",
            Self::AuthRequired => "auth-required",
            Self::Unrecognized(_) => "unknown",
        }
    }
}

// ---------------------------------------------------------------- Part

/// `Part.content`: exactly one of text, raw, url or data.
#[derive(Clone, Debug, PartialEq)]
pub enum PartContent {
    Text(String),
    /// `bytes raw`: kept as the base64 text the peer sent (ProtoJSON).
    Raw(String),
    /// A URL to file content. This client never fetches it.
    Url(String),
    /// `google.protobuf.Value`: any JSON value, `null` included.
    Data(Value),
}

/// `lf.a2a.v1.Part`.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub content: PartContent,
    pub metadata: Option<Struct>,
    pub filename: String,
    pub media_type: String,
}

impl Part {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: PartContent::Text(text.into()),
            metadata: None,
            filename: String::new(),
            media_type: String::new(),
        }
    }

    /// The decoded length of a raw part (standard or URL-safe base64, as
    /// ProtoJSON parsers accept); `None` when it is not valid base64.
    pub fn raw_len(b64: &str) -> Option<usize> {
        use base64::Engine as _;
        use base64::engine::general_purpose::{STANDARD, URL_SAFE};
        let pad = |s: &str| {
            let mut s = s.trim_end_matches('=').to_string();
            while !s.len().is_multiple_of(4) {
                s.push('=');
            }
            s
        };
        let p = pad(b64);
        STANDARD
            .decode(&p)
            .or_else(|_| URL_SAFE.decode(&p))
            .ok()
            .map(|b| b.len())
    }
}

const PART_KEYS: [&str; 4] = ["text", "raw", "url", "data"];

impl Serialize for Part {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        match &self.content {
            PartContent::Text(t) => m.serialize_entry("text", t)?,
            PartContent::Raw(r) => m.serialize_entry("raw", r)?,
            PartContent::Url(u) => m.serialize_entry("url", u)?,
            PartContent::Data(d) => m.serialize_entry("data", d)?,
        }
        if let Some(md) = &self.metadata {
            m.serialize_entry("metadata", md)?;
        }
        if !self.filename.is_empty() {
            m.serialize_entry("filename", &self.filename)?;
        }
        if !self.media_type.is_empty() {
            m.serialize_entry("mediaType", &self.media_type)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for Part {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut map = Struct::deserialize(d)?;
        // A oneof member set to `null` is absent, except `data`, where
        // `null` is the JSON null value (google.protobuf.Value).
        let present: Vec<&str> = PART_KEYS
            .into_iter()
            .filter(|k| match map.get(*k) {
                None => false,
                Some(Value::Null) => *k == "data",
                Some(_) => true,
            })
            .collect();
        let key = match present.as_slice() {
            [one] => *one,
            [] => return Err(de::Error::custom("part has none of text, raw, url, data")),
            many => {
                return Err(de::Error::custom(format!(
                    "part sets more than one of its oneof members: {}",
                    many.join(", ")
                )));
            }
        };
        let value = map.remove(key).unwrap_or(Value::Null);
        let string = |v: Value| match v {
            Value::String(s) => Ok(s),
            other => Err(de::Error::custom(format!(
                "part.{key} must be a string, got {other}"
            ))),
        };
        let content = match key {
            "text" => PartContent::Text(string(value)?),
            "raw" => {
                let r = string(value)?;
                if Part::raw_len(&r).is_none() {
                    return Err(de::Error::custom("part.raw is not valid base64"));
                }
                PartContent::Raw(r)
            }
            "url" => PartContent::Url(string(value)?),
            _ => PartContent::Data(value),
        };
        let metadata = match map.remove("metadata") {
            None | Some(Value::Null) => None,
            Some(Value::Object(o)) => Some(o),
            Some(other) => {
                return Err(de::Error::custom(format!(
                    "part.metadata must be an object, got {other}"
                )));
            }
        };
        let text_field = |map: &mut Struct, k: &str| -> Result<String, D::Error> {
            match map.remove(k) {
                None | Some(Value::Null) => Ok(String::new()),
                Some(Value::String(s)) => Ok(s),
                Some(other) => Err(de::Error::custom(format!(
                    "part.{k} must be a string, got {other}"
                ))),
            }
        };
        let filename = text_field(&mut map, "filename")?;
        let media_type = text_field(&mut map, "mediaType")?;
        Ok(Part {
            content,
            metadata,
            filename,
            media_type,
        })
    }
}

// ---------------------------------------------------------------- core objects

/// `lf.a2a.v1.Message`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    #[serde(default, deserialize_with = "null_default")]
    pub message_id: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub context_id: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub task_id: String,
    #[serde(default)]
    pub role: Role,
    #[serde(default, deserialize_with = "null_default")]
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub extensions: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub reference_task_ids: Vec<String>,
}

impl Message {
    /// A user message with one text part.
    pub fn user_text(message_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            message_id: message_id.into(),
            role: Role::User,
            parts: vec![Part::text(text)],
            ..Self::default()
        }
    }

    /// A fresh message id (UUID v4).
    pub fn new_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

/// `lf.a2a.v1.TaskStatus`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    #[serde(default)]
    pub state: TaskState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// `google.protobuf.Timestamp` as the RFC 3339 text the peer sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// `lf.a2a.v1.Artifact`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    #[serde(default, deserialize_with = "null_default")]
    pub artifact_id: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub name: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub description: String,
    #[serde(default, deserialize_with = "null_default")]
    pub parts: Vec<Part>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub extensions: Vec<String>,
}

/// `lf.a2a.v1.Task`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    #[serde(default, deserialize_with = "null_default")]
    pub id: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub context_id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub status: TaskStatus,
    /// `None`: the field was absent (ListTasks without `includeArtifacts`
    /// omits it); `Some(vec![])`: present and empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<Artifact>>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub history: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
}

// ---------------------------------------------------------------- requests

/// `lf.a2a.v1.SendMessageConfiguration` (push notification config is not
/// modelled: this client never asks for push delivery).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageConfiguration {
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub accepted_output_modes: Vec<String>,
    #[serde(
        default,
        deserialize_with = "opt_int32",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_length: Option<i32>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "is_false"
    )]
    pub return_immediately: bool,
}

/// `lf.a2a.v1.SendMessageRequest`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendMessageRequest {
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub tenant: String,
    pub message: Message,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<SendMessageConfiguration>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
}

/// `lf.a2a.v1.GetTaskRequest`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTaskRequest {
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub tenant: String,
    pub id: String,
    #[serde(
        default,
        deserialize_with = "opt_int32",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_length: Option<i32>,
}

/// `lf.a2a.v1.ListTasksRequest`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksRequest {
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub tenant: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub context_id: String,
    /// proto3 enum without presence: `None` and UNSPECIFIED are not sent.
    #[serde(default, skip_serializing_if = "ListTasksRequest::no_status")]
    pub status: Option<TaskState>,
    #[serde(
        default,
        deserialize_with = "opt_int32",
        skip_serializing_if = "Option::is_none"
    )]
    pub page_size: Option<i32>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub page_token: String,
    #[serde(
        default,
        deserialize_with = "opt_int32",
        skip_serializing_if = "Option::is_none"
    )]
    pub history_length: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_timestamp_after: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_artifacts: Option<bool>,
}

impl ListTasksRequest {
    fn no_status(s: &Option<TaskState>) -> bool {
        matches!(s, None | Some(TaskState::Unspecified))
    }
}

/// `lf.a2a.v1.ListTasksResponse`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListTasksResponse {
    #[serde(default, deserialize_with = "null_default")]
    pub tasks: Vec<Task>,
    #[serde(default, deserialize_with = "null_default")]
    pub next_page_token: String,
    #[serde(default, deserialize_with = "int32")]
    pub page_size: i32,
    #[serde(default, deserialize_with = "int32")]
    pub total_size: i32,
}

/// `lf.a2a.v1.CancelTaskRequest`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelTaskRequest {
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub tenant: String,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Struct>,
}

/// `lf.a2a.v1.SendMessageResponse`: the `payload` oneof.
// One value per call, matched right away: boxing the Task buys nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum SendMessageResponse {
    Task(Task),
    Message(Message),
}

impl Serialize for SendMessageResponse {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(1))?;
        match self {
            Self::Task(t) => m.serialize_entry("task", t)?,
            Self::Message(msg) => m.serialize_entry("message", msg)?,
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for SendMessageResponse {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut map = Struct::deserialize(d)?;
        let task = map.remove("task").filter(|v| !v.is_null());
        let message = map.remove("message").filter(|v| !v.is_null());
        match (task, message) {
            (Some(t), None) => serde_json::from_value(t)
                .map(Self::Task)
                .map_err(de::Error::custom),
            (None, Some(m)) => serde_json::from_value(m)
                .map(Self::Message)
                .map_err(de::Error::custom),
            (Some(_), Some(_)) => Err(de::Error::custom(
                "SendMessageResponse sets both task and message (payload is a oneof)",
            )),
            (None, None) => Err(de::Error::custom(
                "SendMessageResponse has neither task nor message",
            )),
        }
    }
}

// ---------------------------------------------------------------- Agent Card

/// `lf.a2a.v1.AgentInterface`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInterface {
    #[serde(default, deserialize_with = "null_default")]
    pub url: String,
    #[serde(default, deserialize_with = "null_default")]
    pub protocol_binding: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub tenant: String,
    #[serde(default, deserialize_with = "null_default")]
    pub protocol_version: String,
}

/// `lf.a2a.v1.AgentProvider`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentProvider {
    #[serde(default, deserialize_with = "null_default")]
    pub url: String,
    #[serde(default, deserialize_with = "null_default")]
    pub organization: String,
}

/// `lf.a2a.v1.AgentExtension`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExtension {
    #[serde(default, deserialize_with = "null_default")]
    pub uri: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub description: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "is_false"
    )]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Struct>,
}

/// `lf.a2a.v1.AgentCapabilities`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub push_notifications: Option<bool>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub extensions: Vec<AgentExtension>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extended_agent_card: Option<bool>,
}

/// `lf.a2a.v1.StringList`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StringList {
    #[serde(default, deserialize_with = "null_default")]
    pub list: Vec<String>,
}

/// `lf.a2a.v1.SecurityRequirement`: scheme name -> required scopes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SecurityRequirement {
    #[serde(default, deserialize_with = "null_default")]
    pub schemes: BTreeMap<String, StringList>,
}

/// `lf.a2a.v1.APIKeySecurityScheme`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeySecurityScheme {
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub description: String,
    #[serde(default, deserialize_with = "null_default")]
    pub location: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
}

/// `lf.a2a.v1.HTTPAuthSecurityScheme`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HttpAuthSecurityScheme {
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub description: String,
    #[serde(default, deserialize_with = "null_default")]
    pub scheme: String,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "String::is_empty"
    )]
    pub bearer_format: String,
}

/// `lf.a2a.v1.SecurityScheme` (the `scheme` oneof). OAuth 2.0, OpenID
/// Connect and mTLS are kept as JSON: this client only reports them.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecurityScheme {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_security_scheme: Option<ApiKeySecurityScheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_auth_security_scheme: Option<HttpAuthSecurityScheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth2_security_scheme: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_id_connect_security_scheme: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mtls_security_scheme: Option<Value>,
}

impl SecurityScheme {
    /// A short description: `http bearer`, `apiKey header X-Key`, `oauth2`, ...
    pub fn describe(&self) -> String {
        if let Some(h) = &self.http_auth_security_scheme {
            return format!("http {}", h.scheme.to_ascii_lowercase());
        }
        if let Some(k) = &self.api_key_security_scheme {
            return format!("apiKey {} {}", k.location, k.name);
        }
        if self.oauth2_security_scheme.is_some() {
            return "oauth2".into();
        }
        if self.open_id_connect_security_scheme.is_some() {
            return "openIdConnect".into();
        }
        if self.mtls_security_scheme.is_some() {
            return "mutualTls".into();
        }
        "unknown".into()
    }

    /// HTTP Bearer: the one scheme whose credential this client can send.
    pub fn is_http_bearer(&self) -> bool {
        self.http_auth_security_scheme
            .as_ref()
            .is_some_and(|h| h.scheme.eq_ignore_ascii_case("bearer"))
    }
}

/// `lf.a2a.v1.AgentSkill`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkill {
    #[serde(default, deserialize_with = "null_default")]
    pub id: String,
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub description: String,
    #[serde(default, deserialize_with = "null_default")]
    pub tags: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub examples: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub input_modes: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub output_modes: Vec<String>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub security_requirements: Vec<SecurityRequirement>,
}

/// `lf.a2a.v1.AgentCardSignature` (kept, never verified by this client).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentCardSignature {
    #[serde(default, deserialize_with = "null_default")]
    pub protected: String,
    #[serde(default, deserialize_with = "null_default")]
    pub signature: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<Struct>,
}

/// `lf.a2a.v1.AgentCard`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCard {
    #[serde(default, deserialize_with = "null_default")]
    pub name: String,
    #[serde(default, deserialize_with = "null_default")]
    pub description: String,
    #[serde(default, deserialize_with = "null_default")]
    pub supported_interfaces: Vec<AgentInterface>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    #[serde(default, deserialize_with = "null_default")]
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub capabilities: AgentCapabilities,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub security_schemes: BTreeMap<String, SecurityScheme>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub security_requirements: Vec<SecurityRequirement>,
    #[serde(default, deserialize_with = "null_default")]
    pub default_input_modes: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub default_output_modes: Vec<String>,
    #[serde(default, deserialize_with = "null_default")]
    pub skills: Vec<AgentSkill>,
    #[serde(
        default,
        deserialize_with = "null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub signatures: Vec<AgentCardSignature>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
}

/// `Major.Minor` of a protocol version; patch numbers do not count (§3.6).
pub fn major_minor(v: &str) -> Option<(u32, u32)> {
    let mut it = v.trim().split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    if let Some(patch) = it.next() {
        patch.parse::<u32>().ok()?;
    }
    it.next().is_none().then_some((major, minor))
}

// ---------------------------------------------------------------- validation

/// Required-field checks (spec §5.7) on what a peer returned. The error
/// text names the path that failed.
pub mod validate {
    use super::*;

    fn need(cond: bool, what: impl FnOnce() -> String) -> Result<(), String> {
        if cond { Ok(()) } else { Err(what()) }
    }

    pub fn parts(path: &str, parts: &[Part]) -> Result<(), String> {
        need(!parts.is_empty(), || {
            format!("{path}.parts is empty (REQUIRED)")
        })
    }

    pub fn message(path: &str, m: &Message) -> Result<(), String> {
        need(!m.message_id.is_empty(), || {
            format!("{path}.messageId is missing (REQUIRED)")
        })?;
        need(m.role != Role::Unspecified, || {
            format!("{path}.role is missing (REQUIRED)")
        })?;
        parts(path, &m.parts)
    }

    pub fn artifact(path: &str, a: &Artifact) -> Result<(), String> {
        need(!a.artifact_id.is_empty(), || {
            format!("{path}.artifactId is missing (REQUIRED)")
        })?;
        parts(path, &a.parts)
    }

    pub fn task(path: &str, t: &Task) -> Result<(), String> {
        need(!t.id.is_empty(), || {
            format!("{path}.id is missing (REQUIRED)")
        })?;
        need(t.status.state != TaskState::Unspecified, || {
            format!("{path}.status.state is missing (REQUIRED)")
        })?;
        if let Some(m) = &t.status.message {
            message(&format!("{path}.status.message"), m)?;
        }
        for (i, a) in t.artifacts.iter().flatten().enumerate() {
            artifact(&format!("{path}.artifacts[{i}]"), a)?;
        }
        for (i, m) in t.history.iter().enumerate() {
            message(&format!("{path}.history[{i}]"), m)?;
        }
        Ok(())
    }

    pub fn send_response(r: &SendMessageResponse) -> Result<(), String> {
        match r {
            SendMessageResponse::Task(t) => task("result.task", t),
            SendMessageResponse::Message(m) => message("result.message", m),
        }
    }

    pub fn card(c: &AgentCard) -> Result<(), String> {
        need(!c.name.trim().is_empty(), || {
            "agent card name is missing (REQUIRED)".into()
        })?;
        need(!c.supported_interfaces.is_empty(), || {
            "agent card supportedInterfaces is empty (REQUIRED; A2A 0.3 cards are not supported)"
                .into()
        })?;
        for (i, iface) in c.supported_interfaces.iter().enumerate() {
            need(
                !iface.url.is_empty()
                    && !iface.protocol_binding.is_empty()
                    && !iface.protocol_version.is_empty(),
                || {
                    format!(
                        "supportedInterfaces[{i}] lacks url, protocolBinding or protocolVersion (REQUIRED)"
                    )
                },
            )?;
        }
        Ok(())
    }
}
