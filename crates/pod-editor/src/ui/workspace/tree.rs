//! File tree commands: keyboard navigation, open/preview from the tree, and
//! new file/folder, rename and delete inside the root (never `.git`).

use std::path::{Path, PathBuf};

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;

use crate::tree::TreeAction;
use crate::ui::strings as txt;
use crate::uri::path_key;

use super::{
    EditorEvent, EditorWorkspace, Overlay, TabKey, TreeDelete, TreeDown, TreeEnd, TreeHome,
    TreeLeft, TreeNewFile, TreeOpen, TreePreview, TreeRename, TreeRight, TreeUp, rel_of,
};

impl EditorWorkspace {
    pub(super) fn tree_moved(&mut self, cx: &mut Context<Self>) {
        if let Some(i) = self.tree.selected_index() {
            self.tree_scroll.scroll_to_item(i, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    pub(super) fn on_tree_up(&mut self, _: &TreeUp, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_up();
        self.tree_moved(cx);
    }

    pub(super) fn on_tree_down(&mut self, _: &TreeDown, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_down();
        self.tree_moved(cx);
    }

    pub(super) fn on_tree_left(&mut self, _: &TreeLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_left();
        self.tree_moved(cx);
    }

    pub(super) fn on_tree_right(&mut self, _: &TreeRight, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_right();
        self.tree_moved(cx);
    }

    pub(super) fn on_tree_home(&mut self, _: &TreeHome, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_home();
        self.tree_moved(cx);
    }

    pub(super) fn on_tree_end(&mut self, _: &TreeEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_end();
        self.tree_moved(cx);
    }

    pub(super) fn on_tree_open(
        &mut self,
        _: &TreeOpen,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = self.tree.selected_index() {
            self.tree_activate(i, false, false, window, cx);
        }
    }

    pub(super) fn on_tree_preview(
        &mut self,
        _: &TreePreview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = self.tree.selected_index() {
            self.tree_activate(i, false, true, window, cx);
        }
    }

    fn tree_target(&self) -> Option<(PathBuf, bool)> {
        let i = self.tree.selected_index()?;
        let r = self.tree.rows().get(i)?;
        Some((r.path.clone(), r.is_dir))
    }

    pub(super) fn on_tree_new_file(
        &mut self,
        _: &TreeNewFile,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let parent = match self.tree_target() {
            Some((p, true)) => p,
            Some((p, false)) => p
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| self.root.clone()),
            None => self.root.clone(),
        };
        self.start_new_entry(parent, false, window, cx);
    }

    pub(super) fn on_tree_rename(
        &mut self,
        _: &TreeRename,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((p, _)) = self.tree_target() {
            self.start_rename_entry(p, window, cx);
        }
    }

    pub(super) fn on_tree_delete(
        &mut self,
        _: &TreeDelete,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((p, _)) = self.tree_target() {
            self.overlay = Overlay::ConfirmDelete(p);
            self.prompt_focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Tree: "new file/folder" prompt under `parent`.
    pub fn start_new_entry(
        &mut self,
        parent: PathBuf,
        folder: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(txt::NAME_PLACEHOLDER));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.commit_new_entry(window, cx);
                }
            },
        );
        self._subs.push(sub);
        input.update(cx, |s, cx| s.focus(window, cx));
        self.overlay = Overlay::NewEntry {
            parent,
            folder,
            input,
            error: None,
        };
        cx.notify();
    }

