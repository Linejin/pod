//! Tab strip layout on a real `EditorWorkspace` (GPUI test platform, real
//! DirectWrite text): when tabs overflow the strip it scrolls so the active
//! tab is always fully visible (open, switch, Ctrl+Tab, reopen, move), the
//! open-files button appears, and same-named files get a parent-path suffix.
//! docs/13 §9.
#![recursion_limit = "512"]

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
            Some("tabs_ui"),
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
            ed.update(cx, |e, cx| {
                e.lsp().clone().update(cx, |s, cx| s.set_enabled(false, cx))
            });
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

    fn read<R>(&mut self, f: impl FnOnce(&EditorWorkspace) -> R) -> R {
        let ed = self.ed.clone();
        self.cx.update(|_, cx| f(ed.read(cx)))
    }

    /// Run tasks and draw two frames: the scroll request is applied while
    /// painting, and overflow changes re-render once after layout.
    fn settle(&mut self) {
        for _ in 0..2 {
            self.cx.run_until_parked();
            self.cx.update(|window, cx| {
                window.refresh();
                let _ = window.draw(cx);
            });
        }
        self.cx.run_until_parked();
    }

    fn active(&mut self) -> usize {
        self.read(|e| {
            e.tab_states()
                .iter()
                .position(|t| t.active)
                .expect("an active tab")
        })
    }

    /// The active tab is fully inside the strip.
    #[track_caller]
    fn assert_active_visible(&mut self, when: &str) {
        let a = self.active();
        let (seen, x) = self.read(|e| (e.tab_in_view(a), e.tab_scroll_x()));
        assert!(
            seen,
            "{when}: active tab {a} not fully visible (scroll x {x})"
        );
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

fn worktree(n: usize) -> (tempfile::TempDir, PathBuf, Vec<PathBuf>) {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("wt");
    std::fs::create_dir_all(&root).unwrap();
    let files = (0..n)
        .map(|i| {
            let p = root.join(format!("긴-파일-이름-{i:02}-overflow.txt"));
            std::fs::write(&p, format!("{i}\n")).unwrap();
            p
        })
        .collect();
    (dir, root, files)
}

#[core::prelude::v1::test]
fn active_tab_scrolls_into_view_and_overflow_button_appears() {
    let (_dir, root, files) = worktree(12);
    let mut h = Harness::new(&root);
    // Narrow window: tree 256 px + a strip that holds two or three tabs.
    h.cx.simulate_resize(size(px(760.), px(480.)));
    h.settle();

    h.with(|e, w, cx| e.open_file(&files[0], w, cx)).unwrap();
    assert!(!h.read(|e| e.tabs_overflowing()), "one tab never overflows");
    for (i, f) in files.iter().enumerate().skip(1) {
        h.with(|e, w, cx| e.open_file(f, w, cx)).unwrap();
        h.assert_active_visible(&format!("open #{i}"));
    }
    assert_eq!(h.active(), 11);
    assert!(
        h.read(|e| e.tabs_overflowing()),
        "twelve long tabs overflow: the open-files button shows"
    );
    assert!(
        h.read(|e| e.tab_scroll_x()) < -100.0,
        "the strip scrolled right to the newest tab"
    );
    assert!(
        !h.read(|e| e.tab_in_view(0)),
        "the first tab is scrolled out"
    );

    // Ctrl+PageDown wraps to the first tab: scrolled back to the start.
    h.with(|e, w, cx| e.step_tab(true, w, cx));
    assert_eq!(h.active(), 0);
    h.assert_active_visible("Ctrl+PageDown wrap");
    assert!(h.read(|e| e.tab_scroll_x()).abs() < 1.0);

    // Ctrl+Tab (MRU) back to the last tab.
    h.with(|e, w, cx| e.switch_mru(true, w, cx));
    h.with(|e, _, cx| e.end_switch(cx));
    assert_eq!(h.active(), 11);
    h.assert_active_visible("Ctrl+Tab");

    // Activating a far tab from the open-files list path (by position).
    for _ in 0..6 {
        h.with(|e, w, cx| e.step_tab(false, w, cx));
    }
    assert_eq!(h.active(), 5);
    h.assert_active_visible("Ctrl+PageUp x6");

    // Close the active tab and reopen it: back in view at its place.
    h.with(|e, w, cx| e.close_active(w, cx));
    // Go to the first tab (the strip scrolls back to the start) …
    h.with(|e, w, cx| e.open_file(&files[0], w, cx)).unwrap();
    assert!(h.read(|e| e.tab_scroll_x()).abs() < 1.0);
    // … then Ctrl+Shift+T: tab 5 is back at its place and scrolled to.
    assert!(h.with(|e, w, cx| e.reopen_closed(w, cx)));
    assert_eq!(h.active(), 5);
    h.assert_active_visible("Ctrl+Shift+T");

    // Moving the active tab to the far end keeps it in view.
    for _ in 0..8 {
        h.with(|e, _, cx| e.move_active_tab(true, cx));
    }
    assert_eq!(h.active(), 11);
    h.assert_active_visible("Ctrl+Shift+PageDown to the end");
    for _ in 0..11 {
        h.with(|e, _, cx| e.move_active_tab(false, cx));
    }
    assert_eq!(h.active(), 0);
    h.assert_active_visible("Ctrl+Shift+PageUp to the start");

    // A wide window: no overflow, no button.
    h.cx.simulate_resize(size(px(6000.), px(480.)));
    h.settle();
    assert!(
        !h.read(|e| e.tabs_overflowing()),
        "everything fits: the button hides"
    );
    // The open-files list lists every tab and activates one.
    h.with(|e, w, cx| e.open_tab_list(w, cx));
    assert_eq!(h.read(|e| e.tab_menu_items().len()), 12);
}

#[core::prelude::v1::test]
fn same_name_tabs_show_the_distinguishing_parent() {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("wt");
    for d in ["crates/podd/src", "crates/pod-app/src"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
        std::fs::write(root.join(d).join("mod.rs"), "\n").unwrap();
    }
    std::fs::write(root.join("crates/podd/src/lib.rs"), "\n").unwrap();
    let mut h = Harness::new(&root);
    for f in [
        "crates/podd/src/mod.rs",
        "crates/pod-app/src/mod.rs",
        "crates/podd/src/lib.rs",
    ] {
        h.with(|e, w, cx| e.open_file(&root.join(f), w, cx))
            .unwrap();
    }
    assert_eq!(
        h.read(|e| e.tab_titles()),
        ["mod.rs · podd/src", "mod.rs · pod-app/src", "lib.rs"]
    );
    // Closing one of the pair drops the suffix of the other.
    h.with(|e, w, cx| e.close_path(&root.join("crates/pod-app/src/mod.rs"), false, w, cx));
    assert_eq!(h.read(|e| e.tab_titles()), ["mod.rs", "lib.rs"]);
}

/// The layout JSON a real workspace saves (tab order with `/` paths, cursor,
/// preview tab, active tab, Ctrl+Tab order). A new tab opens right after the
/// active one. Frozen from the unsplit
/// `workspace.rs` before card L4; the split keeps these exact bytes.
const SESSION_GOLDEN: &str = r#"{
  "tabs": [
    {
      "path": "src/main.rs",
      "line": 2,
      "col": 5
    },
    {
      "path": "docs/한글 노트.md",
      "line": 1,
      "col": 1,
      "preview": true
    },
    {
      "path": "src/lib.rs",
      "line": 1,
      "col": 8
    }
  ],
  "active": 1,
  "mru": [
    1,
    0,
    2
  ]
}"#;

