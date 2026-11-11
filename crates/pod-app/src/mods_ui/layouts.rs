//! Layout presets from mods (docs/16 §7.2): a preset's `center` tree
//! resolved to the workspace's panels and set as the center dock. Leaves
//! reuse open panels (each at most once) and open what is missing; gates and
//! the agent tree go to their usual right dock; open panels the preset does
//! not place are kept as tabs, never closed; empty leaves and splits drop
//! out, and a lone child takes its split's slot and size.

use std::sync::Arc;

use gpui_kit::component::dock::{DockLayout, panel_handle};
use gpui_kit::*;
use podd::protocol::{Axis, LayoutNode, LayoutPreset, PanelKind};

use super::copy::s;
use crate::workspace::{ToastKind, Workspace};

/// A preset resolved to panels.
pub(super) enum Built {
    Tabs(
        Vec<Arc<dyn gpui_kit::component::dock::BasePanelView>>,
        Option<f32>,
        bool,
    ),
    Split(Axis, Vec<Built>, Option<f32>),
}

impl Built {
    pub(super) fn size(&self) -> Option<f32> {
        match self {
            Built::Tabs(_, s, _) | Built::Split(_, _, s) => *s,
        }
    }

    pub(super) fn with_size(mut self, size: Option<f32>) -> Self {
        match &mut self {
            Built::Tabs(_, s, _) | Built::Split(_, _, s) => *s = size,
        }
        self
    }

    /// Append `extra` to the first terminal leaf (`any`: to the first leaf).
    fn adopt(
        &mut self,
        extra: &mut Vec<Arc<dyn gpui_kit::component::dock::BasePanelView>>,
        any: bool,
    ) -> bool {
        match self {
            Built::Tabs(panels, _, terminal) if *terminal || any => {
                panels.append(extra);
                true
            }
            Built::Tabs(..) => false,
            Built::Split(_, children, _) => children.iter_mut().any(|c| c.adopt(extra, any)),
        }
    }

    fn into_layout(self, extent: (f32, f32), cx: &App) -> DockLayout {
        match self {
            Built::Tabs(panels, _, _) => panels
                .into_iter()
                .fold(DockLayout::tabs(), |t, p| t.panel_view(p, cx)),
            Built::Split(axis, children, _) => {
                let (mut layout, len) = match axis {
                    Axis::Horizontal => (DockLayout::h_split(), extent.0),
                    Axis::Vertical => (DockLayout::v_split(), extent.1),
                };
                for c in children {
                    let size = c.size().map(|f| px(f * len));
                    layout = layout.child(c.into_layout(extent, cx), size);
                }
                layout
            }
        }
    }
}

/// Lay out the center region from a preset (docs/16 §7.2). Gates and the
/// agent tree open in their usual right dock; open terminal panels the
/// preset does not place go to the first terminal slot as tabs.
pub(super) fn apply_layout(
    ws: &mut Workspace,
    preset: &LayoutPreset,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let task = ws.current_task(window, cx);
    let focused = ws.focused_terminal.clone();
    let mut used: Vec<EntityId> = Vec::new();
    let built = build(ws, &preset.center, task.as_deref(), &mut used, window, cx);
    let Some(mut built) = built else {
        ws.raise_toast(ToastKind::Info, s::LAYOUT_EMPTY, s::LAYOUT_EMPTY_DETAIL, cx);
        return;
    };
    // Open panels the preset does not place are kept as tabs, never closed
    // (an editor may hold unsaved files): terminals in the first terminal
    // slot, review and editor panels after them.
    let mut rest: Vec<Arc<dyn gpui_kit::component::dock::BasePanelView>> = ws
        .live_panels()
        .into_iter()
        .filter(|p| !used.contains(&p.entity_id()))
        .map(|p| panel_handle(p))
        .collect();
    rest.extend(
        ws.live_reviews()
            .into_iter()
            .filter(|p| !used.contains(&p.entity_id()))
            .map(|p| panel_handle(p)),
    );
    rest.extend(
        ws.live_editors()
            .into_iter()
            .filter(|p| !used.contains(&p.entity_id()))
            .map(|p| panel_handle(p)),
    );
    if !rest.is_empty() && !built.adopt(&mut rest, false) {
        built.adopt(&mut rest, true);
    }
    let size = window.viewport_size();
    let layout = built.into_layout(
        (f32::from(size.width) * 0.8, f32::from(size.height) * 0.9),
        cx,
    );
    ws.dock.update(cx, |a, cx| a.set_center(layout, window, cx));
    // Opening gates/review moved keyboard focus; give it back to the
    // terminal the user was in (the preset rearranges, it does not navigate).
    if let Some(p) = focused.and_then(|id| {
        ws.live_panels()
            .into_iter()
            .find(|p| p.read(cx).terminal_id == id)
    }) {
        let f = p.read(cx).view.read(cx).focus_handle(cx);
        window.focus(&f, cx);
    }
    ws.schedule_save(cx);
    ws.touch(cx);
}

