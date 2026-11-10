//! "발견된 세션" page (docs/18 §7): agent sessions found on this PC, with
//! resume / register / hide. Discovery runs in podd (`sessions.*`); this
//! module only shows and asks. Uses the existing components and tokens.

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::dock::panel_handle;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use podd::protocol::{
    DiscoveredAgent, DiscoveredSession, SessionState, SessionsHide, SessionsHideParams,
    SessionsList, SessionsListMethod, SessionsListParams, SessionsRegister, SessionsRegisterParams,
    SessionsResume, SessionsResumeParams, SessionsScan, SessionsScanParams, SessionsSettings,
    SessionsSettingsParams,
};

use crate::workspace::{ToastKind, Workspace};

/// Korean copy of the page (kept here; docs/18).
pub(crate) mod s {
    pub const SECTION: &str = "발견된 세션";
    pub const OPEN_PAGE_DETAIL: &str = "이 PC의 Claude Code·Codex 세션 찾기, 재개, 작업으로 등록";
    pub const INTRO: &str = "이 PC에 있는 Claude Code와 Codex 세션을 찾아 Pod에서 이어서 실행할 수 있습니다. 에이전트의 기록 파일은 읽기만 하고 바꾸지 않으며, 찾은 내용은 이 PC의 Pod에만 저장됩니다.";
    pub const OFF: &str = "세션 찾기가 꺼져 있습니다. 켜면 에이전트의 세션 기록(폴더, 시간, 제목, 첫 프롬프트)과 실행 중인 에이전트 프로세스를 읽습니다.";
    pub const ENABLE: &str = "세션 찾기 켜기";
    pub const DISABLE: &str = "세션 찾기 끄기";
    pub const RESCAN: &str = "다시 찾기";
    pub const PREVIEW_ON: &str = "첫 프롬프트 표시: 켜짐";
    pub const PREVIEW_OFF: &str = "첫 프롬프트 표시: 꺼짐";
    pub const SHOW_HIDDEN: &str = "숨긴 세션 보기";
    pub const HIDE_HIDDEN: &str = "숨긴 세션 감추기";
    pub const LOADING: &str = "세션 목록을 불러오는 중…";
    pub const UNAVAILABLE: &str =
        "세션 목록을 불러오지 못했습니다. 데몬이 세션 찾기를 지원하는 버전인지 확인하세요.";
    pub const EMPTY: &str = "찾은 세션이 없습니다.";
    pub const NOT_SCANNED: &str = "아직 찾지 않았습니다. \"다시 찾기\"를 누르세요.";
    pub const TRUNCATED: &str = "한도에 걸려 일부만 읽었습니다. 다음 찾기에서 나머지를 읽습니다.";
    pub const RUNNING_POD: &str = "실행 중 (Pod)";
    pub const RUNNING_OTHER: &str = "실행 중 (다른 터미널)";
    pub const RESUMABLE: &str = "재개 가능";
    pub const STALE: &str = "오래됨";
    pub const OTHER_NOTE: &str = "다른 앱의 터미널에서 실행 중입니다. 그 터미널은 그 앱이 소유하므로 Pod로 옮길 수 없습니다. 세션이 끝나면 여기서 재개할 수 있습니다.";
    pub const NO_FOLDER: &str = "폴더가 없어 재개할 수 없습니다.";
    pub const RESUME: &str = "이 세션 재개";
    pub const OPEN_TERMINAL: &str = "터미널 열기";
    pub const REGISTER_TASK: &str = "작업으로 등록";
    pub const REGISTER_REPO: &str = "저장소 등록";
    pub const HIDE: &str = "숨기기";
    pub const UNHIDE: &str = "다시 보이기";
    pub const LINKED: &str = "연결됨";
    pub const MATCHED: &str = "일치";
    pub const UNKNOWN_FOLDER: &str = "Pod에 등록되지 않은 폴더";
    pub const FAILED: &str = "세션 작업 실패";
    pub const RESUMED: &str = "세션을 재개했습니다";
    pub const REGISTERED: &str = "세션을 등록했습니다";
}

