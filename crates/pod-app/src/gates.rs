//! Gates inbox (P5, docs/09 §8, backend docs/11 §3.3): pending approval
//! gates with their message, origin task / terminal and expiry; Approve /
//! Reject with an optional note (`gate.resolve`); recent decisions below.
//!
//! The pending set is kept by the workspace ([`crate::orch::OrchState`]) from
//! the status stream (`status.subscribe` snapshot + `gate.changed`), so the
//! status bar count and toasts work without this panel. The panel reads
//! `gate.list` once when opened (history of resolved gates) and re-renders
//! once a second only while a pending gate has an expiry (countdown).

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use pod_ui::{ControlState, Frame, FrameKind, Tone, semantic_button};
use podd::protocol::{self as p, GateDecision, GateInfo, GateState, orch_codes};
use serde_json::json;

use crate::groups::error_text;
use crate::orch::{gate_origin, gate_state_label, mode, now_ms, tc, tokens};
use crate::source::PoddSource;
use crate::workspace::{ToastKind, Workspace};

pub const GATES_PANEL_NAME: &str = "PodGates";
/// Resolved gates shown under the pending ones.
const HISTORY: usize = 12;

/// `75_000` -> `1m 15s`; `5_000` -> `5s`; hours and days for long ones.
pub fn duration_text(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..=59 => crate::tr!(DURATION_SECONDS; s;),
        60..=3599 => crate::tr!(DURATION_MINUTES; ; s / 60, s % 60),
        3600..=86_399 => crate::tr!(DURATION_HOURS; ; s / 3600, (s % 3600) / 60),
        _ => crate::tr!(DURATION_DAYS; ; s / 86_400, (s % 86_400) / 3600),
    }
}

/// Expiry line of a pending gate at `now`.
pub fn expiry_text(g: &GateInfo, now: u64) -> Option<String> {
    let e = g.expires_ms?;
    Some(if e <= now {
        crate::strings::EXPIRING_NOW.into()
    } else {
        crate::tr!(EXPIRES_IN; ; duration_text(e - now))
    })
}

/// Why a `gate.resolve` failed.
pub(crate) struct ResolveError {
    pub text: String,
    /// Already decided elsewhere (CLI, expiry, the other surface).
    pub already_resolved: bool,
}

/// The one gate decision path (inbox panel, response-queue cards): sends
/// `gate.resolve`, merges the answer into the workspace's gate set (which
/// re-renders every surface showing the gate) and toasts a failure. `done`
/// runs after that, for the caller's own busy/error state.
pub(crate) fn resolve_gate(
    source: &PoddSource,
    workspace: WeakEntity<Workspace>,
    params: p::GateResolveParams,
    cx: &mut App,
    done: impl FnOnce(Result<(), ResolveError>, &mut App) + 'static,
) {
    let id = params.id.clone();
    let rx = source.call::<p::GateResolve>(&params);
    cx.spawn(async move |cx| {
        let r = rx.await;
        cx.update(|cx| {
            let r = match r {
                Ok(g) => {
                    let _ = workspace.update(cx, |w, cx| {
                        w.orch.apply(g);
                        w.touch(cx);
                    });
                    Ok(())
                }
                Err(e) => {
                    let _ = workspace.update(cx, |w, cx| {
                        w.raise_toast(
                            ToastKind::Error,
                            crate::tr!(COULD_NOT_RESOLVE_ID; id;),
                            e.message.clone(),
                            cx,
                        )
                    });
                    // Already decided elsewhere (CLI, expiry): podd sends the current gate.
                    Err(ResolveError {
                        text: error_text(&e),
                        already_resolved: e.code == Some(orch_codes::GATE_RESOLVED),
                    })
                }
            };
            done(r, cx);
        })
    })
    .detach();
}

pub struct GatesPanel {
    source: PoddSource,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    notes: HashMap<String, Entity<InputState>>,
    busy: HashMap<String, GateDecision>,
    errors: HashMap<String, String>,
    load_error: Option<String>,
    _tick: Task<()>,
}

