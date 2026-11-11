//! LSP process lifecycle against the scripted fake server
//! (`pod-editor-fake-lsp`) and, when installed, a real rust-analyzer; and
//! the JSON the store sends a server, compared with a golden.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use ::gpui::{AppContext as _, Entity, Task, TestAppContext};
use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use lsp_types as lt;
use pod_editor::lsp::registry::{Availability, ServerRegistry, check_available, which};
use pod_editor::lsp::rpc::SERVER_GONE;
use pod_editor::lsp::server::{ServerEvent, ServerProcess, initialize};
use pod_editor::lsp::{FileEdit, LspStore, LspStoreEvent, ServerConfig, ServerState};
use pod_editor::text::{self, PosEncoding};
use pod_editor::uri::{path_to_uri_string, uri_str_to_path};
use pod_test_support::{tools, wait::until};
use serde_json::{Value, json};

fn fake_cfg(args: &[&str]) -> (PathBuf, ServerConfig) {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_pod-editor-fake-lsp"));
    let cfg = ServerConfig {
        id: "fake".into(),
        languages: vec!["rust".into()],
        command: exe.to_string_lossy().into_owned(),
        args: args.iter().map(|s| s.to_string()).collect(),
        root_markers: vec![],
        root_strategy: Default::default(),
        initialization_options: None,
        probe_args: vec![],
        install_hint: String::new(),
        enabled: true,
    };
    (exe, cfg)
}

/// Waits (blocking) for the next event matching `pred`.
fn next_event(
    rx: &mut UnboundedReceiver<(u64, ServerEvent)>,
    secs: u64,
    pred: impl Fn(&ServerEvent) -> bool,
) -> Option<ServerEvent> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        match rx.try_recv() {
            Ok((_, ev)) if pred(&ev) => return Some(ev),
            Ok(_) => {}
            Err(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    None
}

/// A non-blocking timer future (thread + oneshot).
fn timer(secs: u64) -> impl std::future::Future<Output = ()> {
    let (tx, rx) = futures::channel::oneshot::channel::<()>();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(secs));
        let _ = tx.send(());
    });
    async move {
        let _ = rx.await;
    }
}

fn block<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

#[test]
fn fake_server_full_lifecycle() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("my repo");
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("한글 main.rs");
    let text = "fn alpha() {}\nlet s = \"😀\"; BAD();\nalpha();\n";
    std::fs::write(&file, text).unwrap();
    let (exe, cfg) = fake_cfg(&[]);
    let (tx, mut rx) = unbounded();
    let p = ServerProcess::spawn(&exe, &cfg, &root, tx, 1).unwrap();
    let init = block(initialize(&p.client, &root, None)).unwrap();
    assert_eq!(
        init.encoding,
        PosEncoding::Utf16,
        "fake server speaks UTF-16"
    );
    assert_eq!(init.server_name.as_deref(), Some("fake-lsp"));

    let uri = path_to_uri_string(&file);
    p.client
        .notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"rust","version":0,"text":text}}),
        )
        .unwrap();
    let ev = next_event(&mut rx, 10, |e| matches!(e, ServerEvent::Notification { method, .. } if method == "textDocument/publishDiagnostics")).expect("diagnostics");
    let ServerEvent::Notification { params, .. } = ev else {
        unreachable!()
    };
    let pd: lt::PublishDiagnosticsParams = serde_json::from_value(params).unwrap();
    assert_eq!(
        uri_str_to_path(pd.uri.as_str()).unwrap(),
        file,
        "URI with spaces/Hangul round-trips"
    );
    assert_eq!(pd.diagnostics.len(), 1);
    // Wire range is UTF-16 ('😀' = 2 units); editor columns count chars.
    let wire = pd.diagnostics[0].range;
    assert_eq!(wire.start, lt::Position::new(1, 14));
    let ed = text::range_from_wire(text, wire, init.encoding);
    assert_eq!(ed.start, lt::Position::new(1, 13));
    assert_eq!(ed.end, lt::Position::new(1, 16));

    // Requests: hover / definition / references / rename / completion.
    let at = |line, ch| json!({"textDocument":{"uri":uri},"position":{"line":line,"character":ch}});
    let hover = block(p.client.request("textDocument/hover", at(2, 1))).unwrap();
    assert!(
        hover["contents"]["value"]
            .as_str()
            .unwrap()
            .contains("alpha")
    );
    let def: lt::GotoDefinitionResponse = serde_json::from_value(
        block(p.client.request("textDocument/definition", at(2, 1))).unwrap(),
    )
    .unwrap();
    match def {
        lt::GotoDefinitionResponse::Link(v) => {
            assert_eq!(v[0].target_selection_range.start, lt::Position::new(0, 3))
        }
        other => panic!("{other:?}"),
    }
    let refs = block(p.client.request("textDocument/references", at(0, 4))).unwrap();
    assert_eq!(refs.as_array().unwrap().len(), 2);
    let mut params = at(0, 4);
    params["newName"] = Value::from("beta");
    let we: lt::WorkspaceEdit =
        serde_json::from_value(block(p.client.request("textDocument/rename", params)).unwrap())
            .unwrap();
    assert_eq!(we.changes.unwrap().values().next().unwrap().len(), 2);
    let comp = block(p.client.request("textDocument/completion", at(2, 0))).unwrap();
    assert_eq!(comp["items"][0]["label"], "alpha");
    // Unknown method -> JSON-RPC error, connection stays usable.
    assert!(block(p.client.request("pod/unknown", Value::Null)).is_err());
    assert!(p.client.is_alive());

    // Polite shutdown: shutdown + exit, then the reader sees EOF.
    block(p.shutdown(timer(3)));
    assert!(next_event(&mut rx, 10, |e| *e == ServerEvent::Exited).is_some());
    assert!(p.exited());
}

