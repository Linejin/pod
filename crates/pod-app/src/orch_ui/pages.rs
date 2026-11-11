//! Center-dock orchestration pages: the Run board (runs → items by column
//! with dependency indent → worker cards), the cleanup screen for finished
//! workers, and the orchestration settings. One panel type, one page each;
//! this is the panel and the bits every page shares, the pages themselves
//! are [`super::board`], [`super::cleanup`] and [`super::settings`].
//!
//! Every control is a toolkit button (Tab / Shift+Tab, Enter / Space) with a
//! Korean accessible name that says which worker or task it acts on.

use gpui_kit::base::dock::{PanelEvent, PanelState};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::panel_handle;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::model::{self, Tone};
use super::{board, cleanup, read, s, settings, state};
use crate::orch::mode;
use crate::qa_marks::QaId as _;
use crate::workspace::Workspace;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Page {
    Board,
    Cleanup,
    Settings,
}

impl Page {
    pub fn panel_name(self) -> &'static str {
        match self {
            Page::Board => "PodOrchBoard",
            Page::Cleanup => "PodOrchCleanup",
            Page::Settings => "PodOrchSettings",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Page::Board => s::BOARD,
            Page::Cleanup => s::CLEANUP,
            Page::Settings => s::SETTINGS,
        }
    }
}

pub(crate) struct OrchPage {
    pub page: Page,
    workspace: WeakEntity<Workspace>,
    pub focus: FocusHandle,
    _sub: Option<Subscription>,
}

impl EventEmitter<PanelEvent> for OrchPage {}
impl Focusable for OrchPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_kit::base::dock::Panel for OrchPage {
    fn panel_name(&self) -> &'static str {
        self.page.panel_name()
    }
    fn dump(&self, _: &App) -> PanelState {
        PanelState::new(self.page.panel_name())
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let me = cx.entity().downgrade();
        cx.defer(move |cx| {
            state(cx)
                .pages
                .retain(|(_, p)| p != &me && p.upgrade().is_some())
        });
    }
}
impl gpui_kit::component::dock::Panel for OrchPage {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(self.page.title().into())
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(self.page.title())
    }
}

impl Render for OrchPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        let body = match self.page {
            Page::Board => board::board(&self.workspace, cx),
            Page::Cleanup => cleanup::cleanup(&self.workspace, cx),
            Page::Settings => settings::settings(&self.workspace, cx),
        };
        div()
            .id(SharedString::from(format!(
                "orch-page-{}",
                self.page.panel_name()
            )))
            .track_focus(&self.focus)
            .when(self.page == Page::Board, |d| {
                d.on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                    let m = &e.keystroke.modifiers;
                    if m.control || m.alt || m.platform || !this.focus.is_focused(window) {
                        return;
                    }
                    if board::board_key(&this.workspace, e.keystroke.key.as_str(), window, cx) {
                        cx.stop_propagation();
                    }
                }))
            })
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .bg(rgb(t.bg_surface))
            .text_color(rgb(t.text_primary))
            .child(body)
    }
}

/// A page panel, registered with the module state (also used by the layout restore).
pub(crate) fn new_panel(
    page: Page,
    workspace: WeakEntity<Workspace>,
    cx: &mut App,
) -> Entity<OrchPage> {
    let panel = cx.new(|cx| {
        let sub = workspace
            .upgrade()
            .map(|w| cx.observe(&w, |_, _, cx| cx.notify()));
        OrchPage {
            page,
            workspace,
            focus: cx.focus_handle(),
            _sub: sub,
        }
    });
    state(cx).pages.push((page, panel.downgrade()));
    panel
}

/// Open (or focus) `page` in the center dock.
pub(crate) fn open(
    ws: &mut Workspace,
    page: Page,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    use gpui_kit::component::dock::{DockPlacement, PanelId};
    let existing = state(cx)
        .pages
        .iter()
        .find(|(p, w)| *p == page && w.upgrade().is_some())
        .and_then(|(_, w)| w.upgrade());
    if let Some(p) = existing {
        ws.dock.update(cx, |d, cx| {
            d.select_panel(PanelId::from(p.entity_id()), window, cx)
        });
        let focus = p.read(cx).focus.clone();
        focus.focus(window, cx);
        return;
    }
    let panel = new_panel(page, cx.entity().downgrade(), cx);
    let focus = panel.read(cx).focus.clone();
    ws.dock.update(cx, |d, cx| {
        d.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx)
    });
    focus.focus(window, cx);
    ws.touch(cx);
}

// ───────────────────────────── shared bits ─────────────────────────────

fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_lg()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

