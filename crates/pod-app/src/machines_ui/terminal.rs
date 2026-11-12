//! A dock panel showing one **remote** terminal through
//! [`pod_remote::RemoteAttach`] (docs/17 R4, docs/22 TM-14).
//!
//! - It targets `{machine, machine_id, terminal id}` and attaches only while
//!   the board has that machine connected *and confirmed* with that
//!   `machine_id`; a profile that now reaches another machine leaves the
//!   panel waiting (never attaches to a same-named terminal there).
//! - Input goes through the machine's control thread (`terminal.input_once`)
//!   only while the attach is live and the machine may be changed; otherwise
//!   it is dropped, never queued, and the banner says why. Nothing typed
//!   while disconnected is sent after a reconnect.
//! - A re-attach that skipped output (or met a restarted daemon) draws a
//!   marker strip over the terminal with pod-remote's label; the snapshot
//!   restores the screen, but the marker says that bytes are missing.
//! - VT replies (DA, colors …) are never answered from here: a late reply
//!   over a slow link would land in the program as typed text.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use pod_remote::{AttachOptions, AttachUpdate, RemoteAttach};
use pod_terminal_view::element::hsla_of;
use pod_terminal_view::vt::{AlacrittyVt, GridSize, Modes, RowText, Snapshot, VtEngine, VtEvent};
use pod_terminal_view::{InputSink, TermSize, TerminalConfig, TerminalView};
use podd::protocol::TerminalRef;
use serde_json::json;

use super::board::{MachineBoard, MachinesGlobal, NotReady};
use super::controller::InputPort;
use super::copy;
use super::model::{Blocked, RemoteRef};
use crate::qa_marks::QaId;

pub const REMOTE_PANEL_NAME: &str = "PodRemoteTerminal";
/// Scrollback lines asked for with each snapshot.
const SNAPSHOT_SCROLLBACK: usize = 2000;
/// Updates the forwarder holds for the UI before it stops reading (the
/// attach thread then blocks, the daemon lags the view, and it re-attaches
/// with a gap marker instead of buffering without bound).
const PUMP_CAP_BYTES: usize = 8 << 20;

/// The panel's view of its attach.
#[derive(Clone, Debug, PartialEq)]
pub enum RemoteLink {
    Waiting(NotReady),
    /// All attach slots of the machine are in use.
    NoSlot,
    Attaching,
    Live,
    Down {
        at: SystemTime,
        reason: String,
        attempt: u32,
        delay: Option<Duration>,
    },
    Exited(Option<i32>),
    Failed(String),
}

impl RemoteLink {
    fn is_final(&self) -> bool {
        matches!(self, RemoteLink::Exited(_) | RemoteLink::Failed(_))
    }
}

/// The VT the view runs: alacritty, rebuilt on every resync, replies dropped.
struct RemoteVt {
    inner: AlacrittyVt,
    reset: Rc<Cell<bool>>,
    scrollback: usize,
    cell_px: (u16, u16),
    palette: pod_terminal_view::TerminalPalette,
}

impl VtEngine for RemoteVt {
    fn set_palette(&mut self, palette: pod_terminal_view::TerminalPalette) {
        self.palette = palette.clone();
        self.inner.set_palette(palette);
    }
    fn feed(&mut self, bytes: &[u8]) {
        if self.reset.replace(false) {
            self.inner = AlacrittyVt::new(self.inner.size(), self.scrollback);
            self.inner.set_palette(self.palette.clone());
            self.inner.resize(self.inner.size(), self.cell_px);
        }
        self.inner.feed(bytes)
    }
    fn take_events(&mut self) -> Vec<VtEvent> {
        let mut events = self.inner.take_events();
        events.retain(|e| !matches!(e, VtEvent::Reply(_)));
        events
    }
    fn sync_deadline(&self) -> Option<std::time::Instant> {
        self.inner.sync_deadline()
    }
    fn flush_sync(&mut self) {
        self.inner.flush_sync()
    }
    fn size(&self) -> GridSize {
        self.inner.size()
    }
    fn resize(&mut self, size: GridSize, cell_px: (u16, u16)) {
        self.cell_px = cell_px;
        self.inner.resize(size, cell_px)
    }
    fn modes(&self) -> Modes {
        self.inner.modes()
    }
    fn snapshot(&self) -> Snapshot {
        self.inner.snapshot()
    }
    fn scroll_display(&mut self, delta: i32) {
        self.inner.scroll_display(delta)
    }
    fn scroll_to_bottom(&mut self) {
        self.inner.scroll_to_bottom()
    }
    fn scroll_to_top(&mut self) {
        self.inner.scroll_to_top()
    }
    fn display_offset(&self) -> usize {
        self.inner.display_offset()
    }
    fn history_size(&self) -> usize {
        self.inner.history_size()
    }
    fn max_history(&self) -> usize {
        self.inner.max_history()
    }
    fn row_text(&self, line: i32) -> Option<RowText> {
        self.inner.row_text(line)
    }
}

