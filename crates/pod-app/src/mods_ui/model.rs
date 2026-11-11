//! What the extensions UI shows and what a palette id means, decided from
//! podd's mod list alone (no window, no request): a mod's name and state
//! labels, its error line, the permission toggles a card offers, the facts
//! and checks lines, the Settings summary, the palette rows, the palette id
//! grammar and the preset a `layout:` id names, a notice's toast kind and
//! author, and the terminal a Windows toast click shows.

use podd::protocol::{LayoutPreset, ModInfo, ModListResult, ModState, Permission};

use super::copy::{self, s};
use crate::workspace::ToastKind;

pub(super) fn display_name(m: &ModInfo) -> String {
    m.title.clone().unwrap_or_else(|| m.name.clone())
}

pub(super) fn state_label(m: &ModInfo) -> String {
    match m.state {
        ModState::Running => s::RUNNING.into(),
        ModState::Disabled => s::DISABLED.into(),
        ModState::Stopped => format!("{}: {}", s::STOPPED, copy::stop_reason_label(m.stop_reason)),
        ModState::Invalid => s::INVALID.into(),
    }
}

/// The card's error line in Korean: what kind of problem, then the detail
/// (the detail is the daemon's message for the mod author).
pub(crate) fn error_summary(m: &ModInfo) -> Option<String> {
    let first = m.error.as_deref()?.lines().next().unwrap_or_default();
    Some(match m.state {
        ModState::Invalid if first.starts_with("layout ") => {
            format!("{}: {first}", s::LAYOUT_INVALID)
        }
        ModState::Invalid => format!("{}: {first}", s::MANIFEST_INVALID),
        ModState::Stopped => format!("{}: {first}", copy::stop_reason_label(m.stop_reason)),
        _ => first.to_string(),
    })
}

/// One permission toggle on a card (only permissions the mod requests).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct PermissionToggle {
    pub permission: Permission,
    pub granted: bool,
    pub label: &'static str,
    /// What it is now (허용 / 거부).
    pub verdict: &'static str,
    /// What a press makes it.
    pub next: &'static str,
}

pub(super) fn permission_toggles(m: &ModInfo) -> Vec<PermissionToggle> {
    m.permissions
        .iter()
        .filter(|p| p.requested)
        .map(|p| PermissionToggle {
            permission: p.name,
            granted: p.granted,
            label: copy::permission_label(p.name),
            verdict: if p.granted { s::ALLOWED } else { s::DENIED },
            next: if p.granted { s::DENIED } else { s::ALLOWED },
        })
        .collect()
}

/// What the mod provides: commands, detectors, presets, memory while running.
pub(super) fn facts_line(m: &ModInfo) -> Option<String> {
    let mut facts = Vec::new();
    if !m.commands.is_empty() {
        facts.push(copy::command_count(m.commands.len()));
    }
    if !m.detectors.is_empty() {
        let names: Vec<String> = m.detectors.iter().flat_map(|d| d.names.clone()).collect();
        facts.push(copy::detectors(&names.join(", ")));
    }
    if !m.layouts.is_empty() {
        facts.push(copy::layouts(
            &m.layouts
                .iter()
                .map(|l| l.title.clone())
                .collect::<Vec<_>>()
                .join(", "),
        ));
    }
    if m.state == ModState::Running && m.memory_kib > 0 {
        facts.push(copy::memory(m.memory_kib));
    }
    (!facts.is_empty()).then(|| facts.join(" · "))
}

/// The mod's check commands as `name = argv`.
pub(super) fn checks_line(m: &ModInfo) -> Option<String> {
    if m.checks.is_empty() {
        return None;
    }
    let checks: Vec<String> = m
        .checks
        .iter()
        .map(|c| format!("{} = {}", c.name, c.argv.join(" ")))
        .collect();
    Some(format!("{}: {}", s::CHECKS, checks.join(" · ")))
}

/// The Settings row's summary (empty until `mod.list` answered).
pub(super) fn installed_summary(list: Option<&ModListResult>) -> String {
    list.map(|l| {
        copy::installed(
            l.list.mods.len(),
            l.list.mods.iter().filter(|m| m.enabled).count(),
        )
    })
    .unwrap_or_default()
}

/// Palette rows: `(id, title, detail)` for running mods' commands and layouts.
pub(super) fn palette_rows(list: &ModListResult) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for m in list
        .list
        .mods
        .iter()
        .filter(|m| m.state == ModState::Running)
    {
        for c in &m.commands {
            out.push((
                format!("mod:{}:{}", m.name, c.id),
                format!("{}: {}", display_name(m), c.title),
                c.detail
                    .clone()
                    .unwrap_or_else(|| format!("{} {}", s::SECTION, m.name)),
            ));
        }
        for l in &m.layouts {
            out.push((
                format!("layout:{}:{}", m.name, l.id),
                format!("{}: {}", s::LAYOUT_PREFIX, l.title),
                l.description.clone().unwrap_or_else(|| display_name(m)),
            ));
        }
    }
    out
}

/// A palette id this module owns.
#[derive(Debug, PartialEq)]
pub(super) enum PaletteId<'a> {
    /// `mod:<name>:<command>` (the command keeps any further `:`).
    Command { name: &'a str, command: &'a str },
    /// `layout:<name>:<id>`.
    Layout { name: &'a str, layout: &'a str },
    /// `mod:` or `layout:` without the second part: ours, nothing to do.
    Incomplete,
}

/// `None`: not a mod or preset id (another module's).
pub(super) fn palette_id(id: &str) -> Option<PaletteId<'_>> {
    if let Some(rest) = id.strip_prefix("mod:") {
        return Some(match rest.split_once(':') {
            Some((name, command)) => PaletteId::Command { name, command },
            None => PaletteId::Incomplete,
        });
    }
    if let Some(rest) = id.strip_prefix("layout:") {
        return Some(match rest.split_once(':') {
            Some((name, layout)) => PaletteId::Layout { name, layout },
            None => PaletteId::Incomplete,
        });
    }
    None
}

/// The preset `layout:<name>:<layout>` names, if the list still has it.
pub(super) fn find_preset(
    list: Option<&ModListResult>,
    name: &str,
    layout: &str,
) -> Option<LayoutPreset> {
    list.and_then(|l| l.list.mods.iter().find(|m| m.name == name))
        .and_then(|m| m.layouts.iter().find(|l| l.id == layout))
        .cloned()
}

/// A `mod.notify` level as a toast kind (unknown levels are info).
pub(super) fn toast_kind(level: &str) -> ToastKind {
    match level {
        "success" => ToastKind::Success,
        "warning" => ToastKind::Warning,
        "error" => ToastKind::Error,
        _ => ToastKind::Info,
    }
}

/// Who a notice is from: the listed mod's title, else the name podd sent.
pub(super) fn notice_author(list: Option<&ModListResult>, mod_name: &str) -> String {
    list.and_then(|l| l.list.mods.iter().find(|m| m.name == mod_name))
        .map(display_name)
        .unwrap_or_else(|| mod_name.to_string())
}

/// The terminal a toast click should show: the notice's terminal if it still
/// exists, else the first terminal of its task.
pub(super) fn click_terminal(
    target: &crate::os_notify::Target,
    terminals: &[String],
    tasks: &[(String, Vec<String>)],
) -> Option<String> {
    target
        .terminal
        .clone()
        .filter(|t| terminals.contains(t))
        .or_else(|| {
            let task = target.task.as_deref()?;
            tasks
                .iter()
                .find(|(id, _)| id == task)?
                .1
                .iter()
                .find(|t| terminals.contains(t))
                .cloned()
        })
}
