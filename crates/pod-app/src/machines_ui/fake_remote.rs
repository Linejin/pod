//! A fake remote Pod for the machine-UI tests (no ssh, no daemon; none may
//! be installed here). It speaks the NDJSON JSON-RPC that `pod
//! remote-stdio` relays, enough for the app: `server.status`,
//! `connection.hello`, `terminal.list`, `task.list`, `terminal.attach`
//! (inline snapshot, then `terminal.output`), `terminal.input_once` with the
//! daemon's at-most-once ledger and `check` fencing, `terminal.resize`.
//!
//! It sits behind pod-test-support's loopback [`FaultProxy`]
//! ([`FakeRemote::proxy`]) so tests cut, restore and delay the link the way
//! the R4 transport tests do; [`ProxyDial`] is the app's dialer for it.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pod_remote::Machine;
use pod_remote::session::Link;
use pod_test_support::fault_proxy::{FaultProxy, Upstream};
use podd::client::Client;
use podd::protocol::*;
use podd::transport::Transport;
use serde_json::{Value, json};

use super::controller::Dial;
use super::model::Failure;

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

#[derive(Default)]
struct Ledger {
    newest: u64,
    bytes: HashMap<u64, usize>,
    fenced: HashSet<u64>,
}

#[derive(Default)]
struct State {
    machine_id: String,
    boot_id: String,
    terminals: Vec<TerminalInfo>,
    output: HashMap<String, Vec<u8>>,
    ledger: HashMap<(String, String), Ledger>,
    typed: Vec<(String, Vec<u8>)>,
    calls: Vec<String>,
    attached: Vec<(String, Writer)>,
}

#[derive(Clone)]
pub struct FakeRemote(Arc<Mutex<State>>);