/// Typed bytes go to the port while there is one; otherwise they are counted
/// and dropped (never kept for later).
struct RemoteSink {
    port: Rc<RefCell<Option<InputPort>>>,
    refused: Rc<Cell<u32>>,
    size: Rc<Cell<Option<(u16, u16)>>>,
}

impl InputSink for RemoteSink {
    fn write(&self, bytes: &[u8]) {
        match &*self.port.borrow() {
            Some(p) => p.send(bytes.to_vec()),
            None => self.refused.set(self.refused.get() + 1),
        }
    }

    fn resize(&self, size: TermSize) {
        self.size.set(Some((size.cols, size.rows)));
        if let Some(p) = &*self.port.borrow() {
            p.resize(size.cols, size.rows);
        }
    }
}

pub struct RemoteTerminalPanel {
    pub target: RemoteRef,
    pub view: Entity<TerminalView>,
    board: WeakEntity<MachineBoard>,
    pub link: RemoteLink,
    /// Gap / restarted-daemon marker over the terminal (until dismissed).
    pub marker: Option<String>,
    pub last_rx: Option<SystemTime>,
    /// Why typing is not sent right now (`None`: it is).
    pub blocked: Option<Blocked>,
    /// Keystrokes dropped because nothing could be sent (tests read it).
    pub refused: Rc<Cell<u32>>,
    port: Rc<RefCell<Option<InputPort>>>,
    reset: Rc<Cell<bool>>,
    size: Rc<Cell<Option<(u16, u16)>>>,
    attach: Option<RemoteAttach>,
    pump: Option<Task<()>>,
    /// The fixture's frozen clock (`--ui-fixture`), else the system clock.
    frozen_now: Option<SystemTime>,
    _subs: Vec<Subscription>,
}

impl RemoteTerminalPanel {
    pub fn new(
        target: RemoteRef,
        board: WeakEntity<MachineBoard>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let port: Rc<RefCell<Option<InputPort>>> = Rc::default();
        let refused = Rc::new(Cell::new(0));
        let size = Rc::new(Cell::new(None));
        let reset = Rc::new(Cell::new(false));
        let sink = RemoteSink {
            port: port.clone(),
            refused: refused.clone(),
            size: size.clone(),
        };
        let settings = cx.global::<crate::theme::Settings>();
        let config = TerminalConfig {
            font_family: settings.terminal_font.clone().into(),
            font_size: settings.terminal_font_size,
            ..TerminalConfig::default()
        };
        let vt = RemoteVt {
            inner: AlacrittyVt::new(GridSize { cols: 80, rows: 24 }, config.scrollback),
            reset: reset.clone(),
            scrollback: config.scrollback,
            cell_px: (8, 16),
            palette: Default::default(),
        };
        let view = cx
            .new(|cx| TerminalView::with_engine(Box::new(vt), Box::new(sink), config, window, cx));
        view.update(cx, |v, cx| {
            v.set_palette(crate::theme::terminal_palette(), cx)
        });
        let mut subs = Vec::new();
        if let Some(b) = board.upgrade() {
            subs.push(cx.observe(&b, |p: &mut Self, _, cx| p.sync(cx)));
        }
        let mut p = Self {
            target,
            view,
            board,
            link: RemoteLink::Waiting(NotReady::Waiting),
            marker: None,
            last_rx: None,
            blocked: Some(Blocked::NotConnected),
            refused,
            port,
            reset,
            size,
            attach: None,
            pump: None,
            frozen_now: None,
            _subs: subs,
        };
        p.sync(cx);
        p
    }

