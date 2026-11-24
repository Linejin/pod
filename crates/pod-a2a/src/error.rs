//! What can go wrong, in words a person and a script can both act on.

use std::fmt;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// Whether a request may have reached the peer when it failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Delivery {
    /// Failed before any request byte was written (DNS, TCP, TLS).
    NotSent,
    /// The request may have been received and acted on.
    Unknown,
}

/// The A2A error a JSON-RPC error code stands for (spec §5.4, §9.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum RpcErrorKind {
    JsonParse,
    InvalidRequest,
    MethodNotFound,
    InvalidParams,
    Internal,
    TaskNotFound,
    TaskNotCancelable,
    PushNotificationNotSupported,
    UnsupportedOperation,
    ContentTypeNotSupported,
    InvalidAgentResponse,
    ExtendedAgentCardNotConfigured,
    ExtensionSupportRequired,
    VersionNotSupported,
    /// Any other code (servers may define their own, e.g. auth errors).
    Other,
}

impl RpcErrorKind {
    pub fn from_code(code: i64) -> Self {
        match code {
            -32700 => Self::JsonParse,
            -32600 => Self::InvalidRequest,
            -32601 => Self::MethodNotFound,
            -32602 => Self::InvalidParams,
            -32603 => Self::Internal,
            -32001 => Self::TaskNotFound,
            -32002 => Self::TaskNotCancelable,
            -32003 => Self::PushNotificationNotSupported,
            -32004 => Self::UnsupportedOperation,
            -32005 => Self::ContentTypeNotSupported,
            -32006 => Self::InvalidAgentResponse,
            -32007 => Self::ExtendedAgentCardNotConfigured,
            -32008 => Self::ExtensionSupportRequired,
            -32009 => Self::VersionNotSupported,
            _ => Self::Other,
        }
    }

    /// The spec's error name (`TaskNotFoundError`, ...).
    pub fn name(self) -> &'static str {
        match self {
            Self::JsonParse => "JSONParseError",
            Self::InvalidRequest => "InvalidRequestError",
            Self::MethodNotFound => "MethodNotFoundError",
            Self::InvalidParams => "InvalidParamsError",
            Self::Internal => "InternalError",
            Self::TaskNotFound => "TaskNotFoundError",
            Self::TaskNotCancelable => "TaskNotCancelableError",
            Self::PushNotificationNotSupported => "PushNotificationNotSupportedError",
            Self::UnsupportedOperation => "UnsupportedOperationError",
            Self::ContentTypeNotSupported => "ContentTypeNotSupportedError",
            Self::InvalidAgentResponse => "InvalidAgentResponseError",
            Self::ExtendedAgentCardNotConfigured => "ExtendedAgentCardNotConfiguredError",
            Self::ExtensionSupportRequired => "ExtensionSupportRequiredError",
            Self::VersionNotSupported => "VersionNotSupportedError",
            Self::Other => "error",
        }
    }
}

/// A JSON-RPC error object returned by the peer.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcError {
    pub code: i64,
    pub kind: RpcErrorKind,
    pub message: String,
    /// `error.data`: ProtoJSON `Any` objects (`@type`) per spec §9.5.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// `google.rpc.ErrorInfo.reason` values in `error.data`.
    pub fn reasons(&self) -> Vec<String> {
        let Some(Value::Array(items)) = &self.data else {
            return Vec::new();
        };
        items
            .iter()
            .filter(|d| {
                d.get("@type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| t.ends_with("google.rpc.ErrorInfo"))
            })
            .filter_map(|d| d.get("reason").and_then(Value::as_str).map(str::to_string))
            .collect()
    }
}

