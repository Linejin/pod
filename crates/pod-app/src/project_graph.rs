//! Project agent graph (docs/09 §8.1): the agents of one registered
//! repository, the parent/child links podd recorded between them and their
//! work status, drawn as a graph in one dock panel.
//!
//! Opened from the "에이전트 그래프" action of a sidebar repository row; one
//! panel follows the repository chosen last. It reads the workspace's task and
//! terminal snapshot on every render and is repainted by the same source events
//! as the agent tree ([`Workspace::orch_data_changed`]): no polling, no RPC.
//! The data side (membership, edges, layout) is [`model`].

pub mod copy;
pub mod model;
#[cfg(test)]
mod qa;
#[cfg(test)]
mod tests;

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::component::dock::{DockPlacement, PanelId, panel_handle};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde_json::json;

use crate::orch::{mode, tc, tokens};
use crate::qa_marks::QaId;
use crate::workspace::Workspace;
use model::{EdgeKind, GraphNode, ProjectGraph};

pub const PROJECT_GRAPH_PANEL_NAME: &str = "PodProjectAgentGraph";

/// Node card and spacing, in logical pixels.
const NODE_W: f32 = 236.;
/// The narrowest (compact) card, for a graph column narrower than one full card.
const NODE_MIN_W: f32 = 148.;
const NODE_H: f32 = 96.;
const GAP_X: f32 = 28.;
const GAP_Y: f32 = 52.;
const PAD: f32 = 16.;
const LINE: f32 = 2.;
/// The right dock's share of the dock area when the graph opens it.
const DOCK_W: f32 = 480.;

fn node_y(depth: usize) -> f32 {
    PAD + depth as f32 * (NODE_H + GAP_Y)
}

/// Horizontal geometry of the cards: one card fits the visible graph width
/// with its padding, between [`NODE_MIN_W`] and [`NODE_W`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Geom {
    pub(crate) node_w: f32,
}

impl Geom {
    /// For a graph whose visible width is `visible` (`None`: not measured yet).
    pub(crate) fn for_width(visible: Option<f32>) -> Self {
        let node_w = match visible {
            Some(w) if w > 0. => (w - 2. * PAD).clamp(NODE_MIN_W, NODE_W),
            _ => NODE_W,
        };
        Self { node_w }
    }

    pub(crate) fn x(&self, col: f32) -> f32 {
        PAD + col * (self.node_w + GAP_X)
    }

    fn center(&self, n: &GraphNode) -> f32 {
        self.x(n.col) + self.node_w / 2.
    }

    fn width(&self, cols: usize) -> f32 {
        PAD * 2. + cols.max(1) as f32 * (self.node_w + GAP_X) - GAP_X
    }
}

/// Scroll offset along one axis that shows the span `[start, start + len)`
/// (content coordinates) in a viewport of `view` px, a padding away from the
/// edge when there is room. A span longer than the viewport shows its start.
pub(crate) fn reveal_axis(off: f32, start: f32, len: f32, view: f32) -> f32 {
    let pad = PAD.min(((view - len) / 2.).max(0.));
    if start + off < pad || len > view {
        pad - start
    } else if start + len + off > view - pad {
        view - pad - start - len
    } else {
        off
    }
}

/// The width the right dock gets when the graph creates it, from the dock
/// area's measured width (`area` 0: not laid out yet → [`DOCK_W`]): half of
/// it, so the centre keeps room at small window sizes, but at least one whole
/// card, at most [`DOCK_W`], and never wider than the area itself (a dock
/// that overflows its column is clipped by it).
pub(crate) fn dock_width(area: f32) -> f32 {
    if area <= 0. {
        return DOCK_W;
    }
    let one_card = NODE_W + 2. * PAD;
    (area / 2.)
        .max(one_card)
        .min(DOCK_W)
        .min(area)
        .max(f32::from(gpui_kit::base::PANEL_MIN_SIZE))
}

/// The panel's box and the part of it actually on screen (the ancestors' clip:
/// a dock wider than its column, a scrolled or clipped layout region).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Viewport {
    pub(crate) area: Bounds<Pixels>,
    pub(crate) visible: Bounds<Pixels>,
}

