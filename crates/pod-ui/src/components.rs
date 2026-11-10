use crate::tokens::*;
use crate::{Mode, Tokens};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
/// Lucide 0.468.0 icons bundled with Pod (ISC; see THIRD-PARTY.md). Names are the
/// upstream file names; the inventory is docs/10 §D4.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Accessibility,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Bell,
    BellOff,
    Bot,
    Check,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    ChevronUp,
    ChevronsUpDown,
    Circle,
    CircleCheck,
    CircleDot,
    CircleHelp,
    ClipboardCheck,
    Clock,
    Columns2,
    Copy,
    Ellipsis,
    ExternalLink,
    FileCode,
    FileDiff,
    FileText,
    FolderGit2,
    FolderOpen,
    GitBranch,
    GitCompareArrows,
    GitMerge,
    GitPullRequest,
    Info,
    Keyboard,
    Layers,
    LayoutGrid,
    Link,
    ListFilter,
    ListTree,
    LoaderCircle,
    LockKeyhole,
    Maximize,
    Maximize2,
    MessageCircleQuestion,
    MessageSquare,
    Minimize2,
    Minus,
    Monitor,
    Moon,
    OctagonX,
    Palette,
    PanelLeft,
    PanelRight,
    Pencil,
    Plug,
    Plus,
    RefreshCw,
    RotateCcw,
    Rows2,
    Search,
    Send,
    Server,
    Settings,
    ShieldQuestion,
    Square,
    SquareTerminal,
    Sun,
    Trash2,
    TriangleAlert,
    Type,
    Unplug,
    Users,
    Volume2,
    VolumeX,
    WifiOff,
    WrapText,
    X,
}
impl Icon {
    pub const ALL: [Self; 78] = [
        Self::Accessibility,
        Self::ArrowDown,
        Self::ArrowLeft,
        Self::ArrowRight,
        Self::ArrowUp,
        Self::Bell,
        Self::BellOff,
        Self::Bot,
        Self::Check,
        Self::ChevronDown,
        Self::ChevronLeft,
        Self::ChevronRight,
        Self::ChevronUp,
        Self::ChevronsUpDown,
        Self::Circle,
        Self::CircleCheck,
        Self::CircleDot,
        Self::CircleHelp,
        Self::ClipboardCheck,
        Self::Clock,
        Self::Columns2,
        Self::Copy,
        Self::Ellipsis,
        Self::ExternalLink,
        Self::FileCode,
        Self::FileDiff,
        Self::FileText,
        Self::FolderGit2,
        Self::FolderOpen,
        Self::GitBranch,
        Self::GitCompareArrows,
        Self::GitMerge,
        Self::GitPullRequest,
        Self::Info,
        Self::Keyboard,
        Self::Layers,
        Self::LayoutGrid,
        Self::Link,
        Self::ListFilter,
        Self::ListTree,
        Self::LoaderCircle,
        Self::LockKeyhole,
        Self::Maximize,
        Self::Maximize2,
        Self::MessageCircleQuestion,
        Self::MessageSquare,
        Self::Minimize2,
        Self::Minus,
        Self::Monitor,
        Self::Moon,
        Self::OctagonX,
        Self::Palette,
        Self::PanelLeft,
        Self::PanelRight,
        Self::Pencil,
        Self::Plug,
        Self::Plus,
        Self::RefreshCw,
        Self::RotateCcw,
        Self::Rows2,
        Self::Search,
        Self::Send,
        Self::Server,
        Self::Settings,
        Self::ShieldQuestion,
        Self::Square,
        Self::SquareTerminal,
        Self::Sun,
        Self::Trash2,
        Self::TriangleAlert,
        Self::Type,
        Self::Unplug,
        Self::Users,
        Self::Volume2,
        Self::VolumeX,
        Self::WifiOff,
        Self::WrapText,
        Self::X,
    ];
    /// Upstream SVG as shipped (2px stroke).
    pub fn source(self) -> &'static [u8] {
        match self {
            Self::Accessibility => include_bytes!("../assets/icons/accessibility.svg"),
            Self::ArrowDown => include_bytes!("../assets/icons/arrow-down.svg"),
            Self::ArrowLeft => include_bytes!("../assets/icons/arrow-left.svg"),
            Self::ArrowRight => include_bytes!("../assets/icons/arrow-right.svg"),
            Self::ArrowUp => include_bytes!("../assets/icons/arrow-up.svg"),
            Self::Bell => include_bytes!("../assets/icons/bell.svg"),
            Self::BellOff => include_bytes!("../assets/icons/bell-off.svg"),
            Self::Bot => include_bytes!("../assets/icons/bot.svg"),
            Self::Check => include_bytes!("../assets/icons/check.svg"),
            Self::ChevronDown => include_bytes!("../assets/icons/chevron-down.svg"),
            Self::ChevronLeft => include_bytes!("../assets/icons/chevron-left.svg"),
            Self::ChevronRight => include_bytes!("../assets/icons/chevron-right.svg"),
            Self::ChevronUp => include_bytes!("../assets/icons/chevron-up.svg"),
            Self::ChevronsUpDown => include_bytes!("../assets/icons/chevrons-up-down.svg"),
            Self::Circle => include_bytes!("../assets/icons/circle.svg"),
            Self::CircleCheck => include_bytes!("../assets/icons/circle-check.svg"),
            Self::CircleDot => include_bytes!("../assets/icons/circle-dot.svg"),
            Self::CircleHelp => include_bytes!("../assets/icons/circle-help.svg"),
            Self::ClipboardCheck => include_bytes!("../assets/icons/clipboard-check.svg"),
            Self::Clock => include_bytes!("../assets/icons/clock.svg"),
            Self::Columns2 => include_bytes!("../assets/icons/columns-2.svg"),
            Self::Copy => include_bytes!("../assets/icons/copy.svg"),
            Self::Ellipsis => include_bytes!("../assets/icons/ellipsis.svg"),
            Self::ExternalLink => include_bytes!("../assets/icons/external-link.svg"),
            Self::FileCode => include_bytes!("../assets/icons/file-code.svg"),
            Self::FileDiff => include_bytes!("../assets/icons/file-diff.svg"),
            Self::FileText => include_bytes!("../assets/icons/file-text.svg"),
            Self::FolderGit2 => include_bytes!("../assets/icons/folder-git-2.svg"),
            Self::FolderOpen => include_bytes!("../assets/icons/folder-open.svg"),
            Self::GitBranch => include_bytes!("../assets/icons/git-branch.svg"),
            Self::GitCompareArrows => include_bytes!("../assets/icons/git-compare-arrows.svg"),
            Self::GitMerge => include_bytes!("../assets/icons/git-merge.svg"),
            Self::GitPullRequest => include_bytes!("../assets/icons/git-pull-request.svg"),
            Self::Info => include_bytes!("../assets/icons/info.svg"),
            Self::Keyboard => include_bytes!("../assets/icons/keyboard.svg"),
            Self::Layers => include_bytes!("../assets/icons/layers.svg"),
            Self::LayoutGrid => include_bytes!("../assets/icons/layout-grid.svg"),
            Self::Link => include_bytes!("../assets/icons/link.svg"),
            Self::ListFilter => include_bytes!("../assets/icons/list-filter.svg"),
            Self::ListTree => include_bytes!("../assets/icons/list-tree.svg"),
            Self::LoaderCircle => include_bytes!("../assets/icons/loader-circle.svg"),
            Self::LockKeyhole => include_bytes!("../assets/icons/lock-keyhole.svg"),
            Self::Maximize => include_bytes!("../assets/icons/maximize.svg"),
            Self::Maximize2 => include_bytes!("../assets/icons/maximize-2.svg"),
            Self::MessageCircleQuestion => {
                include_bytes!("../assets/icons/message-circle-question.svg")
            }
            Self::MessageSquare => include_bytes!("../assets/icons/message-square.svg"),
            Self::Minimize2 => include_bytes!("../assets/icons/minimize-2.svg"),
            Self::Minus => include_bytes!("../assets/icons/minus.svg"),
            Self::Monitor => include_bytes!("../assets/icons/monitor.svg"),
            Self::Moon => include_bytes!("../assets/icons/moon.svg"),
            Self::OctagonX => include_bytes!("../assets/icons/octagon-x.svg"),
            Self::Palette => include_bytes!("../assets/icons/palette.svg"),
            Self::PanelLeft => include_bytes!("../assets/icons/panel-left.svg"),
            Self::PanelRight => include_bytes!("../assets/icons/panel-right.svg"),
            Self::Pencil => include_bytes!("../assets/icons/pencil.svg"),
            Self::Plug => include_bytes!("../assets/icons/plug.svg"),
            Self::Plus => include_bytes!("../assets/icons/plus.svg"),
            Self::RefreshCw => include_bytes!("../assets/icons/refresh-cw.svg"),
            Self::RotateCcw => include_bytes!("../assets/icons/rotate-ccw.svg"),
            Self::Rows2 => include_bytes!("../assets/icons/rows-2.svg"),
            Self::Search => include_bytes!("../assets/icons/search.svg"),
            Self::Send => include_bytes!("../assets/icons/send.svg"),
            Self::Server => include_bytes!("../assets/icons/server.svg"),
            Self::Settings => include_bytes!("../assets/icons/settings.svg"),
            Self::ShieldQuestion => include_bytes!("../assets/icons/shield-question.svg"),
            Self::Square => include_bytes!("../assets/icons/square.svg"),
            Self::SquareTerminal => include_bytes!("../assets/icons/square-terminal.svg"),
            Self::Sun => include_bytes!("../assets/icons/sun.svg"),
            Self::Trash2 => include_bytes!("../assets/icons/trash-2.svg"),
            Self::TriangleAlert => include_bytes!("../assets/icons/triangle-alert.svg"),
            Self::Type => include_bytes!("../assets/icons/type.svg"),
            Self::Unplug => include_bytes!("../assets/icons/unplug.svg"),
            Self::Users => include_bytes!("../assets/icons/users.svg"),
            Self::Volume2 => include_bytes!("../assets/icons/volume-2.svg"),
            Self::VolumeX => include_bytes!("../assets/icons/volume-x.svg"),
            Self::WifiOff => include_bytes!("../assets/icons/wifi-off.svg"),
            Self::WrapText => include_bytes!("../assets/icons/wrap-text.svg"),
            Self::X => include_bytes!("../assets/icons/x.svg"),
        }
    }
    /// Lucide file name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Accessibility => "accessibility",
            Self::ArrowDown => "arrow-down",
            Self::ArrowLeft => "arrow-left",
            Self::ArrowRight => "arrow-right",
            Self::ArrowUp => "arrow-up",
            Self::Bell => "bell",
            Self::BellOff => "bell-off",
            Self::Bot => "bot",
            Self::Check => "check",
            Self::ChevronDown => "chevron-down",
            Self::ChevronLeft => "chevron-left",
            Self::ChevronRight => "chevron-right",
            Self::ChevronUp => "chevron-up",
            Self::ChevronsUpDown => "chevrons-up-down",
            Self::Circle => "circle",
            Self::CircleCheck => "circle-check",
            Self::CircleDot => "circle-dot",
            Self::CircleHelp => "circle-help",
            Self::ClipboardCheck => "clipboard-check",
            Self::Clock => "clock",
            Self::Columns2 => "columns-2",
            Self::Copy => "copy",
            Self::Ellipsis => "ellipsis",
            Self::ExternalLink => "external-link",
            Self::FileCode => "file-code",
            Self::FileDiff => "file-diff",
            Self::FileText => "file-text",
            Self::FolderGit2 => "folder-git-2",
            Self::FolderOpen => "folder-open",
            Self::GitBranch => "git-branch",
            Self::GitCompareArrows => "git-compare-arrows",
            Self::GitMerge => "git-merge",
            Self::GitPullRequest => "git-pull-request",
            Self::Info => "info",
            Self::Keyboard => "keyboard",
            Self::Layers => "layers",
            Self::LayoutGrid => "layout-grid",
            Self::Link => "link",
            Self::ListFilter => "list-filter",
            Self::ListTree => "list-tree",
            Self::LoaderCircle => "loader-circle",
            Self::LockKeyhole => "lock-keyhole",
            Self::Maximize => "maximize",
            Self::Maximize2 => "maximize-2",
            Self::MessageCircleQuestion => "message-circle-question",
            Self::MessageSquare => "message-square",
            Self::Minimize2 => "minimize-2",
            Self::Minus => "minus",
            Self::Monitor => "monitor",
            Self::Moon => "moon",
            Self::OctagonX => "octagon-x",
            Self::Palette => "palette",
            Self::PanelLeft => "panel-left",
            Self::PanelRight => "panel-right",
            Self::Pencil => "pencil",
            Self::Plug => "plug",
            Self::Plus => "plus",
            Self::RefreshCw => "refresh-cw",
            Self::RotateCcw => "rotate-ccw",
            Self::Rows2 => "rows-2",
            Self::Search => "search",
            Self::Send => "send",
            Self::Server => "server",
            Self::Settings => "settings",
            Self::ShieldQuestion => "shield-question",
            Self::Square => "square",
            Self::SquareTerminal => "square-terminal",
            Self::Sun => "sun",
            Self::Trash2 => "trash-2",
            Self::TriangleAlert => "triangle-alert",
            Self::Type => "type",
            Self::Unplug => "unplug",
            Self::Users => "users",
            Self::Volume2 => "volume-2",
            Self::VolumeX => "volume-x",
            Self::WifiOff => "wifi-off",
            Self::WrapText => "wrap-text",
            Self::X => "x",
        }
    }
    /// SVG with the active style's stroke width (docs/10 §D5.2 `icon_stroke`), cached.
    pub fn bytes(self) -> &'static [u8] {
        stroked(self, crate::themes::metrics().icon_stroke)
    }
}
#[cfg(test)]
pub(crate) fn stroked_for_test(icon: Icon, width: f32) -> &'static [u8] {
    stroked(icon, width)
}
/// Rewrites `stroke-width="2"`; one leaked copy per (icon, width) pair, bounded by the inventory.
fn stroked(icon: Icon, width: f32) -> &'static [u8] {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    let width = (width.clamp(0.5, 3.) * 4.).round() / 4.;
    if width == 2. {
        return icon.source();
    }
    type Cache = Mutex<HashMap<(Icon, u32), &'static [u8]>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    cache.entry((icon, (width * 4.) as u32)).or_insert_with(|| {
        let svg = String::from_utf8_lossy(icon.source())
            .replace("stroke-width=\"2\"", &format!("stroke-width=\"{width}\""));
        Box::leak(svg.into_bytes().into_boxed_slice())
    })
}
pub fn icon(name: Icon, fg: u32) -> Svg {
    icon_sized(name, ICON_SIZE, fg)
}
/// 14 meta / 16 default / 20 dialog / 32 empty state (docs/10 §D4.4).
pub fn icon_sized(name: Icon, size: f32, fg: u32) -> Svg {
    svg()
        .data(name.bytes())
        .size(px(size))
        .flex_shrink_0()
        .text_color(color(fg))
}
/// Icon button hit area for the active density (28 compact, 36 comfortable and up).
pub fn hit_area() -> f32 {
    if crate::themes::metrics().density.row >= 34. {
        36.
    } else {
        HIT_AREA
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ControlState {
    #[default]
    Ready,
    Disabled,
    Loading,
    Error(String),
}
impl ControlState {
    pub fn interactive(&self) -> bool {
        matches!(self, Self::Ready | Self::Error(_))
    }
}
/// A keyboard-operable icon control. The label is both accessible name and tooltip.
pub struct IconButton {
    pub id: ElementId,
    pub icon: Icon,
    pub label: SharedString,
    pub state: ControlState,
    /// Longer tooltip (e.g. with the shortcut); the label stays the accessible name.
    pub tooltip: Option<SharedString>,
}
impl IconButton {
    pub fn new(id: impl Into<ElementId>, icon: Icon, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon,
            label: label.into(),
            state: ControlState::Ready,
            tooltip: None,
        }
    }
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }
    pub fn render(
        self,
        mode: Mode,
        on_activate: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let t = Tokens::for_mode(mode);
        let enabled = self.state.interactive();
        let action = std::rc::Rc::new(on_activate);
        let key = action.clone();
        let label = self.tooltip.clone().unwrap_or_else(|| self.label.clone());
        div()
            .id(self.id)
            .role(Role::Button)
            .aria_label(self.label)
            .tab_index(if enabled { 0 } else { -1 })
            .size(px(hit_area()))
            .rounded(px(crate::themes::metrics().radius.control))
            .border_2()
            .border_color(gpui_kit::transparent_black())
            .flex()
            .items_center()
            .justify_center()
            .text_color(color(if enabled {
                t.text_secondary
            } else {
                t.text_disabled
            }))
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(color(t.bg_hover)))
                    .active(|s| s.bg(color(t.bg_pressed)))
                    .focus_visible(|s| focus_style(s, mode))
            })
            .tooltip(move |window, cx| {
                gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx)
            })
            .child(icon(
                if self.state == ControlState::Loading {
                    Icon::LoaderCircle
                } else {
                    self.icon
                },
                if enabled {
                    t.text_secondary
                } else {
                    t.text_disabled
                },
            ))
            .on_click(move |_, w, cx| {
                if enabled {
                    action(w, cx);
                }
            })
            .on_key_down(move |e, w, cx| {
                if enabled && matches!(e.keystroke.key.as_str(), "enter" | "space") {
                    key(w, cx);
                    cx.stop_propagation();
                }
            })
    }
}
/// Presentation row; host owns stable selection, expanded state and tree navigation.
pub struct TreeRow {
    pub id: ElementId,
    pub label: SharedString,
    pub depth: usize,
    pub selected: bool,
    pub state: ControlState,
    pub badge: Option<crate::StatusBadge>,
    /// Node type icon (task, terminal, group). The tree, not the row, owns the one chevron.
    pub glyph: Option<Icon>,
    /// Agent mark element (`agent_mark_el`, named `Claude 에이전트`); replaces `glyph` (docs/10 §D5.11).
    pub mark: Option<AnyElement>,
}
impl TreeRow {
    pub fn render(
        self,
        mode: Mode,
        on_open: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Stateful<Div> {
        let t = Tokens::for_mode(mode);
        let enabled = self.state.interactive();
        let callback = std::rc::Rc::new(on_open);
        let key = callback.clone();
        let label = self.label.clone();
        div()
            .id(self.id)
            .role(Role::TreeItem)
            .aria_label(self.label.clone())
            .aria_selected(self.selected)
            .tab_index(if enabled { 0 } else { -1 })
            .h(px(crate::themes::metrics().density.row))
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(8.))
            .pl(px(8. + self.depth as f32 * TREE_INDENT))
            .pr(px(8.))
            .border_l_2()
            .border_color(color(if self.selected {
                t.accent_fg
            } else {
                t.bg_surface
            }))
            .bg(color(if self.selected {
                t.bg_selected
            } else {
                t.bg_surface
            }))
            .text_size(px(13.))
            .text_color(color(if enabled {
                t.text_primary
            } else {
                t.text_disabled
            }))
            .when(enabled, |d| {
                d.hover(|s| s.bg(color(t.bg_hover)))
                    .focus_visible(|s| focus_style(s, mode))
            })
            .when_some(self.glyph.filter(|_| self.mark.is_none()), |d, g| {
                d.child(icon(g, t.text_secondary))
            })
            .when_some(self.mark, |d, m| d.child(m))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(self.label),
            )
            .when_some(self.badge, |d, b| d.child(b.render_compact(mode)))
            .tooltip(move |w, cx| {
                gpui_kit::component::tooltip::Tooltip::new(label.clone())
                    .mt(px(28.))
                    .build(w, cx)
            })
            .on_click(move |_, w, cx| {
                if enabled {
                    callback(w, cx);
                }
            })
            .on_key_down(move |e, w, cx| {
                if enabled && e.keystroke.key == "enter" {
                    key(w, cx);
                    cx.stop_propagation();
                }
            })
    }
}
/// Small deterministic focus ownership stack. Closing a nested owner returns to its opener.
#[derive(Debug)]
pub struct FocusStack<T> {
    stack: Vec<T>,
}
impl<T> Default for FocusStack<T> {
    fn default() -> Self {
        Self { stack: Vec::new() }
    }
}
impl<T> FocusStack<T> {
    pub fn enter(&mut self, previous: T) {
        self.stack.push(previous);
    }
    pub fn leave(&mut self) -> Option<T> {
        self.stack.pop()
    }
    pub fn depth(&self) -> usize {
        self.stack.len()
    }
}
#[derive(Default)]
pub struct FocusReturn {
    stack: FocusStack<FocusHandle>,
}
impl FocusReturn {
    pub fn enter(&mut self, target: &FocusHandle, window: &mut Window, cx: &mut App) {
        if let Some(previous) = window.focused(cx) {
            self.stack.enter(previous);
        }
        target.focus(window, cx);
    }
    pub fn leave(&mut self, window: &mut Window, cx: &mut App) {
        if let Some(previous) = self.stack.leave() {
            previous.focus(window, cx);
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    Dialog,
    CommandPalette,
    List,
    Toast,
    Empty,
}
/// Common shell; content owns its input entities, validation and actions.
/// Modal frames require a focus handle and restore callback; Tab stays within supplied controls.
pub struct Frame {
    pub id: ElementId,
    pub kind: FrameKind,
    pub title: SharedString,
    pub description: SharedString,
    pub state: ControlState,
    /// 32px icon above an empty state's title (docs/10 §D4.6).
    pub icon: Option<Icon>,
}
impl Frame {
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }
    pub fn new(
        id: impl Into<ElementId>,
        kind: FrameKind,
        title: impl Into<SharedString>,
        description: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            title: title.into(),
            description: description.into(),
            state: ControlState::Ready,
            icon: None,
        }
    }
    pub fn render(
        self,
        mode: Mode,
        children: impl IntoIterator<Item = AnyElement>,
    ) -> Stateful<Div> {
        let t = Tokens::for_mode(mode);
        let enabled = self.state.interactive();
        if self.kind == FrameKind::Empty {
            return self.render_empty(t, children);
        }
        let elevated = matches!(
            self.kind,
            FrameKind::Dialog | FrameKind::CommandPalette | FrameKind::Toast
        );
        let max_width = match self.kind {
            FrameKind::Dialog => 560.,
            FrameKind::CommandPalette => 640.,
            FrameKind::Toast => 360.,
            FrameKind::Empty => 420.,
            FrameKind::List => 720.,
        };
        let mut d = div()
            .id(self.id)
            .w_full()
            .max_w(px(max_width))
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(if self.kind == FrameKind::Dialog {
                24.
            } else {
                16.
            }))
            .rounded(px(if elevated { 12. } else { 8. }))
            .border_1()
            .border_color(color(t.border_subtle))
            .bg(color(if elevated { t.bg_raised } else { t.bg_surface }))
            .text_color(color(t.text_primary))
            .text_size(px(13.))
            .line_height(px(20.))
            .when(elevated, |d| d.shadow(elevation(2, mode)))
            .child(
                div()
                    .text_size(px(
                        if self.kind == FrameKind::Empty || self.kind == FrameKind::Dialog {
                            20.
                        } else {
                            15.
                        },
                    ))
                    .child(self.title),
            )
            .child(
                div()
                    .text_color(color(t.text_muted))
                    .child(self.description),
            );
        d = match self.state {
            ControlState::Loading => d.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(icon(Icon::LoaderCircle, t.info_fg))
                    .child("불러오는 중…"),
            ),
            ControlState::Error(message) => d.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .text_color(color(t.danger_fg))
                    .child(icon(Icon::OctagonX, t.danger_fg))
                    .child(message),
            ),
            ControlState::Disabled => d.child("사용할 수 없음"),
            ControlState::Ready => d,
        };
        d.children(children).when(!enabled, |d| {
            d.capture_any_mouse_down(|_, w, cx| {
                w.prevent_default();
                cx.stop_propagation();
            })
            .capture_key_down(|e, w, cx| {
                if e.keystroke.key != "tab" {
                    w.prevent_default();
                    cx.stop_propagation();
                }
            })
        })
    }
}
impl Frame {
    /// Empty state (docs/10 §D4.6): no outline, max 360, icon 32, title 18/26 as a
    /// heading, description 13/20 wrapped between words; the caller adds one CTA.
    fn render_empty(
        self,
        t: Tokens,
        children: impl IntoIterator<Item = AnyElement>,
    ) -> Stateful<Div> {
        let title = self.title.clone();
        div()
            .id(self.id)
            .w_full()
            .max_w(px(360.))
            .min_w_0()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .p(px(16.))
            .text_color(color(t.text_primary))
            .text_size(px(13.))
            .line_height(px(20.))
            .when_some(self.icon, |d, i| d.child(icon_sized(i, 32., t.text_muted)))
            .child(
                keep_all(&self.title)
                    .id("frame-title")
                    .role(Role::Heading)
                    .aria_level(1)
                    .aria_label(title)
                    .text_size(px(18.))
                    .line_height(px(26.))
                    .font_weight(FontWeight::SEMIBOLD),
            )
            .child(keep_all(&self.description).text_color(color(t.text_muted)))
            .children(children)
    }
}

