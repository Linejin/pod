//! Style themes (docs/10 §D5): built-in JSON specs, user theme files, the active
//! theme and its resolved tokens/metrics. Layout modes are a separate axis owned
//! by the host; a [`Preset`] only names one of each.
use crate::tokens::Tokens;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// How panels sit on the window (docs/10 §D5.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    /// Flat planes separated by 1px hairlines (01, 04).
    #[default]
    Flat,
    /// Rounded panels on a darker frame, surface steps instead of borders (05, 06).
    Layered,
    /// Floating rounded islands with gaps and shadow (08).
    Islands,
    /// No borders: whitespace and weight only (09).
    Mono,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shadow {
    #[default]
    None,
    Soft,
    Raised,
}
/// Logical px at 100%.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Density {
    pub row: f32,
    pub titlebar: f32,
    pub tabbar: f32,
    pub panel_header: f32,
    pub statusbar: f32,
    pub sidebar: f32,
    pub ui_font: f32,
}
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct Radius {
    pub control: f32,
    pub card: f32,
    pub overlay: f32,
}
/// User density override (settings › 모양 › 밀도). `None` keeps the style's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DensityLevel {
    Dense,
    Compact,
    Comfortable,
    Accessible,
}
impl DensityLevel {
    pub const ALL: [Self; 4] = [
        Self::Dense,
        Self::Compact,
        Self::Comfortable,
        Self::Accessible,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Dense => "dense",
            Self::Compact => "compact",
            Self::Comfortable => "comfortable",
            Self::Accessible => "accessible",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Dense => "고밀도",
            Self::Compact => "기본",
            Self::Comfortable => "여유롭게",
            Self::Accessible => "접근성",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|d| d.id() == id)
    }
    /// Standard heights; the sidebar width stays the style's.
    pub fn apply(self, base: Density) -> Density {
        let (row, titlebar, tabbar, panel_header, statusbar, ui_font) = match self {
            Self::Dense => (22., 30., 26., 22., 22., 12.),
            Self::Compact => (28., 36., 32., 28., 24., 13.),
            Self::Comfortable => (34., 40., 34., 32., 26., 13.),
            Self::Accessible => (40., 44., 40., 40., 32., 15.),
        };
        Density {
            row,
            titlebar,
            tabbar,
            panel_header,
            statusbar,
            ui_font,
            sidebar: base.sidebar,
        }
    }
}

/// One style theme file (`crates/pod-ui/themes/<id>.json`, or a user file).
#[derive(Clone, Debug, Deserialize)]
pub struct ThemeSpec {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sample: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub surface: Surface,
    pub density: Density,
    pub radius: Radius,
    #[serde(default)]
    pub gap: f32,
    #[serde(default)]
    pub shadow: Shadow,
    pub icon_stroke: f32,
    pub focus_width: f32,
    pub border_width: f32,
    #[serde(default)]
    pub accent_override: bool,
    /// Palette name (`dark`, `light`, `hc-dark`, `hc-light`) → token name → `#RRGGBB[AA]`.
    pub colors: BTreeMap<String, BTreeMap<String, String>>,
    /// Accent id → palette → token overrides (precomputed per style by tools/themes/gen.py).
    #[serde(default)]
    pub accents: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
    /// Set for user themes only.
    #[serde(skip)]
    pub path: Option<PathBuf>,
}

/// Every token a palette must define, in docs/10 §D4.4 names.
pub const TOKEN_NAMES: [&str; 38] = [
    "bg.canvas",
    "bg.chrome",
    "bg.surface",
    "bg.raised",
    "bg.overlay",
    "bg.hover",
    "bg.pressed",
    "bg.selected",
    "border.subtle",
    "border.strong",
    "border.control",
    "text.primary",
    "text.secondary",
    "text.muted",
    "text.disabled",
    "accent.fg",
    "focus.ring",
    "action.primary.bg",
    "action.primary.hover",
    "action.primary.pressed",
    "action.primary.fg",
    "success.fg",
    "success.bg",
    "warning.fg",
    "warning.bg",
    "danger.fg",
    "danger.bg",
    "info.fg",
    "info.bg",
    "diff.add.bg",
    "diff.add.fg",
    "diff.del.bg",
    "diff.del.fg",
    "terminal.bg",
    "terminal.fg",
    "terminal.selection",
    "terminal.cursor",
    "scrim",
];