#[test]
fn crash_is_reported_and_pending_requests_fail() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, cfg) = fake_cfg(&["--crash-on-open"]);
    let (tx, mut rx) = unbounded();
    let p = ServerProcess::spawn(&exe, &cfg, dir.path(), tx, 7).unwrap();
    block(initialize(&p.client, dir.path(), None)).unwrap();
    p.client.notify("textDocument/didOpen", json!({"textDocument":{"uri":"file:///C:/x.rs","languageId":"rust","version":0,"text":""}})).unwrap();
    // Sent after the fatal didOpen: never answered.
    let pending = p.client.request("textDocument/hover", json!({}));
    // The generation tag lets the store ignore events of replaced processes.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut got = None;
    while Instant::now() < deadline && got.is_none() {
        if let Ok((generation, ev)) = rx.try_recv() {
            if ev == ServerEvent::Exited {
                got = Some(generation);
            }
        } else {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert_eq!(got, Some(7));
    let err = block(pending).unwrap_err();
    assert_eq!(err.code, SERVER_GONE);
    assert!(!p.client.is_alive());
}

#[cfg(windows)]
fn pid_alive(pid: u32) -> bool {
    let out = std::process::Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(&format!("\"{pid}\""))
}

#[cfg(windows)]
#[test]
fn dropping_a_server_kills_its_process_tree() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("child.pid");
    let (exe, cfg) = fake_cfg(&["--child-pid-file", pid_file.to_str().unwrap()]);
    let (tx, _rx) = unbounded();
    let p = ServerProcess::spawn(&exe, &cfg, dir.path(), tx, 1).unwrap();
    // The file may exist before its pid is written, and the grandchild may
    // not be in the job yet: wait for a parsed, live pid inside the tree.
    let pid = until(Duration::from_secs(10), Duration::from_millis(50), || {
        let text = std::fs::read_to_string(&pid_file).unwrap_or_default();
        let pid: Option<u32> = text.trim().parse().ok();
        let active = p.tree.active_processes();
        match pid {
            Some(pid) if pid_alive(pid) && active >= 2 => Ok(pid),
            _ => Err((text, active)),
        }
    })
    .unwrap_or_else(|e| panic!("grandchild in the tree: {e}"));
    drop(p);
    let gone = until(Duration::from_secs(10), Duration::from_millis(100), || {
        if pid_alive(pid) { Err(pid) } else { Ok(()) }
    });
    assert!(gone.is_ok(), "grandchild {pid} must die with the job");
}

#[test]
fn missing_server_is_reported_not_spawned() {
    let mut cfg = ServerRegistry::default().servers[0].clone();
    cfg.command = "pod-no-such-language-server".into();
    match check_available(&cfg) {
        Availability::Missing(msg) => assert!(msg.contains("pod-no-such-language-server")),
        a => panic!("{a:?}"),
    }
}

// ---- real rust-analyzer ------------------------------------------------------

fn ra_available() -> Option<PathBuf> {
    let cfg = ServerRegistry::default().for_language("rust").cloned()?;
    match check_available(&cfg) {
        Availability::Installed(p) => Some(p),
        Availability::Missing(m) => {
            tools::missing(&format!("rust-analyzer ({m})"));
            None
        }
    }
}

fn wait_diag_for(
    rx: &mut UnboundedReceiver<(u64, ServerEvent)>,
    file: &Path,
    secs: u64,
) -> Option<Vec<lt::Diagnostic>> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let left = deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .max(1);
        let ev = next_event(
            rx,
            left,
            |e| matches!(e, ServerEvent::Notification { method, .. } if method == "textDocument/publishDiagnostics"),
        )?;
        if let ServerEvent::Notification { params, .. } = ev {
            let pd: lt::PublishDiagnosticsParams = serde_json::from_value(params).ok()?;
            if uri_str_to_path(pd.uri.as_str()).as_deref() == Some(file)
                && !pd.diagnostics.is_empty()
            {
                return Some(pd.diagnostics);
            }
        }
    }
    None
}

/// Real rust-analyzer on the small fixture repository. Ignored by default
/// (CPU heavy while it loads the sysroot); run with
/// `cargo test -p pod-editor --test lsp_lifecycle -- --ignored`.
#[test]
#[ignore]
fn rust_analyzer_smoke() {
    let Some(exe) = ra_available() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = pod_editor::fixture::create(&dir.path().join("demo repo"), true).unwrap();
    let root = pod_editor::uri::simplify(&root.canonicalize().unwrap());
    let cfg = ServerRegistry::default()
        .for_language("rust")
        .cloned()
        .unwrap();
    let (tx, mut rx) = unbounded();
    let p = ServerProcess::spawn(&exe, &cfg, &root, tx, 1).unwrap();
    let init = block(initialize(&p.client, &root, None)).expect("initialize");
    eprintln!("rust-analyzer encoding {:?}", init.encoding);
    let main = root.join("src").join("main.rs");
    let text = std::fs::read_to_string(&main).unwrap();
    let uri = path_to_uri_string(&main);
    p.client
        .notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"rust","version":0,"text":text}}),
        )
        .unwrap();
    // The fixture calls word_count with 2 args: rust-analyzer's native
    // diagnostic (or flycheck's E0061) reports it.
    let diags = wait_diag_for(&mut rx, &main, 180).expect("diagnostics for main.rs");
    assert!(
        diags
            .iter()
            .any(|d| d.severity == Some(lt::DiagnosticSeverity::ERROR)),
        "{diags:#?}"
    );
    // Go to definition across files: perimeter -> src/geometry.rs.
    let line = text
        .lines()
        .position(|l| l.contains("perimeter(&square)"))
        .unwrap() as u32;
    let col = text
        .lines()
        .nth(line as usize)
        .unwrap()
        .find("perimeter(&square)")
        .unwrap() as u32
        + 2;
    let at = json!({"textDocument":{"uri":uri},"position":{"line":line,"character":col}});
    let t = Instant::now();
    let def = loop {
        let v =
            block(p.client.request("textDocument/definition", at.clone())).unwrap_or(Value::Null);
        if !v.is_null() && v.as_array().is_none_or(|a| !a.is_empty()) {
            break v;
        }
        assert!(t.elapsed() < Duration::from_secs(120), "no definition");
        std::thread::sleep(Duration::from_millis(500));
    };
    let def: lt::GotoDefinitionResponse = serde_json::from_value(def).unwrap();
    let target = match def {
        lt::GotoDefinitionResponse::Link(v) => v[0].target_uri.clone(),
        lt::GotoDefinitionResponse::Array(v) => v[0].uri.clone(),
        lt::GotoDefinitionResponse::Scalar(l) => l.uri,
    };
    assert!(
        uri_str_to_path(target.as_str())
            .unwrap()
            .ends_with("geometry.rs"),
        "{}",
        target.as_str()
    );
    let hover = block(p.client.request("textDocument/hover", at)).unwrap();
    assert!(hover.to_string().contains("perimeter"), "{hover}");
    block(p.shutdown(timer(3)));
    assert!(p.exited());
    let _ = which("rust-analyzer");
}

