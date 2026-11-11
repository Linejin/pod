//! Dock panels: the panel builders and [`RestoreCtx`] the saved layout is
//! rebuilt through, restore/save, and opening terminal and Review panels in
//! the dock (closing a panel detaches, it never kills).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::dock::{
    DockLayout, DockPlacement, PanelBuildContext, PanelId, panel_handle,
};
use gpui_kit::*;

use crate::editor_panel::EditorPanel;
use crate::layout;
use crate::panel::TerminalPanel;
use crate::review::ReviewPanel;
use crate::source::PoddSource;
use crate::vm::TaskVm;

use super::Workspace;
use super::notifications::ToastKind;

/// App-wide handles the dock's panel builder needs (it runs inside
/// `DockArea::load`, so it must not borrow the workspace).
pub struct AppGlobals {
    pub source: PoddSource,
    pub workspace: WeakEntity<Workspace>,
    pub panels: Rc<RefCell<Vec<WeakEntity<TerminalPanel>>>>,
    pub labels: Rc<RefCell<HashMap<String, String>>>,
    /// Data for the non-terminal panel builders during a layout restore.
    pub restore: Rc<RefCell<RestoreCtx>>,
}

impl Global for AppGlobals {}

/// Shared between [`Workspace::restore`] and the panel builders while
/// `DockArea::load` runs (the builders cannot read the leased workspace).
#[derive(Default)]
pub struct RestoreCtx {
    /// Tasks known at restore time (Review panels need the task).
    pub tasks: Vec<TaskVm>,
    /// Panels the builders made; the workspace adopts them after the load.
    pub built: Vec<RestoredPanel>,
}

pub enum RestoredPanel {
    Review(WeakEntity<ReviewPanel>),
    Group(WeakEntity<crate::groups::GroupPanel>),
    Gates(WeakEntity<crate::gates::GatesPanel>),
    Tree(WeakEntity<crate::tree::AgentTreePanel>),
    Settings(WeakEntity<crate::chrome::ChromePanel>),
    Inspector(WeakEntity<crate::chrome::ChromePanel>),
    Editor(WeakEntity<EditorPanel>),
}

