//! The resource gate (docs/19 §5.1): which queued worker may start now, and
//! why the others wait. Pure: the daemon measures (an injectable meter) and
//! passes a [`Sample`]; this module only decides.
//!
//! Rules:
//! - FIFO by queue time (ties by attempt number). A worker that waits for a
//!   global resource (worker count, CPU, memory, disk) holds back every
//!   worker behind it, so later arrivals never overtake it (no starvation).
//! - A worker that waits only for its own run's limit does not hold back
//!   other runs' workers (waiting cannot help them).
//! - CPU is a soft limit: after `cpu_max_wait_ms` in the queue the worker
//!   starts anyway (like `tools/cpu-gate.ps1`: delayed, never blocked forever).
//!   Worker counts, memory and disk are hard limits.
//! - At most one start per pass, so the next pass measures again.
//! - Running workers are never stopped for resources.

use std::collections::HashMap;

use crate::model::{Attempt, Ms, WaitKind};

pub const GIB: u64 = 1024 * 1024 * 1024;

/// Limits of the resource gate. `None` turns a check off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceLimits {
    /// Concurrent active workers over all runs.
    pub max_workers: usize,
    /// Open workers per run.
    pub run_workers: usize,
    /// System CPU (average over the meter's window), percent.
    pub cpu_max_pct: Option<f64>,
    /// Available physical memory, bytes.
    pub min_free_mem: Option<u64>,
    /// Free space on the worktrees drive, bytes.
    pub min_free_disk: Option<u64>,
    /// After this long in the queue the CPU limit no longer holds a worker.
    pub cpu_max_wait_ms: Option<Ms>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        ResourceLimits {
            max_workers: 3,
            run_workers: 8,
            cpu_max_pct: Some(70.0),
            min_free_mem: Some(4 * GIB),
            min_free_disk: Some(20 * GIB),
            cpu_max_wait_ms: Some(30 * 60_000),
        }
    }
}

/// One measurement. `None` = unknown (does not hold a worker back).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    pub cpu_pct: Option<f64>,
    pub free_mem: Option<u64>,
}

/// The verdict for one queued attempt.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub attempt: String,
    pub kind: WaitKind,
    pub reason: String,
    pub position: usize,
}

/// The result of one pass.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Start this attempt now.
    pub start: Option<String>,
    /// Every other queued attempt, in queue order.
    pub waiting: Vec<Verdict>,
}

fn gib(b: u64) -> String {
    format!("{:.1} GiB", b as f64 / GIB as f64)
}

/// FIFO order of queued attempts.
pub fn queue_order<'a>(attempts: impl Iterator<Item = &'a Attempt>) -> Vec<&'a Attempt> {
    let mut q: Vec<&Attempt> = attempts.filter(|a| a.is_queued()).collect();
    q.sort_by_key(|a| {
        (
            a.queue.as_ref().map_or(0, |s| s.since_ms),
            a.id.trim_start_matches(|c: char| c.is_ascii_alphabetic())
                .parse::<u64>()
                .unwrap_or(0),
        )
    });
    q
}

