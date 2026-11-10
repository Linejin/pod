//! `--ui-fixture` answers for the orchestration screens (visual baselines,
//! docs/10 §12): one active run with five items, four workers (one asking a
//! question with choices, one queued for resources, one finished) and its
//! mail, plus a finished run. Times are relative to a frozen clock so the
//! captures never change from one run to the next. Never contacts podd.

use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::source::SourceError;

/// Fixture "now" (Unix ms). 0 = not a fixture: the real clock.
static FROZEN: AtomicU64 = AtomicU64::new(0);
/// 2026-11-10 09:30:00 UTC.
const NOW: u64 = 1_794_303_000_000;

/// The fixture scene is an orchestration one (only those get a gate: the
/// other baselines stay as they are).
static ORCH_SCENE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The board with unconfirmed wake-ups (orch-02): the coordinator and a2.
pub(crate) const WAKE_SCENE: &str = "orch-wake";
static WAKE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn set_scene(scene: Option<&str>) {
    ORCH_SCENE.store(
        scene.is_some_and(|s| s.starts_with("orch-")),
        Ordering::Relaxed,
    );
    WAKE.store(scene == Some(WAKE_SCENE), Ordering::Relaxed);
}

fn wake(v: &mut Value, min: u64, on: bool) {
    if on {
        v["wake_unconfirmed_ms"] = json!(ago(min));
    }
}

/// Pending gates of the fixture: one with choices for run r1 (v17).
pub(crate) fn gates() -> Option<Vec<podd::protocol::GateInfo>> {
    if !ORCH_SCENE.load(Ordering::Relaxed) {
        return None;
    }
    serde_json::from_value(json!([{"id": "g3", "title": "배포 방식", "message": "스테이징에 먼저 올릴까요?", "state": "pending", "created_ms": ago(6),
        "task": "sample/task-002", "run": "r1", "item": "i2", "options": ["스테이징 먼저", "바로 운영"], "requested_by": "attempt:a2"}]))
    .ok()
}

/// The fixture's option gate regardless of the scene (tests).
#[cfg(test)]
pub(crate) fn gates_for_test() -> Vec<podd::protocol::GateInfo> {
    let was = ORCH_SCENE.swap(true, Ordering::Relaxed);
    let g = gates().unwrap_or_default();
    ORCH_SCENE.store(was, Ordering::Relaxed);
    g
}

/// The clock the orchestration screens use.
pub(crate) fn now_ms() -> u64 {
    match FROZEN.load(Ordering::Relaxed) {
        0 => podd::now_ms(),
        n => n,
    }
}

const MIN: u64 = 60_000;

fn ago(m: u64) -> u64 {
    NOW - m * MIN
}

fn item(id: &str, title: &str, state: &str, deps: &[&str]) -> Value {
    json!({"id": id, "run": "r1", "title": title, "spec": title, "deps": deps, "state": state, "generation": 1,
        "created_ms": ago(60), "updated_ms": ago(5), "circuit_open": state == "failed", "start_failures": if state == "failed" { 3 } else { 0 }})
}

fn attempt(
    id: &str,
    item: &str,
    state: &str,
    worker: &str,
    agent: &str,
    task: Option<&str>,
    terminal: Option<&str>,
) -> Value {
    let mut a = json!({"id": id, "run": "r1", "item": item, "generation": 1, "state": state, "worker": worker, "agent": agent,
        "place": "new", "depth": 1, "created_ms": ago(50)});
    if let Some(t) = task {
        a["task"] = json!(t);
    }
    if let Some(t) = terminal {
        a["terminal"] = json!(t);
        a["ready_ms"] = json!(ago(48));
    }
    a
}

