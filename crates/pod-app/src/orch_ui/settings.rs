//! The orchestration settings page: the four settings and the resource
//! limits as radio groups, and the Codex worker allowance per repository.

use gpui_kit::*;
use podd::protocol::OrchSettings;

use super::model::{self, Tone};
use super::pages::{Page, chip, nav, radio, subheading};
use super::{s, state};
use crate::orch::mode;
use crate::workspace::Workspace;

/// A settings row: label column, the radio group, then chips for a value
/// set elsewhere (`현재 …`) and a lock.
fn choice_row(g: &model::ChoiceGroup, group: Stateful<Div>, env: Option<&str>) -> Div {
    let mut row = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(div().w(px(150.)).flex_shrink_0().child(g.label.clone()))
        .child(group.flex_initial());
    if let Some(cur) = &g.off_list {
        row = row.child(chip(format!("현재 {cur}"), Tone::Accent));
    }
    if g.locked {
        row = row.child(chip(
            env.map_or(s::LOCKED.to_string(), |e| format!("{} ({e})", s::LOCKED)),
            Tone::Muted,
        ));
    }
    row
}

pub(super) fn settings(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let mut body = div()
        .id("orch-settings")
        .role(Role::Group)
        .aria_label(s::SETTINGS)
        .flex()
        .flex_col()
        .gap_3()
        .child(nav(Page::Settings, ws, cx))
        .child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::SETTINGS_INTRO),
        );
    let st = state(cx);
    let Some(cur) = st.settings.clone() else {
        let text = if st.settings_failed {
            s::SETTINGS_UNAVAILABLE
        } else {
            s::LOADING
        };
        return body
            .child(div().text_sm().text_color(rgb(t.text_muted)).child(text))
            .into_any_element();
    };
    for g in model::settings_groups(&cur) {
        let mut group = pod_ui::radio_group(
            SharedString::from(format!("orch-set-{}", g.field)),
            g.label.clone(),
            mode(),
        );
        for o in &g.options {
            let next: OrchSettings = model::with_change(&cur, &g.field, &o.value);
            let w = ws.clone();
            group = group.child(radio(
                format!("orch-set-{}-{}", g.field, o.value),
                &g,
                o,
                o.text.clone(),
                move |_, cx| super::set_settings(&w, next.clone(), cx),
            ));
        }
        body = body.child(choice_row(&g, group, None));
    }
    body.child(limits_section(ws, cx))
        .child(allowance_section(cx))
        .into_any_element()
}

/// The narrow Codex worker allowance (docs/19 §18.1): per repository, read
/// only here (it lives in the repository's `.pod/config.toml`).
fn allowance_section(cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let st = state(cx);
    let mut sec = div()
        .id("orch-allow")
        .role(Role::Group)
        .aria_label(s::ALLOW)
        .flex()
        .flex_col()
        .gap_3()
        .pt_4()
        .mt_2()
        .border_t_1()
        .border_color(rgb(t.border_subtle))
        .child(subheading(s::ALLOW))
        .child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::ALLOW_INTRO),
        );
    if st.allowances.is_empty() {
        return sec
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(t.text_muted))
                    .child(s::ALLOW_NO_REPOS),
            )
            .into_any_element();
    }
    for (repo, on) in &st.allowances {
        let (text, tone) = model::allowance_label(*on);
        sec = sec.child(
            div()
                .id(SharedString::from(format!("orch-allow-{repo}")))
                .role(Role::Group)
                .aria_label(format!("{repo}: {} {text}", s::ALLOW))
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(150.)).flex_shrink_0().child(repo.clone()))
                .child(chip(text, tone)),
        );
    }
    sec.into_any_element()
}

/// The resource gate's limits (`orch.limits.*`, v18).
fn limits_section(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let st = state(cx);
    let mut sec = div()
        .id("orch-limits")
        .role(Role::Group)
        .aria_label(s::LIMITS)
        .flex()
        .flex_col()
        .gap_3()
        .pt_4()
        .mt_2()
        .border_t_1()
        .border_color(rgb(t.border_subtle))
        .child(subheading(s::LIMITS))
        .child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::LIMITS_INTRO),
        );
    let Some(lim) = st.limits.clone() else {
        let text = if st.limits_failed {
            s::LIMITS_UNAVAILABLE
        } else {
            s::LOADING
        };
        return sec
            .child(div().text_sm().text_color(rgb(t.text_muted)).child(text))
            .into_any_element();
    };
    for (key, choices) in model::LIMIT_CHOICES {
        let Some(e) = lim.entries.iter().find(|e| e.key == key) else {
            continue;
        };
        let g = model::limit_group(key, choices, &e.value, e.source == "env");
        let mut group = pod_ui::radio_group(
            SharedString::from(format!("orch-lim-{key}")),
            format!("{}: {}", g.label, e.help),
            mode(),
        );
        for o in &g.options {
            let (w, k, v) = (ws.clone(), key.to_string(), o.value.clone());
            group = group.child(radio(
                format!("orch-lim-{key}-{}", o.value),
                &g,
                o,
                o.text.clone(),
                move |_, cx| super::set_limit(&w, k.clone(), v.clone(), cx),
            ));
        }
        let row = choice_row(&g, group, Some(&e.env));
        sec = sec.child(row).child(
            div()
                .pl(px(158.))
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(e.help.clone()),
        );
    }
    sec.into_any_element()
}
