//! Review panel copy: diff toolbar, comments, delivery and submit status.

pub const NOT_DELIVERED: &str = "전달 안 됨";
pub const OPTIONAL_INSTRUCTIONS_FOR_THE_AGENT_SENT_BEFORE_THE_COMMENTS: &str =
    "에이전트에게 보낼 요청 (선택, 의견보다 먼저 전달)";
pub const TASK_HAS_NO_WORKTREE: &str = "작업 폴더가 없습니다";
pub const COULD_NOT_READ_REVIEW_COMMENTS: &str = "리뷰 의견을 읽지 못했습니다";
pub const ADD_COMMENT_152: &str = "의견 추가";
pub const SELECT_LINES_CLICK_A_LINE_NUMBER_SHIFT_OR_POINT_AT_A_LINE_FIRST: &str =
    "줄을 먼저 선택하세요. 줄 번호 클릭 또는 Shift+↑↓로 선택할 수 있습니다.";
pub const COMMENT_NOT_SAVED: &str = "의견을 저장하지 못했습니다";
pub const COULD_NOT_REMOVE_COMMENT: &str = "의견을 삭제하지 못했습니다";
pub const REVIEW_UPDATE_FAILED: &str = "리뷰를 갱신하지 못했습니다";
pub const CANCEL_FAILED: &str = "취소하지 못했습니다";
pub const SPLIT: &str = "나란히";
pub const SIDE_BY_SIDE_NEEDS_A_WIDE_PANEL_HIDE_FILES_COMMENTS: &str =
    "나란히 보기 (넓은 패널 필요 · 파일/의견을 숨겨보세요)";
pub const INLINE: &str = "한 줄로";
pub const EXPAND: &str = "펼치기";
pub const COLLAPSE: &str = "접기";
pub const SHOW_OR_FOLD_UNCHANGED_LINES: &str = "변경 없는 줄 펼치기·접기";
pub const WRAP: &str = "줄바꿈 ✓";
pub const WRAP_169: &str = "줄바꿈";
pub const SOFT_WRAP_LONG_LINES: &str = "긴 줄 자동 줄바꿈";
pub const PREVIOUS_CHANGE_SHIFT_F7_ALT: &str = "이전 변경 (Shift+F7 / Alt+↑)";
pub const NEXT_CHANGE_F7_ALT: &str = "다음 변경 (F7 / Alt+↓)";
pub const PREVIOUS_FILE_ALT_PGUP: &str = "이전 파일 (Alt+PgUp)";
pub const NEXT_FILE_ALT_PGDN: &str = "다음 파일 (Alt+PgDn)";
pub const FILES_175: &str = "파일 ✓";
pub const FILES_176: &str = "파일";
pub const SHOW_OR_HIDE_THE_FILE_LIST: &str = "파일 목록 표시·숨기기";
pub const COMMENTS: &str = "의견 ✓";
pub const COMMENTS_179: &str = "의견";
pub const SHOW_OR_HIDE_THE_COMMENTS_PANE: &str = "의견 패널 표시·숨기기";
pub const RE_READ_CHANGES_COMMENTS_AND_DELIVERIES_F5: &str = "변경, 의견과 전송 기록 새로고침 (F5)";
pub const WILL_QUEUE_UNTIL_IDLE: &str = " · 대기 상태가 되면 전달";
pub const SEND_WILL_BE_REFUSED: &str = " · 지금은 보낼 수 없음";
pub const DELIVERS_NOW: &str = " · 바로 전달";
pub const NO_COMMENTS_YET_CLICK_A_LINE_OR_DRAG_LINE_NUMBERS_THEN_TO_COMMENT: &str =
    "아직 의견이 없습니다. 줄을 클릭하거나 줄 번호를 선택한 뒤 +를 누르세요.";
pub const SEND_TO_AGENT_194: &str = "에이전트에 보내기";
pub const SENDING: &str = "보내는 중…";
pub const REVIEW_SEND_CTRL_SHIFT_ENTER_CHECKED_COMMENTS_ELSE_ALL_OPEN_ONES: &str =
    "의견 보내기 (Ctrl+Shift+Enter). 선택한 의견, 없으면 모든 미해결 의견을 보냅니다.";
pub const REOPEN: &str = "다시 열기";
pub const RESOLVE: &str = "해결";
pub const DELETE: &str = "삭제";
pub const LOADING_CHANGES: &str = "변경 사항 읽는 중…";
pub const NO_CHANGES_AGAINST_THE_TASK_BASE_YET: &str = "기준 브랜치와 다른 변경 사항이 없습니다.";

pub const SUBMIT_PENDING: &str = "제출 확인 중";
pub const SUBMIT_CONFIRMED: &str = "에이전트가 받음";
pub const SUBMIT_UNCONFIRMED: &str = "제출 확인 안 됨: 입력칸에 남아 있을 수 있음";
pub const REVIEW_SUBMIT_UNCONFIRMED: &str = "리뷰 제출 확인 안 됨";
pub const CHECKED_COMMENTS_ALL_RESOLVED: &str = "체크한 코멘트가 모두 해결됨 상태라 보낼 코멘트가 없습니다. 열린 코멘트를 체크하거나 체크를 모두 풀면 열린 코멘트 전체를 보냅니다.";
pub const PRESS_ENTER: &str = "Enter 누르기";

pub const OPEN_IN_EDITOR: &str = "편집기에서 열기";
pub const OPEN_IN_EDITOR_SHORT: &str = "↗ 편집";
pub const OPEN_IN_EDITOR_TIP: &str = "이 파일을 작업 편집기에서 열기 (Ctrl+Shift+O: 가리킨 줄)";

pub const RETRY: &str = "다시 시도";

pub const ORIGINAL: &str = "이전";
pub const MODIFIED: &str = "변경";
pub const UNRESOLVED: &str = "미해결";
pub const QUEUED: &str = "전송 예약";
pub const SENT: &str = "전송됨";
pub const RESOLVED: &str = "해결됨";

pub const REFRESHING: &str = "새로 읽는 중…";
pub const BINARY: &str = "바이너리";
/// File list note: the file differs from the base only in line endings (CRLF/LF).
pub const EOL_ONLY: &str = "줄바꿈만 바뀜";

pub const DELIVERED: &str = "전달됨";
