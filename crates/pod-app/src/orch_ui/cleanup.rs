//! The cleanup page: finished workers by cleanup tier, with a bulk
//! selection (default: the ready tier) and per-row actions.

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use super::model::{self, CleanupRow, Tone};
use super::pages::{Page, chip, nav, status_line};
use super::{WorkerVerb, s, state, with_ws};
use crate::orch::mode;
use crate::workspace::Workspace;

pub(super) fn cleanup(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let mut body = div()
        .id("orch-cleanup")
        .role(Role::Group)
        .aria_label(s::CLEANUP)
        .flex()
        .flex_col()
        .gap_3()
        .child(nav(Page::Cleanup, ws, cx))
        .child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::CLEANUP_INTRO),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(s::TIER_HELP),
        );
    if let Some(line) = status_line(cx) {
        return body.child(line).into_any_element();
    }
    let st = state(cx);
    let rows = model::cleanup_rows(st.runs.values(), &st.candidates);
    if rows.is_empty() {
        return body
            .child(
                pod_ui::Frame::new(
                    "orch-cleanup-empty",
                    pod_ui::FrameKind::Empty,
                    s::NO_FINISHED,
                    s::CLEANUP_INTRO,
                )
                .render(mode(), []),
            )
            .into_any_element();
    }
    let selection = st
        .selection
        .clone()
        .unwrap_or_else(|| model::default_selection(&rows));
    let busy = st.busy.clone();
    let chosen: Vec<String> = rows
        .iter()
        .filter(|r| selection.contains(&r.attempt) && r.actions.archive)
        .filter_map(|r| r.task.clone())
        .collect();
    let w = ws.clone();
    let bulk_label = format!("{} ({})", s::ARCHIVE_SELECTED, chosen.len());
    body = body.child(
        div().flex().child(
            Button::new("orch-cleanup-bulk")
                .label(bulk_label.clone())
                .compact()
                .primary()
                .disabled(chosen.is_empty())
                .accessibility_label(bulk_label)
                .on_click(move |_, _, cx| {
                    for task in &chosen {
                        super::task_verb(&w, task.clone(), true, cx);
                    }
                }),
        ),
    );
    let mut list = div()
        .id("orch-cleanup-list")
        .role(Role::List)
        .aria_label(s::CLEANUP)
        .flex()
        .flex_col()
        .gap_2();
    for r in &rows {
        list = list.child(cleanup_row(ws, r, selection.contains(&r.attempt), &busy));
    }
    body.child(list).into_any_element()
}

fn cleanup_row(
    ws: &WeakEntity<Workspace>,
    r: &CleanupRow,
    selected: bool,
    busy: &std::collections::HashSet<String>,
) -> AnyElement {
    let t = crate::theme::tokens();
    let id = |part: &str| SharedString::from(format!("orch-clean-{}-{part}", r.attempt));
    let title = format!("{} · {}", r.attempt, r.item_title);
    let tier = model::tier_label(r.tier);
    let tone = match r.tier {
        Some(podd::protocol::CleanupTier::Ready) => Tone::Ok,
        Some(podd::protocol::CleanupTier::Review) => Tone::Warn,
        Some(podd::protocol::CleanupTier::Protected) => Tone::Bad,
        None => Tone::Muted,
    };
    let mut facts = vec![
        r.run.clone(),
        model::attempt_state_label(r.state).to_string(),
        model::worker_state_label(r.worker).to_string(),
    ];
    if let Some(task) = &r.task {
        facts.push(task.clone());
    }
    if r.archived {
        facts.push(s::ARCHIVED.into());
    }
    let mut row = div().flex().flex_wrap().items_center().gap_1().pt_1();
    if r.actions.archive {
        let attempt = r.attempt.clone();
        let w = ws.clone();
        // A checkbox (ui-10): the selection is its checked state, not a fill.
        row = row.child(
            Checkbox::new(id("select"))
                .label(s::SELECT)
                .checked(selected)
                .accessibility_label(format!("{title} {}", s::SELECT))
                .on_click(move |_, _, cx| {
                    let rows = {
                        let st = state(cx);
                        model::cleanup_rows(st.runs.values(), &st.candidates)
                    };
                    let st = state(cx);
                    let sel = st
                        .selection
                        .get_or_insert_with(|| model::default_selection(&rows));
                    if !sel.remove(&attempt) {
                        sel.insert(attempt.clone());
                    }
                    with_ws(&w, cx, |_, cx| cx.notify());
                }),
        );
    }
    if r.actions.release {
        let (w, attempt) = (ws.clone(), r.attempt.clone());
        row = row.child(
            Button::new(id("release"))
                .label(s::RELEASE)
                .compact()
                .loading(busy.contains(&super::busy_key(&r.attempt, WorkerVerb::Release)))
                .accessibility_label(format!("{title} {}", s::RELEASE))
                .on_click(move |_, window, cx| {
                    super::worker_verb(&w, attempt.clone(), WorkerVerb::Release, window, cx)
                }),
        );
    }
    if r.actions.keep {
        let (w, attempt) = (ws.clone(), r.attempt.clone());
        row = row.child(
            Button::new(id("keep"))
                .label(s::KEEP)
                .compact()
                .accessibility_label(format!("{title} {}", s::KEEP))
                .on_click(move |_, window, cx| {
                    super::worker_verb(&w, attempt.clone(), WorkerVerb::Keep, window, cx)
                }),
        );
    }
    if let Some(task) = r.task.clone() {
        let loading = busy.contains(&format!("task:{task}"));
        if r.actions.archive {
            let (w, task) = (ws.clone(), task.clone());
            row = row.child(
                Button::new(id("archive"))
                    .label(s::ARCHIVE)
                    .compact()
                    .loading(loading)
                    .accessibility_label(format!("{title} {}", s::ARCHIVE))
                    .on_click(move |_, _, cx| super::task_verb(&w, task.clone(), true, cx)),
            );
        }
        if r.actions.resume {
            let w = ws.clone();
            row = row.child(
                Button::new(id("resume"))
                    .label(s::RESUME)
                    .compact()
                    .loading(loading)
                    .accessibility_label(format!("{title} {}", s::RESUME))
                    .on_click(move |_, _, cx| super::task_verb(&w, task.clone(), false, cx)),
            );
        }
    }
    div()
        .id(id("row"))
        .role(Role::ListItem)
        .aria_label(format!(
            "{title}, {}: {tier}, {}",
            "정리 단계",
            facts.join(", ")
        ))
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(t.border_subtle))
        .bg(rgb(t.bg_raised))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.clone()),
                )
                .child(chip(tier, tone)),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(facts.join(" · ")),
        )
        .when(!r.reasons.is_empty(), |d| {
            d.child(
                div()
                    .text_xs()
                    .text_color(rgb(t.text_secondary))
                    .child(r.reasons.join(" · ")),
            )
        })
        .child(row)
        .into_any_element()
}