/// Saving and restoring the editor layout on real workspaces: the saved
/// JSON is the golden above, and a restore that skips a missing file shifts
/// the active tab and the Ctrl+Tab order onto the files that remain.
#[core::prelude::v1::test]
fn session_round_trips_through_a_real_workspace() {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("wt");
    for (f, body) in [
        ("src/main.rs", "fn main() {\n    run();\n}\n"),
        ("src/lib.rs", "pub fn run() {}\n"),
        ("docs/한글 노트.md", "# 제목\n\n본문\n"),
    ] {
        let p = root.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    let saved = {
        let mut h = Harness::new(&root);
        h.with(|e, w, cx| e.open_file(&root.join("src/main.rs"), w, cx))
            .unwrap();
        h.with(|e, w, cx| e.goto(&root.join("src/lib.rs"), 1, 8, w, cx))
            .unwrap();
        // A root-relative path names the same tab.
        h.with(|e, w, cx| e.goto(Path::new("src/main.rs"), 2, 5, w, cx))
            .unwrap();
        h.with(|e, w, cx| e.preview_file(&root.join("docs/한글 노트.md"), w, cx))
            .unwrap();
        let s = h.with(|e, _, cx| e.session(cx));
        assert_eq!(serde_json::to_string_pretty(&s).unwrap(), SESSION_GOLDEN);
        s
    };

    // The same layout plus a file deleted since it was saved.
    let mut with_gone = saved.clone();
    with_gone.tabs.insert(
        1,
        pod_editor::TabSession {
            path: "src/gone.rs".into(),
            line: 9,
            col: 9,
            preview: false,
        },
    );
    with_gone.active = Some(2);
    with_gone.mru = vec![2, 1, 0, 3];

    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.restore_session(&with_gone, w, cx));
    assert_eq!(h.with(|e, _, cx| e.session(cx)), saved);
    let states: Vec<(String, bool, bool)> = h.read(|e| {
        e.tab_states()
            .into_iter()
            .map(|t| {
                let rel = t.path.strip_prefix(e.root()).unwrap().to_owned();
                (
                    rel.to_string_lossy().replace('\\', "/"),
                    t.active,
                    t.preview,
                )
            })
            .collect()
    });
    assert_eq!(
        states,
        [
            ("src/main.rs".to_string(), false, false),
            ("docs/한글 노트.md".to_string(), true, true),
            ("src/lib.rs".to_string(), false, false),
        ]
    );
    assert_eq!(
        h.read(|e| e.tab_titles()),
        ["main.rs", "한글 노트.md", "lib.rs"]
    );
    // Ctrl+Tab follows the restored order: the previous file is main.rs.
    h.with(|e, w, cx| e.switch_mru(true, w, cx));
    h.with(|e, _, cx| e.end_switch(cx));
    let s = h.with(|e, _, cx| e.session(cx));
    assert_eq!((s.active, s.mru), (Some(0), vec![0, 1, 2]));
}

