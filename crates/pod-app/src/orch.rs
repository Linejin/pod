//! Orchestration UI glue (P5, docs/09 §8): the state the workspace keeps for
//! gates and the orchestration panels, routing of the orchestration events of
//! the status stream (`gate.changed`, `group.check`, `review.delivery`), and
//! the small pieces the shared sidebar / status bar embed.
//!
//! The panels themselves live in [`crate::groups`] (New Group dialog, Group
//! panel), [`crate::gates`] (gates inbox) and [`crate::tree`] (agent tree).
//! Presentation uses the pod-ui design system (StatusBadge, StatusSummary,
//! TreeRow, Frame) so it already matches the D2 restyle.

use std::collections::BTreeMap;

use gpui_kit::component::dock::{DockPlacement, PanelId, panel_handle};
use gpui_kit::*;
use pod_status::{AgentState as S, BlockedKind, Source};
use pod_ui::{StatusBadge, StatusSummary};
use podd::protocol::{CheckJobState, GateInfo, GateState, GroupInfo};

use crate::gates::GatesPanel;
use crate::groups::GroupPanel;
use crate::source::SourceEvent;
use crate::tree::AgentTreePanel;
use crate::vm::{AgentState, Liveness, StatusVm, TerminalVm};
use crate::workspace::{ToastKind, Workspace};

actions!(pod_orch, [NewGroup, OpenGates, OpenAgentTree]);

/// App-wide keys: Ctrl+Shift+N new group, Ctrl+Shift+I gates inbox, Ctrl+Shift+E agent tree.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-n", NewGroup, None),
        KeyBinding::new("ctrl-shift-i", OpenGates, None),
        KeyBinding::new("ctrl-shift-e", OpenAgentTree, None),
    ]);
}

/// Current persisted or system appearance.
pub use crate::theme::mode;

/// Current design tokens.
pub fn tokens() -> pod_ui::Tokens {
    pod_ui::Tokens::for_mode(mode())
}

/// Color from a design token: the layout kit's [`color`](crate::layouts::kit::color).
pub fn tc(hex: u32) -> Hsla {
    crate::layouts::kit::color(hex)
}

/// Orchestration state owned by the workspace.
#[derive(Default)]
pub struct OrchState {
    /// Every gate seen this session (pending and resolved), by id.
    pub gates: BTreeMap<String, GateInfo>,
    /// A `status.subscribe` snapshot arrived at least once.
    pub gates_synced: bool,
    pub groups: Vec<WeakEntity<GroupPanel>>,
    pub gates_panel: Option<WeakEntity<GatesPanel>>,
    pub tree_panel: Option<WeakEntity<AgentTreePanel>>,
}

impl OrchState {
    pub fn has_panels(&self) -> bool {
        !self.live_groups().is_empty()
            || self
                .gates_panel
                .as_ref()
                .is_some_and(|w| w.upgrade().is_some())
            || self
                .tree_panel
                .as_ref()
                .is_some_and(|w| w.upgrade().is_some())
    }

    pub fn live_groups(&self) -> Vec<Entity<GroupPanel>> {
        self.groups.iter().filter_map(|w| w.upgrade()).collect()
    }

    /// Pending gates, oldest first.
    pub fn pending(&self) -> Vec<GateInfo> {
        let mut v: Vec<GateInfo> = self
            .gates
            .values()
            .filter(|g| g.state == GateState::Pending)
            .cloned()
            .collect();
        v.sort_by_key(|g| (g.created_ms, gate_number(&g.id)));
        v
    }

    /// Resolved gates, newest first.
    pub fn resolved(&self) -> Vec<GateInfo> {
        let mut v: Vec<GateInfo> = self
            .gates
            .values()
            .filter(|g| g.state != GateState::Pending)
            .cloned()
            .collect();
        v.sort_by_key(|g| {
            std::cmp::Reverse((g.resolved_ms.unwrap_or(g.created_ms), gate_number(&g.id)))
        });
        v
    }