impl GatesPanel {
    pub fn new(
        source: PoddSource,
        workspace: WeakEntity<Workspace>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Countdown: re-render once a second while some pending gate expires.
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(()) = this.update(cx, |p, cx| {
                    if p.pending(cx).iter().any(|g| g.expires_ms.is_some()) {
                        cx.notify();
                    }
                }) else {
                    break;
                };
            }
        });
        let mut this = Self {
            source,
            workspace,
            focus: cx.focus_handle(),
            notes: HashMap::new(),
            busy: HashMap::new(),
            errors: HashMap::new(),
            load_error: None,
            _tick: tick,
        };
        this.reload(cx);
        this
    }

    fn pending(&self, cx: &App) -> Vec<GateInfo> {
        self.workspace
            .upgrade()
            .map(|w| w.read(cx).orch.pending())
            .unwrap_or_default()
    }

    /// `gate.list`: every gate (history), merged into the workspace's set.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let rx = self
            .source
            .call::<p::GateList>(&p::GateListParams::default());
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                match r {
                    Ok(list) => {
                        p.load_error = None;
                        if let Some(ws) = p.workspace.upgrade() {
                            ws.update(cx, |w, cx| {
                                for g in list.gates {
                                    w.orch.apply(g);
                                }
                                cx.notify();
                            });
                        }
                    }
                    Err(e) => p.load_error = Some(error_text(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn resolve(
        &mut self,
        id: String,
        decision: GateDecision,
        choice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.busy.contains_key(&id) {
            return;
        }
        let note = self
            .notes
            .get(&id)
            .map(|n| n.read(cx).value().trim().to_string())
            .filter(|n| !n.is_empty());
        self.busy.insert(id.clone(), decision);
        self.errors.remove(&id);
        let me = cx.entity().downgrade();
        let params = p::GateResolveParams {
            id: id.clone(),
            decision,
            note,
            choice,
        };
        resolve_gate(
            &self.source,
            self.workspace.clone(),
            params,
            cx,
            move |r, cx| {
                let _ = me.update(cx, |p, cx| {
                    p.busy.remove(&id);
                    if let Err(e) = r {
                        if e.already_resolved {
                            p.reload(cx);
                        }
                        p.errors.insert(id.clone(), e.text);
                    }
                    cx.notify();
                });
            },
        );
        cx.notify();
    }

    fn open_origin(&self, g: &GateInfo, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspace.upgrade() else {
            return;
        };
        let (terminal, task) = (g.terminal.clone(), g.task.clone());
        cx.defer_in(window, move |_, window, cx| {
            ws.update(cx, |w, cx| {
                let term = terminal.filter(|t| w.terminal(t).is_some()).or_else(|| {
                    task.as_deref()
                        .and_then(|t| w.task(t))
                        .and_then(|t| t.terminals.first().cloned())
                });
                match term {
                    Some(t) => w.open_terminal(&t, false, window, cx),
                    None => w.raise_toast(
                        ToastKind::Info,
                        crate::strings::NO_TERMINAL_FOR_THIS_GATE,
                        crate::strings::ITS_TERMINAL_OR_TASK_IS_GONE,
                        cx,
                    ),
                }
            })
        });
    }

    fn note_input(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        self.notes
            .entry(id.to_string())
            .or_insert_with(|| {
                cx.new(|cx| {
                    InputState::new(window, cx)
                        .placeholder(crate::strings::OPTIONAL_NOTE_FOR_THE_AGENT)
                })
            })
            .clone()
    }

    fn render_pending(
        &mut self,
        g: &GateInfo,
        now: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens();
        let note = self.note_input(&g.id, window, cx);
        let busy = self.busy.get(&g.id).copied();
        let state = |d: GateDecision| {
            if busy == Some(d) {
                ControlState::Loading
            } else if busy.is_some() {
                ControlState::Disabled
            } else {
                ControlState::Ready
            }
        };
        let me = cx.entity().downgrade();
        let gate = g.clone();
        let expiry = expiry_text(g, now);
        Frame::new(SharedString::from(format!("gate-{}", g.id)), FrameKind::List, format!("{} · {}", g.id, g.title), g.message.clone())
            .render(
                mode(),
                [
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(tc(t.text_secondary))
                        .child(gate_origin(g))
                        .child(crate::tr!(REQUESTED_AGO; ; duration_text(now.saturating_sub(g.created_ms))))
                        .when_some(expiry, |d, e| d.child(div().px_1().rounded_sm().bg(tc(t.warning_bg)).text_color(tc(t.warning_fg)).child(e)))
                        .when(g.expires_ms.is_none(), |d| d.child(crate::strings::NO_EXPIRY))
                        .child(
                            div()
                                .id(SharedString::from(format!("gate-open-{}", g.id)))
                                .text_color(tc(t.accent_fg))
                                .cursor_pointer()
                                .child(crate::strings::OPEN_TERMINAL)
                                .on_click(cx.listener(move |p, _, window, cx| p.open_origin(&gate, window, cx))),
                        )
                        .into_any_element(),
                    Input::new(&note).into_any_element(),
                    {
                        // v17: a gate with options is approved by picking one
                        // (`orch_ui::model::gate_buttons`, shared with the inbox and kanban).
                        let mut row = div().flex().flex_wrap().gap_2();
                        for (n, b) in crate::orch_ui::model::gate_buttons(g).into_iter().enumerate() {
                            let decision = if b.approve { GateDecision::Approve } else { GateDecision::Reject };
                            let id = match (&b.choice, b.approve) {
                                (Some(_), _) => format!("gate-choice-{}-{n}", g.id),
                                (None, true) => format!("gate-approve-{}", g.id),
                                (None, false) => format!("gate-reject-{}", g.id),
                            };
                            let (me, gid, choice) = (me.clone(), g.id.clone(), b.choice.clone());
                            row = row.child(semantic_button(
                                SharedString::from(id),
                                b.label.clone(),
                                if b.approve { Tone::Success } else { Tone::Danger },
                                mode(),
                                state(decision),
                                move |_, cx| {
                                    let _ = me.update(cx, |p, cx| p.resolve(gid.clone(), decision, choice.clone(), cx));
                                },
                            ));
                        }
                        row
                    }
                        .into_any_element(),
                    match self.errors.get(&g.id) {
                        Some(e) => div().text_xs().text_color(tc(t.danger_fg)).child(e.clone()).into_any_element(),
                        None => div().into_any_element(),
                    },
                ],
            )
            .max_w_full()
            .into_any_element()
    }
}

fn render_resolved(g: &GateInfo) -> impl IntoElement {
    let t = tokens();
    let fg = match g.state {
        GateState::Approved => t.success_fg,
        GateState::Rejected => t.danger_fg,
        _ => t.warning_fg,
    };
    div()
        .flex()
        .flex_col()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(tc(t.border_subtle))
        .child(
            div()
                .flex()
                .gap_2()
                .text_sm()
                .child(
                    div()
                        .text_color(tc(fg))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(gate_state_label(g.state)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(format!("{} · {}", g.id, g.title)),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(tc(t.text_secondary))
                .truncate()
                .child(format!("{} — {}", g.message, gate_origin(g))),
        )
        .when_some(g.note.clone(), |d, n| {
            d.child(
                div()
                    .text_xs()
                    .text_color(tc(t.text_muted))
                    .child(crate::tr!(NOTE_N; n;)),
            )
        })
}

impl EventEmitter<PanelEvent> for GatesPanel {}

impl Focusable for GatesPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for GatesPanel {
    fn panel_name(&self) -> &'static str {
        GATES_PANEL_NAME
    }

    fn dump(&self, _cx: &App) -> PanelState {
        let mut s = PanelState::new(GATES_PANEL_NAME);
        s.info = PanelInfo::panel(json!({}));
        s
    }

    fn on_removed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspace.upgrade() {
            cx.defer(move |cx| ws.update(cx, |w, cx| w.orch_panel_removed(cx)));
        }
    }
}

impl gpui_kit::component::dock::Panel for GatesPanel {
    fn tab_name(&self, cx: &App) -> Option<SharedString> {
        let n = self.pending(cx).len();
        Some(
            if n > 0 {
                crate::tr!(GATES_N; n;)
            } else {
                crate::strings::GATES.to_string()
            }
            .into(),
        )
    }

    fn title(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.pending(cx).len();
        div().child(if n > 0 {
            crate::tr!(GATES_N; n;)
        } else {
            crate::strings::GATES.to_string()
        })
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for GatesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens();
        let now = now_ms();
        let pending = self.pending(cx);
        let resolved: Vec<GateInfo> = self
            .workspace
            .upgrade()
            .map(|w| w.read(cx).orch.resolved())
            .unwrap_or_default();
        // Drop note inputs of gates that are gone.
        let live: std::collections::HashSet<&str> = pending.iter().map(|g| g.id.as_str()).collect();
        self.notes.retain(|id, _| live.contains(id.as_str()));
        let cards: Vec<AnyElement> = pending
            .iter()
            .map(|g| self.render_pending(g, now, window, cx))
            .collect();
        div()
            .id("gates-panel")
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .bg(tc(t.bg_canvas))
            .text_color(tc(t.text_primary))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(tc(t.border_subtle))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(crate::tr!(PENDING_APPROVALS_306; ; pending.len())),
                    )
                    .child(
                        Button::new("gates-reload")
                            .icon(pod_ui::kit_icon(pod_ui::Icon::RefreshCw))
                            .accessibility_label("승인 요청 새로고침")
                            .ghost()
                            .compact()
                            .tooltip(crate::strings::RE_READ_GATES_GATE_LIST)
                            .on_click(cx.listener(|p, _, _, cx| p.reload(cx))),
                    ),
            )
            .when_some(self.load_error.clone(), |d, e| {
                d.child(
                    div()
                        .px_2()
                        .pt_2()
                        .child(pod_ui::banner(pod_ui::Tone::Danger, mode(), e, None).text_xs()),
                )
            })
            .child(div().flex().flex_col().gap_2().p_2().children(cards).when(
                pending.is_empty(),
                |d| {
                    d.child(
                        Frame::new(
                            "gates-empty",
                            FrameKind::Empty,
                            crate::strings::NO_PENDING_GATES,
                            crate::strings::GATE_HELP,
                        )
                        .render(mode(), []),
                    )
                },
            ))
            .child(
                div()
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .text_xs()
                    .text_color(tc(t.text_muted))
                    .child(crate::tr!(RECENT_DECISIONS; ; resolved.len().min(HISTORY))),
            )
            .children(resolved.iter().take(HISTORY).map(render_resolved))
    }
}

