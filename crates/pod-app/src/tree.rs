//! Agent tree (P5, docs/09 §8): parent/child relations as a tree with
//! aggregated status.
//!
//! What podd records (docs/08 §4.1, docs/11 §4): a task started from inside a
//! Pod terminal (`pod task new`, `pod group new`) gets that terminal as
//! `parent_terminal` (and its task as `parent_task`); group members share the
//! group. So the tree is
//!
//! ```text
//! task                       (root: started from the app or outside Pod)
//! └ terminal                 (the task's terminals)
//!   └ group                  (fan-out started from this terminal)
//!     └ member task
//!       └ terminal
//!   └ task                   (single task started from this terminal)
//!   └ terminal               (`pod run` from this terminal, v7 parent fields)
//! terminal                   (terminals outside tasks)
//! ```
//!
//! Every non-terminal node shows the summary of all terminals below it
//! (pod-ui `StatusSummary`, each terminal counted once). Plain `pod run`
//! terminals nest under `TerminalInfo.parent_terminal` (or, when that terminal
//! is gone, `parent_task`); ones without a known parent appear at the root.

use std::collections::{HashMap, HashSet};

use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{ControlState, Frame, FrameKind, StatusBadge, TreeRow};
use serde_json::json;

use crate::orch::{mode, summary_badge, summary_of, summary_short, tc, terminal_badge, tokens};
use crate::vm::{TaskVm, TerminalVm};
use crate::workspace::Workspace;

pub const TREE_PANEL_NAME: &str = "PodAgentTree";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Group(String),
    Task(String),
    Terminal(String),
}

impl NodeKind {
    fn key(&self) -> String {
        match self {
            NodeKind::Group(id) => format!("g:{id}"),
            NodeKind::Task(id) => format!("k:{id}"),
            NodeKind::Terminal(id) => format!("t:{id}"),
        }
    }
}

/// One visible row of the tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub kind: NodeKind,
    pub depth: usize,
    /// Every terminal in this subtree (for a terminal: itself + descendants).
    pub terminals: Vec<String>,
    pub has_children: bool,
}

/// Parent of each task/group/terminal, from podd's records.
fn parent_of(
    kind: &NodeKind,
    tasks: &[TaskVm],
    terminals: &[TerminalVm],
    groups: &HashMap<String, Option<String>>,
) -> Option<NodeKind> {
    let terminal_known = |t: &str| {
        terminals.iter().any(|x| x.id == t)
            || tasks.iter().any(|k| k.terminals.iter().any(|x| x == t))
    };
    match kind {
        NodeKind::Group(g) => groups
            .get(g)
            .cloned()
            .flatten()
            .filter(|t| terminal_known(t))
            .map(NodeKind::Terminal),
        NodeKind::Task(id) => {
            let t = tasks.iter().find(|t| &t.id == id)?;
            if let Some(g) = &t.group {
                return Some(NodeKind::Group(g.clone()));
            }
            if let Some(pt) = t.parent_terminal.as_deref().filter(|pt| terminal_known(pt)) {
                return Some(NodeKind::Terminal(pt.to_string()));
            }
            t.parent_task
                .as_ref()
                .filter(|p| *p != id && tasks.iter().any(|x| &x.id == *p))
                .map(|p| NodeKind::Task(p.clone()))
        }
        NodeKind::Terminal(tid) => {
            if let Some(k) = tasks.iter().find(|k| k.terminals.iter().any(|x| x == tid)) {
                return Some(NodeKind::Task(k.id.clone()));
            }
            // A loose terminal started by `pod run` inside a Pod terminal
            // (protocol v7): under that terminal while podd still lists it,
            // else under the parent's task. podd keeps both fields after exit.
            let t = terminals.iter().find(|x| &x.id == tid)?;
            if let Some(pt) = t
                .parent_terminal
                .as_deref()
                .filter(|pt| pt != tid && terminal_known(pt))
            {
                return Some(NodeKind::Terminal(pt.to_string()));
            }
            t.parent_task
                .as_ref()
                .filter(|p| tasks.iter().any(|x| &x.id == *p))
                .map(|p| NodeKind::Task(p.clone()))
        }
    }
}