    /// Merge a `status.subscribe` snapshot (the pending gates right now).
    /// Returns the ids that are new to us. Gates we knew as pending that are
    /// missing from the snapshot were resolved while we were away: they are
    /// dropped here and come back with their outcome from `gate.list`.
    pub fn merge_snapshot(&mut self, pending: Vec<GateInfo>) -> Vec<String> {
        let ids: std::collections::HashSet<&str> = pending.iter().map(|g| g.id.as_str()).collect();
        self.gates
            .retain(|id, g| g.state != GateState::Pending || ids.contains(id.as_str()));
        let mut new = Vec::new();
        for g in pending {
            if !self.gates.contains_key(&g.id) {
                new.push(g.id.clone());
            }
            self.gates.insert(g.id.clone(), g);
        }
        self.gates_synced = true;
        new
    }

    /// Apply a `gate.changed` (or a resolve reply). Returns the previous record.
    pub fn apply(&mut self, g: GateInfo) -> Option<GateInfo> {
        self.gates.insert(g.id.clone(), g)
    }
}

/// `g12` -> 12 (ids sort numerically, not as strings).
fn gate_number(id: &str) -> u64 {
    id.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0)
}

/// Where a gate came from, for display: `task demo/fx · terminal t3`.
pub fn gate_origin(g: &GateInfo) -> String {
    let mut parts = Vec::new();
    if let Some(t) = &g.task {
        parts.push(crate::tr!(TASK_T; t;));
    }
    if let Some(gr) = &g.group {
        parts.push(crate::tr!(GROUP_GR; gr;));
    }
    if let Some(t) = &g.terminal {
        parts.push(crate::tr!(TERMINAL_T; t;));
    }
    if parts.is_empty() {
        crate::strings::NO_ORIGIN_RECORDED.into()
    } else {
        parts.join(" · ")
    }
}

// ───────────────────────────── status → pod-ui ─────────────────────────────

/// pod-ui badge for a terminal: its live status, or its lifecycle when it has none.
pub fn terminal_badge(t: &TerminalVm) -> StatusBadge {
    match t.live {
        Liveness::Running => status_badge(t.status.as_ref()),
        Liveness::Exited { code, .. } => {
            let mut b = StatusBadge::fixture(S::Exited);
            b.exit_code = code;
            b.source = None;
            b.confidence = 0.;
            b
        }
        Liveness::Lost => {
            let mut b = StatusBadge::fixture(S::Exited);
            b.exit_code = None;
            b.source = None;
            b.confidence = 0.;
            b
        }
    }
}

/// pod-ui badge for a podd status (unknown when absent).
pub fn status_badge(s: Option<&StatusVm>) -> StatusBadge {
    let Some(s) = s else {
        let mut b = StatusBadge::fixture(S::Unknown);
        b.source = None;
        b.confidence = 0.;
        b.exit_code = None;
        return b;
    };
    let state = domain_state(s);
    let mut b = StatusBadge::fixture(state);
    b.exit_code = None;
    b.source = match s.source.as_deref() {
        Some("hook") => Some(Source::Hook),
        Some("osc") => Some(Source::Osc),
        Some("heuristic") => Some(Source::Heuristic),
        Some("mod") => Some(Source::Mod),
        Some("process") => Some(Source::Process),
        _ => None,
    };
    b.confidence = s.confidence;
    b.final_state = s
        .final_state
        .map(|state| domain_state(&StatusVm::of(state)));
    b.running_children = s.running_children;
    b
}

/// podd's state label (+ blocked kind) as the status engine's state.
pub fn domain_state(s: &StatusVm) -> S {
    match s.state {
        AgentState::Idle => S::Idle,
        AgentState::Working => S::Working,
        AgentState::Blocked => S::Blocked(match s.kind.as_deref() {
            Some("permission") => BlockedKind::Permission,
            Some("question") => BlockedKind::Question,
            Some("auth") => BlockedKind::Auth,
            _ => BlockedKind::Other,
        }),
        AgentState::Done => S::Done,
        AgentState::Error => S::Error,
        AgentState::Unknown => S::Unknown,
        AgentState::Exited => S::Exited,
    }
}