/// `workspace/applyEdit` from a command: the request reaches the owner as
/// `ServerEvent::ApplyEdit`, and the server gets the owner's answer.
#[test]
fn server_apply_edit_is_forwarded_and_answered() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let file = root.join("a.rs");
    std::fs::write(&file, "fn a() {}\n").unwrap();
    let (exe, cfg) = fake_cfg(&[]);
    let (tx, mut rx) = unbounded();
    let p = ServerProcess::spawn(&exe, &cfg, &root, tx, 1).unwrap();
    block(initialize(&p.client, &root, None)).unwrap();
    let uri = path_to_uri_string(&file);
    for applied in [true, false] {
        let done = p.client.request(
            "workspace/executeCommand",
            json!({"command": "fake.insertHeader", "arguments": [uri]}),
        );
        let ev = next_event(&mut rx, 10, |e| matches!(e, ServerEvent::ApplyEdit { .. }))
            .expect("applyEdit forwarded");
        let ServerEvent::ApplyEdit { params, reply } = ev else {
            unreachable!()
        };
        let edit: lt::ApplyWorkspaceEditParams = serde_json::from_value(params).unwrap();
        assert_eq!(edit.label.as_deref(), Some("fake header"));
        assert_eq!(
            edit.edit.changes.unwrap().values().next().unwrap()[0].new_text,
            "// header\n"
        );
        reply.send(applied);
        let answer = block(done).expect("executeCommand answered");
        assert_eq!(answer["applied"], applied, "server saw the editor's answer");
    }
    block(p.shutdown(timer(3)));
}

/// Text edits of a code action's workspace edit for one file (editor
/// columns: rust-analyzer negotiates UTF-32 with Pod).
fn edits_for(we: &lt::WorkspaceEdit, uri: &str) -> Vec<lt::TextEdit> {
    // Servers spell URIs their own way (`file:///c%3A/...`): compare paths.
    let want = uri_str_to_path(uri);
    let same = |u: &str| uri_str_to_path(u) == want;
    let mut out = Vec::new();
    if let Some(ch) = &we.changes {
        for (u, e) in ch {
            if same(u.as_str()) {
                out.extend(e.iter().cloned());
            }
        }
    }
    let docs: Vec<&lt::TextDocumentEdit> = match &we.document_changes {
        Some(lt::DocumentChanges::Edits(v)) => v.iter().collect(),
        Some(lt::DocumentChanges::Operations(v)) => v
            .iter()
            .filter_map(|o| {
                if let lt::DocumentChangeOperation::Edit(e) = o {
                    Some(e)
                } else {
                    None
                }
            })
            .collect(),
        None => vec![],
    };
    for d in docs
        .into_iter()
        .filter(|d| same(d.text_document.uri.as_str()))
    {
        out.extend(d.edits.iter().map(|e| match e {
            lt::OneOf::Left(t) => t.clone(),
            lt::OneOf::Right(a) => a.text_edit.clone(),
        }));
    }
    out
}