    fn commit_new_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::NewEntry {
            parent,
            folder,
            input,
            ..
        } = &self.overlay
        else {
            return;
        };
        let (parent, folder) = (parent.clone(), *folder);
        let name = input.read(cx).value().trim().replace('\\', "/");
        let target = match valid_child(&self.root, &parent, &name) {
            Ok(t) => t,
            Err(e) => return self.set_overlay_error(e, cx),
        };
        if target.exists() {
            return self.set_overlay_error(txt::ALREADY_EXISTS.into(), cx);
        }
        let res = if folder {
            std::fs::create_dir_all(&target)
        } else {
            target
                .parent()
                .map(std::fs::create_dir_all)
                .transpose()
                .and_then(|_| std::fs::write(&target, ""))
        };
        match res {
            Ok(()) => {
                self.close_overlay(window, cx);
                self.tree.refresh_paths(&[parent]);
                let rel = rel_of(&self.root, &target);
                self.flash(txt::created(&rel), false, cx);
                if folder {
                    self.tree.reveal(&target);
                } else if let Err(e) = self.open_file(&target, window, cx) {
                    self.flash(e, true, cx);
                }
                self.rebuild_index(cx);
            }
            Err(e) => self.set_overlay_error(txt::fs_error("만들지", &e), cx),
        }
    }

    pub fn start_rename_entry(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.commit_rename_entry(window, cx);
                }
            },
        );
        self._subs.push(sub);
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.overlay = Overlay::RenameEntry {
            path,
            input,
            error: None,
        };
        cx.notify();
    }

    fn commit_rename_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::RenameEntry { path, input, .. } = &self.overlay else {
            return;
        };
        let path = path.clone();
        let name = input.read(cx).value().trim().to_string();
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.root.clone());
        if name.contains(['/', '\\']) {
            return self.set_overlay_error(txt::INVALID_NAME.into(), cx);
        }
        let target = match valid_child(&self.root, &parent, &name) {
            Ok(t) => t,
            Err(e) => return self.set_overlay_error(e, cx),
        };
        if target == path {
            return self.close_overlay(window, cx);
        }
        // Case-only renames are fine on Windows; anything else must be new.
        if target.exists() && path_key(&target) != path_key(&path) {
            return self.set_overlay_error(txt::ALREADY_EXISTS.into(), cx);
        }
        match std::fs::rename(&path, &target) {
            Ok(()) => {
                self.close_overlay(window, cx);
                self.retarget_buffers(&path, &target, cx);
                self.tree.refresh_paths(&[parent]);
                self.tree.reveal(&target);
                let (a, b) = (rel_of(&self.root, &path), rel_of(&self.root, &target));
                self.flash(txt::renamed(&a, &b), false, cx);
                self.rebuild_index(cx);
            }
            Err(e) => self.set_overlay_error(txt::fs_error("이름을 바꾸지", &e), cx),
        }
    }

    /// Open buffers at/under `from` follow a rename to `to`.
    fn retarget_buffers(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let fk = path_key(from);
        for i in 0..self.buffers.len() {
            let old = self.buffers[i].path.clone();
            let k = path_key(&old);
            let new = if k == fk {
                to.to_path_buf()
            } else if let Ok(rest) = old.strip_prefix(from) {
                to.join(rest)
            } else {
                continue;
            };
            let text_now = self.buffers[i].state.read(cx).value().to_string();
            let reopen = self.buffers[i].lsp_open;
            self.lsp.update(cx, |s, cx| {
                s.close_document(&old);
                if reopen {
                    s.open_document(&new, text_now, cx);
                }
            });
            let rel = rel_of(&self.root, &new);
            self.tabs.rename(&TabKey::of(&old), TabKey::of(&new));
            let b = &mut self.buffers[i];
            b.path = new.clone();
            b.rel = rel;
            cx.emit(EditorEvent::FileClosed(old));
            cx.emit(EditorEvent::FileOpened(new));
        }
        cx.emit(EditorEvent::ActiveFileChanged(
            self.active_path().map(Path::to_path_buf),
        ));
    }

    pub(super) fn commit_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::ConfirmDelete(path) = std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return;
        };
        let dir = path.is_dir();
        let res = if dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match res {
            Ok(()) => {
                // Open tabs of deleted files close (they are gone; unsaved
                // edits there were confirmed away with the delete).
                let doomed: Vec<PathBuf> = self
                    .buffers
                    .iter()
                    .filter(|b| path_key(&b.path) == path_key(&path) || b.path.starts_with(&path))
                    .map(|b| b.path.clone())
                    .collect();
                for p in doomed {
                    if let Some(ix) = self.find_buffer(&p) {
                        self.remove_tab_ex(ix, false, window, cx);
                    }
                }
                if let Some(parent) = path.parent() {
                    self.tree.refresh_paths(&[parent.to_path_buf()]);
                }
                self.flash(txt::deleted(&rel_of(&self.root, &path)), false, cx);
                self.rebuild_index(cx);
                self.close_overlay(window, cx);
            }
            Err(e) => self.flash(txt::fs_error("삭제", &e), true, cx),
        }
        cx.notify();
    }

    fn set_overlay_error(&mut self, e: String, cx: &mut Context<Self>) {
        match &mut self.overlay {
            Overlay::NewEntry { error, .. } | Overlay::RenameEntry { error, .. } => {
                *error = Some(e)
            }
            _ => {}
        }
        cx.notify();
    }

    /// Opens tree row `ix` (folders toggle). `preview`: single click / Space;
    /// double click and Enter open a pinned tab.
    pub(super) fn tree_activate(
        &mut self,
        ix: usize,
        focus_editor: bool,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let TreeAction::Open(p) = self.tree.activate(ix) {
            match self.open_tab(&p, None, preview, window, cx) {
                Ok(()) if !focus_editor => self.tree_focus.focus(window, cx),
                Ok(()) => {}
                Err(e) => self.flash(e, true, cx),
            }
        }
        cx.notify();
    }
}

