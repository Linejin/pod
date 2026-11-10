//! Transport-level LSP client: request/response matching, notifications and
//! server-to-client requests over any byte stream pair.
//!
//! A reader thread parses frames; responses complete the pending request's
//! oneshot, everything else goes to the [`Handler`]. Writes go through a
//! mutex so any thread may send. When the stream ends every pending request
//! fails with [`SERVER_GONE`] and `on_exit` runs once.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use futures::channel::oneshot;
use serde_json::Value;

use super::rpc::{self, Message, RpcError, SERVER_GONE};

/// Callbacks run on the reader thread.
pub trait Handler: Send + Sync + 'static {
    fn notification(&self, method: &str, params: Value);
    /// Answers a server->client request.
    fn request(&self, method: &str, params: Value) -> Result<Value, RpcError>;
    /// The stream closed (process exit or broken pipe).
    fn exited(&self);
}

type Pending = Mutex<HashMap<i64, oneshot::Sender<Result<Value, RpcError>>>>;

struct Inner {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: Pending,
    next_id: AtomicI64,
    alive: AtomicBool,
}

#[derive(Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    pub fn start(
        reader: Box<dyn Read + Send>,
        writer: Box<dyn Write + Send>,
        handler: Arc<dyn Handler>,
    ) -> Client {
        let inner = Arc::new(Inner {
            writer: Mutex::new(writer),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            alive: AtomicBool::new(true),
        });
        let weak = Arc::downgrade(&inner);
        std::thread::Builder::new()
            .name("lsp-reader".into())
            .spawn(move || {
                let mut r = BufReader::with_capacity(1 << 16, reader);
                read_loop(&mut r, &weak, &*handler);
                if let Some(inner) = weak.upgrade() {
                    // Under the pending lock: a request either got in before
                    // this drain (and fails here) or sees `alive == false`.
                    let mut map = inner.pending.lock().unwrap();
                    inner.alive.store(false, Ordering::SeqCst);
                    let pending: Vec<_> = map.drain().collect();
                    drop(map);
                    for (_, tx) in pending {
                        let _ = tx.send(Err(RpcError {
                            code: SERVER_GONE,
                            message: "language server exited".into(),
                        }));
                    }
                }
                handler.exited();
            })
            .expect("spawn lsp reader");
        Client { inner }
    }

    pub fn is_alive(&self) -> bool {
        self.inner.alive.load(Ordering::SeqCst)
    }

    fn send(&self, msg: &Message) -> Result<(), RpcError> {
        let mut w = self.inner.writer.lock().unwrap();
        rpc::write_message(&mut **w, &msg.to_value()).map_err(|e| {
            self.inner.alive.store(false, Ordering::SeqCst);
            RpcError {
                code: SERVER_GONE,
                message: format!("write failed: {e}"),
            }
        })
    }

    /// Sends a request; the future resolves with the raw result.
    pub fn request(
        &self,
        method: &str,
        params: Value,
    ) -> impl std::future::Future<Output = Result<Value, RpcError>> + Send + 'static {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        // `alive` is read under the pending lock the reader drains under, so
        // a request can never be left waiting after the server is gone.
        let registered = {
            let mut map = self.inner.pending.lock().unwrap();
            if self.is_alive() {
                map.insert(id, tx);
                true
            } else {
                false
            }
        };
        let sent = if registered {
            let r = self.send(&Message::Request {
                id: Value::from(id),
                method: method.into(),
                params,
            });
            if r.is_err() {
                self.inner.pending.lock().unwrap().remove(&id);
            }
            r
        } else {
            Err(RpcError {
                code: SERVER_GONE,
                message: "language server is not running".into(),
            })
        };
        async move {
            sent?;
            rx.await.unwrap_or_else(|_| {
                Err(RpcError {
                    code: SERVER_GONE,
                    message: "request dropped".into(),
                })
            })
        }
    }

    /// Typed request helper.
    pub fn request_typed<R: lsp_types::request::Request>(
        &self,
        params: R::Params,
    ) -> impl std::future::Future<Output = Result<R::Result, RpcError>> + Send + 'static {
        let fut = self.request(
            R::METHOD,
            serde_json::to_value(params).unwrap_or(Value::Null),
        );
        async move {
            let v = fut.await?;
            serde_json::from_value(v).map_err(|e| RpcError {
                code: -32700,
                message: format!("bad {} result: {e}", R::METHOD),
            })
        }
    }

    pub fn notify(&self, method: &str, params: Value) -> Result<(), RpcError> {
        super::server::trace("->", method, &params);
        self.send(&Message::Notification {
            method: method.into(),
            params,
        })
    }

    pub fn notify_typed<N: lsp_types::notification::Notification>(
        &self,
        params: N::Params,
    ) -> Result<(), RpcError> {
        self.notify(
            N::METHOD,
            serde_json::to_value(params).unwrap_or(Value::Null),
        )
    }

    pub fn pending_count(&self) -> usize {
        self.inner.pending.lock().unwrap().len()
    }
}