/// `#RRGGBB`; `#RRGGBBAA` only where the token carries alpha (`scrim`). Colour
/// tokens are painted with `rgb()`, which would drop the top byte of an
/// 8-digit value and draw `GGBBAA` instead.
fn parse_hex(v: &str, alpha: bool) -> Option<u32> {
    let h = v.strip_prefix('#')?;
    if !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        6 => u32::from_str_radix(h, 16).ok(),
        8 if alpha => u32::from_str_radix(h, 16).ok(),
        _ => None,
    }
}

impl ThemeSpec {
    /// Palettes this spec defines itself (high contrast may come from the shared one).
    pub fn has_palette(&self, name: &str) -> bool {
        self.colors.contains_key(name)
    }
    /// Resolve a palette (with an optional accent preset) into tokens.
    pub fn tokens(&self, palette: &str, accent: Option<&str>) -> Result<Tokens, String> {
        let mut map = self
            .colors
            .get(palette)
            .cloned()
            .ok_or_else(|| format!("{}: 팔레트 {palette} 없음", self.id))?;
        if let Some(over) = accent
            .and_then(|a| self.accents.get(a))
            .and_then(|a| a.get(palette))
        {
            map.extend(over.clone());
        }
        let get = |name: &str| -> Result<u32, String> {
            let raw = map
                .get(name)
                .ok_or_else(|| format!("{}/{palette}: {name} 없음", self.id))?;
            parse_hex(raw, name == "scrim").ok_or_else(|| {
                if name == "scrim" {
                    format!(
                        "{}/{palette}: {name} 값 {raw} 은 #RRGGBB 또는 #RRGGBBAA 형식이 아님",
                        self.id
                    )
                } else {
                    format!(
                        "{}/{palette}: {name} 값 {raw} 은 #RRGGBB 형식이 아님(투명도는 scrim에만)",
                        self.id
                    )
                }
            })
        };
        let scrim = get("scrim")?;
        let mut t = Tokens {
            bg_canvas: get("bg.canvas")?,
            bg_chrome: get("bg.chrome")?,
            bg_surface: get("bg.surface")?,
            bg_raised: get("bg.raised")?,
            bg_overlay: get("bg.overlay")?,
            bg_hover: get("bg.hover")?,
            bg_pressed: get("bg.pressed")?,
            bg_selected: get("bg.selected")?,
            border_subtle: get("border.subtle")?,
            border_strong: get("border.strong")?,
            border_control: get("border.control")?,
            text_primary: get("text.primary")?,
            text_secondary: get("text.secondary")?,
            text_muted: get("text.muted")?,
            text_disabled: get("text.disabled")?,
            accent_fg: get("accent.fg")?,
            focus_ring: get("focus.ring")?,
            action_primary_bg: get("action.primary.bg")?,
            action_primary_hover: get("action.primary.hover")?,
            action_primary_pressed: get("action.primary.pressed")?,
            action_primary_fg: get("action.primary.fg")?,
            success_fg: get("success.fg")?,
            success_bg: get("success.bg")?,
            warning_fg: get("warning.fg")?,
            warning_bg: get("warning.bg")?,
            danger_fg: get("danger.fg")?,
            danger_bg: get("danger.bg")?,
            info_fg: get("info.fg")?,
            info_bg: get("info.bg")?,
            diff_add_bg: get("diff.add.bg")?,
            diff_add_fg: get("diff.add.fg")?,
            diff_del_bg: get("diff.del.bg")?,
            diff_del_fg: get("diff.del.fg")?,
            // 6-digit scrims get a default 40% alpha.
            overlay_scrim: if map.get("scrim").is_some_and(|s| s.len() == 7) {
                (scrim << 8) | 0x66
            } else {
                scrim
            },
            terminal_bg: get("terminal.bg")?,
            terminal_fg: get("terminal.fg")?,
            terminal_selection: get("terminal.selection")?,
            terminal_cursor: get("terminal.cursor")?,
            agent_claude: 0,
            agent_codex: 0,
            agent_gemini: 0,
            agent_aider: 0,
        };
        // Optional per-palette agent mark tokens; derived from the theme's own surfaces otherwise.
        for (mark, name, seed_dark, seed_light) in crate::agent::MARK_SEEDS {
            let value = if map.contains_key(name) {
                get(name)?
            } else {
                crate::agent::derive_mark_color(seed_dark, seed_light, &t)
            };
            match mark {
                crate::agent::AgentMark::Claude => t.agent_claude = value,
                crate::agent::AgentMark::Codex => t.agent_codex = value,
                crate::agent::AgentMark::Gemini => t.agent_gemini = value,
                _ => t.agent_aider = value,
            }
        }
        Ok(t)
    }
    /// Every palette resolves (and every accent preset over it).
    pub fn validate(&self) -> Result<(), String> {
        for p in ["dark", "light"] {
            if !self.has_palette(p) {
                return Err(format!("{}: 필수 팔레트 {p} 없음", self.id));
            }
        }
        for p in self.colors.keys() {
            self.tokens(p, None)?;
            for a in self.accents.keys() {
                self.tokens(p, Some(a))?;
            }
        }
        Ok(())
    }
}

