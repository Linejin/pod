//! Headless model of the orchestration screens (docs/19 §8, S7): what the
//! Run board, worker cards, inbox, cleanup screen and settings show, and
//! which actions each row offers. Pure data in, view rows out — no GPUI —
//! so every rule here has a plain unit test.
//!
//! Data comes from `orch.run.show/list`, `orch.worker.list`, `orch.mail.list`
//! and `task.cleanup_candidates`, re-read only when the status stream says
//! something changed (`orch.changed`, `orch.mail`; see `orch_ui/controller.rs`).
//!
//! The run, its items and the settings are here; worker cards and wake-ups in
//! [`workers`], inbox threads and gate buttons in [`mail`], the cleanup
//! screen's rows in [`cleanup`]. Their items keep their `model::` paths.

use std::collections::{HashMap, HashSet};

use podd::protocol::{
    Attempt, AttemptState, Item, ItemState, Mail, MailType, OrchMailEvent, OrchSettings, RunInfo,
    WorkerInfo,
};
use serde_json::Value;

use super::s;

mod cleanup;
mod mail;
#[cfg(test)]
mod tests;
mod workers;

pub use cleanup::{CleanupRow, cleanup_rows, default_selection, tier_label};
pub use mail::{
    Thread, address_label, gate_buttons, mail_type_label, provenance_label, run_gates, threads,
};
pub use workers::{
    StuckWake, WorkerCard, attempt_state_label, cards, stuck_wakes, worker_state_label,
};

/// Board columns, in order (docs/19 §8: 대기·준비·진행·차단·완료·실패).
pub const COLUMNS: [ItemState; 6] = [
    ItemState::Pending,
    ItemState::Ready,
    ItemState::Assigned,
    ItemState::Blocked,
    ItemState::Completed,
    ItemState::Failed,
];

/// `r12` / `a3` / `m40` -> 12 / 3 / 40 (ids sort numerically).
pub fn id_number(id: &str) -> u64 {
    id.trim_start_matches(|c: char| !c.is_ascii_digit())
        .parse()
        .unwrap_or(0)
}

/// A worker waiting for resources (docs/19 §5.1, S6). Read from the raw
/// attempt JSON so the board shows it whether or not the daemon has S6.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueVm {
    pub kind: String,
    pub reason: String,
    pub position: usize,
}

impl QueueVm {
    pub fn from_attempt_json(a: &Value) -> Option<QueueVm> {
        let q = a.get("queue")?;
        if q.is_null() {
            return None;
        }
        Some(QueueVm {
            kind: q
                .get("kind")
                .and_then(Value::as_str)
                .unwrap_or("queued")
                .to_string(),
            reason: q
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            position: q.get("position").and_then(Value::as_u64).unwrap_or(0) as usize,
        })
    }
}