/// An A2A client failure.
#[derive(Clone, Debug, PartialEq)]
pub enum A2aError {
    /// The request itself is invalid (checked before sending).
    Invalid(String),
    /// The URL or option is not allowed by the endpoint policy.
    Target(String),
    /// The Agent Card could not be fetched, parsed or validated.
    Card(String),
    /// The card advertises no JSON-RPC interface for A2A 1.0.
    NoCompatibleInterface { advertised: Vec<String> },
    /// The selected interface is on another origin than the card and was
    /// not named explicitly with an explicit endpoint.
    CrossOrigin {
        card_origin: String,
        interface: String,
    },
    /// Connecting, writing or reading failed.
    Transport { delivery: Delivery, detail: String },
    /// No complete response in time.
    Timeout { delivery: Delivery, after: Duration },
    /// The peer answered with a redirect; redirects are never followed.
    Redirect {
        status: u16,
        location: Option<String>,
    },
    /// HTTP 401/403: credentials missing, wrong, or insufficient.
    AuthRequired {
        status: u16,
        challenges: Vec<String>,
    },
    /// Any other non-success HTTP status without a JSON-RPC error body.
    Http { status: u16, excerpt: String },
    /// The response body exceeded the configured limit.
    TooLarge { limit: usize },
    /// The response is not a valid A2A 1.0 JSON-RPC response.
    Protocol(String),
    /// The peer returned a JSON-RPC error.
    Rpc(RpcError),
}

impl A2aError {
    /// Stable machine-readable name.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid-request",
            Self::Target(_) => "target-refused",
            Self::Card(_) => "agent-card",
            Self::NoCompatibleInterface { .. } => "no-compatible-interface",
            Self::CrossOrigin { .. } => "cross-origin-interface",
            Self::Transport { .. } => "transport",
            Self::Timeout { .. } => "timeout",
            Self::Redirect { .. } => "redirect-refused",
            Self::AuthRequired { .. } => "auth-required",
            Self::Http { .. } => "http-status",
            Self::TooLarge { .. } => "response-too-large",
            Self::Protocol(_) => "invalid-agent-response",
            Self::Rpc(_) => "rpc-error",
        }
    }

    /// True when a state-changing request (SendMessage, CancelTask) may have
    /// taken effect at the peer even though no valid answer came back. Such a
    /// request must not be repeated blindly.
    pub fn outcome_unknown(&self) -> bool {
        match self {
            Self::Transport { delivery, .. } | Self::Timeout { delivery, .. } => {
                *delivery == Delivery::Unknown
            }
            Self::TooLarge { .. } | Self::Protocol(_) => true,
            Self::Http { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

impl fmt::Display for A2aError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid request (not sent): {s}"),
            Self::Target(s) => write!(f, "endpoint refused: {s}"),
            Self::Card(s) => write!(f, "agent card: {s}"),
            Self::NoCompatibleInterface { advertised } => write!(
                f,
                "the agent card advertises no JSONRPC interface for A2A 1.0 (advertised: {})",
                if advertised.is_empty() {
                    "none".into()
                } else {
                    advertised.join("; ")
                }
            ),
            Self::CrossOrigin {
                card_origin,
                interface,
            } => write!(
                f,
                "the selected interface {interface} is not on the agent card's origin {card_origin}; \
                 name it explicitly with --endpoint to use it"
            ),
            Self::Transport { delivery, detail } => match delivery {
                Delivery::NotSent => write!(f, "connection failed before sending: {detail}"),
                Delivery::Unknown => write!(f, "connection failed after sending started: {detail}"),
            },
            Self::Timeout { delivery, after } => match delivery {
                Delivery::NotSent => write!(f, "timed out connecting after {after:?}"),
                Delivery::Unknown => write!(f, "no complete response within {after:?}"),
            },
            Self::Redirect { status, location } => write!(
                f,
                "HTTP {status} redirect{} not followed (redirects are refused)",
                location
                    .as_ref()
                    .map(|l| format!(" to {l}"))
                    .unwrap_or_default()
            ),
            Self::AuthRequired { status, challenges } => write!(
                f,
                "HTTP {status}: the peer requires authorization{}",
                if challenges.is_empty() {
                    String::new()
                } else {
                    format!(" (challenge: {})", challenges.join(", "))
                }
            ),
            Self::Http { status, excerpt } => {
                write!(f, "HTTP {status}")?;
                if !excerpt.is_empty() {
                    write!(f, ": {excerpt}")?;
                }
                Ok(())
            }
            Self::TooLarge { limit } => write!(f, "response larger than {limit} bytes; not read"),
            Self::Protocol(s) => write!(f, "invalid agent response: {s}"),
            Self::Rpc(e) => {
                write!(f, "{} ({}): {}", e.kind.name(), e.code, e.message)?;
                let reasons = e.reasons();
                if !reasons.is_empty() {
                    write!(f, " [{}]", reasons.join(", "))?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for A2aError {}
