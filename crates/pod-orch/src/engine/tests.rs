//! The engine's tests, moved unchanged from `engine.rs` (card L3).

use super::*;
use crate::resource::{ResourceLimits, Sample};
use crate::store;
use crate::text;

fn temp(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "pod-orch-eng-{tag}-{}-{nanos}-{seq}",
        std::process::id()
    ))
}

struct T {
    dir: PathBuf,
    o: Orch,
}

impl Drop for T {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup(tag: &str) -> T {
    let dir = temp(tag);
    T {
        o: Orch::load(&dir, Limits::default(), 1),
        dir,
    }
}

fn begin(
    o: &mut Orch,
    run: &str,
    item: &str,
    token: &str,
    retry_of: Option<&str>,
) -> Result<Attempt> {
    o.begin_attempt(
        BeginAttempt {
            run: run.into(),
            item: item.into(),
            agent: "codex".into(),
            place: "new".into(),
            retry_of: retry_of.map(str::to_string),
            token_hash: hash_token(token),
        },
        10,
    )
}

/// docs/19 §18.2: a worker waits for other items of its run; the wait
/// settles once they all finished (completed or failed), survives a
/// reload, and is dropped when the worker ends first.
#[test]
fn waiting_for_other_items() {
    let mut t = setup("wait");
    let o = &mut t.o;
    o.create_run("r", Some("t1"), None, 5).unwrap();
    for spec in ["a", "b", "c"] {
        o.add_item("r1", None, spec, &[], None, 5).unwrap();
    }
    begin(o, "r1", "i1", "k1", None).unwrap();
    begin(o, "r1", "i2", "k2", None).unwrap();
    begin(o, "r1", "i3", "k3", None).unwrap();
    // Bad requests.
    assert_eq!(
        o.wait_for_items("a2", &[]).unwrap_err().kind,
        ErrorKind::InvalidParams
    );
    assert_eq!(
        o.wait_for_items("a2", &["i2".into()]).unwrap_err().kind,
        ErrorKind::InvalidParams,
        "own item"
    );
    assert_eq!(
        o.wait_for_items("a2", &["i9".into()]).unwrap_err().kind,
        ErrorKind::ItemNotFound
    );
    // a2 waits for i1 and i3.
    let st = o
        .wait_for_items("a2", &["i1".into(), "i3".into(), "i1".into()])
        .unwrap();
    assert_eq!(
        st,
        vec![
            ("i1".to_string(), ItemState::Assigned),
            ("i3".to_string(), ItemState::Assigned)
        ]
    );
    assert_eq!(o.attempt("a2").unwrap().waiting_on, vec!["i1", "i3"]);
    assert!(o.settle_waits().unwrap().is_empty());
    o.done("a1", Some("k1"), true, "ok", &[], None, None, 20)
        .unwrap();
    assert!(o.settle_waits().unwrap().is_empty(), "i3 still runs");
    // Survives a reload.
    let mut o2 = Orch::load(&t.dir, Limits::default(), 30);
    assert_eq!(o2.attempt("a2").unwrap().waiting_on, vec!["i1", "i3"]);
    o2.done("a3", Some("k3"), false, "broke", &[], None, None, 40)
        .unwrap();
    let due = o2.settle_waits().unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].0.id, "a2");
    assert_eq!(
        due[0].1,
        vec![
            ("i1".to_string(), ItemState::Completed),
            ("i3".to_string(), ItemState::Failed)
        ]
    );
    assert!(o2.attempt("a2").unwrap().waiting_on.is_empty());
    assert!(o2.settle_waits().unwrap().is_empty(), "settled once");
    // Everything already finished: nothing is recorded.
    assert_eq!(
        o2.wait_for_items("a2", &["i1".into()]).unwrap(),
        vec![("i1".to_string(), ItemState::Completed)]
    );
    assert!(o2.attempt("a2").unwrap().waiting_on.is_empty());
    // A finished worker cannot wait; one that ends while waiting is dropped silently.
    assert_eq!(
        o2.wait_for_items("a1", &["i2".into()]).unwrap_err().kind,
        ErrorKind::InvalidState
    );
    o2.add_item("r1", None, "d", &[], None, 50).unwrap();
    o2.wait_for_items("a2", &["i4".into()]).unwrap();
    o2.done(
        "a2",
        Some("k2"),
        true,
        "gave up waiting",
        &[],
        None,
        None,
        60,
    )
    .unwrap();
    assert!(o2.settle_waits().unwrap().is_empty());
    assert!(o2.attempt("a2").unwrap().waiting_on.is_empty());
}

