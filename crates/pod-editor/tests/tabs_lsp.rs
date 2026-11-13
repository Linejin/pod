//! Editor tabs ↔ language-server documents on a real `EditorWorkspace`
//! (GPUI test platform) with the fake server (`pod-editor-fake-lsp --log`):
//! every tab that reaches the server sends exactly one `didOpen`, every tab
//! that leaves (closed, or a preview replaced) exactly one `didClose`.
//! docs/13 §9.
#![recursion_limit = "512"]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use ::gpui::{TestAppContext, TestDispatcher, VisualTestContext};
use gpui_kit::*;
use pod_editor::EditorWorkspace;
use pod_test_support::wait::until;

thread_local! {
    static TEXT_SYSTEM: Arc<dyn PlatformTextSystem> = gpui_kit::platform::current_platform(false).text_system();
}

struct Harness {
    app: TestAppContext,
    cx: VisualTestContext,
    ed: Entity<EditorWorkspace>,
}

impl Harness {
    fn new(root: &Path) -> Self {
        let dispatcher = TestDispatcher::new(0);
        let app = TestAppContext::build_with_text_system(
            dispatcher,
            Some("tabs_lsp"),
            TEXT_SYSTEM.with(Arc::clone),
        );
        app.update(|cx| {
            gpui_kit::init(cx);
            pod_ui::apply_theme(pod_ui::Mode::Dark, true, cx);
            pod_editor::init(cx);
        });
        let slot = RefCell::new(None);
        let mut app = app;
        let root = root.to_path_buf();
        let handle = app.add_window(|window, cx| {
            let ed = cx.new(|cx| EditorWorkspace::new(root, pod_ui::Mode::Dark, window, cx));
            *slot.borrow_mut() = Some(ed.clone());
            gpui_kit::base::Root::new(ed, window, cx)
        });
        let ed = slot.into_inner().expect("editor built");
        let mut cx = VisualTestContext::from_window(handle.into(), &app);
        cx.update(|window, _| window.activate_window());
        Harness { app, cx, ed }
    }

    fn with<R>(
        &mut self,
        f: impl FnOnce(&mut EditorWorkspace, &mut Window, &mut Context<EditorWorkspace>) -> R,
    ) -> R {
        let ed = self.ed.clone();
        let r = self
            .cx
            .update(|window, cx| ed.update(cx, |e, cx| f(e, window, cx)));
        self.settle();
        r
    }

    /// Run tasks and draw a frame (focus-in listeners fire on draw).
    fn settle(&mut self) {
        self.cx.run_until_parked();
        self.cx.update(|window, cx| {
            window.refresh();
            let _ = window.draw(cx);
        });
        self.cx.run_until_parked();
    }

    /// Runs the scheduler until `probe` holds (the server answers on its own
    /// threads): every poll pumps GPUI first. A timeout shows the last probe.
    fn wait<O: std::fmt::Debug>(&mut self, what: &str, mut probe: impl FnMut() -> Result<(), O>) {
        let cx = &mut self.cx;
        let ready = until(Duration::from_secs(30), Duration::from_millis(20), || {
            cx.run_until_parked();
            probe()
        });
        if let Err(e) = ready {
            panic!("waiting for {what}: {e}");
        }
    }

    fn rename(&mut self, path: &Path, name: &str) {
        self.with(|e, w, cx| e.start_rename_entry(path.to_path_buf(), w, cx));
        self.cx.simulate_input(name);
        self.cx.simulate_keystrokes("enter");
        self.settle();
        assert!(!self.with(|e, _, _| e.has_overlay()), "rename must succeed");
    }

    fn type_text(&mut self, text: &str) {
        self.with(|e, w, cx| {
            e.active_editor().unwrap().update(cx, |s, cx| {
                s.set_selected_range(0..s.text().len(), cx);
                s.replace(text.to_owned(), w, cx);
            });
        });
    }

