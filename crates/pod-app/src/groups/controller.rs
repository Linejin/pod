//! Group requests and their replies, in the order they were sent before the
//! split: `group.create` (with `repo.add` for a new path), `group.show`,
//! `group.compare` (one at a time, again once when asked meanwhile, and 800 ms
//! after a member's git summary changed), `group.check` /
//! `group.check_cancel` and the `group.check` events, `group.pick` with its
//! toasts and the force confirmation, and the card buttons' hand-offs to the
//! workspace (terminal, review, PR), deferred as before.

use std::time::Duration;

use gpui_kit::*;
use podd::protocol::{self as p, CheckJobState};

use super::model::{
    PickOutcome, apply_check_event, changed_check_text, check_job_ended, check_params,
    check_running, member_ids, pick_toast_body, removed_terminals,
};
use super::{GroupPanel, error_text, git_signature};
use crate::vm::TaskVm;
use crate::workspace::{ToastKind, Workspace};

/// Coalesce compare triggers (task poll + several members ending turns).
const COMPARE_DEBOUNCE: Duration = Duration::from_millis(800);

impl Workspace {
    /// `group.create` (after `repo.add` when the dialog named a new path),
    /// then the new group's panel.
    pub(super) fn create_group(
        &mut self,
        add_repo: Option<String>,
        mut params: p::GroupCreateParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.clone();
        let n = params.agents.len();
        self.raise_toast(
            ToastKind::Info,
            crate::tr!(CREATING_GROUP; ; params.name),
            crate::tr!(N_MEMBER_TASK_S_WORKTREE_AGENT_EACH; n;),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if let Some(path) = add_repo {
                match source.add_repo(&path, None).await {
                    Ok(Ok(r)) => params.repo = r.name,
                    Ok(Err(e)) => {
                        let _ = this.update(cx, |w, cx| {
                            w.raise_toast(
                                ToastKind::Error,
                                crate::strings::COULD_NOT_ADD_REPOSITORY,
                                e.message,
                                cx,
                            )
                        });
                        return;
                    }
                    Err(_) => return,
                }
            }
            let title = format!("{}/{}", params.repo, params.name);
            let result = source.call::<p::GroupCreate>(&params).await;
            let _ = this.update_in(cx, |w, window, cx| match result {
                Ok(info) => {
                    // Members are new tasks: re-read the task list now, not in up to 3 s.
                    w.source.refresh_tasks();
                    w.raise_toast(
                        ToastKind::Success,
                        crate::tr!(GROUP_CREATED; ; info.id),
                        crate::tr!(MEMBERS_FROM; ; info.members.len(), info.base),
                        cx,
                    );
                    let id = info.id.clone();
                    w.open_group(&id, Some(info), window, cx);
                }
                Err(e) => w.raise_toast(
                    ToastKind::Error,
                    crate::tr!(COULD_NOT_CREATE_GROUP_TITLE; title;),
                    error_text(&e),
                    cx,
                ),
            });
        })
        .detach();
    }
}

impl GroupPanel {
    fn toast(&self, kind: ToastKind, title: String, body: String, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        cx.defer(move |cx| {
            let _ = ws.update(cx, |w, cx| w.raise_toast(kind, title, body, cx));
        });
    }

    // ---- data ------------------------------------------------------------

