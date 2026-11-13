//! B05: failed snapshots leave transitions eligible for retry, including
//! after restarting, without repeating generation or breaker bookkeeping.

use super::*;

fn starting() -> T {
    let mut t = setup("attempt-persistence");
    t.o.create_run("x", Some("coordinator"), None, 1).unwrap();
    t.o.add_item("r1", None, "work", &[], None, 1).unwrap();
    // Exercise a nonzero breaker and a generation beyond the first.
    for _ in 0..2 {
        let a = begin(&mut t.o, "r1", "i1", "old", None).unwrap();
        assert!(!t.o.start_failed(&a.id, "not started", 11).unwrap());
    }
    begin(&mut t.o, "r1", "i1", "current", None).unwrap();
    assert_eq!(t.o.item("i1").unwrap().start_failures, 2);
    t
}

fn assert_saved(t: &T) {
    let disk: RunRecord =
        serde_json::from_slice(&std::fs::read(store::run_json(&t.dir, "r1")).unwrap()).unwrap();
    assert_eq!(t.o.run("r1").unwrap().rec, disk);
    let restarted = Orch::load(&t.dir, Limits::default(), 99);
    assert!(restarted.notes.is_empty(), "{:?}", restarted.notes);
    assert_eq!(restarted.run("r1").unwrap().rec, disk);
}