/// Korean-friendly wrapping (CSS `word-break: keep-all`): lines break only between
/// words (spaces), never inside a word as GPUI's per-character CJK wrapping does.
/// Centered rows of unbreakable words; the text itself is unchanged.
pub fn keep_all(text: &str) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .justify_center()
        .gap_x(px(4.))
        .children(keep_all_words(text).into_iter().map(|w| {
            div()
                .whitespace_nowrap()
                .child(SharedString::from(w.to_string()))
        }))
}
/// [`keep_all`] left-aligned (banners, descriptions). A piece longer than [`KEEP_ALL_MAX`]
/// characters (a path, a URL) may still break so it never overflows its box.
pub fn keep_all_start(text: &str) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_wrap()
        .gap_x(px(4.))
        .children(keep_all_words(text).into_iter().map(|w| {
            let piece = div()
                .max_w_full()
                .min_w_0()
                .child(SharedString::from(w.to_string()));
            if keep_all_unbreakable(w) {
                piece.whitespace_nowrap()
            } else {
                piece
            }
        }))
}
/// Longest piece kept on one line by [`keep_all_start`].
pub const KEEP_ALL_MAX: usize = 24;
/// Whether [`keep_all_start`] keeps this piece unbroken.
pub fn keep_all_unbreakable(word: &str) -> bool {
    word.chars().count() <= KEEP_ALL_MAX
}
/// The unbreakable pieces of [`keep_all`].
pub fn keep_all_words(text: &str) -> Vec<&str> {
    text.split_whitespace().collect()
}

