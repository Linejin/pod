//! Adapter for the actual gpui-component 0.9 ThemeColor API.
use crate::tokens::*;
use gpui_kit::{
    component::theme::{Theme, ThemeMode},
    *,
};
/// Call after gpui_kit::init and whenever the appearance changes.
/// Theme::update reconciles legacy tokens, projects Base tokens, and refreshes windows.
pub fn apply_theme(mode: Mode, reduced_motion: bool, cx: &mut App) {
    crate::install_key_bindings(cx);
    cx.set_reduce_motion(reduced_motion);
    Theme::change(
        match mode {
            Mode::Dark => ThemeMode::Dark,
            Mode::Light => ThemeMode::Light,
        },
        None,
        cx,
    );
    let fonts = cx.text_system().all_font_names();
    let pick = |choices: &[&str]| {
        choices
            .iter()
            .find(|n| fonts.iter().any(|f| f.as_str() == **n))
            .copied()
            .unwrap_or(".SystemUIFont")
            .to_string()
    };
    let ui = pick(&UI_FONTS);
    let mono = pick(&MONO_FONTS);
    let t = Tokens::for_mode(mode);
    let m = crate::themes::metrics();
    Theme::update(cx, |theme| {
        let c = &mut theme.colors;
        c.background = color(t.bg_canvas);
        c.foreground = color(t.text_primary);
        c.accent_foreground = color(t.text_primary);
        c.button_foreground = color(t.text_primary);
        c.popover_foreground = color(t.text_primary);
        c.sidebar_foreground = color(t.text_primary);
        c.sidebar_accent_foreground = color(t.text_primary);
        c.sidebar_primary_foreground = color(t.text_primary);
        c.tab_active_foreground = color(t.text_primary);
        c.slider_thumb = color(t.text_primary);
        c.switch_thumb = color(t.text_primary);
        c.group_box_foreground = color(t.text_primary);
        c.border = color(t.border_subtle);
        c.window_border = color(t.border_subtle);
        c.sidebar_border = color(t.border_subtle);
        c.title_bar_border = color(t.border_subtle);
        c.status_bar_border = color(t.border_subtle);
        c.table_row_border = color(t.border_subtle);
        c.input = color(t.border_control);
        c.scrollbar_thumb = color(t.border_control);
        c.ring = color(t.focus_ring);
        c.caret = color(t.accent_fg);
        c.list_active_border = color(t.accent_fg);
        c.table_active_border = color(t.accent_fg);
        c.link = color(t.accent_fg);
        c.link_hover = color(t.accent_fg);
        c.link_active = color(t.accent_fg);
        c.drag_border = color(t.accent_fg);
        c.progress_bar = color(t.accent_fg);
        c.slider_bar = color(t.accent_fg);
        c.accent = color(t.bg_hover);
        c.button_hover = color(t.bg_hover);
        c.secondary_hover = color(t.bg_hover);
        c.button_secondary_hover = color(t.bg_hover);
        c.skeleton = color(t.bg_hover);
        c.sidebar_accent = color(t.bg_hover);
        c.list_hover = color(t.bg_hover);
        c.table_hover = color(t.bg_hover);
        c.primary = color(t.action_primary_bg);
        c.button_primary = color(t.action_primary_bg);
        c.primary_hover = color(t.action_primary_hover);
        c.button_primary_hover = color(t.action_primary_hover);
        c.primary_active = color(t.action_primary_pressed);
        c.button_primary_active = color(t.action_primary_pressed);
        c.primary_foreground = color(t.action_primary_fg);
        c.button_primary_foreground = color(t.action_primary_fg);
        c.button = color(t.bg_surface);
        c.secondary = color(t.bg_surface);
        c.button_secondary = color(t.bg_surface);
        c.list = color(t.bg_surface);
        c.list_even = color(t.bg_surface);
        c.tab_active = color(t.bg_surface);
        c.table = color(t.bg_surface);
        c.table_even = color(t.bg_surface);
        c.accordion = color(t.bg_surface);
        c.group_box = color(t.bg_surface);
        c.button_active = color(t.bg_pressed);
        c.secondary_active = color(t.bg_pressed);
        c.button_secondary_active = color(t.bg_pressed);
        c.switch = color(t.bg_pressed);
        c.muted = color(t.bg_chrome);
        c.sidebar = color(t.bg_chrome);
        c.list_head = color(t.bg_chrome);
        c.tab = color(t.bg_chrome);
        c.tab_bar = color(t.bg_chrome);
        c.tab_bar_segmented = color(t.bg_chrome);
        c.title_bar = color(t.bg_chrome);
        c.status_bar = color(t.bg_chrome);
        c.table_head = color(t.bg_chrome);
        c.table_foot = color(t.bg_chrome);
        c.scrollbar = color(t.bg_chrome);
        c.description_list_label = color(t.bg_chrome);
        c.sidebar_primary = color(t.bg_selected);
        c.list_active = color(t.bg_selected);
        c.table_active = color(t.bg_selected);
        c.selection = color(t.bg_selected);
        c.drop_target = color(t.bg_selected);
        c.popover = color(t.bg_overlay);
        c.secondary_foreground = color(t.text_secondary);
        c.button_secondary_foreground = color(t.text_secondary);
        c.tab_foreground = color(t.text_secondary);
        c.table_head_foreground = color(t.text_secondary);
        c.table_foot_foreground = color(t.text_secondary);
        c.scrollbar_thumb_hover = color(t.text_secondary);
        c.description_list_label_foreground = color(t.text_secondary);
        c.muted_foreground = color(t.text_muted);
        c.success = color(t.success_bg);
        c.success_hover = color(t.success_bg);
        c.success_active = color(t.success_bg);
        c.button_success = color(t.success_bg);
        c.button_success_hover = color(t.success_bg);
        c.button_success_active = color(t.success_bg);
        c.success_foreground = color(t.success_fg);
        c.button_success_foreground = color(t.success_fg);
        c.warning = color(t.warning_bg);
        c.warning_hover = color(t.warning_bg);
        c.warning_active = color(t.warning_bg);
        c.button_warning = color(t.warning_bg);
        c.button_warning_hover = color(t.warning_bg);
        c.button_warning_active = color(t.warning_bg);
        c.warning_foreground = color(t.warning_fg);
        c.button_warning_foreground = color(t.warning_fg);
        c.danger = color(t.danger_bg);
        c.danger_hover = color(t.danger_bg);
        c.danger_active = color(t.danger_bg);
        c.button_danger = color(t.danger_bg);
        c.button_danger_hover = color(t.danger_bg);
        c.button_danger_active = color(t.danger_bg);
        c.danger_foreground = color(t.danger_fg);
        c.button_danger_foreground = color(t.danger_fg);
        c.info = color(t.info_bg);
        c.info_hover = color(t.info_bg);
        c.info_active = color(t.info_bg);
        c.button_info = color(t.info_bg);
        c.button_info_hover = color(t.info_bg);
        c.button_info_active = color(t.info_bg);
        c.info_foreground = color(t.info_fg);
        c.button_info_foreground = color(t.info_fg);
        c.overlay = alpha(t.overlay_scrim);
        theme.font_family = ui.into();
        theme.mono_font_family = mono.into();
        theme.font_size = px(m.density.ui_font);
        theme.mono_font_size = px(14.);
        // docs/10 §D4.4: controls 4 (style `radius.control`), menus/dialogs 8 (`radius.overlay`).
        theme.radius = px(m.radius.control);
        theme.radius_lg = px(m.radius.overlay);
        theme.focus_ring = true;
        theme.cursor_blink = if reduced_motion {
            gpui_kit::base::input::CursorBlink::steady()
        } else {
            gpui_kit::base::input::CursorBlink::default()
        };
        let duration = |ms| std::time::Duration::from_millis(if reduced_motion { 0 } else { ms });
        theme.motion.duration_instant = duration(MOTION_MS[0]);
        theme.motion.duration_fast = duration(MOTION_MS[1]);
        theme.motion.duration_normal = duration(MOTION_MS[2]);
        theme.motion.duration_slow = duration(MOTION_MS[3]);
        theme.motion.distance_short = rems(4. / 13.);
        theme.motion.distance_medium = rems(8. / 13.);
        theme.motion.spring_control = gpui_kit::base::Spring::new(duration(MOTION_MS[1]));
        theme.motion.spring_move = gpui_kit::base::Spring::new(duration(MOTION_MS[3]));
        let curve = |v: [f32; 4]| {
            gpui_kit::base::Easing::cubic_bezier(v[0], v[1], v[2], v[3]).expect("valid Pod curve")
        };
        theme.motion.easing_enter = curve(EASING_ENTER);
        theme.motion.easing_exit = curve(EASING_EXIT);
        theme.motion.easing_move = curve(EASING_MOVE);
    });
}
