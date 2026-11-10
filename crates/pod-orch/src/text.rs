//! Daemon-written texts as data (docs/19 §16.3): a stable `kind` plus its
//! parameters. The daemon stores the English rendering in the old string
//! fields (`Mail.subject`, `WorkerInfo.next`, ...) for the CLI, `--json`
//! and older clients, and the structured form next to it so a client can
//! render its own language (the app renders Korean).
//!
//! Every kind is listed in [`KINDS`]; a client test checks it renders each.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A text the daemon wrote: `kind` and its parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct MsgText {
    pub kind: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
}

impl MsgText {
    pub fn new(kind: &str) -> Self {
        MsgText {
            kind: kind.to_string(),
            params: BTreeMap::new(),
        }
    }

    /// Add a parameter.
    pub fn with(mut self, key: &str, value: impl ToString) -> Self {
        self.params.insert(key.to_string(), value.to_string());
        self
    }

    /// A parameter, empty when missing.
    pub fn p(&self, key: &str) -> &str {
        self.params.get(key).map(String::as_str).unwrap_or("")
    }

    /// The English rendering (what `subject` / `next` hold).
    pub fn en(&self) -> String {
        english(self)
    }
}

// Mail subjects.
pub const TURN_ENDED: &str = "escalation.turn_ended";
pub const STALLED: &str = "escalation.stalled";
/// `cause`: `terminal_gone`, `agent_exited`, `host_restart`, `host_restart_starting`.
pub const LOST: &str = "escalation.lost";
pub const LOW_DISK: &str = "escalation.low_disk";
pub const START_FAILED: &str = "escalation.start_failed";
pub const DONE: &str = "done.report";
pub const DONE_LATE: &str = "done.late";
pub const GATE_REQUESTED: &str = "gate.requested";
/// `outcome`: `approved`, `rejected`, `expired`, `decided` (+ `choice`).
pub const GATE_SETTLED: &str = "gate.settled";
pub const REVIEW_DELIVERED: &str = "review.delivered";
pub const GROUP_CHECKED: &str = "group.checked";
/// The items a worker waited for finished (`items`: `i1 completed, i2 failed`).
pub const WAIT_FINISHED: &str = "wait.finished";
// Mail bodies.
pub const START_FAILED_RETRY: &str = "start_failed.retry";
pub const START_FAILED_TRIPPED: &str = "start_failed.tripped";
pub const GATE_REQUESTED_BODY: &str = "gate.requested.body";
pub const GATE_SETTLED_BODY: &str = "gate.settled.body";
// Worker `next`.
pub const NEXT_RELEASED: &str = "next.released";
pub const NEXT_WAITING_ANSWER: &str = "next.waiting_answer";
pub const NEXT_QUEUED: &str = "next.queued";
pub const NEXT_STARTING: &str = "next.starting";
pub const NEXT_BLOCKED: &str = "next.blocked";
pub const NEXT_WORKING: &str = "next.working";
pub const NEXT_IDLE: &str = "next.idle";
pub const NEXT_KEPT: &str = "next.kept";
pub const NEXT_RELEASE_IN: &str = "next.release_in";
pub const NEXT_FINISHED: &str = "next.finished";
/// `pod orch wait --item` (`items`: `i1, i2`).
pub const NEXT_WAITING_ITEMS: &str = "next.waiting_items";

/// Every kind the daemon writes.
pub const KINDS: &[&str] = &[
    TURN_ENDED,
    STALLED,
    LOST,
    LOW_DISK,
    START_FAILED,
    DONE,
    DONE_LATE,
    GATE_REQUESTED,
    GATE_SETTLED,
    REVIEW_DELIVERED,
    GROUP_CHECKED,
    WAIT_FINISHED,
    START_FAILED_RETRY,
    START_FAILED_TRIPPED,
    GATE_REQUESTED_BODY,
    GATE_SETTLED_BODY,
    NEXT_RELEASED,
    NEXT_WAITING_ANSWER,
    NEXT_QUEUED,
    NEXT_STARTING,
    NEXT_BLOCKED,
    NEXT_WORKING,
    NEXT_IDLE,
    NEXT_KEPT,
    NEXT_RELEASE_IN,
    NEXT_FINISHED,
    NEXT_WAITING_ITEMS,
];

