//! The extensions UI's render: the Settings row that leads to the "확장"
//! page, the page (intro, folder and 다시 읽기, then one card per mod or the
//! loading / unavailable / off / empty line) and a mod's card (header and
//! toggle, description, error, permission toggles, facts, checks, log).
//! What to show comes from [`model`]; presses call [`controller`].

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use podd::protocol::{ModInfo, ModState};

use super::controller::{self, grant, reload, set_enabled, toggle_log};
use super::copy::{self, s};
use super::model::{self, display_name, state_label};
use super::{ExtensionsPanel, ModsUi, error_summary, open_page, state};
use crate::workspace::Workspace;

/// A row in Settings that leads to the "확장" page.
pub(crate) fn settings_link(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let summary = model::installed_summary(cx.try_global::<ModsUi>().and_then(|s| s.list.as_ref()));
    let ws = ws.clone();
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .pt_4()
        .mt_2()
        .border_t_1()
        .border_color(rgb(t.border_subtle))
        .child(div().w(px(110.)).flex_shrink_0().child(s::SECTION))
        .child(
            Button::new("open-extensions")
                .label(s::OPEN_PAGE)
                .compact()
                .accessibility_label(format!("{} — {}", s::OPEN_PAGE, s::OPEN_PAGE_DETAIL))
                .on_click(move |_, window, cx| {
                    let _ = ws.update(cx, |w, cx| open_page(w, window, cx));
                }),
        )
        .child(div().text_sm().text_color(rgb(t.text_muted)).child(summary))
        .into_any_element()
}

impl Render for ExtensionsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        let ws = self.workspace.clone();
        div()
            .id("extensions-panel")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .bg(rgb(t.bg_surface))
            .text_color(rgb(t.text_primary))
            .child(page(&ws, cx))
    }
}

/// The content of the "확장" page.
fn page(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    if cx
        .try_global::<ModsUi>()
        .is_none_or(|s| s.list.is_none() && !s.loading && !s.failed)
    {
        let ws2 = ws.clone();
        cx.defer(move |cx| {
            let _ = ws2.update(cx, controller::refresh);
        });
    }
    let st = state(cx);
    let (list, loading, failed, log_open) =
        (st.list.clone(), st.loading, st.failed, st.log_open.clone());
    let logs = st.logs.clone();
    let mut body = div()
        .id("mods-section")
        .role(Role::Group)
        .aria_label(s::SECTION)
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(s::SECTION),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::INTRO),
        );
    let Some(list) = list else {
        return body
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(t.text_muted))
                    .child(if failed { s::UNAVAILABLE } else { s::LOADING }),
            )
            .into_any_element();
    };
    if !list.enabled {
        return body.child(div().text_sm().child(s::OFF)).into_any_element();
    }
    let ws_reload = ws.clone();
    body = body.child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().text_sm().text_color(rgb(t.text_muted)).child(format!(
                "{}: {}",
                s::FOLDER,
                list.list.root
            )))
            .child(
                Button::new("mods-reload")
                    .label(s::RELOAD)
                    .compact()
                    .loading(loading)
                    .accessibility_label(format!("{} — {}", s::SECTION, s::RELOAD))
                    .on_click(move |_, _, cx| reload(&ws_reload, cx)),
            ),
    );
    if list.list.mods.is_empty() {
        return body
            .child(div().text_sm().child(s::EMPTY))
            .into_any_element();
    }
    for m in &list.list.mods {
        body = body.child(mod_card(
            ws,
            m,
            log_open.as_deref() == Some(m.name.as_str()),
            logs.get(&m.name),
        ));
    }
    body.into_any_element()
}

