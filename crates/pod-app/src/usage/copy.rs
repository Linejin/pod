//! Korean copy of the usage monitor (feature-local, like `review/copy.rs`).
//! Protocol names, commands and paths stay untranslated.

pub const TITLE: &str = "AI 사용 한도";
pub const ENTRY_TOOLTIP: &str = "AI 사용 한도: 남은 양과 초기화까지 남은 시간 (세부 정보)";

pub const WINDOW_5H: &str = "5시간";
pub const WINDOW_5H_SHORT: &str = "5h";
pub const WINDOW_WEEKLY: &str = "주간";
pub const WINDOW_UNKNOWN: &str = "기간 미보고";
pub const SPEND: &str = "지출 한도";
pub const SPEND_DAILY: &str = "지출 한도(일)";
pub const SPEND_WEEKLY: &str = "지출 한도(주)";
pub const SPEND_MONTHLY: &str = "지출 한도(월)";

pub const SOURCE_CODEX: &str = "Codex app-server · account/rateLimits/read";
pub const SOURCE_CLAUDE: &str = "Claude Code statusLine · pod-hook 수집기";

pub const ST_PENDING: &str = "확인 중";
pub const ST_NOT_INSTALLED: &str = "설치 안 됨";
pub const ST_NOT_CONNECTED: &str = "연결 안 됨";
pub const ST_UNSUPPORTED: &str = "지원 안 됨";
pub const ST_AWAITING: &str = "첫 보고 대기";
pub const ST_NO_LIMITS: &str = "한도 정보 없음";
pub const ST_ERROR: &str = "읽기 실패";
pub const ST_ERROR_STALE: &str = "읽기 실패 · 이전 값";
pub const ST_NO_WINDOWS: &str = "한도 미보고";
pub const ST_EXHAUSTED: &str = "소진";
pub const ST_RESET_ELAPSED: &str = "초기화 시각 지남 · 새 보고 대기";
pub const ST_STALE: &str = "오래된 데이터";
pub const ST_LOW: &str = "얼마 안 남음";
pub const ST_OK: &str = "정상";

pub const EXHAUSTED: &str = "소진";
pub const EXHAUSTED_SHORT: &str = "소진";
pub const RESET_ELAPSED: &str = "초기화 시각 지남 · 새 보고 대기";
pub const RESET_UNKNOWN: &str = "초기화 시각 미보고";
/// Compact entry: a window without a reported reset time.
pub const RESET_UNKNOWN_SHORT: &str = "미보고";
/// Compact entry: every window passed its reset time (never "reset").
pub const RESET_ELAPSED_COMPACT: &str = "초기화 시각 지남";
pub const ELAPSED_SHORT: &str = "지남";
pub const UNDER_A_MINUTE: &str = "1분 미만";
pub const JUST_NOW: &str = "방금";
pub const NO_DATA_SHORT: &str = "한도 없음";
pub const PROFILE: &str = "프로필";
pub const PLAN: &str = "요금제";
pub const SOURCE: &str = "출처";

pub const HINT_CODEX_NOT_INSTALLED: &str =
    "Codex CLI를 찾지 못했습니다. 설치하고 로그인하면 사용 한도가 표시됩니다.";
pub const HINT_CODEX_LOGIN: &str =
    "Codex에 ChatGPT 계정으로 로그인되어 있지 않습니다. 터미널에서 codex login 후 새로고침하세요.";
pub const HINT_CODEX_API_KEY: &str =
    "API 키로 로그인된 Codex는 ChatGPT 사용 한도를 제공하지 않습니다.";
pub const HINT_CODEX_OTHER_AUTH: &str =
    "이 인증 방식(Bedrock·GCP 등)은 사용 한도를 제공하지 않습니다.";
pub const HINT_CLAUDE_NOT_CONNECTED: &str = "Claude Code 상태 줄(statusLine)에 Pod 수집기를 연결하면, 사용 중인 Claude 세션이 보고하는 한도가 표시됩니다.";
pub const HINT_CLAUDE_AWAITING: &str =
    "수집기가 연결되었습니다. Claude Code 세션이 첫 응답을 받으면 한도가 표시됩니다.";
