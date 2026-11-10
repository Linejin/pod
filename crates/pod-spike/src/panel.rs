//! A dockable terminal panel: a `TerminalView` fed by a local ConPTY.

use std::sync::Arc;

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::base::dock::{Panel, PanelEvent};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use pod_terminal_view::element::hsla_of;
use pod_terminal_view::palette::DEFAULT_BG;
use pod_terminal_view::{TerminalConfig, TerminalEvent, TerminalView};

use crate::pty::{LocalPty, PtyEvent, PtySink};

pub struct TerminalPanel {
    pub view: Entity<TerminalView>,
    _pty: Arc<LocalPty>,
    title: SharedString,
    exited: bool,
    /// Embed the view with `Entity::cached` (only repaint when it changed).
    cache: bool,
    _pump: Task<()>,
    _sub: Subscription,
}

impl TerminalPanel {
    pub fn new(
        index: usize,
        program: Option<(String, Vec<String>)>,
        cache: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<Self> {
        let (tx, mut rx) = mpsc::unbounded();
        let pty = LocalPty::spawn(program, tx)?;
        let sink = PtySink(pty.clone());
        let view =
            cx.new(|cx| TerminalView::new(Box::new(sink), TerminalConfig::default(), window, cx));

        let sub = cx.subscribe(&view, |panel: &mut Self, _, ev: &TerminalEvent, cx| {
            // Shells often reset the title to "", keep ours then.
            if let TerminalEvent::TitleChanged(Some(t)) = ev
                && !t.trim().is_empty()
            {
                panel.title = t.clone().into();
                cx.notify();
            }
        });

        let (pump_pty, pump_view) = (pty.clone(), view.downgrade());
        // Baseline for measurements: without view caching every change repaints
        // every terminal. `refresh` ignores all view caches for the next frame.
        let force_full_repaint = !cache;
        let pump = cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                match ev {
                    PtyEvent::Output => {
                        let bytes = pump_pty.take_output();
                        if !bytes.is_empty()
                            && pump_view.update(cx, |v, cx| v.feed(&bytes, cx)).is_err()
                        {
                            break;
                        }
                        if force_full_repaint {
                            cx.refresh();
                        }
                    }
                    PtyEvent::Exit => {
                        let bytes = pump_pty.take_output();
                        let _ = pump_view.update(cx, |v, cx| v.feed(&bytes, cx));
                        let _ = this.update(cx, |p, cx| {
                            p.exited = true;
                            cx.notify();
                        });
                    }
                }
            }
        });

        Ok(Self {
            view,
            _pty: pty,
            title: format!("term {}", index + 1).into(),
            exited: false,
            cache,
            _pump: pump,
            _sub: sub,
        })
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).focus_handle(cx)
    }
}

impl Panel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        "TerminalPanel"
    }
}

impl gpui_kit::component::dock::Panel for TerminalPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.title.clone()
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let body: AnyElement = if self.cache {
            let mut style = StyleRefinement::default();
            style.size.width = Some(relative(1.).into());
            style.size.height = Some(relative(1.).into());
            self.view.clone().cached(style).into_any_element()
        } else {
            self.view.clone().into_any_element()
        };
        div()
            .size_full()
            .p_1()
            .bg(hsla_of(DEFAULT_BG))
            .child(body)
            .when(self.exited, |d| {
                d.child(
                    div()
                        .absolute()
                        .bottom_1()
                        .right_2()
                        .text_xs()
                        .text_color(rgb(0xe06c75))
                        .child("[exited]"),
                )
            })
    }
}
