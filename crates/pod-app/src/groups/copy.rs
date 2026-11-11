//! Group wording that lives with the panel and the New Group dialog: the
//! agent steppers' product names and UIA labels, the member preview's
//! placeholder name, and a card's branch line and marks. The shared catalog
//! is `crate::strings` (split by A-T1); member names (`claude`, `argv`, ...)
//! and error code names are podd's, not wording.

/// Stepper titles of the two coding agents (product names, not translated).
pub(super) const CLAUDE: &str = "Claude";
pub(super) const CODEX: &str = "Codex";
/// Member preview before a group name is typed.
pub(super) const NAME_PLACEHOLDER: &str = "<name>";
/// Branch of a member neither the task poll nor the group record knows.
pub(super) const NO_BRANCH: &str = "—";
/// Line counts of a binary file.
pub(super) const BINARY: &str = "bin";

/// UIA name of a stepper's − button.
pub(super) fn decrease(title: &str) -> String {
    format!("{title} 줄이기")
}

/// UIA name of a stepper's + button.
pub(super) fn increase(title: &str) -> String {
    format!("{title} 늘리기")
}

pub(super) fn branch_line(branch: &str) -> String {
    format!("⎇ {branch}")
}
