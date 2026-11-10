//! Korean product copy. Keep protocol identifiers, user data and command lines untranslated.
//! Constants are static labels; tr! owns format templates and explicit interpolation.
// Agent marks (docs/10 §D5.11): service names stay Latin; the suffix is Korean.
pub const AGENT_SUFFIX: &str = "에이전트";
pub const AGENT_SHELL_TERMINAL: &str = "셸 터미널";
pub const NEW_TASK: &str = "새 작업";
pub const OPEN_REPOSITORY: &str = "저장소 열기";
pub const REGISTER_A_LOCAL_GIT_REPOSITORY: &str = "로컬 Git 저장소 추가";
pub const NEW_TERMINAL: &str = "새 터미널";
pub const SETTINGS: &str = "설정";
pub const CURRENT_BRANCH: &str = "현재 브랜치";
pub const FIND_NEXT_BLOCKED_REQUEST: &str = "응답할 요청 찾기";
pub const RETURN_TO_THE_TERMINAL_THAT_NEEDS_YOU: &str = "응답이 필요한 터미널 열기";
pub const INSPECT_CURRENT_TERMINAL_STATUS: &str = "현재 터미널 상태";
pub const SOURCE_AND_FRESHNESS: &str = "출처와 확인 시간";
pub const NOTIFICATIONS: &str = "알림";
pub const SETTINGS_DAMAGED: &str = "설정 파일이 손상되어 읽을 수 있는 값만 불러왔습니다";
pub const SETTINGS_DAMAGED_BACKUP: &str = "원래 파일은 다음에 보관했습니다:";
pub const SETTINGS_DAMAGED_NO_BACKUP: &str = "원래 파일을 보관하지 못했습니다";
pub const NO_NOTIFICATIONS: &str = "새 알림이 없습니다.";
pub const JUST_NOW: &str = "방금";
pub const PENDING_REQUESTS_AND_RECENT_EVENTS: &str = "응답할 요청과 최근 알림";
pub const SEND_CTRL_K_TO_CURRENT_TERMINAL: &str = "현재 터미널에 Ctrl+K 보내기";
pub const TERMINAL_SHORTCUT_PASSTHROUGH: &str = "터미널 단축키 보내기";
pub const ARRANGE_PANELS_IN_A_GRID: &str = "패널 격자 정렬";
pub const NEW_COMPARISON_GROUP: &str = "새 비교 그룹";
pub const COMPARE_PARALLEL_TASKS: &str = "병렬 작업 비교";
pub const OPEN_GATES_INBOX: &str = "승인 요청 열기";
pub const PENDING_APPROVALS: &str = "승인 대기";
pub const OPEN_AGENT_TREE: &str = "에이전트 트리 열기";
pub const TASKS_GROUPS_AND_PARENT_RELATIONS: &str = "작업, 그룹과 상위 관계";
pub const TERMINALS: &str = "터미널";
pub const YOUR_WORKSPACE: &str = "작업 공간";
pub const SEARCH_COMMANDS_TERMINALS_CTRL_K: &str = "명령·터미널 검색";
/// Visible text of the title-bar search pill (its accessible name stays the line above).
pub const SEARCH_PILL_PLACEHOLDER: &str = "작업, 터미널, 명령 검색";
pub const NO_BLOCKED_REQUESTS: &str = "응답할 요청이 없습니다";
pub const ALL_TERMINALS_ARE_CLEAR_OF_REPORTED_REQUESTS: &str =
    "모든 터미널에 응답 대기 요청이 없습니다.";
pub const LOCAL_GIT_REPOSITORY_PATH: &str = "로컬 Git 저장소 경로";
pub const OPENING: &str = "여는 중…";
pub const OPEN: &str = "열기";
pub const CONNECTION_CLOSED_RETRY_WHEN_CONNECTED: &str =
    "연결이 끊겼습니다. 연결 후 다시 시도하세요.";
pub const JUMP_TO_WORK: &str = "작업 찾기";
pub const SEARCH_TASKS_TERMINALS_OR_COMMANDS: &str = "작업, 터미널 검색 · > 명령만";
pub const NO_MATCHING_ITEMS_CLEAR_THE_SEARCH_TO_SHOW_ALL_COMMANDS: &str =
    "일치하는 항목이 없습니다. 검색을 지워 다시 찾아보세요.";
pub const NOT_REPORTED: &str = "보고 없음";
pub const SETTINGS_COULD_NOT_BE_SAVED: &str = "설정을 저장하지 못했습니다";
pub const PODD_DISCONNECTED: &str = "연결이 끊겼습니다";
pub const PODD_RECONNECTED: &str = "다시 연결했습니다";
pub const COULD_NOT_SAVE_LAYOUT: &str = "배치를 저장하지 못했습니다";
pub const KILL_TERMINAL: &str = "터미널 종료";
pub const CANCEL: &str = "취소";
pub const KILL: &str = "종료";
pub const RENAME: &str = "이름 바꾸기";
pub const RENAME_FAILED: &str = "이름을 바꾸지 못했습니다";
pub const WORKING_DIRECTORY: &str = "작업 폴더";
pub const OPTIONAL_E_G_REVIEW_BOT: &str = "이름 (선택, 예: review-bot)";
pub const PROGRAM: &str = "프로그램";
pub const COMMAND: &str = "명령";
pub const NAME: &str = "이름";
pub const RUN: &str = "실행";
pub const PODD_CLIENT_STOPPED: &str = "podd 연결이 닫혔습니다";
pub const REVIEW_NEEDS_A_NEWER_PODD: &str = "리뷰에 새 podd가 필요합니다";
pub const PODD_PROTOCOL_V5_REVIEW_IS_REQUIRED: &str = "podd 프로토콜 v5 (review.*)가 필요합니다.";
pub const NO_SUCH_TASK: &str = "작업을 찾을 수 없습니다";
pub const NO_TASK_TO_REVIEW: &str = "리뷰할 작업이 없습니다";
pub const CREATE_A_TASK_FIRST_TASK: &str = "먼저 새 작업을 만드세요.";
pub const TASK_NAME_E_G_FIX_LOGIN_BRANCH_POD_NAME: &str =
    "작업 이름 (예: fix-login, 브랜치 pod/<name>)";
pub const BASE_REF_DEFAULT_THE_REPO_S_DEFAULT_BRANCH: &str =
    "기준 브랜치 (기본: 저장소의 기본 브랜치)";
pub const OPTIONAL_FIRST_PROMPT_CLAUDE_CODEX: &str = "첫 요청 (선택, claude / codex)";
pub const ADD_REPOSITORY: &str = "저장소 추가…";
pub const SHELL: &str = "셸";
pub const REPOSITORY: &str = "저장소";
pub const BASE: &str = "기준";
pub const AGENT: &str = "에이전트";
pub const PROMPT: &str = "요청";
pub const CREATING: &str = "만드는 중…";
pub const CREATE: &str = "만들기";
pub const ENTER_A_TASK_NAME: &str = "작업 이름을 입력하세요.";
pub const CHOOSE_A_REPOSITORY_OR_ENTER_ITS_PATH: &str = "저장소를 선택하거나 경로를 입력하세요.";
pub const CONNECTION_CLOSED_WHILE_ADDING_REPOSITORY: &str =
    "저장소를 추가하는 중 연결이 끊겼습니다.";
pub const CONNECTION_CLOSED_WHILE_CREATING_TASK_CHECK_THE_TASK_LIST_BEFORE_: &str =
    "작업을 만드는 중 연결이 끊겼습니다. 다시 시도하기 전에 작업 목록을 확인하세요.";