impl Viewport {
    fn measured(&self) -> bool {
        self.visible.size.width > px(0.) && self.visible.size.height > px(0.)
    }
}

/// The graph of `repo` from the workspace's current snapshot.
pub fn snapshot(ws: &Workspace, repo: &str) -> ProjectGraph {
    let registered = ws.repos.iter().any(|r| r.name == repo);
    let label = |t: &crate::vm::TerminalVm| ws.label_of(t);
    model::build(repo, registered, &ws.tasks, &ws.terminals, &label)
}

pub struct ProjectGraphPanel {
    workspace: WeakEntity<Workspace>,
    /// The repository shown (its `RepoVm.name` / `TaskVm.repo`).
    pub(crate) repo: String,
    /// The panel's handle (tracked by the dock's tab group: not a tab stop).
    focus: FocusHandle,
    /// The graph: the tab stop with arrow-key navigation.
    graph_focus: FocusHandle,
    _forward_focus: Option<Subscription>,
    /// Selected node (terminal id): survives status changes and reordering.
    pub(crate) selected: Option<String>,
    /// Last thing the panel could not do (a node podd no longer lists).
    pub(crate) notice: Option<String>,
    scroll: ScrollHandle,
    /// Measured on every prepaint; the content is laid out in its visible part.
    pub(crate) viewport: Rc<Cell<Viewport>>,
    /// Bring the selected node back into view on the next render (the visible
    /// width, and with it the card width, changed).
    reveal_pending: bool,
    /// The external A2A peer section, while shown ([`crate::project_a2a`]).
    pub(crate) a2a: Option<Entity<crate::project_a2a::A2aSection>>,
    /// The header's A2A toggle (a tab stop before the graph).
    pub(crate) a2a_toggle: FocusHandle,
}

impl ProjectGraphPanel {
    pub fn new(workspace: WeakEntity<Workspace>, repo: String, cx: &mut Context<Self>) -> Self {
        Self {
            workspace,
            repo,
            focus: cx.focus_handle(),
            graph_focus: cx.focus_handle().tab_stop(true),
            _forward_focus: None,
            selected: None,
            notice: None,
            scroll: ScrollHandle::new(),
            viewport: Rc::default(),
            reveal_pending: false,
            a2a: None,
            a2a_toggle: cx.focus_handle().tab_stop(true),
        }
    }

    /// The card geometry for the current visible width.
    pub(crate) fn geom(&self) -> Geom {
        let v = self.viewport.get();
        Geom::for_width(v.measured().then(|| f32::from(v.visible.size.width)))
    }

