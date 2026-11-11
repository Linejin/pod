//! The request connection: typed and raw requests of the feature
//! controllers (review, groups, orchestration, mods, sessions, PRs), one at a
//! time, kept off the control connection (docs/09 §5-4).

use std::sync::mpsc as std_mpsc;

use podd::client::Client;
use serde_json::Value;

use super::control::is_transport;
use super::{Reply, SourceError};

/// A request for the request connection: method, params, reply.
pub(super) type RpcCmd = (&'static str, Value, Reply<Value>);

/// The request connection: serves [`super::PoddSource::request`] one at a
/// time. Connects lazily and never starts podd (the control connection
/// does); a transport error drops the connection and the next request
/// reconnects.
pub(super) fn request_loop(pipe: String, rx: std_mpsc::Receiver<RpcCmd>) {
    let mut client: Option<Client> = None;
    while let Ok((method, params, reply)) = rx.recv() {
        if client.is_none() {
            client = Client::connect(&pipe).ok();
        }
        let Some(c) = client.as_mut() else {
            let _ = reply.send(Err(SourceError::msg("not connected to podd")));
            continue;
        };
        match c.call_raw(method, params) {
            Ok(v) => drop(reply.send(Ok(v))),
            Err(e) => {
                if is_transport(&e) {
                    client = None;
                }
                let _ = reply.send(Err(SourceError::of(&e)));
            }
        }
    }
}