/// What the app knows about discovered sessions.
#[derive(Default)]
pub(crate) struct SessionsUi {
    pub list: Option<SessionsList>,
    pub loading: bool,
    pub scanning: bool,
    pub failed: bool,
    pub show_hidden: bool,
    /// A resumed terminal to open once the workspace lists it.
    pub pending_open: Option<String>,
    pub panel: Option<WeakEntity<SessionsPanel>>,
}

impl Global for SessionsUi {}

fn state(cx: &mut App) -> &mut SessionsUi {
    cx.default_global::<SessionsUi>()
}

pub(crate) fn state_label(s: SessionState) -> &'static str {
    match s {
        SessionState::RunningPod => s::RUNNING_POD,
        SessionState::RunningOther => s::RUNNING_OTHER,
        SessionState::Resumable => s::RESUMABLE,
        SessionState::Stale => s::STALE,
    }
}

fn agent_label(a: DiscoveredAgent) -> &'static str {
    a.label()
}

/// "3분 전" style age of a unix-ms time.
pub(crate) fn ago(ms: Option<u64>, now_ms: u64) -> String {
    let Some(ms) = ms else { return "-".into() };
    let d = now_ms.saturating_sub(ms) / 1000;
    match d {
        0..60 => "방금".into(),
        60..3600 => format!("{}분 전", d / 60),
        3600..86_400 => format!("{}시간 전", d / 3600),
        _ => format!("{}일 전", d / 86_400),
    }
}

/// The card heading: title, else first prompt, else the agent and short id.
pub(crate) fn heading(x: &DiscoveredSession) -> String {
    x.title
        .clone()
        .or_else(|| x.preview.clone())
        .unwrap_or_else(|| {
            format!(
                "{} {}",
                agent_label(x.agent),
                x.id.get(..8).unwrap_or(&x.id)
            )
        })
}

/// Which actions a session offers.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Actions {
    pub open_terminal: Option<String>,
    pub resume: bool,
    /// `Some(false)`: register to the matching task/repo; `Some(true)`: register the repository first.
    pub register: Option<bool>,
}

pub(crate) fn actions(x: &DiscoveredSession) -> Actions {
    Actions {
        open_terminal: x.running.as_ref().and_then(|r| r.pod_terminal.clone()),
        resume: x.running.is_none() && x.cwd_exists,
        register: if x.link.is_some() || !x.cwd_exists {
            None
        } else {
            Some(x.matched.is_empty())
        },
    }
}

fn apply(
    ws: &WeakEntity<Workspace>,
    r: Result<SessionsList, crate::source::SourceError>,
    cx: &mut AsyncApp,
) {
    let _ = ws.update(cx, |w, cx| {
        let st = state(cx);
        st.loading = false;
        st.scanning = false;
        match r {
            Ok(l) => {
                st.list = Some(l);
                st.failed = false;
            }
            Err(e) => {
                st.failed = st.list.is_none();
                w.raise_toast(ToastKind::Error, s::FAILED, e.message, cx);
            }
        }
        cx.notify();
    });
}

