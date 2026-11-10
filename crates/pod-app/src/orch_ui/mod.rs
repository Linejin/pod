//! Orchestration screens (docs/19 §8, S7): Run board, worker cards,
//! coordinator inbox, cleanup of finished workers and orchestration
//! settings, over the v16 `orch.*` RPCs.
//!
//! **Events only**: nothing here polls. The status stream's `orch.changed`
//! re-reads that run (`orch.run.show` + `orch.worker.list`), `orch.mail`
//! re-reads its mail (`orch.mail.list`, which acknowledges nothing), and a
//! (re)subscribe re-reads every run (`orch.run.list`). Reads of the same run
//! are coalesced: one in flight, at most one more queued.
//!
//! The rules (columns, cards, threads, cleanup tiers, notices) are in
//! [`model`]; the screens in [`pages`] (center dock) and [`inbox`] (right
//! dock). Copy is Korean ([`s`]).

pub(crate) mod fixture;
mod inbox;
pub(crate) mod ko;
pub(crate) mod model;
mod pages;
#[cfg(test)]
mod qa;
pub(crate) mod s;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use gpui_kit::component::WindowExt;
use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::*;
use podd::protocol::{self as p, CleanupCandidate, Method as _, OrchSettings, WorkerInfo};
use serde_json::{Value, json};

use crate::source::{Answer, SourceError, SourceEvent};
use crate::workspace::{ToastKind, Workspace};
pub(crate) use inbox::InboxPanel;
use model::{Notice, NoticeKind, RunEntry};
pub(crate) use pages::{OrchPage, Page};

actions!(pod_orch_ui, [OpenRunBoard, OpenOrchInbox]);

/// Ctrl+Shift+J run board, Ctrl+Shift+M inbox.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-shift-j", OpenRunBoard, None),
        KeyBinding::new("ctrl-shift-m", OpenOrchInbox, None),
    ]);
}

/// What the app knows about runs (a global: the screens and the workspace share it).
#[derive(Default)]
pub(crate) struct OrchUi {
    pub runs: BTreeMap<String, RunEntry>,
    /// `orch.run.list` answered at least once.
    pub loaded: bool,
    pub loading: bool,
    /// The daemon has no `orch.*` (older than v16).
    pub unsupported: bool,
    /// The daemon has no `orch.mail.list` (inbox shows what `run.show` counts).
    pub mail_unsupported: bool,
    pub error: Option<String>,
    /// Run shown on the board (default: the newest unfinished one).
    pub selected: Option<String>,
    /// Worker card selected with the arrow keys (board keyboard actions).
    pub selected_card: Option<String>,
    pub settings: Option<OrchSettings>,
    pub settings_failed: bool,
    /// `orch.limits.get` (v18): the resource gate's limits as settings.
    pub limits: Option<p::OrchLimitsResult>,
    pub limits_failed: bool,
    /// `[worker] codex_allowance` by repository (`repo.config`, docs/19
    /// §18.1); `None` = unknown (older daemon or not readable).
    pub allowances: BTreeMap<String, Option<bool>>,
    /// `task.cleanup_candidates` by task (loaded with the cleanup screen).
    pub candidates: HashMap<String, CleanupCandidate>,
    pub candidates_loaded: bool,
    /// Bulk selection of the cleanup screen (`None` = the default: ready tier).
    pub selection: Option<BTreeSet<String>>,
    /// Actions in flight (`<attempt>:<verb>`, `reply:<mail>`, `task:<id>`).
    pub busy: HashSet<String>,
    run_fetch: HashSet<String>,
    run_dirty: HashSet<String>,
    mail_fetch: HashSet<String>,
    mail_dirty: HashSet<String>,
    pub pages: Vec<(Page, WeakEntity<OrchPage>)>,
    pub inbox: Option<WeakEntity<InboxPanel>>,
    os: Option<crate::os_notify::OsToasts>,
    os_clicks: Option<Task<()>>,
}

impl Global for OrchUi {}

pub(crate) fn state(cx: &mut App) -> &mut OrchUi {
    cx.default_global::<OrchUi>()
}

pub(crate) fn read(cx: &App) -> Option<&OrchUi> {
    cx.try_global::<OrchUi>()
}

impl OrchUi {
    /// Runs for the board: unfinished first, newest first.
    pub fn ordered(&self) -> Vec<&RunEntry> {
        let mut v: Vec<&RunEntry> = self.runs.values().collect();
        v.sort_by_key(|r| (r.is_finished(), std::cmp::Reverse(model::id_number(r.id()))));
        v
    }

    pub fn selected_run(&self) -> Option<&RunEntry> {
        self.selected
            .as_deref()
            .and_then(|id| self.runs.get(id))
            .or_else(|| self.ordered().into_iter().next())
    }

    pub fn open_questions(&self) -> usize {
        self.runs.values().map(|r| r.open_questions()).sum()
    }

    pub fn page_open(&self, page: Page) -> bool {
        self.pages
            .iter()
            .any(|(p, w)| *p == page && w.upgrade().is_some())
    }
}

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

// ───────────────────────────── screens ─────────────────────────────