/// Decide one pass. `attempts` are all attempts of all runs; `free_disk`
/// measures a path (the queued worker's worktrees root).
pub fn plan(
    attempts: &[&Attempt],
    limits: &ResourceLimits,
    sample: &Sample,
    mut free_disk: impl FnMut(&str) -> Option<u64>,
    now: Ms,
) -> Plan {
    let mut active = 0usize;
    let mut per_run: HashMap<&str, usize> = HashMap::new();
    for a in attempts.iter().filter(|a| a.is_active()) {
        active += 1;
        *per_run.entry(a.run.as_str()).or_default() += 1;
    }
    let queue = queue_order(attempts.iter().copied());
    let mut out = Plan::default();
    // The first worker that waits for a global resource holds back the rest.
    let mut held: Option<usize> = None;
    for (i, a) in queue.iter().enumerate() {
        let position = i + 1;
        let verdict = |kind: WaitKind, reason: String| Verdict {
            attempt: a.id.clone(),
            kind,
            reason,
            position,
        };
        let run_open = per_run.get(a.run.as_str()).copied().unwrap_or(0);
        if run_open >= limits.run_workers {
            out.waiting.push(verdict(
                WaitKind::RunWorkers,
                format!(
                    "이 Run의 워커가 {run_open}/{}개 열려 있어 대기해요. 하나가 끝나면 시작해요.",
                    limits.run_workers
                ),
            ));
            continue;
        }
        if let Some(ahead) = held {
            out.waiting.push(verdict(
                WaitKind::Behind,
                format!("앞에서 대기 중인 워커 {ahead}개가 먼저 시작해요."),
            ));
            held = Some(ahead + 1);
            continue;
        }
        let q = a.queue.as_ref();
        let waited = now.saturating_sub(q.map_or(now, |s| s.since_ms));
        let disk_path = q.and_then(|s| s.disk_path.as_deref());
        let blocked = if active >= limits.max_workers {
            Some((
                WaitKind::Workers,
                format!(
                    "동시 워커 {active}/{}개가 실행 중이라 대기해요. 하나가 끝나면 차례대로 시작해요.",
                    limits.max_workers
                ),
            ))
        } else if let (Some(min), Some(free)) =
            (limits.min_free_disk, disk_path.and_then(&mut free_disk))
            && free < min
        {
            Some((
                WaitKind::Disk,
                format!(
                    "작업 폴더 드라이브({})의 여유 공간이 {}로 최소 {}보다 적어 대기해요. 공간을 비우면 시작해요.",
                    disk_path.unwrap_or("?"),
                    gib(free),
                    gib(min)
                ),
            ))
        } else if let (Some(min), Some(free)) = (limits.min_free_mem, sample.free_mem)
            && free < min
        {
            Some((
                WaitKind::Memory,
                format!(
                    "사용 가능한 메모리가 {}로 최소 {}보다 적어 대기해요.",
                    gib(free),
                    gib(min)
                ),
            ))
        } else if let (Some(max), Some(cpu)) = (limits.cpu_max_pct, sample.cpu_pct)
            && cpu > max
            && limits.cpu_max_wait_ms.is_none_or(|w| waited < w)
        {
            let tail = limits
                .cpu_max_wait_ms
                .map(|w| format!(" 늦어도 {}분 뒤에는 시작해요.", w.div_ceil(60_000)))
                .unwrap_or_default();
            Some((
                WaitKind::Cpu,
                format!("CPU 사용률이 최근 평균 {cpu:.0}%로 한도 {max:.0}%를 넘어 대기해요.{tail}"),
            ))
        } else {
            None
        };
        match blocked {
            Some((kind, reason)) => {
                out.waiting.push(verdict(kind, reason));
                held = Some(1);
            }
            None if out.start.is_none() => {
                out.start = Some(a.id.clone());
                active += 1;
                *per_run.entry(a.run.as_str()).or_default() += 1;
                // The next pass measures again before anything else starts.
                held = Some(1);
            }
            None => unreachable!("held is set after a start"),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AttemptState, QueueState, WorkerState};

    fn att(id: &str, run: &str, queued: Option<Ms>) -> Attempt {
        let mut a: Attempt = serde_json::from_value(serde_json::json!({
            "id": id, "run": run, "item": "i1", "generation": 1, "state": "starting",
            "worker": "active", "agent": "shell", "place": "new", "depth": 1, "created_ms": 0
        }))
        .unwrap();
        a.queue = queued.map(|since_ms| QueueState {
            since_ms,
            kind: WaitKind::Queued,
            reason: String::new(),
            position: 0,
            disk_path: Some("C:\\wt".into()),
        });
        if queued.is_none() {
            a.state = AttemptState::Ready;
        }
        a
    }

    fn lim() -> ResourceLimits {
        ResourceLimits {
            max_workers: 1,
            ..Default::default()
        }
    }

    fn ok() -> Sample {
        Sample {
            cpu_pct: Some(10.0),
            free_mem: Some(64 * GIB),
        }
    }

    fn disk(_: &str) -> Option<u64> {
        Some(500 * GIB)
    }

    #[test]
    fn limit_one_queues_the_second_in_fifo_order() {
        let mut v = [
            att("a1", "r1", None),
            att("a3", "r1", Some(20)),
            att("a2", "r1", Some(10)),
        ];
        let p = plan(&v.iter().collect::<Vec<_>>(), &lim(), &ok(), disk, 100);
        assert_eq!(p.start, None);
        assert_eq!(p.waiting[0].attempt, "a2", "FIFO by queue time, not by id");
        assert_eq!(p.waiting[0].kind, WaitKind::Workers);
        assert!(
            p.waiting[0].reason.contains("1/1"),
            "{}",
            p.waiting[0].reason
        );
        assert_eq!(
            (
                p.waiting[1].attempt.as_str(),
                p.waiting[1].kind,
                p.waiting[1].position
            ),
            ("a3", WaitKind::Behind, 2)
        );
        // The first worker ends: the oldest queued one starts, one per pass.
        v[0].state = AttemptState::Succeeded;
        v[0].worker = WorkerState::Finished;
        let p = plan(&v.iter().collect::<Vec<_>>(), &lim(), &ok(), disk, 100);
        assert_eq!(p.start.as_deref(), Some("a2"));
        assert_eq!(p.waiting.len(), 1);
        assert_eq!(p.waiting[0].kind, WaitKind::Behind);
    }

    #[test]
    fn cpu_memory_disk_from_the_sample() {
        let v = [att("a1", "r1", Some(0)), att("a2", "r2", Some(1))];
        let l = ResourceLimits::default();
        let hot = Sample {
            cpu_pct: Some(85.0),
            ..ok()
        };
        let p = plan(&v.iter().collect::<Vec<_>>(), &l, &hot, disk, 1_000);
        assert_eq!(p.start, None);
        assert_eq!(p.waiting[0].kind, WaitKind::Cpu);
        assert!(
            p.waiting[0].reason.contains("85%") && p.waiting[0].reason.contains("70%"),
            "{}",
            p.waiting[0].reason
        );
        assert_eq!(p.waiting[1].kind, WaitKind::Behind, "no overtaking");
        // CPU is soft: after the maximum wait the head starts anyway.
        let p = plan(
            &v.iter().collect::<Vec<_>>(),
            &l,
            &hot,
            disk,
            30 * 60_000 + 1,
        );
        assert_eq!(p.start.as_deref(), Some("a1"));
        // Unknown measurements do not hold anything back.
        let p = plan(
            &v.iter().collect::<Vec<_>>(),
            &l,
            &Sample::default(),
            |_| None,
            1_000,
        );
        assert_eq!(p.start.as_deref(), Some("a1"));
        // Memory and disk are hard limits.
        let low = Sample {
            free_mem: Some(GIB),
            ..ok()
        };
        let p = plan(&v.iter().collect::<Vec<_>>(), &l, &low, disk, u64::MAX / 2);
        assert_eq!(p.waiting[0].kind, WaitKind::Memory);
        assert!(
            p.waiting[0].reason.contains("1.0 GiB"),
            "{}",
            p.waiting[0].reason
        );
        let p = plan(
            &v.iter().collect::<Vec<_>>(),
            &l,
            &ok(),
            |path| (path == "C:\\wt").then_some(5 * GIB),
            u64::MAX / 2,
        );
        assert_eq!(p.waiting[0].kind, WaitKind::Disk);
        assert!(
            p.waiting[0].reason.contains("C:\\wt"),
            "{}",
            p.waiting[0].reason
        );
        // Checks can be turned off.
        let off = ResourceLimits {
            cpu_max_pct: None,
            min_free_mem: None,
            min_free_disk: None,
            ..l
        };
        let p = plan(&v.iter().collect::<Vec<_>>(), &off, &low, |_| Some(0), 0);
        assert_eq!(p.start.as_deref(), Some("a1"));
    }

    #[test]
    fn a_full_run_does_not_hold_back_other_runs() {
        let l = ResourceLimits {
            max_workers: 5,
            run_workers: 1,
            ..Default::default()
        };
        let v = [
            att("a1", "r1", None),
            att("a2", "r1", Some(0)),
            att("a3", "r2", Some(1)),
        ];
        let p = plan(&v.iter().collect::<Vec<_>>(), &l, &ok(), disk, 10);
        assert_eq!(p.start.as_deref(), Some("a3"));
        assert_eq!(p.waiting[0].attempt, "a2");
        assert_eq!(p.waiting[0].kind, WaitKind::RunWorkers);
    }
}
