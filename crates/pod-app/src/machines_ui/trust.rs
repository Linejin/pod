//! What the user confirmed about each machine (docs/22 TM-13): the profile's
//! target, the host key fingerprints known_hosts had for it, and the remote
//! Pod's `machine_id`. Kept in `machine-trust.json` next to `machines.json`.
//!
//! This file is Pod's own record. **known_hosts is never written**: ssh
//! (`StrictHostKeyChecking=yes`) stays the gate that refuses unknown or
//! changed keys; this record only makes the app ask again before any
//! change is sent when the key, the machine or the target differs from what
//! the user saw last time.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "machine-trust.json";
const FILE_VERSION: u32 = 1;

/// One confirmed machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pin {
    /// Profile name.
    pub name: String,
    /// `user@host:port` of the profile when confirmed.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known_hosts: Option<PathBuf>,
    /// `ED25519 SHA256:…` lines from known_hosts (empty: could not be read).
    pub fingerprints: Vec<String>,
    pub machine_id: String,
    pub confirmed_ms: u64,
}

/// What the current connection showed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub target: String,
    pub known_hosts: Option<PathBuf>,
    /// `Err`: known_hosts could not be read (shown, and pinned as empty).
    pub fingerprints: Result<Vec<String>, String>,
    pub machine_id: String,
}

impl Identity {
    pub fn fingerprint_list(&self) -> Vec<String> {
        self.fingerprints.clone().unwrap_or_default()
    }

    pub fn pin(&self, name: &str, now_ms: u64) -> Pin {
        Pin {
            name: name.to_string(),
            target: self.target.clone(),
            known_hosts: self.known_hosts.clone(),
            fingerprints: self.fingerprint_list(),
            machine_id: self.machine_id.clone(),
            confirmed_ms: now_ms,
        }
    }
}

/// Why the user must confirm before anything is sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// Never confirmed (a new profile, or a renamed one: another alias).
    First,
    /// The profile now points elsewhere (host, user, port or known_hosts).
    Profile { old_target: String },
    /// known_hosts holds another key for the host than the one confirmed.
    HostKey { old: Vec<String> },
    /// Another remote Pod answered (another `machine_id`).
    Machine { old: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trust {
    Confirmed,
    Needs(Confirm),
}

/// Compare the connection with the record. The first difference wins, in
/// the order the user can act on: target, host key, machine.
pub fn evaluate(pin: Option<&Pin>, id: &Identity) -> Trust {
    let Some(pin) = pin else {
        return Trust::Needs(Confirm::First);
    };
    if pin.target != id.target || pin.known_hosts != id.known_hosts {
        return Trust::Needs(Confirm::Profile {
            old_target: pin.target.clone(),
        });
    }
    if pin.fingerprints != id.fingerprint_list() {
        return Trust::Needs(Confirm::HostKey {
            old: pin.fingerprints.clone(),
        });
    }
    if pin.machine_id != id.machine_id {
        return Trust::Needs(Confirm::Machine {
            old: pin.machine_id.clone(),
        });
    }
    Trust::Confirmed
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileFormat {
    version: u32,
    #[serde(default)]
    pins: Vec<Pin>,
}

/// The record file.
#[derive(Clone, Debug)]
pub struct TrustStore {
    path: PathBuf,
}

impl TrustStore {
    /// Next to the profiles file.
    pub fn beside(machines_json: &Path) -> Self {
        Self {
            path: machines_json
                .parent()
                .unwrap_or(Path::new("."))
                .join(FILE_NAME),
        }
    }

    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// All pins; a missing file is none. A damaged file is an error (and is
    /// never overwritten): every machine then asks again.
    pub fn load(&self) -> Result<Vec<Pin>, String> {
        let bytes = match std::fs::read(&self.path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{}: {e}", self.path.display())),
        };
        let f: FileFormat =
            serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", self.path.display()))?;
        if f.version != FILE_VERSION {
            return Err(format!(
                "{}: version {} (expected {FILE_VERSION})",
                self.path.display(),
                f.version
            ));
        }
        Ok(f.pins)
    }

    pub fn get(&self, name: &str) -> Option<Pin> {
        self.load().ok()?.into_iter().find(|p| p.name == name)
    }

    /// Add or replace the pin of `pin.name`.
    pub fn put(&self, pin: Pin) -> Result<(), String> {
        let mut all = self.load()?;
        all.retain(|p| p.name != pin.name);
        all.push(pin);
        self.save(all)
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut all = self.load()?;
        let before = all.len();
        all.retain(|p| p.name != name);
        if all.len() == before {
            return Ok(());
        }
        self.save(all)
    }

    fn save(&self, pins: Vec<Pin>) -> Result<(), String> {
        let text = serde_json::to_string_pretty(&FileFormat {
            version: FILE_VERSION,
            pins,
        })
        .map_err(|e| e.to_string())?;
        let dir = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let tmp = self
            .path
            .with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&tmp, text.as_bytes())
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                format!("{}: {e}", self.path.display())
            })
    }
}