pub const THIS_TASK_NO_LONGER_EXISTS: &str = "이 작업은 삭제되었습니다.";
pub const MISSING: &str = "  (없음)";
pub const LAST_COMMIT: &str = "마지막 커밋";
pub const DELETE_BRANCH: &str = "☑ 브랜치 삭제";
pub const DELETE_BRANCH_94: &str = "☐ 브랜치 삭제";
pub const REVIEW_CHANGES: &str = "변경 사항 리뷰";
pub const CLOSE: &str = "닫기";
pub const REMOVE_TASK: &str = "작업 삭제…";
pub const REMOVE_ANYWAY: &str = "그래도 삭제";
pub const PODD_REFUSED_TO_REMOVE_THE_TASK: &str = "작업을 삭제할 수 없습니다";
pub const REMOVING_ANYWAY_DELETES_THE_WORKTREE_WITH_ITS_UNCOMMITTED_CHANGES: &str = "그래도 삭제하면 커밋하지 않은 변경 사항과 작업 폴더가 사라집니다. 전송하지 않은 커밋은 브랜치를 삭제하지 않는 한 남습니다.";
pub const KEEP_TASK: &str = "작업 유지";
pub const GRID: &str = "격자";
pub const ARRANGE_OPEN_PANELS_IN_A_GRID_CTRL_SHIFT_G: &str = "패널 격자 정렬 (Ctrl+Shift+G)";
pub const TASK_107: &str = "+ 작업";
pub const NEW_TASK_GIT_WORKTREE_AGENT_TERMINAL: &str = "새 작업: Git 작업 폴더와 에이전트 터미널";
pub const NEW: &str = "+ 터미널";
pub const NEW_TERMINAL_CTRL_SHIFT_T: &str = "새 터미널 (Ctrl+Shift+T)";
pub const CONNECTION_LOST: &str = "연결 끊김";
pub const KILL_TERMINAL_112: &str = "터미널 종료…";
pub const REMOVE_EXITED_TERMINAL: &str = "종료된 터미널 삭제";
pub const PODD_CONNECTING: &str = "podd 연결 중…";
pub const PODD_CONNECTED: &str = "podd 연결됨";
pub const STATUS: &str = "상태";
pub const TASK_DETAILS: &str = "작업 상세";
pub const ADD_SHELL_TERMINAL: &str = "셸 터미널 추가";
pub const CONNECTING_TO_PODD_STARTING_IT_IF_NEEDED: &str = "podd에 연결하는 중…";
pub const A_CLEAR_SPACE_FOR_YOUR_NEXT_TASK: &str = "첫 저장소를 열어 보세요";
pub const CONNECTING_TO_YOUR_WORKSPACE: &str = "작업 공간에 연결하는 중";
pub const EMPTY_NO_TASK_TITLE: &str = "첫 작업을 만들어 보세요";
pub const EMPTY_NO_TASK: &str =
    "작업마다 브랜치와 터미널을 따로 둡니다. 새 작업에서 에이전트를 고르세요.";
pub const EMPTY_PICK_TASK_TITLE: &str = "작업을 선택해 계속하세요";
pub const EMPTY_PICK_TASK: &str = "사이드바에서 작업의 터미널을 열거나 새 작업을 만드세요.";
pub const OPEN_A_REPOSITORY_CREATE_A_TASK_OR_RETURN_TO_AN_EXISTING_TERMINAL: &str =
    "작업마다 공간을 나누고 터미널을 함께 관리합니다.";
pub const YOUR_TERMINALS_KEEP_RUNNING_WHILE_POD_RECONNECTS_LAST_OUTPUT_AND_: &str =
    "다시 연결하는 동안에도 터미널은 실행됩니다. 마지막 출력과 상태를 보관합니다.";
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
pub const CHOOSE_A_REPOSITORY_OR_ENTER_THE_PATH_OF_ONE_TO_REGISTER: &str =
    "저장소를 선택하거나 추가할 경로를 입력하세요.";
pub const ENTER_A_GROUP_NAME: &str = "그룹 이름을 입력하세요.";
pub const NAMES_USE_LETTERS_DIGITS_AND_ONLY: &str =
    "이름에는 영문, 숫자, '.', '_', '-'만 사용할 수 있습니다.";
pub const ENTER_THE_PROMPT_EVERY_AGENT_GETS: &str = "모든 에이전트에게 보낼 요청을 입력하세요.";
pub const ADD_AT_LEAST_ONE_AGENT: &str = "에이전트를 하나 이상 추가하세요.";
pub const AT_MOST_16_MEMBERS_PER_GROUP: &str = "그룹에는 최대 16개 멤버를 추가할 수 있습니다.";
pub const ENTER_THE_CUSTOM_AGENT_S_COMMAND_LINE: &str = "사용자 지정 에이전트 명령을 입력하세요.";
pub const GROUPS_NEED_A_NEWER_PODD: &str = "그룹에 새 podd가 필요합니다";
pub const PODD_PROTOCOL_V6_GROUP_IS_REQUIRED: &str = "podd 프로토콜 v6 (group.*)가 필요합니다.";
pub const GROUP_NAME_E_G_LOGIN_FIX_MEMBERS_NAME_AGENT_N: &str =
    "그룹 이름 (예: login-fix, 멤버 <name>-<agent>-<n>)";
pub const THE_PROMPT_EVERY_AGENT_GETS: &str = "모든 에이전트에게 보낼 요청";
pub const CUSTOM_AGENT_COMMAND_LINE_E_G_AIDER_YES: &str = "사용자 지정 명령 (예: aider --yes)";
pub const OPTIONAL_CHECK_COMMAND_E_G_CARGO_TEST_RUNS_VIA_CMD_C_IN_EACH_WORK: &str =
    "검사 명령 (선택, 예: cargo test · 각 작업 폴더에서 cmd /c 실행)";
pub const PROMPT_AS_CLI_ARGUMENT: &str = "요청을 CLI 인수로 전달";
pub const POWERSHELL_PROMPT_TYPED_AS_A_COMMAND: &str = "PowerShell · 요청을 명령으로 입력";
pub const CUSTOM: &str = "사용자 지정";
pub const ANY_PROGRAM_ARGV_BELOW_PROMPT_TYPED_IN: &str = "지정한 프로그램에 요청 입력";
pub const NO_MEMBERS_YET: &str = "아직 멤버가 없습니다.";
pub const NEW_GROUP_FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS: &str =
    "새 그룹 · 여러 에이전트에 같은 요청 보내기";
pub const CHECK_OPTIONAL: &str = "검사 (선택)";
pub const CREATE_GROUP: &str = "그룹 만들기";
pub const COULD_NOT_ADD_REPOSITORY: &str = "저장소를 추가하지 못했습니다";
pub const CHECK_COMMAND_E_G_CARGO_TEST_CMD_C_IN_EACH_WORKTREE: &str =
    "검사 명령 (예: cargo test · 각 작업 폴더에서 cmd /c 실행)";