impl FakeRemote {
    /// A remote with one running terminal per `(id, name)`.
    pub fn new(machine_id: &str, terminals: &[(&str, &str)]) -> Self {
        let st = State {
            machine_id: machine_id.into(),
            boot_id: "boot-1".into(),
            terminals: terminals
                .iter()
                .enumerate()
                .map(|(i, (id, name))| TerminalInfo {
                    id: (*id).into(),
                    name: Some((*name).into()),
                    argv: vec!["bash".into()],
                    cwd: "/home/test".into(),
                    cols: 80,
                    rows: 24,
                    created_ms: 1_000 + i as u64,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        Self(Arc::new(Mutex::new(st)))
    }

    /// A loopback proxy whose every connection reaches this remote.
    pub fn proxy(&self) -> FaultProxy {
        let me = self.clone();
        FaultProxy::start(move || me.upstream()).expect("proxy")
    }

    fn upstream(&self) -> std::io::Result<Upstream> {
        let (server_r, proxy_w) = std::io::pipe()?;
        let (proxy_r, server_w) = std::io::pipe()?;
        let w: Writer = Arc::new(Mutex::new(Box::new(server_w)));
        let me = self.clone();
        std::thread::spawn(move || me.serve(BufReader::new(server_r), w));
        Ok(Upstream {
            reader: Box::new(proxy_r),
            writer: Box::new(proxy_w),
            close: Box::new(|| {}),
        })
    }

    /// Output on terminal `id` (sent to every attached view).
    pub fn emit(&self, id: &str, data: &[u8]) {
        let mut st = self.0.lock().unwrap();
        let out = st.output.entry(id.into()).or_default();
        let seq = out.len() as u64;
        out.extend_from_slice(data);
        let line = note(
            NOTIFY_OUTPUT,
            json!({"id": id, "seq": seq, "data": b64_encode(data)}),
        );
        st.attached.retain(|(t, w)| {
            t != id || {
                let mut w = w.lock().unwrap();
                w.write_all(line.as_bytes())
                    .and_then(|()| w.flush())
                    .is_ok()
            }
        });
    }

    /// Bytes the remote programs received, per terminal, in order.
    pub fn typed(&self) -> Vec<(String, Vec<u8>)> {
        self.0.lock().unwrap().typed.clone()
    }

    pub fn typed_text(&self, id: &str) -> String {
        self.typed()
            .into_iter()
            .filter(|(t, _)| t == id)
            .map(|(_, b)| String::from_utf8_lossy(&b).into_owned())
            .collect()
    }

    /// Requests of `method` seen so far.
    pub fn calls(&self, method: &str) -> usize {
        let st = self.0.lock().unwrap();
        st.calls.iter().filter(|m| *m == method).count()
    }

    /// Every request that changes something remotely.
    pub fn mutations(&self) -> usize {
        ["terminal.input_once", "terminal.input", "terminal.resize"]
            .iter()
            .map(|m| self.calls(m))
            .sum()
    }

    /// Another machine (or a reinstalled one) now answers here.
    pub fn set_machine_id(&self, id: &str) {
        self.0.lock().unwrap().machine_id = id.into();
    }

    fn serve(&self, r: BufReader<std::io::PipeReader>, w: Writer) {
        for line in r.lines() {
            let Ok(line) = line else { break };
            let Ok(req) = serde_json::from_str::<Request>(&line) else {
                break;
            };
            let Some(id) = req.id.clone() else { continue };
            let target = (req.method == TerminalAttach::NAME)
                .then(|| serde_json::from_value::<AttachParams>(req.params.clone()).ok())
                .flatten()
                .and_then(|p| p.target.id);
            // One lock for answer + registration: no output can slip in
            // before the attach reply or be missed after it.
            let mut st = self.0.lock().unwrap();
            let text = match Self::handle(&mut st, &req.method, req.params) {
                Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}),
                Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
            };
            let ok = {
                let mut g = w.lock().unwrap();
                g.write_all(format!("{text}\n").as_bytes())
                    .and_then(|()| g.flush())
                    .is_ok()
            };
            if !ok {
                break;
            }
            if let Some(t) = target {
                st.attached.push((t, w.clone()));
            }
        }
        // The client side ended: let go of this connection's output end so
        // the proxy (and the client's blocked read) see the close at once.
        self.0
            .lock()
            .unwrap()
            .attached
            .retain(|(_, x)| !Arc::ptr_eq(x, &w));
    }

    fn handle(st: &mut State, method: &str, params: Value) -> Result<Value, RpcError> {
        st.calls.push(method.to_string());
        let bad = |e: serde_json::Error| RpcError::new(codes::INVALID_PARAMS, e.to_string());
        match method {
            "server.status" => Ok(json!({
                "version": "test", "protocol": PROTOCOL_VERSION, "pid": 1,
                "pipe": "fake", "started_ms": 0, "uptime_ms": 0,
                "terminals": st.terminals.len(), "running": st.terminals.len(),
                "state_dir": "/fake",
            })),
            "connection.hello" => {
                let p: HelloParams = serde_json::from_value(params).map_err(bad)?;
                Ok(serde_json::to_value(HelloResult {
                    protocol: p.max_protocol.min(PROTOCOL_VERSION),
                    min_protocol: 12,
                    max_protocol: PROTOCOL_VERSION,
                    capabilities: Vec::new(),
                    machine_id: st.machine_id.clone(),
                    boot_id: st.boot_id.clone(),
                    version: "test".into(),
                    os: "linux".into(),
                    limits: FrameLimits {
                        max_line_bytes: 8 << 20,
                        max_request_bytes: 8 << 20,
                        max_output_chunk_bytes: 32 << 10,
                        attach_queue_bytes: 2 << 20,
                        // The fake remote serves no files (v22 fs.*).
                        max_read_bytes: None,
                        max_write_bytes: None,
                    },
                })
                .unwrap())
            }
            "terminal.list" => Ok(json!({"terminals": st.terminals})),
            "task.list" => Ok(json!({"tasks": []})),
            "terminal.resize" => {
                let p: ResizeParams = serde_json::from_value(params).map_err(bad)?;
                let t = st
                    .terminals
                    .iter()
                    .find(|t| Some(&t.id) == p.target.id.as_ref())
                    .cloned()
                    .ok_or_else(|| RpcError::new(codes::INVALID_PARAMS, "no terminal"))?;
                Ok(serde_json::to_value(t).unwrap())
            }
            "terminal.attach" => {
                let p: AttachParams = serde_json::from_value(params).map_err(bad)?;
                let t = st
                    .terminals
                    .iter()
                    .find(|t| Some(&t.id) == p.target.id.as_ref())
                    .cloned()
                    .ok_or_else(|| RpcError::new(codes::INVALID_PARAMS, "no terminal"))?;
                let out = st.output.get(&t.id).cloned().unwrap_or_default();
                let mut snap = b"\x1b[0m\x1b[H\x1b[2J\x1b[3J".to_vec();
                snap.extend_from_slice(&out);
                Ok(json!({
                    "terminal": t,
                    "seq": out.len(),
                    "snapshot": b64_encode(&snap),
                }))
            }
            "terminal.input_once" => {
                let p: InputOnceParams = serde_json::from_value(params).map_err(bad)?;
                let id = p.target.id.clone().unwrap_or_default();
                let data =
                    b64_decode(&p.data).map_err(|e| RpcError::new(codes::INVALID_PARAMS, e))?;
                let led = st.ledger.entry((id.clone(), p.stream.clone())).or_default();
                let outcome = if p.check {
                    match led.bytes.get(&p.seq) {
                        Some(n) => ("already", *n),
                        None => {
                            led.fenced.insert(p.seq);
                            ("fenced", 0)
                        }
                    }
                } else if led.fenced.contains(&p.seq) {
                    ("fenced", 0)
                } else if p.seq <= led.newest {
                    ("already", led.bytes.get(&p.seq).copied().unwrap_or(0))
                } else {
                    led.newest = p.seq;
                    led.bytes.insert(p.seq, data.len());
                    st.typed.push((id.clone(), data.clone()));
                    ("written", data.len())
                };
                Ok(json!({"id": id, "outcome": outcome.0, "bytes": outcome.1}))
            }
            other => Err(RpcError::new(
                codes::METHOD_NOT_FOUND,
                format!("fake remote: {other}"),
            )),
        }
    }
}

fn note(method: &str, params: Value) -> String {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "method": method, "params": params})
    )
}