    fn tabs(&mut self) -> Vec<(String, bool)> {
        let ed = self.ed.clone();
        self.cx.update(|_, cx| {
            ed.read(cx)
                .tab_states()
                .into_iter()
                .map(|t| {
                    (
                        t.path.file_name().unwrap().to_string_lossy().into_owned(),
                        t.preview,
                    )
                })
                .collect()
        })
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let ed = self.ed.clone();
        self.cx
            .update(|_, cx| ed.update(cx, |e, cx| e.shutdown(cx)))
            .detach();
        self.cx.run_until_parked();
        self.app.update(|cx| cx.quit());
        self.app.run_until_parked();
    }
}

/// `(didOpen, didClose)` counts per file name from the fake server's log.
fn counts(log: &Path, name: &str) -> (usize, usize) {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let of = |what: &str| {
        text.lines()
            .filter(|l| l.starts_with(what) && l.ends_with(name))
            .count()
    };
    (of("didOpen "), of("didClose "))
}

/// `(didOpen, didClose)` counts by file name.
type Counts<'a> = Vec<(&'a str, (usize, usize))>;

/// `Ok` when each file's `(didOpen, didClose)` counts are as given,
/// otherwise the counts the log shows.
fn logged<'a>(log: &Path, want: &[(&'a str, (usize, usize))]) -> Result<(), Counts<'a>> {
    let seen: Vec<_> = want.iter().map(|&(n, _)| (n, counts(log, n))).collect();
    if seen.as_slice() == want {
        Ok(())
    } else {
        Err(seen)
    }
}

fn worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("wt");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join(".pod")).unwrap();
    for n in ["a", "b", "c"] {
        std::fs::write(
            root.join("src").join(format!("{n}.rs")),
            format!("fn {n}() {{}}\n"),
        )
        .unwrap();
    }
    let log = dir.path().join("lsp.log");
    let fake = env!("CARGO_BIN_EXE_pod-editor-fake-lsp").replace('\\', "/");
    let cfg = serde_json::json!({"servers": [{
        "id": "rust-analyzer", "languages": ["rust"], "command": fake,
        "args": ["--log", log.to_string_lossy().replace('\\', "/"),
            "--wire", log.with_extension("wire").to_string_lossy().replace('\\', "/"),
            "--incremental"], "root_strategy": "nearest"
    }]});
    std::fs::write(root.join(".pod").join("lsp.json"), cfg.to_string()).unwrap();
    // A repository's lsp.json runs only once its content is approved
    // (editor-01): approve it in a test-only user config folder.
    let user = user_config_dir();
    let canon = pod_editor::uri::simplify(&root.canonicalize().unwrap());
    let (_, _, repo) = pod_editor::lsp::registry::ServerRegistry::load_with(&canon, Some(&user));
    // Tests run in parallel and share the approvals file: one write at a time.
    static TRUST: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = TRUST.lock().unwrap_or_else(|e| e.into_inner());
    repo.expect("repo lsp.json").trust(&user).unwrap();
    (dir, root, log)
}

/// One user config folder for this test process (never the real
/// `%APPDATA%\Pod`), set before any editor reads it.
fn user_config_dir() -> PathBuf {
    static DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let d = std::env::temp_dir().join(format!("pod-editor-tabs-lsp-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        // SAFETY: set once, before any thread of this process reads it.
        unsafe { std::env::set_var("POD_EDITOR_CONFIG_DIR", &d) };
        d
    })
    .clone()
}