/// The store already uses a directory at run.json.tmp to inject write
/// failure. Unlike an exclusive Windows lock, this keeps run.json readable
/// while the failure is active and works on every supported platform.
fn transient_failure(t: &mut T, restart: bool, mut change: impl FnMut(&mut Orch) -> Result<()>) {
    let before = t.o.run("r1").unwrap().rec.clone();
    let path = store::run_json(&t.dir, "r1");
    let bytes = std::fs::read(&path).unwrap();
    let blocked = path.with_extension("json.tmp");
    std::fs::create_dir(&blocked).unwrap();
    for _ in 0..2 {
        assert_eq!(change(&mut t.o).unwrap_err().kind, ErrorKind::Io);
        assert_eq!(t.o.run("r1").unwrap().rec, before);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_saved(t);
    }
    if restart {
        t.o = Orch::load(&t.dir, Limits::default(), 99);
        assert_eq!(change(&mut t.o).unwrap_err().kind, ErrorKind::Io);
        assert_eq!(t.o.run("r1").unwrap().rec, before);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::remove_dir(&blocked).unwrap();
    change(&mut t.o).unwrap();
    assert_saved(t);
}

#[test]
fn mark_ready_save_failure_preserves_starting_and_breaker() {
    for restart in [false, true] {
        let mut t = starting();
        transient_failure(&mut t, restart, |o| {
            assert!(o.mark_ready("a3", 20)?);
            Ok(())
        });
        let a = t.o.attempt("a3").unwrap();
        assert_eq!((a.state, a.ready_ms), (AttemptState::Ready, Some(20)));
        assert_eq!(t.o.item("i1").unwrap().start_failures, 0);
        assert!(!t.o.mark_ready("a3", 21).unwrap());
        assert_eq!(t.o.attempts().count(), 3);
        assert_saved(&t);
    }
}

#[test]
fn start_failed_save_failure_counts_only_the_committed_failure() {
    for restart in [false, true] {
        let mut t = starting();
        t.o.enqueue("a3", serde_json::json!({"agent": "codex"}), None, 12)
            .unwrap();
        transient_failure(&mut t, restart, |o| {
            assert!(o.start_failed("a3", "spawn failed", 20)?);
            Ok(())
        });
        let a = t.o.attempt("a3").unwrap();
        assert_eq!(a.state, AttemptState::StartFailed);
        assert_eq!(a.worker, WorkerState::Released);
        assert_eq!((a.ended_ms, a.released_ms), (Some(20), Some(20)));
        assert_eq!(a.error.as_deref(), Some("spawn failed"));
        assert!(a.queue.is_none() && a.launch.is_none());
        let i = t.o.item("i1").unwrap();
        assert_eq!((i.start_failures, i.circuit_open), (3, true));
        assert_eq!((i.state, i.generation), (ItemState::Failed, 3));
        assert_eq!(t.o.attempts().count(), 3);
    }
}

#[test]
fn end_attempt_save_failure_preserves_queue_and_retry_eligibility() {
    for state in [
        AttemptState::Stopped,
        AttemptState::Lost,
        AttemptState::Unknown,
        AttemptState::Abandoned,
    ] {
        for restart in [false, true] {
            let mut t = starting();
            t.o.enqueue("a3", serde_json::json!({"agent": "codex"}), None, 12)
                .unwrap();
            transient_failure(&mut t, restart, |o| {
                assert!(o.end_attempt("a3", state, "ended", None, Some(10), 20)?);
                Ok(())
            });
            let a = t.o.attempt("a3").unwrap();
            assert_eq!((a.state, a.worker), (state, WorkerState::Finished));
            assert_eq!((a.ended_ms, a.release_at_ms), (Some(20), Some(30)));
            assert!(a.queue.is_none() && a.launch.is_none());
            assert_eq!(t.o.item("i1").unwrap().state, ItemState::Ready);
            assert_eq!(t.o.due_releases(30), vec!["a3"]);
            assert!(
                !t.o.end_attempt("a3", state, "again", None, None, 21)
                    .unwrap()
            );
            let next = begin(&mut t.o, "r1", "i1", "next", None).unwrap();
            assert_eq!((next.id.as_str(), next.generation), ("a4", 4));
            assert_eq!(t.o.attempts().count(), 4);
            assert_saved(&t);
        }
    }
}

#[test]
fn worker_save_failure_preserves_due_release_and_mailbox() {
    for state in [WorkerState::Kept, WorkerState::Released] {
        for restart in [false, true] {
            let mut t = starting();
            t.o.attach_worker("a3", Some("task"), "worker", Some("brief"), 4)
                .unwrap();
            t.o.end_attempt("a3", AttemptState::Stopped, "stop", None, Some(10), 20)
                .unwrap();
            transient_failure(&mut t, restart, |o| {
                assert_eq!(o.due_releases(30), vec!["a3"]);
                assert_eq!(o.mailbox_terminal("attempt:a3").as_deref(), Some("worker"));
                o.set_worker("a3", state, Some("archive"), 30)?;
                Ok(())
            });
            let a = t.o.attempt("a3").unwrap();
            assert_eq!(a.worker, state);
            assert!(a.release_at_ms.is_none());
            assert!(t.o.due_releases(30).is_empty());
            if state == WorkerState::Released {
                assert_eq!(a.released_ms, Some(30));
                assert_eq!(a.log_path.as_deref(), Some("archive"));
                assert!(t.o.mailbox_terminal("attempt:a3").is_none());
            } else {
                assert!(a.released_ms.is_none() && a.log_path.is_none());
                assert_eq!(
                    t.o.mailbox_terminal("attempt:a3").as_deref(),
                    Some("worker")
                );
            }
        }
    }
}

#[test]
fn attach_and_bookkeeping_save_failures_are_retryable() {
    for restart in [false, true] {
        let mut t = starting();
        transient_failure(&mut t, restart, |o| {
            o.attach_worker("a3", Some("task"), "worker", Some("brief"), 4)?;
            Ok(())
        });
        let a = t.o.attempt("a3").unwrap();
        assert_eq!(a.task.as_deref(), Some("task"));
        assert_eq!(a.terminal.as_deref(), Some("worker"));
        assert_eq!(a.brief_path.as_deref(), Some("brief"));
        assert_eq!(a.seen_turn, 4);
        transient_failure(&mut t, restart, |o| {
            o.update_attempt("a3", |a| {
                a.seen_turn += 1;
                a.escalated_turn = Some(a.seen_turn);
                a.stalled_at_output_ms = Some(15);
            })?;
            Ok(())
        });
        let a = t.o.attempt("a3").unwrap();
        assert_eq!((a.seen_turn, a.escalated_turn), (5, Some(5)));
        assert_eq!(a.stalled_at_output_ms, Some(15));
        transient_failure(&mut t, restart, |o| {
            o.enqueue("a3", serde_json::json!({"agent": "codex"}), None, 16)?;
            Ok(())
        });
        assert_eq!(t.o.queue().len(), 1);
        assert_eq!(t.o.queue()[0].id, "a3");
        assert_eq!(t.o.attempts().count(), 3);
    }
}

#[test]
fn begin_save_failure_preserves_attempts_and_generations() {
    for retry in [false, true] {
        for restart in [false, true] {
            let mut t = starting();
            if !retry {
                t.o.end_attempt("a3", AttemptState::Stopped, "stop", None, None, 20)
                    .unwrap();
            }
            transient_failure(&mut t, restart, |o| {
                let a = begin(o, "r1", "i1", "next", retry.then_some("a3"))?;
                assert_eq!(a.generation, 4);
                Ok(())
            });
            let old = t.o.attempt("a3").unwrap();
            if retry {
                assert_eq!(
                    (old.state, old.worker),
                    (AttemptState::Abandoned, WorkerState::Kept)
                );
            } else {
                assert_eq!(old.state, AttemptState::Stopped);
            }
            let i = t.o.item("i1").unwrap();
            assert_eq!((i.state, i.generation), (ItemState::Assigned, 4));
            // Failed run saves reserve ids, but never insert phantom attempts
            // or skip generations. A restart must not reuse a reserved id.
            let expected = if restart { "a7" } else { "a6" };
            assert_eq!(i.attempt.as_deref(), Some(expected));
            assert_eq!(
                t.o.attempts().map(|a| a.id.as_str()).collect::<Vec<_>>(),
                vec!["a1", "a2", "a3", expected]
            );
            assert_eq!(
                t.o.verify_token(expected, Some("next")).unwrap().generation,
                4
            );
            assert_eq!(
                t.o.verify_token(expected, Some("current"))
                    .unwrap_err()
                    .kind,
                ErrorKind::AttemptFenced
            );
            assert_saved(&t);
        }
    }
}

#[test]
fn begin_index_save_failure_does_not_replace_or_reserve_an_attempt() {
    for restart in [false, true] {
        let mut t = starting();
        let before = t.o.run("r1").unwrap().rec.clone();
        let path = store::index_json(&t.dir);
        let bytes = std::fs::read(&path).unwrap();
        let blocked = path.with_extension("json.tmp");
        std::fs::create_dir(&blocked).unwrap();
        for _ in 0..2 {
            assert_eq!(
                begin(&mut t.o, "r1", "i1", "next", Some("a3"))
                    .unwrap_err()
                    .kind,
                ErrorKind::Io
            );
            assert_eq!(t.o.run("r1").unwrap().rec, before);
            assert_eq!(t.o.index.counters.get("a"), Some(&3));
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_saved(&t);
        }
        if restart {
            t.o = Orch::load(&t.dir, Limits::default(), 99);
        }
        std::fs::remove_dir(&blocked).unwrap();
        let a = begin(&mut t.o, "r1", "i1", "next", Some("a3")).unwrap();
        assert_eq!((a.id.as_str(), a.generation), ("a4", 4));
        assert_eq!(t.o.attempts().count(), 4);
        assert_saved(&t);
    }
}

#[test]
fn admission_save_failure_preserves_launch_token_and_wait_verdict() {
    for allow_start in [false, true] {
        for restart in [false, true] {
            let mut t = starting();
            let launch = serde_json::json!({"agent": "codex"});
            t.o.enqueue("a3", launch.clone(), None, 12).unwrap();
            let limits = ResourceLimits {
                max_workers: usize::from(allow_start),
                ..Default::default()
            };
            transient_failure(&mut t, restart, |o| {
                assert!(o.verify_token("a3", Some("current")).is_ok());
                assert_eq!(o.attempt("a3")?.launch, Some(launch.clone()));
                let out = o.admit(
                    &limits,
                    &Sample::default(),
                    |_| None,
                    Some(hash_token("admitted")),
                    20,
                )?;
                if allow_start {
                    let (a, data) = out.start.unwrap();
                    assert_eq!(a.id, "a3");
                    assert!(a.token_hash.is_empty() && a.launch.is_none());
                    assert_eq!(data, launch);
                } else {
                    assert!(out.start.is_none());
                    assert_eq!(out.changed.len(), 1);
                }
                Ok(())
            });
            let again =
                t.o.admit(
                    &limits,
                    &Sample::default(),
                    |_| None,
                    Some(hash_token("unused")),
                    21,
                )
                .unwrap();
            assert!(again.start.is_none() && again.changed.is_empty());
            let a = t.o.attempt("a3").unwrap();
            if allow_start {
                assert!(a.queue.is_none() && a.launch.is_none());
                assert!(t.o.verify_token("a3", Some("admitted")).is_ok());
                assert_eq!(t.o.active_workers(), 1);
            } else {
                let q = a.queue.as_ref().unwrap();
                assert_eq!((q.kind, q.position), (WaitKind::Workers, 1));
                assert_eq!(a.launch, Some(launch));
            }
            assert_saved(&t);
        }
    }
}

#[test]
fn admission_never_loses_launch_when_another_run_save_fails() {
    // r2's waiting verdict saves first; fail either the next waiting record
    // (r3) or the final admission (r1). Partial verdict progress is safe,
    // but the launch must remain queued until the caller can receive it.
    for blocked_run in ["r3", "r1"] {
        for restart in [false, true] {
            let mut t = starting();
            t.o.enqueue("a3", serde_json::json!({"launch": 1}), None, 12)
                .unwrap();
            for n in 2..=3 {
                let run = t.o.create_run("other", None, None, 12).unwrap();
                let item = t.o.add_item(&run.id, None, "work", &[], None, 12).unwrap();
                let a = begin(&mut t.o, &run.id, &item.id, "queued", None).unwrap();
                t.o.enqueue(&a.id, serde_json::json!({"launch": n}), None, 12 + n)
                    .unwrap();
            }
            let limits = ResourceLimits::default();
            let blocked = store::run_json(&t.dir, blocked_run).with_extension("json.tmp");
            std::fs::create_dir(&blocked).unwrap();
            for _ in 0..2 {
                assert_eq!(
                    t.o.admit(
                        &limits,
                        &Sample::default(),
                        |_| None,
                        Some(hash_token("admitted")),
                        20
                    )
                    .unwrap_err()
                    .kind,
                    ErrorKind::Io
                );
                let disk = Orch::load(&t.dir, Limits::default(), 99);
                for run in ["r1", "r2", "r3"] {
                    let rec: RunRecord = serde_json::from_slice(
                        &std::fs::read(store::run_json(&t.dir, run)).unwrap(),
                    )
                    .unwrap();
                    assert_eq!(t.o.run(run).unwrap().rec, rec);
                    assert_eq!(disk.run(run).unwrap().rec, rec);
                }
                assert_eq!(t.o.queue().len(), 3);
                assert_eq!(t.o.active_workers(), 0);
                assert!(t.o.verify_token("a3", Some("current")).is_ok());
                assert_eq!(
                    t.o.attempt("a3").unwrap().launch,
                    Some(serde_json::json!({"launch": 1}))
                );
            }
            if restart {
                t.o = Orch::load(&t.dir, Limits::default(), 99);
            }
            std::fs::remove_dir(&blocked).unwrap();
            let out =
                t.o.admit(
                    &limits,
                    &Sample::default(),
                    |_| None,
                    Some(hash_token("admitted")),
                    20,
                )
                .unwrap();
            let (a, launch) = out.start.unwrap();
            assert_eq!(
                (a.id.as_str(), launch),
                ("a3", serde_json::json!({"launch": 1}))
            );
            assert_eq!(t.o.queue().len(), 2);
            assert_eq!(t.o.active_workers(), 1);
            assert_eq!(t.o.attempts().count(), 5);
            assert_saved(&t);
        }
    }
}

#[cfg(windows)]
#[test]
fn mark_ready_rename_failure_does_not_publish_the_temporary_snapshot() {
    let mut t = starting();
    let before = t.o.run("r1").unwrap().rec.clone();
    let path = store::run_json(&t.dir, "r1");
    let held = lock(&path);
    assert_eq!(t.o.mark_ready("a3", 20).unwrap_err().kind, ErrorKind::Io);
    assert_eq!(t.o.run("r1").unwrap().rec, before);
    assert!(path.with_extension("json.tmp").is_file());
    drop(held);
    assert_saved(&t);
    t.o = Orch::load(&t.dir, Limits::default(), 99);
    assert!(t.o.mark_ready("a3", 21).unwrap());
    assert_eq!(t.o.attempt("a3").unwrap().ready_ms, Some(21));
    assert_eq!(t.o.item("i1").unwrap().start_failures, 0);
    assert!(!path.with_extension("json.tmp").exists());
    assert_saved(&t);
}
