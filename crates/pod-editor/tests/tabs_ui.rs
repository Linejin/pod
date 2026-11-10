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
