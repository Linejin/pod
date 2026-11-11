//! The page and its cards: the intro, the off state with its enable button,
//! the toolbar (다시 찾기, first prompt, hidden sessions, 끄기), the notes
//! and one card per session with its tool mark, state chip, facts, link and
//! buttons. The lines come from the [`model`](super::model); the buttons call
//! the [`controller`](super::controller).

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use podd::protocol::{DiscoveredSession, SessionState, SessionsSettingsParams};

use super::controller::{
    hide, open_resumed, open_terminal, register, resume, scan, settings, toggle_hidden,
};
use super::model::{
    actions, empty_line, facts_line, heading, link_line, no_list_line, notes_line, state_label,
};
use super::{SessionsPanel, copy, s, state};
use crate::qa_marks::QaId;
use crate::workspace::Workspace;

impl Render for SessionsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        // A resumed terminal: show it once the workspace knows it.
        open_resumed(&self.workspace, window, cx);
        div()
            .id("sessions-panel")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .bg(rgb(t.bg_surface))
            .text_color(rgb(t.text_primary))
            .child(page(&self.workspace, cx))
    }
}

fn page(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let st = state(cx);
    let (list, loading, scanning, failed, show_hidden) = (
        st.list.clone(),
        st.loading,
        st.scanning,
        st.failed,
        st.show_hidden,
    );
    let mut body = div()
        .id("sessions-section")
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
                    .child(no_list_line(failed, loading)),
            )
            .into_any_element();
    };
    if !list.enabled {
        let ws_on = ws.clone();
        return body
            .child(div().text_sm().child(s::OFF))
            .child(
                div().flex().child(
                    Button::new("sessions-enable")
                        .label(s::ENABLE)
                        .primary()
                        .loading(scanning)
                        .accessibility_label(s::ENABLE)
                        .qa_id(|| "sessions-enable".into())
                        .on_click(move |_, _, cx| {
                            settings(
                                &ws_on,
                                SessionsSettingsParams {
                                    enabled: Some(true),
                                    preview: None,
                                },
                                cx,
                            )
                        }),
                ),
            )
            .into_any_element();
    }
    let (ws_scan, ws_prev, ws_off, ws_hidden) = (ws.clone(), ws.clone(), ws.clone(), ws.clone());
    let preview = list.preview;
    let mut toolbar = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(
            Button::new("sessions-rescan")
                .label(s::RESCAN)
                .compact()
                .loading(scanning)
                .accessibility_label(copy::rescan_name())
                .qa_id(|| "sessions-rescan".into())
                .on_click(move |_, _, cx| scan(&ws_scan, false, cx)),
        )
        .child(
            Button::new("sessions-preview")
                .label(copy::preview_label(preview))
                .compact()
                .when(preview, |b| b.primary())
                .accessibility_label(copy::preview_name(preview))
                .qa_id(|| "sessions-preview".into())
                .on_click(move |_, _, cx| {
                    settings(
                        &ws_prev,
                        SessionsSettingsParams {
                            enabled: None,
                            preview: Some(!preview),
                        },
                        cx,
                    )
                }),
        );
    if list.hidden > 0 || show_hidden {
        toolbar = toolbar.child(
            Button::new("sessions-hidden")
                .label(copy::hidden_label(show_hidden, list.hidden))
                .compact()
                .accessibility_label(copy::hidden_name(show_hidden, list.hidden))
                .qa_id(|| "sessions-hidden".into())
                .on_click(move |_, _, cx| toggle_hidden(&ws_hidden, cx)),
        );
    }
    toolbar = toolbar.child(div().flex_1()).child(
        Button::new("sessions-disable")
            .label(s::DISABLE)
            .compact()
            .accessibility_label(s::DISABLE)
            .qa_id(|| "sessions-disable".into())
            .on_click(move |_, _, cx| {
                settings(
                    &ws_off,
                    SessionsSettingsParams {
                        enabled: Some(false),
                        preview: None,
                    },
                    cx,
                )
            }),
    );
    body = body.child(toolbar);
    let now = podd::now_ms();
    body = body.child(
        div()
            .text_xs()
            .text_color(rgb(t.text_muted))
            .child(notes_line(&list, now)),
    );
    if list.truncated {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(t.warning_fg))
                .child(s::TRUNCATED),
        );
    }
    if list.sessions.is_empty() {
        return body
            .child(div().text_sm().child(empty_line(&list)))
            .into_any_element();
    }
    let mut cards = div()
        .id("sessions-list")
        .role(Role::List)
        .aria_label(s::SECTION)
        .flex()
        .flex_col()
        .gap_2();
    for x in &list.sessions {
        cards = cards.child(card(ws, x, now));
    }
    body.child(cards).into_any_element()
}

