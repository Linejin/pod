//! Request ids: the remembered result of an earlier call, so a repeated
//! request gets the same answer (the oldest are dropped first).

use serde_json::Value;

use super::{ErrorKind, MAX_REQUESTS, Orch, OrchError, RequestEntry, Result};
use crate::model::Ms;

impl Orch {
    /// The stored result of an earlier call with this request id.
    pub fn request(&self, id: &str, method: &str) -> Result<Option<Value>> {
        match self.index.requests.iter().find(|r| r.id == id) {
            Some(r) if r.method == method => Ok(Some(r.result.clone())),
            Some(r) => Err(OrchError::new(
                ErrorKind::InvalidParams,
                format!("request_id {id} was used for {}", r.method),
            )),
            None => Ok(None),
        }
    }

    /// Remember a result for a request id.
    pub fn remember(&mut self, id: &str, method: &str, result: Value, now: Ms) -> Result<()> {
        self.index.requests.retain(|r| r.id != id);
        self.index.requests.push(RequestEntry {
            id: id.to_string(),
            method: method.to_string(),
            result,
            ms: now,
        });
        let n = self.index.requests.len();
        if n > MAX_REQUESTS {
            self.index.requests.drain(..n - MAX_REQUESTS);
        }
        self.save_index()
    }
}
