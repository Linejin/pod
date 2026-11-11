//! The extensions UI's requests and podd's mod events: `mod.list` (one at a
//! time; on connect, when the page opens or renders without a list),
//! `mod.set_enabled` / `mod.grant` / `mod.reload` / `mod.log` from the page,
//! `mod.command` and preset ids from the palette, and `mod.changed` /
//! `mod.notify` (an in-app toast, and a Windows toast while Pod is not in
//! front). Every answer lands in [`ModsUi`]; a refused change toasts.

use gpui_kit::*;
use podd::protocol::{
    Empty, ModCommand, ModCommandParams, ModEnableParams, ModGrant, ModGrantParams, ModInfo,
    ModListMethod, ModLog, ModLogParams, ModReload, ModReloadParams, ModSetEnabled, Notice,
    Permission,
};

use super::copy::s;
use super::model::{self, PaletteId};
use super::{ModsUi, layouts, state};
use crate::workspace::{ToastKind, Workspace};

/// Re-read `mod.list` (on connect, when Settings opens, after reload).
pub(crate) fn refresh(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    if std::mem::replace(&mut state(cx).loading, true) {
        return;
    }
    let call = ws.source.call::<ModListMethod>(&Empty {});
    cx.spawn(async move |this, cx| {
        let r = call.await;
        let _ = this.update(cx, |_, cx| {
            let st = state(cx);
            st.loading = false;
            match r {
                Ok(list) => {
                    st.list = Some(list);
                    st.failed = false;
                }
                Err(_) => st.failed = true,
            }
            cx.notify();
        });
    })
    .detach();
}

/// `mod.changed`: replace that mod's entry.
pub(crate) fn on_changed(info: ModInfo, cx: &mut Context<Workspace>) {
    if let Some(list) = state(cx).list.as_mut() {
        match list.list.mods.iter_mut().find(|m| m.name == info.name) {
            Some(m) => *m = info,
            None => list.list.mods.push(info),
        }
    }
    cx.notify();
}

/// `mod.notify`: an in-app toast titled with the mod's name and, when the
/// user is not looking at Pod and the "OS 알림" setting is on, a Windows toast
/// (falls back silently when toasts are unavailable, os_notify.rs).
pub(crate) fn on_notice(
    ws: &mut Workspace,
    n: Notice,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let kind = model::toast_kind(&n.level);
    let who = model::notice_author(state(cx).list.as_ref(), &n.mod_name);
    ws.raise_toast(kind, format!("{} · {}", n.title, who), n.body.clone(), cx);
    let setting = cx
        .try_global::<crate::theme::Settings>()
        .is_none_or(|s| s.os_notifications);
    if crate::os_notify::should_post(
        setting,
        window.is_window_active(),
        crate::os_notify::minimized(window),
    ) {
        if state(cx).os.is_none() {
            let (tx, mut rx) = futures::channel::mpsc::unbounded();
            let task = cx.spawn_in(window, async move |this, cx| {
                use futures::StreamExt as _;
                while let Some(target) = rx.next().await {
                    let _ =
                        this.update_in(cx, |w, window, cx| on_toast_click(w, target, window, cx));
                }
            });
            let st = state(cx);
            st.os = Some(crate::os_notify::OsToasts::new(tx));
            st.os_clicks = Some(task);
        }
        let target = crate::os_notify::Target {
            task: n.task.clone(),
            terminal: n.terminal.clone(),
        };
        if let Some(os) = state(cx).os.as_mut() {
            os.post(&n.title, &n.body, &format!("{} · Pod", who), target);
        }
    }
}

/// A Windows toast was clicked: bring Pod forward and show the terminal.
fn on_toast_click(
    ws: &mut Workspace,
    target: crate::os_notify::Target,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    window.activate_window();
    let terminals: Vec<String> = ws.terminals.iter().map(|t| t.id.clone()).collect();
    let tasks: Vec<(String, Vec<String>)> = ws
        .tasks
        .iter()
        .map(|t| (t.id.clone(), t.terminals.clone()))
        .collect();
    if let Some(id) = model::click_terminal(&target, &terminals, &tasks) {
        ws.open_terminal(&id, false, window, cx);
    }
}

/// Palette rows: `(id, title, detail)` for running mods' commands and layouts.
pub(crate) fn palette_entries(cx: &App) -> Vec<(String, String, String)> {
    let Some(list) = cx.try_global::<ModsUi>().and_then(|s| s.list.as_ref()) else {
        return Vec::new();
    };
    model::palette_rows(list)
}