#[test]
fn ids_items_deps_and_reload() {
    let mut t = setup("deps");
    let o = &mut t.o;
    let r = o.create_run("ship it", Some("t1"), None, 5).unwrap();
    assert_eq!((r.id.as_str(), r.depth), ("r1", 0));
    let a = o
        .add_item("r1", None, "# Build the thing\nmore", &[], None, 5)
        .unwrap();
    assert_eq!(
        (a.id.as_str(), a.title.as_str(), a.state),
        ("i1", "Build the thing", ItemState::Ready)
    );
    let b = o
        .add_item("r1", Some("Test"), "test it", &["i1".into()], None, 5)
        .unwrap();
    assert_eq!(b.state, ItemState::Pending);
    assert_eq!(
        o.add_item("r1", None, "x", &["i9".into()], None, 5)
            .unwrap_err()
            .kind,
        ErrorKind::ItemNotFound
    );
    assert_eq!(
        begin(o, "r1", "i2", "k", None).unwrap_err().kind,
        ErrorKind::DepsNotDone
    );
    let at = begin(o, "r1", "i1", "tok", None).unwrap();
    assert_eq!((at.id.as_str(), at.generation, at.depth), ("a1", 1, 1));
    let d = o
        .done(
            "a1",
            Some("tok"),
            true,
            "did it",
            &[],
            None,
            Some(600_000),
            20,
        )
        .unwrap();
    assert!(d.accepted);
    assert_eq!(d.attempt.worker, WorkerState::Finished);
    assert_eq!(d.attempt.release_at_ms, Some(600_020));
    assert_eq!(
        o.item("i2").unwrap().state,
        ItemState::Ready,
        "dependent promoted"
    );
    assert_eq!(d.mail.recipients, vec!["run:r1".to_string()]);
    // The subject is data too (docs/19 §16.3), and survives a reload.
    let t1 = d.mail.text.clone().expect("structured subject");
    assert_eq!(
        (t1.kind.as_str(), t1.p("outcome"), t1.p("attempt")),
        (text::DONE, "succeeded", "a1")
    );
    assert_eq!(d.mail.subject, t1.en());
    assert!(
        d.mail.body_text.is_none(),
        "the worker's own body is not Pod's text"
    );
    // Everything survives a reload; counters continue.
    let o2 = Orch::load(&t.dir, Limits::default(), 30);
    assert!(o2.notes.is_empty(), "{:?}", o2.notes);
    assert_eq!(o2.item("i1").unwrap().state, ItemState::Completed);
    assert_eq!(o2.attempt("a1").unwrap().token_hash, hash_token("tok"));
    assert_eq!(o2.unread("run:r1").len(), 1);
    assert_eq!(o2.unread("run:r1")[0].text, d.mail.text);
    let mut o2 = o2;
    assert_eq!(o2.create_run("next", None, None, 31).unwrap().id, "r2");
}

#[test]
fn fences_token_and_generation() {
    let mut t = setup("fence");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.add_item("r1", None, "spec", &[], None, 1).unwrap();
    begin(o, "r1", "i1", "old", None).unwrap();
    assert_eq!(
        begin(o, "r1", "i1", "k", None).unwrap_err().kind,
        ErrorKind::InvalidState
    );
    let a2 = begin(o, "r1", "i1", "new", Some("a1")).unwrap();
    assert_eq!(a2.generation, 2);
    assert_eq!(o.attempt("a1").unwrap().state, AttemptState::Abandoned);
    // Wrong token: refused outright.
    let e = o
        .done("a1", Some("new"), true, "", &[], None, None, 5)
        .unwrap_err();
    assert_eq!(e.kind, ErrorKind::AttemptFenced);
    assert_eq!(
        o.done("a1", None, true, "", &[], None, None, 5)
            .unwrap_err()
            .kind,
        ErrorKind::AttemptFenced
    );
    // Late done of the old generation: recorded, item untouched.
    let late = o
        .done("a1", Some("old"), true, "late", &[], None, None, 5)
        .unwrap();
    assert!(!late.accepted);
    assert!(late.mail.fenced);
    assert_eq!(
        late.mail.text.as_ref().map(|t| t.kind.as_str()),
        Some(text::DONE_LATE)
    );
    assert_eq!(
        late.mail.subject,
        "late done (succeeded) from superseded attempt a1; ignored"
    );
    assert_eq!(o.item("i1").unwrap().state, ItemState::Assigned);
    assert_eq!(o.item("i1").unwrap().attempt.as_deref(), Some("a2"));
    let ok = o
        .done("a2", Some("new"), false, "nope", &[], None, None, 6)
        .unwrap();
    assert!(ok.accepted);
    assert_eq!(o.item("i1").unwrap().state, ItemState::Failed);
    assert_eq!(
        o.done("a2", Some("new"), true, "again", &[], None, None, 7)
            .unwrap_err()
            .kind,
        ErrorKind::InvalidState
    );
    // A worker-reported failure is not a start failure.
    assert_eq!(o.item("i1").unwrap().start_failures, 0);
}

