//! Re-connecting, at start, to the holders that survived a previous daemon.

use std::sync::atomic::Ordering;
use std::time::Duration;

use super::terminal::Terminal;
use super::terminal_ops::id_number;
use super::{Daemon, lifecycle, log, status};
use crate::holder_proto::HolderRecord;
use crate::protocol::*;

/// How long the start waits for one surviving holder (they are reached
/// concurrently).
const REDISCOVER_TIMEOUT: Duration = Duration::from_secs(5);

impl Daemon {
    /// Re-connect to holders that survived a previous daemon.
    ///
    /// Every `t<n>.json` record reserves its id, whether or not its holder
    /// answers: a holder that is alive but unreachable right now (busy,
    /// slow, access error) keeps its id, pipe name, token and state files,
    /// and no new terminal is given that id (daemon-03). Holders are reached
    /// concurrently, each within [`REDISCOVER_TIMEOUT`], so one stuck holder
    /// cannot hold up the start (daemon-04).
    pub(in crate::server) async fn rediscover(&self) {
        let Ok(entries) = std::fs::read_dir(&self.config.state_dir) else {
            return;
        };
        let mut reserved = 0;
        let mut pending = Vec::new();
        let mut lost = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.extension().and_then(|e| e.to_str()) != Some("json")
                || name.ends_with(".spec.json")
            {
                continue;
            }
            if let Some(stem) = name.strip_suffix(".json")
                && is_terminal_id(stem)
            {
                reserved = reserved.max(id_number(stem));
            }
            let rec = match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|b| serde_json::from_slice::<HolderRecord>(&b).map_err(|e| e.to_string()))
            {
                Ok(rec) => rec,
                Err(e) => {
                    // Listed (and re-read on retry) so `pod kill` can forget it.
                    if let Some(stem) = name.strip_suffix(".json")
                        && is_terminal_id(stem)
                    {
                        lost.push(lifecycle::Unreachable::new(
                            stem.to_string(),
                            path.clone(),
                            None,
                            format!("record unreadable: {e}"),
                        ));
                    }
                    continue;
                }
            };
            if is_terminal_id(&rec.id) {
                reserved = reserved.max(id_number(&rec.id));
            }
            let pipe = rec.pipe.clone();
            let connect =
                tokio::spawn(
                    async move { Terminal::connect_within(&pipe, REDISCOVER_TIMEOUT).await },
                );
            pending.push((rec, path, connect));
        }
        let mut found = Vec::new();
        for (rec, path, connect) in pending {
            let res = connect
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e)));
            match res {
                Ok(t) => {
                    log(&format!(
                        "re-attached {} (holder pid {}, {})",
                        t.id,
                        t.holder_pid,
                        state_word(&t.info())
                    ));
                    found.push(t);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    log(&format!(
                        "removing stale record {} ({})",
                        rec.id,
                        path.display()
                    ));
                    let _ = std::fs::remove_file(&path);
                    let _ = std::fs::remove_file(path.with_extension("log"));
                    status::remove_token(&self.config.state_dir, &rec.id);
                }
                Err(e) => {
                    log(&format!(
                        "cannot re-attach {} (holder pid {}): {e}; listed as unreachable, \
                         its id stays reserved",
                        rec.id, rec.holder_pid
                    ));
                    let id = rec.id.clone();
                    lost.push(lifecycle::Unreachable::new(
                        id,
                        path,
                        Some(rec),
                        e.to_string(),
                    ));
                }
            }
        }
        // One entry per id (a holder that answered wins).
        lost.retain(|u| !found.iter().any(|t| t.id == u.id));
        lost.sort_by_key(|u| id_number(&u.id));
        lost.dedup_by(|a, b| a.id == b.id);
        *self.unreachable.lock().unwrap() = lost;
        found.sort_by_key(|t| id_number(&t.id));
        let max = found
            .iter()
            .map(|t| id_number(&t.id))
            .max()
            .unwrap_or(0)
            .max(reserved);
        self.next_id.fetch_max(max + 1, Ordering::Relaxed);
        *self.terminals.lock().unwrap() = found;
    }
}

fn state_word(i: &TerminalInfo) -> &'static str {
    match i.state {
        TerminalState::Exited if i.incompatible_holder.is_some() => "incompatible holder",
        TerminalState::Running => "running",
        TerminalState::Exited => "exited",
        TerminalState::Unreachable => "unreachable",
    }
}
