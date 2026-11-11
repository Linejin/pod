//! Bounded child processes: one way to run an external command with a time
//! limit that holds for the whole call (orch-03, security-02/03).
//!
//! The runner and its tests live in the `pod-process` leaf crate
//! ([`pod_process::bounded`]); this module re-exports it so
//! `pod_forge::bounded::*` keeps compiling (docs/20-refactor/libs.md L1).

pub use pod_process::bounded::*;
