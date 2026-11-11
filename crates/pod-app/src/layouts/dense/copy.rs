//! Product copy of the dense layout (kept here so layout modules merge
//! independently): labels, tooltips, log and details text, byte for byte as
//! the layout showed them before the split (docs/20-refactor/app.md A-P11).
pub const ACTIVITY: &str = "활동 막대";
pub const EXPLORER: &str = "작업 탐색 보이기/숨기기";
pub const FILES: &str = "파일";
pub const AGENTS: &str = "에이전트 트리";
pub const SEARCH: &str = "검색";
pub const GATES: &str = "승인 요청";
pub const OUTPUT_TOGGLE: &str = "출력 패널 보이기/숨기기";
pub const DETAILS_TOGGLE: &str = "세부 정보 보이기/숨기기";
pub const SETTINGS: &str = "설정";
pub const OUTPUT: &str = "출력";
pub const LOG: &str = "출력 · 이벤트 기록";
pub const CLEAR: &str = "출력 지우기";
pub const CLOSE_OUTPUT: &str = "출력 패널 닫기";
pub const EVENTS: &str = "이벤트";
pub const LOG_EMPTY: &str = "아직 기록된 이벤트가 없습니다";
pub const DETAILS: &str = "세부 정보";
pub const CLOSE_DETAILS: &str = "세부 정보 닫기";
pub const NOTHING: &str = "작업이나 에이전트를 고르면 여기에 세부 정보가 보입니다";
pub const NAME: &str = "이름";
pub const AGENT: &str = "에이전트";
pub const STATE: &str = "상태";
pub const TASK: &str = "작업";
pub const PROGRAM: &str = "프로그램";
pub const COMMAND: &str = "명령";
pub const REPO: &str = "저장소";
pub const BRANCH: &str = "브랜치";
pub const WORKTREE: &str = "작업 폴더";
pub const ELAPSED: &str = "경과";
pub const ORIGIN: &str = "출처";
pub const GIT: &str = "변경";
pub const COMMENTS: &str = "검토 의견";
pub const PR: &str = "PR";
pub const NO_PR: &str = "없음";
pub const ACTIONS: &str = "동작";
pub const ALLOW_ONCE: &str = "이번만 허용";
pub const REJECT: &str = "거부";
pub const CHOOSE: &str = "선택";
pub const OPEN_TERMINAL: &str = "터미널 열기";
pub const REVIEW: &str = "변경 검토";
pub const OPEN_FILES: &str = "파일 열기";
pub const OPEN_PR: &str = "PR 보기";
pub const RERUN: &str = "다시 실행";
pub const LAST_LINES: &str = "마지막 줄";
pub const NO_LINES: &str = "출력 없음";
pub const HISTORY: &str = "상태 기록";
pub const NO_HISTORY: &str = "이 레이아웃을 연 뒤로 바뀐 것이 없습니다";
pub const AGENTS_OF_TASK: &str = "에이전트";
pub const STARTED: &str = "기록 시작";
pub const GATE_REQUESTED: &str = "승인 요청";
pub const APPROVED: &str = "승인됨";
pub const REJECTED: &str = "거부됨";
pub const EXPIRED: &str = "만료됨";
pub const LOST: &str = "연결 끊김";
pub const NOTICE: &str = "알림";
pub const POD: &str = "pod";
pub const GATE: &str = "gate";
pub const COULD_NOT_RERUN: &str = "다시 실행하지 못했습니다";

// Activity bar tooltips.
pub(super) const EXPLORER_TIP: &str = "작업 탐색 보이기/숨기기";
pub(super) const FILES_TIP: &str = "파일 (Ctrl+P)";
pub(super) const AGENTS_TIP: &str = "에이전트 트리 (Ctrl+Shift+E)";
pub(super) const SEARCH_TIP: &str = "작업, 터미널, 명령 검색 (Ctrl+K)";
pub(super) const GATES_TIP: &str = "승인 요청 (Ctrl+Shift+I)";
pub(super) const OUTPUT_TIP: &str = "출력 패널 (Ctrl+Shift+U)";
pub(super) const DETAILS_TIP: &str = "세부 정보 (Ctrl+Shift+D)";
pub(super) const SETTINGS_TIP: &str = "설정 (Ctrl+,)";

// Pull-request states in the details column.
pub(super) const PR_DRAFT: &str = "초안";
pub(super) const PR_OPEN: &str = "열림";
pub(super) const PR_MERGED: &str = "병합됨";
pub(super) const PR_CLOSED: &str = "닫힘";

/// The log's first line: how many terminals were running when it started.
pub(super) fn started(running: usize) -> String {
    format!("터미널 {running}개 · 기존 상태에서 확인이 필요한 것만")
}

/// Open review comments of a task.
pub(super) fn comments(n: impl std::fmt::Display) -> String {
    format!("{n}개")
}