/// Build the flattened tree. `collapsed` holds node keys whose children are hidden.
pub fn build(tasks: &[TaskVm], terminals: &[TerminalVm], collapsed: &HashSet<String>) -> Vec<Node> {
    // Groups: from their members; parent = the members' common parent terminal.
    let mut groups: HashMap<String, Option<String>> = HashMap::new();
    let mut group_order: Vec<String> = Vec::new();
    for t in tasks {
        if let Some(g) = &t.group {
            match groups.get_mut(g) {
                None => {
                    group_order.push(g.clone());
                    groups.insert(g.clone(), t.parent_terminal.clone());
                }
                Some(p) if *p != t.parent_terminal => *p = None,
                Some(_) => {}
            }
        }
    }
    let mut nodes: Vec<NodeKind> = group_order.iter().cloned().map(NodeKind::Group).collect();
    nodes.extend(tasks.iter().map(|t| NodeKind::Task(t.id.clone())));
    let mut seen_terms: HashSet<&str> = HashSet::new();
    for t in tasks {
        for x in &t.terminals {
            if seen_terms.insert(x) {
                nodes.push(NodeKind::Terminal(x.clone()));
            }
        }
    }
    for t in terminals {
        if seen_terms.insert(&t.id) {
            nodes.push(NodeKind::Terminal(t.id.clone()));
        }
    }

    let mut children: HashMap<String, Vec<NodeKind>> = HashMap::new();
    let mut roots: Vec<NodeKind> = Vec::new();
    for n in &nodes {
        match parent_of(n, tasks, terminals, &groups) {
            Some(p) => children.entry(p.key()).or_default().push(n.clone()),
            None => roots.push(n.clone()),
        }
    }
    // Roots: tasks (and groups started outside Pod) first, then loose terminals.
    roots.sort_by_key(|n| matches!(n, NodeKind::Terminal(_)));

    fn walk(
        n: &NodeKind,
        depth: usize,
        children: &HashMap<String, Vec<NodeKind>>,
        collapsed: &HashSet<String>,
        on_path: &mut HashSet<String>,
        reached: &mut HashSet<String>,
        out: &mut Vec<Node>,
    ) -> Vec<String> {
        let key = n.key();
        if !on_path.insert(key.clone()) {
            return Vec::new(); // cycle guard (records pointing at each other)
        }
        reached.insert(key.clone());
        let at = out.len();
        let kids = children.get(&key).cloned().unwrap_or_default();
        out.push(Node {
            kind: n.clone(),
            depth,
            terminals: Vec::new(),
            has_children: !kids.is_empty(),
        });
        let mut terms = match n {
            NodeKind::Terminal(id) => vec![id.clone()],
            _ => Vec::new(),
        };
        let mut sub = Vec::new();
        for k in &kids {
            terms.extend(walk(
                k,
                depth + 1,
                children,
                collapsed,
                on_path,
                reached,
                &mut sub,
            ));
        }
        if !collapsed.contains(&key) {
            out.append(&mut sub);
        }
        on_path.remove(&key);
        out[at].terminals = terms.clone();
        terms
    }
    let mut out = Vec::new();
    let mut on_path = HashSet::new();
    let mut reached = HashSet::new();
    for r in &roots {
        walk(
            r,
            0,
            &children,
            collapsed,
            &mut on_path,
            &mut reached,
            &mut out,
        );
    }
    // Records that point at each other (a parent cycle) have no root: show
    // each such node at the root instead of hiding it.
    for n in &nodes {
        if !reached.contains(&n.key()) {
            walk(
                n,
                0,
                &children,
                collapsed,
                &mut on_path,
                &mut reached,
                &mut out,
            );
        }
    }
    out
}

pub struct AgentTreePanel {
    workspace: WeakEntity<Workspace>,
    /// The panel's handle. The dock's tab group tracks it on its frame (before
    /// the tab bar), so it must not be a tab stop: registered twice, GPUI's
    /// Tab would reach the first copy and Shift+Tab leave from the second.
    focus: FocusHandle,
    /// The tree itself: the tab stop with arrow-key navigation.
    tree_focus: FocusHandle,
    _forward_focus: Option<Subscription>,
    collapsed: HashSet<String>,
    selected: Option<String>,
    scroll: ScrollHandle,
}

