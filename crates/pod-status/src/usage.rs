//! AI service usage limits that Pod records locally (the usage monitor).
//!
//! Claude Code passes its documented status-line JSON to a `statusLine`
//! command on stdin (<https://code.claude.com/docs/en/statusline#rate-limit-usage>).
//! `pod-hook statusline` is such a command: it keeps **only** the
//! `rate_limits` numbers ([`ClaudeLimits::from_statusline`]) and writes them,
//! with when they were seen, to a small per-profile file in Pod's user-local
//! cache ([`write_record`]). The app reads those files ([`read_records`]).
//!
//! What is never kept: the session, prompt, cwd, transcript path, model,
//! workspace, cost, any token or account detail, the raw input. A record
//! holds percentages, reset times, the spend-limit period and timestamps.
//!
//! Files: `<usage dir>/claude-<profile>.json`, `profile` is `default` or
//! `cfg-<16 hex>` (a hash of `CLAUDE_CONFIG_DIR`, so separate Claude
//! configurations never overwrite or mix with each other). Writes are atomic
//! (temp file + rename); reads are bounded in size and count and only accept
//! file names Pod itself writes.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Format version of a record file.
pub const RECORD_VERSION: u32 = 1;
/// The record's `source`: Claude Code status-line samples through pod-hook.
pub const SOURCE_CLAUDE_STATUSLINE: &str = "claude-statusline";
/// A record file larger than this is ignored (records are ~300 bytes).
pub const MAX_RECORD_BYTES: u64 = 16 * 1024;
/// At most this many profile files are read.
pub const MAX_PROFILES: usize = 8;
/// Status-line input larger than this is not parsed (it is a few KiB).
pub const MAX_STATUSLINE_BYTES: usize = 1024 * 1024;
/// Overrides the usage directory (tests, portable setups).
pub const ENV_USAGE_DIR: &str = "POD_USAGE_DIR";
/// Claude Code's own variable for a separate configuration directory.
pub const ENV_CLAUDE_CONFIG_DIR: &str = "CLAUDE_CONFIG_DIR";

/// Reset times outside this range (Unix seconds, 2001…2100) are malformed.
const RESETS_AT_RANGE: std::ops::RangeInclusive<i64> = 1_000_000_000..=4_102_444_800;

/// One window: percentage used (0–100; a spend limit may pass 100) and
/// when it resets (Unix seconds), each as reported. Missing stays missing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LimitSample {
    pub used_percentage: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// Spend limit only: `daily`, `weekly` or `monthly`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period: Option<String>,
}

/// The `rate_limits` object of one status-line sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClaudeLimits {
    pub five_hour: Option<LimitSample>,
    pub seven_day: Option<LimitSample>,
    pub spend_limit: Option<LimitSample>,
}

