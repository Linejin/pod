//! Korean rendering of the daemon's structured texts (docs/19 §16.3). The
//! daemon keeps English in `subject` / `body` / `next` for the CLI; the app
//! shows these instead. Unknown kinds (a newer daemon) fall back to English.

use podd::protocol::msg_text as tx;
use podd::protocol::{Mail, MsgText, WorkerInfo};

fn lost_cause(cause: &str) -> &'static str {
    match cause {
        "terminal_gone" => "터미널이 사라졌습니다",
        "agent_exited" => "에이전트가 완료 보고 없이 끝났습니다",
        "host_restart" => "다시 시작한 뒤 터미널이 남아 있지 않습니다",
        "host_restart_starting" => "시작하는 동안 데몬이 멈췄습니다",
        _ => "터미널이 없습니다",
    }
}

fn outcome(o: &str) -> &'static str {
    match o {
        "succeeded" => "성공",
        "failed" => "실패",
        _ => "보고",
    }
}

fn settled(t: &MsgText) -> String {
    match t.p("outcome") {
        "approved" => "승인됨".into(),
        "rejected" => "거절됨".into(),
        "expired" => "만료됨".into(),
        "decided" => format!("선택: {}", t.p("choice")),
        o => o.to_string(),
    }
}

/// Korean for a known kind; `None` for one this app does not know.
pub fn render(t: &MsgText) -> Option<String> {
    let p = |k: &str| t.p(k);
    Some(match t.kind.as_str() {
        tx::TURN_ENDED => format!(
            "워커 {}: `pod orch done` 없이 턴을 끝냈습니다",
            p("attempt")
        ),
        tx::STALLED => {
            let secs: u64 = p("secs").parse().unwrap_or(0);
            let span = if secs >= 120 {
                format!("{}분", secs / 60)
            } else {
                format!("{secs}초")
            };
            format!(
                "워커 {}: 작업 중인데 {span} 동안 출력이 없습니다",
                p("attempt")
            )
        }
        tx::LOST => format!("워커 {} 잃음: {}", p("attempt"), lost_cause(p("cause"))),
        tx::LOW_DISK => format!("워커 {}: 디스크 공간을 기다립니다", p("attempt")),
        tx::START_FAILED => format!("대기하던 워커 {}를 시작하지 못했습니다", p("attempt")),
        tx::DONE => format!("완료 보고({}): {}", outcome(p("outcome")), p("title")),
        tx::DONE_LATE => format!(
            "늦은 완료 보고({}) — 이전 시도 {}, 반영 안 됨",
            outcome(p("outcome")),
            p("attempt")
        ),
        tx::GATE_REQUESTED => format!("승인 요청 {}: {}", p("gate"), p("title")),
        tx::GATE_SETTLED => format!("승인 요청 {}: {}", p("gate"), settled(t)),
        tx::REVIEW_DELIVERED => format!(
            "워커 {}에게 리뷰 코멘트를 전달했습니다 ({})",
            p("attempt"),
            p("task")
        ),
        tx::GROUP_CHECKED => {
            let verdict = match p("all_passed") {
                "true" => "모두 통과",
                "false" => "일부 실패",
                _ => match p("state") {
                    "cancelled" => "취소됨",
                    "interrupted" => "중단됨",
                    _ => "결과 없음",
                },
            };
            format!("그룹 {} 검사 끝: {verdict}", p("group"))
        }
        tx::START_FAILED_RETRY => format!(
            "워커가 시작되지 않았습니다({}). 원인을 고친 뒤 다시 시작하세요.",
            p("error")
        ),
        tx::START_FAILED_TRIPPED => format!(
            "항목 {item}이 {}번 연속 시작에 실패해 실패로 바뀌었습니다. 원인을 고친 뒤 `pod orch item reopen {item}`.",
            p("strikes"),
            item = p("item")
        ),
        tx::GATE_REQUESTED_BODY => {
            let mut b = p("message").to_string();
            if !p("options").is_empty() {
                b.push_str(&format!("\n\n선택지: {}", p("options")));
            }
            if p("decider") == "person" {
                // O1: the agent trust policy keeps this gate for a person.
                b.push_str(&format!(
                    "\n\n{} 승인 요청은 사람이 정합니다(승인 요청 패널·휴대폰). 에이전트 신뢰 정책상 코디네이터는 정할 수 없습니다.",
                    gate_kind(p("kind"))
                ));
            } else {
                b.push_str("\n\n여기서 선택하거나 승인 요청 패널에서 정할 수 있습니다.");
            }
            b
        }
        tx::GATE_SETTLED_BODY => {
            let mut b = format!("승인 요청 {} ({}) {}", p("gate"), p("title"), settled(t));
            if !p("note").is_empty() {
                b.push_str(&format!("\n메모: {}", p("note")));
            }
            b
        }
        tx::NEXT_RELEASED => "터미널 정리됨 · 워크트리는 남음".into(),
        tx::NEXT_WAITING_ANSWER => "답을 기다리는 중".into(),
        tx::NEXT_QUEUED => format!("자원 대기 ({}번째): {}", p("position"), p("reason")),
        tx::NEXT_STARTING => "시작하는 중".into(),
        tx::NEXT_BLOCKED => "사람의 확인을 기다림".into(),
        tx::NEXT_WORKING => "작업 중".into(),
        tx::NEXT_IDLE => "쉬는 중 · 우편이나 완료 보고를 기다림".into(),
        tx::NEXT_KEPT => "끝남 · 사람이 유지(자동 정리 안 함)".into(),
        tx::NEXT_RELEASE_IN => {
            let secs: u64 = p("secs").parse().unwrap_or(0);
            format!("끝남 · {}분 뒤 터미널 정리", secs.div_ceil(60))
        }
        tx::NEXT_FINISHED => "끝남".into(),
        tx::WAIT_FINISHED => {
            // `items`: "i1 completed, i3 failed".
            let items: Vec<String> = p("items")
                .split(", ")
                .map(|e| e.replace(" completed", " 완료").replace(" failed", " 실패"))
                .collect();
            format!("기다리던 항목이 모두 끝났습니다: {}", items.join(", "))
        }
        tx::NEXT_WAITING_ITEMS => format!("항목 {}을 기다리는 중", p("items")),
        _ => return None,
    })
}

