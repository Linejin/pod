//! Persisted host appearance and the UI/VT palette boundary.
use gpui_kit::component::theme::Theme;
use gpui_kit::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
static LIGHT: AtomicBool = AtomicBool::new(false);
pub fn mode() -> pod_ui::Mode {
    if LIGHT.load(Ordering::Relaxed) {
        pod_ui::Mode::Light
    } else {
        pod_ui::Mode::Dark
    }
}
pub fn tokens() -> pod_ui::Tokens {
    pod_ui::Tokens::for_mode(mode())
}

/// Windows "Contrast themes" is on (`SPI_GETHIGHCONTRAST`, `HCF_HIGHCONTRASTON`).
/// Read only; Pod never changes the system setting.
pub fn os_high_contrast() -> bool {
    #[repr(C)]
    struct HighContrastW {
        cb_size: u32,
        flags: u32,
        default_scheme: *mut u16,
    }
    const HCF_HIGHCONTRASTON: u32 = 1;
    let mut hc = HighContrastW {
        cb_size: std::mem::size_of::<HighContrastW>() as u32,
        flags: 0,
        default_scheme: std::ptr::null_mut(),
    };
    // SAFETY: `hc` is a correctly sized HIGHCONTRASTW that outlives the call.
    let ok = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::SystemParametersInfoW(
            windows_sys::Win32::UI::WindowsAndMessaging::SPI_GETHIGHCONTRAST,
            hc.cb_size,
            (&mut hc as *mut HighContrastW).cast(),
            0,
        )
    };
    ok != 0 && hc.flags & HCF_HIGHCONTRASTON != 0
}

/// The OS contrast theme forces the high-contrast tokens; the in-app
/// preview can turn them on without it.
pub fn effective_high_contrast(setting: bool, os: bool) -> bool {
    setting || os
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub collapsed: std::collections::BTreeSet<String>,
    pub theme: String,
    pub font: String,
    pub font_size: f32,
    pub terminal_font: String,
    pub terminal_font_size: f32,
    pub recents: Vec<String>,
    /// Legacy density toggle (before docs/10 §D5): read as density "comfortable".
    pub comfortable: bool,
    /// Style theme id (docs/10 §D5.2); `theme` above stays the dark/light/system mode.
    pub style: String,
    /// `theme` (the style's own) or a [`pod_ui::themes::DensityLevel`] id.
    pub density: String,
    /// `theme` or an accent id of [`pod_ui::themes::ACCENTS`] (only where the style allows it).
    pub accent: String,
    /// Layout mode id (crate::layout_mode).
    pub layout: String,
    pub reduced_motion: bool,
    pub high_contrast: bool,
    pub jump_shortcut: String,
    /// Agent hook prompt dismissed ("나중에") for this app + pod-hook build.
    pub hooks_dismissed: String,
    /// Mod notices also as Windows toasts while Pod is not in front.
    pub os_notifications: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            collapsed: Default::default(),
            theme: "system".into(),
            font: "Segoe UI".into(),
            font_size: 13.,
            terminal_font: "Cascadia Mono".into(),
            terminal_font_size: 14.,
            recents: Vec::new(),
            comfortable: false,
            style: pod_ui::themes::DEFAULT_STYLE.into(),
            density: "theme".into(),
            accent: "theme".into(),
            layout: crate::layout_mode::LayoutMode::Workbench.id().into(),
            reduced_motion: false,
            high_contrast: false,
            jump_shortcut: "ctrl-k".into(),
            hooks_dismissed: String::new(),
            os_notifications: true,
        }
    }
}
impl Global for Settings {}
/// The OS contrast flag the current tokens were built with.
#[derive(Clone, Copy, Default)]
pub struct OsHighContrast(pub bool);
impl Global for OsHighContrast {}

/// Re-applies the settings when the OS contrast flag changed (Windows sends
/// no dedicated GPUI event: checked on appearance change and activation).
/// Returns whether the tokens changed.
pub fn refresh_os_high_contrast(query: impl Fn() -> bool, cx: &mut App) -> bool {
    let now = query();
    if cx
        .try_global::<OsHighContrast>()
        .is_some_and(|g| g.0 == now)
    {
        return false;
    }
    let settings = cx.try_global::<Settings>().cloned().unwrap_or_default();
    settings.apply_with(now, cx);
    true
}
/// User style themes: `POD_THEMES_DIR`, else `<POD_APP_CONFIG_DIR>\themes`, else
/// `%APPDATA%\Pod\themes`. Tests only ever see their own config dir.
pub fn themes_dir() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        if std::env::var_os("POD_THEMES_DIR").is_none() && !crate::layout::test_config_dir_set() {
            return None;
        }
    }
    if let Some(d) = std::env::var_os("POD_THEMES_DIR") {
        return Some(d.into());
    }
    if std::env::var_os("POD_APP_CONFIG_DIR").is_some() || cfg!(test) {
        return Some(crate::layout::config_dir().join("themes"));
    }
    std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("Pod").join("themes"))
}
/// (Re)read built-in and user style themes; returns the invalid files with reasons.
/// Agent mark overrides (`<themes>\agents\<id>.svg`, docs/10 §D5.11) reload with them.
pub fn reload_themes() -> Vec<(std::path::PathBuf, String)> {
    let dir = themes_dir();
    let reg = pod_ui::themes::ThemeRegistry::load(dir.as_deref());
    let mut errors = reg.errors.clone();
    pod_ui::themes::set_registry(reg);
    errors.extend(pod_ui::agent::load_overrides(dir.as_deref()));
    errors
}