    pub(super) fn reload_show(&mut self, cx: &mut Context<Self>) {
        let rx = self.source.call::<p::GroupShow>(&p::GroupRef {
            group: self.group_id.clone(),
        });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                match r {
                    Ok(info) => {
                        if let Some(text) = changed_check_text(p.info.as_ref(), &info) {
                            p.pending_check_text = Some(text);
                        }
                        p.check_busy = check_running(&info);
                        p.info = Some(info);
                        p.show_error = None;
                    }
                    Err(e) => p.show_error = Some(error_text(&e)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn reload_compare(&mut self, cx: &mut Context<Self>) {
        if self.compare_loading {
            self.compare_again = true;
            return;
        }
        self.compare_loading = true;
        self.compare_sig = self.latest_sig.clone();
        let rx = self.source.call::<p::GroupCompare>(&p::GroupRef {
            group: self.group_id.clone(),
        });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                p.compare_loading = false;
                match r {
                    Ok(c) => {
                        p.compare = Some(c);
                        p.compare_error = None;
                    }
                    Err(e) => p.compare_error = Some(error_text(&e)),
                }
                if std::mem::take(&mut p.compare_again) {
                    p.reload_compare(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// ⟳ Compare: the group and the comparison again.
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reload_show(cx);
        self.reload_compare(cx);
    }

    /// The workspace's tasks / terminals / statuses changed (called while the
    /// workspace is being updated: must not read it back, hence `tasks`).
    /// A member's git summary changing means its diff changed: compare again.
    pub fn data_changed(&mut self, tasks: &[TaskVm], cx: &mut Context<Self>) {
        let sig = git_signature(&member_ids(self.info.as_ref()), tasks);
        self.latest_sig = sig.clone();
        if self.compare_sig.is_empty() {
            // The first compare ran before the workspace told us anything.
            self.compare_sig = sig;
        } else if self.info.is_some() && sig != self.compare_sig && self.compare_timer.is_none() {
            self.compare_timer = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(COMPARE_DEBOUNCE).await;
                let _ = this.update(cx, |p, cx| {
                    p.compare_timer = None;
                    p.reload_compare(cx);
                });
            }));
        }
        cx.notify();
    }

    // ---- check -------------------------------------------------------------

    /// `group.check` progress from the status stream.
    pub fn check_event(&mut self, e: &p::GroupCheckEvent, cx: &mut Context<Self>) {
        apply_check_event(self.info.as_mut(), self.compare.as_mut(), e);
        self.check_busy = e.job.state == CheckJobState::Running;
        if check_job_ended(e) {
            self.reload_show(cx);
        }
        cx.notify();
    }

    pub(super) fn run_check(&mut self, cx: &mut Context<Self>) {
        let line = self.check_input.read(cx).value().to_string();
        let stored = self.info.as_ref().and_then(|g| g.check.clone());
        let Some(params) = check_params(&self.group_id, &line, stored.as_ref()) else {
            self.check_error = Some(crate::strings::ENTER_A_CHECK_COMMAND_FIRST.into());
            cx.notify();
            return;
        };
        self.check_busy = true;
        self.check_error = None;
        let rx = self.source.call::<p::GroupCheck>(&params);
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                match r {
                    Ok(res) => {
                        if let (Some(info), Some(job)) = (p.info.as_mut(), res.job.clone()) {
                            if info.check.as_ref() != Some(&job.argv) {
                                info.check = Some(job.argv.clone());
                            }
                            p.check_busy = job.state == CheckJobState::Running;
                            info.check_job = Some(job);
                        }
                    }
                    Err(e) => {
                        p.check_busy = false;
                        p.check_error = Some(error_text(&e));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn cancel_check(&mut self, cx: &mut Context<Self>) {
        let rx = self.source.call::<p::GroupCheckCancel>(&p::GroupRef {
            group: self.group_id.clone(),
        });
        cx.spawn(async move |this, cx| {
            let r = rx.await;
            let _ = this.update(cx, |p, cx| {
                if let Err(e) = r {
                    p.check_error = Some(error_text(&e));
                }
                p.reload_show(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Show or hide a member's check output.
    pub(super) fn toggle_output(&mut self, task: &str, cx: &mut Context<Self>) {
        if !self.expanded.remove(task) {
            self.expanded.insert(task.to_string());
        }
        cx.notify();
    }

    // ---- pick --------------------------------------------------------------

    pub(super) fn pick(
        &mut self,
        task: String,
        remove_others: bool,
        force: bool,
        delete_branch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let params = p::GroupPickParams {
            group: self.group_id.clone(),
            task: task.clone(),
            remove_others,
            force,
            delete_branch,
        };
        let rx = self.source.call::<p::GroupPick>(&params);
        cx.spawn_in(window, async move |this, cx| {
            let r = rx.await;
            let _ = this.update_in(cx, |p, window, cx| {
                match r {
                    Ok(res) => {
                        let closed = removed_terminals(&res);
                        if let Some(ws) = p.workspace.upgrade() {
                            ws.update(cx, |w, cx| {
                                for t in &closed {
                                    w.close_panels_of(t, window, cx);
                                }
                            });
                        }
                        p.toast(
                            ToastKind::Success,
                            crate::tr!(WINNER; ; res.picked),
                            pick_toast_body(&res),
                            cx,
                        );
                        if !res.kept.is_empty() {
                            p.confirm_force(
                                task.clone(),
                                res.kept.clone(),
                                delete_branch,
                                window,
                                cx,
                            );
                        }
                        p.pick = Some(PickOutcome::Done(res));
                    }
                    Err(e) => {
                        let msg = error_text(&e);
                        p.toast(
                            ToastKind::Error,
                            crate::tr!(COULD_NOT_PICK_TASK; task;),
                            msg.clone(),
                            cx,
                        );
                        p.pick = Some(PickOutcome::Failed(msg));
                    }
                }
                p.reload_show(cx);
                p.reload_compare(cx);
                cx.notify();
            });
        })
        .detach();
    }

    // ---- hand-offs to the workspace ------------------------------------------

    pub(super) fn open_terminal(&self, task: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspace.upgrade() else {
            return;
        };
        let task = task.to_string();
        cx.defer_in(window, move |_, window, cx| {
            ws.update(cx, |w, cx| {
                match w.task(&task).and_then(|t| t.terminals.first().cloned()) {
                    Some(t) => w.open_terminal(&t, false, window, cx),
                    None => w.raise_toast(
                        ToastKind::Warning,
                        crate::strings::NO_TERMINAL,
                        crate::tr!(TASK_HAS_NO_TERMINAL_REMOVED; task;),
                        cx,
                    ),
                }
            });
        });
    }

    pub(super) fn open_review(&self, task: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws) = self.workspace.upgrade() else {
            return;
        };
        let task = task.to_string();
        cx.defer_in(window, move |_, window, cx| {
            ws.update(cx, |w, cx| w.open_review(&task, window, cx))
        });
    }

    /// P6: the winner goes on to a pull request (docs/15).
    pub(super) fn open_winner_pr(&self, task: String, window: &mut Window, cx: &mut Context<Self>) {
        let ws = self.workspace.clone();
        cx.defer_in(window, move |_, window, cx| {
            let _ = ws.update(cx, |w, cx| w.open_pr(&task, true, window, cx));
        });
    }
}
