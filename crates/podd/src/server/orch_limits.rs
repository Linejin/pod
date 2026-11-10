//! Resource limits of the worker queue as daemon settings (docs/19 §5.1,
//! §14.4): `<state>\orch\limits.json` written by `orch.limits.set`
//! (`pod orch limits`), read at start. An environment variable still wins
//! over the file for its key (tests and one-off overrides); `get` says which
//! source each value comes from.
//!
//! The file holds the same text forms the variables take (`4`, `70%`,
//! `4G`, `30m`, `off`), so one parser serves both.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pod_orch::ResourceLimits;
use serde::{Deserialize, Serialize};

use super::orch_runs::{parse_bytes, parse_pct};
use super::retention::parse_duration;
use crate::protocol::{OrchLimitEntry, ResourceLimitsInfo};

/// Worker counts accept 1..=this.
pub const MAX_COUNT: usize = 64;

/// One settable limit: key, environment variable, Korean help.
pub struct Key {
    pub name: &'static str,
    pub env: &'static str,
    pub help: &'static str,
}

pub const KEYS: &[Key] = &[
    Key {
        name: "max_workers",
        env: "POD_MAX_WORKERS",
        help: "동시에 일하는 워커 수(전체), 1–64",
    },
    Key {
        name: "run_workers",
        env: "POD_ORCH_RUN_WORKERS",
        help: "Run 하나의 열린 워커 수, 1–64",
    },
    Key {
        name: "cpu_max_pct",
        env: "POD_ORCH_CPU_LIMIT",
        help: "최근 1분 CPU가 이 %를 넘으면 새 워커 대기, 1–100 또는 off",
    },
    Key {
        name: "min_free_mem",
        env: "POD_ORCH_MIN_FREE_MEM",
        help: "사용 가능 메모리가 이보다 적으면 대기(예: 4G), 또는 off",
    },
    Key {
        name: "min_free_disk",
        env: "POD_ORCH_MIN_FREE_DISK",
        help: "worktree 드라이브 여유가 이보다 적으면 대기(예: 20G), 또는 off",
    },
    Key {
        name: "cpu_max_wait",
        env: "POD_ORCH_CPU_MAX_WAIT",
        help: "CPU 때문에 기다리는 최대 시간(예: 30m), 또는 off",
    },
];

fn key_names() -> String {
    KEYS.iter().map(|k| k.name).collect::<Vec<_>>().join(", ")
}

/// A parsed value of one key.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Val {
    Count(usize),
    Pct(Option<f64>),
    Bytes(Option<u64>),
    Ms(Option<u64>),
}

/// Parse and check `value` for `key`. Errors are Korean, for people.
fn parse(key: &str, value: &str) -> Result<Val, String> {
    let v = value.trim();
    let bad = |what: &str| format!("{key}: '{value}'은(는) 쓸 수 없어요. {what}");
    match key {
        "max_workers" | "run_workers" => match v.parse::<usize>() {
            Ok(n) if (1..=MAX_COUNT).contains(&n) => Ok(Val::Count(n)),
            _ => Err(bad(&format!("1–{MAX_COUNT} 사이의 정수를 주세요."))),
        },
        "cpu_max_pct" => match parse_pct(v) {
            Some(Some(p)) if p >= 1.0 => Ok(Val::Pct(Some(p))),
            Some(None) => Ok(Val::Pct(None)),
            _ => Err(bad("1–100 사이의 퍼센트(예: 70, 70%) 또는 off를 주세요.")),
        },
        "min_free_mem" | "min_free_disk" => match parse_bytes(v) {
            Some(b) if !v.is_empty() => Ok(Val::Bytes(b)),
            _ => Err(bad("크기(예: 4G, 512M, 1.5GiB) 또는 off를 주세요.")),
        },
        "cpu_max_wait" => match parse_duration(v) {
            Some(d) if !v.is_empty() => Ok(Val::Ms(d.map(|d| d.as_millis() as u64))),
            _ => Err(bad("시간(예: 30m, 90s, 2h) 또는 off를 주세요.")),
        },
        _ => Err(format!(
            "알 수 없는 한도 '{key}'. 쓸 수 있는 것: {}",
            key_names()
        )),
    }
}