impl AgentTreePanel {
    pub fn new(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        // `.tab_index()` only applies to element-owned handles; a tracked handle must be a tab stop itself.
        Self {
            workspace,
            focus: cx.focus_handle(),
            tree_focus: cx.focus_handle().tab_stop(true),
            _forward_focus: None,
            collapsed: HashSet::new(),
            selected: None,
            scroll: ScrollHandle::new(),
        }
    }

    /// Focusing the panel (Ctrl+Shift+E, its dock tab) lands in the tree.
    pub fn forward_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._forward_focus = Some(cx.on_focus(&self.focus, window, |p, window, cx| {
            p.tree_focus.focus(window, cx)
        }));
    }

    fn activate(&mut self, kind: NodeKind, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(kind.key());
        let Some(ws) = self.workspace.upgrade() else {
            return;
        };
        cx.defer_in(window, move |_, window, cx| {
            ws.update(cx, |w, cx| match &kind {
                NodeKind::Terminal(id) => w.open_terminal(id, false, window, cx),
                NodeKind::Task(id) => w.open_task_dialog(id.clone(), window, cx),
                NodeKind::Group(id) => w.open_group(id, None, window, cx),
            })
        });
        cx.notify();
    }
}

impl EventEmitter<PanelEvent> for AgentTreePanel {}

impl Focusable for AgentTreePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for AgentTreePanel {
    fn panel_name(&self) -> &'static str {
        TREE_PANEL_NAME
    }

    fn dump(&self, _cx: &App) -> PanelState {
        let mut s = PanelState::new(TREE_PANEL_NAME);
        s.info = PanelInfo::panel(json!({}));
        s
    }

    fn on_removed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspace.upgrade() {
            cx.defer(move |cx| ws.update(cx, |w, cx| w.orch_panel_removed(cx)));
        }
    }
}

