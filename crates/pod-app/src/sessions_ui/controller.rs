//! The page's `sessions.*` requests and their replies, as before the split:
//! `sessions.list` when the page opens or re-reads (one at a time: reads
//! asked while one is in flight make one follow-up read after it), `sessions.scan` from 다시 찾기
//! and once after discovery is turned on (nothing scans on its own),
//! `sessions.settings` (then the list again), `sessions.hide` /
//! `sessions.register` / `sessions.resume` with their toasts, and the
//! resumed terminal, opened once the workspace lists it.

use gpui_kit::*;
use podd::protocol::{
    SessionsHide, SessionsHideParams, SessionsList, SessionsListMethod, SessionsListParams,
    SessionsRegister, SessionsRegisterParams, SessionsResume, SessionsResumeParams, SessionsScan,
    SessionsScanParams, SessionsSettings, SessionsSettingsParams,
};

use super::model::{heading, place};
use super::{SessionsUi, copy, s, state};
use crate::source::SourceError;
use crate::workspace::{ToastKind, Workspace};

impl SessionsUi {
    /// A list-shaped reply arrived: nothing is loading or scanning any more;
    /// a failure marks the page unavailable only while it has no list yet.
    /// Returns the error message to raise.
    pub(super) fn answered(&mut self, r: Result<SessionsList, SourceError>) -> Option<String> {
        self.loading = false;
        self.scanning = false;
        match r {
            Ok(l) => {
                self.list = Some(l);
                self.failed = false;
                None
            }
            Err(e) => {
                self.failed = self.list.is_none();
                Some(e.message)
            }
        }
    }
}

fn apply(ws: &WeakEntity<Workspace>, r: Result<SessionsList, SourceError>, cx: &mut AsyncApp) {
    let _ = ws.update(cx, |w, cx| {
        if let Some(message) = state(cx).answered(r) {
            w.raise_toast(ToastKind::Error, s::FAILED, message, cx);
        }
        cx.notify();
    });
}

/// Re-read `sessions.list`. While one read is in flight, a re-read is
/// queued (any number of them make one) and goes out when it answers.
pub(super) fn refresh(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    if std::mem::replace(&mut state(cx).loading, true) {
        state(cx).reread = true;
        return;
    }
    let include_hidden = state(cx).show_hidden;
    let call = ws
        .source
        .call::<SessionsListMethod>(&SessionsListParams { include_hidden });
    let weak = cx.entity().downgrade();
    cx.spawn(async move |_, cx| {
        apply(&weak, call.await, cx);
        let _ = weak.update(cx, |w, cx| {
            if std::mem::take(&mut state(cx).reread) {
                refresh(w, cx);
            }
        });
    })
    .detach();
}

fn with_ws(
    ws: &WeakEntity<Workspace>,
    cx: &mut App,
    f: impl FnOnce(&mut Workspace, &mut Context<Workspace>),
) {
    if let Some(w) = ws.upgrade() {
        w.update(cx, f);
    }
}

pub(super) fn scan(ws: &WeakEntity<Workspace>, enable: bool, cx: &mut App) {
    let weak = ws.clone();
    with_ws(ws, cx, |w, cx| {
        let include_hidden = state(cx).show_hidden;
        state(cx).scanning = true;
        let call = w.source.call::<SessionsScan>(&SessionsScanParams {
            enable,
            include_hidden,
        });
        cx.spawn(async move |_, cx| apply(&weak, call.await, cx))
            .detach();
        cx.notify();
    });
}

pub(super) fn settings(ws: &WeakEntity<Workspace>, p: SessionsSettingsParams, cx: &mut App) {
    let weak = ws.clone();
    let rescan = p.enabled == Some(true);
    with_ws(ws, cx, |w, cx| {
        let call = w.source.call::<SessionsSettings>(&p);
        cx.spawn(async move |_, cx| {
            let r = call.await;
            let ok = r.is_ok();
            apply(&weak, r, cx);
            if ok && rescan {
                cx.update(|cx| scan(&weak, false, cx));
            } else if ok {
                let _ = weak.update(cx, refresh);
            }
        })
        .detach();
    });
}