/// Summary over terminal ids (each counted once; unknown ids are skipped).
pub fn summary_of<'a>(
    ids: impl IntoIterator<Item = &'a String>,
    terminals: &[TerminalVm],
) -> StatusSummary {
    let badges: Vec<(String, StatusBadge)> = ids
        .into_iter()
        .filter_map(|id| terminals.iter().find(|t| &t.id == id))
        .map(|t| (t.id.clone(), terminal_badge(t)))
        .collect();
    StatusSummary::from_terminals(badges.iter().map(|(id, b)| (id.as_str(), b)))
}

/// One badge standing for a summary: its priority state (`None` = mixed done/exited).
pub fn summary_badge(s: &StatusSummary) -> Option<StatusBadge> {
    s.priority().map(|state| {
        let mut b = StatusBadge::fixture(state);
        b.source = None;
        b.confidence = 1.;
        b.exit_code = s.exit_code;
        b
    })
}

/// Short named counts (`작업 2 · 응답 1`), only the non-zero ones (docs/10 §4.4).
pub fn summary_short(s: &StatusSummary) -> String {
    let parts: Vec<String> = [
        (crate::strings::COUNT_WORKING, s.working),
        (crate::strings::COUNT_BLOCKED, s.blocked),
        (crate::strings::ERROR, s.error),
        (crate::strings::COUNT_UNKNOWN, s.unknown),
        (crate::strings::IDLE, s.idle),
        (crate::strings::DONE, s.done),
        (crate::strings::KILL, s.exited),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .map(|(l, n)| format!("{l} {n}"))
    .collect();
    if parts.is_empty() {
        crate::strings::NO_TERMINAL.into()
    } else {
        parts.join(" · ")
    }
}

// ───────────────────────────── workspace glue ─────────────────────────────

impl Workspace {
    /// Orchestration events of the status stream.
    pub(crate) fn on_orch_event(
        &mut self,
        ev: SourceEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            SourceEvent::Gates(pending) => {
                let first = !self.orch.gates_synced;
                let new = self.orch.merge_snapshot(pending);
                if first && !new.is_empty() {
                    self.raise_toast(
                        ToastKind::Warning,
                        crate::tr!(GATE_S_WAITING_FOR_APPROVAL; ; new.len()),
                        crate::strings::OPEN_THE_GATES_INBOX_STATUS_BAR_CTRL_SHIFT_I,
                        cx,
                    );
                } else {
                    for id in new {
                        if let Some(g) = self.orch.gates.get(&id).cloned() {
                            self.gate_requested(&g, cx);
                        }
                    }
                }
            }
            SourceEvent::Gate(g) => {
                let prev = self.orch.apply((*g).clone());
                match (prev.as_ref().map(|p| p.state), g.state) {
                    (None, GateState::Pending) => self.gate_requested(&g, cx),
                    (Some(GateState::Pending) | None, s) if s != GateState::Pending => {
                        let kind = if s == GateState::Approved {
                            ToastKind::Success
                        } else {
                            ToastKind::Info
                        };
                        let note = g
                            .note
                            .as_deref()
                            .map(|n| format!(" · {n}"))
                            .unwrap_or_default();
                        self.raise_toast(
                            kind,
                            crate::tr!(GATE; ; g.id, gate_state_label(s)),
                            format!("{}{note}", g.title),
                            cx,
                        );
                    }
                    _ => {}
                }
            }
            SourceEvent::GroupCheck(e) => {
                for p in self.orch.live_groups() {
                    if p.read(cx).group_id == e.group {
                        p.update(cx, |p, cx| p.check_event(&e, cx));
                    }
                }
                match e.job.state {
                    // The job's end is its own event (no member).
                    CheckJobState::Finished if e.member.is_none() => {
                        if e.job.all_passed == Some(true) {
                            self.raise_toast(
                                ToastKind::Success,
                                crate::tr!(CHECK_PASSED; ; e.group),
                                e.job.argv.join(" "),
                                cx,
                            );
                        } else {
                            self.raise_toast(
                                ToastKind::Error,
                                crate::tr!(CHECK_FAILED; ; e.group),
                                e.job.argv.join(" "),
                                cx,
                            );
                        }
                    }
                    CheckJobState::Cancelled if e.member.is_none() => {
                        self.raise_toast(
                            ToastKind::Info,
                            crate::tr!(CHECK_CANCELLED; ; e.group),
                            e.job.argv.join(" "),
                            cx,
                        );
                    }
                    _ => {}
                }
            }
            SourceEvent::Delivery(d, submit) => {
                for r in self.live_reviews() {
                    if r.read(cx).task_id == d.task {
                        r.update(cx, |r, cx| r.delivery_event(&d, submit.as_ref(), cx));
                    }
                }
            }
            _ => {}
        }
        if let Some(p) = self.orch.gates_panel.as_ref().and_then(|w| w.upgrade()) {
            p.update(cx, |_, cx| cx.notify());
        }
        let _ = window;
        cx.notify();
    }

    /// A new pending gate: toast (and beep, like a blocked agent).
    fn gate_requested(&mut self, g: &GateInfo, cx: &mut Context<Self>) {
        let expiry = g.expires_ms.map(|e| crate::tr!(EXPIRES_IN_325; ; crate::gates::duration_text(e.saturating_sub(now_ms())))).unwrap_or_default();
        self.raise_toast(
            ToastKind::Warning,
            crate::tr!(APPROVAL_REQUESTED; ; g.title),
            crate::tr!(EXPIRY_INBOX; expiry; g.message, gate_origin(g)),
            cx,
        );
        if self.options.sound {
            // SAFETY: plain Win32 call without pointers.
            unsafe {
                windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(
                    windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
                );
            }
        }
    }

    /// Tasks, terminals or statuses changed: refresh the orchestration panels.
    pub(crate) fn orch_data_changed(&mut self, cx: &mut Context<Self>) {
        for p in self.orch.live_groups() {
            p.update(cx, |p, cx| p.data_changed(&self.tasks, cx));
        }
        if let Some(p) = self.orch.tree_panel.as_ref().and_then(|w| w.upgrade()) {
            p.update(cx, |_, cx| cx.notify());
        }
    }

    /// Show the Group panel of `group` (focus the open one, else open it).
    pub fn open_group(
        &mut self,
        group: &str,
        info: Option<GroupInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self
            .orch
            .live_groups()
            .into_iter()
            .find(|p| p.read(cx).group_id == group);
        let panel = match existing {
            Some(p) => {
                let pid = PanelId::from(p.entity_id());
                self.dock
                    .update(cx, |a, cx| a.select_panel(pid, window, cx));
                p
            }
            None => {
                let (source, ws) = (self.source.clone(), cx.entity().downgrade());
                let panel =
                    cx.new(|cx| GroupPanel::new(source, ws, group.to_string(), info, window, cx));
                self.orch.groups.push(panel.downgrade());
                self.dock.update(cx, |a, cx| {
                    a.add_panel_view(
                        panel_handle(panel.clone()),
                        DockPlacement::Center,
                        None,
                        window,
                        cx,
                    )
                });
                panel
            }
        };
        let focus = panel.read(cx).focus_handle();
        window.focus(&focus, cx);
        self.touch_all(cx);
    }

    /// Show the gates inbox (right dock).
    pub fn open_gates(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.caps.orch {
            self.raise_toast(
                ToastKind::Warning,
                crate::strings::GATES_NEED_A_NEWER_PODD,
                crate::strings::PODD_PROTOCOL_V6_IS_REQUIRED,
                cx,
            );
            return;
        }
        if let Some(p) = self.orch.gates_panel.as_ref().and_then(|w| w.upgrade()) {
            let pid = PanelId::from(p.entity_id());
            self.dock
                .update(cx, |a, cx| a.select_panel(pid, window, cx));
            p.update(cx, |p, cx| p.reload(cx));
            // Keyboard users land in the inbox they asked for.
            p.read(cx).focus_handle(cx).focus(window, cx);
            return;
        }
        let (source, ws) = (self.source.clone(), cx.entity().downgrade());
        let panel = cx.new(|cx| GatesPanel::new(source, ws, window, cx));
        self.orch.gates_panel = Some(panel.downgrade());
        let focus = panel.read(cx).focus_handle(cx);
        self.dock.update(cx, |a, cx| {
            a.add_panel_view(
                panel_handle(panel),
                DockPlacement::Right,
                Some(px(380.)),
                window,
                cx,
            )
        });
        focus.focus(window, cx);
        self.touch_all(cx);
    }

    /// Show the agent tree (right dock).
    pub fn open_agent_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.orch.tree_panel.as_ref().and_then(|w| w.upgrade()) {
            let pid = PanelId::from(p.entity_id());
            self.dock
                .update(cx, |a, cx| a.select_panel(pid, window, cx));
            p.read(cx).focus_handle(cx).focus(window, cx);
            return;
        }
        let ws = cx.entity().downgrade();
        let panel = cx.new(|cx| AgentTreePanel::new(ws, cx));
        panel.update(cx, |p, cx| p.forward_focus(window, cx));
        self.orch.tree_panel = Some(panel.downgrade());
        let focus = panel.read(cx).focus_handle(cx);
        self.dock.update(cx, |a, cx| {
            a.add_panel_view(
                panel_handle(panel),
                DockPlacement::Right,
                Some(px(380.)),
                window,
                cx,
            )
        });
        focus.focus(window, cx);
        self.touch_all(cx);
    }

    /// An orchestration panel was closed.
    pub(crate) fn orch_panel_removed(&mut self, cx: &mut Context<Self>) {
        self.orch.groups.retain(|w| w.upgrade().is_some());
        cx.notify();
    }

    fn touch_all(&mut self, cx: &mut Context<Self>) {
        cx.notify();
    }
}