#[test]
fn breaker_opens_after_three_start_failures() {
    let mut t = setup("breaker");
    let o = &mut t.o;
    o.create_run("x", None, None, 1).unwrap();
    o.add_item("r1", None, "spec", &[], None, 1).unwrap();
    for n in 1..=3 {
        let a = begin(o, "r1", "i1", "k", None).unwrap();
        let tripped = o.start_failed(&a.id, "no such program", 2).unwrap();
        assert_eq!(tripped, n == 3);
    }
    let it = o.item("i1").unwrap();
    assert_eq!(
        (it.state, it.start_failures, it.circuit_open),
        (ItemState::Failed, 3, true)
    );
    assert_eq!(
        begin(o, "r1", "i1", "k", None).unwrap_err().kind,
        ErrorKind::CircuitOpen
    );
    assert_eq!(
        begin(o, "r1", "i1", "k", Some("a3")).unwrap_err().kind,
        ErrorKind::CircuitOpen
    );
    let it = o.reopen_item("i1", 3).unwrap();
    assert_eq!(
        (it.state, it.start_failures, it.circuit_open),
        (ItemState::Ready, 0, false)
    );
    let a = begin(o, "r1", "i1", "k", None).unwrap();
    o.start_failed(&a.id, "again", 4).unwrap();
    let a = begin(o, "r1", "i1", "k", None).unwrap();
    assert!(o.mark_ready(&a.id, 5).unwrap());
    assert_eq!(
        o.item("i1").unwrap().start_failures,
        0,
        "a good start resets the count"
    );
}

#[test]
fn depth_and_worker_caps() {
    let mut t = setup("caps");
    let o = &mut t.o;
    o.limits = Limits {
        max_depth: 2,
        run_worker_cap: 2,
        start_strikes: 3,
    };
    o.create_run("top", Some("t1"), None, 1).unwrap();
    for _ in 0..3 {
        o.add_item("r1", None, "s", &[], None, 1).unwrap();
    }
    begin(o, "r1", "i1", "k", None).unwrap();
    begin(o, "r1", "i2", "k", None).unwrap();
    // Over the run's worker limit: queued, not refused (docs/19 §5.1).
    let a3 = begin(o, "r1", "i3", "k", None).unwrap();
    o.enqueue(&a3.id, serde_json::json!({"x": 1}), None, 1)
        .unwrap();
    let lim = ResourceLimits {
        run_workers: 2,
        cpu_max_pct: None,
        min_free_mem: None,
        min_free_disk: None,
        ..Default::default()
    };
    let ad = o
        .admit(&lim, &Sample::default(), |_| None, Some(hash_token("q")), 2)
        .unwrap();
    assert!(ad.start.is_none());
    assert_eq!(
        ad.changed[0].queue.as_ref().unwrap().kind,
        WaitKind::RunWorkers
    );
    // A retry replaces a slot instead of taking one.
    begin(o, "r1", "i2", "k2", Some("a2")).unwrap();
    // Worker a1 (depth 1) coordinates r2 (depth 1): its workers are depth 2.
    let r2 = o.create_run("sub", Some("t9"), Some("a1"), 2).unwrap();
    assert_eq!(r2.depth, 1);
    o.add_item("r2", None, "s", &[], None, 2).unwrap();
    let deep = begin(o, "r2", "i4", "k", None).unwrap();
    assert_eq!(deep.depth, 2);
    let r3 = o
        .create_run("subsub", Some("t10"), Some(&deep.id), 3)
        .unwrap();
    o.add_item(&r3.id, None, "s", &[], None, 3).unwrap();
    let e = begin(o, &r3.id, "i5", "k", None).unwrap_err();
    assert_eq!(e.kind, ErrorKind::DepthExceeded);
    assert!(e.message.contains("yourself"), "{}", e.message);
}

#[test]
fn mail_of_lists_a_run_without_acking() {
    let mut t = setup("mail-of");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.create_run("y", Some("t2"), None, 1).unwrap();
    o.add_item("r1", None, "s", &[], None, 1).unwrap();
    begin(o, "r1", "i1", "k1", None).unwrap();
    o.attach_worker("a1", Some("demo/w1"), "t5", None, 0)
        .unwrap();
    let q = o
        .send(
            None,
            NewMail::new("attempt:a1", "run:r1", MailType::Question, "which db?"),
            |_| false,
            2,
        )
        .unwrap();
    o.send(
        None,
        NewMail::new("run:r2", "run:r2", MailType::Status, "other run"),
        |_| false,
        2,
    )
    .unwrap();
    let all = o.mail_of("r1").unwrap();
    assert_eq!(
        all.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        [q.id.as_str()]
    );
    assert_eq!(all[0].read_ms, None);
    assert_eq!(o.unread("run:r1").len(), 1, "listing marks nothing read");
    assert!(o.wants_wake("run:r1"), "and leaves the wake-up alone");
    o.check(&["run:r1".into()], true, 9).unwrap();
    assert_eq!(o.mail_of("r1").unwrap()[0].read_ms, Some(9));
    assert_eq!(o.mail_of("r9").unwrap_err().kind, ErrorKind::RunNotFound);
}

