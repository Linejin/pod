//! Agent marks (docs/10 §D5.11): which AI service runs in a terminal, at a glance.
//!
//! The marks are Pod's own drawings (`assets/agents`, Apache-2.0), not the
//! services' logos: a distinct silhouette with a cut-out monogram per service,
//! tinted with a per-theme token. The generic agent and the plain shell use the
//! bundled Lucide `bot` / `square-terminal`. A mark never stands alone: callers
//! pair it with the text name (tooltip + accessible name, [`agent_mark_el`]).
//!
//! Users may replace a mark with `<themes dir>/agents/<id>.svg` ([`load_overrides`]);
//! it is drawn as a one-colour mask in the same token, like the built-ins.
use crate::components::Icon;
use crate::tokens::{Tokens, color, contrast};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// The visual families. pod-app maps its agent identity onto these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgentMark {
    Claude,
    Codex,
    Gemini,
    Aider,
    /// Any other agent (OSC `app=`, task agent): the generic bot.
    Generic,
    /// A plain terminal (no agent detected).
    Shell,
}

impl AgentMark {
    pub const ALL: [Self; 6] = [
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Aider,
        Self::Generic,
        Self::Shell,
    ];

    /// File stem of the built-in drawing and of a user override (`agents/<id>.svg`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Gemini => "gemini",
            Self::Aider => "aider",
            Self::Generic => "agent",
            Self::Shell => "shell",
        }
    }

    /// The built-in drawing.
    pub fn source(self) -> &'static [u8] {
        match self {
            Self::Claude => include_bytes!("../assets/agents/claude.svg"),
            Self::Codex => include_bytes!("../assets/agents/codex.svg"),
            Self::Gemini => include_bytes!("../assets/agents/gemini.svg"),
            Self::Aider => include_bytes!("../assets/agents/aider.svg"),
            Self::Generic => Icon::Bot.source(),
            Self::Shell => Icon::SquareTerminal.source(),
        }
    }

    /// What is drawn: a user override, else the built-in (Lucide ones in the style's stroke).
    pub fn bytes(self) -> Arc<[u8]> {
        if let Some(b) = OVERRIDES
            .read()
            .ok()
            .and_then(|o| o.get(self.id()).cloned())
        {
            return b;
        }
        match self {
            Self::Generic => Arc::from(Icon::Bot.bytes()),
            Self::Shell => Arc::from(Icon::SquareTerminal.bytes()),
            _ => Arc::from(self.source()),
        }
    }

    /// The mark's colour token in `t` (`agent.<id>`; generic/shell use text roles).
    pub fn color(self, t: &Tokens) -> u32 {
        match self {
            Self::Claude => t.agent_claude,
            Self::Codex => t.agent_codex,
            Self::Gemini => t.agent_gemini,
            Self::Aider => t.agent_aider,
            Self::Generic => t.text_secondary,
            Self::Shell => t.text_muted,
        }
    }
}

/// Seeds `(dark, light)` of the brand-family hues. Each palette moves its seed
/// toward `text.primary` until it clears [`MARK_MIN_CONTRAST`] on every surface.
pub const MARK_SEEDS: [(AgentMark, &str, u32, u32); 4] = [
    // Warm orange family.
    (AgentMark::Claude, "agent.claude", 0xE3916A, 0xB4532A),
    // Neutral green-black family.
    (AgentMark::Codex, "agent.codex", 0x9DC9B0, 0x1E4635),
    (AgentMark::Gemini, "agent.gemini", 0xA8A4F2, 0x4A45B5),
    (AgentMark::Aider, "agent.aider", 0xD8C26B, 0x745F0C),
];
/// Icon contrast target with a little margin over WCAG's 3:1 (rounding, AA on hover).
pub const MARK_MIN_CONTRAST: f64 = 3.2;