    /// Focusing the panel (its dock tab, the sidebar action) lands in the graph.
    pub fn forward_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._forward_focus = Some(cx.on_focus(&self.focus, window, |p, window, cx| {
            p.graph_focus.focus(window, cx)
        }));
    }

    /// Show another repository (keeps the panel and its place in the dock).
    /// `true` when the repository changed: the saved layout is then stale and
    /// the caller must persist it. Nothing subscribes to the panel's own
    /// `PanelEvent`s: the workspace saves on the dock's `DockEvent`s only, and a
    /// retarget changes no dock structure.
    pub fn set_repo(&mut self, repo: &str, cx: &mut Context<Self>) -> bool {
        let changed = self.repo != repo;
        if changed {
            self.repo = repo.to_string();
            self.selected = None;
            self.notice = None;
            self.scroll.set_offset(point(px(0.), px(0.)));
            cx.emit(PanelEvent::LayoutChanged);
            self.show_a2a_viewer(cx);
        }
        cx.notify();
        changed
    }

    /// Shows or hides the external A2A peer section of this project.
    pub(crate) fn toggle_a2a(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.a2a.take() {
            s.update(cx, |s, cx| s.set_viewer(None, cx));
        } else if let Some(ws) = self.workspace.upgrade() {
            self.a2a = Some(ws.update(cx, |w, cx| w.a2a_section(cx)));
            self.show_a2a_viewer(cx);
        }
        cx.notify();
    }

    fn show_a2a_viewer(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.a2a.clone() {
            let me = cx.entity().downgrade();
            let repo = self.repo.clone();
            s.update(cx, |s, cx| s.set_viewer(Some((me, repo)), cx));
        }
    }

    /// Open (or focus) the node's terminal; a node podd no longer lists only
    /// says so.
    pub(crate) fn activate(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = Some(id.clone());
        let Some(ws) = self.workspace.upgrade() else {
            return;
        };
        if ws.read(cx).terminal(&id).is_none() {
            self.notice = Some(copy::cannot_open(&id));
            cx.notify();
            return;
        }
        self.notice = None;
        cx.defer_in(window, move |_, window, cx| {
            ws.update(cx, |w, cx| w.open_terminal(&id, false, window, cx))
        });
        cx.notify();
    }

    /// Scroll node `n` into view. The scroll area is laid out inside the
    /// visible part of the panel (see `render`); its width is also capped by
    /// the latest measurement, which can be a frame newer than the layout.
    fn reveal(&self, n: &GraphNode, g: Geom) {
        let view = self.scroll.bounds().size;
        let mut vw = f32::from(view.width);
        let vp = self.viewport.get();
        if vp.measured() {
            vw = vw.min(f32::from(vp.visible.size.width));
        }
        let vh = f32::from(view.height);
        if vw <= 0. || vh <= 0. {
            return;
        }
        let off = self.scroll.offset();
        let ox = reveal_axis(f32::from(off.x), g.x(n.col), g.node_w, vw);
        let oy = reveal_axis(f32::from(off.y), node_y(n.depth), NODE_H, vh);
        self.scroll.set_offset(point(px(ox), px(oy)));
    }

    /// Record the panel's box and its on-screen part (prepaint); a changed
    /// visible size re-renders the panel and keeps the selected node in view.
    fn measure(
        viewport: &Rc<Cell<Viewport>>,
        me: &WeakEntity<Self>,
        area: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let visible = area.intersect(&window.content_mask().bounds);
        let next = Viewport { area, visible };
        let before = viewport.replace(next);
        let shape = |v: Viewport| {
            (
                v.area.size,
                v.visible.size,
                v.visible.origin - v.area.origin,
            )
        };
        if shape(before) != shape(next) {
            let me = me.clone();
            cx.defer(move |cx| {
                let _ = me.update(cx, |p, cx| {
                    p.reveal_pending = true;
                    cx.notify();
                });
            });
        }
    }
}

impl EventEmitter<PanelEvent> for ProjectGraphPanel {}

impl Focusable for ProjectGraphPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for ProjectGraphPanel {
    fn panel_name(&self) -> &'static str {
        PROJECT_GRAPH_PANEL_NAME
    }

    fn dump(&self, _cx: &App) -> PanelState {
        let mut s = PanelState::new(PROJECT_GRAPH_PANEL_NAME);
        s.info = PanelInfo::panel(json!({ "repo": self.repo }));
        s
    }

    fn on_removed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.a2a.take() {
            s.update(cx, |s, cx| s.set_viewer(None, cx));
        }
        if let Some(ws) = self.workspace.upgrade() {
            cx.defer(move |cx| ws.update(cx, |w, cx| w.orch_panel_removed(cx)));
        }
    }
}

