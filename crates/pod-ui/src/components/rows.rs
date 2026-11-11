//! Tree rows and the small row decorations (status dot, count badge).
use super::buttons::ControlState;
use super::focus::focus_style;
use super::icons::{Icon, icon};
use crate::tokens::{TREE_INDENT, color};
use crate::{Mode, Tokens};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
/// Presentation row; host owns stable selection, expanded state and tree navigation.
pub struct TreeRow {
    pub id: ElementId,
    pub label: SharedString,
    pub depth: usize,
    pub selected: bool,
    pub state: ControlState,
    pub badge: Option<crate::StatusBadge>,
    /// Node type icon (task, terminal, group). The tree, not the row, owns the one chevron.
    pub glyph: Option<Icon>,
    /// Agent mark element (`agent_mark_el`, named `Claude 에이전트`); replaces `glyph` (docs/10 §D5.11).
    pub mark: Option<AnyElement>,
}
impl TreeRow {
    pub fn render(
        self,
        mode: Mode,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let t = Tokens::for_mode(mode);
        let enabled = self.state.interactive();
        let callback = std::rc::Rc::new(on_open);
        let key = callback.clone();
        let label = self.label.clone();
        div()
            .id(self.id)
            .role(Role::TreeItem)
            .aria_label(self.label.clone())
            .aria_selected(self.selected)
            .tab_index(if enabled { 0 } else { -1 })
            .h(px(crate::themes::metrics().density.row))
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(8. + self.depth as f32 * TREE_INDENT))
            .pr(px(8.))
            .border_l_2()
            .border_color(color(if self.selected {
                t.accent_fg
            } else {
                t.bg_surface
            }))
            .bg(color(if self.selected {
                t.bg_selected
            } else {
                t.bg_surface
            }))
            .text_size(px(13.))
            .text_color(color(if enabled {
                t.text_primary
            } else {
                t.text_disabled
            }))
            .when(enabled, |d| {
                d.hover(|s| s.bg(color(t.bg_hover)))
                    .focus_visible(|s| focus_style(s, mode))
            })
            .when_some(self.glyph.filter(|_| self.mark.is_none()), |d, g| {
                d.child(icon(g, t.text_secondary))
            })
            .when_some(self.mark, |d, m| d.child(m))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.label),
            )
            .when_some(self.badge, |d, b| d.child(b.render_compact(mode)))
            .tooltip(move |w, cx| {
                gpui_kit::component::tooltip::Tooltip::new(label.clone())
                    .mt(px(28.))
                    .build(w, cx)
            })
            .on_click(move |_, w, cx| {
                if enabled {
                    callback(w, cx);
                }
            })
            .on_key_down(move |e, w, cx| {
                if enabled && e.keystroke.key == "enter" {
                    key(w, cx);
                    cx.stop_propagation();
                }
            })
    }
}
/// 8px task status dot (docs/10 §D4.5): never the only carrier of meaning; pair it with a count or label.
pub fn status_dot(fg: u32) -> Div {
    div()
        .size(px(8.))
        .flex_shrink_0()
        .rounded_full()
        .bg(color(fg))
}
/// Small count badge (응답 수 · 실패 수) for task rows.
pub fn count_badge(n: usize, fg: u32, bg: u32) -> Div {
    div()
        .flex_shrink_0()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(5.))
        .rounded(px(9.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(fg))
        .bg(color(bg))
        .child(n.to_string())
}
