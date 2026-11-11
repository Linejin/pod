//! The chrome's own copy (docs/20-refactor/app.md §2 "문구/i18n"): words
//! only the chrome shows. Words other features share stay in `crate::strings`.

/// Title bar search pill: its tooltip (also the accessible description) and
/// key cap, for Ctrl+K and for Ctrl+Shift+K.
pub(super) const JUMP_TIP: &str = "명령·터미널 검색 (Ctrl+K)";
pub(super) const JUMP_TIP_ALT: &str = "명령·터미널 검색 (Ctrl+Shift+K)";
pub(super) const JUMP_KEYCAP: &str = "Ctrl K";
pub(super) const JUMP_KEYCAP_ALT: &str = "Ctrl Shift K";
/// Title bar settings button tooltip.
pub(super) const SETTINGS_TIP: &str = "설정 (Ctrl+,)";

/// Shortcut help: the palette command, the dialog title and its first row
/// (`<jump key> — 명령·터미널 검색`).
pub(super) const SHORTCUTS_LABEL: &str = "키보드 단축키 도움말";
pub(super) const SHORTCUTS_DETAIL: &str = "단축키 · Keyboard shortcuts";
pub(super) const SHORTCUTS_TITLE: &str = "키보드 단축키";
pub(super) const JUMP_KEY: &str = "Ctrl+K";
pub(super) const JUMP_KEY_ALT: &str = "Ctrl+Shift+K";
pub(super) const JUMP_HELP: &str = "명령·터미널 검색";

/// The rest of the shortcut help, as docs/10 lists it (`| keys | label |`).
pub(super) const SHORTCUTS: &[(&str, &str)] = &[
    ("Ctrl+N", "새 작업"),
    ("Ctrl+Shift+T", "새 터미널"),
    ("Ctrl+,", "설정"),
    ("Ctrl+Shift+S", "상태 인스펙터"),
    ("Ctrl+Shift+B", "알림"),
    ("Ctrl+Shift+R", "변경 검토"),
    ("Ctrl+Shift+G", "패널 격자 정렬"),
    ("Ctrl+Shift+N", "새 비교 그룹"),
    ("Ctrl+Shift+I", "승인 요청"),
    ("Ctrl+Shift+E", "에이전트 트리"),
    ("Ctrl+P", "파일 열기 (현재 작업 폴더)"),
    ("Ctrl+S / Ctrl+K S", "편집기: 저장 / 모두 저장"),
    ("Ctrl+W", "편집기: 탭 닫기 (저장 확인)"),
    ("Ctrl+.", "편집기: 코드 작업 (빠른 수정)"),
    ("Ctrl+클릭", "터미널: 파일:줄 · 주소 열기"),
    ("Alt+1 / Alt+2", "편집기: 파일 트리 / 편집기로 초점"),
    ("F2 / Delete", "파일 트리: 이름 바꾸기 / 삭제"),
    ("Ctrl+Shift+O", "검토: 가리킨 줄을 편집기에서 열기"),
    ("Tab / Shift+Tab", "다음 / 이전 컨트롤"),
    ("Esc", "대화 상자 닫고 이전 포커스로"),
    ("↑ / ↓, Enter", "검색 결과 탐색 / 열기"),
    ("F7 / Shift+F7", "검토: 다음 / 이전 변경"),
    ("Alt+↓ / Alt+↑", "검토: 다음 / 이전 변경"),
    ("Alt+PageDown / Alt+PageUp", "검토: 다음 / 이전 파일"),
    ("Ctrl+Shift+C", "검토: 의견 추가"),
    ("Ctrl+Shift+Enter", "검토: 미해결 의견 보내기"),
    ("F5", "검토 새로 고침"),
];

/// Theme modes in the palette: `theme-mode:<id>` and `테마: <name>`.
pub(super) const THEME_MODES: [(&str, &str); 3] =
    [("dark", "다크"), ("light", "라이트"), ("system", "시스템")];
