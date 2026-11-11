//! The enable/grant state kept in the mods folder ([`STATE_FILE`]).
//!
//! JSON adapter over `pod-storage`: pretty JSON written to
//! `pod-mods.json.tmp`, then renamed over the file. The folder is not
//! created here (it is the mods folder itself), a failure reads
//! `"<path>: <error>"` and leaves the temporary file, and an unreadable or
//! corrupt file loads as empty.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{Manager, STATE_FILE};
use crate::manifest::Permission;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct StateFile {
    #[serde(default)]
    pub(super) mods: BTreeMap<String, SavedMod>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(super) struct SavedMod {
    #[serde(default)]
    pub(super) enabled: bool,
    #[serde(default)]
    pub(super) granted: Vec<Permission>,
}

pub(super) fn load_state(root: &Path) -> StateFile {
    std::fs::read(root.join(STATE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn save_state(root: &Path, s: &StateFile) -> Result<(), String> {
    let path = root.join(STATE_FILE);
    let tmp = path.with_extension("json.tmp");
    let text = pod_storage::json::to_pretty_bytes(s).map_err(|e| e.to_string())?;
    pod_storage::replace::write_then_rename(&path, &tmp, &text)
        .map_err(|e| format!("{}: {e}", path.display()))
}

impl Manager {
    /// Writes every known mod's enable flag and grants (in permission
    /// order) over what the file holds, keeping entries of mods that are
    /// gone.
    pub(super) fn save(&self) -> Result<(), String> {
        let mut st = load_state(&self.cfg.root);
        for s in self.slots.values() {
            let c = s.core.borrow();
            st.mods.insert(
                c.name.clone(),
                SavedMod {
                    enabled: c.enabled,
                    granted: c.granted.iter().copied().collect(),
                },
            );
        }
        save_state(&self.cfg.root, &st)
    }
}