/// Code actions and completion extras against a real rust-analyzer on the
/// fixture repository (docs/13 §7): the "Import `HashMap`" quick fix
/// (resolved lazily), an auto-import completion whose `additionalTextEdits`
/// come from `completionItem/resolve`, and snippet tab stops of a call
/// completion. Ignored by default (CPU heavy); run with
/// `cargo test -p pod-editor --test lsp_lifecycle rust_analyzer_code_actions -- --ignored`.
#[test]
#[ignore]
fn rust_analyzer_code_actions() {
    use pod_editor::lsp::snippet::Snippet;
    use pod_editor::lsp::store::{apply_edits, completion_extra};
    let Some(exe) = ra_available() else { return };
    let dir = tempfile::tempdir().unwrap();
    let root = pod_editor::fixture::create(&dir.path().join("demo repo"), false).unwrap();
    let root = pod_editor::uri::simplify(&root.canonicalize().unwrap());
    let main = root.join("src").join("main.rs");
    let mut text = std::fs::read_to_string(&main)
        .unwrap()
        .replace("\r\n", "\n");
    text.push_str("\nfn pod_imports() -> usize {\n    let m: HashMap<u8, u8> = HashMap::new();\n    m.len()\n}\n\nfn pod_complete() {\n    let _s = BTreeS\n}\n\nfn pod_call() {\n    let _n = word_c\n}\n");
    std::fs::write(&main, &text).unwrap();
    let cfg = ServerRegistry::default()
        .for_language("rust")
        .cloned()
        .unwrap();
    let (tx, mut rx) = unbounded();
    let p = ServerProcess::spawn(&exe, &cfg, &root, tx, 1).unwrap();
    let init = block(initialize(&p.client, &root, None)).expect("initialize");
    assert_eq!(init.encoding, PosEncoding::Utf32);
    let uri = path_to_uri_string(&main);
    p.client
        .notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":"rust","version":0,"text":text}}),
        )
        .unwrap();
    let diags = wait_diag_for(&mut rx, &main, 180).unwrap_or_default();
    eprintln!(
        "diagnostics: {:?}",
        diags.iter().map(|d| d.message.as_str()).collect::<Vec<_>>()
    );

    // 1. Quick fix: import HashMap.
    let line = text
        .lines()
        .position(|l| l.contains("HashMap::new()"))
        .unwrap() as u32;
    let col = text
        .lines()
        .nth(line as usize)
        .unwrap()
        .find("HashMap::new")
        .unwrap() as u32
        + 2;
    let range = json!({"start":{"line":line,"character":col},"end":{"line":line,"character":col}});
    let overlapping: Vec<&lt::Diagnostic> = diags
        .iter()
        .filter(|d| d.range.start.line <= line && line <= d.range.end.line)
        .collect();
    let t = Instant::now();
    let action = loop {
        let v = block(p.client.request(
            "textDocument/codeAction",
            json!({"textDocument":{"uri":uri},"range":range,"context":{"diagnostics":overlapping}}),
        ));
        let actions: Vec<lt::CodeActionOrCommand> = v
            .ok()
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        let hit = actions.into_iter().find_map(|a| match a {
            lt::CodeActionOrCommand::CodeAction(a)
                if a.title.contains("Import") && a.title.contains("HashMap") =>
            {
                Some(a)
            }
            _ => None,
        });
        if let Some(a) = hit {
            break a;
        }
        assert!(t.elapsed() < Duration::from_secs(120), "no import action");
        std::thread::sleep(Duration::from_millis(500));
    };
    eprintln!(
        "code action: {:?} (edit inline: {})",
        action.title,
        action.edit.is_some()
    );
    let action = if action.edit.is_none() {
        let v = block(
            p.client
                .request("codeAction/resolve", serde_json::to_value(&action).unwrap()),
        )
        .expect("resolve");
        serde_json::from_value::<lt::CodeAction>(v).unwrap()
    } else {
        action
    };
    let fixed = apply_edits(&text, &edits_for(action.edit.as_ref().expect("edit"), &uri));
    assert!(fixed.contains("use std::collections::HashMap;"), "{fixed}");
    assert!(
        fixed.contains("let m: HashMap<u8, u8> = HashMap::new();"),
        "the rest stays"
    );

    // 2. Auto-import completion: `BTreeS` → BTreeSet, its `use` from resolve.
    let line = text
        .lines()
        .position(|l| l.contains("let _s = BTreeS"))
        .unwrap() as u32;
    let col = text.lines().nth(line as usize).unwrap().len() as u32;
    let at = json!({"textDocument":{"uri":uri},"position":{"line":line,"character":col},"context":{"triggerKind":1}});
    let t = Instant::now();
    let item = loop {
        let v =
            block(p.client.request("textDocument/completion", at.clone())).unwrap_or(Value::Null);
        let items = match serde_json::from_value::<lt::CompletionResponse>(v) {
            Ok(lt::CompletionResponse::Array(v)) => v,
            Ok(lt::CompletionResponse::List(l)) => l.items,
            Err(_) => vec![],
        };
        if let Some(i) = items.into_iter().find(|i| i.label.starts_with("BTreeSet")) {
            break i;
        }
        assert!(
            t.elapsed() < Duration::from_secs(120),
            "no BTreeSet completion"
        );
        std::thread::sleep(Duration::from_millis(500));
    };
    eprintln!(
        "completion: {:?} additional inline: {}",
        item.label,
        item.additional_text_edits.is_some()
    );
    let extra = completion_extra(&item, true).expect("auto-import extra");
    let additional = match extra.resolve {
        Some(orig) => {
            let v = block(p.client.request(
                "completionItem/resolve",
                serde_json::to_value(&orig).unwrap(),
            ))
            .expect("resolve");
            serde_json::from_value::<lt::CompletionItem>(v)
                .unwrap()
                .additional_text_edits
                .unwrap_or_default()
        }
        None => extra.additional,
    };
    let imported = apply_edits(&text, &additional);
    assert!(
        imported.contains("use std::collections::BTreeSet;") || imported.contains("BTreeSet}"),
        "{imported}"
    );

    // 3. Snippet: a call completion has tab stops for its arguments.
    let line = text
        .lines()
        .position(|l| l.contains("let _n = word_c"))
        .unwrap() as u32;
    let col = text.lines().nth(line as usize).unwrap().len() as u32;
    let at = json!({"textDocument":{"uri":uri},"position":{"line":line,"character":col},"context":{"triggerKind":1}});
    let v = block(p.client.request("textDocument/completion", at)).unwrap_or(Value::Null);
    let items = match serde_json::from_value::<lt::CompletionResponse>(v) {
        Ok(lt::CompletionResponse::Array(v)) => v,
        Ok(lt::CompletionResponse::List(l)) => l.items,
        Err(_) => vec![],
    };
    let call = items
        .iter()
        .find(|i| i.label.starts_with("word_count"))
        .expect("word_count completion");
    let raw = match &call.text_edit {
        Some(lt::CompletionTextEdit::Edit(e)) => e.new_text.clone(),
        Some(lt::CompletionTextEdit::InsertAndReplace(e)) => e.new_text.clone(),
        None => call.insert_text.clone().unwrap_or_default(),
    };
    eprintln!("snippet: {raw:?} format {:?}", call.insert_text_format);
    assert_eq!(
        call.insert_text_format,
        Some(lt::InsertTextFormat::SNIPPET),
        "{raw}"
    );
    let sn = Snippet::parse(&raw);
    assert!(!sn.is_trivial(), "{raw} has stops");
    assert!(sn.text.starts_with("word_count("), "{}", sn.text);
    eprintln!("snippet text {:?} stops {:?}", sn.text, sn.stops);

    block(p.shutdown(timer(3)));
    assert!(p.exited());
}