/// Queue states of a raw `RunInfo` JSON, by attempt id.
pub fn queues_of(raw: &Value) -> HashMap<String, QueueVm> {
    raw.get("attempts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some((
                        x.get("id")?.as_str()?.to_string(),
                        QueueVm::from_attempt_json(x)?,
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What the app knows about one run.
#[derive(Clone, Debug, PartialEq)]
pub struct RunEntry {
    pub info: RunInfo,
    pub queues: HashMap<String, QueueVm>,
    /// `orch.worker.list` (status, last output, waiting answer, next).
    pub workers: Vec<WorkerInfo>,
    /// `orch.mail.list` (whole thread, nothing acked).
    pub mail: Vec<Mail>,
    pub mail_loaded: bool,
}

impl RunEntry {
    pub fn new(info: RunInfo, queues: HashMap<String, QueueVm>) -> Self {
        RunEntry {
            info,
            queues,
            workers: Vec::new(),
            mail: Vec::new(),
            mail_loaded: false,
        }
    }

    pub fn id(&self) -> &str {
        &self.info.run.id
    }

    pub fn item(&self, id: &str) -> Option<&Item> {
        self.info.items.iter().find(|i| i.id == id)
    }

    pub fn attempt(&self, id: &str) -> Option<&Attempt> {
        self.info.attempts.iter().find(|a| a.id == id)
    }

    pub fn worker(&self, attempt: &str) -> Option<&WorkerInfo> {
        self.workers.iter().find(|w| w.attempt.id == attempt)
    }

    /// Nothing runs and every item ended (an empty run is not finished).
    pub fn is_finished(&self) -> bool {
        !self.info.items.is_empty()
            && self.info.attempts.iter().all(|a| !a.state.is_open())
            && self
                .info
                .items
                .iter()
                .all(|i| matches!(i.state, ItemState::Completed | ItemState::Failed))
    }

    /// Questions from workers that have no answer yet.
    pub fn open_questions(&self) -> usize {
        threads(self).iter().filter(|t| t.open_question).count()
    }
}

// ───────────────────────────── items (DAG) ─────────────────────────────

/// One item on the board.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemRow {
    pub id: String,
    pub title: String,
    pub state: ItemState,
    /// Dependency depth (0 = no deps): the board indents by it.
    pub depth: usize,
    pub deps: Vec<String>,
    pub attempts: usize,
    pub circuit_open: bool,
}

/// Longest dependency chain above each item (cycles and unknown deps are cut).
pub fn item_depths(items: &[Item]) -> HashMap<String, usize> {
    fn depth(
        id: &str,
        by: &HashMap<&str, &Item>,
        memo: &mut HashMap<String, usize>,
        path: &mut HashSet<String>,
    ) -> usize {
        if let Some(d) = memo.get(id) {
            return *d;
        }
        let Some(item) = by.get(id) else { return 0 };
        if !path.insert(id.to_string()) {
            return 0;
        }
        let d = item
            .deps
            .iter()
            .filter(|d| by.contains_key(d.as_str()))
            .map(|d| depth(d, by, memo, path) + 1)
            .max()
            .unwrap_or(0);
        path.remove(id);
        memo.insert(id.to_string(), d);
        d
    }
    let by: HashMap<&str, &Item> = items.iter().map(|i| (i.id.as_str(), i)).collect();
    let mut memo = HashMap::new();
    for i in items {
        depth(&i.id, &by, &mut memo, &mut HashSet::new());
    }
    memo
}

/// Items in dependency order (depth, then id).
pub fn item_rows(run: &RunEntry) -> Vec<ItemRow> {
    let depths = item_depths(&run.info.items);
    let mut rows: Vec<ItemRow> = run
        .info
        .items
        .iter()
        .map(|i| ItemRow {
            id: i.id.clone(),
            title: i.title.clone(),
            state: i.state,
            depth: depths.get(&i.id).copied().unwrap_or(0),
            deps: i.deps.clone(),
            attempts: run.info.attempts.iter().filter(|a| a.item == i.id).count(),
            circuit_open: i.circuit_open,
        })
        .collect();
    rows.sort_by_key(|r| (r.depth, id_number(&r.id)));
    rows
}

/// Items grouped by board column (every column present, possibly empty).
pub fn columns(run: &RunEntry) -> Vec<(ItemState, Vec<ItemRow>)> {
    let rows = item_rows(run);
    COLUMNS
        .iter()
        .map(|c| (*c, rows.iter().filter(|r| r.state == *c).cloned().collect()))
        .collect()
}

pub fn item_state_label(st: ItemState) -> &'static str {
    match st {
        ItemState::Pending => s::COL_PENDING,
        ItemState::Ready => s::COL_READY,
        ItemState::Assigned => s::COL_ASSIGNED,
        ItemState::Blocked => s::COL_BLOCKED,
        ItemState::Completed => s::COL_COMPLETED,
        ItemState::Failed => s::COL_FAILED,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Accent,
    Ok,
    Warn,
    Bad,
    Muted,
}

// ───────────────────────────── notifications ─────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NoticeKind {
    Warning,
    Error,
}

/// A person-facing notification (docs/19 §4.3: blocked, gates, breaker,
/// `unknown`, and escalations — never a worker's ordinary turn or report).
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub kind: NoticeKind,
    pub title: String,
    pub body: String,
    /// Where a click goes: the run's coordinator terminal.
    pub run: String,
}

pub fn notice_for_mail(e: &OrchMailEvent) -> Option<Notice> {
    (e.kind == MailType::Escalation).then(|| Notice {
        kind: NoticeKind::Warning,
        title: format!("{} · {}", s::NOTICE_ESCALATION, e.run),
        body: super::ko::event_subject(e.text.as_ref(), &e.subject),
        run: e.run.clone(),
    })
}