/// Dock names of the orchestration panels (restored from a saved layout).
pub(crate) const PANEL_NAMES: [&str; 4] = [
    "PodOrchBoard",
    "PodOrchCleanup",
    "PodOrchSettings",
    inbox::INBOX_PANEL_NAME,
];

/// `PanelRegistry` builder: a saved layout brings the screens back.
pub(crate) fn build_panel(
    ctx: gpui_kit::component::dock::PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> std::sync::Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let ws = cx
        .global::<crate::workspace::AppGlobals>()
        .workspace
        .clone();
    let name = ctx.state().panel_name.clone();
    let page = match name.as_str() {
        "PodOrchCleanup" => Some(Page::Cleanup),
        "PodOrchSettings" => Some(Page::Settings),
        n if n == inbox::INBOX_PANEL_NAME => None,
        _ => Some(Page::Board),
    };
    let w = ws.clone();
    cx.defer(move |cx| {
        let _ = w.update(cx, |w, cx| match page {
            Some(Page::Cleanup) => refresh_candidates(w, cx),
            Some(Page::Settings) => refresh_settings(w, cx),
            _ => {}
        });
    });
    match page {
        Some(page) => gpui_kit::component::dock::panel_handle(pages::new_panel(page, ws, cx)),
        None => gpui_kit::component::dock::panel_handle(inbox::new_panel(ws, window, cx)),
    }
}

/// Open (or focus) a center page.
pub(crate) fn open_page(
    ws: &mut Workspace,
    page: Page,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let loaded = state(cx).loaded;
    if !loaded {
        reload_all(ws, window, cx);
    }
    match page {
        // A first load reads the candidates itself once the runs are in.
        Page::Cleanup if loaded => refresh_candidates(ws, cx),
        Page::Cleanup => {}
        Page::Settings => refresh_settings(ws, cx),
        Page::Board => {}
    }
    pages::open(ws, page, window, cx);
}

pub(crate) fn open_inbox(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    if !state(cx).loaded {
        reload_all(ws, window, cx);
    }
    inbox::open(ws, window, cx);
}

/// Palette rows `(id, title, detail)`.
pub(crate) fn palette_entries(cx: &App) -> Vec<(String, String, String)> {
    let q = read(cx).map(|s| s.open_questions()).unwrap_or(0);
    let mut out = vec![
        (
            "orch-board".to_string(),
            s::BOARD.to_string(),
            s::BOARD_DETAIL.to_string(),
        ),
        (
            "orch-inbox".into(),
            if q > 0 {
                format!("{} ({}: {q})", s::INBOX, s::OPEN_QUESTIONS)
            } else {
                s::INBOX.into()
            },
            s::INBOX_DETAIL.into(),
        ),
        (
            "orch-cleanup".into(),
            s::CLEANUP.into(),
            s::CLEANUP_DETAIL.into(),
        ),
        (
            "orch-settings".into(),
            s::SETTINGS.into(),
            s::SETTINGS_DETAIL.into(),
        ),
    ];
    if q > 0 {
        out.push((
            "orch-next-question".into(),
            s::NEXT_QUESTION.into(),
            s::NEXT_QUESTION_DETAIL.into(),
        ));
    }
    out
}

/// Palette ids of this module. Returns false for other ids.
pub(crate) fn run(
    ws: &mut Workspace,
    id: &str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> bool {
    match id {
        // `orch-wake`: the board of the fixture scene with unconfirmed wake-ups.
        "orch-board" | fixture::WAKE_SCENE => open_page(ws, Page::Board, window, cx),
        "orch-cleanup" => open_page(ws, Page::Cleanup, window, cx),
        "orch-settings" => open_page(ws, Page::Settings, window, cx),
        "orch-inbox" => open_inbox(ws, window, cx),
        "orch-next-question" => {
            open_inbox(ws, window, cx);
            inbox::focus_next_question(window, cx);
        }
        _ => return false,
    }
    true
}

/// Settings page link (one row under the general settings).
pub(crate) fn settings_link(ws: &WeakEntity<Workspace>, _cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let ws = ws.clone();
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .pt_4()
        .mt_2()
        .border_t_1()
        .border_color(rgb(t.border_subtle))
        .child(div().w(px(110.)).flex_shrink_0().child(s::SETTINGS))
        .child(
            Button::new("open-orch-settings")
                .label(s::OPEN_SETTINGS_PAGE)
                .compact()
                .accessibility_label(format!(
                    "{} — {}",
                    s::OPEN_SETTINGS_PAGE,
                    s::SETTINGS_DETAIL
                ))
                .on_click(move |_, window, cx| {
                    let _ = ws.update(cx, |w, cx| open_page(w, Page::Settings, window, cx));
                }),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(t.text_muted))
                .child(s::SETTINGS_DETAIL),
        )
        .into_any_element()
}

/// Is an orchestration screen open (the workspace's empty state must not cover it)?
pub(crate) fn page_open(cx: &App) -> bool {
    read(cx).is_some_and(|s| {
        s.pages.iter().any(|(_, p)| p.upgrade().is_some())
            || s.inbox.as_ref().is_some_and(|p| p.upgrade().is_some())
    })
}