/// The app's dialer for a [`FakeRemote`] behind a proxy: TCP to the proxy,
/// then the real client handshake (`server.status` + `connection.hello`).
pub struct ProxyDial {
    pub addr: SocketAddr,
}

impl Dial for ProxyDial {
    fn dial(&self, _: &Machine) -> Result<Link, Box<Failure>> {
        let fail = |m: String| Box::new(Failure::other(m, true));
        let s = TcpStream::connect(self.addr).map_err(|e| fail(e.to_string()))?;
        let _ = s.set_nodelay(true);
        let r = s.try_clone().map_err(|e| fail(e.to_string()))?;
        let closer_sock = s.try_clone().map_err(|e| fail(e.to_string()))?;
        let guard = s.try_clone().map_err(|e| fail(e.to_string()))?;
        let mut c = Client::from_transport("ssh:test", Transport::new(r, s, 8 << 20));
        // A handshake into a dead link must not hang the test.
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            if rx.recv_timeout(Duration::from_secs(10)).is_err() {
                let _ = guard.shutdown(Shutdown::Both);
            }
        });
        let hello = c.hello(&[]);
        let _ = tx.send(());
        let hello = hello.map_err(|e| fail(e.to_string()))?;
        Ok(Link::new(
            c,
            hello,
            Arc::new(move || {
                let _ = closer_sock.shutdown(Shutdown::Both);
            }),
        ))
    }
}

/// A profile that exists only in the test.
pub fn machine(name: &str) -> Machine {
    Machine {
        name: name.into(),
        host: "fake.example.com".into(),
        user: Some("tester".into()),
        port: None,
        identity_file: None,
        known_hosts: None,
    }
}
