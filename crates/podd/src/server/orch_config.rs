//! Orchestration defaults a person can change from the app (docs/19 §8
//! "설정 > 오케스트레이션", §15.3): `orch.config.get/set` — auto-release and
//! stall timings, nesting depth and the default worker place. Worker and
//! resource limits are `orch.limits.*` (`orch_limits.rs`).
//!
//! Layers, lowest first: built-in defaults, `<state>\orch\config.json`
//! (written by `set`), then the daemon's environment
//! (`POD_ORCH_RELEASE_AFTER`, `POD_ORCH_STALL_AFTER`, `POD_ORCH_MAX_DEPTH`,
//! `POD_ORCH_DEFAULT_PLACE`). A field set by the environment is reported in
//! `locked` and `set` leaves it alone, so tests and scripts that pin a value
//! keep it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use pod_orch::engine::Limits;

use crate::protocol::OrchSettings;

pub const ENV_RELEASE_AFTER: &str = "POD_ORCH_RELEASE_AFTER";
pub const ENV_STALL_AFTER: &str = "POD_ORCH_STALL_AFTER";
pub const ENV_MAX_DEPTH: &str = "POD_ORCH_MAX_DEPTH";
pub const ENV_DEFAULT_PLACE: &str = "POD_ORCH_DEFAULT_PLACE";

/// Field names of [`OrchSettings`] as reported in `locked`.
pub const RELEASE: &str = "release_after_min";
pub const STALL: &str = "stall_after_min";
pub const DEPTH: &str = "max_depth";
pub const PLACE: &str = "default_place";

/// Upper bounds `set` accepts (a typo must not disable the safety limits).
pub const MAX_DEPTH_LIMIT: u32 = 8;
pub const MINUTES_LIMIT: u64 = 7 * 24 * 60;

/// Timings and defaults in effect.
#[derive(Debug, Clone, PartialEq)]
pub struct OrchConfig {
    /// Default 10m, `None` = never.
    pub release_after: Option<Duration>,
    /// Default 20m, `None` = never.
    pub stall_after: Option<Duration>,
    /// Depth 3. `run_worker_cap` follows the resource limits' `run_workers`
    /// (`orch_limits.rs`); the daemon sets it after loading.
    pub limits: Limits,
    /// `new` (default) or `current`.
    pub default_place: String,
    /// Fields pinned by the environment.
    pub locked: Vec<String>,
}

impl Default for OrchConfig {
    fn default() -> Self {
        OrchConfig {
            release_after: Some(Duration::from_secs(600)),
            stall_after: Some(Duration::from_secs(20 * 60)),
            limits: Limits::default(),
            default_place: "new".into(),
            locked: Vec::new(),
        }
    }
}

pub fn config_path(state_dir: &Path) -> PathBuf {
    state_dir.join("orch").join("config.json")
}

fn minutes(d: Option<Duration>) -> Option<u64> {
    // Sub-minute values (tests) show as 1 minute rather than "never".
    d.map(|d| (d.as_secs().div_ceil(60)).max(1))
}

fn from_minutes(m: Option<u64>) -> Option<Duration> {
    m.filter(|m| *m > 0).map(|m| Duration::from_secs(m * 60))
}