impl gpui_kit::component::dock::Panel for ProjectGraphPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(copy::tab_name(&self.repo).into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(copy::tab_name(&self.repo))
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

/// What one node card shows, gathered before drawing.
struct Card {
    node: GraphNode,
    badge: Option<pod_ui::StatusBadge>,
    status: String,
    line2: String,
    line3: Option<(String, bool)>,
    spoken: String,
}

fn cards(g: &ProjectGraph, ws: &Workspace) -> Vec<Card> {
    let connected = ws.conn.is_connected();
    g.nodes
        .iter()
        .map(|n| {
            let badge = ws.terminal(&n.id).map(|t| {
                let mut b = crate::orch::terminal_badge(t);
                if !connected {
                    b.stale_since = Some(crate::strings::CONNECTION_LOST.into());
                }
                b
            });
            let status = match &badge {
                Some(b) => b.label(),
                None => copy::NOT_LISTED.into(),
            };
            let line2 = format!(
                "{} · {}",
                n.kind.name(),
                copy::task_text(n.task_title.as_deref(), n.group.as_deref())
            );
            let line3 = match (&n.note, n.parent, n.via) {
                (Some(note), _, _) => Some((copy::note_text(note), true)),
                (None, Some(p), Some(via)) => {
                    Some((copy::parent_text(&node_title(&g.nodes[p]), via), false))
                }
                _ => None,
            };
            let mut spoken = format!(
                "{} {} · {} · {status}",
                n.kind.accessible_name(),
                node_title(n),
                copy::task_text(n.task_title.as_deref(), n.group.as_deref()),
            );
            if let Some((text, _)) = &line3 {
                spoken = format!("{spoken} · {text}");
            }
            if !n.children.is_empty() {
                spoken = format!("{spoken} · {}", copy::children_text(n.children.len()));
            }
            Card {
                node: n.clone(),
                badge,
                status,
                line2,
                line3,
                spoken,
            }
        })
        .collect()
}

/// `리더 (t1)`; the id alone when the name is the id.
fn node_title(n: &GraphNode) -> String {
    if n.name == n.id {
        n.id.clone()
    } else {
        format!("{} ({})", n.name, n.id)
    }
}

fn edge_color(kind: EdgeKind, t: &pod_ui::Tokens) -> u32 {
    match kind {
        EdgeKind::StartedTask => t.accent_fg,
        EdgeKind::RanTerminal => t.info_fg,
    }
}

/// Elbow connectors parent → children: a stem down from the parent, a bar
/// across the children, a drop into each child (coloured by its relation).
fn connectors(g: &ProjectGraph, geom: Geom, t: &pod_ui::Tokens) -> Vec<AnyElement> {
    let seg = |x: f32, y: f32, w: f32, h: f32, color: u32| {
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .bg(tc(color))
    };
    let mut out = Vec::new();
    for (i, n) in g.nodes.iter().enumerate() {
        if n.children.is_empty() {
            continue;
        }
        let px_center = |m: &GraphNode| geom.center(m);
        let top = node_y(n.depth) + NODE_H;
        let mid = top + GAP_Y / 2.;
        let cx = px_center(n);
        out.push(seg(cx - LINE / 2., top, LINE, GAP_Y / 2., t.border_strong).into_any_element());
        let xs: Vec<f32> = n.children.iter().map(|&c| px_center(&g.nodes[c])).collect();
        let lo = xs.iter().copied().fold(cx, f32::min);
        let hi = xs.iter().copied().fold(cx, f32::max);
        if hi > lo {
            out.push(
                seg(
                    lo - LINE / 2.,
                    mid - LINE / 2.,
                    hi - lo + LINE,
                    LINE,
                    t.border_strong,
                )
                .into_any_element(),
            );
        }
        for &c in &n.children {
            let child = &g.nodes[c];
            let color = edge_color(child.via.unwrap_or(EdgeKind::StartedTask), t);
            let (from, to) = (g.nodes[i].id.clone(), child.id.clone());
            out.push(
                seg(
                    px_center(child) - LINE / 2.,
                    mid - LINE / 2.,
                    LINE,
                    node_y(child.depth) - mid + LINE / 2.,
                    color,
                )
                .qa_id(move || format!("graph-edge:{from}->{to}"))
                .into_any_element(),
            );
        }
    }
    out
}

impl Render for ProjectGraphPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let m = mode();
        let Some(ws) = self.workspace.upgrade() else {
            return div().into_any_element();
        };
        let (g, cards, conn, status_cap) = {
            let w = ws.read(cx);
            let g = snapshot(w, &self.repo);
            let cards = cards(&g, w);
            (g, cards, w.conn.clone(), w.caps.status)
        };
        let selected = self.selected.as_deref().and_then(|id| g.index_of(id));
        let me = cx.entity().downgrade();
        let geom = self.geom();
        if std::mem::take(&mut self.reveal_pending)
            && let Some(i) = selected
        {
            self.reveal(&g.nodes[i], geom);
        }

        // Header: what the graph shows, counts, the two relations.
        let legend = |kind: EdgeKind| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().w(px(16.)).h(px(LINE)).bg(tc(edge_color(kind, &t))))
                .child(copy::edge_kind(kind))
        };
        let header = div()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_b_1()
            .border_color(tc(t.border_subtle))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(pod_ui::icon_sized(
                        pod_ui::Icon::FolderGit2,
                        14.,
                        t.text_muted,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.repo.clone()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(tc(t.text_muted))
                            .child(copy::summary(g.tasks, g.nodes.len(), g.edges.len())),
                    )
                    .child(crate::project_a2a::toggle(
                        self.a2a.is_some(),
                        &self.a2a_toggle,
                        me.clone(),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(tc(t.text_muted))
                    .child(copy::HINT),
            )
            .child(
                div()
                    .flex()
                    .gap_3()
                    .text_xs()
                    .text_color(tc(t.text_secondary))
                    .child(legend(EdgeKind::StartedTask))
                    .child(legend(EdgeKind::RanTerminal)),
            );

        // Banners: daemon state first (stale data), then data problems.
        let banner = |id: &'static str, icon: pod_ui::Icon, text: String, fg: u32, bg: u32| {
            div()
                .id(id)
                .qa_id(move || id.to_string())
                .role(Role::Status)
                .aria_label(text.clone())
                .mx_3()
                .mt_2()
                .px_2()
                .py_1()
                .rounded_sm()
                .flex()
                .items_center()
                .gap_2()
                .text_xs()
                .bg(tc(bg))
                .text_color(tc(fg))
                .child(pod_ui::icon_sized(icon, 14., fg))
                .child(div().flex_1().min_w_0().child(text))
        };
        let mut banners: Vec<AnyElement> = Vec::new();
        match &conn {
            crate::vm::ConnState::Connected { .. } => {}
            crate::vm::ConnState::Connecting => banners.push(
                banner(
                    "graph-connecting",
                    pod_ui::Icon::LoaderCircle,
                    copy::CONNECTING.into(),
                    t.info_fg,
                    t.info_bg,
                )
                .into_any_element(),
            ),
            crate::vm::ConnState::Reconnecting { .. } => banners.push(
                banner(
                    "graph-disconnected",
                    pod_ui::Icon::WifiOff,
                    copy::DISCONNECTED.into(),
                    t.warning_fg,
                    t.warning_bg,
                )
                .into_any_element(),
            ),
        }
        if conn.is_connected() && !status_cap && !g.nodes.is_empty() {
            banners.push(
                banner(
                    "graph-no-status",
                    pod_ui::Icon::CircleHelp,
                    copy::NO_STATUS.into(),
                    t.text_secondary,
                    t.bg_raised,
                )
                .into_any_element(),
            );
        }
        if !g.duplicates.is_empty() {
            banners.push(
                banner(
                    "graph-duplicates",
                    pod_ui::Icon::TriangleAlert,
                    copy::duplicates(&g.duplicates),
                    t.warning_fg,
                    t.warning_bg,
                )
                .into_any_element(),
            );
        }
        if let Some(n) = &self.notice {
            banners.push(
                banner(
                    "graph-notice",
                    pod_ui::Icon::Info,
                    n.clone(),
                    t.text_secondary,
                    t.bg_raised,
                )
                .into_any_element(),
            );
        }

        let body: AnyElement = if g.nodes.is_empty() {
            let (title, text) = if g.known {
                (copy::EMPTY_TITLE, copy::EMPTY_BODY)
            } else {
                (copy::UNKNOWN_TITLE, copy::UNKNOWN_BODY)
            };
            let id = if g.known {
                "graph-empty"
            } else {
                "graph-unknown"
            };
            div()
                .p_3()
                .qa_id(move || id.to_string())
                .child(pod_ui::Frame::new(id, pod_ui::FrameKind::Empty, title, text).render(m, []))
                .into_any_element()
        } else {
            let width = geom.width(g.cols);
            let height = PAD * 2. + g.rows.max(1) as f32 * (NODE_H + GAP_Y) - GAP_Y;
            let nodes: Vec<AnyElement> = cards
                .into_iter()
                .enumerate()
                .map(|(i, c)| {
                    let n = &c.node;
                    let on = selected == Some(i);
                    let id = n.id.clone();
                    let open = me.clone();
                    let qa = n.id.clone();
                    div()
                        .id(SharedString::from(format!("graph-node-{}", n.id)))
                        .qa_id(move || format!("graph-node:{qa}"))
                        .role(Role::TreeItem)
                        .aria_label(c.spoken.clone())
                        .aria_selected(on)
                        .absolute()
                        .left(px(geom.x(n.col)))
                        .top(px(node_y(n.depth)))
                        .w(px(geom.node_w))
                        .h(px(NODE_H))
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .rounded_md()
                        .overflow_hidden()
                        .bg(tc(if on { t.bg_selected } else { t.bg_raised }))
                        .border_color(tc(if on { t.accent_fg } else { t.border_subtle }))
                        .when(on, |d| d.border_2())
                        .when(!on, |d| d.border_1())
                        .when(n.present, |d| {
                            d.cursor_pointer().hover(|s| s.bg(tc(t.bg_hover)))
                        })
                        .when(!n.present, |d| d.opacity(0.75))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .min_w_0()
                                .child(crate::agent_kind::mark_el(
                                    &format!("graph-{}", n.id),
                                    &n.kind,
                                    pod_ui::tokens::ICON_SIZE,
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(n.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(tc(t.text_muted))
                                        .child(n.id.clone()),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .truncate()
                                .text_color(tc(t.text_secondary))
                                .child(c.line2.clone()),
                        )
                        .child(match &c.badge {
                            Some(b) => div()
                                .flex()
                                .min_w_0()
                                .text_xs()
                                .child(b.render_compact(m))
                                .into_any_element(),
                            None => div()
                                .text_xs()
                                .truncate()
                                .text_color(tc(t.text_muted))
                                .child(c.status.clone())
                                .into_any_element(),
                        })
                        .when_some(c.line3.clone(), |d, (text, warn)| {
                            d.child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(tc(if warn { t.warning_fg } else { t.text_muted }))
                                    .child(text),
                            )
                        })
                        .on_click(move |_, window, cx| {
                            let _ = open.update(cx, |p, cx| {
                                p.graph_focus.focus(window, cx);
                                p.activate(id.clone(), window, cx)
                            });
                        })
                        .tooltip({
                            let tip = c.spoken.clone();
                            move |w, cx| {
                                gpui_kit::component::tooltip::Tooltip::new(tip.clone())
                                    .max_w(px(480.))
                                    .build(w, cx)
                            }
                        })
                        .into_any_element()
                })
                .collect();
            let keys = g.clone();
            div()
                .id("project-graph-scroll")
                .flex_1()
                .min_h_0()
                .overflow_scroll()
                .track_scroll(&self.scroll)
                .child(
                    div()
                        .id("project-graph")
                        .qa_id(|| "project-graph".into())
                        .track_focus(&self.graph_focus)
                        .tab_index(0)
                        .role(Role::Tree)
                        .aria_label(copy::graph_label(&self.repo, g.nodes.len(), g.edges.len()))
                        .focus_visible(|s| pod_ui::focus_style(s, m))
                        .on_key_down(cx.listener(move |p, e: &KeyDownEvent, window, cx| {
                            let at = p.selected.as_deref().and_then(|id| keys.index_of(id));
                            let key = e.keystroke.key.as_str();
                            if matches!(key, "enter" | "space") {
                                if let Some(i) = at {
                                    p.activate(keys.nodes[i].id.clone(), window, cx);
                                    cx.stop_propagation();
                                }
                                return;
                            }
                            if let Some(next) = model::graph_key(&keys, at, key) {
                                p.selected = Some(keys.nodes[next].id.clone());
                                p.reveal(&keys.nodes[next], p.geom());
                                cx.notify();
                                cx.stop_propagation();
                            }
                        }))
                        .relative()
                        .flex_shrink_0()
                        .w(px(width))
                        .h(px(height))
                        .children(connectors(&g, geom, &t))
                        .children(nodes),
                )
                .into_any_element()
        };

        // The relations as text (screen readers, and when the canvas is scrolled away).
        let connections: AnyElement = {
            let items: Vec<AnyElement> = g
                .edges
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    let text = copy::edge_text(
                        &node_title(&g.nodes[e.from]),
                        &node_title(&g.nodes[e.to]),
                        e.kind,
                    );
                    let qa = format!("graph-link:{}->{}", g.nodes[e.from].id, g.nodes[e.to].id);
                    div()
                        .id(("graph-link", i))
                        .qa_id(move || qa)
                        .role(Role::ListItem)
                        .aria_label(text.clone())
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(12.)).h(px(LINE)).bg(tc(edge_color(e.kind, &t))))
                        .child(div().flex_1().min_w_0().truncate().child(text))
                        .into_any_element()
                })
                .collect();
            div()
                .id("graph-connections")
                .role(Role::List)
                .aria_label(copy::CONNECTIONS)
                .flex_shrink_0()
                .max_h(px(140.))
                .overflow_y_scroll()
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .gap_1()
                .border_t_1()
                .border_color(tc(t.border_subtle))
                .text_xs()
                .text_color(tc(t.text_secondary))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tc(t.text_muted))
                        .child(copy::CONNECTIONS),
                )
                .when(items.is_empty(), |d| d.child(copy::NO_CONNECTIONS))
                .children(items)
                .into_any_element()
        };

        // The content takes only the part of the panel that is on screen: a
        // dock wider than its column (dense layout, small window) is clipped by
        // the column, and the graph must not scroll nodes into the clipped part.
        let vp = self.viewport.get();
        let content = div()
            .id("project-graph-content")
            .qa_id(|| "project-graph-content".into())
            .absolute()
            .map(|d| {
                if vp.measured() {
                    let at = vp.visible.origin - vp.area.origin;
                    d.left(at.x)
                        .top(at.y)
                        .w(vp.visible.size.width)
                        .h(vp.visible.size.height)
                } else {
                    d.top_0().left_0().size_full()
                }
            })
            .flex()
            .flex_col()
            .text_color(tc(t.text_primary))
            .child(header)
            .children(banners)
            .child(body)
            .when(!g.nodes.is_empty(), |d| d.child(connections))
            .when_some(self.a2a.clone(), |d, s| d.child(s));
        let (viewport, measure_me) = (self.viewport.clone(), me.clone());
        div()
            .id("project-graph-panel")
            .size_full()
            .relative()
            .overflow_hidden()
            .bg(tc(t.bg_surface))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        Self::measure(&viewport, &measure_me, bounds, window, cx)
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(content)
            .into_any_element()
    }
}