#[core::prelude::v1::test]
fn tabs_open_and_close_server_documents_exactly_once() {
    let (_dir, root, log) = worktree();
    let mut h = Harness::new(&root);
    let src = root.join("src");

    // Single click / quick open: preview tab, and the server starts.
    h.with(|e, w, cx| e.preview_file(&src.join("a.rs"), w, cx))
        .unwrap();
    h.wait("didOpen a.rs", || logged(&log, &[("a.rs", (1, 0))]));
    assert_eq!(h.tabs(), [("a.rs".to_string(), true)]);

    // Another preview replaces it in place: didClose a, didOpen b.
    h.with(|e, w, cx| e.preview_file(&src.join("b.rs"), w, cx))
        .unwrap();
    h.wait("preview replaced", || {
        logged(&log, &[("a.rs", (1, 1)), ("b.rs", (1, 0))])
    });
    assert_eq!(h.tabs(), [("b.rs".to_string(), true)]);

    // Explicit open: pinned, after the active tab. Pinning b sends nothing.
    h.with(|e, w, cx| e.open_file(&src.join("c.rs"), w, cx))
        .unwrap();
    h.with(|e, w, cx| e.open_file(&src.join("b.rs"), w, cx))
        .unwrap();
    h.wait("didOpen c.rs", || logged(&log, &[("c.rs", (1, 0))]));
    assert_eq!(
        h.tabs(),
        [("b.rs".to_string(), false), ("c.rs".to_string(), false)]
    );

    // a.rs again as preview (after b), closed with Ctrl+W, reopened with Ctrl+Shift+T.
    h.with(|e, w, cx| e.preview_file(&src.join("a.rs"), w, cx))
        .unwrap();
    assert_eq!(
        h.tabs(),
        [
            ("b.rs".to_string(), false),
            ("a.rs".to_string(), true),
            ("c.rs".to_string(), false)
        ]
    );
    h.with(|e, w, cx| e.close_active(w, cx));
    h.wait("didClose a.rs (2)", || logged(&log, &[("a.rs", (2, 2))]));
    assert!(h.with(|e, w, cx| e.reopen_closed(w, cx)));
    h.wait("didOpen a.rs (3)", || logged(&log, &[("a.rs", (3, 2))]));
    assert_eq!(
        h.tabs(),
        [
            ("b.rs".to_string(), false),
            ("a.rs".to_string(), false),
            ("c.rs".to_string(), false)
        ],
        "reopened pinned, at its old place"
    );

    // Host close (task removal path): didClose c.
    h.with(|e, w, cx| e.close_path(&src.join("c.rs"), true, w, cx));
    h.wait("didClose c.rs", || logged(&log, &[("c.rs", (1, 1))]));
    assert!(
        h.with(|e, _, _| e.closed_paths().iter().all(|p| !p.ends_with("c.rs"))),
        "host closes are not offered by Ctrl+Shift+T"
    );

    // Balance: open tabs == documents the server holds open; the store agrees.
    let open_on_server: usize = ["a.rs", "b.rs", "c.rs"]
        .iter()
        .map(|n| counts(&log, n))
        .map(|(o, c)| o - c)
        .sum();
    assert_eq!(open_on_server, h.tabs().len());
    let ed = h.ed.clone();
    // Documents are keyed by the buffers' spelling: the canonical root (TEMP
    // may be an 8.3 short name such as RUNNER~1).
    let canon_src = pod_editor::uri::simplify(&src.canonicalize().unwrap());
    let store_docs = h.cx.update(|_, cx| {
        let store = ed.read(cx).lsp().read(cx);
        ["a.rs", "b.rs", "c.rs"]
            .iter()
            .filter(|n| store.doc(&canon_src.join(n)).is_some())
            .count()
    });
    assert_eq!(store_docs, 2);
    // Nothing else was opened twice without a close in between.
    let text = std::fs::read_to_string(&log).unwrap();
    let mut open = std::collections::BTreeSet::new();
    for l in text.lines() {
        let (what, uri) = l.split_once(' ').unwrap();
        match what {
            "didOpen" => assert!(open.insert(uri.to_string()), "double didOpen {uri}\n{text}"),
            "didClose" => assert!(open.remove(uri), "didClose without didOpen {uri}\n{text}"),
            _ => {}
        }
    }
}

#[core::prelude::v1::test]
fn restored_background_tabs_open_on_the_server_only_when_shown() {
    let (_dir, root, log) = worktree();
    let mut h = Harness::new(&root);
    let session = pod_editor::EditorSession {
        tabs: ["a", "b", "c"]
            .iter()
            .map(|n| pod_editor::TabSession {
                path: format!("src/{n}.rs"),
                line: 1,
                col: 1,
                preview: *n == "c",
            })
            .collect(),
        active: Some(1),
        mru: vec![1, 2, 0],
    };
    h.with(|e, w, cx| e.restore_session(&session, w, cx));
    assert_eq!(
        h.tabs(),
        [
            ("a.rs".to_string(), false),
            ("b.rs".to_string(), false),
            ("c.rs".to_string(), true)
        ]
    );
    // No server until the view is used.
    std::thread::sleep(Duration::from_millis(300));
    h.cx.run_until_parked();
    assert!(
        !log.exists() || std::fs::read_to_string(&log).unwrap().is_empty(),
        "restore starts no server"
    );
    // Ctrl+Tab goes to the saved previous file (c), then the active one opens.
    h.with(|e, w, cx| {
        e.focus_handle(cx).focus(w, cx);
    });
    h.with(|e, w, cx| e.switch_mru(true, w, cx));
    h.with(|e, _, cx| e.end_switch(cx));
    let active = h.with(|e, _, _| {
        e.active_path()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
    });
    assert_eq!(active.as_deref(), Some("c.rs"), "saved MRU order: b, c, a");
    h.wait("didOpen c.rs", || logged(&log, &[("c.rs", (1, 0))]));
    assert_eq!(counts(&log, "a.rs"), (0, 0), "a was never shown");
}