gpui_kit::actions!(pod_modal, [ModalNext, ModalPrevious, ModalClose]);
pub fn install_key_bindings(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("tab", ModalNext, Some("PodModal")),
        KeyBinding::new("shift-tab", ModalPrevious, Some("PodModal")),
        KeyBinding::new("escape", ModalClose, Some("PodModal")),
    ]);
}
/// Attach to the modal frame after render. Host passes only enabled tab stops.
pub fn modal_keyboard(
    frame: Stateful<Div>,
    owner: &FocusHandle,
    stops: Vec<FocusHandle>,
    close: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let previous = stops.clone();
    frame
        .track_focus(owner)
        .key_context("PodModal")
        .role(Role::Dialog)
        .on_action(move |_: &ModalNext, w, cx| {
            if !stops.is_empty() {
                let i = stops.iter().position(|h| h.is_focused(w));
                stops[next_focus(i, stops.len(), false)].focus(w, cx);
            }
            cx.stop_propagation();
        })
        .on_action(move |_: &ModalPrevious, w, cx| {
            if !previous.is_empty() {
                let i = previous.iter().position(|h| h.is_focused(w));
                previous[next_focus(i, previous.len(), true)].focus(w, cx);
            }
            cx.stop_propagation();
        })
        .on_action(move |_: &ModalClose, w, cx| {
            close(w, cx);
            cx.stop_propagation();
        })
}
pub fn next_focus(current: Option<usize>, len: usize, backwards: bool) -> usize {
    if len == 0 {
        return 0;
    }
    match current {
        None => {
            if backwards {
                len - 1
            } else {
                0
            }
        }
        Some(i) => {
            if backwards {
                (i + len - 1) % len
            } else {
                (i + 1) % len
            }
        }
    }
}
/// Toast timing never resolves the underlying domain event. Error/blocked persist.
pub fn toast_timeout(
    tone: crate::Tone,
    hovered: bool,
    focused: bool,
) -> Option<std::time::Duration> {
    if hovered || focused {
        return None;
    }
    match tone {
        crate::Tone::Success => Some(std::time::Duration::from_secs(5)),
        crate::Tone::Info => Some(std::time::Duration::from_secs(7)),
        _ => None,
    }
}

