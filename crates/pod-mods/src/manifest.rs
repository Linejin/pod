//! `mod.json`: name, version, entry, permissions, check commands and layout
//! presets (docs/16 §3.1, §7.2). Validation is strict: a bad manifest marks
//! only that mod `invalid`.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Mod API version understood by this host.
pub const API_VERSION: u32 = 1;
pub const MANIFEST_FILE: &str = "mod.json";
pub const MAX_ENTRY_BYTES: u64 = 512 * 1024;
pub const MAX_LAYOUT_BYTES: u64 = 64 * 1024;
pub const MAX_LAYOUTS: usize = 8;
pub const MAX_CHECKS: usize = 8;
const MAX_LAYOUT_DEPTH: usize = 4;
const MAX_LAYOUT_LEAVES: usize = 16;

/// A capability a mod asks for in its manifest and the user grants.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum Permission {
    #[serde(rename = "notify")]
    Notify,
    #[serde(rename = "commands")]
    Commands,
    #[serde(rename = "status")]
    Status,
    #[serde(rename = "terminal.send")]
    TerminalSend,
    #[serde(rename = "check")]
    Check,
    #[serde(rename = "gate")]
    Gate,
    /// Read orchestration events (`orch.changed`, `orch.mail` subjects),
    /// docs/19 §16.2. Needs an explicit grant: mail subjects are the
    /// agents' words.
    #[serde(rename = "orch")]
    Orch,
}

impl Permission {
    pub const ALL: [Permission; 7] = [
        Permission::Notify,
        Permission::Commands,
        Permission::Status,
        Permission::TerminalSend,
        Permission::Check,
        Permission::Gate,
        Permission::Orch,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Permission::Notify => "notify",
            Permission::Commands => "commands",
            Permission::Status => "status",
            Permission::TerminalSend => "terminal.send",
            Permission::Check => "check",
            Permission::Gate => "gate",
            Permission::Orch => "orch",
        }
    }

    pub fn parse(s: &str) -> Option<Permission> {
        Permission::ALL.into_iter().find(|p| p.name() == s)
    }

    /// Granted automatically when the user enables a mod. Typing into
    /// terminals and running commands need an explicit grant.
    pub fn granted_by_default(self) -> bool {
        !matches!(
            self,
            Permission::TerminalSend | Permission::Check | Permission::Orch
        )
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    pod: u32,
    name: String,
    version: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    entry: Option<String>,
    #[serde(default)]
    permissions: Vec<String>,
    #[serde(default)]
    checks: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    layouts: Vec<String>,
}

/// A validated manifest. Paths are resolved inside the mod folder.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub entry: Option<PathBuf>,
    pub permissions: Vec<Permission>,
    pub checks: BTreeMap<String, Vec<String>>,
    pub layouts: Vec<LayoutPreset>,
}

pub fn valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 48
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// Command / layout ids: `[a-z0-9-]{1,32}`.
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// A relative path that stays inside the mod folder.
fn inside(dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel);
    if rel.is_empty()
        || p.is_absolute()
        || p.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(format!(
            "path {rel:?} must be relative and stay inside the mod folder"
        ));
    }
    Ok(dir.join(p))
}

fn short(field: &str, v: Option<String>, max: usize) -> Result<Option<String>, String> {
    match v {
        Some(s) if s.chars().count() > max => {
            Err(format!("{field} is longer than {max} characters"))
        }
        other => Ok(other),
    }
}

impl Manifest {
    /// Read and validate `<dir>/mod.json` (and its layout files).
    pub fn load(dir: &Path) -> Result<Manifest, String> {
        let text = std::fs::read_to_string(dir.join(MANIFEST_FILE))
            .map_err(|e| format!("{MANIFEST_FILE}: {e}"))?;
        let folder = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        Self::parse(&text, dir, &folder)
    }

