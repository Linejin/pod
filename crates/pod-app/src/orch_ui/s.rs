//! Korean copy of the orchestration screens (docs/19 §8). Kept with the
//! module, like `sessions_ui::s`, so the shared catalog stays untouched.

// Pages and commands
pub const BOARD: &str = "실행 보드";
pub const BOARD_DETAIL: &str = "Run·작업 항목·워커 상태 (Ctrl+Shift+J)";
pub const INBOX: &str = "받은 편지함";
pub const INBOX_DETAIL: &str = "워커의 질문·완료 보고·에스컬레이션에 답하기 (Ctrl+Shift+M)";
pub const CLEANUP: &str = "끝난 워커 정리";
pub const CLEANUP_DETAIL: &str = "끝난 워커의 터미널 정리, 작업 보관·재개";
pub const SETTINGS: &str = "오케스트레이션 설정";
pub const SETTINGS_DETAIL: &str = "워커 기본 배치, 자동 정리 시간, 한도";
pub const NEXT_QUESTION: &str = "다음 답할 질문으로 이동";
pub const NEXT_QUESTION_DETAIL: &str = "답을 기다리는 워커 질문의 답장 칸으로";

// Board
pub const RUNS: &str = "실행 목록";
pub const NO_RUNS: &str = "실행이 없습니다";
pub const NO_RUNS_HELP: &str = "코디네이터 에이전트가 `pod orch worker start`로 워커를 띄우면 여기에 나타납니다. 데이터는 데몬 이벤트로만 갱신됩니다.";
pub const LOADING: &str = "실행 목록을 불러오는 중…";
pub const UNSUPPORTED: &str = "이 데몬은 오케스트레이션(프로토콜 v16)을 지원하지 않습니다";
pub const RELOAD: &str = "다시 읽기";
pub const ACTIVE: &str = "진행 중";
pub const FINISHED_RUN: &str = "끝남";
pub const OBJECTIVE: &str = "목표";
pub const COORDINATOR: &str = "코디네이터";
pub const OPEN_COORDINATOR: &str = "코디네이터 터미널 열기";
pub const ITEMS: &str = "작업 항목";
pub const WORKERS: &str = "워커";
pub const NO_WORKERS: &str = "자원을 가진 워커가 없습니다. 끝난 워커는 정리 화면에 있습니다.";
pub const ITEM: &str = "항목";
pub const DEPENDS_ON: &str = "선행";
pub const ATTEMPTS: &str = "시도";
pub const BREAKER_OPEN: &str = "차단기 열림";
pub const WORKER: &str = "워커";
pub const HUMAN: &str = "사람(나)";
pub const HAS_QUESTION: &str = "질문 있음";
pub const WAITING_ANSWER: &str = "답을 기다림";
pub const QUEUED: &str = "자원 대기";
pub const LAST_ACTIVITY: &str = "마지막 활동";
pub const NO_ACTIVITY: &str = "활동 없음";
pub const UNREAD: &str = "안 읽은 우편";
pub const RELEASE_IN: &str = "정리까지";
pub const BOARD_KEYS: &str = "키보드(보드에 포커스): ↑↓ 워커 선택 · Enter 터미널 · K 유지 · R 터미널 정리 · A 포기 · S 멈추기 · V 리뷰 · [ ] 실행 바꾸기";
pub const UNREAD_FOR_COORD: &str = "코디네이터가 안 읽은 우편";

// Column labels (item states)
pub const COL_PENDING: &str = "대기";
pub const COL_READY: &str = "준비";
pub const COL_ASSIGNED: &str = "진행";
pub const COL_BLOCKED: &str = "차단";
pub const COL_COMPLETED: &str = "완료";
pub const COL_FAILED: &str = "실패";

// Attempt states
pub const A_STARTING: &str = "시작 중";
pub const A_READY: &str = "실행 중";
pub const A_SUCCEEDED: &str = "성공";
pub const A_FAILED: &str = "실패 보고";
pub const A_START_FAILED: &str = "시작 실패";
pub const A_STOPPED: &str = "멈춤";
pub const A_ABANDONED: &str = "포기";
pub const A_LOST: &str = "잃음";
pub const A_UNKNOWN: &str = "멈춤 확인 불가";

// Worker resource states
pub const W_ACTIVE: &str = "자원 사용 중";
pub const W_FINISHED: &str = "끝남 · 자동 정리 대기";
pub const W_RELEASED: &str = "터미널 정리됨";
pub const W_KEPT: &str = "사람이 유지";

