//! Dialog / palette / list / toast / empty frames, the inline banner and toast timing.
use super::buttons::ControlState;
use super::icons::{Icon, icon, icon_sized};
use super::text::{keep_all, keep_all_start};
use crate::tokens::{color, elevation};
use crate::{Mode, Tokens};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    Dialog,
    CommandPalette,
    List,
    Toast,
    Empty,
}
/// Common shell; content owns its input entities, validation and actions.
/// Modal frames require a focus handle and restore callback; Tab stays within supplied controls.
pub struct Frame {
    pub id: ElementId,
    pub kind: FrameKind,
    pub title: SharedString,
    pub description: SharedString,
    pub state: ControlState,
    /// 32px icon above an empty state's title (docs/10 §D4.6).
    pub icon: Option<Icon>,
}
impl Frame {
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn new(
        id: impl Into<ElementId>,
        kind: FrameKind,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            title: title.into(),
            description: description.into(),
            state: ControlState::Ready,
            icon: None,
        }
    }
    pub fn render(
        self,
        mode: Mode,
        children: impl IntoIterator<Item = AnyElement>,
    ) -> Stateful<Div> {
        let t = Tokens::for_mode(mode);
        let enabled = self.state.interactive();
        if self.kind == FrameKind::Empty {
            return self.render_empty(t, children);
        }
        let elevated = matches!(
            self.kind,
            FrameKind::Dialog | FrameKind::CommandPalette | FrameKind::Toast
        );
        let max_width = match self.kind {
            FrameKind::Dialog => 560.,
            FrameKind::CommandPalette => 640.,
            FrameKind::Toast => 360.,
            FrameKind::Empty => 420.,
            FrameKind::List => 720.,
        };
        let mut d = div()
            .id(self.id)
            .w_full()
            .max_w(px(max_width))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(if self.kind == FrameKind::Dialog {
                24.
            } else {
                16.
            }))
            .rounded(px(if elevated { 12. } else { 8. }))
            .border_1()
            .border_color(color(t.border_subtle))
            .bg(color(if elevated { t.bg_raised } else { t.bg_surface }))
            .text_color(color(t.text_primary))
            .text_size(px(13.))
            .line_height(px(20.))
            .when(elevated, |d| d.shadow(elevation(2, mode)))
            .child(
                div()
                    .text_size(px(
                        if self.kind == FrameKind::Empty || self.kind == FrameKind::Dialog {
                            20.
                        } else {
                            15.
                        },
                    ))
                    .child(self.title),
            )
            .child(
                div()
                    .text_color(color(t.text_muted))
                    .child(self.description),
            );
        d = match self.state {
            ControlState::Loading => d.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(icon(Icon::LoaderCircle, t.info_fg))
                    .child("불러오는 중…"),
            ),
            ControlState::Error(message) => d.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .text_color(color(t.danger_fg))
                    .child(icon(Icon::OctagonX, t.danger_fg))
                    .child(message),
            ),
            ControlState::Disabled => d.child("사용할 수 없음"),
            ControlState::Ready => d,
        };
        d.children(children).when(!enabled, |d| {
            d.capture_any_mouse_down(|_, w, cx| {
                w.prevent_default();
                cx.stop_propagation();
            })
            .capture_key_down(|e, w, cx| {
                if e.keystroke.key != "tab" {
                    w.prevent_default();
                    cx.stop_propagation();
                }
            })
        })
    }
}
impl Frame {
    /// Empty state (docs/10 §D4.6): no outline, max 360, icon 32, title 18/26 as a
    /// heading, description 13/20 wrapped between words; the caller adds one CTA.
    fn render_empty(
        self,
        t: Tokens,
        children: impl IntoIterator<Item = AnyElement>,
    ) -> Stateful<Div> {
        let title = self.title.clone();
        div()
            .id(self.id)
            .w_full()
            .max_w(px(360.))
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .p(px(16.))
            .text_color(color(t.text_primary))
            .text_size(px(13.))
            .line_height(px(20.))
            .when_some(self.icon, |d, i| d.child(icon_sized(i, 32., t.text_muted)))
            .child(
                keep_all(&self.title)
                    .id("frame-title")
                    .role(Role::Heading)
                    .aria_level(1)
                    .aria_label(title)
                    .text_size(px(18.))
                    .line_height(px(26.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(keep_all(&self.description).text_color(color(t.text_muted)))
            .children(children)
    }
}

/// Toast timing never resolves the underlying domain event. Error/blocked persist.
pub fn toast_timeout(
    tone: crate::Tone,
    hovered: bool,
    focused: bool,
) -> Option<std::time::Duration> {
    if hovered || focused {
        return None;
    }
    match tone {
        crate::Tone::Success => Some(std::time::Duration::from_secs(5)),
        crate::Tone::Info => Some(std::time::Duration::from_secs(7)),
        _ => None,
    }
}

/// Inline error / warning / info banner (docs/10 §D4.6): icon + title + detail; callers add an action.
pub fn banner(
    tone: crate::Tone,
    mode: Mode,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
) -> Div {
    let t = Tokens::for_mode(mode);
    let (fg, bg) = tone.colors(t);
    let glyph = match tone {
        crate::Tone::Danger => Icon::OctagonX,
        crate::Tone::Warning => Icon::TriangleAlert,
        crate::Tone::Success => Icon::CircleCheck,
        _ => Icon::Info,
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(6.))
        .min_w_0()
        .rounded(px(crate::themes::metrics().radius.control))
        .bg(color(bg))
        .text_color(color(t.text_primary))
        .child(icon(glyph, fg))
        .child(
            // Text column wraps; an action the caller appends keeps its size.
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                // Word-boundary wrapping (keep-all): `작업 폴더가 없습니다` never splits inside a word.
                .child(
                    keep_all_start(&Into::<SharedString>::into(title))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .children(detail.map(|d| keep_all_start(&d).text_color(color(t.text_secondary)))),
        )
}
