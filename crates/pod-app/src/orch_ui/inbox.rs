//! Coordinator inbox (right dock, docs/19 §8): every run's mail as threads,
//! open worker questions first, with a reply box (a person answers for the
//! coordinator: `orch.mail.reply`, `from: human`), choice buttons for
//! questions that carry `payload.options`, and the pending approval gates of
//! the run's worker tasks (approve / reject, as in the gates inbox).

use std::collections::HashMap;

use gpui_kit::base::dock::{PanelEvent, PanelState};
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::panel_handle;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use podd::protocol::{GateDecision, GateInfo, Mail, MailType};

use super::model::{self, RunEntry, Thread};
use super::{read, s, state};
use crate::orch::mode;
use crate::workspace::Workspace;

pub const INBOX_PANEL_NAME: &str = "PodOrchInbox";

pub(crate) struct InboxPanel {
    workspace: WeakEntity<Workspace>,
    pub focus: FocusHandle,
    /// Reply boxes by thread root id.
    pub replies: HashMap<String, Entity<InputState>>,
    _sub: Option<Subscription>,
}

impl InboxPanel {
    /// The reply box of a thread (made on first use; Enter sends).
    pub fn reply_input(
        &mut self,
        root: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(i) = self.replies.get(root) {
            return i.clone();
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(s::REPLY_PLACEHOLDER));
        let (ws, id, inp) = (self.workspace.clone(), root.to_string(), input.clone());
        let submit: crate::chrome::Submit =
            std::rc::Rc::new(move |window: &mut Window, cx: &mut App| {
                send(&ws, &id, &inp, window, cx)
            });
        crate::chrome::submit_on_enter(&input, submit, window, cx);
        self.replies.insert(root.to_string(), input.clone());
        input
    }
}

/// Send the reply box's text and clear it on success of the hand-off.
fn send(
    ws: &WeakEntity<Workspace>,
    root: &str,
    input: &Entity<InputState>,
    window: &mut Window,
    cx: &mut App,
) {
    let body = input.read(cx).value().to_string();
    if super::reply(ws, root.to_string(), body, cx) {
        input.update(cx, |i, cx| i.set_value("", window, cx));
    }
}

impl EventEmitter<PanelEvent> for InboxPanel {}
impl Focusable for InboxPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_kit::base::dock::Panel for InboxPanel {
    fn panel_name(&self) -> &'static str {
        INBOX_PANEL_NAME
    }
    fn dump(&self, _: &App) -> PanelState {
        PanelState::new(INBOX_PANEL_NAME)
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.defer(|cx| state(cx).inbox = None);
    }
}
impl gpui_kit::component::dock::Panel for InboxPanel {
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        Some(title(cx).into())
    }
    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().child(title(cx))
    }
    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

fn title(cx: &App) -> String {
    let q = read(cx).map(|s| s.open_questions()).unwrap_or(0);
    if q > 0 {
        format!("{} {q}", s::INBOX)
    } else {
        s::INBOX.to_string()
    }
}

/// The inbox panel, registered with the module state (also used by the layout restore).
pub(crate) fn new_panel(
    workspace: WeakEntity<Workspace>,
    _window: &mut Window,
    cx: &mut App,
) -> Entity<InboxPanel> {
    let panel = cx.new(|cx| {
        let sub = workspace
            .upgrade()
            .map(|w| cx.observe(&w, |_, _, cx| cx.notify()));
        InboxPanel {
            workspace,
            focus: cx.focus_handle(),
            replies: HashMap::new(),
            _sub: sub,
        }
    });
    state(cx).inbox = Some(panel.downgrade());
    panel
}

