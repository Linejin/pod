//! Discovery, interface selection and the four core operations.
//!
//! Blocking API on a private current-thread runtime, so the CLI and later a
//! UI background thread can call it the same way. Every call is one HTTP
//! exchange; nothing is retried, cached on disk or logged.

use std::sync::Arc;
use std::time::Duration;

use http::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::{A2aError, Delivery};
use crate::http::{self as transport, BearerToken, Exchange, Limits};
use crate::jsonrpc;
use crate::model::{
    AgentCard, BINDING_JSONRPC, CancelTaskRequest, GetTaskRequest, ListTasksRequest,
    ListTasksResponse, Message, PROTOCOL_VERSION, SendMessageConfiguration, SendMessageRequest,
    SendMessageResponse, Task, major_minor, validate,
};
use crate::sanitize_line;
use crate::target::{Endpoint, Origin, TargetPolicy};

/// Client limits and policy.
#[derive(Clone, Copy, Debug)]
pub struct ClientConfig {
    pub policy: TargetPolicy,
    pub connect_timeout: Duration,
    /// Per call, from sending to the last response byte. A blocking
    /// SendMessage waits for the task to finish or pause, so it can be long.
    pub request_timeout: Duration,
    pub max_card_bytes: usize,
    pub max_response_bytes: usize,
    /// Largest request body this client sends.
    pub max_request_bytes: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            policy: TargetPolicy::default(),
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(120),
            max_card_bytes: 512 * 1024,
            max_response_bytes: 8 * 1024 * 1024,
            max_request_bytes: 2 * 1024 * 1024,
        }
    }
}

/// Where the Agent Card comes from.
#[derive(Clone, Debug)]
pub enum CardSource {
    /// A peer base URL: `{base}/.well-known/agent-card.json`.
    Peer(String),
    /// An explicit Agent Card URL, fetched as given.
    CardUrl(String),
}

/// A fetched and validated Agent Card.
#[derive(Clone, Debug)]
pub struct Discovery {
    pub card_url: Endpoint,
    pub card: AgentCard,
}

/// The interface chosen from `supportedInterfaces` (spec §8.3.2).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectedInterface {
    /// Position in `supportedInterfaces`.
    pub index: usize,
    pub url: String,
    pub protocol_binding: String,
    pub protocol_version: String,
    /// Sent as `tenant` in every request when not empty.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tenant: String,
    /// Whether the interface was named explicitly (`--endpoint`).
    pub explicit: bool,
    #[serde(skip)]
    endpoint: Endpoint,
}

impl SelectedInterface {
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, A2aError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| A2aError::Transport {
            delivery: Delivery::NotSent,
            detail: format!("cannot start the I/O runtime: {e}"),
        })
}

/// Fetches and validates the Agent Card (no credential is sent).
pub fn discover(source: &CardSource, cfg: &ClientConfig) -> Result<Discovery, A2aError> {
    let card_url = match source {
        CardSource::Peer(base) => Endpoint::parse(base, cfg.policy)?.well_known_card(),
        CardSource::CardUrl(u) => Endpoint::parse(u, cfg.policy)?,
    };
    let rt = runtime()?;
    let tls = tls_for(&card_url)?;
    let resp = rt.block_on(transport::exchange(
        tls.as_ref(),
        Exchange {
            method: Method::GET,
            endpoint: &card_url,
            accept: "application/json",
            body: None,
            bearer: None,
        },
        Limits {
            connect_timeout: cfg.connect_timeout,
            response_timeout: cfg.connect_timeout.max(Duration::from_secs(30)),
            max_response_bytes: cfg.max_card_bytes,
        },
    ))?;
    if resp.status != 200 {
        return Err(A2aError::Card(format!(
            "{card_url} answered HTTP {}{}",
            resp.status,
            excerpt(&resp.body)
        )));
    }
    let mt = transport::media_type(resp.content_type.as_deref());
    if !is_json_media_type(&mt) {
        return Err(A2aError::Card(format!(
            "{card_url} is not JSON (Content-Type {:?})",
            sanitize_line(&mt, 80)
        )));
    }
    let card: AgentCard = serde_json::from_slice(&resp.body)
        .map_err(|e| A2aError::Card(format!("{card_url}: {e}")))?;
    validate::card(&card).map_err(A2aError::Card)?;
    Ok(Discovery { card_url, card })
}