/// Parameters a sample of each kind carries (for client tests).
pub fn sample(kind: &str) -> MsgText {
    let t = MsgText::new(kind).with("attempt", "a2").with("item", "i1");
    match kind {
        STALLED => t.with("secs", 1200),
        LOST => t.with("cause", "agent_exited"),
        START_FAILED_RETRY => t.with("error", "program not found"),
        START_FAILED_TRIPPED => t.with("strikes", 3),
        DONE | DONE_LATE => t.with("outcome", "succeeded").with("title", "fix login"),
        GATE_REQUESTED => t.with("gate", "g1").with("title", "pick a db"),
        GATE_REQUESTED_BODY => t
            .with("gate", "g1")
            .with("message", "which one?")
            .with("options", "sqlite | postgres"),
        GATE_SETTLED | GATE_SETTLED_BODY => t
            .with("gate", "g1")
            .with("title", "pick a db")
            .with("outcome", "decided")
            .with("choice", "sqlite")
            .with("note", "smaller"),
        REVIEW_DELIVERED => t.with("task", "repo/r1-a2").with("delivery", "d1"),
        GROUP_CHECKED => t
            .with("group", "repo/g")
            .with("state", "finished")
            .with("all_passed", "true"),
        NEXT_QUEUED => t.with("position", 2).with("reason", "동시 워커 3/3"),
        NEXT_RELEASE_IN => t.with("secs", 540),
        WAIT_FINISHED => t.with("items", "i1 completed, i3 failed"),
        NEXT_WAITING_ITEMS => t.with("items", "i1, i3"),
        _ => t,
    }
}

fn lost_cause_en(cause: &str) -> &'static str {
    match cause {
        "terminal_gone" => "the worker's terminal was removed",
        "agent_exited" => "the agent exited without reporting done",
        "host_restart" => "the worker's terminal did not survive a restart",
        "host_restart_starting" => "the daemon stopped while the worker was starting",
        _ => "its terminal is gone",
    }
}

