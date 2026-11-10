//! Exited-terminal retention (v6, docs/03 §7-1): a terminal whose program
//! has ended keeps its holder process (final screen, exit code) until
//! `terminal.rm`. With a retention period configured (default 24 h,
//! `podd --retain-exited`), the daemon removes exited terminals that are
//! older than that, unless they are kept (`terminal.keep`, persisted as
//! `<state dir>\<id>.keep`) or belong to a task (task terminals are removed
//! with the task).

use std::sync::Arc;
use std::time::Duration;

use super::{Daemon, log};
use crate::protocol::*;

/// Default retention of exited terminals.
pub const DEFAULT_RETENTION: Duration = Duration::from_secs(24 * 3600);

/// Parse `24h`, `90m`, `30s`, `500ms`, a bare number of seconds, or
/// `0`/`off`/`never` (= no retention).
pub fn parse_duration(s: &str) -> Option<Option<Duration>> {
    let s = s.trim().to_ascii_lowercase();
    if matches!(s.as_str(), "0" | "off" | "never" | "none") {
        return Some(None);
    }
    let (num, unit) = match s.find(|c: char| !(c.is_ascii_digit() || c == '.')) {
        Some(i) => s.split_at(i),
        None => (s.as_str(), "s"),
    };
    let n: f64 = num.parse().ok()?;
    let ms = match unit {
        "ms" => n,
        "s" => n * 1e3,
        "m" => n * 60e3,
        "h" => n * 3600e3,
        "d" => n * 86400e3,
        _ => return None,
    };
    if !ms.is_finite() || ms < 0.0 {
        return None;
    }
    Some((ms >= 1.0).then(|| Duration::from_millis(ms as u64)))
}

fn keep_path(daemon: &Daemon, id: &str) -> std::path::PathBuf {
    daemon.config.state_dir.join(format!("{id}.keep"))
}

impl Daemon {
    /// Load the keep markers of the terminals found at start.
    pub(super) fn load_kept(&self) {
        let ids: Vec<String> = self
            .terminals
            .lock()
            .unwrap()
            .iter()
            .map(|t| t.id.clone())
            .collect();
        let mut kept = self.kept.lock().unwrap();
        for id in ids {
            if keep_path(self, &id).exists() {
                kept.insert(id);
            }
        }
    }

    fn in_task(&self, id: &str) -> bool {
        self.registry
            .lock()
            .unwrap()
            .tasks
            .iter()
            .any(|t| t.terminals.iter().any(|x| x == id))
    }

    /// Retention facts of one terminal.
    pub(super) fn retention_of(&self, info: &TerminalInfo) -> Retention {
        let kept = self.kept.lock().unwrap().contains(&info.id);
        let expires_ms = match (self.config.exited_retention, info.exited_ms) {
            // An incompatible holder still runs its process: never swept
            // (only `rm --force` ends it).
            (Some(r), Some(ex))
                if !kept && !self.in_task(&info.id) && info.incompatible_holder.is_none() =>
            {
                Some(ex + r.as_millis() as u64)
            }
            _ => None,
        };
        Retention {
            id: info.id.clone(),
            kept,
            expires_ms,
        }
    }

    pub(super) fn keep(&self, p: KeepParams) -> Result<Retention, RpcError> {
        let t = self.resolve(&p.target)?;
        let path = keep_path(self, &t.id);
        let io = |e: std::io::Error| RpcError::new(codes::INTERNAL_ERROR, format!("keep: {e}"));
        if p.keep {
            std::fs::write(&path, b"").map_err(io)?;
            self.kept.lock().unwrap().insert(t.id.clone());
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e)),
            }
            self.kept.lock().unwrap().remove(&t.id);
        }
        log(&format!(
            "{} {}",
            if p.keep { "keep" } else { "unkeep" },
            t.id
        ));
        Ok(self.retention_of(&t.info()))
    }

    /// Forget the keep marker of a removed terminal.
    pub(super) fn forget_kept(&self, id: &str) {
        self.kept.lock().unwrap().remove(id);
        let _ = std::fs::remove_file(keep_path(self, id));
    }

    /// Remove every exited terminal whose retention has passed.
    pub(super) async fn sweep_exited(&self) {
        let now = crate::now_ms();
        let due: Vec<String> = self
            .list()
            .iter()
            .filter(|i| i.state == TerminalState::Exited)
            .filter(|i| self.retention_of(i).expires_ms.is_some_and(|e| e <= now))
            .map(|i| i.id.clone())
            .collect();
        for id in due {
            log(&format!("retention: removing exited terminal {id}"));
            if let Err(e) = self
                .rm(RmParams {
                    target: TerminalRef::id(id.clone()),
                    force: false,
                })
                .await
            {
                log(&format!("retention: cannot remove {id}: {}", e.message));
            }
        }
    }
}

/// Start the retention sweeper (no-op when retention is off).
pub(super) fn start(daemon: &Arc<Daemon>) {
    let Some(r) = daemon.config.exited_retention else {
        return;
    };
    let period = (r / 4).clamp(Duration::from_millis(200), Duration::from_secs(60));
    let daemon = daemon.clone();
    tokio::spawn(async move {
        loop {
            daemon.sweep_exited().await;
            tokio::time::sleep(period).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        let d = |s| parse_duration(s);
        assert_eq!(d("24h"), Some(Some(Duration::from_secs(86_400))));
        assert_eq!(d("90m"), Some(Some(Duration::from_secs(5_400))));
        assert_eq!(d("1.5s"), Some(Some(Duration::from_millis(1_500))));
        assert_eq!(d("500ms"), Some(Some(Duration::from_millis(500))));
        assert_eq!(d("2d"), Some(Some(Duration::from_secs(172_800))));
        assert_eq!(d("30"), Some(Some(Duration::from_secs(30))));
        assert_eq!(d("off"), Some(None));
        assert_eq!(d("0"), Some(None));
        assert_eq!(d("x"), None);
        assert_eq!(d("5y"), None);
    }
}
