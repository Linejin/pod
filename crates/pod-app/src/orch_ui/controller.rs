//! Requests and answers of the orchestration screens. **Events only**: the
//! status stream's `orch.changed` re-reads that run, `orch.mail` its mail,
//! a (re)subscribe every run; reads of the same run are coalesced (one in
//! flight, at most one more queued). Actions mark themselves busy until
//! podd answers (a double click sends once) and report with a toast;
//! escalations also raise a Windows toast while Pod is in the background.

use std::collections::BTreeMap;

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::*;
use podd::protocol::{self as p, Method as _, OrchSettings, WorkerInfo};
use serde_json::{Value, json};

use super::model::{self, Notice, NoticeKind, RunEntry};
use super::{Page, ko, open_inbox, s, state};
use crate::source::{Answer, SourceError, SourceEvent};
use crate::workspace::{ToastKind, Workspace};

fn err(m: impl Into<String>) -> SourceError {
    SourceError {
        code: None,
        message: m.into(),
    }
}

async fn value(rx: Answer<Value>) -> Result<Value, SourceError> {
    rx.await.map_err(|_| err("podd client stopped"))?
}

fn not_found(e: &SourceError) -> bool {
    e.code == Some(p::codes::METHOD_NOT_FOUND)
}

// ───────────────────────────── events ─────────────────────────────

/// Orchestration events of the status stream (routed by the workspace).
pub(crate) fn on_event(
    ws: &mut Workspace,
    ev: SourceEvent,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    match ev {
        SourceEvent::OrchResync => reload_all(ws, window, cx),
        SourceEvent::OrchChanged(e) => refresh_run(ws, e.run.clone(), window, cx),
        SourceEvent::OrchMail(e) => {
            if let Some(n) = model::notice_for_mail(&e) {
                notify(ws, n, window, cx);
            }
            refresh_mail(ws, e.run.clone(), window, cx);
        }
        _ => {}
    }
}

/// `orch.run.list`, then workers and mail of every run.
pub(crate) fn reload_all(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if std::mem::replace(&mut state(cx).loading, true) {
        return;
    }
    let rx = ws.source.request(p::OrchRunList::NAME, json!({}));
    cx.spawn_in(window, async move |this, cx| {
        let r = value(rx).await;
        let _ = this.update_in(cx, |w, window, cx| {
            let st = state(cx);
            st.loading = false;
            match r {
                Ok(v) => {
                    let raw: Vec<Value> = v
                        .get("runs")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let mut runs = BTreeMap::new();
                    for x in raw {
                        if let Ok(info) = serde_json::from_value::<p::RunInfo>(x.clone()) {
                            let id = info.run.id.clone();
                            let mut e = RunEntry::new(info, model::queues_of(&x));
                            // Keep what we had until the re-reads answer.
                            if let Some(old) = st.runs.get(&id) {
                                e.workers = old.workers.clone();
                                e.mail = old.mail.clone();
                                e.mail_loaded = old.mail_loaded;
                            }
                            runs.insert(id, e);
                        }
                    }
                    st.runs = runs;
                    st.loaded = true;
                    st.unsupported = false;
                    st.error = None;
                    let ids: Vec<String> = st.runs.keys().cloned().collect();
                    for id in ids {
                        refresh_workers(w, id.clone(), window, cx);
                        refresh_mail(w, id, window, cx);
                    }
                    if state(cx).page_open(Page::Cleanup) {
                        refresh_candidates(w, cx);
                    }
                }
                Err(e) if not_found(&e) => {
                    st.unsupported = true;
                    st.loaded = true;
                }
                Err(e) => st.error = Some(e.message),
            }
            cx.notify();
        });
    })
    .detach();
}