pub(super) fn subheading(text: impl Into<SharedString>) -> Div {
    div()
        .pt_2()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

pub(super) fn chip(text: impl Into<SharedString>, tone: Tone) -> Div {
    let t = crate::theme::tokens();
    let (fg, bg) = match tone {
        Tone::Accent => (t.info_fg, t.info_bg),
        Tone::Ok => (t.success_fg, t.success_bg),
        Tone::Warn => (t.warning_fg, t.warning_bg),
        Tone::Bad => (t.danger_fg, t.danger_bg),
        Tone::Muted => (t.text_secondary, t.bg_hover),
    };
    div()
        .flex_shrink_0()
        .px_2()
        .rounded_sm()
        .text_xs()
        .text_color(rgb(fg))
        .bg(rgb(bg))
        .child(text.into())
}

/// `2분 전` (relative; re-rendered with every data change, no timer).
pub(crate) fn ago(ms: u64, now: u64) -> String {
    if ms == 0 {
        return s::NO_ACTIVITY.into();
    }
    format!("{} 전", crate::gates::duration_text(now.saturating_sub(ms)))
}

/// Page header with the cross-links every page has (keyboard: Tab to them).
pub(super) fn nav(here: Page, ws: &WeakEntity<Workspace>, cx: &App) -> Div {
    let q = read(cx).map(|s| s.open_questions()).unwrap_or(0);
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(div().flex_1().min_w_0().child(heading(here.title())));
    let inbox_label = if q > 0 {
        format!("{} · {}: {q}", s::INBOX, s::OPEN_QUESTIONS)
    } else {
        s::INBOX.to_string()
    };
    let w = ws.clone();
    row = row.child(
        Button::new("orch-nav-inbox")
            .label(inbox_label.clone())
            .compact()
            .icon(pod_ui::kit_icon(pod_ui::Icon::MessageSquare))
            .when(q > 0, |b| b.primary())
            .accessibility_label(format!("{inbox_label} (Ctrl+Shift+M)"))
            .on_click(move |_, window, cx| {
                let _ = w.update(cx, |w, cx| super::open_inbox(w, window, cx));
            }),
    );
    for p in [Page::Board, Page::Cleanup, Page::Settings] {
        if p == here {
            continue;
        }
        let w = ws.clone();
        row = row.child(
            Button::new(SharedString::from(format!("orch-nav-{}", p.panel_name())))
                .label(p.title())
                .compact()
                .accessibility_label(p.title())
                .on_click(move |_, window, cx| {
                    let _ = w.update(cx, |w, cx| super::open_page(w, p, window, cx));
                }),
        );
    }
    let w = ws.clone();
    row.child(
        Button::new(SharedString::from(format!(
            "orch-reload-{}",
            here.panel_name()
        )))
        .icon(pod_ui::kit_icon(pod_ui::Icon::RefreshCw))
        .ghost()
        .compact()
        .tooltip(s::RELOAD)
        .accessibility_label(format!("{} {}", here.title(), s::RELOAD))
        .on_click(move |_, window, cx| {
            let _ = w.update(cx, |w, cx| {
                super::reload_all(w, window, cx);
                if here == Page::Cleanup {
                    super::refresh_candidates(w, cx);
                }
                if here == Page::Settings {
                    super::refresh_settings(w, cx);
                }
            });
        }),
    )
}

/// Loading / unsupported / error banner, or `None` when there is data.
pub(super) fn status_line(cx: &App) -> Option<AnyElement> {
    let t = crate::theme::tokens();
    let st = read(cx)?;
    if st.unsupported {
        return Some(
            pod_ui::banner(pod_ui::Tone::Warning, mode(), s::UNSUPPORTED, None).into_any_element(),
        );
    }
    if let Some(e) = &st.error {
        return Some(
            pod_ui::banner(pod_ui::Tone::Danger, mode(), e.clone(), None).into_any_element(),
        );
    }
    if !st.loaded {
        return Some(
            div()
                .text_sm()
                .text_color(rgb(t.text_muted))
                .child(s::LOADING)
                .into_any_element(),
        );
    }
    None
}

/// One radio option (ui-10): `pod_ui::choice` (RadioButton, checked =
/// selected), marked `radio:<name>:<checked>` for the native-QA tests.
pub(super) fn radio(
    id: String,
    g: &model::ChoiceGroup,
    o: &model::ChoiceOpt,
    text: String,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let name = g.option_name(o);
    let mark = format!("radio:{name}:{}", o.selected);
    pod_ui::choice(
        SharedString::from(id),
        text,
        name,
        None,
        o.selected,
        !g.locked,
        mode(),
        activate,
    )
    .qa_id(move || mark)
}