/// Stable-ID list navigation; callers filter IDs without resetting a surviving selection.
#[derive(Clone, Debug, Default)]
pub struct ListSelection {
    pub ids: Vec<String>,
    pub selected: Option<String>,
}
impl ListSelection {
    pub fn replace(&mut self, ids: Vec<String>) {
        self.ids = ids;
        if !self
            .selected
            .as_ref()
            .is_some_and(|id| self.ids.contains(id))
        {
            self.selected = self.ids.first().cloned();
        }
    }
    pub fn navigate(&mut self, backwards: bool) {
        if self.ids.is_empty() {
            self.selected = None;
            return;
        }
        let i = self
            .selected
            .as_ref()
            .and_then(|id| self.ids.iter().position(|x| x == id));
        self.selected = Some(self.ids[next_focus(i, self.ids.len(), backwards)].clone());
    }
    /// Never turn IME composition confirmation into command execution.
    pub fn activation(&self, composing: bool, state: &ControlState) -> Option<&str> {
        if composing || !state.interactive() {
            None
        } else {
            self.selected.as_deref()
        }
    }
}

/// A 2px (style `focus_width`; 3 in 14 접근성) focus ring separated from the control by a 2px gap.
pub fn focus_style(style: StyleRefinement, mode: Mode) -> StyleRefinement {
    let t = Tokens::for_mode(mode);
    let ring = crate::themes::metrics().focus_width.max(FOCUS_RING);
    style.shadow(vec![
        BoxShadow {
            inset: false,
            color: color(t.focus_ring),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(FOCUS_OFFSET + ring),
        },
        BoxShadow {
            inset: false,
            color: color(t.bg_surface),
            offset: point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(FOCUS_OFFSET),
        },
    ])
}