/// Re-read one run (`orch.changed`).
pub(crate) fn refresh_run(
    ws: &mut Workspace,
    run: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let st = state(cx);
    if st.run_fetch.contains(&run) {
        st.run_dirty.insert(run);
        return;
    }
    st.run_fetch.insert(run.clone());
    let rx = ws.source.request(p::OrchRunShow::NAME, json!({"run": run}));
    let workers = ws.source.call::<p::OrchWorkerList>(&p::OrchRunRef {
        run: Some(run.clone()),
        caller: Default::default(),
    });
    cx.spawn_in(window, async move |this, cx| {
        let r = value(rx).await;
        let wk = workers.await;
        let _ = this.update_in(cx, |w, window, cx| {
            let st = state(cx);
            st.run_fetch.remove(&run);
            let mut notices = Vec::new();
            match r.and_then(|v| {
                serde_json::from_value::<p::RunInfo>(v.clone())
                    .map(|i| (i, v))
                    .map_err(|e| err(e.to_string()))
            }) {
                Ok((info, raw)) => {
                    let old = st.runs.get(&run);
                    notices = model::notices_for_change(old.map(|o| &o.info), &info);
                    let mut e = RunEntry::new(info, model::queues_of(&raw));
                    if let Some(old) = old {
                        e.workers = old.workers.clone();
                        e.mail = old.mail.clone();
                        e.mail_loaded = old.mail_loaded;
                    }
                    let fresh = !st.runs.contains_key(&run);
                    st.runs.insert(run.clone(), e);
                    if fresh {
                        // A new run: its mail too (no event may follow).
                        refresh_mail(w, run.clone(), window, cx);
                    }
                }
                Err(e) if e.code == Some(p::run_codes::RUN_NOT_FOUND) => {
                    st.runs.remove(&run);
                }
                Err(_) => {}
            }
            if let (Ok(list), Some(e)) = (wk, state(cx).runs.get_mut(&run)) {
                e.workers = list.workers;
            }
            for n in notices {
                notify(w, n, window, cx);
            }
            if state(cx).page_open(Page::Cleanup) {
                refresh_candidates(w, cx);
            }
            if state(cx).run_dirty.remove(&run) {
                refresh_run(w, run.clone(), window, cx);
            }
            cx.notify();
        });
    })
    .detach();
}

fn refresh_workers(
    ws: &mut Workspace,
    run: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let call = ws.source.call::<p::OrchWorkerList>(&p::OrchRunRef {
        run: Some(run.clone()),
        caller: Default::default(),
    });
    cx.spawn_in(window, async move |this, cx| {
        let r = call.await;
        let _ = this.update(cx, |_, cx| {
            if let (Ok(list), Some(e)) = (r, state(cx).runs.get_mut(&run)) {
                e.workers = list.workers;
            }
            cx.notify();
        });
    })
    .detach();
}

/// Re-read a run's mail (`orch.mail`).
pub(crate) fn refresh_mail(
    ws: &mut Workspace,
    run: String,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let st = state(cx);
    if st.mail_unsupported {
        return;
    }
    if st.mail_fetch.contains(&run) {
        st.mail_dirty.insert(run);
        return;
    }
    st.mail_fetch.insert(run.clone());
    let call = ws
        .source
        .call::<p::OrchMailList>(&p::OrchMailListParams { run: run.clone() });
    cx.spawn_in(window, async move |this, cx| {
        let r = call.await;
        let _ = this.update_in(cx, |w, window, cx| {
            let st = state(cx);
            st.mail_fetch.remove(&run);
            match r {
                Ok(list) => {
                    if let Some(e) = st.runs.get_mut(&run) {
                        e.mail = list.messages;
                        e.mail_loaded = true;
                    }
                }
                Err(e) if not_found(&e) => st.mail_unsupported = true,
                Err(_) => {}
            }
            if state(cx).mail_dirty.remove(&run) {
                refresh_mail(w, run.clone(), window, cx);
            }
            cx.notify();
        });
    })
    .detach();
}

/// `task.cleanup_candidates` (S3 tiers) for the cleanup screen.
pub(crate) fn refresh_candidates(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    let call = ws
        .source
        .call::<p::TaskCleanupCandidates>(&p::CleanupParams::default());
    cx.spawn(async move |this, cx| {
        let r = call.await;
        let _ = this.update(cx, |_, cx| {
            let st = state(cx);
            st.candidates_loaded = true;
            if let Ok(list) = r {
                st.candidates = list
                    .candidates
                    .into_iter()
                    .map(|c| (c.task.clone(), c))
                    .collect();
            }
            cx.notify();
        });
    })
    .detach();
}