// ---- the store's JSON on the wire ---------------------------------------------

/// `(event, state of the server of root/a.rs)` in emit order.
type Seen = Rc<RefCell<Vec<(LspStoreEvent, Option<ServerState>)>>>;

/// An [`LspStore`] on the GPUI test platform whose only server is the fake,
/// recording what it receives (`--wire`). Applies every `workspace/applyEdit`.
struct StoreHarness {
    cx: TestAppContext,
    store: Entity<LspStore>,
    events: Seen,
    /// Pump timers (restart backoff) while waiting.
    advance_clock: bool,
    root: PathBuf,
    wire: PathBuf,
    _dir: tempfile::TempDir,
}

impl StoreHarness {
    fn new(args: &[&str], backoff_scale: f32) -> Self {
        let dir = tempfile::tempdir().unwrap();
        // Space and Hangul in the root: percent-encoded URIs on the wire.
        let root = dir.path().join("wire 저장소");
        std::fs::create_dir_all(&root).unwrap();
        let root = pod_editor::uri::simplify(&root.canonicalize().unwrap());
        let wire = dir.path().join("wire.jsonl");
        let wire_arg = wire.to_string_lossy().into_owned();
        let mut all = vec!["--wire", wire_arg.as_str()];
        all.extend_from_slice(args);
        let (_, cfg) = fake_cfg(&all);
        let cx = TestAppContext::single();
        let registry = ServerRegistry { servers: vec![cfg] };
        let store = cx.update(|cx| {
            cx.new(|_| {
                let mut s = LspStore::with_registry(root.clone(), registry, vec![]);
                s.backoff_scale = backoff_scale;
                s
            })
        });
        let events: Seen = Rc::default();
        let seen = events.clone();
        let a = root.join("a.rs");
        cx.update(|cx| {
            cx.subscribe(&store, move |st, ev: &LspStoreEvent, cx| {
                if let LspStoreEvent::ApplyEdit { reply, .. } = ev {
                    reply.send(true);
                }
                let state = st.read(cx).status_for(&a).map(|s| s.state);
                seen.borrow_mut().push((ev.clone(), state));
            })
            .detach()
        });
        StoreHarness {
            cx,
            store,
            events,
            advance_clock: false,
            root,
            wire,
            _dir: dir,
        }
    }

    fn with<R>(&mut self, f: impl FnOnce(&mut LspStore, &mut ::gpui::Context<LspStore>) -> R) -> R {
        let store = self.store.clone();
        let r = self.cx.update(|cx| store.update(cx, f));
        self.cx.run_until_parked();
        r
    }

    fn read<R>(&self, f: impl FnOnce(&LspStore) -> R) -> R {
        self.cx.read(|cx| f(self.store.read(cx)))
    }

    /// Runs GPUI until `probe` holds (the server answers on its own threads).
    fn wait<T, O: std::fmt::Debug>(
        &mut self,
        what: &str,
        mut probe: impl FnMut(&LspStore, &[String]) -> Result<T, O>,
    ) -> T {
        let (cx, store) = (&self.cx, &self.store);
        let (wire, root, advance) = (&self.wire, &self.root, self.advance_clock);
        until(Duration::from_secs(30), Duration::from_millis(20), || {
            if advance {
                cx.executor().advance_clock(Duration::from_secs(1));
            }
            cx.run_until_parked();
            let lines = wire_lines(wire, root);
            cx.read(|cx| probe(store.read(cx), &lines))
        })
        .unwrap_or_else(|e| panic!("waiting for {what}: {e}"))
    }

    /// The output of a store task, running GPUI until it is done.
    fn resolve<T: 'static>(&mut self, what: &str, task: Task<T>) -> T {
        let out: Rc<RefCell<Option<T>>> = Rc::default();
        let put = out.clone();
        self.cx
            .foreground_executor()
            .spawn(async move { *put.borrow_mut() = Some(task.await) })
            .detach();
        self.wait(what, |_, _| out.borrow_mut().take().ok_or("pending"))
    }

    fn state(&self, path: &Path) -> Option<ServerState> {
        self.read(|s| s.status_for(path).map(|s| s.state))
    }

    fn wire(&self) -> Vec<String> {
        wire_lines(&self.wire, &self.root)
    }
}

impl Drop for StoreHarness {
    fn drop(&mut self) {
        self.with(|s, _| s.kill_all());
        self.cx.update(|cx| cx.quit());
        self.cx.run_until_parked();
    }
}

/// The `--wire` file as canonical JSON lines: object keys sorted, the root
/// spelled `ROOT`, `processId` 0, the client version and capabilities
/// replaced (the capabilities are compared on their own). Without `exit`:
/// the client kills the tree right after sending it, so the server may die
/// before reading it.
fn wire_lines(wire: &Path, root: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(wire).unwrap_or_default();
    let uri = path_to_uri_string(root);
    let path = root.to_string_lossy().into_owned();
    // Complete lines only (the server may be writing the last one).
    let done = text.rfind('\n').map_or("", |n| &text[..n]);
    done.lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("wire line is JSON"))
        .filter(|v| v["method"] != "exit")
        .map(|mut v| {
            if v["method"] == "initialize" {
                let p = &mut v["params"];
                p["processId"] = json!(0);
                p["clientInfo"]["version"] = json!("<version>");
                assert_eq!(
                    p["capabilities"],
                    pod_editor::lsp::server::client_capabilities()
                );
                p["capabilities"] = json!("<client capabilities>");
            }
            canonical(v, &uri, &path).to_string()
        })
        .collect()
}

