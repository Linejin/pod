//! Scripted language server used by pod-editor's tests (not a product).
//!
//! Speaks UTF-16 columns (the LSP default) so the client's conversion is
//! exercised. Behaviour:
//! - diagnostics: every `BAD` in an open document is an error;
//! - hover: `fake hover: <word>`; completion: `alpha`, `beta`;
//! - definition: first `fn <word>` in any open document;
//! - references / rename: every occurrence of the word in open documents.
//!
//! Args: `--crash-on-open` exits with code 3 on the first didOpen;
//! `--child-pid-file <path>` starts a long-lived grandchild and writes its
//! pid there (process-tree tests); `--log <path>` appends one line per
//! document notification (`didOpen <uri>`, `didClose <uri>`) so tests can
//! count what the editor's tabs opened and closed.

use std::collections::BTreeMap;
use std::io::{BufReader, Write};

use pod_editor::lsp::rpc::{Message, read_message, write_message};
use serde_json::{Value, json};

fn utf16_col(line: &str, byte: usize) -> u32 {
    line[..byte].encode_utf16().count() as u32
}

fn byte_of_utf16(line: &str, col: u32) -> usize {
    let mut n = 0u32;
    for (b, c) in line.char_indices() {
        if n >= col {
            return b;
        }
        n += c.len_utf16() as u32;
    }
    line.len()
}

fn word_at(text: &str, line: u32, col: u32) -> Option<String> {
    let l = text.split('\n').nth(line as usize)?;
    let b = byte_of_utf16(l, col);
    let is_w = |c: char| c.is_alphanumeric() || c == '_';
    let start = l[..b]
        .rfind(|c: char| !is_w(c))
        .map(|i| i + l[i..].chars().next().unwrap().len_utf8())
        .unwrap_or(0);
    let end = l[b..]
        .find(|c: char| !is_w(c))
        .map(|i| b + i)
        .unwrap_or(l.len());
    (start < end).then(|| l[start..end].to_string())
}

