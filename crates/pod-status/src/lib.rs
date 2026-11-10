//! `pod-status` — Pod's agent status engine.
//!
//! Pure, synchronous, no PTY or IPC of its own. podd owns the terminals and
//! calls into one [`TerminalStatus`] per terminal:
//!
//! | podd sees | call |
//! |---|---|
//! | PTY output bytes | [`TerminalStatus::feed_output`] → reply [`QUERY_REPLY`] per query |
//! | title change (OSC 0/2) | [`TerminalStatus::on_title`] |
//! | visible screen text | [`TerminalStatus::on_screen`] |
//! | foreground process | [`TerminalStatus::on_foreground`] / [`TerminalStatus::on_process_exit`] |
//! | `pod-hook` pipe line | [`HookMessage::from_line`] → [`TerminalStatus::on_hook`] |
//! | user views the pane | [`TerminalStatus::ack`] |
//! | timer (≈1 s) | [`TerminalStatus::tick`] |
//!
//! See `docs/07-pod-status.md`.

pub mod engine;
pub mod heuristic;
pub mod hook;
pub mod install;
pub mod osc;
pub mod types;

pub use engine::{
    OutputResult, Snapshot, StatusConfig, SubRecord, TerminalStatus, Transition,
    aggregate_snapshots, is_answer_input,
};
pub use heuristic::{ProcessInfo, detect_agent};
pub use hook::{HookAgent, HookEffect, HookMessage, map_hook, map_message};
pub use osc::{Osc7501Parser, OscEvent, QUERY_REPLY, Report, ReportKind, ReportState, query_reply};
pub use types::{AgentKind, AgentState, BlockedKind, Millis, Outcome, Source, aggregate};