fn apply(r: &mut ResourceLimits, key: &str, v: Val) {
    match (key, v) {
        ("max_workers", Val::Count(n)) => r.max_workers = n,
        ("run_workers", Val::Count(n)) => r.run_workers = n,
        ("cpu_max_pct", Val::Pct(p)) => r.cpu_max_pct = p,
        ("min_free_mem", Val::Bytes(b)) => r.min_free_mem = b,
        ("min_free_disk", Val::Bytes(b)) => r.min_free_disk = b,
        ("cpu_max_wait", Val::Ms(m)) => r.cpu_max_wait_ms = m,
        _ => {}
    }
}

fn show_bytes(b: Option<u64>) -> String {
    match b {
        None => "off".into(),
        Some(b) if b != 0 && b % (1 << 30) == 0 => format!("{}G", b >> 30),
        Some(b) if b != 0 && b % (1 << 20) == 0 => format!("{}M", b >> 20),
        Some(b) => b.to_string(),
    }
}

fn show_ms(m: Option<u64>) -> String {
    match m {
        None => "off".into(),
        Some(m) if m % 3_600_000 == 0 => format!("{}h", m / 3_600_000),
        Some(m) if m % 60_000 == 0 => format!("{}m", m / 60_000),
        Some(m) if m % 1_000 == 0 => format!("{}s", m / 1_000),
        Some(m) => format!("{m}ms"),
    }
}

/// The effective value of `key` in display form.
fn show(r: &ResourceLimits, key: &str) -> String {
    match key {
        "max_workers" => r.max_workers.to_string(),
        "run_workers" => r.run_workers.to_string(),
        "cpu_max_pct" => r
            .cpu_max_pct
            .map(|p| format!("{p}%"))
            .unwrap_or_else(|| "off".into()),
        "min_free_mem" => show_bytes(r.min_free_mem),
        "min_free_disk" => show_bytes(r.min_free_disk),
        "cpu_max_wait" => show_ms(r.cpu_max_wait_ms),
        _ => String::new(),
    }
}

