//! Product copy of this layout (kept here so layout modules merge independently).

pub const TITLE: &str = "작업 관제";
pub const LIST: &str = "응답 대기열";
pub const WORKING: &str = "작업 중";
pub const NEEDS: &str = "응답 필요";
pub const DONE: &str = "완료";
pub const FAILED: &str = "실패";
pub const SEARCH: &str = "작업과 요청 검색";
pub const ALLOW_ONCE: &str = "이번만 허용";
pub const REJECT: &str = "거부";
/// Accessible-name prefix of a gate option button (`선택: <option>: <title>`).
pub const CHOOSE: &str = "선택";
pub const OPEN: &str = "열기";
pub const ANSWER: &str = "답하기";
pub const DISMISS: &str = "확인";
pub const SHOW_ALL: &str = "모두 보기";
pub const SHOW_LESS: &str = "접기";
pub const APPROVAL: &str = "승인 필요";
pub const QUESTION: &str = "질문";
pub const EMPTY: &str = "응답을 기다리는 요청이 없습니다";
pub const NO_MATCH: &str = "검색과 맞는 항목이 없습니다";
pub const DOCK_EMPTY: &str = "대기열에서 카드를 고르면 그 터미널이 여기에 열립니다";
pub const NEW_TASK: &str = "새 작업";
pub const NEW_TASK_TIP: &str = "새 작업 (Ctrl+N)";
pub const FILTER: &str = "검색 열기/닫기";
pub const ACTIVITY: &str = "활동 막대";
pub const SHOW_QUEUE: &str = "응답 대기열 보이기/숨기기";
pub const AGENTS: &str = "에이전트 트리";
pub const AGENTS_TIP: &str = "에이전트 트리 (Ctrl+Shift+E)";
pub const FILES: &str = "파일";
pub const FILES_TIP: &str = "파일 (Ctrl+Shift+P)";
pub const REVIEW: &str = "변경 검토";
pub const REVIEW_TIP: &str = "변경 검토 (Ctrl+Shift+R)";
pub const GATES: &str = "승인 요청";
pub const GATES_TIP: &str = "승인 요청 (Ctrl+Shift+I)";
pub const SETTINGS: &str = "설정";
pub const SETTINGS_TIP: &str = "설정 (Ctrl+,)";
pub const PILLS: &str = "작업 상태 요약";

/// `12초`, `3분`, `2시간`, `4일`: an age in its largest whole unit.
pub fn age_seconds(n: u64) -> String {
    format!("{n}초")
}
pub fn age_minutes(n: u64) -> String {
    format!("{n}분")
}
pub fn age_hours(n: u64) -> String {
    format!("{n}시간")
}
pub fn age_days(n: u64) -> String {
    format!("{n}일")
}

/// Accessible name of a title-bar pill (`응답 필요 2개`).
pub fn pill_name(label: &str, n: usize) -> String {
    format!("{label} {n}개")
}