#[test]
fn queue_admits_in_fifo_order_and_survives_reload() {
    let mut t = setup("queue");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    for _ in 0..3 {
        o.add_item("r1", None, "s", &[], None, 1).unwrap();
    }
    let lim = ResourceLimits {
        max_workers: 1,
        cpu_max_pct: None,
        min_free_mem: None,
        min_free_disk: None,
        ..Default::default()
    };
    // a1 runs; a2 and a3 queue behind it.
    begin(o, "r1", "i1", "k", None).unwrap();
    for (n, item) in [(2, "i2"), (3, "i3")] {
        let a = begin(o, "r1", item, "unused", None).unwrap();
        o.enqueue(
            &a.id,
            serde_json::json!({ "n": n }),
            Some(r"C:\wt".into()),
            10 + n,
        )
        .unwrap();
    }
    assert_eq!(o.active_workers(), 1);
    let ad = o
        .admit(
            &lim,
            &Sample::default(),
            |_| None,
            Some(hash_token("t2")),
            20,
        )
        .unwrap();
    assert!(ad.start.is_none());
    assert_eq!(ad.changed.len(), 2);
    let q = o.attempt("a2").unwrap().queue.clone().unwrap();
    assert_eq!((q.kind, q.position), (WaitKind::Workers, 1));
    // The same verdicts again: nothing changes, nothing is written.
    let ad = o
        .admit(
            &lim,
            &Sample::default(),
            |_| None,
            Some(hash_token("t2")),
            21,
        )
        .unwrap();
    assert!(ad.changed.is_empty() && ad.start.is_none());
    // Restart while queued: the order and reasons are kept.
    let mut o2 = Orch::load(&t.dir, Limits::default(), 30);
    let ids: Vec<&str> = o2.queue().iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["a2", "a3"]);
    assert_eq!(
        o2.attempt("a2").unwrap().launch,
        Some(serde_json::json!({"n": 2}))
    );
    assert!(
        o2.attempt("a2").unwrap().public().launch.is_none(),
        "launch data stays off the wire"
    );
    // a1 ends: a2 (the oldest) is taken out with its new token hash.
    o2.end_attempt("a1", AttemptState::Stopped, "stop", None, None, 31)
        .unwrap();
    let ad = o2
        .admit(
            &lim,
            &Sample::default(),
            |_| None,
            Some(hash_token("t2")),
            32,
        )
        .unwrap();
    let (a, launch) = ad.start.unwrap();
    assert_eq!((a.id.as_str(), launch), ("a2", serde_json::json!({"n": 2})));
    assert!(
        a.queue.is_none() && a.token_hash.is_empty(),
        "public record"
    );
    assert!(o2.verify_token("a2", Some("t2")).is_ok());
    assert_eq!(
        o2.attempt("a3").unwrap().queue.as_ref().unwrap().kind,
        WaitKind::Behind
    );
    assert_eq!(o2.active_workers(), 1);
}

#[test]
fn gates_block_items_until_decided() {
    let mut t = setup("gates");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.add_item("r1", None, "s", &[], None, 1).unwrap();
    o.add_item("r1", None, "s", &[], None, 1).unwrap();
    // A ready item blocked, then free again.
    assert_eq!(
        o.block_item("i2", "g1", 2).unwrap().state,
        ItemState::Blocked
    );
    assert_eq!(
        begin(o, "r1", "i2", "k", None).unwrap_err().kind,
        ErrorKind::InvalidState
    );
    assert_eq!(
        o.reopen_item("i2", 3).unwrap().state,
        ItemState::Blocked,
        "reopen keeps the block"
    );
    assert_eq!(
        o.unblock_item("i2", "g1", 4).unwrap().unwrap().state,
        ItemState::Ready
    );
    assert_eq!(o.unblock_item("i2", "g1", 5).unwrap(), None, "idempotent");
    // An assigned item: two gates need both decided.
    begin(o, "r1", "i1", "tok", None).unwrap();
    o.block_item("i1", "g2", 6).unwrap();
    o.block_item("i1", "g3", 6).unwrap();
    assert_eq!(
        o.unblock_item("i1", "g2", 7).unwrap().unwrap().state,
        ItemState::Blocked
    );
    // Ending the attempt does not clear the block.
    o.end_attempt("a1", AttemptState::Stopped, "x", None, None, 8)
        .unwrap();
    assert_eq!(o.item("i1").unwrap().state, ItemState::Blocked);
    let it = o.unblock_item("i1", "g3", 9).unwrap().unwrap();
    assert_eq!(
        (it.state, it.gates.len()),
        (ItemState::Ready, 0),
        "no open attempt any more"
    );
    let a2 = begin(o, "r1", "i1", "t2", None).unwrap();
    o.block_item("i1", "g4", 10).unwrap();
    assert_eq!(
        o.unblock_item("i1", "g4", 11).unwrap().unwrap().state,
        ItemState::Assigned
    );
    o.done(&a2.id, Some("t2"), true, "ok", &[], None, None, 12)
        .unwrap();
    assert_eq!(
        o.block_item("i1", "g5", 13).unwrap_err().kind,
        ErrorKind::InvalidState
    );
    // Survives a reload.
    o.block_item("i2", "g6", 14).unwrap();
    let o2 = Orch::load(&t.dir, Limits::default(), 15);
    assert_eq!(o2.item("i2").unwrap().gates, ["g6"]);
}

