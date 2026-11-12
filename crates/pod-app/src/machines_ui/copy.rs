//! Korean product copy of the remote-machine UI (settings category, machine
//! board, remote terminal panel). Wire values, host names, fingerprints and
//! ssh's own diagnostics are shown as they are, never translated.

pub const CATEGORY: &str = "원격 머신";
/// Palette detail of the `machines` command.
pub const COMMAND_DETAIL: &str = "SSH 머신 프로필 추가·편집, 연결 확인";
pub const SECTION_TITLE: &str = "원격 머신 (SSH)";
/// TM-13: the one statement every profile screen carries.
pub const FULL_RIGHTS_TITLE: &str = "머신 프로필은 원격 계정의 전체 사용자 권한을 줍니다";
pub const FULL_RIGHTS_DETAIL: &str = "Pod는 이 계정으로 원격 Pod를 실행합니다. 그 계정이 할 수 있는 모든 명령 실행·파일 읽기와 쓰기를 이 앱에서 할 수 있게 됩니다. 믿는 머신과 계정만 추가하세요. 호스트 키는 Pod가 받아들이거나 바꾸지 않습니다(known_hosts는 직접 관리).";
pub const ONLY_THESE_FIELDS: &str =
    "프로필에는 아래 항목만 저장합니다. 임의 SSH 옵션·ProxyCommand·원격 명령은 받지 않습니다.";
pub const NO_PROFILES: &str = "등록한 머신이 없습니다.";
pub const ADD: &str = "머신 추가";
pub const EDIT: &str = "편집";
pub const REMOVE: &str = "삭제";
pub const SAVE: &str = "저장";
pub const CANCEL: &str = "취소";
pub const ADD_TITLE: &str = "머신 추가";
pub const EDIT_TITLE: &str = "머신 편집";
pub const REMOVE_TITLE: &str = "머신 삭제";
pub const FIELD_NAME: &str = "이름";
pub const FIELD_HOST: &str = "호스트";
pub const FIELD_USER: &str = "사용자 (선택)";
pub const FIELD_PORT: &str = "포트 (선택, 기본 22)";
pub const FIELD_KEY: &str = "키 파일 (선택, 절대 경로)";
pub const FIELD_KNOWN_HOSTS: &str = "known_hosts 파일 (선택, 절대 경로)";
pub const PH_NAME: &str = "예: build-box";
pub const PH_HOST: &str = "예: build.example.com";
pub const PH_USER: &str = "예: alice";
pub const PH_PORT: &str = "22";
pub const PH_KEY: &str = "예: C:\\Users\\me\\.ssh\\id_ed25519";
pub const PH_KNOWN_HOSTS: &str = "비우면 ~/.ssh/known_hosts";
pub const PORT_INVALID: &str = "포트는 1–65535 사이의 숫자여야 합니다";
pub const NAME_TAKEN: &str = "같은 이름의 머신이 이미 있습니다";
pub const STORE_ERROR: &str = "머신 목록을 읽을 수 없습니다";

// ── machine board (sidebar) ──
pub const BOARD_TITLE: &str = "원격 머신";
pub const CONNECT: &str = "연결";
pub const DISCONNECT: &str = "끊기";
pub const RETRY: &str = "다시 연결";
pub const CONFIRM_BUTTON: &str = "확인";
pub const STATE_OFF: &str = "연결 안 함";
pub const STATE_CONNECTING: &str = "연결 중…";
pub const STATE_CONNECTED: &str = "연결됨";
pub const STATE_LOST: &str = "연결 끊김";
pub const STATE_FAILED: &str = "연결 실패";
pub const STATE_NEEDS_CONFIRM: &str = "확인 필요";
pub const NO_TERMINALS: &str = "터미널 없음";
pub const NEVER_RECEIVED: &str = "받은 적 없음";
pub const TASKS: &str = "작업";
pub const TERMINALS: &str = "터미널";
pub const BLOCKED_NOT_CONFIRMED: &str =
    "머신을 확인하기 전에는 아무것도 보내지 않습니다. 머신 보드에서 확인하세요";
pub const BLOCKED_DISCONNECTED: &str = "연결이 끊겨 입력을 보내지 않았습니다";