impl ClaudeLimits {
    /// `None` when the sample has no `rate_limits` object (not a subscriber,
    /// or before the session's first API response). Each window is read
    /// independently: absent or malformed fields make that window missing.
    pub fn from_statusline(v: &Value) -> Option<Self> {
        let rl = v.get("rate_limits")?.as_object()?;
        let window = |key: &str, spend: bool| -> Option<LimitSample> {
            let w = rl.get(key)?.as_object()?;
            let used = w.get("used_percentage")?.as_f64()?;
            let max = if spend { 10_000.0 } else { 100.0 };
            if !used.is_finite() || !(0.0..=max).contains(&used) {
                return None;
            }
            let resets_at = w
                .get("resets_at")
                .and_then(|r| r.as_i64().or_else(|| r.as_f64().map(|f| f as i64)))
                .filter(|r| RESETS_AT_RANGE.contains(r));
            let period = spend
                .then(|| w.get("period").and_then(Value::as_str))
                .flatten()
                .filter(|p| matches!(*p, "daily" | "weekly" | "monthly"))
                .map(str::to_string);
            Some(LimitSample {
                used_percentage: used,
                resets_at,
                period,
            })
        };
        Some(Self {
            five_hour: window("five_hour", false),
            seven_day: window("seven_day", false),
            spend_limit: window("spend_limit", true),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.five_hour.is_none() && self.seven_day.is_none() && self.spend_limit.is_none()
    }
}

/// What Pod keeps per Claude profile.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaudeUsageRecord {
    pub v: u32,
    pub source: String,
    pub profile: String,
    /// Last component of `CLAUDE_CONFIG_DIR` (a label, not a path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_label: Option<String>,
    /// Last status-line sample (Unix ms), with or without limits.
    pub seen_at_ms: u64,
    /// Last sample that carried `rate_limits` (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<LimitSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<LimitSample>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend_limit: Option<LimitSample>,
}

impl ClaudeUsageRecord {
    pub fn empty(profile: &Profile, now_ms: u64) -> Self {
        Self {
            v: RECORD_VERSION,
            source: SOURCE_CLAUDE_STATUSLINE.into(),
            profile: profile.key.clone(),
            profile_label: profile.label.clone(),
            seen_at_ms: now_ms,
            limits_at_ms: None,
            five_hour: None,
            seven_day: None,
            spend_limit: None,
        }
    }

    /// Fold one sample into the previous record.
    ///
    /// - No `rate_limits`: only `seen_at_ms` moves; earlier limits stay
    ///   (another session may still be reporting them).
    /// - With `rate_limits`: each window is replaced. Claude Code drops a
    ///   window once its `resets_at` passes; the old window is kept only in
    ///   that case, so the app can say "reset time passed, waiting for a new
    ///   report" instead of guessing that it reset.
    pub fn merge(
        old: Option<&Self>,
        profile: &Profile,
        limits: Option<&ClaudeLimits>,
        now_ms: u64,
    ) -> Self {
        let mut rec = match old {
            Some(o) if o.profile == profile.key => o.clone(),
            _ => Self::empty(profile, now_ms),
        };
        rec.v = RECORD_VERSION;
        rec.source = SOURCE_CLAUDE_STATUSLINE.into();
        rec.profile_label = profile.label.clone();
        rec.seen_at_ms = now_ms;
        if let Some(l) = limits {
            let now_s = (now_ms / 1000) as i64;
            let keep = |new: &Option<LimitSample>, old: &Option<LimitSample>| match (new, old) {
                (Some(n), _) => Some(n.clone()),
                (None, Some(o)) if o.resets_at.is_some_and(|r| r <= now_s) => Some(o.clone()),
                _ => None,
            };
            rec.five_hour = keep(&l.five_hour, &rec.five_hour);
            rec.seven_day = keep(&l.seven_day, &rec.seven_day);
            rec.spend_limit = keep(&l.spend_limit, &rec.spend_limit);
            rec.limits_at_ms = Some(now_ms);
        }
        rec
    }

    /// One short line for Claude Code's status bar: `5h 76% 남음 · 7d 59% 남음`.
    pub fn summary_line(&self) -> String {
        let mut parts = Vec::new();
        let left = |s: &LimitSample| (100.0 - s.used_percentage).clamp(0.0, 100.0).round();
        if let Some(s) = &self.five_hour {
            parts.push(format!("5h {}% 남음", left(s)));
        }
        if let Some(s) = &self.seven_day {
            parts.push(format!("7d {}% 남음", left(s)));
        }
        if let Some(s) = &self.spend_limit {
            parts.push(format!("지출 {}% 사용", s.used_percentage.round()));
        }
        parts.join(" · ")
    }

    fn valid(&self, file_profile: &str) -> bool {
        self.v == RECORD_VERSION
            && self.source == SOURCE_CLAUDE_STATUSLINE
            && self.profile == file_profile
            && valid_profile_key(&self.profile)
    }
}

/// A Claude configuration: `default`, or one `CLAUDE_CONFIG_DIR`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub key: String,
    pub label: Option<String>,
}