/// Visible text of a [`semantic_button`] while it runs (Korean UI; the accessible name stays the label).
pub const BUSY_LABEL: &str = "처리 중…";
fn button_text(label: SharedString, state: &ControlState) -> SharedString {
    if *state == ControlState::Loading {
        BUSY_LABEL.into()
    } else {
        label
    }
}

/// Semantic action control: hover adds a border and press adds an inset, not color alone.
/// Status indicators must use StatusBadge instead.
pub fn semantic_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    tone: crate::Tone,
    mode: Mode,
    state: ControlState,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Tokens::for_mode(mode);
    let (fg, bg) = tone.colors(t);
    let label = label.into();
    let enabled = state.interactive();
    let activate = std::rc::Rc::new(activate);
    let key = activate.clone();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .tab_index(if enabled { 0 } else { -1 })
        .h(px(HEIGHTS[1]))
        .px(px(SPACE[3]))
        .rounded(px(RADII[2]))
        .flex()
        .items_center()
        .border_1()
        .border_color(color(bg))
        .bg(color(bg))
        .text_color(color(if enabled { fg } else { t.text_disabled }))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.border_color(color(fg)))
                .active(|s| {
                    s.shadow(vec![BoxShadow {
                        inset: true,
                        color: color(fg),
                        offset: point(px(0.), px(1.)),
                        blur_radius: px(0.),
                        spread_radius: px(1.),
                    }])
                })
                .focus_visible(|s| focus_style(s, mode))
        })
        .child(button_text(label, &state))
        .on_click(move |_, w, cx| {
            if enabled {
                activate(w, cx);
            }
        })
        .on_key_down(move |e, w, cx| {
            if enabled && matches!(e.keystroke.key.as_str(), "enter" | "space") {
                key(w, cx);
                cx.stop_propagation();
            }
        })
}