pub(crate) fn refresh_settings(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    // The Codex worker allowance is per repository (`.pod/config.toml`).
    for repo in ws.repos.iter().map(|r| r.name.clone()) {
        let call = ws.source.call::<p::RepoConfigMethod>(&p::RepoConfigParams {
            repo: repo.clone(),
            task: None,
        });
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |_, cx| {
                state(cx)
                    .allowances
                    .insert(repo, r.ok().and_then(|c| c.codex_allowance));
                cx.notify();
            });
        })
        .detach();
    }
    let limits = ws
        .source
        .call::<p::OrchLimitsGet>(&p::OrchLimitsGetParams {});
    cx.spawn(async move |this, cx| {
        let r = limits.await;
        let _ = this.update(cx, |_, cx| {
            let st = state(cx);
            match r {
                Ok(l) => {
                    st.limits = Some(l);
                    st.limits_failed = false;
                }
                Err(_) => st.limits_failed = st.limits.is_none(),
            }
            cx.notify();
        });
    })
    .detach();
    let call = ws.source.call::<p::OrchConfigGet>(&p::Empty {});
    cx.spawn(async move |this, cx| {
        let r = call.await;
        let _ = this.update(cx, |_, cx| {
            let st = state(cx);
            match r {
                Ok(s) => {
                    st.settings = Some(s);
                    st.settings_failed = false;
                }
                Err(_) => st.settings_failed = st.settings.is_none(),
            }
            cx.notify();
        });
    })
    .detach();
}

// ───────────────────────────── notifications ─────────────────────────────

/// A toast (escalations stay until dismissed: warnings and errors are not
/// timed), plus a Windows toast when Pod is in the background.
fn notify(ws: &mut Workspace, n: Notice, window: &mut Window, cx: &mut Context<Workspace>) {
    let kind = if n.kind == NoticeKind::Error {
        ToastKind::Error
    } else {
        ToastKind::Warning
    };
    ws.raise_toast(
        kind,
        n.title.clone(),
        format!("{} — {} (Ctrl+Shift+M)", n.body, s::INBOX),
        cx,
    );
    let setting = cx
        .try_global::<crate::theme::Settings>()
        .is_none_or(|s| s.os_notifications);
    if !crate::os_notify::should_post(
        setting,
        window.is_window_active(),
        crate::os_notify::minimized(window),
    ) {
        return;
    }
    if state(cx).os.is_none() {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<crate::os_notify::Target>();
        let task = cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt as _;
            while rx.next().await.is_some() {
                let _ = this.update_in(cx, |w, window, cx| {
                    window.activate_window();
                    open_inbox(w, window, cx);
                });
            }
        });
        let st = state(cx);
        st.os = Some(crate::os_notify::OsToasts::new(tx));
        st.os_clicks = Some(task);
    }
    let terminal = state(cx)
        .runs
        .get(&n.run)
        .and_then(|r| r.info.run.coordinator_terminal.clone());
    if let Some(os) = state(cx).os.as_mut() {
        os.post(
            &n.title,
            &n.body,
            "Pod · 오케스트레이션",
            crate::os_notify::Target {
                task: None,
                terminal,
            },
        );
    }
}

// ───────────────────────────── actions ─────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkerVerb {
    Stop,
    Abandon,
    Keep,
    Release,
}

impl WorkerVerb {
    fn method(self) -> &'static str {
        match self {
            WorkerVerb::Stop => p::OrchWorkerStop::NAME,
            WorkerVerb::Abandon => p::OrchWorkerAbandon::NAME,
            WorkerVerb::Keep => p::OrchWorkerKeep::NAME,
            WorkerVerb::Release => p::OrchWorkerRelease::NAME,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WorkerVerb::Stop => s::STOP,
            WorkerVerb::Abandon => s::ABANDON,
            WorkerVerb::Keep => s::KEEP,
            WorkerVerb::Release => s::RELEASE,
        }
    }

    fn key(self) -> &'static str {
        match self {
            WorkerVerb::Stop => "stop",
            WorkerVerb::Abandon => "abandon",
            WorkerVerb::Keep => "keep",
            WorkerVerb::Release => "release",
        }
    }
}

pub(crate) fn busy_key(attempt: &str, verb: WorkerVerb) -> String {
    format!("{attempt}:{}", verb.key())
}

pub(crate) fn with_ws(
    ws: &WeakEntity<Workspace>,
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Context<Workspace>),
) {
    if let Some(w) = ws.upgrade() {
        w.update(cx, f);
    }
}