pub(super) fn toggle_hidden(ws: &WeakEntity<Workspace>, cx: &mut App) {
    let st = state(cx);
    st.show_hidden = !st.show_hidden;
    with_ws(ws, cx, refresh);
}

pub(super) fn hide(ws: &WeakEntity<Workspace>, key: String, hidden: bool, cx: &mut App) {
    let weak = ws.clone();
    with_ws(ws, cx, |w, cx| {
        let call = w.source.call::<SessionsHide>(&SessionsHideParams {
            session: key,
            hidden: Some(hidden),
        });
        cx.spawn(async move |_, cx| match call.await {
            Ok(_) => {
                let _ = weak.update(cx, refresh);
            }
            Err(e) => {
                let _ = weak.update(cx, |w, cx| {
                    w.raise_toast(ToastKind::Error, s::FAILED, e.message, cx)
                });
            }
        })
        .detach();
    });
}

pub(super) fn register(ws: &WeakEntity<Workspace>, key: String, add_repo: bool, cx: &mut App) {
    let weak = ws.clone();
    with_ws(ws, cx, |w, cx| {
        let call = w.source.call::<SessionsRegister>(&SessionsRegisterParams {
            session: key,
            task: None,
            add_repo,
        });
        cx.spawn(async move |_, cx| {
            let r = call.await;
            let _ = weak.update(cx, |w, cx| {
                match r {
                    Ok(x) => {
                        // The repository may be new to the sidebar.
                        w.source.refresh_tasks();
                        let to = x.link.as_ref().map(place).unwrap_or_default();
                        w.raise_toast(
                            ToastKind::Success,
                            s::REGISTERED,
                            copy::registered(&heading(&x), &to),
                            cx,
                        );
                    }
                    Err(e) => w.raise_toast(ToastKind::Error, s::FAILED, e.message, cx),
                }
                refresh(w, cx);
            });
        })
        .detach();
    });
}

pub(super) fn resume(ws: &WeakEntity<Workspace>, key: String, title: String, cx: &mut App) {
    let weak = ws.clone();
    with_ws(ws, cx, |w, cx| {
        let call = w.source.call::<SessionsResume>(&SessionsResumeParams {
            session: key,
            name: None,
        });
        cx.spawn(async move |_, cx| {
            let r = call.await;
            let _ = weak.update(cx, |w, cx| {
                match r {
                    Ok(t) => {
                        w.raise_toast(
                            ToastKind::Success,
                            s::RESUMED,
                            copy::resumed(&title, &t.id),
                            cx,
                        );
                        state(cx).pending_open = Some(t.id);
                    }
                    Err(e) => w.raise_toast(ToastKind::Error, s::FAILED, e.message, cx),
                }
                refresh(w, cx);
            });
        })
        .detach();
    });
}

/// Open a terminal the page asked for (needs a window).
pub(super) fn open_terminal(
    ws: &WeakEntity<Workspace>,
    id: String,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(w) = ws.upgrade() {
        w.update(cx, |w, cx| w.open_terminal(&id, false, window, cx));
    }
}

/// A resumed terminal: open it once the workspace knows it (the panel asks
/// on every render, before it draws).
pub(super) fn open_resumed(ws: &WeakEntity<Workspace>, window: &mut Window, cx: &mut App) {
    if let Some(id) = cx
        .try_global::<SessionsUi>()
        .and_then(|s| s.pending_open.clone())
    {
        let known = ws
            .upgrade()
            .is_some_and(|w| w.read(cx).terminal(&id).is_some());
        if known {
            state(cx).pending_open = None;
            open_terminal(ws, id, window, cx);
        }
    }
}