/// `(sample number, Korean name, style id, layout id)`: the gallery presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub number: &'static str,
    pub name: &'static str,
    pub style: &'static str,
    pub layout: &'static str,
}
const fn preset(
    number: &'static str,
    name: &'static str,
    style: &'static str,
    layout: &'static str,
) -> Preset {
    Preset {
        number,
        name,
        style,
        layout,
    }
}
/// Layout ids are owned by the host (pod-app `layout_mode`); here they are names only.
pub const PRESETS: [Preset; 18] = [
    preset("01", "정밀한 작업대", "workbench", "workbench"),
    preset("02", "한 작업에 집중", "workbench", "focus-rail"),
    preset("03", "응답 중심 관제", "workbench", "queue"),
    preset("04", "조용한 편집기", "quiet", "workbench"),
    preset("05", "레이어형", "layered", "workbench"),
    preset("06", "관제형", "mission", "queue"),
    preset("07", "고밀도 편집기", "dense", "dense-details"),
    preset("08", "섬형", "islands", "workbench"),
    preset("09", "미니멀 단색", "mono", "workbench"),
    preset("10", "따뜻한 중성색", "warm", "workbench"),
    preset("11", "블록 터미널", "blocks", "block-terminal"),
    preset("12", "칸반 보드", "kanban", "kanban"),
    preset("13", "분할 리뷰 중심", "review", "review"),
    preset("14", "고대비·접근성 우선", "access", "workbench"),
    preset("15", "조용한 편집기 · 청록", "quiet-teal", "workbench"),
    preset("16", "조용한 편집기 · 보라", "quiet-violet", "workbench"),
    preset(
        "17",
        "정밀한 작업대 · 중립 회색",
        "workbench-neutral",
        "workbench",
    ),
    preset("18", "정밀한 작업대 · 녹청", "workbench-green", "workbench"),
];
pub const DEFAULT_STYLE: &str = "quiet";
/// Accent ids users can pick when a style allows it.
pub const ACCENTS: [(&str, &str); 5] = [
    ("blue", "파랑"),
    ("teal", "청록"),
    ("violet", "보라"),
    ("green", "녹청"),
    ("indigo", "인디고"),
];

macro_rules! builtin {
    ($($id:literal),* $(,)?) => {
        const BUILTIN_JSON: &[(&str, &str)] = &[$(($id, include_str!(concat!("../themes/", $id, ".json")))),*];
    };
}
builtin!(
    "workbench",
    "quiet",
    "layered",
    "mission",
    "dense",
    "islands",
    "mono",
    "warm",
    "blocks",
    "kanban",
    "review",
    "access",
    "quiet-teal",
    "quiet-violet",
    "workbench-neutral",
    "workbench-green",
);