fn english(t: &MsgText) -> String {
    let p = |k: &str| t.p(k);
    match t.kind.as_str() {
        TURN_ENDED => format!(
            "worker {} ended its turn without `pod orch done`",
            p("attempt")
        ),
        STALLED => format!(
            "worker {} is working but printed nothing for {} s",
            p("attempt"),
            p("secs")
        ),
        LOST => format!(
            "worker {} lost: {}",
            p("attempt"),
            lost_cause_en(p("cause"))
        ),
        LOW_DISK => format!("worker {} waits for disk space", p("attempt")),
        START_FAILED => format!("queued worker {} could not start", p("attempt")),
        DONE => format!("done ({}): {}", p("outcome"), p("title")),
        DONE_LATE => format!(
            "late done ({}) from superseded attempt {}; ignored",
            p("outcome"),
            p("attempt")
        ),
        GATE_REQUESTED => format!("gate {}: {}", p("gate"), p("title")),
        GATE_SETTLED => format!("gate {} {}", p("gate"), settled_en(t)),
        REVIEW_DELIVERED => format!(
            "review comments were delivered to worker {} ({})",
            p("attempt"),
            p("task")
        ),
        GROUP_CHECKED => {
            let verdict = match p("all_passed") {
                "true" => "all passed",
                "false" => "some failed",
                _ => "no verdict",
            };
            format!("group {} check {} ({verdict})", p("group"), p("state"))
        }
        START_FAILED_RETRY => format!(
            "The worker did not start ({}); fix the cause and start it again.",
            p("error")
        ),
        START_FAILED_TRIPPED => format!(
            "Item {item} failed to start {} times in a row and is now failed; fix the cause, then `pod orch item reopen {item}`.",
            p("strikes"),
            item = p("item")
        ),
        GATE_REQUESTED_BODY => {
            let mut b = p("message").to_string();
            let how = if p("options").is_empty() {
                format!("pod orch gate decide {} (or --reject)", p("gate"))
            } else {
                b.push_str(&format!("\n\noptions: {}", p("options")));
                format!(
                    "pod orch gate decide {} --choice <option> (or --reject)",
                    p("gate")
                )
            };
            b.push_str(&format!(
                "\n\nDecide with `{how}`; a person can decide it in the Gates panel too."
            ));
            b
        }
        GATE_SETTLED_BODY => {
            let mut b = format!("gate {} ({}) {}", p("gate"), p("title"), settled_en(t));
            if !p("note").is_empty() {
                b.push_str(&format!("\nnote: {}", p("note")));
            }
            b
        }
        NEXT_RELEASED => "released: terminal closed, worktree kept".into(),
        NEXT_WAITING_ANSWER => "waiting for an answer".into(),
        NEXT_QUEUED => format!("queued_for_resources (#{}): {}", p("position"), p("reason")),
        NEXT_STARTING => "starting".into(),
        NEXT_BLOCKED => "blocked: needs a person".into(),
        NEXT_WORKING => "working".into(),
        NEXT_IDLE => "idle: waiting for mail or its done report".into(),
        NEXT_KEPT => "finished: kept (never released automatically)".into(),
        NEXT_RELEASE_IN => format!("finished: release in {} s", p("secs")),
        NEXT_FINISHED => "finished".into(),
        WAIT_FINISHED => format!("the items you waited for finished: {}", p("items")),
        NEXT_WAITING_ITEMS => format!("waiting for items {}", p("items")),
        other => other.to_string(),
    }
}

fn settled_en(t: &MsgText) -> String {
    match t.p("outcome") {
        "decided" => format!("decided: {}", t.p("choice")),
        o => o.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_matches_the_old_strings() {
        // The CLI and older clients read these exact strings (S4–S7).
        assert_eq!(
            MsgText::new(TURN_ENDED).with("attempt", "a2").en(),
            "worker a2 ended its turn without `pod orch done`"
        );
        assert_eq!(
            MsgText::new(NEXT_WAITING_ANSWER).en(),
            "waiting for an answer"
        );
        assert_eq!(
            MsgText::new(LOST)
                .with("attempt", "a1")
                .with("cause", "host_restart")
                .en(),
            "worker a1 lost: the worker's terminal did not survive a restart"
        );
        assert_eq!(
            MsgText::new(GATE_SETTLED)
                .with("gate", "g1")
                .with("outcome", "decided")
                .with("choice", "b")
                .en(),
            "gate g1 decided: b"
        );
        assert_eq!(
            MsgText::new(GATE_SETTLED)
                .with("gate", "g1")
                .with("outcome", "rejected")
                .en(),
            "gate g1 rejected"
        );
        let body = MsgText::new(GATE_REQUESTED_BODY)
            .with("gate", "g2")
            .with("message", "m")
            .with("options", "a | b")
            .en();
        assert_eq!(
            body,
            "m\n\noptions: a | b\n\nDecide with `pod orch gate decide g2 --choice <option> (or --reject)`; a person can decide it in the Gates panel too."
        );
    }

    #[test]
    fn every_kind_has_an_english_rendering_and_round_trips() {
        for k in KINDS {
            let t = sample(k);
            assert_ne!(t.en(), *k, "{k} has no English rendering");
            let back: MsgText = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
            assert_eq!(back, t);
        }
        // No params: the field is left out on the wire.
        assert_eq!(
            serde_json::to_value(MsgText::new(NEXT_WORKING)).unwrap(),
            serde_json::json!({"kind": "next.working"})
        );
    }
}
