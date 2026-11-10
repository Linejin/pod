//! Center-dock orchestration pages: the Run board (runs → items by column
//! with dependency indent → worker cards), the cleanup screen for finished
//! workers, and the orchestration settings. One panel type, one page each.
//!
//! Every control is a toolkit button (Tab / Shift+Tab, Enter / Space) with a
//! Korean accessible name that says which worker or task it acts on.

use gpui_kit::base::dock::{PanelEvent, PanelState};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dock::panel_handle;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use podd::protocol::{ItemState, OrchSettings};

use super::model::{self, CleanupRow, ItemRow, RunEntry, Tone, WorkerCard};
use super::{WorkerVerb, read, s, state, with_ws};
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

impl Render for OrchPage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        let body = match self.page {
            Page::Board => board(&self.workspace, cx),
            Page::Cleanup => cleanup(&self.workspace, cx),
            Page::Settings => settings(&self.workspace, cx),
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
                    if board_key(&this.workspace, e.keystroke.key.as_str(), window, cx) {
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

fn subheading(text: impl Into<SharedString>) -> Div {
    div()
        .pt_2()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

fn chip(text: impl Into<SharedString>, tone: Tone) -> Div {
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
fn nav(here: Page, ws: &WeakEntity<Workspace>, cx: &App) -> Div {
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
fn status_line(cx: &App) -> Option<AnyElement> {
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

// ───────────────────────────── board ─────────────────────────────

fn board(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
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

// ───────────────────────────── cleanup ─────────────────────────────

fn cleanup(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
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

// ───────────────────────────── settings ─────────────────────────────

/// One radio option (ui-10): `pod_ui::choice` (RadioButton, checked =
/// selected), marked `radio:<name>:<checked>` for the native-QA tests.
fn radio(
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

fn settings(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
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