/// Open (or focus) the inbox in the right dock.
pub(crate) fn open(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    use gpui_kit::component::dock::{DockPlacement, PanelId};
    if let Some(p) = state(cx).inbox.as_ref().and_then(|p| p.upgrade()) {
        ws.dock.update(cx, |d, cx| {
            d.select_panel(PanelId::from(p.entity_id()), window, cx)
        });
        let focus = p.read(cx).focus.clone();
        focus.focus(window, cx);
        return;
    }
    let panel = new_panel(cx.entity().downgrade(), window, cx);
    let focus = panel.read(cx).focus.clone();
    ws.dock.update(cx, |d, cx| {
        d.add_panel_view(
            panel_handle(panel),
            DockPlacement::Right,
            Some(px(420.)),
            window,
            cx,
        )
    });
    focus.focus(window, cx);
    ws.touch(cx);
}

/// The oldest unanswered question of all runs: `(run, root id)`.
pub(crate) fn next_question(cx: &App) -> Option<(String, String)> {
    let st = read(cx)?;
    st.ordered()
        .into_iter()
        .flat_map(|r| {
            model::threads(r)
                .into_iter()
                .filter(|t| t.open_question)
                .map(|t| (r.id().to_string(), t.root.id.clone()))
        })
        .min_by_key(|(_, id)| model::id_number(id))
}

/// Focus the reply box of the oldest unanswered question.
pub(crate) fn focus_next_question(window: &mut Window, cx: &mut App) {
    let Some((_, root)) = next_question(cx) else {
        return;
    };
    let Some(panel) = state(cx).inbox.as_ref().and_then(|p| p.upgrade()) else {
        return;
    };
    let input = panel.update(cx, |p, cx| p.reply_input(&root, window, cx));
    input.focus_handle(cx).focus(window, cx);
}

/// Decide a gate through the one decision path (`gates::resolve_gate`).
fn resolve_gate(
    ws: &WeakEntity<Workspace>,
    id: String,
    decision: GateDecision,
    choice: Option<String>,
    cx: &mut App,
) {
    let Some(source) = ws.upgrade().map(|w| w.read(cx).source.clone()) else {
        return;
    };
    let params = podd::protocol::GateResolveParams {
        id,
        decision,
        note: None,
        choice,
    };
    crate::gates::resolve_gate(&source, ws.clone(), params, cx, |_, _| {});
}

impl Render for InboxPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        let ws = self.workspace.clone();
        let runs: Vec<RunEntry> = read(cx)
            .map(|s| s.ordered().into_iter().cloned().collect())
            .unwrap_or_default();
        let gates: Vec<GateInfo> = ws
            .upgrade()
            .map(|w| w.read(cx).orch.pending())
            .unwrap_or_default();
        let q: usize = runs.iter().map(|r| r.open_questions()).sum();
        let mut body = div().flex().flex_col().gap_2().p_2();
        let mut any = false;
        let busy = read(cx).map(|s| s.busy.clone()).unwrap_or_default();
        for run in &runs {
            let threads = model::threads(run);
            let run_gates: Vec<&GateInfo> = model::run_gates(run, &gates);
            if threads.is_empty() && run_gates.is_empty() {
                continue;
            }
            any = true;
            let mut section = div()
                .id(SharedString::from(format!("orch-inbox-run-{}", run.id())))
                .role(Role::Group)
                .aria_label(format!(
                    "{} {} · {}",
                    s::RUNS,
                    run.id(),
                    run.info.run.objective
                ))
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .px_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(t.text_secondary))
                        .child(format!("{} · {}", run.id(), run.info.run.objective)),
                );
            for g in run_gates {
                section = section.child(gate_row(&ws, g));
            }
            let mut list = div()
                .id(SharedString::from(format!("orch-threads-{}", run.id())))
                .role(Role::List)
                .aria_label(format!("{} {}", s::INBOX, run.id()))
                .flex()
                .flex_col()
                .gap_2();
            for th in &threads {
                let input = th
                    .can_reply
                    .then(|| self.reply_input(&th.root.id, window, cx));
                list = list.child(thread(
                    &ws,
                    run,
                    th,
                    input,
                    busy.contains(&format!("reply:{}", th.root.id)),
                ));
            }
            body = body.child(section.child(list));
        }
        if !any {
            body = body.child(
                pod_ui::Frame::new(
                    "orch-inbox-empty",
                    pod_ui::FrameKind::Empty,
                    s::NO_MAIL,
                    s::NO_MAIL_HELP,
                )
                .render(mode(), []),
            );
        }
        // Drop reply boxes of threads that are gone (or answered and closed).
        let live: std::collections::HashSet<String> = runs
            .iter()
            .flat_map(|r| r.mail.iter().map(|m| m.id.clone()))
            .collect();
        self.replies.retain(|id, _| live.contains(id));
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(t.border_subtle))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if q > 0 {
                        format!("{} · {}: {q}", s::INBOX, s::OPEN_QUESTIONS)
                    } else {
                        format!("{} · {}", s::INBOX, s::NO_OPEN_QUESTION)
                    }),
            )
            .child({
                let w = ws.clone();
                Button::new("orch-inbox-board")
                    .label(s::BOARD)
                    .compact()
                    .accessibility_label(format!("{} (Ctrl+Shift+J)", s::BOARD))
                    .on_click(move |_, window, cx| {
                        let _ = w.update(cx, |w, cx| {
                            super::open_page(w, super::Page::Board, window, cx)
                        });
                    })
            });
        div()
            .id("orch-inbox")
            .role(Role::Group)
            .aria_label(s::INBOX)
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(rgb(t.bg_canvas))
            .text_color(rgb(t.text_primary))
            .child(header)
            .child(body)
    }
}

