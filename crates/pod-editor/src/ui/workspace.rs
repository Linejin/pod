//! `EditorWorkspace`: the embeddable editing view for one worktree root.
//!
//! Layout: file tree | (tabs / banner / code editor / problems+references) /
//! status bar, plus overlays (quick open, go to line, rename + preview).
//! Hosts (pod-app) create one per task worktree and listen to
//! [`EditorEvent`]; see docs/13-editor-and-lsp.md §API.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::base::input::Diagnostic as EditorDiagnostic;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{
    Editor, EditorState, Escape, GoToDefinition, IndentInline, Input, InputEditsEvent, InputEvent,
    InputState, OutdentInline, Rope, RopeExt as _, ShowCompletions, ShowDocumentHandler,
};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use lsp_types as lt;
use pod_ui::{Icon, Mode, Tokens, icon, tokens::color as tc};
use serde::{Deserialize, Serialize};

use super::quick_open::{QuickOpen, QuickOpenEvent};
use super::strings as txt;
use crate::git::{self, GitMark};
use crate::index::{self, IndexSnapshot, Recents};
use crate::lsp::providers::LspBridge;
use crate::lsp::snippet::{self, SnippetSession};
use crate::lsp::store::{apply_edits, back_to_front};
use crate::lsp::{
    CompletionExtra, FileEdit, Language, Location, LspStore, LspStoreEvent, ServerState,
    language_for_path,
};
use crate::tabs::{BulkClose, TabSet, disambiguate};
use crate::text::{self, DiskStamp, FileFormat};
use crate::tree::{FileTree, TreeAction};
use crate::uri::{path_key, simplify, uri_to_path};
use crate::watch::{Batch, RootWatcher};

const CTX: &str = "PodEditor";
const TREE_CTX: &str = "PodEditorTree";
const TREE_ROW_H: f32 = 28.;
const TREE_W: f32 = 256.;
const PANEL_H: f32 = 200.;
const TAB_STRIP_H: f32 = 32.;
/// Width of the open-files (overflow) button at the end of the tab strip.
const OVERFLOW_BTN_W: f32 = 32.;

actions!(
    pod_editor,
    [
        QuickOpenFile,
        Save,
        SaveAll,
        CloseTab,
        NextTab,
        PrevTab,
        NextTabPosition,
        PrevTabPosition,
        ReopenClosedTab,
        PinTab,
        MoveTabLeft,
        MoveTabRight,
        TabMenu,
        GotoLine,
        FindReferences,
        RenameSymbol,
        NextProblem,
        TogglePanel,
        FocusTree,
        FocusEditor
    ]
);
actions!(
    pod_editor_tree,
    [
        TreeUp,
        TreeDown,
        TreeLeft,
        TreeRight,
        TreeOpen,
        TreePreview,
        TreeHome,
        TreeEnd,
        TreeNewFile,
        TreeRename,
        TreeDelete
    ]
);

/// Registers key bindings. Call once after `gpui_kit::init`.
pub fn init(cx: &mut App) {
    let c = Some(CTX);
    let tc_ = Some(TREE_CTX);
    cx.bind_keys([
        KeyBinding::new("ctrl-p", QuickOpenFile, c),
        KeyBinding::new("ctrl-s", Save, c),
        KeyBinding::new("ctrl-k s", SaveAll, c),
        KeyBinding::new("ctrl-w", CloseTab, c),
        KeyBinding::new("ctrl-tab", NextTab, c),
        KeyBinding::new("ctrl-shift-tab", PrevTab, c),
        KeyBinding::new("ctrl-pagedown", NextTabPosition, c),
        KeyBinding::new("ctrl-pageup", PrevTabPosition, c),
        // Reopen the last closed tab. A host with a context-free Ctrl+Shift+T
        // (pod-app: new terminal) outranks this in GPUI and forwards it to
        // `reopen_closed` itself when focus is in the editor.
        KeyBinding::new("ctrl-shift-t", ReopenClosedTab, c),
        KeyBinding::new("ctrl-k enter", PinTab, c),
        // Move the active tab (no wrap); the order is saved with the layout.
        KeyBinding::new("ctrl-shift-pageup", MoveTabLeft, c),
        KeyBinding::new("ctrl-shift-pagedown", MoveTabRight, c),
        // The active tab's menu (close others, copy path, …).
        KeyBinding::new("shift-f10", TabMenu, c),
        KeyBinding::new("menu", TabMenu, c),
        KeyBinding::new("ctrl-g", GotoLine, c),
        KeyBinding::new("shift-f12", FindReferences, c),
        KeyBinding::new("f2", RenameSymbol, c),
        KeyBinding::new("f8", NextProblem, c),
        KeyBinding::new("ctrl-j", TogglePanel, c),
        KeyBinding::new("alt-1", FocusTree, c),
        KeyBinding::new("alt-2", FocusEditor, c),
        // gpui-component defines these editor actions without default keys.
        KeyBinding::new("f12", GoToDefinition, Some("Input")),
        KeyBinding::new("ctrl-space", ShowCompletions, Some("Input")),
        KeyBinding::new("up", TreeUp, tc_),
        KeyBinding::new("down", TreeDown, tc_),
        KeyBinding::new("left", TreeLeft, tc_),
        KeyBinding::new("right", TreeRight, tc_),
        // Enter opens a pinned tab, Space a preview (like a single click).
        KeyBinding::new("enter", TreeOpen, tc_),
        KeyBinding::new("space", TreePreview, tc_),
        KeyBinding::new("home", TreeHome, tc_),
        KeyBinding::new("end", TreeEnd, tc_),
        KeyBinding::new("f2", TreeRename, tc_),
        KeyBinding::new("delete", TreeDelete, tc_),
        KeyBinding::new("ctrl-n", TreeNewFile, tc_),
    ]);
}

/// Events for the host (pod-app).
#[derive(Clone, Debug, PartialEq)]
pub enum EditorEvent {
    FileOpened(PathBuf),
    ActiveFileChanged(Option<PathBuf>),
    FileSaved(PathBuf),
    DirtyChanged {
        path: PathBuf,
        dirty: bool,
    },
    /// The file changed on disk (e.g. an agent wrote it). `reloaded` is
    /// false when the buffer had unsaved edits and a prompt is shown.
    ExternalChange {
        path: PathBuf,
        reloaded: bool,
    },
    FileClosed(PathBuf),
    /// Tab order, preview/pinned state or the Ctrl+Tab order changed
    /// (layout persistence).
    TabsChanged,
}

/// One tab as the host sees it ([`EditorWorkspace::tab_states`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabState {
    pub path: PathBuf,
    pub active: bool,
    pub preview: bool,
    pub dirty: bool,
    /// The file changed or vanished on disk while the tab had unsaved edits.
    pub disk_changed: bool,
    /// Opened read-only (large, or not decodable without loss).
    pub read_only: bool,
    /// Encoding the file is read and written in (`UTF-8`, `EUC-KR`, ...).
    pub encoding: &'static str,
    /// The last save could not write the file (banner: retry/overwrite).
    pub save_failed: bool,
}

/// One open tab for layout persistence (path relative to the root, `/`
/// separators; 1-based line/column).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSession {
    pub path: String,
    #[serde(default = "one")]
    pub line: u32,
    #[serde(default = "one")]
    pub col: u32,
    /// A preview tab (replaced by the next single-click open).
    #[serde(default, skip_serializing_if = "is_false")]
    pub preview: bool,
}

fn one() -> u32 {
    1
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Open tabs and the active one, restored with the host layout.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorSession {
    #[serde(default)]
    pub tabs: Vec<TabSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<usize>,
    /// Ctrl+Tab order: tab indices, most recently used first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mru: Vec<usize>,
}

/// Tab identity: compared by [`path_key`] (case-insensitive on Windows),
/// keeping the real path for reopening a closed tab.
#[derive(Clone, Debug)]
struct TabKey {
    key: String,
    path: PathBuf,
}

impl PartialEq for TabKey {
    fn eq(&self, o: &Self) -> bool {
        self.key == o.key
    }
}

impl TabKey {
    fn of(path: &Path) -> Self {
        TabKey {
            key: path_key(path),
            path: path.to_path_buf(),
        }
    }
}

/// A save that could not write the file (ui-editor-02).
#[derive(Clone, Debug)]
struct SaveFailure {
    message: String,
    /// The atomic replace failed (file in use): "overwrite in place" is
    /// offered as an explicit, non-atomic choice.
    replace: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DiskState {
    Same,
    /// Disk differs and the buffer is dirty: prompt.
    Changed(String),
    Deleted,
}

struct Buffer {
    path: PathBuf,
    rel: String,
    state: Entity<EditorState>,
    language: Option<Language>,
    /// Encoding, BOM and line endings to write the file back with.
    format: FileFormat,
    /// LF text as last loaded/saved.
    saved: String,
    dirty: bool,
    large: bool,
    /// The bytes did not decode losslessly (not UTF-8, or not valid in the
    /// chosen encoding): read-only, never saved (editor-02).
    undecodable: bool,
    /// The file's bytes as last read or written; `None`: no file. Saving
    /// refuses when the disk no longer matches (editor-06).
    stamp: Option<DiskStamp>,
    /// The last save was refused because the disk changed (banner text).
    save_blocked: bool,
    /// The last save failed to write; the original file is untouched.
    save_failed: Option<SaveFailure>,
    disk: DiskState,
    /// Registered with the language server (done on first activation, so
    /// restored background tabs never start a server).
    lsp_open: bool,
    /// Tab stops of the last inserted snippet completion (Tab / Shift+Tab / Esc).
    snippet: Option<SnippetSession>,
    _subs: Vec<Subscription>,
    _sync: Task<()>,
}

enum Overlay {
    None,
    QuickOpen(Entity<QuickOpen>),
    GotoLine(Entity<InputState>),
    Rename {
        input: Entity<InputState>,
        path: PathBuf,
        pos: lt::Position,
    },
    RenamePreview {
        new_name: String,
        edits: Vec<FileEdit>,
    },
    /// Line diff of the file on disk (`-`) against the buffer (`+`).
    DiskDiff {
        rel: String,
        lines: Vec<(char, String)>,
    },
    /// Closing tabs, some with unsaved edits (`dirty`, listed): save all,
    /// discard or cancel. `then` becomes active afterwards (tab menu).
    ConfirmClose {
        dirty: Vec<PathBuf>,
        close: Vec<PathBuf>,
        then: Option<PathBuf>,
    },
    NewEntry {
        parent: PathBuf,
        folder: bool,
        input: Entity<InputState>,
        error: Option<String>,
    },
    RenameEntry {
        path: PathBuf,
        input: Entity<InputState>,
        error: Option<String>,
    },
    ConfirmDelete(PathBuf),
}

/// A command of the tab menu (right click, Shift+F10 / menu key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabCommand {
    Close,
    CloseOthers,
    CloseRight,
    CloseSaved,
    CloseAll,
    TogglePin,
    CopyPath,
    CopyRelPath,
    RevealInTree,
}

#[derive(Clone)]
enum MenuEntry {
    Command(TabCommand),
    /// The open-files list: activates that tab.
    Tab(PathBuf),
}

struct MenuItem {
    entry: MenuEntry,
    label: String,
    /// Accessible name (state, path).
    name: String,
    shortcut: Option<&'static str>,
    enabled: bool,
    separator_before: bool,
}

/// The open tab menu or open-files list. Keyboard focus sits on the menu
/// (`menu_focus`); the selected item is its active descendant.
struct TabMenuState {
    title: String,
    items: Vec<MenuItem>,
    selected: usize,
    /// Tab the commands act on (`None` for the open-files list).
    target: Option<PathBuf>,
    position: Point<Pixels>,
    anchor: Anchor,
    /// Focus before the menu opened; Esc and commands return it.
    restore: Option<FocusHandle>,
}

/// One item of the open tab menu ([`EditorWorkspace::tab_menu_items`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabMenuItem {
    pub label: String,
    pub enabled: bool,
    pub selected: bool,
}

/// Drag payload of a tab.
#[derive(Clone)]
struct DraggedTab {
    path: PathBuf,
    title: String,
}

/// What follows the cursor while a tab is dragged.
struct TabDragPreview {
    title: String,
    mode: Mode,
}