impl gpui_kit::component::dock::Panel for AgentTreePanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(crate::strings::AGENT.into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(crate::strings::AGENT)
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

/// Row data gathered from the workspace before rendering.
struct RowData {
    node: Node,
    label: String,
    detail: String,
    badge: Option<StatusBadge>,
    /// Agent kinds: the terminal's own, or a task's distinct agents (docs/10 §D5.11).
    kinds: Vec<crate::agent_kind::AgentKind>,
}

impl Render for AgentTreePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let rows: Vec<RowData> = match self.workspace.upgrade() {
            None => Vec::new(),
            Some(ws) => {
                let ws = ws.read(cx);
                build(&ws.tasks, &ws.terminals, &self.collapsed)
                    .into_iter()
                    .map(|node| {
                        let summary = summary_of(node.terminals.iter(), &ws.terminals);
                        let (label, detail, badge) = match &node.kind {
                            NodeKind::Group(id) => (
                                crate::tr!(GROUP_ID; id;),
                                summary_short(&summary),
                                summary_badge(&summary),
                            ),
                            NodeKind::Task(id) => {
                                let task = ws.task(id);
                                let agent = task.and_then(|t| t.agent.clone()).unwrap_or_default();
                                let branch =
                                    task.and_then(|t| t.branch.clone()).unwrap_or_default();
                                (
                                    id.to_string(),
                                    format!(
                                        "{} · {branch} · {}",
                                        short_program(&agent),
                                        summary_short(&summary)
                                    ),
                                    summary_badge(&summary),
                                )
                            }
                            NodeKind::Terminal(id) => match ws.terminal(id) {
                                Some(term) => {
                                    let detail = if node.terminals.len() > 1 {
                                        format!(
                                            "{} · {id} · 하위 {}",
                                            short_program(&term.program),
                                            summary_short(&summary_of(
                                                node.terminals.iter().skip(1),
                                                &ws.terminals
                                            ))
                                        )
                                    } else {
                                        format!("{} · {id}", short_program(&term.program))
                                    };
                                    let name = ws.label_of(term);
                                    let name =
                                        if name == term.program || name == term.argv.join(" ") {
                                            short_program(&name).to_string()
                                        } else {
                                            name
                                        };
                                    (name, detail, Some(terminal_badge(term)))
                                }
                                None => (
                                    id.to_string(),
                                    crate::strings::NOT_RUNNING_IN_PODD.into(),
                                    None,
                                ),
                            },
                        };
                        let kinds = match &node.kind {
                            NodeKind::Task(id) => ws
                                .task(id)
                                .map(|k| crate::agent_kind::task_kinds(k, &ws.terminals))
                                .unwrap_or_default(),
                            NodeKind::Terminal(id) => ws
                                .terminal(id)
                                .map(|t| vec![crate::agent_kind::identify_in(t, &ws.tasks).kind])
                                .unwrap_or_default(),
                            NodeKind::Group(_) => Vec::new(),
                        };
                        RowData {
                            node,
                            label,
                            detail,
                            badge,
                            kinds,
                        }
                    })
                    .collect()
            }
        };
        let me = cx.entity().downgrade();
        let keys: Vec<_> = rows
            .iter()
            .map(|r| (r.node.kind.clone(), r.node.has_children))
            .collect();
        let list: Vec<_> = rows
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let key = r.node.kind.key();
                let collapsed = self.collapsed.contains(&key);
                let (kind, me_open, me_fold, key_fold) =
                    (r.node.kind.clone(), me.clone(), me.clone(), key.clone());
                // Narrator name: the node type in Korean; the row shows a type icon.
                let agents = r
                    .kinds
                    .iter()
                    .map(|k| k.accessible_name())
                    .collect::<Vec<_>>()
                    .join(", ");
                let spoken = match &r.node.kind {
                    NodeKind::Task(_) if !agents.is_empty() => {
                        format!("작업 {} · {agents} · {}", r.label, r.detail)
                    }
                    NodeKind::Task(_) => format!("작업 {} · {}", r.label, r.detail),
                    NodeKind::Terminal(_) => {
                        format!("터미널 {} · {agents} · {}", r.label, r.detail)
                    }
                    NodeKind::Group(_) => format!("{} · {}", r.label, r.detail),
                };
                let glyph = match &r.node.kind {
                    NodeKind::Task(_) => pod_ui::Icon::Bot,
                    NodeKind::Terminal(_) => pod_ui::Icon::SquareTerminal,
                    NodeKind::Group(_) => pod_ui::Icon::Layers,
                };
                let row = TreeRow {
                    id: ElementId::Name(format!("tree-{key}").into()),
                    label: r.label.clone().into(),
                    depth: r.node.depth,
                    selected: self.selected.as_deref() == Some(key.as_str()),
                    state: ControlState::Ready,
                    badge: r.badge,
                    glyph: Some(glyph),
                    mark: match (&r.node.kind, r.kinds.as_slice()) {
                        (NodeKind::Terminal(_), [k]) => Some(
                            crate::agent_kind::mark_el(
                                &format!("tree-{key}"),
                                k,
                                pod_ui::tokens::ICON_SIZE,
                            )
                            .into_any_element(),
                        ),
                        (NodeKind::Task(_), [_, ..]) => Some(
                            crate::agent_kind::stack_el(&format!("tree-{key}"), &r.kinds, 14.)
                                .into_any_element(),
                        ),
                        _ => None,
                    },
                }
                .render(mode(), move |window, cx| {
                    let _ = me_open.update(cx, |p, cx| p.activate(kind.clone(), window, cx));
                })
                .aria_label(spoken.clone());
                let command = match &r.node.kind {
                    NodeKind::Terminal(id) => self.workspace.upgrade().and_then(|w| {
                        w.read(cx)
                            .terminal(id)
                            .map(|t| format!("{} {}", t.program, t.argv.join(" ")))
                    }),
                    NodeKind::Task(id) => self
                        .workspace
                        .upgrade()
                        .and_then(|w| w.read(cx).task(id).and_then(|t| t.agent.clone())),
                    _ => None,
                };
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id(("tree-fold", i))
                                    .w(px(18.))
                                    .text_xs()
                                    .text_color(tc(t.text_muted))
                                    .when(r.node.has_children, |d| {
                                        let (keyboard_owner, keyboard_key) =
                                            (me_fold.clone(), key_fold.clone());
                                        d.role(Role::Button)
                                            .aria_label(format!(
                                                "{} {}",
                                                spoken,
                                                if collapsed { "펼치기" } else { "접기" }
                                            ))
                                            .tab_index(0)
                                            .focus_visible(|s| pod_ui::focus_style(s, mode()))
                                            .on_key_down(move |e, _, cx| {
                                                if matches!(
                                                    e.keystroke.key.as_str(),
                                                    "enter" | "space"
                                                ) {
                                                    let _ = keyboard_owner.update(cx, |p, cx| {
                                                        if !p.collapsed.remove(&keyboard_key) {
                                                            p.collapsed
                                                                .insert(keyboard_key.clone());
                                                        }
                                                        cx.notify();
                                                    });
                                                    cx.stop_propagation();
                                                }
                                            })
                                            .cursor_pointer()
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(pod_ui::icon_sized(
                                                if collapsed {
                                                    pod_ui::Icon::ChevronRight
                                                } else {
                                                    pod_ui::Icon::ChevronDown
                                                },
                                                14.,
                                                t.text_muted,
                                            ))
                                            .on_click(move |_, _, cx| {
                                                let _ = me_fold.update(cx, |p, cx| {
                                                    if !p.collapsed.remove(&key_fold) {
                                                        p.collapsed.insert(key_fold.clone());
                                                    }
                                                    cx.notify();
                                                });
                                            })
                                    }),
                            )
                            .child(div().flex_1().min_w_0().child(row)),
                    )
                    .child(
                        div()
                            .id(("tree-detail", i))
                            .pl(px(18. + 8. + r.node.depth as f32 * 16. + 24.))
                            .pb_1()
                            .text_xs()
                            .text_color(tc(t.text_muted))
                            .truncate()
                            .child(r.detail)
                            .when_some(command, |d, command| {
                                d.tooltip(move |w, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(command.clone())
                                        .max_w(px(560.))
                                        .mt(px(28.))
                                        .build(w, cx)
                                })
                            }),
                    )
            })
            .collect();
        let empty = self.workspace.upgrade().is_none_or(|w| {
            let w = w.read(cx);
            w.tasks.is_empty() && w.terminals.is_empty()
        });
        div()
            .id("agent-tree")
            .track_focus(&self.tree_focus)
            .tab_index(0)
            .role(Role::Tree)
            .aria_label("에이전트 트리")
            .focus_visible(|s| pod_ui::focus_style(s, mode()))
            .on_key_down(cx.listener(move |p, e: &KeyDownEvent, window, cx| {
                if keys.is_empty() { return; }
                let index = keys.iter().position(|(k, _)| p.selected.as_deref() == Some(k.key().as_str())).unwrap_or(0);
                let next = match e.keystroke.key.as_str() {
                    "down" => Some((index + 1).min(keys.len() - 1)),
                    "up" => Some(index.saturating_sub(1)),
                    "left" => { if keys[index].1 { p.collapsed.insert(keys[index].0.key()); } Some(index) },
                    "right" => { p.collapsed.remove(&keys[index].0.key()); Some(index) },
                    "enter" => { p.activate(keys[index].0.clone(), window, cx); Some(index) },
                    _ => None,
                };
                if let Some(next) = next { p.selected = Some(keys[next].0.key()); p.scroll.scroll_to_item(next + 1); cx.notify(); cx.stop_propagation(); }
            }))
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .bg(tc(t.bg_surface))
            .text_color(tc(t.text_primary))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(tc(t.text_muted))
                    .border_b_1()
                    .border_color(tc(t.border_subtle))
                    .child(crate::strings::TASKS_AND_GROUPS_STARTED_FROM_INSIDE_A_POD_TERMINAL_APPEAR_UNDER_),
            )
            .when(empty, |d| {
                d.child(
                    div()
                        .p_3()
                        .child(Frame::new("tree-empty", FrameKind::Empty, crate::strings::NO_AGENTS_YET, crate::strings::CREATE_A_TASK_OR_A_GROUP_TO_SEE_IT_HERE).render(mode(), [])),
                )
            })
            .children(list)
    }
}

