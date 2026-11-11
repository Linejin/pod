//! Source binding: the event pump from the podd source and what each
//! [`SourceEvent`] changes, in arrival order (terminal list, agent status,
//! the toasts and acks they raise).

use std::collections::HashMap;
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::*;

use crate::layout::label_key;
use crate::source::SourceEvent;
use crate::vm::{AgentState, ConnState, Liveness, StatusVm, TerminalVm};

use super::Workspace;
use super::notifications::ToastKind;

fn request_age(since: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    relative_age(now.saturating_sub(since) / 1000)
}

fn relative_age(seconds: u64) -> String {
    match seconds {
        0..60 => "방금".into(),
        60..3600 => format!("{}분 전", seconds / 60),
        3600..86400 => format!("{}시간 전", seconds / 3600),
        _ => format!("{}일 전", seconds / 86400),
    }
}

impl Workspace {
    // ---- data source ----------------------------------------------------

    /// The event pump [`Workspace::new`] spawns: each source event reaches
    /// [`Self::on_source_event`] in arrival order until the workspace is gone.
    pub(super) fn pump(
        events: UnboundedReceiver<SourceEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            let mut events = events;
            while let Some(ev) = events.next().await {
                if this
                    .update_in(cx, |w, window, cx| w.on_source_event(ev, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    fn on_source_event(&mut self, ev: SourceEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            SourceEvent::Conn(state) => {
                let was = self.conn.clone();
                match (&was, &state) {
                    (ConnState::Connected { .. }, ConnState::Reconnecting { error, .. }) => {
                        self.raise_toast(
                            ToastKind::Warning,
                            crate::strings::PODD_DISCONNECTED,
                            crate::tr!(RECONNECTING_ERROR; error;),
                            cx,
                        );
                    }
                    (ConnState::Reconnecting { .. }, ConnState::Connected { pid, .. }) => {
                        self.raise_toast(
                            ToastKind::Success,
                            crate::strings::PODD_RECONNECTED,
                            crate::tr!(DAEMON_PID_PID_PANELS_RE_ATTACH; pid;),
                            cx,
                        );
                    }
                    _ => {}
                }
                self.conn = state;
            }
            SourceEvent::Caps(caps) => {
                self.caps = caps;
                crate::mods_ui::refresh(self, cx);
            }
            SourceEvent::ModNotice(n) => crate::mods_ui::on_notice(self, *n, window, cx),
            SourceEvent::ModChanged(m) => crate::mods_ui::on_changed(*m, cx),
            SourceEvent::Terminals(list) => self.on_terminals(list, window, cx),
            SourceEvent::Tasks(tasks) => {
                self.tasks = tasks;
                self.have_tasks = true;
                if !self.restored && self.have_list {
                    self.restore(window, cx);
                }
                for r in self.live_reviews() {
                    let id = r.read(cx).task_id.clone();
                    if let Some(t) = self.task(&id).cloned() {
                        r.update(cx, |r, cx| r.set_task(&t, cx));
                    }
                }
                if self.restored {
                    let gone: Vec<String> = self
                        .live_editors()
                        .iter()
                        .map(|e| e.read(cx).task_id.clone())
                        .filter(|id| self.task(id).is_none())
                        .collect();
                    for id in gone {
                        self.close_editor_of(&id, window, cx);
                    }
                }
                if let Some(id) = self.options.review.take() {
                    self.open_review(&id, window, cx);
                }
                if std::mem::take(&mut self.options.settings) {
                    crate::mods_ui::open_page(self, window, cx);
                }
            }
            SourceEvent::Repos(repos) => self.repos = repos,
            SourceEvent::TaskFilesChanged(id) => {
                for r in self.live_reviews() {
                    if r.read(cx).task_id == id {
                        r.update(cx, |r, cx| r.refresh_diff(cx));
                    }
                }
            }
            SourceEvent::TaskRemoving { id, worktree } => {
                self.task_removing(&id, &worktree, window, cx)
            }
            SourceEvent::TaskCreated { terminal, hint, .. } => {
                crate::auto_open::on_task_created(self, terminal, hint, window, cx)
            }
            SourceEvent::Gates(_)
            | SourceEvent::Gate(_)
            | SourceEvent::GroupCheck(_)
            | SourceEvent::Delivery(..) => {
                self.on_orch_event(ev, window, cx);
            }
            SourceEvent::OrchChanged(_) | SourceEvent::OrchMail(_) | SourceEvent::OrchResync => {
                crate::orch_ui::on_event(self, ev, window, cx)
            }
            SourceEvent::Status { id, status } => {
                if let Some(t) = self.terminals.iter_mut().find(|t| t.id == id) {
                    let old = std::mem::replace(&mut t.status, status.clone());
                    if let Some(new) = status {
                        self.status_changed(&id, old.as_ref(), &new, window, cx);
                        for r in self.live_reviews() {
                            r.update(cx, |r, cx| r.agent_status(&id, &new, cx));
                        }
                    }
                }
            }
        }
        self.orch_data_changed(cx);
        self.refresh_link_roots(cx);
        self.touch(cx);
    }

    fn on_terminals(&mut self, list: Vec<TerminalVm>, window: &mut Window, cx: &mut Context<Self>) {
        if self.have_list {
            let before: HashMap<&str, Liveness> = self
                .terminals
                .iter()
                .map(|t| (t.id.as_str(), t.live))
                .collect();
            let exits: Vec<(String, String)> = list
                .iter()
                .filter(|t| {
                    !t.is_running() && before.get(t.id.as_str()) == Some(&Liveness::Running)
                })
                .map(|t| {
                    (
                        self.label_of(t),
                        format!("{} ({}) {}", t.id, t.program, t.live_label()),
                    )
                })
                .collect();
            for (title, body) in exits {
                self.terminal_exited(&title, &body, cx);
            }
            // Status changes the subscription stream did not deliver (e.g. while reconnecting).
            let old: HashMap<String, Option<StatusVm>> = self
                .terminals
                .iter()
                .map(|t| (t.id.clone(), t.status.clone()))
                .collect();
            for t in &list {
                if let (Some(prev), Some(new)) = (old.get(&t.id), &t.status) {
                    self.status_changed(&t.id, prev.as_ref(), new, window, cx);
                }
            }
        }
        self.toasts.retain(|toast| {
            toast.terminal_request.as_ref().is_none_or(|id| {
                list.iter()
                    .any(|terminal| &terminal.id == id && terminal.is_running())
            })
        });
        self.terminals = list;
        self.have_list = true;
        self.refresh_labels(cx);
        if !self.restored {
            // Review/Group panels need the task list, which the source sends
            // right after the first terminal list. Give it a moment, then
            // restore without it (a daemon whose task.list fails).
            if self.have_tasks || !self.caps.tasks {
                self.restore(window, cx);
            } else if self.restore_timer.is_none() {
                self.restore_timer = Some(cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(3)).await;
                    let _ = this.update_in(cx, |w, window, cx| {
                        if !w.restored {
                            w.restore(window, cx);
                        }
                    });
                }));
            }
        }
    }

