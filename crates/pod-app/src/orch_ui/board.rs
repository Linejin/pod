//! The Run board page: runs → items by column with dependency indent →
//! worker cards, unconfirmed wake-ups, and the board's keyboard actions.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use podd::protocol::ItemState;

use super::model::{self, ItemRow, RunEntry, Tone, WorkerCard};
use super::pages::{Page, ago, chip, nav, radio, status_line, subheading};
use super::{WorkerVerb, read, s, state, with_ws};
use crate::orch::mode;
use crate::qa_marks::QaId as _;
use crate::workspace::Workspace;

/// Board keys while the board itself (not one of its buttons) has focus:
/// ↑/↓ select a worker card, `[`/`]` switch runs, Enter opens the
/// terminal, K keep, R release, A abandon, S stop (asks), V review.
pub(crate) fn board_key(
    ws: &WeakEntity<Workspace>,
    key: &str,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let (cards, runs, run) = {
        let st = state(cx);
        let run = st.selected_run().map(|r| r.id().to_string());
        let cards = st.selected_run().map(model::cards).unwrap_or_default();
        (
            cards,
            st.ordered()
                .iter()
                .map(|r| r.id().to_string())
                .collect::<Vec<_>>(),
            run,
        )
    };
    let pos = state(cx)
        .selected_card
        .as_ref()
        .and_then(|a| cards.iter().position(|c| &c.attempt == a));
    let card = pos.and_then(|i| cards.get(i)).cloned();
    match key {
        "down" | "up" if !cards.is_empty() => {
            let n = cards.len();
            let i = match (pos, key) {
                (None, "down") => 0,
                (None, _) => n - 1,
                (Some(i), "down") => (i + 1) % n,
                (Some(i), _) => (i + n - 1) % n,
            };
            state(cx).selected_card = Some(cards[i].attempt.clone());
        }
        "[" | "]" if runs.len() > 1 => {
            let i = run
                .and_then(|r| runs.iter().position(|x| *x == r))
                .unwrap_or(0);
            let j = if key == "]" {
                (i + 1) % runs.len()
            } else {
                (i + runs.len() - 1) % runs.len()
            };
            let st = state(cx);
            st.selected = Some(runs[j].clone());
            st.selected_card = None;
        }
        "enter" => {
            let Some(term) = card
                .filter(|c| c.actions.open_terminal)
                .and_then(|c| c.terminal)
            else {
                return false;
            };
            let _ = ws.update(cx, |w, cx| w.open_terminal(&term, false, window, cx));
            return true;
        }
        "v" => {
            let Some(task) = card.filter(|c| c.actions.open_review).and_then(|c| c.task) else {
                return false;
            };
            let _ = ws.update(cx, |w, cx| w.open_review(&task, window, cx));
            return true;
        }
        "k" | "r" | "a" | "s" => {
            let Some(c) = card else { return false };
            let (on, verb) = match key {
                "k" => (c.actions.keep, WorkerVerb::Keep),
                "r" => (c.actions.release, WorkerVerb::Release),
                "a" => (c.actions.abandon, WorkerVerb::Abandon),
                _ => (c.actions.stop, WorkerVerb::Stop),
            };
            if !on {
                return false;
            }
            super::worker_verb(ws, c.attempt.clone(), verb, window, cx);
            return true;
        }
        _ => return false,
    }
    with_ws(ws, cx, |_, cx| cx.notify());
    true
}