/// Deep merge: objects merge key by key, everything else replaces.
fn merge(base: &mut serde_json::Value, over: serde_json::Value) {
    match (base, over) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

/// Built-in styles plus valid user files; invalid user files are listed with a reason.
#[derive(Clone, Debug, Default)]
pub struct ThemeRegistry {
    pub styles: Vec<Arc<ThemeSpec>>,
    pub errors: Vec<(PathBuf, String)>,
    /// Loaded user styles whose text/control pairs miss WCAG AA (shown in 설정 › 모양).
    pub warnings: Vec<(PathBuf, String)>,
}
impl ThemeRegistry {
    pub fn builtin() -> Self {
        let styles = BUILTIN_JSON
            .iter()
            .map(|(id, json)| {
                Arc::new(
                    serde_json::from_str::<ThemeSpec>(json)
                        .unwrap_or_else(|e| panic!("built-in theme {id}: {e}")),
                )
            })
            .collect();
        Self {
            styles,
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }
    /// Built-ins plus `*.json` in `dir` (sorted by file name). A user file may
    /// `"extends": "<id>"` and override only some keys; its `id` must be new.
    pub fn load(dir: Option<&Path>) -> Self {
        let mut reg = Self::builtin();
        let Some(dir) = dir else { return reg };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return reg;
        };
        let mut files: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("json"))
            })
            .collect();
        files.sort();
        for path in files {
            match reg.parse_user(&path) {
                Ok(spec) => {
                    let failures = user_contrast_failures(&spec);
                    if !failures.is_empty() {
                        let more = if failures.len() > 3 {
                            format!(" 외 {}개", failures.len() - 3)
                        } else {
                            String::new()
                        };
                        reg.warnings.push((
                            path,
                            format!(
                                "대비 부족: {}{more}",
                                failures[..failures.len().min(3)].join("; ")
                            ),
                        ));
                    }
                    reg.styles.push(Arc::new(spec))
                }
                Err(e) => reg.errors.push((path, e)),
            }
        }
        reg
    }
    fn parse_user(&self, path: &Path) -> Result<ThemeSpec, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let mut value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("JSON 오류: {e}"))?;
        if let Some(base_id) = value
            .get("extends")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
        {
            let base = self
                .get(&base_id)
                .ok_or_else(|| format!("extends: 테마 {base_id} 없음"))?;
            let mut merged = serde_json::to_value(SpecOut(&base)).map_err(|e| e.to_string())?;
            value.as_object_mut().map(|o| o.remove("extends"));
            merge(&mut merged, value);
            value = merged;
        }
        let mut spec: ThemeSpec =
            serde_json::from_value(value).map_err(|e| format!("형식 오류: {e}"))?;
        if spec.id.is_empty() || self.get(&spec.id).is_some() {
            return Err(format!("id {:?}는 비었거나 이미 있음", spec.id));
        }
        spec.validate()?;
        spec.path = Some(path.to_path_buf());
        Ok(spec)
    }
    pub fn get(&self, id: &str) -> Option<Arc<ThemeSpec>> {
        self.styles.iter().find(|s| s.id == id).cloned()
    }
}
/// Serialize a spec back to the file shape (for `extends`).
struct SpecOut<'a>(&'a ThemeSpec);
impl serde::Serialize for SpecOut<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let t = self.0;
        let mut m = s.serialize_map(None)?;
        m.serialize_entry("id", &t.id)?;
        m.serialize_entry("name", &t.name)?;
        m.serialize_entry("sample", &t.sample)?;
        m.serialize_entry("source", &t.source)?;
        m.serialize_entry(
            "surface",
            match t.surface {
                Surface::Flat => "flat",
                Surface::Layered => "layered",
                Surface::Islands => "islands",
                Surface::Mono => "mono",
            },
        )?;
        let d = t.density;
        m.serialize_entry(
            "density",
            &serde_json::json!({"row": d.row, "titlebar": d.titlebar, "tabbar": d.tabbar, "panel_header": d.panel_header, "statusbar": d.statusbar, "sidebar": d.sidebar, "ui_font": d.ui_font}),
        )?;
        m.serialize_entry("radius", &serde_json::json!({"control": t.radius.control, "card": t.radius.card, "overlay": t.radius.overlay}))?;
        m.serialize_entry("gap", &t.gap)?;
        m.serialize_entry(
            "shadow",
            match t.shadow {
                Shadow::None => "none",
                Shadow::Soft => "soft",
                Shadow::Raised => "raised",
            },
        )?;
        m.serialize_entry("icon_stroke", &t.icon_stroke)?;
        m.serialize_entry("focus_width", &t.focus_width)?;
        m.serialize_entry("border_width", &t.border_width)?;
        m.serialize_entry("accent_override", &t.accent_override)?;
        m.serialize_entry("colors", &t.colors)?;
        m.serialize_entry("accents", &t.accents)?;
        m.end()
    }
}