/// Icon (14px) + text in one colour: replaces the old text glyphs (✓ ✕ ◷ ● ★ ⟳) in labels.
pub fn icon_text(name: Icon, fg: u32, text: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .min_w_0()
        .text_color(color(fg))
        .child(icon_sized(name, 14., fg))
        .child(text.into())
}
/// 8px task status dot (docs/10 §D4.5): never the only carrier of meaning; pair it with a count or label.
pub fn status_dot(fg: u32) -> Div {
    div()
        .size(px(8.))
        .flex_shrink_0()
        .rounded_full()
        .bg(color(fg))
}
/// Small count badge (응답 수 · 실패 수) for task rows.
pub fn count_badge(n: usize, fg: u32, bg: u32) -> Div {
    div()
        .flex_shrink_0()
        .min_w(px(18.))
        .h(px(18.))
        .px(px(5.))
        .rounded(px(9.))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(color(fg))
        .bg(color(bg))
        .child(n.to_string())
}

/// Inline error / warning / info banner (docs/10 §D4.6): icon + title + detail; callers add an action.
pub fn banner(
    tone: crate::Tone,
    mode: Mode,
    title: impl Into<SharedString>,
    detail: Option<SharedString>,
) -> Div {
    let t = Tokens::for_mode(mode);
    let (fg, bg) = tone.colors(t);
    let glyph = match tone {
        crate::Tone::Danger => Icon::OctagonX,
        crate::Tone::Warning => Icon::TriangleAlert,
        crate::Tone::Success => Icon::CircleCheck,
        _ => Icon::Info,
    };
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(12.))
        .py(px(6.))
        .min_w_0()
        .rounded(px(crate::themes::metrics().radius.control))
        .bg(color(bg))
        .text_color(color(t.text_primary))
        .child(icon(glyph, fg))
        .child(
            // Text column wraps; an action the caller appends keeps its size.
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                // Word-boundary wrapping (keep-all): `작업 폴더가 없습니다` never splits inside a word.
                .child(
                    keep_all_start(&Into::<SharedString>::into(title))
                        .font_weight(FontWeight::SEMIBOLD),
                )
                .children(detail.map(|d| keep_all_start(&d).text_color(color(t.text_secondary)))),
        )
}