pub(super) fn board(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let mut body = div()
        .id("orch-board")
        .role(Role::Group)
        .aria_label(s::BOARD)
        .flex()
        .flex_col()
        .gap_3()
        .child(nav(Page::Board, ws, cx));
    if let Some(line) = status_line(cx) {
        return body.child(line).into_any_element();
    }
    let st = state(cx);
    let runs: Vec<RunEntry> = st.ordered().into_iter().cloned().collect();
    let selected = st.selected_run().map(|r| r.id().to_string());
    if runs.is_empty() {
        return body
            .child(
                pod_ui::Frame::new(
                    "orch-runs-empty",
                    pod_ui::FrameKind::Empty,
                    s::NO_RUNS,
                    s::NO_RUNS_HELP,
                )
                .render(mode(), []),
            )
            .into_any_element();
    }
    // Run picker: a radio group (ui-10: checked state, not a filled button).
    let refs: Vec<&RunEntry> = runs.iter().collect();
    let g = model::run_group(&refs, selected.as_deref());
    let mut list = pod_ui::radio_group("orch-runs", g.label.clone(), mode());
    for o in &g.options {
        let (w, id) = (ws.clone(), o.value.clone());
        list = list.child(radio(
            format!("orch-run-{id}"),
            &g,
            o,
            truncate(&o.text, 60),
            move |_, cx| {
                state(cx).selected = Some(id.clone());
                with_ws(&w, cx, |_, cx| cx.notify());
            },
        ));
    }
    body = body.child(div().flex().child(list));
    let Some(run) = selected.and_then(|id| runs.iter().find(|r| r.id() == id)) else {
        return body.into_any_element();
    };
    let now = super::fixture::now_ms();
    let tasks: Vec<crate::vm::TaskVm> = ws
        .upgrade()
        .map(|w| w.read(cx).tasks.clone())
        .unwrap_or_default();
    // Run header: objective, coordinator, counts.
    let mut facts = vec![
        format!("{} {}", s::ITEMS, run.info.items.len()),
        format!(
            "{} {}",
            s::WORKERS,
            run.info
                .attempts
                .iter()
                .filter(|a| a.state.is_open())
                .count()
        ),
    ];
    if run.info.unread > 0 {
        facts.push(format!("{} {}", s::UNREAD_FOR_COORD, run.info.unread));
    }
    let q = run.open_questions();
    if q > 0 {
        facts.push(format!("{} {q}", s::OPEN_QUESTIONS));
    }
    let mut head = div()
        .id(SharedString::from(format!("orch-run-head-{}", run.id())))
        .role(Role::Group)
        .aria_label(format!(
            "{} {}: {}",
            s::RUNS,
            run.id(),
            run.info.run.objective
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
                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!(
                    "{} · {}",
                    run.id(),
                    s::OBJECTIVE
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(run.info.run.objective.clone()),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(facts.join(" · ")),
        );
    if let Some(term) = run.info.run.coordinator_terminal.clone() {
        let w = ws.clone();
        head = head.child(
            div().flex().child(
                Button::new(SharedString::from(format!("orch-coord-{}", run.id())))
                    .label(s::OPEN_COORDINATOR)
                    .compact()
                    .icon(pod_ui::kit_icon(pod_ui::Icon::SquareTerminal))
                    .accessibility_label(format!("{} {} ({term})", run.id(), s::OPEN_COORDINATOR))
                    .on_click(move |_, window, cx| {
                        let _ = w.update(cx, |w, cx| w.open_terminal(&term, false, window, cx));
                    }),
            ),
        );
    }
    body = body.child(head);
    // orch-02: wake-ups typed but not confirmed, with a retry.
    for st in model::stuck_wakes(run) {
        body = body.child(stuck_wake(ws, run.id(), st, now, cx));
    }
    // Items by column, indented by dependency depth.
    body = body.child(subheading(s::ITEMS));
    let mut cols = div().id("orch-columns").flex().flex_wrap().gap_2();
    for (state_, rows) in model::columns(run) {
        cols = cols.child(column(state_, &rows));
    }
    body = body.child(cols);
    // Worker cards.
    let cards = model::cards(run);
    body = body
        .child(subheading(format!("{} ({})", s::WORKERS, cards.len())))
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(s::BOARD_KEYS),
        );
    let picked = state(cx).selected_card.clone();
    if cards.is_empty() {
        body = body.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_muted))
                .child(s::NO_WORKERS),
        );
    } else {
        let mut grid = div()
            .id("orch-cards")
            .role(Role::List)
            .aria_label(s::WORKERS)
            .flex()
            .flex_wrap()
            .gap_2();
        for c in &cards {
            let branch = c
                .task
                .as_ref()
                .and_then(|id| tasks.iter().find(|t| &t.id == id))
                .and_then(|t| t.branch.clone());
            grid = grid.child(worker_card(
                ws,
                c,
                branch,
                now,
                picked.as_deref() == Some(c.attempt.as_str()),
                cx,
            ));
        }
        body = body.child(grid);
    }
    body.into_any_element()
}