    /// Fixture panels: no board, no attach; [`Self::apply`] feeds them.
    pub fn fixture(
        target: RemoteRef,
        now: SystemTime,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut p = Self::new(target, WeakEntity::new_invalid(), window, cx);
        p.frozen_now = Some(now);
        p
    }

    fn now(&self) -> SystemTime {
        self.frozen_now.unwrap_or_else(SystemTime::now)
    }

    /// Follow the board: attach when the machine is ready, stop when it is
    /// not, and decide whether typing may go out.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(board) = self.board.upgrade() else {
            self.sync_port(None, cx);
            return;
        };
        let parts = board.read(cx).attach_parts(&self.target);
        match parts {
            Err(reason) => {
                if let Some(a) = self.attach.take() {
                    detach(a);
                }
                self.pump = None;
                if !self.link.is_final() {
                    self.link = RemoteLink::Waiting(reason);
                }
            }
            Ok(parts) => {
                if self.attach.is_none() && matches!(self.link, RemoteLink::Waiting(_)) {
                    self.start_attach(parts, cx);
                }
            }
        }
        let port = board.read(cx).input_port(&self.target);
        self.sync_port(Some(port), cx);
        cx.notify();
    }

    /// Start over after the attach ended (exited or refused): the panel was
    /// opened again from the board.
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if self.link.is_final() {
            self.link = RemoteLink::Waiting(NotReady::Waiting);
            self.sync(cx);
        }
    }

    /// Typing goes out only to a live attach of a machine that may be changed.
    fn sync_port(&mut self, port: Option<Result<InputPort, Blocked>>, cx: &mut Context<Self>) {
        let live = self.link == RemoteLink::Live;
        let (next, blocked) = match port {
            Some(Ok(p)) if live => (Some(p), None),
            Some(Ok(_)) | None => (None, Some(Blocked::NotConnected)),
            Some(Err(b)) => (None, Some(b)),
        };
        let blocked = if live {
            blocked
        } else {
            blocked.or(Some(Blocked::NotConnected))
        };
        let sent_size = next.is_some() && self.port.borrow().is_none();
        *self.port.borrow_mut() = next;
        self.blocked = blocked;
        // A live, writable view takes the PTY size (last resize wins, docs/03 §4.1).
        if sent_size && let (Some((c, r)), Some(p)) = (self.size.get(), self.port.borrow().as_ref())
        {
            p.resize(c, r);
        }
        cx.notify();
    }

    fn start_attach(&mut self, parts: super::board::AttachParts, cx: &mut Context<Self>) {
        let opts = AttachOptions {
            target: TerminalRef::id(&self.target.id),
            scrollback: Some(SNAPSHOT_SCROLLBACK),
            size: self.size.get(),
            channel: 64,
        };
        match RemoteAttach::spawn(parts.connector, parts.policy, opts, &parts.slots) {
            None => self.link = RemoteLink::NoSlot,
            Some((attach, rx)) => {
                self.link = RemoteLink::Attaching;
                self.attach = Some(attach);
                self.pump = Some(pump(rx, cx));
            }
        }
    }

    /// One attach update (from the pump, or a fixture/test directly).
    pub fn apply(&mut self, u: AttachUpdate, cx: &mut Context<Self>) {
        match u {
            AttachUpdate::Resync(r) => {
                self.reset.set(true);
                self.view.update(cx, |v, cx| v.feed(&r.snapshot, cx));
                if let Some(m) = r
                    .gap
                    .map(|g| g.label())
                    .or(r.reason.label().map(str::to_string))
                {
                    self.marker = Some(m);
                }
                self.link = RemoteLink::Live;
                self.last_rx = Some(self.now());
            }
            AttachUpdate::Output { data, .. } => {
                self.view.update(cx, |v, cx| v.feed(&data, cx));
                self.last_rx = Some(self.now());
            }
            AttachUpdate::Resized { .. } => {}
            AttachUpdate::Disconnected { at, reason, .. } => {
                self.link = RemoteLink::Down {
                    at,
                    reason,
                    attempt: 0,
                    delay: None,
                };
            }
            AttachUpdate::Reconnecting { attempt, delay } => {
                let now = self.now();
                match &mut self.link {
                    RemoteLink::Down {
                        attempt: a,
                        delay: d,
                        ..
                    } => {
                        *a = attempt;
                        *d = Some(delay);
                    }
                    other => {
                        *other = RemoteLink::Down {
                            at: now,
                            reason: String::new(),
                            attempt,
                            delay: Some(delay),
                        }
                    }
                }
            }
            AttachUpdate::Exited(e) => {
                self.link = RemoteLink::Exited(e.exit_code);
                if let Some(a) = self.attach.take() {
                    detach(a);
                }
            }
            AttachUpdate::Failed { reason } => {
                self.link = RemoteLink::Failed(reason);
                if let Some(a) = self.attach.take() {
                    detach(a);
                }
            }
        }
        let port = self
            .board
            .upgrade()
            .map(|b| b.read(cx).input_port(&self.target));
        self.sync_port(port, cx);
    }

    /// The banner over the terminal: (tone, title, detail), none when live
    /// and writable.
    pub fn banner(&self) -> Option<(pod_ui::Tone, String, Option<String>)> {
        use pod_ui::Tone;
        let last = self
            .last_rx
            .map(clock_text)
            .unwrap_or_else(|| copy::NEVER_RECEIVED.into());
        Some(match &self.link {
            RemoteLink::Live => {
                let b = self.blocked?;
                let title = match b {
                    Blocked::NeedsConfirm => {
                        format!(
                            "{} · {}",
                            copy::STATE_NEEDS_CONFIRM,
                            copy::PANEL_INPUT_BLOCKED
                        )
                    }
                    Blocked::NotConnected => copy::lost_banner(&last),
                };
                (Tone::Warning, title, Some(self.blocked_detail(b.message())))
            }
            RemoteLink::Down {
                reason,
                attempt,
                delay,
                ..
            } => {
                let mut detail = String::new();
                if *attempt > 0 {
                    detail.push_str(&copy::reconnecting(
                        *attempt,
                        delay.map(|d| d.as_secs_f64().round() as u64),
                    ));
                }
                if !reason.is_empty() {
                    if !detail.is_empty() {
                        detail.push_str(" — ");
                    }
                    detail.push_str(reason);
                }
                (
                    Tone::Warning,
                    copy::lost_banner(&last),
                    Some(self.blocked_detail(&detail)).filter(|d| !d.is_empty()),
                )
            }
            RemoteLink::Waiting(NotReady::Waiting) => {
                (Tone::Info, copy::PANEL_WAITING.into(), None)
            }
            RemoteLink::Waiting(NotReady::OtherMachine) => {
                (Tone::Danger, copy::PANEL_OTHER_MACHINE.into(), None)
            }
            RemoteLink::NoSlot => (Tone::Info, copy::PANEL_NO_SLOT.into(), None),
            RemoteLink::Attaching => (Tone::Info, copy::PANEL_ATTACHING.into(), None),
            RemoteLink::Exited(code) => (Tone::Muted, copy::exited(*code), None),
            RemoteLink::Failed(reason) => (Tone::Danger, copy::failed(reason), None),
        })
    }

    /// `detail`, led by how many keystrokes were dropped (not sent, not kept).
    fn blocked_detail(&self, detail: &str) -> String {
        match self.refused.get() {
            0 => detail.to_string(),
            n if detail.is_empty() => copy::dropped_keys(n),
            n => format!("{} — {detail}", copy::dropped_keys(n)),
        }
    }

    /// Tab title: `<machine> · <terminal>`.
    fn title_text(&self) -> SharedString {
        format!("{} · {}", self.target.machine, self.target.id).into()
    }

    pub fn state_for(target: &RemoteRef) -> PanelState {
        let mut s = PanelState::new(REMOTE_PANEL_NAME);
        s.info = PanelInfo::panel(json!({
            "machine": target.machine,
            "machine_id": target.machine_id,
            "terminal": target.id,
        }));
        s
    }

    pub fn target_of_state(state: &PanelState) -> Option<RemoteRef> {
        let PanelInfo::Panel(v) = &state.info else {
            return None;
        };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        Some(RemoteRef {
            machine: s("machine")?,
            machine_id: s("machine_id")?,
            id: s("terminal")?,
        })
    }
}

