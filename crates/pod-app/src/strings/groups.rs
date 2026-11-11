//! Comparison groups, gates inbox, agent tree and orchestration summary copy.

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

pub const NO_GATES: &str = "⚑ 승인 요청 없음";

pub const COUNT_WORKING: &str = "작업 중";
pub const COUNT_BLOCKED: &str = "응답";

pub const COUNT_UNKNOWN: &str = "미확인";

#[cfg(test)]
pub const INFERRED: &str = "추정";
#[cfg(test)]
pub const SUMMARY_SAMPLE: &str = "작업 중 1 · 완료 1";

pub const APPROVED: &str = "승인됨";
pub const REJECTED: &str = "거절됨";
pub const EXPIRED: &str = "만료됨";

pub const GATE_HELP: &str = "Pod 터미널에서 pod gate request 명령으로 승인을 요청할 수 있습니다.";

pub const REMOVED: &str = "삭제됨";
