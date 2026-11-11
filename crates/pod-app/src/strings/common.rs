//! Copy shared by several features: dialog field labels, the agent mark suffix and the
//! terminal/gate status words of the view model.

// Agent marks (docs/10 §D5.11): service names stay Latin; the suffix is Korean.
pub const AGENT_SUFFIX: &str = "에이전트";
pub const AGENT_SHELL_TERMINAL: &str = "셸 터미널";

// Fields and buttons of the task, group and terminal dialogs.
pub const TERMINALS: &str = "터미널";
pub const CANCEL: &str = "취소";
pub const KILL: &str = "종료";
pub const NAME: &str = "이름";
pub const BASE_REF_DEFAULT_THE_REPO_S_DEFAULT_BRANCH: &str =
    "기준 브랜치 (기본: 저장소의 기본 브랜치)";
pub const ADD_REPOSITORY: &str = "저장소 추가…";
pub const SHELL: &str = "셸";
pub const REPOSITORY: &str = "저장소";
pub const BASE: &str = "기준";
pub const AGENT: &str = "에이전트";
pub const PROMPT: &str = "요청";
pub const REMOVE_ANYWAY: &str = "그래도 삭제";
pub const CUSTOM: &str = "사용자 지정";
pub const REPO_PATH: &str = "Git 저장소 경로 (예: C:/repos/sample)";

// Gate decisions.
pub const APPROVE: &str = "승인";
pub const REJECT: &str = "거절";

// Terminal and task status words of the view model.
pub const ERROR: &str = "오류";
pub const IDLE: &str = "대기";
pub const DONE: &str = "완료";
pub const WORKING: &str = "작업 중";
pub const BLOCKED: &str = "응답 필요";
pub const UNKNOWN: &str = "상태 미확인";
pub const LIVE: &str = "실행 중";
pub const KILLED: &str = "강제 종료";
pub const LOST: &str = "프로세스 연결 끊김";
/// Tooltip of an unreachable (v19) terminal whose holder podd cannot reach.
pub const UNREACHABLE: &str = "podd가 이 터미널에 연결할 수 없음";
pub const EXITED: &str = "종료";
