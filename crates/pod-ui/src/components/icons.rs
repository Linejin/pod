//! Bundled Lucide icons, the style's stroke width and icon elements.
use crate::tokens::{ICON_SIZE, color};
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
            Self::Accessibility => include_bytes!("../../assets/icons/accessibility.svg"),
            Self::ArrowDown => include_bytes!("../../assets/icons/arrow-down.svg"),
            Self::ArrowLeft => include_bytes!("../../assets/icons/arrow-left.svg"),
            Self::ArrowRight => include_bytes!("../../assets/icons/arrow-right.svg"),
            Self::ArrowUp => include_bytes!("../../assets/icons/arrow-up.svg"),
            Self::Bell => include_bytes!("../../assets/icons/bell.svg"),
            Self::BellOff => include_bytes!("../../assets/icons/bell-off.svg"),
            Self::Bot => include_bytes!("../../assets/icons/bot.svg"),
            Self::Check => include_bytes!("../../assets/icons/check.svg"),
            Self::ChevronDown => include_bytes!("../../assets/icons/chevron-down.svg"),
            Self::ChevronLeft => include_bytes!("../../assets/icons/chevron-left.svg"),
            Self::ChevronRight => include_bytes!("../../assets/icons/chevron-right.svg"),
            Self::ChevronUp => include_bytes!("../../assets/icons/chevron-up.svg"),
            Self::ChevronsUpDown => include_bytes!("../../assets/icons/chevrons-up-down.svg"),
            Self::Circle => include_bytes!("../../assets/icons/circle.svg"),
            Self::CircleCheck => include_bytes!("../../assets/icons/circle-check.svg"),
            Self::CircleDot => include_bytes!("../../assets/icons/circle-dot.svg"),
            Self::CircleHelp => include_bytes!("../../assets/icons/circle-help.svg"),
            Self::ClipboardCheck => include_bytes!("../../assets/icons/clipboard-check.svg"),
            Self::Clock => include_bytes!("../../assets/icons/clock.svg"),
            Self::Columns2 => include_bytes!("../../assets/icons/columns-2.svg"),
            Self::Copy => include_bytes!("../../assets/icons/copy.svg"),
            Self::Ellipsis => include_bytes!("../../assets/icons/ellipsis.svg"),
            Self::ExternalLink => include_bytes!("../../assets/icons/external-link.svg"),
            Self::FileCode => include_bytes!("../../assets/icons/file-code.svg"),
            Self::FileDiff => include_bytes!("../../assets/icons/file-diff.svg"),
            Self::FileText => include_bytes!("../../assets/icons/file-text.svg"),
            Self::FolderGit2 => include_bytes!("../../assets/icons/folder-git-2.svg"),
            Self::FolderOpen => include_bytes!("../../assets/icons/folder-open.svg"),
            Self::GitBranch => include_bytes!("../../assets/icons/git-branch.svg"),
            Self::GitCompareArrows => include_bytes!("../../assets/icons/git-compare-arrows.svg"),
            Self::GitMerge => include_bytes!("../../assets/icons/git-merge.svg"),
            Self::GitPullRequest => include_bytes!("../../assets/icons/git-pull-request.svg"),
            Self::Info => include_bytes!("../../assets/icons/info.svg"),
            Self::Keyboard => include_bytes!("../../assets/icons/keyboard.svg"),
            Self::Layers => include_bytes!("../../assets/icons/layers.svg"),
            Self::LayoutGrid => include_bytes!("../../assets/icons/layout-grid.svg"),
            Self::Link => include_bytes!("../../assets/icons/link.svg"),
            Self::ListFilter => include_bytes!("../../assets/icons/list-filter.svg"),
            Self::ListTree => include_bytes!("../../assets/icons/list-tree.svg"),
            Self::LoaderCircle => include_bytes!("../../assets/icons/loader-circle.svg"),
            Self::LockKeyhole => include_bytes!("../../assets/icons/lock-keyhole.svg"),
            Self::Maximize => include_bytes!("../../assets/icons/maximize.svg"),
            Self::Maximize2 => include_bytes!("../../assets/icons/maximize-2.svg"),
            Self::MessageCircleQuestion => {
                include_bytes!("../../assets/icons/message-circle-question.svg")
            }
            Self::MessageSquare => include_bytes!("../../assets/icons/message-square.svg"),
            Self::Minimize2 => include_bytes!("../../assets/icons/minimize-2.svg"),
            Self::Minus => include_bytes!("../../assets/icons/minus.svg"),
            Self::Monitor => include_bytes!("../../assets/icons/monitor.svg"),
            Self::Moon => include_bytes!("../../assets/icons/moon.svg"),
            Self::OctagonX => include_bytes!("../../assets/icons/octagon-x.svg"),
            Self::Palette => include_bytes!("../../assets/icons/palette.svg"),
            Self::PanelLeft => include_bytes!("../../assets/icons/panel-left.svg"),
            Self::PanelRight => include_bytes!("../../assets/icons/panel-right.svg"),
            Self::Pencil => include_bytes!("../../assets/icons/pencil.svg"),
            Self::Plug => include_bytes!("../../assets/icons/plug.svg"),
            Self::Plus => include_bytes!("../../assets/icons/plus.svg"),
            Self::RefreshCw => include_bytes!("../../assets/icons/refresh-cw.svg"),
            Self::RotateCcw => include_bytes!("../../assets/icons/rotate-ccw.svg"),
            Self::Rows2 => include_bytes!("../../assets/icons/rows-2.svg"),
            Self::Search => include_bytes!("../../assets/icons/search.svg"),
            Self::Send => include_bytes!("../../assets/icons/send.svg"),
            Self::Server => include_bytes!("../../assets/icons/server.svg"),
            Self::Settings => include_bytes!("../../assets/icons/settings.svg"),
            Self::ShieldQuestion => include_bytes!("../../assets/icons/shield-question.svg"),
            Self::Square => include_bytes!("../../assets/icons/square.svg"),
            Self::SquareTerminal => include_bytes!("../../assets/icons/square-terminal.svg"),
            Self::Sun => include_bytes!("../../assets/icons/sun.svg"),
            Self::Trash2 => include_bytes!("../../assets/icons/trash-2.svg"),
            Self::TriangleAlert => include_bytes!("../../assets/icons/triangle-alert.svg"),
            Self::Type => include_bytes!("../../assets/icons/type.svg"),
            Self::Unplug => include_bytes!("../../assets/icons/unplug.svg"),
            Self::Users => include_bytes!("../../assets/icons/users.svg"),
            Self::Volume2 => include_bytes!("../../assets/icons/volume-2.svg"),
            Self::VolumeX => include_bytes!("../../assets/icons/volume-x.svg"),
            Self::WifiOff => include_bytes!("../../assets/icons/wifi-off.svg"),
            Self::WrapText => include_bytes!("../../assets/icons/wrap-text.svg"),
            Self::X => include_bytes!("../../assets/icons/x.svg"),
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

/// A bundled icon as a gpui-component icon (for `Button::icon`), with the style's stroke.
pub fn kit_icon(name: Icon) -> gpui_kit::component::Icon {
    gpui_kit::component::Icon::default().data(name.bytes())
}
