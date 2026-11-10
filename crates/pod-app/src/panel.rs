//! A dock panel showing one podd terminal: `TerminalView` + attach stream + podd sink.
//!
//! Several panels may show the same terminal; each has its own attach
//! connection and VT. Closing a panel only detaches.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::base::dock::{Panel, PanelEvent, PanelInfo, PanelState};
use gpui_kit::component::menu::PopupMenu;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use pod_terminal_view::element::hsla_of;

use pod_terminal_view::vt::{AlacrittyVt, GridSize, Modes, RowText, Snapshot, VtEngine, VtEvent};
use pod_terminal_view::{InputSink, TermSize, TerminalConfig, TerminalEvent, TerminalView};
use serde_json::json;

use crate::attach::{AttachHandle, Chunk};
use crate::source::PoddSource;
use crate::workspace::{Workspace, WorkspaceAction};

pub const PANEL_NAME: &str = "PodTerminal";

/// Sends what the user does to podd: input via `terminal.input`, the grid
/// size via `terminal.resize` (and remembers it for the next attach).
struct PoddSink {
    source: PoddSource,
    id: String,
    size: Rc<Cell<Option<(u16, u16)>>>,
    /// Set by the panel once the attach handle exists.
    on_size: Rc<dyn Fn(u16, u16)>,
}

impl InputSink for PoddSink {
    fn write(&self, bytes: &[u8]) {
        self.source.input(&self.id, bytes.to_vec());
    }

    fn resize(&self, size: TermSize) {
        self.size.set(Some((size.cols, size.rows)));
        (self.on_size)(size.cols, size.rows);
        self.source.resize(&self.id, size.cols, size.rows);
    }
}

/// The VT the view runs, wrapping alacritty with two pod-app concerns:
/// - **reply filtering**: the holder already answers DA/DSR-style queries
///   (docs/03 §3), so the view must not answer them again or the program sees
///   two replies. Color queries (OSC 10/11/4) and CSI 14 t are not answered by
///   the holder, so those still go out, but not through the view's input
///   sink: [`PoddSource::reply`] sends them on the input connection, stamped,
///   so stale and duplicate answers are dropped instead of being typed into
///   the program (E2E B4, [`crate::source::ReplyFilter`]).
/// - **reset**: on re-attach the panel replaces the engine with a fresh one
///   before feeding the new snapshot (no leftover scrollback, modes, colors).
struct AttachVt {
    inner: AlacrittyVt,
    reset: Rc<Cell<bool>>,
    scrollback: usize,
    cell_px: (u16, u16),
    palette: pod_terminal_view::TerminalPalette,
    reply: Box<dyn Fn(Vec<u8>)>,
}

/// Replies podd's holder does not produce (so the view must).
fn view_must_reply(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x1b]") || bytes.last() == Some(&b't')
}

