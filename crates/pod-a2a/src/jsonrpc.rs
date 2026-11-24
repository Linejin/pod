//! The JSON-RPC 2.0 envelope of the A2A JSON-RPC binding (spec §9).

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::error::{A2aError, RpcError, RpcErrorKind};

/// Method names (spec §5.3: PascalCase, as in gRPC).
pub const SEND_MESSAGE: &str = "SendMessage";
pub const GET_TASK: &str = "GetTask";
pub const LIST_TASKS: &str = "ListTasks";
pub const CANCEL_TASK: &str = "CancelTask";

#[derive(Serialize)]
struct Request<'a, P: Serialize> {
    jsonrpc: &'static str,
    id: &'a str,
    method: &'a str,
    params: &'a P,
}

/// The request body for `method` with `params` and the string id `id`.
pub fn encode_request<P: Serialize>(id: &str, method: &str, params: &P) -> Vec<u8> {
    serde_json::to_vec(&Request {
        jsonrpc: "2.0",
        id,
        method,
        params,
    })
    .expect("A2A request params serialize")
}

/// Whether `body` looks like a JSON-RPC response object (for non-2xx HTTP
/// statuses: a JSON-RPC error there is still the peer's answer).
pub fn looks_like_response(body: &[u8]) -> bool {
    serde_json::from_slice::<Map<String, Value>>(body)
        .is_ok_and(|m| m.get("jsonrpc").and_then(Value::as_str) == Some("2.0"))
}

/// Decodes the response to request `id`: its `result` as `T`, or the
/// peer's JSON-RPC error.
pub fn decode_response<T: DeserializeOwned>(id: &str, body: &[u8]) -> Result<T, A2aError> {
    let bad = |s: String| A2aError::Protocol(s);
    let mut obj: Map<String, Value> = serde_json::from_slice(body)
        .map_err(|e| bad(format!("response is not a JSON object: {e}")))?;
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(bad("response lacks \"jsonrpc\": \"2.0\"".into()));
    }
    let result = obj.remove("result");
    let error = obj.remove("error");
    let rid = obj.remove("id").unwrap_or(Value::Null);
    match (result, error) {
        (Some(_), Some(_)) => Err(bad("response has both result and error".into())),
        (None, None) => Err(bad("response has neither result nor error".into())),
        (None, Some(err)) => {
            // An error may carry `id: null` when the server could not read
            // the request id (JSON-RPC 2.0 §5); any other id must be ours.
            if !(rid.is_null() || rid.as_str() == Some(id)) {
                return Err(bad(format!(
                    "error response id {rid} does not match request id"
                )));
            }
            Err(A2aError::Rpc(parse_error(err)?))
        }
        (Some(res), None) => {
            if rid.as_str() != Some(id) {
                return Err(bad(format!(
                    "response id {rid} does not match request id {id:?}"
                )));
            }
            serde_json::from_value(res).map_err(|e| bad(format!("result: {e}")))
        }
    }
}

fn parse_error(v: Value) -> Result<RpcError, A2aError> {
    let Value::Object(mut o) = v else {
        return Err(A2aError::Protocol("error is not an object".into()));
    };
    let code = o
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| A2aError::Protocol("error.code is not an integer".into()))?;
    let message = match o.remove("message") {
        Some(Value::String(s)) => s,
        _ => String::new(),
    };
    Ok(RpcError {
        code,
        kind: RpcErrorKind::from_code(code),
        message,
        data: o.remove("data").filter(|d| !d.is_null()),
    })
}
