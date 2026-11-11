//! App shell copy: workspace, chrome (palette, settings, notifications, inspector), panels,
//! dialogs, editor and terminal-link integration, language server menu and agent hook prompt.

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

pub const RENAME: &str = "이름 바꾸기";
pub const RENAME_FAILED: &str = "이름을 바꾸지 못했습니다";
pub const WORKING_DIRECTORY: &str = "작업 폴더";
pub const OPTIONAL_E_G_REVIEW_BOT: &str = "이름 (선택, 예: review-bot)";
pub const PROGRAM: &str = "프로그램";
pub const COMMAND: &str = "명령";

pub const RUN: &str = "실행";
pub const PODD_CLIENT_STOPPED: &str = "podd 연결이 닫혔습니다";
pub const REVIEW_NEEDS_A_NEWER_PODD: &str = "리뷰에 새 podd가 필요합니다";
pub const PODD_PROTOCOL_V5_REVIEW_IS_REQUIRED: &str = "podd 프로토콜 v5 (review.*)가 필요합니다.";
pub const NO_SUCH_TASK: &str = "작업을 찾을 수 없습니다";
pub const NO_TASK_TO_REVIEW: &str = "리뷰할 작업이 없습니다";
pub const CREATE_A_TASK_FIRST_TASK: &str = "먼저 새 작업을 만드세요.";
pub const TASK_NAME_E_G_FIX_LOGIN_BRANCH_POD_NAME: &str =
    "작업 이름 (예: fix-login, 브랜치 pod/<name>)";

pub const OPTIONAL_FIRST_PROMPT_CLAUDE_CODEX: &str = "첫 요청 (선택, claude / codex)";

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

pub const GROUP_330: &str = "+ 그룹";
pub const FAN_OUT_ONE_PROMPT_TO_SEVERAL_AGENTS_CTRL_SHIFT_N: &str =
    "여러 에이전트에 같은 요청 보내기 (Ctrl+Shift+N)";
pub const AGENT_TREE_GROUPS_TASKS_AND_TERMINALS_STARTED_FROM_OTHER_TERMINAL: &str =
    "에이전트 트리 (Ctrl+Shift+E)";

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

// ---- P3 editor integration (editor_panel.rs, docs/13 §2) ----
pub const FILE_CHANGED_ON_DISK: &str = "에이전트가 열린 파일을 바꿨습니다";
pub const QUICK_OPEN_FILE: &str = "파일 열기";
pub const QUICK_OPEN_FILE_DETAIL: &str = "Ctrl+P · 현재 작업 폴더에서 파일 이름 찾기";
pub const FILES_PANEL: &str = "파일 탐색기 열기";
pub const FILES_PANEL_DETAIL: &str = "현재 작업의 파일 트리와 편집기";
pub const SAVE_ALL_FILES: &str = "모든 파일 저장";
pub const SAVE_ALL_FILES_DETAIL: &str = "열린 편집기의 저장하지 않은 파일";

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

pub const RECENT: &str = "최근 항목과 명령";
pub const RESULTS: &str = "검색 결과";
pub const CLEAR_SEARCH: &str = "검색 지우기";

pub const COMMAND_EXAMPLE: &str = "명령줄 (예: cmd /c echo hello)";

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

pub const ATTACHING: &str = "연결 중…";