/// O1: a gate kind in Korean.
fn gate_kind(k: &str) -> &str {
    match k {
        "merge" => "병합",
        "permission" => "권한",
        "sensitive" => "민감한",
        "approval" => "단순 승인",
        "choice" => "선택",
        other => other,
    }
}

/// A message's subject as shown in the app: one cleaned line (O1: agent
/// text is data; hidden characters and chat role tokens are shown).
pub fn subject(m: &Mail) -> String {
    let s = m
        .text
        .as_ref()
        .and_then(render)
        .unwrap_or_else(|| m.subject.clone());
    pod_orch::trust::clean_inline(&s)
}

/// A message's body as shown in the app, cleaned the same way (O1).
pub fn body(m: &Mail) -> String {
    let b = m
        .body_text
        .as_ref()
        .and_then(render)
        .unwrap_or_else(|| m.body.clone());
    pod_orch::trust::clean_text(&b)
}

/// A worker's `next` as shown in the app.
pub fn next(w: &WorkerInfo) -> String {
    w.next_text
        .as_ref()
        .and_then(render)
        .unwrap_or_else(|| w.next.clone())
}

/// An `orch.mail` event's subject as shown in the app.
pub fn event_subject(text: Option<&MsgText>, subject: &str) -> String {
    text.and_then(render).unwrap_or_else(|| subject.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_daemon_kind_renders_in_korean() {
        for k in tx::KINDS {
            let t = tx::sample(k);
            let ko = render(&t).unwrap_or_else(|| panic!("{k} has no Korean rendering"));
            assert!(
                ko.chars().any(|c| ('\u{AC00}'..='\u{D7A3}').contains(&c)),
                "{k}: {ko}"
            );
            assert!(!ko.contains("{"), "{k}: {ko}");
        }
    }

    #[test]
    fn the_old_english_strings_now_render_korean() {
        // The two texts S7 left in English in the app.
        let t = MsgText::new(tx::TURN_ENDED).with("attempt", "a2");
        assert_eq!(t.en(), "worker a2 ended its turn without `pod orch done`");
        assert_eq!(
            render(&t).unwrap(),
            "워커 a2: `pod orch done` 없이 턴을 끝냈습니다"
        );
        let n = MsgText::new(tx::NEXT_WAITING_ANSWER);
        assert_eq!(n.en(), "waiting for an answer");
        assert_eq!(render(&n).unwrap(), "답을 기다리는 중");
        assert_eq!(
            render(&MsgText::new(tx::NEXT_RELEASE_IN).with("secs", 61)).unwrap(),
            "끝남 · 2분 뒤 터미널 정리"
        );
        assert_eq!(
            render(
                &MsgText::new(tx::STALLED)
                    .with("attempt", "a1")
                    .with("secs", 1200)
            )
            .unwrap(),
            "워커 a1: 작업 중인데 20분 동안 출력이 없습니다"
        );
    }

    #[test]
    fn waiting_for_items_renders_korean() {
        assert_eq!(
            render(&tx::sample(tx::WAIT_FINISHED)).unwrap(),
            "기다리던 항목이 모두 끝났습니다: i1 완료, i3 실패"
        );
        assert_eq!(
            render(&tx::sample(tx::NEXT_WAITING_ITEMS)).unwrap(),
            "항목 i1, i3을 기다리는 중"
        );
    }

    #[test]
    fn unknown_kinds_and_plain_mail_keep_the_english() {
        let m: Mail = serde_json::from_value(serde_json::json!({
            "id": "m1", "run": "r1", "from": "attempt:a1", "to": "run:r1", "recipients": ["run:r1"],
            "type": "question", "subject": "which db?", "body": "sqlite?", "created_ms": 1
        }))
        .unwrap();
        assert_eq!(
            (subject(&m), body(&m)),
            ("which db?".to_string(), "sqlite?".to_string())
        );
        let mut m2 = m.clone();
        m2.text = Some(MsgText::new("future.kind"));
        assert_eq!(subject(&m2), "which db?");
        m2.text = Some(MsgText::new(tx::LOW_DISK).with("attempt", "a1"));
        assert_eq!(subject(&m2), "워커 a1: 디스크 공간을 기다립니다");
        assert_eq!(event_subject(None, "x"), "x");
    }
}
