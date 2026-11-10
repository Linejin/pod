//! Extensions in the app (docs/16): the "확장" section of Settings (list,
//! enable/disable, permissions, log), mod palette commands and layout
//! presets, and `mod.notify` toasts. Mods run in podd; this module only
//! shows and asks.

use std::collections::BTreeMap;
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::{DockLayout, panel_handle};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use podd::protocol::{Axis, PanelKind};
use podd::protocol::{
    Empty, LayoutNode, LayoutPreset, ModCommand, ModCommandParams, ModEnableParams, ModGrant,
    ModGrantParams, ModInfo, ModListMethod, ModListResult, ModLog, ModLogParams, ModReload,
    ModReloadParams, ModSetEnabled, ModState, Notice, Permission, StopReason,
};

use crate::workspace::{ToastKind, Workspace};

/// Korean copy of the extensions UI (kept here; docs/16).
pub(crate) mod s {
    pub const SECTION: &str = "확장";
    pub const OPEN_PAGE: &str = "확장 관리";
    pub const OPEN_PAGE_DETAIL: &str = "모드 켜기·끄기, 권한, 로그";
    pub const INTRO: &str = "모드는 Pod 데몬 안의 샌드박스에서 돕니다. 파일·네트워크에 접근할 수 없고, 허용한 권한만 씁니다.";
    pub const FOLDER: &str = "모드 폴더";
    pub const RELOAD: &str = "다시 읽기";
    pub const LOADING: &str = "확장 목록을 불러오는 중…";
    pub const EMPTY: &str =
        "설치된 확장이 없습니다. 모드 폴더에 모드를 복사한 뒤 \"다시 읽기\"를 누르세요.";
    pub const OFF: &str = "이 데몬은 확장이 꺼져 있습니다 (POD_MODS=off).";
    pub const UNAVAILABLE: &str = "확장 목록을 불러오지 못했습니다. 데몬이 확장을 지원하는 버전(프로토콜 v11)인지 확인하세요.";
    pub const ENABLE: &str = "켜기";
    pub const DISABLE: &str = "끄기";
    pub const RUNNING: &str = "실행 중";
    pub const DISABLED: &str = "꺼짐";
    pub const STOPPED: &str = "정지됨";
    pub const INVALID: &str = "설정 오류";
    pub const PERMISSIONS: &str = "권한";
    pub const NO_PERMISSIONS: &str = "권한 없음 (레이아웃만 제공)";
    pub const ALLOWED: &str = "허용";
    pub const DENIED: &str = "거부";
    pub const SHOW_LOG: &str = "로그 보기";
    pub const HIDE_LOG: &str = "로그 닫기";
    pub const LOG_EMPTY: &str = "로그가 없습니다.";
    pub const CHECKS: &str = "검사 명령";
    pub const LAYOUT_PREFIX: &str = "레이아웃";
    pub const LAYOUT_EMPTY: &str = "적용할 패널이 없습니다";
    pub const LAYOUT_EMPTY_DETAIL: &str = "현재 작업에 터미널이나 검토할 내용이 없습니다.";
    pub const LAYOUT_MISSING: &str = "레이아웃을 찾을 수 없습니다";
    pub const LAYOUT_MISSING_DETAIL: &str =
        "확장이 꺼졌거나 다시 읽는 동안 프리셋이 사라졌습니다. 설정 → 확장에서 확인하세요.";
    pub const LAYOUT_INVALID: &str = "레이아웃 프리셋 오류";
    pub const MANIFEST_INVALID: &str = "mod.json 오류";
    pub const COMMAND_FAILED: &str = "확장 명령 실패";
    pub const CHANGE_FAILED: &str = "확장 설정을 바꾸지 못했습니다";
}

fn permission_label(p: Permission) -> &'static str {
    match p {
        Permission::Notify => "알림 표시",
        Permission::Commands => "팔레트 명령",
        Permission::Status => "상태 감지",
        Permission::TerminalSend => "터미널에 입력",
        Permission::Check => "검사 명령 실행",
        Permission::Gate => "승인 요청",
        Permission::Orch => "오케스트레이션 이벤트 읽기",
    }
}