fn gate_row(ws: &WeakEntity<Workspace>, g: &GateInfo) -> AnyElement {
    let t = crate::theme::tokens();
    let mut row = div().flex().flex_wrap().items_center().gap_2();
    if !g.options.is_empty() {
        row = row.child(
            div()
                .text_xs()
                .text_color(rgb(t.text_secondary))
                .child(format!("{}:", s::GATE_CHOOSE)),
        );
    }
    for (n, b) in model::gate_buttons(g).into_iter().enumerate() {
        let (w, id, choice) = (ws.clone(), g.id.clone(), b.choice.clone());
        let decision = if b.approve {
            GateDecision::Approve
        } else {
            GateDecision::Reject
        };
        let tone = if b.approve {
            pod_ui::Tone::Success
        } else {
            pod_ui::Tone::Danger
        };
        row = row.child(pod_ui::semantic_button(
            SharedString::from(format!("orch-gate-{}-{n}", g.id)),
            b.label.clone(),
            tone,
            mode(),
            pod_ui::ControlState::Ready,
            move |_, cx| resolve_gate(&w, id.clone(), decision, choice.clone(), cx),
        ));
    }
    pod_ui::Frame::new(
        SharedString::from(format!("orch-gate-{}", g.id)),
        pod_ui::FrameKind::List,
        format!("{} · {}", s::GATES_FOR_RUN, g.title),
        g.message.clone(),
    )
    .render(mode(), [row.into_any_element()])
    .max_w_full()
    .into_any_element()
}