pub fn info(r: &ResourceLimits) -> ResourceLimitsInfo {
    ResourceLimitsInfo {
        max_workers: r.max_workers,
        run_workers: r.run_workers,
        cpu_max_pct: r.cpu_max_pct,
        min_free_mem: r.min_free_mem,
        min_free_disk: r.min_free_disk,
        cpu_max_wait_ms: r.cpu_max_wait_ms,
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    values: BTreeMap<String, String>,
}

const FILE_VERSION: u32 = 1;

/// The saved settings, the environment overrides (read once at start) and
/// the effective limits.
#[derive(Debug)]
pub struct LimitsState {
    path: PathBuf,
    saved: BTreeMap<String, String>,
    env: BTreeMap<String, String>,
    pub effective: ResourceLimits,
}

impl LimitsState {
    /// Read `<state>\orch\limits.json` and the environment. A broken file
    /// is moved aside (`limits.corrupt-<ms>.json`); unknown keys and bad
    /// values are dropped. Notes go to `notes` (the daemon log).
    pub fn load(
        path: &Path,
        env: impl Fn(&str) -> Option<String>,
        notes: &mut Vec<String>,
    ) -> Self {
        let mut saved = BTreeMap::new();
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<File>(&bytes) {
                Ok(f) => {
                    for (k, v) in f.values {
                        match parse(&k, &v) {
                            Ok(_) => {
                                saved.insert(k, v);
                            }
                            Err(e) => notes.push(format!("{}: ignored ({e})", path.display())),
                        }
                    }
                }
                Err(e) => {
                    let aside =
                        path.with_file_name(format!("limits.corrupt-{}.json", crate::now_ms()));
                    let _ = std::fs::rename(path, &aside);
                    notes.push(format!(
                        "{}: unreadable ({e}), moved to {}",
                        path.display(),
                        aside.display()
                    ));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => notes.push(format!("{}: {e}", path.display())),
        }
        let mut envs = BTreeMap::new();
        for k in KEYS {
            let Some(v) = env(k.env).filter(|v| !v.trim().is_empty()) else {
                continue;
            };
            match parse(k.name, &v) {
                Ok(_) => {
                    envs.insert(k.name.to_string(), v);
                }
                Err(e) => notes.push(format!("{}={v} ignored ({e})", k.env)),
            }
        }
        let mut s = LimitsState {
            path: path.to_path_buf(),
            saved,
            env: envs,
            effective: ResourceLimits::default(),
        };
        s.effective = s.resolve(&s.saved);
        s
    }

    fn resolve(&self, saved: &BTreeMap<String, String>) -> ResourceLimits {
        let mut r = ResourceLimits::default();
        for k in KEYS {
            let v = self.env.get(k.name).or_else(|| saved.get(k.name));
            if let Some(Ok(v)) = v.map(|v| parse(k.name, v)) {
                apply(&mut r, k.name, v);
            }
        }
        r
    }

    /// Change the saved settings: `set` values (checked) and `reset` keys
    /// (back to the default). All or nothing; saved before it takes effect.
    pub fn set(&mut self, set: &BTreeMap<String, String>, reset: &[String]) -> Result<(), String> {
        let mut next = self.saved.clone();
        for k in reset {
            if !KEYS.iter().any(|x| x.name == k) {
                return Err(format!(
                    "알 수 없는 한도 '{k}'. 쓸 수 있는 것: {}",
                    key_names()
                ));
            }
            if set.contains_key(k) {
                return Err(format!("{k}: 값을 주면서 기본값으로 되돌릴 수는 없어요."));
            }
            next.remove(k);
        }
        for (k, v) in set {
            parse(k, v)?;
            next.insert(k.clone(), v.trim().to_string());
        }
        if next != self.saved {
            self.save(&next)
                .map_err(|e| format!("설정을 저장하지 못했어요 ({}): {e}", self.path.display()))?;
        }
        self.effective = self.resolve(&next);
        self.saved = next;
        Ok(())
    }

    fn save(&self, values: &BTreeMap<String, String>) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let f = File {
            version: FILE_VERSION,
            values: values.clone(),
        };
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&f).map_err(std::io::Error::other)?,
        )?;
        std::fs::rename(&tmp, &self.path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// One row per key: effective value, source, the saved value.
    pub fn entries(&self) -> Vec<OrchLimitEntry> {
        KEYS.iter()
            .map(|k| {
                let source = if self.env.contains_key(k.name) {
                    "env"
                } else if self.saved.contains_key(k.name) {
                    "settings"
                } else {
                    "default"
                };
                OrchLimitEntry {
                    key: k.name.to_string(),
                    value: show(&self.effective, k.name),
                    source: source.to_string(),
                    env: k.env.to_string(),
                    saved: self.saved.get(k.name).cloned(),
                    help: k.help.to_string(),
                }
            })
            .collect()
    }
}

/// Default location: `<state>\orch\limits.json`.
pub fn default_path(state_dir: &Path) -> PathBuf {
    state_dir.join("orch").join("limits.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pod-limits-{tag}-{}-{}",
            std::process::id(),
            crate::now_ms()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn map(kv: &[(&str, &str)]) -> BTreeMap<String, String> {
        kv.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn set_checks_saves_and_reloads() {
        let d = dir("set");
        let path = default_path(&d);
        let mut notes = Vec::new();
        let mut s = LimitsState::load(&path, |_| None, &mut notes);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(s.effective, ResourceLimits::default());
        assert!(s.entries().iter().all(|e| e.source == "default"));

        s.set(
            &map(&[
                ("max_workers", "5"),
                ("cpu_max_pct", "off"),
                ("min_free_mem", "2G"),
                ("cpu_max_wait", "90s"),
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(s.effective.max_workers, 5);
        assert_eq!(s.effective.cpu_max_pct, None);
        assert_eq!(s.effective.min_free_mem, Some(2 << 30));
        assert_eq!(s.effective.cpu_max_wait_ms, Some(90_000));
        let e = s.entries();
        let mw = e.iter().find(|e| e.key == "max_workers").unwrap();
        assert_eq!(
            (mw.value.as_str(), mw.source.as_str(), mw.saved.as_deref()),
            ("5", "settings", Some("5"))
        );
        assert_eq!(
            e.iter().find(|e| e.key == "min_free_mem").unwrap().value,
            "2G"
        );
        assert_eq!(
            e.iter().find(|e| e.key == "cpu_max_wait").unwrap().value,
            "90s"
        );

        // Reload from the file.
        let r = LimitsState::load(&path, |_| None, &mut notes);
        assert_eq!(r.effective, s.effective);

        // Reset one key: back to its default, the others stay.
        s.set(&BTreeMap::new(), &["max_workers".into()]).unwrap();
        assert_eq!(
            s.effective.max_workers,
            ResourceLimits::default().max_workers
        );
        assert_eq!(s.effective.cpu_max_pct, None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bad_values_are_refused_whole() {
        let d = dir("bad");
        let path = default_path(&d);
        let mut s = LimitsState::load(&path, |_| None, &mut Vec::new());
        for (k, v, want) in [
            ("max_workers", "0", "1–64"),
            ("max_workers", "65", "1–64"),
            ("run_workers", "two", "1–64"),
            ("cpu_max_pct", "150", "1–100"),
            ("cpu_max_pct", "0", "1–100"),
            ("min_free_mem", "lots", "크기"),
            ("min_free_disk", "", "크기"),
            ("cpu_max_wait", "soon", "시간"),
            ("gpu", "1", "알 수 없는 한도"),
        ] {
            let e = s
                .set(&map(&[("max_workers", "4"), (k, v)]), &[])
                .unwrap_err();
            assert!(e.contains(want), "{k}={v}: {e}");
        }
        assert_eq!(s.effective, ResourceLimits::default(), "nothing applied");
        assert!(!path.exists(), "nothing saved");
        let e = s
            .set(&map(&[("max_workers", "4")]), &["max_workers".into()])
            .unwrap_err();
        assert!(e.contains("되돌릴"), "{e}");
        let e = s.set(&BTreeMap::new(), &["nope".into()]).unwrap_err();
        assert!(e.contains("알 수 없는 한도"), "{e}");
    }

    #[test]
    fn env_wins_and_broken_files_move_aside() {
        let d = dir("env");
        let path = default_path(&d);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"version":1,"values":{"max_workers":"6","run_workers":"0","cpu_max_pct":"50"}}"#,
        )
        .unwrap();
        let env = |k: &str| match k {
            "POD_MAX_WORKERS" => Some("2".to_string()),
            "POD_ORCH_MIN_FREE_DISK" => Some("bogus".to_string()),
            "POD_ORCH_CPU_LIMIT" => Some(" ".to_string()),
            _ => None,
        };
        let mut notes = Vec::new();
        let s = LimitsState::load(&path, env, &mut notes);
        assert_eq!(s.effective.max_workers, 2, "env over the file");
        assert_eq!(s.effective.cpu_max_pct, Some(50.0), "blank env = unset");
        assert_eq!(
            s.effective.run_workers,
            ResourceLimits::default().run_workers,
            "bad saved value dropped"
        );
        assert_eq!(
            s.effective.min_free_disk,
            ResourceLimits::default().min_free_disk,
            "bad env ignored"
        );
        assert_eq!(notes.len(), 2, "{notes:?}");
        let e = s.entries();
        let mw = e.iter().find(|e| e.key == "max_workers").unwrap();
        assert_eq!(
            (mw.source.as_str(), mw.saved.as_deref()),
            ("env", Some("6"))
        );

        std::fs::write(&path, "{ not json").unwrap();
        let mut notes = Vec::new();
        let s = LimitsState::load(&path, |_| None, &mut notes);
        assert_eq!(s.effective, ResourceLimits::default());
        assert!(!path.exists() && notes[0].contains("moved"), "{notes:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
