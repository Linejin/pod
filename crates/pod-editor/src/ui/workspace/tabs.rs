//! Tab lifecycle on top of `TabSet` (the single owner of tab rules): open,
//! preview and goto, activation, order and drag moves, Ctrl+Tab, the tab
//! menu and open-files list, closing (unsaved prompt, bulk and host closes)
//! and reopening closed tabs. Activation registers the file with its
//! language server unless a session restore deferred that (`defer_lsp`,
//! cleared by the first focus in the view).

use std::path::{Path, PathBuf};

use gpui_kit::*;
use lsp_types as lt;

use crate::tabs::{BulkClose, disambiguate};
use crate::text;
use crate::ui::strings as txt;
use crate::uri::{simplify, under_root_spelling};

use super::{
    DiskState, EditorEvent, EditorWorkspace, MenuEntry, MenuItem, OVERFLOW_BTN_W, Overlay,
    TabCommand, TabKey, TabMenuItem, TabMenuState, TabState, file_name, rel_of,
};

impl EditorWorkspace {
    /// Opens (or activates) a file in a pinned tab. `path` may be relative to the root.
    pub fn open_file(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.open_at(path, None, window, cx)
    }

    /// Opens a file in the preview tab (as a single click in the tree does):
    /// it replaces the current preview tab until it is edited or pinned.
    pub fn preview_file(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.open_tab(path, None, true, window, cx)
    }

    /// Tab paths in order with their state, for hosts and tests.
    pub fn tab_states(&self) -> Vec<TabState> {
        self.buffers
            .iter()
            .enumerate()
            .map(|(i, b)| TabState {
                path: b.path.clone(),
                active: self.tabs.active() == Some(i),
                preview: self.tabs.is_preview(&TabKey::of(&b.path)),
                dirty: b.dirty,
                disk_changed: b.disk != DiskState::Same,
                read_only: b.large || b.undecodable,
                encoding: b.format.encoding.name(),
                save_failed: b.save_failed.is_some(),
            })
            .collect()
    }

    /// Paths of closed tabs Ctrl+Shift+T can reopen (newest last).
    pub fn closed_paths(&self) -> Vec<PathBuf> {
        self.tabs
            .closed()
            .iter()
            .map(|c| c.key.path.clone())
            .collect()
    }

    /// Makes the active tab permanent (double click on the tab, Ctrl+K Enter).
    pub fn pin_active(&mut self, cx: &mut Context<Self>) {
        if let Some(k) = self.tabs.active_key().cloned() {
            self.tabs.pin(&k);
            cx.emit(EditorEvent::TabsChanged);
            cx.notify();
        }
    }

    /// Moves tab `from` to index `to` (drag and drop). A preview tab is
    /// pinned. Returns whether the order changed.
    pub fn move_tab(&mut self, from: usize, to: usize, cx: &mut Context<Self>) -> bool {
        let Some(to) = self.tabs.move_tab(from, to) else {
            return false;
        };
        if from != to {
            let b = self.buffers.remove(from);
            self.buffers.insert(to, b);
        }
        debug_assert!(
            self.buffers
                .iter()
                .zip(self.tabs.keys())
                .all(|(b, k)| TabKey::of(&b.path) == *k)
        );
        self.scroll_active = true;
        cx.emit(EditorEvent::TabsChanged);
        cx.notify();
        true
    }

    /// Ctrl+Shift+PageDown / PageUp: moves the active tab right / left.
    pub fn move_active_tab(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(a) = self.tabs.active() else {
            return false;
        };
        let to = if forward { a + 1 } else { a.wrapping_sub(1) };
        to < self.buffers.len() && self.move_tab(a, to, cx)
    }

    pub(super) fn drop_tab(&mut self, path: &Path, slot: usize, cx: &mut Context<Self>) {
        self.drop_slot = None;
        if let Some(from) = self.find_buffer(path) {
            let to = self.tabs.drop_index(from, slot);
            self.move_tab(from, to, cx);
        }
        cx.notify();
    }

