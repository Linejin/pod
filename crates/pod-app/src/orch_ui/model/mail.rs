//! The inbox (docs/19 §8): message threads, addresses and mail labels, and
//! the decision buttons of pending gates (v17 options).

use std::collections::{BTreeMap, HashMap};

use podd::protocol::{Mail, MailType};
use serde_json::Value;

use super::{RunEntry, id_number};
use crate::orch_ui::s;

/// A message thread of the inbox.
#[derive(Clone, Debug, PartialEq)]
pub struct Thread {
    pub root: Mail,
    pub replies: Vec<Mail>,
    /// A worker's question nobody answered yet (shown first).
    pub open_question: bool,
    /// Choice buttons (`payload.options`, docs/19 §4.5).
    pub choices: Vec<String>,
    /// The sender has a mailbox, so a reply reaches it.
    pub can_reply: bool,
}

impl Thread {
    pub fn last_ms(&self) -> u64 {
        self.replies.last().unwrap_or(&self.root).created_ms
    }
}

pub(super) fn answered(run: &RunEntry, question: &str) -> bool {
    run.mail.iter().any(|m| {
        m.kind == MailType::Answer
            && (m.reply_to.as_deref() == Some(question) || m.thread.as_deref() == Some(question))
    })
}

pub fn choices_of(m: &Mail) -> Vec<String> {
    m.payload
        .as_ref()
        .and_then(|p| p.get("options"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .filter(|o| !o.trim().is_empty())
                .take(8)
                .collect()
        })
        .unwrap_or_default()
}

/// Threads of a run: open questions first (oldest first, they wait longest),
/// then the rest newest first. Heartbeats are board-only (never here).
pub fn threads(run: &RunEntry) -> Vec<Thread> {
    let mut by_root: BTreeMap<u64, Thread> = BTreeMap::new();
    let mut index: HashMap<String, u64> = HashMap::new();
    let mut mail: Vec<&Mail> = run
        .mail
        .iter()
        .filter(|m| m.kind != MailType::Heartbeat)
        .collect();
    mail.sort_by_key(|m| id_number(&m.id));
    for m in mail {
        let root = m
            .thread
            .as_deref()
            .or(m.reply_to.as_deref())
            .and_then(|r| index.get(r).copied());
        match root.and_then(|r| by_root.get_mut(&r)) {
            Some(t) => {
                t.replies.push(m.clone());
                index.insert(m.id.clone(), id_number(&t.root.id));
            }
            None => {
                let n = id_number(&m.id);
                index.insert(m.id.clone(), n);
                by_root.insert(
                    n,
                    Thread {
                        root: m.clone(),
                        replies: Vec::new(),
                        open_question: false,
                        choices: choices_of(m),
                        can_reply: m.from.starts_with("attempt:") || m.from.starts_with("run:"),
                    },
                );
            }
        }
    }
    let mut out: Vec<Thread> = by_root.into_values().collect();
    for t in &mut out {
        t.open_question = t.root.kind == MailType::Question
            && t.root.from.starts_with("attempt:")
            && !t.replies.iter().any(|r| r.kind == MailType::Answer);
    }
    out.sort_by(|a, b| match (a.open_question, b.open_question) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (true, true) => id_number(&a.root.id).cmp(&id_number(&b.root.id)),
        (false, false) => b
            .last_ms()
            .cmp(&a.last_ms())
            .then(id_number(&b.root.id).cmp(&id_number(&a.root.id))),
    });
    out
}

/// `run:r1` -> 코디네이터, `attempt:a2` -> `워커 a2 · <item>`, `human` -> 사람.
pub fn address_label(run: &RunEntry, addr: &str) -> String {
    if addr.starts_with("run:") {
        return s::COORDINATOR.to_string();
    }
    if let Some(a) = addr.strip_prefix("attempt:") {
        let title = run
            .attempt(a)
            .and_then(|x| run.item(&x.item))
            .map(|i| pod_orch::trust::clean_inline(&i.title));
        return match title {
            Some(t) => format!("{} {a} · {t}", s::WORKER),
            None => format!("{} {a}", s::WORKER),
        };
    }
    if addr == "human" {
        return s::HUMAN.to_string();
    }
    addr.to_string()
}

