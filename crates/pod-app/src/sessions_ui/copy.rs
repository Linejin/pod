//! The page's wording (docs/18). [`s`] is the catalog other modules read
//! (`sessions_ui::s::SECTION` names the command; `orch_ui::s` follows the same
//! pattern); the rest are the lines the model and the view format. Not part
//! of the shared `crate::strings` catalog. Session titles, prompts, folders,
//! ids, models and argv are data, not wording.

/// Korean copy of the page (kept here; docs/18).
pub(crate) mod s {
    pub const SECTION: &str = "발견된 세션";
    pub const OPEN_PAGE_DETAIL: &str = "이 PC의 Claude Code·Codex 세션 찾기, 재개, 작업으로 등록";
    pub const INTRO: &str = "이 PC에 있는 Claude Code와 Codex 세션을 찾아 Pod에서 이어서 실행할 수 있습니다. 에이전트의 기록 파일은 읽기만 하고 바꾸지 않으며, 찾은 내용은 이 PC의 Pod에만 저장됩니다.";
    pub const OFF: &str = "세션 찾기가 꺼져 있습니다. 켜면 에이전트의 세션 기록(폴더, 시간, 제목, 첫 프롬프트)과 실행 중인 에이전트 프로세스를 읽습니다.";
    pub const ENABLE: &str = "세션 찾기 켜기";
    pub const DISABLE: &str = "세션 찾기 끄기";
    pub const RESCAN: &str = "다시 찾기";
    pub const PREVIEW_ON: &str = "첫 프롬프트 표시: 켜짐";
    pub const PREVIEW_OFF: &str = "첫 프롬프트 표시: 꺼짐";
    pub const SHOW_HIDDEN: &str = "숨긴 세션 보기";
    pub const HIDE_HIDDEN: &str = "숨긴 세션 감추기";
    pub const LOADING: &str = "세션 목록을 불러오는 중…";
    pub const UNAVAILABLE: &str =
        "세션 목록을 불러오지 못했습니다. 데몬이 세션 찾기를 지원하는 버전인지 확인하세요.";
    pub const EMPTY: &str = "찾은 세션이 없습니다.";
    pub const NOT_SCANNED: &str = "아직 찾지 않았습니다. \"다시 찾기\"를 누르세요.";
    pub const TRUNCATED: &str = "한도에 걸려 일부만 읽었습니다. 다음 찾기에서 나머지를 읽습니다.";
    pub const RUNNING_POD: &str = "실행 중 (Pod)";
    pub const RUNNING_OTHER: &str = "실행 중 (다른 터미널)";
    pub const RESUMABLE: &str = "재개 가능";
    pub const STALE: &str = "오래됨";
    pub const OTHER_NOTE: &str = "다른 앱의 터미널에서 실행 중입니다. 그 터미널은 그 앱이 소유하므로 Pod로 옮길 수 없습니다. 세션이 끝나면 여기서 재개할 수 있습니다.";
    pub const NO_FOLDER: &str = "폴더가 없어 재개할 수 없습니다.";
    pub const RESUME: &str = "이 세션 재개";
    pub const OPEN_TERMINAL: &str = "터미널 열기";
    pub const REGISTER_TASK: &str = "작업으로 등록";
    pub const REGISTER_REPO: &str = "저장소 등록";
    pub const HIDE: &str = "숨기기";
    pub const UNHIDE: &str = "다시 보이기";
    pub const LINKED: &str = "연결됨";
    pub const MATCHED: &str = "일치";
    pub const UNKNOWN_FOLDER: &str = "Pod에 등록되지 않은 폴더";
    pub const FAILED: &str = "세션 작업 실패";
    pub const RESUMED: &str = "세션을 재개했습니다";
    pub const REGISTERED: &str = "세션을 등록했습니다";
}

/// Joins a card's facts and the page's notes.
pub(super) const SEP: &str = " · ";

// ---- ages ------------------------------------------------------------------

/// No time recorded.
pub(super) const NO_TIME: &str = "-";
/// Under a minute ago.
pub(super) const JUST_NOW: &str = "방금";

pub(super) fn minutes_ago(n: u64) -> String {
    format!("{n}분 전")
}

pub(super) fn hours_ago(n: u64) -> String {
    format!("{n}시간 전")
}

pub(super) fn days_ago(n: u64) -> String {
    format!("{n}일 전")
}

// ---- page and card lines -----------------------------------------------------

/// Heading of a session with neither a title nor a first prompt.
pub(super) fn untitled(agent: &str, short_id: &str) -> String {
    format!("{agent} {short_id}")
}

pub(super) fn scanned_at(ago: &str) -> String {
    format!("마지막으로 찾은 시각: {ago}")
}

pub(super) fn session_count(n: usize) -> String {
    format!("세션 {n}개")
}

pub(super) fn unknown_running(n: usize) -> String {
    format!("세션을 알 수 없는 실행 중 에이전트 {n}개")
}

pub(super) fn branch(b: &str) -> String {
    format!("브랜치 {b}")
}

pub(super) fn last_activity(ago: &str) -> String {
    format!("마지막 활동 {ago}")
}

pub(super) fn messages(n: u64) -> String {
    format!("메시지 약 {n}개")
}

/// `연결됨: <task or repo>` / `일치: <task or repo>`.
pub(super) fn labeled(label: &str, place: &str) -> String {
    format!("{label}: {place}")
}

// ---- buttons -----------------------------------------------------------------

pub(super) fn preview_label(on: bool) -> &'static str {
    if on { s::PREVIEW_ON } else { s::PREVIEW_OFF }
}

/// The preview toggle's UIA name: its state and what a press does.
pub(super) fn preview_name(on: bool) -> String {
    format!(
        "{}. 누르면 {}",
        preview_label(on),
        if on { "끕니다" } else { "켭니다" }
    )
}

pub(super) fn rescan_name() -> String {
    format!("{} — {}", s::SECTION, s::RESCAN)
}

pub(super) fn hidden_label(show_hidden: bool, hidden: usize) -> String {
    if show_hidden {
        s::HIDE_HIDDEN.to_string()
    } else {
        format!("{} ({})", s::SHOW_HIDDEN, hidden)
    }
}

pub(super) fn hidden_name(show_hidden: bool, hidden: usize) -> String {
    if show_hidden {
        s::HIDE_HIDDEN.to_string()
    } else {
        format!("{} {}개", s::SHOW_HIDDEN, hidden)
    }
}

pub(super) const REGISTER_REPO_TIP: &str =
    "이 폴더의 git 저장소를 Pod에 등록하고 세션을 연결합니다";
pub(super) const REGISTER_TASK_TIP: &str = "세션을 일치하는 작업·저장소에 연결합니다";

/// A card's UIA name: heading, tool, state.
pub(super) fn card_name(title: &str, agent: &str, state: &str) -> String {
    format!("{title}, {agent}, {state}")
}

/// A card button's UIA name: the card's heading and the action.
pub(super) fn action_name(title: &str, action: &str) -> String {
    format!("{title} {action}")
}

// ---- toasts --------------------------------------------------------------------

pub(super) fn registered(heading: &str, place: &str) -> String {
    format!("{heading} → {place}")
}

pub(super) fn resumed(title: &str, terminal: &str) -> String {
    format!("{title} · {terminal}")
}