/// End an attach off the UI thread: its thread may be inside a connect
/// (an ssh handshake) that only its own deadline ends.
fn detach(a: RemoteAttach) {
    std::thread::Builder::new()
        .name("remote-detach".into())
        .spawn(move || a.close())
        .ok();
}

impl Drop for RemoteTerminalPanel {
    fn drop(&mut self) {
        if let Some(a) = self.attach.take() {
            detach(a);
        }
    }
}

/// `HH:MM:SS` local time.
pub fn clock_text(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + local_offset_min() * 60;
    let day = secs.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", day / 3600, day / 60 % 60, day % 60)
}

/// Minutes from UTC to local time (read once).
fn local_offset_min() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::SYSTEMTIME;
            use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
            // SAFETY: both calls only fill the SYSTEMTIME passed in.
            let (mut l, mut u): (SYSTEMTIME, SYSTEMTIME) =
                unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
            unsafe {
                GetLocalTime(&mut l);
                GetSystemTime(&mut u);
            }
            let mins = |h: u16, m: u16| h as i64 * 60 + m as i64;
            let mut d = mins(l.wHour, l.wMinute) - mins(u.wHour, u.wMinute);
            if l.wDay != u.wDay {
                let ahead = (l.wDay > u.wDay && !(u.wDay == 1 && l.wDay >= 28))
                    || (l.wDay == 1 && u.wDay >= 28);
                d += if ahead { 1440 } else { -1440 };
            }
            ((d as f64 / 15.).round() * 15.) as i64
        }
        #[cfg(not(windows))]
        {
            0
        }
    })
}