    /// A terminal's agent status changed. Turning `blocked` or `done` raises a
    /// toast (blocked also beeps) unless the user is looking at that terminal;
    /// a `done` the user is looking at is acknowledged right away.
    fn status_changed(
        &mut self,
        id: &str,
        old: Option<&StatusVm>,
        new: &StatusVm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A request's lifetime follows its terminal status, including cancellation
        // and exit. Remove resolved requests from both the stack and history.
        if new.state != AgentState::Blocked {
            self.toasts
                .retain(|toast| toast.terminal_request.as_deref() != Some(id));
        }
        let entered = |s: AgentState| {
            new.state == s && old.is_none_or(|o| o.state != s || o.kind != new.kind)
        };
        let blocked = entered(AgentState::Blocked);
        let done = entered(AgentState::Done);
        if !blocked && !done {
            return;
        }
        let viewed = self.terminal_in_view(window, cx);
        if watching(window.is_window_active(), viewed.as_deref(), id) {
            if done {
                self.source.ack(id);
            }
            return;
        }
        let Some(t) = self.terminal(id).cloned() else {
            return;
        };
        let title = self.label_of(&t);
        if blocked {
            let why = new.kind.clone().unwrap_or_else(|| "input".into());
            self.toasts
                .retain(|toast| toast.terminal_request.as_deref() != Some(id));
            let age = request_age(new.since);
            self.raise_toast(
                ToastKind::Warning,
                crate::tr!(TITLE_NEEDS_YOU; title;),
                crate::tr!(BLOCKED_ON_WHY_TURN_AT; why; t.id, new.turn, age),
                cx,
            );
            if let Some(toast) = self.toasts.last_mut() {
                toast.terminal_request = Some(id.to_owned());
            }
            if self.options.sound {
                beep();
            }
        } else {
            self.raise_toast(
                ToastKind::Success,
                crate::tr!(TITLE_IS_DONE; title;),
                crate::tr!(TURN; ; t.id, new.label(), new.turn),
                cx,
            );
        }
    }