impl Render for TabDragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::for_mode(self.mode);
        div()
            .h(px(28.))
            .px(px(12.))
            .flex()
            .items_center()
            .rounded(px(6.))
            .bg(tc(t.bg_raised))
            .border_1()
            .border_color(tc(t.border_subtle))
            .shadow(pod_ui::tokens::elevation(2, self.mode))
            .text_size(px(13.))
            .text_color(tc(t.text_primary))
            .whitespace_nowrap()
            .child(self.title.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelTab {
    Problems,
    References,
}

pub struct EditorWorkspace {
    root: PathBuf,
    mode: Mode,
    focus: FocusHandle,
    tree_focus: FocusHandle,
    /// Confirm prompts (unsaved close, delete): focused while shown so
    /// Enter/Esc and Tab between their buttons reach them.
    prompt_focus: FocusHandle,
    tree: FileTree,
    tree_scroll: UniformListScrollHandle,
    /// Open files, in tab order (mirrors `tabs.keys()`).
    buffers: Vec<Buffer>,
    /// Tab order, active tab, preview tab, MRU and closed-tab history.
    tabs: TabSet<TabKey>,
    /// Horizontal scroll of the tab strip.
    tab_scroll: ScrollHandle,
    /// Scroll the active tab into view on the next frame.
    scroll_active: bool,
    /// Follow-up frames that may re-scroll: GPUI scrolls against the
    /// previous frame's viewport, which is stale when the width changes.
    scroll_retries: u8,
    /// The strip overflows: the open-files button is shown.
    tab_overflow: bool,
    /// Insertion slot (0..=tabs) under a dragged tab.
    drop_slot: Option<usize>,
    tab_menu: Option<TabMenuState>,
    menu_focus: FocusHandle,
    lsp: Entity<LspStore>,
    index: IndexSnapshot,
    indexing: bool,
    index_cancel: Arc<AtomicBool>,
    recents: Recents,
    overlay: Overlay,
    panel_open: bool,
    panel_tab: PanelTab,
    references: Vec<Location>,
    references_title: String,
    message: Option<(String, bool)>,
    branch: Option<String>,
    git_changes: usize,
    /// Reload clean buffers silently when the file changes on disk.
    pub auto_reload_clean: bool,
    /// "무시" on the `.pod/lsp.json` approval banner (this session).
    repo_lsp_dismissed: bool,
    /// While true (session restore), activating a tab does not start its
    /// language server; the first focus inside the view clears it.
    defer_lsp: bool,
    _watcher: Option<RootWatcher>,
    _tasks: Vec<Task<()>>,
    _subs: Vec<Subscription>,
    _index_task: Task<()>,
    _git_task: Task<()>,
    _msg_task: Task<()>,
}

impl EventEmitter<EditorEvent> for EditorWorkspace {}

impl Focusable for EditorWorkspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Applies editor-column edits to an open buffer as undoable ranged
/// replacements (back to front), so the edits reach the language server
/// incrementally and the caret keeps its place in the text around them.
fn edit_buffer(
    state: &Entity<EditorState>,
    edits: &[lt::TextEdit],
    window: &mut Window,
    cx: &mut App,
) {
    state.update(cx, |s, cx| {
        let text = s.text().clone();
        let ranges: Vec<(std::ops::Range<usize>, &str)> = edits
            .iter()
            .map(|e| {
                let a = text.position_to_offset(&e.range.start);
                (
                    a..text.position_to_offset(&e.range.end).max(a),
                    e.new_text.as_str(),
                )
            })
            .collect();
        let starts: Vec<usize> = ranges.iter().map(|(r, _)| r.start).collect();
        let caret = s.cursor();
        let moved: isize = ranges
            .iter()
            .filter(|(r, _)| r.end <= caret)
            .map(|(r, t)| t.len() as isize - r.len() as isize)
            .sum();
        for (r, t) in back_to_front(&starts)
            .into_iter()
            .map(|i| ranges[i].clone())
        {
            s.set_selected_range(r, cx);
            s.replace(t.to_string(), window, cx);
        }
        let caret = (caret as isize + moved).max(0) as usize;
        s.set_selected_range(caret..caret, cx);
    });
}

fn rel_of(root: &Path, path: &Path) -> String {
    crate::uri::rel_path(root, path)
}

use crate::uri::under_root_spelling;

impl EditorWorkspace {
    pub fn new(root: PathBuf, mode: Mode, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let root = simplify(&root.canonicalize().unwrap_or(root));
        let lsp = cx.new(|_| LspStore::new(root.clone()));
        let sub = cx.subscribe_in(
            &lsp,
            window,
            |this: &mut Self, _, ev: &LspStoreEvent, window, cx| match ev {
                LspStoreEvent::DiagnosticsChanged(p) => this.apply_diagnostics(p, window, cx),
                LspStoreEvent::ServersChanged => cx.notify(),
                LspStoreEvent::ApplyEdit {
                    label,
                    edits,
                    reply,
                } => {
                    // A server-driven edit (`workspace/applyEdit`, e.g. a code action's command).
                    let wanted = edits.len();
                    let n = this.apply_file_edits(edits.clone(), window, cx);
                    reply.send(n == wanted);
                    let what = label.as_deref().unwrap_or("언어 서버 편집");
                    this.flash(
                        format!("{what}: 파일 {n}개를 고쳤습니다 (저장 전)"),
                        n != wanted,
                        cx,
                    );
                }
            },
        );
        let mut ws = EditorWorkspace {
            tree: FileTree::new(root.clone()),
            root: root.clone(),
            mode,
            focus: cx.focus_handle(),
            // Tracked handles ignore `.tab_index()`; the tree must be a tab stop itself.
            tree_focus: cx.focus_handle().tab_stop(true),
            prompt_focus: cx.focus_handle(),
            tree_scroll: UniformListScrollHandle::new(),
            buffers: vec![],
            tabs: TabSet::new(),
            tab_scroll: ScrollHandle::new(),
            scroll_active: false,
            scroll_retries: 0,
            tab_overflow: false,
            drop_slot: None,
            tab_menu: None,
            menu_focus: cx.focus_handle(),
            lsp,
            index: IndexSnapshot::default(),
            indexing: false,
            index_cancel: Arc::new(AtomicBool::new(false)),
            recents: Recents::default(),
            overlay: Overlay::None,
            panel_open: false,
            panel_tab: PanelTab::Problems,
            references: vec![],
            references_title: String::new(),
            message: None,
            branch: None,
            git_changes: 0,
            auto_reload_clean: true,
            repo_lsp_dismissed: false,
            defer_lsp: false,
            _watcher: None,
            _tasks: vec![],
            _subs: vec![sub],
            _index_task: Task::ready(()),
            _git_task: Task::ready(()),
            _msg_task: Task::ready(()),
        };
        let focus_in = cx.on_focus_in(&ws.focus, window, |this: &mut Self, _, cx| {
            if std::mem::take(&mut this.defer_lsp)
                && let Some(i) = this.tabs.active()
            {
                this.ensure_lsp(i, cx);
            }
        });
        ws._subs.push(focus_in);
        ws.rebuild_index(cx);
        ws.refresh_git(cx);
        match RootWatcher::start(&root) {
            Ok((w, mut rx)) => {
                ws._watcher = Some(w);
                ws._tasks.push(cx.spawn_in(window, async move |this, cx| {
                    while let Some(batch) = rx.next().await {
                        if this
                            .update_in(cx, |s, window, cx| s.on_fs_batch(batch, window, cx))
                            .is_err()
                        {
                            break;
                        }
                    }
                }));
            }
            Err(e) => ws.message = Some((format!("파일 감시를 시작하지 못했습니다: {e}"), true)),
        }
        ws
    }

    // ---- public API --------------------------------------------------------

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn lsp(&self) -> &Entity<LspStore> {
        &self.lsp
    }
    pub fn mode(&self) -> Mode {
        self.mode
    }
    /// Switch light/dark. The host applies `pod_ui::apply_theme` itself.
    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.mode = mode;
        cx.notify();
    }
    pub fn active_path(&self) -> Option<&Path> {
        self.tabs
            .active()
            .and_then(|i| self.buffers.get(i))
            .map(|b| b.path.as_path())
    }
    pub fn open_paths(&self) -> Vec<PathBuf> {
        self.buffers.iter().map(|b| b.path.clone()).collect()
    }
    pub fn dirty_paths(&self) -> Vec<PathBuf> {
        self.buffers
            .iter()
            .filter(|b| b.dirty)
            .map(|b| b.path.clone())
            .collect()
    }
    /// The active tab's editor state (for host-driven scripting/tests).
    pub fn active_editor(&self) -> Option<Entity<EditorState>> {
        self.tabs
            .active()
            .and_then(|i| self.buffers.get(i))
            .map(|b| b.state.clone())
    }
    pub fn tree(&self) -> &FileTree {
        &self.tree
    }
    pub fn index_len(&self) -> (usize, bool) {
        (self.index.files.len(), self.indexing)
    }

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

    fn drop_tab(&mut self, path: &Path, slot: usize, cx: &mut Context<Self>) {
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

    fn open_tab_menu_at(
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
    fn menu_move(&mut self, delta: isize, cx: &mut Context<Self>) {
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

    fn menu_edge(&mut self, last: bool, cx: &mut Context<Self>) {
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
    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn menu_activate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_editor() {
            Some(st) => st.update(cx, |s, cx| s.focus(window, cx)),
            None => self.tree_focus.focus(window, cx),
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

    pub fn open_quick_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let qo = cx.new(|cx| {
            QuickOpen::new(
                self.index.files.clone(),
                self.recents.items().to_vec(),
                self.indexing,
                self.index.truncated,
                self.mode,
                window,
                cx,
            )
        });
        let sub = cx.subscribe_in(
            &qo,
            window,
            |this: &mut Self, _, ev: &QuickOpenEvent, window, cx| match ev {
                QuickOpenEvent::Open(rel) => {
                    this.close_overlay(window, cx);
                    let p = this.root.join(rel);
                    if let Err(e) = this.preview_file(&p, window, cx) {
                        this.flash(e, true, cx);
                    }
                }
                QuickOpenEvent::Dismiss => this.close_overlay(window, cx),
            },
        );
        self._subs.push(sub);
        qo.update(cx, |q, cx| q.focus(window, cx));
        self.overlay = Overlay::QuickOpen(qo);
        cx.notify();
    }

    /// Sets the quick-open query (scripting; opens it if needed).
    pub fn quick_open_query(&mut self, q: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::QuickOpen(_)) {
            self.open_quick_open(window, cx);
        }
        if let Overlay::QuickOpen(qo) = &self.overlay {
            qo.update(cx, |q2, cx| q2.set_query(q, window, cx));
        }
    }

    pub fn save_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.active() {
            self.save_buffer(i, window, cx);
        }
    }

    pub fn save_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for i in 0..self.buffers.len() {
            if self.buffers[i].dirty {
                self.save_buffer(i, window, cx);
            }
        }
    }

    /// Stops language servers (call before dropping the view / on quit).
    pub fn shutdown(&mut self, cx: &mut Context<Self>) -> Task<()> {
        self.index_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.lsp.update(cx, |s, cx| s.shutdown_all(cx))
    }

    pub fn show_panel(&mut self, references: bool, cx: &mut Context<Self>) {
        self.panel_open = true;
        self.panel_tab = if references {
            PanelTab::References
        } else {
            PanelTab::Problems
        };
        cx.notify();
    }

    /// Open tabs with cursor positions, for the host's layout file.
    pub fn session(&self, cx: &App) -> EditorSession {
        let tabs = self
            .buffers
            .iter()
            .map(|b| {
                let p = b.state.read(cx).cursor_position();
                TabSession {
                    path: b.rel.clone(),
                    line: p.line + 1,
                    col: p.character + 1,
                    preview: self.tabs.is_preview(&TabKey::of(&b.path)),
                }
            })
            .collect();
        let mru = self.tabs.mru_indices();
        EditorSession {
            tabs,
            active: self.tabs.active(),
            mru: if mru.len() > 1 { mru } else { vec![] },
        }
    }

    /// Reopens saved tabs (missing files are skipped) without starting a
    /// language server; the server starts when the view first gets focus.
    pub fn restore_session(
        &mut self,
        session: &EditorSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.defer_lsp = true;
        let mut active = None;
        // Saved index → restored index (skipped files shift the rest).
        let mut restored: Vec<Option<usize>> = vec![None; session.tabs.len()];
        for (i, t) in session.tabs.iter().enumerate() {
            let path = self.root.join(&t.path);
            if self.find_buffer(&path).is_some() {
                continue;
            }
            let Ok(loaded) = text::load(&path) else {
                continue;
            };
            let b = self.make_buffer(&path, loaded, window, cx);
            let pos = lt::Position::new(t.line.saturating_sub(1), t.col.saturating_sub(1));
            b.state
                .update(cx, |s, cx| s.set_cursor_position(pos, window, cx));
            let ix = self.tabs.push(TabKey::of(&b.path), t.preview);
            debug_assert_eq!(ix, self.buffers.len());
            self.buffers.push(b);
            restored[i] = Some(ix);
            cx.emit(EditorEvent::FileOpened(path));
            if session.active == Some(i) {
                active = Some(ix);
            }
        }
        let mru: Vec<usize> = session
            .mru
            .iter()
            .filter_map(|&i| restored.get(i).copied().flatten())
            .collect();
        self.tabs.set_mru_indices(&mru);
        if let Some(ix) = active.or((!self.buffers.is_empty()).then_some(0)) {
            self.activate(ix, window, cx);
            if let Some(p) = self.active_path().map(Path::to_path_buf) {
                self.tree.reveal(&p);
            }
        }
        cx.notify();
    }

    /// Whether the view has an open modal overlay (quick open, prompts).
    pub fn has_overlay(&self) -> bool {
        if self.tab_menu.is_some() {
            return true;
        }
        !matches!(self.overlay, Overlay::None)
    }

    // ---- files ---------------------------------------------------------------

    /// The buffer of `path` in any spelling: buffers under the root are
    /// keyed in the root's canonical spelling, and a host or server may name
    /// the same file through an 8.3 short name or another letter case.
    fn find_buffer(&self, path: &Path) -> Option<usize> {
        let k = path_key(path);
        if let Some(i) = self.buffers.iter().position(|b| path_key(&b.path) == k) {
            return Some(i);
        }
        if !path.is_absolute() {
            return None;
        }
        let k2 = path_key(&under_root_spelling(&self.root, &simplify(path)));
        if k2 == k {
            return None;
        }
        self.buffers.iter().position(|b| path_key(&b.path) == k2)
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
    fn open_tab(
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

    fn make_buffer(
        &mut self,
        path: &Path,
        loaded: text::Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Buffer {
        let language = language_for_path(path);
        let large = loaded.large;
        let undecodable = !loaded.lossless;
        let initial = loaded.text.clone();
        let state = cx.new(|cx| {
            let mut s = EditorState::new(window, cx)
                .line_number(true)
                .searchable(true)
                .soft_wrap(false);
            if let (Some(l), false) = (language, large) {
                s = s.language(l.highlight);
            }
            s.default_value(initial)
        });
        if large || undecodable {
            state.update(cx, |s, cx| s.set_readonly(true, cx));
        }
        if !large && language.is_some() {
            let apply: crate::lsp::providers::ApplyEdits = {
                let ws = cx.entity().downgrade();
                Rc::new(move |edits, window: &mut Window, cx: &mut App| {
                    let _ = ws.update(cx, |ws, cx| {
                        let n = ws.apply_file_edits(edits, window, cx);
                        ws.flash(
                            format!("코드 작업을 적용했습니다: 파일 {n}개 (저장 전)"),
                            false,
                            cx,
                        );
                    });
                })
            };
            let bridge = LspBridge::with_apply(self.lsp.clone(), path.to_path_buf(), Some(apply));
            let ws = cx.entity().downgrade();
            let show: ShowDocumentHandler = Rc::new(
                move |params: &lt::ShowDocumentParams, window: &mut Window, cx: &mut App| {
                    let Some(target) = uri_to_path(&params.uri) else {
                        return false;
                    };
                    let pos = params.selection.map(|r| r.start);
                    let ws = ws.clone();
                    // The editor is mid-update here: defer so a same-file jump can update it.
                    window.defer(cx, move |window, cx| {
                        let _ = ws.update(cx, |ws, cx| {
                            if let Err(e) = ws.open_at(&target, pos, window, cx) {
                                ws.flash(e, true, cx);
                            }
                        });
                    });
                    true
                },
            );
            state.update(cx, |s, cx| {
                let l = s.lsp_mut();
                l.completion_provider = Some(bridge.clone());
                l.hover_provider = Some(bridge.clone());
                l.code_action_providers = vec![bridge.clone()];
                l.definition_provider = Some(bridge);
                l.show_document = Some(show);
                cx.notify();
            });
        }
        let p2 = path.to_path_buf();
        let change = cx.subscribe_in(
            &state,
            window,
            move |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::Change = ev {
                    this.on_buffer_changed(&p2, window, cx);
                }
            },
        );
        // Incremental sync: each edit goes to the store as a ranged change
        // (the debounced full-text sync below stays as a repair path).
        // The same edits keep a snippet session's stops in place, and an
        // inserted completion picks up its tab stops and auto-imports.
        let p3 = path.to_path_buf();
        let lsp = self.lsp.clone();
        let edits = cx.subscribe_in(
            &state,
            window,
            move |this: &mut Self, state, ev: &InputEditsEvent, window, cx| {
                for (e, after) in ev.edits() {
                    let new_text = after.slice(e.start_byte..e.new_end_byte).to_string();
                    lsp.update(cx, |s, _| {
                        s.edit_document(&p3, e.start_byte, e.old_end_byte, &new_text)
                    });
                    this.track_snippet(
                        &p3,
                        snippet::Edit {
                            start: e.start_byte,
                            old_end: e.old_end_byte,
                            new_end: e.new_end_byte,
                        },
                    );
                    let line = after.offset_to_position(e.start_byte).line;
                    if let Some(extra) =
                        lsp.update(cx, |s, _| s.take_completion(&p3, line, &new_text))
                    {
                        this.after_completion(&p3, state.clone(), e.start_byte, extra, window, cx);
                    }
                }
            },
        );
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        let mut b = Buffer {
            path: path.to_path_buf(),
            rel: rel_of(&self.root, path),
            state,
            language,
            format: loaded.format,
            saved: loaded.text,
            dirty: false,
            large,
            undecodable,
            stamp: Some(loaded.stamp),
            save_blocked: false,
            save_failed: None,
            disk: DiskState::Same,
            lsp_open: false,
            _subs: vec![change, edits, observe],
            snippet: None,
            _sync: Task::ready(()),
        };
        // Diagnostics that arrived before the tab existed.
        let diags = self.lsp.read(cx).diagnostics_for(path).to_vec();
        if !diags.is_empty() {
            set_editor_diagnostics(&b.state, diags, cx);
        }
        b.dirty = false;
        b
    }

    fn on_buffer_changed(&mut self, path: &Path, _: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.find_buffer(path) else {
            return;
        };
        // The buffer's own spelling: tab keys and server documents use it.
        let path = &self.buffers[i].path.clone();
        let text = self.buffers[i].state.read(cx).value().to_string();
        let dirty = text != self.buffers[i].saved;
        if dirty != self.buffers[i].dirty {
            self.buffers[i].dirty = dirty;
            cx.emit(EditorEvent::DirtyChanged {
                path: path.to_path_buf(),
                dirty,
            });
        }
        // Editing a preview tab keeps it.
        if dirty && self.tabs.pin(&TabKey::of(path)) {
            cx.emit(EditorEvent::TabsChanged);
        }
        if self.buffers[i].language.is_some() && !self.buffers[i].large {
            // Debounced full-text sync; providers flush synchronously anyway.
            let lsp = self.lsp.clone();
            let p = path.to_path_buf();
            self.buffers[i]._sync = cx.spawn(async move |_, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(120))
                    .await;
                lsp.update(cx, |s, _| s.change_document(&p, text));
            });
        }
        cx.notify();
    }

    /// Answers the active tab's save-failure banner: retry the atomic save,
    /// or (`overwrite`) write the file in place, not atomically.
    pub fn answer_save_failure(
        &mut self,
        overwrite: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = self.tabs.active() {
            self.save_buffer_ex(i, overwrite, window, cx);
        }
    }

    fn dismiss_save_failure(&mut self, cx: &mut Context<Self>) {
        if let Some(b) = self.tabs.active().and_then(|i| self.buffers.get_mut(i)) {
            b.save_failed = None;
        }
        cx.notify();
    }

    fn save_buffer(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.save_buffer_ex(i, false, window, cx);
    }

    /// `in_place`: the user chose to overwrite after an atomic replace
    /// failed; otherwise always the atomic write (never a silent fallback).
    fn save_buffer_ex(
        &mut self,
        i: usize,
        in_place: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.buffers[i].save_failed = None;
        let b = &self.buffers[i];
        let path = b.path.clone();
        if b.undecodable || b.large {
            self.flash(
                format!("읽기 전용으로 연 파일은 저장하지 않습니다: {}", b.rel),
                true,
                cx,
            );
            return;
        }
        let text = b.state.read(cx).value().to_string();
        let bytes = match text::encode(&text, &b.format) {
            Ok(bytes) => bytes,
            Err(e) => {
                self.flash(format!("저장하지 못했습니다: {e}"), true, cx);
                return;
            }
        };
        // Never overwrite a change on disk the buffer has not seen (an agent
        // may have written the file just now, before the watcher said so).
        match text::disk_stamp(&path) {
            Ok(now) if now != b.stamp => {
                self.block_save(i, now.is_none(), window, cx);
                return;
            }
            Ok(_) => {}
            Err(e) => {
                self.flash(format!("저장하지 못했습니다: {e}"), true, cx);
                return;
            }
        }
        let written = if in_place {
            text::write_in_place(&path, &bytes)
        } else {
            text::write_atomic(&path, &bytes)
        };
        match written {
            Ok(()) => {
                let encoding = self.buffers[i].format.encoding;
                let b = &mut self.buffers[i];
                b.stamp = Some(text::stamp_of(&bytes));
                // The written bytes are the new base for mixed line endings.
                if let Ok(l) = text::decode_as(bytes, 0, encoding) {
                    b.format = l.format;
                }
                b.saved = text.clone();
                b.save_blocked = false;
                b.disk = DiskState::Same;
                if b.dirty {
                    b.dirty = false;
                    cx.emit(EditorEvent::DirtyChanged {
                        path: path.clone(),
                        dirty: false,
                    });
                }
                self.lsp.update(cx, |s, _| {
                    s.change_document(&path, text);
                    s.save_document(&path);
                });
                cx.emit(EditorEvent::FileSaved(path.clone()));
                let rel = self.buffers[i].rel.clone();
                self.flash(format!("저장했습니다: {rel}"), false, cx);
            }
            Err(e) => {
                self.buffers[i].save_failed = Some(SaveFailure {
                    message: e.to_string(),
                    replace: text::is_replace_failure(&e),
                });
                self.flash(format!("저장하지 못했습니다: {e}"), true, cx);
            }
        }
        cx.notify();
    }

    /// A save found the disk changed since the buffer read it: nothing is
    /// written; the banner offers reload, overwrite or a diff.
    fn block_save(&mut self, i: usize, deleted: bool, _: &mut Window, cx: &mut Context<Self>) {
        let disk = if deleted {
            DiskState::Deleted
        } else {
            match text::load_as(&self.buffers[i].path, self.buffers[i].format.encoding) {
                Ok(l) => DiskState::Changed(l.text),
                Err(e) => {
                    self.flash(
                        format!("저장하지 않았습니다: 디스크의 파일을 다시 읽지 못했습니다 ({e})"),
                        true,
                        cx,
                    );
                    return;
                }
            }
        };
        let b = &mut self.buffers[i];
        b.save_blocked = true;
        b.disk = disk;
        let rel = b.rel.clone();
        self.flash(
            format!("저장하지 않았습니다: 디스크의 {rel}이(가) 열어 둔 뒤 바뀌었습니다"),
            true,
            cx,
        );
        cx.notify();
    }

    /// Reopens tab `i` reading its bytes in `encoding` (editor-02). It
    /// becomes editable only when the bytes decode without loss.
    pub fn reopen_with_encoding(
        &mut self,
        i: usize,
        encoding: &'static text::Encoding,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(b) = self.buffers.get(i) else { return };
        if b.dirty {
            self.flash("저장하지 않은 변경이 있어 다시 열 수 없습니다", true, cx);
            return;
        }
        match text::load_as(&b.path, encoding) {
            Ok(l) if l.lossless => {
                self.reload_buffer(i, l, window, cx);
                self.flash(
                    format!("{}(으)로 다시 열었습니다", encoding.name()),
                    false,
                    cx,
                );
            }
            Ok(_) => self.flash(
                format!(
                    "{}(으)로도 손실 없이 읽을 수 없습니다. 읽기 전용으로 둡니다",
                    encoding.name()
                ),
                true,
                cx,
            ),
            Err(e) => self.flash(e.to_string(), true, cx),
        }
    }

    /// The user approved the `.pod/lsp.json` content shown in the banner.
    pub fn trust_repo_lsp(&mut self, hash: &str, cx: &mut Context<Self>) {
        match self.lsp.update(cx, |s, cx| s.trust_repo_config(hash, cx)) {
            Ok(()) => self.flash(
                "저장소 언어 서버 설정을 승인했습니다. 서버를 다시 시작합니다",
                false,
                cx,
            ),
            Err(e) => self.flash(e, true, cx),
        }
        cx.notify();
    }

    /// Shows the line diff between the file on disk and the active buffer.
    pub fn show_disk_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.tabs.active() else { return };
        let b = &self.buffers[i];
        let disk = match &b.disk {
            DiskState::Changed(t) => t.clone(),
            DiskState::Deleted => String::new(),
            DiskState::Same => b.saved.clone(),
        };
        let mine = b.state.read(cx).value().to_string();
        let lines = disk_diff_lines(&disk, &mine);
        self.overlay = Overlay::DiskDiff {
            rel: b.rel.clone(),
            lines,
        };
        self.prompt_focus.focus(window, cx);
        cx.notify();
    }

    /// Lines of the open diff overlay (`-` disk, `+` buffer), for tests.
    pub fn disk_diff(&self) -> Option<Vec<(char, String)>> {
        match &self.overlay {
            Overlay::DiskDiff { lines, .. } => Some(lines.clone()),
            _ => None,
        }
    }

    /// Answers the active tab's disk banner: `reload` takes the disk
    /// version; otherwise keep the buffer (and, after a refused save,
    /// overwrite the file now).
    pub fn answer_disk_prompt(
        &mut self,
        reload: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resolve_disk_prompt(reload, window, cx);
    }

    fn activate(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) {
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

    /// Releases a buffer that left the tab strip: the language server
    /// closes the document (`didClose`) and the host hears about it.
    fn drop_buffer(&mut self, b: Buffer, cx: &mut Context<Self>) {
        if b.dirty {
            cx.emit(EditorEvent::DirtyChanged {
                path: b.path.clone(),
                dirty: false,
            });
        }
        self.lsp.update(cx, |s, _| s.close_document(&b.path));
        cx.emit(EditorEvent::FileClosed(b.path.clone()));
    }

    /// Registers buffer `ix` with its language server (starting it on first
    /// use), or marks the server used so the host's LRU cap keeps it.
    fn ensure_lsp(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(b) = self.buffers.get_mut(ix) else {
            return;
        };
        if b.language.is_none() || b.large {
            return;
        }
        let path = b.path.clone();
        if b.lsp_open {
            self.lsp.update(cx, |s, cx| s.touch(&path, cx));
            return;
        }
        b.lsp_open = true;
        let text_now = b.state.read(cx).value().to_string();
        self.lsp
            .update(cx, |s, cx| s.open_document(&path, text_now, cx));
    }

    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
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

    fn resolve_close(&mut self, save: Option<bool>, window: &mut Window, cx: &mut Context<Self>) {
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
    fn remove_tab_ex(
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

    fn flash(&mut self, msg: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.message = Some((msg.into(), error));
        self._msg_task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(if error { 8 } else { 4 }))
                .await;
            let _ = this.update(cx, |s, cx| {
                s.message = None;
                cx.notify();
            });
        });
        cx.notify();
    }

    // ---- background: index, git, watcher ------------------------------------

    fn rebuild_index(&mut self, cx: &mut Context<Self>) {
        self.index_cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        self.index_cancel = cancel.clone();
        self.indexing = true;
        let root = self.root.clone();
        let job = cx.background_spawn(async move { index::build(&root, &cancel) });
        self._index_task = cx.spawn(async move |this, cx| {
            let snap = job.await;
            let _ = this.update(cx, |s, cx| {
                s.index = snap;
                s.indexing = false;
                if let Overlay::QuickOpen(qo) = &s.overlay {
                    let (files, tr) = (s.index.files.clone(), s.index.truncated);
                    qo.update(cx, |q, cx| q.set_files(files, false, tr, cx));
                }
                cx.notify();
            });
        });
    }

    fn refresh_git(&mut self, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let job = cx.background_spawn(async move { git::read_status(&root) });
        self._git_task = cx.spawn(async move |this, cx| {
            let st = job.await;
            let _ = this.update(cx, |s, cx| {
                s.branch = st.branch.clone();
                s.git_changes = st.changed_count();
                s.tree.set_git(st);
                cx.notify();
            });
        });
    }

    fn on_fs_batch(&mut self, batch: Batch, window: &mut Window, cx: &mut Context<Self>) {
        if !batch.paths.is_empty() {
            if batch.paths.iter().any(|p| p == &self.root) {
                self.tree.refresh_all();
            } else {
                self.tree.refresh_paths(&batch.paths);
            }
            // Re-index on creations/deletions only, decided off the UI
            // thread (a build can report thousands of paths at once).
            let lookup = self.index.lookup.clone();
            let paths = batch.paths.clone();
            let root = self.root.clone();
            let job = cx.background_spawn(async move {
                index::is_structural(&paths, &lookup, |p| rel_of(&root, p))
            });
            cx.spawn(async move |this, cx| {
                if job.await {
                    let _ = this.update(cx, |s, cx| s.rebuild_index(cx));
                }
            })
            .detach();
            // A changed `.pod/lsp.json` is checked against the approvals again.
            let lsp_json = path_key(&self.root.join(".pod").join("lsp.json"));
            if batch.paths.iter().any(|p| path_key(p) == lsp_json) {
                let dir = crate::lsp::registry::user_config_dir();
                self.lsp
                    .update(cx, |s, cx| s.reload_registry(dir.as_deref(), cx));
                self.repo_lsp_dismissed = false;
            }
            for p in &batch.paths {
                if let Some(i) = self.find_buffer(p) {
                    self.check_disk(i, window, cx);
                }
            }
        }
        self.refresh_git(cx);
        cx.notify();
    }

    /// Compares an open buffer with the file on disk after a change event.
    fn check_disk(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.buffers[i].path.clone();
        if !path.exists() {
            self.buffers[i].disk = DiskState::Deleted;
            cx.emit(EditorEvent::ExternalChange {
                path,
                reloaded: false,
            });
            return;
        }
        let Ok(loaded) = text::load_as(&path, self.buffers[i].format.encoding) else {
            return;
        };
        let b = &mut self.buffers[i];
        if loaded.text == b.saved {
            // Our own save, a touch, or only the line endings changed: the
            // disk bytes become the base for the next save.
            b.disk = DiskState::Same;
            b.stamp = Some(loaded.stamp);
            b.format = loaded.format;
            b.save_blocked = false;
            return;
        }
        if !b.dirty && self.auto_reload_clean {
            self.reload_buffer(i, loaded, window, cx);
            let rel = self.buffers[i].rel.clone();
            self.flash(
                format!("디스크에서 바뀐 내용을 다시 불러왔습니다: {rel}"),
                false,
                cx,
            );
            cx.emit(EditorEvent::ExternalChange {
                path,
                reloaded: true,
            });
        } else {
            b.disk = DiskState::Changed(loaded.text);
            cx.emit(EditorEvent::ExternalChange {
                path,
                reloaded: false,
            });
        }
    }

    fn reload_buffer(
        &mut self,
        i: usize,
        loaded: text::Loaded,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let b = &mut self.buffers[i];
        b.format = loaded.format;
        b.stamp = Some(loaded.stamp);
        b.undecodable = !loaded.lossless;
        b.save_blocked = false;
        let read_only = b.large || b.undecodable;
        b.saved = loaded.text.clone();
        b.disk = DiskState::Same;
        let was_dirty = b.dirty;
        b.dirty = false;
        let state = b.state.clone();
        let path = b.path.clone();
        let text_v = loaded.text.clone();
        state.update(cx, |s, cx| {
            let pos = s.cursor_position();
            s.set_readonly(false, cx);
            s.set_value(text_v, window, cx);
            s.set_cursor_position(pos, window, cx);
            s.set_readonly(read_only, cx);
        });
        self.lsp
            .update(cx, |s, _| s.change_document(&path, loaded.text));
        if was_dirty {
            cx.emit(EditorEvent::DirtyChanged { path, dirty: false });
        }
        cx.notify();
    }

    fn resolve_disk_prompt(&mut self, reload: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.tabs.active() else { return };
        let blocked = std::mem::take(&mut self.buffers[i].save_blocked);
        let path = self.buffers[i].path.clone();
        match std::mem::replace(&mut self.buffers[i].disk, DiskState::Same) {
            DiskState::Changed(_) if reload => {
                if let Ok(l) = text::load_as(&path, self.buffers[i].format.encoding) {
                    self.reload_buffer(i, l, window, cx);
                }
            }
            DiskState::Changed(disk_text) => {
                // Keep mine: the disk version becomes the comparison base so
                // the buffer stays dirty and saving overwrites it (an
                // explicit choice, so the current disk bytes are accepted).
                self.buffers[i].saved = disk_text;
                self.buffers[i].stamp = text::disk_stamp(&path).ok().flatten();
            }
            DiskState::Deleted if reload => {
                self.buffers[i].dirty = false;
                self.remove_tab_ex(i, false, window, cx);
            }
            DiskState::Deleted => {
                self.buffers[i].saved = String::new();
                self.buffers[i].dirty = true;
                self.buffers[i].stamp = text::disk_stamp(&path).ok().flatten();
            }
            DiskState::Same => {}
        }
        if blocked && !reload && self.find_buffer(&path) == Some(i) {
            self.save_buffer(i, window, cx);
        }
        cx.notify();
    }

    // ---- diagnostics ---------------------------------------------------------

    fn apply_diagnostics(&mut self, path: &Path, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.find_buffer(path) {
            let diags = self.lsp.read(cx).diagnostics_for(path).to_vec();
            set_editor_diagnostics(&self.buffers[i].state, diags, cx);
        }
        cx.notify();
    }

    fn next_problem(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut all: Vec<(PathBuf, lt::Position)> = vec![];
        for f in self.lsp.read(cx).diagnostics() {
            for d in &f.items {
                all.push((f.path.clone(), d.range.start));
            }
        }
        if all.is_empty() {
            self.flash("문제가 없습니다", false, cx);
            return;
        }
        all.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then((a.1.line, a.1.character).cmp(&(b.1.line, b.1.character)))
        });
        let cur = self.tabs.active().map(|i| {
            (
                self.buffers[i].path.clone(),
                self.buffers[i].state.read(cx).cursor_position(),
            )
        });
        let next = cur
            .and_then(|(p, c)| {
                all.iter()
                    .find(|(fp, pos)| {
                        (path_key(fp) == path_key(&p)
                            && (pos.line, pos.character) > (c.line, c.character))
                            || fp > &p
                    })
                    .cloned()
            })
            .unwrap_or_else(|| all[0].clone());
        let _ = self.open_at(&next.0, Some(next.1), window, cx);
    }

    // ---- LSP actions -----------------------------------------------------------

    fn cursor_target(&self, cx: &App) -> Option<(PathBuf, lt::Position, Entity<EditorState>)> {
        let b = self.buffers.get(self.tabs.active()?)?;
        let st = b.state.read(cx);
        Some((
            b.path.clone(),
            st.text().offset_to_position(st.cursor()),
            b.state.clone(),
        ))
    }

    fn flush_active(&mut self, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.active() {
            let text = self.buffers[i].state.read(cx).value().to_string();
            let p = self.buffers[i].path.clone();
            self.lsp.update(cx, |s, _| s.change_document(&p, text));
        }
    }

    pub fn find_references(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, pos, _)) = self.cursor_target(cx) else {
            return;
        };
        self.flush_active(cx);
        let task = self.lsp.update(cx, |s, cx| s.references(&path, pos, cx));
        let word = self.word_at_cursor(cx).unwrap_or_default();
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |s, cx| match res {
                Ok(v) => {
                    s.references_title = format!("`{word}` 참조 {}개", v.len());
                    s.references = v;
                    s.show_panel(true, cx);
                }
                Err(e) => s.flash(format!("참조를 찾지 못했습니다: {e}"), true, cx),
            });
        }));
    }

    fn word_at_cursor(&self, cx: &App) -> Option<String> {
        let b = self.buffers.get(self.tabs.active()?)?;
        let st = b.state.read(cx);
        let text: &Rope = st.text();
        let r = text.word_range(st.cursor())?;
        Some(text.slice(r).to_string())
    }

    pub fn start_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((path, pos, _)) = self.cursor_target(cx) else {
            return;
        };
        let Some(word) = self.word_at_cursor(cx) else {
            self.flash("커서 위치에 이름이 없습니다", true, cx);
            return;
        };
        let input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("새 이름")
                .default_value(word)
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.request_rename(window, cx);
                }
            },
        );
        self._subs.push(sub);
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        self.overlay = Overlay::Rename { input, path, pos };
        cx.notify();
    }

    fn request_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Rename { input, path, pos } = &self.overlay else {
            return;
        };
        let new_name = input.read(cx).value().trim().to_string();
        if new_name.is_empty() {
            return;
        }
        let (path, pos) = (path.clone(), *pos);
        self.flush_active(cx);
        let task = self
            .lsp
            .update(cx, |s, cx| s.rename(&path, pos, new_name.clone(), cx));
        self._tasks.push(cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |s, window, cx| match res {
                Ok(edits) if edits.iter().all(|f| f.edits.is_empty()) => {
                    s.close_overlay(window, cx);
                    s.flash("바꿀 곳이 없습니다", false, cx);
                }
                Ok(edits) => {
                    s.overlay = Overlay::RenamePreview { new_name, edits };
                    s.focus.focus(window, cx);
                    cx.notify();
                }
                Err(e) => {
                    s.close_overlay(window, cx);
                    s.flash(format!("이름을 바꾸지 못했습니다: {e}"), true, cx);
                }
            });
        }));
    }

    /// Applies the previewed rename to buffers (opening files as needed).
    /// Files end up dirty; the user reviews and saves (Ctrl+K S saves all).
    pub fn apply_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::RenamePreview { edits, .. } =
            std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return;
        };
        let changed = self.apply_file_edits(edits, window, cx);
        self.flash(
            format!("파일 {changed}개에서 이름을 바꿨습니다 (저장 전)"),
            false,
            cx,
        );
        cx.notify();
    }

    /// Applies LSP file edits (rename, code actions) to buffers, opening
    /// files as needed; returns how many files changed. Files end up dirty.
    pub fn apply_file_edits(
        &mut self,
        edits: Vec<FileEdit>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let mut changed = 0;
        let keep_active = self.active_path().map(Path::to_path_buf);
        for fe in edits {
            if self.find_buffer(&fe.path).is_none()
                && self.open_at(&fe.path, None, window, cx).is_err()
            {
                continue;
            }
            let Some(i) = self.find_buffer(&fe.path) else {
                continue;
            };
            if self.buffers[i].undecodable || self.buffers[i].large {
                continue; // read-only: an edit could not be saved without loss
            }
            let st = self.buffers[i].state.clone();
            edit_buffer(&st, &fe.edits, window, cx);
            self.on_buffer_changed(&fe.path, window, cx);
            changed += 1;
        }
        if let Some(a) = keep_active.and_then(|p| self.find_buffer(&p)) {
            self.activate(a, window, cx);
        }
        cx.notify();
        changed
    }

    /// Moves a snippet session's stops with an edit; ends it when the edit
    /// cuts across a stop.
    fn track_snippet(&mut self, path: &Path, edit: snippet::Edit) {
        if let Some(i) = self.find_buffer(path)
            && let Some(s) = self.buffers[i].snippet.as_mut()
            && !s.apply(edit)
        {
            self.buffers[i].snippet = None;
        }
    }

    /// A completion item was just inserted at byte `at`: start its tab-stop
    /// session and add its auto-imports (`additionalTextEdits`, resolved
    /// now if the server sends them lazily).
    fn after_completion(
        &mut self,
        path: &Path,
        state: Entity<EditorState>,
        at: usize,
        extra: CompletionExtra,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(i) = self.find_buffer(path) else {
            return;
        };
        self.buffers[i].snippet = extra
            .snippet
            .as_ref()
            .and_then(|s| SnippetSession::new(s, at));
        if !extra.additional.is_empty() {
            // Edits above the insertion shift the session through `track_snippet`.
            edit_buffer(&state, &extra.additional, window, cx);
        } else if let Some(item) = extra.resolve {
            let task = self
                .lsp
                .update(cx, |s, cx| s.resolve_additional_edits(path, item, cx));
            cx.spawn_in(window, async move |this, cx| {
                if let Ok(edits) = task.await
                    && !edits.is_empty()
                {
                    let _ =
                        this.update_in(cx, |_, window, cx| edit_buffer(&state, &edits, window, cx));
                }
            })
            .detach();
        }
        if self.buffers[i].snippet.is_some() {
            // After the queued edit events (auto-import) have moved the stops.
            let ws = cx.entity().downgrade();
            let p = path.to_path_buf();
            window.defer(cx, move |_, cx| {
                let _ = ws.update(cx, |ws, cx| ws.select_snippet_stop(&p, cx));
            });
        }
    }

    /// Selects the session's current stop; the final stop (`$0`) only
    /// places the caret and ends the session.
    fn select_snippet_stop(&mut self, path: &Path, cx: &mut Context<Self>) {
        let Some(i) = self.find_buffer(path) else {
            return;
        };
        let Some(s) = &self.buffers[i].snippet else {
            return;
        };
        let (range, last) = (s.current_range(), s.at_last());
        self.buffers[i]
            .state
            .update(cx, |st, cx| st.set_selected_range(range, cx));
        if last {
            self.buffers[i].snippet = None;
        }
        cx.notify();
    }

    /// Tab / Shift+Tab inside a snippet session: next / previous stop.
    /// `false` (the key does its usual indent) when there is no session, a
    /// completion menu is open, or the caret has left the current stop.
    fn snippet_step(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(i) = self.tabs.active() else {
            return false;
        };
        let st = self.buffers[i].state.read(cx);
        let (menu_open, sel) = (st.completion_menu_state().open, st.selected_range());
        let Some(s) = self.buffers[i].snippet.as_mut() else {
            return false;
        };
        if menu_open {
            return false;
        }
        let cur = s.current_range();
        if sel.start < cur.start || sel.end > cur.end {
            self.buffers[i].snippet = None;
            return false;
        }
        if s.step(forward).is_none() {
            self.buffers[i].snippet = None;
            return false;
        }
        let path = self.buffers[i].path.clone();
        self.select_snippet_stop(&path, cx);
        true
    }

    /// Whether the active tab has a snippet session (tests, status).
    pub fn snippet_active(&self) -> bool {
        self.tabs
            .active()
            .and_then(|i| self.buffers.get(i))
            .is_some_and(|b| b.snippet.is_some())
    }

    fn close_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay = Overlay::None;
        if let Some(st) = self.active_editor() {
            st.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.tree_focus.focus(window, cx);
        }
        cx.notify();
    }

    // ---- action handlers -------------------------------------------------------

    fn on_quick_open(&mut self, _: &QuickOpenFile, window: &mut Window, cx: &mut Context<Self>) {
        self.open_quick_open(window, cx);
    }
    fn on_save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        self.save_active(window, cx);
    }
    fn on_save_all(&mut self, _: &SaveAll, window: &mut Window, cx: &mut Context<Self>) {
        self.save_all(window, cx);
    }
    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        self.close_active(window, cx);
    }
    fn on_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_mru(true, window, cx);
    }
    fn on_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.switch_mru(false, window, cx);
    }
    fn on_next_tab_position(
        &mut self,
        _: &NextTabPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_tab(true, window, cx);
    }
    fn on_prev_tab_position(
        &mut self,
        _: &PrevTabPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.step_tab(false, window, cx);
    }
    fn on_reopen_closed(
        &mut self,
        _: &ReopenClosedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reopen_closed(window, cx);
    }
    fn on_move_tab_left(&mut self, _: &MoveTabLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_active_tab(false, cx);
    }
    fn on_move_tab_right(&mut self, _: &MoveTabRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_active_tab(true, cx);
    }
    fn on_tab_menu(&mut self, _: &TabMenu, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab_menu(None, window, cx);
    }
    fn on_pin_tab(&mut self, _: &PinTab, _: &mut Window, cx: &mut Context<Self>) {
        self.pin_active(cx);
    }
    fn on_goto_line(&mut self, _: &GotoLine, window: &mut Window, cx: &mut Context<Self>) {
        self.open_goto_line(window, cx);
    }
    pub fn open_goto_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.active().is_none() {
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("줄 번호 또는 줄:열 (예: 42:5)"));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this: &mut Self, inp, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let v = inp.read(cx).value().to_string();
                    this.close_overlay(window, cx);
                    if let Some(pos) = parse_line_col(&v)
                        && let Some(st) = this.active_editor()
                    {
                        st.update(cx, |s, cx| s.set_cursor_position(pos, window, cx));
                    }
                }
            },
        );
        self._subs.push(sub);
        input.update(cx, |s, cx| s.focus(window, cx));
        self.overlay = Overlay::GotoLine(input);
        cx.notify();
    }
    fn on_find_references(
        &mut self,
        _: &FindReferences,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.find_references(window, cx);
    }
    fn on_rename(&mut self, _: &RenameSymbol, window: &mut Window, cx: &mut Context<Self>) {
        self.start_rename(window, cx);
    }
    fn on_next_problem(&mut self, _: &NextProblem, window: &mut Window, cx: &mut Context<Self>) {
        self.next_problem(window, cx);
    }
    fn on_toggle_panel(&mut self, _: &TogglePanel, _: &mut Window, cx: &mut Context<Self>) {
        self.panel_open = !self.panel_open;
        cx.notify();
    }
    fn on_focus_tree(&mut self, _: &FocusTree, window: &mut Window, cx: &mut Context<Self>) {
        self.tree_focus.focus(window, cx);
        if self.tree.selected().is_none() {
            self.tree.move_home();
        }
        cx.notify();
    }
    fn on_focus_editor(&mut self, _: &FocusEditor, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(st) = self.active_editor() {
            st.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    fn tree_moved(&mut self, cx: &mut Context<Self>) {
        if let Some(i) = self.tree.selected_index() {
            self.tree_scroll.scroll_to_item(i, ScrollStrategy::Nearest);
        }
        cx.notify();
    }
    fn on_tree_up(&mut self, _: &TreeUp, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_up();
        self.tree_moved(cx);
    }
    fn on_tree_down(&mut self, _: &TreeDown, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_down();
        self.tree_moved(cx);
    }
    fn on_tree_left(&mut self, _: &TreeLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_left();
        self.tree_moved(cx);
    }
    fn on_tree_right(&mut self, _: &TreeRight, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_right();
        self.tree_moved(cx);
    }
    fn on_tree_home(&mut self, _: &TreeHome, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_home();
        self.tree_moved(cx);
    }
    fn on_tree_end(&mut self, _: &TreeEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.tree.move_end();
        self.tree_moved(cx);
    }
    fn on_tree_open(&mut self, _: &TreeOpen, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tree.selected_index() {
            self.tree_activate(i, false, false, window, cx);
        }
    }
    fn on_tree_preview(&mut self, _: &TreePreview, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(i) = self.tree.selected_index() {
            self.tree_activate(i, false, true, window, cx);
        }
    }
    fn tree_target(&self) -> Option<(PathBuf, bool)> {
        let i = self.tree.selected_index()?;
        let r = self.tree.rows().get(i)?;
        Some((r.path.clone(), r.is_dir))
    }
    fn on_tree_new_file(&mut self, _: &TreeNewFile, window: &mut Window, cx: &mut Context<Self>) {
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
    fn on_tree_rename(&mut self, _: &TreeRename, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((p, _)) = self.tree_target() {
            self.start_rename_entry(p, window, cx);
        }
    }
    fn on_tree_delete(&mut self, _: &TreeDelete, window: &mut Window, cx: &mut Context<Self>) {
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

    fn commit_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn tree_activate(
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

    // ---- rendering ------------------------------------------------------------

    fn render_tree(
        &mut self,
        t: Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focused = self.tree_focus.contains_focused(window, cx);
        let mode = self.mode;
        let root_name = self
            .root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let count = self.tree.rows().len();
        div()
            .id("pod-editor-tree")
            .track_focus(&self.tree_focus)
            .key_context(TREE_CTX)
            .on_action(cx.listener(Self::on_tree_up))
            .on_action(cx.listener(Self::on_tree_down))
            .on_action(cx.listener(Self::on_tree_left))
            .on_action(cx.listener(Self::on_tree_right))
            .on_action(cx.listener(Self::on_tree_open))
            .on_action(cx.listener(Self::on_tree_preview))
            .on_action(cx.listener(Self::on_tree_home))
            .on_action(cx.listener(Self::on_tree_end))
            .on_action(cx.listener(Self::on_tree_new_file))
            .on_action(cx.listener(Self::on_tree_rename))
            .on_action(cx.listener(Self::on_tree_delete))
            .tab_index(0)
            .role(Role::Tree)
            .aria_label(txt::TREE_LABEL)
            .focus_visible(move |st| pod_ui::focus_style(st, mode))
            .w(px(TREE_W))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .bg(tc(t.bg_chrome))
            .border_r_1()
            .border_color(tc(t.border_subtle))
            .child(
                div()
                    .h(px(32.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tc(t.text_primary))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(root_name),
                    )
                    .child(div().text_size(px(12.)).text_color(tc(t.text_muted)).child(
                        if self.git_changes > 0 {
                            format!("변경 {}", self.git_changes)
                        } else {
                            String::new()
                        },
                    )),
            )
            .child(
                uniform_list(
                    "pod-editor-tree-rows",
                    count,
                    cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                        let t = Tokens::for_mode(this.mode);
                        let sel = this.tree.selected_index();
                        let active_key = this.active_path().map(path_key);
                        range
                            .map(|ix| {
                                let r = &this.tree.rows()[ix];
                                let is_sel = sel == Some(ix);
                                let is_active =
                                    active_key.as_deref() == Some(path_key(&r.path).as_str());
                                let mark_color = r.mark.map(|m| mark_color(m, t));
                                let (row_path, row_dir) = (r.path.clone(), r.is_dir);
                                let ws = cx.entity().downgrade();
                                let row_label = match r.mark {
                                    Some(m) if !r.is_dir => format!("{} ({})", r.name, m.letter()),
                                    _ => r.name.clone(),
                                };
                                let name_color = mark_color.unwrap_or(t.text_primary);
                                let chevron = if r.is_dir {
                                    let mut s =
                                        icon(Icon::ChevronRight, t.text_secondary).size(px(14.));
                                    if r.expanded {
                                        s = s.with_transformation(Transformation::rotate(radians(
                                            std::f32::consts::FRAC_PI_2,
                                        )));
                                    }
                                    div()
                                        .w(px(16.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(s)
                                        .into_any_element()
                                } else {
                                    div().w(px(16.)).flex_none().into_any_element()
                                };
                                div()
                                    .id(("tree-row", ix))
                                    .role(Role::TreeItem)
                                    .aria_label(row_label)
                                    .aria_selected(is_sel)
                                    .w_full()
                                    .h(px(TREE_ROW_H))
                                    .pl(px(8. + r.depth as f32 * pod_ui::tokens::TREE_INDENT))
                                    .pr(px(8.))
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .cursor_pointer()
                                    .text_size(px(13.))
                                    .when(is_sel, |d| d.bg(tc(t.bg_selected)))
                                    .when(is_sel && focused, |d| {
                                        d.border_1().border_color(tc(t.accent_fg))
                                    })
                                    .when(!is_sel, |d| d.hover(|d| d.bg(tc(t.bg_hover))))
                                    .on_click(cx.listener(
                                        move |this, e: &ClickEvent, window, cx| {
                                            this.tree_focus.focus(window, cx);
                                            let double = e.click_count() > 1;
                                            this.tree_activate(ix, double, !double, window, cx);
                                        },
                                    ))
                                    .context_menu(move |menu, _, _| {
                                        let dir = if row_dir {
                                            row_path.clone()
                                        } else {
                                            row_path
                                                .parent()
                                                .map(Path::to_path_buf)
                                                .unwrap_or_else(|| row_path.clone())
                                        };
                                        let (w1, w2, w3, w4) =
                                            (ws.clone(), ws.clone(), ws.clone(), ws.clone());
                                        let (d1, d2, p3, p4) =
                                            (dir.clone(), dir, row_path.clone(), row_path.clone());
                                        menu.item(PopupMenuItem::new(txt::NEW_FILE).on_click(
                                            move |_, window, cx| {
                                                let _ = w1.update(cx, |w, cx| {
                                                    w.start_new_entry(d1.clone(), false, window, cx)
                                                });
                                            },
                                        ))
                                        .item(PopupMenuItem::new(txt::NEW_FOLDER).on_click(
                                            move |_, window, cx| {
                                                let _ = w2.update(cx, |w, cx| {
                                                    w.start_new_entry(d2.clone(), true, window, cx)
                                                });
                                            },
                                        ))
                                        .separator()
                                        .item(PopupMenuItem::new(txt::RENAME).on_click(
                                            move |_, window, cx| {
                                                let _ = w3.update(cx, |w, cx| {
                                                    w.start_rename_entry(p3.clone(), window, cx)
                                                });
                                            },
                                        ))
                                        .item(
                                            PopupMenuItem::new(txt::DELETE).on_click(
                                                move |_, window, cx| {
                                                    let _ = w4.update(cx, |w, cx| {
                                                        w.overlay =
                                                            Overlay::ConfirmDelete(p4.clone());
                                                        w.prompt_focus.focus(window, cx);
                                                        cx.notify();
                                                    });
                                                },
                                            ),
                                        )
                                    })
                                    .child(chevron)
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_color(tc(name_color))
                                            .when(is_active, |d| {
                                                d.font_weight(FontWeight::SEMIBOLD)
                                            })
                                            .child(r.name.clone()),
                                    )
                                    .when_some(r.mark, |d, m| {
                                        d.child(
                                            div()
                                                .flex_none()
                                                .w(px(14.))
                                                .text_size(px(12.))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .text_color(tc(mark_color.unwrap_or(t.text_muted)))
                                                .child(if r.is_dir { "•" } else { m.letter() }),
                                        )
                                    })
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.tree_scroll)
                .flex_1(),
            )
            .into_any_element()
    }

    fn render_tabs(
        &mut self,
        t: Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let n = self.buffers.len();
        let mode = self.mode;
        if std::mem::take(&mut self.scroll_active)
            && let Some(a) = self.tabs.active()
        {
            self.tab_scroll.scroll_to_item(a);
            self.scroll_retries = 2;
        }
        // Overflow, from the last layout. While the button shows, the strip
        // is narrower by its width, so that width is the threshold (no flicker).
        let overflowing = |this: &Self| {
            let over = this.tab_scroll.max_offset().x;
            this.buffers.len() > 1
                && if this.tab_overflow {
                    over > px(OVERFLOW_BTN_W)
                } else {
                    over > px(0.5)
                }
        };
        self.tab_overflow = overflowing(self);
        if n > 0 {
            // Layout changes (a new tab, a resize) can start or end overflow.
            // Deferred: runs after this frame's layout updated the scroll handle.
            cx.defer_in(window, move |this, _, cx| {
                if overflowing(this) != this.tab_overflow {
                    cx.notify();
                }
                // Not fully visible after a scroll request (the strip's
                // width changed meanwhile, e.g. the button appeared): again.
                if this.scroll_retries > 0 {
                    this.scroll_retries -= 1;
                    if this.tabs.active().is_some_and(|a| !this.tab_in_view(a)) {
                        this.scroll_active = true;
                        cx.notify();
                    }
                }
            });
        }
        if !cx.has_active_drag() {
            self.drop_slot = None;
        }
        let slot = self.drop_slot;
        let titles = self.tab_titles();
        let menu_for = self.tab_menu.as_ref().and_then(|m| m.target.clone());
        let list_open = self.tab_menu.as_ref().is_some_and(|m| m.target.is_none());
        let tabs: Vec<AnyElement> = self
            .buffers
            .iter()
            .zip(titles)
            .enumerate()
            .map(|(ix, (b, title))| {
                let active = self.tabs.active() == Some(ix);
                let preview = self.tabs.is_preview(&TabKey::of(&b.path));
                let name = file_name(&b.path);
                let has_err = self
                    .lsp
                    .read(cx)
                    .diagnostics_for(&b.path)
                    .iter()
                    .any(|d| d.severity == Some(lt::DiagnosticSeverity::ERROR));
                let disk = match b.disk {
                    DiskState::Same => None,
                    DiskState::Changed(_) => Some(txt::TabDisk::Changed),
                    DiskState::Deleted => Some(txt::TabDisk::Deleted),
                };
                let label = txt::tab_label_full(
                    &name,
                    &b.rel,
                    txt::TabFlags {
                        dirty: b.dirty,
                        preview,
                        disk,
                    },
                );
                let full = b.path.to_string_lossy().into_owned();
                let parent = title
                    .strip_prefix(name.as_str())
                    .and_then(|r| r.strip_prefix(" · "))
                    .map(str::to_string);
                let drag = DraggedTab {
                    path: b.path.clone(),
                    title: title.clone(),
                };
                let menu_open = menu_for
                    .as_ref()
                    .is_some_and(|p| path_key(p) == path_key(&b.path));
                div()
                    .id(("tab", ix))
                    .role(Role::Tab)
                    .aria_label(label)
                    .aria_description(full.clone())
                    .aria_selected(active)
                    .aria_expanded(menu_open)
                    .relative()
                    .h(px(TAB_STRIP_H))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .flex_none()
                    .cursor_pointer()
                    .text_size(px(13.))
                    .border_r_1()
                    .border_color(tc(t.border_subtle))
                    .when(active, |d| {
                        d.bg(tc(t.bg_surface))
                            .text_color(tc(t.text_primary))
                            .border_b_2()
                            .border_color(tc(t.accent_fg))
                    })
                    .when(!active, |d| {
                        d.text_color(tc(t.text_secondary))
                            .hover(|d| d.bg(tc(t.bg_hover)))
                    })
                    .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            this.close_tab(ix, window, cx);
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.open_tab_menu_at(ix, e.position, window, cx);
                        }),
                    )
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        if ix >= this.buffers.len() {
                            return;
                        }
                        this.activate(ix, window, cx);
                        if e.click_count() > 1 {
                            this.pin_active(cx);
                        }
                        this.focus_active(window, cx);
                    }))
                    .on_drag(drag, move |d: &DraggedTab, _, _, cx| {
                        cx.new(|_| TabDragPreview {
                            title: d.title.clone(),
                            mode,
                        })
                    })
                    // Left half of a tab: insert before it; right half: after it.
                    .on_drag_move(
                        cx.listener(move |this, e: &DragMoveEvent<DraggedTab>, _, cx| {
                            if e.bounds.contains(&e.event.position) {
                                let s = if e.event.position.x < e.bounds.center().x {
                                    ix
                                } else {
                                    ix + 1
                                };
                                if this.drop_slot != Some(s) {
                                    this.drop_slot = Some(s);
                                    cx.notify();
                                }
                            }
                        }),
                    )
                    .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| {
                        let s = this.drop_slot.unwrap_or(ix);
                        this.drop_tab(&d.path, s, cx);
                    }))
                    // Drop indicator: a bar on the insertion edge.
                    .when(slot == Some(ix), |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left_0()
                                .w(px(2.))
                                .bg(tc(t.accent_fg)),
                        )
                    })
                    .when(slot == Some(n) && ix + 1 == n, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .right_0()
                                .w(px(2.))
                                .bg(tc(t.accent_fg)),
                        )
                    })
                    .when_some(disk, |d, k| {
                        let col = if k == txt::TabDisk::Deleted {
                            t.danger_fg
                        } else {
                            t.warning_fg
                        };
                        d.child(icon(Icon::TriangleAlert, col).size(px(12.)))
                    })
                    .child(
                        div()
                            .when(preview, |d| d.italic())
                            .when(has_err, |d| d.text_color(tc(t.danger_fg)))
                            .child(name.clone()),
                    )
                    .when_some(parent, |d, p| {
                        d.child(
                            div()
                                .max_w(px(180.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(12.))
                                .text_color(tc(t.text_muted))
                                .child(p),
                        )
                    })
                    .child(
                        div()
                            .id(("tab-close", ix))
                            .role(Role::Button)
                            .aria_label(format!("{} {name}", txt::CLOSE_TAB))
                            .w(px(16.))
                            .h(px(16.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_size(px(12.))
                            .text_color(tc(t.text_muted))
                            .hover(|d| d.bg(tc(t.bg_pressed)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(ix, window, cx);
                            }))
                            .child(if b.dirty { "●" } else { "×" }),
                    )
                    .into_any_element()
            })
            .collect();
        // Space after the last tab: dropping there moves a tab to the end.
        let tail = div()
            .id("pod-editor-tabs-tail")
            .flex_1()
            .min_w(px(24.))
            .h_full()
            .on_drag_move(
                cx.listener(move |this, e: &DragMoveEvent<DraggedTab>, _, cx| {
                    if e.bounds.contains(&e.event.position) && this.drop_slot != Some(n) {
                        this.drop_slot = Some(n);
                        cx.notify();
                    }
                }),
            )
            .on_drop(cx.listener(move |this, d: &DraggedTab, _, cx| this.drop_tab(&d.path, n, cx)));
        let mut chevron = icon(Icon::ChevronRight, t.text_secondary).size(px(14.));
        chevron = chevron
            .with_transformation(Transformation::rotate(radians(std::f32::consts::FRAC_PI_2)));
        div()
            .h(px(TAB_STRIP_H))
            .flex_none()
            .flex()
            .bg(tc(t.bg_chrome))
            .border_b_1()
            .border_color(tc(t.border_subtle))
            .child(
                div()
                    .id("pod-editor-tabs")
                    .role(Role::TabList)
                    .aria_label(txt::TABS_LABEL)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_end()
                    .overflow_x_scroll()
                    .track_scroll(&self.tab_scroll)
                    .children(tabs)
                    .child(tail),
            )
            .when(self.tab_overflow, |d| {
                d.child(
                    div()
                        .id("pod-editor-tabs-overflow")
                        .role(Role::Button)
                        .aria_label(txt::OPEN_TABS_BUTTON)
                        .aria_expanded(list_open)
                        .tab_index(0)
                        .focus_visible(move |st| pod_ui::focus_style(st, mode))
                        .flex_none()
                        .w(px(OVERFLOW_BTN_W))
                        .h_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .border_l_1()
                        .border_color(tc(t.border_subtle))
                        .cursor_pointer()
                        .hover(|d| d.bg(tc(t.bg_hover)))
                        .tooltip(|window, cx| Tooltip::new(txt::OPEN_TABS_BUTTON).build(window, cx))
                        .on_click(cx.listener(|this, _, window, cx| this.open_tab_list(window, cx)))
                        .child(chevron),
                )
            })
            .into_any_element()
    }

    fn render_tab_menu(&mut self, t: Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.mode;
        let m = self.tab_menu.as_ref()?;
        let keys = cx.listener(|this, e: &KeyDownEvent, w, cx| {
            let k = e.keystroke.key.as_str();
            let handled = match k {
                "escape" | "tab" => {
                    this.close_menu(w, cx);
                    true
                }
                "up" => {
                    this.menu_move(-1, cx);
                    true
                }
                "down" => {
                    this.menu_move(1, cx);
                    true
                }
                "home" => {
                    this.menu_edge(false, cx);
                    true
                }
                "end" => {
                    this.menu_edge(true, cx);
                    true
                }
                "enter" | "space" => {
                    this.menu_activate(w, cx);
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
            }
        });
        let mut rows: Vec<AnyElement> = Vec::with_capacity(m.items.len() * 2);
        for (i, it) in m.items.iter().enumerate() {
            if it.separator_before && i > 0 {
                rows.push(
                    div()
                        .my(px(4.))
                        .h(px(1.))
                        .bg(tc(t.border_subtle))
                        .into_any_element(),
                );
            }
            let sel = i == m.selected;
            let enabled = it.enabled;
            rows.push(
                div()
                    .id(("tab-menu-item", i))
                    .role(Role::MenuItem)
                    .aria_label(it.name.clone())
                    .when_some(it.shortcut, |d, s| d.aria_keyshortcuts(s))
                    .when(sel, |d| d.aria_active_descendant().bg(tc(t.bg_selected)))
                    .h(px(28.))
                    .px(px(12.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(24.))
                    .text_size(px(13.))
                    .whitespace_nowrap()
                    .text_color(tc(if enabled {
                        t.text_primary
                    } else {
                        t.text_muted
                    }))
                    .when(enabled, |d| {
                        d.cursor_pointer()
                            .when(!sel, |d| d.hover(|d| d.bg(tc(t.bg_hover))))
                            .on_click(cx.listener(move |this, _, w, cx| {
                                if let Some(m) = &mut this.tab_menu {
                                    m.selected = i;
                                }
                                this.menu_activate(w, cx);
                            }))
                    })
                    .child(it.label.clone())
                    .when_some(it.shortcut, |d, s| {
                        d.child(
                            div()
                                .text_size(px(12.))
                                .text_color(tc(t.text_muted))
                                .child(s),
                        )
                    })
                    .into_any_element(),
            );
        }
        let menu = div()
            .id("tab-menu")
            .track_focus(&self.menu_focus)
            .role(Role::Menu)
            .aria_label(m.title.clone())
            .capture_key_down(keys)
            .on_mouse_down_out(cx.listener(|this, _, w, cx| this.close_menu(w, cx)))
            .min_w(px(220.))
            .max_w(px(480.))
            .max_h(px(400.))
            .overflow_y_scroll()
            .py(px(4.))
            .flex()
            .flex_col()
            .bg(tc(t.bg_raised))
            .border_1()
            .border_color(tc(t.border_subtle))
            .rounded(px(8.))
            .shadow(pod_ui::tokens::elevation(2, mode))
            .children(rows);
        Some(
            deferred(
                anchored()
                    .position(m.position)
                    .anchor(m.anchor)
                    .snap_to_window_with_margin(px(8.))
                    .child(menu),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }

    fn render_banner(&mut self, t: Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.mode;
        let b = self.buffers.get(self.tabs.active()?)?;
        let bar = |bg: u32, fg: u32| {
            div()
                .flex_none()
                .min_h(px(32.))
                .px(px(12.))
                .py(px(4.))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .bg(tc(bg))
                .text_size(px(13.))
                .text_color(tc(fg))
        };
        let button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .tab_index(0)
                .focus_visible(move |st| pod_ui::focus_style(st, mode))
                .flex_none()
                .px(px(8.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .border_1()
                .border_color(tc(t.border_control))
                .text_color(tc(t.text_primary))
                .bg(tc(t.bg_surface))
                .cursor_pointer()
                .hover(|d| d.bg(tc(t.bg_hover)))
                .child(label)
        };
        if let (Some(f), DiskState::Same) = (&b.save_failed, &b.disk) {
            let replace = f.replace;
            let mut bar = bar(t.danger_bg, t.danger_fg)
                .child(icon(Icon::OctagonX, t.danger_fg))
                .child(div().flex_1().min_w(px(220.)).child(format!(
                    "저장하지 못했습니다. 디스크의 파일은 그대로입니다: {}",
                    f.message
                )))
                .child(button("save-retry", "다시 저장").on_click(
                    cx.listener(|this, _, w, cx| this.answer_save_failure(false, w, cx)),
                ));
            if replace {
                bar =
                    bar.child(button("save-overwrite", "그대로 덮어쓰기").on_click(
                        cx.listener(|this, _, w, cx| this.answer_save_failure(true, w, cx)),
                    ));
            }
            return Some(
                bar.child(
                    button("save-dismiss", "닫기")
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss_save_failure(cx))),
                )
                .into_any_element(),
            );
        }
        match &b.disk {
            DiskState::Changed(_) => {
                let (msg, keep) = if b.save_blocked {
                    (
                        "저장하지 않았습니다: 열어 둔 뒤 디스크의 파일이 바뀌었습니다 (에이전트 또는 다른 프로그램).",
                        "덮어써서 저장",
                    )
                } else {
                    (
                        "디스크의 파일이 바뀌었습니다 (에이전트 또는 다른 프로그램). 저장하지 않은 변경이 있습니다.",
                        "내 변경 유지",
                    )
                };
                return Some(
                    bar(t.warning_bg, t.warning_fg)
                        .child(icon(Icon::TriangleAlert, t.warning_fg))
                        .child(div().flex_1().min_w(px(220.)).child(msg))
                        .child(button("disk-reload", "디스크 내용 불러오기").on_click(
                            cx.listener(|this, _, w, cx| this.resolve_disk_prompt(true, w, cx)),
                        ))
                        .child(
                            button("disk-diff", "차이 보기")
                                .on_click(cx.listener(|this, _, w, cx| this.show_disk_diff(w, cx))),
                        )
                        .child(button("disk-keep", keep).on_click(
                            cx.listener(|this, _, w, cx| this.resolve_disk_prompt(false, w, cx)),
                        ))
                        .into_any_element(),
                );
            }
            DiskState::Deleted => {
                return Some(
                    bar(t.danger_bg, t.danger_fg)
                        .child(icon(Icon::OctagonX, t.danger_fg))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(220.))
                                .child("디스크에서 파일이 삭제되었습니다."),
                        )
                        .child(button("del-close", "탭 닫기").on_click(
                            cx.listener(|this, _, w, cx| this.resolve_disk_prompt(true, w, cx)),
                        ))
                        .child(button("del-keep", "내 변경 유지").on_click(
                            cx.listener(|this, _, w, cx| this.resolve_disk_prompt(false, w, cx)),
                        ))
                        .into_any_element(),
                );
            }
            DiskState::Same => {}
        }
        if let Some(r) = self
            .lsp
            .read(cx)
            .untrusted_repo_config()
            .filter(|_| !self.repo_lsp_dismissed)
        {
            let hash = r.hash.clone();
            let mut text = div().flex_1().min_w(px(220.)).flex().flex_col().child("이 저장소의 .pod/lsp.json이 언어 서버 실행 명령을 정합니다. 승인하기 전에는 이 파일의 명령을 쓰지 않습니다:");
            for c in &r.commands {
                text = text.child(
                    div()
                        .font_family("Cascadia Mono")
                        .text_size(px(12.))
                        .child(c.clone()),
                );
            }
            return Some(
                bar(t.warning_bg, t.warning_fg)
                    .child(icon(Icon::TriangleAlert, t.warning_fg))
                    .child(text)
                    .child(button("repo-lsp-trust", "이 내용 신뢰하고 사용").on_click(
                        cx.listener(move |this, _, _, cx| this.trust_repo_lsp(&hash, cx)),
                    ))
                    .child(button("repo-lsp-ignore", "무시").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.repo_lsp_dismissed = true;
                            cx.notify();
                        },
                    )))
                    .into_any_element(),
            );
        }
        if b.large {
            return Some(
                bar(t.info_bg, t.info_fg)
                    .child(icon(Icon::CircleHelp, t.info_fg))
                    .child("큰 파일입니다: 읽기 전용으로 열었고 구문 강조와 언어 서버를 껐습니다.")
                    .into_any_element(),
            );
        }
        if b.undecodable {
            let msg = if b.format.encoding == text::utf8() {
                "UTF-8이 아닌 파일이라 읽기 전용으로 열었습니다 (그대로 저장하면 글자가 깨집니다). 인코딩을 골라 다시 열 수 있습니다.".to_string()
            } else {
                format!(
                    "{}(으)로 손실 없이 읽을 수 없어 읽기 전용으로 열었습니다. 다른 인코딩을 골라 보세요.",
                    b.format.encoding.name()
                )
            };
            let ix = self.tabs.active()?;
            let mut row = bar(t.warning_bg, t.warning_fg)
                .child(icon(Icon::TriangleAlert, t.warning_fg))
                .child(div().flex_1().min_w(px(220.)).child(msg));
            for (n, (label, enc)) in text::REOPEN_ENCODINGS.iter().enumerate() {
                let enc: &'static text::Encoding = enc;
                row = row.child(
                    button(["reopen-enc-0", "reopen-enc-1", "reopen-enc-2"][n], label).on_click(
                        cx.listener(move |this, _, w, cx| {
                            this.reopen_with_encoding(ix, enc, w, cx)
                        }),
                    ),
                );
            }
            return Some(row.into_any_element());
        }
        let path = b.path.clone();
        let status = self.lsp.read(cx).status_for(&path)?;
        match &status.state {
            ServerState::NotInstalled(reason, hint) => Some(
                bar(t.info_bg, t.info_fg)
                    .child(icon(Icon::CircleHelp, t.info_fg))
                    .child(div().flex_1().min_w(px(220.)).child(format!(
                        "언어 서버 {}가 설치되어 있지 않습니다 ({reason}). 설치: {hint}",
                        status.id
                    )))
                    .into_any_element(),
            ),
            ServerState::Failed(reason) => Some(
                bar(t.danger_bg, t.danger_fg)
                    .child(icon(Icon::OctagonX, t.danger_fg))
                    .child(div().flex_1().min_w(px(220.)).child(format!(
                        "{}가 반복해서 종료되어 멈췄습니다: {}",
                        status.id,
                        reason.lines().next().unwrap_or("")
                    )))
                    .child(button("lsp-restart", "다시 시작").on_click(cx.listener(
                        move |this, _, _, cx| {
                            let p = path.clone();
                            this.lsp.update(cx, |s, cx| s.restart(&p, cx));
                        },
                    )))
                    .into_any_element(),
            ),
            _ => None,
        }
    }

    fn render_panel(&mut self, t: Tokens, cx: &mut Context<Self>) -> AnyElement {
        let store = self.lsp.read(cx);
        let mut problems: Vec<(PathBuf, lt::Diagnostic)> = store
            .diagnostics()
            .flat_map(|f| f.items.iter().map(move |d| (f.path.clone(), d.clone())))
            .collect();
        problems.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then(
                    a.1.severity
                        .map(|s| format!("{s:?}"))
                        .cmp(&b.1.severity.map(|s| format!("{s:?}"))),
                )
                .then(a.1.range.start.line.cmp(&b.1.range.start.line))
        });
        let (errs, warns) = store.diagnostic_counts();
        let tab = |id: &'static str, label: String, on: bool| {
            div()
                .id(id)
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_size(px(12.))
                .when(on, |d| {
                    d.text_color(tc(t.text_primary))
                        .border_b_2()
                        .border_color(tc(t.accent_fg))
                })
                .when(!on, |d| d.text_color(tc(t.text_secondary)))
                .child(label)
        };
        let root = self.root.clone();
        let rows: Vec<AnyElement> = match self.panel_tab {
            PanelTab::Problems => problems
                .iter()
                .enumerate()
                .map(|(i, (p, d))| {
                    let (ic, col) = match d.severity {
                        Some(lt::DiagnosticSeverity::ERROR) => (Icon::OctagonX, t.danger_fg),
                        Some(lt::DiagnosticSeverity::WARNING) => {
                            (Icon::TriangleAlert, t.warning_fg)
                        }
                        _ => (Icon::CircleHelp, t.info_fg),
                    };
                    let (p2, pos) = (p.clone(), d.range.start);
                    panel_row(("prob", i), t)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            let _ = this.open_at(&p2, Some(pos), w, cx);
                        }))
                        .child(icon(ic, col).size(px(14.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(tc(t.text_primary))
                                .child(d.message.lines().next().unwrap_or("").to_string()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_color(tc(t.text_muted))
                                .child(format!("{}:{}", rel_of(&root, p), d.range.start.line + 1)),
                        )
                        .into_any_element()
                })
                .collect(),
            PanelTab::References => self
                .references
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let (p2, pos) = (l.path.clone(), l.range.start);
                    let line = std::fs::read_to_string(&l.path)
                        .ok()
                        .and_then(|s| {
                            s.lines()
                                .nth(l.range.start.line as usize)
                                .map(|x| x.trim().to_string())
                        })
                        .unwrap_or_default();
                    panel_row(("ref", i), t)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            let _ = this.open_at(&p2, Some(pos), w, cx);
                        }))
                        .child(
                            div()
                                .flex_none()
                                .text_color(tc(t.text_muted))
                                .child(format!(
                                    "{}:{}",
                                    rel_of(&root, &l.path),
                                    l.range.start.line + 1
                                )),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .font_family("Cascadia Mono")
                                .text_color(tc(t.text_primary))
                                .child(line),
                        )
                        .into_any_element()
                })
                .collect(),
        };
        let empty = rows.is_empty();
        let empty_text = match self.panel_tab {
            PanelTab::Problems => "문제가 없습니다",
            PanelTab::References => "Shift+F12로 참조를 찾으세요",
        };
        div()
            .h(px(PANEL_H))
            .flex_none()
            .flex()
            .flex_col()
            .bg(tc(t.bg_surface))
            .border_t_1()
            .border_color(tc(t.border_subtle))
            .child(
                div()
                    .flex()
                    .items_center()
                    .bg(tc(t.bg_chrome))
                    .child(
                        tab(
                            "panel-problems",
                            format!("문제 · 오류 {errs} · 경고 {warns}"),
                            self.panel_tab == PanelTab::Problems,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.panel_tab = PanelTab::Problems;
                            cx.notify();
                        })),
                    )
                    .child(
                        tab(
                            "panel-refs",
                            if self.references_title.is_empty() {
                                "참조".to_string()
                            } else {
                                self.references_title.clone()
                            },
                            self.panel_tab == PanelTab::References,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.panel_tab = PanelTab::References;
                            cx.notify();
                        })),
                    ),
            )
            .child(
                div()
                    .id("panel-body")
                    .flex_1()
                    .overflow_y_scroll()
                    .when(empty, |d| {
                        d.child(
                            div()
                                .p(px(12.))
                                .text_size(px(13.))
                                .text_color(tc(t.text_muted))
                                .child(empty_text),
                        )
                    })
                    .children(rows),
            )
            .into_any_element()
    }

    fn render_status(&mut self, t: Tokens, cx: &mut Context<Self>) -> AnyElement {
        let mut left: Vec<String> = vec![];
        if let Some(b) = &self.branch {
            left.push(format!("⎇ {b}"));
        }
        let (e, w) = self.lsp.read(cx).diagnostic_counts();
        left.push(format!("오류 {e} · 경고 {w}"));
        if self.indexing {
            left.push("파일 색인 중…".into());
        }
        let mut right: Vec<String> = vec![];
        let mut lsp_label: Option<(String, u32)> = None;
        if let Some(b) = self.tabs.active().and_then(|i| self.buffers.get(i)) {
            let pos = b.state.read(cx).cursor_position();
            right.push(format!("줄 {}, 열 {}", pos.line + 1, pos.character + 1));
            right.push(b.format.ending_label());
            let enc = b.format.encoding.name();
            right.push(if b.format.bom {
                format!("{enc} BOM")
            } else {
                enc.to_string()
            });
            if b.undecodable {
                right.push("읽기 전용".into());
            }
            right.push(b.language.map(|l| l.label).unwrap_or("일반 텍스트").into());
            if let Some(s) = self.lsp.read(cx).status_for(&b.path) {
                let col = match s.state {
                    ServerState::Running => t.success_fg,
                    ServerState::NotInstalled(..)
                    | ServerState::Checking
                    | ServerState::Starting => t.text_muted,
                    ServerState::Restarting { .. } => t.warning_fg,
                    _ => t.danger_fg,
                };
                let mut label = s.state.label(&s.id);
                if let Some(p) = s.progress {
                    let p: String = if p.chars().count() > 60 {
                        p.chars().take(59).chain(std::iter::once('…')).collect()
                    } else {
                        p
                    };
                    label = format!("{} · {p}", s.id);
                }
                lsp_label = Some((label, col));
            }
        }
        div()
            .h(px(24.))
            .flex_none()
            .px(px(12.))
            .flex()
            .items_center()
            .gap(px(16.))
            .bg(tc(t.bg_chrome))
            .border_t_1()
            .border_color(tc(t.border_subtle))
            .text_size(px(12.))
            .text_color(tc(t.text_secondary))
            .children(left.into_iter().map(|s| div().flex_none().child(s)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when_some(self.message.clone(), |d, (m, err)| {
                        d.text_color(tc(if err { t.danger_fg } else { t.text_primary }))
                            .child(m)
                    }),
            )
            .children(right.into_iter().map(|s| div().flex_none().child(s)))
            .when_some(lsp_label, |d, (l, c)| {
                d.child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(div().w(px(8.)).h(px(8.)).rounded_full().bg(tc(c)))
                        .child(l),
                )
            })
            .into_any_element()
    }

    fn render_overlay(&mut self, t: Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let mode = self.mode;
        let frame = |title: &str| {
            div()
                .w(px(480.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(12.))
                .bg(tc(t.bg_raised))
                .border_1()
                .border_color(tc(t.border_subtle))
                .rounded(px(12.))
                .shadow(pod_ui::tokens::elevation(2, mode))
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tc(t.text_primary))
                        .child(title.to_string()),
                )
        };
        let esc = cx.listener(|this, e: &KeyDownEvent, w, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                this.close_overlay(w, cx);
            }
        });
        let button = |id: &'static str, label: &'static str, primary: bool, danger: bool| {
            div()
                .id(id)
                .role(Role::Button)
                .aria_label(label)
                .tab_index(0)
                .focus_visible(move |st| pod_ui::focus_style(st, mode))
                .px(px(12.))
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(primary, |d| {
                    d.bg(tc(if danger {
                        t.danger_fg
                    } else {
                        t.action_primary_bg
                    }))
                    .text_color(tc(t.action_primary_fg))
                })
                .when(!primary, |d| {
                    d.border_1()
                        .border_color(tc(t.border_control))
                        .text_color(tc(t.text_primary))
                        .hover(|d| d.bg(tc(t.bg_hover)))
                })
                .child(label)
        };
        let err_line = |e: &Option<String>| {
            e.clone().map(|e| {
                div()
                    .text_size(px(12.))
                    .text_color(tc(t.danger_fg))
                    .child(e)
            })
        };
        let el = match &self.overlay {
            Overlay::None => return None,
            Overlay::ConfirmClose { dirty, close, .. } => {
                let many = close.len() > 1;
                let body = if many {
                    txt::unsaved_many_body(close.len(), dirty.len())
                } else {
                    txt::unsaved_body(&rel_of(&self.root, &dirty[0]))
                };
                let files: Vec<String> = if many {
                    dirty.iter().map(|p| rel_of(&self.root, p)).collect()
                } else {
                    vec![]
                };
                let keys =
                    cx.listener(
                        |this, e: &KeyDownEvent, w, cx| match e.keystroke.key.as_str() {
                            "escape" => {
                                cx.stop_propagation();
                                this.resolve_close(None, w, cx);
                            }
                            "enter" if this.prompt_focus.is_focused(w) => {
                                cx.stop_propagation();
                                this.resolve_close(Some(true), w, cx);
                            }
                            _ => {}
                        },
                    );
                frame(txt::UNSAVED_TITLE)
                    .id("confirm-close")
                    .track_focus(&self.prompt_focus)
                    .role(Role::AlertDialog)
                    .aria_label(txt::UNSAVED_TITLE)
                    .capture_key_down(keys)
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(tc(t.text_secondary))
                            .child(body),
                    )
                    .when(!files.is_empty(), |d| {
                        d.child(
                            div()
                                .id("confirm-close-files")
                                .role(Role::List)
                                .aria_label(txt::UNSAVED_TITLE)
                                .max_h(px(160.))
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .children(files.into_iter().enumerate().map(|(i, f)| {
                                    div()
                                        .id(("confirm-close-file", i))
                                        .role(Role::ListItem)
                                        .aria_label(f.clone())
                                        .text_size(px(12.))
                                        .font_family("Cascadia Mono")
                                        .text_color(tc(t.text_primary))
                                        .child(f)
                                })),
                        )
                    })
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(button("close-cancel", txt::CANCEL, false, false).on_click(
                                cx.listener(|this, _, w, cx| this.resolve_close(None, w, cx)),
                            ))
                            .child(
                                button("close-discard", txt::CLOSE_WITHOUT_SAVING, false, false)
                                    .on_click(cx.listener(|this, _, w, cx| {
                                        this.resolve_close(Some(false), w, cx)
                                    })),
                            )
                            .child(
                                button(
                                    "close-save",
                                    if many {
                                        txt::SAVE_ALL_AND_CLOSE
                                    } else {
                                        txt::SAVE_AND_CLOSE
                                    },
                                    true,
                                    false,
                                )
                                .on_click(cx.listener(
                                    |this, _, w, cx| this.resolve_close(Some(true), w, cx),
                                )),
                            ),
                    )
                    .into_any_element()
            }
            Overlay::NewEntry {
                parent,
                folder,
                input,
                error,
            } => {
                let rel = rel_of(&self.root, parent);
                let title = format!(
                    "{} · {}",
                    if *folder {
                        txt::NEW_FOLDER_TITLE
                    } else {
                        txt::NEW_FILE_TITLE
                    },
                    if rel.is_empty() {
                        "/".to_string()
                    } else {
                        format!("{rel}/")
                    }
                );
                frame(&title)
                    .capture_key_down(esc)
                    .child(Input::new(input))
                    .children(err_line(error))
                    .child(hint(t, txt::ENTER_CREATE))
                    .into_any_element()
            }
            Overlay::RenameEntry { path, input, error } => {
                let title = format!("{} · {}", txt::RENAME_TITLE, rel_of(&self.root, path));
                frame(&title)
                    .capture_key_down(esc)
                    .child(Input::new(input))
                    .children(err_line(error))
                    .child(hint(t, txt::ENTER_RENAME))
                    .into_any_element()
            }
            Overlay::ConfirmDelete(path) => {
                let rel = rel_of(&self.root, path);
                let keys =
                    cx.listener(
                        |this, e: &KeyDownEvent, w, cx| match e.keystroke.key.as_str() {
                            "escape" => {
                                cx.stop_propagation();
                                this.close_overlay(w, cx);
                            }
                            "enter" if this.prompt_focus.is_focused(w) => {
                                cx.stop_propagation();
                                this.commit_delete(w, cx);
                            }
                            _ => {}
                        },
                    );
                frame(txt::DELETE_TITLE)
                    .id("confirm-delete")
                    .track_focus(&self.prompt_focus)
                    .role(Role::AlertDialog)
                    .aria_label(txt::DELETE_TITLE)
                    .capture_key_down(keys)
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(tc(t.text_secondary))
                            .child(txt::delete_body(&rel, path.is_dir())),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(
                                button("delete-cancel", txt::CANCEL, false, false).on_click(
                                    cx.listener(|this, _, w, cx| this.close_overlay(w, cx)),
                                ),
                            )
                            .child(
                                button("delete-ok", txt::DELETE_CONFIRM, true, true).on_click(
                                    cx.listener(|this, _, w, cx| this.commit_delete(w, cx)),
                                ),
                            ),
                    )
                    .into_any_element()
            }
            Overlay::DiskDiff { rel, lines } => {
                let keys = cx.listener(|this, e: &KeyDownEvent, w, cx| {
                    if e.keystroke.key == "escape" {
                        cx.stop_propagation();
                        this.close_overlay(w, cx);
                    }
                });
                let mut list = div()
                    .id("disk-diff")
                    .max_h(px(420.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .font_family("Cascadia Mono")
                    .text_size(px(12.));
                if lines.is_empty() {
                    list = list.child(div().text_color(tc(t.text_muted)).child("차이가 없습니다"));
                }
                for (tag, line) in lines {
                    let (bg, fg) = match tag {
                        '-' => (Some(t.danger_bg), t.danger_fg),
                        '+' => (Some(t.success_bg), t.success_fg),
                        _ => (None, t.text_muted),
                    };
                    list = list.child(
                        div()
                            .px(px(4.))
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_color(tc(fg))
                            .when_some(bg, |d, bg| d.bg(tc(bg)))
                            .child(format!("{tag} {line}")),
                    );
                }
                frame(&format!("디스크(-)와 내 변경(+) 비교 · {rel}"))
                    .id("disk-diff-dialog")
                    .w(px(720.))
                    .track_focus(&self.prompt_focus)
                    .role(Role::AlertDialog)
                    .capture_key_down(keys)
                    .child(list)
                    .child(
                        div().flex().justify_end().child(
                            button("disk-diff-close", "닫기 (Esc)", false, false)
                                .on_click(cx.listener(|this, _, w, cx| this.close_overlay(w, cx))),
                        ),
                    )
                    .into_any_element()
            }
            Overlay::QuickOpen(qo) => qo.clone().into_any_element(),
            Overlay::GotoLine(inp) => frame("줄로 이동 (Ctrl+G)")
                .capture_key_down(esc)
                .child(Input::new(inp))
                .child(hint(t, "Enter: 이동 · Esc: 닫기"))
                .into_any_element(),
            Overlay::Rename { input, .. } => frame("이름 바꾸기 (F2)")
                .capture_key_down(esc)
                .child(Input::new(input))
                .child(hint(t, "Enter: 미리 보기 · Esc: 취소"))
                .into_any_element(),
            Overlay::RenamePreview { new_name, edits } => {
                let total: usize = edits.iter().map(|f| f.edits.len()).sum();
                let root = self.root.clone();
                let mut list = div()
                    .id("rename-preview")
                    .max_h(px(320.))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(2.));
                for f in edits {
                    let text = self
                        .buffers
                        .iter()
                        .find(|b| path_key(&b.path) == path_key(&f.path))
                        .map(|b| b.state.read(cx).value().to_string())
                        .or_else(|| text::load(&f.path).ok().map(|l| l.text))
                        .unwrap_or_default();
                    list = list.child(
                        div()
                            .pt(px(4.))
                            .text_size(px(12.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(tc(t.text_secondary))
                            .child(format!("{} · {}곳", rel_of(&root, &f.path), f.edits.len())),
                    );
                    for e in f.edits.iter().take(20) {
                        let line = text::line_of(&text, e.range.start.line).to_string();
                        let after = if e.range.start.line == e.range.end.line {
                            apply_edits(
                                &line,
                                &[lt::TextEdit {
                                    range: lt::Range::new(
                                        lt::Position::new(0, e.range.start.character),
                                        lt::Position::new(0, e.range.end.character),
                                    ),
                                    new_text: e.new_text.clone(),
                                }],
                            )
                        } else {
                            e.new_text.clone()
                        };
                        list = list.child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .text_size(px(12.))
                                .font_family("Cascadia Mono")
                                .child(
                                    div()
                                        .w(px(40.))
                                        .flex_none()
                                        .text_color(tc(t.text_muted))
                                        .child(format!("{}", e.range.start.line + 1)),
                                )
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_color(tc(t.text_primary))
                                        .child(format!("{} → {}", line.trim(), after.trim())),
                                ),
                        );
                    }
                }
                let enter =
                    cx.listener(
                        |this, e: &KeyDownEvent, w, cx| match e.keystroke.key.as_str() {
                            "enter" => {
                                cx.stop_propagation();
                                this.apply_rename(w, cx);
                            }
                            "escape" => {
                                cx.stop_propagation();
                                this.close_overlay(w, cx);
                            }
                            _ => {}
                        },
                    );
                frame(&format!(
                    "이름 바꾸기 미리 보기: `{new_name}` · 파일 {}개 · {total}곳",
                    edits.len()
                ))
                .w(px(640.))
                .capture_key_down(enter)
                .child(list)
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(8.))
                        .child(
                            div()
                                .id("rename-cancel")
                                .px(px(12.))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .border_1()
                                .border_color(tc(t.border_control))
                                .text_color(tc(t.text_primary))
                                .cursor_pointer()
                                .child("취소 (Esc)")
                                .on_click(cx.listener(|this, _, w, cx| this.close_overlay(w, cx))),
                        )
                        .child(
                            div()
                                .id("rename-apply")
                                .px(px(12.))
                                .h(px(28.))
                                .flex()
                                .items_center()
                                .rounded(px(6.))
                                .bg(tc(t.action_primary_bg))
                                .text_color(tc(t.action_primary_fg))
                                .cursor_pointer()
                                .child("적용 (Enter)")
                                .on_click(cx.listener(|this, _, w, cx| this.apply_rename(w, cx))),
                        ),
                )
                .into_any_element()
            }
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .pt(px(48.))
                .child(div().child(el))
                .into_any_element(),
        )
    }
}

