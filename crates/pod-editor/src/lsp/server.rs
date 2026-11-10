//! One running language-server process: spawn, `initialize`, `shutdown`.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use lsp_types as lt;
use serde_json::{Value, json};

use super::client::{Client, Handler, default_server_request};
use super::job::ProcessTree;
use super::registry::ServerConfig;
use super::rpc::RpcError;
use crate::text::PosEncoding;

#[derive(Debug, Clone, PartialEq)]
pub enum ServerEvent {
    Notification {
        method: String,
        params: Value,
    },
    /// `workspace/applyEdit`: the reader thread waits (up to
    /// [`APPLY_EDIT_WAIT`]) for the owner to apply it and answer on `reply`.
    ApplyEdit {
        params: Value,
        reply: EditReply,
    },
    Exited,
}

/// How long a server's `workspace/applyEdit` waits for the editor.
pub const APPLY_EDIT_WAIT: Duration = Duration::from_secs(5);

/// One-shot answer to a `workspace/applyEdit` request. Dropping it unanswered
/// answers `applied: false`.
#[derive(Clone, Debug)]
pub struct EditReply(Arc<Mutex<Option<std::sync::mpsc::Sender<bool>>>>);

impl EditReply {
    pub fn new() -> (EditReply, std::sync::mpsc::Receiver<bool>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (EditReply(Arc::new(Mutex::new(Some(tx)))), rx)
    }

    pub fn send(&self, applied: bool) {
        if let Some(tx) = self.0.lock().unwrap().take() {
            let _ = tx.send(applied);
        }
    }
}

impl PartialEq for EditReply {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for EditReply {}

/// Waits for the owner's answer to a forwarded `workspace/applyEdit`.
pub fn apply_edit_response(rx: std::sync::mpsc::Receiver<bool>, wait: Duration) -> Value {
    match rx.recv_timeout(wait) {
        Ok(true) => json!({ "applied": true }),
        Ok(false) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            json!({ "applied": false, "failureReason": "the editor could not apply the edit" })
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            json!({ "applied": false, "failureReason": "the editor did not answer in time" })
        }
    }
}

/// Forwards reader-thread callbacks to the owner, tagged with a generation
/// so events of a replaced (crashed) process are ignored.
struct Forward {
    tx: UnboundedSender<(u64, ServerEvent)>,
    generation: u64,
}

/// Appends to `$POD_EDITOR_LSP_LOG` (if set): one line per incoming message.
pub fn trace(dir: &str, method: &str, params: &Value) {
    if let Some(path) = std::env::var_os("POD_EDITOR_LSP_LOG") {
        let mut s = params.to_string();
        if s.len() > 400 {
            let mut cut = 400;
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            s.truncate(cut);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = std::io::Write::write_all(
                &mut f,
                format!("{:?} {dir} {method} {s}\n", std::time::SystemTime::now()).as_bytes(),
            );
        }
    }
}

impl Handler for Forward {
    fn notification(&self, method: &str, params: Value) {
        trace("<-", method, &params);
        let _ = self.tx.unbounded_send((
            self.generation,
            ServerEvent::Notification {
                method: method.into(),
                params,
            },
        ));
    }
    fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        trace("<-req", method, &params);
        if method == "workspace/applyEdit" {
            let (reply, rx) = EditReply::new();
            let _ = self
                .tx
                .unbounded_send((self.generation, ServerEvent::ApplyEdit { params, reply }));
            return Ok(apply_edit_response(rx, APPLY_EDIT_WAIT));
        }
        default_server_request(method, &params)
    }
    fn exited(&self) {
        let _ = self
            .tx
            .unbounded_send((self.generation, ServerEvent::Exited));
    }
}

pub struct ServerProcess {
    pub client: Client,
    pub tree: ProcessTree,
    child: Mutex<Child>,
    /// Last lines of stderr, shown when the server crashes.
    pub stderr_tail: Arc<Mutex<VecDeque<String>>>,
}

