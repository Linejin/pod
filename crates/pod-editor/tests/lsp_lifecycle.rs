//! LSP process lifecycle against the scripted fake server
//! (`pod-editor-fake-lsp`) and, when installed, a real rust-analyzer.

#[path = "../../../tools/test-support/require_tools.rs"]
mod require_tools;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use futures::channel::mpsc::{UnboundedReceiver, unbounded};
use lsp_types as lt;
use pod_editor::lsp::ServerConfig;
use pod_editor::lsp::registry::{Availability, ServerRegistry, check_available, which};
use pod_editor::lsp::rpc::SERVER_GONE;
use pod_editor::lsp::server::{ServerEvent, ServerProcess, initialize};
use pod_editor::text::{self, PosEncoding};
use pod_editor::uri::{path_to_uri_string, uri_str_to_path};
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
    let t = Instant::now();
    while !pid_file.exists() && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(200));
    let pid: u32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!(pid_alive(pid), "grandchild started");
    assert!(p.tree.active_processes() >= 2);
    drop(p);
    let t = Instant::now();
    while pid_alive(pid) && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(!pid_alive(pid), "grandchild {pid} must die with the job");
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
            require_tools::missing(&format!("rust-analyzer ({m})"));
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