    /// The terminal whose panel holds keyboard focus in the last rendered
    /// frame. Unlike `focused_terminal` (the last terminal pane focused, kept
    /// for selection), this is `None` while a Review/Group/Gates/Agents panel,
    /// the sidebar or a dialog has focus, or when the panel is not rendered
    /// (hidden tab, closed dock).
    fn terminal_in_view(&self, window: &Window, cx: &App) -> Option<String> {
        self.live_panels().into_iter().find_map(|p| {
            let p = p.read(cx);
            p.view
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
                .then(|| p.terminal_id.clone())
        })
    }

    /// A terminal pane got keyboard focus: remember it and tell podd the user
    /// looked at it (`terminal.ack`, done -> idle).
    pub fn pane_focused(&mut self, id: &str, cx: &mut Context<Self>) {
        if std::env::var_os("POD_APP_DEBUG").is_some() {
            println!(
                "[pod-app] pane focused: {id} (status caps: {})",
                self.caps.status
            );
        }
        self.focused_terminal = Some(id.to_string());
        if self.caps.status {
            self.source.ack(id);
        }
        cx.notify();
    }

    /// Notification hook for a finished terminal process.
    fn terminal_exited(&mut self, title: &str, body: &str, cx: &mut Context<Self>) {
        self.raise_toast(
            ToastKind::Info,
            crate::tr!(TITLE_EXITED; title;),
            body.to_string(),
            cx,
        );
    }

    pub(crate) fn label_of(&self, t: &TerminalVm) -> String {
        self.saved
            .labels
            .get(&label_key(&t.id, t.created_ms))
            .cloned()
            .or_else(|| t.name.clone())
            .unwrap_or_else(|| t.id.clone())
    }

    pub(super) fn refresh_labels(&mut self, cx: &mut Context<Self>) {
        let map: HashMap<String, String> = self
            .terminals
            .iter()
            .map(|t| (t.id.clone(), self.label_of(t)))
            .collect();
        for p in self.live_panels() {
            let id = p.read(cx).terminal_id.clone();
            if let Some(l) = map.get(&id) {
                let l: SharedString = l.clone().into();
                p.update(cx, |p, cx| p.set_label(l, cx));
            }
            if let Some(t) = self.terminal(&id) {
                let agent = crate::agent_kind::identify_in(t, &self.tasks).kind;
                p.update(cx, |p, cx| p.set_agent(agent, cx));
            }
        }
        *self.labels.borrow_mut() = map;
    }
}

/// Short system sound (the default "asterisk" sound).
/// Whether the user is looking at terminal `id`, so blocked/done needs no toast
/// (E2E B5): the window is active and that terminal's panel holds focus in the
/// rendered frame. `in_view` must come from real focus
/// ([`Workspace::terminal_in_view`]), not the last-focused cache.
pub(super) fn watching(window_active: bool, in_view: Option<&str>, id: &str) -> bool {
    window_active && in_view == Some(id)
}

fn beep() {
    // SAFETY: plain Win32 call without pointers.
    unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::MessageBeep(
            windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
        );
    }
}