fn card(ws: &WeakEntity<Workspace>, x: &DiscoveredSession, now: u64) -> AnyElement {
    let t = crate::theme::tokens();
    let title = heading(x);
    let state = state_label(x.state);
    let (chip_fg, chip_bg) = match x.state {
        SessionState::RunningPod => (t.success_fg, t.success_bg),
        SessionState::RunningOther => (t.warning_fg, t.warning_bg),
        SessionState::Resumable => (t.text_secondary, t.bg_hover),
        SessionState::Stale => (t.text_muted, t.bg_hover),
    };
    let id = |part: &str| SharedString::from(format!("session-{}-{part}", x.key));
    let kind = crate::agent_kind::AgentKind::of_discovered(x.agent);
    let header = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        // The session's tool (docs/10 §D5.11).
        .child(crate::agent_kind::mark_el(
            &format!("session-{}", x.key),
            &kind,
            pod_ui::tokens::ICON_SIZE,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.clone()),
        )
        .child(
            div()
                .px_2()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(chip_fg))
                .bg(rgb(chip_bg))
                .child(state),
        );
    let mut card = div()
        .id(id("card"))
        .role(Role::ListItem)
        .aria_label(copy::card_name(&title, &kind.accessible_name(), state))
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(t.border_subtle))
        .bg(rgb(t.bg_raised))
        .when(x.hidden, |d| d.opacity(0.6))
        .child(header);
    if x.title.is_some()
        && let Some(p) = &x.preview
    {
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .truncate()
                .child(p.clone()),
        );
    }
    card = card
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(facts_line(x, now)),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(link_line(x)),
        );
    if x.state == SessionState::RunningOther {
        card = card.child(
            div()
                .id(id("note"))
                .role(Role::Note)
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::OTHER_NOTE),
        );
    } else if !x.cwd_exists {
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::NO_FOLDER),
        );
    }
    let a = actions(x);
    let mut row = div().flex().flex_wrap().items_center().gap_2();
    if let Some(term) = a.open_terminal {
        let ws = ws.clone();
        row = row.child(
            Button::new(id("open"))
                .label(s::OPEN_TERMINAL)
                .compact()
                .primary()
                .accessibility_label(copy::action_name(&title, s::OPEN_TERMINAL))
                .qa_id(|| id("open").to_string())
                .on_click(move |_, window, cx| open_terminal(&ws, term.clone(), window, cx)),
        );
    }
    if a.resume {
        let (ws, key, t2) = (ws.clone(), x.key.clone(), title.clone());
        row = row.child(
            Button::new(id("resume"))
                .label(s::RESUME)
                .compact()
                .primary()
                .tooltip(x.resume_argv.join(" "))
                .accessibility_label(copy::action_name(&title, s::RESUME))
                .qa_id(|| id("resume").to_string())
                .on_click(move |_, _, cx| resume(&ws, key.clone(), t2.clone(), cx)),
        );
    }
    if let Some(add_repo) = a.register {
        let (ws, key) = (ws.clone(), x.key.clone());
        let label = if add_repo {
            s::REGISTER_REPO
        } else {
            s::REGISTER_TASK
        };
        row = row.child(
            Button::new(id("register"))
                .label(label)
                .compact()
                .tooltip(if add_repo {
                    copy::REGISTER_REPO_TIP
                } else {
                    copy::REGISTER_TASK_TIP
                })
                .accessibility_label(copy::action_name(&title, label))
                .qa_id(|| id("register").to_string())
                .on_click(move |_, _, cx| register(&ws, key.clone(), add_repo, cx)),
        );
    }
    let (ws_h, key, hidden) = (ws.clone(), x.key.clone(), x.hidden);
    let label = if hidden { s::UNHIDE } else { s::HIDE };
    row = row.child(
        Button::new(id("hide"))
            .label(label)
            .compact()
            .accessibility_label(copy::action_name(&title, label))
            .qa_id(|| id("hide").to_string())
            .on_click(move |_, _, cx| hide(&ws_h, key.clone(), !hidden, cx)),
    );
    card.child(row).into_any_element()
}