/// The 8.3 short spelling of an existing path (`cmd`'s `%~s`), `None` when
/// the volume has no short names.
fn short_spelling(p: &Path) -> Option<PathBuf> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("cmd")
        .raw_arg(format!(
            "/d /c for %I in (\"{}\") do @echo %~sI",
            p.display()
        ))
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && s.contains('~')).then(|| PathBuf::from(s))
}

/// A host (task removal, a server's file edit) names an open file through
/// an 8.3 short name of the root (GitHub runners' `RUNNER~1`): it is the
/// same tab, so it closes, and the remaining tab titles see one spelling.
#[core::prelude::v1::test]
fn host_close_accepts_a_short_spelling_of_the_root() {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("LongWorktreeName");
    std::fs::create_dir_all(root.join("src")).unwrap();
    for f in ["src/a.rs", "src/b.rs"] {
        std::fs::write(root.join(f), "\n").unwrap();
    }
    let Some(short) = short_spelling(&root) else {
        eprintln!("skipped: no 8.3 short names on this volume");
        return;
    };
    let mut h = Harness::new(&root);
    for f in ["src/a.rs", "src/b.rs"] {
        h.with(|e, w, cx| e.open_file(&root.join(f), w, cx))
            .unwrap();
    }
    h.with(|e, w, cx| e.close_path(&short.join("src").join("a.rs"), false, w, cx));
    assert_eq!(h.read(|e| e.tab_titles()), ["b.rs"]);
    // A file deleted meanwhile still maps through its existing folder.
    std::fs::remove_file(root.join("src/b.rs")).unwrap();
    h.with(|e, w, cx| e.close_path(&short.join("src").join("b.rs"), true, w, cx));
    assert!(h.read(|e| e.tab_titles()).is_empty());
}
