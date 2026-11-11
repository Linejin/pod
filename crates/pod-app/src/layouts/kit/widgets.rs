//! Presentation pieces several modes draw the same way. Each one reproduces
//! the current values of the code it replaces; whatever differs between modes
//! (sizes, anchors, backgrounds, ids) is an explicit argument.
//!
//! Rows and cards are not here: the review rows and kanban cards share no
//! frame values today, so each mode keeps its own.
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::themes::{Metrics, Shadow, Surface};

use super::metrics::color;
use crate::qa_marks::QaId as _;

/// The connection warning above a body: a warning `pod_ui::banner` at the
/// small text size, inset by `px_2`/`pt_2`. Same in the workbench and all six
/// layout files.
pub(crate) fn warning_banner(text: impl Into<SharedString>) -> Div {
    div()
        .px_2()
        .pt_2()
        .child(pod_ui::banner(pod_ui::Tone::Warning, crate::theme::mode(), text, None).text_xs())
}

/// Rounded "island" panels of the Layered and Islands styles and the gap
/// between them; plain in every other style. The workbench, queue and dense
/// bodies use these rules.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Island {
    metrics: Metrics,
    surface: u32,
    border: u32,
}

impl Island {
    /// `surface`/`border`: the `bg_surface` and `border_subtle` tokens.
    pub(crate) fn new(metrics: Metrics, surface: u32, border: u32) -> Self {
        Self {
            metrics,
            surface,
            border,
        }
    }

    /// From the active style and tokens.
    pub(crate) fn current() -> Self {
        let tk = crate::theme::tokens();
        Self::new(pod_ui::themes::metrics(), tk.bg_surface, tk.border_subtle)
    }

    pub(crate) fn rounded(&self) -> bool {
        matches!(self.metrics.surface, Surface::Layered | Surface::Islands)
    }

    /// Space between and around islands; 0 when not rounded.
    pub(crate) fn gap(&self) -> f32 {
        if self.rounded() { self.metrics.gap } else { 0. }
    }

    /// `d` as an island: card radius, clipped, surface colour, and the
    /// style's elevation (with a hairline for Islands and Raised).
    pub(crate) fn wrap(&self, d: Div) -> Div {
        if !self.rounded() {
            return d;
        }
        let m = self.metrics;
        let d = d
            .rounded(px(m.radius.card))
            .overflow_hidden()
            .bg(rgb(self.surface));
        match (m.surface, m.shadow) {
            (Surface::Islands, _) | (_, Shadow::Raised) => d
                .shadow(pod_ui::tokens::elevation(1, crate::theme::mode()))
                .border_1()
                .border_color(rgb(self.border)),
            (_, Shadow::Soft) => d.shadow(pod_ui::tokens::elevation(1, crate::theme::mode())),
            _ => d,
        }
    }
}

/// A panel's header row: title on the left, buttons on the right, a bottom
/// hairline except in the Mono style. `min_height` is the caller's formula
/// (dense: `density.panel_header.max(row_height)`).
pub(crate) fn panel_header(
    qa_id: &'static str,
    title: impl IntoElement,
    buttons: Vec<AnyElement>,
    min_height: f32,
) -> Div {
    let tk = crate::theme::tokens();
    header_with(
        pod_ui::themes::metrics(),
        tk.border_subtle,
        qa_id,
        title,
        buttons,
        min_height,
    )
}

/// [`panel_header`] for given style metrics and `border_subtle` token.
pub(super) fn header_with(
    m: Metrics,
    border: u32,
    qa_id: &'static str,
    title: impl IntoElement,
    buttons: Vec<AnyElement>,
    min_height: f32,
) -> Div {
    div()
        .qa_id(move || qa_id.to_string())
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .pl_3()
        .pr_1()
        .min_h(px(min_height))
        .when(m.surface != Surface::Mono, |d| {
            d.border_b(px(m.border_width)).border_color(color(border))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(title),
        )
        .children(buttons)
}

/// Where the toast stack floats over a body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ToastAnchor {
    TopRight { top: f32, right: f32, width: f32 },
    BottomLeft { left: f32, bottom: f32, width: f32 },
}

impl ToastAnchor {
    /// Kanban, block terminal and review layouts.
    pub(crate) const TOP_RIGHT: Self = Self::TopRight {
        top: 52.,
        right: 16.,
        width: 340.,
    };
}

/// Direction of the body's children inside the shell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BodyFlow {
    /// Kanban, block terminal and review layouts.
    Column,
    /// Focus rail (rail beside the dock).
    Row,
}

/// A body under the connection warning and the hook prompt, with toasts
/// floating over it: the shell of the kanban, block terminal, review and
/// focus-rail layouts.
pub(crate) struct ConnectionShell {
    /// Connection warning text (`LayoutFrame::warning`).
    pub(crate) warning: Option<String>,
    /// The host's hook prompt, while one is waiting.
    pub(crate) hook_banner: Option<AnyElement>,
    /// Background token of the whole shell; `None` = none (focus rail).
    pub(crate) canvas: Option<u32>,
    pub(crate) flow: BodyFlow,
    pub(crate) body: AnyElement,
    pub(crate) toasts: AnyElement,
    pub(crate) anchor: ToastAnchor,
}

impl ConnectionShell {
    pub(crate) fn render(self) -> Div {
        let body = match self.flow {
            BodyFlow::Column => div().flex().flex_col().flex_1().min_h_0(),
            BodyFlow::Row => div().flex().flex_1().min_h_0(),
        };
        let toasts = match self.anchor {
            ToastAnchor::TopRight { top, right, width } => {
                div().absolute().top(px(top)).right(px(right)).w(px(width))
            }
            ToastAnchor::BottomLeft {
                left,
                bottom,
                width,
            } => div()
                .absolute()
                .left(px(left))
                .bottom(px(bottom))
                .w(px(width)),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .relative()
            .when_some(self.canvas, |d, bg| d.bg(rgb(bg)))
            .when_some(self.warning, |d, text| d.child(warning_banner(text)))
            .when_some(self.hook_banner, |d, b| d.child(b))
            .child(body.child(self.body))
            .child(toasts.child(self.toasts))
    }
}
