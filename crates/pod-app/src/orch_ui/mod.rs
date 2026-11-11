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
//! [`model`]; requests, answers and busy marks in [`controller`]; the
//! screens in [`pages`] (center dock: [`board`], [`cleanup`], [`settings`])
//! and [`inbox`] (right dock). Copy is Korean ([`s`]).

mod board;
mod cleanup;
mod controller;
pub(crate) mod fixture;
mod inbox;
pub(crate) mod ko;
pub(crate) mod model;
mod pages;
#[cfg(test)]
mod qa;
pub(crate) mod s;
mod settings;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use gpui_kit::component::button::Button;
use gpui_kit::*;
use podd::protocol::{self as p, CleanupCandidate, OrchSettings};

use crate::workspace::Workspace;
pub(crate) use controller::{
    WorkerVerb, busy_key, on_event, refresh_candidates, refresh_settings, reload_all, reply,
    rewake, set_limit, set_settings, task_verb, with_ws, worker_verb,
};
pub(crate) use inbox::InboxPanel;
use model::RunEntry;
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