fn mod_card(
    ws: &WeakEntity<Workspace>,
    m: &ModInfo,
    log_open: bool,
    log: Option<&Vec<String>>,
) -> AnyElement {
    let t = crate::theme::tokens();
    let name = display_name(m);
    let state = state_label(m);
    let (chip_fg, chip_bg) = match m.state {
        ModState::Running => (t.success_fg, t.success_bg),
        ModState::Disabled => (t.text_secondary, t.bg_hover),
        ModState::Stopped => (t.warning_fg, t.warning_bg),
        ModState::Invalid => (t.danger_fg, t.danger_bg),
    };
    let id = |part: &str| SharedString::from(format!("mod-{}-{part}", m.name));
    let ws_toggle = ws.clone();
    let (mod_name, enabled) = (m.name.clone(), m.enabled);
    let header = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(format!("{} · v{}", m.name, m.version)),
        )
        .child(
            div()
                .px_2()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(chip_fg))
                .bg(rgb(chip_bg))
                .child(state.clone()),
        )
        .child(div().flex_1())
        .when(m.state != ModState::Invalid, |d| {
            d.child(
                Button::new(id("toggle"))
                    .label(if enabled { s::DISABLE } else { s::ENABLE })
                    .compact()
                    .when(!enabled, |b| b.primary())
                    .accessibility_label(format!(
                        "{name} {} ({state})",
                        if enabled { s::DISABLE } else { s::ENABLE }
                    ))
                    .on_click(move |_, _, cx| {
                        set_enabled(&ws_toggle, mod_name.clone(), !enabled, cx)
                    }),
            )
        });
    let mut card = div()
        .id(id("card"))
        .role(Role::Group)
        .aria_label(format!("{name}, {state}"))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(t.border_subtle))
        .bg(rgb(t.bg_raised))
        .child(header);
    if let Some(d) = &m.description {
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(d.clone()),
        );
    }
    if let Some(e) = error_summary(m) {
        card = card.child(
            div()
                .id(id("error"))
                .role(Role::Alert)
                .text_sm()
                .text_color(rgb(t.danger_fg))
                .child(e),
        );
    }
    // Permissions.
    let toggles = model::permission_toggles(m);
    let mut perms = div().flex().flex_wrap().items_center().gap_2().child(
        div()
            .w(px(56.))
            .flex_shrink_0()
            .text_sm()
            .child(s::PERMISSIONS),
    );
    if toggles.is_empty() {
        perms = perms.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_muted))
                .child(s::NO_PERMISSIONS),
        );
    }
    for p in toggles {
        let ws_grant = ws.clone();
        let mod_name = m.name.clone();
        perms = perms.child(
            Button::new(id(&format!("perm-{}", p.permission.name())))
                .label(format!("{}: {}", p.label, p.verdict))
                .compact()
                .when(p.granted, |b| b.primary())
                .tooltip(format!("{} ({})", p.label, p.permission.name()))
                .accessibility_label(copy::permission_toggle(&name, p.label, p.verdict, p.next))
                .on_click(move |_, _, cx| {
                    grant(&ws_grant, mod_name.clone(), p.permission, !p.granted, cx)
                }),
        );
    }
    card = card.child(perms);
    // What it provides.
    if let Some(facts) = model::facts_line(m) {
        card = card.child(div().text_xs().text_color(rgb(t.text_muted)).child(facts));
    }
    if let Some(checks) = model::checks_line(m) {
        card = card.child(div().text_xs().text_color(rgb(t.text_muted)).child(checks));
    }
    let ws_log = ws.clone();
    let mod_name = m.name.clone();
    card = card.child(
        div().flex().child(
            Button::new(id("log"))
                .label(if log_open { s::HIDE_LOG } else { s::SHOW_LOG })
                .compact()
                .accessibility_label(format!(
                    "{name} {}",
                    if log_open { s::HIDE_LOG } else { s::SHOW_LOG }
                ))
                .on_click(move |_, _, cx| toggle_log(&ws_log, mod_name.clone(), cx)),
        ),
    );
    if log_open {
        let lines = log.cloned().unwrap_or_default();
        card = card.child(
            div()
                .id(id("log-lines"))
                .role(Role::Document)
                .aria_label(format!("{name} {}", s::SHOW_LOG))
                .max_h(px(220.))
                .overflow_y_scroll()
                .p_2()
                .rounded_sm()
                .bg(rgb(t.bg_canvas))
                .font_family("Consolas")
                .text_xs()
                .flex()
                .flex_col()
                .when(lines.is_empty(), |d| d.child(s::LOG_EMPTY))
                .children(lines.into_iter().map(|l| div().child(l))),
        );
    }
    card.into_any_element()
}
