//! Korean UI text of the 리뷰 중심 layout (kept here so layout modules merge
//! independently). `layouts::review::s` is this module under its old name.

pub const TASKS: &str = "작업";
pub const NO_TASKS: &str = "작업이 없습니다. 새 작업을 만들면 여기에서 변경 사항을 검토합니다.";
pub const NO_REVIEW: &str = "이 podd는 리뷰를 지원하지 않습니다 (프로토콜 v5 필요).";
pub const REVIEW_TAB: &str = "리뷰";
pub const PANELS_TAB: &str = "패널";
pub const CANVAS: &str = "리뷰 화면";
pub const TERMINALS: &str = "터미널";
pub const NO_TERMINALS: &str = "이 작업에는 터미널이 없습니다.";
pub const TERMINALS_HINT: &str = "F6 영역 이동 · Ctrl+Shift+X 크게/작게";
pub const STRIP_OPEN: &str = "터미널 펼치기";
pub const STRIP_CLOSE: &str = "터미널 접기";
pub const GROW: &str = "크게 보기";
pub const SHRINK: &str = "작게 보기";
pub const APPROVE: &str = "승인";
pub const REJECT: &str = "거부";
pub const ANSWER: &str = "응답하기";
pub const COMMENTS: &str = "의견";
pub const GATES: &str = "승인 대기";
pub const AGENTS: &str = "에이전트 상태";
pub const NO_BRANCH: &str = "브랜치 없음";
pub const COULD_NOT_RESOLVE: &str = "승인 요청을 처리하지 못했습니다";

/// The header's first words: `pod/a → main`.
pub fn branch_to_base(branch: &str, base: &str) -> String {
    format!("{branch} → {base}")
}

/// Start of a task row's accessible name: `작업 제목, pod/a → main, 상태: 작업 1`.
pub fn row_name(title: &str, branch: &str, status: &str) -> String {
    format!("{title}, {branch}, 상태: {status}")
}

/// A count after [`row_name`]: `, 의견 2개`.
pub fn row_count(what: &str, n: usize) -> String {
    format!(", {what} {n}개")
}

/// A task row's second line: `pod/a → main · 작업 1`.
pub fn row_sub(branch: &str, status: &str) -> String {
    format!("{branch} · {status}")
}

/// Accessible name of the task list: `작업, 3개`.
pub fn list_name(n: usize) -> String {
    format!("{TASKS}, {n}개")
}

/// The approve button of one gate option: `승인: 빠르게`.
pub fn approve_option(option: &str) -> String {
    format!("{APPROVE}: {option}")
}

/// Who a terminal is: `Claude · Worker 0`.
pub fn who(kind: &str, label: &str) -> String {
    format!("{kind} · {label}")
}

/// Accessible name of a terminal tile: `Claude · Worker 0, 작업 중`.
pub fn tile_name(who: &str, status: &str) -> String {
    format!("{who}, {status}")
}

/// Accessible name of a pending gate in the header: `승인 대기: 제목, 내용`.
pub fn gate_name(title: &str, message: &str) -> String {
    format!("{GATES}: {title}, {message}")
}

/// A blocked agent without a gate: `Claude · Worker 0 — 승인 필요`.
pub fn ask(who: &str, status: &str) -> String {
    format!("{who} — {status}")
}

/// Accessible name of the terminal strip: `터미널, 3개`.
pub fn terminals_name(n: usize) -> String {
    format!("{TERMINALS}, {n}개")
}

/// Title of the terminal strip: `터미널 3`.
pub fn terminals_title(n: usize) -> String {
    format!("{TERMINALS} {n}")
}

/// Terminals of the task that are not docked: `(+2)`.
pub fn hidden_tiles(n: usize) -> String {
    format!("(+{n})")
}