impl Workspace {
    /// Show the agent graph of `repo`: focus the open panel (now showing
    /// `repo`), else open it in the right dock.
    pub fn open_project_graph(&mut self, repo: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.orch.graph_panel.as_ref().and_then(|w| w.upgrade()) {
            if p.update(cx, |p, cx| p.set_repo(repo, cx)) {
                self.schedule_save(cx);
            }
            let pid = PanelId::from(p.entity_id());
            self.dock
                .update(cx, |a, cx| a.select_panel(pid, window, cx));
            p.read(cx).focus_handle(cx).focus(window, cx);
            cx.notify();
            return;
        }
        let ws = cx.entity().downgrade();
        let panel = cx.new(|cx| ProjectGraphPanel::new(ws, repo.to_string(), cx));
        panel.update(cx, |p, cx| p.forward_focus(window, cx));
        self.orch.graph_panel = Some(panel.downgrade());
        let focus = panel.read(cx).focus_handle(cx);
        // Seeds a new right dock only; an existing one keeps the user's width.
        let width = dock_width(f32::from(self.dock.read(cx).bounds().size.width));
        self.dock.update(cx, |a, cx| {
            a.add_panel_view(
                panel_handle(panel),
                DockPlacement::Right,
                Some(px(width)),
                window,
                cx,
            )
        });
        focus.focus(window, cx);
        cx.notify();
    }

    /// A repository row was selected in the sidebar: an open graph follows it
    /// (without taking focus). Must not touch the sidebar (it may be updating).
    pub(crate) fn follow_project_selection(&mut self, repo: &str, cx: &mut Context<Self>) {
        if let Some(p) = self.orch.graph_panel.as_ref().and_then(|w| w.upgrade())
            && p.update(cx, |p, cx| p.set_repo(repo, cx))
        {
            self.schedule_save(cx);
        }
    }

    /// The open graph panel.
    #[cfg(test)]
    pub(crate) fn project_graph_panel(&self) -> Option<Entity<ProjectGraphPanel>> {
        self.orch.graph_panel.as_ref().and_then(|w| w.upgrade())
    }
}