/// Re-read `sessions.list`.
pub(crate) fn refresh(ws: &mut Workspace, cx: &mut Context<Workspace>) {
    if std::mem::replace(&mut state(cx).loading, true) {
        return;
    }
    let include_hidden = state(cx).show_hidden;
    let call = ws
        .source
        .call::<SessionsListMethod>(&SessionsListParams { include_hidden });
    let weak = cx.entity().downgrade();
    cx.spawn(async move |_, cx| apply(&weak, call.await, cx))
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

fn scan(ws: &WeakEntity<Workspace>, enable: bool, cx: &mut App) {
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

fn settings(ws: &WeakEntity<Workspace>, p: SessionsSettingsParams, cx: &mut App) {
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

fn toggle_hidden(ws: &WeakEntity<Workspace>, cx: &mut App) {
    let st = state(cx);
    st.show_hidden = !st.show_hidden;
    with_ws(ws, cx, refresh);
}

fn hide(ws: &WeakEntity<Workspace>, key: String, hidden: bool, cx: &mut App) {
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

fn register(ws: &WeakEntity<Workspace>, key: String, add_repo: bool, cx: &mut App) {
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
                        let to = x
                            .link
                            .as_ref()
                            .and_then(|l| l.task.clone().or(l.repo.clone()))
                            .unwrap_or_default();
                        w.raise_toast(
                            ToastKind::Success,
                            s::REGISTERED,
                            format!("{} → {to}", heading(&x)),
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

fn resume(ws: &WeakEntity<Workspace>, key: String, title: String, cx: &mut App) {
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
                            format!("{title} · {}", t.id),
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
fn open_terminal(ws: &WeakEntity<Workspace>, id: String, window: &mut Window, cx: &mut App) {
    if let Some(w) = ws.upgrade() {
        w.update(cx, |w, cx| w.open_terminal(&id, false, window, cx));
    }
}

/// Is the page open (the workspace's empty state must not cover it)?
pub(crate) fn page_open(cx: &App) -> bool {
    cx.try_global::<SessionsUi>()
        .and_then(|s| s.panel.as_ref())
        .is_some_and(|p| p.upgrade().is_some())
}

/// Open (or focus) the page in the center dock.
pub(crate) fn open_page(ws: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    use gpui_kit::component::dock::{DockPlacement, PanelId};
    if let Some(p) = state(cx).panel.as_ref().and_then(|p| p.upgrade()) {
        ws.dock.update(cx, |d, cx| {
            d.select_panel(PanelId::from(p.entity_id()), window, cx)
        });
        let focus = p.read(cx).focus.clone();
        focus.focus(window, cx);
        refresh(ws, cx);
        return;
    }
    let workspace = cx.entity().downgrade();
    let panel = cx.new(|cx| {
        let sub = workspace
            .upgrade()
            .map(|w| cx.observe(&w, |_, _, cx| cx.notify()));
        SessionsPanel {
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

pub(crate) struct SessionsPanel {
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    _sub: Option<Subscription>,
}

impl EventEmitter<gpui_kit::base::dock::PanelEvent> for SessionsPanel {}
impl Focusable for SessionsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
impl gpui_kit::base::dock::Panel for SessionsPanel {
    fn panel_name(&self) -> &'static str {
        "PodSessions"
    }
    fn dump(&self, _: &App) -> gpui_kit::base::dock::PanelState {
        gpui_kit::base::dock::PanelState::new("PodSessions")
    }
    fn on_removed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        cx.defer(|cx| state(cx).panel = None);
    }
}
impl gpui_kit::component::dock::Panel for SessionsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(s::SECTION.into())
    }
    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(s::SECTION)
    }
}
impl Render for SessionsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        // A resumed terminal: show it once the workspace knows it.
        if let Some(id) = cx
            .try_global::<SessionsUi>()
            .and_then(|s| s.pending_open.clone())
        {
            let known = self
                .workspace
                .upgrade()
                .is_some_and(|w| w.read(cx).terminal(&id).is_some());
            if known {
                state(cx).pending_open = None;
                open_terminal(&self.workspace, id, window, cx);
            }
        }
        div()
            .id("sessions-panel")
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .bg(rgb(t.bg_surface))
            .text_color(rgb(t.text_primary))
            .child(page(&self.workspace, cx))
    }
}

fn page(ws: &WeakEntity<Workspace>, cx: &mut App) -> AnyElement {
    let t = crate::theme::tokens();
    let st = state(cx);
    let (list, loading, scanning, failed, show_hidden) = (
        st.list.clone(),
        st.loading,
        st.scanning,
        st.failed,
        st.show_hidden,
    );
    let mut body = div()
        .id("sessions-section")
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
                    .child(if failed && !loading {
                        s::UNAVAILABLE
                    } else {
                        s::LOADING
                    }),
            )
            .into_any_element();
    };
    if !list.enabled {
        let ws_on = ws.clone();
        return body
            .child(div().text_sm().child(s::OFF))
            .child(
                div().flex().child(
                    Button::new("sessions-enable")
                        .label(s::ENABLE)
                        .primary()
                        .loading(scanning)
                        .accessibility_label(s::ENABLE)
                        .on_click(move |_, _, cx| {
                            settings(
                                &ws_on,
                                SessionsSettingsParams {
                                    enabled: Some(true),
                                    preview: None,
                                },
                                cx,
                            )
                        }),
                ),
            )
            .into_any_element();
    }
    let (ws_scan, ws_prev, ws_off, ws_hidden) = (ws.clone(), ws.clone(), ws.clone(), ws.clone());
    let preview = list.preview;
    let mut toolbar = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .child(
            Button::new("sessions-rescan")
                .label(s::RESCAN)
                .compact()
                .loading(scanning)
                .accessibility_label(format!("{} — {}", s::SECTION, s::RESCAN))
                .on_click(move |_, _, cx| scan(&ws_scan, false, cx)),
        )
        .child(
            Button::new("sessions-preview")
                .label(if preview {
                    s::PREVIEW_ON
                } else {
                    s::PREVIEW_OFF
                })
                .compact()
                .when(preview, |b| b.primary())
                .accessibility_label(format!(
                    "{}. 누르면 {}",
                    if preview {
                        s::PREVIEW_ON
                    } else {
                        s::PREVIEW_OFF
                    },
                    if preview { "끕니다" } else { "켭니다" }
                ))
                .on_click(move |_, _, cx| {
                    settings(
                        &ws_prev,
                        SessionsSettingsParams {
                            enabled: None,
                            preview: Some(!preview),
                        },
                        cx,
                    )
                }),
        );
    if list.hidden > 0 || show_hidden {
        toolbar = toolbar.child(
            Button::new("sessions-hidden")
                .label(if show_hidden {
                    s::HIDE_HIDDEN.to_string()
                } else {
                    format!("{} ({})", s::SHOW_HIDDEN, list.hidden)
                })
                .compact()
                .accessibility_label(if show_hidden {
                    s::HIDE_HIDDEN.to_string()
                } else {
                    format!("{} {}개", s::SHOW_HIDDEN, list.hidden)
                })
                .on_click(move |_, _, cx| toggle_hidden(&ws_hidden, cx)),
        );
    }
    toolbar = toolbar.child(div().flex_1()).child(
        Button::new("sessions-disable")
            .label(s::DISABLE)
            .compact()
            .accessibility_label(s::DISABLE)
            .on_click(move |_, _, cx| {
                settings(
                    &ws_off,
                    SessionsSettingsParams {
                        enabled: Some(false),
                        preview: None,
                    },
                    cx,
                )
            }),
    );
    body = body.child(toolbar);
    let now = podd::now_ms();
    let mut notes = Vec::new();
    if let Some(ms) = list.scanned_ms {
        notes.push(format!("마지막으로 찾은 시각: {}", ago(Some(ms), now)));
    }
    notes.push(format!("세션 {}개", list.sessions.len()));
    if !list.unknown_running.is_empty() {
        notes.push(format!(
            "세션을 알 수 없는 실행 중 에이전트 {}개",
            list.unknown_running.len()
        ));
    }
    body = body.child(
        div()
            .text_xs()
            .text_color(rgb(t.text_muted))
            .child(notes.join(" · ")),
    );
    if list.truncated {
        body = body.child(
            div()
                .text_xs()
                .text_color(rgb(t.warning_fg))
                .child(s::TRUNCATED),
        );
    }
    if list.sessions.is_empty() {
        return body
            .child(div().text_sm().child(if list.scanned_ms.is_none() {
                s::NOT_SCANNED
            } else {
                s::EMPTY
            }))
            .into_any_element();
    }
    let mut cards = div()
        .id("sessions-list")
        .role(Role::List)
        .aria_label(s::SECTION)
        .flex()
        .flex_col()
        .gap_2();
    for x in &list.sessions {
        cards = cards.child(card(ws, x, now));
    }
    body.child(cards).into_any_element()
}

fn card(ws: &WeakEntity<Workspace>, x: &DiscoveredSession, now: u64) -> AnyElement {
    let t = crate::theme::tokens();
    let title = heading(x);
    let state = state_label(x.state);
    let (chip_fg, chip_bg) = match x.state {
        SessionState::RunningPod => (t.success_fg, t.success_bg),
        SessionState::RunningOther => (t.warning_fg, t.warning_bg),
        SessionState::Resumable => (t.text_secondary, t.bg_hover),
        SessionState::Stale => (t.text_muted, t.bg_hover),
    };
    let id = |part: &str| SharedString::from(format!("session-{}-{part}", x.key));
    let kind = crate::agent_kind::AgentKind::of_discovered(x.agent);
    let header = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        // The session's tool (docs/10 §D5.11).
        .child(crate::agent_kind::mark_el(
            &format!("session-{}", x.key),
            &kind,
            pod_ui::tokens::ICON_SIZE,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.clone()),
        )
        .child(
            div()
                .px_2()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(chip_fg))
                .bg(rgb(chip_bg))
                .child(state),
        );
    let mut facts = vec![agent_label(x.agent).to_string(), x.cwd.clone()];
    if let Some(b) = &x.branch {
        facts.push(format!("브랜치 {b}"));
    }
    facts.push(format!("마지막 활동 {}", ago(x.last_ms, now)));
    if let Some(n) = x.messages {
        facts.push(format!("메시지 약 {n}개"));
    }
    if let Some(m) = &x.model {
        facts.push(m.clone());
    }
    let link = match (&x.link, x.matched.is_empty()) {
        (Some(l), _) => format!(
            "{}: {}",
            s::LINKED,
            l.task.clone().or(l.repo.clone()).unwrap_or_default()
        ),
        (None, false) => format!(
            "{}: {}",
            s::MATCHED,
            x.matched
                .task
                .clone()
                .or(x.matched.repo.clone())
                .unwrap_or_default()
        ),
        (None, true) => s::UNKNOWN_FOLDER.to_string(),
    };
    let mut card = div()
        .id(id("card"))
        .role(Role::ListItem)
        .aria_label(format!("{title}, {}, {state}", kind.accessible_name()))
        .flex()
        .flex_col()
        .gap_1()
        .p_3()
        .rounded_md()
        .border_1()
        .border_color(rgb(t.border_subtle))
        .bg(rgb(t.bg_raised))
        .when(x.hidden, |d| d.opacity(0.6))
        .child(header);
    if x.title.is_some()
        && let Some(p) = &x.preview
    {
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .truncate()
                .child(p.clone()),
        );
    }
    card = card
        .child(
            div()
                .text_xs()
                .text_color(rgb(t.text_muted))
                .child(facts.join(" · ")),
        )
        .child(div().text_xs().text_color(rgb(t.text_muted)).child(link));
    if x.state == SessionState::RunningOther {
        card = card.child(
            div()
                .id(id("note"))
                .role(Role::Note)
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::OTHER_NOTE),
        );
    } else if !x.cwd_exists {
        card = card.child(
            div()
                .text_sm()
                .text_color(rgb(t.text_secondary))
                .child(s::NO_FOLDER),
        );
    }
    let a = actions(x);
    let mut row = div().flex().flex_wrap().items_center().gap_2();
    if let Some(term) = a.open_terminal {
        let ws = ws.clone();
        row = row.child(
            Button::new(id("open"))
                .label(s::OPEN_TERMINAL)
                .compact()
                .primary()
                .accessibility_label(format!("{title} {}", s::OPEN_TERMINAL))
                .on_click(move |_, window, cx| open_terminal(&ws, term.clone(), window, cx)),
        );
    }
    if a.resume {
        let (ws, key, t2) = (ws.clone(), x.key.clone(), title.clone());
        row = row.child(
            Button::new(id("resume"))
                .label(s::RESUME)
                .compact()
                .primary()
                .tooltip(x.resume_argv.join(" "))
                .accessibility_label(format!("{title} {}", s::RESUME))
                .on_click(move |_, _, cx| resume(&ws, key.clone(), t2.clone(), cx)),
        );
    }
    if let Some(add_repo) = a.register {
        let (ws, key) = (ws.clone(), x.key.clone());
        let label = if add_repo {
            s::REGISTER_REPO
        } else {
            s::REGISTER_TASK
        };
        row = row.child(
            Button::new(id("register"))
                .label(label)
                .compact()
                .tooltip(if add_repo {
                    "이 폴더의 git 저장소를 Pod에 등록하고 세션을 연결합니다"
                } else {
                    "세션을 일치하는 작업·저장소에 연결합니다"
                })
                .accessibility_label(format!("{title} {label}"))
                .on_click(move |_, _, cx| register(&ws, key.clone(), add_repo, cx)),
        );
    }
    let (ws_h, key, hidden) = (ws.clone(), x.key.clone(), x.hidden);
    let label = if hidden { s::UNHIDE } else { s::HIDE };
    row = row.child(
        Button::new(id("hide"))
            .label(label)
            .compact()
            .accessibility_label(format!("{title} {label}"))
            .on_click(move |_, _, cx| hide(&ws_h, key.clone(), !hidden, cx)),
    );
    card.child(row).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // GPUI's glob exports its own `test` attribute; these are plain Rust tests.
    use core::prelude::v1::test;
    use podd::protocol::{CwdMatch, SessionRunning};

    fn session(state: SessionState) -> DiscoveredSession {
        DiscoveredSession {
            key: "claude:0f8b6a3e-1111-4222-8333-944445555666".into(),
            agent: DiscoveredAgent::Claude,
            id: "0f8b6a3e-1111-4222-8333-944445555666".into(),
            cwd: r"C:\src\demo".into(),
            title: None,
            preview: None,
            model: None,
            branch: None,
            version: None,
            origin: None,
            started_ms: None,
            last_ms: None,
            messages: None,
            state,
            running: None,
            matched: CwdMatch::default(),
            link: None,
            hidden: false,
            cwd_exists: true,
            resume_argv: vec![],
        }
    }

    /// The page's cards carry the tool's mark and accessible name (docs/10 §D5.11).
    #[test]
    fn session_cards_show_agent_marks() {
        let mut codex = session(SessionState::Resumable);
        codex.key = "codex:1".into();
        codex.agent = DiscoveredAgent::Codex;
        let list = podd::protocol::SessionsList {
            enabled: true,
            preview: false,
            scanned_ms: Some(1),
            truncated: false,
            sessions: vec![session(SessionState::Resumable), codex],
            hidden: 0,
            unknown_running: vec![],
        };
        let answer = serde_json::to_value(&list).unwrap();
        let mut h = crate::native_qa::Harness::new("sessions-agent-marks", move |b| {
            let answer = answer.clone();
            b.rpc = Some(Box::new(move |method, _| {
                (method == "sessions.list").then(|| Ok(answer.clone()))
            }));
        });
        let ws = h.ws.clone();
        h.cx.update(|window, cx| ws.update(cx, |w, cx| open_page(w, window, cx)));
        h.settle();
        assert!(
            h.bounds(
                "agent-mark:session-claude:0f8b6a3e-1111-4222-8333-944445555666:Claude 에이전트"
            )
            .is_some(),
            "claude session"
        );
        assert!(
            h.bounds("agent-mark:session-codex:1:Codex 에이전트")
                .is_some(),
            "codex session"
        );
    }

    #[test]
    fn actions_follow_state() {
        let x = session(SessionState::Resumable);
        assert_eq!(
            actions(&x),
            Actions {
                open_terminal: None,
                resume: true,
                register: Some(true)
            }
        );
        let mut y = session(SessionState::RunningOther);
        y.running = Some(SessionRunning {
            pid: 5,
            pod_terminal: None,
            guessed: false,
        });
        y.matched = CwdMatch {
            repo: Some("pod".into()),
            task: None,
        };
        assert_eq!(
            actions(&y),
            Actions {
                open_terminal: None,
                resume: false,
                register: Some(false)
            }
        );
        let mut z = session(SessionState::RunningPod);
        z.running = Some(SessionRunning {
            pid: 5,
            pod_terminal: Some("t3".into()),
            guessed: false,
        });
        z.link = Some(CwdMatch {
            repo: Some("pod".into()),
            task: None,
        });
        assert_eq!(
            actions(&z),
            Actions {
                open_terminal: Some("t3".into()),
                resume: false,
                register: None
            }
        );
        let mut gone = session(SessionState::Stale);
        gone.cwd_exists = false;
        assert_eq!(
            actions(&gone),
            Actions {
                open_terminal: None,
                resume: false,
                register: None
            }
        );
    }

    #[test]
    fn headings_and_ages() {
        let mut x = session(SessionState::Resumable);
        assert_eq!(heading(&x), "Claude Code 0f8b6a3e");
        x.preview = Some("첫 프롬프트".into());
        assert_eq!(heading(&x), "첫 프롬프트");
        x.title = Some("제목".into());
        assert_eq!(heading(&x), "제목");
        assert_eq!(ago(Some(1_000), 30_000), "방금");
        assert_eq!(ago(Some(0), 3 * 3600 * 1000), "3시간 전");
        assert_eq!(ago(None, 0), "-");
    }
}
