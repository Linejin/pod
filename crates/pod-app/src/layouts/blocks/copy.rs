//! Product copy of the 블록 터미널 layout (kept here so layout modules merge
//! independently). Shortcut names in labels and tooltips are part of the copy;
//! key names in the input hint row (`Enter`) are not.

pub const TASKS: &str = "작업";
pub const ALL_AGENTS: &str = "모든 에이전트";
pub const SHELL: &str = "셸";
pub const CHRONO: &str = "시간순";
pub const NEEDS_YOU: &str = "응답 필요";
pub const ALLOW_ONCE: &str = "이번만 허용";
pub const ALLOW_ALWAYS: &str = "항상 허용";
pub const REJECT: &str = "거부";
pub const NO_ALWAYS_TIP: &str = "이 승인 요청은 ‘항상 허용’ 선택지를 주지 않았습니다.";
pub const COPY: &str = "복사";
pub const COPY_TIP: &str = "복사 (C)";
pub const COPIED: &str = "블록을 복사했습니다";
pub const FULL_TERMINAL: &str = "전체 터미널로 보기";
pub const RAW_ON: &str = "원본 터미널 보기";
pub const RAW_OFF: &str = "블록으로 보기";
pub const RUNNING: &str = "실행 중";
pub const WAITING: &str = "승인 대기";
pub const EARLIER: &str = "이전 출력";
pub const OUTPUT: &str = "출력";
pub const DONE: &str = "완료";
pub const ENDED: &str = "끝남";
pub const TUI_LIVE: &str = "전체 화면 · 실행 중";
pub const TUI_DONE: &str = "전체 화면 · 끝남";
pub const APPROVAL: &str = "승인 요청";
pub const NO_TASK: &str = "작업을 고르세요";
pub const NO_BLOCKS: &str =
    "아직 블록이 없습니다. 아래 입력으로 명령을 보내거나 터미널 출력이 오면 여기에 쌓입니다.";
pub const NO_TARGET: &str = "입력할 터미널이 없습니다";
pub const FEED: &str = "블록 피드";
pub const INPUT_HINT: &str = "Tab 대상 변경 · Esc 블록으로 · F6 영역 이동";
pub const OMITTED: &str = "줄 생략";
pub const COULD_NOT_RESOLVE: &str = "승인 요청을 처리하지 못했습니다";
pub const RAW_EMPTY: &str = "보여 줄 터미널이 없습니다.";
/// Body of an expanded block without lines, still running / finished.
pub const WAITING_FOR_OUTPUT: &str = "출력 기다리는 중…";
pub const NO_OUTPUT: &str = "출력 없음";

/// Step titles of the agent states (`state_label`); 응답 필요 and 완료 are
/// [`NEEDS_YOU`] and [`DONE`].
pub const STEP_WORKING: &str = "작업 중";
pub const STEP_IDLE: &str = "입력 대기";
pub const STEP_ERROR: &str = "오류";
pub const STEP_UNKNOWN: &str = "상태 확인 중";
pub const STEP_EXITED: &str = "에이전트 종료";

/// Fold state at the end of a block's accessible name.
pub const EXPANDED: &str = ", 펼쳐짐";
pub const COLLAPSED: &str = ", 접힘";

/// Status tail of a block that ended with an exit code.
pub fn exit_code(code: i32) -> String {
    format!("종료 {code}")
}

/// First body line when older lines are not shown.
pub fn omitted(n: usize) -> String {
    format!("… 앞 {n}{OMITTED}")
}

/// Accessible name of a task row: its title and the pending approvals.
pub fn task_row(title: &str, gates: usize) -> String {
    if gates > 0 {
        format!("{title}, 승인 대기 {gates}개")
    } else {
        title.to_string()
    }
}

/// Accessible name of an agent filter chip.
pub fn chip(label: &str, on: bool) -> String {
    format!("{label}{}", if on { ", 선택됨" } else { "" })
}

/// Label of the 모든 에이전트 chip.
pub fn all_agents(n: usize) -> String {
    format!("{ALL_AGENTS} {n}")
}

/// Accessible name of the task list.
pub fn task_list(n: usize) -> String {
    format!("{TASKS}, {n}개")
}

/// The line after the task title in the feed header.
pub fn meta(blocks: usize, agents: usize) -> String {
    format!("블록 {blocks} · 에이전트 {agents} · {CHRONO}")
}

/// The header's 응답 필요 count.
pub fn needs_you(n: usize) -> String {
    format!("{NEEDS_YOU} {n}")
}

/// Accessible name of the feed list.
pub fn feed(n: usize) -> String {
    format!("{FEED}, 블록 {n}개")
}

/// Placeholder of the input while it has a target.
pub fn input_placeholder(target: &str) -> String {
    format!("{target}에게 입력 · Tab으로 대상 변경")
}

/// Accessible name of the input row.
pub fn input_row(target: &str) -> String {
    format!("입력: {target} · {INPUT_HINT}")
}