/// Every surface a mark sits on (rows, cards, hover, selection, menus).
pub fn mark_surfaces(t: &Tokens) -> [(&'static str, u32); 8] {
    [
        ("bg.canvas", t.bg_canvas),
        ("bg.chrome", t.bg_chrome),
        ("bg.surface", t.bg_surface),
        ("bg.raised", t.bg_raised),
        ("bg.overlay", t.bg_overlay),
        ("bg.hover", t.bg_hover),
        ("bg.pressed", t.bg_pressed),
        ("bg.selected", t.bg_selected),
    ]
}

fn mix(a: u32, b: u32, f: f64) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 255) as f64, ((b >> s) & 255) as f64);
        ((x + (y - x) * f).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

/// The seed for this palette (dark or light by its surface), nudged toward
/// `text.primary` until it clears [`MARK_MIN_CONTRAST`] on every surface.
pub fn derive_mark_color(seed_dark: u32, seed_light: u32, t: &Tokens) -> u32 {
    let dark = contrast(t.bg_surface, 0x000000) < contrast(t.bg_surface, 0xFFFFFF);
    let seed = if dark { seed_dark } else { seed_light };
    let surfaces = mark_surfaces(t);
    (0..=20)
        .map(|i| mix(seed, t.text_primary, i as f64 / 20.))
        .find(|c| {
            surfaces
                .iter()
                .all(|(_, s)| contrast(*c, *s) >= MARK_MIN_CONTRAST)
        })
        .unwrap_or(t.text_primary)
}

/// `(token, colour, surface, colour, 3.0)` for every mark on every surface.
pub fn mark_contrast_pairs(t: &Tokens) -> Vec<(&'static str, u32, &'static str, u32, f64)> {
    let mut out = Vec::new();
    for (mark, name, _, _) in MARK_SEEDS {
        for (b, bv) in mark_surfaces(t) {
            out.push((name, mark.color(t), b, bv, 3.));
        }
    }
    for (b, bv) in mark_surfaces(t) {
        out.push((
            "agent.generic (text.secondary)",
            t.text_secondary,
            b,
            bv,
            3.,
        ));
        out.push(("agent.shell (text.muted)", t.text_muted, b, bv, 3.));
    }
    out
}

static OVERRIDES: RwLock<BTreeMap<&'static str, Arc<[u8]>>> = RwLock::new(BTreeMap::new());
/// Files the last [`load_overrides`] rejected, for 설정 › 모양 (docs/10 §D5.11).
static OVERRIDE_ERRORS: RwLock<Vec<(PathBuf, String)>> = RwLock::new(Vec::new());
/// Rejected override files from the last [`load_overrides`], with reasons.
pub fn override_errors() -> Vec<(PathBuf, String)> {
    OVERRIDE_ERRORS
        .read()
        .map(|e| e.clone())
        .unwrap_or_default()
}
/// Largest accepted override file.
pub const OVERRIDE_MAX_BYTES: u64 = 64 * 1024;

/// Replace the user overrides with `<dir>/agents/<id>.svg` (ids: [`AgentMark::id`]).
/// A file that is too big, not SVG, scripted or links outside itself is skipped
/// and returned with the reason (shown next to the theme errors).
pub fn load_overrides(dir: Option<&Path>) -> Vec<(PathBuf, String)> {
    let mut found = BTreeMap::new();
    let mut errors = Vec::new();
    if let Some(dir) = dir.map(|d| d.join("agents")) {
        for mark in AgentMark::ALL {
            let path = dir.join(format!("{}.svg", mark.id()));
            if !path.is_file() {
                continue;
            }
            match read_override(&path) {
                Ok(bytes) => {
                    found.insert(mark.id(), bytes);
                }
                Err(e) => errors.push((path, e)),
            }
        }
    }
    if let Ok(mut o) = OVERRIDES.write() {
        *o = found;
    }
    if let Ok(mut e) = OVERRIDE_ERRORS.write() {
        e.clone_from(&errors);
    }
    errors
}

fn read_override(path: &Path) -> Result<Arc<[u8]>, String> {
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len > OVERRIDE_MAX_BYTES {
        return Err(format!("{len} 바이트: 최대 {OVERRIDE_MAX_BYTES} 바이트"));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    check_svg(&bytes)?;
    Ok(Arc::from(bytes))
}

/// The same safety rules as the bundled icons: SVG, no script, no external references.
pub fn check_svg(bytes: &[u8]) -> Result<(), String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "UTF-8 SVG가 아님".to_string())?;
    let lower = text.to_ascii_lowercase();
    if !lower.contains("<svg") {
        return Err("<svg> 요소 없음".into());
    }
    if lower.contains("<script")
        || lower.contains("href")
        || lower.contains("<foreignobject")
        || lower.contains("<image")
    {
        return Err("스크립트·외부 참조·이미지는 쓸 수 없음".into());
    }
    Ok(())
}

/// The mark itself at `size` (12 meta / 14 / 16 rows).
pub fn agent_mark(mark: AgentMark, size: f32, t: &Tokens) -> Svg {
    svg()
        .data(&mark.bytes())
        .size(px(size))
        .flex_shrink_0()
        .text_color(color(mark.color(t)))
}

/// The mark as a named image: role `Image`, accessible name and tooltip `name`
/// (e.g. `Claude 에이전트`). `id` must be unique in its list.
pub fn agent_mark_el(
    id: impl Into<ElementId>,
    mark: AgentMark,
    name: impl Into<SharedString>,
    size: f32,
    t: &Tokens,
) -> Stateful<Div> {
    let name: SharedString = name.into();
    let tip = name.clone();
    div()
        .id(id.into())
        .role(Role::Image)
        .aria_label(name)
        .flex_shrink_0()
        .flex()
        .items_center()
        .child(agent_mark(mark, size, t))
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
}