fn canonical(v: Value, uri: &str, path: &str) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace(uri, "file:///ROOT").replace(path, "ROOT")),
        Value::Array(a) => a.into_iter().map(|x| canonical(x, uri, path)).collect(),
        Value::Object(m) => {
            let mut kv: Vec<(String, Value)> = m.into_iter().collect();
            kv.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                kv.into_iter()
                    .map(|(k, x)| (k, canonical(x, uri, path)))
                    .collect(),
            )
        }
        other => other,
    }
}

fn method_of(line: &str) -> String {
    let v: Value = serde_json::from_str(line).unwrap();
    v["method"].as_str().unwrap_or("(response)").to_string()
}

/// Compares line by line and shows the whole received wire on a mismatch.
fn assert_wire(got: &[String], want: &str) {
    let want: Vec<&str> = want.lines().filter(|l| !l.trim().is_empty()).collect();
    for (n, (g, w)) in got.iter().zip(&want).enumerate() {
        assert_eq!(g, w, "wire line {n}; received:\n{}", got.join("\n"));
    }
    assert_eq!(
        got.len(),
        want.len(),
        "wire length; received:\n{}",
        got.join("\n")
    );
}

const A_TEXT: &str = "fn alpha() {}\nlet s = \"😀한글\"; BAD();\nalpha();\n";

/// What the store sends a server (incremental sync, UTF-16 columns) through
/// open, edits, save, requests, `workspace/applyEdit`, close, LRU suspend and
/// resume, manual restart and shutdown, against a golden taken before the
/// store was split into modules (docs/20-refactor/libs.md L5).
#[test]
fn store_wire_matches_golden() {
    let mut h = StoreHarness::new(&["--incremental"], 1.0);
    let a = h.root.join("a.rs");
    let b = h.root.join("b.rs");
    std::fs::write(&a, A_TEXT).unwrap();
    std::fs::write(&b, "fn beta() {}\nalpha();\n").unwrap();

    // Open: check, spawn, initialize, didOpen; diagnostics in editor columns.
    h.with(|s, cx| s.open_document(&a, A_TEXT.into(), cx));
    let diags = h.wait("a.rs diagnostics", |s, _| {
        let d = s.diagnostics_for(&a).to_vec();
        if d.is_empty() { Err(d) } else { Ok(d) }
    });
    // Wire (1, 16): '😀' is two UTF-16 units; the editor counts chars.
    let range =
        |l0, c0, l1, c1| lt::Range::new(lt::Position::new(l0, c0), lt::Position::new(l1, c1));
    assert_eq!(
        diags.iter().map(|d| d.range).collect::<Vec<_>>(),
        [range(1, 15, 1, 18)]
    );
    assert_eq!(h.state(&a), Some(ServerState::Running));
    h.with(|s, cx| s.open_document(&b, "fn beta() {}\nalpha();\n".into(), cx));

    // Edits: unchanged text sends nothing; full text; then ranged changes
    // the server replays (its diagnostics land on the editor's text).
    h.with(|s, _| s.change_document(&a, A_TEXT.into()));
    let full = format!("{A_TEXT}// 끝\n");
    h.with(|s, _| s.change_document(&a, full.clone()));
    let after_emoji = full.find('한').unwrap();
    h.with(|s, _| s.edit_document(&a, after_emoji, after_emoji, "x"));
    h.with(|s, _| s.edit_document(&a, 0, 2, "pub fn"));
    let end = h.read(|s| s.doc(&a).unwrap().text.len());
    h.with(|s, _| s.edit_document(&a, end, end, "BAD\n"));
    // Out of sync (inside '😀'): rejected, nothing sent, version unchanged.
    let emoji = h.read(|s| s.doc(&a).unwrap().text.find('😀').unwrap());
    h.with(|s, _| s.edit_document(&a, emoji + 1, emoji + 2, "?"));
    let (text, version) = h.read(|s| {
        let d = s.doc(&a).unwrap();
        (d.text.clone(), d.version)
    });
    assert_eq!(
        text,
        "pub fn alpha() {}\nlet s = \"😀x한글\"; BAD();\nalpha();\n// 끝\nBAD\n"
    );
    assert_eq!(version, 4);
    h.wait("diagnostics of version 4", |s, _| {
        let got: Vec<_> = s.diagnostics_for(&a).iter().map(|d| d.range).collect();
        if got == [range(1, 16, 1, 19), range(4, 0, 4, 3)] {
            Ok(())
        } else {
            Err(got)
        }
    });
    h.with(|s, _| s.save_document(&a));

    // Requests, answered in editor columns.
    let at = lt::Position::new(2, 1);
    let t = h.with(|s, cx| s.hover(&a, at, cx));
    let hover = h.resolve("hover", t).unwrap().expect("hover");
    assert_eq!(
        hover.contents,
        lt::HoverContents::Markup(lt::MarkupContent {
            kind: lt::MarkupKind::Markdown,
            value: "fake hover: `alpha`".into()
        })
    );
    let t = h.with(|s, cx| s.definition(&a, at, cx));
    let def = h.resolve("definition", t).unwrap();
    assert_eq!(def.len(), 1);
    assert_eq!(
        (def[0].path.clone(), def[0].range),
        (a.clone(), range(0, 7, 0, 12))
    );
    let t = h.with(|s, cx| s.references(&a, at, cx));
    let refs = h.resolve("references", t).unwrap();
    let refs: Vec<_> = refs.into_iter().map(|l| (l.path, l.range)).collect();
    assert_eq!(
        refs,
        [
            (a.clone(), range(0, 7, 0, 12)),
            (a.clone(), range(2, 0, 2, 5)),
            (b.clone(), range(1, 0, 1, 5)),
        ]
    );
    let ctx = lt::CompletionContext {
        trigger_kind: lt::CompletionTriggerKind::INVOKED,
        trigger_character: None,
    };
    let t = h.with(|s, cx| s.completion(&a, lt::Position::new(2, 3), ctx, cx));
    let items = match h.resolve("completion", t).unwrap() {
        lt::CompletionResponse::List(l) => l.items,
        lt::CompletionResponse::Array(v) => v,
    };
    assert_eq!(
        items.iter().map(|i| i.label.as_str()).collect::<Vec<_>>(),
        ["alpha"]
    );
    assert_eq!(h.with(|s, _| s.take_completion(&a, 2, "alpha")), None);
    let t = h.with(|s, cx| s.resolve_additional_edits(&a, items[0].clone(), cx));
    assert!(
        h.resolve("completionItem/resolve", t).is_err(),
        "the fake has no resolve"
    );
    let t = h.with(|s, cx| s.rename(&a, at, "gamma".into(), cx));
    let edits = h.resolve("rename", t).unwrap();
    let per_file: Vec<_> = edits
        .iter()
        .map(|f| (f.path.clone(), f.edits.len()))
        .collect();
    assert_eq!(per_file, [(a.clone(), 2), (b.clone(), 1)]);
    let t = h.with(|s, cx| s.code_actions(&a, range(1, 14, 1, 17), cx));
    assert!(
        h.resolve("codeAction", t).is_err(),
        "the fake has no code actions"
    );
    let cmd = lt::Command {
        title: "header".into(),
        command: "fake.insertHeader".into(),
        arguments: Some(vec![json!(path_to_uri_string(&a))]),
    };
    let t = h.with(|s, cx| s.execute_command(&a, cmd, cx));
    h.resolve("executeCommand", t).unwrap();
    let applied: Vec<_> = h
        .events
        .borrow()
        .iter()
        .filter_map(|(e, _)| match e {
            LspStoreEvent::ApplyEdit { label, edits, .. } => Some((label.clone(), edits.clone())),
            _ => None,
        })
        .collect();
    let header = lt::TextEdit {
        range: range(0, 0, 0, 0),
        new_text: "// header\n".into(),
    };
    assert_eq!(
        applied,
        [(
            Some("fake header".to_string()),
            vec![FileEdit {
                path: a.clone(),
                edits: vec![header]
            }]
        )]
    );
    h.with(|s, _| s.close_document(&b));

    // LRU: suspend (shutdown + exit), then touching a.rs restarts it.
    let key = h.read(|s| s.running_servers());
    assert_eq!(key.len(), 1);
    let t = h.with(|s, cx| s.suspend(&key[0].key, cx));
    h.resolve("suspend: shutdown, exit, kill", t);
    assert_eq!(h.state(&a), Some(ServerState::Suspended));
    assert!(
        h.read(|s| s.diagnostics_for(&a).is_empty()),
        "suspended: no diagnostics"
    );
    h.with(|s, cx| s.touch(&a, cx));
    h.wait("resumed", |s, w| {
        let running = s.status_for(&a).map(|s| s.state) == Some(ServerState::Running);
        if running
            && w.last()
                .is_some_and(|l| method_of(l) == "textDocument/didOpen")
        {
            Ok(())
        } else {
            Err(w.len())
        }
    });

    // Manual restart: the process is killed (nothing sent), a new one opens a.rs.
    let before = h.wire().len();
    h.with(|s, cx| s.restart(&a, cx));
    h.wait("restarted", |s, w| {
        let running = s.status_for(&a).map(|s| s.state) == Some(ServerState::Running);
        if running
            && w.len() > before
            && w.last()
                .is_some_and(|l| method_of(l) == "textDocument/didOpen")
        {
            Ok(())
        } else {
            Err(w.len())
        }
    });
    assert_eq!(
        h.read(|s| s.doc(&a).unwrap().version),
        0,
        "reopened from version 0"
    );

    let t = h.with(|s, cx| s.shutdown_all(cx));
    h.resolve("shutdown_all", t);
    h.wait("stopped", |s, _| match s.status_for(&a).map(|s| s.state) {
        Some(ServerState::Stopped) => Ok(()),
        other => Err(other),
    });
    assert_wire(&h.wire(), STORE_WIRE);
}

