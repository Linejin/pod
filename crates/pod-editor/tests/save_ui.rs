//! Saving on a real `EditorWorkspace` (GPUI test platform): a save never
//! overwrites a disk change the buffer has not seen (editor-06), and a file
//! that is not UTF-8 opens read-only and is never saved lossy, until it is
//! reopened in an encoding that reads it without loss (editor-02).
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
            Some("save_ui"),
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
        let cx = VisualTestContext::from_window(handle.into(), &app);
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
        self.cx.run_until_parked();
        r
    }

    fn read<R>(&mut self, f: impl FnOnce(&EditorWorkspace) -> R) -> R {
        let ed = self.ed.clone();
        self.cx.update(|_, cx| f(ed.read(cx)))
    }

    /// Replaces the active buffer's text as typing would.
    fn type_text(&mut self, text: &str) {
        let text = text.to_string();
        self.with(|e, w, cx| {
            let st = e.active_editor().expect("active editor");
            st.update(cx, |s, cx| {
                let len = s.text().len();
                s.set_selected_range(0..len, cx);
                s.replace(text, w, cx);
            });
        });
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

fn worktree() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp");
    let root = dir.path().join("wt");
    std::fs::create_dir_all(&root).unwrap();
    (dir, root)
}

/// editor-06: an agent writes the file after it was opened and before the
/// watcher said so; Ctrl+S keeps the agent's version, shows the diff, and
/// overwrites only when asked.
#[core::prelude::v1::test]
fn save_does_not_overwrite_an_unseen_disk_change() {
    let (_dir, root) = worktree();
    let file = root.join("a.txt");
    std::fs::write(&file, "one\r\ntwo\r\n").unwrap();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&file, w, cx)).unwrap();
    h.type_text("one\nmine\n");
    std::fs::write(&file, "one\r\nagent\r\n").unwrap();
    h.with(|e, w, cx| e.save_active(w, cx));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "one\r\nagent\r\n",
        "the agent's write survives"
    );
    let tab = &h.read(|e| e.tab_states())[0];
    assert!(tab.dirty && tab.disk_changed, "{tab:?}");

    h.with(|e, w, cx| e.show_disk_diff(w, cx));
    let lines = h.read(|e| e.disk_diff()).expect("diff open");
    assert!(
        lines.contains(&('-', "agent".into())) && lines.contains(&('+', "mine".into())),
        "{lines:?}"
    );

    // "덮어써서 저장" (or "내 변경 유지" + save when the watcher came first).
    h.with(|e, w, cx| e.answer_disk_prompt(false, w, cx));
    h.with(|e, w, cx| e.save_active(w, cx));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "one\r\nmine\r\n",
        "explicit overwrite, CRLF kept"
    );
    let tab = &h.read(|e| e.tab_states())[0];
    assert!(!tab.dirty && !tab.disk_changed, "{tab:?}");

    // Our own save is not a conflict: edit and save again.
    h.type_text("one\nmine\nthree\n");
    h.with(|e, w, cx| e.save_active(w, cx));
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "one\r\nmine\r\nthree\r\n"
    );
}

/// editor-02: a CP949 file opens read-only (never saved with U+FFFD), and
/// reopened as EUC-KR it is edited and saved in CP949 byte for byte.
#[core::prelude::v1::test]
fn non_utf8_file_is_read_only_until_reopened_in_its_encoding() {
    let (_dir, root) = worktree();
    let file = root.join("legacy.txt");
    let euc_kr = pod_editor::text::REOPEN_ENCODINGS[0].1;
    let (bytes, _, _) = euc_kr.encode("한글 설명\n");
    std::fs::write(&file, &bytes).unwrap();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&file, w, cx)).unwrap();
    let tab = &h.read(|e| e.tab_states())[0];
    assert!(tab.read_only && tab.encoding == "UTF-8", "{tab:?}");
    h.with(|e, w, cx| e.save_active(w, cx));
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes.as_ref(),
        "never written lossy"
    );

    // A rename/code action cannot edit it either.
    let edit = pod_editor::lsp::store::FileEdit {
        path: file.clone(),
        edits: vec![lsp_types::TextEdit {
            range: lsp_types::Range::new(
                lsp_types::Position::new(0, 0),
                lsp_types::Position::new(0, 0),
            ),
            new_text: "x".into(),
        }],
    };
    assert_eq!(h.with(|e, w, cx| e.apply_file_edits(vec![edit], w, cx)), 0);

    h.with(|e, w, cx| e.reopen_with_encoding(0, euc_kr, w, cx));
    let tab = &h.read(|e| e.tab_states())[0];
    assert!(!tab.read_only && tab.encoding == "EUC-KR", "{tab:?}");
    assert_eq!(
        h.read(|e| e.active_path().map(Path::to_path_buf)),
        Some(file.clone())
    );
    h.type_text("한글 설명 고침\n");
    h.with(|e, w, cx| e.save_active(w, cx));
    assert_eq!(
        std::fs::read(&file).unwrap(),
        euc_kr.encode("한글 설명 고침\n").0.as_ref()
    );
}

/// ui-editor-02: another program holds the file without delete sharing,
/// so the atomic replace fails. The save reports it (no silent in-place
/// write), the disk keeps the old text and the tab stays dirty; the banner
/// offers retry and an explicit in-place overwrite.
#[cfg(windows)]
#[core::prelude::v1::test]
fn failed_replace_is_reported_then_retried_or_overwritten() {
    use std::os::windows::fs::OpenOptionsExt;
    let (_dir, root) = worktree();
    let file = root.join("held.txt");
    std::fs::write(&file, "old\n").unwrap();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&file, w, cx)).unwrap();
    h.type_text("new\n");
    let hold = || {
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x2) // no FILE_SHARE_DELETE
            .open(&file)
            .unwrap()
    };
    let reader = hold();
    h.with(|e, w, cx| e.save_active(w, cx));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old\n");
    let tab = &h.read(|e| e.tab_states())[0];
    assert!(tab.dirty && tab.save_failed, "{tab:?}");
    let leftovers = std::fs::read_dir(&root).unwrap().count();
    assert_eq!(leftovers, 1, "no temporary file left");

    // Retry while still held: still refused, still intact.
    h.with(|e, w, cx| e.answer_save_failure(false, w, cx));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "old\n");
    assert!(h.read(|e| e.tab_states())[0].save_failed);

    // Explicit overwrite in place.
    h.with(|e, w, cx| e.answer_save_failure(true, w, cx));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "new\n");
    let tab = &h.read(|e| e.tab_states())[0];
    assert!(!tab.dirty && !tab.save_failed, "{tab:?}");
    drop(reader);

    // Once free, a normal retry succeeds.
    h.type_text("newer\n");
    let reader = hold();
    h.with(|e, w, cx| e.save_active(w, cx));
    assert!(h.read(|e| e.tab_states())[0].save_failed);
    drop(reader);
    h.with(|e, w, cx| e.answer_save_failure(false, w, cx));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "newer\n");
    assert!(!h.read(|e| e.tab_states())[0].save_failed);
}