/// TLS roots are loaded for https endpoints only.
fn tls_for(ep: &Endpoint) -> Result<Option<Arc<rustls::ClientConfig>>, A2aError> {
    match ep.scheme {
        crate::target::Scheme::Https => transport::tls_config().map(Some),
        crate::target::Scheme::Http => Ok(None),
    }
}

fn is_json_media_type(mt: &str) -> bool {
    mt == "application/json" || mt == "application/a2a+json"
}

fn excerpt(body: &[u8]) -> String {
    let s = sanitize_line(&String::from_utf8_lossy(&body[..body.len().min(200)]), 200);
    if s.is_empty() {
        String::new()
    } else {
        format!(": {s}")
    }
}

impl Discovery {
    /// Every advertised interface as `BINDING VERSION URL`.
    pub fn advertised(&self) -> Vec<String> {
        self.card
            .supported_interfaces
            .iter()
            .map(|i| {
                format!(
                    "{} {} {}",
                    sanitize_line(&i.protocol_binding, 60),
                    sanitize_line(&i.protocol_version, 20),
                    sanitize_line(&i.url, 200)
                )
            })
            .collect()
    }

    /// Whether this client can use the interface (binding and version only).
    pub fn is_compatible(binding: &str, version: &str) -> bool {
        binding == BINDING_JSONRPC && major_minor(version) == major_minor(PROTOCOL_VERSION)
    }

    /// Selects the interface to talk to (spec §8.3.2): the first JSON-RPC
    /// interface for A2A 1.0, or the one whose URL equals `explicit`.
    ///
    /// The card is peer data, not permission: without `explicit`, the
    /// interface must be on the card's own origin (same scheme, host and
    /// port), so a card cannot send this client, or a credential, to a host
    /// the user did not name. `explicit` must be one of the advertised
    /// compatible URLs, and never downgrades an https card to http.
    pub fn select(
        &self,
        explicit: Option<&str>,
        policy: TargetPolicy,
    ) -> Result<SelectedInterface, A2aError> {
        let card_origin = self.card_url.origin();
        let wanted = explicit.map(|e| Endpoint::parse(e, policy)).transpose()?;
        let mut cross_origin: Option<String> = None;
        let mut refused: Option<A2aError> = None;
        for (index, iface) in self.card.supported_interfaces.iter().enumerate() {
            if !Self::is_compatible(&iface.protocol_binding, &iface.protocol_version) {
                continue;
            }
            let endpoint = match Endpoint::parse(&iface.url, policy) {
                Ok(e) => e,
                Err(e) => {
                    refused.get_or_insert(e);
                    continue;
                }
            };
            let chosen = match &wanted {
                Some(w) => *w == endpoint,
                None => {
                    if endpoint.origin() != card_origin {
                        cross_origin.get_or_insert_with(|| sanitize_line(&iface.url, 200));
                        continue;
                    }
                    true
                }
            };
            if !chosen {
                continue;
            }
            if self.card_url.scheme == crate::target::Scheme::Https
                && endpoint.scheme != crate::target::Scheme::Https
            {
                return Err(A2aError::Target(format!(
                    "refusing to downgrade from the https agent card to {}",
                    sanitize_line(&iface.url, 200)
                )));
            }
            return Ok(SelectedInterface {
                index,
                url: endpoint.to_string(),
                protocol_binding: iface.protocol_binding.clone(),
                protocol_version: iface.protocol_version.clone(),
                tenant: iface.tenant.clone(),
                explicit: wanted.is_some(),
                endpoint,
            });
        }
        if let Some(w) = wanted {
            return Err(A2aError::Target(format!(
                "{w} is not a JSONRPC {PROTOCOL_VERSION} interface advertised by the agent card"
            )));
        }
        if let Some(interface) = cross_origin {
            return Err(A2aError::CrossOrigin {
                card_origin: card_origin.to_string(),
                interface,
            });
        }
        if let Some(e) = refused {
            return Err(e);
        }
        Err(A2aError::NoCompatibleInterface {
            advertised: self.advertised(),
        })
    }
}

/// A client bound to one selected interface.
pub struct A2aClient {
    rt: tokio::runtime::Runtime,
    tls: Option<Arc<rustls::ClientConfig>>,
    interface: SelectedInterface,
    bearer: Option<BearerToken>,
    cfg: ClientConfig,
}

impl A2aClient {
    /// A client for `interface`. `bearer` is sent on every JSON-RPC request
    /// to that interface's endpoint and nowhere else.
    pub fn new(
        interface: SelectedInterface,
        bearer: Option<BearerToken>,
        cfg: ClientConfig,
    ) -> Result<Self, A2aError> {
        Ok(Self {
            rt: runtime()?,
            tls: tls_for(&interface.endpoint)?,
            interface,
            bearer,
            cfg,
        })
    }

