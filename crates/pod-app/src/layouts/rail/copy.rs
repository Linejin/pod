//! Product copy of the 집중 레일 layout (kept here so layout modules merge
//! independently). Shortcut names in tooltips are part of the copy; the
//! `aria-keyshortcuts` key names are not.

pub const RAIL: &str = "레일 탐색";
pub const WORKSPACE: &str = "작업 공간";
pub const WORKSPACE_TIP: &str = "작업 공간 — 저장소·작업·터미널 (Ctrl+Alt+W)";
pub const TERMINAL: &str = "터미널";
pub const TERMINAL_TIP: &str = "현재 작업의 터미널로 이동";
pub const FILES: &str = "파일";
pub const FILES_TIP: &str = "현재 작업의 파일 (Ctrl+P)";
pub const AGENTS: &str = "에이전트";
pub const AGENTS_TIP: &str = "에이전트 트리 (Ctrl+Shift+E)";
pub const ATTENTION: &str = "응답 대기";
pub const ATTENTION_TIP: &str = "응답 대기 — 다른 작업의 승인·실패 (Ctrl+Alt+A)";
pub const ATTENTION_TITLE: &str = "응답이 필요한 다른 작업";
pub const ATTENTION_EMPTY: &str = "다른 작업에 기다리는 요청이 없습니다.";
pub const ALL_NOTIFICATIONS: &str = "알림 모두 보기";
pub const GATES: &str = "승인함 열기";
pub const TABS: &str = "작업 탭";
pub const SWITCHER: &str = "작업 전환";
pub const SWITCHER_TIP: &str = "작업 전환 (Ctrl+Alt+T) · 다음/이전 작업 Ctrl+Alt+PageDown/PageUp";
pub const ALL_TASKS: &str = "모든 작업";
pub const NO_TASKS: &str = "작업이 없습니다";
pub const EMPTY_TITLE: &str = "이 작업에 열린 패널이 없습니다";
pub const EMPTY_BODY: &str =
    "터미널이나 파일을 열어 시작하세요. 다른 작업은 위 탭이나 작업 전환(Ctrl+Alt+T)에서 고릅니다.";
pub const NO_TASK_BODY: &str = "새 작업을 만들거나 작업 전환에서 작업을 고르세요.";
pub const CLOSE: &str = "닫기";
pub const CLOSE_TIP: &str = "닫기 (Esc)";
pub const NEW_TASK_TIP: &str = "새 작업 (Ctrl+N)";
pub const NEW_SHELL: &str = "새 셸 터미널";
/// What waits in a task (`TaskRow::attention_label`).
pub const BLOCKED: &str = "승인 필요";
pub const GATE_REQUESTS: &str = "승인 요청";
pub const FAILED: &str = "실패";

/// Accessible name of the 응답 대기 rail button.
pub fn attention_button(n: usize) -> String {
    if n == 0 {
        format!("{ATTENTION}: 없음")
    } else {
        format!("{ATTENTION}: 다른 작업 {n}개")
    }
}

/// Accessible name of the `+N` button (tabs that did not fit).
pub fn hidden_tasks(n: usize) -> String {
    format!("숨은 작업 {n}개 — {SWITCHER}")
}

/// The empty state's first button: open the running terminals, or a shell.
pub fn open_terminals(running: usize) -> String {
    if running > 0 {
        format!("터미널 열기 ({running})")
    } else {
        NEW_SHELL.to_string()
    }
}

/// Accessible name of the current task's drawer row.
pub fn current_task(name: &str) -> String {
    format!("{name} (현재 작업)")
}
