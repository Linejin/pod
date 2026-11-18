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

    fn rename(&mut self, path: &Path, name: &str) {
        self.with(|e, w, cx| e.start_rename_entry(path.to_path_buf(), w, cx));
        self.cx.update(|w, cx| {
            w.activate_window();
            w.refresh();
            let _ = w.draw(cx);
        });
        self.cx.simulate_input(name);
        self.cx.simulate_keystrokes("enter");
        self.cx.run_until_parked();
        assert!(!self.read(|e| e.has_overlay()), "rename must succeed");
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
    // Buffers are named in the root's canonical spelling (TEMP may be an
    // 8.3 short name such as RUNNER~1).
    assert_eq!(
        h.read(|e| e.active_path().map(Path::to_path_buf)),
        Some(pod_editor::uri::simplify(&file.canonicalize().unwrap()))
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

/// B01: use the real rename prompt and editor events, then the same dirty
/// flag used by save-all, tab close and the host's task-close safeguard.
#[core::prelude::v1::test]
fn renamed_buffers_keep_edit_save_and_close_tracking() {
    for directory in [false, true] {
        for already_dirty in [false, true] {
            let (_dir, root) = worktree();
            std::fs::create_dir_all(root.join("src")).unwrap();
            let file = root.join("src/a.txt");
            std::fs::write(&file, "saved\n").unwrap();
            let mut h = Harness::new(&root);
            h.with(|e, w, cx| e.preview_file(&file, w, cx)).unwrap();
            let old = h.read(|e| e.active_path().unwrap().to_path_buf());
            let state = h.with(|e, _, _| e.active_editor().unwrap());
            if already_dirty {
                h.type_text("before rename\n");
            }
            let new = if directory {
                let parent = old.parent().unwrap();
                h.rename(parent, "renamed");
                parent.parent().unwrap().join("renamed/a.txt")
            } else {
                h.rename(&old, "renamed.txt");
                old.with_file_name("renamed.txt")
            };
            assert!(!old.exists());
            assert_eq!(h.read(|e| e.active_path().unwrap().to_path_buf()), new);
            assert_eq!(h.with(|e, _, _| e.active_editor().unwrap()), state);
            assert_eq!(h.read(|e| e.tab_states())[0].dirty, already_dirty);

            // Returning to the saved text clears an already-dirty renamed tab.
            h.type_text("saved\n");
            assert!(h.read(|e| e.dirty_paths()).is_empty());
            h.type_text("after rename\n");
            assert_eq!(h.read(|e| e.dirty_paths()), vec![new.clone()]);
            assert!(!h.read(|e| e.tab_states())[0].preview);
            h.with(|e, w, cx| e.close_active(w, cx));
            assert!(h.read(|e| e.close_prompt_open()));
            h.with(|e, w, cx| e.answer_close_prompt(None, w, cx));
            assert_eq!(h.read(|e| e.open_paths()), vec![new.clone()]);
            h.with(|e, w, cx| e.save_all(w, cx));
            assert_eq!(std::fs::read_to_string(&new).unwrap(), "after rename\n");
            assert!(!old.exists(), "save must not recreate the old path");
            assert!(h.read(|e| e.dirty_paths()).is_empty());

            h.type_text("save on close\n");
            h.with(|e, w, cx| e.close_path(&new, false, w, cx));
            assert!(h.read(|e| e.close_prompt_open()));
            h.with(|e, w, cx| e.answer_close_prompt(Some(true), w, cx));
            assert!(h.read(|e| e.open_paths()).is_empty());
            assert_eq!(std::fs::read_to_string(&new).unwrap(), "save on close\n");
        }
    }
}

#[core::prelude::v1::test]
fn directory_rename_saves_background_and_active_buffers() {
    let (_dir, root) = worktree();
    std::fs::create_dir_all(root.join("src")).unwrap();
    for name in ["a.txt", "b.txt"] {
        std::fs::write(root.join("src").join(name), "saved\n").unwrap();
    }
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&root.join("src/a.txt"), w, cx))
        .unwrap();
    h.type_text("background unsaved\n");
    h.with(|e, w, cx| e.open_file(&root.join("src/b.txt"), w, cx))
        .unwrap();
    let old_dir = h.read(|e| e.active_path().unwrap().parent().unwrap().to_path_buf());
    h.rename(&old_dir, "moved");
    h.type_text("active unsaved\n");
    assert_eq!(h.read(|e| e.dirty_paths()).len(), 2);
    h.with(|e, w, cx| e.save_all(w, cx));
    assert!(h.read(|e| e.dirty_paths()).is_empty());
    assert_eq!(
        std::fs::read_to_string(root.join("moved/a.txt")).unwrap(),
        "background unsaved\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("moved/b.txt")).unwrap(),
        "active unsaved\n"
    );
    assert!(!old_dir.exists());
}