/// Palette ids `mod:<name>:<command>` and `layout:<name>:<id>`. Returns false for other ids.
pub(crate) fn run(
    ws: &mut Workspace,
    id: &str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> bool {
    let Some(id) = model::palette_id(id) else {
        return false;
    };
    match id {
        PaletteId::Incomplete => {}
        PaletteId::Command { name, command } => {
            let task = ws.current_task(window, cx);
            let terminal = ws.focused_terminal.clone();
            let call = ws.source.call::<ModCommand>(&ModCommandParams {
                name: name.into(),
                command: command.into(),
                task,
                terminal,
            });
            cx.spawn(async move |this, cx| {
                if let Err(e) = call.await {
                    let _ = this.update(cx, |w, cx| {
                        w.raise_toast(ToastKind::Error, s::COMMAND_FAILED, e.message, cx)
                    });
                }
            })
            .detach();
        }
        PaletteId::Layout { name, layout } => {
            let preset = model::find_preset(
                cx.try_global::<ModsUi>().and_then(|s| s.list.as_ref()),
                name,
                layout,
            );
            match preset {
                Some(p) => layouts::apply_layout(ws, &p, window, cx),
                None => ws.raise_toast(
                    ToastKind::Error,
                    s::LAYOUT_MISSING,
                    format!("{} ({name}: {layout})", s::LAYOUT_MISSING_DETAIL),
                    cx,
                ),
            }
        }
    }
    true
}

// ───────────────────────────── the page's requests ─────────────────────────────

fn after_change(
    ws: &WeakEntity<Workspace>,
    r: Result<ModInfo, crate::source::SourceError>,
    cx: &mut AsyncApp,
) {
    let _ = ws.update(cx, |w, cx| match r {
        Ok(info) => on_changed(info, cx),
        Err(e) => w.raise_toast(ToastKind::Error, s::CHANGE_FAILED, e.message, cx),
    });
}

pub(super) fn set_enabled(ws: &WeakEntity<Workspace>, name: String, enabled: bool, cx: &mut App) {
    let Some(w) = ws.upgrade() else { return };
    let call = w
        .read(cx)
        .source
        .call::<ModSetEnabled>(&ModEnableParams { name, enabled });
    let ws = ws.clone();
    cx.spawn(async move |cx| after_change(&ws, call.await, cx))
        .detach();
}

pub(super) fn grant(
    ws: &WeakEntity<Workspace>,
    name: String,
    permission: Permission,
    granted: bool,
    cx: &mut App,
) {
    let Some(w) = ws.upgrade() else { return };
    let call = w.read(cx).source.call::<ModGrant>(&ModGrantParams {
        name,
        permission,
        granted,
    });
    let ws = ws.clone();
    cx.spawn(async move |cx| after_change(&ws, call.await, cx))
        .detach();
}

pub(super) fn reload(ws: &WeakEntity<Workspace>, cx: &mut App) {
    let Some(w) = ws.upgrade() else { return };
    let call = w
        .read(cx)
        .source
        .call::<ModReload>(&ModReloadParams::default());
    let ws = ws.clone();
    cx.spawn(async move |cx| {
        let r = call.await;
        let _ = ws.update(cx, |w, cx| {
            match r {
                Ok(list) => {
                    let st = state(cx);
                    st.list = Some(list);
                    st.failed = false;
                }
                Err(e) => w.raise_toast(ToastKind::Error, s::CHANGE_FAILED, e.message, cx),
            }
            cx.notify();
        });
    })
    .detach();
}

pub(super) fn toggle_log(ws: &WeakEntity<Workspace>, name: String, cx: &mut App) {
    let st = state(cx);
    if st.log_open.as_deref() == Some(name.as_str()) {
        st.log_open = None;
        let _ = ws.update(cx, |_, cx| cx.notify());
        return;
    }
    st.log_open = Some(name.clone());
    let Some(w) = ws.upgrade() else { return };
    let call = w.read(cx).source.call::<ModLog>(&ModLogParams {
        name: name.clone(),
        tail: Some(60),
    });
    let ws = ws.clone();
    cx.spawn(async move |cx| {
        let r = call.await;
        let _ = ws.update(cx, |_, cx| {
            let lines = r.map(|l| l.lines).unwrap_or_else(|e| vec![e.message]);
            state(cx).logs.insert(name, lines);
            cx.notify();
        });
    })
    .detach();
}