fn wire_messages(log: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log.with_extension("wire"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// B01: providers attached to the editor (not a freshly constructed test
/// bridge) and incremental edits must all use the new document URI.
#[core::prelude::v1::test]
fn renamed_buffers_route_edits_and_providers_to_new_uri() {
    let (_dir, root, log) = worktree();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&root.join("src/a.rs"), w, cx))
        .unwrap();
    h.wait("initial document", || logged(&log, &[("a.rs", (1, 0))]));
    let mut old = h.with(|e, _, _| e.active_path().unwrap().to_path_buf());
    for directory in [false, true] {
        let new = if directory {
            let parent = old.parent().unwrap();
            h.rename(parent, "moved");
            parent.parent().unwrap().join("moved/renamed.rs")
        } else {
            h.rename(&old, "renamed.rs");
            old.with_file_name("renamed.rs")
        };
        let uri = pod_editor::uri::path_to_uri(&new).to_string();
        let old_uri = pod_editor::uri::path_to_uri(&old).to_string();
        h.wait("new didOpen", || {
            let seen = wire_messages(&log);
            if seen.iter().any(|v| {
                v["method"] == "textDocument/didOpen" && v["params"]["textDocument"]["uri"] == uri
            }) {
                Ok(())
            } else {
                Err(seen)
            }
        });
        h.type_text(if directory {
            "fn second() {}\n"
        } else {
            "fn first() {}\n"
        });
        let text = if directory {
            "fn second() {}\n"
        } else {
            "fn first() {}\n"
        };
        // No provider flush or debounce is needed to repair incremental sync.
        let (old_open, synced) = h.with(|e, _, cx| {
            let store = e.lsp().read(cx);
            (
                store.doc(&old).is_some(),
                store.doc(&new).map(|d| d.text.clone()),
            )
        });
        assert!(!old_open);
        assert_eq!(synced.as_deref(), Some(text));
        h.wait("ranged edit on new URI", || {
            let seen = wire_messages(&log);
            if seen.iter().any(|v| {
                v["method"] == "textDocument/didChange"
                    && v["params"]["textDocument"]["uri"] == uri
                    && v["params"]["contentChanges"][0]["range"].is_object()
            }) {
                Ok(())
            } else {
                Err(seen)
            }
        });
        // Exercise each installed provider; successful wire requests prove
        // the path wasn't left pointing at the closed document.
        h.with(|e, w, cx| {
            let state = e.active_editor().unwrap();
            let s = state.read(cx);
            let rope = s.text().clone();
            let hover = s.lsp().hover_provider.clone().unwrap();
            let completion = s.lsp().completion_provider.clone().unwrap();
            let definition = s.lsp().definition_provider.clone().unwrap();
            let actions = s.lsp().code_action_providers[0].clone();
            hover.hover(&rope, 4, w, cx).detach();
            completion
                .completions(
                    &rope,
                    4,
                    lsp_types::CompletionContext {
                        trigger_kind: lsp_types::CompletionTriggerKind::INVOKED,
                        trigger_character: None,
                    },
                    w,
                    cx,
                )
                .detach();
            definition.definitions(&rope, 4, w, cx).detach();
            actions.code_actions(state, 0..2, w, cx).detach();
        });
        h.wait("provider requests on new URI", || {
            let seen = wire_messages(&log);
            if ["hover", "completion", "definition", "codeAction"]
                .iter()
                .all(|method| {
                    seen.iter().any(|v| {
                        v["method"] == format!("textDocument/{method}")
                            && v["params"]["textDocument"]["uri"] == uri
                    })
                })
            {
                Ok(())
            } else {
                Err(seen)
            }
        });
        let seen = wire_messages(&log);
        let closed = seen
            .iter()
            .position(|v| {
                v["method"] == "textDocument/didClose"
                    && v["params"]["textDocument"]["uri"] == old_uri
            })
            .unwrap();
        assert!(
            seen[closed + 1..]
                .iter()
                .all(|v| v["params"]["textDocument"]["uri"] != old_uri),
            "stale URI after close: {seen:?}"
        );
        old = new;
    }
}

