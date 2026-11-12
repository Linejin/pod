//! The machine board entity: profiles from `machines.json`, one
//! [`MachineHandle`] per machine the user connected, the view models the
//! sidebar and the settings page draw, and the trust record.
//!
//! Nothing connects on its own: the user presses 연결. A first connection
//! (or one whose target, host key or machine differs from the record) ends
//! in "확인 필요" and the workspace opens the confirmation dialog; until the
//! user confirms, no input, resize or attach goes to that machine.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use futures::channel::mpsc::{self, UnboundedSender};
use gpui_kit::*;
use pod_remote::session::Connector;
use pod_remote::{AttachSlots, Machine, MachineStore, ReconnectPolicy};

use super::controller::{Dial, InputPort, MachineHandle, SystemSsh};
use super::form::{self, FormError, ProfileForm};
use super::host_keys::{HostKeys, SshKeygen};
use super::model::{self, Blocked, LinkState, MachineEvent, MachineVm, RemoteRef};
use super::trust::{Trust, TrustStore, evaluate};

/// Where profiles live and how machines are reached.
#[derive(Clone)]
pub struct MachinesBackend {
    /// `machines.json` (pod-remote's file; the trust record sits beside it).
    pub store: PathBuf,
    pub dial: Arc<dyn Dial>,
    pub keys: Arc<dyn HostKeys>,
    pub policy: ReconnectPolicy,
    /// How often a connected machine's lists are fetched.
    pub poll: Duration,
    /// `--ui-fixture` scene: a fixed board, no threads, no files.
    pub fixture: Option<String>,
}

impl std::fmt::Debug for MachinesBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MachinesBackend")
            .field("store", &self.store)
            .field("fixture", &self.fixture)
            .finish_non_exhaustive()
    }
}

impl MachinesBackend {
    /// The real one: the same profiles as `pod machine`, the system ssh.
    pub fn system() -> Self {
        Self {
            store: MachineStore::open_default().path().to_path_buf(),
            dial: Arc::new(SystemSsh),
            keys: Arc::new(SshKeygen),
            policy: ReconnectPolicy::default(),
            poll: Duration::from_secs(5),
            fixture: None,
        }
    }

    /// Visual fixtures: fixed machines, nothing read, written or contacted.
    pub fn fixture(scene: Option<&str>) -> Self {
        Self {
            store: std::env::temp_dir().join("pod-fixture-machines-never-written.json"),
            dial: Arc::new(super::fixture::NoDial),
            keys: Arc::new(super::host_keys::FixedKeys::new(&[])),
            policy: ReconnectPolicy::default(),
            poll: Duration::from_secs(3600),
            fixture: Some(scene.unwrap_or_default().to_string()),
        }
    }

    /// The default of a workspace started without one: the system backend
    /// in the app; tests never read the real profiles.
    pub fn default_for_build() -> Self {
        #[cfg(test)]
        {
            let mut b = Self::fixture(None);
            b.fixture = None;
            b.store = std::env::temp_dir().join(format!(
                "pod-app-test-machines-{}/machines.json",
                std::process::id()
            ));
            b
        }
        #[cfg(not(test))]
        Self::system()
    }

    fn store(&self) -> MachineStore {
        MachineStore::at(self.store.clone())
    }

    fn trust(&self) -> TrustStore {
        TrustStore::beside(&self.store)
    }
}

/// The board for windows and panels restored from the dock layout.
pub struct MachinesGlobal(pub WeakEntity<MachineBoard>);
impl Global for MachinesGlobal {}

/// Why a remote panel may not attach (yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotReady {
    /// Not connected or not confirmed.
    Waiting,
    /// The machine now answering is not the one the panel's terminal is on.
    OtherMachine,
}

/// What a remote panel attaches with.
pub struct AttachParts {
    pub connector: Arc<dyn Connector>,
    pub policy: ReconnectPolicy,
    pub slots: AttachSlots,
}