// Worker actions
pub const STOP: &str = "멈추기";
pub const ABANDON: &str = "포기";
pub const KEEP: &str = "유지";
pub const RELEASE: &str = "터미널 정리";
pub const OPEN_TERMINAL: &str = "터미널 열기";
pub const OPEN_REVIEW: &str = "리뷰";
pub const STOP_CONFIRM_TITLE: &str = "워커를 멈출까요?";
pub const STOP_CONFIRM_BODY: &str = "에이전트 프로세스 트리를 끝냅니다. 끝난 것이 확인될 때만 '멈춤'이 되고, 아니면 '멈춤 확인 불가'로 남습니다. 워크트리와 브랜치는 그대로입니다.";
pub const CANCEL: &str = "취소";
pub const ACTION_FAILED: &str = "워커 작업 실패";
pub const ACTION_DONE: &str = "워커 상태를 바꿨습니다";

// Unconfirmed wake-ups (orch-02)
pub const WAKE_UNCONFIRMED: &str = "깨우기 확인 안 됨";
pub const WAKE_STUCK: &str = "깨우기 알림을 받았는지 확인되지 않았습니다";
pub const WAKE_STUCK_HELP: &str = "입력한 알림 줄이 입력 칸에 남아 있을 수 있습니다. 에이전트가 우편을 확인할 때까지 Pod는 새 알림을 입력하지 않습니다.";
pub const WAKE_SINCE: &str = "부터";
pub const WAKE_RETRY: &str = "다시 깨우기 (Enter)";
pub const WAKE_RETRIED: &str = "에이전트가 알림을 받았습니다";
pub const WAKE_NOT_TAKEN: &str = "이번에도 확인되지 않았습니다. 터미널을 열어 확인하세요.";
pub const WAKE_NO_CHECK: &str = "Enter를 눌렀습니다(이 터미널은 확인할 수 없음)";
pub const WAKE_FAILED: &str = "다시 깨우지 못했습니다";

// Mail
pub const M_QUESTION: &str = "질문";
pub const M_ANSWER: &str = "답";
pub const M_DONE: &str = "완료 보고";
pub const M_ESCALATION: &str = "에스컬레이션";
pub const M_STATUS: &str = "상태";
pub const M_HANDOFF: &str = "인계";
pub const M_HEARTBEAT: &str = "진행 메모";
pub const M_GATE: &str = "승인 요청";
pub const GATE_CHOOSE: &str = "선택해서 승인";
pub const NO_MAIL: &str = "우편이 없습니다";
pub const NO_MAIL_HELP: &str = "워커의 질문·완료 보고·에스컬레이션이 오면 여기에 모입니다. 답을 기다리는 질문이 맨 위에 옵니다.";
pub const REPLY_PLACEHOLDER: &str = "코디네이터 대신 답장 (Enter로 보내기)";
pub const REPLY: &str = "답장 보내기";
pub const REPLY_SENT: &str = "답장을 보냈습니다";
pub const REPLY_FAILED: &str = "답장을 보내지 못했습니다";
pub const CHOICE: &str = "선택";
pub const OPEN_QUESTIONS: &str = "답을 기다리는 질문";
pub const NO_OPEN_QUESTION: &str = "답을 기다리는 질문이 없습니다";
pub const READ: &str = "읽음";
pub const NOT_READ: &str = "안 읽음";
pub const FENCED: &str = "늦은 보고(반영 안 됨)";
pub const GATES_FOR_RUN: &str = "승인 요청";
// O1 (docs/19 §19): provenance of agent text, from Pod's records.
pub const PROV: &str = "출처";
pub const PROV_MACHINE_LOCAL: &str = "이 PC";
pub const PROV_RUN: &str = "실행";
pub const PROV_ITEM: &str = "항목";
pub const PROV_ATTEMPT: &str = "시도";
pub const PROV_GENERATION: &str = "세대";
pub const PROV_REPLACED: &str = "교체됨";
pub const PROV_BY_POD: &str = "Pod가 씀";
pub const PROV_BY_WORKER: &str = "워커가 씀";
pub const PROV_BY_COORDINATOR: &str = "코디네이터가 씀";
pub const PROV_BY_OUTSIDE: &str = "Pod 터미널 밖에서 씀";
pub const PROV_DATA: &str = "지시가 아닌 데이터";

// Notifications
pub const NOTICE_ESCALATION: &str = "에스컬레이션";
pub const NOTICE_BREAKER: &str = "시작 차단기 열림";
pub const NOTICE_UNKNOWN: &str = "워커 멈춤 확인 불가";