/// Extension changes reconfigure the existing editor, including plain text
/// gaining LSP and a recognized language losing it again.
#[core::prelude::v1::test]
fn rename_extension_rebinds_language_without_replacing_editor() {
    let (_dir, root, log) = worktree();
    let file = root.join("src/plain.txt");
    std::fs::write(&file, "fn renamed() {}\n").unwrap();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&file, w, cx)).unwrap();
    let state = h.with(|e, _, _| e.active_editor().unwrap());
    let mut path = h.with(|e, _, _| e.active_path().unwrap().to_path_buf());
    for (name, language) in [
        ("plain.rs", Some("rust")),
        ("plain.json", Some("json")),
        ("plain.txt", None),
    ] {
        h.rename(&path, name);
        let old = path;
        path = old.with_file_name(name);
        let (editor, text, highlight, providers, old_open, doc_language) = h.with(|e, _, cx| {
            let editor = e.active_editor().unwrap();
            let s = editor.read(cx);
            let lsp = s.lsp();
            let store = e.lsp().read(cx);
            (
                editor.clone(),
                s.value().to_string(),
                s.language_name(),
                [
                    lsp.hover_provider.is_some(),
                    lsp.completion_provider.is_some(),
                    lsp.definition_provider.is_some(),
                    !lsp.code_action_providers.is_empty(),
                ],
                store.doc(&old).is_some(),
                store.doc(&path).map(|d| d.language.lsp_id),
            )
        });
        assert_eq!(editor, state);
        assert_eq!(text, "fn renamed() {}\n");
        assert_eq!(highlight.as_ref(), language.unwrap_or("plaintext"));
        assert_eq!(providers, [language.is_some(); 4]);
        assert!(!old_open);
        assert_eq!(doc_language, language);
        if language == Some("rust") {
            h.wait("extension starts rust server", || {
                logged(&log, &[("plain.rs", (1, 0))])
            });
        }
    }
    h.wait("extension closes rust document", || {
        logged(&log, &[("plain.rs", (1, 1))])
    });
    h.type_text("still editable\n");
    assert_eq!(h.with(|e, _, _| e.dirty_paths()), vec![path]);
}

#[core::prelude::v1::test]
fn rename_cancels_pending_sync_before_old_path_is_reused() {
    let (_dir, root, log) = worktree();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&root.join("src/a.rs"), w, cx))
        .unwrap();
    h.wait("initial document", || logged(&log, &[("a.rs", (1, 0))]));
    let old = h.with(|e, _, _| e.active_path().unwrap().to_path_buf());
    h.type_text("fn unsaved() {}\n");
    h.rename(&old, "renamed.rs");
    // Reopen the old name before the queued full-text sync runs, without
    // another edit of the renamed buffer that would replace its sync task.
    std::fs::write(&old, "fn replacement() {}\n").unwrap();
    h.with(|e, w, cx| e.open_file(&old, w, cx)).unwrap();
    h.app.executor().advance_clock(Duration::from_millis(200));
    h.settle();
    let (replacement, renamed) = h.with(|e, _, cx| {
        let store = e.lsp().read(cx);
        (
            store.doc(&old).map(|d| d.text.clone()),
            store
                .doc(&old.with_file_name("renamed.rs"))
                .map(|d| d.text.clone()),
        )
    });
    assert_eq!(replacement.as_deref(), Some("fn replacement() {}\n"));
    assert_eq!(renamed.as_deref(), Some("fn unsaved() {}\n"));
}