pub struct MachineBoard {
    backend: MachinesBackend,
    /// The workspace that opens remote panels and dialogs for the board.
    pub workspace: WeakEntity<crate::workspace::Workspace>,
    /// Open remote terminal panels (one per remote terminal).
    pub panels: Vec<WeakEntity<super::terminal::RemoteTerminalPanel>>,
    pub machines: Vec<MachineVm>,
    handles: HashMap<String, MachineHandle>,
    /// `machines.json` could not be read (shown; the file is left alone).
    pub load_error: Option<String>,
    /// A machine waiting for the user's confirmation (the workspace opens
    /// the dialog once and clears it).
    pub pending_confirm: Option<String>,
    events: UnboundedSender<(String, MachineEvent)>,
    _pump: Task<()>,
}

impl MachineBoard {
    pub fn new(
        backend: MachinesBackend,
        workspace: WeakEntity<crate::workspace::Workspace>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (events, mut rx) = mpsc::unbounded::<(String, MachineEvent)>();
        let pump = cx.spawn(async move |this, cx| {
            while let Some((name, ev)) = rx.next().await {
                if this.update(cx, |b, cx| b.on_event(&name, ev, cx)).is_err() {
                    break;
                }
            }
        });
        let mut b = Self {
            backend,
            workspace,
            panels: Vec::new(),
            machines: Vec::new(),
            handles: HashMap::new(),
            load_error: None,
            pending_confirm: None,
            events,
            _pump: pump,
        };
        if let Some(scene) = b.backend.fixture.clone() {
            b.machines = super::fixture::machines(&scene);
        } else {
            b.reload();
        }
        b
    }

    pub fn is_fixture(&self) -> bool {
        self.backend.fixture.is_some()
    }

    pub fn vm(&self, name: &str) -> Option<&MachineVm> {
        self.machines.iter().find(|m| m.name() == name)
    }

    fn vm_mut(&mut self, name: &str) -> Option<&mut MachineVm> {
        self.machines.iter_mut().find(|m| m.name() == name)
    }

    /// Read the profiles again. A changed profile starts over (its old
    /// connection is closed: it may now point at another machine).
    pub fn reload(&mut self) {
        if self.is_fixture() {
            return;
        }
        let loaded = match self.backend.store().load() {
            Ok(v) => {
                self.load_error = None;
                v
            }
            Err(e) => {
                self.load_error = Some(e);
                return;
            }
        };
        let mut next = Vec::with_capacity(loaded.len());
        for m in loaded {
            match self.machines.iter().position(|v| v.profile == m) {
                Some(i) => next.push(self.machines.remove(i)),
                None => {
                    self.handles.remove(&m.name);
                    next.push(MachineVm::new(m));
                }
            }
        }
        for gone in &self.machines {
            self.handles.remove(gone.name());
        }
        self.machines = next;
    }

    pub fn connect(&mut self, name: &str, cx: &mut Context<Self>) {
        if self.is_fixture() {
            return;
        }
        let Some(profile) = self.vm(name).map(|v| v.profile.clone()) else {
            return;
        };
        let b = &self.backend;
        let handle = self.handles.entry(name.to_string()).or_insert_with(|| {
            MachineHandle::spawn(
                profile,
                b.dial.clone(),
                b.keys.clone(),
                b.policy.clone(),
                b.poll,
                self.events.clone(),
            )
        });
        handle.connect();
        if let Some(vm) = self.vm_mut(name) {
            vm.link = LinkState::Connecting;
        }
        cx.notify();
    }

    pub fn disconnect(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(h) = self.handles.get(name) {
            h.disconnect();
        }
        if self.pending_confirm.as_deref() == Some(name) {
            self.pending_confirm = None;
        }
        cx.notify();
    }

