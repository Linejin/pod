//! Review wording that lives with the panel: the comment drawer's UIA name
//! and toolbar tooltip, and which shared string names a diff side or a
//! comment state. The shared catalog is `crate::strings` (split by A-T1).

use podd::protocol::{CommentState, Side};

/// UIA name of the drawer and the toggle's tooltip (strings.rs is owned elsewhere this wave).
pub(super) const COMMENTS_DRAWER_NAME: &str = "의견 서랍";
pub(super) const COMMENTS_DRAWER_TIP: &str = "의견 서랍 열기·닫기 (Alt+C, 닫기 Esc)";

pub(super) fn side_label(side: Side) -> &'static str {
    match side {
        Side::Old => crate::strings::ORIGINAL,
        Side::New => crate::strings::MODIFIED,
    }
}

pub(super) fn state_label(s: CommentState) -> &'static str {
    match s {
        CommentState::Open => crate::strings::UNRESOLVED,
        CommentState::Queued => crate::strings::QUEUED,
        CommentState::Sent => crate::strings::SENT,
        CommentState::Resolved => crate::strings::RESOLVED,
    }
}