/// What the user picked (pod-app settings).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemeChoice {
    pub style: String,
    pub accent: Option<String>,
    pub density: Option<DensityLevel>,
}

/// Non-colour tokens of the active style after the density override.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub surface: Surface,
    pub density: Density,
    pub radius: Radius,
    pub gap: f32,
    pub shadow: Shadow,
    pub icon_stroke: f32,
    pub focus_width: f32,
    pub border_width: f32,
}

/// The active style with its palettes resolved once.
#[derive(Clone, Debug)]
pub struct Active {
    pub spec: Arc<ThemeSpec>,
    pub accent: Option<String>,
    pub metrics: Metrics,
    pub dark: Tokens,
    pub light: Tokens,
    pub hc_dark: Tokens,
    pub hc_light: Tokens,
}
impl Active {
    pub fn new(reg: &ThemeRegistry, choice: &ThemeChoice) -> Self {
        let spec = reg
            .get(&choice.style)
            .or_else(|| reg.get(DEFAULT_STYLE))
            .expect("default style is built in");
        let accent = choice
            .accent
            .clone()
            .filter(|a| spec.accent_override && spec.accents.contains_key(a));
        let shared = reg.get("workbench").expect("workbench is built in");
        let resolve = |p: &str, acc: Option<&str>| -> Tokens {
            let own = if spec.has_palette(p) {
                spec.tokens(p, acc)
            } else {
                shared.tokens(p, None)
            };
            own.unwrap_or_else(|_| shared.tokens(p, None).expect("shared palettes resolve"))
        };
        let acc = accent.as_deref();
        let metrics = Metrics {
            surface: spec.surface,
            density: choice
                .density
                .map_or(spec.density, |d| d.apply(spec.density)),
            radius: spec.radius,
            gap: spec.gap,
            shadow: spec.shadow,
            icon_stroke: spec.icon_stroke,
            focus_width: spec.focus_width,
            border_width: spec.border_width,
        };
        Self {
            dark: resolve("dark", acc),
            light: resolve("light", acc),
            hc_dark: resolve("hc-dark", None),
            hc_light: resolve("hc-light", None),
            metrics,
            accent,
            spec,
        }
    }
}

static REGISTRY: RwLock<Option<Arc<ThemeRegistry>>> = RwLock::new(None);
static ACTIVE: RwLock<Option<Arc<Active>>> = RwLock::new(None);

/// The loaded registry (built-ins only until [`set_registry`]).
pub fn registry() -> Arc<ThemeRegistry> {
    if let Some(r) = REGISTRY.read().ok().and_then(|g| g.clone()) {
        return r;
    }
    let r = Arc::new(ThemeRegistry::builtin());
    if let Ok(mut g) = REGISTRY.write() {
        g.get_or_insert_with(|| r.clone());
    }
    r
}
pub fn set_registry(reg: ThemeRegistry) {
    if let Ok(mut g) = REGISTRY.write() {
        *g = Some(Arc::new(reg));
    }
}
/// Switch the active style (live: callers re-apply the GPUI theme afterwards).
pub fn activate(choice: &ThemeChoice) -> Arc<Active> {
    let a = Arc::new(Active::new(&registry(), choice));
    if let Ok(mut g) = ACTIVE.write() {
        *g = Some(a.clone());
    }
    a
}
pub fn active() -> Arc<Active> {
    if let Some(a) = ACTIVE.read().ok().and_then(|g| g.clone()) {
        return a;
    }
    activate(&ThemeChoice {
        style: DEFAULT_STYLE.into(),
        ..Default::default()
    })
}
pub fn metrics() -> Metrics {
    active().metrics
}