#[cfg(test)]
mod tests {
    use super::{duration_text, expiry_text};
    use podd::protocol::{GateInfo, GateState};

    #[test]
    fn durations() {
        assert_eq!(duration_text(0), "0초");
        assert_eq!(duration_text(5_400), "5초");
        assert_eq!(duration_text(75_000), "1분 15초");
        assert_eq!(duration_text(3_600_000 + 120_000), "1시간 02분");
        assert_eq!(duration_text(2 * 86_400_000 + 3_600_000), "2일 1시간");
    }

    #[test]
    fn expiry_lines() {
        let mut g = GateInfo {
            id: "g1".into(),
            title: "t".into(),
            message: "m".into(),
            task: None,
            group: None,
            terminal: None,
            state: GateState::Pending,
            created_ms: 0,
            resolved_ms: None,
            note: None,
            expires_ms: None,
            run: None,
            item: None,
            options: Vec::new(),
            choice: None,
            requested_by: None,
            decided_by: None,
        };
        assert_eq!(expiry_text(&g, 10), None);
        g.expires_ms = Some(70_000);
        assert_eq!(expiry_text(&g, 10_000).unwrap(), "1분 00초 후 만료");
        assert_eq!(
            expiry_text(&g, 80_000).unwrap(),
            crate::strings::EXPIRING_NOW
        );
    }
}