impl VtEngine for AttachVt {
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
        events.retain_mut(|e| match e {
            VtEvent::Reply(b) => {
                if view_must_reply(b) {
                    (self.reply)(std::mem::take(b));
                }
                false
            }
            _ => true,
        });
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

#[derive(Clone, Debug, PartialEq)]
pub enum PanelLink {
    Attaching,
    Live,
    Disconnected,
    Exited { code: Option<i32>, killed: bool },
    Gone,
}

pub struct TerminalPanel {
    pub terminal_id: String,
    pub view: Entity<TerminalView>,
    source: PoddSource,
    workspace: WeakEntity<Workspace>,
    /// Label from the workspace (podd name or app-local rename).
    label: SharedString,
    /// Title set by the program (OSC 0/2).
    program_title: Option<String>,
    /// Which service runs here (docs/10 §D5.11), set by the workspace with the label.
    agent: Option<crate::agent_kind::AgentKind>,
    pub link: PanelLink,
    size: Rc<Cell<Option<(u16, u16)>>>,
    reset: Rc<Cell<bool>>,
    attach: Option<AttachHandle>,
    /// Snapshots/resets received (re-attach counter, shown in debug builds).
    pub attaches: u32,
    /// Folders printed relative paths resolve against (cwd, task worktree),
    /// kept current by the workspace; the view's link filter reads them.
    link_roots: Rc<RefCell<Vec<PathBuf>>>,
    /// Output was skipped and the screen re-attached (shown for a few seconds).
    lagged_note: Option<Task<()>>,
    _pump: Task<()>,
    _subs: Vec<Subscription>,
}

impl TerminalPanel {
    pub fn new(
        source: PoddSource,
        workspace: WeakEntity<Workspace>,
        terminal_id: String,
        label: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let size = Rc::new(Cell::new(None));
        let reset = Rc::new(Cell::new(false));
        let (wake_tx, mut wake_rx) = mpsc::unbounded();
        let attach = AttachHandle::spawn(source.pipe().to_string(), terminal_id.clone(), wake_tx);
        // The sink tells the attach thread the view size so every snapshot fits.
        let size_for_attach: Rc<dyn Fn(u16, u16)> = Rc::new(attach.size_setter());
        let sink = PoddSink {
            source: source.clone(),
            id: terminal_id.clone(),
            size: size.clone(),
            on_size: size_for_attach,
        };
        let settings = cx.global::<crate::theme::Settings>();
        let config = TerminalConfig {
            font_family: settings.terminal_font.clone().into(),
            font_size: settings.terminal_font_size,
            ..TerminalConfig::default()
        };
        let vt = AttachVt {
            inner: AlacrittyVt::new(GridSize { cols: 80, rows: 24 }, config.scrollback),
            reset: reset.clone(),
            scrollback: config.scrollback,
            cell_px: (8, 16),
            palette: Default::default(),
            reply: {
                let (source, id) = (source.clone(), terminal_id.clone());
                Box::new(move |bytes| source.reply(&id, bytes))
            },
        };
        let view = cx
            .new(|cx| TerminalView::with_engine(Box::new(vt), Box::new(sink), config, window, cx));

        view.update(cx, |v, cx| {
            v.set_palette(crate::theme::terminal_palette(), cx)
        });
        let link_roots: Rc<RefCell<Vec<PathBuf>>> = Rc::default();
        {
            let roots = link_roots.clone();
            view.update(cx, |v, _| {
                v.set_link_filter(move |t| crate::term_links::accept(t, &roots.borrow()))
            });
        }

        let mut subs = vec![
            cx.subscribe(&view, |panel: &mut Self, _, ev: &TerminalEvent, cx| {
                if let TerminalEvent::TitleChanged(t) = ev {
                    // Shells set their exe path as the title ("Administrator: C:\...\powershell.exe"): not useful.
                    panel.program_title = t
                        .clone()
                        .filter(|t| !t.trim().is_empty() && !t.contains('\\'));
                    cx.notify();
                }
            }),
        ];
        // Ctrl+click on a link (or the link picker): the workspace opens it.
        subs.push(cx.subscribe_in(
            &view,
            window,
            |panel: &mut Self, _, ev: &TerminalEvent, window, cx| {
                if let TerminalEvent::OpenLink(target) = ev
                    && let Some(ws) = panel.workspace.upgrade()
                {
                    let (id, target) = (panel.terminal_id.clone(), target.clone());
                    ws.update(cx, |w, cx| w.open_terminal_link(&id, target, window, cx));
                }
            },
        ));
        // Size policy (docs/03 §4.1: last resize wins): when two panels show
        // one terminal at different sizes, the focused one takes the PTY size.
        let focus = view.read(cx).focus_handle(cx);
        subs.push(cx.on_focus(&focus, window, |panel: &mut Self, _, cx| {
            panel.claim_size(cx)
        }));

        let pump = cx.spawn(async move |this, cx| {
            while wake_rx.next().await.is_some() {
                let alive = this.update(cx, |panel, cx| panel.drain(cx)).is_ok();
                if !alive {
                    break;
                }
            }
        });

        Self {
            terminal_id,
            view,
            source,
            workspace,
            label,
            program_title: None,
            agent: None,
            link: PanelLink::Attaching,
            size,
            reset,
            attach: Some(attach),
            attaches: 0,
            link_roots,
            lagged_note: None,
            _pump: pump,
            _subs: subs,
        }
    }