impl Settings {
    /// What the theme engine should show (legacy `comfortable` becomes a density override).
    pub fn theme_choice(&self) -> pod_ui::themes::ThemeChoice {
        use pod_ui::themes::DensityLevel;
        let density = DensityLevel::from_id(&self.density).or(
            if self.comfortable && self.density == "theme" {
                Some(DensityLevel::Comfortable)
            } else {
                None
            },
        );
        pod_ui::themes::ThemeChoice {
            style: self.style.clone(),
            accent: (self.accent != "theme").then(|| self.accent.clone()),
            density,
        }
    }
    /// A preset sets style and layout together (docs/10 §D5.1).
    pub fn with_preset(&self, p: &pod_ui::themes::Preset) -> Self {
        Self {
            style: p.style.into(),
            layout: p.layout.into(),
            ..self.clone()
        }
    }
    /// The preset that matches the current style + layout, if any.
    pub fn preset(&self) -> Option<pod_ui::themes::Preset> {
        pod_ui::themes::PRESETS
            .into_iter()
            .find(|p| p.style == self.style && p.layout == self.layout)
    }
    /// Settings from the config dir; a damaged file is reported, never silently replaced.
    pub fn load() -> (Self, Option<String>) {
        Self::load_from(&crate::layout::config_dir())
    }
    /// A missing file is the defaults. A file that does not parse is copied to
    /// `settings.json.bad-<unix seconds>` before anything can overwrite it; fields that
    /// still read (a JSON object with one bad value) are kept. Returns the notice to show.
    pub fn load_from(dir: &std::path::Path) -> (Self, Option<String>) {
        let path = dir.join("settings.json");
        let Ok(bytes) = std::fs::read(&path) else {
            return (Self::default(), None);
        };
        if let Ok(s) = serde_json::from_slice::<Self>(&bytes) {
            return (s, None);
        }
        // Salvage: start from the defaults and take every field that still deserializes.
        let mut settings = Self::default();
        if let Ok(serde_json::Value::Object(fields)) =
            serde_json::from_slice::<serde_json::Value>(&bytes)
        {
            let mut good = serde_json::to_value(&settings).unwrap_or_default();
            for (k, v) in fields {
                let mut trial = good.clone();
                if let Some(o) = trial.as_object_mut() {
                    o.insert(k, v);
                }
                if let Ok(s) = serde_json::from_value::<Self>(trial.clone()) {
                    settings = s;
                    good = trial;
                }
            }
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let backup = dir.join(format!("settings.json.bad-{stamp}"));
        let notice = match std::fs::copy(&path, &backup) {
            Ok(_) => format!(
                "{} {}",
                crate::strings::SETTINGS_DAMAGED_BACKUP,
                backup.display()
            ),
            Err(e) => format!("{} ({e})", crate::strings::SETTINGS_DAMAGED_NO_BACKUP),
        };
        (settings, Some(notice))
    }
    pub fn save(&self) -> std::io::Result<()> {
        let dir = crate::layout::config_dir();
        std::fs::create_dir_all(&dir)?;
        let tmp = dir.join("settings.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, dir.join("settings.json"))
    }
    pub fn apply(&self, cx: &mut App) {
        self.apply_with(os_high_contrast(), cx);
    }
    /// [`Self::apply`] with the OS contrast flag given (tests inject it).
    pub fn apply_with(&self, os_high_contrast: bool, cx: &mut App) {
        crate::strings::init_toolkit_locale();
        let light = match self.theme.as_str() {
            "light" => true,
            "dark" => false,
            _ => matches!(
                cx.window_appearance(),
                WindowAppearance::Light | WindowAppearance::VibrantLight
            ),
        };
        LIGHT.store(light, Ordering::Relaxed);
        pod_ui::tokens::set_high_contrast(effective_high_contrast(
            self.high_contrast,
            os_high_contrast,
        ));
        let active = pod_ui::themes::activate(&self.theme_choice());
        cx.set_global(OsHighContrast(os_high_contrast));
        pod_ui::apply_theme(mode(), self.reduced_motion, cx);
        let ui_size = ui_font_size(self.font_size, active.metrics.density.ui_font);
        Theme::update(cx, |t| {
            t.font_family = self.font.clone().into();
            t.font_size = px(ui_size);
            t.mono_font_family = self.terminal_font.clone().into();
            t.mono_font_size = px(self.terminal_font_size.clamp(10., 28.));
        });
        cx.set_global(self.clone());
    }
}
/// The user's UI size shifted by the style's density (12 고밀도, 13 기본, 15 접근성).
pub fn ui_font_size(user: f32, density_ui: f32) -> f32 {
    (user + density_ui - 13.).clamp(10., 24.)
}
pub fn terminal_palette() -> pod_terminal_view::TerminalPalette {
    let p = pod_ui::terminal::TerminalPalette::for_mode(mode());
    pod_terminal_view::TerminalPalette {
        foreground: p.foreground,
        background: p.background,
        cursor: p.cursor,
        selection_bg: p.selection,
        ansi: p.ansi,
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;
    /// Tests that switch the process-wide active theme run one at a time.
    static THEME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn os_contrast_theme_forces_high_contrast_tokens() {
        use super::effective_high_contrast as eff;
        assert!(!eff(false, false));
        assert!(eff(false, true), "OS contrast theme alone switches tokens");
        assert!(eff(true, false), "in-app preview alone switches tokens");
        assert!(eff(true, true));
    }

    /// The OS query is injected: the real one is never changed by tests.
    #[test]
    fn app_switches_tokens_when_the_os_reports_high_contrast() {
        let _lock = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use ::gpui::{TestAppContext, TestDispatcher};
        let cx = TestAppContext::build(TestDispatcher::new(0), None);
        cx.update(|cx| {
            gpui_kit::init(cx);
            let settings = Settings {
                theme: "dark".into(),
                ..Default::default()
            };
            settings.apply_with(false, cx);
            let normal = super::tokens();
            assert_ne!(normal.bg_canvas, 0x000000);
            // OS turns a contrast theme on: next activation/appearance check.
            assert!(
                super::refresh_os_high_contrast(|| true, cx),
                "tokens rebuilt"
            );
            let hc = super::tokens();
            let spec = pod_ui::themes::active()
                .spec
                .tokens("hc-dark", None)
                .unwrap();
            assert_eq!(hc, spec, "the style's dark high-contrast palette");
            assert_eq!((hc.bg_canvas, hc.text_primary), (0x000000, 0xFFFFFF));
            assert!(
                !super::refresh_os_high_contrast(|| true, cx),
                "unchanged flag: no rebuild"
            );
            // The user's preview setting is untouched by the OS flag.
            assert!(!cx.global::<Settings>().high_contrast);
            // OS turns it off again.
            assert!(super::refresh_os_high_contrast(|| false, cx));
            assert_eq!(super::tokens().bg_canvas, normal.bg_canvas);
        });
        cx.update(|cx| cx.quit());
    }
    #[test]
    fn style_switch_is_live_and_old_settings_keep_working() {
        let _lock = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        use ::gpui::{TestAppContext, TestDispatcher};
        let cx = TestAppContext::build(TestDispatcher::new(0), None);
        cx.update(|cx| {
            gpui_kit::init(cx);
            let quiet = Settings {
                theme: "dark".into(),
                ..Default::default()
            };
            quiet.apply_with(false, cx);
            assert_eq!(
                pod_ui::themes::active().spec.id,
                "quiet",
                "default style: 04 조용한 편집기"
            );
            let q = super::tokens();
            let islands = Settings {
                style: "islands".into(),
                accent: "violet".into(),
                density: "dense".into(),
                ..quiet.clone()
            };
            islands.apply_with(false, cx);
            let i = super::tokens();
            assert_ne!(q.bg_canvas, i.bg_canvas, "tokens switched without restart");
            assert_eq!(pod_ui::themes::metrics().density.row, 22.);
            let theme = gpui_kit::component::theme::Theme::global(cx);
            assert_eq!(
                theme.font_size,
                gpui_kit::px(12.),
                "dense density lowers the UI size"
            );
            assert_eq!(theme.colors.background, gpui_kit::rgb(i.bg_canvas).into());
            // Light mode of the same style.
            Settings {
                theme: "light".into(),
                ..islands.clone()
            }
            .apply_with(false, cx);
            assert_eq!(super::mode(), pod_ui::Mode::Light);
            assert_eq!(super::tokens(), pod_ui::themes::active().light);
        });
        cx.update(|cx| cx.quit());
        // Legacy files: no style -> default; `comfortable` -> density override.
        let old: Settings = serde_json::from_str(r#"{"theme":"dark","comfortable":true}"#).unwrap();
        assert_eq!(old.style, "quiet");
        assert_eq!(
            old.theme_choice().density,
            Some(pod_ui::themes::DensityLevel::Comfortable)
        );
        let new: Settings =
            serde_json::from_str(r#"{"comfortable":true,"density":"dense"}"#).unwrap();
        assert_eq!(
            new.theme_choice().density,
            Some(pod_ui::themes::DensityLevel::Dense),
            "explicit density wins"
        );
        let p = pod_ui::themes::PRESETS[7];
        let s = Settings::default().with_preset(&p);
        assert_eq!(
            (s.style.as_str(), s.layout.as_str()),
            ("islands", "workbench")
        );
        assert_eq!(s.preset().map(|p| p.number), Some("08"));
        assert_eq!(super::ui_font_size(13., 15.), 15.);
        assert_eq!(super::ui_font_size(24., 15.), 24., "clamped");
    }
    #[test]
    fn user_theme_dir_is_isolated_in_tests() {
        let _lock = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        crate::layout::set_test_config_dir(Some(dir.path().to_path_buf()));
        assert_eq!(super::themes_dir(), Some(dir.path().join("themes")));
        std::fs::create_dir_all(dir.path().join("themes")).unwrap();
        std::fs::write(
            dir.path().join("themes/mine.json"),
            r##"{"extends":"warm","id":"mine","name":"내 테마"}"##,
        )
        .unwrap();
        std::fs::write(dir.path().join("themes/bad.json"), "{").unwrap();
        // Agent mark overrides live next to the themes (docs/10 §D5.11); a bad one is reported too.
        std::fs::create_dir_all(dir.path().join("themes/agents")).unwrap();
        std::fs::write(
            dir.path().join("themes/agents/shell.svg"),
            r#"<svg viewBox="0 0 24 24"><rect width="20" height="20"/></svg>"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("themes/agents/codex.svg"), "<script/>").unwrap();
        let errors = super::reload_themes();
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(pod_ui::themes::registry().get("mine").is_some());
        assert_ne!(
            &*pod_ui::AgentMark::Shell.bytes(),
            pod_ui::AgentMark::Shell.source()
        );
        crate::layout::set_test_config_dir(None);
        pod_ui::themes::set_registry(pod_ui::themes::ThemeRegistry::builtin());
        pod_ui::agent::load_overrides(None);
    }
    /// ui-09: a damaged settings.json is backed up and its readable fields kept;
    /// the next save cannot destroy the only copy.
    #[test]
    fn damaged_settings_are_backed_up_and_salvaged() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Settings::load_from(dir.path()).1.is_none(),
            "no file: defaults, no notice"
        );
        let bad = r#"{"theme":"light","font":"D2Coding","font_size":"big","collapsed":["repo:x"]}"#;
        std::fs::write(dir.path().join("settings.json"), bad).unwrap();
        let (s, notice) = Settings::load_from(dir.path());
        let notice = notice.expect("damaged file is reported");
        assert_eq!(
            (s.theme.as_str(), s.font.as_str(), s.font_size),
            ("light", "D2Coding", 13.),
            "good fields kept, bad one defaulted"
        );
        assert!(s.collapsed.contains("repo:x"));
        let backups: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("settings.json.bad-")
            })
            .collect();
        assert_eq!(backups.len(), 1, "{notice}");
        assert_eq!(
            std::fs::read_to_string(backups[0].path()).unwrap(),
            bad,
            "backup is the original bytes"
        );
        std::fs::write(dir.path().join("settings.json"), "{ not json").unwrap();
        let (s, notice) = Settings::load_from(dir.path());
        assert!(notice.is_some());
        assert_eq!(s.theme, "system", "unreadable: defaults");
        std::fs::write(dir.path().join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
        assert!(
            Settings::load_from(dir.path()).1.is_none(),
            "valid file: no notice"
        );
    }

    #[test]
    fn old_settings_get_independent_font_defaults() {
        let settings: Settings =
            serde_json::from_str(r#"{"theme":"dark","font":"Malgun Gothic"}"#).unwrap();
        assert_eq!(settings.font, "Malgun Gothic");
        assert_eq!(settings.font_size, 13.);
        assert_eq!(settings.terminal_font, "Cascadia Mono");
        assert_eq!(settings.terminal_font_size, 14.);
        assert!(settings.recents.is_empty());
        assert!(
            settings.os_notifications,
            "OS toasts default to on for old settings files"
        );
        let off: Settings = serde_json::from_str(r#"{"os_notifications":false}"#).unwrap();
        assert!(!off.os_notifications);
    }
}