fn short_program(command: &str) -> &str {
    let command = command.trim();
    let executable = if let Some(quoted) = command.strip_prefix('"') {
        quoted.split('"').next().unwrap_or(quoted)
    } else {
        command.split_whitespace().next().unwrap_or(command)
    };
    executable.rsplit(['/', '\\']).next().unwrap_or(executable)
}

#[cfg(test)]
mod tests {
    use super::{Node, NodeKind, build};
    use crate::orch::summary_of;
    use crate::vm::{AgentState, Liveness, StatusVm, TaskVm, TerminalVm};
    use std::collections::HashSet;

    fn task(
        id: &str,
        terms: &[&str],
        parent_terminal: Option<&str>,
        group: Option<&str>,
    ) -> TaskVm {
        TaskVm {
            id: id.into(),
            title: id.into(),
            repo: Some("demo".into()),
            branch: Some(format!("pod/{id}")),
            base: None,
            agent: Some("shell".into()),
            worktree: None,
            worktree_exists: true,
            terminals: terms.iter().map(|s| s.to_string()).collect(),
            git: None,
            git_error: None,
            review_open: 0,
            status: None,
            parent_task: None,
            parent_terminal: parent_terminal.map(str::to_owned),
            group: group.map(str::to_owned),
            prompt: None,
            pr: None,
        }
    }