/// A server that crashes on every `didOpen` is restarted with backoff
/// [`pod_editor::lsp::server::MAX_RESTARTS`] times (each run reopens the
/// document), then reported failed.
#[test]
fn store_restarts_a_crashing_server_then_fails() {
    // Backoff 50..800 ms of test-clock time; a run counts as stable after 3 s.
    let mut h = StoreHarness::new(&["--crash-on-open"], 0.05);
    h.advance_clock = true;
    let a = h.root.join("a.rs");
    std::fs::write(&a, A_TEXT).unwrap();
    h.with(|s, cx| s.open_document(&a, A_TEXT.into(), cx));
    let reason = h.wait("failed", |s, _| match s.status_for(&a).map(|s| s.state) {
        Some(ServerState::Failed(r)) => Ok(r),
        other => Err(other),
    });
    assert_eq!(reason, "서버가 예기치 않게 종료되었습니다");
    let wire = h.wire();
    let methods: Vec<String> = wire.iter().map(|l| method_of(l)).collect();
    let run = ["initialize", "initialized", "textDocument/didOpen"];
    let max = pod_editor::lsp::server::MAX_RESTARTS as usize;
    assert_eq!(methods, run.repeat(max + 1));
    // Every run opens the same document the same way.
    let opens: std::collections::BTreeSet<_> = wire
        .iter()
        .filter(|l| method_of(l) == "textDocument/didOpen")
        .collect();
    assert_eq!(opens.len(), 1, "{opens:?}");
    // States as the status bar saw them (consecutive repeats folded).
    let mut states: Vec<ServerState> = Vec::new();
    for s in h.events.borrow().iter().filter_map(|(_, s)| s.clone()) {
        if states.last() != Some(&s) {
            states.push(s);
        }
    }
    let mut want = vec![
        ServerState::Checking,
        ServerState::Starting,
        ServerState::Running,
    ];
    for attempt in 1..=max as u32 {
        want.push(ServerState::Restarting {
            attempt,
            reason: "서버가 예기치 않게 종료되었습니다".into(),
        });
        want.push(ServerState::Running);
    }
    want.push(ServerState::Failed(
        "서버가 예기치 않게 종료되었습니다".into(),
    ));
    assert_eq!(states, want);
}