fn occurrences(text: &str, word: &str) -> Vec<Value> {
    let mut out = Vec::new();
    for (ln, l) in text.split('\n').enumerate() {
        let mut from = 0;
        while let Some(i) = l[from..].find(word) {
            let s = from + i;
            let e = s + word.len();
            let before_ok = l[..s]
                .chars()
                .last()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            let after_ok = l[e..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
            if before_ok && after_ok {
                out.push(json!({"start":{"line":ln,"character":utf16_col(l, s)},"end":{"line":ln,"character":utf16_col(l, e)}}));
            }
            from = e;
        }
    }
    out
}

fn main() {
    let stdin = std::io::stdin();
    let mut r = BufReader::new(stdin.lock());
    let mut out = std::io::stdout();
    let mut docs: BTreeMap<String, String> = BTreeMap::new();
    let argv: Vec<String> = std::env::args().collect();
    let crash_on_open = argv.iter().any(|a| a == "--crash-on-open");
    let pid_file = argv
        .windows(2)
        .find(|w| w[0] == "--child-pid-file")
        .map(|w| w[1].clone());
    let log_file = argv
        .windows(2)
        .find(|w| w[0] == "--log")
        .map(|w| w[1].clone());
    let log = |what: &str, uri: &str| {
        if let Some(f) = &log_file
            && let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(f)
        {
            let _ = writeln!(f, "{what} {uri}");
        }
    };
    let mut _grandchild = None;
    if let Some(pid_file) = pid_file {
        let c = std::process::Command::new("cmd.exe")
            .args(["/C", "ping -n 120 127.0.0.1 >NUL"])
            .spawn()
            .expect("grandchild");
        std::fs::write(pid_file, c.id().to_string()).unwrap();
        _grandchild = Some(c);
    }
    let send = |out: &mut std::io::Stdout, m: Message| {
        write_message(out, &m.to_value()).unwrap();
        out.flush().unwrap();
    };
    while let Ok(Some(v)) = read_message(&mut r) {
        let Some(msg) = Message::parse(v) else {
            continue;
        };
        match msg {
            Message::Request { id, method, params } => {
                let pos = |p: &Value| {
                    (
                        p["textDocument"]["uri"].as_str().unwrap_or("").to_string(),
                        p["position"]["line"].as_u64().unwrap_or(0) as u32,
                        p["position"]["character"].as_u64().unwrap_or(0) as u32,
                    )
                };
                let result = match method.as_str() {
                    "initialize" => json!({
                        "capabilities": {
                            "positionEncoding": "utf-16",
                            "textDocumentSync": 1,
                            "hoverProvider": true,
                            "definitionProvider": true,
                            "referencesProvider": true,
                            "renameProvider": true,
                            "completionProvider": { "triggerCharacters": ["."] }
                        },
                        "serverInfo": { "name": "fake-lsp" }
                    }),
                    "shutdown" => Value::Null,
                    "textDocument/hover" => {
                        let (uri, l, c) = pos(&params);
                        match docs.get(&uri).and_then(|t| word_at(t, l, c)) {
                            Some(w) => {
                                json!({"contents":{"kind":"markdown","value":format!("fake hover: `{w}`")}})
                            }
                            None => Value::Null,
                        }
                    }
                    // A command that edits through the client (`workspace/applyEdit`),
                    // the way servers implement some code actions; answers with the
                    // client's reply.
                    "workspace/executeCommand" if params["command"] == "fake.insertHeader" => {
                        let uri = params["arguments"][0].as_str().unwrap_or("").to_string();
                        let edit = json!({"changes": {uri: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// header\n"}]}});
                        send(
                            &mut out,
                            Message::Request {
                                id: json!("fake-apply-1"),
                                method: "workspace/applyEdit".into(),
                                params: json!({"label": "fake header", "edit": edit}),
                            },
                        );
                        let mut answer = Value::Null;
                        while let Ok(Some(v)) = read_message(&mut r) {
                            if let Some(Message::Response { id, result }) = Message::parse(v)
                                && id == json!("fake-apply-1")
                            {
                                answer = result.unwrap_or(Value::Null);
                                break;
                            }
                        }
                        answer
                    }
                    "textDocument/completion" => {
                        json!({"isIncomplete":false,"items":[{"label":"alpha","kind":3,"detail":"fn alpha()"},{"label":"beta","kind":6}]})
                    }
                    "textDocument/definition" => {
                        let (uri, l, c) = pos(&params);
                        let w = docs.get(&uri).and_then(|t| word_at(t, l, c));
                        let mut found = Value::Null;
                        if let Some(w) = w {
                            'outer: for (u, t) in &docs {
                                for (ln, line) in t.split('\n').enumerate() {
                                    if let Some(i) = line.find(&format!("fn {w}")) {
                                        let s = i + 3;
                                        let r = json!({"start":{"line":ln,"character":utf16_col(line, s)},"end":{"line":ln,"character":utf16_col(line, s + w.len())}});
                                        found = json!([{"targetUri":u,"targetRange":r,"targetSelectionRange":r}]);
                                        break 'outer;
                                    }
                                }
                            }
                        }
                        found
                    }
                    "textDocument/references" => {
                        let (uri, l, c) = pos(&params);
                        let w = docs
                            .get(&uri)
                            .and_then(|t| word_at(t, l, c))
                            .unwrap_or_default();
                        let mut v = Vec::new();
                        for (u, t) in &docs {
                            for r in occurrences(t, &w) {
                                v.push(json!({"uri":u,"range":r}));
                            }
                        }
                        Value::Array(v)
                    }
                    "textDocument/rename" => {
                        let (uri, l, c) = pos(&params);
                        let new_name = params["newName"].as_str().unwrap_or("").to_string();
                        let w = docs
                            .get(&uri)
                            .and_then(|t| word_at(t, l, c))
                            .unwrap_or_default();
                        let mut changes = serde_json::Map::new();
                        for (u, t) in &docs {
                            let edits: Vec<Value> = occurrences(t, &w)
                                .into_iter()
                                .map(|r| json!({"range":r,"newText":new_name}))
                                .collect();
                            if !edits.is_empty() {
                                changes.insert(u.clone(), Value::Array(edits));
                            }
                        }
                        json!({"changes": changes})
                    }
                    _ => {
                        send(
                            &mut out,
                            Message::Response {
                                id,
                                result: Err(pod_editor::lsp::rpc::RpcError {
                                    code: -32601,
                                    message: method,
                                }),
                            },
                        );
                        continue;
                    }
                };
                send(
                    &mut out,
                    Message::Response {
                        id,
                        result: Ok(result),
                    },
                );
            }
            Message::Notification { method, params } => {
                let publish = |out: &mut std::io::Stdout,
                               uri: &str,
                               text: &str,
                               version: &Value| {
                    let diags: Vec<Value> = occurrences(text, "BAD").into_iter().map(|r| json!({"range":r,"severity":1,"source":"fake","message":"BAD is not allowed"})).collect();
                    send(
                        out,
                        Message::Notification {
                            method: "textDocument/publishDiagnostics".into(),
                            params: json!({"uri":uri,"version":version,"diagnostics":diags}),
                        },
                    );
                };
                match method.as_str() {
                    "exit" => std::process::exit(0),
                    "textDocument/didOpen" => {
                        if crash_on_open {
                            std::process::exit(3);
                        }
                        let uri = params["textDocument"]["uri"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        log("didOpen", &uri);
                        let text = params["textDocument"]["text"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        publish(&mut out, &uri, &text, &params["textDocument"]["version"]);
                        docs.insert(uri, text);
                    }
                    "textDocument/didChange" => {
                        let uri = params["textDocument"]["uri"]
                            .as_str()
                            .unwrap_or("")
                            .to_string();
                        if let Some(text) = params["contentChanges"]
                            .as_array()
                            .and_then(|a| a.last())
                            .and_then(|c| c["text"].as_str())
                        {
                            publish(&mut out, &uri, text, &params["textDocument"]["version"]);
                            docs.insert(uri, text.to_string());
                        }
                    }
                    "textDocument/didClose" => {
                        log(
                            "didClose",
                            params["textDocument"]["uri"].as_str().unwrap_or(""),
                        );
                        docs.remove(params["textDocument"]["uri"].as_str().unwrap_or(""));
                    }
                    _ => {}
                }
            }
            Message::Response { .. } => {}
        }
    }
}