    fn term(id: &str, state: AgentState) -> TerminalVm {
        TerminalVm {
            id: id.into(),
            name: None,
            program: "powershell".into(),
            argv: vec![],
            cwd: String::new(),
            live: Liveness::Running,
            attached: 0,
            created_ms: 0,
            task_id: None,
            status: Some(StatusVm::of(state)),
            parent_terminal: None,
            parent_task: None,
            unreachable: None,
        }
    }

    fn shape(nodes: &[Node]) -> Vec<(String, usize)> {
        nodes.iter().map(|n| (n.kind.key(), n.depth)).collect()
    }

    #[test]
    fn group_started_from_a_terminal_nests_under_it_with_aggregate() {
        let tasks = vec![
            task("demo/lead", &["t1"], None, None),
            task("demo/fx-shell-1", &["t2"], Some("t1"), Some("demo/fx")),
            task("demo/fx-shell-2", &["t3"], Some("t1"), Some("demo/fx")),
            task("demo/child", &["t5"], Some("t2"), None),
        ];
        let terms = vec![
            term("t1", AgentState::Working),
            term("t2", AgentState::Done),
            term("t3", AgentState::Blocked),
            term("t4", AgentState::Idle),
            term("t5", AgentState::Working),
        ];
        let nodes = build(&tasks, &terms, &HashSet::new());
        let s: Vec<(String, usize)> = shape(&nodes);
        let expect = [
            ("k:demo/lead", 0),
            ("t:t1", 1),
            ("g:demo/fx", 2),
            ("k:demo/fx-shell-1", 3),
            ("t:t2", 4),
            ("k:demo/child", 5),
            ("t:t5", 6),
            ("k:demo/fx-shell-2", 3),
            ("t:t3", 4),
            ("t:t4", 0),
        ];
        assert_eq!(
            s,
            expect
                .iter()
                .map(|(k, d)| (k.to_string(), *d))
                .collect::<Vec<_>>()
        );
        // Aggregates: the lead task covers every terminal below it once.
        let lead = &nodes[0];
        assert_eq!(lead.terminals, ["t1", "t2", "t5", "t3"]);
        let summary = summary_of(lead.terminals.iter(), &terms);
        assert_eq!(
            (
                summary.total,
                summary.working,
                summary.blocked,
                summary.done
            ),
            (4, 2, 1, 1)
        );
        assert_eq!(
            summary.priority(),
            Some(pod_status::AgentState::Blocked(
                pod_status::BlockedKind::Other
            ))
        );
        let group = nodes
            .iter()
            .find(|n| n.kind == NodeKind::Group("demo/fx".into()))
            .unwrap();
        assert_eq!(group.terminals, ["t2", "t5", "t3"]);
    }