/// Up to `max` distinct marks side by side (a task's agents), then `+N`.
/// One accessible name for the group: `에이전트: Claude, Codex`.
pub fn agent_stack(
    id: impl Into<ElementId>,
    marks: &[(AgentMark, String)],
    max: usize,
    size: f32,
    t: &Tokens,
) -> Stateful<Div> {
    let names: Vec<&str> = marks.iter().map(|(_, n)| n.as_str()).collect();
    let label: SharedString = format!("에이전트: {}", names.join(", ")).into();
    let tip = label.clone();
    let extra = marks.len().saturating_sub(max);
    div()
        .id(id.into())
        .role(Role::Image)
        .aria_label(label)
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(2.))
        .children(marks.iter().take(max).map(|(m, _)| agent_mark(*m, size, t)))
        .when(extra > 0, |d| {
            d.child(
                div()
                    .text_size(px(11.))
                    .text_color(color(t.text_muted))
                    .child(format!("+{extra}")),
            )
        })
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    // After the glob: GPUI exports its own `test` attribute; these are plain Rust tests.
    use core::prelude::v1::test;

    #[test]
    fn built_in_marks_are_safe_original_svgs() {
        for m in AgentMark::ALL {
            let svg = std::str::from_utf8(m.source()).unwrap();
            check_svg(m.source()).unwrap_or_else(|e| panic!("{}: {e}", m.id()));
            assert!(svg.contains("viewBox=\"0 0 24 24\""), "{}", m.id());
        }
        for m in [
            AgentMark::Claude,
            AgentMark::Codex,
            AgentMark::Gemini,
            AgentMark::Aider,
        ] {
            let svg = std::str::from_utf8(m.source()).unwrap();
            assert!(
                svg.contains("Pod original agent mark"),
                "{}: our own drawing",
                m.id()
            );
            // Filled silhouette with a cut-out monogram: not affected by the style's stroke rewrite.
            assert!(
                svg.contains("mask=\"url(#cut)\"") && !svg.contains("stroke-width=\"2\""),
                "{}",
                m.id()
            );
        }
        let shapes: std::collections::HashSet<_> =
            AgentMark::ALL.iter().map(|m| m.source()).collect();
        assert_eq!(
            shapes.len(),
            AgentMark::ALL.len(),
            "every mark has its own drawing"
        );
    }

    #[test]
    fn derived_colors_clear_three_to_one_and_keep_the_hue_family() {
        let reg = crate::themes::ThemeRegistry::builtin();
        let quiet = reg.get("quiet").unwrap();
        let dark = quiet.tokens("dark", None).unwrap();
        let light = quiet.tokens("light", None).unwrap();
        // Warm stays warm (red > blue), green-black stays green-dominant.
        let rgb = |c: u32| ((c >> 16) & 255, (c >> 8) & 255, c & 255);
        for t in [dark, light] {
            let (r, _, b) = rgb(t.agent_claude);
            assert!(r > b + 40, "claude warm: {:06X}", t.agent_claude);
            let (r, g, b) = rgb(t.agent_codex);
            assert!(
                g >= r && g >= b,
                "codex green family: {:06X}",
                t.agent_codex
            );
            assert_ne!(t.agent_claude, t.agent_codex);
        }
        // A palette whose seed already passes keeps it unchanged.
        assert_eq!(derive_mark_color(0xE3916A, 0xB4532A, &dark), 0xE3916A);
    }

    #[test]
    fn explicit_token_wins_over_the_derived_one() {
        let reg = crate::themes::ThemeRegistry::builtin();
        let mut spec = (*reg.get("quiet").unwrap()).clone();
        spec.colors
            .get_mut("dark")
            .unwrap()
            .insert("agent.codex".into(), "#123456".into());
        assert_eq!(spec.tokens("dark", None).unwrap().agent_codex, 0x123456);
        spec.colors
            .get_mut("dark")
            .unwrap()
            .insert("agent.codex".into(), "green".into());
        assert!(
            spec.tokens("dark", None).is_err(),
            "a bad explicit value is reported, not ignored"
        );
    }

    #[test]
    fn overrides_load_validate_and_reset() {
        let dir = std::env::temp_dir().join(format!("pod-ui-agent-marks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("agents")).unwrap();
        let mine = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="9"/></svg>"#;
        std::fs::write(dir.join("agents/codex.svg"), mine).unwrap();
        std::fs::write(
            dir.join("agents/claude.svg"),
            br#"<svg><image href="http://x/y.png"/></svg>"#,
        )
        .unwrap();
        std::fs::write(dir.join("agents/gemini.svg"), b"not svg").unwrap();
        std::fs::write(dir.join("agents/unknown.svg"), mine).unwrap();
        let errors = load_overrides(Some(&dir));
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert_eq!(
            override_errors(),
            errors,
            "kept for the settings page (ui-16)"
        );
        assert_eq!(&*AgentMark::Codex.bytes(), &mine[..]);
        assert_eq!(
            &*AgentMark::Claude.bytes(),
            AgentMark::Claude.source(),
            "rejected file keeps the built-in"
        );
        assert!(load_overrides(None).is_empty());
        assert!(override_errors().is_empty(), "a reload clears old reasons");
        assert_eq!(
            &*AgentMark::Codex.bytes(),
            AgentMark::Codex.source(),
            "reload without the folder resets"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