    /// Validate manifest text; `folder` is the mod folder's name.
    pub fn parse(text: &str, dir: &Path, folder: &str) -> Result<Manifest, String> {
        let raw: RawManifest =
            serde_json::from_str(text).map_err(|e| format!("{MANIFEST_FILE}: {e}"))?;
        if raw.pod != API_VERSION {
            return Err(format!(
                "unsupported mod API version {} (this Pod supports {API_VERSION})",
                raw.pod
            ));
        }
        if !valid_name(&raw.name) {
            return Err(format!(
                "name {:?} must match [a-z0-9][a-z0-9-]{{0,47}}",
                raw.name
            ));
        }
        if raw.name != folder {
            return Err(format!(
                "name {:?} must equal the folder name {folder:?}",
                raw.name
            ));
        }
        if raw.version.is_empty() || raw.version.len() > 64 {
            return Err("version must be 1-64 characters".into());
        }
        let mut permissions = Vec::new();
        for p in &raw.permissions {
            let perm = Permission::parse(p).ok_or_else(|| format!("unknown permission {p:?}"))?;
            if !permissions.contains(&perm) {
                permissions.push(perm);
            }
        }
        let entry = match &raw.entry {
            Some(e) => {
                if !e.ends_with(".js") {
                    return Err(format!("entry {e:?} must be a .js file"));
                }
                Some(inside(dir, e)?)
            }
            None => None,
        };
        if raw.checks.len() > MAX_CHECKS {
            return Err(format!("at most {MAX_CHECKS} checks"));
        }
        for (name, argv) in &raw.checks {
            if !valid_id(name) {
                return Err(format!("check name {name:?} must match [a-z0-9-]{{1,32}}"));
            }
            if argv.is_empty() || argv[0].trim().is_empty() {
                return Err(format!("check {name:?} needs a program"));
            }
        }
        if raw.layouts.len() > MAX_LAYOUTS {
            return Err(format!("at most {MAX_LAYOUTS} layouts"));
        }
        let mut layouts = Vec::new();
        for rel in &raw.layouts {
            let path = inside(dir, rel)?;
            let size = std::fs::metadata(&path)
                .map_err(|e| format!("layout {rel}: {e}"))?
                .len();
            if size > MAX_LAYOUT_BYTES {
                return Err(format!(
                    "layout {rel} is larger than {MAX_LAYOUT_BYTES} bytes"
                ));
            }
            let text = std::fs::read_to_string(&path).map_err(|e| format!("layout {rel}: {e}"))?;
            let preset = LayoutPreset::parse(&text).map_err(|e| format!("layout {rel}: {e}"))?;
            if layouts.iter().any(|l: &LayoutPreset| l.id == preset.id) {
                return Err(format!("layout id {:?} is used twice", preset.id));
            }
            layouts.push(preset);
        }
        Ok(Manifest {
            name: raw.name,
            version: raw.version,
            title: short("title", raw.title, 120)?,
            description: short("description", raw.description, 500)?,
            entry,
            permissions,
            checks: raw.checks,
            layouts,
        })
    }
}

// ───────────────────────────── layouts ─────────────────────────────