impl Profile {
    /// From `CLAUDE_CONFIG_DIR` (`None` or blank: the default profile).
    pub fn from_config_dir(dir: Option<&str>) -> Self {
        let Some(dir) = dir.map(str::trim).filter(|d| !d.is_empty()) else {
            return Self {
                key: "default".into(),
                label: None,
            };
        };
        let norm = dir.replace('\\', "/").trim_end_matches('/').to_lowercase();
        let label: String = norm
            .rsplit('/')
            .find(|s| !s.is_empty())
            .unwrap_or("")
            .chars()
            .filter(|c| !c.is_control())
            .take(40)
            .collect();
        Self {
            key: format!("cfg-{:016x}", fnv1a(norm.as_bytes())),
            label: (!label.is_empty()).then_some(label),
        }
    }

    /// The profile of the current process environment.
    pub fn from_env() -> Self {
        Self::from_config_dir(std::env::var(ENV_CLAUDE_CONFIG_DIR).ok().as_deref())
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// `default` or `cfg-` + 16 lowercase hex digits.
pub fn valid_profile_key(key: &str) -> bool {
    key == "default"
        || key.strip_prefix("cfg-").is_some_and(|h| {
            h.len() == 16
                && h.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}

/// `%LOCALAPPDATA%\Pod\usage` (or [`ENV_USAGE_DIR`]).
pub fn usage_dir() -> PathBuf {
    if let Some(d) = std::env::var_os(ENV_USAGE_DIR).filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Pod")
        .join("usage")
}

pub fn record_path(dir: &Path, profile_key: &str) -> PathBuf {
    dir.join(format!("claude-{profile_key}.json"))
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read one record: bounded size, Pod's own format and name, else `None`.
pub fn read_record(dir: &Path, profile_key: &str) -> Option<ClaudeUsageRecord> {
    if !valid_profile_key(profile_key) {
        return None;
    }
    read_bounded(&record_path(dir, profile_key), profile_key)
}

fn read_bounded(path: &Path, profile_key: &str) -> Option<ClaudeUsageRecord> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    f.take(MAX_RECORD_BYTES + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > MAX_RECORD_BYTES {
        return None;
    }
    let rec: ClaudeUsageRecord = serde_json::from_slice(&buf).ok()?;
    rec.valid(profile_key).then_some(rec)
}

/// The valid Claude records in `dir` (at most [`MAX_PROFILES`]), `default` first.
pub fn read_records(dir: &Path) -> Vec<ClaudeUsageRecord> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut keys: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let key = name.strip_prefix("claude-")?.strip_suffix(".json")?;
            valid_profile_key(key).then(|| key.to_string())
        })
        .collect();
    keys.sort();
    // `default` first, then the others in a stable order.
    keys.sort_by_key(|k| k != "default");
    // Bounded work: at most 4× the profiles are opened, at most
    // MAX_PROFILES valid records are returned.
    keys.into_iter()
        .take(MAX_PROFILES * 4)
        .filter_map(|k| read_record(dir, &k))
        .take(MAX_PROFILES)
        .collect()
}

/// Write `rec` atomically: a temp file in the same directory, then a rename
/// over the old file (a reader sees the old or the new record, never half).
pub fn write_record(dir: &Path, rec: &ClaudeUsageRecord) -> std::io::Result<()> {
    if !valid_profile_key(&rec.profile) {
        return Err(std::io::Error::other("invalid profile key"));
    }
    std::fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec(rec).map_err(std::io::Error::other)?;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        ".claude-{}.{}.{nanos}.tmp",
        rec.profile,
        std::process::id()
    ));
    std::fs::write(&tmp, &bytes)?;
    let dst = record_path(dir, &rec.profile);
    let mut last = None;
    for _ in 0..5 {
        match std::fs::rename(&tmp, &dst) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(15));
            }
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Err(last.unwrap_or_else(|| std::io::Error::other("rename failed")))
}