// Cleanup
pub const CLEANUP_INTRO: &str = "Attempt가 끝난 워커입니다. 터미널을 정리하면 에이전트와 언어 서버가 끝나 메모리를 돌려받고, 보관하면 작업 사이드바에서 접힙니다. 워크트리와 브랜치는 지우지 않습니다.";
pub const NO_FINISHED: &str = "끝난 워커가 없습니다";
pub const TIER_READY: &str = "준비";
pub const TIER_REVIEW: &str = "검토";
pub const TIER_PROTECTED: &str = "보호";
pub const TIER_NONE: &str = "단계 모름";
pub const SELECT: &str = "보관 대상";
pub const ARCHIVE: &str = "보관";
pub const RESUME: &str = "재개";
pub const ARCHIVE_SELECTED: &str = "선택한 작업 보관";
pub const ARCHIVED: &str = "보관됨";
pub const ARCHIVE_DONE: &str = "작업을 보관했습니다";
pub const RESUME_DONE: &str = "작업을 재개했습니다";
pub const CLEANUP_FAILED: &str = "정리 작업 실패";
pub const TIER_HELP: &str = "준비: 깨끗하고 오래 쉬었거나 병합됨(기본 선택) · 검토: 커밋 안 한 변경·푸시 안 한 커밋 · 보호: 실행 중이거나 상태를 모름(보관하지 않음)";

// Settings
pub const SETTINGS_INTRO: &str = "데몬이 지키는 기본값입니다. 바꾸면 바로 적용되고 데몬 상태 폴더의 orch\\config.json에 저장됩니다.";
pub const SET_PLACE: &str = "워커 기본 배치";
pub const SET_RELEASE: &str = "끝난 워커 자동 정리";
pub const SET_STALL: &str = "멈춤 알림";
pub const SET_DEPTH: &str = "중첩 깊이 한도";
pub const LIMITS: &str = "자원 한도 (대기열)";
pub const LIMITS_INTRO: &str = "한도에 걸리면 새 워커는 대기열에서 순서대로 기다립니다. 데몬 상태 폴더의 orch\\limits.json에 저장되고, 환경 변수가 있으면 그 값이 이깁니다.";
pub const LIM_MAX_WORKERS: &str = "동시 워커(전체)";
pub const LIM_RUN_WORKERS: &str = "실행당 워커";
pub const LIM_CPU: &str = "CPU 한도";
pub const LIM_MEM: &str = "최소 여유 메모리";
pub const LIM_DISK: &str = "최소 여유 디스크";
pub const LIM_CPU_WAIT: &str = "CPU 대기 최대";
pub const LIMITS_UNAVAILABLE: &str = "자원 한도를 읽지 못했습니다(데몬 프로토콜 v18 필요).";
pub const ALLOW: &str = "Codex 워커 좁은 허용";
pub const ALLOW_INTRO: &str = "Pod가 시작한 Codex 워커는 Pod 협업 명령(pod orch의 질문·우편·완료·대기·커밋 등)만 승인 없이 샌드박스 밖에서 실행합니다. 커밋은 pod orch commit으로 Pod가 훅 없이 대신 합니다. 이 커밋은 저장소 훅(pre-commit 검사 포함)과 LFS 같은 git 필터를 돌리지 않으며(필터가 필요한 경로는 거부), merge나 검사 통과가 아닙니다. 그 밖의 명령, 파일 쓰기 범위, 네트워크는 Codex 샌드박스 그대로입니다. 저장소별로 .pod/config.toml의 [worker] codex_allowance = false로 끕니다(기본 켬).";
pub const ALLOW_ON: &str = "켬";
pub const ALLOW_OFF: &str = "끔 (.pod/config.toml)";
pub const ALLOW_UNKNOWN: &str = "알 수 없음";
pub const ALLOW_NO_REPOS: &str = "등록된 저장소가 없습니다.";
pub const PLACE_NEW: &str = "새 워크트리";
pub const PLACE_CURRENT: &str = "호출자 워크트리";
pub const NEVER: &str = "끔";
pub const LOCKED: &str = "환경 변수로 고정됨";
pub const SAVED: &str = "오케스트레이션 설정을 저장했습니다";
pub const SAVE_FAILED: &str = "설정을 저장하지 못했습니다";
pub const SETTINGS_UNAVAILABLE: &str =
    "설정을 읽지 못했습니다. 데몬이 orch.config를 지원하는 버전인지 확인하세요.";
pub const OPEN_SETTINGS_PAGE: &str = "오케스트레이션 설정 열기";