/// `parent/name` for a tree file operation, refusing empty names, `..`,
/// absolute paths and anything outside the worktree root.
pub(crate) fn valid_child(root: &Path, parent: &Path, name: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    let name = name.trim().trim_matches('/');
    if name.is_empty() || name.contains(':') {
        return Err(txt::INVALID_NAME.into());
    }
    let rel = Path::new(name);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(txt::INVALID_NAME.into());
    }
    let target = parent.join(rel);
    if !target.starts_with(root)
        || target
            .strip_prefix(root)
            .unwrap_or(&target)
            .components()
            .any(|c| is_git_dir_name(&c.as_os_str().to_string_lossy()))
    {
        return Err(txt::INVALID_NAME.into());
    }
    Ok(target)
}

/// Whether a path component names `.git` on Windows: NTFS ignores case,
/// Win32 drops trailing dots and spaces (`.git.`), and `GIT~1` is its 8.3
/// short name.
fn is_git_dir_name(name: &str) -> bool {
    let n = name.trim_end_matches(['.', ' ']);
    if n.eq_ignore_ascii_case(".git") {
        return true;
    }
    let lower = n.to_ascii_lowercase();
    lower
        .strip_prefix("git~")
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[core::prelude::v1::test]
    fn tree_names_stay_inside_the_root() {
        let root = Path::new("C:/w/repo");
        let src = root.join("src");
        assert_eq!(valid_child(root, &src, "a.rs").unwrap(), src.join("a.rs"));
        assert_eq!(
            valid_child(root, &src, "mod/b.rs").unwrap(),
            src.join("mod/b.rs")
        );
        assert_eq!(
            valid_child(root, root, "한글 노트.md").unwrap(),
            root.join("한글 노트.md")
        );
        assert_eq!(
            valid_child(root, root, ".github/x.yml").unwrap(),
            root.join(".github/x.yml")
        );
        assert_eq!(
            valid_child(root, root, ".gitignore").unwrap(),
            root.join(".gitignore")
        );
        // editor-14: `.git` in any case, with trailing dots/spaces, or by its 8.3 name.
        for bad in [
            "",
            "  ",
            "../x",
            "a/../../x",
            "C:/x",
            ".git/config",
            "a/.git",
            ".GIT/hooks/pre-commit",
            "a/.Git",
            ".git./x",
            ".git /x",
            "GIT~1/hooks/x",
        ] {
            assert!(valid_child(root, &src, bad).is_err(), "{bad}");
        }
    }
}