fn message(run: &RunEntry, m: &Mail, reply: bool) -> Div {
    let t = crate::theme::tokens();
    let subject = super::ko::subject(m);
    let who = format!(
        "{} → {}",
        model::address_label(run, &m.from),
        model::address_label(run, &m.to)
    );
    let tone = match m.kind {
        MailType::Question => model::Tone::Warn,
        MailType::Escalation => model::Tone::Bad,
        MailType::Done => model::Tone::Ok,
        MailType::Answer => model::Tone::Accent,
        _ => model::Tone::Muted,
    };
    let (fg, bg) = match tone {
        model::Tone::Warn => (t.warning_fg, t.warning_bg),
        model::Tone::Bad => (t.danger_fg, t.danger_bg),
        model::Tone::Ok => (t.success_fg, t.success_bg),
        model::Tone::Accent => (t.info_fg, t.info_bg),
        model::Tone::Muted => (t.text_secondary, t.bg_hover),
    };
    let mut d = div()
        .flex()
        .flex_col()
        .gap_1()
        .when(reply, |d| {
            d.pl_3().border_l_2().border_color(rgb(t.border_subtle))
        })
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_shrink_0()
                        .px_2()
                        .rounded_sm()
                        .text_xs()
                        .text_color(rgb(fg))
                        .bg(rgb(bg))
                        .child(model::mail_type_label(m.kind)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_sm()
                        .child(subject.clone()),
                )
                .when(m.fenced, |d| {
                    d.child(
                        div()
                            .text_xs()
                            .text_color(rgb(t.text_muted))
                            .child(s::FENCED),
                    )
                }),
        )
        .child(div().text_xs().text_color(rgb(t.text_muted)).child(format!(
            "{who} · {} · {}",
            super::pages::ago(m.created_ms, super::fixture::now_ms()),
            if m.read_ms.is_some() {
                s::READ
            } else {
                s::NOT_READ
            }
        )));
    let body = super::ko::body(m);
    if !body.trim().is_empty() && body.trim() != subject.trim() {
        // Long bodies are cut; the whole text is in `pod orch` / the agent's terminal.
        let body: String = body.chars().take(1200).collect();
        d = d.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(body),
        );
    }
    d
}

fn thread(
    ws: &WeakEntity<Workspace>,
    run: &RunEntry,
    th: &Thread,
    input: Option<Entity<InputState>>,
    busy: bool,
) -> AnyElement {
    let t = crate::theme::tokens();
    let root = &th.root;
    let name = format!(
        "{} {}: {} — {}",
        model::mail_type_label(root.kind),
        root.id,
        super::ko::subject(root),
        model::address_label(run, &root.from)
    );
    let mut card = div()
        .id(SharedString::from(format!("orch-thread-{}", root.id)))
        .role(Role::ListItem)
        .aria_label(if th.open_question {
            format!("{}, {}", s::OPEN_QUESTIONS, name)
        } else {
            name.clone()
        })
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(if th.open_question {
            t.warning_fg
        } else {
            t.border_subtle
        }))
        .bg(rgb(t.bg_raised))
        .child(message(run, root, false));
    for r in &th.replies {
        card = card.child(message(run, r, true));
    }
    if th.open_question && !th.choices.is_empty() {
        let mut row = div().flex().flex_wrap().gap_1().child(
            div()
                .text_xs()
                .text_color(rgb(t.text_secondary))
                .child(format!("{}:", s::CHOICE)),
        );
        for (n, choice) in th.choices.iter().enumerate() {
            let (w, id, c) = (ws.clone(), root.id.clone(), choice.clone());
            row = row.child(
                Button::new(SharedString::from(format!("orch-choice-{}-{n}", root.id)))
                    .label(choice.clone())
                    .compact()
                    .accessibility_label(format!("{} {}: {choice}", root.id, s::CHOICE))
                    .on_click(move |_, _, cx| {
                        super::reply(&w, id.clone(), c.clone(), cx);
                    }),
            );
        }
        card = card.child(row);
    }
    if let Some(input) = input {
        let (w, id, inp) = (ws.clone(), root.id.clone(), input.clone());
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w_0().child(Input::new(&input)))
                .child(
                    Button::new(SharedString::from(format!("orch-reply-{}", root.id)))
                        .label(s::REPLY)
                        .compact()
                        .when(th.open_question, |b| b.primary())
                        .loading(busy)
                        .icon(pod_ui::kit_icon(pod_ui::Icon::Send))
                        .accessibility_label(format!(
                            "{} {} ({})",
                            root.id,
                            s::REPLY,
                            model::address_label(run, &root.from)
                        ))
                        .on_click(move |_, window, cx| send(&w, &id, &inp, window, cx)),
                ),
        );
    }
    card.into_any_element()
}