pub const ENTER_A_CHECK_COMMAND_FIRST: &str = "먼저 검사 명령을 입력하세요.";
pub const SAFE_PODD_REFUSES_A_MEMBER_WITH_UNCOMMITTED_CHANGES_OR_UNPUSHED_C: &str = "커밋하지 않은 변경이나 전송하지 않은 커밋이 있으면 삭제하지 않습니다 (TASK_DIRTY). 확인 후 강제로 삭제할 수 있습니다.";
pub const ALSO_DELETE_THEIR_BRANCHES: &str = "다른 멤버의 브랜치도 삭제";
pub const PICK_WINNER: &str = "최종 선택";
pub const NO_MEMBERS_REMOVED: &str = "삭제한 멤버 없음";
pub const PODD_KEPT_SOME_MEMBERS: &str = "일부 멤버를 유지했습니다";
pub const REMOVING_ANYWAY_DELETES_THEIR_WORKTREES_WITH_UNCOMMITTED_CHANGES_: &str = "그래도 삭제하면 커밋하지 않은 변경과 작업 폴더가 사라집니다. 전송하지 않은 커밋은 브랜치를 삭제하지 않는 한 남습니다.";
pub const KEEP_THEM: &str = "유지";
pub const NO_TERMINAL: &str = "터미널 없음";
pub const CHECK: &str = "검사";
pub const CHECKING: &str = "검사 중…";
pub const RUN_CHECK: &str = "검사 실행";
pub const GROUP_CHECK_RUN_THE_COMMAND_IN_EVERY_MEMBER_S_WORKTREE_AT_ONCE: &str =
    "모든 멤버의 작업 폴더에서 명령 실행";
pub const CANCEL_CHECK: &str = "검사 취소";
pub const COMPARING: &str = "비교 중…";
pub const COMPARE: &str = "비교";
pub const GROUP_COMPARE_RE_READ_EVERY_MEMBER_S_DIFF_AGAINST_THE_BASE: &str =
    "모든 멤버의 변경 사항 다시 비교";
pub const NO_CHECK_RUN_YET: &str = "아직 검사하지 않았습니다.";
pub const NO_DIFF_YET: &str = "변경 사항 없음";
pub const CHECK_RUNNING: &str = "◔ 검사 중…";
pub const CHECK_NOT_RUN: &str = "검사 전";
pub const HIDE_OUTPUT: &str = "출력 숨기기";
pub const SHOW_OUTPUT: &str = "출력 보기";
pub const WINNER_279: &str = "최종 선택";
pub const TASK_REMOVED: &str = "삭제된 작업";
pub const OPEN_THE_MEMBER_S_AGENT_TERMINAL: &str = "멤버의 에이전트 터미널 열기";
pub const REVIEW_284: &str = "리뷰";
pub const OPEN_THE_MEMBER_S_REVIEW_PANEL_DIFF_COMMENTS: &str =
    "멤버의 리뷰 열기 (변경 사항과 의견)";
pub const COULD_NOT_LOAD_THE_GROUP: &str = "그룹을 읽지 못했습니다";
pub const LOADING_GROUP: &str = "그룹 읽는 중…";
pub const EXPIRING_NOW: &str = "곧 만료…";
pub const NO_TERMINAL_FOR_THIS_GATE: &str = "이 요청의 터미널이 없습니다";
pub const ITS_TERMINAL_OR_TASK_IS_GONE: &str = "터미널이나 작업이 삭제되었습니다.";
pub const OPTIONAL_NOTE_FOR_THE_AGENT: &str = "에이전트에게 남길 메모 (선택)";
pub const NO_EXPIRY: &str = "· 만료 없음";
pub const OPEN_TERMINAL: &str = "터미널 열기";
pub const APPROVE: &str = "승인";
pub const REJECT: &str = "거절";
pub const GATES: &str = "승인 요청";
pub const RE_READ_GATES_GATE_LIST: &str = "승인 요청 새로고침 (gate.list)";
pub const NO_PENDING_GATES: &str = "대기 중인 승인 요청이 없습니다";
pub const NOT_RUNNING_IN_PODD: &str = "podd에서 실행 중이 아님";
pub const TASKS_AND_GROUPS_STARTED_FROM_INSIDE_A_POD_TERMINAL_APPEAR_UNDER_: &str =
    "Pod 터미널에서 시작한 작업과 그룹이 그 아래에 표시됩니다. 행을 눌러 여세요.";
pub const NO_AGENTS_YET: &str = "아직 에이전트가 없습니다";
pub const CREATE_A_TASK_OR_A_GROUP_TO_SEE_IT_HERE: &str =
    "작업이나 그룹을 만들면 여기에 표시됩니다.";
pub const NO_ORIGIN_RECORDED: &str = "요청 출처 없음";
pub const OPEN_THE_GATES_INBOX_STATUS_BAR_CTRL_SHIFT_I: &str =
    "승인 요청을 확인하세요 (상태 표시줄 ⚑, Ctrl+Shift+I).";
pub const GATES_NEED_A_NEWER_PODD: &str = "승인 요청에 새 podd가 필요합니다";
pub const PODD_PROTOCOL_V6_IS_REQUIRED: &str = "podd 프로토콜 v6이 필요합니다.";
pub const GROUP_330: &str = "+ 그룹";
pub const FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS_CTRL_SHIFT_N: &str =
    "여러 에이전트에 같은 요청 보내기 (Ctrl+Shift+N)";
pub const AGENT_TREE_GROUPS_TASKS_AND_TERMINALS_STARTED_FROM_OTHER_TERMINAL: &str =
    "에이전트 트리 (Ctrl+Shift+E)";
pub const NO_GATES: &str = "⚑ 승인 요청 없음";
pub const OPEN_IN_ANOTHER_PANEL: &str = "다른 패널에서 열기";
pub const RENAME_336: &str = "이름 바꾸기…";
pub const CLOSE_PANEL_DETACH: &str = "패널 닫기 (연결 해제)";
pub const PODD_DISCONNECTED_REATTACHING: &str = "연결 끊김 · 다시 연결 중…";
pub const TERMINAL_REMOVED: &str = "[삭제된 터미널]";
pub const INSPECTOR_EMPTY: &str = "터미널을 선택하면 상태를 볼 수 있습니다.";
pub const CTRL_K: &str =
    "단축키는 바로 적용됩니다. 명령 검색에서 현재 터미널에 Ctrl+K를 보낼 수 있습니다.";
#[cfg(test)]
pub const FUZZY_SAMPLE: &str = "새 작업 만들기";
#[cfg(test)]
pub const FUZZY_QUERY: &str = "새작";
pub const SIDEBAR_SEARCH: &str = "저장소, 작업, 터미널 검색";
pub const COUNT_WORKING: &str = "작업 중";
pub const COUNT_BLOCKED: &str = "응답";
pub const ERROR: &str = "오류";
pub const COUNT_UNKNOWN: &str = "미확인";
pub const IDLE: &str = "대기";
pub const DONE: &str = "완료";
#[cfg(test)]
pub const INFERRED: &str = "추정";
#[cfg(test)]
pub const SUMMARY_SAMPLE: &str = "작업 중 1 · 완료 1";

pub const SUBMIT_PENDING: &str = "제출 확인 중";
pub const SUBMIT_CONFIRMED: &str = "에이전트가 받음";
pub const SUBMIT_UNCONFIRMED: &str = "제출 확인 안 됨: 입력칸에 남아 있을 수 있음";
pub const REVIEW_SUBMIT_UNCONFIRMED: &str = "리뷰 제출 확인 안 됨";
pub const CHECKED_COMMENTS_ALL_RESOLVED: &str = "체크한 코멘트가 모두 해결됨 상태라 보낼 코멘트가 없습니다. 열린 코멘트를 체크하거나 체크를 모두 풀면 열린 코멘트 전체를 보냅니다.";
pub const PRESS_ENTER: &str = "Enter 누르기";