#[core::prelude::v1::test]
fn b08_reload_rechecks_large_file_editing_and_highlighting() {
    let (_dir, root) = worktree();
    let file = root.join("size.rs");
    let line = format!("//{}\n", "x".repeat(1021));
    let small = line.repeat(1024); // 1 MiB
    let large = line.repeat(3072); // 3 MiB
    std::fs::write(&file, &small).unwrap();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&file, w, cx)).unwrap();
    for (contents, read_only, language) in [(&large, true, "plaintext"), (&small, false, "rust")] {
        std::fs::write(&file, contents).unwrap();
        // Encoding reopen uses the same reload path as watcher/explicit reload.
        h.with(|e, w, cx| e.reopen_with_encoding(0, pod_editor::text::utf8(), w, cx));
        assert_eq!(h.read(|e| e.tab_states())[0].read_only, read_only);
        h.with(|e, _, cx| {
            let editor = e.active_editor().unwrap();
            assert_eq!(editor.read(cx).value().to_string(), *contents);
            assert_eq!(editor.read(cx).language_name().as_ref(), language);
            assert_eq!(
                editor.read(cx).lsp().completion_provider.is_some(),
                !read_only
            );
        });
    }
    h.type_text("fn edited_after_shrink() {}\n");
    h.with(|e, w, cx| e.save_active(w, cx));
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "fn edited_after_shrink() {}\n"
    );
}

#[cfg(windows)]
#[core::prelude::v1::test]
fn b09_failed_reload_keeps_conflict_buffer_and_retry() {
    use std::os::windows::fs::OpenOptionsExt;
    let (_dir, root) = worktree();
    let file = root.join("conflict.txt");
    std::fs::write(&file, "original").unwrap();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&file, w, cx)).unwrap();
    h.type_text("my unsaved work");
    std::fs::write(&file, "external version").unwrap();
    h.with(|e, w, cx| e.save_active(w, cx));
    assert!(h.read(|e| e.tab_states())[0].disk_changed);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&file)
        .unwrap();
    h.with(|e, w, cx| e.answer_disk_prompt(true, w, cx));
    let tab = &h.read(|e| e.tab_states())[0];
    assert!(
        tab.disk_changed && tab.dirty,
        "failed read must preserve conflict: {tab:?}"
    );
    h.with(|e, _, cx| {
        assert_eq!(
            e.active_editor().unwrap().read(cx).value().to_string(),
            "my unsaved work"
        )
    });
    h.with(|e, w, cx| e.show_disk_diff(w, cx));
    assert!(
        h.read(|e| e.disk_diff())
            .unwrap()
            .contains(&('-', "external version".into()))
    );
    drop(lock);
    h.with(|e, w, cx| e.answer_disk_prompt(true, w, cx));
    let tab = &h.read(|e| e.tab_states())[0];
    assert!(!tab.disk_changed && !tab.dirty);
    h.with(|e, _, cx| {
        assert_eq!(
            e.active_editor().unwrap().read(cx).value().to_string(),
            "external version"
        )
    });
}

#[core::prelude::v1::test]
fn b08_initially_large_file_becomes_editable_after_reload() {
    let (_dir, root) = worktree();
    let file = root.join("shrinking.rs");
    let line = format!("//{}\n", "x".repeat(1021));
    std::fs::write(&file, line.repeat(3072)).unwrap();
    let mut h = Harness::new(&root);
    h.with(|e, w, cx| e.open_file(&file, w, cx)).unwrap();
    assert!(h.read(|e| e.tab_states())[0].read_only);
    std::fs::write(&file, line.repeat(1024)).unwrap();
    h.with(|e, w, cx| e.reopen_with_encoding(0, pod_editor::text::utf8(), w, cx));
    assert!(!h.read(|e| e.tab_states())[0].read_only);
    h.type_text("fn writable_again() {}\n");
    h.with(|e, w, cx| e.save_active(w, cx));
    assert_eq!(
        std::fs::read_to_string(file).unwrap(),
        "fn writable_again() {}\n"
    );
}

/// Rendering the input must not reset the workspace's read-only policy.
#[core::prelude::v1::test]
fn rendered_large_and_undecodable_buffers_reject_native_input() {
    for bytes in [vec![b'x'; 3 * 1024 * 1024], vec![0xff, 0xfe, b'x']] {
        let (_dir, root) = worktree();
        let file = root.join("protected.txt");
        std::fs::write(&file, &bytes).unwrap();
        let mut h = Harness::new(&root);
        h.with(|e, w, cx| e.open_file(&file, w, cx)).unwrap();
        let before = h.cx.update(|_, cx| {
            h.ed.read(cx)
                .active_editor()
                .unwrap()
                .read(cx)
                .value()
                .to_string()
        });
        h.cx.update(|w, cx| {
            w.activate_window();
            w.refresh();
            let _ = w.draw(cx);
        });
        h.cx.simulate_input("must not enter read-only buffer");
        h.cx.run_until_parked();
        let unchanged = h.with(|e, _, cx| {
            e.active_editor().unwrap().read(cx).value() == before
        });
        assert!(unchanged, "render must preserve read-only input behavior");
        assert!(!h.read(|e| e.tab_states())[0].dirty);
        h.with(|e, w, cx| e.save_active(w, cx));
        assert_eq!(std::fs::read(&file).unwrap(), bytes);

        // The same rendered entity becomes editable again after a safe reload.
        std::fs::write(&file, "small valid UTF-8").unwrap();
        h.with(|e, w, cx| e.reopen_with_encoding(0, pod_editor::text::utf8(), w, cx));
        h.cx.update(|w, cx| {
            w.refresh();
            let _ = w.draw(cx);
        });
        h.cx.simulate_keystrokes("ctrl-end");
        h.cx.simulate_input(" edited");
        h.cx.run_until_parked();
        assert!(h.read(|e| e.tab_states())[0].dirty);
        h.with(|e, w, cx| e.save_active(w, cx));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "small valid UTF-8 edited"
        );
    }
}