/// Forward attach updates from the attach thread to the panel: a reader
/// thread fills a bounded queue and wakes one UI task per batch.
fn pump(
    rx: std::sync::mpsc::Receiver<AttachUpdate>,
    cx: &mut Context<RemoteTerminalPanel>,
) -> Task<()> {
    struct Q {
        items: Mutex<(VecDeque<AttachUpdate>, usize)>,
        signaled: AtomicBool,
        closed: AtomicBool,
    }
    let q = Arc::new(Q {
        items: Mutex::new((VecDeque::new(), 0)),
        signaled: AtomicBool::new(false),
        closed: AtomicBool::new(false),
    });
    let (wake, mut woken) = mpsc::unbounded::<()>();
    let tq = q.clone();
    std::thread::Builder::new()
        .name("remote-attach-pump".into())
        .spawn(move || {
            let size = |u: &AttachUpdate| match u {
                AttachUpdate::Output { data, .. } => data.len(),
                AttachUpdate::Resync(r) => r.snapshot.len(),
                _ => 0,
            };
            while let Ok(u) = rx.recv() {
                while tq.items.lock().unwrap().1 > PUMP_CAP_BYTES {
                    // The panel's task is gone (closed or cancelled).
                    if tq.closed.load(Ordering::SeqCst) || wake.is_closed() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                {
                    let mut g = tq.items.lock().unwrap();
                    g.1 += size(&u);
                    g.0.push_back(u);
                }
                if !tq.signaled.swap(true, Ordering::AcqRel) && wake.unbounded_send(()).is_err() {
                    return;
                }
            }
        })
        .expect("spawn pump thread");
    cx.spawn(async move |this, cx| {
        while woken.next().await.is_some() {
            q.signaled.store(false, Ordering::Release);
            let batch: Vec<AttachUpdate> = {
                let mut g = q.items.lock().unwrap();
                g.1 = 0;
                g.0.drain(..).collect()
            };
            let alive = this
                .update(cx, |p, cx| {
                    for u in batch {
                        p.apply(u, cx);
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
        }
        q.closed.store(true, Ordering::SeqCst);
    })
}

impl EventEmitter<PanelEvent> for RemoteTerminalPanel {}

impl Focusable for RemoteTerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).focus_handle(cx)
    }
}

impl Panel for RemoteTerminalPanel {
    fn panel_name(&self) -> &'static str {
        REMOTE_PANEL_NAME
    }

    fn dump(&self, _cx: &App) -> PanelState {
        Self::state_for(&self.target)
    }

    /// Closing the tab detaches; the remote terminal keeps running.
    fn on_removed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(a) = self.attach.take() {
            detach(a);
        }
        self.pump = None;
        *self.port.borrow_mut() = None;
    }
}