impl ServerProcess {
    pub fn spawn(
        exe: &Path,
        cfg: &ServerConfig,
        root: &Path,
        tx: UnboundedSender<(u64, ServerEvent)>,
        generation: u64,
    ) -> std::io::Result<ServerProcess> {
        let mut cmd = Command::new(exe);
        cmd.args(&cfg.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Agent shells export NO_COLOR etc.; servers do not need them, but
        // they must not inherit our console either.
        // CREATE_NO_WINDOW; started suspended until it is in its job.
        let (mut child, tree) = super::job::spawn_in_tree(&mut cmd, 0x0800_0000)?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let t2 = tail.clone();
        std::thread::Builder::new()
            .name("lsp-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let mut t = t2.lock().unwrap();
                    if t.len() >= 40 {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            })
            .ok();
        let client = Client::start(
            Box::new(stdout),
            Box::new(stdin),
            Arc::new(Forward { tx, generation }),
        );
        Ok(ServerProcess {
            client,
            tree,
            child: Mutex::new(child),
            stderr_tail: tail,
        })
    }

    /// Asks politely (shutdown + exit), then kills the whole tree.
    pub async fn shutdown(&self, timer: impl std::future::Future<Output = ()>) {
        if self.client.is_alive() {
            let req = self.client.request("shutdown", Value::Null);
            futures::pin_mut!(req);
            futures::pin_mut!(timer);
            if let futures::future::Either::Left((Ok(_), _)) =
                futures::future::select(req, timer).await
            {
                let _ = self.client.notify("exit", Value::Null);
            }
        }
        self.kill();
    }

    /// Kills the process tree immediately (also what `Drop` does).
    pub fn kill(&self) {
        self.tree.terminate();
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    pub fn exited(&self) -> bool {
        self.child
            .lock()
            .map(|mut c| c.try_wait().ok().flatten().is_some())
            .unwrap_or(true)
    }

    /// The last `n` lines the server wrote to stderr (its log).
    pub fn stderr_lines(&self, n: usize) -> Vec<String> {
        let t = self.stderr_tail.lock().unwrap();
        let skip = t.len().saturating_sub(n);
        t.iter().skip(skip).cloned().collect()
    }

    pub fn stderr_summary(&self) -> String {
        let t = self.stderr_tail.lock().unwrap();
        t.iter()
            .rev()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.kill();
    }
}

/// What a successful `initialize` gives the store.
#[derive(Clone, Debug)]
pub struct Initialized {
    pub capabilities: lt::ServerCapabilities,
    pub encoding: PosEncoding,
    pub server_name: Option<String>,
}

pub fn client_capabilities() -> Value {
    json!({
        "general": { "positionEncodings": ["utf-32", "utf-16"] },
        "window": { "workDoneProgress": true, "showDocument": { "support": false } },
        "workspace": {
            "configuration": true,
            "workspaceFolders": true,
            "applyEdit": true,
            "workspaceEdit": { "documentChanges": true },
            "didChangeWatchedFiles": { "dynamicRegistration": false }
        },
        "textDocument": {
            "synchronization": { "didSave": true, "willSave": false, "dynamicRegistration": false },
            "hover": { "contentFormat": ["markdown", "plaintext"] },
            "completion": {
                "completionItem": {
                    "snippetSupport": true,
                    "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] },
                    "documentationFormat": ["markdown", "plaintext"],
                    "labelDetailsSupport": true,
                    "deprecatedSupport": true
                },
                "contextSupport": true
            },
            "definition": { "linkSupport": true },
            "codeAction": {
                "codeActionLiteralSupport": { "codeActionKind": { "valueSet": ["", "quickfix", "refactor", "refactor.extract", "refactor.inline", "refactor.rewrite", "source", "source.organizeImports"] } },
                "resolveSupport": { "properties": ["edit"] },
                "dataSupport": true,
                "isPreferredSupport": true,
                "disabledSupport": true
            },
            "references": {},
            "rename": { "prepareSupport": false },
            "publishDiagnostics": { "relatedInformation": true, "versionSupport": true, "tagSupport": { "valueSet": [1, 2] } }
        }
    })
}

pub fn initialize_params(root: &Path, init_options: Option<Value>) -> Value {
    let uri = crate::uri::path_to_uri_string(root);
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "root".into());
    json!({
        "processId": std::process::id(),
        "clientInfo": { "name": "pod-editor", "version": env!("CARGO_PKG_VERSION") },
        "rootUri": uri,
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": [{ "uri": uri, "name": name }],
        "capabilities": client_capabilities(),
        "initializationOptions": init_options,
        "trace": "off",
        "locale": "ko"
    })
}

/// Runs the `initialize` handshake.
pub async fn initialize(
    client: &Client,
    root: &Path,
    init_options: Option<Value>,
) -> Result<Initialized, RpcError> {
    let res = client
        .request("initialize", initialize_params(root, init_options))
        .await?;
    let parsed: lt::InitializeResult = serde_json::from_value(res).map_err(|e| RpcError {
        code: -32700,
        message: format!("bad initialize result: {e}"),
    })?;
    client.notify("initialized", json!({}))?;
    Ok(Initialized {
        encoding: PosEncoding::from_kind(parsed.capabilities.position_encoding.as_ref()),
        server_name: parsed.server_info.map(|s| s.name),
        capabilities: parsed.capabilities,
    })
}

/// Backoff before restart attempt `n` (0-based): 1, 2, 4, 8, 16 s.
pub fn restart_backoff(n: u32) -> Option<Duration> {
    (n < MAX_RESTARTS).then(|| Duration::from_secs(1u64 << n.min(4)))
}
pub const MAX_RESTARTS: u32 = 5;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_edit_waits_for_the_editor_answer() {
        let (reply, rx) = EditReply::new();
        let r2 = reply.clone();
        assert_eq!(reply, r2);
        std::thread::spawn(move || r2.send(true));
        assert_eq!(
            apply_edit_response(rx, Duration::from_secs(5)),
            json!({ "applied": true })
        );
        // Dropped unanswered (no editor took it): not applied, at once.
        let (reply, rx) = EditReply::new();
        drop(reply);
        assert_eq!(
            apply_edit_response(rx, Duration::from_secs(5))["applied"],
            false
        );
        // No answer in time.
        let (_keep, rx) = EditReply::new();
        assert_eq!(
            apply_edit_response(rx, Duration::from_millis(10))["failureReason"],
            "the editor did not answer in time"
        );
        // A second answer is ignored.
        let (reply, rx) = EditReply::new();
        reply.send(false);
        reply.send(true);
        assert_eq!(
            apply_edit_response(rx, Duration::from_secs(1))["applied"],
            false
        );
    }

    #[test]
    fn backoff_doubles_then_gives_up() {
        let v: Vec<_> = (0..6).map(restart_backoff).collect();
        assert_eq!(v[0], Some(Duration::from_secs(1)));
        assert_eq!(v[3], Some(Duration::from_secs(8)));
        assert_eq!(v[4], Some(Duration::from_secs(16)));
        assert_eq!(v[5], None);
    }

    #[test]
    fn initialize_params_use_file_uri() {
        let root = if cfg!(windows) {
            Path::new(r"C:\w\my repo")
        } else {
            Path::new("/w/my repo")
        };
        let p = initialize_params(root, None);
        let uri = p["rootUri"].as_str().unwrap();
        assert!(uri.starts_with("file:///"), "{uri}");
        assert!(uri.ends_with("my%20repo"));
        assert_eq!(
            p["capabilities"]["general"]["positionEncodings"][0],
            "utf-32"
        );
    }
}
