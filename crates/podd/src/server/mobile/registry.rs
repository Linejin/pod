//! Paired devices and their per-task capabilities
//! (`<state_dir>\mobile\devices.json`, temp file + rename like the other
//! stores). A corrupt file is moved aside and the daemon starts with no
//! devices: everything fails closed.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::super::log;
use crate::protocol::*;

const REGISTRY_VERSION: u32 = 1;
/// Active (not revoked) devices at once (docs/17 §4.6).
pub const MAX_DEVICES: usize = 3;
const MAX_NAME: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// Placeholder until pairing (M2/M4) stores the real identity key.
    pub public_key: String,
    pub created_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_ms: Option<u64>,
    #[serde(default)]
    pub auth_epoch: u64,
    /// Task id -> capabilities. Absent task = nothing allowed.
    #[serde(default)]
    pub policy: BTreeMap<String, BTreeSet<MobileCap>>,
}

impl Device {
    pub fn active(&self) -> bool {
        self.revoked_ms.is_none()
    }

    pub fn info(&self, sessions: usize) -> MobileDeviceInfo {
        MobileDeviceInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            public_key: self.public_key.clone(),
            created_ms: self.created_ms,
            last_seen_ms: self.last_seen_ms,
            revoked_ms: self.revoked_ms,
            auth_epoch: self.auth_epoch,
            policy: self
                .policy
                .iter()
                .map(|(t, c)| (t.clone(), c.iter().copied().collect()))
                .collect(),
            sessions,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Registry {
    #[serde(default)]
    pub version: u32,
    /// Last device number handed out (`m<N>`, never reused).
    #[serde(default)]
    pub counter: u64,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub settings: MobileSettings,
}

fn denied(m: impl Into<String>) -> RpcError {
    RpcError::new(mobile_codes::MOBILE_DENIED, m)
}

impl Registry {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("mobile").join("devices.json")
    }

    pub fn load(state_dir: &Path) -> Self {
        let path = Self::path(state_dir);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(e) => {
                log(&format!(
                    "cannot read {}: {e}; no mobile devices",
                    path.display()
                ));
                return Self::default();
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(r) => r,
            Err(e) => {
                let aside = path.with_extension(format!("corrupt-{}.json", crate::now_ms()));
                log(&format!(
                    "mobile registry {} is corrupt ({e}); moved to {}; no mobile devices",
                    path.display(),
                    aside.display()
                ));
                let _ = std::fs::rename(&path, aside);
                Self::default()
            }
        }
    }

    pub fn save(&mut self, state_dir: &Path) -> Result<(), RpcError> {
        self.version = REGISTRY_VERSION;
        let path = Self::path(state_dir);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
            std::fs::rename(&tmp, &path)
        };
        write().map_err(|e| {
            RpcError::new(
                codes::INTERNAL_ERROR,
                format!("cannot save {}: {e}", path.display()),
            )
        })
    }

    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn device_mut(&mut self, id: &str) -> Result<&mut Device, RpcError> {
        self.devices
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| denied(format!("no mobile device {id:?}")))
    }

    /// The pairing stub (M1): registers a device directly. Real pairing (QR,
    /// one-time secret, PC confirmation) is M2/M4; until then this exists
    /// only in tests and debug builds.
    #[cfg(any(test, debug_assertions))]
    pub fn pair_stub(
        &mut self,
        name: &str,
        public_key: &str,
        now: u64,
    ) -> Result<Device, RpcError> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME || name.chars().any(char::is_control)
        {
            return Err(RpcError::new(codes::INVALID_PARAMS, "invalid device name"));
        }
        if self.devices.iter().filter(|d| d.active()).count() >= MAX_DEVICES {
            return Err(denied(format!(
                "at most {MAX_DEVICES} paired devices; revoke one first"
            )));
        }
        self.counter += 1;
        let d = Device {
            id: format!("m{}", self.counter),
            name: name.to_string(),
            public_key: public_key.to_string(),
            created_ms: now,
            last_seen_ms: None,
            revoked_ms: None,
            auth_epoch: 1,
            policy: BTreeMap::new(),
        };
        self.devices.push(d.clone());
        Ok(d)
    }

    /// Revoke: the epoch moves on, so every session of the device is dead.
    /// Returns false when it was already revoked.
    pub fn revoke(&mut self, id: &str, now: u64) -> Result<bool, RpcError> {
        let d = self.device_mut(id)?;
        if d.revoked_ms.is_some() {
            return Ok(false);
        }
        d.revoked_ms = Some(now);
        d.auth_epoch += 1;
        d.policy.clear();
        Ok(true)
    }

    pub fn set_policy(
        &mut self,
        id: &str,
        task: &str,
        allow: &[MobileCap],
        deny: &[MobileCap],
        clear: bool,
    ) -> Result<Device, RpcError> {
        let d = self.device_mut(id)?;
        if !d.active() {
            return Err(denied(format!("device {id} is revoked")));
        }
        let caps = d.policy.entry(task.to_string()).or_default();
        if clear {
            caps.clear();
        }
        caps.extend(allow.iter().copied());
        for c in deny {
            caps.remove(c);
        }
        if caps.is_empty() {
            d.policy.remove(task);
        }
        Ok(d.clone())
    }

    /// Deny by default: an active device with this capability on this task.
    pub fn allows(&self, device: &str, task: &str, cap: MobileCap) -> bool {
        self.device(device)
            .is_some_and(|d| d.active() && d.policy.get(task).is_some_and(|c| c.contains(&cap)))
    }

    /// Tasks the device may see (`status`), with all its capabilities there.
    pub fn visible_tasks(&self, device: &str) -> Vec<(String, Vec<MobileCap>)> {
        let Some(d) = self.device(device).filter(|d| d.active()) else {
            return Vec::new();
        };
        d.policy
            .iter()
            .filter(|(_, c)| c.contains(&MobileCap::Status))
            .map(|(t, c)| (t.clone(), c.iter().copied().collect()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pod-mobile-reg-{tag}-{}-{}",
            std::process::id(),
            crate::now_ms()
        ))
    }

    #[test]
    fn deny_by_default_and_per_task_caps() {
        let mut r = Registry::default();
        let d = r.pair_stub("Pixel", "pk", 10).unwrap();
        assert_eq!(d.id, "m1");
        for c in MobileCap::ALL {
            assert!(!r.allows("m1", "demo/a", c), "{c:?} allowed by default");
        }
        r.set_policy(
            "m1",
            "demo/a",
            &[MobileCap::Status, MobileCap::Approve],
            &[],
            false,
        )
        .unwrap();
        assert!(r.allows("m1", "demo/a", MobileCap::Approve));
        assert!(
            !r.allows("m1", "demo/a", MobileCap::Output),
            "output is never implied"
        );
        assert!(!r.allows("m1", "demo/a", MobileCap::Message));
        assert!(
            !r.allows("m1", "demo/b", MobileCap::Status),
            "other tasks stay hidden"
        );
        assert!(
            !r.allows("m2", "demo/a", MobileCap::Status),
            "unknown device"
        );
        r.set_policy("m1", "demo/a", &[], &[MobileCap::Approve], false)
            .unwrap();
        assert!(!r.allows("m1", "demo/a", MobileCap::Approve));
        assert_eq!(
            r.visible_tasks("m1"),
            vec![("demo/a".to_string(), vec![MobileCap::Status])]
        );
        r.set_policy("m1", "demo/a", &[], &[], true).unwrap();
        assert!(r.visible_tasks("m1").is_empty());
        assert!(
            r.device("m1").unwrap().policy.is_empty(),
            "empty entries are dropped"
        );
    }

    #[test]
    fn revoke_denies_and_frees_a_slot() {
        let mut r = Registry::default();
        for n in ["a", "b", "c"] {
            r.pair_stub(n, "pk", 1).unwrap();
        }
        assert_eq!(
            r.pair_stub("d", "pk", 1).unwrap_err().code,
            mobile_codes::MOBILE_DENIED
        );
        r.set_policy("m2", "demo/a", &[MobileCap::Status], &[], false)
            .unwrap();
        assert!(r.revoke("m2", 5).unwrap());
        assert!(!r.revoke("m2", 6).unwrap(), "second revoke is a no-op");
        assert!(!r.allows("m2", "demo/a", MobileCap::Status));
        assert_eq!(r.device("m2").unwrap().auth_epoch, 2);
        assert!(
            r.set_policy("m2", "demo/a", &[MobileCap::Status], &[], false)
                .is_err()
        );
        assert_eq!(
            r.pair_stub("d", "pk", 1).unwrap().id,
            "m4",
            "ids are never reused"
        );
        assert!(r.pair_stub(" ", "pk", 1).is_err());
        assert!(r.pair_stub("bad\u{1b}name", "pk", 1).is_err());
    }

    #[test]
    fn persists_and_fails_closed_on_corruption() {
        let dir = temp_dir("persist");
        let mut r = Registry::default();
        r.pair_stub("Pixel", "pk", 1).unwrap();
        r.set_policy("m1", "demo/a", &[MobileCap::Message], &[], false)
            .unwrap();
        r.settings.audit_message_bodies = true;
        r.save(&dir).unwrap();
        let back = Registry::load(&dir);
        assert_eq!(back.version, REGISTRY_VERSION);
        assert!(back.allows("m1", "demo/a", MobileCap::Message));
        assert!(back.settings.audit_message_bodies);
        std::fs::write(Registry::path(&dir), b"{not json").unwrap();
        let empty = Registry::load(&dir);
        assert!(empty.devices.is_empty());
        assert!(!empty.allows("m1", "demo/a", MobileCap::Message));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
