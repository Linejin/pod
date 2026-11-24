//! Korean copy of the project agent graph (feature-local, like orch_ui's `ko`).

use super::model::{EdgeKind, ParentNote};

/// The sidebar action and the panel's tab name.
pub const ACTION: &str = "에이전트 그래프";
pub const GRAPH_NAME: &str = "프로젝트 에이전트 그래프";
pub const HINT: &str = "이 저장소 작업의 에이전트 터미널과, podd가 기록한 시작 관계(작업 시작 · pod run)만 연결합니다. 노드를 선택하면 그 터미널을 엽니다.";
pub const LEGEND_TASK: &str = "작업 시작";
pub const LEGEND_RUN: &str = "pod run";
pub const EMPTY_TITLE: &str = "이 프로젝트에 에이전트가 없습니다";
pub const EMPTY_BODY: &str =
    "작업을 만들어 에이전트를 시작하면 여기에 표시됩니다. 일반 셸 터미널은 그래프에 넣지 않습니다.";
pub const UNKNOWN_TITLE: &str = "등록된 저장소가 아닙니다";
pub const UNKNOWN_BODY: &str =
    "podd에 이 저장소가 없거나 제거되었습니다. 사이드바에서 다른 저장소를 고르세요.";
pub const CONNECTING: &str = "podd에 연결하는 중 — 아직 받은 목록이 없습니다.";
pub const DISCONNECTED: &str =
    "podd 연결 끊김 — 마지막으로 받은 상태이며 지금과 다를 수 있습니다. 다시 연결 중…";
pub const NO_STATUS: &str =
    "이 podd는 에이전트 상태를 보내지 않습니다 — 상태는 미확인으로 표시됩니다.";
pub const NOT_LISTED: &str = "podd 목록에 없음 — 열 수 없음";
pub const CONNECTIONS: &str = "연결";
pub const NO_CONNECTIONS: &str = "기록된 시작 관계가 없습니다. 각 에이전트는 따로 표시됩니다.";
pub const NO_TASK: &str = "작업 없음";

pub fn tab_name(repo: &str) -> String {
    format!("{ACTION} · {repo}")
}

pub fn graph_label(repo: &str, nodes: usize, edges: usize) -> String {
    format!("{repo} {GRAPH_NAME} · 에이전트 {nodes} · 연결 {edges}")
}

pub fn action_label(repo: &str) -> String {
    format!("{repo} {ACTION}")
}

pub fn action_tip(repo: &str) -> String {
    format!("{repo} 저장소의 에이전트, 연결, 작업 상태를 그래프로 보기")
}

pub fn summary(tasks: usize, nodes: usize, edges: usize) -> String {
    format!("작업 {tasks}개 · 에이전트 {nodes} · 연결 {edges}")
}

pub fn edge_kind(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::StartedTask => LEGEND_TASK,
        EdgeKind::RanTerminal => LEGEND_RUN,
    }
}

/// `Claude 리더 (t1) → Codex 검토 (t2) · 작업 시작`
pub fn edge_text(from: &str, to: &str, kind: EdgeKind) -> String {
    format!("{from} → {to} · {}", edge_kind(kind))
}

pub fn parent_text(parent: &str, kind: EdgeKind) -> String {
    format!("상위 {parent} · {}", edge_kind(kind))
}

pub fn children_text(n: usize) -> String {
    format!("하위 {n}")
}

pub fn note_text(note: &ParentNote) -> String {
    match note {
        ParentNote::Missing(p) => format!("상위 터미널 {p} 없음 (종료·제거됨)"),
        ParentNote::Shell(p) => format!("상위: 셸 터미널 {p} (그래프 밖)"),
        ParentNote::OtherProject(p) => format!("상위: 다른 프로젝트의 터미널 {p}"),
        ParentNote::SelfParent => "자기 자신을 상위로 기록 — 무시".into(),
        ParentNote::Cycle(p) => format!("{p}와 순환 관계 — 연결 생략"),
    }
}

pub fn duplicates(ids: &[String]) -> String {
    format!("중복된 터미널 ID {} — 첫 기록만 표시", ids.join(", "))
}

pub fn cannot_open(id: &str) -> String {
    format!("{id} 터미널은 podd 목록에 없어 열 수 없습니다")
}

pub fn task_text(title: Option<&str>, group: Option<&str>) -> String {
    let task = title
        .map(|t| format!("작업 {t}"))
        .unwrap_or_else(|| NO_TASK.into());
    match group {
        Some(g) => format!("{task} · 그룹 {g}"),
        None => task,
    }
}
