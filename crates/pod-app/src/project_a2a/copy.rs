//! Korean copy of the A2A peer section (feature-local, like the graph's).

use pod_a2a::model::{StateClass, TaskState};

use super::model::NotReady;

pub const TOGGLE: &str = "외부 A2A 피어";
pub const TOGGLE_TIP: &str = "이 프로젝트에 설정된 표준 Agent2Agent(A2A 1.0) 피어에 검토한 메시지를 보내고 원격 작업 상태를 봅니다";
pub const TITLE: &str = "외부 A2A 피어 · 표준 Agent2Agent 1.0";
pub const SCOPE: &str = "Pod 터미널 그래프·오케스트레이션과 별개입니다. 검토한 글만 보내며, 프로젝트 파일·터미널 기록·자격 증명은 자동으로 보내지 않습니다. 피어가 보낸 내용은 데이터로만 표시합니다.";
pub const RELOAD: &str = "피어 설정 다시 읽기";
pub const NO_FILE_TITLE: &str = "설정된 A2A 피어가 없습니다";
pub const NOT_CONFIGURED_TITLE: &str = "이 프로젝트에 설정된 A2A 피어가 없습니다";
pub const INVALID_TITLE: &str = "A2A 피어 설정을 읽을 수 없습니다";
pub const PEERS: &str = "피어 선택";
pub const CHOOSE_PEER: &str =
    "보낼 피어를 직접 고르세요. 고르기 전에는 아무 피어에도 연결하지 않습니다.";
pub const CARD_LOADING: &str = "에이전트 카드를 가져오는 중…";
pub const CARD_RELOAD: &str = "카드 다시 가져오기";
pub const SKILLS: &str = "기술";
pub const NO_SKILL: &str = "지정 안 함";
pub const SKILL_NOTE: &str = "A2A 1.0 요청에는 기술 필드가 없습니다. 고른 기술의 입력 형식을 확인하고 출력 형식을 acceptedOutputModes로 보냅니다. 처리할 기술은 피어가 정합니다.";
pub const DRAFT: &str = "보낼 메시지";
pub const DRAFT_PLACEHOLDER: &str = "피어에게 보낼 글 (보내기 전에 그대로 검토합니다)";
pub const REVIEW: &str = "검토";
pub const REVIEW_TITLE: &str = "보낼 내용 검토 — 아래 글과 정보만 전송됩니다";
pub const SEND: &str = "보내기";
pub const EDIT: &str = "수정";
pub const SENDING: &str = "보내는 중…";
pub const NOTHING_ELSE: &str = "그 밖에 보내는 것 없음: 프로젝트 파일, 터미널 출력, Pod 작업 정보, 자격 증명은 포함되지 않습니다.";
pub const RESPONSE_MODE: &str = "즉시 반환(returnImmediately) 후 GetTask로 상태 확인";
pub const FAILED_TITLE: &str = "보내지 않았습니다 — 초안은 그대로 있습니다";
pub const UNKNOWN_TITLE: &str =
    "전송 결과를 알 수 없습니다 — 피어가 받았을 수 있습니다. 자동으로 다시 보내지 않습니다.";
pub const RESEND: &str = "같은 메시지 ID로 다시 검토";
pub const NEW_MESSAGE: &str = "새 메시지로 작성";
pub const TASKS: &str = "이 프로젝트에서 보낸 원격 A2A 작업";
pub const NO_TASKS: &str = "아직 보낸 작업이 없습니다.";
pub const REPLIES: &str = "직접 답장 (원격 작업 없음)";
pub const REFRESH: &str = "상태 새로 고침";
pub const CONTINUE_TASK: &str = "이 작업에 답하기";
pub const CONTINUE_CONTEXT: &str = "이 대화 이어서";
pub const STOP_CONTINUE: &str = "이어 보내기 해제";
pub const AUTO_STOPPED: &str = "자동 확인 멈춤 — 새로 고침을 누르세요";
pub const PEER_DATA: &str = "피어 제공 내용";
pub const URL_NOT_OPENED: &str = "링크 (Pod는 열거나 받지 않음)";
pub const AUTH_REQUIRED_NOTE: &str = "피어 안내에 따라 Pod 밖에서 인증한 뒤 새로 고침하세요. Pod는 자격 증명을 대신 보내지 않습니다.";

