//! Fan-out groups (P5, docs/09 §8, backend docs/11 §3.2): the New Group
//! dialog (`group.create`) and the Group panel — one card per member task
//! with live status, branch, +/− totals, changed files and check result,
//! the group check (`group.check` / `group.check_cancel`, results from
//! `group.check` events) and Pick winner (`group.pick`).
//!
//! Data: `group.show` (members, check job) on open, after a pick and when a
//! check job ends; `group.compare` (diff stats per member, git runs in podd,
//! seconds for big groups) on open, on ⟳ and when a member's git summary from
//! the regular task poll changes (debounced). Member status comes live from the
//! workspace's terminal list / status stream; nothing here polls on a timer.
//!
//! Layout (A-P4): this file owns the panel's state and creation, its dock
//! identity (`PodGroup`, the `{"group"}` dump) and the workspace's New Group
//! entry point. Agents, validation, check summaries, the members' live status
//! and the display plans are in [`model`]; requests, the compare debounce and
//! the check/pick transitions in [`controller`]; the New Group, Pick winner
//! and force dialogs in [`dialog`]; the header, check line, pick banner and
//! cards in [`view`]; panel-local wording in [`copy`]. Tests: `groups::tests`
//! (pure) and `groups::qa` (the panel and the dialogs in the headless
//! workspace).

use std::collections::HashSet;

use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::component::input::InputState;
use gpui_kit::*;
use podd::protocol::{GroupCompareResult, GroupInfo};
use serde_json::json;

use crate::source::PoddSource;
use crate::workspace::{ToastKind, Workspace};

mod controller;
mod copy;
mod dialog;
mod model;
#[cfg(test)]
mod qa;
#[cfg(test)]
mod tests;
mod view;

use model::PickOutcome;
pub use model::{
    AgentCounts, check_argv, check_line, check_summary, error_text, git_signature, tail_lines,
    validate,
};

pub const GROUP_PANEL_NAME: &str = "PodGroup";

impl Workspace {
    /// New Group: repo, name, base, prompt, agents with counts, optional check
    /// command -> `group.create`, then the Group panel.
    pub fn open_new_group_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.caps.orch {
            self.raise_toast(
                ToastKind::Warning,
                crate::strings::GROUPS_NEED_A_NEWER_PODD,
                crate::strings::PODD_PROTOCOL_V6_GROUP_IS_REQUIRED,
                cx,
            );
            return;
        }
        self.new_group_dialog(window, cx);
    }
}

// ───────────────────────────── Group panel ─────────────────────────────

pub struct GroupPanel {
    pub group_id: String,
    source: PoddSource,
    workspace: WeakEntity<Workspace>,
    focus: FocusHandle,
    info: Option<GroupInfo>,
    show_error: Option<String>,
    compare: Option<GroupCompareResult>,
    compare_error: Option<String>,
    compare_loading: bool,
    compare_again: bool,
    /// Members' git summaries the last compare was started for, and the
    /// latest ones from the workspace (`data_changed`).
    compare_sig: String,
    latest_sig: String,
    compare_timer: Option<Task<()>>,
    check_input: Entity<InputState>,
    /// Text for `check_input`, applied on the next render (needs a window).
    pending_check_text: Option<String>,
    check_busy: bool,
    check_error: Option<String>,
    expanded: HashSet<String>,
    pick: Option<PickOutcome>,
}

impl GroupPanel {
    pub fn new(
        source: PoddSource,
        workspace: WeakEntity<Workspace>,
        group_id: String,
        info: Option<GroupInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let initial = info
            .as_ref()
            .and_then(|g| g.check.as_deref())
            .map(check_line)
            .unwrap_or_default();
        let check_input = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(initial)
                .placeholder(crate::strings::CHECK_COMMAND_E_G_CARGO_TEST_CMD_C_IN_EACH_WORKTREE)
        });
        let mut this = Self {
            group_id,
            source,
            workspace,
            focus: cx.focus_handle(),
            info,
            show_error: None,
            compare: None,
            compare_error: None,
            compare_loading: false,
            compare_again: false,
            compare_sig: String::new(),
            latest_sig: String::new(),
            compare_timer: None,
            check_input,
            pending_check_text: None,
            check_busy: false,
            check_error: None,
            expanded: HashSet::new(),
            pick: None,
        };
        this.reload_show(cx);
        this.reload_compare(cx);
        this
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<PanelEvent> for GroupPanel {}

impl Focusable for GroupPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Panel for GroupPanel {
    fn panel_name(&self) -> &'static str {
        GROUP_PANEL_NAME
    }

    /// Persisted shape; restored at start while the group exists (`layout::prune`).
    fn dump(&self, _cx: &App) -> PanelState {
        let mut s = PanelState::new(GROUP_PANEL_NAME);
        s.info = PanelInfo::panel(json!({ "group": self.group_id }));
        s
    }

    fn on_removed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.compare_timer = None;
        if let Some(ws) = self.workspace.upgrade() {
            cx.defer(move |cx| ws.update(cx, |w, cx| w.orch_panel_removed(cx)));
        }
    }
}

impl gpui_kit::component::dock::Panel for GroupPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some(crate::tr!(GROUP_290; ; self.group_id).into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(crate::tr!(GROUP_290; ; self.group_id))
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}