fn stop_reason_label(r: Option<StopReason>) -> &'static str {
    match r {
        Some(StopReason::Cpu) => "CPU 시간 한도 초과",
        Some(StopReason::Memory) => "메모리 한도 초과",
        Some(StopReason::Errors) => "오류 반복",
        Some(StopReason::Load) => "불러오기 실패",
        None => "",
    }
}

fn state_label(m: &ModInfo) -> String {
    match m.state {
        ModState::Running => s::RUNNING.into(),
        ModState::Disabled => s::DISABLED.into(),
        ModState::Stopped => format!("{}: {}", s::STOPPED, stop_reason_label(m.stop_reason)),
        ModState::Invalid => s::INVALID.into(),
    }
}

/// The card's error line in Korean: what kind of problem, then the detail
/// (the detail is the daemon's message for the mod author).
pub(crate) fn error_summary(m: &ModInfo) -> Option<String> {
    let first = m.error.as_deref()?.lines().next().unwrap_or_default();
    Some(match m.state {
        ModState::Invalid if first.starts_with("layout ") => {
            format!("{}: {first}", s::LAYOUT_INVALID)
        }
        ModState::Invalid => format!("{}: {first}", s::MANIFEST_INVALID),
        ModState::Stopped => format!("{}: {first}", stop_reason_label(m.stop_reason)),
        _ => first.to_string(),
    })
}

fn display_name(m: &ModInfo) -> String {
    m.title.clone().unwrap_or_else(|| m.name.clone())
}

/// What the app knows about the daemon's mods.
#[derive(Default)]
pub(crate) struct ModsUi {
    pub list: Option<ModListResult>,
    pub loading: bool,
    pub failed: bool,
    pub log_open: Option<String>,
    pub logs: BTreeMap<String, Vec<String>>,
    /// The open "확장" page, if any.
    pub panel: Option<WeakEntity<ExtensionsPanel>>,
    /// Windows toasts for notices while Pod is not in front (created on first use).
    pub os: Option<crate::os_notify::OsToasts>,
    /// Routes toast clicks back to the workspace.
    pub os_clicks: Option<Task<()>>,
}

impl Global for ModsUi {}

fn state(cx: &mut App) -> &mut ModsUi {
    cx.default_global::<ModsUi>()
}

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
    let kind = match n.level.as_str() {
        "success" => ToastKind::Success,
        "warning" => ToastKind::Warning,
        "error" => ToastKind::Error,
        _ => ToastKind::Info,
    };
    let who = state(cx)
        .list
        .as_ref()
        .and_then(|l| l.list.mods.iter().find(|m| m.name == n.mod_name))
        .map(display_name)
        .unwrap_or_else(|| n.mod_name.clone());
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

/// The terminal a toast click should show: the notice's terminal if it still
/// exists, else the first terminal of its task.
pub(crate) fn click_terminal(
    target: &crate::os_notify::Target,
    terminals: &[String],
    tasks: &[(String, Vec<String>)],
) -> Option<String> {
    target
        .terminal
        .clone()
        .filter(|t| terminals.contains(t))
        .or_else(|| {
            let task = target.task.as_deref()?;
            tasks
                .iter()
                .find(|(id, _)| id == task)?
                .1
                .iter()
                .find(|t| terminals.contains(t))
                .cloned()
        })
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
    if let Some(id) = click_terminal(&target, &terminals, &tasks) {
        ws.open_terminal(&id, false, window, cx);
    }
}