    /// Visible tab titles in order (`mod.rs · podd/src` for same names).
    pub fn tab_titles(&self) -> Vec<String> {
        let rels: Vec<&str> = self.buffers.iter().map(|b| b.rel.as_str()).collect();
        disambiguate(&rels)
            .into_iter()
            .zip(&self.buffers)
            .map(|(d, b)| txt::tab_title(&file_name(&b.path), d.as_deref()))
            .collect()
    }

    /// Whether tab `ix` lies fully inside the tab strip's visible area
    /// (last painted frame).
    pub fn tab_in_view(&self, ix: usize) -> bool {
        let Some(b) = self.tab_scroll.bounds_for_item(ix) else {
            return false;
        };
        let view = self.tab_scroll.bounds();
        let x = self.tab_scroll.offset().x;
        let slack = px(0.5);
        b.left() + x >= view.left() - slack && b.right() + x <= view.right() + slack
    }

    /// Whether the tab strip overflows (the open-files button is shown).
    pub fn tabs_overflowing(&self) -> bool {
        self.tab_overflow
    }

    /// Horizontal scroll offset of the tab strip (0 or negative).
    pub fn tab_scroll_x(&self) -> f32 {
        f32::from(self.tab_scroll.offset().x)
    }

    /// Opens the tab menu of tab `ix` (the active tab when `None`) below
    /// the tab, as Shift+F10 / the menu key do.
    pub fn open_tab_menu(
        &mut self,
        ix: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = ix.or(self.tabs.active()) else {
            return;
        };
        let pos = self
            .tab_scroll
            .bounds_for_item(ix)
            .map(|b| point(b.left() + self.tab_scroll.offset().x, b.bottom()))
            .unwrap_or_else(|| self.tab_scroll.bounds().bottom_left());
        self.open_tab_menu_at(ix, pos, window, cx);
    }

    pub(super) fn open_tab_menu_at(
        &mut self,
        ix: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.buffers.get(ix) else {
            return;
        };
        if self.has_overlay() {
            return;
        }
        let n = self.buffers.len();
        let preview = self.tabs.is_preview(&TabKey::of(&b.path));
        let any_saved = self.buffers.iter().any(|b| !b.dirty);
        let item = |cmd: TabCommand,
                    label: &str,
                    shortcut: Option<&'static str>,
                    enabled: bool,
                    separator_before: bool| MenuItem {
            entry: MenuEntry::Command(cmd),
            label: label.to_string(),
            name: if enabled {
                label.to_string()
            } else {
                format!("{label} ({})", txt::UNAVAILABLE)
            },
            shortcut,
            enabled,
            separator_before,
        };
        let items = vec![
            item(
                TabCommand::Close,
                txt::MENU_CLOSE,
                Some("Ctrl+W"),
                true,
                false,
            ),
            item(
                TabCommand::CloseOthers,
                txt::MENU_CLOSE_OTHERS,
                None,
                n > 1,
                false,
            ),
            item(
                TabCommand::CloseRight,
                txt::MENU_CLOSE_RIGHT,
                None,
                ix + 1 < n,
                false,
            ),
            item(
                TabCommand::CloseSaved,
                txt::MENU_CLOSE_SAVED,
                None,
                any_saved,
                false,
            ),
            item(TabCommand::CloseAll, txt::MENU_CLOSE_ALL, None, true, false),
            if preview {
                item(
                    TabCommand::TogglePin,
                    txt::MENU_PIN,
                    Some("Ctrl+K Enter"),
                    true,
                    true,
                )
            } else {
                item(TabCommand::TogglePin, txt::MENU_UNPIN, None, !b.dirty, true)
            },
            item(TabCommand::CopyPath, txt::MENU_COPY_PATH, None, true, true),
            item(
                TabCommand::CopyRelPath,
                txt::MENU_COPY_REL_PATH,
                None,
                true,
                false,
            ),
            item(TabCommand::RevealInTree, txt::MENU_REVEAL, None, true, true),
        ];
        let title = txt::tab_menu_label(&file_name(&b.path));
        let target = Some(b.path.clone());
        self.show_menu(
            TabMenuState {
                title,
                items,
                selected: 0,
                target,
                position,
                anchor: Anchor::TopLeft,
                restore: None,
            },
            window,
            cx,
        );
    }