pub fn gate_state_label(s: GateState) -> &'static str {
    match s {
        GateState::Pending => crate::strings::IDLE,
        GateState::Approved => crate::strings::APPROVED,
        GateState::Rejected => crate::strings::REJECTED,
        GateState::Expired => crate::strings::EXPIRED,
    }
}

/// Now in epoch milliseconds: the layout kit's system [`Clock`](crate::layouts::kit::Clock).
pub fn now_ms() -> u64 {
    crate::layouts::kit::Clock::System.now_ms()
}

/// Status bar item: pending gate count, opens the inbox.
pub fn status_bar_gates(ws: &Workspace, cx: &mut Context<Workspace>) -> impl IntoElement {
    let n = ws.orch.pending().len();
    let t = tokens();
    let label = if n == 0 {
        crate::strings::NO_GATES.to_string()
    } else {
        crate::tr!(N_GATE_PENDING; n; "개")
    };
    div()
        .id("status-gates")
        .px_1()
        .rounded_sm()
        .cursor_pointer()
        .text_color(tc(if n > 0 { t.warning_fg } else { t.text_muted }))
        .when(n > 0, |d| d.bg(tc(t.warning_bg)))
        .hover(|s| s.bg(tc(t.bg_hover)))
        .child(label)
        .on_click(cx.listener(|w, _, window, cx| w.open_gates(window, cx)))
}