fn run1(stuck: bool) -> Value {
    let mut a5 = attempt("a5", "i4", "starting", "active", "claude", None, None);
    a5["queue"] = json!({"since_ms": ago(3), "kind": "workers", "reason": "동시 워커 3/3 — 앞의 워커가 끝나면 시작", "position": 1});
    let mut a1 = attempt(
        "a1",
        "i1",
        "succeeded",
        "finished",
        "codex",
        Some("sample/task-001"),
        Some("fixture-3"),
    );
    a1["ended_ms"] = json!(ago(12));
    a1["release_at_ms"] = json!(NOW + 4 * MIN);
    a1["body"] = json!("로그인 실패 원인 수정 완료");
    let mut a6 = attempt(
        "a6",
        "i5",
        "start_failed",
        "released",
        "codex",
        Some("sample/task-004"),
        None,
    );
    a6["error"] = json!("agent program not found: codex-nightly");
    a6["ended_ms"] = json!(ago(30));
    let mut r = json!({
        "run": {"id": "r1", "objective": "로그인 버그 수정과 테스트 보강", "coordinator_terminal": "fixture-0", "depth": 0, "created_ms": ago(60)},
        "items": [
            item("i1", "로그인 실패 원인 찾기", "completed", &[]),
            item("i2", "세션 만료 처리 고치기", "assigned", &["i1"]),
            item("i3", "회귀 테스트 추가", "assigned", &["i1"]),
            item("i4", "문서·변경 기록 갱신", "assigned", &["i2", "i3"]),
            item("i5", "야간 빌드 확인", "failed", &[]),
        ],
        "attempts": [
            a1,
            attempt("a2", "i2", "ready", "active", "claude", Some("sample/task-002"), Some("fixture-6")),
            attempt("a3", "i3", "ready", "active", "codex", Some("sample/task-003"), Some("fixture-9")),
            a5,
            a6,
        ],
        "unread": 2,
        "mail": 6
    });
    wake(&mut r, 3, stuck);
    r
}

fn run2() -> Value {
    let mut a = attempt(
        "a4",
        "i6",
        "succeeded",
        "released",
        "codex",
        Some("sample/task-005"),
        None,
    );
    a["run"] = json!("r2");
    a["ended_ms"] = json!(ago(240));
    let mut i = item("i6", "의존성 버전 올리기", "completed", &[]);
    i["run"] = json!("r2");
    json!({
        "run": {"id": "r2", "objective": "의존성 정리", "coordinator_terminal": "fixture-12", "depth": 0, "created_ms": ago(300)},
        "items": [i], "attempts": [a], "unread": 0, "mail": 1
    })
}

fn status(state: &str) -> Value {
    json!({"state": state, "source": "osc", "confidence": 1.0, "since": ago(2), "turn": 3, "badge": state})
}