/// `(fg, bg, minimum)`: the audited pairs; the same list as tools/themes/gen.py `pairs()`.
pub fn contrast_pairs(t: &Tokens) -> Vec<(&'static str, u32, &'static str, u32, f64)> {
    let surfaces = [
        ("bg.canvas", t.bg_canvas),
        ("bg.chrome", t.bg_chrome),
        ("bg.surface", t.bg_surface),
        ("bg.raised", t.bg_raised),
        ("bg.hover", t.bg_hover),
        ("bg.selected", t.bg_selected),
    ];
    let mut out = Vec::new();
    for (f, fv) in [
        ("text.primary", t.text_primary),
        ("text.secondary", t.text_secondary),
        ("text.muted", t.text_muted),
    ] {
        for (b, bv) in surfaces {
            out.push((f, fv, b, bv, 4.5));
        }
    }
    for (f, fv) in [
        ("success.fg", t.success_fg),
        ("warning.fg", t.warning_fg),
        ("danger.fg", t.danger_fg),
        ("info.fg", t.info_fg),
    ] {
        for (b, bv) in surfaces.iter().filter(|(b, _)| *b != "bg.raised") {
            out.push((f, fv, b, *bv, 4.5));
        }
    }
    out.push(("success.fg", t.success_fg, "success.bg", t.success_bg, 4.5));
    out.push(("warning.fg", t.warning_fg, "warning.bg", t.warning_bg, 4.5));
    out.push(("danger.fg", t.danger_fg, "danger.bg", t.danger_bg, 4.5));
    out.push(("info.fg", t.info_fg, "info.bg", t.info_bg, 4.5));
    for (b, bv) in surfaces {
        out.push(("accent.fg", t.accent_fg, b, bv, 3.));
    }
    for (b, bv) in &surfaces[..4] {
        out.push(("focus.ring", t.focus_ring, b, *bv, 3.));
        out.push(("border.control", t.border_control, b, *bv, 3.));
    }
    for (b, bv) in [
        ("action.primary.bg", t.action_primary_bg),
        ("action.primary.hover", t.action_primary_hover),
        ("action.primary.pressed", t.action_primary_pressed),
    ] {
        out.push(("action.primary.fg", t.action_primary_fg, b, bv, 4.5));
    }
    out.push((
        "terminal.fg",
        t.terminal_fg,
        "terminal.bg",
        t.terminal_bg,
        4.5,
    ));
    out.push((
        "diff.add.fg",
        t.diff_add_fg,
        "diff.add.bg",
        t.diff_add_bg,
        4.5,
    ));
    out.push((
        "diff.del.fg",
        t.diff_del_fg,
        "diff.del.bg",
        t.diff_del_bg,
        4.5,
    ));
    out
}
/// AA failures of a user style's own palettes (built-ins are fixed in gen.py instead).
pub fn user_contrast_failures(spec: &ThemeSpec) -> Vec<String> {
    let mut palettes: Vec<&String> = spec.colors.keys().collect();
    palettes.sort();
    palettes
        .into_iter()
        .filter_map(|p| spec.tokens(p, None).ok().map(|t| contrast_failures(p, &t)))
        .flatten()
        .collect()
}
/// Failed pairs as readable lines (empty = passes).
pub fn contrast_failures(label: &str, t: &Tokens) -> Vec<String> {
    contrast_pairs(t)
        .into_iter()
        .filter_map(|(f, fv, b, bv, min)| {
            let r = crate::tokens::contrast(fv, bv);
            (r < min).then(|| format!("{label}: {f} #{fv:06X} on {b} #{bv:06X} = {r:.2} < {min}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_style_and_preset_resolves() {
        let reg = ThemeRegistry::builtin();
        assert_eq!(reg.styles.len(), 16);
        for s in &reg.styles {
            s.validate().unwrap();
            assert_eq!(
                s.has_palette("hc-dark"),
                s.has_palette("hc-light"),
                "{}: both or neither hc palette",
                s.id
            );
        }
        for p in PRESETS {
            assert!(
                reg.get(p.style).is_some(),
                "preset {} names style {}",
                p.number,
                p.style
            );
        }
        let numbers: Vec<_> = PRESETS.iter().map(|p| p.number).collect();
        assert_eq!(
            numbers,
            (1..=18).map(|n| format!("{n:02}")).collect::<Vec<_>>(),
            "every gallery sample is a preset, in order"
        );
        assert!(reg.get(DEFAULT_STYLE).is_some());
    }

    /// Contrast is fixed in the themes (tools/themes/gen.py), never waived here.
    #[test]
    fn builtin_contrast() {
        let reg = ThemeRegistry::builtin();
        let mut failures = Vec::new();
        let mut checked = 0;
        for s in &reg.styles {
            let mut palettes: Vec<String> = s.colors.keys().cloned().collect();
            for p in ["hc-dark", "hc-light"] {
                if !s.has_palette(p) {
                    palettes.push(p.into());
                }
            }
            for p in palettes {
                let base = if s.has_palette(&p) {
                    s.tokens(&p, None)
                } else {
                    reg.get("workbench").unwrap().tokens(&p, None)
                };
                failures.extend(contrast_failures(&format!("{}/{p}", s.id), &base.unwrap()));
                checked += 1;
                if s.accent_override && !p.starts_with("hc") {
                    for (a, _) in ACCENTS {
                        assert!(
                            s.accents.contains_key(a),
                            "{}: accent {a} precomputed",
                            s.id
                        );
                        failures.extend(contrast_failures(
                            &format!("{}/{p}+{a}", s.id),
                            &s.tokens(&p, Some(a)).unwrap(),
                        ));
                        checked += 1;
                    }
                }
            }
        }
        println!("{checked} palettes checked");
        assert!(
            failures.is_empty(),
            "contrast failures:\n{}",
            failures.join("\n")
        );
    }

    /// Agent marks (docs/10 §D5.11): 3:1 icon contrast on every surface, every built-in
    /// style × palette (own or shared high contrast) × allowed accent.
    #[test]
    fn agent_mark_contrast_in_every_builtin_theme() {
        let reg = ThemeRegistry::builtin();
        let (mut failures, mut checked, mut worst) = (Vec::new(), 0, f64::INFINITY);
        for s in &reg.styles {
            for p in ["dark", "light", "hc-dark", "hc-light"] {
                let src = if s.has_palette(p) {
                    s.clone()
                } else {
                    reg.get("workbench").unwrap()
                };
                let accents: Vec<Option<&str>> = if s.accent_override && !p.starts_with("hc") {
                    std::iter::once(None)
                        .chain(ACCENTS.iter().map(|(a, _)| Some(*a)))
                        .collect()
                } else {
                    vec![None]
                };
                for a in accents {
                    let t = src.tokens(p, a).unwrap();
                    for (f, fv, b, bv, min) in crate::agent::mark_contrast_pairs(&t) {
                        let r = crate::tokens::contrast(fv, bv);
                        worst = worst.min(r);
                        checked += 1;
                        if r < min {
                            failures.push(format!(
                                "{}/{p}{}: {f} #{fv:06X} on {b} #{bv:06X} = {r:.2}",
                                s.id,
                                a.map(|a| format!("+{a}")).unwrap_or_default()
                            ));
                        }
                    }
                }
            }
        }
        println!("{checked} agent-mark pairs checked, minimum {worst:.2}");
        assert!(
            failures.is_empty(),
            "agent mark contrast failures:\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn active_theme_switches_live_with_overrides() {
        let reg = ThemeRegistry::builtin();
        let quiet = Active::new(
            &reg,
            &ThemeChoice {
                style: "quiet".into(),
                ..Default::default()
            },
        );
        assert_eq!(quiet.dark.bg_canvas, 0x16181C, "Claude A canvas");
        assert_eq!(quiet.metrics.density.row, 28.);
        let islands = Active::new(
            &reg,
            &ThemeChoice {
                style: "islands".into(),
                accent: Some("teal".into()),
                density: Some(DensityLevel::Dense),
            },
        );
        assert_eq!(islands.metrics.surface, Surface::Islands);
        assert_eq!(islands.metrics.density.row, 22., "density override");
        assert_eq!(
            islands.metrics.density.sidebar, 236.,
            "sidebar width stays the style's"
        );
        assert_eq!(islands.accent.as_deref(), Some("teal"));
        assert_ne!(
            islands.dark.action_primary_bg,
            Active::new(
                &reg,
                &ThemeChoice {
                    style: "islands".into(),
                    ..Default::default()
                }
            )
            .dark
            .action_primary_bg
        );
        // Mono does not allow accents: the request is dropped, not half-applied.
        let mono = Active::new(
            &reg,
            &ThemeChoice {
                style: "mono".into(),
                accent: Some("teal".into()),
                density: None,
            },
        );
        assert_eq!(mono.accent, None);
        // Unknown style falls back to the default.
        assert_eq!(
            Active::new(
                &reg,
                &ThemeChoice {
                    style: "nope".into(),
                    ..Default::default()
                }
            )
            .spec
            .id,
            DEFAULT_STYLE
        );
        // A style without its own high-contrast palette uses the shared one.
        let hc = Active::new(
            &reg,
            &ThemeChoice {
                style: "islands".into(),
                ..Default::default()
            },
        );
        assert_eq!(
            (hc.hc_dark.bg_canvas, hc.hc_dark.text_primary),
            (0x000000, 0xFFFFFF)
        );
    }

    #[test]
    fn user_themes_load_extend_and_report_errors() {
        let dir = std::env::temp_dir().join(format!(
            "pod-ui-themes-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a-mine.json"), r##"{"extends":"quiet","id":"mine","name":"내 테마","colors":{"dark":{"bg.canvas":"#101010"}},"density":{"row":30}}"##).unwrap();
        std::fs::write(dir.join("b-broken.json"), "{ not json").unwrap();
        std::fs::write(
            dir.join("c-dupe.json"),
            r##"{"extends":"quiet","id":"quiet","name":"x"}"##,
        )
        .unwrap();
        std::fs::write(dir.join("d-badcolor.json"), r##"{"extends":"quiet","id":"bad","name":"x","colors":{"light":{"text.muted":"blue"}}}"##).unwrap();
        std::fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let reg = ThemeRegistry::load(Some(&dir));
        let mine = reg.get("mine").expect("user theme loaded");
        assert_eq!(mine.tokens("dark", None).unwrap().bg_canvas, 0x101010);
        assert_eq!(
            mine.tokens("dark", None).unwrap().text_primary,
            reg.get("quiet")
                .unwrap()
                .tokens("dark", None)
                .unwrap()
                .text_primary,
            "inherited"
        );
        assert_eq!(mine.density.row, 30.);
        assert_eq!(mine.density.titlebar, 36., "partial density merges");
        assert!(mine.path.is_some());
        assert_eq!(reg.errors.len(), 3, "{:?}", reg.errors);
        assert_eq!(reg.styles.len(), 17);
        assert!(
            reg.warnings.is_empty(),
            "inherited quiet colours pass AA: {:?}",
            reg.warnings
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ui-12: `#RRGGBBAA` is alpha only for `scrim`; elsewhere it was drawn as `GGBBAA`.
    #[test]
    fn eight_digit_colours_only_for_scrim_and_user_contrast_is_reported() {
        assert_eq!(parse_hex("#112233", false), Some(0x112233));
        assert_eq!(parse_hex("#11223344", false), None);
        assert_eq!(parse_hex("#11223344", true), Some(0x11223344));
        assert_eq!(
            parse_hex("#+12233", false),
            None,
            "no sign accepted by from_str_radix"
        );
        let dir = std::env::temp_dir().join(format!(
            "pod-ui-themes-alpha-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), r##"{"extends":"quiet","id":"alpha","name":"a","colors":{"dark":{"bg.canvas":"#101010FF"}}}"##).unwrap();
        std::fs::write(dir.join("b.json"), r##"{"extends":"quiet","id":"scrim","name":"b","colors":{"dark":{"scrim":"#00000080"}}}"##).unwrap();
        std::fs::write(dir.join("c.json"), r##"{"extends":"quiet","id":"lowc","name":"c","colors":{"dark":{"text.muted":"#303030"}}}"##).unwrap();
        let reg = ThemeRegistry::load(Some(&dir));
        assert!(reg.get("alpha").is_none(), "8-digit colour token rejected");
        assert!(
            reg.errors
                .iter()
                .any(|(_, e)| e.contains("bg.canvas") && e.contains("scrim에만")),
            "{:?}",
            reg.errors
        );
        assert_eq!(
            reg.get("scrim")
                .unwrap()
                .tokens("dark", None)
                .unwrap()
                .overlay_scrim,
            0x00000080
        );
        assert!(reg.get("lowc").is_some(), "low contrast still loads");
        assert_eq!(reg.warnings.len(), 1, "{:?}", reg.warnings);
        assert!(
            reg.warnings[0]
                .1
                .starts_with("대비 부족: dark: text.muted #303030"),
            "{:?}",
            reg.warnings
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