// ---- P3 editor integration (editor_panel.rs, docs/13 §2) ----
pub const FILE_CHANGED_ON_DISK: &str = "에이전트가 열린 파일을 바꿨습니다";
pub const QUICK_OPEN_FILE: &str = "파일 열기";
pub const QUICK_OPEN_FILE_DETAIL: &str = "Ctrl+P · 현재 작업 폴더에서 파일 이름 찾기";
pub const FILES_PANEL: &str = "파일 탐색기 열기";
pub const FILES_PANEL_DETAIL: &str = "현재 작업의 파일 트리와 편집기";
pub const SAVE_ALL_FILES: &str = "모든 파일 저장";
pub const SAVE_ALL_FILES_DETAIL: &str = "열린 편집기의 저장하지 않은 파일";
pub const OPEN_IN_EDITOR: &str = "편집기에서 열기";
pub const OPEN_IN_EDITOR_SHORT: &str = "↗ 편집";
pub const OPEN_IN_EDITOR_TIP: &str = "이 파일을 작업 편집기에서 열기 (Ctrl+Shift+O: 가리킨 줄)";
pub const OPEN_FILES: &str = "파일 열기";
pub const NO_TASK_FOR_FILES: &str = "파일을 열 작업이 없습니다";
pub const PICK_A_TASK_FIRST: &str = "사이드바에서 작업의 터미널을 선택하거나 새 작업을 만드세요.";
// Terminal links (docs/09 §12).
pub const OPEN_LINK_FROM_TERMINAL: &str = "터미널에서 링크 열기";
pub const FILTER_LINKS: &str = "링크 검색 (파일:줄, 주소)";
pub const OPEN_LINK_FROM_TERMINAL_DETAIL: &str = "화면의 파일:줄 · 주소 (Ctrl+클릭과 같음)";
pub const LINK_KIND_FILE: &str = "파일 · 편집기에서 열기";
pub const LINK_KIND_URL: &str = "주소 · 기본 브라우저에서 열기";
pub const NO_TERMINAL_FOR_LINKS: &str = "링크를 찾을 터미널이 없습니다";
pub const PICK_A_TERMINAL_FIRST: &str = "먼저 터미널을 선택하세요.";
pub const NO_LINKS_IN_VIEW: &str = "화면에 링크가 없습니다";
pub const NO_LINKS_IN_VIEW_BODY: &str = "보이는 출력에 열 수 있는 파일:줄이나 주소가 없습니다.";
pub const LINK_FILE_NOT_FOUND: &str = "파일을 찾을 수 없습니다";
pub const LINK_OUTSIDE_TASKS: &str = "작업 폴더 밖의 파일입니다";
pub const LINK_OPEN_FAILED: &str = "링크를 열지 못했습니다";
// Terminal key passthrough.
pub const SEND_CTRL_P_TO_CURRENT_TERMINAL: &str = "현재 터미널에 Ctrl+P 보내기";
// Status-bar language server menu.
pub const LSP_MENU_RESTART: &str = "다시 시작";
pub const LSP_MENU_OFF: &str = "끄기";
pub const LSP_MENU_LOG: &str = "로그 보기";
pub const LSP_OFF_LABEL: &str = "언어 서버 꺼짐";
pub const LSP_TURNED_OFF: &str = "언어 서버를 껐습니다";
pub const LSP_TURNED_OFF_BODY: &str =
    "이 작업의 편집기에서 언어 서버를 끕니다. 상태 표시줄 메뉴의 다시 시작으로 켭니다.";
pub const LSP_LOG_TITLE: &str = "언어 서버 로그";
pub const LSP_LOG_EMPTY: &str = "기록된 출력이 없습니다.";
pub const LSP_STATUS_TOOLTIP: &str = "언어 서버 메뉴: 다시 시작 · 끄기 · 로그 보기";
// Agent hook prompt (docs/14 §11).
pub const HOOKS_MISSING_TITLE: &str = "에이전트 훅이 설치되어 있지 않습니다";
pub const HOOKS_OUTDATED_TITLE: &str = "에이전트 훅이 이 Pod의 pod-hook과 다릅니다";
pub const HOOKS_INSTALL: &str = "훅 설치";
pub const HOOKS_INSTALL_TOOLTIP: &str =
    "바뀔 파일을 먼저 보여 주고, 확인하면 pod hooks install을 실행합니다";
pub const HOOKS_LATER: &str = "나중에";
pub const HOOKS_LATER_TOOLTIP: &str =
    "이 버전에서는 다시 묻지 않습니다 (팔레트의 에이전트 훅 설치로 언제든 설치)";
pub const HOOKS_CHECKING: &str = "확인 중…";
pub const HOOKS_PREVIEW_TITLE: &str = "에이전트 훅 설치";
pub const HOOKS_PREVIEW_INTRO: &str = "pod hooks install이 다음을 바꿉니다. Pod 훅 그룹만 추가하고 다른 설정과 다른 도구의 훅은 그대로 둡니다.";
pub const HOOKS_PREVIEW_BACKUP: &str = "바꾸는 파일은 쓰기 전에 <파일>.pod-backup-<시각>으로 백업합니다. 되돌리려면 이 백업으로 복원하거나 Pod 제거 프로그램에서 훅을 지웁니다.";
pub const HOOKS_PREVIEW_CODEX_TRUST: &str =
    "Codex · config.toml — Pod 훅 신뢰 기록 (Codex에 물어 쓰고, 먼저 백업)";
pub const HOOKS_INSTALL_CONFIRM: &str = "설치";
pub const HOOKS_INSTALLED: &str = "에이전트 훅을 설치했습니다";
pub const HOOKS_INSTALLED_NO_BACKUP: &str = "바뀐 파일이 없습니다.";
pub const HOOKS_INSTALL_FAILED: &str = "훅을 설치하지 못했습니다";
pub const HOOKS_NO_CLI: &str = "pod.exe를 찾을 수 없습니다";
pub const HOOKS_NO_CLI_BODY: &str = "Pod 설치 폴더의 pod.exe로 pod hooks install을 실행하세요.";
pub const HOOKS_FILE_CREATE: &str = "새로 만듦";
pub const HOOKS_FILE_UPDATE: &str = "Pod 훅 추가";
pub const HOOKS_FILE_UNCHANGED: &str = "변경 없음";
pub const TASK_REMOVING_SAVED: &str = "작업 삭제 전에 편집 내용을 저장했습니다";
pub const TERMINAL_LAGGED: &str = "출력이 밀려 화면을 다시 받았습니다";
pub const INSTALL_AGENT_HOOKS: &str = "에이전트 훅 설치";
pub const INSTALL_AGENT_HOOKS_DETAIL: &str = "바뀔 파일 확인 후 pod hooks install";
pub const UNSAVED_FILES_TITLE: &str = "저장하지 않은 파일";
pub const SAVE_ALL_AND_QUIT: &str = "모두 저장하고 닫기";
pub const QUIT_WITHOUT_SAVING: &str = "저장하지 않고 닫기";
pub const REMOVE_WITHOUT_SAVING: &str = "저장하지 않고 삭제";
pub const LSP_SUSPENDED: &str = "언어 서버를 일시 중지했습니다";