/// Notices for what changed between two snapshots of a run: a start circuit
/// breaker that opened, a stop that could not be proven.
pub fn notices_for_change(old: Option<&RunInfo>, new: &RunInfo) -> Vec<Notice> {
    let Some(old) = old else { return Vec::new() };
    let mut out = Vec::new();
    for i in &new.items {
        let was = old
            .items
            .iter()
            .find(|o| o.id == i.id)
            .is_some_and(|o| o.circuit_open);
        if i.circuit_open && !was {
            out.push(Notice {
                kind: NoticeKind::Error,
                title: format!("{} · {}", s::NOTICE_BREAKER, new.run.id),
                body: format!("{} {} — {}", s::ITEM, i.id, i.title),
                run: new.run.id.clone(),
            });
        }
    }
    for a in &new.attempts {
        let was = old.attempts.iter().find(|o| o.id == a.id).map(|o| o.state);
        if a.state == AttemptState::Unknown && was.is_some_and(|w| w != AttemptState::Unknown) {
            out.push(Notice {
                kind: NoticeKind::Warning,
                title: format!("{} · {}", s::NOTICE_UNKNOWN, new.run.id),
                body: format!(
                    "{} {} — {}",
                    s::WORKER,
                    a.id,
                    a.error.clone().unwrap_or_default()
                ),
                run: new.run.id.clone(),
            });
        }
    }
    out
}

// ───────────────────────────── settings ─────────────────────────────

/// Choices the settings page offers (buttons, so the page is keyboard-only).
pub const RELEASE_CHOICES: [Option<u64>; 5] = [None, Some(5), Some(10), Some(30), Some(60)];
pub const STALL_CHOICES: [Option<u64>; 4] = [None, Some(10), Some(20), Some(60)];
pub const DEPTH_CHOICES: [u32; 4] = [1, 2, 3, 5];
/// Resource limits (`orch.limits.*`, v18) and the values offered for each,
/// in the daemon's text forms.
pub const LIMIT_CHOICES: [(&str, &[&str]); 6] = [
    ("max_workers", &["1", "2", "3", "4", "6"]),
    ("run_workers", &["2", "4", "8", "16"]),
    ("cpu_max_pct", &["off", "50%", "70%", "85%"]),
    ("min_free_mem", &["off", "2G", "4G", "8G"]),
    ("min_free_disk", &["off", "10G", "20G", "50G"]),
    ("cpu_max_wait", &["off", "10m", "30m", "60m"]),
];

/// Korean label of a limit key.
/// The Codex worker allowance of one repository as a chip (docs/19 §18.1).
pub fn allowance_label(on: Option<bool>) -> (&'static str, Tone) {
    match on {
        Some(true) => (s::ALLOW_ON, Tone::Ok),
        Some(false) => (s::ALLOW_OFF, Tone::Muted),
        None => (s::ALLOW_UNKNOWN, Tone::Muted),
    }
}

pub fn limit_label(key: &str) -> &'static str {
    match key {
        "max_workers" => s::LIM_MAX_WORKERS,
        "run_workers" => s::LIM_RUN_WORKERS,
        "cpu_max_pct" => s::LIM_CPU,
        "min_free_mem" => s::LIM_MEM,
        "min_free_disk" => s::LIM_DISK,
        "cpu_max_wait" => s::LIM_CPU_WAIT,
        _ => "",
    }
}

/// Is `choice` the limit's current value? Text forms may differ in case and
/// a trailing `%` (`70` vs `70%`).
pub fn limit_is(value: &str, choice: &str) -> bool {
    let norm = |v: &str| v.trim().trim_end_matches('%').to_ascii_lowercase();
    norm(value) == norm(choice)
}

/// Text for a choice button.
pub fn limit_choice_label(key: &str, choice: &str) -> String {
    match (key, choice) {
        (_, "off") => s::NEVER.to_string(),
        ("max_workers" | "run_workers", n) => format!("{n}개"),
        (_, v) if v.ends_with('m') => format!("{}분", v.trim_end_matches('m')),
        (_, v) => v.to_string(),
    }
}
pub const PLACE_CHOICES: [&str; 2] = ["new", "current"];

pub fn minutes_label(m: Option<u64>) -> String {
    match m {
        None => s::NEVER.to_string(),
        Some(m) if m % 60 == 0 => format!("{}시간", m / 60),
        Some(m) => format!("{m}분"),
    }
}

pub fn place_label(p: &str) -> &'static str {
    if p == "current" {
        s::PLACE_CURRENT
    } else {
        s::PLACE_NEW
    }
}

/// The settings with one field changed (what a button sends).
pub fn with_change(base: &OrchSettings, field: &str, value: &str) -> OrchSettings {
    let mut s = base.clone();
    let mins = |v: &str| v.parse::<u64>().ok().filter(|m| *m > 0);
    match field {
        "release_after_min" => s.release_after_min = mins(value),
        "stall_after_min" => s.stall_after_min = mins(value),
        "max_depth" => s.max_depth = value.parse().unwrap_or(s.max_depth),
        "default_place" => s.default_place = value.to_string(),
        _ => {}
    }
    s.locked.clear();
    s
}