/// Remove Pod's own record for `profile_key` (the user's "기록 지우기").
pub fn remove_record(dir: &Path, profile_key: &str) -> std::io::Result<()> {
    if !valid_profile_key(profile_key) {
        return Err(std::io::Error::other("invalid profile key"));
    }
    match std::fs::remove_file(record_path(dir, profile_key)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// A sample without limits shortly after the last write changes nothing
/// worth a disk write.
pub const IDLE_REWRITE_MS: u64 = 60_000;

/// Fold one status-line sample into the profile's record and store it.
/// Returns the stored (or unchanged) record.
pub fn ingest_statusline(
    dir: &Path,
    profile: &Profile,
    sample: Option<&Value>,
    now_ms: u64,
) -> std::io::Result<ClaudeUsageRecord> {
    let old = read_record(dir, &profile.key);
    let limits = sample.and_then(ClaudeLimits::from_statusline);
    if limits.is_none()
        && let Some(o) = &old
        && now_ms.saturating_sub(o.seen_at_ms) < IDLE_REWRITE_MS
    {
        return Ok(o.clone());
    }
    let rec = ClaudeUsageRecord::merge(old.as_ref(), profile, limits.as_ref(), now_ms);
    write_record(dir, &rec)?;
    Ok(rec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW_MS: u64 = 1_780_000_000_000;
    const NOW_S: i64 = 1_780_000_000;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pod-usage-test-{name}-{}-{}",
            std::process::id(),
            fnv1a(name.as_bytes()) & 0xffff
        ));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    /// The documented example input, plus fields Pod must never keep.
    fn full_sample() -> Value {
        json!({
            "cwd": "C:/secret/project",
            "session_id": "abc123",
            "transcript_path": "C:/Users/me/.claude/projects/x.jsonl",
            "model": {"id": "claude-opus", "display_name": "Opus"},
            "workspace": {"current_dir": "C:/secret/project"},
            "cost": {"total_cost_usd": 1.23},
            "rate_limits": {
                "five_hour": {"used_percentage": 23.5, "resets_at": NOW_S + 3600},
                "seven_day": {"used_percentage": 41.2, "resets_at": NOW_S + 86400},
                "spend_limit": {"used_percentage": 62.8, "resets_at": NOW_S + 864000,
                    "used_usd": 314.12, "limit_usd": 500, "period": "monthly"}
            }
        })
    }

    #[test]
    fn parses_every_documented_window() {
        let l = ClaudeLimits::from_statusline(&full_sample()).unwrap();
        assert_eq!(l.five_hour.as_ref().unwrap().used_percentage, 23.5);
        assert_eq!(l.five_hour.as_ref().unwrap().resets_at, Some(NOW_S + 3600));
        assert_eq!(l.seven_day.as_ref().unwrap().used_percentage, 41.2);
        let s = l.spend_limit.unwrap();
        assert_eq!(
            (s.used_percentage, s.period.as_deref()),
            (62.8, Some("monthly"))
        );
    }

    #[test]
    fn absent_and_partial_rate_limits_stay_missing() {
        assert_eq!(
            ClaudeLimits::from_statusline(&json!({"model": {}})),
            None,
            "no rate_limits: not a subscriber or before the first response"
        );
        assert_eq!(
            ClaudeLimits::from_statusline(&json!({"rate_limits": null})),
            None
        );
        let l = ClaudeLimits::from_statusline(&json!({
            "rate_limits": {"seven_day": {"used_percentage": 10}}
        }))
        .unwrap();
        assert!(l.five_hour.is_none() && l.spend_limit.is_none());
        assert_eq!(
            l.seven_day.unwrap().resets_at,
            None,
            "missing reset stays missing"
        );
        // Spend limit before the dollar fields arrive: percentage only.
        let l = ClaudeLimits::from_statusline(&json!({
            "rate_limits": {"spend_limit": {"used_percentage": 120, "resets_at": NOW_S}}
        }))
        .unwrap();
        let s = l.spend_limit.unwrap();
        assert_eq!((s.used_percentage, s.period), (120.0, None));
    }

    #[test]
    fn malformed_windows_are_dropped_not_guessed() {
        let l = ClaudeLimits::from_statusline(&json!({
            "rate_limits": {
                "five_hour": {"used_percentage": "23", "resets_at": NOW_S},
                "seven_day": {"used_percentage": 140, "resets_at": NOW_S},
                "spend_limit": {"used_percentage": -1}
            }
        }))
        .unwrap();
        assert!(l.is_empty(), "{l:?}");
        let l = ClaudeLimits::from_statusline(&json!({
            "rate_limits": {"five_hour": {"used_percentage": 5, "resets_at": "soon"},
                            "seven_day": {"used_percentage": 5, "resets_at": 12}}
        }))
        .unwrap();
        assert_eq!(l.five_hour.unwrap().resets_at, None);
        assert_eq!(l.seven_day.unwrap().resets_at, None, "implausible epoch");
        assert_eq!(
            ClaudeLimits::from_statusline(&json!({"rate_limits": [1, 2]})),
            None
        );
    }

    #[test]
    fn a_record_never_serializes_anything_but_limits() {
        let p = Profile::from_config_dir(None);
        let l = ClaudeLimits::from_statusline(&full_sample());
        let rec = ClaudeUsageRecord::merge(None, &p, l.as_ref(), NOW_MS);
        let text = serde_json::to_string(&rec).unwrap();
        for secret in [
            "abc123",
            "secret",
            "transcript",
            "Opus",
            "claude-opus",
            "314.12",
            "500",
            "1.23",
            "cwd",
            "session",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert!(text.contains("\"used_percentage\":23.5"), "{text}");
    }

    #[test]
    fn merge_keeps_old_limits_without_new_ones_and_marks_dropped_windows() {
        let p = Profile::from_config_dir(None);
        let first = ClaudeLimits::from_statusline(&full_sample());
        let rec = ClaudeUsageRecord::merge(None, &p, first.as_ref(), NOW_MS);
        // A new session before its first response: limits stay, seen moves.
        let later = ClaudeUsageRecord::merge(Some(&rec), &p, None, NOW_MS + 5_000);
        assert_eq!(later.five_hour, rec.five_hour);
        assert_eq!(
            (later.seen_at_ms, later.limits_at_ms),
            (NOW_MS + 5_000, Some(NOW_MS))
        );
        // Two hours on: the 5h window passed its reset and Claude dropped it.
        let now2 = NOW_MS + 2 * 3_600_000;
        let only_week = ClaudeLimits::from_statusline(&json!({
            "rate_limits": {"seven_day": {"used_percentage": 50, "resets_at": NOW_S + 86400}}
        }));
        let r2 = ClaudeUsageRecord::merge(Some(&later), &p, only_week.as_ref(), now2);
        assert_eq!(
            r2.five_hour, rec.five_hour,
            "elapsed window kept for 'waiting for a new report'"
        );
        assert_eq!(r2.seven_day.as_ref().unwrap().used_percentage, 50.0);
        assert_eq!(
            r2.spend_limit, None,
            "a dropped window that has not reset is gone"
        );
        assert_eq!(r2.limits_at_ms, Some(now2));
    }

    #[test]
    fn profiles_are_isolated_and_keys_validated() {
        let a = Profile::from_config_dir(Some(r"C:\Users\me\.claude-work\"));
        let b = Profile::from_config_dir(Some("c:/users/me/.claude-work"));
        let c = Profile::from_config_dir(Some("C:/Users/me/.claude-home"));
        assert_eq!(a, b, "same directory, any spelling");
        assert_ne!(a.key, c.key);
        assert_eq!(a.label.as_deref(), Some(".claude-work"));
        assert_eq!(Profile::from_config_dir(Some("  ")).key, "default");
        assert!(valid_profile_key(&a.key) && valid_profile_key("default"));
        for bad in ["../x", "cfg-XYZ", "cfg-0123", "", "default.json"] {
            assert!(!valid_profile_key(bad), "{bad}");
        }

        let d = dir("isolated");
        let la = ClaudeLimits::from_statusline(&full_sample());
        let ra = ClaudeUsageRecord::merge(None, &a, la.as_ref(), NOW_MS);
        let rc = ClaudeUsageRecord::merge(None, &c, None, NOW_MS);
        write_record(&d, &ra).unwrap();
        write_record(&d, &rc).unwrap();
        assert_eq!(read_record(&d, &a.key).unwrap(), ra);
        assert_eq!(read_record(&d, &c.key).unwrap().five_hour, None);
        // A record copied under another profile's name is refused.
        std::fs::copy(record_path(&d, &a.key), record_path(&d, &c.key)).unwrap();
        assert_eq!(read_record(&d, &c.key), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn reads_are_bounded_and_only_pods_own_files() {
        let d = dir("bounded");
        std::fs::create_dir_all(&d).unwrap();
        // Oversized, malformed and foreign files are ignored.
        std::fs::write(
            record_path(&d, "default"),
            vec![b' '; MAX_RECORD_BYTES as usize + 1],
        )
        .unwrap();
        std::fs::write(d.join("claude-cfg-00000000000000aa.json"), b"{not json").unwrap();
        std::fs::write(d.join("claude-../evil.json"), b"{}").ok();
        std::fs::write(d.join("other.json"), b"{}").unwrap();
        assert!(read_records(&d).is_empty());
        // Many profiles: at most MAX_PROFILES are read.
        for i in 0..(MAX_PROFILES + 3) {
            let p = Profile {
                key: format!("cfg-{i:016x}"),
                label: None,
            };
            write_record(&d, &ClaudeUsageRecord::empty(&p, NOW_MS)).unwrap();
        }
        assert_eq!(read_records(&d).len(), MAX_PROFILES);
        // No temp file is left behind by the atomic writes.
        let tmp = std::fs::read_dir(&d)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(tmp, 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn ingest_skips_idle_rewrites_and_remove_forgets() {
        let d = dir("ingest");
        let p = Profile::from_config_dir(None);
        let r = ingest_statusline(&d, &p, Some(&full_sample()), NOW_MS).unwrap();
        assert_eq!(r.limits_at_ms, Some(NOW_MS));
        let same = ingest_statusline(&d, &p, Some(&json!({})), NOW_MS + 1_000).unwrap();
        assert_eq!(same.seen_at_ms, NOW_MS, "no write within the idle window");
        let moved =
            ingest_statusline(&d, &p, Some(&json!({})), NOW_MS + IDLE_REWRITE_MS + 1).unwrap();
        assert_eq!(moved.seen_at_ms, NOW_MS + IDLE_REWRITE_MS + 1);
        assert_eq!(moved.five_hour, r.five_hour);
        // A first sample without limits still records that the collector runs.
        let other = Profile::from_config_dir(Some("D:/claude2"));
        let first = ingest_statusline(&d, &other, None, NOW_MS).unwrap();
        assert_eq!((first.limits_at_ms, first.five_hour), (None, None));
        remove_record(&d, &p.key).unwrap();
        assert_eq!(read_record(&d, &p.key), None);
        remove_record(&d, &p.key).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn summary_line_shows_what_is_left() {
        let p = Profile::from_config_dir(None);
        let rec = ClaudeUsageRecord::merge(
            None,
            &p,
            ClaudeLimits::from_statusline(&full_sample()).as_ref(),
            NOW_MS,
        );
        assert_eq!(
            rec.summary_line(),
            "5h 77% 남음 · 7d 59% 남음 · 지출 63% 사용"
        );
        assert_eq!(ClaudeUsageRecord::empty(&p, NOW_MS).summary_line(), "");
    }
}