/// Received by the fake server in `store_wire_matches_golden` (canonical
/// JSON, see [`wire_lines`]).
const STORE_WIRE: &str = r#"
{"id":1,"jsonrpc":"2.0","method":"initialize","params":{"capabilities":"<client capabilities>","clientInfo":{"name":"pod-editor","version":"<version>"},"initializationOptions":null,"locale":"ko","processId":0,"rootPath":"ROOT","rootUri":"file:///ROOT","trace":"off","workspaceFolders":[{"name":"wire 저장소","uri":"file:///ROOT"}]}}
{"jsonrpc":"2.0","method":"initialized","params":{}}
{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"languageId":"rust","text":"fn alpha() {}\nlet s = \"😀한글\"; BAD();\nalpha();\n","uri":"file:///ROOT/a.rs","version":0}}}
{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"languageId":"rust","text":"fn beta() {}\nalpha();\n","uri":"file:///ROOT/b.rs","version":0}}}
{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"contentChanges":[{"text":"fn alpha() {}\nlet s = \"😀한글\"; BAD();\nalpha();\n// 끝\n"}],"textDocument":{"uri":"file:///ROOT/a.rs","version":1}}}
{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"contentChanges":[{"range":{"end":{"character":11,"line":1},"start":{"character":11,"line":1}},"text":"x"}],"textDocument":{"uri":"file:///ROOT/a.rs","version":2}}}
{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"contentChanges":[{"range":{"end":{"character":2,"line":0},"start":{"character":0,"line":0}},"text":"pub fn"}],"textDocument":{"uri":"file:///ROOT/a.rs","version":3}}}
{"jsonrpc":"2.0","method":"textDocument/didChange","params":{"contentChanges":[{"range":{"end":{"character":0,"line":4},"start":{"character":0,"line":4}},"text":"BAD\n"}],"textDocument":{"uri":"file:///ROOT/a.rs","version":4}}}
{"jsonrpc":"2.0","method":"textDocument/didSave","params":{"textDocument":{"uri":"file:///ROOT/a.rs"}}}
{"id":2,"jsonrpc":"2.0","method":"textDocument/hover","params":{"position":{"character":1,"line":2},"textDocument":{"uri":"file:///ROOT/a.rs"}}}
{"id":3,"jsonrpc":"2.0","method":"textDocument/definition","params":{"position":{"character":1,"line":2},"textDocument":{"uri":"file:///ROOT/a.rs"}}}
{"id":4,"jsonrpc":"2.0","method":"textDocument/references","params":{"context":{"includeDeclaration":true},"position":{"character":1,"line":2},"textDocument":{"uri":"file:///ROOT/a.rs"}}}
{"id":5,"jsonrpc":"2.0","method":"textDocument/completion","params":{"context":{"triggerKind":1},"position":{"character":3,"line":2},"textDocument":{"uri":"file:///ROOT/a.rs"}}}
{"id":6,"jsonrpc":"2.0","method":"completionItem/resolve","params":{"detail":"fn alpha()","kind":3,"label":"alpha"}}
{"id":7,"jsonrpc":"2.0","method":"textDocument/rename","params":{"newName":"gamma","position":{"character":1,"line":2},"textDocument":{"uri":"file:///ROOT/a.rs"}}}
{"id":8,"jsonrpc":"2.0","method":"textDocument/codeAction","params":{"context":{"diagnostics":[{"message":"BAD is not allowed","range":{"end":{"character":20,"line":1},"start":{"character":17,"line":1}},"severity":1,"source":"fake"}],"triggerKind":1},"range":{"end":{"character":18,"line":1},"start":{"character":15,"line":1}},"textDocument":{"uri":"file:///ROOT/a.rs"}}}
{"id":9,"jsonrpc":"2.0","method":"workspace/executeCommand","params":{"arguments":["file:///ROOT/a.rs"],"command":"fake.insertHeader"}}
{"id":"fake-apply-1","jsonrpc":"2.0","result":{"applied":true}}
{"jsonrpc":"2.0","method":"textDocument/didClose","params":{"textDocument":{"uri":"file:///ROOT/b.rs"}}}
{"id":10,"jsonrpc":"2.0","method":"shutdown","params":null}
{"id":1,"jsonrpc":"2.0","method":"initialize","params":{"capabilities":"<client capabilities>","clientInfo":{"name":"pod-editor","version":"<version>"},"initializationOptions":null,"locale":"ko","processId":0,"rootPath":"ROOT","rootUri":"file:///ROOT","trace":"off","workspaceFolders":[{"name":"wire 저장소","uri":"file:///ROOT"}]}}
{"jsonrpc":"2.0","method":"initialized","params":{}}
{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"languageId":"rust","text":"pub fn alpha() {}\nlet s = \"😀x한글\"; BAD();\nalpha();\n// 끝\nBAD\n","uri":"file:///ROOT/a.rs","version":4}}}
{"id":1,"jsonrpc":"2.0","method":"initialize","params":{"capabilities":"<client capabilities>","clientInfo":{"name":"pod-editor","version":"<version>"},"initializationOptions":null,"locale":"ko","processId":0,"rootPath":"ROOT","rootUri":"file:///ROOT","trace":"off","workspaceFolders":[{"name":"wire 저장소","uri":"file:///ROOT"}]}}
{"jsonrpc":"2.0","method":"initialized","params":{}}
{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"languageId":"rust","text":"pub fn alpha() {}\nlet s = \"😀x한글\"; BAD();\nalpha();\n// 끝\nBAD\n","uri":"file:///ROOT/a.rs","version":0}}}
{"id":2,"jsonrpc":"2.0","method":"shutdown","params":null}
"#;