    fn on_event(&mut self, name: &str, ev: MachineEvent, cx: &mut Context<Self>) {
        let trust = self.backend.trust();
        let connected = matches!(ev, MachineEvent::Connected { .. });
        let pin = trust.get(name);
        let Some(vm) = self.vm_mut(name) else {
            return;
        };
        model::apply(vm, ev, |id| evaluate(pin.as_ref(), id));
        if connected {
            match (&vm.trust, &vm.identity) {
                (Some(Trust::Confirmed), Some(id)) => {
                    let id = id.clone();
                    if let Some(h) = self.handles.get(name) {
                        h.confirm(&id);
                    }
                }
                (Some(Trust::Needs(_)), _) => self.pending_confirm = Some(name.to_string()),
                _ => {}
            }
        }
        cx.notify();
    }

    /// The user confirmed the shown identity: record it, let mutations and
    /// attaches through.
    pub fn confirm(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let Some(vm) = self.vm(name) else {
            return Ok(());
        };
        let Some(id) = vm.identity.clone() else {
            return Ok(());
        };
        if !self.is_fixture() {
            self.backend
                .trust()
                .put(id.pin(name, crate::orch::now_ms()))?;
        }
        if let Some(vm) = self.vm_mut(name) {
            vm.trust = Some(Trust::Confirmed);
        }
        if let Some(h) = self.handles.get(name) {
            h.confirm(&id);
            // Lists now, not at the next poll.
            h.refresh();
        }
        self.pending_confirm = None;
        cx.notify();
        Ok(())
    }

    /// Add (`original` None) or edit a profile.
    pub fn save_profile(
        &mut self,
        original: Option<&str>,
        f: &ProfileForm,
        cx: &mut Context<Self>,
    ) -> Result<(), FormError> {
        let m: Machine = f.to_machine()?;
        if self.is_fixture() {
            return Ok(());
        }
        let renamed = original.filter(|o| *o != m.name).map(str::to_string);
        form::save(&self.backend.store(), original, m)?;
        if let Some(old) = renamed {
            // The record belongs to the old alias; the new one asks again.
            let _ = self.backend.trust().remove(&old);
        }
        self.reload();
        cx.notify();
        Ok(())
    }

    pub fn remove_profile(&mut self, name: &str, cx: &mut Context<Self>) -> Result<(), String> {
        if self.is_fixture() {
            return Ok(());
        }
        self.handles.remove(name);
        self.backend.store().remove(name)?;
        self.backend.trust().remove(name)?;
        if self.pending_confirm.as_deref() == Some(name) {
            self.pending_confirm = None;
        }
        self.reload();
        cx.notify();
        Ok(())
    }

    /// Where a panel of `target` may send input, if anywhere right now.
    pub fn input_port(&self, target: &RemoteRef) -> Result<InputPort, Blocked> {
        let vm = self.vm(&target.machine).ok_or(Blocked::NotConnected)?;
        vm.can_mutate()?;
        if vm.confirmed_machine_id() != Some(target.machine_id.as_str()) {
            return Err(Blocked::NeedsConfirm);
        }
        let h = self
            .handles
            .get(&target.machine)
            .ok_or(Blocked::NotConnected)?;
        Ok(h.port(&target.id))
    }

    /// What a panel of `target` attaches with, or why not.
    pub fn attach_parts(&self, target: &RemoteRef) -> Result<AttachParts, NotReady> {
        let vm = self.vm(&target.machine).ok_or(NotReady::Waiting)?;
        let Some(confirmed) = vm.confirmed_machine_id() else {
            return Err(NotReady::Waiting);
        };
        if confirmed != target.machine_id {
            return Err(NotReady::OtherMachine);
        }
        if !matches!(vm.link, LinkState::Connected | LinkState::Lost { .. }) {
            return Err(NotReady::Waiting);
        }
        let h = self.handles.get(&target.machine).ok_or(NotReady::Waiting)?;
        Ok(AttachParts {
            connector: h.attach_connector(),
            policy: h.policy.clone(),
            slots: h.slots.clone(),
        })
    }
}