fn read_loop(r: &mut dyn BufRead, inner: &std::sync::Weak<Inner>, handler: &dyn Handler) {
    loop {
        let v = match rpc::read_message(r) {
            Ok(Some(v)) => v,
            Ok(None) | Err(_) => return,
        };
        let Some(msg) = Message::parse(v) else {
            continue;
        };
        match msg {
            Message::Response { id, result } => {
                let Some(inner) = inner.upgrade() else { return };
                if let Some(id) = id.as_i64()
                    && let Some(tx) = inner.pending.lock().unwrap().remove(&id)
                {
                    let _ = tx.send(result);
                }
            }
            Message::Notification { method, params } => handler.notification(&method, params),
            Message::Request { id, method, params } => {
                let result = handler.request(&method, params);
                let Some(inner) = inner.upgrade() else { return };
                let mut w = inner.writer.lock().unwrap();
                let _ = rpc::write_message(&mut **w, &Message::Response { id, result }.to_value());
            }
        }
    }
}

/// Default answers for server->client requests Pod does not act on.
pub fn default_server_request(method: &str, params: &Value) -> Result<Value, RpcError> {
    match method {
        // One `null` per requested section: "use your defaults".
        "workspace/configuration" => {
            let n = params
                .get("items")
                .and_then(|i| i.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            Ok(Value::Array(vec![Value::Null; n]))
        }
        "client/registerCapability"
        | "client/unregisterCapability"
        | "window/workDoneProgress/create"
        | "workspace/semanticTokens/refresh"
        | "workspace/inlayHint/refresh"
        | "workspace/codeLens/refresh"
        | "workspace/diagnostic/refresh" => Ok(Value::Null),
        "window/showMessageRequest" => Ok(Value::Null),
        // The process owner (`server::Forward`) answers this one; here only without an editor.
        "workspace/applyEdit" => {
            Ok(serde_json::json!({"applied": false, "failureReason": "no editor attached"}))
        }
        _ => Err(RpcError {
            code: rpc::METHOD_NOT_FOUND,
            message: format!("unhandled: {method}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// In-process pipe pair standing in for a server's stdio.
    struct PipeReader(mpsc::Receiver<Vec<u8>>, Vec<u8>, usize);
    impl Read for PipeReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.2 >= self.1.len() {
                match self.0.recv() {
                    Ok(b) => {
                        self.1 = b;
                        self.2 = 0;
                    }
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len() - self.2);
            buf[..n].copy_from_slice(&self.1[self.2..self.2 + n]);
            self.2 += n;
            Ok(n)
        }
    }
    struct PipeWriter(mpsc::Sender<Vec<u8>>);
    impl Write for PipeWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .send(buf.to_vec())
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn pipe() -> (PipeWriter, PipeReader) {
        let (tx, rx) = mpsc::channel();
        (PipeWriter(tx), PipeReader(rx, Vec::new(), 0))
    }

    #[derive(Default)]
    struct Rec {
        notes: Mutex<Vec<String>>,
        exited: AtomicBool,
    }
    impl Handler for Rec {
        fn notification(&self, method: &str, _: Value) {
            self.notes.lock().unwrap().push(method.into());
        }
        fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
            default_server_request(method, &params)
        }
        fn exited(&self) {
            self.exited.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn requests_notifications_and_server_requests() {
        let (c2s_w, c2s_r) = pipe();
        let (s2c_w, s2c_r) = pipe();
        let rec = Arc::new(Rec::default());
        let client = Client::start(Box::new(s2c_r), Box::new(c2s_w), rec.clone());
        // Fake server thread: answers request id with params echo, then
        // sends a notification and a configuration request, then exits.
        let server = std::thread::spawn(move || {
            let mut r = BufReader::new(c2s_r);
            let mut w = s2c_w;
            let req = rpc::read_message(&mut r).unwrap().unwrap();
            let id = req["id"].clone();
            rpc::write_message(
                &mut w,
                &serde_json::json!({"jsonrpc":"2.0","method":"window/logMessage","params":{}}),
            )
            .unwrap();
            rpc::write_message(&mut w, &serde_json::json!({"jsonrpc":"2.0","id":"cfg1","method":"workspace/configuration","params":{"items":[{},{}]}})).unwrap();
            let cfg_reply = rpc::read_message(&mut r).unwrap().unwrap();
            rpc::write_message(
                &mut w,
                &serde_json::json!({"jsonrpc":"2.0","id":id,"result":{"echo":req["params"]}}),
            )
            .unwrap();
            cfg_reply
        });
        let res =
            futures::executor::block_on(client.request("test/echo", serde_json::json!({"x":1})))
                .unwrap();
        assert_eq!(res, serde_json::json!({"echo":{"x":1}}));
        let cfg = server.join().unwrap();
        assert_eq!(cfg["id"], "cfg1");
        assert_eq!(cfg["result"], serde_json::json!([null, null]));
        assert_eq!(rec.notes.lock().unwrap().as_slice(), ["window/logMessage"]);
        // Server side dropped: the client notices, pending/new requests fail.
        let t = std::time::Instant::now();
        while client.is_alive() && t.elapsed().as_secs() < 5 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!client.is_alive());
        assert!(rec.exited.load(Ordering::SeqCst));
        let err = futures::executor::block_on(client.request("x", Value::Null)).unwrap_err();
        assert_eq!(err.code, SERVER_GONE);
    }

    /// Requests sent while the server goes away all finish (none waits
    /// forever in `pending` after the reader drained it).
    #[test]
    fn requests_racing_the_exit_all_finish() {
        use futures::FutureExt as _;
        for _ in 0..20 {
            let (s2c_w, s2c_r) = pipe();
            let rec = Arc::new(Rec::default());
            // Writes always succeed: only the drain can end a request.
            let client = Client::start(Box::new(s2c_r), Box::new(std::io::sink()), rec.clone());
            let c2 = client.clone();
            let sender = std::thread::spawn(move || {
                (0..300)
                    .map(|_| c2.request("x", Value::Null).boxed())
                    .collect::<Vec<_>>()
            });
            drop(s2c_w);
            let futs = sender.join().unwrap();
            let t = std::time::Instant::now();
            while !rec.exited.load(Ordering::SeqCst) && t.elapsed().as_secs() < 5 {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            for f in futs {
                let r = f.now_or_never().expect("finished, not left pending");
                assert_eq!(r.unwrap_err().code, SERVER_GONE);
            }
        }
    }

    #[test]
    fn pending_requests_fail_when_server_dies() {
        let (c2s_w, c2s_r) = pipe();
        let (s2c_w, s2c_r) = pipe();
        let client = Client::start(Box::new(s2c_r), Box::new(c2s_w), Arc::new(Rec::default()));
        let fut = client.request("never/answered", Value::Null);
        // Wait until the request is on the wire, then kill the "server".
        let mut r = BufReader::new(c2s_r);
        rpc::read_message(&mut r).unwrap();
        assert_eq!(client.pending_count(), 1);
        drop(s2c_w);
        let err = futures::executor::block_on(fut).unwrap_err();
        assert_eq!(err.code, SERVER_GONE);
        assert_eq!(client.pending_count(), 0);
    }
}
