//! Editor tabs ↔ language-server documents on a real `EditorWorkspace`
//! (GPUI test platform) with the fake server (`pod-editor-fake-lsp --log`):
//! every tab that reaches the server sends exactly one `didOpen`, every tab
//! that leaves (closed, or a preview replaced) exactly one `didClose`.
//! docs/13 §9.
#![recursion_limit = "512"]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ::gpui::{TestAppContext, TestDispatcher, VisualTestContext};
use gpui_kit::*;
use pod_editor::EditorWorkspace;

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

    /// Runs the scheduler until `done` holds (the server answers on its own threads).
    fn wait(&mut self, what: &str, mut done: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(30);
        while Instant::now() < end {
            self.cx.run_until_parked();
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {what}");
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
        "args": ["--log", log.to_string_lossy().replace('\\', "/")], "root_strategy": "nearest"
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
    h.wait("didOpen a.rs", || counts(&log, "a.rs") == (1, 0));
    assert_eq!(h.tabs(), [("a.rs".to_string(), true)]);

    // Another preview replaces it in place: didClose a, didOpen b.
    h.with(|e, w, cx| e.preview_file(&src.join("b.rs"), w, cx))
        .unwrap();
    h.wait("preview replaced", || {
        counts(&log, "a.rs") == (1, 1) && counts(&log, "b.rs") == (1, 0)
    });
    assert_eq!(h.tabs(), [("b.rs".to_string(), true)]);

    // Explicit open: pinned, after the active tab. Pinning b sends nothing.
    h.with(|e, w, cx| e.open_file(&src.join("c.rs"), w, cx))
        .unwrap();
    h.with(|e, w, cx| e.open_file(&src.join("b.rs"), w, cx))
        .unwrap();
    h.wait("didOpen c.rs", || counts(&log, "c.rs") == (1, 0));
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
    h.wait("didClose a.rs (2)", || counts(&log, "a.rs") == (2, 2));
    assert!(h.with(|e, w, cx| e.reopen_closed(w, cx)));
    h.wait("didOpen a.rs (3)", || counts(&log, "a.rs") == (3, 2));
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
    h.wait("didClose c.rs", || counts(&log, "c.rs") == (1, 1));
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
    let store_docs = h.cx.update(|_, cx| {
        let store = ed.read(cx).lsp().read(cx);
        ["a.rs", "b.rs", "c.rs"]
            .iter()
            .filter(|n| store.doc(&src.join(n)).is_some())
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
    h.wait("didOpen c.rs", || counts(&log, "c.rs") == (1, 0));
    assert_eq!(counts(&log, "a.rs"), (0, 0), "a was never shown");
}