    pub fn interface(&self) -> &SelectedInterface {
        &self.interface
    }

    pub fn origin(&self) -> Origin {
        self.interface.endpoint.origin()
    }

    fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, A2aError> {
        let id = format!("pod-{}", uuid::Uuid::new_v4());
        let body = jsonrpc::encode_request(&id, method, params);
        if body.len() > self.cfg.max_request_bytes {
            return Err(A2aError::Invalid(format!(
                "request of {} bytes exceeds the {}-byte limit",
                body.len(),
                self.cfg.max_request_bytes
            )));
        }
        let resp = self.rt.block_on(transport::exchange(
            self.tls.as_ref(),
            Exchange {
                method: Method::POST,
                endpoint: &self.interface.endpoint,
                accept: "application/json",
                body: Some(body),
                bearer: self.bearer.as_ref(),
            },
            Limits {
                connect_timeout: self.cfg.connect_timeout,
                response_timeout: self.cfg.request_timeout,
                max_response_bytes: self.cfg.max_response_bytes,
            },
        ))?;
        let mt = transport::media_type(resp.content_type.as_deref());
        let json = is_json_media_type(&mt);
        if !(200..300).contains(&resp.status) {
            // A JSON-RPC error body is the peer's answer whatever the status.
            if json && jsonrpc::looks_like_response(&resp.body) {
                return jsonrpc::decode_response(&id, &resp.body);
            }
            return Err(A2aError::Http {
                status: resp.status,
                excerpt: sanitize_line(
                    &String::from_utf8_lossy(&resp.body[..resp.body.len().min(200)]),
                    200,
                ),
            });
        }
        if !json {
            return Err(A2aError::Protocol(format!(
                "Content-Type {:?} is not application/json",
                sanitize_line(&mt, 80)
            )));
        }
        jsonrpc::decode_response(&id, &resp.body)
    }

    /// `SendMessage`. `message.role` must be user; the selected interface's
    /// tenant is filled in. Never retried: on an error whose
    /// [`A2aError::outcome_unknown`] is true the message may have arrived.
    pub fn send_message(
        &self,
        message: Message,
        configuration: Option<SendMessageConfiguration>,
    ) -> Result<SendMessageResponse, A2aError> {
        validate::message("message", &message).map_err(A2aError::Invalid)?;
        let req = SendMessageRequest {
            tenant: self.interface.tenant.clone(),
            message,
            configuration,
            metadata: None,
        };
        let r: SendMessageResponse = self.call(jsonrpc::SEND_MESSAGE, &req)?;
        validate::send_response(&r).map_err(A2aError::Protocol)?;
        Ok(r)
    }

    /// `GetTask`.
    pub fn get_task(&self, id: &str, history_length: Option<i32>) -> Result<Task, A2aError> {
        let req = GetTaskRequest {
            tenant: self.interface.tenant.clone(),
            id: id.to_string(),
            history_length,
        };
        let t: Task = self.call(jsonrpc::GET_TASK, &req)?;
        check_task(id, &t)?;
        Ok(t)
    }

    /// `ListTasks` (one page).
    pub fn list_tasks(&self, mut req: ListTasksRequest) -> Result<ListTasksResponse, A2aError> {
        req.tenant = self.interface.tenant.clone();
        let r: ListTasksResponse = self.call(jsonrpc::LIST_TASKS, &req)?;
        for (i, t) in r.tasks.iter().enumerate() {
            validate::task(&format!("result.tasks[{i}]"), t).map_err(A2aError::Protocol)?;
        }
        Ok(r)
    }

    /// `CancelTask`.
    pub fn cancel_task(&self, id: &str) -> Result<Task, A2aError> {
        let req = CancelTaskRequest {
            tenant: self.interface.tenant.clone(),
            id: id.to_string(),
            metadata: None,
        };
        let t: Task = self.call(jsonrpc::CANCEL_TASK, &req)?;
        check_task(id, &t)?;
        Ok(t)
    }
}

fn check_task(id: &str, t: &Task) -> Result<(), A2aError> {
    validate::task("result", t).map_err(A2aError::Protocol)?;
    if t.id != id {
        return Err(A2aError::Protocol(format!(
            "asked for task {id:?}, the peer returned task {:?}",
            sanitize_line(&t.id, 100)
        )));
    }
    Ok(())
}
