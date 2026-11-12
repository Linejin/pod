//! Remote machines in the app (docs/17 R3/R4, docs/22 TM-13/TM-14).
//!
//! - **Settings › 원격 머신**: SSH machine profiles (`machines.json`, the
//!   same file as `pod machine`): list, add, edit, remove; only the fields
//!   pod-remote accepts, validated by its rules; the statement that a
//!   profile gives the remote account's full user rights.
//! - **Machine board** (sidebar): per machine its connection state
//!   (connected / reconnecting n / lost / failed with ssh diagnostics), the
//!   last received time, its tasks and terminals, connect/disconnect.
//! - **Trust**: the first connection, and any later one whose target, host
//!   key fingerprint (from known_hosts, read-only) or `machine_id` differs
//!   from what the user confirmed, asks again before anything is sent
//!   (`trust.rs`). Pod never accepts a host key or edits known_hosts.
//! - **Remote terminals** in the dock (`terminal.rs`): `RemoteAttach` per
//!   panel, input disabled with a banner while disconnected, gap markers,
//!   no replay of input after a reconnect.
//!
//! Module map: `board` (entity, backend), `controller` (per-machine thread,
//! guarded connector), `model` (view models + reducer), `form` (profile
//! form), `trust` (confirmation record), `host_keys` (ssh-keygen lookup),
//! `view` (settings, board, dialogs), `terminal` (dock panel), `fixture`
//! (`--ui-fixture` scenes), `copy` (Korean text).

mod board;
mod controller;
mod copy;
pub(crate) mod fixture;
mod form;
mod host_keys;
mod model;
mod terminal;
mod trust;
mod view;

#[cfg(test)]
mod fake_remote;
#[cfg(test)]
mod qa;
#[cfg(test)]
mod tests;

use gpui_kit::*;

pub(crate) use board::{MachineBoard, MachinesBackend, MachinesGlobal};
pub(crate) use copy::{CATEGORY, COMMAND_DETAIL};
pub(crate) use model::RemoteRef;
pub(crate) use terminal::{REMOTE_PANEL_NAME, RemoteTerminalPanel, build_remote_panel};
pub(crate) use view::settings_section;

use crate::workspace::Workspace;

/// Index of 원격 머신 in the settings categories (`chrome::settings`).
pub(crate) const SETTINGS_CATEGORY: usize = 3;

impl Workspace {
    /// Open (or focus) the panel of a remote terminal.
    pub(crate) fn open_remote_terminal(
        &mut self,
        target: RemoteRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let existing = self
            .machines
            .read(cx)
            .panels
            .iter()
            .filter_map(|p| p.upgrade())
            .find(|p| p.read(cx).target == target);
        if let Some(p) = existing {
            use gpui_kit::component::dock::PanelId;
            self.dock.update(cx, |d, cx| {
                d.select_panel(PanelId::from(p.entity_id()), window, cx)
            });
            p.update(cx, |p, cx| p.retry(cx));
            p.read(cx).focus_handle(cx).focus(window, cx);
            return;
        }
        let board = self.machines.downgrade();
        let panel = cx.new(|cx| RemoteTerminalPanel::new(target, board, window, cx));
        self.add_remote_panel(panel, window, cx);
    }

    pub(crate) fn add_remote_panel(
        &mut self,
        panel: Entity<RemoteTerminalPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use gpui_kit::component::dock::{DockPlacement, panel_handle};
        let weak = panel.downgrade();
        self.machines.update(cx, |b, _| {
            b.panels.retain(|p| p.upgrade().is_some());
            b.panels.push(weak);
        });
        let focus = panel.read(cx).focus_handle(cx);
        self.dock.update(cx, |d, cx| {
            d.add_panel_view(panel_handle(panel), DockPlacement::Center, None, window, cx)
        });
        focus.focus(window, cx);
        self.touch(cx);
    }

    /// Settings at the 원격 머신 category (palette `machines`).
    pub(crate) fn open_machine_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_settings(window, cx);
        if let Some(p) = self.settings_panel.as_ref().and_then(|p| p.upgrade()) {
            p.update(cx, |p, cx| p.select_category(SETTINGS_CATEGORY, window, cx));
        }
    }

    /// The board asked for a confirmation: open the dialog once.
    pub(crate) fn machine_confirm_requested(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let name = self.machines.update(cx, |b, _| b.pending_confirm.take());
        if let Some(name) = name {
            let board = self.machines.downgrade();
            view::open_trust_dialog(board, name, window, cx);
        }
    }
}