// ── trust (host key / machine) confirmation ──
pub const TRUST_FIRST_TITLE: &str = "이 머신에 처음 연결합니다";
pub const TRUST_HOST_KEY_TITLE: &str = "호스트 키가 바뀌었습니다";
pub const TRUST_MACHINE_TITLE: &str = "다른 머신이 응답했습니다";
pub const TRUST_PROFILE_TITLE: &str = "프로필의 접속 대상이 바뀌었습니다";
pub const TRUST_FIRST_BODY: &str = "아래 호스트 키 지문이 서버 관리자에게 별도 경로(직접 확인, 사내 문서 등)로 받은 값과 같은지 확인하세요. 확인하기 전에는 이 머신에 입력·크기 변경 등 어떤 변경도 보내지 않습니다.";
pub const TRUST_HOST_KEY_BODY: &str = "known_hosts의 호스트 키가 지난번 확인한 키와 다릅니다. 서버가 다시 설치됐거나 중간자 공격일 수 있습니다. 새 지문을 관리자에게 확인한 경우에만 계속하세요.";
pub const TRUST_MACHINE_BODY: &str = "같은 프로필로 접속했지만 원격 Pod의 머신 ID가 지난번과 다릅니다. 이전 머신의 작업·터미널에는 자동으로 연결하지 않습니다.";
pub const TRUST_PROFILE_BODY: &str = "프로필의 호스트·사용자·포트·known_hosts가 지난번 확인한 것과 다릅니다. 같은 이름이라도 다른 머신일 수 있습니다.";
pub const TRUST_TARGET: &str = "접속 대상";
pub const TRUST_FINGERPRINTS: &str = "호스트 키 지문 (known_hosts)";
pub const TRUST_OLD_FINGERPRINTS: &str = "지난번 확인한 지문";
pub const TRUST_MACHINE_ID: &str = "원격 머신 ID";
pub const TRUST_OLD_MACHINE_ID: &str = "지난번 머신 ID";
pub const TRUST_OLD_TARGET: &str = "지난번 접속 대상";
pub const TRUST_NO_FINGERPRINT: &str =
    "known_hosts에서 지문을 읽지 못했습니다. ssh-keygen -l -F로 직접 확인하세요";
pub const TRUST_ACCEPT: &str = "확인했습니다";
pub const TRUST_ACCEPT_CHANGED: &str = "새 키를 확인했습니다";
pub const TRUST_SAVE_FAILED: &str = "확인 기록을 저장할 수 없습니다";

// ── failures ──
pub const FAILURE_SENT_KEY: &str = "원격이 보낸 호스트 키";
pub const FAILURE_SSH: &str = "ssh 진단";
pub const FAILURE_NO_ACCEPT: &str = "Pod는 호스트 키를 받아들이거나 known_hosts를 고치지 않습니다.";

// ── remote terminal panel ──
pub const PANEL_WAITING: &str = "머신이 연결되고 확인되면 붙습니다";
pub const PANEL_OTHER_MACHINE: &str =
    "이 터미널은 다른 머신(이전 머신 ID)의 것입니다. 자동으로 연결하지 않습니다";
pub const PANEL_ATTACHING: &str = "원격 터미널에 붙는 중…";
pub const PANEL_NO_SLOT: &str = "이 머신의 동시 연결 4개를 모두 쓰고 있어 상태만 보여 줍니다";
pub const PANEL_INPUT_BLOCKED: &str = "입력 막힘";
pub const PANEL_DISMISS: &str = "표시 닫기";
pub const RESOLUTION_APPLIED: &str =
    "끊기기 전 보낸 입력은 원격에 적용됐습니다. 다시 보내지 않았습니다";
pub const RESOLUTION_NOT_APPLIED: &str =
    "끊기기 전 보낸 입력은 원격에 닿지 않았고 앞으로도 적용되지 않습니다. 필요하면 다시 입력하세요";
pub const BOOT_CHANGED: &str = "원격 데몬이 다시 시작되었습니다. 상태를 새로 받았습니다";

/// `연결 끊김 · 마지막 수신 14:03:05 · 입력 막힘`.
pub fn lost_banner(last_rx: &str) -> String {
    format!("{STATE_LOST} · 마지막 수신 {last_rx} · {PANEL_INPUT_BLOCKED}")
}

/// `다시 연결 중 (3번째, 4초 후)`.
pub fn reconnecting(attempt: u32, delay_secs: Option<u64>) -> String {
    match delay_secs {
        Some(s) => format!("다시 연결 중 ({attempt}번째, {s}초 후)"),
        None => format!("다시 연결 중 ({attempt}번째)"),
    }
}

/// `마지막 수신 14:03:05` (local time), or that nothing came yet.
pub fn last_received_at(clock: Option<&str>) -> String {
    format!("마지막 수신 {}", clock.unwrap_or(NEVER_RECEIVED))
}

/// `보내지 않은 입력 3번`: keystrokes dropped while nothing could be sent.
pub fn dropped_keys(n: u32) -> String {
    format!("보내지 않은 입력 {n}번")
}

pub fn remove_body(name: &str, target: &str) -> String {
    format!(
        "머신 {name} ({target}) 프로필과 확인 기록을 지웁니다. 원격의 데몬·터미널·작업은 그대로 둡니다."
    )
}

pub fn unknown_resolution(reason: &str) -> String {
    format!("끊기기 전 보낸 입력이 적용됐는지 알 수 없습니다: {reason}. 화면을 보고 판단하세요")
}

pub fn exited(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("종료됨 (코드 {c})"),
        None => "종료됨".into(),
    }
}

pub fn failed(reason: &str) -> String {
    format!("원격 터미널 연결을 멈췄습니다: {reason}")
}

pub fn counts(tasks: usize, terminals: usize) -> String {
    format!("{TASKS} {tasks} · {TERMINALS} {terminals}")
}