use gpui_kit::prelude::FluentBuilder as _;

#[cfg(test)]
mod tests {
    use super::{OrchState, S, gate_origin, status_badge, summary_badge, summary_short};
    use crate::vm::{AgentState, StatusVm};
    use pod_status::BlockedKind;
    use pod_ui::StatusSummary;
    use podd::protocol::{GateInfo, GateState};

    fn gate(id: &str, state: GateState, created: u64) -> GateInfo {
        GateInfo {
            id: id.into(),
            title: "Approval needed".into(),
            message: "deploy?".into(),
            task: Some("demo/a".into()),
            group: None,
            terminal: Some("t3".into()),
            state,
            created_ms: created,
            resolved_ms: None,
            note: None,
            expires_ms: None,
            run: None,
            item: None,
            options: Vec::new(),
            choice: None,
            requested_by: None,
            decided_by: None,
        }
    }

    #[test]
    fn snapshot_merge_reports_new_and_drops_vanished_pending() {
        let mut o = OrchState::default();
        assert_eq!(
            o.merge_snapshot(vec![
                gate("g1", GateState::Pending, 1),
                gate("g2", GateState::Pending, 2)
            ]),
            ["g1", "g2"]
        );
        o.apply(gate("g0", GateState::Approved, 0));
        // g1 was resolved while disconnected; g3 is new.
        let new = o.merge_snapshot(vec![
            gate("g2", GateState::Pending, 2),
            gate("g3", GateState::Pending, 3),
        ]);
        assert_eq!(new, ["g3"]);
        assert!(!o.gates.contains_key("g1"));
        assert!(o.gates.contains_key("g0"), "resolved history is kept");
        assert_eq!(
            o.pending()
                .iter()
                .map(|g| g.id.as_str())
                .collect::<Vec<_>>(),
            ["g2", "g3"]
        );
    }