#[macro_export]
macro_rules! tr {
    (TASK_REMOVING_SAVED_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {}을(를) 삭제하는 중이었습니다. 저장하지 않은 파일 {}개를 저장했으므로 podd가 삭제를 멈추고 작업 폴더({})를 남깁니다.", $($arg,)* $($capture = $capture),*) };
    (HOOKS_VERSION_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("설치된 pod-hook {} · 이 Pod의 pod-hook {}", $($arg,)* $($capture = $capture),*) };
    (HOOKS_MISSING_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} 훅이 없어 작업 중·응답 필요 상태를 출력으로만 추정합니다.", $($arg,)* $($capture = $capture),*) };
    (HOOKS_OUTDATED_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("다른 pod-hook을 실행합니다: {}", $($arg,)* $($capture = $capture),*) };
    (HOOKS_PREVIEW_BINARY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("pod-hook.exe 복사 → {}", $($arg,)* $($capture = $capture),*) };
    (HOOKS_INSTALLED_BACKUPS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("백업: {}", $($arg,)* $($capture = $capture),*) };
    (LSP_LOG_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · {}", $($arg,)* $($capture = $capture),*) };
    (FILES_TAB; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("파일 · {}{}", $($arg,)* $($capture = $capture),*) };
    (UNSAVED_COUNT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!(" · 저장 안 됨 {}", $($arg,)* $($capture = $capture),*) };
    (FILE_CHANGED_ON_DISK_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} ({}) · 저장하지 않은 변경이 있어 자동으로 다시 불러오지 않았습니다. 편집기 배너에서 고르세요.", $($arg,)* $($capture = $capture),*) };
    (UNSAVED_FILES_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("저장하지 않은 파일 {}개: {}", $($arg,)* $($capture = $capture),*) };
    (REMOVE_TASK_UNSAVED_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {}의 편집기에 저장하지 않은 파일 {}개가 있습니다: {}. 작업을 삭제하면 작업 폴더와 함께 사라집니다.", $($arg,)* $($capture = $capture),*) };
    (LSP_SUSPENDED_BODY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("동시에 {}개까지만 실행합니다. 가장 오래 쓰지 않은 {} ({})를 멈췄고, 그 파일로 돌아가면 다시 시작합니다.", $($arg,)* $($capture = $capture),*) };
    (LSP_MORE; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} 외 {}개", $($arg,)* $($capture = $capture),*) };
    (NO_WORKTREE_FOR_FILES; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {}에 작업 폴더가 없습니다", $($arg,)* $($capture = $capture),*) };
    (CHECK_AGENT_INPUT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · {id}: 에이전트 입력칸을 확인하고 Enter를 누르세요", $($arg,)* $($capture = $capture),*) };
    (CHECK_PROGRESS_REPORT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 {} 실행 중: {line} · {}/{} 완료 · 통과 {passed} · 실패 {failed}", $($arg,)* $($capture = $capture),*) };
    (CHECK_PASSED_REPORT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 {} 통과: {line} · 멤버 {passed}개 모두 통과", $($arg,)* $($capture = $capture),*) };
    (CHECK_FAILED_REPORT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 {} 실패: {line} · 실패 {failed} · 통과 {passed}", $($arg,)* $($capture = $capture),*) };
    (CHECK_CANCELLED_REPORT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 {} 취소: {line}", $($arg,)* $($capture = $capture),*) };
    (CHECK_INTERRUPTED_REPORT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 {} 중단 (podd 종료): {line}", $($arg,)* $($capture = $capture),*) };
    (PICK_RESULT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} 선택됨{}", $($arg,)* $($capture = $capture),*) };
    (DURATION_MINUTES; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}분 {:02}초", $($arg,)* $($capture = $capture),*) };
    (DURATION_HOURS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}시간 {:02}분", $($arg,)* $($capture = $capture),*) };
    (DURATION_DAYS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}일 {}시간", $($arg,)* $($capture = $capture),*) };
    (DURATION_SECONDS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{s}초", $($arg,)* $($capture = $capture),*) };
    (UP_DOWN_NAVIGATE_ENTER_OPEN_ESC_CLOSE_RESULTS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("↑↓ 이동 · Enter 열기 · Esc 닫기 · 결과 {}개", $($arg,)* $($capture = $capture),*) };
    (S_AGO; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}초 전", $($arg,)* $($capture = $capture),*) };
    (RECONNECTING_ERROR; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("다시 연결하는 중… ({error})", $($arg,)* $($capture = $capture),*) };
    (DAEMON_PID_PID_PANELS_RE_ATTACH; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("데몬 pid {pid} · 패널 다시 연결", $($arg,)* $($capture = $capture),*) };
    (TITLE_NEEDS_YOU; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{title} · 응답이 필요합니다", $($arg,)* $($capture = $capture),*) };
    (BLOCKED_ON_WHY_TURN_AT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · 응답 대기 {why} · 차례 {} · {}", $($arg,)* $($capture = $capture),*) };
    (TITLE_IS_DONE; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{title} · 완료했습니다", $($arg,)* $($capture = $capture),*) };
    (TURN; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · {} · 차례 {}", $($arg,)* $($capture = $capture),*) };
    (TITLE_EXITED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{title} · 종료했습니다", $($arg,)* $($capture = $capture),*) };
    (KILL_THE_WHOLE_PROCESS_TREE_ENDS_PANELS_STAY_OPEN_SHOWING_THE_FIN; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} ({}, {})을 종료할까요? 하위 프로세스도 모두 종료합니다. 패널에는 마지막 화면이 남습니다.", $($arg,)* $($capture = $capture),*) };
    (KILL_ID_FAILED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{id} 종료 실패", $($arg,)* $($capture = $capture),*) };
    (REMOVE_ID_FAILED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{id} 삭제 실패", $($arg,)* $($capture = $capture),*) };
    (RENAME_ID; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{id} 이름 바꾸기", $($arg,)* $($capture = $capture),*) };
    (COULD_NOT_START; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}을 시작하지 못했습니다", $($arg,)* $($capture = $capture),*) };
    (TASK_CREATED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {}을 만들었습니다", $($arg,)* $($capture = $capture),*) };
    (COULD_NOT_CREATE_TASK_TITLE; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {title}을 만들지 못했습니다: {}", $($arg,)* $($capture = $capture),*) };
    (TASK_ID; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {id}", $($arg,)* $($capture = $capture),*) };
    (TASK; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {}", $($arg,)* $($capture = $capture),*) };
    (UNAVAILABLE_E; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("확인할 수 없음: {e}", $($arg,)* $($capture = $capture),*) };
    (AHEAD_BEHIND_DIRTY_UNPUSHED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("앞섬 {} · 뒤처짐 {} · 미커밋 {} · 미전송 {}", $($arg,)* $($capture = $capture),*) };
    (TASK_ID_REMOVED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {id}을 삭제했습니다", $($arg,)* $($capture = $capture),*) };
    (TERMINALS_100; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("터미널: {}", $($arg,)* $($capture = $capture),*) };
    (COULD_NOT_REMOVE_ID; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{id}을 삭제하지 못했습니다", $($arg,)* $($capture = $capture),*) };
    (PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("podd 연결 끊김 · 다시 연결 중 ({attempt}회)", $($arg,)* $($capture = $capture),*) };
    (TERMINALS_RUNNING_PANELS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("터미널 {} · 실행 {} · 패널 {}", $($arg,)* $($capture = $capture),*) };
    (WORKING; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 중 {}", $($arg,)* $($capture = $capture),*) };
    (BLOCKED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("응답 {}", $($arg,)* $($capture = $capture),*) };
    (DONE; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("완료 {}", $($arg,)* $($capture = $capture),*) };
    (TASKS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {}개", $($arg,)* $($capture = $capture),*) };
    (NOTIFICATIONS_122; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("알림 {}", $($arg,)* $($capture = $capture),*) };
    (TASKS_123; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · 작업 {}개", $($arg,)* $($capture = $capture),*) };
    (PODD_DISCONNECTED_RECONNECTING_ATTEMPT_ATTEMPT_ERROR; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("연결 재시도 중 ({attempt}회) · 마지막 출력 보관됨: {error}", $($arg,)* $($capture = $capture),*) };
    (N_COMMENT_S_QUEUED_UNTIL_THE_AGENT_IS_IDLE_WAS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · 의견 {n}개 · 에이전트 대기 중 전송 예약 (이전 {})", $($arg,)* $($capture = $capture),*) };
    (N_COMMENT_S_DELIVERED_TO; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · 의견 {n}개 · {}에 전달 ({})", $($arg,)* $($capture = $capture),*) };
    (CANCELLED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · 취소됨", $($arg,)* $($capture = $capture),*) };
    (FAILED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · 실패: {}", $($arg,)* $($capture = $capture),*) };
    (PREVIEW; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · 미리보기", $($arg,)* $($capture = $capture),*) };
    (REFUSED_AGENT_BLOCKED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("보낼 수 없습니다. 에이전트 응답 대기: {}", $($arg,)* $($capture = $capture),*) };
    (REFUSED_AGENT_TERMINAL_EXITED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("보낼 수 없습니다. 에이전트 터미널 종료: {}", $($arg,)* $($capture = $capture),*) };
    (NOTHING_TO_SEND; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("보낼 내용이 없습니다: {}", $($arg,)* $($capture = $capture),*) };
    (SEND_FAILED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("전송 실패: {}", $($arg,)* $($capture = $capture),*) };
    (REVIEW_FOR; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} 리뷰 · {}", $($arg,)* $($capture = $capture),*) };
    (READING_FILES_E; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("파일 읽기: {e:#}", $($arg,)* $($capture = $capture),*) };
    (WORKTREE_VS_SHORT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 폴더와 {} 비교 ({short})", $($arg,)* $($capture = $capture),*) };
    (COMMENT_ON_LINE_ENTER_SAVES_SHIFT_ENTER_NEW_LINE; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}:{line} 의견 · Enter 저장 · Shift+Enter 줄바꿈", $($arg,)* $($capture = $capture),*) };
    (REVIEW_SENT_TO; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}에 리뷰를 보냈습니다", $($arg,)* $($capture = $capture),*) };
    (REVIEW_QUEUED_FOR; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}에 리뷰 전송을 예약했습니다", $($arg,)* $($capture = $capture),*) };
    (REVIEW_NOT_SENT_TO; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{}에 리뷰를 보내지 못했습니다", $($arg,)* $($capture = $capture),*) };
    (FILES; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("파일 {}개", $($arg,)* $($capture = $capture),*) };
    (FILES_182; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("파일 ({})", $($arg,)* $($capture = $capture),*) };
    (SEND_SELECTED_TO_AGENT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("선택한 {}개 보내기", $($arg,)* $($capture = $capture),*) };
    (SEND_N_OPEN_TO_AGENT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("미해결 {n}개 보내기", $($arg,)* $($capture = $capture),*) };
    (AGENT_HINT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("에이전트 {}: {}{hint}", $($arg,)* $($capture = $capture),*) };
    (AGENT_189; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("에이전트 {}", $($arg,)* $($capture = $capture),*) };
    (COMMENTS_OPEN_TOTAL; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("의견 (미해결 {} · 전체 {})", $($arg,)* $($capture = $capture),*) };
    (HIDE_RESOLVED_RESOLVED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("해결한 {resolved}개 숨기기", $($arg,)* $($capture = $capture),*) };
    (SHOW_RESOLVED_RESOLVED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("해결한 {resolved}개 표시", $($arg,)* $($capture = $capture),*) };
    (NEW_COMMENT_ON_LABEL; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} {label}에 새 의견", $($arg,)* $($capture = $capture),*) };
    (NOTE_N; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("메모: {n}", $($arg,)* $($capture = $capture),*) };
    (REVIEW; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("리뷰 · {}", $($arg,)* $($capture = $capture),*) };
    (COULD_NOT_LOAD_THE_DIFF_E; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("변경 사항을 읽지 못했습니다: {e}", $($arg,)* $($capture = $capture),*) };
    (MEMBER_TASK_S_ONE_WORKTREE_EACH; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("멤버 작업 {}개 · 각각 작업 폴더 생성: {}", $($arg,)* $($capture = $capture),*) };
    (CREATING_GROUP; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("그룹 {} 만드는 중…", $($arg,)* $($capture = $capture),*) };
    (N_MEMBER_TASK_S_WORKTREE_AGENT_EACH; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("멤버 {n}개 · 각각 작업 폴더와 에이전트", $($arg,)* $($capture = $capture),*) };
    (GROUP_CREATED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("그룹 {}을 만들었습니다", $($arg,)* $($capture = $capture),*) };
    (MEMBERS_FROM; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("멤버 {}개 · 기준 {}", $($arg,)* $($capture = $capture),*) };
    (COULD_NOT_CREATE_GROUP_TITLE; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("그룹 {title}을 만들지 못했습니다", $($arg,)* $($capture = $capture),*) };
    (PICK_TASK_AS_THE_WINNER; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{task}을 최종 선택", $($arg,)* $($capture = $capture),*) };
    (RECORDS_TASK_AS_THE_WINNER_OF_GROUP_NOTHING_IS_MERGED_THE_WINNER_; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{group}의 최종 작업으로 {task}을 기록합니다. 병합하지 않으며 선택한 브랜치는 그대로 남습니다.", $($arg,)* $($capture = $capture),*) };
    (REMOVE_THE_OTHER_MEMBERS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("다른 멤버 삭제 ({})", $($arg,)* $($capture = $capture),*) };
    (REMOVED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} 삭제됨", $($arg,)* $($capture = $capture),*) };
    (WINNER; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("최종 선택: {}", $($arg,)* $($capture = $capture),*) };
    (COULD_NOT_PICK_TASK; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{task}을 선택하지 못했습니다", $($arg,)* $($capture = $capture),*) };
    (TASK_HAS_NO_TERMINAL_REMOVED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{task}에 터미널이 없습니다. 삭제되었을 수 있습니다.", $($arg,)* $($capture = $capture),*) };
    (BASE_253; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("기준 {} ({})", $($arg,)* $($capture = $capture),*) };
    (GROUP; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("그룹 {}", $($arg,)* $($capture = $capture),*) };
    (BASE_MEMBERS_MEMBER_S; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{base} · 멤버 {members}개 · 상태 {}", $($arg,)* $($capture = $capture),*) };
    (WINNER_W; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("최종 선택: {w}", $($arg,)* $($capture = $capture),*) };
    (PROMPT_257; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("요청: {}", $($arg,)* $($capture = $capture),*) };
    (PICK_REFUSED_M; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("선택할 수 없습니다: {m}", $($arg,)* $($capture = $capture),*) };
    (REMOVED_269; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!(" · {} 삭제됨", $($arg,)* $($capture = $capture),*) };
    (KEPT; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} 유지: {}", $($arg,)* $($capture = $capture),*) };
    (FILE_S; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("파일 {}개", $($arg,)* $($capture = $capture),*) };
    (DIRTY; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("미커밋 {}", $($arg,)* $($capture = $capture),*) };
    (MORE_MORE; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("… 외 {more}개", $($arg,)* $($capture = $capture),*) };
    (CHECK_ERROR_E; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 오류: {e}", $($arg,)* $($capture = $capture),*) };
    (CHECK_TIMED_OUT_AFTER_SECS_1_S; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 시간 초과 · {secs:.1}초", $($arg,)* $($capture = $capture),*) };
    (CHECK_PASSED_EXIT_SECS_1_S; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 통과 · 종료 {} · {secs:.1}초", $($arg,)* $($capture = $capture),*) };
    (CHECK_FAILED_EXIT_SECS_1_S; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 실패 · 종료 {} · {secs:.1}초", $($arg,)* $($capture = $capture),*) };
    (GROUP_290; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("그룹 · {}", $($arg,)* $($capture = $capture),*) };
    (EXPIRES_IN; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} 후 만료", $($arg,)* $($capture = $capture),*) };
    (COULD_NOT_RESOLVE_ID; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{id}을 처리하지 못했습니다", $($arg,)* $($capture = $capture),*) };
    (REQUESTED_AGO; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("· {} 전 요청", $($arg,)* $($capture = $capture),*) };
    (GATES_N; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("승인 요청 ({n})", $($arg,)* $($capture = $capture),*) };
    (PENDING_APPROVALS_306; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("승인 대기 · {}", $($arg,)* $($capture = $capture),*) };
    (RECENT_DECISIONS; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("최근 처리 ({})", $($arg,)* $($capture = $capture),*) };
    (GROUP_ID; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("⧉ 그룹 {id}", $($arg,)* $($capture = $capture),*) };
    (TASK_T; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("작업 {t}", $($arg,)* $($capture = $capture),*) };
    (GROUP_GR; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("그룹 {gr}", $($arg,)* $($capture = $capture),*) };
    (TERMINAL_T; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("터미널 {t}", $($arg,)* $($capture = $capture),*) };
    (GATE_S_WAITING_FOR_APPROVAL; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("승인 대기 {}개", $($arg,)* $($capture = $capture),*) };
    (GATE; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("승인 요청 {} {}", $($arg,)* $($capture = $capture),*) };
    (CHECK_PASSED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 통과 · {}", $($arg,)* $($capture = $capture),*) };
    (CHECK_FAILED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 실패 · {}", $($arg,)* $($capture = $capture),*) };
    (CHECK_CANCELLED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("검사 취소 · {}", $($arg,)* $($capture = $capture),*) };
    (EXPIRES_IN_325; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!(" · {} 후 만료", $($arg,)* $($capture = $capture),*) };
    (APPROVAL_REQUESTED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("승인 요청 · {}", $($arg,)* $($capture = $capture),*) };
    (EXPIRY_INBOX; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · {}{expiry} (⚑ 승인 요청)", $($arg,)* $($capture = $capture),*) };
    (N_GATE_PENDING; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("⚑ 승인 대기 {n}{}", $($arg,)* $($capture = $capture),*) };
    (EXITED_C; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("[종료 {c}]", $($arg,)* $($capture = $capture),*) };
    (UNREACHABLE_R; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("podd가 이 터미널에 연결할 수 없음: {}", $($arg,)* $($capture = $capture),*) };
    (EXITED_C_342; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("종료 {c}", $($arg,)* $($capture = $capture),*) };
    (DIRTY_343; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!(" · 미커밋 {}", $($arg,)* $($capture = $capture),*) };
    (THEME_FOLDER; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("사용자 테마 폴더: {} (명령 팔레트 › 테마: 사용자 테마 다시 읽기)", $($arg,)* $($capture = $capture),*) };
    (INSPECTOR_TERMINAL; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("터미널: {} / {}", $($arg,)* $($capture = $capture),*) };
    (INSPECTOR_UPDATED; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("마지막 상태: {}", $($arg,)* $($capture = $capture),*) };
    (ID; $($capture:ident),*; $($arg:expr),* $(,)?) => { format!("{} · {id} · 하위 {}", $($arg,)* $($capture = $capture),*) };
}

pub const RETRY: &str = "다시 시도";
pub const APPEARANCE: &str = "모양";
pub const SETTINGS_GENERAL: &str = "일반";
pub const SETTINGS_FONTS: &str = "글꼴";
pub const SETTINGS_CATEGORIES: &str = "설정 분류";
pub const REDUCE_MOTION: &str = "움직임 줄이기";
pub const HIGH_CONTRAST_PREVIEW: &str = "고대비 미리보기";
pub const OS_NOTIFICATIONS: &str = "Windows 알림";
pub const THEME_STYLE: &str = "스타일";
pub const THEME_ACCENT: &str = "강조색";
pub const THEME_LAYOUT: &str = "레이아웃";
pub const THEME_DEFAULT: &str = "테마 기본";
pub const THEME_ACCENT_FIXED: &str = "이 스타일은 강조색이 고정되어 있습니다.";
pub const LAYOUT_NOT_READY: &str = "이 레이아웃은 아직 준비 중 — 작업대로 표시합니다.";
pub const THEME_FILE_SKIPPED: &str = "테마 파일을 건너뛰었습니다";
pub const AGENT_MARK_SKIPPED: &str = "에이전트 그림 파일을 건너뛰었습니다";
pub const THEME_LOW_CONTRAST: &str = "사용자 테마의 글자 대비가 부족합니다";
pub const THEME_PRESETS: &str = "프리셋";
pub const THEME_PRESET_DETAIL: &str = "프리셋 · 스타일과 레이아웃";
pub const THEME_MODE_DETAIL: &str = "다크 · 라이트 · 시스템";
pub const THEME_RELOAD: &str = "테마: 사용자 테마 다시 읽기";
pub const THEME_RELOAD_DETAIL: &str = "테마 폴더의 JSON 파일";

/// Presentation for persisted setting keys. Font names and shortcut syntax stay intact.
pub fn setting_label(key: &str) -> &str {
    match key {
        "Theme" => "테마",
        "Font" => "UI 글꼴",
        "UI size" => "UI 글자 크기",
        "Terminal font" => "터미널 글꼴",
        "Terminal size" => "터미널 글자 크기",
        "Density" => "밀도",
        "Motion" => "움직임",
        "Jump shortcut" => "검색 단축키",
        "system" => "시스템",
        "dark" => "다크",
        "light" => "라이트",
        "theme" => "테마 기본",
        "dense" => "고밀도",
        "compact" => "기본",
        "comfortable" => "여유롭게",
        "accessible" => "접근성",
        "full" => "기본",
        "reduced" => "줄이기",
        _ => key,
    }
}

pub const WORKING: &str = "작업 중";
pub const BLOCKED: &str = "응답 필요";
pub const UNKNOWN: &str = "상태 미확인";
pub const LIVE: &str = "실행 중";
pub const KILLED: &str = "강제 종료";
pub const LOST: &str = "프로세스 연결 끊김";
/// Tooltip of an unreachable (v19) terminal whose holder podd cannot reach.
pub const UNREACHABLE: &str = "podd가 이 터미널에 연결할 수 없음";
pub const APPROVED: &str = "승인됨";
pub const REJECTED: &str = "거절됨";
pub const EXPIRED: &str = "만료됨";
pub const RECENT: &str = "최근 항목과 명령";
pub const RESULTS: &str = "검색 결과";
pub const CLEAR_SEARCH: &str = "검색 지우기";
pub const GATE_HELP: &str = "Pod 터미널에서 pod gate request 명령으로 승인을 요청할 수 있습니다.";
pub const REPO_PATH: &str = "Git 저장소 경로 (예: C:/repos/sample)";
pub const COMMAND_EXAMPLE: &str = "명령줄 (예: cmd /c echo hello)";

pub const ORIGINAL: &str = "이전";
pub const MODIFIED: &str = "변경";
pub const UNRESOLVED: &str = "미해결";
pub const QUEUED: &str = "전송 예약";
pub const SENT: &str = "전송됨";
pub const RESOLVED: &str = "해결됨";
pub fn field_label(key: &str) -> &str {
    match key {
        "branch" => "브랜치",
        "base" => "기준",
        "agent" => "에이전트",
        "worktree" => "작업 폴더",
        "terminals" => "터미널",
        _ => key,
    }
}

pub const EXITED: &str = "종료";

pub const REMOVED: &str = "삭제됨";
pub const REFRESHING: &str = "새로 읽는 중…";
pub const BINARY: &str = "바이너리";
/// File list note: the file differs from the base only in line endings (CRLF/LF).
pub const EOL_ONLY: &str = "줄바꿈만 바뀜";

pub const DELIVERED: &str = "전달됨";

pub const ATTACHING: &str = "연결 중…";

// Toolkit-owned menus and diff accessibility use the same host catalog.
const TOOLKIT_COPY: &[(&str, &str)] = &[
    ("Attachment.Remove", "삭제"),
    ("Attachment.Retry", "다시 시도"),
    ("Select.placeholder", "선택하세요"),
    ("ComboBox.placeholder", "선택하세요"),
    ("ComboBox.search_placeholder", "검색…"),
    ("ComboBox.empty", "검색 결과 없음"),
    ("DropdownButton.more_options", "더 보기"),
    ("Dock.Unnamed", "이름 없음"),
    ("Dock.Close", "닫기"),
    ("Dock.PanelMenu", "패널 메뉴"),
    ("Dock.Zoom In", "크게 보기"),
    ("Dock.Zoom Out", "원래 크기로"),
    ("Dock.Collapse", "접기"),
    ("Dock.Expand", "펼치기"),
    (
        "Dock.Unavailable Panel",
        "이 버전에서 사용할 수 없는 패널입니다.",
    ),
    ("Dialog.ok", "확인"),
    ("Dialog.cancel", "취소"),
    ("Command.placeholder", "명령 입력 또는 검색…"),
    ("Command.empty", "검색 결과가 없습니다."),
    ("List.search_placeholder", "검색…"),
    ("Input.Decrement", "줄이기"),
    ("Input.Increment", "늘리기"),
    ("Input.Replace", "바꾸기"),
    ("Input.Replace All", "모두 바꾸기"),
    ("Input.Clear", "지우기"),
    ("Input.Fold", "접기"),
    ("Input.Unfold", "펼치기"),
    ("Input.Cut", "잘라내기"),
    ("Input.Copy", "복사"),
    ("Input.Paste", "붙여넣기"),
    ("Input.Select All", "모두 선택"),
    ("Input.Go to Definition", "정의로 이동"),
    ("Input.Show Code Actions", "코드 작업 보기"),
    ("Input.Show Password", "비밀번호 표시"),
    ("Input.Hide Password", "비밀번호 숨기기"),
    ("Settings.search_placeholder", "검색…"),
    ("Settings.no_results", "일치하는 설정이 없습니다"),
    (
        "Settings.no_results_description",
        "다른 검색어로 찾아보세요.",
    ),
    ("Settings.Reset All", "모두 초기화"),
    ("Pagination.previous", "이전"),
    ("Pagination.next", "다음"),
    ("Diff.BinaryChanges", "바이너리 파일 변경"),
    ("Diff.NoTextChanges", "텍스트 변경 없음"),
    ("Diff.NoChanges", "변경 없음"),
    ("Diff.EmptyFile", "빈 파일"),
    ("Diff.AddedFile", "추가된 파일"),
    ("Diff.DeletedFile", "삭제된 파일"),
    ("Diff.RenamedFile", "이름 변경"),
    ("Diff.CopiedFile", "복사됨"),
    ("Diff.HiddenLines", "패치에 없는 %{count}줄"),
    ("Diff.ExpandAbove", "위 %{count}줄 보기"),
    ("Diff.ExpandBelow", "아래 %{count}줄 보기"),
    ("Diff.CollapseFile", "%{name} 접기"),
    ("Diff.ExpandFile", "%{name} 펼치기"),
    ("Diff.AddAnnotation", "%{line}줄에 의견 추가"),
    ("Diff.ConflictedFile", "충돌"),
    ("Diff.CurrentChange", "현재 변경"),
    ("Diff.IncomingChange", "들어오는 변경"),
    ("Diff.BaseChange", "공통 기준"),
    ("Diff.AcceptCurrent", "현재 변경 사용"),
    ("Diff.AcceptIncoming", "들어오는 변경 사용"),
    ("Diff.AcceptBoth", "두 변경 모두 사용"),
    ("Diff.AcceptedCurrent", "현재 변경 사용됨"),
    ("Diff.AcceptedIncoming", "들어오는 변경 사용됨"),
    ("Diff.AcceptedBoth", "두 변경 모두 사용됨"),
    ("Diff.UndoResolution", "되돌리기"),
    ("Diff.Original", "원본"),
    ("Diff.Modified", "변경"),
    ("Diff.ExpandLines", "변경 없는 %{count}줄 펼치기"),
    ("Diff.SourceLine", "%{side} %{line}줄"),
    ("Diff.AddedLine", "%{side} %{line}줄, 추가됨"),
    ("Diff.RemovedLine", "%{side} %{line}줄, 삭제됨"),
    ("Diff.UnchangedLines", "변경 없는 %{count}줄"),
    ("Diff.SelectLine", "%{side} %{line}줄 선택"),
    ("Diff.NoNewline", "파일 끝에 줄바꿈 없음"),
    ("Diff.Viewer", "%{name} 변경 비교"),
    ("Diff.ViewerFiles", "파일 %{count}개 변경 비교"),
];

struct KoreanToolkit;
impl rust_i18n::Backend for KoreanToolkit {
    fn available_locales(&self) -> Vec<std::borrow::Cow<'_, str>> {
        vec!["ko".into()]
    }
    fn translate(&self, locale: &str, key: &str) -> Option<std::borrow::Cow<'_, str>> {
        if locale != "ko" {
            return None;
        }
        let key = key.strip_prefix("pod.")?;
        TOOLKIT_COPY
            .iter()
            .find(|(id, _)| *id == key)
            .map(|(_, text)| (*text).into())
    }
    fn messages_for_locale(
        &self,
        locale: &str,
    ) -> Option<Vec<(std::borrow::Cow<'_, str>, std::borrow::Cow<'_, str>)>> {
        (locale == "ko").then(|| {
            TOOLKIT_COPY
                .iter()
                .map(|(key, text)| (format!("pod.{key}").into(), (*text).into()))
                .collect()
        })
    }
}
/// rust-i18n's exported extension hook keeps library source and generated assets untouched.
pub fn init_toolkit_locale() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    static KOREAN: KoreanToolkit = KoreanToolkit;
    ONCE.call_once(|| {
        gpui_kit::component::_rust_i18n_extend(&KOREAN, "pod");
        gpui_kit::component::set_locale("ko");
    });
}

#[cfg(test)]
mod toolkit_tests {
    #[test]
    fn host_catalog_reaches_dock_and_diff() {
        super::init_toolkit_locale();
        assert_eq!(
            gpui_kit::component::_rust_i18n_translate("ko", "Dock.Close"),
            "닫기"
        );
        assert_eq!(
            gpui_kit::component::_rust_i18n_translate("ko", "Diff.SelectLine"),
            "%{side} %{line}줄 선택"
        );
    }
}