/// Changed lines (with 2 lines of context, `…` between hunks) of `disk`
/// against `mine`: `-` only on disk, `+` only in the buffer, ` ` context.
fn disk_diff_lines(disk: &str, mine: &str) -> Vec<(char, String)> {
    let diff = similar::TextDiff::configure()
        .timeout(Duration::from_secs(1))
        .diff_lines(disk, mine);
    let mut out = Vec::new();
    for (n, group) in diff.grouped_ops(2).iter().enumerate() {
        if n > 0 {
            out.push((' ', "…".to_string()));
        }
        for op in group {
            for ch in diff.iter_changes(op) {
                let tag = match ch.tag() {
                    similar::ChangeTag::Delete => '-',
                    similar::ChangeTag::Insert => '+',
                    similar::ChangeTag::Equal => ' ',
                };
                out.push((tag, ch.value().trim_end_matches('\n').to_string()));
            }
        }
    }
    out
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

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn hint(t: Tokens, s: &'static str) -> impl IntoElement {
    div()
        .text_size(px(12.))
        .text_color(tc(t.text_muted))
        .child(s)
}

fn panel_row(id: (&'static str, usize), t: Tokens) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(24.))
        .px(px(12.))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(12.))
        .cursor_pointer()
        .hover(|d| d.bg(tc(t.bg_hover)))
}

fn mark_color(m: GitMark, t: Tokens) -> u32 {
    match m {
        GitMark::Modified => t.warning_fg,
        GitMark::Added | GitMark::Untracked => t.success_fg,
        GitMark::Deleted | GitMark::Conflicted => t.danger_fg,
        GitMark::Renamed => t.info_fg,
    }
}

fn set_editor_diagnostics(state: &Entity<EditorState>, diags: Vec<lt::Diagnostic>, cx: &mut App) {
    state.update(cx, |s, cx| {
        if let Some(set) = s.diagnostics_mut() {
            set.clear();
            set.extend(diags.into_iter().map(EditorDiagnostic::from));
        }
        cx.notify();
    });
}

/// "42" -> line 42, "42:5" / "42,5" -> line 42 col 5 (1-based input).
pub fn parse_line_col(s: &str) -> Option<lt::Position> {
    let s = s.trim().trim_start_matches(':');
    let mut it = s.split([':', ',']);
    let line: u32 = it.next()?.trim().parse().ok()?;
    let col: u32 = it.next().and_then(|c| c.trim().parse().ok()).unwrap_or(1);
    (line >= 1).then(|| lt::Position::new(line - 1, col.saturating_sub(1)))
}

impl Render for EditorWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = Tokens::for_mode(self.mode);
        let tree = self.render_tree(t, window, cx);
        let tabs = self.render_tabs(t, window, cx);
        let menu = self.render_tab_menu(t, cx);
        let banner = self.render_banner(t, cx);
        let panel = self.panel_open.then(|| self.render_panel(t, cx));
        let status = self.render_status(t, cx);
        let overlay = self.render_overlay(t, cx);
        let editor: AnyElement = match self.tabs.active().and_then(|i| self.buffers.get(i)) {
            Some(b) => div()
                .flex_1()
                .min_h_0()
                .bg(tc(t.bg_surface))
                .child(
                    Editor::new(&b.state)
                        .aria_label(txt::editor_label(&b.rel))
                        .bordered(false)
                        .h_full(),
                )
                .into_any_element(),
            None => div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .bg(tc(t.bg_canvas))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tc(t.text_primary))
                        .child("열린 파일이 없습니다"),
                )
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(tc(t.text_muted))
                        .child("왼쪽 트리에서 파일을 고르거나 Ctrl+P로 파일 이름을 검색하세요"),
                )
                .into_any_element(),
        };
        div()
            .id("pod-editor")
            .key_context(CTX)
            .track_focus(&self.focus)
            // Capture phase: before the editor's own Tab (indent) / Esc.
            .capture_action(cx.listener(|this, _: &IndentInline, _, cx| {
                if this.snippet_step(true, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &OutdentInline, _, cx| {
                if this.snippet_step(false, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_action(cx.listener(|this, _: &Escape, _, cx| {
                // Esc ends the session; a completion menu still closes first.
                if let Some(i) = this.tabs.active()
                    && this.buffers[i].snippet.is_some()
                    && !this.buffers[i].state.read(cx).completion_menu_state().open
                {
                    this.buffers[i].snippet = None;
                    cx.stop_propagation();
                }
            }))
            .on_action(cx.listener(Self::on_quick_open))
            .on_action(cx.listener(Self::on_save))
            .on_action(cx.listener(Self::on_save_all))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_next_tab_position))
            .on_action(cx.listener(Self::on_prev_tab_position))
            .on_action(cx.listener(Self::on_reopen_closed))
            .on_action(cx.listener(Self::on_pin_tab))
            .on_action(cx.listener(Self::on_move_tab_left))
            .on_action(cx.listener(Self::on_move_tab_right))
            .on_action(cx.listener(Self::on_tab_menu))
            // Ctrl released: a Ctrl+Tab switch settles on the tab it reached.
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| {
                if !e.modifiers.control {
                    this.end_switch(cx);
                }
            }))
            .on_action(cx.listener(Self::on_goto_line))
            .on_action(cx.listener(Self::on_find_references))
            .on_action(cx.listener(Self::on_rename))
            .on_action(cx.listener(Self::on_next_problem))
            .on_action(cx.listener(Self::on_toggle_panel))
            .on_action(cx.listener(Self::on_focus_tree))
            .on_action(cx.listener(Self::on_focus_editor))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(tc(t.bg_canvas))
            .text_color(tc(t.text_primary))
            .font_family(cx.theme().font_family.clone())
            .child(
                div().flex_1().min_h_0().flex().child(tree).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(tabs)
                        .children(banner)
                        .child(editor)
                        .children(panel),
                ),
            )
            .child(status)
            .children(overlay)
            .children(menu)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[core::prelude::v1::test]
    fn line_col_parsing() {
        assert_eq!(parse_line_col("42"), Some(lt::Position::new(41, 0)));
        assert_eq!(parse_line_col("42:5"), Some(lt::Position::new(41, 4)));
        assert_eq!(parse_line_col(":7,2"), Some(lt::Position::new(6, 1)));
        assert_eq!(parse_line_col("0"), None);
        assert_eq!(parse_line_col("x"), None);
    }

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

    #[core::prelude::v1::test]
    fn session_json_is_stable() {
        let s = EditorSession {
            tabs: vec![TabSession {
                path: "src/main.rs".into(),
                line: 3,
                col: 5,
                preview: false,
            }],
            active: Some(0),
            mru: vec![],
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(
            j,
            serde_json::json!({"tabs":[{"path":"src/main.rs","line":3,"col":5}],"active":0})
        );
        let back: EditorSession =
            serde_json::from_value(serde_json::json!({"tabs":[{"path":"a"}]})).unwrap();
        assert_eq!(back.tabs[0].line, 1);
        assert_eq!(back.active, None);
        assert!(!back.tabs[0].preview && back.mru.is_empty());
        // Preview flag and Ctrl+Tab order round-trip (older layouts lack them).
        let s = EditorSession {
            tabs: vec![
                TabSession {
                    path: "a".into(),
                    line: 1,
                    col: 1,
                    preview: false,
                },
                TabSession {
                    path: "b".into(),
                    line: 2,
                    col: 1,
                    preview: true,
                },
            ],
            active: Some(1),
            mru: vec![1, 0],
        };
        let j = serde_json::to_value(&s).unwrap();
        assert_eq!(j["tabs"][1]["preview"], true);
        assert!(j["tabs"][0].get("preview").is_none());
        assert_eq!(serde_json::from_value::<EditorSession>(j).unwrap(), s);
    }
}