impl gpui_kit::component::dock::Panel for RemoteTerminalPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        let mark = match self.link {
            RemoteLink::Live if self.blocked.is_none() => "",
            RemoteLink::Exited(_) | RemoteLink::Failed(_) => "■ ",
            _ => "… ",
        };
        Some(format!("{mark}{}", self.title_text()).into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = crate::theme::tokens();
        let dot = match self.link {
            RemoteLink::Live if self.blocked.is_none() => t.success_fg,
            RemoteLink::Exited(_) => t.text_muted,
            RemoteLink::Failed(_) | RemoteLink::Waiting(NotReady::OtherMachine) => t.danger_fg,
            _ => t.warning_fg,
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .overflow_hidden()
            .child(pod_ui::icon_sized(pod_ui::Icon::Server, 12., t.text_muted))
            .child(
                div()
                    .size(px(7.))
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(rgb(dot)),
            )
            .child(div().truncate().child(self.title_text()))
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for RemoteTerminalPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mode = crate::theme::mode();
        let t = crate::theme::tokens();
        let mut style = StyleRefinement::default();
        style.size.width = Some(relative(1.).into());
        style.size.height = Some(relative(1.).into());
        let banner = self.banner();
        let marker = self.marker.clone();
        let me = cx.entity().downgrade();
        div()
            .id("remote-terminal")
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla_of(t.terminal_bg))
            .when_some(banner, |d, (tone, title, detail)| {
                d.child(
                    div()
                        .id("remote-terminal-banner")
                        .qa_id(|| "remote-terminal-banner".into())
                        .role(Role::Status)
                        .aria_label(SharedString::from(match &detail {
                            Some(x) => format!("{title} — {x}"),
                            None => title.clone(),
                        }))
                        .flex_shrink_0()
                        .child(pod_ui::banner(tone, mode, title, detail.map(Into::into)).text_xs()),
                )
            })
            .when_some(marker, |d, text| {
                d.child(
                    div()
                        .id("remote-terminal-gap")
                        .qa_id(|| "remote-terminal-gap".into())
                        .role(Role::Note)
                        .aria_label(SharedString::from(text.clone()))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(rgb(t.warning_fg))
                        .bg(rgb(t.bg_chrome))
                        .text_xs()
                        .text_color(rgb(t.warning_fg))
                        .child(div().flex_1().min_w_0().child(text))
                        .child(
                            pod_ui::IconButton::new(
                                "remote-terminal-gap-close",
                                pod_ui::Icon::X,
                                copy::PANEL_DISMISS,
                            )
                            .render(mode, move |_, cx| {
                                let _ = me.update(cx, |p, cx| {
                                    p.marker = None;
                                    cx.notify();
                                });
                            }),
                        ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .p_2()
                    .child(self.view.clone().cached(style)),
            )
    }
}

/// Restore a remote panel from the saved dock layout: it waits until its
/// machine is connected and confirmed with the same machine id.
pub fn build_remote_panel(
    ctx: gpui_kit::component::dock::PanelBuildContext,
    window: &mut Window,
    cx: &mut App,
) -> std::sync::Arc<dyn gpui_kit::component::dock::BasePanelView> {
    let target = RemoteTerminalPanel::target_of_state(ctx.state()).unwrap_or(RemoteRef {
        machine: String::new(),
        machine_id: String::new(),
        id: String::new(),
    });
    let board = cx
        .try_global::<MachinesGlobal>()
        .map(|g| g.0.clone())
        .unwrap_or_else(WeakEntity::new_invalid);
    let panel = cx.new(|cx| RemoteTerminalPanel::new(target, board, window, cx));
    gpui_kit::component::dock::panel_handle(panel)
}
