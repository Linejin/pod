//! The method dispatch: the single entry point of every request that is not
//! a stream ([`Daemon::handle_request`]), the params/result helpers, and
//! [`mode`], which tells the connection which methods turn it into a stream.
//! Both look a method up in the registry: `registry` holds the types and
//! the lookup index, `domains` the registrations of each domain.

mod domains;
mod registry;

use std::sync::{Arc, LazyLock};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::Daemon;
use super::connection::{err_response, ok_response};
use crate::protocol::*;
pub(in crate::server) use registry::Mode;
use registry::{RpcRegistry, unknown_method};

/// Built on first use from the constant tables, read-only afterwards.
static REGISTRY: LazyLock<RpcRegistry> =
    LazyLock::new(|| RpcRegistry::new(domains::REGISTERED, domains::CLAIMS));

/// The [`Mode`] of `method` (the connection switches to a stream only for a
/// request with an id).
pub(in crate::server) fn mode(method: &str) -> Mode {
    REGISTRY.get(method).map_or(Mode::Unary, |r| r.mode)
}

pub(in crate::server) fn params<P: DeserializeOwned>(v: Value) -> Result<P, RpcError> {
    let v = if v.is_null() {
        Value::Object(Default::default())
    } else {
        v
    };
    serde_json::from_value(v)
        .map_err(|e| RpcError::new(codes::INVALID_PARAMS, format!("invalid params: {e}")))
}

pub(in crate::server) fn to_value<T: Serialize>(t: T) -> Result<Value, RpcError> {
    serde_json::to_value(t).map_err(|e| RpcError::new(codes::INTERNAL_ERROR, e.to_string()))
}

impl Daemon {
    /// Returns `None` for notifications.
    pub(in crate::server) async fn handle_request(
        self: Arc<Self>,
        req: Request,
    ) -> Option<Response> {
        let id = req.id.clone();
        if req.jsonrpc != JSONRPC_VERSION {
            return Some(err_response(
                id.unwrap_or(Value::Null),
                RpcError::new(codes::INVALID_REQUEST, "jsonrpc must be \"2.0\""),
            ));
        }
        let result = self.dispatch(&req.method, req.params).await;
        let id = id?;
        Some(match result {
            Ok(v) => ok_response(id, v),
            Err(e) => err_response(id, e),
        })
    }

    async fn dispatch(self: Arc<Self>, method: &str, p: Value) -> Result<Value, RpcError> {
        match REGISTRY.handler(method) {
            Some(handler) => handler(self, method, p).await,
            None => Err(unknown_method(method)),
        }
    }
}