fn worker(
    run: &Value,
    id: &str,
    title: &str,
    st: Option<&str>,
    last_min: u64,
    waiting: bool,
    next: &str,
) -> Value {
    let a = run["attempts"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"] == id))
        .cloned()
        .unwrap_or(Value::Null);
    let mut w = json!({"attempt": a, "item_title": title, "last_output_ms": if last_min == 0 { 0 } else { ago(last_min) }, "waiting_answer": waiting, "unread": 0, "next": next});
    if let Some(s) = st {
        w["status"] = status(s);
    }
    w
}

fn mail(id: &str, from: &str, to: &str, kind: &str, subject: &str, body: &str, min: u64) -> Value {
    json!({"id": id, "run": "r1", "from": from, "to": to, "recipients": [to], "type": kind, "subject": subject, "body": body,
        "priority": "normal", "created_ms": ago(min)})
}

fn mails() -> Value {
    let mut q = mail(
        "m4",
        "attempt:a2",
        "run:r1",
        "question",
        "세션 만료를 401로 둘까요, 403으로 바꿀까요?",
        "기존 클라이언트는 401을 기대합니다. 403이 의미상 맞지만 호환성이 깨집니다.",
        4,
    );
    q["payload"] = json!({"options": ["401 유지", "403으로 변경"]});
    let mut a = mail(
        "m2",
        "run:r1",
        "attempt:a1",
        "answer",
        "re: 재현 로그 위치?",
        "logs/auth.log 를 보세요.",
        40,
    );
    a["reply_to"] = json!("m1");
    a["thread"] = json!("m1");
    a["read_ms"] = json!(ago(39));
    let mut m1 = mail(
        "m1",
        "attempt:a1",
        "run:r1",
        "question",
        "재현 로그 위치?",
        "",
        42,
    );
    m1["read_ms"] = json!(ago(41));
    let mut done = mail(
        "m3",
        "attempt:a1",
        "run:r1",
        "done",
        "로그인 실패 원인 수정 완료",
        "무엇을: 토큰 갱신 경쟁 수정\n찾은 것: 만료 직전 요청이 두 번 갱신\n남은 것: 없음",
        12,
    );
    done["read_ms"] = json!(ago(11));
    json!({"messages": [
        m1, a, done, q,
        mail("m5", "attempt:a3", "run:r1", "escalation", "stalled: 20분 동안 출력 없음", "", 2),
        mail("m6", "attempt:a3", "run:r1", "heartbeat", "테스트 12/30", "", 1),
    ]})
}

/// The fixture's answer to `method`, or `None` (not ours).
pub(crate) fn answer(method: &str, params: &Value) -> Option<Result<Value, SourceError>> {
    answer_with(method, params, WAKE.load(Ordering::Relaxed))
}

/// [`answer`], with or without the unconfirmed wake-ups of the `orch-wake`
/// scene (`stuck`).
pub(crate) fn answer_with(
    method: &str,
    params: &Value,
    stuck: bool,
) -> Option<Result<Value, SourceError>> {
    // The settings screen's Codex worker allowance (docs/19 §18.1).
    if method == "repo.config" && ORCH_SCENE.load(Ordering::Relaxed) {
        let repo = params
            .get("repo")
            .and_then(Value::as_str)
            .unwrap_or("sample");
        return Some(Ok(
            json!({"repo": repo, "path": format!("C:/fixtures/{repo}/.pod/config.toml"), "exists": false, "trusted": false, "setup_mode": "parallel", "codex_allowance": true}),
        ));
    }
    if !method.starts_with("orch.") && method != "task.cleanup_candidates" {
        return None;
    }
    FROZEN.store(NOW, Ordering::Relaxed);
    let run = params.get("run").and_then(Value::as_str);
    let r1 = run1(stuck);
    let mut a2 = worker(
        &r1,
        "a2",
        "세션 만료 처리 고치기",
        Some("idle"),
        4,
        true,
        "질문에 대한 답을 기다림",
    );
    wake(&mut a2, 2, stuck);
    Some(Ok(match method {
        "orch.run.list" => json!({"runs": [r1, run2()]}),
        "orch.run.show" => {
            if run == Some("r2") {
                run2()
            } else {
                r1
            }
        }
        "orch.worker.list" if run == Some("r2") => json!({"workers": []}),
        "orch.worker.list" => json!({"workers": [
            worker(&r1, "a1", "로그인 실패 원인 찾기", Some("done"), 12, false, "끝남: 4분 뒤 터미널 정리"),
            a2,
            worker(&r1, "a3", "회귀 테스트 추가", Some("working"), 1, false, "작업 중"),
            worker(&r1, "a5", "문서·변경 기록 갱신", None, 0, false, "자원 대기: 동시 워커 3/3"),
        ]}),
        "orch.mail.list" if run == Some("r2") => json!({"messages": []}),
        "orch.mail.list" => mails(),
        "orch.limits.get" => json!({
        "limits": {"max_workers": 3, "run_workers": 8, "cpu_max_pct": 70.0, "min_free_mem": 4294967296u64, "min_free_disk": 21474836480u64, "cpu_max_wait_ms": 1800000},
        "path": "C:/fixtures/state/orch/limits.json",
        "entries": [
            {"key": "max_workers", "value": "3", "source": "default", "env": "POD_MAX_WORKERS", "help": "동시에 일하는 워커 수(전체), 1–64"},
            {"key": "run_workers", "value": "8", "source": "default", "env": "POD_ORCH_RUN_WORKERS", "help": "Run 하나의 열린 워커 수, 1–64"},
            {"key": "cpu_max_pct", "value": "70%", "source": "settings", "env": "POD_ORCH_CPU_LIMIT", "saved": "70%", "help": "최근 1분 CPU가 이 %를 넘으면 새 워커 대기, 1–100 또는 off"},
            {"key": "min_free_mem", "value": "4G", "source": "default", "env": "POD_ORCH_MIN_FREE_MEM", "help": "사용 가능 메모리가 이보다 적으면 대기(예: 4G), 또는 off"},
            {"key": "min_free_disk", "value": "20G", "source": "env", "env": "POD_ORCH_MIN_FREE_DISK", "help": "worktree 드라이브 여유가 이보다 적으면 대기(예: 20G), 또는 off"},
            {"key": "cpu_max_wait", "value": "30m", "source": "default", "env": "POD_ORCH_CPU_MAX_WAIT", "help": "CPU 때문에 기다리는 최대 시간(예: 30m), 또는 off"}
        ]}),
        "orch.config.get" => {
            json!({"release_after_min": 10, "stall_after_min": 20, "max_depth": 3, "default_place": "new", "locked": ["max_depth"]})
        }
        "task.cleanup_candidates" => json!({"candidates": [
            {"task": "sample/task-001", "tier": "ready", "reasons": ["깨끗함", "병합됨"], "lifecycle": "active", "last_activity_ms": ago(12)},
            {"task": "sample/task-004", "tier": "review", "reasons": ["커밋 안 한 변경 2개"], "lifecycle": "active", "last_activity_ms": ago(30)},
            {"task": "sample/task-005", "tier": "ready", "reasons": ["보관됨"], "lifecycle": "archived", "last_activity_ms": ago(240)},
        ]}),
        // Read-only fixture: changes are refused (nothing to change).
        _ => {
            return Some(Err(SourceError {
                code: None,
                message: "UI fixture (read only)".into(),
            }));
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    // GPUI's glob exports its own `test` attribute; these are plain Rust tests.
    use core::prelude::v1::test;

    #[test]
    fn fixture_answers_parse_as_the_wire_types() {
        let ok = |m: &str, p: Value| answer(m, &p).unwrap().unwrap();
        let list: podd::protocol::OrchRunListResult =
            serde_json::from_value(ok("orch.run.list", json!({}))).unwrap();
        assert_eq!(list.runs.len(), 2);
        let _: podd::protocol::OrchWorkerListResult =
            serde_json::from_value(ok("orch.worker.list", json!({"run": "r1"}))).unwrap();
        let mail: podd::protocol::OrchMailListResult =
            serde_json::from_value(ok("orch.mail.list", json!({"run": "r1"}))).unwrap();
        assert_eq!(mail.messages.len(), 6);
        let _: podd::protocol::OrchSettings =
            serde_json::from_value(ok("orch.config.get", json!({}))).unwrap();
        let _: podd::protocol::OrchLimitsResult =
            serde_json::from_value(ok("orch.limits.get", json!({}))).unwrap();
        let _: podd::protocol::CleanupResult =
            serde_json::from_value(ok("task.cleanup_candidates", json!({}))).unwrap();
        assert!(answer("orch.worker.stop", &json!({})).unwrap().is_err());
        assert!(answer("task.list", &json!({})).is_none());
        let was = ORCH_SCENE.swap(true, Ordering::Relaxed);
        let rc: podd::protocol::RepoConfigResult =
            serde_json::from_value(ok("repo.config", json!({"repo": "sample"}))).unwrap();
        ORCH_SCENE.store(was, Ordering::Relaxed);
        assert_eq!(rc.codex_allowance, Some(true));
        assert_eq!(now_ms(), NOW);
    }

    /// The `orch-wake` scene (orch-02): the coordinator and a2 have
    /// unconfirmed wake-ups; the other scenes have none.
    #[test]
    fn wake_scene_has_unconfirmed_wakeups() {
        let get = |m: &str, stuck: bool| {
            answer_with(m, &json!({"run": "r1"}), stuck)
                .unwrap()
                .unwrap()
        };
        let info: podd::protocol::RunInfo =
            serde_json::from_value(get("orch.run.show", true)).unwrap();
        let ws: podd::protocol::OrchWorkerListResult =
            serde_json::from_value(get("orch.worker.list", true)).unwrap();
        let mut run = super::super::model::RunEntry::new(info, Default::default());
        run.workers = ws.workers;
        let st = super::super::model::stuck_wakes(&run);
        assert_eq!(
            st.iter()
                .map(|w| (w.attempt.clone(), w.terminal.clone(), w.since_ms))
                .collect::<Vec<_>>(),
            [
                (None, Some("fixture-0".to_string()), ago(3)),
                (
                    Some("a2".to_string()),
                    Some("fixture-6".to_string()),
                    ago(2)
                ),
            ]
        );
        let plain: podd::protocol::RunInfo =
            serde_json::from_value(get("orch.run.show", false)).unwrap();
        assert_eq!(plain.wake_unconfirmed_ms, None);
        assert!(
            !get("orch.worker.list", false)
                .to_string()
                .contains("wake_unconfirmed_ms")
        );
    }
}