fn build(
    ws: &mut Workspace,
    node: &LayoutNode,
    task: Option<&str>,
    used: &mut Vec<EntityId>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Option<Built> {
    type Handle = Arc<dyn gpui_kit::component::dock::BasePanelView>;
    let leaf = |ws: &mut Workspace,
                panel: PanelKind,
                index: Option<usize>,
                used: &mut Vec<EntityId>,
                window: &mut Window,
                cx: &mut Context<Workspace>|
     -> Vec<Handle> {
        match panel {
            PanelKind::Terminal => {
                let i = index.unwrap_or(0);
                let id = match task.and_then(|t| ws.task(t)) {
                    Some(t) => t.terminals.get(i).cloned(),
                    None => ws
                        .live_panels()
                        .get(i)
                        .map(|p| p.read(cx).terminal_id.clone()),
                };
                let Some(id) = id else { return Vec::new() };
                if ws.terminal(&id).is_none() {
                    return Vec::new();
                }
                let existing = ws
                    .live_panels()
                    .into_iter()
                    .find(|p| p.read(cx).terminal_id == id && !used.contains(&p.entity_id()));
                let panel = match existing {
                    Some(p) => p,
                    None => {
                        ws.open_terminal(&id, true, window, cx);
                        let Some(p) = ws
                            .live_panels()
                            .into_iter()
                            .rev()
                            .find(|p| p.read(cx).terminal_id == id)
                        else {
                            return Vec::new();
                        };
                        p
                    }
                };
                used.push(panel.entity_id());
                vec![panel_handle(panel)]
            }
            PanelKind::Terminals => {
                let wanted: Option<Vec<String>> =
                    task.and_then(|t| ws.task(t)).map(|t| t.terminals.clone());
                let panels: Vec<_> = ws
                    .live_panels()
                    .into_iter()
                    .filter(|p| {
                        !used.contains(&p.entity_id())
                            && wanted
                                .as_ref()
                                .is_none_or(|w| w.contains(&p.read(cx).terminal_id))
                    })
                    .collect();
                used.extend(panels.iter().map(|p| p.entity_id()));
                panels.into_iter().map(|p| panel_handle(p)).collect()
            }
            PanelKind::Review => {
                let Some(t) = task else { return Vec::new() };
                ws.open_review(t, window, cx);
                let r = ws
                    .live_reviews()
                    .into_iter()
                    .find(|r| r.read(cx).task_id == t && !used.contains(&r.entity_id()));
                r.map(|r| {
                    used.push(r.entity_id());
                    vec![panel_handle(r)]
                })
                .unwrap_or_default()
            }
            PanelKind::Editor => {
                let Some(t) = task else { return Vec::new() };
                ws.open_editor(t, None, false, window, cx);
                let e = ws
                    .live_editors()
                    .into_iter()
                    .find(|e| e.read(cx).task_id == t && !used.contains(&e.entity_id()));
                e.map(|e| {
                    used.push(e.entity_id());
                    vec![panel_handle(e)]
                })
                .unwrap_or_default()
            }
            PanelKind::Gates => {
                ws.open_gates(window, cx);
                Vec::new()
            }
            PanelKind::Agents => {
                ws.open_agent_tree(window, cx);
                Vec::new()
            }
        }
    };
    match node {
        LayoutNode::Panel { panel, index, size } => {
            let panels = leaf(ws, *panel, *index, used, window, cx);
            let terminal = matches!(panel, PanelKind::Terminal | PanelKind::Terminals);
            (!panels.is_empty()).then_some(Built::Tabs(panels, *size, terminal))
        }
        LayoutNode::Tabs { tabs, size } => {
            let mut panels = Vec::new();
            let mut terminal = false;
            for t in tabs {
                if let LayoutNode::Panel { panel, index, .. } = t {
                    terminal |= matches!(panel, PanelKind::Terminal | PanelKind::Terminals);
                    panels.extend(leaf(ws, *panel, *index, used, window, cx));
                }
            }
            (!panels.is_empty()).then_some(Built::Tabs(panels, *size, terminal))
        }
        LayoutNode::Split {
            split,
            children,
            size,
        } => {
            let mut built: Vec<Built> = children
                .iter()
                .filter_map(|c| build(ws, c, task, used, window, cx))
                .collect();
            match built.len() {
                0 => None,
                // The lone child takes the split's slot, and its size.
                1 => built.pop().map(|b| b.with_size(*size)),
                _ => Some(Built::Split(*split, built, *size)),
            }
        }
    }
}