pub const HINT_CLAUDE_NO_LIMITS: &str = "Claude Code가 한도를 보고하지 않았습니다. claude.ai Pro·Max·Ultra 구독(또는 지출 한도를 둔 Claude apps gateway)에서 세션의 첫 응답 후에 보고되며, API 키 사용 시에는 제공되지 않습니다.";
pub const HINT_CLAUDE_OTHER_STATUSLINE: &str = "다른 statusLine 명령이 설정되어 있습니다. 아래 명령은 기존 명령을 유지한 채 앞에 Pod 수집기를 연결합니다.";
pub const HINT_CLAUDE_DEPENDS: &str =
    "값은 실행 중인 Claude Code 세션이 응답할 때만 갱신됩니다. Pod가 따로 요청하지 않습니다.";

pub const REFRESH: &str = "새로고침";
pub const REFRESHING: &str = "확인 중…";
pub const COPY_SETUP_GIT_BASH: &str = "설정 복사 (Git Bash)";
pub const COPY_SETUP_POWERSHELL: &str = "설정 복사 (PowerShell)";
pub const COPIED_SETUP_BODY: &str = "~/.claude/settings.json의 \"statusLine\"을 붙여 넣은 값으로 바꾸세요. 다른 설정은 그대로 둡니다.";
pub const FORGET: &str = "기록 지우기";
pub const SETUP_GIT_BASH: &str = "Git Bash가 설치된 Windows용 statusLine 명령";
pub const SETUP_POWERSHELL: &str =
    "Git Bash가 없는 Windows(PowerShell)용 statusLine 명령 (cmd로 실행)";
pub const HINT_CLAUDE_SHELL: &str = "Windows의 Claude Code는 Git Bash가 설치되어 있으면 Git Bash로, 없으면 PowerShell로 statusLine 명령을 실행합니다. 해당하는 설정을 복사하세요.";
pub const HINT_CLAUDE_POWERSHELL_MANUAL: &str = "기존 statusLine 명령은 PowerShell 형식으로 자동 결합할 수 없습니다(식·여러 줄). 사용 안내 문서를 참고해 직접 결합하세요.";

pub fn copied_setup(shell: super::claude::Shell) -> String {
    let name = match shell {
        super::claude::Shell::GitBash => "Git Bash",
        super::claude::Shell::PowerShell => "PowerShell",
    };
    format!("Claude statusLine 설정({name})을 복사했습니다")
}
pub const OTHER_SERVICES: &str = "감지된 다른 서비스";
pub const UNSUPPORTED_SERVICE: &str =
    "한도 연동 미지원 — 문서화된 기계 판독용 한도 정보가 없습니다";
pub const NOT_SUMMED: &str = "서로 다른 한도는 합산하지 않습니다.";

pub fn synced_ago(ago: &str) -> String {
    if ago == JUST_NOW {
        format!("{ago} 동기화")
    } else {
        format!("{ago} 전 동기화")
    }
}

pub fn reported_ago(ago: &str) -> String {
    if ago == JUST_NOW {
        format!("{ago} 보고")
    } else {
        format!("{ago} 전 보고")
    }
}

pub fn left_pct(pct: f64) -> String {
    format!("{pct}% 남음")
}

pub fn previous_pct(pct: f64) -> String {
    format!("이전 {pct}% 남음")
}

pub fn spend_used_pct(pct: f64) -> String {
    format!("{pct}% 사용")
}

pub fn resets_in(countdown: &str) -> String {
    format!("{countdown} 후 초기화 예정")
}

pub fn read_failed(detail: &str) -> String {
    format!("사용 한도를 읽지 못했습니다: {detail}")
}

pub fn entry_name(sentence: &str) -> String {
    format!("{TITLE}: {sentence}. 세부 정보 열기")
}

/// The compact entry's name: the one window it shows, in full, then all.
pub fn entry_name_compact(lead: &str, sentence: &str) -> String {
    format!("{TITLE}: 가장 빠듯한 한도 {lead}. 전체: {sentence}. 세부 정보 열기")
}