    /// The open-files list (overflow button): every tab, Enter activates.
    pub fn open_tab_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.buffers.is_empty() || self.has_overlay() {
            return;
        }
        let titles = self.tab_titles();
        let active = self.tabs.active();
        let items = self
            .buffers
            .iter()
            .zip(titles)
            .enumerate()
            .map(|(i, (b, title))| {
                let preview = self.tabs.is_preview(&TabKey::of(&b.path));
                let mut name = txt::tab_label_full(
                    &file_name(&b.path),
                    &b.rel,
                    txt::TabFlags {
                        dirty: b.dirty,
                        preview,
                        disk: None,
                    },
                );
                if active == Some(i) {
                    name = format!("{name}, {}", txt::CURRENT_TAB);
                }
                MenuItem {
                    entry: MenuEntry::Tab(b.path.clone()),
                    label: if b.dirty {
                        format!("{title} ●")
                    } else {
                        title
                    },
                    name,
                    shortcut: None,
                    enabled: true,
                    separator_before: false,
                }
            })
            .collect();
        let view = self.tab_scroll.bounds();
        let position = point(view.right() + px(OVERFLOW_BTN_W), view.bottom());
        self.show_menu(
            TabMenuState {
                title: txt::OPEN_TABS_MENU.into(),
                items,
                selected: active.unwrap_or(0),
                target: None,
                position,
                anchor: Anchor::TopRight,
                restore: None,
            },
            window,
            cx,
        );
    }

    fn show_menu(&mut self, mut m: TabMenuState, window: &mut Window, cx: &mut Context<Self>) {
        m.restore = window.focused(cx).filter(|f| *f != self.menu_focus);
        if !m.items.get(m.selected).is_some_and(|i| i.enabled) {
            m.selected = m.items.iter().position(|i| i.enabled).unwrap_or(0);
        }
        self.tab_menu = Some(m);
        self.menu_focus.focus(window, cx);
        cx.notify();
    }

    /// Whether the tab menu or the open-files list is open.
    pub fn tab_menu_open(&self) -> bool {
        self.tab_menu.is_some()
    }

    /// Whether keyboard focus is on the open menu.
    pub fn tab_menu_has_focus(&self, window: &Window) -> bool {
        self.tab_menu.is_some() && self.menu_focus.is_focused(window)
    }

    /// Items of the open menu (labels, availability, keyboard selection).
    pub fn tab_menu_items(&self) -> Vec<TabMenuItem> {
        let Some(m) = &self.tab_menu else {
            return vec![];
        };
        m.items
            .iter()
            .enumerate()
            .map(|(i, it)| TabMenuItem {
                label: it.label.clone(),
                enabled: it.enabled,
                selected: i == m.selected,
            })
            .collect()
    }

    /// Up/Down (`delta`), Home/End (`None` with `to_end`): moves the menu
    /// selection over available items, wrapping.
    pub(super) fn menu_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(m) = &mut self.tab_menu else { return };
        let n = m.items.len() as isize;
        let mut i = m.selected as isize;
        for _ in 0..n {
            i = (i + delta).rem_euclid(n);
            if m.items[i as usize].enabled {
                m.selected = i as usize;
                break;
            }
        }
        cx.notify();
    }

    pub(super) fn menu_edge(&mut self, last: bool, cx: &mut Context<Self>) {
        let Some(m) = &mut self.tab_menu else { return };
        let mut it = m
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.enabled)
            .map(|(ix, _)| ix);
        if let Some(ix) = if last { it.next_back() } else { it.next() } {
            m.selected = ix;
        }
        cx.notify();
    }

    /// Closes the menu; focus goes back where it was.
    pub(super) fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.tab_menu.take() else {
            return;
        };
        match m.restore {
            Some(f) => f.focus(window, cx),
            None => self.focus_active(window, cx),
        }
        cx.notify();
    }

    /// Runs the selected menu item (Enter / Space / click).
    pub(super) fn menu_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &self.tab_menu else { return };
        let Some(item) = m.items.get(m.selected).filter(|i| i.enabled) else {
            return;
        };
        let (entry, target) = (item.entry.clone(), m.target.clone());
        self.close_menu(window, cx);
        match entry {
            MenuEntry::Command(cmd) => {
                if let Some(ix) = target.and_then(|p| self.find_buffer(&p)) {
                    self.run_tab_command(ix, cmd, window, cx);
                }
            }
            MenuEntry::Tab(path) => {
                if let Some(ix) = self.find_buffer(&path) {
                    self.activate(ix, window, cx);
                    self.focus_active(window, cx);
                }
            }
        }
    }

    /// Runs a tab menu command on tab `ix`.
    pub fn run_tab_command(
        &mut self,
        ix: usize,
        cmd: TabCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.buffers.get(ix) else {
            return;
        };
        let (path, rel, dirty) = (b.path.clone(), b.rel.clone(), b.dirty);
        match cmd {
            TabCommand::Close => self.close_tab(ix, window, cx),
            TabCommand::CloseOthers => self.bulk_close(BulkClose::Others, ix, window, cx),
            TabCommand::CloseRight => self.bulk_close(BulkClose::Right, ix, window, cx),
            TabCommand::CloseSaved => self.bulk_close(BulkClose::Saved, ix, window, cx),
            TabCommand::CloseAll => self.bulk_close(BulkClose::All, ix, window, cx),
            TabCommand::TogglePin => {
                let key = TabKey::of(&path);
                if self.tabs.is_preview(&key) {
                    self.tabs.pin(&key);
                } else if !dirty {
                    self.tabs.unpin(&key);
                }
                cx.emit(EditorEvent::TabsChanged);
                cx.notify();
            }
            TabCommand::CopyPath | TabCommand::CopyRelPath => {
                let text = if cmd == TabCommand::CopyPath {
                    path.to_string_lossy().into_owned()
                } else {
                    rel.replace('/', std::path::MAIN_SEPARATOR_STR)
                };
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                self.flash(txt::copied(&text), false, cx);
            }
            TabCommand::RevealInTree => {
                self.tree.reveal(&path);
                self.tree_moved(cx);
                self.tree_focus.focus(window, cx);
            }
        }
    }

    /// Ctrl+Shift+T: reopens the newest closed tab whose file still loads,
    /// at its old position. Returns whether one was reopened.
    pub fn reopen_closed(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        while let Some(c) = self.tabs.pop_closed() {
            let Ok(loaded) = text::load(&c.key.path) else {
                continue;
            };
            let before = self.tabs.active_key().cloned();
            let b = self.make_buffer(&c.key.path, loaded, window, cx);
            let ix = self.tabs.insert_at(TabKey::of(&b.path), c.index);
            let path = b.path.clone();
            let rel = b.rel.clone();
            self.buffers.insert(ix, b);
            cx.emit(EditorEvent::FileOpened(path));
            self.after_activate(before, cx);
            self.focus_active(window, cx);
            self.recents.touch(&rel);
            return true;
        }
        self.flash(txt::NOTHING_TO_REOPEN, false, cx);
        false
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab: most recently used order while Ctrl is
    /// held (the order updates when Ctrl is released).
    pub fn switch_mru(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.tabs.active_key().cloned();
        if self.tabs.mru_step(forward).is_some() {
            self.after_activate(before, cx);
            self.focus_active(window, cx);
        }
    }

    /// Ctrl released: the Ctrl+Tab switch ends on the tab it reached.
    pub fn end_switch(&mut self, cx: &mut Context<Self>) {
        if self.tabs.switching() {
            self.tabs.commit_switch();
            cx.emit(EditorEvent::TabsChanged);
        }
    }

    /// Ctrl+PageDown / Ctrl+PageUp: next/previous tab by position.
    pub fn step_tab(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let before = self.tabs.active_key().cloned();
        if self.tabs.step_position(forward).is_some() {
            self.after_activate(before, cx);
            self.focus_active(window, cx);
        }
    }

    /// Closes the active tab like Ctrl+W (a dirty tab asks first).
    pub fn close_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.active() {
            self.close_tab(i, window, cx);
        }
    }

    /// Answers the open save/discard/cancel prompt (`Some(true)` save,
    /// `Some(false)` discard, `None` cancel), as its buttons do.
    pub fn answer_close_prompt(
        &mut self,
        save: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resolve_close(save, window, cx);
    }

    /// Whether the save/discard/cancel prompt for a tab is open.
    pub fn close_prompt_open(&self) -> bool {
        matches!(self.overlay, Overlay::ConfirmClose { .. })
    }

    /// Unsaved files the open close prompt lists (root-relative).
    pub fn close_prompt_files(&self) -> Vec<String> {
        match &self.overlay {
            Overlay::ConfirmClose { dirty, .. } => {
                dirty.iter().map(|p| rel_of(&self.root, p)).collect()
            }
            _ => vec![],
        }
    }

    /// Opens a file and moves the cursor to 1-based `line`/`col`.
    pub fn goto(
        &mut self,
        path: &Path,
        line: u32,
        col: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.open_at(
            path,
            Some(lt::Position::new(
                line.saturating_sub(1),
                col.saturating_sub(1),
            )),
            window,
            cx,
        )
    }

    /// Opens (or activates) a file in a pinned tab.
    pub(crate) fn open_at(
        &mut self,
        path: &Path,
        pos: Option<lt::Position>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        self.open_tab(path, pos, false, window, cx)
    }

    /// Opens (or activates) a file; `preview` reuses the preview tab.
    pub(super) fn open_tab(
        &mut self,
        path: &Path,
        pos: Option<lt::Position>,
        preview: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let path = if path.is_absolute() {
            under_root_spelling(&self.root, &simplify(path))
        } else {
            self.root.join(path)
        };
        let before = self.tabs.active_key().cloned();
        let was_preview = self.tabs.preview().cloned();
        let ix = match self.find_buffer(&path) {
            Some(_) => self.tabs.open(TabKey::of(&path), preview).index,
            None => {
                let loaded = text::load(&path).map_err(|e| e.to_string())?;
                let b = self.make_buffer(&path, loaded, window, cx);
                let o = self.tabs.open(TabKey::of(&b.path), preview);
                if o.replaced.is_some() {
                    // The old preview tab gives its place to the new file.
                    let old = self.buffers.remove(o.index);
                    self.drop_buffer(old, cx);
                }
                self.buffers.insert(o.index, b);
                cx.emit(EditorEvent::FileOpened(path.clone()));
                o.index
            }
        };
        debug_assert_eq!(self.buffers.len(), self.tabs.len());
        self.after_activate(before, cx);
        if self.tabs.preview() != was_preview.as_ref() {
            cx.emit(EditorEvent::TabsChanged);
        }
        let state = self.buffers[ix].state.clone();
        if let Some(p) = pos {
            state.update(cx, |s, cx| s.set_cursor_position(p, window, cx));
        } else {
            state.update(cx, |s, cx| s.focus(window, cx));
        }
        let rel = self.buffers[ix].rel.clone();
        self.recents.touch(&rel);
        self.tree.reveal(&path);
        if let Some(i) = self.tree.selected_index() {
            self.tree_scroll.scroll_to_item(i, ScrollStrategy::Nearest);
        }
        cx.notify();
        Ok(())
    }

    pub(super) fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
        let before = self.tabs.active_key().cloned();
        self.tabs.activate(ix);
        self.after_activate(before, cx);
    }

    /// After the tab set changed its active tab: tell the host and register
    /// the file with its language server (unless restore defers that).
    fn after_activate(&mut self, before: Option<TabKey>, cx: &mut Context<Self>) {
        self.scroll_active = true;
        if self.tabs.active_key() != before.as_ref() {
            cx.emit(EditorEvent::ActiveFileChanged(
                self.active_path().map(Path::to_path_buf),
            ));
        }
        if let Some(ix) = self.tabs.active()
            && !self.defer_lsp
        {
            self.ensure_lsp(ix, cx);
        }
        cx.notify();
    }

    pub(super) fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix < self.buffers.len() {
            self.request_close(&[ix], None, window, cx);
        }
    }

    /// Closes tabs `ixs`; when some have unsaved edits, one prompt lists
    /// them (save all / discard / cancel). `then` is activated afterwards.
    fn request_close(
        &mut self,
        ixs: &[usize],
        then: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let close: Vec<PathBuf> = ixs
            .iter()
            .filter_map(|&i| self.buffers.get(i))
            .map(|b| b.path.clone())
            .collect();
        if close.is_empty() {
            return;
        }
        let dirty: Vec<PathBuf> = ixs
            .iter()
            .filter_map(|&i| self.buffers.get(i))
            .filter(|b| b.dirty)
            .map(|b| b.path.clone())
            .collect();
        if dirty.is_empty() {
            self.close_now(&close, then, window, cx);
            return;
        }
        self.overlay = Overlay::ConfirmClose { dirty, close, then };
        self.prompt_focus.focus(window, cx);
        cx.notify();
    }

    /// Closes the tabs of `paths` without asking (Ctrl+Shift+T can reopen
    /// them), right to left; `then` (if still open) or the most recently
    /// used tab becomes active and gets focus.
    fn close_now(
        &mut self,
        paths: &[PathBuf],
        then: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ixs: Vec<usize> = paths.iter().filter_map(|p| self.find_buffer(p)).collect();
        if ixs.is_empty() {
            return;
        }
        let before = self.tabs.active_key().cloned();
        let then = then.map(|p| TabKey::of(&p));
        for (i, _) in self.tabs.close_many(&ixs, then.as_ref()) {
            let b = self.buffers.remove(i);
            self.drop_buffer(b, cx);
        }
        debug_assert_eq!(self.buffers.len(), self.tabs.len());
        self.after_activate(before, cx);
        if !self.has_overlay() {
            self.focus_active(window, cx);
        }
        cx.emit(EditorEvent::TabsChanged);
        cx.notify();
    }

    /// Tab menu bulk close at tab `ix` (dirty targets ask once, together).
    pub fn bulk_close(
        &mut self,
        kind: BulkClose,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(at) = self.buffers.get(ix).map(|b| b.path.clone()) else {
            return;
        };
        let buffers = &self.buffers;
        let targets = self.tabs.bulk_targets(kind, ix, |k| {
            buffers.iter().any(|b| b.dirty && TabKey::of(&b.path) == *k)
        });
        let then = matches!(kind, BulkClose::Others | BulkClose::Right).then_some(at);
        self.request_close(&targets, then, window, cx);
    }

    /// Closes the tab of `path` (if open). With `force`, unsaved edits are
    /// discarded; otherwise a dirty tab asks first.
    pub fn close_path(
        &mut self,
        path: &Path,
        force: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.find_buffer(path) else {
            return;
        };
        if force {
            self.remove_tab_ex(ix, false, window, cx);
        } else {
            self.close_tab(ix, window, cx);
        }
    }

    pub(super) fn resolve_close(
        &mut self,
        save: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Overlay::ConfirmClose {
            dirty,
            mut close,
            then,
        } = std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return;
        };
        match save {
            Some(true) => {
                for p in &dirty {
                    if let Some(ix) = self.find_buffer(p) {
                        self.save_buffer(ix, window, cx);
                    }
                }
                // A file that failed to save stays open (the error is shown).
                close.retain(|p| self.find_buffer(p).is_none_or(|ix| !self.buffers[ix].dirty));
                self.close_now(&close, then, window, cx);
                if self.tabs.active().is_some_and(|a| self.buffers[a].dirty) {
                    self.focus_active(window, cx);
                }
            }
            Some(false) => self.close_now(&close, then, window, cx),
            None => self.close_overlay(window, cx),
        }
        cx.notify();
    }

    /// Closes tab `ix` without asking. `remember: false` for files that are
    /// gone (deleted) or closed by the host, which are not offered again.
    /// Focus moves to the next active tab's editor, or the tree when none.
    pub(super) fn remove_tab_ex(
        &mut self,
        ix: usize,
        remember: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if ix >= self.buffers.len() {
            return;
        }
        let before = self.tabs.active_key().cloned();
        self.tabs.close(ix, remember);
        let b = self.buffers.remove(ix);
        self.drop_buffer(b, cx);
        debug_assert_eq!(self.buffers.len(), self.tabs.len());
        self.after_activate(before, cx);
        if !self.has_overlay() {
            self.focus_active(window, cx);
        }
        cx.emit(EditorEvent::TabsChanged);
        cx.notify();
    }
}
