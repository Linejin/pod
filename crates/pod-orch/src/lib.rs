//! Orchestration core for Pod (docs/19 S4–S6): the model and storage of
//! runs, work items, attempts and mail, plus the pure wake-up rules. The
//! daemon (`podd`, `server/orch_runs.rs`) owns the terminals and calls in here.

pub mod engine;
pub mod model;
pub mod resource;
pub mod store;
pub mod text;
pub mod wake;

pub use engine::{
    Admitted, BeginAttempt, DoneResult, ErrorKind, Limits, NewMail, Orch, OrchError, hash_token,
    spec_title,
};
pub use model::*;
pub use resource::{ResourceLimits, Sample};
pub use text::MsgText;
