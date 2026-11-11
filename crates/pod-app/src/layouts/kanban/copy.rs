//! Korean UI text of the board (kept here so layout modules merge
//! independently). `layouts::kanban::s` is this module under its old name.

pub const ALL_WORK: &str = "모든 작업";
pub const WORKING: &str = "작업 중";
pub const NEEDS_YOU: &str = "응답 필요";
pub const DONE: &str = "완료";
pub const APPROVE: &str = "승인";
pub const REJECT: &str = "거부";
pub const LOG: &str = "로그";
pub const ANSWER: &str = "응답하기";
pub const RERUN: &str = "다시 실행";
pub const PINNED: &str = "고정";
pub const NO_TASK: &str = "작업 없음";
pub const NO_CARDS: &str = "카드 없음";
pub const NO_OUTPUT: &str = "출력 없음";
pub const ALL_REPOS: &str = "모든 저장소";
pub const ALL_AGENTS: &str = "모든 에이전트";
pub const REPO: &str = "저장소";
pub const AGENT: &str = "에이전트";
pub const CLEAR_FILTERS: &str = "필터 지우기";
pub const DRAWER: &str = "터미널 서랍";
pub const DRAWER_TERMINAL: &str = "터미널";
pub const DRAWER_PANELS: &str = "패널";
pub const DRAWER_EMPTY: &str = "카드를 고르고 Enter를 누르면 여기에 터미널이 열립니다.";
pub const DRAWER_GROW: &str = "서랍 크게";
pub const DRAWER_SHRINK: &str = "서랍 작게";
pub const DRAWER_CLOSE: &str = "서랍 닫기";
pub const DRAWER_OPEN: &str = "서랍 열기";
pub const HINT: &str = "끌어서 순서·고정 정리 · 상태는 에이전트가 보고 · F6 보드↔서랍";
pub const DRAG_TIP: &str = "끌어서 이 열 안에서 순서를 바꾸거나 열 머리에 놓아 맨 위에 고정합니다 (Alt+↑/↓, Alt+Home). \
    열(상태)은 에이전트가 보고하므로 끌어서 바꿀 수 없습니다.";
pub const NO_GATE_TIP: &str = "승인 요청(gate)이 없는 질문입니다. 터미널에서 직접 답하세요.";
pub const COULD_NOT_RERUN: &str = "다시 실행하지 못했습니다";

/// The header's count line: `에이전트 4 · 작업 2`.
pub fn counts(agents: usize, tasks: usize) -> String {
    format!("에이전트 {agents} · 작업 {tasks}")
}

/// Accessible name of a column: `응답 필요, 카드 2개`.
pub fn column_name(label: &str, cards: usize) -> String {
    format!("{label}, 카드 {cards}개")
}

/// Text of a filter button: `저장소: 모든 저장소`.
pub fn filter(what: &str, value: &str) -> String {
    format!("{what}: {value}")
}

/// Accessible name of the approve button of one gate option: `승인: 바로 운영`.
pub fn approve_option(option: &str) -> String {
    format!("{APPROVE}: {option}")
}

/// Start of a card's accessible name: `Claude · 이름, 응답 필요: 승인 필요, 작업: 글자 정렬`.
pub fn card_name(title: &str, column: &str, status: &str, task: &str) -> String {
    format!("{title}, {column}: {status}, 작업: {task}")
}

/// The card's gate, after [`card_name`].
pub fn card_gate(title: &str) -> String {
    format!(", 승인 요청: {title}")
}

/// The card's last output line, after the gate.
pub fn card_line(line: &str) -> String {
    format!(", 마지막 줄: {line}")
}

/// The end of a pinned card's accessible name.
pub const CARD_PINNED: &str = ", 고정됨";
