//! Pod design language v1. All colors are opaque sRGB except the modal scrim.
use gpui_kit::{Hsla, rgb, rgba};
use std::sync::atomic::{AtomicBool, Ordering};
static HIGH_CONTRAST: AtomicBool = AtomicBool::new(false);
/// App-local preview; never changes the Windows accessibility setting.
pub fn set_high_contrast(enabled: bool) {
    HIGH_CONTRAST.store(enabled, Ordering::Relaxed);
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}
/// Resolved colour tokens (docs/10 §D4.4 roles) of the active style theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub bg_canvas: u32,
    pub bg_chrome: u32,
    pub bg_surface: u32,
    pub bg_raised: u32,
    pub bg_overlay: u32,
    pub bg_hover: u32,
    pub bg_pressed: u32,
    pub bg_selected: u32,
    pub border_subtle: u32,
    pub border_strong: u32,
    pub border_control: u32,
    pub text_primary: u32,
    pub text_secondary: u32,
    pub text_muted: u32,
    pub text_disabled: u32,
    pub accent_fg: u32,
    pub focus_ring: u32,
    pub action_primary_bg: u32,
    pub action_primary_hover: u32,
    pub action_primary_pressed: u32,
    pub action_primary_fg: u32,
    pub success_fg: u32,
    pub success_bg: u32,
    pub warning_fg: u32,
    pub warning_bg: u32,
    pub danger_fg: u32,
    pub danger_bg: u32,
    pub info_fg: u32,
    pub info_bg: u32,
    pub diff_add_bg: u32,
    pub diff_add_fg: u32,
    pub diff_del_bg: u32,
    pub diff_del_fg: u32,
    /// `0xRRGGBBAA`.
    pub overlay_scrim: u32,
    pub terminal_bg: u32,
    pub terminal_fg: u32,
    pub terminal_selection: u32,
    pub terminal_cursor: u32,
    /// Agent marks (docs/10 §D5.11): optional `agent.<id>` in a palette, else
    /// derived from the service's hue family to clear 3:1 on every surface.
    pub agent_claude: u32,
    pub agent_codex: u32,
    pub agent_gemini: u32,
    pub agent_aider: u32,
}
impl Tokens {
    /// The active style theme's palette for `mode` (its high-contrast palette while
    /// the OS contrast theme or the in-app preview is on).
    pub fn for_mode(mode: Mode) -> Self {
        let a = crate::themes::active();
        match (mode, HIGH_CONTRAST.load(Ordering::Relaxed)) {
            (Mode::Dark, false) => a.dark,
            (Mode::Light, false) => a.light,
            (Mode::Dark, true) => a.hc_dark,
            (Mode::Light, true) => a.hc_light,
        }
    }
}
pub fn high_contrast() -> bool {
    HIGH_CONTRAST.load(Ordering::Relaxed)
}
pub fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}
pub fn alpha(hex: u32) -> Hsla {
    rgba(hex).into()
}
pub const SPACE: [f32; 8] = [0., 4., 8., 12., 16., 24., 32., 48.];
pub const HALF_SPACE: f32 = 2.;
pub const RADII: [f32; 6] = [0., 4., 6., 8., 12., 999.];
pub const TREE_INDENT: f32 = 16.;
pub const ICON_SIZE: f32 = 16.;
pub const HIT_AREA: f32 = 28.;
pub const FOCUS_RING: f32 = 2.;
pub const FOCUS_OFFSET: f32 = 2.;
pub const HEIGHTS: [f32; 5] = [36., 32., 28., 24., 32.];
pub const TYPE: [(f32, f32, u16); 6] = [
    (12., 16., 400),
    (13., 20., 400),
    (13., 20., 600),
    (15., 24., 600),
    (20., 28., 600),
    (28., 36., 600),
];
pub const UI_FONTS: [&str; 4] = [
    "Segoe UI Variable Text",
    "Segoe UI",
    "Malgun Gothic",
    "Noto Sans CJK KR",
];
pub const MONO_FONTS: [&str; 2] = ["Cascadia Mono", "Consolas"];
pub fn contrast(a: u32, b: u32) -> f64 {
    fn lum(c: u32) -> f64 {
        let channel = |shift| {
            let v = ((c >> shift) & 255u32) as f64 / 255.;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    }
    let (a, b) = (lum(a), lum(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// Durations in ms: instant, fast, normal, slow. Reduced motion resolves all to zero.
pub const MOTION_MS: [u64; 4] = [0, 100, 160, 220];
pub const ACTIVITY_MS: u64 = 1000;
pub const EASING_ENTER: [f32; 4] = [0.16, 1., 0.3, 1.];
pub const EASING_EXIT: [f32; 4] = [0.4, 0., 1., 1.];
pub const EASING_MOVE: [f32; 4] = [0.2, 0., 0., 1.];
/// (offset y, blur, dark color, light color), no spread and no dock shadow.
pub const ELEVATION: [(f32, f32, u32, u32); 3] = [
    (0., 0., 0, 0),
    (4., 12., 0x00000040, 0x2030471f),
    (12., 32., 0x00000066, 0x2030472e),
];
pub fn elevation(level: usize, mode: Mode) -> Vec<gpui_kit::BoxShadow> {
    // High contrast: no shadows (docs/10 §D4.4); outlines carry the separation.
    if level == 0 || high_contrast() {
        return Vec::new();
    }
    let (y, blur, dark, light) = ELEVATION[level.min(2)];
    vec![gpui_kit::BoxShadow {
        inset: false,
        color: alpha(if mode == Mode::Dark { dark } else { light }),
        offset: gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(y)),
        blur_radius: gpui_kit::px(blur),
        spread_radius: gpui_kit::px(0.),
    }]
}