impl OrchConfig {
    /// Defaults, then the saved file, then `env` (a lookup, so tests need not
    /// touch the process environment).
    pub fn load_with(
        state_dir: &Path,
        env: impl Fn(&str) -> Option<String>,
    ) -> (Self, Option<String>) {
        let mut c = OrchConfig::default();
        let mut note = None;
        match std::fs::read_to_string(config_path(state_dir)) {
            Ok(text) => match serde_json::from_str::<OrchSettings>(&text) {
                Ok(s) => c.apply(&s),
                Err(e) => note = Some(format!("orch config.json ignored: {e}")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => note = Some(format!("orch config.json unreadable: {e}")),
        }
        let dur = |k: &str| env(k).and_then(|v| super::retention::parse_duration(&v));
        if let Some(d) = dur(ENV_RELEASE_AFTER) {
            c.release_after = d;
            c.locked.push(RELEASE.into());
        }
        if let Some(d) = dur(ENV_STALL_AFTER) {
            c.stall_after = d;
            c.locked.push(STALL.into());
        }
        if let Some(n) = env(ENV_MAX_DEPTH).and_then(|v| v.trim().parse::<u32>().ok()) {
            c.limits.max_depth = n;
            c.locked.push(DEPTH.into());
        }
        if let Some(p) = env(ENV_DEFAULT_PLACE)
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| valid_place(p))
        {
            c.default_place = p;
            c.locked.push(PLACE.into());
        }
        (c, note)
    }

    pub fn load(state_dir: &Path) -> (Self, Option<String>) {
        Self::load_with(state_dir, |k| std::env::var(k).ok())
    }

    /// Apply settings to the fields that are not locked.
    fn apply(&mut self, s: &OrchSettings) {
        let free = |f: &str| !self.locked.iter().any(|l| l == f);
        let (r, st, d, p) = (free(RELEASE), free(STALL), free(DEPTH), free(PLACE));
        if r {
            self.release_after = from_minutes(s.release_after_min);
        }
        if st {
            self.stall_after = from_minutes(s.stall_after_min);
        }
        if d {
            self.limits.max_depth = s.max_depth.clamp(1, MAX_DEPTH_LIMIT);
        }
        if p && valid_place(&s.default_place) {
            self.default_place = s.default_place.clone();
        }
    }

    pub fn settings(&self) -> OrchSettings {
        OrchSettings {
            release_after_min: minutes(self.release_after),
            stall_after_min: minutes(self.stall_after),
            max_depth: self.limits.max_depth,
            default_place: self.default_place.clone(),
            locked: self.locked.clone(),
        }
    }

    /// `orch.config.set`: validate, apply, save. Returns the settings in effect.
    pub fn set(&mut self, state_dir: &Path, s: &OrchSettings) -> Result<OrchSettings, String> {
        if !valid_place(&s.default_place) {
            return Err(format!(
                "default_place must be \"new\" or \"current\", not {:?}",
                s.default_place
            ));
        }
        if s.max_depth == 0 || s.max_depth > MAX_DEPTH_LIMIT {
            return Err(format!("max_depth must be 1..={MAX_DEPTH_LIMIT}"));
        }
        for m in [s.release_after_min, s.stall_after_min]
            .into_iter()
            .flatten()
        {
            if m == 0 || m > MINUTES_LIMIT {
                return Err(format!(
                    "minutes must be 1..={MINUTES_LIMIT} (or null for never)"
                ));
            }
        }
        self.apply(s);
        // The file keeps what the person chose (locked fields included: they
        // apply once the environment variable is gone).
        let mut saved = s.clone();
        saved.locked.clear();
        let path = config_path(state_dir);
        let text = serde_json::to_string_pretty(&saved).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(self.settings())
    }
}

fn valid_place(p: &str) -> bool {
    matches!(p, "new" | "current")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("podd-orch-config-{tag}-{}-{n}", std::process::id()))
    }

    #[test]
    fn defaults_file_and_env_layers() {
        let dir = temp("layers");
        let (c, note) = OrchConfig::load_with(&dir, |_| None);
        assert!(note.is_none());
        let s = c.settings();
        assert_eq!(
            (s.release_after_min, s.stall_after_min, s.max_depth),
            (Some(10), Some(20), 3)
        );
        assert_eq!(s.default_place, "new");
        assert!(s.locked.is_empty());

        let mut c = c;
        let want = OrchSettings {
            release_after_min: None,
            stall_after_min: Some(45),
            max_depth: 2,
            default_place: "current".into(),
            locked: vec![],
        };
        assert_eq!(c.set(&dir, &want).unwrap(), want);
        assert_eq!(c.release_after, None, "never release");
        // Reload: the file wins over defaults, the environment over the file.
        let env = |k: &str| match k {
            ENV_RELEASE_AFTER => Some("3s".to_string()),
            ENV_MAX_DEPTH => Some("4".to_string()),
            _ => None,
        };
        let (mut c2, _) = OrchConfig::load_with(&dir, env);
        let s2 = c2.settings();
        assert_eq!(s2.release_after_min, Some(1), "3 s shows as 1 minute");
        assert_eq!(c2.release_after, Some(Duration::from_secs(3)));
        assert_eq!(
            (s2.stall_after_min, s2.max_depth, s2.default_place.as_str()),
            (Some(45), 4, "current")
        );
        assert_eq!(s2.locked, vec![RELEASE.to_string(), DEPTH.to_string()]);
        // `set` keeps locked fields as the environment says.
        let r = c2
            .set(
                &dir,
                &OrchSettings {
                    max_depth: 1,
                    release_after_min: Some(30),
                    ..want.clone()
                },
            )
            .unwrap();
        assert_eq!(
            (r.max_depth, c2.release_after),
            (4, Some(Duration::from_secs(3)))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_rejects_bad_values_and_bad_files_are_ignored() {
        let dir = temp("bad");
        let (mut c, _) = OrchConfig::load_with(&dir, |_| None);
        let ok = c.settings();
        for bad in [
            OrchSettings {
                default_place: "elsewhere".into(),
                ..ok.clone()
            },
            OrchSettings {
                max_depth: 0,
                ..ok.clone()
            },
            OrchSettings {
                max_depth: 99,
                ..ok.clone()
            },
            OrchSettings {
                release_after_min: Some(0),
                ..ok.clone()
            },
        ] {
            assert!(c.set(&dir, &bad).is_err(), "{bad:?}");
        }
        assert_eq!(c.settings(), ok, "nothing changed");
        std::fs::create_dir_all(dir.join("orch")).unwrap();
        std::fs::write(config_path(&dir), "{ not json").unwrap();
        let (c, note) = OrchConfig::load_with(&dir, |_| None);
        assert!(note.unwrap().contains("ignored"));
        assert_eq!(c.settings(), ok);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