#[test]
fn mail_check_ack_wake_and_reopen() {
    let mut t = setup("mail");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.add_item("r1", None, "s", &[], None, 1).unwrap();
    o.add_item("r1", None, "s2", &[], None, 1).unwrap();
    begin(o, "r1", "i1", "k1", None).unwrap();
    o.attach_worker("a1", Some("demo/w1"), "t5", None, 0)
        .unwrap();
    let mut a2 = begin(o, "r1", "i2", "k2", None).unwrap();
    a2.agent = "claude".into();
    o.update_attempt("a2", |a| a.agent = "claude".into())
        .unwrap();
    o.attach_worker("a2", Some("demo/w2"), "t6", None, 0)
        .unwrap();
    assert_eq!(o.terminal_mailboxes("t1"), vec!["run:r1"]);
    assert_eq!(o.terminal_mailboxes("t5"), vec!["attempt:a1"]);
    assert_eq!(o.mailbox_terminal("run:r1").as_deref(), Some("t1"));

    let q = o
        .send(
            None,
            NewMail::new("attempt:a1", "run:r1", MailType::Question, "which db?"),
            |_| false,
            2,
        )
        .unwrap();
    assert!(o.has_open_question("a1"));
    assert!(o.wants_wake("run:r1"));
    o.mark_notified(&["run:r1".into()], 3).unwrap();
    assert!(!o.wants_wake("run:r1"), "one notification per mailbox");
    o.send(
        None,
        NewMail::new("attempt:a2", "run:r1", MailType::Status, "fyi"),
        |_| false,
        3,
    )
    .unwrap();
    assert!(!o.wants_wake("run:r1"), "still only one");
    let mut reply =
        NewMail::new("run:r1", "attempt:a1", MailType::Answer, "re: which db?").body("sqlite");
    reply.reply_to = Some(q.id.clone());
    let ans = o.send(None, reply, |_| false, 4).unwrap();
    assert_eq!(ans.thread.as_deref(), Some(q.id.as_str()));
    assert_eq!(o.answer_to(&q.id).unwrap().id, ans.id);
    assert!(!o.has_open_question("a1"));
    let got = o.check(&["run:r1".into()], true, 5).unwrap();
    assert_eq!(got.len(), 2);
    assert!(got.iter().all(|m| m.read_ms == Some(5)));
    assert!(o.unread("run:r1").is_empty());
    assert_eq!(o.notified("run:r1"), None, "check clears the notification");
    // Groups expand to live workers; done cannot go to a group.
    let g = o
        .send(
            Some("r1"),
            NewMail::new("run:r1", "@claude", MailType::Status, "hi"),
            |_| false,
            6,
        )
        .unwrap();
    assert_eq!(g.recipients, vec!["attempt:a2".to_string()]);
    let all = o
        .send(
            Some("r1"),
            NewMail::new("run:r1", "@idle", MailType::Status, "hi"),
            |a| a.id == "a1",
            6,
        )
        .unwrap();
    assert_eq!(all.recipients, vec!["attempt:a1".to_string()]);
    assert_eq!(
        o.send(
            Some("r1"),
            NewMail::new("attempt:a1", "@all", MailType::Done, "x"),
            |_| true,
            6
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidParams
    );
    assert_eq!(
        o.send(
            None,
            NewMail::new("run:r1", "@all", MailType::Status, "x"),
            |_| true,
            6
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidParams
    );
    // Heartbeats never wake.
    o.check(&["run:r1".into()], true, 7).unwrap();
    o.send(
        None,
        NewMail::new("attempt:a1", "run:r1", MailType::Heartbeat, "50%"),
        |_| false,
        7,
    )
    .unwrap();
    assert!(!o.wants_wake("run:r1"));
    // Mail to a finished worker re-opens it.
    o.done("a1", Some("k1"), true, "ok", &[], None, Some(1000), 8)
        .unwrap();
    o.send(
        None,
        NewMail::new("run:r1", "attempt:a1", MailType::Status, "one more thing"),
        |_| false,
        9,
    )
    .unwrap();
    let a1 = o.attempt("a1").unwrap();
    assert_eq!(
        (a1.state, a1.worker, a1.reopens, a1.release_at_ms),
        (AttemptState::Ready, WorkerState::Active, 1, None)
    );
    assert_eq!(o.item("i1").unwrap().state, ItemState::Assigned);
    // ...but not once released.
    o.done("a1", Some("k1"), true, "ok", &[], None, Some(0), 10)
        .unwrap();
    assert_eq!(o.due_releases(10), vec!["a1".to_string()]);
    o.set_worker("a1", WorkerState::Released, Some("a1.log"), 11)
        .unwrap();
    assert_eq!(
        o.send(
            None,
            NewMail::new("run:r1", "attempt:a1", MailType::Status, "?"),
            |_| false,
            12
        )
        .unwrap_err()
        .kind,
        ErrorKind::InvalidState
    );
    assert_eq!(o.mailbox_terminal("attempt:a1"), None);
    // Read receipts and wake marks survive a reload.
    o.mark_notified(&["attempt:a2".into()], 13).unwrap();
    let o2 = Orch::load(&t.dir, Limits::default(), 14);
    assert!(
        o2.unread("run:r1")
            .iter()
            .all(|m| m.kind == MailType::Heartbeat || m.kind == MailType::Done)
    );
    assert_eq!(o2.notified("attempt:a2"), Some(13));
}

#[test]
fn end_attempt_requests_and_titles() {
    let mut t = setup("end");
    let o = &mut t.o;
    o.create_run("x", None, None, 1).unwrap();
    o.add_item("r1", None, "s", &[], None, 1).unwrap();
    begin(o, "r1", "i1", "k", None).unwrap();
    assert!(
        o.end_attempt(
            "a1",
            AttemptState::Stopped,
            "stopped by run:r1",
            None,
            Some(50),
            2
        )
        .unwrap()
    );
    let a = o.attempt("a1").unwrap();
    assert_eq!(
        (a.state, a.worker, a.release_at_ms),
        (AttemptState::Stopped, WorkerState::Finished, Some(52))
    );
    assert_eq!(o.item("i1").unwrap().state, ItemState::Ready);
    assert!(
        !o.end_attempt("a1", AttemptState::Lost, "x", None, None, 3)
            .unwrap()
    );
    // request ids
    assert_eq!(o.request("q1", "orch.run.create").unwrap(), None);
    o.remember("q1", "orch.run.create", serde_json::json!({"id": "r1"}), 4)
        .unwrap();
    assert_eq!(
        o.request("q1", "orch.run.create").unwrap().unwrap()["id"],
        "r1"
    );
    assert_eq!(
        o.request("q1", "orch.done").unwrap_err().kind,
        ErrorKind::InvalidParams
    );
    for n in 0..(MAX_REQUESTS + 5) {
        o.remember(&format!("x{n}"), "m", Value::Null, 5).unwrap();
    }
    assert_eq!(
        o.request("q1", "orch.run.create").unwrap(),
        None,
        "oldest dropped"
    );
    let o2 = Orch::load(&t.dir, Limits::default(), 6);
    assert!(
        o2.request(&format!("x{}", MAX_REQUESTS + 4), "m")
            .unwrap()
            .is_some()
    );
    assert_eq!(spec_title("\n\n  ## Fix the bug  \nbody"), "Fix the bug");
    assert_eq!(spec_title(&"y".repeat(100)).chars().count(), 81);
    assert_eq!(hash_token("abc").len(), 64);
}

#[test]
fn torn_mail_line_recovered_on_load() {
    let mut t = setup("torn");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.send(
        None,
        NewMail::new("human", "run:r1", MailType::Status, "one"),
        |_| false,
        2,
    )
    .unwrap();
    o.send(
        None,
        NewMail::new("human", "run:r1", MailType::Status, "two"),
        |_| false,
        3,
    )
    .unwrap();
    let p = store::mail_jsonl(&t.dir, "r1");
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
    f.write_all(br#"{"kind":"mail","mail":{"id":"m3","#)
        .unwrap();
    drop(f);
    let mut o2 = Orch::load(&t.dir, Limits::default(), 4);
    assert_eq!(o2.unread("run:r1").len(), 2);
    assert!(
        o2.notes.iter().any(|n| n.contains("torn")),
        "{:?}",
        o2.notes
    );
    let m = o2
        .send(
            None,
            NewMail::new("human", "run:r1", MailType::Status, "three"),
            |_| false,
            5,
        )
        .unwrap();
    assert_eq!(m.id, "m3");
    let o3 = Orch::load(&t.dir, Limits::default(), 6);
    assert!(o3.notes.is_empty(), "{:?}", o3.notes);
    assert_eq!(o3.unread("run:r1").len(), 3);
}

/// Opens `path` with no sharing: other writes and renames onto it fail
/// until the handle drops (an injected disk failure).
#[cfg(windows)]
fn lock(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(path)
        .unwrap()
}

fn done_mails(o: &Orch, run: &str) -> Vec<Mail> {
    o.mail_of(run)
        .unwrap()
        .into_iter()
        .filter(|m| m.kind == MailType::Done)
        .collect()
}

/// orch-01: the done mail cannot be written. Nothing changes (the
/// item stays assigned, no dependent item starts), on disk or in
/// memory, and the same report succeeds once the disk is back.
#[cfg(windows)]
#[test]
fn done_with_a_failing_mail_write_changes_nothing() {
    let mut t = setup("done-mailfail");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.add_item("r1", None, "a", &[], None, 1).unwrap();
    o.add_item("r1", None, "b", &["i1".into()], None, 1)
        .unwrap();
    begin(o, "r1", "i1", "k1", None).unwrap();
    let held = lock(&store::mail_jsonl(&t.dir, "r1"));
    o.done("a1", Some("k1"), true, "ok", &[], None, None, 5)
        .unwrap_err();
    assert!(o.attempt("a1").unwrap().state.is_open());
    assert_eq!(
        (o.item("i1").unwrap().state, o.item("i2").unwrap().state),
        (ItemState::Assigned, ItemState::Pending)
    );
    drop(held);
    let disk = Orch::load(&t.dir, Limits::default(), 6);
    assert!(
        disk.attempt("a1").unwrap().state.is_open(),
        "nothing reached the disk"
    );
    let d = o
        .done("a1", Some("k1"), true, "ok", &[], None, None, 7)
        .unwrap();
    assert!(d.accepted && !d.repeated);
    assert_eq!(o.item("i2").unwrap().state, ItemState::Ready);
    assert_eq!(done_mails(o, "r1").len(), 1);
}

/// orch-01: the mail is written but `run.json` is not (write failure or
/// a crash in between). A reload applies the recorded done; the same
/// report again returns the recorded result without a second mail; a
/// different report is still refused.
#[cfg(windows)]
#[test]
fn done_recorded_in_mail_survives_a_failed_run_write() {
    let mut t = setup("done-runfail");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.add_item("r1", None, "a", &[], None, 1).unwrap();
    o.add_item("r1", None, "b", &["i1".into()], None, 1)
        .unwrap();
    begin(o, "r1", "i1", "k1", None).unwrap();
    o.attach_worker("a1", Some("demo/w1"), "t5", None, 0)
        .unwrap();
    let held = lock(&store::run_json(&t.dir, "r1"));
    let files = vec!["src/a.rs".to_string()];
    o.done(
        "a1",
        Some("k1"),
        true,
        "did it",
        &files,
        None,
        Some(1000),
        5,
    )
    .unwrap_err();
    drop(held);
    assert_eq!(done_mails(o, "r1").len(), 1, "the journal line is there");

    // A restart at this point: the record still says assigned.
    let mut o2 = Orch::load(&t.dir, Limits::default(), 6);
    assert!(
        o2.notes
            .iter()
            .any(|n| n.contains("applied the recorded done of a1")),
        "{:?}",
        o2.notes
    );
    let a1 = o2.attempt("a1").unwrap().clone();
    assert_eq!(
        (a1.state, a1.worker, a1.release_at_ms, a1.files.clone()),
        (
            AttemptState::Succeeded,
            WorkerState::Finished,
            Some(1005),
            files.clone()
        )
    );
    assert_eq!(
        (o2.item("i1").unwrap().state, o2.item("i2").unwrap().state),
        (ItemState::Completed, ItemState::Ready)
    );
    // ...and it was saved: the next load has nothing to replay.
    assert!(Orch::load(&t.dir, Limits::default(), 7).notes.is_empty());

    // The worker retries the same report: same mail, nothing new.
    let again = o2
        .done(
            "a1",
            Some("k1"),
            true,
            "did it",
            &files,
            None,
            Some(1000),
            8,
        )
        .unwrap();
    assert!(again.accepted && again.repeated);
    assert_eq!(again.mail.id, done_mails(&o2, "r1")[0].id);
    assert_eq!(done_mails(&o2, "r1").len(), 1);
    let other = o2
        .done(
            "a1",
            Some("k1"),
            false,
            "changed my mind",
            &[],
            None,
            None,
            9,
        )
        .unwrap_err();
    assert_eq!(other.kind, ErrorKind::InvalidState);
    // The original process (memory already moved on) also answers the retry.
    let again = o
        .done(
            "a1",
            Some("k1"),
            true,
            "did it",
            &files,
            None,
            Some(1000),
            10,
        )
        .unwrap();
    assert!(again.repeated);
}

/// orch-02: a wake is scheduled, then delivered or failed. Only a
/// delivered one is persisted; a scheduled one that never settled (a
/// restart in between) wants a wake again after the reload; an
/// unconfirmed one is not typed twice until the mailbox is checked.
#[test]
fn wake_scheduled_delivered_failed_and_restart() {
    let mut t = setup("wake");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.add_item("r1", None, "a", &[], None, 1).unwrap();
    begin(o, "r1", "i1", "k1", None).unwrap();
    let q = || NewMail::new("attempt:a1", "run:r1", MailType::Question, "which db?");
    o.send(None, q(), |_| false, 2).unwrap();
    let rb = vec!["run:r1".to_string()];
    assert!(o.wants_wake("run:r1"));

    // Scheduled: no second wake meanwhile, nothing on disk.
    o.wake_scheduled(&rb, 3).unwrap();
    assert!(!o.wants_wake("run:r1") && o.wake_pending("run:r1"));
    assert_eq!(o.notified("run:r1"), None);
    o.update_attempt("a1", |a| a.seen_turn = 1).unwrap(); // any other save
    let mut o2 = Orch::load(&t.dir, Limits::default(), 4);
    assert!(
        o2.wants_wake("run:r1"),
        "a restart forgets the unsettled wake and tries again"
    );

    // Not typed (holder gone): wants a wake again right away.
    o2.wake_scheduled(&rb, 5).unwrap();
    o2.wake_failed(&rb, false).unwrap();
    assert!(o2.wants_wake("run:r1"));
    // Typed but not taken: not typed a second time...
    o2.wake_scheduled(&rb, 6).unwrap();
    o2.wake_failed(&rb, true).unwrap();
    assert!(!o2.wants_wake("run:r1"));
    assert_eq!(
        (o2.wake_unconfirmed("run:r1"), o2.notified("run:r1")),
        (1, None)
    );
    // ...until a check, or a restart.
    assert!(Orch::load(&t.dir, Limits::default(), 7).wants_wake("run:r1"));
    o2.check(&rb, true, 8).unwrap();
    o2.send(None, q(), |_| false, 9).unwrap();
    assert!(o2.wants_wake("run:r1"));

    // Delivered: persisted, no further wake even after a restart.
    o2.wake_scheduled(&rb, 10).unwrap();
    o2.wake_delivered(&rb, 11).unwrap();
    assert_eq!(o2.notified("run:r1"), Some(11));
    assert!(!o2.wants_wake("run:r1"));
    let o3 = Orch::load(&t.dir, Limits::default(), 12);
    assert_eq!(o3.notified("run:r1"), Some(11));
    assert!(!o3.wants_wake("run:r1"));
}

/// A done that was followed by a reopen (mail to the finished worker)
/// is not replayed over the newer opening.
#[test]
fn replay_skips_a_reopened_attempt() {
    let mut t = setup("done-reopen");
    let o = &mut t.o;
    o.create_run("x", Some("t1"), None, 1).unwrap();
    o.add_item("r1", None, "a", &[], None, 1).unwrap();
    begin(o, "r1", "i1", "k1", None).unwrap();
    o.attach_worker("a1", Some("demo/w1"), "t5", None, 0)
        .unwrap();
    o.done("a1", Some("k1"), true, "ok", &[], None, None, 5)
        .unwrap();
    o.send(
        None,
        NewMail::new("run:r1", "attempt:a1", MailType::Status, "one more"),
        |_| false,
        6,
    )
    .unwrap();
    assert!(o.attempt("a1").unwrap().state.is_open());
    let o2 = Orch::load(&t.dir, Limits::default(), 7);
    assert!(o2.notes.is_empty(), "{:?}", o2.notes);
    assert!(o2.attempt("a1").unwrap().state.is_open());
    // The repeat check also only looks at the current opening.
    let mut o2 = o2;
    o2.done("a1", Some("k1"), true, "ok", &[], None, None, 8)
        .unwrap();
    let again = o2
        .done("a1", Some("k1"), true, "ok", &[], None, None, 9)
        .unwrap();
    assert!(again.repeated);
    assert_eq!(done_mails(&o2, "r1").len(), 2);
}