/// One unconfirmed wake-up (orch-02): who, since when, retry (Enter in its
/// terminal) and open the terminal.
fn stuck_wake(
    ws: &WeakEntity<Workspace>,
    run: &str,
    st: model::StuckWake,
    now: u64,
    cx: &App,
) -> AnyElement {
    let t = crate::theme::tokens();
    let who = st.who();
    let since = format!("{}{}", ago(st.since_ms, now), s::WAKE_SINCE);
    let busy = read(cx).is_some_and(|x| x.busy.contains(&st.busy_key()));
    let key = st.attempt.clone().unwrap_or_else(|| "coordinator".into());
    let id = |part: &str| SharedString::from(format!("orch-wake-{run}-{key}-{part}"));
    let mut actions = div().flex().flex_shrink_0().gap_1();
    if let Some(term) = st.terminal.clone() {
        let (w, stuck) = (ws.clone(), st.clone());
        actions = actions.child(
            Button::new(id("retry"))
                .label(s::WAKE_RETRY)
                .compact()
                .loading(busy)
                .accessibility_label(format!("{who} {}", s::WAKE_RETRY))
                .on_click(move |_, _, cx| super::rewake(&w, stuck.clone(), cx)),
        );
        let w = ws.clone();
        actions = actions.child(
            Button::new(id("open"))
                .label(s::OPEN_TERMINAL)
                .compact()
                .accessibility_label(format!("{who} {}", s::OPEN_TERMINAL))
                .on_click(move |_, window, cx| {
                    let _ = w.update(cx, |w, cx| w.open_terminal(&term, false, window, cx));
                }),
        );
    }
    let mark = format!("orch-wake:{run}:{key}");
    div()
        .id(id("row"))
        .qa_id(move || mark)
        .role(Role::Group)
        .aria_label(format!(
            "{who}: {} ({since}). {}",
            s::WAKE_STUCK,
            s::WAKE_STUCK_HELP
        ))
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(6.))
        .rounded_md()
        .bg(rgb(t.warning_bg))
        .text_color(rgb(t.text_primary))
        .child(pod_ui::icon(pod_ui::Icon::TriangleAlert, t.warning_fg))
        .child(
            // Korean breaks between words only (ui-14).
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    pod_ui::keep_all(&format!("{who}: {} ({since})", s::WAKE_STUCK))
                        .justify_start()
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .child(
                    pod_ui::keep_all(s::WAKE_STUCK_HELP)
                        .justify_start()
                        .text_sm()
                        .text_color(rgb(t.text_secondary)),
                ),
        )
        .child(actions)
        .into_any_element()
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

fn column(st: ItemState, rows: &[ItemRow]) -> AnyElement {
    let t = crate::theme::tokens();
    let label = model::item_state_label(st);
    let mut col = div()
        .id(SharedString::from(format!("orch-col-{label}")))
        .role(Role::List)
        .aria_label(format!("{} {label}, {}개", s::ITEMS, rows.len()))
        .flex()
        .flex_col()
        .gap_1()
        .w(px(190.))
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(t.border_subtle))
        .bg(rgb(t.bg_raised))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(t.text_secondary))
                .child(format!("{label} {}", rows.len())),
        );
    for r in rows {
        let mut sub = vec![r.id.clone()];
        if !r.deps.is_empty() {
            sub.push(format!("{} {}", s::DEPENDS_ON, r.deps.join(",")));
        }
        if r.attempts > 0 {
            sub.push(format!("{} {}", s::ATTEMPTS, r.attempts));
        }
        col = col.child(
            div()
                .id(SharedString::from(format!("orch-item-{}", r.id)))
                .role(Role::ListItem)
                .aria_label(format!(
                    "{} {} {}, {label}, {}",
                    s::ITEM,
                    r.id,
                    r.title,
                    sub[1..].join(", ")
                ))
                .pl(px(r.depth.min(4) as f32 * 10.))
                .flex()
                .flex_col()
                .child(div().text_sm().truncate().child(r.title.clone()))
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .text_xs()
                        .text_color(rgb(t.text_muted))
                        .child(sub.join(" · "))
                        .when(r.circuit_open, |d| {
                            d.child(chip(s::BREAKER_OPEN, Tone::Bad))
                        }),
                ),
        );
    }
    col.into_any_element()
}