/// Palette rows: `(id, title, detail)` for running mods' commands and layouts.
pub(crate) fn palette_entries(cx: &App) -> Vec<(String, String, String)> {
    let Some(list) = cx.try_global::<ModsUi>().and_then(|s| s.list.as_ref()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for m in list
        .list
        .mods
        .iter()
        .filter(|m| m.state == ModState::Running)
    {
        for c in &m.commands {
            out.push((
                format!("mod:{}:{}", m.name, c.id),
                format!("{}: {}", display_name(m), c.title),
                c.detail
                    .clone()
                    .unwrap_or_else(|| format!("{} {}", s::SECTION, m.name)),
            ));
        }
        for l in &m.layouts {
            out.push((
                format!("layout:{}:{}", m.name, l.id),
                format!("{}: {}", s::LAYOUT_PREFIX, l.title),
                l.description.clone().unwrap_or_else(|| display_name(m)),
            ));
        }
    }
    out
}

/// Palette ids `mod:<name>:<command>` and `layout:<name>:<id>`. Returns false for other ids.
pub(crate) fn run(
    ws: &mut Workspace,
    id: &str,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> bool {
    if let Some(rest) = id.strip_prefix("mod:") {
        let Some((name, command)) = rest.split_once(':') else {
            return true;
        };
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
        return true;
    }
    if let Some(rest) = id.strip_prefix("layout:") {
        let Some((name, layout)) = rest.split_once(':') else {
            return true;
        };
        let preset = cx
            .try_global::<ModsUi>()
            .and_then(|s| s.list.as_ref())
            .and_then(|l| l.list.mods.iter().find(|m| m.name == name))
            .and_then(|m| m.layouts.iter().find(|l| l.id == layout))
            .cloned();
        match preset {
            Some(p) => apply_layout(ws, &p, window, cx),
            None => ws.raise_toast(
                ToastKind::Error,
                s::LAYOUT_MISSING,
                format!("{} ({name}: {layout})", s::LAYOUT_MISSING_DETAIL),
                cx,
            ),
        }
        return true;
    }
    false
}

// ───────────────────────────── layouts ─────────────────────────────

/// A preset resolved to panels.
enum Built {
    Tabs(
        Vec<Arc<dyn gpui_kit::component::dock::BasePanelView>>,
        Option<f32>,
        bool,
    ),
    Split(Axis, Vec<Built>, Option<f32>),
}

impl Built {
    fn size(&self) -> Option<f32> {
        match self {
            Built::Tabs(_, s, _) | Built::Split(_, _, s) => *s,
        }
    }

    fn with_size(mut self, size: Option<f32>) -> Self {
        match &mut self {
            Built::Tabs(_, s, _) | Built::Split(_, _, s) => *s = size,
        }
        self
    }

    /// Append `extra` to the first terminal leaf (`any`: to the first leaf).
    fn adopt(
        &mut self,
        extra: &mut Vec<Arc<dyn gpui_kit::component::dock::BasePanelView>>,
        any: bool,
    ) -> bool {
        match self {
            Built::Tabs(panels, _, terminal) if *terminal || any => {
                panels.append(extra);
                true
            }
            Built::Tabs(..) => false,
            Built::Split(_, children, _) => children.iter_mut().any(|c| c.adopt(extra, any)),
        }
    }

    fn into_layout(self, extent: (f32, f32), cx: &App) -> DockLayout {
        match self {
            Built::Tabs(panels, _, _) => panels
                .into_iter()
                .fold(DockLayout::tabs(), |t, p| t.panel_view(p, cx)),
            Built::Split(axis, children, _) => {
                let (mut layout, len) = match axis {
                    Axis::Horizontal => (DockLayout::h_split(), extent.0),
                    Axis::Vertical => (DockLayout::v_split(), extent.1),
                };
                for c in children {
                    let size = c.size().map(|f| px(f * len));
                    layout = layout.child(c.into_layout(extent, cx), size);
                }
                layout
            }
        }
    }
}

/// Lay out the center region from a preset (docs/16 §7.2). Gates and the
/// agent tree open in their usual right dock; open terminal panels the
/// preset does not place go to the first terminal slot as tabs.
pub(crate) fn apply_layout(
    ws: &mut Workspace,
    preset: &LayoutPreset,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let task = ws.current_task(window, cx);
    let focused = ws.focused_terminal.clone();
    let mut used: Vec<EntityId> = Vec::new();
    let built = build(ws, &preset.center, task.as_deref(), &mut used, window, cx);
    let Some(mut built) = built else {
        ws.raise_toast(ToastKind::Info, s::LAYOUT_EMPTY, s::LAYOUT_EMPTY_DETAIL, cx);
        return;
    };
    // Open panels the preset does not place are kept as tabs, never closed
    // (an editor may hold unsaved files): terminals in the first terminal
    // slot, review and editor panels after them.
    let mut rest: Vec<Arc<dyn gpui_kit::component::dock::BasePanelView>> = ws
        .live_panels()
        .into_iter()
        .filter(|p| !used.contains(&p.entity_id()))
        .map(|p| panel_handle(p))
        .collect();
    rest.extend(
        ws.live_reviews()
            .into_iter()
            .filter(|p| !used.contains(&p.entity_id()))
            .map(|p| panel_handle(p)),
    );
    rest.extend(
        ws.live_editors()
            .into_iter()
            .filter(|p| !used.contains(&p.entity_id()))
            .map(|p| panel_handle(p)),
    );
    if !rest.is_empty() && !built.adopt(&mut rest, false) {
        built.adopt(&mut rest, true);
    }
    let size = window.viewport_size();
    let layout = built.into_layout(
        (f32::from(size.width) * 0.8, f32::from(size.height) * 0.9),
        cx,
    );
    ws.dock.update(cx, |a, cx| a.set_center(layout, window, cx));
    // Opening gates/review moved keyboard focus; give it back to the
    // terminal the user was in (the preset rearranges, it does not navigate).
    if let Some(p) = focused.and_then(|id| {
        ws.live_panels()
            .into_iter()
            .find(|p| p.read(cx).terminal_id == id)
    }) {
        let f = p.read(cx).view.read(cx).focus_handle(cx);
        window.focus(&f, cx);
    }
    ws.schedule_save(cx);
    ws.touch(cx);
}

fn build(
    ws: &mut Workspace,
    node: &LayoutNode,
    task: Option<&str>,
    used: &mut Vec<EntityId>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Option<Built> {
    type Handle = Arc<dyn gpui_kit::component::dock::BasePanelView>;
    let leaf = |ws: &mut Workspace,
                panel: PanelKind,
                index: Option<usize>,
                used: &mut Vec<EntityId>,
                window: &mut Window,
                cx: &mut Context<Workspace>|
     -> Vec<Handle> {
        match panel {
            PanelKind::Terminal => {
                let i = index.unwrap_or(0);
                let id = match task.and_then(|t| ws.task(t)) {
                    Some(t) => t.terminals.get(i).cloned(),
                    None => ws
                        .live_panels()
                        .get(i)
                        .map(|p| p.read(cx).terminal_id.clone()),
                };
                let Some(id) = id else { return Vec::new() };
                if ws.terminal(&id).is_none() {
                    return Vec::new();
                }
                let existing = ws
                    .live_panels()
                    .into_iter()
                    .find(|p| p.read(cx).terminal_id == id && !used.contains(&p.entity_id()));
                let panel = match existing {
                    Some(p) => p,
                    None => {
                        ws.open_terminal(&id, true, window, cx);
                        let Some(p) = ws
                            .live_panels()
                            .into_iter()
                            .rev()
                            .find(|p| p.read(cx).terminal_id == id)
                        else {
                            return Vec::new();
                        };
                        p
                    }
                };
                used.push(panel.entity_id());
                vec![panel_handle(panel)]
            }
            PanelKind::Terminals => {
                let wanted: Option<Vec<String>> =
                    task.and_then(|t| ws.task(t)).map(|t| t.terminals.clone());
                let panels: Vec<_> = ws
                    .live_panels()
                    .into_iter()
                    .filter(|p| {
                        !used.contains(&p.entity_id())
                            && wanted
                                .as_ref()
                                .is_none_or(|w| w.contains(&p.read(cx).terminal_id))
                    })
                    .collect();
                used.extend(panels.iter().map(|p| p.entity_id()));
                panels.into_iter().map(|p| panel_handle(p)).collect()
            }
            PanelKind::Review => {
                let Some(t) = task else { return Vec::new() };
                ws.open_review(t, window, cx);
                let r = ws
                    .live_reviews()
                    .into_iter()
                    .find(|r| r.read(cx).task_id == t && !used.contains(&r.entity_id()));
                r.map(|r| {
                    used.push(r.entity_id());
                    vec![panel_handle(r)]
                })
                .unwrap_or_default()
            }
            PanelKind::Editor => {
                let Some(t) = task else { return Vec::new() };
                ws.open_editor(t, None, false, window, cx);
                let e = ws
                    .live_editors()
                    .into_iter()
                    .find(|e| e.read(cx).task_id == t && !used.contains(&e.entity_id()));
                e.map(|e| {
                    used.push(e.entity_id());
                    vec![panel_handle(e)]
                })
                .unwrap_or_default()
            }
            PanelKind::Gates => {
                ws.open_gates(window, cx);
                Vec::new()
            }
            PanelKind::Agents => {
                ws.open_agent_tree(window, cx);
                Vec::new()
            }
        }
    };
    match node {
        LayoutNode::Panel { panel, index, size } => {
            let panels = leaf(ws, *panel, *index, used, window, cx);
            let terminal = matches!(panel, PanelKind::Terminal | PanelKind::Terminals);
            (!panels.is_empty()).then_some(Built::Tabs(panels, *size, terminal))
        }
        LayoutNode::Tabs { tabs, size } => {
            let mut panels = Vec::new();
            let mut terminal = false;
            for t in tabs {
                if let LayoutNode::Panel { panel, index, .. } = t {
                    terminal |= matches!(panel, PanelKind::Terminal | PanelKind::Terminals);
                    panels.extend(leaf(ws, *panel, *index, used, window, cx));
                }
            }
            (!panels.is_empty()).then_some(Built::Tabs(panels, *size, terminal))
        }
        LayoutNode::Split {
            split,
            children,
            size,
        } => {
            let mut built: Vec<Built> = children
                .iter()
                .filter_map(|c| build(ws, c, task, used, window, cx))
                .collect();
            match built.len() {
                0 => None,
                // The lone child takes the split's slot, and its size.
                1 => built.pop().map(|b| b.with_size(*size)),
                _ => Some(Built::Split(*split, built, *size)),
            }
        }
    }
}

// ───────────────────────────── settings section ─────────────────────────────

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

fn set_enabled(ws: &WeakEntity<Workspace>, name: String, enabled: bool, cx: &mut App) {
    let Some(w) = ws.upgrade() else { return };
    let call = w
        .read(cx)
        .source
        .call::<ModSetEnabled>(&ModEnableParams { name, enabled });
    let ws = ws.clone();
    cx.spawn(async move |cx| after_change(&ws, call.await, cx))
        .detach();
}

fn grant(
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

fn reload(ws: &WeakEntity<Workspace>, cx: &mut App) {
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

fn toggle_log(ws: &WeakEntity<Workspace>, name: String, cx: &mut App) {
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

/// A row in Settings that leads to the "확장" page.
pub(crate) fn settings_link(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let summary = cx
        .try_global::<ModsUi>()
        .and_then(|s| s.list.as_ref())
        .map(|l| {
            format!(
                "{}개 설치 · {}개 켜짐",
                l.list.mods.len(),
                l.list.mods.iter().filter(|m| m.enabled).count()
            )
        })
        .unwrap_or_default();
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
        .child(div().w(px(110.)).flex_shrink_0().child(s::SECTION))
        .child(
            Button::new("open-extensions")
                .label(s::OPEN_PAGE)
                .compact()
                .accessibility_label(format!("{} — {}", s::OPEN_PAGE, s::OPEN_PAGE_DETAIL))
                .on_click(move |_, window, cx| {
                    let _ = ws.update(cx, |w, cx| open_page(w, window, cx));
                }),
        )
        .child(div().text_sm().text_color(rgb(t.text_muted)).child(summary))
        .into_any_element()
}

/// Is the "확장" page open (the workspace's empty state must not cover it)?
pub(crate) fn page_open(cx: &App) -> bool {
    cx.try_global::<ModsUi>()
        .and_then(|s| s.panel.as_ref())
        .is_some_and(|p| p.upgrade().is_some())
}

/// Open (or focus) the "확장" page in the center dock.
pub(crate) fn open_page(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    use gpui_kit::component::dock::{DockPlacement, PanelId};
    if let Some(p) = state(cx).panel.as_ref().and_then(|p| p.upgrade()) {
        ws.dock.update(cx, |d, cx| {
            d.select_panel(PanelId::from(p.entity_id()), window, cx)
        });
        let focus = p.read(cx).focus.clone();
        focus.focus(window, cx);
        return;
    }
    let workspace = cx.entity().downgrade();
    let panel = cx.new(|cx| {
        let sub = workspace
            .upgrade()
            .map(|w| cx.observe(&w, |_, _, cx| cx.notify()));
        ExtensionsPanel {
            workspace,
            focus: cx.focus_handle(),
            _sub: sub,
        }
    });
    state(cx).panel = Some(panel.downgrade());
    let focus = panel.read(cx).focus.clone();
    ws.dock.update(cx, |d, cx| {
        d.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx)
    });
    focus.focus(window, cx);
    refresh(ws, cx);
    ws.touch(cx);
}

/// The "확장" settings page.
pub(crate) struct ExtensionsPanel {
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    _sub: Option<Subscription>,
}

impl EventEmitter<gpui_kit::base::dock::PanelEvent> for ExtensionsPanel {}
impl Focusable for ExtensionsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_kit::base::dock::Panel for ExtensionsPanel {
    fn panel_name(&self) -> &'static str {
        "PodExtensions"
    }
    fn dump(&self, _: &App) -> gpui_kit::base::dock::PanelState {
        gpui_kit::base::dock::PanelState::new("PodExtensions")
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.defer(|cx| state(cx).panel = None);
    }
}
impl gpui_kit::component::dock::Panel for ExtensionsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(s::SECTION.into())
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(s::SECTION)
    }
}
impl Render for ExtensionsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        let ws = self.workspace.clone();
        div()
            .id("extensions-panel")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .bg(rgb(t.bg_surface))
            .text_color(rgb(t.text_primary))
            .child(page(&ws, cx))
    }
}