/// One option of a radio group on the orchestration screens (ui-10: radio
/// segments with a checked state, not filled buttons).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOpt {
    pub value: String,
    pub text: String,
    pub selected: bool,
}

/// A settings row as a named radio group. `off_list` is the current value
/// when none of the options is it (set by the environment or the file):
/// shown as a chip, never lost.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceGroup {
    pub field: String,
    pub label: String,
    pub options: Vec<ChoiceOpt>,
    pub off_list: Option<String>,
    pub locked: bool,
}

impl ChoiceGroup {
    /// The accessible name of one option, e.g. `자동 정리: 10분`.
    pub fn option_name(&self, o: &ChoiceOpt) -> String {
        format!("{}: {}", self.label, o.text)
    }
}

fn group(
    field: &str,
    label: &str,
    opts: Vec<(String, String)>,
    current: &str,
    off_text: String,
    locked: bool,
) -> ChoiceGroup {
    let options: Vec<ChoiceOpt> = opts
        .into_iter()
        .map(|(value, text)| ChoiceOpt {
            selected: value == current,
            value,
            text,
        })
        .collect();
    let off_list = (!options.iter().any(|o| o.selected)).then_some(off_text);
    ChoiceGroup {
        field: field.into(),
        label: label.into(),
        options,
        off_list,
        locked,
    }
}

/// The four orchestration settings as radio groups.
pub fn settings_groups(cur: &OrchSettings) -> Vec<ChoiceGroup> {
    let locked = |f: &str| cur.locked.iter().any(|l| l == f);
    let mins = |m: Option<u64>| m.map(|m| m.to_string()).unwrap_or_default();
    let minute_opts = |c: &[Option<u64>]| {
        c.iter()
            .map(|m| (mins(*m), minutes_label(*m)))
            .collect::<Vec<_>>()
    };
    vec![
        group(
            "default_place",
            s::SET_PLACE,
            PLACE_CHOICES
                .iter()
                .map(|p| (p.to_string(), place_label(p).to_string()))
                .collect(),
            &cur.default_place,
            cur.default_place.clone(),
            locked("default_place"),
        ),
        group(
            "release_after_min",
            s::SET_RELEASE,
            minute_opts(&RELEASE_CHOICES),
            &mins(cur.release_after_min),
            minutes_label(cur.release_after_min),
            locked("release_after_min"),
        ),
        group(
            "stall_after_min",
            s::SET_STALL,
            minute_opts(&STALL_CHOICES),
            &mins(cur.stall_after_min),
            minutes_label(cur.stall_after_min),
            locked("stall_after_min"),
        ),
        group(
            "max_depth",
            s::SET_DEPTH,
            DEPTH_CHOICES
                .iter()
                .map(|d| (d.to_string(), d.to_string()))
                .collect(),
            &cur.max_depth.to_string(),
            cur.max_depth.to_string(),
            locked("max_depth"),
        ),
    ]
}

/// One resource limit as a radio group (`value` in the daemon's text form).
pub fn limit_group(key: &str, choices: &[&str], value: &str, locked: bool) -> ChoiceGroup {
    let options: Vec<ChoiceOpt> = choices
        .iter()
        .map(|c| ChoiceOpt {
            value: c.to_string(),
            text: limit_choice_label(key, c),
            selected: limit_is(value, c),
        })
        .collect();
    let off_list = (!options.iter().any(|o| o.selected)).then(|| value.to_string());
    ChoiceGroup {
        field: key.into(),
        label: limit_label(key).into(),
        options,
        off_list,
        locked,
    }
}

/// The board's run picker: one option per run, the shown one checked.
pub fn run_group(runs: &[&RunEntry], selected: Option<&str>) -> ChoiceGroup {
    let options = runs
        .iter()
        .map(|r| {
            let phase = if r.is_finished() {
                s::FINISHED_RUN
            } else {
                s::ACTIVE
            };
            ChoiceOpt {
                value: r.id().to_string(),
                text: format!("{} · {} · {phase}", r.id(), r.info.run.objective),
                selected: selected == Some(r.id()),
            }
        })
        .collect();
    ChoiceGroup {
        field: "run".into(),
        label: s::RUNS.into(),
        options,
        off_list: None,
        locked: false,
    }
}