fn worker_card(
    ws: &WeakEntity<Workspace>,
    c: &WorkerCard,
    branch: Option<String>,
    now: u64,
    picked: bool,
    cx: &App,
) -> AnyElement {
    let t = crate::theme::tokens();
    let busy = read(cx).map(|s| s.busy.clone()).unwrap_or_default();
    let id = |part: &str| SharedString::from(format!("orch-card-{}-{part}", c.attempt));
    let title = format!("{} · {}", c.attempt, c.item_title);
    let live = c.status.as_ref().and_then(crate::vm::StatusVm::from_json);
    let kind = crate::agent_kind::identify_parts(live.as_ref(), Some(c.agent.as_str()), &[]).kind;
    let mut head = div()
        .flex()
        .items_center()
        .gap_2()
        // Which service this worker runs (docs/10 §D5.11).
        .child(crate::agent_kind::mark_el(
            &format!("orch-{}", c.attempt),
            &kind,
            14.,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.clone()),
        )
        .child(chip(c.state_label(), c.tone()));
    if let Some(st) = c.status.as_ref().and_then(crate::vm::StatusVm::from_json)
        && c.terminal.is_some()
        && c.worker != podd::protocol::WorkerState::Released
    {
        head = head.child(crate::orch::status_badge(Some(&st)).render_compact(mode()));
    }
    let mut facts = vec![c.agent.clone()];
    if let Some(task) = &c.task {
        facts.push(match &branch {
            Some(b) => format!("{task} ({b})"),
            None => task.clone(),
        });
    }
    facts.push(format!(
        "{} {}",
        s::LAST_ACTIVITY,
        ago(c.last_activity_ms, now)
    ));
    facts.push(model::worker_state_label(c.worker).to_string());
    let mut card = div()
        .id(id("card"))
        .role(Role::ListItem)
        .aria_label(if picked {
            format!("{}, {}, 선택됨", c.aria(), kind.accessible_name())
        } else {
            format!("{}, {}", c.aria(), kind.accessible_name())
        })
        .flex()
        .flex_col()
        .gap_1()
        .w(px(360.))
        .p_3()
        .rounded_md()
        .when(picked, |d| d.border_2().border_color(rgb(t.focus_ring)))
        .when(!picked, |d| {
            d.border_1().border_color(rgb(if c.question.is_some() {
                t.warning_fg
            } else {
                t.border_subtle
            }))
        })
        .bg(rgb(t.bg_raised))
        .child(head)
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(facts.join(" · ")),
        );
    if let Some(q) = &c.question {
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(t.warning_fg))
                .child(format!("{}: {q}", s::M_QUESTION)),
        );
    } else if c.waiting_answer {
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(t.warning_fg))
                .child(s::WAITING_ANSWER),
        );
    }
    if let Some(ms) = c.wake_unconfirmed_ms {
        card = card.child(div().text_xs().text_color(rgb(t.warning_fg)).child(format!(
            "{} · {}{}",
            s::WAKE_UNCONFIRMED,
            ago(ms, now),
            s::WAKE_SINCE
        )));
    }
    if let Some((k, subj)) = &c.last_mail {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(t.text_secondary))
                .truncate()
                .child(format!("{} · {subj}", model::mail_type_label(*k))),
        );
    }
    if let Some(q) = &c.queue {
        card = card.child(div().text_xs().text_color(rgb(t.warning_fg)).child(format!(
            "{}: {}",
            s::QUEUED,
            q.reason
        )));
    }
    if let Some(at) = c.release_at_ms {
        card = card.child(div().text_xs().text_color(rgb(t.text_muted)).child(format!(
            "{} {}",
            s::RELEASE_IN,
            crate::gates::duration_text(at.saturating_sub(now))
        )));
    }
    if c.unread > 0 {
        card = card.child(div().text_xs().text_color(rgb(t.text_muted)).child(format!(
            "{} {}",
            s::UNREAD,
            c.unread
        )));
    }
    if !c.next.is_empty() {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .truncate()
                .child(c.next.clone()),
        );
    }
    if let Some(e) = &c.error {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(t.danger_fg))
                .truncate()
                .child(e.clone()),
        );
    }
    let a = c.actions;
    let mut row = div().flex().flex_wrap().gap_1().pt_1();
    if a.open_terminal {
        let (w, term) = (ws.clone(), c.terminal.clone().unwrap_or_default());
        row = row.child(
            Button::new(id("open"))
                .label(s::OPEN_TERMINAL)
                .compact()
                .primary()
                .accessibility_label(format!("{title} {}", s::OPEN_TERMINAL))
                .on_click(move |_, window, cx| {
                    let _ = w.update(cx, |w, cx| w.open_terminal(&term, false, window, cx));
                }),
        );
    }
    if a.open_review {
        let (w, task) = (ws.clone(), c.task.clone().unwrap_or_default());
        row = row.child(
            Button::new(id("review"))
                .label(s::OPEN_REVIEW)
                .compact()
                .accessibility_label(format!("{title} {}", s::OPEN_REVIEW))
                .on_click(move |_, window, cx| {
                    let _ = w.update(cx, |w, cx| w.open_review(&task, window, cx));
                }),
        );
    }
    for (on, verb) in [
        (a.keep, WorkerVerb::Keep),
        (a.release, WorkerVerb::Release),
        (a.abandon, WorkerVerb::Abandon),
        (a.stop, WorkerVerb::Stop),
    ] {
        if !on {
            continue;
        }
        let (w, attempt) = (ws.clone(), c.attempt.clone());
        let loading = busy.contains(&super::busy_key(&c.attempt, verb));
        row = row.child(
            Button::new(id(verb.label()))
                .label(verb.label())
                .compact()
                .when(verb == WorkerVerb::Stop, |b| b.danger())
                .loading(loading)
                .accessibility_label(format!("{title} {}", verb.label()))
                .on_click(move |_, window, cx| {
                    super::worker_verb(&w, attempt.clone(), verb, window, cx)
                }),
        );
    }
    card.child(row).into_any_element()
}
