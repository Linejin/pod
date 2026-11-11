//! Wording of the extensions UI (docs/16), in Korean. [`s`] is the catalog
//! other modules name (`mods_ui::s`: the palette's "확장 관리" row, native
//! QA's toast titles); the label tables and the formatted lines below are
//! used by the page, the cards and the Settings row only. Mod names,
//! versions, permission ids, command ids and podd's error text are data,
//! not wording.

use podd::protocol::{Permission, StopReason};

/// Korean copy of the extensions UI (kept here; docs/16).
pub(crate) mod s {
    pub const SECTION: &str = "확장";
    pub const OPEN_PAGE: &str = "확장 관리";
    pub const OPEN_PAGE_DETAIL: &str = "모드 켜기·끄기, 권한, 로그";
    pub const INTRO: &str = "모드는 Pod 데몬 안의 샌드박스에서 돕니다. 파일·네트워크에 접근할 수 없고, 허용한 권한만 씁니다.";
    pub const FOLDER: &str = "모드 폴더";
    pub const RELOAD: &str = "다시 읽기";
    pub const LOADING: &str = "확장 목록을 불러오는 중…";
    pub const EMPTY: &str =
        "설치된 확장이 없습니다. 모드 폴더에 모드를 복사한 뒤 \"다시 읽기\"를 누르세요.";
    pub const OFF: &str = "이 데몬은 확장이 꺼져 있습니다 (POD_MODS=off).";
    pub const UNAVAILABLE: &str = "확장 목록을 불러오지 못했습니다. 데몬이 확장을 지원하는 버전(프로토콜 v11)인지 확인하세요.";
    pub const ENABLE: &str = "켜기";
    pub const DISABLE: &str = "끄기";
    pub const RUNNING: &str = "실행 중";
    pub const DISABLED: &str = "꺼짐";
    pub const STOPPED: &str = "정지됨";
    pub const INVALID: &str = "설정 오류";
    pub const PERMISSIONS: &str = "권한";
    pub const NO_PERMISSIONS: &str = "권한 없음 (레이아웃만 제공)";
    pub const ALLOWED: &str = "허용";
    pub const DENIED: &str = "거부";
    pub const SHOW_LOG: &str = "로그 보기";
    pub const HIDE_LOG: &str = "로그 닫기";
    pub const LOG_EMPTY: &str = "로그가 없습니다.";
    pub const CHECKS: &str = "검사 명령";
    pub const LAYOUT_PREFIX: &str = "레이아웃";
    pub const LAYOUT_EMPTY: &str = "적용할 패널이 없습니다";
    pub const LAYOUT_EMPTY_DETAIL: &str = "현재 작업에 터미널이나 검토할 내용이 없습니다.";
    pub const LAYOUT_MISSING: &str = "레이아웃을 찾을 수 없습니다";
    pub const LAYOUT_MISSING_DETAIL: &str =
        "확장이 꺼졌거나 다시 읽는 동안 프리셋이 사라졌습니다. 설정 → 확장에서 확인하세요.";
    pub const LAYOUT_INVALID: &str = "레이아웃 프리셋 오류";
    pub const MANIFEST_INVALID: &str = "mod.json 오류";
    pub const COMMAND_FAILED: &str = "확장 명령 실패";
    pub const CHANGE_FAILED: &str = "확장 설정을 바꾸지 못했습니다";
}

pub(super) fn permission_label(p: Permission) -> &'static str {
    match p {
        Permission::Notify => "알림 표시",
        Permission::Commands => "팔레트 명령",
        Permission::Status => "상태 감지",
        Permission::TerminalSend => "터미널에 입력",
        Permission::Check => "검사 명령 실행",
        Permission::Gate => "승인 요청",
        Permission::Orch => "오케스트레이션 이벤트 읽기",
    }
}

pub(super) fn stop_reason_label(r: Option<StopReason>) -> &'static str {
    match r {
        Some(StopReason::Cpu) => "CPU 시간 한도 초과",
        Some(StopReason::Memory) => "메모리 한도 초과",
        Some(StopReason::Errors) => "오류 반복",
        Some(StopReason::Load) => "불러오기 실패",
        None => "",
    }
}

/// The Settings row's summary of the list.
pub(super) fn installed(total: usize, enabled: usize) -> String {
    format!("{total}개 설치 · {enabled}개 켜짐")
}

/// UIA name of a permission toggle: what it is now and what a press makes it.
pub(super) fn permission_toggle(name: &str, label: &str, verdict: &str, next: &str) -> String {
    format!("{name} 권한 {label}: {verdict}. 누르면 {next}")
}

/// What a card's mod provides, one fact each.
pub(super) fn command_count(n: usize) -> String {
    format!("명령 {n}개")
}

pub(super) fn detectors(names: &str) -> String {
    format!("상태 감지: {names}")
}

pub(super) fn layouts(titles: &str) -> String {
    format!("레이아웃: {titles}")
}

pub(super) fn memory(kib: u64) -> String {
    format!("메모리 {kib} KiB")
}