    fn drain(&mut self, cx: &mut Context<Self>) {
        let Some(attach) = self.attach.as_ref() else {
            return;
        };
        let chunks = attach.take();
        let mut link = None;
        for chunk in chunks {
            match chunk {
                Chunk::Reset { snapshot, info } => {
                    self.attaches += 1;
                    self.reset.set(true);
                    // The snapshot was rendered for our size (attach cols/rows).
                    let _ = info;
                    self.view.update(cx, |v, cx| v.feed(&snapshot, cx));
                    link = Some(PanelLink::Live);
                }
                Chunk::Output(bytes) => self.view.update(cx, |v, cx| v.feed(&bytes, cx)),
                Chunk::Exited { code, killed } => link = Some(PanelLink::Exited { code, killed }),
                Chunk::Lagged => {
                    self.lagged_note = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(4))
                            .await;
                        let _ = this.update(cx, |p, cx| {
                            p.lagged_note = None;
                            cx.notify();
                        });
                    }));
                    cx.notify();
                }
                Chunk::Disconnected => link = Some(PanelLink::Disconnected),
                Chunk::Gone => link = Some(PanelLink::Gone),
            }
        }
        if let Some(l) = link
            && l != self.link
        {
            self.link = l;
            cx.notify();
        }
    }

    fn claim_size(&mut self, cx: &mut Context<Self>) {
        if let Some(ws) = self.workspace.upgrade() {
            let id = self.terminal_id.clone();
            cx.defer(move |cx| ws.update(cx, |w, cx| w.pane_focused(&id, cx)));
        }
        if let (Some((c, r)), Some(_)) = (self.size.get(), self.attach.as_ref()) {
            self.source.resize(&self.terminal_id, c, r);
        }
    }

    /// Folders for relative file links (the workspace calls this when terminals/tasks change).
    pub fn set_link_roots(&mut self, roots: Vec<PathBuf>, cx: &mut Context<Self>) {
        if *self.link_roots.borrow() != roots {
            *self.link_roots.borrow_mut() = roots;
            self.view.update(cx, |_, cx| cx.notify());
        }
    }

    pub fn set_label(&mut self, label: SharedString, cx: &mut Context<Self>) {
        if self.label != label {
            self.label = label;
            cx.notify();
        }
    }

    pub fn set_agent(&mut self, agent: crate::agent_kind::AgentKind, cx: &mut Context<Self>) {
        if self.agent.as_ref() != Some(&agent) {
            self.agent = Some(agent);
            cx.notify();
        }
    }

    fn title_text(&self) -> SharedString {
        match &self.program_title {
            Some(t) if !t.is_empty() && t.as_str() != self.label.as_ref() => {
                format!("{} · {}", self.label, t).into()
            }
            _ => self.label.clone(),
        }
    }

    /// Persisted form: just the terminal id.
    pub fn state_for(terminal_id: &str) -> PanelState {
        let mut s = PanelState::new(PANEL_NAME);
        s.info = PanelInfo::panel(json!({ "terminal": terminal_id }));
        s
    }

    pub fn terminal_of_state(state: &PanelState) -> Option<String> {
        match &state.info {
            PanelInfo::Panel(v) => v
                .get("terminal")
                .and_then(|t| t.as_str())
                .map(str::to_owned),
            _ => None,
        }
    }

    fn act(&self, action: WorkspaceAction, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.terminal_id.clone();
        let me = cx.entity().downgrade();
        if let Some(ws) = self.workspace.upgrade() {
            ws.update(cx, |w, cx| w.on_action(action, id, Some(me), window, cx));
        }
    }
}

impl EventEmitter<PanelEvent> for TerminalPanel {}

impl Focusable for TerminalPanel {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.view.read(cx).focus_handle(cx)
    }
}

impl Panel for TerminalPanel {
    fn panel_name(&self) -> &'static str {
        PANEL_NAME
    }

    fn dump(&self, _cx: &App) -> PanelState {
        Self::state_for(&self.terminal_id)
    }

    /// Closing a tab detaches; the terminal keeps running in podd.
    fn on_removed(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.attach = None;
        if let Some(ws) = self.workspace.upgrade() {
            let me = cx.entity().downgrade();
            cx.defer(move |cx| ws.update(cx, |w, cx| w.panel_removed(&me, cx)));
        }
    }
}