/// The content of the "확장" page.
fn page(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    if cx
        .try_global::<ModsUi>()
        .is_none_or(|s| s.list.is_none() && !s.loading && !s.failed)
    {
        let ws2 = ws.clone();
        cx.defer(move |cx| {
            let _ = ws2.update(cx, refresh);
        });
    }
    let st = state(cx);
    let (list, loading, failed, log_open) =
        (st.list.clone(), st.loading, st.failed, st.log_open.clone());
    let logs = st.logs.clone();
    let mut body = div()
        .id("mods-section")
        .role(Role::Group)
        .aria_label(s::SECTION)
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::SEMIBOLD)
                .child(s::SECTION),
        )
        .child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::INTRO),
        );
    let Some(list) = list else {
        return body
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(t.text_muted))
                    .child(if failed { s::UNAVAILABLE } else { s::LOADING }),
            )
            .into_any_element();
    };
    if !list.enabled {
        return body.child(div().text_sm().child(s::OFF)).into_any_element();
    }
    let ws_reload = ws.clone();
    body = body.child(
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(div().text_sm().text_color(rgb(t.text_muted)).child(format!(
                "{}: {}",
                s::FOLDER,
                list.list.root
            )))
            .child(
                Button::new("mods-reload")
                    .label(s::RELOAD)
                    .compact()
                    .loading(loading)
                    .accessibility_label(format!("{} — {}", s::SECTION, s::RELOAD))
                    .on_click(move |_, _, cx| reload(&ws_reload, cx)),
            ),
    );
    if list.list.mods.is_empty() {
        return body
            .child(div().text_sm().child(s::EMPTY))
            .into_any_element();
    }
    for m in &list.list.mods {
        body = body.child(mod_card(
            ws,
            m,
            log_open.as_deref() == Some(m.name.as_str()),
            logs.get(&m.name),
        ));
    }
    body.into_any_element()
}