/// One option of a segmented control: neutral raised face + 600 weight when selected (no filled accent).
/// Exposed as a radio button whose checked state is the selection (UIA SelectionItem.IsSelected),
/// so the current choice is never carried by colour alone (docs/10 §4.1).
pub fn segment(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    glyph: Option<Icon>,
    selected: bool,
    mode: Mode,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let label = label.into();
    choice(
        id,
        label.clone(),
        label,
        glyph,
        selected,
        true,
        mode,
        activate,
    )
}

/// [`segment`] with its own accessible name (e.g. `밀도: 고밀도`) and an enabled flag.
#[allow(clippy::too_many_arguments)]
pub fn choice(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    name: impl Into<SharedString>,
    glyph: Option<Icon>,
    selected: bool,
    enabled: bool,
    mode: Mode,
    activate: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let t = Tokens::for_mode(mode);
    let activate = std::rc::Rc::new(activate);
    let key = activate.clone();
    let fg = if !enabled {
        t.text_disabled
    } else if selected {
        t.text_primary
    } else {
        t.text_secondary
    };
    div()
        .id(id)
        .role(Role::RadioButton)
        .aria_label(name.into())
        .aria_toggled(if selected {
            Toggled::True
        } else {
            Toggled::False
        })
        .tab_index(if enabled { 0 } else { -1 })
        .h(px(24.))
        .px(px(8.))
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(6.))
        .rounded(px(crate::themes::metrics().radius.control))
        .text_size(px(12.))
        .whitespace_nowrap()
        .text_color(color(fg))
        .border_1()
        .border_color(color(if selected {
            t.border_subtle
        } else {
            t.bg_chrome
        }))
        .when(selected, |d| {
            d.bg(color(t.bg_raised)).font_weight(FontWeight::SEMIBOLD)
        })
        .when(enabled && !selected, |d| {
            d.cursor_pointer().hover(|s| s.bg(color(t.bg_hover)))
        })
        .when(enabled, |d| d.focus_visible(|s| focus_style(s, mode)))
        .when_some(glyph, |d, g| d.child(icon_sized(g, 14., fg)))
        .child(label.into())
        .on_click(move |_, w, cx| {
            if enabled {
                activate(w, cx);
            }
        })
        .on_key_down(move |e, w, cx| {
            if enabled && matches!(e.keystroke.key.as_str(), "enter" | "space") {
                key(w, cx);
                cx.stop_propagation();
            }
        })
}
/// Container for [`segment`]s.
pub fn segmented(mode: Mode) -> Div {
    let t = Tokens::for_mode(mode);
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(crate::themes::metrics().radius.control + 2.))
        .bg(color(t.bg_chrome))
}
/// [`segmented`] as a named radio group (`테마`, `밀도` …); wraps when narrow.
pub fn radio_group(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    mode: Mode,
) -> Stateful<Div> {
    segmented(mode)
        .id(id)
        .role(Role::RadioGroup)
        .aria_label(name.into())
        .flex_wrap()
        .min_w_0()
}