/// A declarative dock arrangement (docs/16 §7.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LayoutPreset {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub center: LayoutNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Axis {
    /// Side by side.
    #[serde(rename = "h")]
    Horizontal,
    /// Stacked.
    #[serde(rename = "v")]
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum PanelKind {
    Terminal,
    Terminals,
    Review,
    Editor,
    Gates,
    Agents,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum LayoutNode {
    Split {
        split: Axis,
        children: Vec<LayoutNode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f32>,
    },
    Tabs {
        tabs: Vec<LayoutNode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f32>,
    },
    Panel {
        panel: PanelKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        size: Option<f32>,
    },
}

impl LayoutNode {
    pub fn size(&self) -> Option<f32> {
        match self {
            LayoutNode::Split { size, .. }
            | LayoutNode::Tabs { size, .. }
            | LayoutNode::Panel { size, .. } => *size,
        }
    }

    fn check(&self, depth: usize, leaves: &mut usize) -> Result<(), String> {
        if depth > MAX_LAYOUT_DEPTH {
            return Err(format!("nested deeper than {MAX_LAYOUT_DEPTH}"));
        }
        if let Some(s) = self.size()
            && !(s > 0.0 && s <= 1.0)
        {
            return Err(format!("size {s} must be in (0, 1]"));
        }
        match self {
            LayoutNode::Split { children, .. } => {
                if !(2..=6).contains(&children.len()) {
                    return Err("a split needs 2-6 children".into());
                }
                children.iter().try_for_each(|c| c.check(depth + 1, leaves))
            }
            LayoutNode::Tabs { tabs, .. } => {
                if tabs.is_empty() || tabs.len() > 8 {
                    return Err("tabs need 1-8 panels".into());
                }
                for t in tabs {
                    if !matches!(t, LayoutNode::Panel { .. }) {
                        return Err("tabs may only hold panels".into());
                    }
                    t.check(depth + 1, leaves)?;
                }
                Ok(())
            }
            LayoutNode::Panel { panel, index, .. } => {
                *leaves += 1;
                if *leaves > MAX_LAYOUT_LEAVES {
                    return Err(format!("more than {MAX_LAYOUT_LEAVES} panels"));
                }
                match (panel, index) {
                    (PanelKind::Terminal, Some(i)) if *i < 16 => Ok(()),
                    (PanelKind::Terminal, Some(_)) => Err("terminal index must be below 16".into()),
                    (PanelKind::Terminal, None) => Err("a terminal panel needs an index".into()),
                    (_, Some(_)) => Err("only terminal panels take an index".into()),
                    _ => Ok(()),
                }
            }
        }
    }
}

impl LayoutPreset {
    pub fn parse(text: &str) -> Result<LayoutPreset, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let p: LayoutPreset =
            serde_json::from_value(v).map_err(|e| format!("not a layout preset: {e}"))?;
        if !valid_id(&p.id) {
            return Err(format!("id {:?} must match [a-z0-9-]{{1,32}}", p.id));
        }
        if p.title.trim().is_empty() || p.title.chars().count() > 60 {
            return Err("title must be 1-60 characters".into());
        }
        let mut leaves = 0;
        p.center.check(1, &mut leaves)?;
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_with(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (name, text) in files {
            let p = d.path().join(name);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        d
    }

    #[test]
    fn manifest_rules() {
        let d = dir_with(&[(
            "l.json",
            r#"{"id":"four","title":"4분할","center":{"split":"v","children":[{"panel":"terminal","index":0},{"panel":"gates"}]}}"#,
        )]);
        let ok = r#"{"pod":1,"name":"m","version":"1","entry":"main.js","permissions":["notify","status"],"checks":{"test":["cargo","test"]},"layouts":["l.json"]}"#;
        let m = Manifest::parse(ok, d.path(), "m").unwrap();
        assert_eq!(m.permissions, vec![Permission::Notify, Permission::Status]);
        assert_eq!(m.layouts[0].id, "four");
        assert!(m.entry.unwrap().ends_with("main.js"));

        let bad = |text: &str| Manifest::parse(text, d.path(), "m").unwrap_err();
        assert!(bad(r#"{"pod":2,"name":"m","version":"1"}"#).contains("API version"));
        assert!(bad(r#"{"pod":1,"name":"M","version":"1"}"#).contains("name"));
        assert!(bad(r#"{"pod":1,"name":"other","version":"1"}"#).contains("folder"));
        assert!(
            bad(r#"{"pod":1,"name":"m","version":"1","permissions":["fs"]}"#)
                .contains("unknown permission")
        );
        assert!(bad(r#"{"pod":1,"name":"m","version":"1","entry":"../x.js"}"#).contains("inside"));
        assert!(bad(r#"{"pod":1,"name":"m","version":"1","entry":"C:/x.js"}"#).contains("inside"));
        assert!(bad(r#"{"pod":1,"name":"m","version":"1","entry":"main.ts"}"#).contains(".js"));
        assert!(bad(r#"{"pod":1,"name":"m","version":"1","checks":{"t":[]}}"#).contains("program"));
        assert!(bad(r#"{"pod":1,"name":"m","version":"1","extra":1}"#).contains("unknown field"));
        assert!(
            bad(r#"{"pod":1,"name":"m","version":"1","layouts":["missing.json"]}"#)
                .contains("missing.json")
        );
    }

    #[test]
    fn layout_rules() {
        let p = |s: &str| LayoutPreset::parse(s);
        assert!(p(r#"{"id":"a","title":"A","center":{"panel":"review"}}"#).is_ok());
        assert!(p(r#"{"id":"a","title":"A","center":{"tabs":[{"panel":"terminals"},{"panel":"agents"}]}}"#).is_ok());
        assert!(
            p(r#"{"id":"a","title":"A","center":{"panel":"terminal"}}"#)
                .unwrap_err()
                .contains("index")
        );
        assert!(
            p(r#"{"id":"a","title":"A","center":{"panel":"review","index":1}}"#)
                .unwrap_err()
                .contains("index")
        );
        assert!(
            p(r#"{"id":"a","title":"A","center":{"split":"h","children":[{"panel":"review"}]}}"#)
                .unwrap_err()
                .contains("2-6")
        );
        assert!(p(r#"{"id":"a","title":"A","center":{"panel":"browser"}}"#).is_err());
        assert!(
            p(r#"{"id":"A B","title":"A","center":{"panel":"review"}}"#)
                .unwrap_err()
                .contains("id")
        );
        assert!(
            p(r#"{"id":"a","title":"A","center":{"panel":"review","size":2}}"#)
                .unwrap_err()
                .contains("size")
        );
        let deep = r#"{"id":"a","title":"A","center":{"split":"h","children":[{"panel":"gates"},{"split":"v","children":[{"panel":"gates"},{"split":"h","children":[{"panel":"gates"},{"split":"v","children":[{"panel":"gates"},{"panel":"agents"}]}]}]}]}}"#;
        assert!(p(deep).unwrap_err().contains("deeper"));
    }
}
