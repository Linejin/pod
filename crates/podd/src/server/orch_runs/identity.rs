//! Who is calling: a worker (attempt id + token, always verified), a
//! coordinator terminal or a person; the run a caller means; and the
//! attempt token (made here, never shown).

use pod_orch::engine::Orch;
use pod_orch::model::{attempt_mailbox, run_mailbox};

use super::rpc::oe;
use crate::protocol::*;
use crate::server::{Daemon, status};

/// Who is calling.
#[derive(Debug, Clone)]
pub(super) struct Who {
    /// Sender address: `attempt:<id>`, `run:<id>` or `human`.
    pub(super) from: String,
    pub(super) attempt: Option<String>,
    pub(super) terminal: Option<String>,
    /// The run the caller coordinates (newest), if any.
    pub(super) coordinates: Option<String>,
    /// The run the caller works in (workers).
    pub(super) works_in: Option<String>,
}

fn newest(mut runs: Vec<String>) -> Option<String> {
    runs.sort_by_key(|r| r.trim_start_matches('r').parse::<u64>().unwrap_or(0));
    runs.pop()
}

/// Resolve the caller. A worker's token is always checked.
pub(super) fn who(o: &Orch, c: &OrchCaller) -> Result<Who, RpcError> {
    if let Some(a) = &c.attempt {
        let at = o.verify_token(a, c.token.as_deref()).map_err(oe)?;
        let terminal = c.terminal.clone().or_else(|| at.terminal.clone());
        let mut coord: Vec<String> = o
            .runs()
            .filter(|r| r.rec.run.coordinator_attempt.as_deref() == Some(a.as_str()))
            .map(|r| r.rec.run.id.clone())
            .collect();
        if let Some(t) = &terminal {
            coord.extend(o.runs_of_terminal(t));
        }
        return Ok(Who {
            from: attempt_mailbox(a),
            attempt: Some(a.clone()),
            terminal,
            coordinates: newest(coord),
            works_in: Some(at.run.clone()),
        });
    }
    if let Some(t) = &c.terminal {
        let coordinates = newest(o.runs_of_terminal(t));
        return Ok(Who {
            from: coordinates
                .as_deref()
                .map(run_mailbox)
                .unwrap_or_else(|| "human".into()),
            attempt: None,
            terminal: Some(t.clone()),
            coordinates,
            works_in: None,
        });
    }
    Ok(Who {
        from: "human".into(),
        attempt: None,
        terminal: None,
        coordinates: None,
        works_in: None,
    })
}

impl Who {
    /// The mailboxes this caller reads.
    pub(super) fn mailboxes(&self, o: &Orch, run: Option<&str>) -> Vec<String> {
        if let Some(r) = run {
            return vec![run_mailbox(r)];
        }
        let mut out = Vec::new();
        if let Some(a) = &self.attempt {
            out.push(attempt_mailbox(a));
        }
        if let Some(t) = &self.terminal {
            for mb in o.terminal_mailboxes(t) {
                if !out.contains(&mb) {
                    out.push(mb);
                }
            }
        }
        out
    }
}

pub(super) fn public(mut a: Attempt) -> Attempt {
    a.token_hash.clear();
    a
}

/// 256 random bits as hex (two 128-bit system RNG draws).
pub(super) fn new_attempt_token() -> Result<String, RpcError> {
    let a = status::new_token()
        .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("token: {e}")))?;
    let b = status::new_token()
        .map_err(|e| RpcError::new(codes::INTERNAL_ERROR, format!("token: {e}")))?;
    Ok(format!("{a}{b}"))
}

impl Daemon {
    pub(super) fn caller_run(
        &self,
        o: &Orch,
        run: Option<String>,
        caller: &OrchCaller,
    ) -> Result<String, RpcError> {
        if let Some(r) = run {
            return Ok(r);
        }
        let w = who(o, caller)?;
        w.coordinates.or(w.works_in).ok_or_else(|| {
            RpcError::new(
                codes::INVALID_PARAMS,
                "no run: pass --run, or call from a terminal that coordinates or works in one",
            )
        })
    }
}