pub fn section_label(repo: &str) -> String {
    format!("{repo} {TITLE}")
}

pub fn config_hint(path: &str, repo: &str) -> String {
    format!(
        "앱 설정 파일 {path} 의 projects.\"{repo}\".peers 에 피어를 적으면 여기에 나옵니다 (docs/09 §8.1)."
    )
}

pub fn other_path(configured: &str) -> String {
    format!("설정의 repo_path({configured})가 이 저장소의 등록 경로와 달라 피어를 쓰지 않습니다.")
}

/// The remote state, never shown like a Pod agent state.
pub fn state_text(s: &TaskState) -> String {
    let class = match s.class() {
        StateClass::Active => "진행 중",
        StateClass::Interrupted => "응답 필요",
        StateClass::Terminal => "끝남",
        StateClass::Unknown => "알 수 없음",
    };
    format!("원격 A2A 상태 {} · {class}", s.word())
}

pub fn not_ready(n: &NotReady) -> String {
    match n {
        NotReady::NoPeer => "먼저 피어를 고르세요.".into(),
        NotReady::CardMissing => "피어의 에이전트 카드를 아직 확인하지 못했습니다.".into(),
        NotReady::EmptyText => "보낼 글을 쓰세요.".into(),
        NotReady::TooLong(n) => format!("글이 너무 깁니다 ({n}바이트, 최대 64 KiB)."),
        NotReady::TextNotAccepted(m) => format!(
            "고른 기술(또는 카드)이 text/plain을 받지 않습니다 (입력 형식: {}).",
            m.iter()
                .map(|s| pod_a2a::sanitize_line(s, 60))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        NotReady::TokenNotDeclared => {
            "토큰이 설정됐지만 카드가 HTTP Bearer를 선언하지 않아 보낼 수 없습니다.".into()
        }
        NotReady::Busy => "보내는 중입니다.".into(),
        NotReady::Unconfirmed => {
            "결과를 모르는 메시지가 있습니다. 같은 ID로 다시 보내거나 새 메시지로 작성을 고르세요."
                .into()
        }
    }
}

pub fn continuing(task: &str, context: &str) -> String {
    if task.is_empty() {
        format!("대화 이어 보내기 · 컨텍스트 {context}")
    } else {
        format!("작업 이어 보내기 · 작업 {task} · 컨텍스트 {context}")
    }
}

pub fn bearer(sent: bool) -> &'static str {
    if sent {
        "HTTP Bearer 토큰 보냄 (설정한 토큰, 선택된 전송 주소에만)"
    } else {
        "보내지 않음"
    }
}

pub fn fetched(n: u32) -> String {
    if n == 0 {
        "SendMessage 응답".into()
    } else {
        format!("GetTask {n}회 확인")
    }
}

pub fn raw(bytes: Option<usize>, name: &str) -> String {
    let size = bytes
        .map(|b| format!("{b}바이트"))
        .unwrap_or_else(|| "크기 불명".into());
    if name.is_empty() {
        format!("이진 데이터 {size} — 저장하지 않음")
    } else {
        format!("이진 데이터 {name} {size} — 저장하지 않음")
    }
}

pub fn capabilities(streaming: Option<bool>, push: Option<bool>) -> String {
    let yn = |v: Option<bool>| match v {
        Some(true) => "있음 (Pod 미사용)",
        _ => "없음",
    };
    format!(
        "스트리밍 {} · 푸시 알림 {} · 상태는 GetTask로 확인",
        yn(streaming),
        yn(push)
    )
}