impl gpui_kit::component::dock::Panel for TerminalPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        // The tab bar draws `tab_name`; mark panels that are not live.
        let mark = match self.link {
            PanelLink::Live => "",
            PanelLink::Attaching | PanelLink::Disconnected => "… ",
            PanelLink::Exited { .. } | PanelLink::Gone => "■ ",
        };
        // The tab's accessible name carries the agent; plain shells keep their label.
        let agent = self
            .agent
            .as_ref()
            .filter(|a| a.is_agent())
            .map(|a| format!(" · {}", a.accessible_name()))
            .unwrap_or_default();
        Some(format!("{mark}{}{agent}", self.title_text()).into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let dot = match self.link {
            PanelLink::Live => rgb(crate::theme::tokens().success_fg),
            PanelLink::Attaching | PanelLink::Disconnected => {
                rgb(crate::theme::tokens().warning_fg)
            }
            PanelLink::Exited { code: Some(0), .. } => rgb(crate::theme::tokens().text_muted),
            PanelLink::Exited { .. } | PanelLink::Gone => rgb(crate::theme::tokens().danger_fg),
        };
        div()
            .flex()
            .items_center()
            .gap_1()
            .min_w_0()
            .overflow_hidden()
            .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(dot))
            .when_some(self.agent.as_ref(), |d, a| {
                d.child(crate::agent_kind::mark_el(
                    &format!("panel-{}", self.terminal_id),
                    a,
                    14.,
                ))
            })
            .child(div().truncate().child(self.title_text()))
    }

    fn dropdown_menu(
        &mut self,
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> PopupMenu {
        let _ = window;
        let entity = cx.entity();
        let item = |menu: PopupMenu, label: &'static str, action: WorkspaceAction| {
            let entity = entity.clone();
            menu.item(
                gpui_kit::component::menu::PopupMenuItem::new(label).on_click(
                    move |_, window, cx| {
                        entity.update(cx, |p, cx| p.act(action, window, cx));
                    },
                ),
            )
        };
        let menu = item(
            menu,
            crate::strings::OPEN_IN_ANOTHER_PANEL,
            WorkspaceAction::Duplicate,
        );
        let menu = item(menu, crate::strings::RENAME_336, WorkspaceAction::Rename);
        let menu = item(
            menu,
            crate::strings::KILL_TERMINAL_112,
            WorkspaceAction::Kill,
        );
        item(
            menu,
            crate::strings::CLOSE_PANEL_DETACH,
            WorkspaceAction::ClosePanel,
        )
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

impl Render for TerminalPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut style = StyleRefinement::default();
        style.size.width = Some(relative(1.).into());
        style.size.height = Some(relative(1.).into());
        let banner = match &self.link {
            PanelLink::Live if self.lagged_note.is_some() => Some((
                crate::strings::TERMINAL_LAGGED.to_string(),
                rgb(crate::theme::tokens().info_fg),
            )),
            PanelLink::Live => None,
            PanelLink::Attaching => Some((
                crate::strings::ATTACHING.to_string(),
                rgb(crate::theme::tokens().warning_fg),
            )),
            PanelLink::Disconnected => Some((
                crate::strings::PODD_DISCONNECTED_REATTACHING.to_string(),
                rgb(crate::theme::tokens().warning_fg),
            )),
            PanelLink::Exited { killed: true, .. } => Some((
                crate::strings::KILLED.to_string(),
                rgb(crate::theme::tokens().danger_fg),
            )),
            PanelLink::Exited { code: Some(c), .. } => Some((
                crate::tr!(EXITED_C; c;),
                rgb(if *c == 0 {
                    crate::theme::tokens().text_muted
                } else {
                    crate::theme::tokens().danger_fg
                }),
            )),
            PanelLink::Exited { code: None, .. } => Some((
                crate::strings::EXITED.to_string(),
                rgb(crate::theme::tokens().danger_fg),
            )),
            PanelLink::Gone => Some((
                crate::strings::TERMINAL_REMOVED.to_string(),
                rgb(crate::theme::tokens().danger_fg),
            )),
        };
        div()
            .size_full()
            .relative()
            .p_2()
            .bg(hsla_of(crate::theme::tokens().terminal_bg))
            .child(self.view.clone().cached(style))
            .when_some(banner, |d, (text, color)| {
                d.child(
                    div()
                        .absolute()
                        .bottom_1()
                        .right_2()
                        .px_2()
                        .rounded_sm()
                        .bg(rgb(crate::theme::tokens().bg_chrome))
                        .text_xs()
                        .text_color(color)
                        .child(text),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: the gpui glob would shadow `#[test]`.
    use super::{PANEL_NAME, TerminalPanel, view_must_reply};

    #[test]
    fn reply_filter() {
        // Holder answers these: the view must stay quiet.
        assert!(!view_must_reply(b"\x1b[?6c"));
        assert!(!view_must_reply(b"\x1b[>0;10;1c"));
        assert!(!view_must_reply(b"\x1b[12;40R"));
        assert!(!view_must_reply(b"\x1b[0n"));
        // Holder does not answer these.
        assert!(view_must_reply(b"\x1b]11;rgb:1616/1717/1a1a\x1b\\"));
        assert!(view_must_reply(b"\x1b[4;390;800t"));
    }

    #[test]
    fn persisted_state_roundtrip() {
        let s = TerminalPanel::state_for("t7");
        assert_eq!(s.panel_name, PANEL_NAME);
        assert_eq!(TerminalPanel::terminal_of_state(&s).as_deref(), Some("t7"));
    }
}