fn mod_card(
    ws: &WeakEntity<Workspace>,
    m: &ModInfo,
    log_open: bool,
    log: Option<&Vec<String>>,
) -> AnyElement {
    let t = crate::theme::tokens();
    let name = display_name(m);
    let state = state_label(m);
    let (chip_fg, chip_bg) = match m.state {
        ModState::Running => (t.success_fg, t.success_bg),
        ModState::Disabled => (t.text_secondary, t.bg_hover),
        ModState::Stopped => (t.warning_fg, t.warning_bg),
        ModState::Invalid => (t.danger_fg, t.danger_bg),
    };
    let id = |part: &str| SharedString::from(format!("mod-{}-{part}", m.name));
    let ws_toggle = ws.clone();
    let (mod_name, enabled) = (m.name.clone(), m.enabled);
    let header = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(div().font_weight(FontWeight::SEMIBOLD).child(name.clone()))
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(format!("{} · v{}", m.name, m.version)),
        )
        .child(
            div()
                .px_2()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(chip_fg))
                .bg(rgb(chip_bg))
                .child(state.clone()),
        )
        .child(div().flex_1())
        .when(m.state != ModState::Invalid, |d| {
            d.child(
                Button::new(id("toggle"))
                    .label(if enabled { s::DISABLE } else { s::ENABLE })
                    .compact()
                    .when(!enabled, |b| b.primary())
                    .accessibility_label(format!(
                        "{name} {} ({state})",
                        if enabled { s::DISABLE } else { s::ENABLE }
                    ))
                    .on_click(move |_, _, cx| {
                        set_enabled(&ws_toggle, mod_name.clone(), !enabled, cx)
                    }),
            )
        });
    let mut card = div()
        .id(id("card"))
        .role(Role::Group)
        .aria_label(format!("{name}, {state}"))
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(t.border_subtle))
        .bg(rgb(t.bg_raised))
        .child(header);
    if let Some(d) = &m.description {
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(d.clone()),
        );
    }
    if let Some(e) = error_summary(m) {
        card = card.child(
            div()
                .id(id("error"))
                .role(Role::Alert)
                .text_sm()
                .text_color(rgb(t.danger_fg))
                .child(e),
        );
    }
    // Permissions.
    let requested: Vec<_> = m
        .permissions
        .iter()
        .filter(|p| p.requested)
        .cloned()
        .collect();
    let mut perms = div().flex().flex_wrap().items_center().gap_2().child(
        div()
            .w(px(56.))
            .flex_shrink_0()
            .text_sm()
            .child(s::PERMISSIONS),
    );
    if requested.is_empty() {
        perms = perms.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_muted))
                .child(s::NO_PERMISSIONS),
        );
    }
    for p in requested {
        let ws_grant = ws.clone();
        let mod_name = m.name.clone();
        let label = permission_label(p.name);
        let verdict = if p.granted { s::ALLOWED } else { s::DENIED };
        perms = perms.child(
            Button::new(id(&format!("perm-{}", p.name.name())))
                .label(format!("{label}: {verdict}"))
                .compact()
                .when(p.granted, |b| b.primary())
                .tooltip(format!("{} ({})", label, p.name.name()))
                .accessibility_label(format!(
                    "{name} 권한 {label}: {verdict}. 누르면 {}",
                    if p.granted { s::DENIED } else { s::ALLOWED }
                ))
                .on_click(move |_, _, cx| {
                    grant(&ws_grant, mod_name.clone(), p.name, !p.granted, cx)
                }),
        );
    }
    card = card.child(perms);
    // What it provides.
    let mut facts = Vec::new();
    if !m.commands.is_empty() {
        facts.push(format!("명령 {}개", m.commands.len()));
    }
    if !m.detectors.is_empty() {
        let names: Vec<String> = m.detectors.iter().flat_map(|d| d.names.clone()).collect();
        facts.push(format!("상태 감지: {}", names.join(", ")));
    }
    if !m.layouts.is_empty() {
        facts.push(format!(
            "레이아웃: {}",
            m.layouts
                .iter()
                .map(|l| l.title.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if m.state == ModState::Running && m.memory_kib > 0 {
        facts.push(format!("메모리 {} KiB", m.memory_kib));
    }
    if !facts.is_empty() {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(facts.join(" · ")),
        );
    }
    if !m.checks.is_empty() {
        let checks: Vec<String> = m
            .checks
            .iter()
            .map(|c| format!("{} = {}", c.name, c.argv.join(" ")))
            .collect();
        card = card.child(div().text_xs().text_color(rgb(t.text_muted)).child(format!(
            "{}: {}",
            s::CHECKS,
            checks.join(" · ")
        )));
    }
    let ws_log = ws.clone();
    let mod_name = m.name.clone();
    card = card.child(
        div().flex().child(
            Button::new(id("log"))
                .label(if log_open { s::HIDE_LOG } else { s::SHOW_LOG })
                .compact()
                .accessibility_label(format!(
                    "{name} {}",
                    if log_open { s::HIDE_LOG } else { s::SHOW_LOG }
                ))
                .on_click(move |_, _, cx| toggle_log(&ws_log, mod_name.clone(), cx)),
        ),
    );
    if log_open {
        let lines = log.cloned().unwrap_or_default();
        card = card.child(
            div()
                .id(id("log-lines"))
                .role(Role::Document)
                .aria_label(format!("{name} {}", s::SHOW_LOG))
                .max_h(px(220.))
                .overflow_y_scroll()
                .p_2()
                .rounded_sm()
                .bg(rgb(t.bg_canvas))
                .font_family("Consolas")
                .text_xs()
                .flex()
                .flex_col()
                .when(lines.is_empty(), |d| d.child(s::LOG_EMPTY))
                .children(lines.into_iter().map(|l| div().child(l))),
        );
    }
    card.into_any_element()
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that glob brings in gpui's `test` attribute, which
    // would shadow the standard one.
    use super::{
        LayoutPreset, Permission, StopReason, click_terminal, permission_label, stop_reason_label,
    };
    use crate::os_notify::Target;

    #[test]
    fn a_collapsed_split_hands_its_size_to_the_remaining_child() {
        let lone = super::Built::Tabs(Vec::new(), Some(0.25), true);
        assert_eq!(lone.with_size(Some(0.6)).size(), Some(0.6));
        assert_eq!(
            super::Built::Tabs(Vec::new(), Some(0.25), true)
                .with_size(None)
                .size(),
            None,
            "an unsized split leaves the child unsized"
        );
    }

    #[test]
    fn toast_click_shows_the_notice_terminal_or_its_task() {
        let terminals = vec!["t1".to_string(), "t2".to_string()];
        let tasks = vec![
            (
                "repo/a".to_string(),
                vec!["gone".to_string(), "t2".to_string()],
            ),
            ("repo/b".to_string(), vec![]),
        ];
        let t = |task: Option<&str>, terminal: Option<&str>| Target {
            task: task.map(Into::into),
            terminal: terminal.map(Into::into),
        };
        assert_eq!(
            click_terminal(&t(Some("repo/a"), Some("t1")), &terminals, &tasks).as_deref(),
            Some("t1"),
            "the notice's terminal first"
        );
        assert_eq!(
            click_terminal(&t(Some("repo/a"), Some("closed")), &terminals, &tasks).as_deref(),
            Some("t2"),
            "closed terminal: the task's first live one"
        );
        assert_eq!(
            click_terminal(&t(Some("repo/a"), None), &terminals, &tasks).as_deref(),
            Some("t2")
        );
        assert_eq!(
            click_terminal(&t(Some("repo/b"), None), &terminals, &tasks),
            None,
            "task without terminals: only bring Pod forward"
        );
        assert_eq!(click_terminal(&t(None, None), &terminals, &tasks), None);
    }

    #[test]
    fn labels_cover_every_permission_and_reason() {
        for p in Permission::ALL {
            assert!(!permission_label(p).is_empty());
        }
        for r in [
            StopReason::Cpu,
            StopReason::Memory,
            StopReason::Errors,
            StopReason::Load,
        ] {
            assert!(!stop_reason_label(Some(r)).is_empty());
        }
    }

    #[test]
    fn example_presets_parse() {
        for f in ["review-focus.json", "agents-quad.json"] {
            let text = std::fs::read_to_string(format!(
                "{}/../../examples/mods/pod-layouts/layouts/{f}",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
            let p = LayoutPreset::parse(&text).unwrap();
            assert!(!p.title.is_empty());
        }
    }
}