    #[test]
    fn collapse_hides_children_but_keeps_aggregate() {
        let tasks = vec![task("demo/a", &["t1", "t2"], None, None)];
        let terms = vec![
            term("t1", AgentState::Working),
            term("t2", AgentState::Idle),
        ];
        let collapsed: HashSet<String> = ["k:demo/a".to_string()].into();
        let nodes = build(&tasks, &terms, &collapsed);
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].has_children);
        assert_eq!(nodes[0].terminals, ["t1", "t2"]);
    }

    #[test]
    fn unknown_parents_and_cycles_fall_back_to_roots() {
        // Parent terminal gone; group members with different parents -> group at root.
        let mut a = task("demo/a", &["t1"], Some("t99"), None);
        a.parent_task = Some("demo/b".into());
        let mut b = task("demo/b", &["t2"], None, None);
        b.parent_task = Some("demo/a".into()); // cycle a <-> b
        let tasks = vec![
            a,
            b,
            task("demo/g-1", &[], Some("t1"), Some("demo/g")),
            task("demo/g-2", &[], Some("t2"), Some("demo/g")),
        ];
        let terms = vec![term("t1", AgentState::Idle), term("t2", AgentState::Idle)];
        let nodes = build(&tasks, &terms, &HashSet::new());
        // Cycle a <-> b: still shown (once each), no endless loop.
        for id in ["demo/a", "demo/b"] {
            assert_eq!(
                nodes
                    .iter()
                    .filter(|n| n.kind == NodeKind::Task(id.into()))
                    .count(),
                1,
                "{id}: {:?}",
                shape(&nodes)
            );
        }
        // The group has no common parent: root.
        assert!(
            nodes
                .iter()
                .any(|n| n.kind == NodeKind::Group("demo/g".into()) && n.depth == 0)
        );
    }

    #[test]
    fn pod_run_child_terminal_nests_under_parent_terminal_and_survives_exit() {
        // E2E B3: Codex t2 (task sample/app-fix) runs `pod run`, creating loose t5.
        let tasks = vec![
            task("sample/cli-fix", &["t1"], None, None),
            task("sample/app-fix", &["t2"], None, None),
        ];
        let mut child = term("t5", AgentState::Unknown);
        child.parent_terminal = Some("t2".into());
        child.parent_task = Some("sample/app-fix".into());
        let mut terms = vec![
            term("t1", AgentState::Idle),
            term("t2", AgentState::Working),
            child.clone(),
        ];
        let s = shape(&build(&tasks, &terms, &HashSet::new()));
        let want = |v: &[(&str, usize)]| {
            v.iter()
                .map(|(k, d)| (k.to_string(), *d))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            s,
            want(&[
                ("k:sample/cli-fix", 0),
                ("t:t1", 1),
                ("k:sample/app-fix", 0),
                ("t:t2", 1),
                ("t:t5", 2)
            ])
        );

        // Child exited: still under its parent.
        terms[2].live = Liveness::Exited {
            code: Some(7),
            killed: false,
        };
        assert_eq!(shape(&build(&tasks, &terms, &HashSet::new())), s);

        // Parent terminal removed from podd: falls back to the parent task.
        let terms2 = vec![term("t1", AgentState::Idle), child.clone()];
        let tasks2 = vec![
            task("sample/cli-fix", &["t1"], None, None),
            task("sample/app-fix", &[], None, None),
        ];
        let s2 = shape(&build(&tasks2, &terms2, &HashSet::new()));
        assert_eq!(
            s2,
            want(&[
                ("k:sample/cli-fix", 0),
                ("t:t1", 1),
                ("k:sample/app-fix", 0),
                ("t:t5", 1)
            ])
        );

        // Both gone: root, after tasks.
        let s3 = shape(&build(&[], &[child], &HashSet::new()));
        assert_eq!(s3, want(&[("t:t5", 0)]));
    }

    #[test]
    fn loose_terminal_chain_and_self_parent() {
        // `pod run` from a loose terminal that itself came from `pod run`.
        let mut a = term("t1", AgentState::Idle);
        a.parent_terminal = Some("t1".into()); // bogus self-parent: root
        let mut b = term("t2", AgentState::Idle);
        b.parent_terminal = Some("t1".into());
        let mut c = term("t3", AgentState::Idle);
        c.parent_terminal = Some("t2".into());
        let nodes = build(&[], &[a, b, c], &HashSet::new());
        assert_eq!(
            shape(&nodes),
            vec![
                ("t:t1".to_string(), 0),
                ("t:t2".to_string(), 1),
                ("t:t3".to_string(), 2)
            ]
        );
        assert_eq!(nodes[0].terminals, ["t1", "t2", "t3"]);
    }
}