    #[test]
    fn pending_sorts_numerically_and_resolved_newest_first() {
        let mut o = OrchState::default();
        o.apply(gate("g10", GateState::Pending, 5));
        o.apply(gate("g9", GateState::Pending, 5));
        let mut a = gate("g1", GateState::Rejected, 1);
        a.resolved_ms = Some(10);
        let mut b = gate("g2", GateState::Expired, 2);
        b.resolved_ms = Some(20);
        o.apply(a);
        o.apply(b);
        assert_eq!(
            o.pending()
                .iter()
                .map(|g| g.id.as_str())
                .collect::<Vec<_>>(),
            ["g9", "g10"]
        );
        assert_eq!(
            o.resolved()
                .iter()
                .map(|g| g.id.as_str())
                .collect::<Vec<_>>(),
            ["g2", "g1"]
        );
        assert_eq!(
            gate_origin(&gate("g1", GateState::Pending, 0)),
            "작업 demo/a · 터미널 t3"
        );
    }

    #[test]
    fn badges_follow_status_and_lifecycle() {
        let mut s = StatusVm::of(AgentState::Blocked);
        s.kind = Some("question".into());
        s.source = Some("hook".into());
        s.confidence = 0.95;
        let b = status_badge(Some(&s));
        assert_eq!(b.state, S::Blocked(BlockedKind::Question));
        assert!(!b.label().contains(crate::strings::INFERRED));
        s.confidence = 0.5;
        assert!(
            status_badge(Some(&s))
                .label()
                .contains(crate::strings::INFERRED),
            "source must not invent confidence"
        );
        s.source = Some("heuristic".into());
        assert!(
            status_badge(Some(&s))
                .label()
                .contains(crate::strings::INFERRED)
        );
        assert_eq!(status_badge(None).state, S::Unknown);
        let summary = StatusSummary {
            total: 2,
            working: 1,
            done: 1,
            ..Default::default()
        };
        assert_eq!(summary_short(&summary), crate::strings::SUMMARY_SAMPLE);
        assert_eq!(summary_badge(&summary).unwrap().state, S::Working);
    }
}
