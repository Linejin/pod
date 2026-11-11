//! Wall-clock source for layout models: epoch milliseconds, either the system
//! clock or a frozen instant.
//!
//! A value, not a global: each caller keeps choosing its clock and when it
//! reads it (docs/20-refactor/app.md §2). No timer and no process-wide freeze
//! are added here.

/// Where "now" comes from, in epoch milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Clock {
    /// `SystemTime::now()` since the Unix epoch, `0` if the system clock is
    /// before it: the same value as `podd::now_ms`, `orch::now_ms` and the
    /// block terminal's local `now_ms`.
    #[default]
    System,
    /// A fixed instant (fixture scenes and tests).
    Frozen(u64),
}

impl Clock {
    /// `0` means "not frozen", the encoding of the orchestration fixture's
    /// frozen clock (`orch_ui::fixture::now_ms`).
    pub(crate) fn frozen_or_system(frozen_ms: u64) -> Self {
        match frozen_ms {
            0 => Self::System,
            ms => Self::Frozen(ms),
        }
    }

    pub(crate) fn now_ms(self) -> u64 {
        match self {
            Self::System => podd::now_ms(),
            Self::Frozen(ms) => ms,
        }
    }
}