/// A worker button. Stop asks first (it kills the agent's process tree).
pub(crate) fn worker_verb(
    ws: &WeakEntity<Workspace>,
    attempt: String,
    verb: WorkerVerb,
    window: &mut Window,
    cx: &mut App,
) {
    if verb == WorkerVerb::Stop {
        let weak = ws.clone();
        window.open_dialog(cx, move |d: Dialog, _, _| {
            let (weak, attempt) = (weak.clone(), attempt.clone());
            d.title(s::STOP_CONFIRM_TITLE)
                .w(px(520.))
                .child(div().text_sm().child(format!(
                    "{} {attempt}. {}",
                    s::WORKER,
                    s::STOP_CONFIRM_BODY
                )))
                .footer(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("orch-stop-cancel")
                                .label(s::CANCEL)
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("orch-stop-confirm")
                                .label(s::STOP)
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    send_verb(&weak, attempt.clone(), WorkerVerb::Stop, cx);
                                }),
                        ),
                )
        });
        return;
    }
    send_verb(ws, attempt, verb, cx);
}

fn send_verb(ws: &WeakEntity<Workspace>, attempt: String, verb: WorkerVerb, cx: &mut App) {
    let key = busy_key(&attempt, verb);
    if !state(cx).busy.insert(key.clone()) {
        return;
    }
    with_ws(ws, cx, |w, cx| {
        let rx = w.source.request(verb.method(), json!({"attempt": attempt}));
        cx.spawn(async move |this, cx| {
            let r = value(rx).await.and_then(|v| {
                serde_json::from_value::<WorkerInfo>(v).map_err(|e| err(e.to_string()))
            });
            let _ = this.update(cx, |w, cx| {
                state(cx).busy.remove(&key);
                match r {
                    Ok(info) => {
                        // Show the answer now; `orch.changed` re-reads the rest.
                        let run = info.attempt.run.clone();
                        if let Some(e) = state(cx).runs.get_mut(&run) {
                            if let Some(a) =
                                e.info.attempts.iter_mut().find(|a| a.id == info.attempt.id)
                            {
                                *a = info.attempt.clone();
                            }
                            match e
                                .workers
                                .iter_mut()
                                .find(|x| x.attempt.id == info.attempt.id)
                            {
                                Some(x) => *x = info.clone(),
                                None => e.workers.push(info.clone()),
                            }
                        }
                        let what = format!(
                            "{} {} · {} → {}",
                            s::WORKER,
                            info.attempt.id,
                            verb.label(),
                            model::attempt_state_label(info.attempt.state)
                        );
                        w.raise_toast(ToastKind::Success, s::ACTION_DONE, what, cx);
                    }
                    Err(e) => w.raise_toast(
                        ToastKind::Error,
                        s::ACTION_FAILED,
                        format!("{} {}: {}", verb.label(), attempt, e.message),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    });
}

/// orch-02: retry an unconfirmed wake-up — press Enter in that agent's
/// terminal (`terminal.send {text: "", enter: true}`, confirmed like any
/// send). The line Pod typed may sit in the input box; Pod types no second
/// one, so Enter is the safe retry. The mark clears when the agent checks
/// its mail (`orch.changed`).
pub(crate) fn rewake(ws: &WeakEntity<Workspace>, stuck: model::StuckWake, cx: &mut App) {
    let Some(term) = stuck.terminal.clone() else {
        return;
    };
    let key = stuck.busy_key();
    if !state(cx).busy.insert(key.clone()) {
        return;
    }
    with_ws(ws, cx, |w, cx| {
        let rx = w.source.request(
            p::TerminalSend::NAME,
            json!({"id": term, "text": "", "enter": true}),
        );
        cx.spawn(async move |this, cx| {
            let r = value(rx).await.and_then(|v| {
                serde_json::from_value::<p::SendResult>(v).map_err(|e| err(e.to_string()))
            });
            let _ = this.update(cx, |w, cx| {
                state(cx).busy.remove(&key);
                let who = stuck.who();
                match r.map(|r| r.submit.map(|s| s.state)) {
                    Ok(Some(p::SubmitState::Confirmed)) => {
                        w.raise_toast(ToastKind::Success, s::WAKE_RETRIED, who, cx)
                    }
                    Ok(Some(p::SubmitState::Unconfirmed | p::SubmitState::Pending)) => w
                        .raise_toast(
                            ToastKind::Warning,
                            s::WAKE_FAILED,
                            format!("{who}: {}", s::WAKE_NOT_TAKEN),
                            cx,
                        ),
                    Ok(_) => w.raise_toast(ToastKind::Info, s::WAKE_NO_CHECK, who, cx),
                    Err(e) => w.raise_toast(
                        ToastKind::Error,
                        s::WAKE_FAILED,
                        format!("{who}: {}", e.message),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    });
}

/// Answer a message for the coordinator (`from: human`).
pub(crate) fn reply(ws: &WeakEntity<Workspace>, id: String, body: String, cx: &mut App) -> bool {
    let body = body.trim().to_string();
    if body.is_empty() {
        return false;
    }
    let key = format!("reply:{id}");
    if !state(cx).busy.insert(key.clone()) {
        return false;
    }
    with_ws(ws, cx, |w, cx| {
        let call = w.source.call::<p::OrchMailReply>(&p::OrchMailReplyParams {
            id: id.clone(),
            body,
            caller: Default::default(),
            request_id: None,
        });
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |w, cx| {
                state(cx).busy.remove(&key);
                match r {
                    Ok(m) => {
                        if let Some(e) = state(cx).runs.get_mut(&m.run)
                            && !e.mail.iter().any(|x| x.id == m.id)
                        {
                            e.mail.push(m.clone());
                        }
                        w.raise_toast(
                            ToastKind::Success,
                            s::REPLY_SENT,
                            format!("{} → {}", ko::subject(&m), m.to),
                            cx,
                        );
                    }
                    Err(e) => w.raise_toast(ToastKind::Error, s::REPLY_FAILED, e.message, cx),
                }
                cx.notify();
            });
        })
        .detach();
    });
    true
}

/// Archive (S3) or resume a finished worker's task.
pub(crate) fn task_verb(ws: &WeakEntity<Workspace>, task: String, archive: bool, cx: &mut App) {
    let key = format!("task:{task}");
    if !state(cx).busy.insert(key.clone()) {
        return;
    }
    with_ws(ws, cx, |w, cx| {
        let rx = if archive {
            w.source
                .request(p::TaskArchive::NAME, json!({"task": task}))
        } else {
            w.source.request(p::TaskResume::NAME, json!({"task": task}))
        };
        cx.spawn(async move |this, cx| {
            let r = value(rx).await;
            let _ = this.update(cx, |w, cx| {
                state(cx).busy.remove(&key);
                match r {
                    Ok(_) => {
                        w.source.refresh_tasks();
                        if let Some(c) = state(cx).candidates.get_mut(&task) {
                            c.lifecycle = if archive {
                                p::Lifecycle::Archived
                            } else {
                                p::Lifecycle::Active
                            };
                        }
                        w.raise_toast(
                            ToastKind::Success,
                            if archive {
                                s::ARCHIVE_DONE
                            } else {
                                s::RESUME_DONE
                            },
                            task.clone(),
                            cx,
                        );
                    }
                    Err(e) => w.raise_toast(
                        ToastKind::Error,
                        s::CLEANUP_FAILED,
                        format!("{task}: {}", e.message),
                        cx,
                    ),
                }
                cx.notify();
            });
        })
        .detach();
    });
}

/// `orch.limits.set` with one limit changed (all or nothing on the daemon).
pub(crate) fn set_limit(ws: &WeakEntity<Workspace>, key: String, value: String, cx: &mut App) {
    with_ws(ws, cx, |w, cx| {
        let mut set = std::collections::BTreeMap::new();
        set.insert(key, value);
        let call = w.source.call::<p::OrchLimitsSet>(&p::OrchLimitsSetParams {
            set,
            reset: Vec::new(),
            caller: Default::default(),
        });
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |w, cx| {
                match r {
                    Ok(v) => {
                        state(cx).limits = Some(v);
                        w.raise_toast(ToastKind::Success, s::SAVED, String::new(), cx);
                    }
                    Err(e) => w.raise_toast(ToastKind::Error, s::SAVE_FAILED, e.message, cx),
                }
                cx.notify();
            });
        })
        .detach();
    });
}

/// `orch.config.set` with one field changed.
pub(crate) fn set_settings(ws: &WeakEntity<Workspace>, next: OrchSettings, cx: &mut App) {
    with_ws(ws, cx, |w, cx| {
        let call = w.source.call::<p::OrchConfigSet>(&next);
        cx.spawn(async move |this, cx| {
            let r = call.await;
            let _ = this.update(cx, |w, cx| {
                match r {
                    Ok(v) => {
                        state(cx).settings = Some(v);
                        w.raise_toast(ToastKind::Success, s::SAVED, String::new(), cx);
                    }
                    Err(e) => w.raise_toast(ToastKind::Error, s::SAVE_FAILED, e.message, cx),
                }
                cx.notify();
            });
        })
        .detach();
    });
}