/// A bundled icon as a gpui-component icon (for `Button::icon`), with the style's stroke.
pub fn kit_icon(name: Icon) -> gpui_kit::component::Icon {
    gpui_kit::component::Icon::default().data(name.bytes())
}

#[cfg(test)]
mod busy_tests {
    use super::*;
    // GPUI's glob exports its own `test` attribute; these are plain Rust tests.
    use core::prelude::v1::test;
    #[test]
    fn keep_all_breaks_only_between_words() {
        assert_eq!(
            keep_all_words("대기열에서 카드를 고르면  그 터미널이 여기에 열립니다"),
            [
                "대기열에서",
                "카드를",
                "고르면",
                "그",
                "터미널이",
                "여기에",
                "열립니다"
            ]
        );
        assert!(keep_all_words("   ").is_empty());
    }
    #[test]
    fn banner_wraps_between_words_only() {
        assert_eq!(
            keep_all_words("작업 폴더가 없습니다"),
            ["작업", "폴더가", "없습니다"]
        );
        assert!(
            keep_all_words("작업 폴더가 없습니다")
                .into_iter()
                .all(keep_all_unbreakable)
        );
        assert!(
            keep_all_words("Review failed: worktree missing")
                .into_iter()
                .all(keep_all_unbreakable)
        );
        // A long path may break so the banner never overflows.
        assert!(!keep_all_unbreakable(
            "C:/Users/me/AppData/Roaming/Pod/themes/broken.json"
        ));
    }
    #[test]
    fn busy_button_shows_korean_not_english() {
        assert_eq!(
            button_text("승인".into(), &ControlState::Loading).as_ref(),
            "처리 중…"
        );
        assert_eq!(
            button_text("승인".into(), &ControlState::Ready).as_ref(),
            "승인"
        );
    }
}