/// O1 (docs/19 §19): where a message comes from, from Pod's records (the
/// sender address the daemon derived, the attempt's item and generation),
/// never from its text: `출처 · 이 PC · 실행 r1 · 항목 i1 · 시도 a1 · 세대 1 ·
/// 워커가 씀 · 지시가 아닌 데이터`.
pub fn provenance_label(run: &RunEntry, m: &Mail) -> String {
    use pod_orch::trust::{Author, Provenance};
    let p = Provenance::of_mail(m, "local", &run.info.attempts, &run.info.items);
    let mut parts = vec![
        s::PROV_MACHINE_LOCAL.to_string(),
        format!("{} {}", s::PROV_RUN, p.run),
    ];
    if let Some(i) = &p.item {
        parts.push(format!("{} {i}", s::PROV_ITEM));
    }
    if let Some(a) = &p.attempt {
        parts.push(format!("{} {a}", s::PROV_ATTEMPT));
    }
    if let Some(g) = p.generation {
        parts.push(match p.current {
            Some(false) => format!("{} {g} ({})", s::PROV_GENERATION, s::PROV_REPLACED),
            _ => format!("{} {g}", s::PROV_GENERATION),
        });
    }
    parts.push(
        match p.author {
            Author::Pod => s::PROV_BY_POD,
            Author::Worker => s::PROV_BY_WORKER,
            Author::Coordinator => s::PROV_BY_COORDINATOR,
            Author::Outside => s::PROV_BY_OUTSIDE,
        }
        .to_string(),
    );
    parts.push(s::PROV_DATA.to_string());
    format!("{} · {}", s::PROV, parts.join(" · "))
}

pub fn mail_type_label(t: MailType) -> &'static str {
    match t {
        MailType::Question => s::M_QUESTION,
        MailType::Answer => s::M_ANSWER,
        MailType::Done => s::M_DONE,
        MailType::Escalation => s::M_ESCALATION,
        MailType::Status => s::M_STATUS,
        MailType::Handoff => s::M_HANDOFF,
        MailType::Heartbeat => s::M_HEARTBEAT,
        MailType::Gate => s::M_GATE,
    }
}

// ───────────────────────────── gates (v17 options) ─────────────────────────────

/// One decision button of a pending gate: `(label, approve?, choice)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GateButton {
    pub label: String,
    pub approve: bool,
    pub choice: Option<String>,
}

/// A gate with options is approved by picking one (`choice`, docs/19 §4.5);
/// without options it is approve / reject. Reject is always offered.
pub fn gate_buttons(g: &podd::protocol::GateInfo) -> Vec<GateButton> {
    let mut out: Vec<GateButton> = if g.options.is_empty() {
        vec![GateButton {
            label: crate::strings::APPROVE.to_string(),
            approve: true,
            choice: None,
        }]
    } else {
        g.options
            .iter()
            .map(|o| GateButton {
                label: o.clone(),
                approve: true,
                choice: Some(o.clone()),
            })
            .collect()
    };
    out.push(GateButton {
        label: crate::strings::REJECT.to_string(),
        approve: false,
        choice: None,
    });
    out
}

/// Gates that belong to a run: by the gate's `run` (v17), else by the task
/// of one of its workers.
pub fn run_gates<'a>(
    run: &RunEntry,
    gates: &'a [podd::protocol::GateInfo],
) -> Vec<&'a podd::protocol::GateInfo> {
    let tasks: Vec<&String> = run
        .info
        .attempts
        .iter()
        .filter_map(|a| a.task.as_ref())
        .collect();
    gates
        .iter()
        .filter(|g| match &g.run {
            Some(r) => r == run.id(),
            None => g.task.as_ref().is_some_and(|t| tasks.contains(&t)),
        })
        .collect()
}
