//! `EditorWorkspace`: the embeddable editing view for one worktree root.
//!
//! Layout: file tree | (tabs / banner / code editor / problems+references) /
//! status bar, plus overlays (quick open, go to line, rename + preview).
//! Hosts (pod-app) create one per task worktree and listen to
//! [`EditorEvent`]; see docs/13-editor-and-lsp.md §API.
//!
//! This file holds the state and its types, the actions and key bindings,
//! the lookups and the top-level render. The methods live in private modules
//! by what they do, all on the same state: `session` (layout save/restore),
//! `buffers`, `files` (save, disk changes, index, git), `tabs`, `tree`,
//! `lsp_actions`, and `render_tree`/`render_tabs`/`render_panels`/
//! `render_overlays` (render functions only).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::base::input::Diagnostic as EditorDiagnostic;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::input::{
    Editor, EditorState, Escape, GoToDefinition, IndentInline, InputEvent, InputState,
    OutdentInline, ShowCompletions,
};
use gpui_kit::*;
use lsp_types as lt;
use pod_ui::{Mode, Tokens, tokens::color as tc};

use super::quick_open::{QuickOpen, QuickOpenEvent};
use super::strings as txt;
use crate::index::{IndexSnapshot, Recents};
use crate::lsp::snippet::SnippetSession;
use crate::lsp::{FileEdit, Language, Location, LspStore, LspStoreEvent};
use crate::tabs::TabSet;
use crate::text::{DiskStamp, FileFormat};
use crate::tree::FileTree;
use crate::uri::{path_key, simplify};
use crate::watch::RootWatcher;

mod buffers;
mod files;
mod lsp_actions;
mod render_overlays;
mod render_panels;
mod render_tabs;
mod render_tree;
mod session;
mod tabs;
mod tree;

pub use session::{EditorSession, TabSession};

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

fn rel_of(root: &Path, path: &Path) -> String {
    crate::uri::rel_path(root, path)
}

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

    fn focus_active(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_editor() {
            Some(st) => st.update(cx, |s, cx| s.focus(window, cx)),
            None => self.tree_focus.focus(window, cx),
        }
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

    /// Whether the view has an open modal overlay (quick open, prompts).
    pub fn has_overlay(&self) -> bool {
        if self.tab_menu.is_some() {
            return true;
        }
        !matches!(self.overlay, Overlay::None)
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
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
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
}