/// `PanelRegistry` builder for persisted editor panels (`{"task", "session"}`).
/// Pruning keeps only tasks whose worktree exists ([`layout::Alive::worktrees`]).
pub fn build_editor_panel(
    ctx: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let task_id = layout::leaf_field(ctx.state(), "task")
        .unwrap_or_default()
        .to_string();
    let session = EditorPanel::session_of_state(ctx.state());
    let g = cx.global::<AppGlobals>();
    let (ws, restore) = (g.workspace.clone(), g.restore.clone());
    let root = restore
        .borrow()
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .and_then(|t| t.worktree.clone())
        .unwrap_or_default();
    let panel = cx.new(|cx| {
        EditorPanel::new(
            task_id,
            std::path::PathBuf::from(root),
            session.as_ref(),
            ws,
            window,
            cx,
        )
    });
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Editor(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for persisted Review panels (`{"task": id}`).
pub fn build_review_panel(
    ctx: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let task_id = layout::leaf_field(ctx.state(), "task")
        .unwrap_or_default()
        .to_string();
    let g = cx.global::<AppGlobals>();
    let (source, ws, restore) = (g.source.clone(), g.workspace.clone(), g.restore.clone());
    // Pruning keeps only reviews of existing tasks; a bare task still works
    // (the next task poll fills it in through `set_task`).
    let task = restore
        .borrow()
        .tasks
        .iter()
        .find(|t| t.id == task_id)
        .cloned();
    let task = task
        .or_else(|| TaskVm::from_json(&serde_json::json!({ "id": task_id })))
        .expect("task id");
    let panel = cx.new(|cx| ReviewPanel::new(source, ws, &task, window, cx));
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Review(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for persisted Group panels (`{"group": id}`).
pub fn build_group_panel(
    ctx: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let group = layout::leaf_field(ctx.state(), "group")
        .unwrap_or_default()
        .to_string();
    let g = cx.global::<AppGlobals>();
    let (source, ws, restore) = (g.source.clone(), g.workspace.clone(), g.restore.clone());
    let panel = cx.new(|cx| crate::groups::GroupPanel::new(source, ws, group, None, window, cx));
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Group(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for the persisted gates inbox.
pub fn build_gates_panel(
    _: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let g = cx.global::<AppGlobals>();
    let (source, ws, restore) = (g.source.clone(), g.workspace.clone(), g.restore.clone());
    let panel = cx.new(|cx| crate::gates::GatesPanel::new(source, ws, window, cx));
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Gates(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for the persisted agent tree.
pub fn build_tree_panel(
    _: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let g = cx.global::<AppGlobals>();
    let (ws, restore) = (g.workspace.clone(), g.restore.clone());
    let panel = cx.new(|cx| crate::tree::AgentTreePanel::new(ws, cx));
    panel.update(cx, |p, cx| p.forward_focus(window, cx));
    restore
        .borrow_mut()
        .built
        .push(RestoredPanel::Tree(panel.downgrade()));
    panel_handle(panel)
}

/// `PanelRegistry` builder for persisted terminal panels.
pub fn build_panel(
    ctx: PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let id = TerminalPanel::terminal_of_state(ctx.state()).unwrap_or_default();
    let panel = new_panel(id, window, cx);
    panel_handle(panel)
}

fn new_panel(id: String, window: &mut Window, cx: &mut App) -> Entity<TerminalPanel> {
    let g = cx.global::<AppGlobals>();
    let (source, ws, panels) = (g.source.clone(), g.workspace.clone(), g.panels.clone());
    let label: SharedString = g
        .labels
        .borrow()
        .get(&id)
        .cloned()
        .unwrap_or_else(|| id.clone())
        .into();
    let panel = cx.new(|cx| TerminalPanel::new(source, ws, id, label, window, cx));
    panels.borrow_mut().push(panel.downgrade());
    panel
}

impl Workspace {
    pub(crate) fn live_panels(&self) -> Vec<Entity<TerminalPanel>> {
        self.panels
            .borrow()
            .iter()
            .filter_map(|w| w.upgrade())
            .collect()
    }

    // ---- layout ---------------------------------------------------------

    pub(super) fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.restored = true;
        let alive = layout::Alive {
            terminals: self.terminals.iter().map(|t| t.id.clone()).collect(),
            tasks: self.tasks.iter().map(|t| t.id.clone()).collect(),
            groups: self.tasks.iter().filter_map(|t| t.group.clone()).collect(),
            review: self.caps.review,
            orch: self.caps.orch,
            worktrees: self
                .tasks
                .iter()
                .filter(|t| {
                    t.worktree
                        .as_deref()
                        .is_some_and(|w| !w.is_empty() && std::path::Path::new(w).is_dir())
                })
                .map(|t| t.id.clone())
                .collect(),
        };
        if let Some(state) = self
            .saved
            .dock
            .as_ref()
            .and_then(|d| layout::prune_dock(d, &alive))
        {
            self.restore_ctx.borrow_mut().tasks = self.tasks.clone();
            let _ = self.dock.update(cx, |a, cx| a.load(state, window, cx));
            let built = {
                let mut ctx = self.restore_ctx.borrow_mut();
                ctx.tasks.clear();
                std::mem::take(&mut ctx.built)
            };
            self.adopt_restored(built, cx);
        }
        if self.options.open_all {
            let running: Vec<String> = self
                .terminals
                .iter()
                .filter(|t| t.is_running())
                .map(|t| t.id.clone())
                .collect();
            let open: HashSet<String> = self
                .live_panels()
                .iter()
                .map(|p| p.read(cx).terminal_id.clone())
                .collect();
            for id in running.into_iter().filter(|id| !open.contains(id)) {
                self.add_panel(id, window, cx);
            }
            self.arrange_grid(window, cx);
        }
        self.schedule_save(cx);
    }

    /// Track the non-terminal panels the dock rebuilt from the saved layout,
    /// like the `open_*` functions do for new ones.
    fn adopt_restored(&mut self, built: Vec<RestoredPanel>, cx: &mut Context<Self>) {
        for p in built {
            match p {
                RestoredPanel::Review(r) => {
                    if let Some(panel) = r.upgrade() {
                        let task = panel.read(cx).task_id.clone();
                        let status = self
                            .task(&task)
                            .and_then(|t| t.terminals.first().cloned())
                            .and_then(|id| {
                                self.terminal(&id)
                                    .and_then(|t| t.status.clone())
                                    .map(|s| (id, s))
                            });
                        if let Some((term, s)) = status {
                            panel.update(cx, |r, cx| r.agent_status(&term, &s, cx));
                        }
                    }
                    self.reviews.push(r);
                }
                RestoredPanel::Group(g) => self.orch.groups.push(g),
                RestoredPanel::Gates(g) => self.orch.gates_panel = Some(g),
                RestoredPanel::Tree(t) => self.orch.tree_panel = Some(t),
                RestoredPanel::Settings(p) => self.settings_panel = Some(p),
                RestoredPanel::Inspector(p) => self.inspector_panel = Some(p),
                RestoredPanel::Editor(e) => {
                    if let Some(p) = e.upgrade() {
                        self.last_editor = Some(p.read(cx).task_id.clone());
                    }
                    self.editors.push(e);
                }
            }
        }
        // Restored terminal panels get their label and agent mark now, not at the next list.
        self.refresh_labels(cx);
        self.touch(cx);
    }

    pub(crate) fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if !self.restored {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            let _ = this.update(cx, |w, cx| w.save_now(cx));
        }));
    }

    pub(crate) fn save_now(&mut self, cx: &mut Context<Self>) {
        self.saved.version = layout::LAYOUT_VERSION;
        self.saved.dock = Some(self.dock.read(cx).dump(cx));
        if let Err(e) = layout::save(&self.saved) {
            self.raise_toast(
                ToastKind::Error,
                crate::strings::COULD_NOT_SAVE_LAYOUT,
                e.to_string(),
                cx,
            );
        }
    }

    /// New panel for `id` in the center (first tab group).
    pub(crate) fn add_panel(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPanel> {
        let panel = new_panel(id, window, cx);
        let handle = panel_handle(panel.clone());
        self.dock.update(cx, |a, cx| {
            a.add_panel_view(handle, DockPlacement::Center, None, window, cx)
        });
        let roots = self.link_roots(&panel.read(cx).terminal_id);
        panel.update(cx, |p, cx| p.set_link_roots(roots, cx));
        if let Some(t) = self.terminal(&panel.read(cx).terminal_id) {
            let agent = crate::agent_kind::identify_in(t, &self.tasks).kind;
            panel.update(cx, |p, cx| p.set_agent(agent, cx));
        }
        panel
    }

    /// Show terminal `id`: focus an existing panel, or open one (always a new one with `another`).
    pub fn open_terminal(
        &mut self,
        id: &str,
        another: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = if another {
            None
        } else {
            self.live_panels()
                .into_iter()
                .find(|p| p.read(cx).terminal_id == id)
        };
        let panel = match existing {
            Some(p) => {
                let pid = PanelId::from(p.entity_id());
                self.dock
                    .update(cx, |a, cx| a.select_panel(pid, window, cx));
                p
            }
            None => self.add_panel(id.to_string(), window, cx),
        };
        let focus = panel.read(cx).view.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    }

    /// Rebuild the center as a near-square grid of the open panels (2x2 for 4).
    pub fn arrange_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let panels = self.live_panels();
        if panels.is_empty() {
            return;
        }
        let (_, cols) = layout::grid_shape(panels.len());
        let mut v = DockLayout::v_split();
        for row in panels.chunks(cols) {
            let mut h = DockLayout::h_split();
            for p in row {
                h = h.child(
                    DockLayout::tabs().panel_view(panel_handle(p.clone()), cx),
                    None,
                );
            }
            v = v.child(h, None);
        }
        self.dock.update(cx, |a, cx| a.set_center(v, window, cx));
    }

    pub fn panel_removed(&mut self, panel: &WeakEntity<TerminalPanel>, cx: &mut Context<Self>) {
        self.panels
            .borrow_mut()
            .retain(|w| w != panel && w.upgrade().is_some());
        self.schedule_save(cx);
        self.touch(cx);
    }

    pub(crate) fn close_panels_of(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panels: Vec<_> = self
            .live_panels()
            .into_iter()
            .filter(|p| p.read(cx).terminal_id == id)
            .collect();
        for p in panels {
            self.dock.update(cx, |a, cx| a.remove_panel(p, window, cx));
        }
    }

    // ---- review (P4, docs/09 §7) -------------------------------------------

    pub(crate) fn live_reviews(&self) -> Vec<Entity<ReviewPanel>> {
        self.reviews.iter().filter_map(|w| w.upgrade()).collect()
    }

    /// Show the Review panel of `task_id` (focus the open one, else open it in the center).
    pub fn open_review(&mut self, task_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.caps.review {
            self.raise_toast(
                ToastKind::Warning,
                crate::strings::REVIEW_NEEDS_A_NEWER_PODD,
                crate::strings::PODD_PROTOCOL_V5_REVIEW_IS_REQUIRED,
                cx,
            );
            return;
        }
        let existing = self
            .live_reviews()
            .into_iter()
            .find(|r| r.read(cx).task_id == task_id);
        let panel = match existing {
            Some(p) => {
                let pid = PanelId::from(p.entity_id());
                self.dock
                    .update(cx, |a, cx| a.select_panel(pid, window, cx));
                p
            }
            None => {
                let Some(task) = self.task(task_id).cloned() else {
                    self.raise_toast(
                        ToastKind::Error,
                        crate::strings::NO_SUCH_TASK,
                        task_id.to_string(),
                        cx,
                    );
                    return;
                };
                let (source, ws) = (self.source.clone(), cx.entity().downgrade());
                let panel = cx.new(|cx| ReviewPanel::new(source, ws, &task, window, cx));
                // Seed the agent status for queue/turn hints.
                if let Some(s) = task
                    .terminals
                    .first()
                    .and_then(|t| self.terminal(t))
                    .and_then(|t| t.status.clone())
                {
                    let term = task.terminals[0].clone();
                    panel.update(cx, |r, cx| r.agent_status(&term, &s, cx));
                }
                self.reviews.push(panel.downgrade());
                self.dock.update(cx, |a, cx| {
                    a.add_panel_view(
                        panel_handle(panel.clone()),
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    )
                });
                panel
            }
        };
        let focus = panel.read(cx).focus_handle();
        window.focus(&focus, cx);
        self.touch(cx);
    }

    /// The Review panel of `task_id` on its PR tab (docs/15); `form` opens
    /// "브랜치 푸시 및 PR 만들기" when the task has no pull request yet.
    pub fn open_pr(
        &mut self,
        task_id: &str,
        form: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_review(task_id, window, cx);
        if let Some(r) = self
            .live_reviews()
            .into_iter()
            .find(|r| r.read(cx).task_id == task_id)
        {
            r.update(cx, |r, cx| r.show_pr(form, window, cx));
        }
    }

    pub fn review_removed(&mut self, panel: &WeakEntity<ReviewPanel>, cx: &mut Context<Self>) {
        self.reviews.retain(|w| w != panel && w.upgrade().is_some());
        self.touch(cx);
    }
}
